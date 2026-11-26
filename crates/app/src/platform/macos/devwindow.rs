//! `MP_WINDOW=secondary` on macOS: test windows out of the way, as on Windows. The window
//! opens on the second display when there is one (else the main one), zoomed to fill it,
//! and is never made key or brought forward, so the app the owner works in keeps the focus.
//! The bench sizes the window itself (`resize_client`).

use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
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

/// The backend for a test window: winit told not to activate the application when it has
/// finished launching. An inactive window (`attributes`) is not enough on macOS: winit's
/// default is to activate the app at launch over whatever is in front, which took the
/// keyboard from the owner's own work at every bench start.
pub fn backend(s: slint::BackendSelector) -> slint::BackendSelector {
    use winit::platform::macos::EventLoopBuilderExtMacOS;
    let mut b = winit::event_loop::EventLoop::<slint::winit_030::SlintEvent>::with_user_event();
    b.with_activate_ignoring_other_apps(false);
    s.with_winit_event_loop_builder(b)
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

/// Parse `MP_WINDOW_SLOT=k/n` (`0 <= k < n`, `n > 1`), set by `bench-runner` when several
/// bench processes share a display in parallel.
fn parse_slot(raw: &str) -> Option<(usize, usize)> {
    let (k, n) = raw.split_once('/')?;
    let (k, n): (usize, usize) = (k.parse().ok()?, n.parse().ok()?);
    (n > 1 && k < n).then_some((k, n))
}

fn slot() -> Option<(usize, usize)> {
    parse_slot(&std::env::var("MP_WINDOW_SLOT").ok()?)
}

/// Pure staggered rectangle inside `(x, y, w, h)` for slot `k` of `n` (`n > 1`):
/// positioned at `(x + k * d, y + (n - 1 - k) * d)` with size `(w - (n - 1) * d, h - (n - 1) * d)`.
/// Every slot's top-left `d x d` corner is strictly left of every higher slot and strictly
/// above every lower slot, so no subset of the other slots can fully cover it regardless of
/// z-order or later `resize_client` calls, preventing macOS `WindowEvent::Occluded(true)`.
fn slot_geometry(area: (f64, f64, f64, f64), k: usize, n: usize) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = area;
    let gaps = (n - 1) as f64;
    let d = ((w - 960.0).min(h - 620.0) / gaps).clamp(2.0, 8.0);
    let sx = x + k as f64 * d;
    let sy = y + (n - 1 - k) as f64 * d;
    let sw = (w - gaps * d).max(1.0);
    let sh = (h - gaps * d).max(1.0);
    (sx, sy, sw, sh)
}

/// The target display's visible area in winit logical coordinates: the second display when
/// there is one, else the main one.
fn target_area() -> Option<(f64, f64, f64, f64)> {
    let mtm = objc2::MainThreadMarker::new()?;
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    if screens.count() == 0 {
        return None;
    }
    let main_h = screens.objectAtIndex(0).frame().size.height;
    let i = if screens.count() >= 2 { 1 } else { 0 };
    let v = screens.objectAtIndex(i).visibleFrame();
    Some((v.origin.x, main_h - (v.origin.y + v.size.height), v.size.width, v.size.height))
}

fn slot_rect() -> Option<(f64, f64, f64, f64)> {
    let (k, n) = slot()?;
    Some(slot_geometry(target_area()?, k, n))
}

/// The window attributes for `MP_WINDOW=secondary`: the usual ones, inactive, on the
/// second display's area when there is one, else zoomed on the main one. With
/// `MP_WINDOW_SLOT=k/n`, placed in a non-occluding staggered slot on that display.
pub fn attributes(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    let attrs = super::window::attributes(attrs).with_active(false);
    if let Some((x, y, w, h)) = slot_rect() {
        log::info!("MP_WINDOW=secondary: staggered slot, {w:.0}x{h:.0} at {x:.0},{y:.0}");
        return attrs.with_position(winit::dpi::LogicalPosition::new(x, y)).with_inner_size(winit::dpi::LogicalSize::new(w, h));
    }
    match secondary_area() {
        Some((x, y, w, h)) => {
            log::info!("MP_WINDOW=secondary: the second display's area, {w}x{h} at {x},{y}");
            attrs.with_position(winit::dpi::LogicalPosition::new(x, y)).with_inner_size(winit::dpi::LogicalSize::new(w, h))
        }
        None => attrs.with_maximized(true),
    }
}

/// Zoom the window (the bench puts it back after sizing it). Under `MP_WINDOW_SLOT`, return
/// to the slot's rectangle instead of full zoom so parallel windows stay uncovered.
pub fn maximize(_hwnd: *mut std::ffi::c_void) {
    if !enabled() {
        return;
    }
    if let Some((x, y, w, h)) = slot_rect() {
        with_app(|app| {
            app.window().set_maximized(false);
            app.window().set_size(slint::LogicalSize::new(w as f32, h as f32));
            app.window().with_winit_window(|win| {
                win.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
            });
        });
        return;
    }
    with_app(|app| app.window().set_maximized(true));
}

/// Bench only: the client area to `w` x `h` physical pixels, not zoomed.
#[cfg(feature = "bench")]
pub fn resize_client(_hwnd: isize, w: i32, h: i32) {
    if !enabled() {
        return;
    }
    let pos = slot_rect().map(|(x, y, _, _)| (x, y));
    with_app(|app| {
        app.window().set_maximized(false);
        app.window().set_size(slint::PhysicalSize::new(w as u32, h as u32));
        if let Some((x, y)) = pos {
            app.window().with_winit_window(|win| {
                win.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_staggered_slot_keeps_its_top_left_corner_uncovered_by_all_others() {
        let area = (0.0, 32.0, 1470.0, 922.0);
        for n in [2usize, 4, 6, 8] {
            let rects: Vec<_> = (0..n).map(|k| slot_geometry(area, k, n)).collect();
            for (k, &(x, y, w, h)) in rects.iter().enumerate() {
                assert!(w >= 960.0 && h >= 620.0, "slot {k}/{n} stayed above minimum: {w}x{h}");
                assert!(x >= area.0 && y >= area.1 && x + w <= area.0 + area.2 + 1e-6 && y + h <= area.1 + area.3 + 1e-6);
                let (px, py) = (x + 1.0, y + 1.0);
                for (j, &(ox, oy, ow, oh)) in rects.iter().enumerate() {
                    if j == k {
                        continue;
                    }
                    let covered = px >= ox && px <= ox + ow && py >= oy && py <= oy + oh;
                    assert!(!covered, "slot {j}/{n} covered slot {k}/{n}'s top-left corner ({px}, {py})");
                }
            }
        }
    }

    #[test]
    fn slot_parser_accepts_valid_pairs_and_rejects_noise() {
        assert_eq!(parse_slot("0/4"), Some((0, 4)));
        assert_eq!(parse_slot("3/4"), Some((3, 4)));
        assert_eq!(parse_slot("4/4"), None);
        assert_eq!(parse_slot("0/1"), None);
        assert_eq!(parse_slot("bad"), None);
    }
}
