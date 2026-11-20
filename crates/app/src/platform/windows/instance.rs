//! One running app per data folder. A second launch asks the first to show and focus
//! its window (it may be hidden in the tray), then exits, instead of starting a second
//! engine, scanner, folder watcher and tray icon on the same database.
//!
//! Keyed by the data folder (`Paths::config`): an instance started with another
//! `MP_DATA_DIR` (a test or bench run) has its own key, so it neither signals nor is
//! stopped by an instance on another folder, the real one included.
//!
//! - A named mutex (`Local\MarqueePlayer-<key>`) says whether an instance runs.
//! - The running instance marks its window with a property named after the key, so
//!   the second one finds that window (hidden or not) with `EnumWindows`.
//! - The second one lets it take the foreground (`AllowSetForegroundWindow`: a process
//!   the user just started may pass that on) and posts it a registered message; the
//!   window procedure (`winmsg`) hands it to `on_show`.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// The key for a data folder: FNV-1a of its path, lowercased (Windows paths are case
/// insensitive), with separators made uniform and no trailing separator.
pub fn key(dir: &Path) -> String {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let text = dir.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The message a second launch posts to the running window (0 until registered).
static SHOW_MSG: AtomicU32 = AtomicU32::new(0);

pub fn show_message() -> u32 {
    SHOW_MSG.load(Ordering::Relaxed)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn prop_name(key: &str) -> String {
    format!("MarqueePlayer.Instance.{key}")
}

/// What `acquire` found.
pub enum Instance {
    /// This is the only instance for the folder. Keep it alive for the whole run.
    First(Guard),
    /// Another instance runs on this folder; it was asked to show its window (or could
    /// not be reached: `false`). Exit.
    Second { signalled: bool },
}

/// Holds the mutex (and the window property's name) while the app runs.
pub struct Guard {
    _mutex: windows_sys::Win32::Foundation::HANDLE,
    prop: Vec<u16>,
}

pub fn acquire(dir: &Path, paths: &[std::path::PathBuf]) -> Instance {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    let key = key(dir);
    let prop = wide(&prop_name(&key));
    // SAFETY: plain calls with NUL-terminated strings that outlive them.
    unsafe {
        SHOW_MSG.store(RegisterWindowMessageW(wide(&format!("MarqueePlayer.Show.{key}")).as_ptr()), Ordering::Relaxed);
        let mutex = CreateMutexW(std::ptr::null(), 0, wide(&format!("Local\\MarqueePlayer-{key}")).as_ptr());
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        if mutex.is_null() {
            // Cannot tell: run, as before the guard existed.
            log::warn!("single instance: CreateMutexW failed");
            return Instance::First(Guard { _mutex: mutex, prop });
        }
        if !existed {
            return Instance::First(Guard { _mutex: mutex, prop });
        }
        windows_sys::Win32::Foundation::CloseHandle(mutex);
    }
    // The first instance may still be starting (its window not there yet): wait a little.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(hwnd) = find_window(&prop) {
            if !paths.is_empty() {
                send_paths(hwnd, paths);
            }
            return Instance::Second { signalled: signal(hwnd) };
        }
        if std::time::Instant::now() >= deadline {
            return Instance::Second { signalled: false };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Hand audio paths to the running instance (`WM_COPYDATA`; it waits for the window to
/// take them, at most a few seconds).
fn send_paths(hwnd: windows_sys::Win32::Foundation::HWND, paths: &[std::path::PathBuf]) {
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW};
    let data = super::winmsg::encode_paths(paths);
    let cds = COPYDATASTRUCT { dwData: super::winmsg::COPYDATA_PATHS, cbData: (data.len() * 2) as u32, lpData: data.as_ptr() as _ };
    let mut result = 0usize;
    // SAFETY: the struct and its data outlive the synchronous send.
    let ok = unsafe { SendMessageTimeoutW(hwnd, 0x004A, 0, &cds as *const _ as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
    log::info!("handed {} path(s) to the running instance ({})", paths.len(), if ok != 0 && result != 0 { "taken" } else { "not taken" });
}

fn find_window(prop: &[u16]) -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetPropW};
    struct Search<'a> {
        prop: &'a [u16],
        found: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> i32 {
        // SAFETY: `data` is the Search below, alive for the enumeration.
        unsafe {
            let s = &mut *(data as *mut Search);
            if !GetPropW(hwnd, s.prop.as_ptr()).is_null() {
                s.found = hwnd;
                return 0;
            }
        }
        1
    }
    let mut s = Search { prop, found: std::ptr::null_mut() };
    // SAFETY: enumeration with a callback that only reads window properties.
    unsafe {
        EnumWindows(Some(each), &mut s as *mut _ as LPARAM);
    }
    (!s.found.is_null()).then_some(s.found)
}

fn signal(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetWindowThreadProcessId, PostMessageW};
    // SAFETY: queries and a post to another process's window; nothing is dereferenced.
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 {
            AllowSetForegroundWindow(pid);
        }
        PostMessageW(hwnd, show_message(), 0, 0) != 0
    }
}

impl Guard {
    /// Mark the app's window so a second launch finds it. Once, when it exists.
    pub fn mark_window(&self, hwnd: *mut std::ffi::c_void) {
        use windows_sys::Win32::UI::WindowsAndMessaging::SetPropW;
        // SAFETY: our own window; any non-null value marks it.
        unsafe {
            SetPropW(hwnd as _, self.prop.as_ptr(), 1 as _);
        }
    }

}

/// Bring the window back for a second launch: show it (from the tray too), restore it
/// if minimized, and give it the foreground, which the second launch allowed. With
/// `MP_WINDOW=secondary` it is shown without activation (and the activation guard in
/// `devwindow` would refuse it anyway).
pub fn bring_back(app: &crate::AppWindow, hwnd: isize) {
    use slint::ComponentHandle;
    super::placement::show_pending(hwnd);
    let _ = app.show();
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOWNOACTIVATE, SetForegroundWindow, ShowWindow};
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        if hwnd.is_null() {
            return;
        }
        // SAFETY: our own window, on its UI thread.
        unsafe {
            let quiet = super::devwindow::enabled();
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, if quiet { SW_SHOWNOACTIVATE } else { SW_RESTORE });
            }
            if !quiet {
                SetForegroundWindow(hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::key;
    use std::path::Path;

    #[test]
    fn key_ignores_case_and_separators() {
        let a = key(Path::new(r"C:\Users\X\AppData\Roaming\MarqueePlayer-nonexistent"));
        assert_eq!(a, key(Path::new(r"c:\users\x\appdata\roaming\marqueeplayer-nonexistent\")));
        assert_eq!(a, key(Path::new("C:/Users/X/AppData/Roaming/MarqueePlayer-nonexistent")));
        assert_ne!(a, key(Path::new(r"C:\Temp\mp-test-nonexistent")), "another data folder, another instance");
        assert_eq!(a.len(), 16);
    }
}
