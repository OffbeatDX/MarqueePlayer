//! Windows: Win32 behind the platform seam (`crate::platform`): the app-drawn caption on the native frame
//! (`frame`), the window procedure subclass (`winmsg`), `MP_WINDOW=secondary`
//! (`devwindow`), single instance and `WM_COPYDATA` (`instance`), the window placement,
//! the media overlay through souvlaki, Explorer and the clipboard (`shell`), and what the
//! system says of itself (`system`).

pub mod devwindow;
mod frame;
pub mod instance;
pub mod integration;
pub mod media;
pub mod placement;
pub mod secret;
pub mod shell;
pub mod system;
pub mod tray;
pub mod window;
mod winmsg;
