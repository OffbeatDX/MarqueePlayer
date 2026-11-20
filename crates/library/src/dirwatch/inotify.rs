//! Linux: inotify. One inotify instance per watched root, read by its own thread, which
//! sleeps in `poll` on it (and on an eventfd that `stop` writes) until something happens.
//!
//! inotify watches one folder at a time, so a recursive watch adds a watch for every
//! folder below the root at start, and for each folder that appears later (created, or
//! moved in from elsewhere: what is already inside it is reported too, as it may have been
//! written before its watch was in place). A folder moved away or deleted loses its
//! watches. Reported per change, as on Windows: created, deleted, moved (both names), a
//! file closed after writing, its time or attributes changed (a `touch`); the event queue
//! overflowing is `Seen::Lost`.
//!
//! Each user has a limit of watches (`fs.inotify.max_user_watches`; 8192 on some systems,
//! more on most). A library with more folders than it allows is watched as far as it goes:
//! the log says so once, and `limit_note` gives the user the line to show, with what to
//! do. Changes in the folders left out are then seen at the next full scan (every start).

use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;

use super::Seen;

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_EXCL_UNLINK
    | libc::IN_DONT_FOLLOW;

/// Set once a watch was refused for the limit: (folders watched, the limit).
static LIMIT: Mutex<Option<(usize, u64)>> = Mutex::new(None);

fn max_user_watches() -> u64 {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

pub fn limit_note() -> Option<String> {
    let (watched, limit) = (*LIMIT.lock().unwrap())?;
    Some(format!(
        "Only {watched} folders are watched for changes: the system allows {limit} (fs.inotify.max_user_watches). Changes elsewhere are seen at the next start. To raise it: echo fs.inotify.max_user_watches=524288 | sudo tee /etc/sysctl.d/90-marquee-player.conf && sudo sysctl --system"
    ))
}

/// The inotify instance and its watches. Owns the descriptor: closed when dropped (the
/// thread's end, or `start` failing after it was made, the thread not started included).
struct Watcher {
    fd: i32,
    recursive: bool,
    dirs: HashMap<i32, PathBuf>,
    limit_logged: bool,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: the descriptor from `inotify_init1`, owned by this alone; closed once.
        unsafe { libc::close(self.fd) };
    }
}

impl Watcher {
    /// Watch `dir` alone. An error when it could not be (gone, not a folder, the limit).
    fn add(&mut self, dir: &Path) -> std::io::Result<()> {
        let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| std::io::Error::other("a NUL in the path"))?;
        // SAFETY: a valid inotify descriptor and a NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), MASK | libc::IN_ONLYDIR) };
        if wd < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ENOSPC) {
                let limit = max_user_watches();
                *LIMIT.lock().unwrap() = Some((self.dirs.len(), limit));
                if !self.limit_logged {
                    self.limit_logged = true;
                    log::warn!(
                        "watching {}: the system's limit of folder watches (fs.inotify.max_user_watches = {limit}) is reached after {} folders; changes in the others are seen at the next full scan",
                        dir.display(),
                        self.dirs.len()
                    );
                }
            }
            return Err(e);
        }
        self.dirs.insert(wd, dir.to_path_buf());
        Ok(())
    }

    /// Watch `dir` and, recursively, every folder below it. Calls `found` for each entry
    /// below it (`report`: a folder that appeared, whose contents may predate its watch).
    fn add_tree(&mut self, dir: &Path, report: bool, found: &mut dyn FnMut(PathBuf)) -> std::io::Result<()> {
        self.add(dir)?;
        if !self.recursive {
            return Ok(());
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for entry in rd.flatten() {
                let p = entry.path();
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                if report {
                    found(p.clone());
                }
                if is_dir && self.add(&p).is_ok() {
                    stack.push(p);
                }
            }
        }
        Ok(())
    }

    /// Stop watching `dir` and every folder below it (moved away or deleted).
    fn remove_tree(&mut self, dir: &Path) {
        let gone: Vec<i32> = self.dirs.iter().filter(|(_, p)| p.starts_with(dir)).map(|(&wd, _)| wd).collect();
        for wd in gone {
            self.dirs.remove(&wd);
            // SAFETY: a watch of this descriptor (one already gone is a harmless EINVAL).
            unsafe { libc::inotify_rm_watch(self.fd, wd) };
        }
    }
}

pub struct Inner {
    stop: i32,
    thread: JoinHandle<()>,
}

pub fn start(path: &Path, recursive: bool, mut on: Box<dyn FnMut(Seen) + Send>) -> std::io::Result<Inner> {
    // SAFETY: plain descriptor creation; the inotify one is closed by `Watcher`, the
    // eventfd by `Inner::stop` (or below).
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    let stop = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if stop < 0 {
        let e = std::io::Error::last_os_error();
        // SAFETY: opened above, not shared.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    let mut w = Watcher { fd, recursive, dirs: HashMap::new(), limit_logged: false };
    // The root must be watched; folders below it as far as the limit allows.
    if let Err(e) = w.add_tree(path, false, &mut |_| {}) {
        // SAFETY: opened above, not shared. (The inotify descriptor closes with `w`.)
        unsafe { libc::close(stop) };
        return Err(e);
    }
    let root = path.to_path_buf();
    let name = format!("mp-dirwatch-{}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
    // The thread owns `w` from here, and closes its descriptor as it ends; a thread that
    // could not be started drops it unrun, which closes it too. (The eventfd is `Inner`'s,
    // closed after the thread has ended.)
    let thread = std::thread::Builder::new().name(name).spawn(move || run(&mut w, &root, stop, &mut *on));
    match thread {
        Ok(thread) => Ok(Inner { stop, thread }),
        Err(e) => {
            // SAFETY: the thread never started; closed once.
            unsafe { libc::close(stop) };
            Err(e)
        }
    }
}

fn run(w: &mut Watcher, root: &Path, stop: i32, on: &mut dyn FnMut(Seen)) {
    // Room for many events at once (each is 16 bytes and a name of up to 256).
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut fds = [libc::pollfd { fd: w.fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: stop, events: libc::POLLIN, revents: 0 }];
        // SAFETY: two valid pollfds; no timeout: sleeps until an event or the stop.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if r < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log::warn!("watching {}: poll failed ({}); stopped", root.display(), std::io::Error::last_os_error());
            on(Seen::Ended);
            return;
        }
        if fds[1].revents != 0 {
            return;
        }
        // SAFETY: reads into a buffer of the given length.
        let n = unsafe { libc::read(w.fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
                continue;
            }
            log::warn!("watching {}: {e}; stopped", root.display());
            on(Seen::Ended);
            return;
        }
        for ev in parse(&buf[..n as usize]) {
            handle(w, ev, on);
        }
        if w.dirs.is_empty() {
            // The root itself went away: nothing is left to watch.
            log::info!("watching {}: the folder is gone; stopped", root.display());
            on(Seen::Ended);
            return;
        }
    }
}

struct Event<'a> {
    wd: i32,
    mask: u32,
    name: &'a [u8],
}

/// The events in what `read` returned: `struct inotify_event` (wd, mask, cookie, len,
/// then the name, NUL-padded to `len`).
fn parse(bytes: &[u8]) -> Vec<Event<'_>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 16 <= bytes.len() {
        let word = |i: usize| [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
        let wd = i32::from_ne_bytes(word(at));
        let mask = u32::from_ne_bytes(word(at + 4));
        let len = u32::from_ne_bytes(word(at + 12)) as usize;
        let end = (at + 16 + len).min(bytes.len());
        let raw = &bytes[at + 16..end];
        let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        out.push(Event { wd, mask, name });
        at += 16 + len;
    }
    out
}

fn handle(w: &mut Watcher, ev: Event, on: &mut dyn FnMut(Seen)) {
    if ev.mask & libc::IN_Q_OVERFLOW != 0 {
        on(Seen::Lost);
        return;
    }
    if ev.mask & libc::IN_IGNORED != 0 {
        // The kernel dropped this watch (its folder deleted, or the filesystem unmounted).
        w.dirs.remove(&ev.wd);
        return;
    }
    let Some(dir) = w.dirs.get(&ev.wd).cloned() else { return };
    if ev.name.is_empty() {
        // The watched folder itself was deleted or moved: its parent's watch reports it by
        // name (the root's removal is noticed when no watch is left).
        if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
            w.remove_tree(&dir);
        }
        return;
    }
    let path = dir.join(OsStr::from_bytes(ev.name));
    let is_dir = ev.mask & libc::IN_ISDIR != 0;
    if is_dir && ev.mask & libc::IN_MOVED_FROM != 0 {
        w.remove_tree(&path);
    }
    on(Seen::Path(path.clone()));
    if is_dir && w.recursive && ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
        let mut found = Vec::new();
        let _ = w.add_tree(&path, true, &mut |p| found.push(p));
        for p in found {
            on(Seen::Path(p));
        }
    }
}

impl Inner {
    pub fn stop(self) {
        let one: u64 = 1;
        // SAFETY: an 8-byte write to our own eventfd (still open: closed below).
        unsafe { libc::write(self.stop, (&raw const one).cast(), 8) };
        let _ = self.thread.join();
        // SAFETY: the eventfd from `start`; the thread that polled it has ended.
        unsafe { libc::close(self.stop) };
    }
}
