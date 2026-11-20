//! macOS: one running app per data folder, as on Linux (the Unix socket, `unix/instance.rs`).
//! A launch from the Finder (a double-clicked audio file, files dropped on the Dock icon)
//! starts no second process: AppKit hands the running app an Apple event instead, which
//! `appleevents` takes once this instance is the first, and passes on as a second launch's
//! request (`hand_over`).

use std::path::{Path, PathBuf};

pub use crate::platform::unix::instance::{Guard, Instance, window_ready};

pub fn acquire(dir: &Path, paths: &[PathBuf]) -> Instance {
    let found = crate::platform::unix::instance::acquire(dir, paths);
    if matches!(found, Instance::First(_)) {
        super::appleevents::install();
    }
    found
}
