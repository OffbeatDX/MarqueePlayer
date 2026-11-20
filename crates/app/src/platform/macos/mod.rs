//! macOS behind the platform seam (`crate::platform`), through AppKit (the objc2 bindings,
//! which need no SDK to build) and winit's macOS extensions: the system's traffic lights over
//! the app's own caption (`window`), one instance per data folder through a Unix socket, the
//! secret's 0600 file and the window's size and state (`instance`, `secret`, `placement`:
//! shared with Linux, in `unix/`), the Finder's open-documents and reopen Apple events
//! (`appleevents`), Now Playing and the media keys through souvlaki (`media`), the Finder,
//! `open` and the pasteboard (`shell`), a LaunchAgent for "Open at login" (`integration`), the
//! menu bar's status item (`tray`), and what the system says of itself (`system`).

mod appleevents;
pub mod devwindow;
pub mod instance;
pub mod integration;
pub mod media;
pub mod shell;
pub mod system;
pub mod tray;
pub mod window;

pub use super::unix::{placement, secret};
