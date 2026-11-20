//! The window's size, position and maximized state, kept across restarts.
//!
//! Windows (`platform/windows/placement.rs`): saved as Win32 `GetWindowPlacement` gives
//! it, the restored ("normal") rectangle in workspace coordinates, which stays right while
//! the window is maximized or minimized, plus whether it is maximized. Restored with
//! `SetWindowPlacement` once the window exists, so Restore after a maximized start goes
//! back to the saved size.
//!
//! A rectangle that is no longer on any monitor (a monitor unplugged, or the layout
//! changed) is moved onto the primary monitor's work area, keeping its size where it
//! fits. `MP_WINDOW=secondary` (devwindow.rs) places test windows itself, so it skips
//! the restore (it still saves).
//!
//! Linux (`platform/linux/placement.rs`): the size and maximized state, set through the
//! toolkit before the window is first shown; the position only on X11 (Wayland lets the
//! compositor place every window, and tells the app nothing of where).

use crate::settings::WindowPlacement;


/// The window's placement now. `None` before the window exists.
pub fn read(app: &crate::AppWindow, hwnd: isize) -> Option<WindowPlacement> {
    crate::platform::placement::now(app, hwnd)
}

thread_local! {
    static POLLED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// `read`, at most every `POLL_EVERY` unless `now`.
pub fn poll(app: &crate::AppWindow, hwnd: isize, now: bool) -> Option<WindowPlacement> {
    const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(3);
    let due = POLLED.with(|p| {
        let t = std::time::Instant::now();
        let due = now || p.get().is_none_or(|at| t.duration_since(at) >= POLL_EVERY);
        if due {
            p.set(Some(t));
        }
        due
    });
    if due { read(app, hwnd) } else { None }
}

/// Before the window is first shown: where the system restores a window through the
/// toolkit (Linux: its size and maximized state). Not under `MP_WINDOW=secondary`.
pub fn before_show(app: &crate::AppWindow, saved: Option<WindowPlacement>) {
    if !crate::platform::devwindow::enabled() {
        crate::platform::placement::before_show(app, saved);
    }
}

/// Put the window where it was, once it exists. Not under `MP_WINDOW=secondary`.
pub fn restore(hwnd: isize, saved: Option<WindowPlacement>) {
    if crate::platform::devwindow::enabled() {
        log::info!("MP_WINDOW=secondary: saved window placement not applied");
        return;
    }
    if let Some(saved) = saved {
        crate::platform::placement::restore(hwnd, saved);
    }
}
