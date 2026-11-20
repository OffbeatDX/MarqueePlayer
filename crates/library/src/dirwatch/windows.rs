//! Windows: `ReadDirectoryChangesW` on the folder, overlapped, one read outstanding at a time.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

use super::Seen;

/// Names (created, removed, renamed) and contents (written, resized).
const FILTER: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_LAST_WRITE
    | FILE_NOTIFY_CHANGE_SIZE;
/// The most a local volume returns per call; DWORD-aligned as the call requires.
const BUF_WORDS: usize = 64 * 1024 / 4;

/// The folder handle and the read's event, used by the watch thread and closed after
/// it has stopped: when the last of `Inner` and the thread lets go of them. Also when the
/// thread could not be started, or ended before saying it was watching.
struct Dir(HANDLE, HANDLE);
// The handle is used by one thread at a time (the watch thread, then `stop`).
unsafe impl Send for Dir {}
unsafe impl Sync for Dir {}

impl Drop for Dir {
    fn drop(&mut self) {
        // SAFETY: both opened by `start` and owned here alone; nothing uses them now (the
        // thread holds a share of this until it ends), so each is closed once.
        unsafe {
            CloseHandle(self.0);
            CloseHandle(self.1);
        }
    }
}

pub struct Inner {
    dir: Arc<Dir>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

pub fn start(path: &Path, recursive: bool, mut on: Box<dyn FnMut(Seen) + Send>) -> std::io::Result<Inner> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the handle is closed by `Dir` (or below).
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: an unnamed manual-reset event, closed by `Dir`.
    let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if ev.is_null() {
        let e = std::io::Error::last_os_error();
        // SAFETY: opened above, not shared yet.
        unsafe { CloseHandle(h) };
        return Err(e);
    }
    let dir = Arc::new(Dir(h, ev));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::io::Result<()>>();
    let stop = Arc::new(AtomicBool::new(false));
    let root = path.to_path_buf();
    let (d, s) = (dir.clone(), stop.clone());
    let name = format!("mp-dirwatch-{}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
    let thread = std::thread::Builder::new().name(name).spawn(move || {
        let mut buf = vec![0u32; BUF_WORDS];
        let mut ready = Some(ready_tx);
        loop {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = d.1;
            // SAFETY: a valid directory handle; `buf` and `ov` outlive the read, which
            // is waited for (or cancelled and then waited for) below.
            let issued = unsafe {
                ReadDirectoryChangesW(
                    d.0,
                    buf.as_mut_ptr().cast(),
                    (buf.len() * 4) as u32,
                    i32::from(recursive),
                    FILTER,
                    std::ptr::null_mut(),
                    &mut ov,
                    None,
                )
            };
            if issued == 0 {
                let e = std::io::Error::last_os_error();
                match ready.take() {
                    Some(r) => {
                        let _ = r.send(Err(e));
                    }
                    None => {
                        log::warn!("watching {}: {e}; stopped", root.display());
                        on(Seen::Ended);
                    }
                }
                return;
            }
            // The system now records changes for this handle: `watch_dir` may return.
            if let Some(r) = ready.take() {
                let _ = r.send(Ok(()));
            }
            let mut n = 0u32;
            // SAFETY: waits (no timeout) for the read issued above to finish.
            let ok = unsafe { GetOverlappedResult(d.0, &ov, &mut n, 1) };
            if s.load(Ordering::Acquire) {
                return;
            }
            if ok == 0 {
                // SAFETY: reads the calling thread's last error.
                match unsafe { GetLastError() } {
                    ERROR_OPERATION_ABORTED => return,
                    ERROR_NOTIFY_ENUM_DIR => on(Seen::Lost),
                    e => {
                        // The folder went away, or the volume cannot be watched.
                        log::warn!("watching {}: error {e}; stopped", root.display());
                        on(Seen::Ended);
                        return;
                    }
                }
                continue;
            }
            if n == 0 {
                // The list did not fit; or, on some network shares, the folder is no longer
                // there (then every read would end at once like this: stop instead).
                if !root.is_dir() {
                    log::warn!("watching {}: the folder is gone; stopped", root.display());
                    on(Seen::Ended);
                    return;
                }
                on(Seen::Lost);
                continue;
            }
            for p in parse(&root, &buf, n as usize) {
                on(Seen::Path(p));
            }
        }
    })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Inner { dir, stop, thread }),
        // The handles close with `dir` once the thread has let go of its share.
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(std::io::Error::other("the watch thread ended"))
        }
    }
}

/// The paths in a `FILE_NOTIFY_INFORMATION` list of `len` bytes.
fn parse(root: &Path, buf: &[u32], len: usize) -> Vec<PathBuf> {
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), len.min(buf.len() * 4)) };
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 12 <= bytes.len() {
        let word = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let (next, name_len) = (word(at), word(at + 8));
        let start = at + 12;
        let end = (start + name_len).min(bytes.len());
        let name: Vec<u16> = bytes[start..end].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        out.push(root.join(OsString::from_wide(&name)));
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

impl Inner {
    pub fn stop(self) {
        self.stop.store(true, Ordering::Release);
        // The thread is (or soon will be) waiting on its read: cancel it until the
        // thread has gone. Between two reads a cancel finds nothing, so retry.
        while !self.thread.is_finished() {
            // SAFETY: cancels this handle's pending reads, from any thread.
            unsafe { CancelIoEx(self.dir.0, std::ptr::null()) };
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.thread.join();
        // The handles close with `self.dir`: the thread's share went with it.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::OpenEventW;

    /// The handles close when the last share of them goes, as when the watch thread could
    /// not be started (its closure, holding one, dropped unrun). Seen from outside: the
    /// folder, opened without delete sharing, can be removed again, and the named event is
    /// gone.
    #[test]
    fn handles_close_with_their_last_owner() {
        let dir = crate::testdir::TestDir::new("dirwatch-handles");
        let folder = dir.join("watched");
        std::fs::create_dir(&folder).unwrap();
        let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated path; closed by `Dir` below.
        let h = unsafe { CreateFileW(wide.as_ptr(), FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut()) };
        assert!(h != INVALID_HANDLE_VALUE && !h.is_null());
        let name: Vec<u16> = format!("mp-dirwatch-test-{}", std::process::id()).encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated name; closed by `Dir` below.
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
        assert!(!ev.is_null());
        let event_open = || {
            // SAFETY: a NUL-terminated name; a handle found is closed at once.
            let o = unsafe { OpenEventW(0x0010_0000, 0, name.as_ptr()) }; // SYNCHRONIZE
            if !o.is_null() {
                // SAFETY: just opened, closed once.
                unsafe { CloseHandle(o) };
            }
            !o.is_null()
        };
        let shared = Arc::new(Dir(h, ev));
        let theirs = shared.clone();
        let never_run = move || drop(theirs);
        assert!(std::fs::remove_dir(&folder).is_err(), "open, without delete sharing");
        assert!(event_open());
        drop(shared);
        assert!(event_open(), "the closure still holds them");
        drop(never_run);
        // Something else (seen with the whole suite running, other tests starting child
        // processes) can hold the event a moment longer: wait for it, not forever.
        let since = std::time::Instant::now();
        while event_open() && since.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!event_open(), "the event is closed");
        std::fs::remove_dir(&folder).expect("the folder handle is closed");
    }
}
