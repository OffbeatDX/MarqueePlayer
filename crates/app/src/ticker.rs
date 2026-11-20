//! When `playback::tick` runs: the UI's housekeeping (engine events, the position in
//! the player bar, play counting, settings and session saves, the tray).
//!
//! Engine events do not wait for a tick: a thread blocks on the engine's channel and
//! runs a tick on the UI thread for each one (`forward`). The timer is only for what
//! moves by itself, and runs no more often than that needs:
//!
//! - playing, window shown: every 100 ms (the position, play counting);
//! - playing, window hidden or minimized: every 500 ms (play counting, session saves);
//! - not playing, window shown: once a second (window placement, saves);
//! - not playing, hidden: not at all.
//!
//! Anything that changes playback or asks for a save calls `kick`, which runs at the
//! full rate for a moment (`SETTLE`), so a pause or a seek is shown promptly whatever
//! the rate was, and a settings change is saved when it is due (`state::SaveDelay`, a
//! second after the last change: within `SETTLE`).

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use player_core::{Event, PlayState};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{playback, state};

const FAST: Duration = Duration::from_millis(100);
const HIDDEN: Duration = Duration::from_millis(500);
const SLOW: Duration = Duration::from_secs(1);
/// After a kick, the full rate for this long (a pause takes effect ~50 ms later).
const SETTLE: Duration = Duration::from_millis(1500);

thread_local! {
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    static FAST_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Forward the engine's events: returns the receiver the UI drains (`State::events`),
/// and a tick runs on the UI thread as each event arrives.
pub fn forward(engine: crossbeam_channel::Receiver<Event>) -> crossbeam_channel::Receiver<Event> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let spawned = std::thread::Builder::new().name("mp-events".into()).spawn(move || {
        // Ends with the engine (its sender is dropped).
        for ev in engine.iter() {
            if tx.send(ev).is_err() {
                return;
            }
            let _ = slint::invoke_from_event_loop(|| {
                kick();
                state::with(playback::tick);
            });
        }
    });
    if let Err(e) = spawned {
        log::error!("cannot start the engine event thread: {e}");
    }
    rx
}

/// Start the timer. Call once, on the UI thread.
pub fn install() {
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, FAST, || {
        state::with(playback::tick);
        reschedule();
    });
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
    kick();
}

/// Playback or settings changed: tick at the full rate for a moment.
pub fn kick() {
    FAST_UNTIL.with(|f| f.set(Some(Instant::now() + SETTLE)));
    set_interval(Some(FAST));
}

fn set_interval(interval: Option<Duration>) {
    TIMER.with(|t| {
        let t = t.borrow();
        let Some(timer) = t.as_ref() else { return };
        match interval {
            Some(i) => {
                if !timer.running() {
                    timer.restart();
                }
                if timer.interval() != i {
                    timer.set_interval(i);
                }
            }
            None => timer.stop(),
        }
    });
}

/// Pick the rate for what is going on now (see the module doc).
fn reschedule() {
    let settling = FAST_UNTIL.with(|f| f.get().is_some_and(|t| Instant::now() < t));
    let (playing, visible) = state::with(|st| {
        let playing = st.player.status().state == PlayState::Playing;
        let visible = st.app.upgrade().is_some_and(|a| a.window().is_visible());
        (playing, visible)
    })
    .unwrap_or((false, false));
    let interval = match (settling, playing, visible) {
        (true, _, _) | (false, true, true) => Some(FAST),
        (false, true, false) => Some(HIDDEN),
        (false, false, true) => Some(SLOW),
        (false, false, false) => None,
    };
    set_interval(interval);
}
