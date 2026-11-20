//! Linux and macOS: one running app per data folder, through a Unix socket in the user's
//! runtime directory (`$XDG_RUNTIME_DIR`, else `/tmp`: macOS has no runtime directory), named
//! after the data folder. The first
//! instance binds it and listens on a thread of its own (asleep in `accept`); a second
//! launch connects, writes its request (show the window, and the files it was given, one
//! path per line) and exits. The listener hands the request to the UI thread, where it
//! waits until the window exists. A socket left by a crash (nobody accepts on it) is
//! removed and bound again.
//!
//! A socket rather than D-Bus: it needs no session bus or service file, works the same on
//! every desktop and in a headless session, and only the user can reach it: the runtime
//! directory is the user's alone, and without one the socket goes in a folder under /tmp that
//! this user made, mode 0700, checked to be theirs alone before each use (if it is not, the
//! app runs without the guard rather than talk to whoever made it). The socket is mode 0600.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::platform::Hooks;

/// The key for a data folder: FNV-1a of its canonical path (letter case is part of a name
/// here), so another `MP_DATA_DIR` is another instance.
pub fn key(dir: &Path) -> String {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let text = dir.to_string_lossy();
    let text = text.trim_end_matches('/');
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The socket's path, or why there is none that is safely this user's.
fn socket_path(dir: &Path) -> Result<PathBuf, String> {
    let name = format!("marquee-player-{}.sock", key(dir));
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()) {
        return Ok(runtime.join(name));
    }
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let base = PathBuf::from(format!("/tmp/marquee-player-{uid}"));
    private_dir(&base, uid)?;
    Ok(base.join(name))
}

/// Make `dir` (mode 0700) or find it, and check it is this user's alone: a real folder, not a
/// link, owned by `uid`, with no access for anyone else. A folder of the user's own that is
/// open to others (made by an older version) is closed first. Anyone can make a folder of
/// that name in /tmp first, and listen there for the paths the app hands over.
fn private_dir(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot make {} ({e})", dir.display())),
    }
    let look = || std::fs::symlink_metadata(dir).map_err(|e| format!("cannot read {} ({e})", dir.display()));
    let mut m = look()?;
    if !m.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link, or a file)", dir.display()));
    }
    if m.uid() != uid {
        return Err(format!("{} belongs to another user (uid {})", dir.display(), m.uid()));
    }
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("cannot close {} to others ({e})", dir.display()))?;
        m = look()?;
        if !m.file_type().is_dir() || m.uid() != uid || m.mode() & 0o077 != 0 {
            return Err(format!("{} is still open to others", dir.display()));
        }
    }
    Ok(())
}

/// What `acquire` found.
pub enum Instance {
    /// This is the only instance for the folder. Keep it alive for the whole run.
    First(Guard),
    /// Another instance runs on this folder; it was asked to show its window (or could
    /// not be reached: `false`). Exit.
    Second { signalled: bool },
}

/// Keeps the socket while the app runs; removes it at exit.
pub struct Guard {
    path: Option<PathBuf>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Guard {
    /// The window carries no mark here (the socket is the instance's).
    pub fn mark_window(&self, _hwnd: *mut std::ffi::c_void) {}
}

/// A second launch's request, waiting for the window when it came first.
struct Request {
    paths: Vec<PathBuf>,
}

static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());
static READY: Mutex<Option<Hooks>> = Mutex::new(None);

/// The window exists: requests go to it from now on (and those that came before it).
pub fn window_ready(hooks: Hooks) {
    *READY.lock().unwrap() = Some(hooks);
    let pending: Vec<Request> = std::mem::take(&mut *PENDING.lock().unwrap());
    for r in pending {
        deliver(hooks, r);
    }
}

/// Files from outside that do not come through the socket (macOS: the Finder's open-documents
/// Apple event, the Dock icon clicked), handed over as a second launch's are: the window is
/// shown, then the files go where a second launch's go. On the UI thread.
pub fn hand_over(paths: Vec<PathBuf>) {
    let request = Request { paths };
    let hooks = *READY.lock().unwrap();
    match hooks {
        Some(h) => deliver(h, request),
        None => PENDING.lock().unwrap().push(request),
    }
}

fn deliver(hooks: Hooks, r: Request) {
    (hooks.show)(0);
    if !r.paths.is_empty() {
        (hooks.files)(r.paths, None);
    }
}

pub fn acquire(dir: &Path, paths: &[PathBuf]) -> Instance {
    let path = match socket_path(dir) {
        Ok(p) => p,
        Err(why) => {
            log::warn!("single instance: off for this run: {why}");
            return Instance::First(Guard { path: None });
        }
    };
    for _ in 0..2 {
        match UnixListener::bind(&path) {
            Ok(listener) => {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                listen(listener);
                return Instance::First(Guard { path: Some(path) });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Someone has it: hand over, or it is a crashed instance's.
                match UnixStream::connect(&path) {
                    Ok(stream) => return Instance::Second { signalled: send(stream, paths) },
                    Err(_) => {
                        log::info!("single instance: {} was left by an instance that is gone; taking it", path.display());
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            Err(e) => {
                // Cannot tell (a read-only runtime directory): run, as without the guard.
                log::warn!("single instance: cannot bind {} ({e})", path.display());
                return Instance::First(Guard { path: None });
            }
        }
    }
    Instance::First(Guard { path: None })
}

fn send(mut stream: UnixStream, paths: &[PathBuf]) -> bool {
    let mut msg = String::from("show\n");
    for p in paths {
        msg.push_str(&p.to_string_lossy());
        msg.push('\n');
    }
    msg.push('\n');
    let ok = stream.write_all(msg.as_bytes()).is_ok();
    // Wait for the first instance to say it took it (or for it to close the socket).
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut answer = String::new();
    let taken = BufReader::new(&stream).read_line(&mut answer).is_ok() && answer.trim() == "ok";
    log::info!("handed {} path(s) to the running instance ({})", paths.len(), if taken { "taken" } else { "not taken" });
    ok && taken
}

/// The most one request may send (a long list of paths fits many times over).
const REQUEST_MAX: u64 = 16 << 20;

/// Accept second launches for the whole run, on a thread that sleeps in `accept`.
fn listen(listener: UnixListener) {
    let spawned = std::thread::Builder::new().name("mp-instance".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            // A client that connects and says nothing (or too much) does not hold up the
            // launches after it.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
            let mut lines = Vec::new();
            let mut reader = BufReader::new(std::io::Read::take(&stream, REQUEST_MAX));
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line == "\n" => break,
                    Ok(_) => lines.push(line.trim_end_matches('\n').to_owned()),
                }
            }
            if lines.first().map(String::as_str) != Some("show") {
                continue;
            }
            let paths: Vec<PathBuf> = lines[1..].iter().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
            let _ = (&stream).write_all(b"ok\n");
            let _ = slint::invoke_from_event_loop(move || hand_over(paths));
        }
    });
    if let Err(e) = spawned {
        log::warn!("single instance: no listener thread ({e})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_follows_the_path_exactly() {
        let a = key(Path::new("/home/x/.config/marquee-player-nonexistent"));
        assert_eq!(a, key(Path::new("/home/x/.config/marquee-player-nonexistent/")), "a trailing slash is the same folder");
        assert_ne!(a, key(Path::new("/home/x/.config/Marquee-Player-nonexistent")), "letter case is part of a name");
        assert_ne!(a, key(Path::new("/tmp/mp-test-nonexistent")), "another data folder, another instance");
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn a_second_launch_reaches_the_first_and_a_stale_socket_is_taken() {
        let dir = std::env::temp_dir().join(format!("mp-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A socket left by a crash: bound, then its listener gone.
        let path = socket_path(&dir).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let first = acquire(&dir, &[]);
        assert!(matches!(first, Instance::First(Guard { path: Some(_) })), "the stale socket is taken");
        let second = acquire(&dir, &[PathBuf::from("/music/a b.flac")]);
        // No event loop runs in a test: the listener answers before handing over.
        assert!(matches!(second, Instance::Second { signalled: true }));
        drop(first);
        assert!(!path.exists(), "removed at exit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fallback_folder_is_used_only_when_it_is_the_users_alone() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let root = std::env::temp_dir().join(format!("mp-instance-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().mode() & 0o777;
        // Made here: 0700, whatever the umask.
        let made = root.join("made");
        private_dir(&made, uid).unwrap();
        assert_eq!(mode(&made), 0o700);
        // The user's own, open to others (an older version made it so): closed, then used.
        let open = root.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&open, uid).unwrap();
        assert_eq!(mode(&open), 0o700);
        // A link to a folder: refused, and the folder it points at is left as it was.
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(private_dir(&link, uid).is_err());
        assert_eq!(mode(&target), 0o755);
        // Someone else's folder (another uid, as it would be in /tmp): refused.
        assert!(private_dir(&made, uid.wrapping_add(1)).is_err());
        // A file in its place: refused.
        let file = root.join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(private_dir(&file, uid).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
