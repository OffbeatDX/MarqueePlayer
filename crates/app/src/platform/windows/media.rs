//! Windows: media keys and the Windows media overlay (System Media Transport Controls),
//! via souvlaki. Needs the window's HWND, so it is set up after the window is shown.
//! (`media.rs` keeps what the app does with the events.)

use std::path::Path;
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::AppWindow;
use crate::platform::{MediaEvent, MediaOptions};

/// What a track shows in the overlay.
pub struct Now<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub art: Option<&'a Path>,
    pub duration: Duration,
}

pub struct Controls(MediaControls);

impl Controls {
    /// Connect to the overlay; `on` receives its requests (on a thread of its own).
    pub fn new(app: &AppWindow, on: impl Fn(MediaEvent) + Send + Sync + 'static) -> Option<Self> {
        let handle = app.window().window_handle();
        let hwnd = match handle.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as *mut std::ffi::c_void,
            _ => {
                log::warn!("media keys: no Win32 window handle");
                return None;
            }
        };
        let config = PlatformConfig { display_name: "Marquee Player", dbus_name: "marquee_player", hwnd: Some(hwnd) };
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
        let cover_url = now.as_ref().and_then(|n| n.art).map(|p| {
            // souvlaki strips "file://" and hands the rest to StorageFile, which wants a
            // native Windows path.
            format!("file://{}", p.display().to_string().replace('/', "\\"))
        });
        let res = self.0.set_metadata(MediaMetadata {
            title: now.as_ref().map(|n| n.title),
            artist: now.as_ref().map(|n| n.artist),
            album: now.as_ref().map(|n| n.album),
            cover_url: cover_url.as_deref(),
            duration: now.as_ref().map(|n| n.duration),
        });
        if let Err(e) = res {
            log::warn!("media overlay metadata ({cover_url:?}): {e:?}");
        }
    }

    /// Playing, paused or stopped (`None`), at `position` seconds. Only on state changes:
    /// the overlay tracks position on its own.
    pub fn set_playback(&mut self, playing: Option<bool>, position: f64) {
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let _ = self.0.set_playback(match playing {
            Some(true) => MediaPlayback::Playing { progress },
            Some(false) => MediaPlayback::Paused { progress },
            None => MediaPlayback::Stopped,
        });
    }

    /// The overlay shows no shuffle, repeat or volume.
    pub fn set_options(&mut self, _options: MediaOptions) {}

    /// The overlay follows a seek by itself.
    pub fn seeked(&mut self, _position: f64) {}
}

/// The overlay's requests as the app's; a plain seek is 10 s.
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
        _ => return None,
    })
}
