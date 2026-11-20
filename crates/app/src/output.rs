//! Settings > Output: which device plays (the system default, or one chosen by its id,
//! kept across restarts), and exclusive mode (Linux: bit-perfect playback). The engine does
//! the switching and the fallback (`player_core::output`: WASAPI, PipeWire); this keeps the
//! choice, lists the devices (off the UI thread: enumerating opens nothing, but COM calls
//! can be slow) and says what is playing where.

use std::cell::RefCell;
use std::rc::Rc;

use player_core::output::{self as engine_output, DeviceInfo};
use player_core::{OutputConfig, OutputInfo};
use slint::ComponentHandle;

use crate::menus::{Act, Entry};
use crate::settings::Settings;
use crate::state::{self, State};
use crate::{Output, views};

thread_local! {
    /// The devices as last listed, and what the engine last opened.
    static DEVICES: RefCell<Option<Vec<DeviceInfo>>> = const { RefCell::new(None) };
    static INFO: RefCell<Option<OutputInfo>> = const { RefCell::new(None) };
    /// The newest listing in `DEVICES`, by the number it was asked with.
    static LISTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// What the engine last opened (`None` before anything played).
pub fn info() -> Option<OutputInfo> {
    INFO.with(|i| i.borrow().clone())
}

/// The system default device's id, as last listed.
pub fn default_device_id() -> Option<String> {
    DEVICES.with(|d| d.borrow().as_ref()?.iter().find(|d| d.default).map(|d| d.id.clone()))
}

/// A listed device's name.
pub fn device_name(id: &str) -> Option<String> {
    DEVICES.with(|d| d.borrow().as_ref()?.iter().find(|d| d.id == id).map(|d| d.name.clone()))
}

#[cfg(feature = "bench")]
/// The devices as last listed (`None` before the first listing).
pub fn devices() -> Option<Vec<DeviceInfo>> {
    DEVICES.with(|d| d.borrow().clone())
}

pub fn config(s: &Settings) -> OutputConfig {
    OutputConfig { device: s.output_device.clone(), exclusive: s.exclusive }
}

/// The engine opened its output (`Event::Output`).
pub fn on_info(st: &Rc<State>, info: OutputInfo) {
    let was_missing = INFO.with(|i| i.borrow().as_ref().is_some_and(|i| i.missing));
    // A reopen for another sample rate reports again with the same note: said once.
    let old_note = INFO.with(|i| i.borrow().as_ref().and_then(|i| i.note.clone()));
    if info.missing && !was_missing {
        let name = st.settings.borrow().output_device_name.clone().unwrap_or_else(|| "The chosen device".into());
        views::flash_long(st, &format!("{name} is not connected: playing on {}", info.device));
    } else if was_missing && !info.missing {
        views::flash(st, &format!("Back on {}", info.device));
    }
    if let Some(note) = &info.note
        && old_note.as_ref() != Some(note)
    {
        views::flash_long(st, note);
    }
    INFO.with(|i| *i.borrow_mut() = Some(info));
    refresh(st);
    // The equalizer of the device that plays now (its own, or the shared one).
    crate::eq::device_changed(st);
    crate::metadata::details::now_playing(st);
}

thread_local! {
    /// Why no device can be opened now (`Event::NoOutput`), shown in Settings > Output.
    static NO_OUTPUT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The engine cannot open a device (`Some`), or can again (`None`): said in the pill and in
/// Settings > Output, so a track never sits "playing" in silence without a word.
pub fn on_no_output(st: &Rc<State>, why: Option<String>) {
    match &why {
        Some(w) => {
            log::warn!("no sound output: {w}");
            views::flash_long(st, &format!("No sound: the output device cannot be opened ({w}). Check that speakers or headphones are connected; playback goes on as soon as one is."));
        }
        None => {
            if NO_OUTPUT.with(|n| n.borrow().is_some()) {
                views::flash(st, "Sound is back");
            }
        }
    }
    NO_OUTPUT.with(|n| *n.borrow_mut() = why);
    refresh(st);
}

/// List the devices again whenever the sound system says they or the default changed
/// (Linux: PipeWire), a moment after the last of a burst: the equalizer and Settings > Output
/// name the default as it is now, also while nothing plays. Call once, at start.
pub fn watch_devices() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PENDING: AtomicBool = AtomicBool::new(false);
    engine_output::on_devices_changed(|| {
        if !PENDING.swap(true, Ordering::AcqRel) {
            let _ = slint::invoke_from_event_loop(|| {
                slint::Timer::single_shot(std::time::Duration::from_millis(250), || {
                    PENDING.store(false, Ordering::Release);
                    // Where the stream plays may have changed while paused or stopped too.
                    state::with(|st| st.player.devices_changed());
                    rescan_devices();
                });
            });
        }
    });
}

/// List the devices again (Settings opened, and at start), then refresh the status line
/// and the equalizer (a device with its own setting may be the default).
pub fn rescan_devices() {
    list_then(|st, _| {
        refresh(st);
        crate::eq::device_changed(st);
    });
}

/// Each listing runs on a thread of its own and they can finish out of order: `DEVICES` keeps
/// the newest asked for, and `f` gets its own.
fn list_then(f: impl FnOnce(&Rc<State>, &[DeviceInfo]) + Send + 'static) {
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::Builder::new()
        .name("mp-devices".into())
        .spawn(move || {
            let list = engine_output::devices().unwrap_or_else(|e| {
                log::warn!("listing output devices: {e}");
                Vec::new()
            });
            state::post(move |st| {
                if LISTED.with(|l| l.replace(l.get().max(n))) < n {
                    DEVICES.with(|d| *d.borrow_mut() = Some(list.clone()));
                }
                f(st, &list);
            });
        })
        .ok();
}

/// The status line and the button's label.
pub fn status(s: &Settings, info: Option<&OutputInfo>, devices: Option<&[DeviceInfo]>) -> (String, String) {
    let chosen_name = s.output_device_name.clone().unwrap_or_else(|| "The chosen device".into());
    let label = match &s.output_device {
        None => "System default".to_owned(),
        Some(id) => devices.and_then(|d| d.iter().find(|d| &d.id == id)).map(|d| d.name.clone()).unwrap_or_else(|| chosen_name.clone()),
    };
    let present = |id: &str| devices.is_none_or(|d| d.iter().any(|d| d.id == id));
    let text = match info {
        Some(i) if i.missing => format!("{chosen_name} is not connected: playing on the system default ({}) until it is back.", i.device),
        Some(i) => {
            let mut t = format!("Playing on {}", i.device);
            match (i.server, &i.exclusive, &i.note) {
                (None, Some(f), _) => t.push_str(&format!(", exclusive, {f}.")),
                (None, None, Some(n)) => t.push_str(&format!(", shared. {n}.")),
                (None, None, None) => t.push('.'),
                // Through a sound server (Linux): its bit-perfect request granted, or why not.
                (Some(server), Some(f), _) => t.push_str(&format!(" through {server}, {f}.")),
                (Some(server), None, Some(n)) => t.push_str(&format!(" through {server}. {n}.")),
                (Some(server), None, None) => t.push_str(&format!(" through {server}.")),
            }
            t
        }
        None => match &s.output_device {
            Some(id) if !present(id) => format!("{chosen_name} is not connected: the system default plays until it is back."),
            _ => crate::platform::system::words().default_output.into(),
        },
    };
    (label, text)
}

/// Not playing as chosen: the chosen device is away, or exclusive mode fell back to
/// shared (Settings marks its Output section).
pub fn attention(s: &Settings, info: Option<&OutputInfo>, devices: Option<&[DeviceInfo]>) -> bool {
    match info {
        Some(i) => i.missing || (s.exclusive && i.exclusive.is_none() && i.note.is_some()),
        None => s.output_device.as_ref().is_some_and(|id| devices.is_some_and(|d| !d.iter().any(|d| &d.id == id))),
    }
}

pub fn refresh(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let s = st.settings.borrow();
    let (label, text, attention) = DEVICES.with(|d| {
        INFO.with(|i| {
            let (info, devices) = (i.borrow(), d.borrow());
            let (label, text) = status(&s, info.as_ref(), devices.as_deref());
            (label, text, attention(&s, info.as_ref(), devices.as_deref()))
        })
    });
    let failing = NO_OUTPUT.with(|n| n.borrow().clone());
    let (text, attention) = match failing {
        Some(why) => (format!("No sound: the output device cannot be opened ({why}). Playback goes on as soon as one can be."), true),
        None => (text, attention),
    };
    let o = app.global::<Output>();
    o.set_device_label(label.into());
    o.set_status(text.into());
    o.set_exclusive(s.exclusive);
    o.set_attention(attention);
}

/// The device menu: the system default, then every active device; the choice checked.
pub fn entries(s: &Settings, devices: &[DeviceInfo]) -> Vec<Entry> {
    let pick = |label: String, act: Act, on: bool| if on { Entry::Checked { label, act } } else { Entry::Item { label, act } };
    let default = devices.iter().find(|d| d.default).map_or("System default".to_owned(), |d| format!("System default ({})", d.name));
    let mut v = vec![pick(default, Act::SetOutput(None), s.output_device.is_none()), Entry::Separator];
    for d in devices {
        v.push(pick(d.name.clone(), Act::SetOutput(Some(d.id.clone())), s.output_device.as_deref() == Some(d.id.as_str())));
    }
    if let Some(id) = &s.output_device
        && !devices.iter().any(|d| &d.id == id)
    {
        let name = s.output_device_name.clone().unwrap_or_else(|| "Chosen device".into());
        v.push(Entry::Checked { label: format!("{name} (not connected)"), act: Act::SetOutput(Some(id.clone())) });
    }
    v
}

pub fn open_menu(x: f32, y: f32) {
    list_then(move |st, list| {
        let e = entries(&st.settings.borrow(), list);
        crate::menus::show_entries(st, e, x, y);
        refresh(st);
    });
}

pub fn choose(st: &Rc<State>, id: Option<String>) {
    let name = id.as_ref().and_then(|id| DEVICES.with(|d| d.borrow().as_ref()?.iter().find(|d| &d.id == id).map(|d| d.name.clone())));
    {
        let mut s = st.settings.borrow_mut();
        if s.output_device == id {
            return;
        }
        if id.is_some() {
            s.output_device_name = name.or_else(|| s.output_device_name.clone());
        }
        s.output_device = id;
    }
    st.save_settings_soon();
    st.player.set_output(config(&st.settings.borrow()));
    refresh(st);
    crate::eq::device_changed(st);
}

pub fn toggle_exclusive(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        s.exclusive = !s.exclusive;
    }
    st.save_settings_soon();
    st.player.set_output(config(&st.settings.borrow()));
    refresh(st);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, default: bool) -> DeviceInfo {
        DeviceInfo { id: id.into(), name: name.into(), default }
    }

    fn labels(v: &[Entry]) -> Vec<String> {
        v.iter()
            .map(|e| match e {
                Entry::Item { label, .. } | Entry::Playlists { label } | Entry::Choices { label, .. } | Entry::Heading { label } | Entry::Disabled { label } => label.clone(),
                Entry::Checked { label, .. } => format!("✓{label}"),
                Entry::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn the_menu_checks_the_choice_and_keeps_a_missing_one() {
        let devices = [dev("a", "Headphones", false), dev("b", "Speakers", true)];
        let s = Settings::default();
        assert_eq!(labels(&entries(&s, &devices)), ["✓System default (Speakers)", "-", "Headphones", "Speakers"]);
        let s = Settings { output_device: Some("a".into()), output_device_name: Some("Headphones".into()), ..Settings::default() };
        assert_eq!(labels(&entries(&s, &devices)), ["System default (Speakers)", "-", "✓Headphones", "Speakers"]);
        let s = Settings { output_device: Some("dac".into()), output_device_name: Some("USB DAC".into()), ..Settings::default() };
        assert_eq!(labels(&entries(&s, &devices)), ["System default (Speakers)", "-", "Headphones", "Speakers", "✓USB DAC (not connected)"]);
    }

    #[test]
    fn the_status_says_where_it_plays_and_when_it_fell_back() {
        let s = Settings { output_device: Some("dac".into()), output_device_name: Some("USB DAC".into()), ..Settings::default() };
        let devices = [dev("b", "Speakers", true)];
        let (label, text) = status(&s, None, Some(&devices));
        assert_eq!(label, "USB DAC");
        assert!(text.starts_with("USB DAC is not connected"), "{text}");
        let info = OutputInfo { device: "Speakers".into(), id: "b".into(), missing: true, exclusive: None, note: None, ..Default::default() };
        assert!(status(&s, Some(&info), Some(&devices)).1.contains("playing on the system default (Speakers)"));
        let info = OutputInfo { device: "USB DAC".into(), id: "dac".into(), missing: false, exclusive: Some("24-bit / 96 kHz".into()), note: None, ..Default::default() };
        assert_eq!(status(&s, Some(&info), None).1, "Playing on USB DAC, exclusive, 24-bit / 96 kHz.");
        let info = OutputInfo { note: Some("The device is in use by another application; playing shared".into()), exclusive: None, ..info };
        assert!(status(&s, Some(&info), None).1.contains("shared. The device is in use"));
    }

    #[test]
    fn attention_when_not_playing_as_chosen() {
        let devices = [dev("b", "Speakers", true)];
        let chosen = Settings { output_device: Some("dac".into()), ..Settings::default() };
        assert!(attention(&chosen, None, Some(&devices)), "the chosen device is away");
        assert!(!attention(&chosen, None, None), "not listed yet: nothing to say");
        assert!(!attention(&Settings::default(), None, Some(&devices)));
        let away = OutputInfo { device: "Speakers".into(), id: "b".into(), missing: true, exclusive: None, note: None, ..Default::default() };
        assert!(attention(&chosen, Some(&away), Some(&devices)));
        let shared = OutputInfo { missing: false, note: Some("The device is in use".into()), ..away.clone() };
        let excl = Settings { exclusive: true, ..Settings::default() };
        assert!(attention(&excl, Some(&shared), None), "exclusive fell back to shared");
        let fine = OutputInfo { note: None, exclusive: Some("24-bit / 48 kHz".into()), ..shared };
        assert!(!attention(&excl, Some(&fine), None));
    }
}
