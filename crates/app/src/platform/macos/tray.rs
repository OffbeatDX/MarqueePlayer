//! The tray icon on macOS: a status item in the menu bar, through Slint's `SystemTrayIcon`
//! (the `Tray` component in app.slint; Slint's AppKit backend, `NSStatusItem` and `NSMenu`).
//! A click on a status item opens its menu, as everywhere on macOS: the menu is the same as on
//! the other systems, and a click, double click, middle button or wheel on the icon is never
//! sent (a status item has no wheel or middle button, and its click is the menu's).

use std::time::Duration;

use crate::platform::TrayEvent;

/// The tray icon: the Slint component.
pub struct Tray {
    ui: crate::Tray,
    #[cfg_attr(not(feature = "bench"), allow(dead_code))]
    on: fn(TrayEvent),
}

impl Tray {
    /// Show the icon with `icon` and `tip`; `on` gets what happens on it, on the UI thread.
    pub fn new(icon: slint::Image, tip: &str, on: fn(TrayEvent)) -> Option<Tray> {
        let ui = crate::Tray::new().ok()?;
        ui.set_tray_icon(icon);
        ui.set_tip(tip.into());
        ui.on_toggle_play(move || on(TrayEvent::TogglePlay));
        ui.on_next(move || on(TrayEvent::Next));
        ui.on_previous(move || on(TrayEvent::Previous));
        ui.on_show_window(move || on(TrayEvent::ShowWindow));
        ui.on_about(move || on(TrayEvent::About));
        ui.on_quit(move || on(TrayEvent::Quit));
        // Slint calls it only when the item has no menu; this one always has one.
        ui.on_activated(move || on(TrayEvent::Click));
        if let Err(e) = ui.show() {
            log::warn!("tray: no status item ({e})");
            return None;
        }
        Some(Tray { ui, on })
    }

    pub fn set_icon(&self, img: slint::Image) {
        self.ui.set_tray_icon(img);
    }

    pub fn set_tooltip(&self, tip: &str) {
        self.ui.set_tip(tip.into());
    }

    /// The menu's Play/Pause item.
    pub fn set_playing(&self, playing: bool) {
        self.ui.set_playing(playing);
    }

    /// The bench: what a click would hand the app, straight to it (no input). A status item
    /// has no middle button or wheel, and its double click is two clicks.
    #[cfg(feature = "bench")]
    pub fn simulate(&self, what: crate::platform::TrayPointer) -> bool {
        use crate::platform::TrayPointer as P;
        let on = self.on;
        match what {
            P::Click => on(TrayEvent::Click),
            P::DoubleClick => {
                on(TrayEvent::Click);
                on(TrayEvent::Click);
            }
            P::Middle | P::Wheel(_) | P::Hover => return false,
        }
        true
    }
}

/// The system's double-click time (`NSEvent.doubleClickInterval`, System Settings > Mouse).
pub fn double_click_time() -> Duration {
    let secs = objc2_app_kit::NSEvent::doubleClickInterval();
    Duration::from_secs_f64(secs.clamp(0.1, 5.0))
}

/// Nothing to take out on macOS (Windows' wheel hook).
pub fn release() {}

/// Is the wheel hook in (the bench)? There is none on macOS.
#[cfg(feature = "bench")]
pub fn wheel_hooked() -> bool {
    false
}

/// Is the pointer over the icon (the bench)? Not known on macOS.
#[cfg(feature = "bench")]
pub fn pointer_over_icon() -> Option<bool> {
    None
}
