//! Settings > Connections > Discord: what plays, on the user's Discord profile ("Listening
//! to"). The pipe to the Discord app on this machine and its thread are
//! `discord_ipc::Presence`'s; this tells it what plays and keeps the Settings card in step.
//!
//! Off unless turned on. It presents Marquee Player's own Discord application (`APP_ID`),
//! or one of the user's own when Settings names its ID (the application's name is what
//! Discord shows after "Listening to"). Shown while a track
//! plays, with how far in it is; cleared when paused or stopped, and when the app exits
//! (the pipe closes). Never from a bench run.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use discord_ipc::{Activity, Options, Presence, State as DcState};
use player_core::{PlayState, Status};
use slint::ComponentHandle;

use crate::Discord;
use crate::state::{self, State};

/// Marquee Player's own Discord application, used unless Settings names another. Its name is
/// what Discord shows after "Listening to". Not a secret: every client that shows a presence
/// sends its application's ID.
pub const APP_ID: &str = "1572352830646394880";

/// The start time shown moves with a seek; less than this is the position's own jitter.
const SEEK_MS: i64 = 2500;

thread_local! {
    static PRESENCE: RefCell<Option<Presence>> = const { RefCell::new(None) };
    static ON: Cell<bool> = const { Cell::new(false) };
    /// What was last asked for: the queue item playing and when it started.
    static ASKED: Cell<Option<(u64, i64)>> = const { Cell::new(None) };
}

pub fn install(st: &Rc<State>) {
    let p = Presence::start(Options::system(), Box::new(|s| state::post(move |st| show(st, &s))));
    PRESENCE.with(|c| *c.borrow_mut() = Some(p));
    configure(st);
}

/// The setting or the application ID changed (or just loaded): tell the thread and Settings.
fn configure(st: &Rc<State>) {
    let s = st.settings.borrow().discord.clone();
    let on = s.enabled && !crate::bench_run();
    ON.set(on);
    // Whatever plays is asked for again at the next tick.
    ASKED.set(None);
    PRESENCE.with(|c| {
        if let Some(p) = c.borrow().as_ref() {
            p.configure(on, effective_id(&s.app_id));
            if !on {
                p.set_activity(None);
            }
        }
    });
    if let Some(app) = st.app.upgrade() {
        let d = app.global::<Discord>();
        d.set_enabled(s.enabled);
        d.set_app_id(s.app_id.as_str().into());
        d.set_applied_id(s.app_id.into());
    }
    crate::ticker::kick();
}

/// The application presented to Discord: the one Settings names, or Marquee Player's own.
fn effective_id(setting: &str) -> &str {
    let setting = setting.trim();
    if setting.is_empty() { APP_ID } else { setting }
}

/// Settings: the switch and the application ID as they now stand.
pub fn set(st: &Rc<State>, enabled: bool, app_id: &str) {
    {
        let mut s = st.settings.borrow_mut();
        s.discord.enabled = enabled;
        s.discord.app_id = app_id.trim().to_owned();
    }
    st.save_settings_soon();
    configure(st);
}

/// What Settings shows for a state (also driven directly by the bench).
pub fn show(st: &Rc<State>, state: &DcState) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Discord>();
    let (title, text) = describe(state);
    d.set_title(title.into());
    d.set_status(text.into());
    d.set_connected(matches!(state, DcState::Connected { .. }));
    d.set_attention(needs_attention(state));
}

/// Nothing is shown until the user acts (the application ID): marked on Settings' section list.
pub fn needs_attention(s: &DcState) -> bool {
    matches!(s, DcState::Refused(_))
}

/// (title, status line)
pub fn describe(s: &DcState) -> (String, String) {
    match s {
        DcState::Off => ("Off".into(), String::new()),
        DcState::NoId => ("Needs an application ID".into(), "Paste your Discord application's ID below.".into()),
        DcState::Waiting => ("Waiting for Discord".into(), "Discord is not running on this computer, or is not answering. This connects by itself once it is.".into()),
        DcState::Connected { user } if user.is_empty() => ("Connected to Discord".into(), "Your profile shows the track while it plays.".into()),
        DcState::Connected { user } => (format!("Connected as {user}"), "Your profile shows the track while it plays.".into()),
        DcState::Refused(why) => ("Not shown on Discord".into(), why.clone()),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// What to ask Discord for, given what was last asked: `None` when nothing changed.
/// `playing` is the queue item playing and when it started, had it played without pause;
/// that moves when the user seeks, and by a little all the time.
fn changed(asked: Option<(u64, i64)>, playing: Option<(u64, i64)>) -> bool {
    match (asked, playing) {
        (None, None) => false,
        (Some((a, was)), Some((b, is))) => a != b || (is - was).abs() >= SEEK_MS,
        _ => true,
    }
}

/// Each tick (playback.rs): the track, a pause, a seek. Nothing is done while it is off.
pub fn update(st: &Rc<State>, s: &Status) {
    // While a server track waits for its download, what Discord was told stands; the time
    // shown is set again once it plays on (a start moved by the wait is a change).
    if !ON.get() || s.buffering {
        return;
    }
    let playing = (s.state == PlayState::Playing && s.token.is_some())
        .then(|| st.queue.borrow().current().map(|i| (i.uid, now_ms() - (s.position * 1000.0) as i64)))
        .flatten();
    if !changed(ASKED.get(), playing) {
        return;
    }
    ASKED.set(playing);
    let activity = playing.and_then(|(_, start_ms)| {
        let q = st.queue.borrow();
        let t = &q.current()?.track;
        let duration = s.duration.unwrap_or(t.duration);
        Some(Activity {
            details: t.title.clone(),
            state: t.artist.clone(),
            start_ms,
            end_ms: if duration > 0.0 { start_ms + (duration * 1000.0) as i64 } else { 0 },
        })
    });
    PRESENCE.with(|c| {
        if let Some(p) = c.borrow().as_ref() {
            p.set_activity(activity);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_again_for_a_new_track_a_pause_and_a_seek_only() {
        let t = 1_700_000_000_000;
        assert!(!changed(None, None));
        assert!(changed(None, Some((1, t))), "started");
        assert!(!changed(Some((1, t)), Some((1, t + 180))), "playing on");
        assert!(changed(Some((1, t)), Some((2, t + 180))), "the next track");
        assert!(changed(Some((1, t)), Some((1, t - 30_000))), "sought forward");
        assert!(changed(Some((1, t)), None), "paused");
    }

    #[test]
    fn the_built_in_application_is_used_unless_another_is_named() {
        assert!(discord_ipc::presence::valid_id(APP_ID));
        assert_eq!(effective_id("  "), APP_ID);
        assert_eq!(effective_id(" 1234567890123456789 "), "1234567890123456789");
    }

    #[test]
    fn only_a_refused_application_id_needs_attention() {
        assert!(needs_attention(&DcState::Refused("no".into())));
        assert!(!needs_attention(&DcState::Waiting));
        assert!(!needs_attention(&DcState::NoId));
        assert_eq!(describe(&DcState::Connected { user: "Alice".into() }).0, "Connected as Alice");
        assert_eq!(describe(&DcState::Connected { user: String::new() }).0, "Connected to Discord");
    }
}
