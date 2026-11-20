//! The tray icon on Linux: a StatusNotifierItem of the app's own, through ksni (the crate
//! Slint's `SystemTrayIcon` uses, with the same features, so nothing new is built). Slint's
//! item only answers `Activate` (its `clicked`); this one also answers `SecondaryActivate`
//! (the middle button on KDE and most hosts) and `Scroll` (the wheel), which the tray's
//! gestures need (`crate::tray`). Its icon, tooltip and menu are the ones Slint's showed:
//! the same pixels (`trayanim`), the same items.
//!
//! ksni runs the item on a thread of its own and calls these methods there; each hands
//! its `TrayEvent` to the UI thread (`slint::invoke_from_event_loop`) and returns. Updates
//! from the UI thread go through `Handle::update`, which ksni turns into the item's
//! change signals.
//!
//! The item's id is `marquee-player` (Slint's was `slint-tray`): KDE's System Tray
//! settings list it under that name.

use std::time::Duration;

use ksni::blocking::TrayMethods;

use crate::platform::TrayEvent;

/// The double-click time: GTK's and Qt's default (the desktops do not publish theirs on a
/// bus every host reads). StatusNotifierItem has no double click: two activations within
/// this time are one (`crate::tray`).
pub fn double_click_time() -> Duration {
    Duration::from_millis(400)
}

/// Nothing to take out on Linux (Windows' wheel hook).
pub fn release() {}

/// Is the wheel hook in (the bench)? There is none on Linux.
#[cfg(feature = "bench")]
pub fn wheel_hooked() -> bool {
    false
}

/// Is the pointer over the icon (the bench)? Not known on Linux.
#[cfg(feature = "bench")]
pub fn pointer_over_icon() -> Option<bool> {
    None
}

fn send(on: fn(TrayEvent), e: TrayEvent) {
    let _ = slint::invoke_from_event_loop(move || on(e));
}

/// Who sends `Scroll`: the hosts differ in what its delta means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollHost {
    /// KDE's panel: Qt's angle delta, 120 a notch, positive up (a touchpad or a fine wheel
    /// sends less per event, down to a few units).
    Qt,
    /// GNOME's AppIndicator extension: Clutter's smooth scroll delta through `Math.floor`
    /// (`appIndicator.js`, `scroll(dx, dy)`), 1 a notch, positive down. (A touchpad's
    /// fractions come out as 0 going down and -1 going up: nothing the item can undo.)
    Clutter,
    /// Another panel: a delta under 30 is taken as Clutter's, else as Qt's.
    Other,
}

/// The host, by the desktop (`XDG_CURRENT_DESKTOP`, "KDE", "ubuntu:GNOME"...).
fn scroll_host() -> ScrollHost {
    let d = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let is = |name: &str| d.split(':').any(|p| p.eq_ignore_ascii_case(name));
    if is("KDE") {
        ScrollHost::Qt
    } else if is("GNOME") {
        ScrollHost::Clutter
    } else {
        ScrollHost::Other
    }
}

/// The wheel as `Scroll` gives it, in 120ths of a notch, positive away from the user.
pub(crate) fn wheel_from_sni(delta: i32, host: ScrollHost) -> i32 {
    let clutter = match host {
        ScrollHost::Qt => false,
        ScrollHost::Clutter => true,
        ScrollHost::Other => delta.abs() < 30,
    };
    if clutter { -delta.saturating_mul(120) } else { delta }
}

struct Item {
    icon: ksni::Icon,
    tip: String,
    playing: bool,
    on: fn(TrayEvent),
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "marquee-player".into()
    }

    fn title(&self) -> String {
        "Marquee Player".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    // Plain text: the title field (the description is the one that takes markup).
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.tip.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        send(self.on, TrayEvent::Click);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        send(self.on, TrayEvent::Middle);
    }

    fn scroll(&mut self, delta: i32, orientation: ksni::Orientation) {
        if orientation == ksni::Orientation::Vertical && delta != 0 {
            send(self.on, TrayEvent::Wheel(wheel_from_sni(delta, scroll_host())));
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        fn item(label: &str, e: TrayEvent) -> ksni::MenuItem<Item> {
            ksni::menu::StandardItem { label: label.into(), activate: Box::new(move |t: &mut Item| send(t.on, e)), ..Default::default() }.into()
        }
        vec![
            item(if self.playing { "Pause" } else { "Play" }, TrayEvent::TogglePlay),
            item("Next", TrayEvent::Next),
            item("Previous", TrayEvent::Previous),
            ksni::MenuItem::Separator,
            item("Show Marquee Player", TrayEvent::ShowWindow),
            item("About Marquee Player", TrayEvent::About),
            item("Quit", TrayEvent::Quit),
        ]
    }
}

/// Slint's image as the ARGB32 pixels (network byte order) StatusNotifierItem wants.
fn to_icon(img: &slint::Image) -> Option<ksni::Icon> {
    let px = img.to_rgba8()?;
    let mut data = px.as_bytes().to_vec();
    for p in data.as_chunks_mut::<4>().0 {
        p.rotate_right(1);
    }
    Some(ksni::Icon { width: px.width() as i32, height: px.height() as i32, data })
}

/// In a Flatpak or Snap sandbox, owning the bus name the specification asks for needs a
/// permission the app may not have; the item then goes without it (as Slint's did).
fn in_sandbox() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
        || std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| {
            c.lines().any(|line| {
                let mut parts = line.splitn(3, ':');
                let (Some(id), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next()) else { return false };
                id.parse::<u32>().is_ok()
                    && matches!(controllers, "" | "freezer" | "name=systemd")
                    && path.rsplit('/').next().is_some_and(|unit| unit.starts_with("snap."))
            })
        })
}

/// The tray icon: the item's handle.
pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
    #[cfg_attr(not(feature = "bench"), allow(dead_code))]
    on: fn(TrayEvent),
}

impl Tray {
    /// Show the icon with `icon` and `tip`; `on` gets what happens on it, on the UI thread.
    pub fn new(icon: slint::Image, tip: &str, on: fn(TrayEvent)) -> Option<Tray> {
        let item = Item { icon: to_icon(&icon)?, tip: tip.to_owned(), playing: false, on };
        // A panel that is not there yet (the app started with the session, before it; a
        // panel restarted, an AppIndicator extension turned on later): the item waits for its
        // StatusNotifierWatcher and registers when one comes. (Whether the window may hide to
        // the tray is asked when it closes: `system::tray_host`.)
        match item.disable_dbus_name(in_sandbox()).assume_sni_available(true).spawn() {
            Ok(handle) => Some(Tray { handle, on }),
            Err(e) => {
                log::warn!("tray: no StatusNotifierItem ({e})");
                None
            }
        }
    }

    pub fn set_icon(&self, img: slint::Image) {
        if let Some(icon) = to_icon(&img) {
            let _ = self.handle.update(move |t: &mut Item| t.icon = icon);
        }
    }

    pub fn set_tooltip(&self, tip: &str) {
        let tip = tip.to_owned();
        let _ = self.handle.update(move |t: &mut Item| t.tip = tip);
    }

    /// The menu's Play/Pause item.
    pub fn set_playing(&self, playing: bool) {
        let _ = self.handle.update(move |t: &mut Item| t.playing = playing);
    }

    /// The bench: what the host would call, straight to the app (no D-Bus, no input).
    #[cfg(feature = "bench")]
    pub fn simulate(&self, what: crate::platform::TrayPointer) -> bool {
        use crate::platform::TrayPointer as P;
        match what {
            P::Click => send(self.on, TrayEvent::Click),
            // Hosts send two activations for a double click.
            P::DoubleClick => {
                send(self.on, TrayEvent::Click);
                send(self.on, TrayEvent::Click);
            }
            P::Middle => send(self.on, TrayEvent::Middle),
            P::Wheel(d) => send(self.on, TrayEvent::Wheel(d)),
            P::Hover => {}
        }
        true
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // The service would otherwise keep the item registered until the process ends.
        let _ = self.handle.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollHost, wheel_from_sni};

    #[test]
    fn scroll_deltas_from_both_kinds_of_host_come_out_in_120ths_up_positive() {
        // KDE: Qt's angle delta, a notch or a touchpad's few units, never turned round.
        assert_eq!(wheel_from_sni(120, ScrollHost::Qt), 120);
        assert_eq!(wheel_from_sni(-240, ScrollHost::Qt), -240);
        assert_eq!(wheel_from_sni(15, ScrollHost::Qt), 15, "a touchpad on KDE scrolls up");
        assert_eq!(wheel_from_sni(-8, ScrollHost::Qt), -8);
        // GNOME's AppIndicator extension: one a notch, down positive.
        assert_eq!(wheel_from_sni(1, ScrollHost::Clutter), -120);
        assert_eq!(wheel_from_sni(-1, ScrollHost::Clutter), 120);
        assert_eq!(wheel_from_sni(-3, ScrollHost::Clutter), 360);
        // Another panel: by the size of the delta.
        assert_eq!(wheel_from_sni(120, ScrollHost::Other), 120);
        assert_eq!(wheel_from_sni(1, ScrollHost::Other), -120);
    }
}
