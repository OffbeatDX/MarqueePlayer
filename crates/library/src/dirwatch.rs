//! Watching a folder for changes without polling: one thread per folder, waiting with
//! no timeout in the kernel until something under it changes, so a watch costs nothing
//! while nothing happens. (The `notify` crate's Windows backend woke every 100 ms per
//! watcher, idle or not.) `watch_dir` returns once the watch is in place, so no change
//! made after it returns is missed.
//!
//! Reports each changed path (a file or a folder, possibly gone by now), or
//! `Seen::Lost` when the system's change buffer overflowed and the details are lost, or
//! `Seen::Ended` (last) when the watch stopped by itself: the folder went away (deleted,
//! its drive unplugged, its share disconnected) or can no longer be watched. Watching it
//! again is the caller's choice (`watch` does, once the folder is back). Dropping the
//! `DirWatch` stops the thread.
//!
//! One implementation per system behind this interface: `ReadDirectoryChangesW` on Windows
//! (`dirwatch/windows.rs`), inotify on Linux (`dirwatch/inotify.rs`, which also adds a
//! watch for every folder that appears, and says when the system's limit of watches is
//! reached: `limit_note`), FSEvents on macOS (`dirwatch/fsevents.rs`).

use std::path::{Path, PathBuf};

/// One change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Something at this path was created, removed, renamed (both names come), or
    /// written to.
    Path(PathBuf),
    /// Too much changed at once: the system dropped the list.
    Lost,
    /// The watch stopped by itself (the folder is gone or cannot be watched any more):
    /// nothing comes after this. What changed meanwhile is not known.
    Ended,
}

#[cfg(windows)]
#[path = "dirwatch/windows.rs"]
mod imp;

#[cfg(target_os = "linux")]
#[path = "dirwatch/inotify.rs"]
mod imp;

#[cfg(target_os = "macos")]
#[path = "dirwatch/fsevents.rs"]
mod imp;

/// Keeps a folder watched; dropping it stops watching (and joins its thread).
pub struct DirWatch {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    inner: Option<imp::Inner>,
}

/// Watch `dir` (and everything below it if `recursive`), calling `on` from the watch's
/// own thread for each change.
pub fn watch_dir(dir: &Path, recursive: bool, on: impl FnMut(Seen) + Send + 'static) -> std::io::Result<DirWatch> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    {
        Ok(DirWatch { inner: Some(imp::start(dir, recursive, Box::new(on))?) })
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (dir, recursive, on);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "folder watching is not implemented on this system"))
    }
}

/// When the system would not watch every folder (Linux: `fs.inotify.max_user_watches` is
/// lower than the library's folders), what to tell the user; `None` while all is watched.
pub fn limit_note() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        imp::limit_note()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        if let Some(inner) = self.inner.take() {
            inner.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn collect(rx: &mpsc::Receiver<Seen>, want: impl Fn(&[Seen]) -> bool) -> Vec<Seen> {
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !want(&got) && Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(50)) {
                got.push(s);
            }
        }
        got
    }

    #[test]
    fn reports_creations_renames_writes_and_removals_and_stops() {
        let dir = crate::testdir::TestDir::new("dirwatch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (tx, rx) = mpsc::channel();
        let w = watch_dir(&dir, true, move |s| {
            let _ = tx.send(s);
        })
        .unwrap();
        let has = |got: &[Seen], p: &Path| got.iter().any(|s| *s == Seen::Path(p.to_path_buf()));
        let a = dir.join("sub").join("a.flac");
        std::fs::write(&a, b"x").unwrap();
        assert!(has(&collect(&rx, |g| has(g, &a)), &a), "created, below the folder");
        let b = dir.join("sub").join("Vol. 2");
        std::fs::create_dir(&b).unwrap();
        let c = b.join("b.flac");
        std::fs::rename(&a, &c).unwrap();
        let got = collect(&rx, |g| has(g, &a) && has(g, &c));
        assert!(has(&got, &a) && has(&got, &c), "a move reports both names: {got:?}");
        std::fs::remove_dir_all(&b).unwrap();
        assert!(has(&collect(&rx, |g| has(g, &b)), &b), "a removed folder");
        let t0 = Instant::now();
        drop(w);
        assert!(t0.elapsed() < Duration::from_secs(1), "stops promptly");
        while rx.try_recv().is_ok() {} // what came before the stop
        std::fs::write(dir.join("late.flac"), b"y").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(rx.try_recv().is_err(), "nothing after it stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_watch_ignores_subfolders() {
        let dir = crate::testdir::TestDir::new("dirwatch-flat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch_dir(&dir, false, move |s| {
            let _ = tx.send(s);
        })
        .unwrap();
        std::fs::write(dir.join("sub").join("deep.txt"), b"x").unwrap();
        let top = dir.join("theme.toml");
        std::fs::write(&top, b"x").unwrap();
        let got = collect(&rx, |g| g.contains(&Seen::Path(top.clone())));
        assert!(got.contains(&Seen::Path(top.clone())), "{got:?}");
        assert!(!got.contains(&Seen::Path(dir.join("sub").join("deep.txt"))), "{got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_appears_is_watched_too() {
        let dir = crate::testdir::TestDir::new("dirwatch-new");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch_dir(&dir, true, move |s| {
            let _ = tx.send(s);
        })
        .unwrap();
        let has = |got: &[Seen], p: &Path| got.iter().any(|s| *s == Seen::Path(p.to_path_buf()));
        let album = dir.join("New Album").join("CD1");
        std::fs::create_dir_all(&album).unwrap();
        let got = collect(&rx, |g| has(g, &dir.join("New Album")));
        assert!(has(&got, &dir.join("New Album")), "the new folder: {got:?}");
        // Give the watch a moment to reach the folders below it, then write there.
        std::thread::sleep(Duration::from_millis(200));
        let f = album.join("01.flac");
        std::fs::write(&f, b"x").unwrap();
        let got = collect(&rx, |g| has(g, &f));
        assert!(has(&got, &f), "a file two levels into the new folder: {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
