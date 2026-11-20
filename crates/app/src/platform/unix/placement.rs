//! Linux and macOS: the window's size and maximized (on macOS, zoomed) state, kept across
//! restarts, and its position where the platform allows it. Saved in logical pixels (the same apparent size at
//! another scale factor). Wayland gives no client its window's position and lets the
//! compositor place every new window, so only the size and maximized state come back
//! there; on X11 and macOS the position too, when it is still on a screen. Restored through Slint
//! before the window is first shown, so it opens at its size instead of resizing after.

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use crate::AppWindow;
use crate::settings::WindowPlacement;

/// Does the window system tell (and take) window positions? X11 and macOS yes, Wayland no.
fn positions() -> bool {
    cfg!(target_os = "macos") || (std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some())
}

/// Size (and position on X11) as saved, then maximized if it was.
/// Nothing waits for a first show here (no start in the tray on Linux or macOS).
pub fn show_pending(_hwnd: isize) {}

pub fn before_show(app: &AppWindow, saved: Option<WindowPlacement>) {
    let Some(p) = saved else { return };
    // Saturating: the numbers come from a file that may have been edited by hand.
    let (w, h) = (p.right.saturating_sub(p.left), p.bottom.saturating_sub(p.top));
    if !(400..=100_000).contains(&w) || !(300..=100_000).contains(&h) {
        log::warn!("window placement {p:?} ignored (implausible size)");
        return;
    }
    LAST.with(|l| l.set(Some(WindowPlacement { maximized: false, ..p })));
    app.window().set_size(slint::LogicalSize::new(w as f32, h as f32));
    if positions() && (p.left != 0 || p.top != 0) {
        app.window().set_position(slint::LogicalPosition::new(p.left as f32, p.top as f32));
    }
    if p.maximized {
        app.window().set_maximized(true);
    }
    log::info!("window placement restored: {w} x {h}{}", if p.maximized { ", maximized" } else { "" });
}

/// Nothing more once the window exists.
pub fn restore(_hwnd: isize, _saved: WindowPlacement) {}

/// The window's placement now: the restored size (not the maximized one: winit reports
/// the current size, so a maximized window keeps what was saved before it was maximized).
pub fn now(app: &AppWindow, _hwnd: isize) -> Option<WindowPlacement> {
    let (maximized, minimized) = app.window().with_winit_window(|w| (w.is_maximized(), w.is_minimized().unwrap_or(false)))?;
    if minimized || !app.window().is_visible() {
        return None;
    }
    if maximized {
        // Keep the size it is restored to; only the flag changes.
        return Some(LAST.with(|l| l.get()).map_or(WindowPlacement { left: 0, top: 0, right: 1440, bottom: 900, maximized: true }, |p| WindowPlacement { maximized: true, ..p }));
    }
    // The monitor's own factor, not Slint's (which includes the interface size): saved in
    // the logical pixels it is restored in, before the zoom applies.
    let scale = app.window().with_winit_window(|w| w.scale_factor() as f32).unwrap_or_else(|| app.window().scale_factor());
    let size = app.window().size().to_logical(scale);
    let (left, top) = if positions() {
        let p = app.window().position().to_logical(scale);
        (p.x as i32, p.y as i32)
    } else {
        (0, 0)
    };
    let p = WindowPlacement { left, top, right: left + size.width as i32, bottom: top + size.height as i32, maximized: false };
    LAST.with(|l| l.set(Some(p)));
    Some(p)
}

thread_local! {
    /// The last restored (not maximized) placement seen.
    static LAST: std::cell::Cell<Option<WindowPlacement>> = const { std::cell::Cell::new(None) };
}
