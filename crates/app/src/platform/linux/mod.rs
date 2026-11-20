//! Linux behind the platform seam (`crate::platform`), by freedesktop conventions, on
//! Wayland and X11 alike: client-side decorations on winit (`window`), one instance per
//! data folder through a Unix socket, the secret's 0600 file and the window's size and
//! state (`instance`, `secret`, `placement`: shared with macOS, in `unix/`), MPRIS for the
//! media controls (`media`), the file manager over `org.freedesktop.FileManager1` and
//! `xdg-open` (`shell`), and what the desktop says of itself (`system`). The session bus is
//! shared (`dbus`).

mod dbus;
pub mod devwindow;
pub mod integration;
pub mod media;
pub mod shell;
pub mod system;
pub mod tray;
pub mod window;

pub use super::unix::{instance, placement, secret};
