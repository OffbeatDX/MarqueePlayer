//! The tray icon: its tooltip and menu label (following the player bar), its picture
//! (`trayanim`), and what the mouse does on it. The icon itself is the system's
//! (`platform::tray`), which hands back `TrayEvent`s.
//!
//! The mouse, with Settings > Window > "Click on the tray icon" on Play/pause (the
//! default): a click plays or pauses (with nothing playing it starts the queue, as the
//! Play button does); a double click shows the window, or hides it if it is shown; the
//! middle button skips to the next track; the wheel changes the volume in the volume keys'
//! 5% steps, the tooltip saying the new volume at once (for `NOTE_FOR`). A click waits the
//! system's double-click time before it plays or pauses, so a double click never also
//! does (nothing to undo, no blip of sound). On "Show window" a click shows the window at
//! once, as it did before, and a double click does nothing more. The right button's menu
//! is the same either way.
//!
//! `anim` is the icon's animation, `icon` its picture (the wave alone, at the tray's size) and
//! `icon_art` the app icon's artwork, shared with `build.rs` and the icon examples.

pub(crate) mod anim;
pub(crate) mod icon;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::platform::{self, TrayEvent};
use crate::settings::TrayClick;
use crate::state::{self, State};
use crate::{AppWindow, Player, about, playback, ticker};

/// How long the tooltip leads with the volume after the wheel changed it.
const NOTE_FOR: Duration = Duration::from_secs(3);

/// What a click or double click comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    PlayPause,
    /// Show the window, or hide it if it is shown.
    ShowHide,
    /// Show the window (the "Show window" setting's click).
    Show,
}

/// What to do now, and when to look again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub act: Option<Act>,
    pub wait_until: Option<Instant>,
}

impl Step {
    fn act(a: Act) -> Self {
        Step { act: Some(a), wait_until: None }
    }
}

/// Clicks into acts. A click waits out the double-click time (`within`): a second one
/// before then, or the system's double click, makes it a double click instead. Pure (the
/// caller brings the time), so it is tested without a tray.
#[derive(Debug, Default)]
pub struct Clicks {
    /// A click waiting to see whether a second follows.
    pending: Option<Instant>,
    /// The system's double click came: the button's release that follows it is part of it.
    swallow: Option<Instant>,
}

impl Clicks {
    /// The left button released over the icon (Windows), or the item activated (Linux).
    pub fn click(&mut self, now: Instant, within: Duration, mode: TrayClick) -> Step {
        if let Some(at) = self.swallow.take()
            && now.saturating_duration_since(at) <= within
        {
            return Step::default();
        }
        if mode == TrayClick::ShowWindow {
            self.pending = None;
            return Step::act(Act::Show);
        }
        match self.pending.take() {
            // The second click of a double click (where the system sends no double click).
            Some(at) if now.saturating_duration_since(at) <= within => Step::act(Act::ShowHide),
            // The first one's time ran out unseen (a late timer): it plays or pauses now,
            // and this one waits its turn.
            Some(_) => {
                self.pending = Some(now);
                Step { act: Some(Act::PlayPause), wait_until: Some(now + within) }
            }
            None => {
                self.pending = Some(now);
                Step { act: None, wait_until: Some(now + within) }
            }
        }
    }

    /// The system's double click (Windows' `WM_LBUTTONDBLCLK`, after the first release).
    pub fn double(&mut self, now: Instant, mode: TrayClick) -> Step {
        self.swallow = Some(now);
        self.pending = None;
        match mode {
            TrayClick::PlayPause => Step::act(Act::ShowHide),
            // The first click showed the window already.
            TrayClick::ShowWindow => Step::default(),
        }
    }

    /// The wait is over (the timer): a click still waiting plays or pauses.
    pub fn due(&mut self, now: Instant, within: Duration) -> Step {
        match self.pending {
            Some(at) if now.saturating_duration_since(at) >= within => {
                self.pending = None;
                Step::act(Act::PlayPause)
            }
            Some(at) => Step { act: None, wait_until: Some(at + within) },
            None => Step::default(),
        }
    }
}

/// The wheel into volume steps: 120 a notch (a fine wheel or a touchpad sends less, and
/// adds up to a step); turning back starts again from nothing.
#[derive(Debug, Default)]
pub struct Wheel {
    rest: i32,
}

impl Wheel {
    /// Steps up (positive) or down for `delta` more.
    pub fn add(&mut self, delta: i32) -> i32 {
        if (self.rest > 0 && delta < 0) || (self.rest < 0 && delta > 0) {
            self.rest = 0;
        }
        self.rest = self.rest.saturating_add(delta);
        let steps = self.rest / 120;
        self.rest -= steps * 120;
        steps
    }
}

/// The tooltip: the volume just set (if any), then what plays, or the app and its version
/// while nothing is loaded.
pub fn tooltip_text(note: &str, now_playing: &str, version: &str) -> String {
    let base = if now_playing.is_empty() { format!("Marquee Player {version}").trim_end().to_owned() } else { now_playing.to_owned() };
    if note.is_empty() { base } else { format!("{note}\n{base}") }
}

struct Shown {
    playing: bool,
    tip: String,
}

thread_local! {
    static TRAY: RefCell<Option<platform::tray::Tray>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Shown> = const { RefCell::new(Shown { playing: false, tip: String::new() }) };
    static CLICKS: RefCell<Clicks> = RefCell::new(Clicks::default());
    static CLICK_TIMER: Timer = Timer::default();
    static WHEEL: RefCell<Wheel> = RefCell::new(Wheel::default());
    static NOTE: RefCell<String> = const { RefCell::new(String::new()) };
    static NOTE_TIMER: Timer = Timer::default();
    /// What the clicks did, for the bench.
    static ACTS: Cell<u32> = const { Cell::new(0) };
}

/// Show the tray icon. False where the shell or desktop gave none.
pub fn install() -> bool {
    let tip = tooltip_text("", "", about::VERSION);
    // Drawn at the notification area's own size (crisper than a big one scaled down).
    let tray = platform::tray::Tray::new(anim::static_icon(), &tip, on_event);
    let has = tray.is_some();
    TRAY.with(|t| *t.borrow_mut() = tray);
    sync();
    has
}

/// Take the icon down (at exit).
pub fn remove() {
    CLICK_TIMER.with(Timer::stop);
    NOTE_TIMER.with(Timer::stop);
    TRAY.with(|t| t.borrow_mut().take());
    platform::tray::release();
}

/// The tray's tooltip and play/pause label follow the player bar. Called when either
/// may have changed (`playback::tick`, `refresh_now_playing`); sets only what changed.
pub fn sync() {
    let Some(app) = state::with(|st| st.app.upgrade()).flatten() else { return };
    let p = app.global::<Player>();
    let playing = p.get_playing();
    let np = if p.get_has_track() { format!("{} — {}", p.get_title(), p.get_artist()) } else { String::new() };
    let tip = NOTE.with(|n| tooltip_text(&n.borrow(), &np, about::VERSION));
    TRAY.with(|t| {
        let t = t.borrow();
        let Some(tray) = t.as_ref() else { return };
        SHOWN.with(|s| {
            let mut s = s.borrow_mut();
            if s.playing != playing {
                tray.set_playing(playing);
                s.playing = playing;
            }
            if s.tip != tip {
                tray.set_tooltip(&tip);
                s.tip = tip;
            }
        });
    });
    state::with(anim::wake);
}

/// The tray icon's tooltip as it stands (for the bench).
#[cfg(feature = "bench")]
pub fn tooltip() -> Option<String> {
    exists().then(|| SHOWN.with(|s| s.borrow().tip.clone()))
}

/// Is there a tray icon (the shell may refuse one)?
pub fn exists() -> bool {
    TRAY.with(|t| t.borrow().is_some())
}

/// Replace the tray icon's picture (`trayanim`).
pub fn set_image(img: slint::Image) {
    TRAY.with(|t| {
        if let Some(tray) = t.borrow().as_ref() {
            tray.set_icon(img);
        }
    });
}

/// The bench: hand the icon what the mouse would (`platform::tray::Tray::simulate`).
#[cfg(feature = "bench")]
pub fn simulate(what: platform::TrayPointer) -> bool {
    TRAY.with(|t| t.borrow().as_ref().is_some_and(|tray| tray.simulate(what)))
}

/// The bench: how many clicks and double clicks have come to something.
#[cfg(feature = "bench")]
pub fn acts() -> u32 {
    ACTS.with(Cell::get)
}

fn on_event(e: TrayEvent) {
    match e {
        TrayEvent::TogglePlay => {
            state::with(playback::toggle_play);
        }
        TrayEvent::Next | TrayEvent::Middle => {
            state::with(playback::next);
        }
        TrayEvent::Previous => {
            state::with(playback::previous);
        }
        TrayEvent::ShowWindow => {
            if let Some(app) = app() {
                crate::show_window(&app);
            }
            ticker::kick();
        }
        TrayEvent::About => {
            if let Some(app) = app() {
                crate::show_about(&app);
            }
        }
        TrayEvent::Quit => {
            crate::request_quit();
        }
        TrayEvent::Click => {
            let within = platform::tray::double_click_time();
            clicks(|c, now, mode| c.click(now, within, mode));
        }
        TrayEvent::DoubleClick => clicks(|c, now, mode| c.double(now, mode)),
        TrayEvent::Wheel(delta) => wheel(delta),
    }
}

fn app() -> Option<AppWindow> {
    state::with(|st| st.app.upgrade()).flatten()
}

fn mode() -> TrayClick {
    state::with(|st| st.settings.borrow().tray_click).unwrap_or_default()
}

fn clicks(f: impl FnOnce(&mut Clicks, Instant, TrayClick) -> Step) {
    let step = CLICKS.with(|c| f(&mut c.borrow_mut(), Instant::now(), mode()));
    follow(step);
}

fn follow(step: Step) {
    if let Some(at) = step.wait_until {
        let wait = at.saturating_duration_since(Instant::now());
        CLICK_TIMER.with(|t| {
            t.start(TimerMode::SingleShot, wait, || {
                let within = platform::tray::double_click_time();
                let step = CLICKS.with(|c| c.borrow_mut().due(Instant::now(), within));
                follow(step);
            });
        });
    }
    if let Some(a) = step.act {
        ACTS.with(|n| n.set(n.get() + 1));
        state::with(|st| act(st, a));
    }
}

fn act(st: &Rc<State>, a: Act) {
    match a {
        Act::PlayPause => playback::toggle_play(st),
        Act::Show => {
            if let Some(app) = st.app.upgrade() {
                bring_back(st, &app);
            }
        }
        Act::ShowHide => {
            let Some(app) = st.app.upgrade() else { return };
            let w = app.window();
            if w.is_visible() && !w.is_minimized() {
                let _ = app.hide();
            } else {
                bring_back(st, &app);
            }
        }
    }
}

/// Show the window where it was saved, unminimized and in front (the click gave the app
/// the right to the foreground; `MP_WINDOW=secondary` still never takes it).
fn bring_back(st: &Rc<State>, app: &AppWindow) {
    crate::show_window(app);
    platform::window::bring_back(app, st.hwnd.get());
    ticker::kick();
}

fn wheel(delta: i32) {
    let steps = WHEEL.with(|w| w.borrow_mut().add(delta));
    if steps == 0 {
        return;
    }
    let Some(v) = state::with(|st| playback::nudge_volume(st, steps)) else { return };
    NOTE.with(|n| *n.borrow_mut() = format!("Volume {}%", (v * 100.0).round()));
    sync();
    NOTE_TIMER.with(|t| {
        t.start(TimerMode::SingleShot, NOTE_FOR, || {
            NOTE.with(|n| n.borrow_mut().clear());
            sync();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITHIN: Duration = Duration::from_millis(500);

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn a_click_plays_or_pauses_once_the_double_click_time_is_over() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        let s = c.click(t0, WITHIN, TrayClick::PlayPause);
        assert_eq!(s, Step { act: None, wait_until: Some(ms(t0, 500)) }, "nothing at once");
        assert_eq!(c.due(ms(t0, 499), WITHIN), Step { act: None, wait_until: Some(ms(t0, 500)) }, "an early timer waits again");
        assert_eq!(c.due(ms(t0, 500), WITHIN), Step::act(Act::PlayPause));
        assert_eq!(c.due(ms(t0, 900), WITHIN), Step::default(), "and only once");
    }

    #[test]
    fn the_systems_double_click_shows_or_hides_and_never_also_plays() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        // Windows: release, double click, release.
        c.click(t0, WITHIN, TrayClick::PlayPause);
        assert_eq!(c.double(ms(t0, 180), TrayClick::PlayPause), Step::act(Act::ShowHide));
        assert_eq!(c.click(ms(t0, 260), WITHIN, TrayClick::PlayPause), Step::default(), "its release is part of it");
        assert_eq!(c.due(ms(t0, 500), WITHIN), Step::default(), "the first click no longer plays");
        // The next click is a click again.
        assert_eq!(c.click(ms(t0, 2000), WITHIN, TrayClick::PlayPause).act, None);
        assert_eq!(c.due(ms(t0, 2500), WITHIN), Step::act(Act::PlayPause));
    }

    #[test]
    fn two_clicks_in_time_are_a_double_click_where_the_system_sends_none() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        c.click(t0, WITHIN, TrayClick::PlayPause);
        assert_eq!(c.click(ms(t0, 300), WITHIN, TrayClick::PlayPause), Step::act(Act::ShowHide));
        assert_eq!(c.due(ms(t0, 500), WITHIN), Step::default());
        // Too slow: two clicks, each its own.
        c.click(ms(t0, 1000), WITHIN, TrayClick::PlayPause);
        assert_eq!(c.due(ms(t0, 1500), WITHIN), Step::act(Act::PlayPause));
        c.click(ms(t0, 1600), WITHIN, TrayClick::PlayPause);
        assert_eq!(c.due(ms(t0, 2100), WITHIN), Step::act(Act::PlayPause));
    }

    #[test]
    fn a_late_timer_still_plays_the_first_click_before_the_next_waits() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        c.click(t0, WITHIN, TrayClick::PlayPause);
        assert_eq!(c.click(ms(t0, 800), WITHIN, TrayClick::PlayPause), Step { act: Some(Act::PlayPause), wait_until: Some(ms(t0, 1300)) });
        assert_eq!(c.due(ms(t0, 1300), WITHIN), Step::act(Act::PlayPause));
    }

    #[test]
    fn a_stale_double_click_does_not_eat_a_later_click() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        c.double(t0, TrayClick::PlayPause);
        assert_eq!(c.click(ms(t0, 3000), WITHIN, TrayClick::PlayPause).wait_until, Some(ms(t0, 3500)));
    }

    #[test]
    fn on_show_window_a_click_shows_at_once_and_a_double_click_adds_nothing() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        assert_eq!(c.click(t0, WITHIN, TrayClick::ShowWindow), Step::act(Act::Show));
        assert_eq!(c.double(ms(t0, 150), TrayClick::ShowWindow), Step::default());
        assert_eq!(c.click(ms(t0, 230), WITHIN, TrayClick::ShowWindow), Step::default());
        assert_eq!(c.due(ms(t0, 1000), WITHIN), Step::default(), "nothing waits");
        assert_eq!(c.click(ms(t0, 2000), WITHIN, TrayClick::ShowWindow), Step::act(Act::Show));
    }

    #[test]
    fn the_wheel_steps_once_a_notch_and_adds_up_fine_deltas() {
        let mut w = Wheel::default();
        assert_eq!(w.add(120), 1);
        assert_eq!(w.add(-240), -2);
        assert_eq!(w.add(40), 0);
        assert_eq!(w.add(40), 0);
        assert_eq!(w.add(40), 1, "three fine steps make a notch");
        assert_eq!(w.add(100), 0);
        assert_eq!(w.add(-100), 0, "turning back starts from nothing");
        assert_eq!(w.add(-20), -1);
        assert_eq!(w.add(360), 3);
    }

    #[test]
    fn the_tooltip_leads_with_the_volume_while_it_is_new() {
        assert_eq!(tooltip_text("", "", "2026.11.18"), "Marquee Player 2026.11.18");
        assert_eq!(tooltip_text("", "", ""), "Marquee Player");
        assert_eq!(tooltip_text("", "Song — Artist", "2026.11.18"), "Song — Artist");
        assert_eq!(tooltip_text("Volume 45%", "Song — Artist", "x"), "Volume 45%\nSong — Artist");
        assert_eq!(tooltip_text("Volume 0%", "", "1"), "Volume 0%\nMarquee Player 1");
    }
}
