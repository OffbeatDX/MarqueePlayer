//! macOS: Now Playing (Control Center, the lock screen, the Touch Bar) and the media keys,
//! via souvlaki (`MPNowPlayingInfoCenter` and `MPRemoteCommandCenter`), as souvlaki serves
//! the Windows media overlay. Remote commands arrive on the main thread and are handed to the
//! app as `MediaEvent`s. (`media.rs` keeps what the app does with the events.)

use std::path::Path;
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::AppWindow;
use crate::platform::{MediaEvent, MediaOptions};

/// What a track shows in Now Playing.
pub struct Now<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub art: Option<&'a Path>,
    pub duration: Duration,
}

pub struct Controls(MediaControls);

impl Controls {
    /// Take Now Playing's remote commands; `on` receives them.
    pub fn new(_app: &AppWindow, on: impl Fn(MediaEvent) + Send + Sync + 'static) -> Option<Self> {
        let config = PlatformConfig { display_name: "Marquee Player", dbus_name: "marquee_player", hwnd: None };
        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("media keys unavailable: {e:?}");
                return None;
            }
        };
        let attached = controls.attach(move |event: MediaControlEvent| {
            if let Some(e) = translate(event) {
                on(e);
            }
        });
        if let Err(e) = attached {
            log::warn!("media keys: attach failed: {e:?}");
            return None;
        }
        Some(Controls(controls))
    }

    pub fn set_metadata(&mut self, now: Option<Now>) {
        // souvlaki strips "file://" and reads the rest as a path.
        let cover_url = now.as_ref().and_then(|n| n.art).map(|p| format!("file://{}", p.display()));
        let res = self.0.set_metadata(MediaMetadata {
            title: now.as_ref().map(|n| n.title),
            artist: now.as_ref().map(|n| n.artist),
            album: now.as_ref().map(|n| n.album),
            cover_url: cover_url.as_deref(),
            duration: now.as_ref().map(|n| n.duration),
        });
        if let Err(e) = res {
            log::warn!("Now Playing metadata ({cover_url:?}): {e:?}");
        }
    }

    /// Playing, paused or stopped (`None`), at `position` seconds. Only on state changes:
    /// Now Playing runs the position on from the playback rate.
    pub fn set_playback(&mut self, playing: Option<bool>, position: f64) {
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let _ = self.0.set_playback(match playing {
            Some(true) => MediaPlayback::Playing { progress },
            Some(false) => MediaPlayback::Paused { progress },
            None => MediaPlayback::Stopped,
        });
    }

    /// Now Playing shows no shuffle, repeat or volume through souvlaki.
    pub fn set_options(&mut self, _options: MediaOptions) {}

    /// Now Playing gets the new position with the next playback state (`set_playback`).
    pub fn seeked(&mut self, _position: f64) {}
}

/// Now Playing's commands as the app's; a plain seek is 10 s.
fn translate(event: MediaControlEvent) -> Option<MediaEvent> {
    Some(match event {
        MediaControlEvent::Play => MediaEvent::Play,
        MediaControlEvent::Pause => MediaEvent::Pause,
        MediaControlEvent::Toggle => MediaEvent::Toggle,
        MediaControlEvent::Next => MediaEvent::Next,
        MediaControlEvent::Previous => MediaEvent::Previous,
        MediaControlEvent::Stop => MediaEvent::Stop,
        MediaControlEvent::Seek(dir) => MediaEvent::SeekBy(if dir == SeekDirection::Forward { 10.0 } else { -10.0 }),
        MediaControlEvent::SeekBy(dir, d) => {
            let s = d.as_secs_f64();
            MediaEvent::SeekBy(if dir == SeekDirection::Forward { s } else { -s })
        }
        MediaControlEvent::SetPosition(MediaPosition(p)) => MediaEvent::SetPosition(p.as_secs_f64()),
        MediaControlEvent::Raise => MediaEvent::Raise,
        MediaControlEvent::Quit => MediaEvent::Quit,
        _ => return None,
    })
}
