//! Folder watching: changes under a root are collected until things have been quiet
//! for `DEBOUNCE`, then handed to `on_change` at once: the paths that changed (for a
//! partial scan of just their folders, `scan::scopes`), or `Change::Everything` when
//! the system lost track (its change buffer overflowed) or too much changed. Nothing
//! polls while every root is watched: each root's watch thread sleeps in the kernel
//! until something changes (`dirwatch`), and so does the thread that keeps them.
//!
//! A root that is not there (a drive unplugged, a share down) is not watched; nor is
//! one whose watch ended (`Seen::Ended`: it went away while watched, which also says
//! `Change::Everything`). Only while some root is unwatched does the keeping thread wake,
//! every `RETRY`, to look for it; once it is back it is watched again, and
//! `Change::Everything` says to scan, as whatever changed meanwhile was not seen. A root
//! that is there but cannot be watched (a volume that does not report changes) is tried
//! less and less often, up to `RETRY_MAX`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::dirwatch::{self, DirWatch, Seen};

const DEBOUNCE: Duration = Duration::from_secs(3);
/// How often a root that is not there is looked for.
const RETRY: Duration = Duration::from_secs(30);
/// The longest wait between two tries of a root that is there but cannot be watched.
const RETRY_MAX: Duration = Duration::from_secs(30 * 60);
/// More paths than this in one batch: a full scan rather than a long list.
const MAX_PATHS: usize = 20_000;
/// The changes are told this many debounces after the first at the latest (30 s), however
/// busy the folders stay: a file written on and on (a download, a recording) does not hold
/// back every other change.
const LONGEST: u32 = 10;

/// What changed under the roots since the last call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Paths(Vec<PathBuf>),
    Everything,
}

/// Keeps the watch alive; dropping it stops watching (no change is reported after the
/// drop; the threads end on their own shortly after).
pub struct FolderWatch {
    ctl: mpsc::Sender<Ctl>,
    alive: Arc<AtomicBool>,
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let _ = self.ctl.send(Ctl::Stop);
    }
}

/// To the thread that keeps the roots' watches.
enum Ctl {
    /// This root's watch stopped by itself.
    Ended(usize),
    Stop,
}

/// One root, watched or waiting to be.
struct Slot {
    root: PathBuf,
    watch: Option<DirWatch>,
    /// When to try again, and the wait after that (unwatched only).
    next: Instant,
    wait: Duration,
}

pub fn watch(roots: &[PathBuf], on_change: impl Fn(Change) + Send + 'static) -> std::io::Result<FolderWatch> {
    // A folder inside another is watched as part of it.
    watch_with(&crate::scan::outermost(roots), DEBOUNCE, RETRY, on_change)
}

fn watch_with(roots: &[PathBuf], debounce: Duration, retry: Duration, on_change: impl Fn(Change) + Send + 'static) -> std::io::Result<FolderWatch> {
    // `None`: everything.
    let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
    let (ctl, ctl_rx) = mpsc::channel::<Ctl>();
    let now = Instant::now();
    // Each root watched before this returns, so no change made after it is missed.
    let mut slots: Vec<Slot> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut s = Slot { root: r.clone(), watch: None, next: now + retry, wait: retry };
            if r.is_dir() {
                match arm(i, r, &tx, &ctl) {
                    Ok(w) => s.watch = Some(w),
                    Err(e) => log::warn!("watching {}: {e}; tried again later", r.display()),
                }
            }
            s
        })
        .collect();
    let alive = Arc::new(AtomicBool::new(true));
    let on_alive = alive.clone();
    std::thread::Builder::new().name("mp-watch".into()).spawn(move || {
        // Ends when every sender is gone: the roots' watches and the keeping thread's.
        while let Ok(first) = rx.recv() {
            // Collect until a quiet period (or `LONGEST`), then fire once.
            let mut paths: HashSet<PathBuf> = HashSet::new();
            let mut everything = false;
            let mut add = |m: Option<PathBuf>| match m {
                Some(p) if !everything => {
                    paths.insert(p);
                    if paths.len() > MAX_PATHS {
                        everything = true;
                        paths = HashSet::new();
                    }
                }
                _ => everything = true,
            };
            add(first);
            let until = Instant::now() + debounce * LONGEST;
            loop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match rx.recv_timeout(debounce.min(left)) {
                    Ok(m) => add(m),
                    Err(_) => break,
                }
            }
            if !on_alive.load(Ordering::Acquire) {
                return;
            }
            let mut paths: Vec<PathBuf> = paths.into_iter().collect();
            paths.sort();
            on_change(if everything { Change::Everything } else { Change::Paths(paths) });
        }
    })?;
    let keep_ctl = ctl.clone();
    std::thread::Builder::new().name("mp-watch-keep".into()).spawn(move || {
        loop {
            // Asleep for good while every root is watched; otherwise until the next try.
            let due = slots.iter().filter(|s| s.watch.is_none()).map(|s| s.next).min();
            let msg = match due {
                None => ctl_rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                Some(at) => ctl_rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            };
            match msg {
                Ok(Ctl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(Ctl::Ended(i)) => {
                    let s = &mut slots[i];
                    // Its thread has ended (or is ending): this joins it.
                    s.watch = None;
                    s.wait = retry;
                    s.next = Instant::now() + retry;
                    log::info!("watching {}: stopped; watched again once it is back", s.root.display());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    for (i, s) in slots.iter_mut().enumerate() {
                        if s.watch.is_some() || s.next > now {
                            continue;
                        }
                        if !s.root.is_dir() {
                            s.next = Instant::now() + retry;
                            continue;
                        }
                        match arm(i, &s.root, &tx, &keep_ctl) {
                            Ok(w) => {
                                log::info!("watching {} again", s.root.display());
                                s.watch = Some(w);
                                // What changed while it was away was not seen.
                                let _ = tx.send(None);
                            }
                            Err(e) => {
                                if s.wait == retry {
                                    log::warn!("watching {}: {e}; tried again later", s.root.display());
                                }
                                s.next = Instant::now() + s.wait;
                                s.wait = (s.wait * 2).min(RETRY_MAX);
                            }
                        }
                    }
                }
            }
        }
    })?;
    Ok(FolderWatch { ctl, alive })
}

/// Watch root `i`, sending what it sees to `tx` and its end to `ctl`.
fn arm(i: usize, root: &std::path::Path, tx: &mpsc::Sender<Option<PathBuf>>, ctl: &mpsc::Sender<Ctl>) -> std::io::Result<DirWatch> {
    let (tx, ctl) = (tx.clone(), ctl.clone());
    dirwatch::watch_dir(root, true, move |seen| match seen {
        Seen::Lost => {
            let _ = tx.send(None);
        }
        Seen::Ended => {
            let _ = tx.send(None);
            let _ = ctl.send(Ctl::Ended(i));
        }
        // Audio, art, folders, and anything gone (which may have been a folder, even
        // one with a dot in its name). Other files do not matter.
        Seen::Path(p) => {
            if crate::scan::is_audio(&p) || crate::art::is_image(&p) || p.is_dir() || !p.exists() {
                let _ = tx.send(Some(p));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(rx: &mpsc::Receiver<Change>, within: Duration) -> Option<Change> {
        rx.recv_timeout(within).ok()
    }

    /// A root that is not there at first is watched once it appears; one that goes away
    /// while watched says so, and is watched again when it is back.
    #[test]
    fn a_root_that_goes_away_is_watched_again_when_it_is_back() {
        let base = crate::testdir::TestDir::new("watch-rearm");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (present, later) = (base.join("present"), base.join("later"));
        std::fs::create_dir_all(&present).unwrap();
        let (tx, rx) = mpsc::channel();
        let w = watch_with(&[present.clone(), later.clone()], Duration::from_millis(100), Duration::from_millis(200), move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        let wait = Duration::from_secs(10);

        // Not there at the start: watched once it appears, with a scan to catch up.
        std::fs::create_dir_all(&later).unwrap();
        assert_eq!(next(&rx, wait), Some(Change::Everything), "the folder that appeared");
        let f = later.join("a.flac");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(next(&rx, wait), Some(Change::Paths(vec![f])));

        // Gone while watched (the drive unplugged): said, then watched again once back.
        std::fs::remove_dir_all(&present).unwrap();
        assert_eq!(next(&rx, wait), Some(Change::Everything), "the folder went away");
        // Windows keeps a deleted folder's name until the watch lets go of it.
        let t0 = Instant::now();
        while std::fs::create_dir(&present).is_err() && t0.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(present.is_dir());
        assert_eq!(next(&rx, wait), Some(Change::Everything), "back, watched again");
        let g = present.join("b.flac");
        std::fs::write(&g, b"x").unwrap();
        assert_eq!(next(&rx, wait), Some(Change::Paths(vec![g])));

        // Nothing after the drop.
        drop(w);
        std::fs::write(present.join("c.flac"), b"x").unwrap();
        assert_eq!(next(&rx, Duration::from_millis(600)), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file written on and on (a download, a recording) does not hold back every change:
    /// what was seen is told `LONGEST` debounces after the first event at the latest.
    #[test]
    fn a_file_written_on_and_on_does_not_hold_back_the_changes() {
        let dir = crate::testdir::TestDir::new("watch-busy");
        let debounce = Duration::from_millis(200);
        let (tx, rx) = mpsc::channel();
        let w = watch_with(std::slice::from_ref(&dir.to_path_buf()), debounce, Duration::from_secs(30), move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        let other = dir.join("other.flac");
        std::fs::write(&other, b"x").unwrap();
        let busy = dir.join("busy.flac");
        let t0 = Instant::now();
        let mut f = std::fs::File::create(&busy).unwrap();
        let mut told = None;
        while t0.elapsed() < debounce * (LONGEST + 10) {
            std::io::Write::write_all(&mut f, &[0; 512]).unwrap();
            f.sync_all().unwrap();
            if let Ok(c) = rx.recv_timeout(Duration::from_millis(20)) {
                told = Some(c);
                break;
            }
        }
        drop(f);
        drop(w);
        let Some(Change::Paths(paths)) = told else { panic!("nothing told while the file was written: {told:?}") };
        assert!(paths.contains(&other), "{paths:?}");
    }
}
