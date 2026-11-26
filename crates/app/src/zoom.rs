//! Interface size: the whole window zoomed, from 70% to 200%.
//!
//! The zoom is the window's scale factor: Slint draws every length (text, icons, covers,
//! spacing, the caption, menus, dialogs, tooltips) at `logical * scale factor` physical
//! pixels, and the system gives that factor as the monitor's (1.0 at 96 DPI, 1.5 at 144...).
//! Zoomed, the app hands Slint `monitor * zoom` instead (`WindowEvent::ScaleFactorChanged`,
//! then `Resized` with the same physical size in the new logical pixels, as a DPI change
//! does), so nothing in the markup knows about it. A move to a monitor of another DPI keeps
//! the zoom: winit's own scale change is taken here first (`winit_event`) and multiplied.
//!
//! What still works in physical pixels follows the factor: the caption buttons' hit testing
//! on Windows (`platform::windows::frame`, through `factor`), the window's minimum size (the
//! logical minimum shrunk when the zoomed one would not fit the monitor, `min_size`), and the
//! album covers of the grids and shelves, decoded at the size they are drawn
//! (`covers::set_card_px`).
//!
//! Ways in: the keys (`Action::ZoomIn`, `ZoomOut`, `ZoomReset`: Ctrl+= / Ctrl++, Ctrl+-,
//! Ctrl+0 by default), Ctrl and the wheel (a notch a step; Windows sends a precision
//! touchpad's pinch to applications as Ctrl and the wheel), Settings > Window > Interface
//! size, and two fingers on a touchscreen (`Pinch`, from winit's touch events). The value is
//! kept in settings.json (`zoom`, a percentage) and applied as the window is made.
//!
//! The ladder, the clamping and the pinch's arithmetic are pure and unit-tested.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use slint::ComponentHandle;

use crate::state::State;
use crate::{AppWindow, Chrome, views};

pub const MIN: u32 = 70;
pub const MAX: u32 = 200;
pub const DEFAULT: u32 = 100;

/// The steps of the keys, the wheel and Settings' buttons: a browser's, between the limits.
pub const LADDER: [u32; 9] = [70, 80, 90, 100, 110, 125, 150, 175, 200];

/// The window's design minimum, logical pixels at 100% (app.slint's `Chrome.min-*`).
pub const BASE_MIN: (f32, f32) = (960.0, 620.0);

/// A percentage within the limits.
pub fn clamp(pct: u32) -> u32 {
    pct.clamp(MIN, MAX)
}

/// The next step of the ladder above (`dir > 0`) or below `pct`. A value between two steps
/// (left by a pinch) goes to the nearer one in that direction; at a limit it stays.
pub fn step(pct: u32, dir: i32) -> u32 {
    let pct = clamp(pct);
    if dir > 0 {
        LADDER.iter().copied().find(|&s| s > pct).unwrap_or(MAX)
    } else if dir < 0 {
        LADDER.iter().rev().copied().find(|&s| s < pct).unwrap_or(MIN)
    } else {
        pct
    }
}

/// The zoom a pinch asks for: the zoom it started from times how far the fingers moved apart
/// (`ratio`, the distance now over the distance at the start), in steps of 5%, clamped.
pub fn pinch_target(start: u32, ratio: f32) -> u32 {
    if !ratio.is_finite() || ratio <= 0.0 {
        return clamp(start);
    }
    let raw = start as f32 * ratio;
    clamp(((raw / 5.0).round() * 5.0).clamp(0.0, 1000.0) as u32)
}

/// The window's logical minimum at a total scale (monitor times zoom): the design minimum,
/// or less where that many physical pixels would not fit the monitor (`monitor`, physical,
/// with room left for a taskbar and the frame), so the window can always be on the screen.
pub fn min_size(monitor: Option<(u32, u32)>, monitor_scale: f32, total: f32) -> (f32, f32) {
    let (bw, bh) = BASE_MIN;
    let Some((mw, mh)) = monitor else { return (bw, bh) };
    if total <= 0.0 || mw == 0 || mh == 0 {
        return (bw, bh);
    }
    let room_w = (mw as f32 - 16.0 * monitor_scale).max(1.0);
    let room_h = (mh as f32 - 80.0 * monitor_scale).max(1.0);
    (bw.min((room_w / total).floor()), bh.min((room_h / total).floor()))
}

/// The wheel with Ctrl held: notches added up (a touchpad's pinch sends fractions of one), a
/// step each time a whole one is reached, either way.
#[derive(Default, Debug)]
pub struct Wheel {
    acc: f32,
}

impl Wheel {
    /// Lines of the wheel (up positive): the steps to take now (+ in, - out).
    pub fn add(&mut self, lines: f32) -> i32 {
        if !lines.is_finite() {
            return 0;
        }
        // A turn the other way starts again from there.
        if self.acc != 0.0 && self.acc.signum() != lines.signum() {
            self.acc = 0.0;
        }
        self.acc += lines;
        let whole = self.acc.trunc();
        self.acc -= whole;
        whole as i32
    }
}

/// A touch as winit gives it, for `Pinch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// What `Pinch::input` decided about one touch event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PinchOut {
    /// Slint does not see this event (a pinch's fingers, but the first one's end).
    pub consume: bool,
    /// The first finger's press is undone without a click (the second finger came down).
    pub cancel_press: bool,
    /// Zoom to this now (while the fingers move).
    pub zoom: Option<u32>,
    /// The fingers are up: this is the zoom to keep.
    pub commit: Option<u32>,
}

/// Two fingers on the window zoom it. One finger is the page's as ever (Slint makes it the
/// pointer); the moment a second comes down the first one's press is undone without a click
/// (`cancel_press`) and from then on, until every finger is up, Slint sees no touch but the
/// end of that first finger (its own touch state must hear it ended; with the press undone
/// it clicks nothing). So a pinch never scrolls a list or presses what is under it.
///
/// The zoom follows the fingers' distance from the moment it changed by more than
/// `THRESHOLD` (so two fingers merely put down do nothing), in steps of 5%.
#[derive(Default, Debug)]
pub struct Pinch {
    /// Fingers down: id and physical position.
    fingers: Vec<(u64, (f32, f32))>,
    /// The finger Slint saw come down, while it is down.
    slint_finger: Option<u64>,
    /// A pinch is on: from a second finger down until every finger is up.
    active: bool,
    /// Moved past the threshold since the current pair came down.
    zooming: bool,
    start_dist: f32,
    start_zoom: u32,
    last: u32,
    /// The zoom when the pinch began, to tell whether it changed anything.
    began: u32,
}

impl Pinch {
    /// How far (physical pixels) the fingers' distance must change before the zoom follows.
    pub const THRESHOLD: f32 = 16.0;

    #[cfg(any(test, feature = "bench"))]
    pub fn active(&self) -> bool {
        self.active
    }

    fn distance(&self) -> Option<f32> {
        let (a, b) = (self.fingers.first()?.1, self.fingers.get(1)?.1);
        Some(((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt())
    }

    /// A new pair of fingers (the first two down): measured from here, at `zoom`.
    fn rebase(&mut self, zoom: u32) {
        self.start_dist = self.distance().unwrap_or(0.0);
        self.start_zoom = zoom;
        self.zooming = false;
    }

    /// One touch event; `current` is the zoom now.
    pub fn input(&mut self, id: u64, phase: Touch, pos: (f32, f32), current: u32) -> PinchOut {
        let mut out = PinchOut::default();
        match phase {
            Touch::Started => {
                self.fingers.retain(|f| f.0 != id);
                self.fingers.push((id, pos));
                if !self.active {
                    if self.fingers.len() == 1 {
                        self.slint_finger = Some(id);
                        return out;
                    }
                    self.active = true;
                    self.began = current;
                    self.last = current;
                    out.cancel_press = self.slint_finger.is_some();
                    self.rebase(current);
                } else if self.fingers.len() == 2 {
                    // A finger put back during the pinch: on from where it is.
                    self.rebase(self.last);
                }
                out.consume = true;
            }
            Touch::Moved => {
                if let Some(f) = self.fingers.iter_mut().find(|f| f.0 == id) {
                    f.1 = pos;
                }
                if !self.active {
                    return out;
                }
                out.consume = true;
                let is_pair = self.fingers.iter().take(2).any(|f| f.0 == id);
                let Some(d) = self.distance().filter(|_| is_pair) else { return out };
                if !self.zooming {
                    if (d - self.start_dist).abs() <= Self::THRESHOLD {
                        return out;
                    }
                    self.zooming = true;
                    self.start_dist = d;
                    return out;
                }
                if self.start_dist < 1.0 {
                    return out;
                }
                let target = pinch_target(self.start_zoom, d / self.start_dist);
                if target != self.last {
                    self.last = target;
                    out.zoom = Some(target);
                }
            }
            Touch::Ended | Touch::Cancelled => {
                let was_pair = self.fingers.iter().take(2).any(|f| f.0 == id);
                self.fingers.retain(|f| f.0 != id);
                let slint_saw = self.slint_finger == Some(id);
                if slint_saw {
                    self.slint_finger = None;
                }
                if !self.active {
                    return out;
                }
                out.consume = !slint_saw;
                if self.fingers.is_empty() {
                    self.active = false;
                    self.zooming = false;
                    if self.last != self.began {
                        out.commit = Some(self.last);
                    }
                } else if was_pair && self.fingers.len() >= 2 {
                    self.rebase(self.last);
                }
            }
        }
        out
    }
}

// ---- in the app -------------------------------------------------------------------------

/// The zoom applied, percent, for the window procedure (`platform::windows::frame`).
static APPLIED: AtomicU32 = AtomicU32::new(DEFAULT);

/// The zoom as a factor (1.0 at 100%), read by code that works in physical pixels.
pub fn factor() -> f32 {
    APPLIED.load(Ordering::Relaxed) as f32 / 100.0
}

thread_local! {
    static PINCH: RefCell<Pinch> = RefCell::default();
    static WHEEL: RefCell<Wheel> = RefCell::default();
    static CTRL: Cell<bool> = const { Cell::new(false) };
    /// The monitor's own scale factor, as the system last gave it.
    static MONITOR: Cell<Option<f32>> = const { Cell::new(None) };
    /// The minimum wanted at the zoom applied (logical) and the total scale factor.
    static MIN_WANTED: Cell<Option<((f32, f32), f32)>> = const { Cell::new(None) };
    /// The native window last given a minimum, and that minimum (physical).
    static NATIVE_MIN: Cell<Option<(slint::winit_030::winit::window::WindowId, (u32, u32))>> = const { Cell::new(None) };
}

/// The scale factor the system gives the window (the monitor's), or a `SLINT_SCALE_FACTOR`
/// override, which stands in for it.
fn monitor_scale(app: &AppWindow) -> Option<f32> {
    use slint::winit_030::WinitWindowAccessor;
    if let Some(sf) = overridden() {
        return Some(sf);
    }
    MONITOR.with(Cell::get).or_else(|| app.window().with_winit_window(|w| w.scale_factor() as f32))
}

/// `SLINT_SCALE_FACTOR`, read once: Slint then ignores the system's scale factor.
fn overridden() -> Option<f32> {
    static SF: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *SF.get_or_init(|| std::env::var("SLINT_SCALE_FACTOR").ok().and_then(|s| s.parse::<f32>().ok()).filter(|s| *s > 0.0))
}

fn monitor_size(app: &AppWindow) -> Option<(u32, u32)> {
    use slint::winit_030::WinitWindowAccessor;
    app.window().with_winit_window(|w| w.current_monitor().map(|m| (m.size().width, m.size().height))).flatten()
}

/// The zoom kept in the settings, as the window should show it.
fn wanted(st: &State) -> u32 {
    clamp(st.settings.borrow().zoom)
}

/// Give the window the scale factor for `pct` on its monitor: Slint's scale factor, the
/// layout's size in the new logical pixels, the minimum size, the covers' size. Nothing
/// happens before the native window exists (the scale factor is the monitor's until then:
/// `install` applies it then). Idempotent.
pub fn apply(app: &AppWindow, pct: u32) {
    use slint::platform::WindowEvent;
    let pct = clamp(pct);
    let Some(monitor) = monitor_scale(app) else { return };
    APPLIED.store(pct, Ordering::Relaxed);
    let total = monitor * pct as f32 / 100.0;
    let window = app.window();
    if (window.scale_factor() - total).abs() > 1e-4 {
        window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: total });
        let size = window.size();
        if size.width > 0 && size.height > 0 {
            window.dispatch_event(WindowEvent::Resized { size: size.to_logical(total) });
        }
        log::info!("interface size {pct}% (scale factor {total:.3}: the monitor's {monitor:.3})");
    }
    // The minimum: in logical pixels for the layout, and given to the native window at once
    // (Slint passes it on only when the logical value changes, not the scale factor).
    let base = min_size(monitor_size(app), monitor, total);
    MIN_WANTED.with(|m| m.set(Some((base, total))));
    fit_minimum(app);
    // Covers of the grids and shelves at the size they are drawn.
    let card = app.global::<crate::Theme>().get_cover_size();
    crate::covers::set_card_px((card * total).ceil() as u32);
    // The visualizer's anti-aliasing, which depends on the density (`visualizer::aa_policy`).
    crate::visualizer::scale_changed(app, total);
    let chrome = app.global::<Chrome>();
    chrome.set_zoom(pct as i32);
    chrome.set_zoom_text(format!("{pct}%").into());
}

/// The minimum a window this size can have, logical: the wanted one (`want`), or less where
/// the window is smaller and cannot grow (`grows` false: maximized, fullscreen, tiled; or a
/// resize the window system did not take), down to the window's own logical size.
pub fn min_within(want: (f32, f32), window_logical: (f32, f32), grows: bool) -> (f32, f32) {
    if grows { want } else { (want.0.min(window_logical.0.floor()), want.1.min(window_logical.1.floor())) }
}

/// Hold the window to its minimum at the zoom applied. Windows and X11's window managers
/// grow a window that is smaller than its new minimum; on Wayland a minimum only bounds the
/// user's resizes, so a window smaller than the zoomed minimum (zoomed in, or put back to a
/// smaller size by the compositor as it unmaximizes) resizes itself. Where it cannot
/// (maximized, fullscreen, tiled: the compositor decides its size, and the resize does not
/// take), the logical minimum shrinks to what the window has, as `min_size` shrinks it to the
/// monitor, so the layout always fits the window.
fn fit_minimum(app: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let Some((want, total)) = MIN_WANTED.with(Cell::get) else { return };
    let window = app.window();
    // The window system's size, not Slint's: as the window comes up on a compositor with a
    // fractional scale, Slint still has the size made at scale 1 when the zoom first applies.
    let size = window.with_winit_window(|w| w.inner_size()).map_or_else(|| window.size(), |s| slint::PhysicalSize::new(s.width, s.height));
    let need = ((want.0 * total).round() as u32, (want.1 * total).round() as u32);
    let small = size.width > 0 && size.height > 0 && (size.width + 1 < need.0 || size.height + 1 < need.1);
    let mut eff = want;
    if small {
        let fixed = window.with_winit_window(|w| w.is_maximized() || w.fullscreen().is_some()).unwrap_or(false);
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some() && cfg!(target_os = "linux");
        let mut grows = !fixed;
        if grows {
            window.set_size(slint::PhysicalSize::new(size.width.max(need.0), size.height.max(need.1)));
            // Wayland applies a client's resize at once, or not at all (a tiled window):
            // what the window has now says which. Elsewhere it comes back as a resize.
            if wayland {
                let now = window.with_winit_window(|w| w.inner_size()).map_or((size.width, size.height), |s| (s.width, s.height));
                grows = now.0 + 1 >= need.0 && now.1 + 1 >= need.1;
            }
        }
        let now = window.with_winit_window(|w| w.inner_size()).map_or_else(|| window.size(), |s| slint::PhysicalSize::new(s.width, s.height));
        let logical = now.to_logical(total);
        eff = min_within(want, (logical.width, logical.height), grows);
        log::info!(
            "window {}x{} px under its minimum at this size ({}x{} px): {}; minimum {:.0}x{:.0} logical",
            size.width,
            size.height,
            need.0,
            need.1,
            if grows { "resized" } else { "it cannot grow" },
            eff.0,
            eff.1
        );
    }
    let chrome = app.global::<Chrome>();
    chrome.set_min_width(eff.0);
    chrome.set_min_height(eff.1);
    // Given to the native window only when it changes: on Windows winit answers every
    // minimum with a resize to the size the window has, and that resize, measured with a
    // caption the frameless window does not have, comes back taller, so a minimum given on
    // each resize grew the window without end.
    window.with_winit_window(|w| {
        let phys = ((eff.0 * total).round() as u32, (eff.1 * total).round() as u32);
        if NATIVE_MIN.with(|m| needs_giving(m, w.id(), phys)) {
            w.set_min_inner_size(Some(slint::winit_030::winit::dpi::PhysicalSize::new(phys.0, phys.1)));
        }
    });
}

/// Whether the native minimum `phys` must be given to `window`: not when that window was
/// last given the same one. Records it.
fn needs_giving<W: Copy + PartialEq>(last: &Cell<Option<(W, (u32, u32))>>, window: W, phys: (u32, u32)) -> bool {
    if last.get() == Some((window, phys)) {
        return false;
    }
    last.set(Some((window, phys)));
    true
}

/// Change the zoom: applied, kept in the settings (`save`; a pinch saves when the fingers
/// lift), said in the pill.
pub fn set(st: &Rc<State>, pct: u32, save: bool) {
    let pct = clamp(pct);
    let Some(app) = st.app.upgrade() else { return };
    let changed = {
        let mut s = st.settings.borrow_mut();
        let changed = s.zoom != pct;
        s.zoom = pct;
        changed
    };
    apply(&app, pct);
    if save && changed {
        st.save_settings_soon();
    }
    views::flash(st, &format!("Interface size {pct}%"));
}

/// The keys and Settings' buttons: a step of the ladder (`dir` +1 or -1), or 0 for 100%.
pub fn step_by(st: &Rc<State>, dir: i32) {
    let now = wanted(st);
    let next = if dir == 0 { DEFAULT } else { step(now, dir) };
    set(st, next, true);
}

/// The window exists: the saved zoom applies (called as the native window is made, and
/// again at the window's first events, whichever comes first).
pub fn install(app: &AppWindow) {
    let pct = crate::state::with(|st| wanted(st)).unwrap_or(DEFAULT);
    apply(app, pct);
}

/// The pinch's touch events (winit's, through the window hook; the bench calls it with
/// fingers of its own). True when Slint must not see the event.
pub fn touch(app: &AppWindow, id: u64, phase: Touch, pos: (f32, f32)) -> bool {
    let current = APPLIED.load(Ordering::Relaxed);
    let out = PINCH.with(|p| p.borrow_mut().input(id, phase, pos, current));
    if out.cancel_press {
        // The first finger's press, undone: no click, no drag, no scroll of a list.
        app.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
    }
    if let Some(z) = out.zoom {
        crate::state::with(|st| set(st, z, false));
    }
    if let Some(z) = out.commit {
        crate::state::with(|st| set(st, z, true));
    }
    out.consume
}

/// Whether a pinch is on (for the bench).
#[cfg(feature = "bench")]
pub fn pinching() -> bool {
    PINCH.with(|p| p.borrow().active())
}

/// Ctrl and the wheel (lines, up positive): steps of the ladder. True when it was taken.
pub fn wheel(st: &Rc<State>, lines: f32) -> bool {
    let steps = WHEEL.with(|w| w.borrow_mut().add(lines));
    if steps != 0 {
        let mut pct = wanted(st);
        for _ in 0..steps.unsigned_abs() {
            pct = step(pct, steps.signum());
        }
        set(st, pct, true);
    }
    true
}

/// The system gave the window another scale factor (it moved to another monitor, or the
/// display's scaling changed): that times the zoom. Slint is not given the system's factor
/// then (true: the event is taken); the size winit suggests (the same logical size on the new
/// monitor) stands, and its Resized comes next. With `SLINT_SCALE_FACTOR` set Slint ignores it
/// anyway.
pub fn monitor_changed(app: &AppWindow, scale_factor: f32) -> bool {
    MONITOR.with(|m| m.set(Some(scale_factor)));
    if overridden().is_some() {
        return false;
    }
    apply(app, APPLIED.load(Ordering::Relaxed));
    true
}

/// What the window hook (`autoscroll::install`, which holds winit's one event filter) lets
/// this module take first: a change of the monitor's scale factor (kept, multiplied by the
/// zoom), Ctrl and the wheel, touches of a pinch. True when Slint must not see the event.
pub fn winit_event(app: &AppWindow, window: &slint::Window, ev: &slint::winit_030::winit::event::WindowEvent) -> bool {
    use slint::winit_030::winit::event::{MouseScrollDelta, TouchPhase, WindowEvent};
    // The saved zoom, at the window's first event (before the native-window poll gets there);
    // and again should anything have given Slint the monitor's own factor since (Slint does
    // as the native window is made). A comparison per event, nothing more.
    match MONITOR.with(Cell::get) {
        None => {
            use slint::winit_030::WinitWindowAccessor;
            if let Some(sf) = window.with_winit_window(|w| w.scale_factor() as f32) {
                MONITOR.with(|m| m.set(Some(sf)));
                install(app);
            }
        }
        Some(sf) => {
            let want = overridden().unwrap_or(sf) * factor();
            if (window.scale_factor() - want).abs() > 1e-4 {
                apply(app, APPLIED.load(Ordering::Relaxed));
            }
        }
    }
    match ev {
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => monitor_changed(app, *scale_factor as f32),
        // A size the window system chose (unmaximized, tiled, restored): held to the minimum
        // once Slint has it.
        WindowEvent::Resized(_) => {
            let weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(app) = weak.upgrade() {
                    fit_minimum(&app);
                }
            });
            false
        }
        WindowEvent::ModifiersChanged(m) => {
            CTRL.with(|c| c.set(m.state().control_key()));
            false
        }
        WindowEvent::MouseWheel { delta, .. } if CTRL.with(Cell::get) => {
            let lines = match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                // A touchpad's pixels (Linux, macOS): about a notch per 60 physical pixels.
                MouseScrollDelta::PixelDelta(p) => (p.y / 60.0) as f32,
            };
            crate::state::with(|st| wheel(st, lines)).unwrap_or(false)
        }
        WindowEvent::Touch(t) => {
            let phase = match t.phase {
                TouchPhase::Started => Touch::Started,
                TouchPhase::Moved => Touch::Moved,
                TouchPhase::Ended => Touch::Ended,
                TouchPhase::Cancelled => Touch::Cancelled,
            };
            touch(app, t.id, phase, (t.location.x as f32, t.location.y as f32))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_steps_and_stops_at_its_ends() {
        assert_eq!(step(100, 1), 110);
        assert_eq!(step(110, 1), 125);
        assert_eq!(step(125, 1), 150);
        assert_eq!(step(175, 1), 200);
        assert_eq!(step(200, 1), 200, "at the top it stays");
        assert_eq!(step(100, -1), 90);
        assert_eq!(step(80, -1), 70);
        assert_eq!(step(70, -1), 70, "at the bottom it stays");
        assert_eq!(step(130, 0), 130);
        // Between two steps (a pinch left it there): the next one that way.
        assert_eq!(step(115, 1), 125);
        assert_eq!(step(115, -1), 110);
        assert_eq!(step(105, -1), 100);
        // Every step up and back down again is the ladder itself.
        let mut z = MIN;
        let mut up = vec![z];
        while step(z, 1) != z {
            z = step(z, 1);
            up.push(z);
        }
        assert_eq!(up, LADDER.to_vec());
        assert!(LADDER.windows(2).all(|w| w[0] < w[1]));
        assert_eq!((LADDER[0], LADDER[LADDER.len() - 1]), (MIN, MAX));
        assert!(LADDER.contains(&DEFAULT));
    }

    #[test]
    fn a_window_that_cannot_grow_gets_a_minimum_it_has() {
        // Maximized or tiled at 200% on a 1920 x 1048 output: 960 x 524 logical.
        assert_eq!(min_within((952.0, 500.0), (960.0, 524.0), false), (952.0, 500.0), "it fits: as wanted");
        assert_eq!(min_within((952.0, 500.0), (720.0, 450.5), false), (720.0, 450.0), "smaller: what it has");
        assert_eq!(min_within((952.0, 500.0), (720.0, 450.0), true), (952.0, 500.0), "it grows: as wanted");
    }

    #[test]
    fn a_resize_that_changes_no_minimum_gives_the_window_none() {
        // Each resize asks again; only a new minimum, or a new window, is given one (on
        // Windows giving one resizes the window, and that resize asks again).
        let last = Cell::new(None);
        assert!(needs_giving(&last, 1, (952, 500)), "the first");
        assert!(!needs_giving(&last, 1, (952, 500)), "the same again: nothing");
        assert!(!needs_giving(&last, 1, (952, 500)), "and again");
        assert!(needs_giving(&last, 1, (1904, 1000)), "zoomed: the new one");
        assert!(needs_giving(&last, 2, (1904, 1000)), "a new window: given it");
        assert!(!needs_giving(&last, 2, (1904, 1000)));
    }

    #[test]
    fn values_out_of_range_are_clamped() {
        assert_eq!(clamp(0), MIN);
        assert_eq!(clamp(69), 70);
        assert_eq!(clamp(100), 100);
        assert_eq!(clamp(201), 200);
        assert_eq!(clamp(u32::MAX), MAX);
        // From outside the range, a step starts at the limit.
        assert_eq!(step(5, 1), 80);
        assert_eq!(step(900, -1), 175);
    }

    #[test]
    fn a_pinch_scales_from_where_it_started_in_steps_of_five() {
        assert_eq!(pinch_target(100, 1.0), 100);
        assert_eq!(pinch_target(100, 1.5), 150);
        assert_eq!(pinch_target(100, 1.12), 110);
        assert_eq!(pinch_target(100, 1.13), 115);
        assert_eq!(pinch_target(150, 0.5), 75);
        assert_eq!(pinch_target(100, 0.2), MIN, "clamped below");
        assert_eq!(pinch_target(100, 9.0), MAX, "and above");
        assert_eq!(pinch_target(100, f32::NAN), 100);
        assert_eq!(pinch_target(100, 0.0), 100);
        assert_eq!(pinch_target(500, 1.0), MAX);
    }

    #[test]
    fn two_fingers_zoom_and_commit_when_they_lift() {
        let mut p = Pinch::default();
        // One finger: the page's.
        let o = p.input(1, Touch::Started, (100.0, 100.0), 100);
        assert_eq!(o, PinchOut::default());
        assert!(!p.input(1, Touch::Moved, (110.0, 100.0), 100).consume);
        // The second: the first one's press is undone, and from now on Slint sees nothing.
        let o = p.input(2, Touch::Started, (310.0, 100.0), 100);
        assert!(o.consume && o.cancel_press && o.zoom.is_none());
        assert!(p.active());
        // Within the threshold: nothing yet.
        let o = p.input(2, Touch::Moved, (320.0, 100.0), 100);
        assert!(o.consume && o.zoom.is_none());
        // Past it: measured from there (210 px apart), then 1.5 times as far apart.
        let o = p.input(2, Touch::Moved, (340.0, 100.0), 100);
        assert!(o.consume && o.zoom.is_none(), "{o:?}");
        let o = p.input(2, Touch::Moved, (110.0 + 230.0 * 1.5, 100.0), 100);
        assert_eq!(o.zoom, Some(150));
        // The same step again says nothing new.
        let o = p.input(1, Touch::Moved, (110.5, 100.0), 150);
        assert_eq!(o.zoom, None);
        // The first finger lifts: Slint hears its end (its press is gone: no click); no
        // commit while a finger is down.
        let o = p.input(1, Touch::Ended, (110.5, 100.0), 150);
        assert!(!o.consume && o.commit.is_none());
        // Moves of the last finger are not the page's.
        assert!(p.input(2, Touch::Moved, (500.0, 100.0), 150).consume);
        let o = p.input(2, Touch::Ended, (500.0, 100.0), 150);
        assert!(o.consume);
        assert_eq!(o.commit, Some(150));
        assert!(!p.active());
        // Then a single finger is the page's again.
        let o = p.input(3, Touch::Started, (50.0, 50.0), 150);
        assert!(!o.consume && !o.cancel_press);
        assert!(!p.input(3, Touch::Ended, (50.0, 50.0), 150).consume);
    }

    #[test]
    fn pinching_in_zooms_out_and_a_pinch_that_changed_nothing_commits_nothing() {
        let mut p = Pinch::default();
        p.input(1, Touch::Started, (0.0, 0.0), 100);
        p.input(2, Touch::Started, (400.0, 0.0), 100);
        // Past the threshold, re-measured at 380.
        assert_eq!(p.input(2, Touch::Moved, (380.0, 0.0), 100).zoom, None);
        assert_eq!(p.input(2, Touch::Moved, (190.0, 0.0), 100).zoom, Some(MIN), "half as far: 50%, clamped to 70%");
        assert_eq!(p.input(2, Touch::Moved, (380.0 * 0.8, 0.0), 70).zoom, Some(80));
        p.input(2, Touch::Ended, (304.0, 0.0), 80);
        assert_eq!(p.input(1, Touch::Ended, (0.0, 0.0), 80).commit, Some(80));
        // Two fingers put down and lifted without moving: no change, nothing to save.
        p.input(5, Touch::Started, (0.0, 0.0), 80);
        p.input(6, Touch::Started, (100.0, 0.0), 80);
        p.input(6, Touch::Moved, (104.0, 0.0), 80);
        assert_eq!(p.input(6, Touch::Ended, (104.0, 0.0), 80), PinchOut { consume: true, ..Default::default() });
        let o = p.input(5, Touch::Ended, (0.0, 0.0), 80);
        assert_eq!(o.commit, None);
        assert!(!o.consume, "the finger Slint saw ends for Slint");
    }

    #[test]
    fn a_cancelled_touch_ends_the_pinch_like_a_lift() {
        let mut p = Pinch::default();
        p.input(1, Touch::Started, (0.0, 0.0), 100);
        p.input(2, Touch::Started, (200.0, 0.0), 100);
        p.input(2, Touch::Moved, (230.0, 0.0), 100);
        assert_eq!(p.input(2, Touch::Moved, (230.0 * 1.25, 0.0), 100).zoom, Some(125));
        assert!(!p.input(1, Touch::Cancelled, (0.0, 0.0), 125).consume);
        let o = p.input(2, Touch::Cancelled, (287.0, 0.0), 125);
        assert_eq!(o.commit, Some(125));
        assert!(!p.active());
    }

    #[test]
    fn the_minimum_size_fits_the_monitor() {
        // 1080p at 100%: the design minimum up to 1.6 or so, then smaller.
        assert_eq!(min_size(Some((1920, 1080)), 1.0, 1.0), BASE_MIN);
        assert_eq!(min_size(Some((1920, 1080)), 1.0, 1.5), (960.0, 620.0));
        let (w, h) = min_size(Some((1920, 1080)), 1.0, 2.0);
        assert_eq!((w, h), (952.0, 500.0));
        assert!(w * 2.0 <= 1920.0 - 16.0 && h * 2.0 <= 1080.0 - 80.0);
        // A 4K monitor at 150% zoomed to 200% (a total of 3): fits as well.
        let (w, h) = min_size(Some((3840, 2160)), 1.5, 3.0);
        assert!(w * 3.0 <= 3840.0 && h * 3.0 <= 2160.0 - 120.0, "{w} x {h}");
        // At 70% the design minimum stays (smaller in physical pixels, never larger).
        assert_eq!(min_size(Some((1366, 768)), 1.0, 0.7), BASE_MIN);
        // No monitor known: the design minimum.
        assert_eq!(min_size(None, 1.0, 2.0), BASE_MIN);
    }

    #[test]
    fn the_wheel_steps_once_a_notch_and_adds_up_fractions() {
        let mut w = Wheel::default();
        assert_eq!(w.add(1.0), 1);
        assert_eq!(w.add(-1.0), -1);
        assert_eq!(w.add(3.0), 3);
        // A touchpad's pinch: fractions of a notch.
        assert_eq!(w.add(0.4), 0);
        assert_eq!(w.add(0.4), 0);
        assert_eq!(w.add(0.4), 1);
        // The other way starts again from nothing.
        assert_eq!(w.add(-0.5), 0);
        assert_eq!(w.add(-0.6), -1);
        assert_eq!(w.add(f32::NAN), 0);
    }
}
