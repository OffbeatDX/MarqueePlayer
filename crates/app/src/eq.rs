//! Settings > Equalizer: the setting that plays (for every device, or one device's own),
//! the presets (built in, the user's, imported, pasted), copy and paste, and the panel's
//! state (`ui/eq.slint`). The filters are the engine's (`player_core::eq`); this keeps the
//! setting in `settings.json`, hands it to the engine whenever it or the output device
//! changes, and draws the curve from the same filters, so what is shown is what plays.
//!
//! - Ten sliders (the graphic layout) for everyone, the bands in full (kind, frequency,
//!   gain, width; up to `MAX_BANDS`) for those who want them. Looking at a setting the
//!   sliders cannot show changes nothing: the sliders show the closest they come and say so.
//! - Any edit, and choosing a preset, turns the equalizer on: what was asked for is heard.
//! - Copy puts the setting on the clipboard as Equalizer APO text (also "Copy as" GraphicEQ
//!   or an EasyEffects preset). Paste, and Import from a file, read any format
//!   `player_core::eq::formats` knows, off the UI thread (a GraphicEQ curve is fitted), and
//!   show what would apply (the curve against the current one, the filters, the preamp,
//!   what was left out or approximated) until the user applies it or cancels.
//! - A device can have a setting of its own ("Separate setting for Headphones"): used
//!   whenever that device plays, by its id, the system default following whatever device
//!   the system plays on. The others share one.
//! - Exports as Equalizer APO text or this app's own preset file (JSON, version 2).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use player_core::eq::formats::{self, Import, NoteKind, Refusal};
use player_core::eq::presets::{self, BUILT_IN, GRAPHIC_FREQS};
use player_core::eq::{self as peq, Band, EqParams, Kind, Preamp, Width, apo};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::settings::{DeviceEq, EqBandSetting, EqCurve, EqKindSetting, EqWidthSetting, Settings, UserPreset};
use crate::state::State;
use crate::{Dialog, EqPreview, Equalizer, MenuStore, views};

/// The ten flat sliders.
pub fn flat_bands() -> Vec<EqBandSetting> {
    presets::graphic(&[0.0; 10]).iter().map(from_band).collect()
}

fn kind_of(k: EqKindSetting) -> Kind {
    match k {
        EqKindSetting::Peak => Kind::Peak,
        EqKindSetting::LowShelf => Kind::LowShelf,
        EqKindSetting::HighShelf => Kind::HighShelf,
        EqKindSetting::LowPass => Kind::LowPass,
        EqKindSetting::HighPass => Kind::HighPass,
        EqKindSetting::BandPass => Kind::BandPass,
        EqKindSetting::Notch => Kind::Notch,
        EqKindSetting::AllPass => Kind::AllPass,
    }
}

fn kind_setting(k: Kind) -> EqKindSetting {
    match k {
        Kind::Peak => EqKindSetting::Peak,
        Kind::LowShelf => EqKindSetting::LowShelf,
        Kind::HighShelf => EqKindSetting::HighShelf,
        Kind::LowPass => EqKindSetting::LowPass,
        Kind::HighPass => EqKindSetting::HighPass,
        Kind::BandPass => EqKindSetting::BandPass,
        Kind::Notch => EqKindSetting::Notch,
        Kind::AllPass => EqKindSetting::AllPass,
    }
}

pub fn to_band(b: &EqBandSetting) -> Band {
    let width = match b.width {
        EqWidthSetting::Q => Width::Q,
        EqWidthSetting::Octaves => Width::Octaves,
        EqWidthSetting::Slope => Width::Slope,
    };
    Band::with_width(kind_of(b.kind), b.freq, b.gain, b.q, width)
}

pub fn from_band(b: &Band) -> EqBandSetting {
    let width = match b.width {
        Width::Q => EqWidthSetting::Q,
        Width::Octaves => EqWidthSetting::Octaves,
        Width::Slope => EqWidthSetting::Slope,
    };
    EqBandSetting { kind: kind_setting(b.kind), freq: b.freq, gain: b.gain_db, q: b.q, width }
}

fn bands_of(c: &EqCurve) -> Vec<Band> {
    c.bands.iter().map(to_band).collect()
}

fn preamp_of(auto: bool, db: f32) -> Preamp {
    if auto { Preamp::Auto } else { Preamp::Manual(db) }
}

/// What the engine gets for a setting.
pub fn params(c: &EqCurve) -> EqParams {
    EqParams::new(c.enabled, &bands_of(c), preamp_of(c.preamp_auto, c.preamp_db))
}

/// The id of the device that plays (or will): what the engine last opened, else the chosen
/// device, else the system default as last listed.
pub fn device_id(s: &Settings) -> Option<String> {
    crate::output::info().map(|i| i.id).filter(|id| !id.is_empty()).or_else(|| s.output_device.clone()).or_else(crate::output::default_device_id)
}

/// The name of that device, to show.
fn device_name(s: &Settings) -> Option<String> {
    if let Some(i) = crate::output::info()
        && !i.device.is_empty()
    {
        return Some(i.device);
    }
    let id = device_id(s)?;
    crate::output::device_name(&id).or_else(|| s.equalizer.devices.get(&id).map(|d| d.name.clone())).or_else(|| s.output_device_name.clone().filter(|_| s.output_device.as_ref() == Some(&id)))
}

/// The setting that plays now: the playing device's own, else the shared one.
pub fn current(s: &Settings) -> &EqCurve {
    device_id(s).and_then(|id| s.equalizer.devices.get(&id)).map_or(&s.equalizer.all, |d| &d.curve)
}

fn with_current<R>(s: &mut Settings, f: impl FnOnce(&mut EqCurve) -> R) -> R {
    match device_id(s).filter(|id| s.equalizer.devices.contains_key(id)) {
        Some(id) => f(&mut s.equalizer.devices.get_mut(&id).unwrap().curve),
        None => f(&mut s.equalizer.all),
    }
}

thread_local! {
    /// The view the user chose (true: the bands in full); `None`: the sliders when the
    /// setting is the ten sliders, else the bands.
    static VIEW: Cell<Option<bool>> = const { Cell::new(None) };
    /// The name prompt is asking for a preset's name: to save it as, or to rename it.
    static PROMPT: RefCell<Option<PromptFor>> = const { RefCell::new(None) };
    /// What the engine was last given, so an unchanged setting is not sent again.
    static SENT: RefCell<Option<EqParams>> = const { RefCell::new(None) };
    /// What the preview shows, to apply on Accept.
    static PREVIEW: RefCell<Option<Pending>> = const { RefCell::new(None) };
    /// The latest read asked for (a newer one wins; an older one's result is dropped).
    static READ_SEQ: Cell<u64> = const { Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq)]
enum PromptFor {
    Save { replace: Option<String> },
    Rename(String),
}

/// A read waiting for the user's yes.
struct Pending {
    import: Import,
    /// The preset's name it will have.
    name: String,
    /// One of the user's presets is exactly this (chosen again, not added twice).
    existing: bool,
}

/// At start, once the state exists: the setting for the device that will play.
pub fn install(st: &Rc<State>) {
    apply(st);
}

/// Hand the engine the setting that plays now, and show it.
pub fn apply(st: &Rc<State>) {
    let p = params(current(&st.settings.borrow()));
    if SENT.with(|s| s.borrow().as_ref() != Some(&p)) {
        st.player.set_eq(&p);
        SENT.with(|s| *s.borrow_mut() = Some(p));
    }
    refresh(st);
    crate::metadata::details::now_playing(st);
}

/// The output device changed (or became known): its own setting, or the shared one.
pub fn device_changed(st: &Rc<State>) {
    apply(st);
}

/// "Bass boost", "Custom", or a device's preset; for the player bar and the details.
pub fn summary(s: &Settings) -> String {
    let c = current(s);
    c.preset.clone().unwrap_or_else(|| "Custom".into())
}

/// The equalizer changes the sound now (on, and something not at 0 dB).
pub fn changes_sound(s: &Settings) -> bool {
    params(current(s)).changes_sound()
}

/// Settings > Equalizer is the page shown (its copy and paste keys act only there).
pub fn shown(st: &Rc<State>) -> bool {
    st.history.borrow().current == crate::nav::Route::Settings(crate::settings::nav::EQUALIZER)
}

/// "105 Hz", "1.2 kHz", "16 kHz".
pub fn freq_text(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k * 10.0).round() % 10.0 == 0.0 { format!("{:.0} kHz", k) } else { format!("{:.1} kHz", k) }
    } else if f >= 100.0 {
        format!("{:.0} Hz", f)
    } else {
        let t = format!("{:.1}", f);
        format!("{} Hz", t.trim_end_matches('0').trim_end_matches('.'))
    }
}

/// "+4.5 dB", "−3.0 dB", "0 dB".
pub fn db_text(db: f64) -> String {
    let r = (db * 10.0).round() / 10.0;
    if r == 0.0 {
        "0 dB".into()
    } else if r > 0.0 {
        format!("+{r:.1} dB")
    } else {
        format!("\u{2212}{:.1} dB", -r)
    }
}

/// A band's width as Settings shows it: the label and the value ("Q", "0.71";
/// "Width", "1.5 oct"; "Slope", "10.8 dB/oct").
pub fn width_text(b: &Band) -> (&'static str, String) {
    match b.width {
        Width::Q => ("Q", format!("{:.2}", b.q)),
        Width::Octaves => ("Width", format!("{} oct", apo::num(f64::from(b.q), 2))),
        Width::Slope => ("Slope", format!("{} dB/oct", apo::num(f64::from(b.q) * 12.0, 1))),
    }
}

/// The curve's frequency span: ten octaves around the sliders' centres, so each slider
/// stands under its own octave of the curve.
pub const CURVE_LO: f64 = 31.5 / std::f64::consts::SQRT_2;
pub const CURVE_OCTAVES: f64 = 10.0;
/// The curve's dB span, either way.
pub const CURVE_DB: f64 = 15.0;
/// The path's view box.
pub const VIEW_W: f64 = 1000.0;
pub const VIEW_H: f64 = 240.0;

pub fn x_of(freq: f64) -> f64 {
    ((freq / CURVE_LO).log2() / CURVE_OCTAVES * VIEW_W).clamp(0.0, VIEW_W)
}

pub fn y_of(db: f64) -> f64 {
    (VIEW_H / 2.0 - db / CURVE_DB * VIEW_H / 2.0).clamp(0.0, VIEW_H)
}

const CURVE_N: usize = 240;

fn curve_freqs() -> Vec<f64> {
    (0..=CURVE_N).map(|i| CURVE_LO * 2f64.powf(CURVE_OCTAVES * i as f64 / CURVE_N as f64)).collect()
}

/// A dB curve (one value per `curve_freqs`) as path commands.
fn path_of(db: &[f64]) -> String {
    let mut line = String::with_capacity(CURVE_N * 16);
    for (i, (&f, &d)) in curve_freqs().iter().zip(db).enumerate() {
        let (x, y) = (x_of(f), y_of(d));
        line.push_str(&format!("{}{x:.1} {y:.1} ", if i == 0 { "M" } else { "L" }));
    }
    line
}

/// The combined curve as path commands (and the area under it to 0 dB), plus `offset` dB.
pub fn curve_paths(bands: &[Band], rate: u32, offset: f64) -> (String, String) {
    let mut db = Vec::new();
    peq::response_curve(bands, rate, &curve_freqs(), &mut db);
    db.iter_mut().for_each(|d| *d += offset);
    let line = path_of(&db);
    let zero = y_of(0.0);
    let fill = format!("{line}L {VIEW_W:.1} {zero:.1} L 0 {zero:.1} Z");
    (line, fill)
}

fn rate_shown() -> u32 {
    crate::output::info().map(|i| i.rate).filter(|&r| r > 0).unwrap_or(48_000)
}

/// What the sliders cannot show of `bands`: the largest difference (dB, 20 Hz to 20 kHz)
/// between their curve and the ten sliders' closest fit, and the kinds no slider is.
fn graphic_shortfall(bands: &[Band], fit: &[f32; 10]) -> (f64, Vec<&'static str>) {
    let sliders = presets::graphic(fit);
    let worst = (0..=120).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 120.0)).map(|f| (peq::response_db(bands, 48_000, f) - peq::response_db(&sliders, 48_000, f)).abs()).fold(0.0, f64::max);
    let mut kinds: Vec<&'static str> = Vec::new();
    for b in bands {
        let name = match b.kind {
            Kind::Peak | Kind::LowShelf | Kind::HighShelf => continue,
            Kind::LowPass => "a low-pass filter",
            Kind::HighPass => "a high-pass filter",
            Kind::BandPass => "a band-pass filter",
            Kind::Notch => "a notch",
            Kind::AllPass => "an all-pass filter",
        };
        if !kinds.contains(&name) {
            kinds.push(name);
        }
    }
    (worst, kinds)
}

/// "a, b and c".
fn and_list(v: &[&str]) -> String {
    match v {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn band_rows(bands: &[Band], rate: u32) -> Vec<crate::EqBandRow> {
    bands
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (wl, wv) = width_text(b);
            crate::EqBandRow {
                kind: b.kind.index() as i32,
                kind_name: b.kind.name().into(),
                has_gain: b.kind.has_gain(),
                freq: freq_text(b.freq).into(),
                gain: if b.kind.has_gain() { db_text(f64::from(b.gain_db)).into() } else { "\u{2014}".into() },
                q: wv.into(),
                width_label: wl.into(),
                x: (x_of(f64::from(b.freq)) / VIEW_W) as f32,
                y: (y_of(peq::response_db(bands, rate, f64::from(b.freq))) / VIEW_H) as f32,
                number: (i + 1) as i32,
            }
        })
        .collect()
}

/// Fill the panel (`Equalizer` in stores.slint).
pub fn refresh(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let s = st.settings.borrow();
    let c = current(&s);
    let e = app.global::<Equalizer>();
    let bands = bands_of(c);
    let rate = rate_shown();
    let graphic = presets::as_graphic(&bands);
    let parametric = VIEW.with(Cell::get).unwrap_or(graphic.is_none());
    e.set_on(c.enabled);
    e.set_preset(summary(&s).into());
    e.set_graphic(!parametric);
    let gains: [f32; 10] = graphic.unwrap_or_else(|| presets::graphic_from(&bands));
    e.set_gains(ModelRc::new(VecModel::from(gains.to_vec())));
    e.set_gain_texts(ModelRc::new(VecModel::from(gains.iter().map(|&g| SharedString::from(db_text(f64::from(g)).replace(" dB", ""))).collect::<Vec<_>>())));
    // The sliders shown for a setting that is not them: say so, and what moving one does.
    let note = if !parametric && graphic.is_none() {
        let (worst, kinds) = graphic_shortfall(&bands, &gains);
        let incl = if kinds.is_empty() { String::new() } else { format!(", including {}", and_list(&kinds)) };
        format!(
            "This setting is {} bands{incl}, not the ten sliders: they show the closest they come (up to {:.1} dB off). Nothing changes until you move one, which replaces the bands with ten sliders; Parametric shows the bands as they are.",
            bands.len(),
            worst
        )
    } else {
        String::new()
    };
    e.set_graphic_note(note.into());
    e.set_bands(ModelRc::new(VecModel::from(band_rows(&bands, rate))));
    e.set_can_add(bands.len() < peq::MAX_BANDS);
    e.set_max_bands(peq::MAX_BANDS as i32);
    let p = params(c);
    let pre = p.preamp_db(rate);
    e.set_auto_preamp(c.preamp_auto);
    e.set_preamp(db_text(pre).into());
    let (line, fill) = curve_paths(&bands, rate, 0.0);
    e.set_curve(line.into());
    e.set_curve_fill(fill.into());
    let (total, _) = curve_paths(&bands, rate, pre);
    e.set_curve_total(if pre.abs() >= 0.05 { total.into() } else { SharedString::new() });
    e.set_zero_y((y_of(0.0) / VIEW_H) as f32);
    let id = device_id(&s);
    let own = id.as_ref().is_some_and(|id| s.equalizer.devices.contains_key(id));
    e.set_device(device_name(&s).unwrap_or_default().into());
    e.set_per_device(own);
    e.set_others((s.equalizer.devices.len() - usize::from(own)) as i32);
    e.set_rate_note(if rate == 48_000 { SharedString::new() } else { format!("Drawn for {} kHz, the rate playing", library::audio::khz(rate)).into() });
    e.set_custom(c.preset.is_none());
    e.set_is_user(c.preset.as_ref().is_some_and(|n| s.equalizer.presets.iter().any(|p| &p.name == n)));
}

/// Save, hand over, show.
fn changed(st: &Rc<State>) {
    st.save_settings_soon();
    apply(st);
}

/// An edit of the playing setting: it turns the equalizer on and is no longer a preset.
fn edit(st: &Rc<State>, f: impl FnOnce(&mut EqCurve)) {
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| {
            f(c);
            c.enabled = true;
            c.preset = None;
        });
    }
    changed(st);
}

pub fn toggle(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| c.enabled = !c.enabled);
    }
    changed(st);
}

/// A slider moved (dB, snapped to 0.5 dB like the keys). On a setting that is not the ten
/// sliders this is where it becomes them (the graphic view said so beforehand).
pub fn set_gain(st: &Rc<State>, i: usize, db: f32) {
    if i >= 10 {
        return;
    }
    let db = ((db * 2.0).round() / 2.0).clamp(-presets::GRAPHIC_MAX_DB, presets::GRAPHIC_MAX_DB);
    let (unchanged, was_graphic) = {
        let s = st.settings.borrow();
        let c = current(&s);
        let g = presets::as_graphic(&bands_of(c));
        (g.is_some_and(|g| g[i] == db) && c.enabled, g.is_some())
    };
    if unchanged {
        return;
    }
    edit(st, |c| {
        let mut g = presets::as_graphic(&bands_of(c)).unwrap_or_else(|| presets::graphic_from(&bands_of(c)));
        g[i] = db;
        c.bands = presets::graphic(&g).iter().map(from_band).collect();
    });
    if !was_graphic {
        views::flash(st, "Now ten sliders: the closest they come to the bands that were");
    }
}

/// Graphic or every band. Only the view changes: a setting the sliders cannot show stays
/// as it is (they show the closest they come, and say so).
pub fn set_mode(st: &Rc<State>, parametric: bool) {
    VIEW.with(|p| p.set(Some(parametric)));
    refresh(st);
}

/// Every band back at 0 dB (the ten sliders), the preamp auto.
pub fn reset(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| {
            c.bands = flat_bands();
            c.preamp_auto = true;
            c.preamp_db = 0.0;
            c.preset = Some("Flat".into());
        });
    }
    VIEW.with(|p| p.set(None));
    changed(st);
}

/// One band's kind (`Kind::ALL`'s index). A width the new kind cannot have becomes its Q.
pub fn set_kind(st: &Rc<State>, i: usize, kind: i32) {
    let kind = Kind::from_index(kind.max(0) as usize);
    VIEW.with(|p| p.set(Some(true)));
    edit(st, |c| {
        if let Some(b) = c.bands.get_mut(i) {
            let mut band = to_band(b);
            // A gain kept for a peak or shelf that comes back from a filter without one.
            let gain = b.gain;
            band.kind = kind;
            band.gain_db = gain;
            *b = from_band(&band.clamped());
            if !kind.has_gain() {
                b.gain = gain;
            }
        }
    });
}

/// The menu of kinds for band `i` (its kind button), at the button.
pub fn open_kind_menu(st: &Rc<State>, i: usize, x: f32, y: f32) {
    use crate::menus::{Act, Entry};
    let current = current(&st.settings.borrow()).bands.get(i).map(|b| kind_of(b.kind));
    let entries: Vec<Entry> = Kind::ALL
        .iter()
        .map(|&k| {
            let act = Act::EqBandKind(i, k.index());
            if Some(k) == current { Entry::Checked { label: k.name().into(), act } } else { Entry::Item { label: k.name().into(), act } }
        })
        .collect();
    crate::menus::show_entries(st, entries, x, y);
}

/// Steps for the band editor: a twelfth of an octave, half a dB, a tenth of the Q or
/// bandwidth, half a dB per octave of a slope.
pub fn step_band(st: &Rc<State>, i: usize, field: &str, dir: i32) {
    let d = dir.signum() as f32;
    VIEW.with(|p| p.set(Some(true)));
    edit(st, |c| {
        let Some(b) = c.bands.get_mut(i) else { return };
        let mut band = to_band(b);
        match field {
            "freq" => band.freq = (band.freq * 2f32.powf(d / 12.0)).clamp(peq::FREQ_MIN, 20_000.0),
            "gain" if band.kind.has_gain() => band.gain_db = ((band.gain_db * 2.0).round() / 2.0 + d * 0.5).clamp(-peq::GAIN_MAX, peq::GAIN_MAX),
            "q" => {
                let (lo, hi) = band.width.range();
                band.q = match band.width {
                    Width::Slope => ((band.q * 24.0).round() + d) / 24.0,
                    _ => band.q * 1.1f32.powf(d),
                }
                .clamp(lo, hi);
            }
            _ => {}
        }
        let gain = b.gain;
        *b = from_band(&band.clamped());
        if !band.kind.has_gain() {
            b.gain = gain;
        }
    });
}

pub fn remove_band(st: &Rc<State>, i: usize) {
    VIEW.with(|p| p.set(Some(true)));
    edit(st, |c| {
        if i < c.bands.len() {
            c.bands.remove(i);
        }
    });
}

pub fn add_band(st: &Rc<State>) {
    VIEW.with(|p| p.set(Some(true)));
    edit(st, |c| {
        if c.bands.len() < peq::MAX_BANDS {
            c.bands.push(EqBandSetting { kind: EqKindSetting::Peak, freq: 1000.0, gain: 0.0, q: 1.0, width: EqWidthSetting::Q });
        }
    });
}

/// Auto (0) or set by hand (1).
pub fn set_preamp_mode(st: &Rc<State>, manual: bool) {
    let rate = rate_shown();
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| {
            if manual && c.preamp_auto {
                // Starts where auto was.
                c.preamp_db = ((params(c).preamp_db(rate) * 2.0).round() / 2.0) as f32;
            }
            c.preamp_auto = !manual;
        });
    }
    changed(st);
}

pub fn step_preamp(st: &Rc<State>, dir: i32) {
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| {
            c.preamp_auto = false;
            c.preamp_db = ((c.preamp_db * 2.0).round() / 2.0 + dir.signum() as f32 * 0.5).clamp(peq::PREAMP_MIN, peq::PREAMP_MAX);
        });
    }
    changed(st);
}

/// "Separate setting for <device>": on, the device gets a copy of what plays now, kept
/// for it; off, it goes back to the shared one.
pub fn toggle_per_device(st: &Rc<State>) {
    {
        let mut s = st.settings.borrow_mut();
        let Some(id) = device_id(&s) else { return };
        if s.equalizer.devices.remove(&id).is_none() {
            let name = device_name(&s).unwrap_or_else(|| "This device".into());
            let curve = s.equalizer.all.clone();
            s.equalizer.devices.insert(id, DeviceEq { name, curve });
        }
    }
    changed(st);
}

/// Choose a preset by name: a built-in one or the user's. It turns the equalizer on.
pub fn choose(st: &Rc<State>, name: &str) {
    let found = {
        let s = st.settings.borrow();
        if let Some(p) = presets::built_in(name) {
            Some((p.name.to_owned(), p.bands().iter().map(from_band).collect::<Vec<_>>(), true, 0.0))
        } else {
            s.equalizer.presets.iter().find(|p| p.name == name).map(|p| (p.name.clone(), p.bands.clone(), p.preamp_auto, p.preamp_db))
        }
    };
    let Some((name, bands, auto, db)) = found else { return };
    VIEW.with(|p| p.set(None));
    {
        let mut s = st.settings.borrow_mut();
        with_current(&mut s, |c| {
            c.bands = bands;
            c.preamp_auto = auto;
            c.preamp_db = db;
            c.preset = Some(name);
            c.enabled = true;
        });
    }
    changed(st);
}

/// The preset picker's menu: the built-in presets, the user's, then what can be done.
pub fn menu_entries(s: &Settings) -> Vec<crate::menus::Entry> {
    use crate::menus::{Act, Entry};
    let c = current(s);
    let pick = |name: &str| {
        let act = Act::EqPreset(name.to_owned());
        if c.preset.as_deref() == Some(name) { Entry::Checked { label: name.to_owned(), act } } else { Entry::Item { label: name.to_owned(), act } }
    };
    let mut v: Vec<Entry> = BUILT_IN.iter().map(|p| pick(p.name)).collect();
    if !s.equalizer.presets.is_empty() {
        v.push(Entry::Separator);
        v.extend(s.equalizer.presets.iter().map(|p| pick(&p.name)));
    }
    v.push(Entry::Separator);
    v.push(Entry::Item { label: "Save as preset\u{2026}".into(), act: Act::EqSave });
    if let Some(name) = c.preset.as_ref().filter(|n| s.equalizer.presets.iter().any(|p| &p.name == *n)) {
        v.push(Entry::Item { label: format!("Rename \u{201c}{name}\u{201d}\u{2026}"), act: Act::EqRename });
        v.push(Entry::Item { label: format!("Delete \u{201c}{name}\u{201d}"), act: Act::EqDelete });
    }
    v.push(Entry::Item { label: "Import a preset file\u{2026}".into(), act: Act::EqImport });
    v.push(Entry::Item { label: "Export as Equalizer APO text\u{2026}".into(), act: Act::EqExport(ExportAs::Apo) });
    v.push(Entry::Item { label: "Export as a Marquee Player preset\u{2026}".into(), act: Act::EqExport(ExportAs::Marquee) });
    v
}

pub fn open_menu(st: &Rc<State>, x: f32, y: f32) {
    let e = menu_entries(&st.settings.borrow());
    crate::menus::show_entries(st, e, x, y);
}

/// A unique name for a new preset: `name`, or `name (2)`, `(3)`...
pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    (2..).map(|i| format!("{name} ({i})")).find(|n| !taken(n)).unwrap()
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn open_prompt(st: &Rc<State>, purpose: PromptFor, title: &str, prefill: &str, confirm: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    if d.get_open() {
        return;
    }
    let (x, y) = (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y());
    PROMPT.with(|p| *p.borrow_mut() = Some(purpose));
    d.set_prompt_title(title.into());
    d.set_prompt_text(prefill.into());
    d.set_prompt_note("".into());
    d.set_prompt_confirm(confirm.into());
    d.set_prompt_placeholder("Preset name".into());
    d.set_x(x);
    d.set_y(y);
    d.set_returns(false);
    d.set_prompt_open(true);
}

/// "Save as preset…": asks for a name.
pub fn save_prompt(st: &Rc<State>) {
    let prefill = {
        let s = st.settings.borrow();
        match current(&s).preset.clone() {
            Some(n) if presets::built_in(&n).is_none() => n,
            Some(n) => format!("{n} (mine)"),
            None => "My preset".into(),
        }
    };
    open_prompt(st, PromptFor::Save { replace: None }, "Save as preset", &prefill, "Save");
}

pub fn rename_prompt(st: &Rc<State>) {
    let Some(name) = current(&st.settings.borrow()).preset.clone() else { return };
    open_prompt(st, PromptFor::Rename(name.clone()), "Rename preset", &name, "Rename");
}

/// The prompt is for a preset (not a playlist).
pub fn prompt_active() -> bool {
    PROMPT.with(|p| p.borrow().is_some())
}

pub fn prompt_edited(st: &Rc<State>, text: &str) {
    let offer = PROMPT.with(|p| matches!(&*p.borrow(), Some(PromptFor::Save { replace: Some(n) }) if !same(n, text)));
    if offer {
        PROMPT.with(|p| *p.borrow_mut() = Some(PromptFor::Save { replace: None }));
    }
    if let Some(app) = st.app.upgrade() {
        let d = app.global::<Dialog>();
        if offer || d.get_prompt_note() != "" {
            d.set_prompt_note("".into());
            d.set_prompt_confirm(if matches!(PROMPT.with(|p| p.borrow().clone()), Some(PromptFor::Rename(_))) { "Rename" } else { "Save" }.into());
        }
    }
}

/// What a name typed for a preset is.
#[derive(Debug, PartialEq)]
pub enum NameCheck {
    Empty,
    BuiltIn(&'static str),
    /// One of the user's presets has it (as written there).
    Taken(String),
    Free(String),
}

pub fn check_name(text: &str, user: &[UserPreset]) -> NameCheck {
    let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return NameCheck::Empty;
    }
    if let Some(p) = BUILT_IN.iter().find(|p| same(p.name, &name)) {
        return NameCheck::BuiltIn(p.name);
    }
    match user.iter().find(|p| same(&p.name, &name)) {
        Some(p) => NameCheck::Taken(p.name.clone()),
        None => NameCheck::Free(name),
    }
}

pub fn prompt_confirm(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    let Some(purpose) = PROMPT.with(|p| p.borrow().clone()) else { return };
    let check = check_name(text, &st.settings.borrow().equalizer.presets);
    let note = |t: &str| d.set_prompt_note(t.into());
    match (&purpose, check) {
        (_, NameCheck::Empty) => note("Type a name for the preset."),
        (_, NameCheck::BuiltIn(n)) => note(&format!("\u{201c}{n}\u{201d} is a built-in preset. Type another name.")),
        (PromptFor::Rename(old), NameCheck::Taken(n)) if !same(old, &n) => note(&format!("Another preset is called \u{201c}{n}\u{201d}. Type another name.")),
        (PromptFor::Save { replace }, NameCheck::Taken(n)) if replace.as_deref() != Some(n.as_str()) => {
            PROMPT.with(|p| *p.borrow_mut() = Some(PromptFor::Save { replace: Some(n.clone()) }));
            note(&format!("There is a preset called \u{201c}{n}\u{201d} already. Replace it, or type another name."));
            d.set_prompt_confirm("Replace".into());
        }
        (PromptFor::Save { .. }, NameCheck::Taken(name) | NameCheck::Free(name)) => {
            close_prompt(st);
            save_as(st, &name);
        }
        (PromptFor::Rename(old), NameCheck::Taken(name) | NameCheck::Free(name)) => {
            close_prompt(st);
            rename(st, old, &name);
        }
    }
}

pub fn prompt_cancel(st: &Rc<State>) {
    close_prompt(st);
}

fn close_prompt(st: &Rc<State>) {
    PROMPT.with(|p| *p.borrow_mut() = None);
    if let Some(app) = st.app.upgrade() {
        let d = app.global::<Dialog>();
        d.set_prompt_open(false);
        d.set_prompt_placeholder("Playlist name".into());
    }
}

/// Keep what plays as a preset called `name` (replacing one of that name).
pub fn save_as(st: &Rc<State>, name: &str) {
    {
        let mut s = st.settings.borrow_mut();
        let c = current(&s).clone();
        let p = UserPreset { name: name.to_owned(), bands: c.bands, preamp_auto: c.preamp_auto, preamp_db: c.preamp_db };
        match s.equalizer.presets.iter_mut().find(|q| same(&q.name, name)) {
            Some(q) => *q = p,
            None => s.equalizer.presets.push(p),
        }
        with_current(&mut s, |c| c.preset = Some(name.to_owned()));
    }
    changed(st);
    views::flash(st, &format!("Saved \u{201c}{name}\u{201d}"));
}

pub fn rename(st: &Rc<State>, old: &str, new: &str) {
    {
        let mut s = st.settings.borrow_mut();
        if let Some(p) = s.equalizer.presets.iter_mut().find(|p| p.name == old) {
            p.name = new.to_owned();
        }
        let eqs = &mut s.equalizer;
        for c in std::iter::once(&mut eqs.all).chain(eqs.devices.values_mut().map(|d| &mut d.curve)) {
            if c.preset.as_deref() == Some(old) {
                c.preset = Some(new.to_owned());
            }
        }
    }
    changed(st);
}

/// Delete the user preset that plays (the setting stays, as "Custom").
pub fn delete(st: &Rc<State>) {
    let name = {
        let mut s = st.settings.borrow_mut();
        let Some(name) = current(&s).preset.clone() else { return };
        s.equalizer.presets.retain(|p| p.name != name);
        let eqs = &mut s.equalizer;
        for c in std::iter::once(&mut eqs.all).chain(eqs.devices.values_mut().map(|d| &mut d.curve)) {
            if c.preset.as_deref() == Some(name.as_str()) {
                c.preset = None;
            }
        }
        name
    };
    changed(st);
    views::flash(st, &format!("Deleted \u{201c}{name}\u{201d}"));
}

// ---- Copy -------------------------------------------------------------------------------

/// What Copy writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyAs {
    /// Equalizer APO text (AutoEq, Peace, Room EQ Wizard read and write it).
    Apo,
    /// One GraphicEQ line (Wavelet, Equalizer APO).
    GraphicEq,
    /// An EasyEffects equalizer preset.
    EasyEffects,
}

/// The text Copy puts on the clipboard for the setting that plays, and what the pill says.
/// Whatever the app plays is what the text says: the sliders as the peaks they are, the
/// auto preamp as its value now (marked auto for this app), off as nothing.
pub fn copy_text(s: &Settings, as_: CopyAs) -> (String, String) {
    let c = current(s);
    let name = summary(s);
    let bands = bands_of(c);
    let rate = rate_shown();
    let p = params(c);
    let pre = p.preamp_db(rate);
    let auto_db = -peq::max_boost_db(&bands, rate);
    let off = if c.enabled { "" } else { " (it is off: written as playing nothing)" };
    match as_ {
        CopyAs::Apo => (apo::write(&name, &bands, preamp_of(c.preamp_auto, c.preamp_db), auto_db, c.enabled), format!("Equalizer copied{off}")),
        CopyAs::GraphicEq => {
            let text = if c.enabled { formats::graphic_eq(&bands, pre, rate) } else { formats::graphic_eq(&[], 0.0, rate) };
            (text, format!("Equalizer copied as GraphicEQ{off}"))
        }
        CopyAs::EasyEffects => {
            let (text, note) = formats::easyeffects_json(&bands, pre, c.enabled);
            let note = note.map(|n| format!(" ({n})")).unwrap_or_default();
            (text, format!("Equalizer copied as an EasyEffects preset{off}{note}"))
        }
    }
}

/// Copy (the button, the key, "Copy as").
pub fn copy(st: &Rc<State>, as_: CopyAs) {
    let (text, msg) = copy_text(&st.settings.borrow(), as_);
    if crate::explorer::copy_text(&text) {
        views::flash(st, &msg);
    } else {
        views::flash(st, "Could not use the clipboard");
    }
}

/// "Copy as…": the formats, at the button.
pub fn open_copy_menu(st: &Rc<State>, x: f32, y: f32) {
    use crate::menus::{Act, Entry};
    let entries = vec![
        Entry::Item { label: "Equalizer APO text (AutoEq, Peace, REW)".into(), act: Act::EqCopyAs(CopyAs::Apo) },
        Entry::Item { label: "GraphicEQ curve (Wavelet)".into(), act: Act::EqCopyAs(CopyAs::GraphicEq) },
        Entry::Item { label: "EasyEffects preset".into(), act: Act::EqCopyAs(CopyAs::EasyEffects) },
    ];
    crate::menus::show_entries(st, entries, x, y);
}

// ---- Paste and import, with a preview ---------------------------------------------------

/// Paste (the button, the key): read the clipboard and show what it would apply.
pub fn paste(st: &Rc<State>) {
    match crate::explorer::paste_text() {
        Err(e) => views::flash_long(st, &e),
        Ok(None) => views::flash_long(st, "Nothing to paste: the clipboard has no text"),
        Ok(Some(t)) if t.trim().is_empty() => views::flash_long(st, "Nothing to paste: the clipboard has no text"),
        Ok(Some(t)) => start_read(st, t.into_bytes(), None),
    }
}

/// Read a file and show what it would apply (nothing applies until the user says so).
/// Errors that stop it before reading (missing, far too large) are returned.
pub fn import_file(st: &Rc<State>, path: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
    if size > (formats::MAX_BYTES * 2) as u64 {
        return Err(format!("{} is too large to be an equalizer setting ({:.1} MB)", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), size as f64 / 1_048_576.0));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    start_read(st, bytes, name);
    Ok(())
}

/// Read off the UI thread (a GraphicEQ curve's fit takes a fraction of a second), then show
/// the preview. Only the newest read is shown.
fn start_read(st: &Rc<State>, bytes: Vec<u8>, file: Option<String>) {
    let seq = READ_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    if let Some(app) = st.app.upgrade() {
        app.global::<EqPreview>().set_reading(true);
    }
    let spawned = std::thread::Builder::new().name("mp-eq-read".into()).spawn(move || {
        let result = formats::read_bytes(&bytes, file.as_deref());
        let _ = slint::invoke_from_event_loop(move || {
            crate::state::with(|st| read_done(st, seq, result, file));
        });
    });
    if spawned.is_err() {
        views::flash(st, "Could not read it (no thread)");
    }
}

fn read_done(st: &Rc<State>, seq: u64, result: Result<Import, Refusal>, file: Option<String>) {
    if READ_SEQ.with(Cell::get) != seq {
        return;
    }
    let Some(app) = st.app.upgrade() else { return };
    app.global::<EqPreview>().set_reading(false);
    match result {
        Err(r) if r.format.is_none() => {
            // Said plainly, short enough for the pill; a more specific reason after it.
            let what = match &file {
                Some(f) => format!("\u{201c}{f}\u{201d} is not an equalizer setting"),
                None => "Nothing to paste: no equalizer settings on the clipboard".to_owned(),
            };
            let msg = if r.reason.contains("no equalizer settings") { what } else { format!("{what}: {}", r.reason) };
            views::flash_long(st, &msg);
        }
        Err(r) => show_refusal(st, &r, file.as_deref()),
        Ok(im) => show_preview(st, im, file.as_deref()),
    }
}

fn preview_title(file: Option<&str>) -> String {
    match file {
        Some(f) => format!("Import \u{201c}{f}\u{201d}"),
        None => "Paste equalizer settings".to_owned(),
    }
}

fn open_preview(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    d.set_returns(false);
    d.set_eq_preview_open(true);
    let p = app.global::<EqPreview>();
    p.set_shown(p.get_shown().wrapping_add(1));
}

/// A recognised setting that cannot be applied: the preview with the reason, and Close.
fn show_refusal(st: &Rc<State>, r: &Refusal, file: Option<&str>) {
    let Some(app) = st.app.upgrade() else { return };
    PREVIEW.with(|p| *p.borrow_mut() = None);
    let e = app.global::<EqPreview>();
    e.set_title(preview_title(file).into());
    e.set_format(r.format.as_ref().map(|f| f.describe()).unwrap_or_default().into());
    e.set_refused(true);
    e.set_reason(r.reason.clone().into());
    e.set_summary("".into());
    e.set_verdict("Nothing can be applied".into());
    e.set_verdict_kind(2);
    e.set_rows(ModelRc::default());
    e.set_notes(ModelRc::default());
    e.set_curve_now("".into());
    e.set_curve_new("".into());
    e.set_curve_new_fill("".into());
    e.set_curve_target("".into());
    e.set_name_line("".into());
    open_preview(st);
}

/// "−4.3 dB", "auto (−6.2 dB now)", "none (auto: −2.1 dB)".
fn preamp_words(p: Option<Preamp>, bands: &[Band], rate: u32) -> String {
    match p {
        Some(Preamp::Manual(db)) => db_text(f64::from(db)),
        Some(Preamp::Auto) => format!("auto ({} now)", db_text(-peq::max_boost_db(bands, rate))),
        None => format!("none given: auto ({})", db_text(-peq::max_boost_db(bands, rate))),
    }
}

fn show_preview(st: &Rc<State>, im: Import, file: Option<&str>) {
    let Some(app) = st.app.upgrade() else { return };
    let rate = rate_shown();
    let s = st.settings.borrow();
    let c = current(&s);
    let now_bands = bands_of(c);
    let base = im.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| if file.is_some() { "Imported".to_owned() } else { "Pasted equalizer".to_owned() });
    let bands_setting: Vec<EqBandSetting> = im.bands.iter().map(from_band).collect();
    let (auto, db) = match im.preamp {
        Some(Preamp::Manual(db)) => (false, db),
        _ => (true, 0.0),
    };
    let existing = s.equalizer.presets.iter().find(|p| same(&p.name, &base) && p.bands == bands_setting && p.preamp_auto == auto && (auto || p.preamp_db == db)).map(|p| p.name.clone());
    let name = existing.clone().unwrap_or_else(|| unique_name(&base, |n| s.equalizer.presets.iter().any(|p| same(&p.name, n)) || presets::built_in(n).is_some()));
    drop(s);
    let e = app.global::<EqPreview>();
    e.set_title(preview_title(file).into());
    e.set_format(im.format.describe().into());
    e.set_refused(false);
    e.set_reason("".into());
    let kinds = im.bands.len();
    let summary = match &im.fit {
        Some(f) => format!("{} points fitted with {} bands \u{00b7} preamp {}", im.graphic.iter().map(Vec::len).sum::<usize>(), f.bands.len(), preamp_words(im.preamp, &im.bands, rate)),
        None => format!("{kinds} {} \u{00b7} preamp {}", if kinds == 1 { "filter" } else { "filters" }, preamp_words(im.preamp, &im.bands, rate)),
    };
    e.set_summary(summary.into());
    let left = im.notes.iter().filter(|n| n.kind == NoteKind::Left).count();
    let approx = im.notes.iter().filter(|n| n.kind == NoteKind::Approximated).count();
    let (verdict, kind) = match (&im.fit, left, approx) {
        (Some(f), 0, _) if f.quality() == player_core::eq::fit::Quality::Close => (format!("A close match: within {:.2} dB of the curve", f.max_err_db), 1),
        (Some(f), _, _) if f.quality() != player_core::eq::fit::Quality::Close => (format!("A rough match: up to {:.1} dB from the curve", f.max_err_db), 2),
        (_, 0, 0) => ("Exact: plays as written".to_owned(), 0),
        (_, 0, n) => (format!("{n} {} played approximately (below)", if n == 1 { "thing" } else { "things" }), 1),
        (_, n, _) => (format!("{n} {} left out (below)", if n == 1 { "thing is" } else { "things are" }), 2),
    };
    e.set_verdict(verdict.into());
    e.set_verdict_kind(kind);
    e.set_rows(ModelRc::new(VecModel::from(band_rows(&im.bands, rate))));
    let notes: Vec<crate::EqNote> = im
        .notes
        .iter()
        .map(|n| crate::EqNote {
            kind: match n.kind {
                NoteKind::Left => 2,
                NoteKind::Approximated => 1,
                NoteKind::Info => 0,
            },
            text: n.text.clone().into(),
        })
        .collect();
    e.set_notes(ModelRc::new(VecModel::from(notes)));
    // The shapes: what plays now, what would, and a GraphicEQ's own curve (each without its
    // preamp, which the summary gives).
    let (now, _) = curve_paths(&now_bands, rate, 0.0);
    e.set_curve_now(if c_enabled(st) { now.into() } else { curve_paths(&[], rate, 0.0).0.into() });
    let (new, new_fill) = curve_paths(&im.bands, rate, 0.0);
    e.set_curve_new(new.into());
    e.set_curve_new_fill(new_fill.into());
    let target = match (&im.fit, im.graphic.is_empty()) {
        (Some(f), false) => {
            let freqs = curve_freqs();
            // The curve less what the bands share with the preamp, so it lies on the bands'.
            let exact: Vec<Band> = im.bands[..im.bands.len() - f.bands.len()].to_vec();
            let db: Vec<f64> = freqs.iter().map(|&x| player_core::eq::fit::curves_at(&im.graphic, x) - f.offset_db + peq::response_db(&exact, rate, x)).collect();
            path_of(&db)
        }
        _ => String::new(),
    };
    e.set_curve_target(target.into());
    e.set_name_line(
        if existing.is_some() { format!("Plays your preset \u{201c}{name}\u{201d}, which is this setting already") } else { format!("Adds the preset \u{201c}{name}\u{201d} and plays it") }.into(),
    );
    e.set_now_label(if c_enabled(st) { format!("Now: {}", summary_now(st)).into() } else { "Now: off".into() });
    PREVIEW.with(|p| *p.borrow_mut() = Some(Pending { import: im, name, existing: existing.is_some() }));
    open_preview(st);
}

fn c_enabled(st: &Rc<State>) -> bool {
    current(&st.settings.borrow()).enabled
}

fn summary_now(st: &Rc<State>) -> String {
    summary(&st.settings.borrow())
}

fn close_preview(st: &Rc<State>) {
    PREVIEW.with(|p| *p.borrow_mut() = None);
    if let Some(app) = st.app.upgrade() {
        app.global::<Dialog>().set_eq_preview_open(false);
    }
}

/// Apply: the setting becomes a preset of its name (or the identical one already there)
/// and plays.
pub fn preview_accept(st: &Rc<State>) {
    let Some(p) = PREVIEW.with(|p| p.borrow_mut().take()) else {
        close_preview(st);
        return;
    };
    close_preview(st);
    if !p.existing {
        let (auto, db) = match p.import.preamp {
            Some(Preamp::Manual(db)) => (false, db),
            _ => (true, 0.0),
        };
        st.settings.borrow_mut().equalizer.presets.push(UserPreset { name: p.name.clone(), bands: p.import.bands.iter().map(from_band).collect(), preamp_auto: auto, preamp_db: db });
    }
    choose(st, &p.name);
    let n = p.import.bands.len();
    views::flash(st, &format!("Applied \u{201c}{}\u{201d}: {n} {}", p.name, if n == 1 { "band" } else { "bands" }));
}

/// Cancel (or Esc): nothing changes.
pub fn preview_cancel(st: &Rc<State>) {
    READ_SEQ.with(|s| s.set(s.get() + 1));
    close_preview(st);
}

pub fn import_dialog(st: &Rc<State>) {
    if !crate::dialog_allowed("import equalizer preset") {
        return;
    }
    let picked = rfd::FileDialog::new()
        .set_title("Import an equalizer setting (Equalizer APO, AutoEq, Room EQ Wizard, EasyEffects)")
        .add_filter("Equalizer settings", &["txt", "json", "cfg"])
        .add_filter("All files", &["*"])
        .pick_file();
    if let Some(path) = picked
        && let Err(e) = import_file(st, &path)
    {
        views::flash_long(st, &e);
    }
}

// ---- Export -----------------------------------------------------------------------------

/// What Export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportAs {
    /// Equalizer APO text (as Copy).
    Apo,
    /// This app's own preset file: JSON, every number as it is, version 2.
    Marquee,
}

/// What plays, as a file of that kind: (its name, its text).
pub fn export_text(s: &Settings, as_: ExportAs) -> (String, String) {
    let c = current(s);
    let name = c.preset.clone().unwrap_or_else(|| "Custom".into());
    let bands = bands_of(c);
    let text = match as_ {
        ExportAs::Apo => {
            let auto_db = -peq::max_boost_db(&bands, rate_shown());
            apo::write(&name, &bands, preamp_of(c.preamp_auto, c.preamp_db), auto_db, true)
        }
        ExportAs::Marquee => formats::marquee_json(&name, &bands, preamp_of(c.preamp_auto, c.preamp_db)),
    };
    (name, text)
}

pub fn export_file(st: &Rc<State>, path: &Path, as_: ExportAs) -> bool {
    let (_, text) = export_text(&st.settings.borrow(), as_);
    match std::fs::write(path, text) {
        Ok(()) => {
            views::flash(st, &format!("Exported to {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            true
        }
        Err(e) => {
            views::flash_long(st, &format!("Could not write {}: {e}", path.display()));
            false
        }
    }
}

pub fn export_dialog(st: &Rc<State>, as_: ExportAs) {
    if !crate::dialog_allowed("export equalizer preset") {
        return;
    }
    let (name, _) = export_text(&st.settings.borrow(), as_);
    let file: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let dialog = rfd::FileDialog::new().set_title("Export the equalizer setting");
    let dialog = match as_ {
        ExportAs::Apo => dialog.set_file_name(format!("{file}.txt")).add_filter("Equalizer APO text", &["txt"]),
        ExportAs::Marquee => dialog.set_file_name(format!("{file}.json")).add_filter("Marquee Player preset", &["json"]),
    };
    if let Some(path) = dialog.save_file() {
        export_file(st, &path, as_);
    }
}

/// The player bar's button: Settings > Equalizer.
pub fn open_panel(st: &Rc<State>) {
    views::settings_section(st, crate::settings::nav::EQUALIZER);
}

/// The graphic sliders' centres, for the bench.
#[allow(dead_code)]
pub fn slider_freqs() -> [f32; 10] {
    GRAPHIC_FREQS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(freq_text(31.5), "31.5 Hz");
        assert_eq!(freq_text(63.0), "63 Hz");
        assert_eq!(freq_text(105.3), "105 Hz");
        assert_eq!(freq_text(1000.0), "1 kHz");
        assert_eq!(freq_text(1250.0), "1.2 kHz");
        assert_eq!(freq_text(16_000.0), "16 kHz");
        assert_eq!(db_text(4.5), "+4.5 dB");
        assert_eq!(db_text(-6.04), "\u{2212}6.0 dB");
        assert_eq!(db_text(0.04), "0 dB");
        assert_eq!(width_text(&Band::with_width(Kind::LowShelf, 100.0, 3.0, 0.9, Width::Slope)), ("Slope", "10.8 dB/oct".into()));
        assert_eq!(width_text(&Band::with_width(Kind::Peak, 100.0, 3.0, 1.5, Width::Octaves)), ("Width", "1.5 oct".into()));
        assert_eq!(and_list(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn the_sliders_stand_under_their_octaves_of_the_curve() {
        for (i, &f) in GRAPHIC_FREQS.iter().enumerate() {
            let x = x_of(f64::from(f)) / VIEW_W;
            // The ISO centres are nominal (125 Hz, not 126): within 2 px of 660.
            assert!((x - (i as f64 + 0.5) / 10.0).abs() < 3e-3, "{f}: {x}");
        }
        assert_eq!(y_of(0.0), VIEW_H / 2.0);
        assert_eq!(y_of(CURVE_DB), 0.0);
    }

    #[test]
    fn preset_names_are_checked() {
        let user = vec![UserPreset { name: "Headphones".into(), ..UserPreset::default() }];
        assert_eq!(check_name("  ", &user), NameCheck::Empty);
        assert_eq!(check_name("bass  BOOST", &user), NameCheck::BuiltIn("Bass boost"));
        assert_eq!(check_name("headphones", &user), NameCheck::Taken("Headphones".into()));
        assert_eq!(check_name(" Car  stereo ", &user), NameCheck::Free("Car stereo".into()));
        assert_eq!(unique_name("HD 650", |n| n == "HD 650" || n == "HD 650 (2)"), "HD 650 (3)");
    }

    #[test]
    fn a_device_s_own_setting_plays_on_it() {
        let mut s = Settings { output_device: Some("{hp}".into()), ..Settings::default() };
        s.equalizer.devices.insert("{hp}".into(), DeviceEq { name: "Headphones".into(), curve: EqCurve { enabled: true, preset: Some("Bass boost".into()), ..EqCurve::default() } });
        assert_eq!(current(&s).preset.as_deref(), Some("Bass boost"));
        s.output_device = Some("{speakers}".into());
        assert_eq!(current(&s).preset.as_deref(), Some("Flat"), "another device: the shared setting");
    }

    #[test]
    fn the_menu_lists_presets_then_actions() {
        let mut s = Settings::default();
        s.equalizer.presets.push(UserPreset { name: "Mine".into(), ..UserPreset::default() });
        s.equalizer.all.preset = Some("Mine".into());
        let labels: Vec<String> = menu_entries(&s)
            .iter()
            .map(|e| match e {
                crate::menus::Entry::Item { label, .. } | crate::menus::Entry::Playlists { label } | crate::menus::Entry::Choices { label, .. } | crate::menus::Entry::Heading { label } | crate::menus::Entry::Disabled { label } => label.clone(),
                crate::menus::Entry::Checked { label, .. } => format!("\u{2713}{label}"),
                crate::menus::Entry::Separator => "-".into(),
            })
            .collect();
        assert_eq!(labels[0], "Flat");
        assert!(labels.contains(&"\u{2713}Mine".to_owned()));
        assert!(labels.iter().any(|l| l.starts_with("Rename")) && labels.iter().any(|l| l.starts_with("Delete")));
        assert_eq!(labels.last().unwrap(), "Export as a Marquee Player preset\u{2026}");
        assert!(labels.iter().any(|l| l == "Export as Equalizer APO text\u{2026}"));
    }

    /// Copy writes what plays: the ten sliders as the peaks they are, the auto preamp as
    /// its value now, and it reads back as the same setting.
    #[test]
    fn copy_writes_what_plays_and_reads_back() {
        let mut s = Settings::default();
        s.equalizer.all.enabled = true;
        s.equalizer.all.bands = presets::BUILT_IN[1].bands().iter().map(from_band).collect();
        s.equalizer.all.preset = Some("Bass boost".into());
        let (text, msg) = copy_text(&s, CopyAs::Apo);
        assert_eq!(msg, "Equalizer copied");
        assert!(text.starts_with("# Marquee Player equalizer preset: Bass boost\n# Marquee Player: preamp auto\nPreamp: -"), "{text}");
        assert!(text.contains("Filter 1: ON PK Fc 31.5 Hz Gain 5.4 dB Q 1.41\n"), "{text}");
        assert_eq!(text.lines().filter(|l| l.starts_with("Filter")).count(), 10);
        let back = formats::read(&text, None).unwrap();
        assert_eq!(back.bands, bands_of(&s.equalizer.all));
        assert_eq!(back.preamp, Some(Preamp::Auto));
        // Off: the filters written OFF, the preamp 0 dB.
        s.equalizer.all.enabled = false;
        let (text, msg) = copy_text(&s, CopyAs::Apo);
        assert!(msg.contains("off") && text.contains("Preamp: 0 dB") && text.contains(": OFF PK"), "{text}");
        // Copy as the other formats: each reads back.
        s.equalizer.all.enabled = true;
        let (ee, _) = copy_text(&s, CopyAs::EasyEffects);
        assert_eq!(formats::read(&ee, None).unwrap().bands.len(), 10);
        let (g, _) = copy_text(&s, CopyAs::GraphicEq);
        assert!(g.starts_with("GraphicEQ: 20 "));
    }

    #[test]
    fn a_band_keeps_its_kind_and_width_through_the_settings() {
        for b in [
            Band::new(Kind::Notch, 50.0, 0.0, 30.0),
            Band::with_width(Kind::LowShelf, 120.0, 4.0, 0.9, Width::Slope),
            Band::with_width(Kind::BandPass, 1000.0, 0.0, 2.0, Width::Octaves),
            Band::new(Kind::AllPass, 300.0, 0.0, 0.7),
        ] {
            assert_eq!(to_band(&from_band(&b)), b);
        }
    }
}
