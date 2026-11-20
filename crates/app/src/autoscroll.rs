//! Middle-click autoscroll, as Chrome, Edge and Firefox do it on Windows.
//!
//! A middle click on something that scrolls turns the mode on and leaves an anchor mark
//! where it was; the content then scrolls toward the side the pointer is on, faster the
//! further it is from the anchor, with a dead zone around the anchor where nothing moves.
//! Any mouse button, the wheel or Esc turns it off (the press that does is not passed on),
//! and so does going elsewhere: a page change, the window losing focus. Held form: press,
//! drag, release: the release ends it if the pointer went beyond the dead zone while the
//! button was held; a quick click without moving leaves it on.
//!
//! Split three ways:
//!
//! - [`Machine`] is the mode, pure and unit-tested: inputs in, what to do out.
//! - [`speed`] is the curve, also pure: gentle just past the dead zone, accelerating with
//!   distance (a power law, `[autoscroll]` in theme.toml).
//! - The UI (`Autoscroll` in stores.slint, `ScrollDriver` / `AutoFlickable` in
//!   widgets.slint) finds the area under the press and moves it. A press bumps
//!   `Autoscroll.probe`; every scroll area under the anchor claims the axes it can scroll,
//!   the innermost (smallest) on the highest layer winning each axis, so a horizontal shelf
//!   takes the horizontal axis and the page around it the vertical one, as in browsers.
//!   Panels over the page (sidebars, the player bar, the pill) block what is under them.
//!   The winning area then scrolls itself on the frame clock (`animation-tick()`) at the
//!   speed set here, only while it has somewhere to go: stopped at an end, in the dead zone
//!   or with the mode off, nothing runs.
//!
//! Input comes from winit (`install`), before Slint sees it, so a middle press never
//! reaches the elements under it (no album opens, no drag starts, no window move) and the
//! press that ends the mode is not a click. The bench feeds the same `handle` and
//! dispatches what it lets through.

use std::cell::{Cell, RefCell};

use slint::ComponentHandle;

use crate::{AppWindow, MenuStore};

/// The curve and the dead zone (theme.toml `[autoscroll]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Logical px around the anchor where nothing moves, on each axis; also how far the
    /// pointer must go while the button is held for the release to end the mode.
    pub dead_zone: f32,
    /// Px per second 100 px beyond the dead zone.
    pub speed: f32,
    /// How fast the speed grows with distance (1 is linear).
    pub exponent: f32,
    /// The fastest it goes, px per second.
    pub max_speed: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { dead_zone: 15.0, speed: 600.0, exponent: 1.8, max_speed: 12_000.0 }
    }
}

/// Speed (logical px per second) for the pointer `offset` px from the anchor on one axis.
/// Positive scrolls toward the end of the content (right, down).
pub fn speed(cfg: &Config, offset: f32) -> f32 {
    let beyond = offset.abs() - cfg.dead_zone;
    if beyond <= 0.0 || !beyond.is_finite() {
        return 0.0;
    }
    let v = (cfg.speed * (beyond / 100.0).powf(cfg.exponent.max(0.1))).min(cfg.max_speed);
    v.copysign(offset)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    Other,
}

impl Button {
    fn bit(self) -> u8 {
        match self {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
            Button::Other => 8,
        }
    }
}

/// What reaches the window (positions in logical px, window coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Press(Button, f32, f32),
    Release(Button, f32, f32),
    Move(f32, f32),
    Wheel,
    Escape,
    EscapeUp,
    /// The window lost the keyboard focus (deactivated, hidden, minimized).
    FocusLost,
    /// Something took the user elsewhere: a page change, another Settings section.
    Away,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// The middle button is still down from the press that started it.
    Held { moved: bool },
    /// Clicked on; stays until something ends it.
    Toggled,
}

/// What `Machine::input` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Keep the event from the UI.
    pub consume: bool,
    /// Turn the mode on with the anchor here: find the area under it.
    pub start: Option<(f32, f32)>,
    /// Turn the mode off.
    pub stop: bool,
    /// The new speed (px per second on each axis).
    pub velocity: Option<(f32, f32)>,
}

/// The mode. `engaged` (the UI found something to scroll under the press) matters only
/// for the presses, the wheel and Esc that would end it: a middle click on something that
/// does not scroll does nothing, and the next click must reach the UI as usual.
#[derive(Debug)]
pub struct Machine {
    pub mode: Mode,
    anchor: (f32, f32),
    pointer: (f32, f32),
    /// Buttons whose release is not passed on (their press was not).
    swallow: u8,
    swallow_esc: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Self { mode: Mode::Off, anchor: (0.0, 0.0), pointer: (0.0, 0.0), swallow: 0, swallow_esc: false }
    }
}

impl Machine {
    pub fn is_on(&self) -> bool {
        self.mode != Mode::Off
    }

    pub fn pointer(&self) -> (f32, f32) {
        self.pointer
    }

    fn velocity(&self, cfg: &Config) -> (f32, f32) {
        (speed(cfg, self.pointer.0 - self.anchor.0), speed(cfg, self.pointer.1 - self.anchor.1))
    }

    fn off(&mut self, out: &mut Outcome) {
        if self.mode != Mode::Off {
            self.mode = Mode::Off;
            out.stop = true;
            out.velocity = Some((0.0, 0.0));
        }
    }

    /// `enabled`: the setting; `menu_open`: a context menu is up (a middle press then goes
    /// to the menu, which closes on any press, as before).
    pub fn input(&mut self, ev: Input, cfg: &Config, engaged: bool, enabled: bool, menu_open: bool) -> Outcome {
        let mut out = Outcome::default();
        // The press that started it found nothing to scroll: as if it never happened.
        if self.is_on() && !engaged && matches!(ev, Input::Press(..) | Input::Wheel | Input::Escape) {
            self.off(&mut out);
        }
        match ev {
            Input::Move(x, y) => {
                self.pointer = (x, y);
                if let Mode::Held { moved } = &mut self.mode {
                    let (dx, dy) = (x - self.anchor.0, y - self.anchor.1);
                    if dx.abs() > cfg.dead_zone || dy.abs() > cfg.dead_zone {
                        *moved = true;
                    }
                }
                if self.is_on() {
                    out.velocity = Some(self.velocity(cfg));
                }
            }
            Input::Press(b, x, y) => {
                self.pointer = (x, y);
                if self.is_on() {
                    // Any button ends it, and that press is not a click.
                    self.off(&mut out);
                    self.swallow |= b.bit();
                    out.consume = true;
                } else if b == Button::Middle && enabled && !menu_open {
                    self.mode = Mode::Held { moved: false };
                    self.anchor = (x, y);
                    self.swallow |= b.bit();
                    out.consume = true;
                    out.start = Some((x, y));
                    out.velocity = Some((0.0, 0.0));
                }
            }
            Input::Release(b, x, y) => {
                self.pointer = (x, y);
                if b == Button::Middle
                    && let Mode::Held { moved } = self.mode
                {
                    if moved || (x - self.anchor.0).abs() > cfg.dead_zone || (y - self.anchor.1).abs() > cfg.dead_zone {
                        self.off(&mut out);
                    } else {
                        self.mode = Mode::Toggled;
                    }
                }
                if self.swallow & b.bit() != 0 {
                    self.swallow &= !b.bit();
                    out.consume = true;
                }
            }
            Input::Wheel => {
                if self.is_on() {
                    self.off(&mut out);
                    out.consume = true;
                }
            }
            Input::Escape => {
                if self.is_on() {
                    self.off(&mut out);
                    self.swallow_esc = true;
                    out.consume = true;
                }
            }
            Input::EscapeUp => {
                out.consume = std::mem::take(&mut self.swallow_esc);
            }
            Input::FocusLost => {
                self.off(&mut out);
                // Releases may never come back to this window.
                self.swallow = 0;
                self.swallow_esc = false;
            }
            Input::Away => self.off(&mut out),
        }
        out
    }
}

// ---- The app's side ---------------------------------------------------------------

thread_local! {
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::default());
    static CONFIG: Cell<Config> = Cell::new(Config::default());
}

/// theme.toml `[autoscroll]`.
pub fn set_config(cfg: Config) {
    CONFIG.with(|c| c.set(cfg));
}

pub fn config() -> Config {
    CONFIG.with(Cell::get)
}

#[cfg(any(feature = "bench", target_os = "linux"))]
/// Is the mode on (for the bench, and Linux's window edges, which wait while it is)?
pub fn mode() -> Mode {
    MACHINE.with(|m| m.borrow().mode)
}

/// Run one input through the mode and apply what it asks for to the UI. Returns whether
/// the event is kept from the UI.
pub fn handle(app: &AppWindow, ev: Input) -> bool {
    let g = app.global::<crate::Autoscroll>();
    let cfg = config();
    let out = MACHINE.with(|m| {
        let mut m = m.borrow_mut();
        // Ended from the UI's side (a page change) since the last input.
        if m.is_on() && !g.get_on() {
            m.input(Input::Away, &cfg, true, true, false);
        }
        m.input(ev, &cfg, g.get_engaged(), g.get_enabled(), app.global::<MenuStore>().get_open())
    });
    if let Some((x, y)) = out.start {
        // Fresh claims, then ask every area under the anchor.
        g.set_x_owner(0);
        g.set_y_owner(0);
        g.set_x_layer(-1);
        g.set_y_layer(-1);
        g.set_x_area(f32::MAX);
        g.set_y_area(f32::MAX);
        g.set_top_layer(-1);
        g.set_anchor_x(x);
        g.set_anchor_y(y);
        g.set_on(true);
        g.set_probe(g.get_probe().wrapping_add(1));
        log::debug!("autoscroll: on at {x:.0},{y:.0}");
    }
    if let Some((vx, vy)) = out.velocity {
        if g.get_vx() != vx {
            g.set_vx(vx);
        }
        if g.get_vy() != vy {
            g.set_vy(vy);
        }
    }
    if out.stop {
        g.set_on(false);
        log::debug!("autoscroll: off ({ev:?})");
    }
    if out.start.is_some() || out.stop {
        refresh_cursor(app);
    }
    out.consume
}

/// The pointer's cursor (the scroll cursor while on, whatever is under it after) and the
/// hover states follow the next pointer move; send one where the pointer is, once the
/// areas have claimed the press (their change handlers run on the next turn of the event
/// loop, so two turns from now).
fn refresh_cursor(app: &AppWindow) {
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            let (x, y) = MACHINE.with(|m| m.borrow().pointer());
            app.window().dispatch_event(slint::platform::WindowEvent::PointerMoved { position: slint::LogicalPosition::new(x, y) });
        });
    });
}

/// Something in the UI took the user elsewhere (`Autoscroll.stop()`).
pub fn away(app: &AppWindow) {
    handle(app, Input::Away);
}

/// Take the window's input before Slint does (winit's event hook).
pub fn install(app: &AppWindow) {
    use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
    use slint::winit_030::winit::keyboard::{Key, NamedKey};
    use slint::winit_030::{EventResult, WinitWindowAccessor};

    let weak = app.as_weak();
    app.window().on_winit_window_event(move |w, ev| {
        let Some(app) = weak.upgrade() else { return EventResult::Propagate };
        // Covered or seen again (macOS, X11): what runs only while the window shows follows;
        // Slint sees it too.
        if let WindowEvent::Occluded(o) = ev {
            crate::ticker::occluded(*o);
        }
        // What the system's window module takes first (Linux: the caption's resize edges and
        // buttons, side buttons, file drops, the window-level pin keys).
        if crate::platform::window::winit_event(&app, w, ev) {
            return EventResult::PreventDefault;
        }
        // The interface size: the monitor's scale factor, Ctrl and the wheel, a pinch.
        if crate::zoom::winit_event(&app, w, ev) {
            return EventResult::PreventDefault;
        }
        let at = || MACHINE.with(|m| m.borrow().pointer());
        let input = match ev {
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f32>(w.scale_factor() as f64);
                Input::Move(p.x, p.y)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Right => Button::Right,
                    MouseButton::Middle => Button::Middle,
                    _ => Button::Other,
                };
                let (x, y) = at();
                match state {
                    ElementState::Pressed => Input::Press(b, x, y),
                    ElementState::Released => Input::Release(b, x, y),
                }
            }
            WindowEvent::MouseWheel { .. } => Input::Wheel,
            WindowEvent::KeyboardInput { event, .. } if event.logical_key == Key::Named(NamedKey::Escape) => match event.state {
                ElementState::Pressed => Input::Escape,
                ElementState::Released => Input::EscapeUp,
            },
            WindowEvent::Focused(false) => Input::FocusLost,
            _ => return EventResult::Propagate,
        };
        if handle(&app, input) { EventResult::PreventDefault } else { EventResult::Propagate }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Config = Config { dead_zone: 15.0, speed: 600.0, exponent: 1.8, max_speed: 12_000.0 };

    fn run(m: &mut Machine, ev: Input) -> Outcome {
        m.input(ev, &C, true, true, false)
    }

    #[test]
    fn curve_is_still_in_the_dead_zone_gentle_past_it_and_accelerates() {
        assert_eq!(speed(&C, 0.0), 0.0);
        assert_eq!(speed(&C, 15.0), 0.0);
        assert_eq!(speed(&C, -14.0), 0.0);
        // Just past it: a few px per second, so text can be read as it goes by.
        let near = speed(&C, 20.0);
        assert!(near > 0.0 && near < 15.0, "{near}");
        assert_eq!(speed(&C, -20.0), -near, "the same curve both ways");
        // 100 px past: the configured speed.
        assert!((speed(&C, 115.0) - 600.0).abs() < 0.01);
        // Accelerating: each step out adds more than the one before.
        let s: Vec<f32> = [40.0, 80.0, 120.0, 160.0, 200.0].iter().map(|&d| speed(&C, d)).collect();
        for w in s.windows(2) {
            assert!(w[1] > w[0]);
        }
        let steps: Vec<f32> = s.windows(2).map(|w| w[1] - w[0]).collect();
        for w in steps.windows(2) {
            assert!(w[1] > w[0], "{steps:?}");
        }
        // And capped.
        assert_eq!(speed(&C, 5000.0), 12_000.0);
        assert_eq!(speed(&C, f32::NAN), 0.0);
    }

    #[test]
    fn a_click_toggles_it_on_and_any_button_ends_it_without_clicking() {
        let mut m = Machine::default();
        let o = run(&mut m, Input::Press(Button::Middle, 100.0, 100.0));
        assert!(o.consume);
        assert_eq!(o.start, Some((100.0, 100.0)));
        let o = run(&mut m, Input::Release(Button::Middle, 102.0, 101.0));
        assert!(o.consume && !o.stop, "a quick click leaves it on");
        assert_eq!(m.mode, Mode::Toggled);
        // Moving sets the speed: below the anchor scrolls down.
        let o = run(&mut m, Input::Move(100.0, 215.0));
        assert!(!o.consume, "moves still reach the UI (the cursor)");
        let (vx, vy) = o.velocity.unwrap();
        assert_eq!(vx, 0.0);
        assert!((vy - 600.0).abs() < 0.01);
        let o = run(&mut m, Input::Move(100.0, 30.0));
        assert!(o.velocity.unwrap().1 < 0.0, "above scrolls up");
        // A left press ends it, and neither it nor its release reaches the UI.
        let o = run(&mut m, Input::Press(Button::Left, 100.0, 30.0));
        assert!(o.consume && o.stop);
        assert_eq!(o.velocity, Some((0.0, 0.0)));
        assert!(run(&mut m, Input::Release(Button::Left, 100.0, 30.0)).consume);
        // The next click is an ordinary one.
        assert!(!run(&mut m, Input::Press(Button::Left, 10.0, 10.0)).consume);
        assert!(!run(&mut m, Input::Release(Button::Left, 10.0, 10.0)).consume);
    }

    #[test]
    fn each_way_of_ending_it() {
        let on = || {
            let mut m = Machine::default();
            run(&mut m, Input::Press(Button::Middle, 50.0, 50.0));
            run(&mut m, Input::Release(Button::Middle, 50.0, 50.0));
            assert!(m.is_on());
            m
        };
        for b in [Button::Right, Button::Middle, Button::Other] {
            let mut m = on();
            let o = run(&mut m, Input::Press(b, 60.0, 60.0));
            assert!(o.stop && o.consume, "{b:?}");
            assert!(run(&mut m, Input::Release(b, 60.0, 60.0)).consume, "{b:?} release");
            assert!(!m.is_on());
        }
        let mut m = on();
        let o = run(&mut m, Input::Wheel);
        assert!(o.stop && o.consume, "the wheel ends it and does not scroll");
        assert!(!run(&mut m, Input::Wheel).consume, "then the wheel scrolls again");
        let mut m = on();
        let o = run(&mut m, Input::Escape);
        assert!(o.stop && o.consume, "Esc ends it and does not go back");
        assert!(run(&mut m, Input::EscapeUp).consume);
        assert!(!run(&mut m, Input::Escape).consume, "then Esc is Esc again");
        let mut m = on();
        assert!(run(&mut m, Input::FocusLost).stop);
        let mut m = on();
        let o = run(&mut m, Input::Away);
        assert!(o.stop && !o.consume);
        // Off already: nothing to stop.
        assert!(!run(&mut m, Input::Away).stop);
    }

    #[test]
    fn held_drag_ends_on_release_once_it_moved() {
        let mut m = Machine::default();
        run(&mut m, Input::Press(Button::Middle, 100.0, 100.0));
        assert_eq!(m.mode, Mode::Held { moved: false });
        let o = run(&mut m, Input::Move(100.0, 300.0));
        assert!(o.velocity.unwrap().1 > 0.0, "scrolls while held");
        assert_eq!(m.mode, Mode::Held { moved: true });
        // Back near the anchor: it still moved while held, so the release ends it.
        run(&mut m, Input::Move(101.0, 104.0));
        let o = run(&mut m, Input::Release(Button::Middle, 101.0, 104.0));
        assert!(o.stop && o.consume);
        assert!(!m.is_on());
        // Within the dead zone the whole time: stays on.
        let mut m = Machine::default();
        run(&mut m, Input::Press(Button::Middle, 100.0, 100.0));
        run(&mut m, Input::Move(110.0, 92.0));
        assert!(!run(&mut m, Input::Release(Button::Middle, 110.0, 92.0)).stop);
        assert_eq!(m.mode, Mode::Toggled);
        // Another button while held ends it; the middle release is still not passed on.
        let mut m = Machine::default();
        run(&mut m, Input::Press(Button::Middle, 100.0, 100.0));
        assert!(run(&mut m, Input::Press(Button::Left, 100.0, 100.0)).stop);
        assert!(run(&mut m, Input::Release(Button::Middle, 100.0, 100.0)).consume);
        assert!(run(&mut m, Input::Release(Button::Left, 100.0, 100.0)).consume);
    }

    #[test]
    fn nothing_to_scroll_does_nothing_and_the_next_click_is_a_click() {
        let mut m = Machine::default();
        let o = m.input(Input::Press(Button::Middle, 5.0, 5.0), &C, false, true, false);
        assert!(o.consume, "a middle press never reaches the UI");
        m.input(Input::Release(Button::Middle, 5.0, 5.0), &C, false, true, false);
        // The UI found no area under it (`engaged` false): the next click goes through.
        let o = m.input(Input::Press(Button::Left, 5.0, 5.0), &C, false, true, false);
        assert!(!o.consume && !m.is_on());
        assert!(!m.input(Input::Release(Button::Left, 5.0, 5.0), &C, false, true, false).consume);
        // And the wheel and Esc too.
        m.input(Input::Press(Button::Middle, 5.0, 5.0), &C, false, true, false);
        m.input(Input::Release(Button::Middle, 5.0, 5.0), &C, false, true, false);
        assert!(!m.input(Input::Wheel, &C, false, true, false).consume);
    }

    #[test]
    fn off_in_settings_or_with_a_menu_open_the_middle_button_passes() {
        let mut m = Machine::default();
        let o = m.input(Input::Press(Button::Middle, 5.0, 5.0), &C, true, false, false);
        assert!(!o.consume && o.start.is_none() && !m.is_on());
        assert!(!m.input(Input::Release(Button::Middle, 5.0, 5.0), &C, true, false, false).consume);
        let o = m.input(Input::Press(Button::Middle, 5.0, 5.0), &C, true, true, true);
        assert!(!o.consume && !m.is_on(), "the open menu gets it (and closes)");
    }

    #[test]
    fn focus_loss_forgets_releases_it_will_not_see() {
        let mut m = Machine::default();
        run(&mut m, Input::Press(Button::Middle, 5.0, 5.0));
        run(&mut m, Input::FocusLost);
        assert!(!m.is_on());
        assert!(!run(&mut m, Input::Release(Button::Middle, 5.0, 5.0)).consume);
    }
}
