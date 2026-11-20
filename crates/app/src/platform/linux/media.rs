//! Linux: the desktop's media controls through MPRIS (the Media Player Remote Interfacing
//! Specification, 2.2): the Plasma media applet and lock screen, GNOME's media controls,
//! `playerctl`, and the media keys, which the desktop routes to MPRIS players.
//!
//! The app owns `org.mpris.MediaPlayer2.MarqueePlayer` on the session bus (another
//! instance, on another data folder, adds `.instance<pid>`, as the specification asks) and
//! serves `/org/mpris/MediaPlayer2` with both interfaces: the root (Identity, DesktopEntry
//! = the desktop file's id, so the desktop shows the app's name and icon; Raise; Quit) and
//! the player (PlaybackStatus, Metadata with the cover's file URL, Position, Seek,
//! SetPosition and the Seeked signal, Shuffle, LoopStatus, Volume, Next, Previous, Play,
//! Pause, PlayPause, Stop). Changes are announced with PropertiesChanged when they happen;
//! Position is read when asked (the specification has clients interpolate it from the
//! status, and a seek says Seeked). Requests from the desktop arrive on zbus's own thread
//! and are handed to the app as `MediaEvent`s; nothing here polls: the connection sleeps
//! on its socket between messages.
//!
//! (souvlaki, which the Windows build uses, has an MPRIS backend, but it wakes 100 times a
//! second for ever and has no DesktopEntry, Shuffle, LoopStatus or Seeked.)

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::AppWindow;
use crate::platform::{APP_ID, MediaEvent, MediaOptions};

const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// What a track shows in the controls.
pub struct Now<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub art: Option<&'a Path>,
    pub duration: Duration,
}

type OnEvent = Arc<dyn Fn(MediaEvent) + Send + Sync>;

#[derive(Default)]
struct Track {
    id: u64,
    title: String,
    artist: String,
    album: String,
    art: Option<String>,
    length_us: i64,
}

struct Shared {
    /// None: stopped; Some(playing).
    playing: Option<bool>,
    /// Microseconds at `at`.
    position_us: i64,
    at: Instant,
    track: Option<Track>,
    options: MediaOptions,
}

impl Shared {
    fn position(&self) -> i64 {
        let ran = if self.playing == Some(true) { self.at.elapsed().as_micros() as i64 } else { 0 };
        let p = self.position_us + ran;
        match &self.track {
            Some(t) if t.length_us > 0 => p.min(t.length_us),
            _ => p,
        }
    }

    fn track_id(&self) -> OwnedObjectPath {
        let path = match &self.track {
            Some(t) => format!("/app/marqueeplayer/MarqueePlayer/track/{}", t.id),
            None => NO_TRACK.to_owned(),
        };
        OwnedObjectPath::try_from(path).unwrap_or_else(|_| OwnedObjectPath::try_from(NO_TRACK).unwrap())
    }

    /// Whether SetPosition(`track_id`, `position` µs) is to be done: for the track playing (else
    /// a stale request), and within it when its length is known (the specification: past the
    /// end, nothing happens).
    fn takes_position(&self, track_id: &str, position: i64) -> bool {
        track_id == self.track_id().as_str() && position >= 0 && self.track.as_ref().is_none_or(|t| t.length_us <= 0 || position <= t.length_us)
    }
}

/// LoopStatus as the app's repeat (off, all, one); None for a value the specification does not
/// have (refused, rather than taken as off).
fn repeat_of(status: &str) -> Option<u8> {
    match status {
        "None" => Some(0),
        "Playlist" => Some(1),
        "Track" => Some(2),
        _ => None,
    }
}

fn owned(v: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(v).expect("no file descriptors in metadata")
}

struct Root {
    on: OnEvent,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        (self.on)(MediaEvent::Raise);
    }
    fn quit(&self) {
        (self.on)(MediaEvent::Quit);
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Marquee Player"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        APP_ID
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    on: OnEvent,
    shared: Arc<Mutex<Shared>>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        (self.on)(MediaEvent::Next);
    }
    fn previous(&self) {
        (self.on)(MediaEvent::Previous);
    }
    fn pause(&self) {
        (self.on)(MediaEvent::Pause);
    }
    fn play_pause(&self) {
        (self.on)(MediaEvent::Toggle);
    }
    fn stop(&self) {
        (self.on)(MediaEvent::Stop);
    }
    fn play(&self) {
        (self.on)(MediaEvent::Play);
    }
    /// Microseconds, either way.
    fn seek(&self, offset: i64) {
        (self.on)(MediaEvent::SeekBy(offset as f64 / 1e6));
    }
    /// Ignored unless `track_id` is the track playing and `position` within it.
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let takes = self.shared.lock().unwrap().takes_position(track_id.as_str(), position);
        if takes {
            (self.on)(MediaEvent::SetPosition(position as f64 / 1e6));
        }
    }
    fn open_uri(&self, _uri: &str) {}

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        match self.shared.lock().unwrap().playing {
            Some(true) => "Playing",
            Some(false) => "Paused",
            None => "Stopped",
        }
    }
    #[zbus(property)]
    fn loop_status(&self) -> &str {
        match self.shared.lock().unwrap().options.repeat {
            1 => "Playlist",
            2 => "Track",
            _ => "None",
        }
    }
    #[zbus(property)]
    fn set_loop_status(&self, value: &str) -> zbus::fdo::Result<()> {
        let r = repeat_of(value).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("LoopStatus {value:?}: it is None, Track or Playlist")))?;
        (self.on)(MediaEvent::Repeat(r));
        Ok(())
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    /// Deliberately ignored: the app plays at one rate (MinimumRate and MaximumRate say 1), and a
    /// client is not to set another (Rate 0, which the specification lets mean pause, included).
    #[zbus(property)]
    fn set_rate(&self, _value: f64) {}
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.shared.lock().unwrap().options.shuffle
    }
    #[zbus(property)]
    fn set_shuffle(&self, value: bool) {
        (self.on)(MediaEvent::Shuffle(value));
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let s = self.shared.lock().unwrap();
        let mut m = HashMap::new();
        m.insert("mpris:trackid".to_owned(), owned(Value::from(s.track_id())));
        if let Some(t) = &s.track {
            if t.length_us > 0 {
                m.insert("mpris:length".to_owned(), owned(Value::from(t.length_us)));
            }
            m.insert("xesam:title".to_owned(), owned(Value::from(t.title.clone())));
            if !t.artist.is_empty() {
                m.insert("xesam:artist".to_owned(), owned(Value::from(vec![t.artist.clone()])));
            }
            if !t.album.is_empty() {
                m.insert("xesam:album".to_owned(), owned(Value::from(t.album.clone())));
            }
            if let Some(a) = &t.art {
                m.insert("mpris:artUrl".to_owned(), owned(Value::from(a.clone())));
            }
        }
        m
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.shared.lock().unwrap().options.volume
    }
    #[zbus(property)]
    fn set_volume(&self, value: f64) {
        (self.on)(MediaEvent::Volume(value));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.shared.lock().unwrap().position()
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;
}

pub struct Controls {
    conn: zbus::blocking::Connection,
    shared: Arc<Mutex<Shared>>,
    next_id: u64,
}

impl Controls {
    /// Own the MPRIS name; `on` receives the desktop's requests (on zbus's thread).
    pub fn new(_app: &AppWindow, on: impl Fn(MediaEvent) + Send + Sync + 'static) -> Option<Self> {
        let on: OnEvent = Arc::new(on);
        let shared = Arc::new(Mutex::new(Shared {
            playing: None,
            position_us: 0,
            at: Instant::now(),
            track: None,
            options: MediaOptions { shuffle: false, repeat: 0, volume: 1.0 },
        }));
        let base = "org.mpris.MediaPlayer2.MarqueePlayer";
        let names = [base.to_owned(), format!("{base}.instance{}", std::process::id())];
        let mut last = String::new();
        for name in names {
            let built = zbus::blocking::connection::Builder::session()
                .and_then(|b| b.name(name.as_str()))
                .and_then(|b| b.serve_at(PATH, Root { on: on.clone() }))
                .and_then(|b| b.serve_at(PATH, Player { on: on.clone(), shared: shared.clone() }))
                .and_then(|b| b.build());
            match built {
                Ok(conn) => {
                    log::info!("media controls: MPRIS as {name}");
                    return Some(Controls { conn, shared, next_id: 1 });
                }
                Err(e) => last = e.to_string(),
            }
        }
        log::warn!("media controls unavailable (MPRIS: {last})");
        None
    }

    /// Announce that these properties of the player changed.
    fn changed(&self, which: &[&str]) {
        let Ok(iface) = self.conn.object_server().interface::<_, Player>(PATH) else { return };
        let emitter = iface.signal_emitter();
        let player = iface.get();
        for w in which {
            let r = zbus::block_on(async {
                match *w {
                    "PlaybackStatus" => player.playback_status_changed(emitter).await,
                    "Metadata" => player.metadata_changed(emitter).await,
                    "Shuffle" => player.shuffle_changed(emitter).await,
                    "LoopStatus" => player.loop_status_changed(emitter).await,
                    "Volume" => player.volume_changed(emitter).await,
                    _ => Ok(()),
                }
            });
            if let Err(e) = r {
                log::debug!("MPRIS {w} changed: {e}");
            }
        }
    }

    pub fn set_metadata(&mut self, now: Option<Now>) {
        {
            let mut s = self.shared.lock().unwrap();
            s.track = now.map(|n| {
                self.next_id += 1;
                Track {
                    id: self.next_id,
                    title: n.title.to_owned(),
                    artist: n.artist.to_owned(),
                    album: n.album.to_owned(),
                    art: n.art.filter(|p| p.exists()).map(super::shell::file_uri),
                    length_us: n.duration.as_micros() as i64,
                }
            });
            s.position_us = 0;
            s.at = Instant::now();
        }
        self.changed(&["Metadata"]);
    }

    /// Playing, paused or stopped (`None`), at `position` seconds.
    pub fn set_playback(&mut self, playing: Option<bool>, position: f64) {
        {
            let mut s = self.shared.lock().unwrap();
            s.playing = playing;
            s.position_us = (position.max(0.0) * 1e6) as i64;
            s.at = Instant::now();
        }
        self.changed(&["PlaybackStatus"]);
    }

    pub fn set_options(&mut self, options: MediaOptions) {
        let before = std::mem::replace(&mut self.shared.lock().unwrap().options, options);
        let mut which = Vec::new();
        if before.shuffle != options.shuffle {
            which.push("Shuffle");
        }
        if before.repeat != options.repeat {
            which.push("LoopStatus");
        }
        if (before.volume - options.volume).abs() > 1e-6 {
            which.push("Volume");
        }
        self.changed(&which);
    }

    /// The position jumped: clients are told with Seeked (microseconds).
    pub fn seeked(&mut self, position: f64) {
        let us = (position.max(0.0) * 1e6) as i64;
        {
            let mut s = self.shared.lock().unwrap();
            s.position_us = us;
            s.at = Instant::now();
        }
        let Ok(iface) = self.conn.object_server().interface::<_, Player>(PATH) else { return };
        if let Err(e) = zbus::block_on(Player::seeked(iface.signal_emitter(), us)) {
            log::debug!("MPRIS Seeked: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_position_is_for_the_track_playing_and_within_it() {
        let mut s = Shared { playing: Some(true), position_us: 0, at: Instant::now(), track: None, options: MediaOptions { shuffle: false, repeat: 0, volume: 1.0 } };
        s.track = Some(Track { id: 7, length_us: 200_000_000, ..Default::default() });
        let id = s.track_id().as_str().to_owned();
        assert!(s.takes_position(&id, 0));
        assert!(s.takes_position(&id, 200_000_000), "the end itself");
        assert!(!s.takes_position(&id, 200_000_001), "past the end: nothing happens");
        assert!(!s.takes_position(&id, -1));
        assert!(!s.takes_position("/app/marqueeplayer/MarqueePlayer/track/6", 1_000_000), "a stale request");
        // A length not known: any position ahead.
        s.track = Some(Track { id: 8, length_us: 0, ..Default::default() });
        let id = s.track_id().as_str().to_owned();
        assert!(s.takes_position(&id, i64::MAX));
    }

    #[test]
    fn a_loop_status_the_specification_does_not_have_is_refused() {
        assert_eq!(repeat_of("None"), Some(0));
        assert_eq!(repeat_of("Playlist"), Some(1));
        assert_eq!(repeat_of("Track"), Some(2));
        assert_eq!(repeat_of("foo"), None);
        assert_eq!(repeat_of("track"), None);
    }
}
