//! `MP_WINDOW=secondary` on macOS: test windows out of the way, as on Windows. The window
//! opens on the second display when there is one (else the main one), zoomed to fill it,
//! and is never made key or brought forward, so the app the owner works in keeps the focus.
//! The bench sizes the window itself (`resize_client`).

use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::winit;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True when `MP_WINDOW=secondary` (read once, at start).
pub fn enabled() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn init() {
    let on = std::env::var("MP_WINDOW").is_ok_and(|v| v.eq_ignore_ascii_case("secondary"));
    ACTIVE.store(on, Ordering::Relaxed);
    if on {
        log::info!("MP_WINDOW=secondary: on the second display if there is one, zoomed, never made active");
    }
}

/// The second display's visible area (without the menu bar and the Dock), in winit's
/// logical coordinates (origin at the top left of the main display, y down); `None` with
/// one display.
fn secondary_area() -> Option<(f64, f64, f64, f64)> {
    let mtm = objc2::MainThreadMarker::new()?;
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    if screens.count() < 2 {
        return None;
    }
    // AppKit's screen coordinates: origin at the bottom left of the first (menu bar) display, y up.
    let main_h = screens.objectAtIndex(0).frame().size.height;
    let v = screens.objectAtIndex(1).visibleFrame();
    Some((v.origin.x, main_h - (v.origin.y + v.size.height), v.size.width, v.size.height))
}

/// The window attributes for `MP_WINDOW=secondary`: the usual ones, inactive, on the
/// second display's area when there is one, else zoomed on the main one.
pub fn attributes(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    let attrs = super::window::attributes(attrs).with_active(false);
    match secondary_area() {
        Some((x, y, w, h)) => {
            log::info!("MP_WINDOW=secondary: the second display's area, {w}x{h} at {x},{y}");
            attrs.with_position(winit::dpi::LogicalPosition::new(x, y)).with_inner_size(winit::dpi::LogicalSize::new(w, h))
        }
        None => attrs.with_maximized(true),
    }
}

/// Zoom the window (the bench puts it back after sizing it).
pub fn maximize(_hwnd: *mut std::ffi::c_void) {
    if enabled() {
        with_app(|app| app.window().set_maximized(true));
    }
}

/// Bench only: the client area to `w` x `h` physical pixels, not zoomed.
#[cfg(feature = "bench")]
pub fn resize_client(_hwnd: isize, w: i32, h: i32) {
    if !enabled() {
        return;
    }
    with_app(|app| {
        app.window().set_maximized(false);
        app.window().set_size(slint::PhysicalSize::new(w as u32, h as u32));
    });
}

/// Bench only: the smallest client the window system allows (unknown here: `None`).
#[cfg(feature = "bench")]
pub fn min_client(_hwnd: isize) -> Option<(i32, i32)> {
    None
}

fn with_app<R>(f: impl FnOnce(&crate::AppWindow) -> R) -> Option<R> {
    crate::state::with(|st| st.app.upgrade().map(|app| f(&app))).flatten()
}
