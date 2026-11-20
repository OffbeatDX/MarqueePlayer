//! `MP_WINDOW=secondary` on Linux. On Windows it keeps test windows on another monitor and
//! out of the foreground; under the headless compositor the Linux tests run in there is no
//! one to disturb and no second monitor, so here it only means what the bench needs: the
//! window opens maximized and is never asked for the focus. The bench sizes the window
//! itself (`resize_client`).

use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Bumped by every `resize_client`: a fill of the screen asked before it does not undo it.
static SIZED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// True when `MP_WINDOW=secondary` (read once, at start).
pub fn enabled() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn init() {
    let on = std::env::var("MP_WINDOW").is_ok_and(|v| v.eq_ignore_ascii_case("secondary"));
    ACTIVE.store(on, Ordering::Relaxed);
    if on {
        log::info!("MP_WINDOW=secondary: maximized, never asked for the focus (no monitor placement on Linux)");
    }
}

/// The window attributes for `MP_WINDOW=secondary`: the usual ones, maximized, inactive.
pub fn attributes(attrs: slint::winit_030::winit::window::WindowAttributes) -> slint::winit_030::winit::window::WindowAttributes {
    super::window::attributes(attrs).with_active(false).with_maximized(true)
}

/// Maximize the window (the bench puts it back after sizing it).
pub fn maximize(_hwnd: *mut std::ffi::c_void) {
    if enabled() {
        with_app(|app| app.window().set_maximized(true));
        // A compositor may ignore a maximize that comes before it first configured the
        // window (Weston does): ask again a moment later.
        slint::Timer::single_shot(std::time::Duration::from_millis(300), || {
            with_app(|app| app.window().set_maximized(true));
        });
        // X11 with no window manager (the headless session's Xvfb): nothing maximizes a
        // window there, and the bench's pages would be laid out for 1440 x 900 on X11 but for
        // the whole screen on Wayland. The window fills its monitor itself.
        let sized = SIZED.load(Ordering::Relaxed);
        slint::Timer::single_shot(std::time::Duration::from_millis(700), move || {
            use slint::winit_030::WinitWindowAccessor;
            with_app(|app| {
                app.window().with_winit_window(|w| {
                    let x11 = std::env::var_os("WAYLAND_DISPLAY").is_none();
                    if x11 && !w.is_maximized() && SIZED.load(Ordering::Relaxed) == sized
                        && let Some(m) = w.current_monitor()
                    {
                        log::info!("MP_WINDOW=secondary: not maximized (no window manager): filling the screen, {}x{}", m.size().width, m.size().height);
                        w.set_outer_position(m.position());
                        let _ = w.request_inner_size(m.size());
                    }
                })
            });
        });
    }
}

/// Bench only: the client area to `w` x `h` physical pixels, not maximized.
#[cfg(feature = "bench")]
pub fn resize_client(_hwnd: isize, w: i32, h: i32) {
    if !enabled() {
        return;
    }
    SIZED.fetch_add(1, Ordering::Relaxed);
    // Through Slint: on Wayland winit applies a size at once and sends no Resized event,
    // which Slint's own call accounts for.
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
