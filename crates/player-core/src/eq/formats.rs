//! Equalizer settings from other tools, and this app's own, read from text the user gives
//! (the clipboard or a file): which format it is, what it holds, and what of it was left
//! out or played approximately, with why. Also the formats a setting is copied or exported
//! as. Nothing here reaches the network.
//!
//! **Detection.** The content decides (a file's name only names what was read: AutoEq's
//! `ParametricEQ.txt` and `FixedBandEQ.txt` are Equalizer APO text): JSON is this app's
//! preset (`marquee_player_equalizer`) or an EasyEffects / PulseEffects preset
//! (`output.equalizer…`); text is Equalizer APO's configuration (`apo.rs`, which also
//! reads Room EQ Wizard's "Filter Settings" export, GraphicEQ lines, Peace's export).
//! miniDSP biquad lists and CamillaDSP configurations are recognised and refused with the
//! reason. Anything else "has no equalizer settings in it".
//!
//! **Bounds.** At most [`MAX_BYTES`] of text (refused beyond, before any parsing); text
//! with NUL or many control characters is not text; at most `MAX_BANDS` filters (refused
//! with the count, never cut short); at most [`MAX_NOTES`] notes kept, the rest counted;
//! numbers held within the equalizer's ranges (said); a GraphicEQ line of at most
//! `fit::MAX_POINTS` points, fitted on a fixed grid whatever its length.
//!
//! **EasyEffects** (and PulseEffects, its earlier name): the presets its equalizer saves
//! (`equalizer_preset.cpp`, read 2026-11: EasyEffects 7/8 `output["equalizer#0"]`,
//! EasyEffects 6 `output.equalizer`, PulseEffects 4 `output.equalizer` with every value a
//! string). The bands are Linux Studio Plugins' parametric equalizer
//! (`lsp-plugins-para-equalizer`, `lsp-dsp-units` `Filter.cpp`, read 2026-11):
//! - "APO (DR)" mode is the cookbook's filters as Equalizer APO's: exact. Bell and
//!   Resonance are peaks, Lo-/Hi-shelf shelves (frequency the midpoint), Lo-/Hi-pass,
//!   Notch, Allpass, Bandpass as here (a gain on these does nothing there either), Q at
//!   least 0.1; Ladder-pass and Ladder-rej are two shelves at the band's edges
//!   (`centre / 2^(width/2)`, `centre * 2^(width/2)`). The slope does nothing there.
//! - "RLC (BT)" is the cookbook's analog prototypes through the same prewarped bilinear
//!   transform, with LSP's own Q: exact as cookbook bands. Bell at slope xN: N peaks of
//!   `gain / N` each, Q `sqrt(g) / (k sin(atan g))` with `g = 10^(gain / 20N)` and
//!   `k = 2 (1/g + g) / (1 + 2Q / N)`; shelves: N shelves of `gain / N`, Q `(1 + Q) / 2`;
//!   Lo-/Hi-pass: N second-order sections of Q `(1 + Q) / 2`; Notch: Q `(1 + Q) / 2`;
//!   Allpass: N + 1 sections of Q `(1 + Q) / 2`.
//! - "RLC (MT)" is the same prototype through the matched-Z transform: played as its
//!   bilinear (BT) version and said (they part only toward the top of the band).
//! - BWC and LRX modes (Butterworth-Chebyshev, Linkwitz-Riley designs), RLC Resonance,
//!   Bandpass and Ladder filters have no exact counterpart: those bands are left out, said.
//! - Off bands play nothing; muted bands neither; with any band on solo, only those play.
//!   Input and output gain are the preamp. FIR/FFT modes are the same curve linear-phase:
//!   played minimum-phase here, said. A balance is left out, said. Pitch shifts every
//!   band's frequency by `2^(semitones / 12)`, as there. Left and right channels set
//!   differently (split channels, or a pitch per side) are the per-channel case: refused.
//!   Other effects in the preset are not an equalizer: said, left out.

use serde_json::{Map, Value, json};

use super::fit::{self, Fit, Quality};
use super::{Band, Kind, MAX_BANDS, PREAMP_MAX, PREAMP_MIN, Preamp, Width, apo, response_db};

/// The most text read (a preset is a few kilobytes; AutoEq's GraphicEQ.txt 1.5).
pub const MAX_BYTES: usize = 1 << 20;
/// The most notes listed; the rest are counted.
pub const MAX_NOTES: usize = 60;
/// This app's preset format's current version (`marquee_player_equalizer`). Version 1 was
/// Equalizer APO text with a `# Marquee Player equalizer preset:` comment, still read.
pub const MARQUEE_VERSION: u64 = 2;

/// What text was read as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
    /// Equalizer APO configuration text (also Peace's export).
    Apo,
    /// AutoEq's `ParametricEQ.txt` (Equalizer APO text; known by the file's name).
    AutoEqParametric,
    /// AutoEq's `FixedBandEQ.txt`.
    AutoEqFixedBand,
    /// A `GraphicEQ:` curve (AutoEq's `GraphicEQ.txt`, Wavelet, Equalizer APO).
    GraphicEq,
    /// Room EQ Wizard's "Filter Settings" text, with its equaliser's name.
    Rew(String),
    EasyEffects,
    PulseEffects,
    /// This app's preset: 2 the JSON file, 1 an older build's Equalizer APO text.
    Marquee(u64),
    /// Recognised and refused (miniDSP coefficients...).
    Other(String),
}

impl Format {
    pub fn describe(&self) -> String {
        match self {
            Format::Apo => "Equalizer APO text".into(),
            Format::AutoEqParametric => "AutoEq parametric EQ".into(),
            Format::AutoEqFixedBand => "AutoEq fixed band EQ".into(),
            Format::GraphicEq => "GraphicEQ curve".into(),
            Format::Rew(eq) if eq.is_empty() => "Room EQ Wizard filters".into(),
            Format::Rew(eq) => format!("Room EQ Wizard filters ({eq})"),
            Format::EasyEffects => "EasyEffects preset".into(),
            Format::PulseEffects => "PulseEffects preset".into(),
            Format::Marquee(_) => "Marquee Player preset".into(),
            Format::Other(what) => what.clone(),
        }
    }
}

/// How a note bears on what plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    /// Something that would sound was left out.
    Left,
    /// Played, but not exactly as the source would.
    Approximated,
    /// Read in a particular way, or nothing lost (an OFF filter, a corner frequency).
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
}

impl Note {
    pub fn new(kind: NoteKind, text: String) -> Note {
        Note { kind, text }
    }
}

/// What was read, ready to apply.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub format: Format,
    pub bands: Vec<Band>,
    /// `None`: the text has no preamp (the equalizer's auto preamp then applies).
    pub preamp: Option<Preamp>,
    pub name: Option<String>,
    pub notes: Vec<Note>,
    /// A GraphicEQ curve's fit (its bands are in `bands`, its offset in the preamp).
    pub fit: Option<Fit>,
    /// The GraphicEQ curves read (several play one after another: their gains add).
    pub graphic: Vec<Vec<(f64, f64)>>,
}

impl Import {
    /// Something that would sound was left out, or played other than the source plays it.
    pub fn lossy(&self) -> bool {
        self.notes.iter().any(|n| n.kind != NoteKind::Info)
    }

    /// The GraphicEQ target at `f` Hz, with the text's preamp (what the source plays).
    pub fn target_db(&self, f: f64) -> Option<f64> {
        (!self.graphic.is_empty()).then(|| fit::curves_at(&self.graphic, f))
    }
}

/// Why nothing is applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    /// What it was recognised as; `None`: not an equalizer setting at all.
    pub format: Option<Format>,
    pub reason: String,
}

fn refusal(format: Option<Format>, reason: impl Into<String>) -> Refusal {
    Refusal { format, reason: reason.into() }
}

/// Read a file's bytes: UTF-8 (with or without a byte order mark), UTF-16 with one (what
/// Notepad writes as "Unicode"), else each byte as Latin-1 (Equalizer APO falls back to the
/// system's code page; numbers and keywords survive either way).
pub fn read_bytes(bytes: &[u8], file_name: Option<&str>) -> Result<Import, Refusal> {
    if bytes.len() > MAX_BYTES * 2 {
        return Err(too_large(bytes.len()));
    }
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>())
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect::<Vec<_>>())
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
        }
    };
    read(&text, file_name)
}

fn too_large(n: usize) -> Refusal {
    refusal(None, format!("This is too large to be an equalizer setting ({:.1} MB; settings are a few kilobytes).", n as f64 / 1_048_576.0))
}

/// Read text (pasted, or a file's), `file_name` a hint only.
pub fn read(text: &str, file_name: Option<&str>) -> Result<Import, Refusal> {
    if text.len() > MAX_BYTES {
        return Err(too_large(text.len()));
    }
    let text = text.trim_start_matches('\u{feff}');
    let controls = text.chars().filter(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n' | '\u{c}')).count();
    if text.contains('\0') || controls * 100 > text.chars().count().max(1) {
        return Err(refusal(None, "This is not text, so it holds no equalizer settings."));
    }
    if text.trim().is_empty() {
        return Err(refusal(None, "There is no text."));
    }
    let import = if text.trim_start().starts_with('{') {
        let v: Value = serde_json::from_str(text).map_err(|e| refusal(None, format!("This looks like JSON but cannot be read as such ({e}).")))?;
        if v.get("marquee_player_equalizer").is_some() {
            read_marquee(&v)?
        } else if ["output", "input"].iter().any(|s| v.get(s).and_then(Value::as_object).is_some_and(|o| o.keys().any(|k| is_eq_key(k)))) {
            read_easyeffects(&v)?
        } else {
            return Err(refusal(None, "This JSON is not an equalizer setting this app knows (EasyEffects or PulseEffects presets, or this app’s own)."));
        }
    } else {
        refuse_known_others(text)?;
        apo::read(text)?
    };
    finish(import, file_name)
}

/// Formats recognised but not read, refused with what to use instead.
fn refuse_known_others(text: &str) -> Result<(), Refusal> {
    let lower = text.to_ascii_lowercase();
    if lower.lines().any(|l| l.trim_start().starts_with("biquad")) && lower.contains("b0=") {
        return Err(refusal(
            Some(Format::Other("miniDSP biquad coefficients".into())),
            "These are biquad coefficients for one sample rate (miniDSP’s format): played at another rate every filter would move. Export the same filters from Room EQ Wizard with its Generic equaliser instead.",
        ));
    }
    if lower.contains("filters:") && lower.contains("type: biquad") {
        return Err(refusal(
            Some(Format::Other("CamillaDSP configuration".into())),
            "This is a CamillaDSP configuration (YAML), which this app does not read: its filters are part of a whole processing pipeline. Equalizer APO text of the same filters can be pasted instead.",
        ));
    }
    Ok(())
}

/// The common last steps: the GraphicEQ fit, the band limit, ranges, the name, the format
/// named by the file, the notes' bound.
fn finish(mut im: Import, file_name: Option<&str>) -> Result<Import, Refusal> {
    let stem = file_name.map(|n| std::path::Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    if im.format == Format::Apo
        && let Some(stem) = &stem
    {
        let l = stem.to_ascii_lowercase();
        if l.ends_with("parametriceq") {
            im.format = Format::AutoEqParametric;
        } else if l.ends_with("fixedbandeq") {
            im.format = Format::AutoEqFixedBand;
        }
    }
    let exact = im.bands.len();
    if exact > MAX_BANDS {
        return Err(refusal(Some(im.format), format!("{exact} filters: the equalizer takes up to {MAX_BANDS}, and none is left out silently. Remove the ones you need least and paste again.")));
    }
    if !im.graphic.is_empty() {
        let budget = MAX_BANDS - exact;
        if budget == 0 {
            return Err(refusal(Some(im.format), format!("{exact} filters leave no band for the GraphicEQ curve (the equalizer takes up to {MAX_BANDS}).")));
        }
        let curves = im.graphic.clone();
        let target = move |f: f64| fit::curves_at(&curves, f);
        let f = fit::fit(&target, budget);
        let points: usize = im.graphic.iter().map(Vec::len).sum();
        let where_ = freq_words(f.worst_hz);
        match f.quality() {
            Quality::Poor => {
                return Err(refusal(
                    Some(im.format),
                    format!(
                        "This GraphicEQ curve cannot be followed closely enough with {} bands: up to {:.1} dB off near {where_} (the limit is {:.0} dB). AutoEq’s ParametricEQ.txt for the same headphones plays exactly.",
                        f.bands.len(),
                        f.max_err_db,
                        fit::POOR_DB
                    ),
                ));
            }
            q => {
                let kind = if q == Quality::Close { NoteKind::Approximated } else { NoteKind::Left };
                let rough = if q == Quality::Rough { " — a rough match: listen before you keep it" } else { "" };
                im.notes.insert(
                    0,
                    Note::new(
                        kind,
                        format!(
                            "The GraphicEQ curve ({points} points) is played as {} bands, within {:.2} dB of it (RMS {:.2} dB) from 20 Hz to 20 kHz, the largest difference near {where_}{rough}",
                            f.bands.len(),
                            f.max_err_db,
                            f.rms_err_db
                        ),
                    ),
                );
            }
        }
        im.bands.extend(f.bands.iter().copied());
        let pre = match im.preamp {
            Some(Preamp::Manual(db)) => f64::from(db),
            _ => 0.0,
        };
        im.preamp = Some(Preamp::Manual((pre + f.offset_db) as f32));
        im.fit = Some(f);
    }
    if let Some(Preamp::Manual(db)) = im.preamp
        && !(PREAMP_MIN..=PREAMP_MAX).contains(&db)
    {
        let held = if db.is_finite() { db.clamp(PREAMP_MIN, PREAMP_MAX) } else { 0.0 };
        im.notes.push(Note::new(NoteKind::Approximated, format!("The preamp of {} dB is held at {} dB (the equalizer’s range is {PREAMP_MIN} to +{PREAMP_MAX} dB)", apo::num(f64::from(db), 1), apo::num(f64::from(held), 1))));
        im.preamp = Some(Preamp::Manual(held));
    }
    if im.name.as_deref().is_none_or(|n| n.trim().is_empty())
        && let Some(stem) = stem
    {
        let n = apo::name_from_file(&stem);
        if !n.is_empty() {
            im.name = Some(n);
        }
    }
    if im.notes.len() > MAX_NOTES {
        let more = im.notes.len() - (MAX_NOTES - 1);
        let worst = if im.notes[MAX_NOTES - 1..].iter().any(|n| n.kind == NoteKind::Left) { NoteKind::Left } else { NoteKind::Info };
        im.notes.truncate(MAX_NOTES - 1);
        im.notes.push(Note::new(worst, format!("…and {more} more lines like these")));
    }
    Ok(im)
}

/// "5.3 kHz", "120 Hz".
fn freq_words(f: f64) -> String {
    if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{f:.0} Hz") }
}

// ---- EasyEffects / PulseEffects ----

fn is_eq_key(k: &str) -> bool {
    k == "equalizer" || k.starts_with("equalizer#")
}

/// A number, or a number written as a string (PulseEffects).
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

fn boolean(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn text(v: Option<&Value>) -> Option<&str> {
    v?.as_str()
}

/// One LSP band as cookbook bands: Ok(bands, an approximation note), or why it is left out.
#[allow(clippy::too_many_arguments)]
fn lsp_band(ty: &str, mode: &str, slope: &str, gain: f64, freq: f64, q: f64, width: f64) -> Result<(Vec<Band>, Option<String>), String> {
    let n = match slope {
        "x1" => 1,
        "x2" => 2,
        "x3" => 3,
        "x4" => 4,
        other => return Err(format!("its slope “{other}” is not one EasyEffects has")),
    };
    let f = freq as f32;
    let g = gain as f32;
    if mode == "APO (DR)" {
        let q = q.max(0.1) as f32;
        let one = |k: Kind| Ok((vec![Band::new(k, f, g, q)], None));
        return match ty {
            "Bell" | "Resonance" => one(Kind::Peak),
            "Lo-shelf" => one(Kind::LowShelf),
            "Hi-shelf" => one(Kind::HighShelf),
            "Lo-pass" => one(Kind::LowPass),
            "Hi-pass" => one(Kind::HighPass),
            "Notch" => one(Kind::Notch),
            "Allpass" => one(Kind::AllPass),
            "Bandpass" => one(Kind::BandPass),
            "Ladder-pass" | "Ladder-rej" => {
                let k = 2f64.powf(width / 2.0);
                let (f1, f2) = ((freq / k) as f32, (freq * k) as f32);
                if ty == "Ladder-pass" {
                    Ok((vec![Band::new(Kind::HighShelf, f1, g, q), Band::new(Kind::HighShelf, f2, -g, q)], None))
                } else {
                    Ok((vec![Band::new(Kind::LowShelf, f1, g, q), Band::new(Kind::HighShelf, f2, g, q)], None))
                }
            }
            other => Err(format!("the filter type “{other}” is not one EasyEffects has")),
        };
    }
    let (rlc, matched) = match mode {
        "RLC (BT)" => (true, false),
        "RLC (MT)" => (true, true),
        _ => (false, false),
    };
    if !rlc {
        return Err(format!("LSP’s {mode} filters are a design of their own, with no exact counterpart here"));
    }
    let rq = ((1.0 + q) / 2.0) as f32;
    let bands = match ty {
        "Bell" => {
            let fg = 10f64.powf(gain / 20.0 / n as f64);
            let k = 2.0 * (1.0 / fg + fg) / (1.0 + 2.0 * q / n as f64);
            let qb = fg.sqrt() / (k * fg.atan().sin());
            vec![Band::new(Kind::Peak, f, g / n as f32, qb as f32); n]
        }
        "Lo-shelf" => vec![Band::new(Kind::LowShelf, f, g / n as f32, rq); n],
        "Hi-shelf" => vec![Band::new(Kind::HighShelf, f, g / n as f32, rq); n],
        "Lo-pass" => vec![Band::new(Kind::LowPass, f, 0.0, rq); n],
        "Hi-pass" => vec![Band::new(Kind::HighPass, f, 0.0, rq); n],
        "Notch" => vec![Band::new(Kind::Notch, f, 0.0, rq)],
        "Allpass" => vec![Band::new(Kind::AllPass, f, 0.0, rq); n + 1],
        "Resonance" | "Bandpass" | "Ladder-pass" | "Ladder-rej" => return Err(format!("LSP’s RLC {ty} filter has no exact counterpart here")),
        other => return Err(format!("the filter type “{other}” is not one EasyEffects has")),
    };
    let note = matched.then(|| "LSP’s matched-Z (MT) version is played as its bilinear (BT) one, the same curve but toward the top of the band".to_owned());
    Ok((bands, note))
}

/// One channel's bands: what plays, and the notes.
fn ee_channel(ch: &Value, n: usize, pitch: f64, side: &str, notes: &mut Vec<Note>) -> Result<Vec<Band>, String> {
    let ch = ch.as_object().ok_or_else(|| format!("its {side} channel is missing"))?;
    let mut read: Vec<(usize, bool, Vec<Band>)> = Vec::new();
    for i in 0..n {
        let key = format!("band{i}");
        let b = ch.get(&key).ok_or_else(|| format!("band {} of {n} is missing from its {side} channel", i + 1))?;
        let ty = text(b.get("type")).unwrap_or("Bell");
        if ty == "Off" || boolean(b.get("mute")) == Some(true) {
            if ty != "Off" && side == "left" {
                notes.push(Note::new(NoteKind::Info, format!("Band {}: muted, so it plays nothing (in EasyEffects either)", i + 1)));
            }
            continue;
        }
        let solo = boolean(b.get("solo")).unwrap_or(false);
        let freq = number(b.get("frequency")).ok_or_else(|| format!("band {} has no frequency", i + 1))? * 2f64.powf(pitch / 12.0);
        let gain = number(b.get("gain")).unwrap_or(0.0);
        let q = number(b.get("q")).or_else(|| number(b.get("quality"))).unwrap_or(4.36);
        let width = number(b.get("width")).unwrap_or(4.0);
        let mode = text(b.get("mode")).unwrap_or("RLC (BT)");
        let slope = text(b.get("slope")).unwrap_or("x1");
        match lsp_band(ty, mode, slope, gain, freq, q, width) {
            Ok((bands, note)) => {
                if let Some(note) = note
                    && side == "left"
                {
                    notes.push(Note::new(NoteKind::Approximated, format!("Band {}: {note}", i + 1)));
                }
                read.push((i, solo, bands));
            }
            Err(why) => {
                if side == "left" {
                    notes.push(Note::new(NoteKind::Left, format!("Band {} ({ty}, {mode}): {why}: left out", i + 1)));
                }
            }
        }
    }
    let any_solo = read.iter().any(|r| r.1);
    if any_solo && side == "left" {
        notes.push(Note::new(NoteKind::Info, "Some bands are on solo: only those play (as in EasyEffects)".into()));
    }
    Ok(read.into_iter().filter(|r| !any_solo || r.1).flat_map(|r| r.2).collect())
}

fn read_easyeffects(v: &Value) -> Result<Import, Refusal> {
    let (section, sec) = ["output", "input"]
        .iter()
        .find_map(|s| v.get(*s).and_then(Value::as_object).filter(|o| o.keys().any(|k| is_eq_key(k))).map(|o| (o, *s)))
        .ok_or_else(|| refusal(None, "No equalizer in this preset."))?;
    let pulse = section.iter().any(|(k, e)| is_eq_key(k) && e.get("state").is_some());
    let format = if pulse { Format::PulseEffects } else { Format::EasyEffects };
    let refuse = |why: String| refusal(Some(format.clone()), why);
    let mut notes = Vec::new();
    if sec == "input" {
        notes.push(Note::new(NoteKind::Info, "This is a preset for the input (microphone): its equalizer is read as it is".into()));
    }
    let mut order: Vec<String> = section.get("plugins_order").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).filter(|k| is_eq_key(k)).map(str::to_owned).collect()).unwrap_or_default();
    let mut rest: Vec<String> = section.keys().filter(|k| is_eq_key(k) && !order.contains(k)).cloned().collect();
    rest.sort();
    order.extend(rest);
    let others: Vec<String> = {
        let mut o: Vec<String> = section.keys().filter(|k| !is_eq_key(k) && *k != "blocklist" && *k != "plugins_order").map(|k| k.split('#').next().unwrap_or(k).replace('_', " ")).collect();
        o.dedup();
        o
    };
    if !others.is_empty() {
        notes.push(Note::new(NoteKind::Left, format!("Its other effects ({}) are not an equalizer: left out", others.join(", "))));
    }
    let mut bands = Vec::new();
    let mut preamp = 0.0f64;
    let mut playing = 0;
    for key in &order {
        let eq = &section[key];
        if boolean(eq.get("bypass")) == Some(true) || boolean(eq.get("state")) == Some(false) {
            notes.push(Note::new(NoteKind::Info, format!("Its equalizer “{key}” is switched off (bypassed), so it plays nothing")));
            continue;
        }
        playing += 1;
        preamp += number(eq.get("input-gain")).unwrap_or(0.0) + number(eq.get("output-gain")).unwrap_or(0.0);
        if let Some(mode) = text(eq.get("mode"))
            && mode != "IIR"
        {
            notes.push(Note::new(NoteKind::Approximated, format!("EasyEffects’ {mode} mode plays these filters’ curve with linear phase; here they are minimum-phase filters, the same curve without the delay")));
        }
        let n = number(eq.get("num-bands")).unwrap_or(0.0);
        if !(1.0..=32.0).contains(&n) || n.fract() != 0.0 {
            return Err(refuse(format!("“num-bands” is {n}, not a band count EasyEffects has (1 to 32).")));
        }
        let n = n as usize;
        let (pl, pr) = (number(eq.get("pitch-left")).unwrap_or(0.0), number(eq.get("pitch-right")).unwrap_or(0.0));
        if pl != pr {
            return Err(refuse("Its left and right channels are pitched differently: this equalizer applies one setting to both channels.".to_owned()));
        }
        if pl != 0.0 {
            notes.push(Note::new(NoteKind::Info, format!("Every band is shifted by {pl:+} semitones, as EasyEffects’ pitch does")));
        }
        let balance = number(eq.get("balance")).unwrap_or(0.0);
        if balance != 0.0 {
            notes.push(Note::new(NoteKind::Left, format!("Its balance ({balance:+}) between left and right has no equivalent here: left out")));
        }
        let mut sink = Vec::new();
        let left = ee_channel(eq.get("left").unwrap_or(&Value::Null), n, pl, "left", &mut notes).map_err(|e| refuse(format!("This preset cannot be read: {e}.")))?;
        let right = ee_channel(eq.get("right").unwrap_or(&Value::Null), n, pr, "right", &mut sink).map_err(|e| refuse(format!("This preset cannot be read: {e}.")))?;
        if left != right {
            let split = boolean(eq.get("split-channels")).unwrap_or(false);
            return Err(refuse(format!(
                "Its left and right channels are set differently{}: this equalizer applies one setting to both channels.",
                if split { " (split channels)" } else { "" }
            )));
        }
        bands.extend(left);
    }
    if playing == 0 {
        return Err(refuse("Its equalizer is switched off (bypassed) in this preset: it plays nothing.".to_owned()));
    }
    if bands.is_empty() {
        return Err(refuse("Every band of its equalizer is off: it plays nothing.".to_owned()));
    }
    let preamp = Some(Preamp::Manual(preamp as f32));
    Ok(Import { format, bands, preamp, name: None, notes, fit: None, graphic: Vec::new() })
}

// ---- This app's own preset ----

fn kind_id(k: Kind) -> &'static str {
    match k {
        Kind::Peak => "peak",
        Kind::LowShelf => "low_shelf",
        Kind::HighShelf => "high_shelf",
        Kind::LowPass => "low_pass",
        Kind::HighPass => "high_pass",
        Kind::BandPass => "band_pass",
        Kind::Notch => "notch",
        Kind::AllPass => "all_pass",
    }
}

fn kind_from_id(s: &str) -> Option<Kind> {
    Kind::ALL.into_iter().find(|k| kind_id(*k) == s)
}

fn read_marquee(v: &Value) -> Result<Import, Refusal> {
    let version = v.get("marquee_player_equalizer").and_then(Value::as_u64).unwrap_or(0);
    let format = Format::Marquee(version);
    let refuse = |why: String| refusal(Some(format.clone()), why);
    if version < 2 {
        return Err(refuse("This preset file’s version is not one this app wrote.".into()));
    }
    let mut notes = Vec::new();
    if version > MARQUEE_VERSION {
        notes.push(Note::new(NoteKind::Info, format!("Written by a newer version of Marquee Player (format {version}); read as far as this one knows it")));
    }
    let name = v.get("name").and_then(Value::as_str).map(str::to_owned);
    let preamp = match v.get("preamp") {
        Some(Value::String(s)) if s == "auto" => Some(Preamp::Auto),
        Some(x) => number(Some(x)).map(|d| Preamp::Manual(d as f32)),
        None => None,
    };
    let list = v.get("bands").and_then(Value::as_array).ok_or_else(|| refuse("This preset has no bands.".into()))?;
    if list.len() > MAX_BANDS {
        return Err(refuse(format!("{} bands: the equalizer takes up to {MAX_BANDS}.", list.len())));
    }
    let mut bands = Vec::new();
    for (i, b) in list.iter().enumerate() {
        let ty = b.get("type").and_then(Value::as_str).unwrap_or("peak");
        let kind = kind_from_id(ty).ok_or_else(|| refuse(format!("Band {} is a kind of filter this version does not have (“{ty}”).", i + 1)))?;
        let freq = number(b.get("freq")).ok_or_else(|| refuse(format!("Band {} has no frequency.", i + 1)))?;
        let gain = number(b.get("gain")).unwrap_or(0.0);
        let (value, width) = if let Some(o) = number(b.get("octaves")) {
            (o, Width::Octaves)
        } else if let Some(s) = number(b.get("slope_db")) {
            (s / 12.0, Width::Slope)
        } else {
            (number(b.get("q")).unwrap_or(std::f64::consts::SQRT_2), Width::Q)
        };
        bands.push(Band::with_width(kind, freq as f32, gain as f32, value as f32, width));
    }
    if bands.is_empty() {
        return Err(refuse("This preset has no bands.".into()));
    }
    Ok(Import { format, bands, preamp, name, notes, fit: None, graphic: Vec::new() })
}

/// A setting as this app's own preset file (JSON, every number as it is).
pub fn marquee_json(name: &str, bands: &[Band], preamp: Preamp) -> String {
    let bands: Vec<Value> = bands
        .iter()
        .map(|b| {
            let mut m = Map::new();
            m.insert("type".into(), json!(kind_id(b.kind)));
            m.insert("freq".into(), json!(b.freq));
            if b.kind.has_gain() {
                m.insert("gain".into(), json!(b.gain_db));
            }
            match b.width {
                Width::Q => m.insert("q".into(), json!(b.q)),
                Width::Octaves => m.insert("octaves".into(), json!(b.q)),
                Width::Slope => m.insert("slope_db".into(), json!(f64::from(b.q) * 12.0)),
            };
            Value::Object(m)
        })
        .collect();
    let preamp = match preamp {
        Preamp::Auto => json!("auto"),
        Preamp::Manual(db) => json!(db),
    };
    let v = json!({ "marquee_player_equalizer": MARQUEE_VERSION, "name": name, "preamp": preamp, "bands": bands });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

// ---- Copy as… ----

/// A setting as an EasyEffects equalizer preset (its "APO (DR)" mode: exactly these
/// filters; off, bypassed). A bandwidth in octaves becomes the Q it has at 48 kHz (said in
/// the returned note), a slope the Q it has at its gain (exact).
pub fn easyeffects_json(bands: &[Band], preamp_db: f64, enabled: bool) -> (String, Option<String>) {
    let mut converted = false;
    let mut ch = Map::new();
    for (i, b) in bands.iter().enumerate() {
        let ty = match b.kind {
            Kind::Peak => "Bell",
            Kind::LowShelf => "Lo-shelf",
            Kind::HighShelf => "Hi-shelf",
            Kind::LowPass => "Lo-pass",
            Kind::HighPass => "Hi-pass",
            Kind::BandPass => "Bandpass",
            Kind::Notch => "Notch",
            Kind::AllPass => "Allpass",
        };
        converted |= b.width == Width::Octaves;
        let q = if b.width == Width::Q { f64::from(b.q) } else { b.q_equivalent(48_000) };
        ch.insert(
            format!("band{i}"),
            json!({ "frequency": b.freq, "gain": b.gain_db, "mode": "APO (DR)", "mute": false, "q": (q * 10_000.0).round() / 10_000.0, "slope": "x1", "solo": false, "type": ty, "width": 4.0 }),
        );
    }
    let eq = json!({
        "balance": 0.0, "bypass": !enabled, "decramp": "Off", "input-gain": (preamp_db * 100.0).round() / 100.0, "left": ch.clone(), "right": ch,
        "mode": "IIR", "num-bands": bands.len(), "output-gain": 0.0, "pitch-left": 0.0, "pitch-right": 0.0, "split-channels": false
    });
    let v = json!({ "output": { "blocklist": [], "equalizer#0": eq, "plugins_order": ["equalizer#0"] } });
    let note = converted.then(|| "bandwidths in octaves written as the Q they have at 48 kHz".to_owned());
    (serde_json::to_string_pretty(&v).unwrap_or_default() + "\n", note)
}

/// The frequencies AutoEq writes its GraphicEQ.txt at: 20 Hz * 1.0563^n up to 20 kHz,
/// whole numbers, 127 of them.
pub fn graphic_freqs() -> Vec<f64> {
    let n = (1000f64.ln() / 1.0563f64.ln()).ceil() as i32;
    let mut v: Vec<f64> = (0..n).map(|i| (20.0 * 1.0563f64.powi(i)).trunc()).collect();
    v.dedup();
    v
}

/// A setting as one `GraphicEQ:` line (Wavelet, Equalizer APO): the curve that plays,
/// preamp included, at AutoEq's 127 frequencies, to 0.1 dB. A curve, not the filters:
/// what is between the points is joined in straight lines over log frequency.
pub fn graphic_eq(bands: &[Band], preamp_db: f64, rate: u32) -> String {
    let pts: Vec<String> = graphic_freqs().iter().map(|&f| format!("{f} {:.1}", (response_db(bands, rate, f) + preamp_db).clamp(-100.0, 100.0))).collect();
    format!("GraphicEQ: {}\n", pts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str, file: Option<&str>) -> Import {
        read(text, file).unwrap_or_else(|e| panic!("{}", e.reason))
    }

    #[test]
    fn detection_is_by_content() {
        let apo = "Preamp: -4.3 dB\nFilter 1: ON LSC Fc 105 Hz Gain -2.7 dB Q 0.70\n";
        assert_eq!(ok(apo, None).format, Format::Apo);
        assert_eq!(ok(apo, Some("FiiO FT1 ParametricEQ.txt")).format, Format::AutoEqParametric);
        assert_eq!(ok(apo, Some("FiiO FT1 ParametricEQ.txt")).name.as_deref(), Some("FiiO FT1"));
        // A misleading extension changes nothing.
        assert_eq!(ok(apo, Some("preset.json")).format, Format::Apo);
        for not in ["", "   \n", "Hello, world", "{\"a\": 1}", "{not json", "\0\0\u{1}binary\u{2}\u{3}"] {
            let e = read(not, None).unwrap_err();
            assert!(e.format.is_none(), "{not:?}: {e:?}");
        }
        let mini = "biquad1,\nb0=1.0,\nb1=-1.9,\nb2=0.9,\na1=1.9,\na2=-0.9,\n";
        assert!(read(mini, None).unwrap_err().reason.contains("one sample rate"));
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(read(&big, None).unwrap_err().reason.contains("too large"));
        // UTF-16 as Notepad writes it.
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(apo.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(read_bytes(&utf16, None).unwrap().bands.len(), 1);
    }

    #[test]
    fn too_many_filters_are_refused_with_the_count() {
        let many: String = (0..40).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1\n", 100 + i)).collect();
        let e = read(&many, None).unwrap_err();
        assert!(e.reason.starts_with("40 filters") && e.reason.contains("up to 32"), "{}", e.reason);
        let ok32: String = (0..32).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1\n", 100 + i)).collect();
        assert_eq!(read(&ok32, None).unwrap().bands.len(), 32);
    }

    #[test]
    fn the_app_s_own_format_round_trips_every_kind() {
        let bands = vec![
            Band::new(Kind::Peak, 1234.567, -3.21, 1.234),
            Band::with_width(Kind::Peak, 500.0, 2.0, 0.333, Width::Octaves),
            Band::with_width(Kind::LowShelf, 90.0, 4.0, 0.9, Width::Slope),
            Band::new(Kind::HighShelf, 9000.0, -2.0, 0.7),
            Band::new(Kind::LowPass, 17_000.0, 0.0, 0.6),
            Band::new(Kind::HighPass, 22.0, 0.0, 0.707),
            Band::new(Kind::BandPass, 1000.0, 0.0, 4.0),
            Band::new(Kind::Notch, 60.0, 0.0, 25.0),
            Band::new(Kind::AllPass, 200.0, 0.0, 0.5),
        ];
        for preamp in [Preamp::Auto, Preamp::Manual(-5.25)] {
            let text = marquee_json("Desk", &bands, preamp);
            let p = ok(&text, None);
            assert_eq!(p.format, Format::Marquee(2));
            assert_eq!(p.bands, bands);
            assert_eq!(p.preamp, Some(preamp));
            assert_eq!(p.name.as_deref(), Some("Desk"));
            assert!(p.notes.is_empty());
        }
        // A newer version's file: read, said; an unknown kind refused.
        let newer = r#"{"marquee_player_equalizer": 3, "bands": [{"type": "peak", "freq": 100, "gain": 2, "q": 1, "colour": "red"}], "mood": "x"}"#;
        assert!(ok(newer, None).notes[0].text.contains("newer"));
        let unknown = r#"{"marquee_player_equalizer": 3, "bands": [{"type": "tilt", "freq": 100}]}"#;
        assert!(read(unknown, None).unwrap_err().reason.contains("tilt"));
    }

    fn ee_band(ty: &str, mode: &str, f: f64, g: f64, q: f64) -> Value {
        json!({"type": ty, "mode": mode, "slope": "x1", "solo": false, "mute": false, "gain": g, "frequency": f, "q": q, "width": 4.0})
    }

    fn ee_preset(bands: Vec<Value>, extra: Value) -> String {
        let mut ch = Map::new();
        for (i, b) in bands.iter().enumerate() {
            ch.insert(format!("band{i}"), b.clone());
        }
        let mut eq = json!({"bypass": false, "input-gain": -3.0, "output-gain": 1.0, "mode": "IIR", "split-channels": false, "balance": 0.0, "pitch-left": 0.0, "pitch-right": 0.0, "num-bands": bands.len(), "left": ch.clone(), "right": ch});
        for (k, v) in extra.as_object().unwrap() {
            eq[k] = v.clone();
        }
        json!({"output": {"blocklist": [], "equalizer#0": eq, "plugins_order": ["equalizer#0"]}}).to_string()
    }

    #[test]
    fn easyeffects_apo_mode_is_exact() {
        let text = ee_preset(
            vec![
                ee_band("Bell", "APO (DR)", 1000.0, 3.0, 1.5),
                ee_band("Lo-shelf", "APO (DR)", 105.0, 6.0, 0.7),
                ee_band("Hi-pass", "APO (DR)", 20.0, 5.0, 0.7),
                ee_band("Off", "APO (DR)", 50.0, 3.0, 1.0),
                ee_band("Notch", "APO (DR)", 60.0, 0.0, 30.0),
            ],
            json!({}),
        );
        let p = ok(&text, Some("Desk.json"));
        assert_eq!(p.format, Format::EasyEffects);
        assert_eq!(p.preamp, Some(Preamp::Manual(-2.0)), "input + output gain");
        assert_eq!(p.bands, vec![Band::new(Kind::Peak, 1000.0, 3.0, 1.5), Band::new(Kind::LowShelf, 105.0, 6.0, 0.7), Band::new(Kind::HighPass, 20.0, 0.0, 0.7), Band::new(Kind::Notch, 60.0, 0.0, 30.0)]);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!(p.name.as_deref(), Some("Desk"));
    }

    #[test]
    fn easyeffects_rlc_bands_are_the_cookbook_s_with_lsp_s_q() {
        // RLC (BT) Bell, x1: one peak; Q from LSP's k. Its curve is LSP's: the analog
        // prototype (1 + kt s + s^2) / (1 + kb s + s^2) at the warped frequency.
        let (g_db, q) = (6.0, 2.0);
        let text = ee_preset(vec![ee_band("Bell", "RLC (BT)", 1000.0, g_db, q)], json!({}));
        let p = ok(&text, None);
        assert_eq!(p.bands.len(), 1);
        let fg = 10f64.powf(g_db / 20.0);
        let k = 2.0 * (1.0 / fg + fg) / (1.0 + 2.0 * q);
        let (kt, kb) = (k * fg.atan().sin(), k * fg.atan().cos());
        let rate = 48_000;
        for f in [100.0, 500.0, 1000.0, 2000.0, 8000.0] {
            let om = (std::f64::consts::PI * f / 48_000.0).tan() / (std::f64::consts::PI * 1000.0 / 48_000.0).tan();
            let re = 1.0 - om * om;
            let lsp = 10.0 * ((re * re + (kt * om).powi(2)) / (re * re + (kb * om).powi(2))).log10();
            let here = response_db(&p.bands, rate, f);
            assert!((lsp - here).abs() < 1e-4, "{f} Hz: LSP {lsp} vs {here}");
        }
        // Slope x3 low-pass: three second-order sections; a shelf at x2: two of half the gain.
        let mut lp = ee_band("Lo-pass", "RLC (BT)", 5000.0, 0.0, 0.414);
        lp["slope"] = json!("x3");
        let mut ls = ee_band("Lo-shelf", "RLC (BT)", 100.0, 8.0, 0.4);
        ls["slope"] = json!("x2");
        let p = ok(&ee_preset(vec![lp, ls], json!({})), None);
        assert_eq!(p.bands.len(), 5);
        assert!(p.bands[..3].iter().all(|b| b.kind == Kind::LowPass && (b.q - 0.707).abs() < 1e-6));
        assert!(p.bands[3..].iter().all(|b| b.kind == Kind::LowShelf && b.gain_db == 4.0 && (b.q - 0.7).abs() < 1e-6));
        // Modes without a counterpart: left out, said; MT: approximated, said.
        let p = ok(&ee_preset(vec![ee_band("Bell", "BWC (BT)", 300.0, 3.0, 1.0), ee_band("Bell", "RLC (MT)", 300.0, 3.0, 1.0)], json!({})), None);
        assert_eq!(p.bands.len(), 1);
        assert_eq!(p.notes.iter().map(|n| n.kind).collect::<Vec<_>>(), vec![NoteKind::Left, NoteKind::Approximated]);
    }

    #[test]
    fn easyeffects_per_channel_and_the_rest() {
        let bands = vec![ee_band("Bell", "APO (DR)", 1000.0, 3.0, 1.5)];
        let mut text: Value = serde_json::from_str(&ee_preset(bands.clone(), json!({"split-channels": true}))).unwrap();
        text["output"]["equalizer#0"]["right"]["band0"]["gain"] = json!(-3.0);
        let e = read(&text.to_string(), None).unwrap_err();
        assert!(e.reason.contains("left and right") && e.reason.contains("split"), "{}", e.reason);
        let e = read(&ee_preset(bands.clone(), json!({"pitch-left": 1.0})), None).unwrap_err();
        assert!(e.reason.contains("pitched"));
        let e = read(&ee_preset(bands.clone(), json!({"bypass": true})), None).unwrap_err();
        assert!(e.reason.contains("off"));
        // Pitch on both sides, balance, other effects, a non-IIR mode, solo.
        let mut v: Value = serde_json::from_str(&ee_preset(vec![ee_band("Bell", "APO (DR)", 1000.0, 3.0, 1.5), ee_band("Bell", "APO (DR)", 2000.0, 3.0, 1.5)], json!({"pitch-left": 12.0, "pitch-right": 12.0, "balance": 10.0, "mode": "FIR"}))).unwrap();
        v["output"]["compressor#0"] = json!({});
        v["output"]["equalizer#0"]["left"]["band1"]["solo"] = json!(true);
        v["output"]["equalizer#0"]["right"]["band1"]["solo"] = json!(true);
        let p = ok(&v.to_string(), None);
        assert_eq!(p.bands, vec![Band::new(Kind::Peak, 4000.0, 3.0, 1.5)], "an octave up, only the solo band");
        let kinds: Vec<NoteKind> = p.notes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&NoteKind::Left) && kinds.contains(&NoteKind::Approximated), "{:?}", p.notes);
        // PulseEffects: every value a string, `state` instead of `bypass`.
        let pe = r#"{"output":{"equalizer":{"state":"true","mode":"IIR","num-bands":"2","input-gain":"-1","output-gain":"0","split-channels":"false","left":{"band0":{"type":"Bell","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"2.5","frequency":"100","q":"1"},"band1":{"type":"Hi-shelf","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"-2","frequency":"8000","q":"0.4"}},"right":{"band0":{"type":"Bell","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"2.5","frequency":"100","q":"1"},"band1":{"type":"Hi-shelf","mode":"RLC (BT)","slope":"x1","solo":"false","mute":"false","gain":"-2","frequency":"8000","q":"0.4"}}}}}"#;
        let p = ok(pe, None);
        assert_eq!(p.format, Format::PulseEffects);
        assert_eq!(p.bands.len(), 2);
        assert_eq!(p.bands[1], Band::new(Kind::HighShelf, 8000.0, -2.0, 0.7));
    }

    #[test]
    fn copies_as_easyeffects_and_reads_back_exactly() {
        let bands = vec![Band::new(Kind::Peak, 1000.0, 3.0, 1.5), Band::with_width(Kind::LowShelf, 105.0, 6.0, 1.0, Width::Slope), Band::new(Kind::LowPass, 16_000.0, 0.0, 0.5), Band::new(Kind::AllPass, 300.0, 0.0, 0.7)];
        let (text, note) = easyeffects_json(&bands, -6.5, true);
        assert!(note.is_none());
        let p = ok(&text, None);
        assert_eq!(p.preamp, Some(Preamp::Manual(-6.5)));
        for f in [30.0, 105.0, 1000.0, 5000.0, 15_000.0] {
            assert!((response_db(&p.bands, 48_000, f) - response_db(&bands, 48_000, f)).abs() < 1e-3, "{f}");
        }
    }

    #[test]
    fn graphic_eq_lines_use_autoeq_s_frequencies() {
        let f = graphic_freqs();
        assert_eq!((f.len(), f[0], f[1], f[126]), (127, 20.0, 21.0, 19871.0));
        let line = graphic_eq(&[Band::new(Kind::Peak, 1000.0, 6.0, 1.0)], -6.0, 48_000);
        assert!(line.starts_with("GraphicEQ: 20 -6.0; 21 -6.0;"), "{line}");
        // Read back, it is close to the filter it came from.
        let p = ok(&line, None);
        let fit = p.fit.unwrap();
        assert!(fit.max_err_db < 0.3, "{fit:?}");
    }
}
