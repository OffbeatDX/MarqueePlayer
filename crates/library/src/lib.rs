//! Music library for Marquee Player: folder scan into SQLite, tags, art cache, and
//! the queries behind every page. Music folders are only ever read, except by `edit`
//! (tag editing, off unless the user turns it on), which writes through verified atomic
//! replacements only.

pub mod accent;
pub mod art;
pub mod audio;
pub mod db;
pub mod dirwatch;
pub mod edit;
pub mod fold;
pub mod group;
pub mod guard;
pub mod hash;
pub mod lyrics;
pub mod marks;
pub mod m3u;
pub mod meta;
pub mod names;
pub mod query;
pub mod relations;
pub mod remote;
pub mod reroot;
pub mod romaji;
pub mod scan;
pub mod smart;
pub mod session;
pub mod tags;
pub mod watch;

pub use art::{ArtCache, ArtPaths};
pub use query::{Album, Playlist, SearchResults, ShelfQuery, Track};

/// Run the calling thread below normal priority: library scans, art resizing and cover
/// decoding yield to the UI and, above all, to audio output (which runs under MMCSS on
/// Windows; PipeWire's own real-time thread on Linux; Core Audio's on macOS).
pub fn background_thread() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        // SAFETY: the pseudo-handle of the calling thread, always valid.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }
    // Linux: a thread is a task with a nice value of its own; raising it (lower priority)
    // needs no privilege. 5 is about a third of a normal thread's share of a busy core.
    #[cfg(target_os = "linux")]
    {
        // SAFETY: gettid has no arguments; setpriority on our own thread id.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            libc::setpriority(libc::PRIO_PROCESS, tid, 5);
        }
    }
    // macOS: the utility QoS class, which the scheduler serves after interactive work (the
    // UI, and the output thread's user-interactive class) and Core Audio's real-time thread.
    #[cfg(target_os = "macos")]
    {
        // SAFETY: changes only the calling thread's scheduling.
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    }
}

#[cfg(test)]
pub(crate) mod testdir {
    use std::path::{Path, PathBuf};

    /// A test's own folder in the temp folder (`mp-<name>-<process id>`), new and empty, and
    /// removed when dropped, a failed assertion's unwinding included. Drop what has files open in
    /// it first (on Windows an open file cannot be removed): declare it before them.
    pub(crate) struct TestDir(PathBuf);

    impl TestDir {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl std::ops::Deref for TestDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for TestDir {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl From<&TestDir> for PathBuf {
        fn from(d: &TestDir) -> PathBuf {
            d.0.clone()
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
