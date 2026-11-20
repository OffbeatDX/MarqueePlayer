//! Media keys and the desktop's media controls: the Windows media overlay (System Media
//! Transport Controls, via souvlaki), MPRIS on Linux (the Plasma media applet, GNOME's
//! media controls, `playerctl`, media keys). How each system is reached is
//! `platform::media`; what a request does, and what the controls are told, is here.
//! Set up after the window is shown (Windows needs its HWND).

use std::rc::Rc;
use std::time::Duration;

use library::Track;
use player_core::{PlayState, Status};
use crate::platform::media::{Controls, Now};
use crate::platform::{MediaEvent, MediaOptions};
use crate::state::{self, State};
use crate::{AppWindow, playback};

pub fn install(st: &Rc<State>, app: &AppWindow) {
    let Some(controls) = Controls::new(app, |event| state::post(move |st| handle_event(st, event))) else { return };
    *st.media.borrow_mut() = Some(controls);
    // What plays now, for controls that connect after the first track started.
    let track = st.queue.borrow().current().map(|i| i.track.clone());
    update_metadata(st, track.as_ref());
    update_options(st);
}

fn handle_event(st: &Rc<State>, event: MediaEvent) {
    crate::ticker::kick();
    match event {
        MediaEvent::Play => playback::set_paused(st, false),
        MediaEvent::Pause => playback::set_paused(st, true),
        MediaEvent::Toggle => playback::toggle_play(st),
        MediaEvent::Next => playback::next(st),
        MediaEvent::Previous => playback::previous(st),
        MediaEvent::Stop => playback::stop(st),
        MediaEvent::SeekBy(s) => playback::seek_by(st, s),
        MediaEvent::SetPosition(p) => playback::seek(st, p),
        MediaEvent::Shuffle(on) => playback::set_shuffle(st, on),
        MediaEvent::Repeat(r) => {
            // The player bar's button cycles off, all, one: step until it is there.
            for _ in 0..3 {
                if playback::repeat_index(st.queue.borrow().repeat) == i32::from(r) {
                    break;
                }
                playback::cycle_repeat(st);
            }
        }
        MediaEvent::Volume(v) => {
            if let Some(v) = volume_of(v) {
                playback::set_volume(st, v);
            }
        }
        MediaEvent::Raise => {
            if let Some(app) = st.app.upgrade() {
                crate::platform::window::bring_back(&app, st.hwnd.get());
            }
        }
        MediaEvent::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// A volume another program asked for, as the player takes it: within 0..1, and none for NaN or
/// an infinity (`clamp` keeps NaN), which would silence the output and be saved in the settings.
fn volume_of(v: f64) -> Option<f32> {
    v.is_finite().then(|| v.clamp(0.0, 1.0) as f32)
}

pub fn update_metadata(st: &Rc<State>, track: Option<&Track>) {
    let mut guard = st.media.borrow_mut();
    let Some(c) = guard.as_mut() else { return };
    let art = track.and_then(|t| t.art.as_ref()).map(|stem| st.art.paths(stem).full);
    c.set_metadata(track.map(|t| Now {
        title: &t.title,
        artist: &t.artist,
        album: &t.album,
        art: art.as_deref(),
        duration: Duration::from_secs_f64(t.duration.max(0.0)),
    }));
}

thread_local! {
    static LAST: std::cell::Cell<Option<Option<bool>>> = const { std::cell::Cell::new(None) };
}

/// What the controls are told: playing (`Some(true)`), paused (`Some(false)`) or stopped. A
/// server track loading (not started yet, or waiting for its download: `loading`) is paused
/// to them, as nothing is heard and the position holds: the overlay's clock does not run on,
/// and its button offers Play, which lets it go on (play/pause from a media key pauses it, as
/// the player bar's button does, and resumes it after).
pub fn shown_state(state: PlayState, loading: bool) -> Option<bool> {
    match state {
        PlayState::Playing if loading => Some(false),
        PlayState::Playing => Some(true),
        PlayState::Paused => Some(false),
        PlayState::Stopped if loading => Some(false),
        PlayState::Stopped => None,
    }
}

/// `position`: the one the player bar shows (a server track not started yet is at its start).
pub fn update_playback(st: &Rc<State>, s: &Status, loading: bool, position: f64) {
    // Only on state changes: the controls track position on their own. The position held while
    // loading is said again when it plays.
    let shown = shown_state(s.state, loading);
    if LAST.with(|l| l.replace(Some(shown))) == Some(shown) {
        return;
    }
    let mut guard = st.media.borrow_mut();
    let Some(c) = guard.as_mut() else { return };
    c.set_playback(shown, position);
}

/// Shuffle, repeat or the volume changed (MPRIS shows them; the Windows overlay does not).
pub fn update_options(st: &Rc<State>) {
    let options = {
        let s = st.settings.borrow();
        let repeat = playback::repeat_index(st.queue.borrow().repeat) as u8;
        MediaOptions { shuffle: s.shuffle, repeat, volume: f64::from(if s.muted { 0.0 } else { s.volume }) }
    };
    if let Some(c) = st.media.borrow_mut().as_mut() {
        c.set_options(options);
    }
}

/// The position jumped (a seek): MPRIS clients are told (the Windows overlay follows by
/// itself).
pub fn seeked(st: &Rc<State>, position: f64) {
    if let Some(c) = st.media.borrow_mut().as_mut() {
        c.seeked(position);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_track_loading_is_paused_to_the_media_controls() {
        assert_eq!(shown_state(PlayState::Playing, false), Some(true));
        assert_eq!(shown_state(PlayState::Playing, true), Some(false), "waiting for its download");
        assert_eq!(shown_state(PlayState::Stopped, true), Some(false), "not started yet");
        assert_eq!(shown_state(PlayState::Paused, true), Some(false));
        assert_eq!(shown_state(PlayState::Stopped, false), None);
    }

    #[test]
    fn a_volume_from_the_media_controls_is_a_number_within_range() {
        assert_eq!(volume_of(0.5), Some(0.5));
        assert_eq!(volume_of(1.7), Some(1.0));
        assert_eq!(volume_of(-3.0), Some(0.0));
        assert_eq!(volume_of(f64::NAN), None);
        assert_eq!(volume_of(f64::INFINITY), None);
        assert_eq!(volume_of(f64::NEG_INFINITY), None);
    }
}
