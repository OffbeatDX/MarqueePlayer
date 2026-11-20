//! Equalizer APO's configuration text: what AutoEq's `ParametricEQ.txt` and
//! `FixedBandEQ.txt` are, what Peace exports, what Room EQ Wizard's "Filter Settings" text
//! export is (its Generic equaliser's `Q` form, its FBQ2496's `BW Oct` form), and what this
//! app copies and exports, so the text works in those too and comes back the same.
//!
//! ```text
//! # Marquee Player equalizer preset: FiiO FT1
//! Preamp: -4.3 dB
//! Filter 1: ON LSC Fc 105 Hz Gain -2.7 dB Q 0.7
//! Filter 2: ON PK Fc 155 Hz Gain -3.4 dB Q 1.47
//! Filter 3: ON LPQ Fc 16000 Hz Q 0.5
//! ```
//!
//! Read as Equalizer APO reads it: its configuration reference (sourceforge.net/p/
//! equalizerapo/wiki/Configuration reference) and its parser (`FilterEngine.cpp`,
//! `BiQuadFilterFactory.cpp`, `BiQuadFilter.cpp`, `PreampFilterFactory.cpp`,
//! `GraphicEQFilterFactory.cpp`, `GainIterator.cpp`, read 2026-11):
//!
//! - A line is `Command: parameters`; anything else (comments, REW's header) is ignored.
//! - `Preamp: <dB> dB`; several add up.
//! - `Filter[ n]: ON <type> ...` (any command starting `Filter`; the number is not read).
//!   `OFF`, and REW's unused `None` slots, play nothing. The parameters are found anywhere
//!   after the type: `Fc <Hz> Hz`, `Gain <dB> dB`, `Q <q>`, `BW Oct <octaves>`, and a
//!   shelf's slope as `<x> dB` right after its type. Every comma is a decimal point. A
//!   frequency of five characters or more with a point four from its end ("1.000") is a
//!   thousands separator (Room EQ Wizard writes them in some languages): 1000 Hz.
//! - Types: `PK`, `PEQ`, `Modal` (peaking, Q or BW required); `LP`, `LPQ`, `HP`, `HPQ`,
//!   `BP` (Q or BW, default 1/sqrt(2)); `NO` (default Q 30); `AP` (Q or BW required);
//!   `LS`, `HS`, `LSC`, `HSC`. A shelf with a Q, or a slope (`x dB` per octave, S = x / 12),
//!   is at its centre frequency when its type ends in `C` and at its corner frequency
//!   otherwise (`LS 6dB`, `LS 12dB`, `LS ... Q`), moved to the centre by Equalizer APO's own
//!   factor `10^(|gain| / 80 / S)`; a shelf with neither is S 0.9 at its centre. A shelf's
//!   bandwidth, a gain on a filter without one, a slope on a filter that is not a shelf are
//!   ignored, as there. A filter missing what it needs plays nothing there (the line is
//!   left out here, and said).
//! - `GraphicEQ: <Hz> <dB>; ...`: a curve, its gains interpolated linearly over log
//!   frequency and flat beyond its ends; commas are decimal points only when the line has
//!   no point. Fitted to bands (`fit.rs`).
//! - `Channel:` selects the channels what follows applies to, `Device:` the devices. What
//!   applies to both stereo channels on every device is read; what applies to one channel,
//!   to channels a stereo output does not have, or to particular devices is left out and
//!   said. `Stage: post-mix` is the default.
//!
//! Keywords are read in any letter case here (Equalizer APO itself reads them only as its
//! reference writes them; the import says so when it matters).
//!
//! Refused, with the line named, because what they do is not in the text or cannot be
//! played here: `Include:` (another file), `Convolution:` (an impulse response file),
//! `IIR` filters (coefficients for one sample rate), `If:` / `Eval:` and inline
//! expressions (worked out by Equalizer APO as it runs). Left out and said: `Copy:`
//! (channel mixing), `Delay:`, `LoudnessCorrection:`, `Stage: pre-mix`.

use super::formats::{Format, Import, Note, NoteKind, Refusal};
use super::{Band, Kind, Preamp, Width};

/// The first line of a copied or exported preset, before its name.
pub const HEADER: &str = "# Marquee Player equalizer preset:";
/// The comment saying the preamp is automatic (the `Preamp:` line has its value now).
pub const AUTO: &str = "# Marquee Player: preamp auto";
/// The comment saying the equalizer was off when copied (every filter is written `OFF`).
pub const OFF_NOTE: &str = "# Marquee Player: equalizer off";

/// The longest float at the start of `s`, as C's `wcstod` reads it (`"1.2.3"` is 1.2;
/// nothing readable is `None`).
fn float_prefix(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut end = 0;
    let mut best = None;
    while end < b.len() && (b[end].is_ascii_digit() || matches!(b[end], b'+' | b'-' | b'.' | b'e' | b'E')) {
        end += 1;
        if let Ok(v) = s[..end].parse::<f64>()
            && v.is_finite()
        {
            best = Some(v);
        }
    }
    best
}

/// Case-insensitive search for `word` preceded by whitespace (or, with `at_start`, at the
/// start). Returns the byte index just after it.
fn find_word(hay: &str, word: &str, at_start: bool) -> Option<usize> {
    let lower = hay.to_ascii_lowercase();
    let w = word.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&w) {
        let at = from + i;
        let before_ok = if at == 0 { at_start } else { lower.as_bytes()[at - 1].is_ascii_whitespace() };
        if before_ok {
            return Some(at + w.len());
        }
        from = at + w.len();
    }
    None
}

/// The run of number characters after optional whitespace, and what follows it.
fn number_run(s: &str, nbsp: bool) -> (String, &str) {
    let t = s.trim_start();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E') || (nbsp && c == '\u{a0}') {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    (t[..end].to_owned(), &t[end..])
}

/// `Fc <n> Hz` (the `Hz` may have spaces in and before it): the frequency, and whether
/// the thousands-separator rule made it.
fn param_freq(rest: &str) -> Option<(f64, bool)> {
    let at = find_word(rest, "fc", false)?;
    let (run, after) = number_run(&rest[at..], true);
    let unit: String = after.chars().filter(|c| !c.is_whitespace()).take(2).collect::<String>().to_ascii_lowercase();
    if run.is_empty() || unit != "hz" {
        return None;
    }
    let s: String = run.chars().filter(|&c| c != '\u{a0}').collect();
    let mut v = float_prefix(&s)?;
    let thousands = s.len() >= 5 && !s.contains(['e', 'E']) && s.as_bytes()[s.len() - 4] == b'.';
    if thousands {
        v *= 1000.0;
    }
    Some((v, thousands))
}

/// `Gain <n> dB`.
fn param_gain(rest: &str) -> Option<f64> {
    let at = find_word(rest, "gain", false)?;
    let (run, after) = number_run(&rest[at..], false);
    if !after.trim_start().to_ascii_lowercase().starts_with("db") {
        return None;
    }
    float_prefix(&run)
}

/// `Q <n>` (0 when there is a `Q` but no number, as `wcstod` gives).
fn param_q(rest: &str) -> Option<f64> {
    let at = find_word(rest, "q", false)?;
    let (run, _) = number_run(&rest[at..], false);
    if run.is_empty() {
        return None;
    }
    Some(float_prefix(&run).unwrap_or(0.0))
}

/// `BW Oct <n>`.
fn param_bw(rest: &str) -> Option<f64> {
    let at = find_word(rest, "bw", false)?;
    let after = &rest[at..];
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let after = after.trim_start();
    if !after.to_ascii_lowercase().starts_with("oct") {
        return None;
    }
    let (run, _) = number_run(&after[3..], false);
    if run.is_empty() {
        return None;
    }
    Some(float_prefix(&run).unwrap_or(0.0))
}

/// A shelf's slope right after its type: `6dB`, `10.8 dB`.
fn param_slope(rest: &str) -> Option<f64> {
    let (run, after) = number_run(rest, false);
    if run.is_empty() || !after.trim_start().to_ascii_lowercase().starts_with("db") {
        return None;
    }
    float_prefix(&run)
}

/// Where the following commands apply.
#[derive(Clone, Debug, PartialEq)]
enum Scope {
    /// Both stereo channels, every device.
    Common,
    /// Somewhere else: why it is left out ("the left channel only").
    Elsewhere(String, NoteKind),
}

fn channel_scope(args: &str) -> Scope {
    let words: Vec<String> = args.split_whitespace().map(|w| w.to_ascii_uppercase()).collect();
    if words.iter().any(|w| w == "ALL") {
        return Scope::Common;
    }
    let left = words.iter().any(|w| w == "L" || w == "1");
    let right = words.iter().any(|w| w == "R" || w == "2");
    match (left, right) {
        (true, true) => Scope::Common,
        (true, false) => Scope::Elsewhere("the left channel only".into(), NoteKind::Left),
        (false, true) => Scope::Elsewhere("the right channel only".into(), NoteKind::Left),
        (false, false) => Scope::Elsewhere(format!("channels a stereo output does not have ({})", args.trim()), NoteKind::Info),
    }
}

/// What a filter line holds, once read.
enum FilterLine {
    Band(Band, Vec<Note>),
    /// Plays nothing in Equalizer APO either (OFF, None): with a note, or none.
    Nothing(Option<Note>),
    /// Plays nothing there because something it needs is missing: left out, with why.
    Broken(String),
    /// Cannot be played here at all: the import is refused.
    Refuse(String),
}

fn kind_of(ty: &str) -> Option<Kind> {
    Some(match ty {
        "PK" | "PEQ" | "MODAL" => Kind::Peak,
        "LP" | "LPQ" => Kind::LowPass,
        "HP" | "HPQ" => Kind::HighPass,
        "BP" => Kind::BandPass,
        "LS" | "LSC" => Kind::LowShelf,
        "HS" | "HSC" => Kind::HighShelf,
        "NO" => Kind::Notch,
        "AP" => Kind::AllPass,
        _ => return None,
    })
}

/// One `Filter` line's parameters (after the colon), read as Equalizer APO reads them.
fn filter_line(no: usize, raw: &str, args: &str) -> FilterLine {
    let params = args.replace(',', ".");
    let t = params.trim_start();
    let (state, rest) = t.split_at(t.find(char::is_whitespace).unwrap_or(t.len()));
    match state.to_ascii_uppercase().as_str() {
        "ON" => {}
        "OFF" => return FilterLine::Nothing(Some(Note::new(NoteKind::Info, format!("Line {no}: an OFF filter, which plays nothing (in Equalizer APO either)")))),
        _ => return FilterLine::Broken(format!("Line {no}: a filter starts with ON or OFF (“{}”)", raw.trim())),
    }
    let rest = rest.trim_start();
    let ty_end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
    let ty_raw = &rest[..ty_end];
    let ty = ty_raw.to_ascii_uppercase();
    let rest = &rest[ty_end..];
    if ty.is_empty() {
        return FilterLine::Broken(format!("Line {no}: no filter type (“{}”)", raw.trim()));
    }
    if ty == "NONE" {
        return FilterLine::Nothing(None);
    }
    if ty == "IIR" {
        return FilterLine::Refuse(format!(
            "Line {no} is a filter given by its coefficients (IIR), which are for one sample rate: it cannot be played at the rate playing here. Room EQ Wizard and AutoEq can write the same filter as a PK, LS or HS filter instead."
        ));
    }
    let Some(kind) = kind_of(&ty) else {
        let hint = if matches!(ty.as_str(), "LP1" | "HP1" | "LT" | "LSQ" | "HSQ") { " (a Room EQ Wizard filter type Equalizer APO does not read either)" } else { "" };
        return FilterLine::Broken(format!("Line {no}: “{ty_raw}” is not a filter type Equalizer APO has{hint}"));
    };
    let mut notes = Vec::new();
    let Some((freq, thousands)) = param_freq(rest) else {
        return FilterLine::Broken(format!("Line {no}: the filter has no frequency (“Fc … Hz”), so it plays nothing (“{}”)", raw.trim()));
    };
    if thousands {
        notes.push(Note::new(NoteKind::Info, format!("Line {no}: the frequency is read as {freq:.0} Hz, the point a thousands separator, as Equalizer APO reads it")));
    }
    if freq <= 0.0 {
        return FilterLine::Broken(format!("Line {no}: the frequency is not above 0 Hz (“{}”)", raw.trim()));
    }
    let gain = param_gain(rest);
    let gain = if kind.has_gain() {
        match gain {
            Some(g) => g,
            None => return FilterLine::Broken(format!("Line {no}: the filter has no gain (“Gain … dB”), so it plays nothing (“{}”)", raw.trim())),
        }
    } else {
        0.0
    };
    // As Equalizer APO: Q, then a bandwidth (not for shelves), then a shelf's slope.
    let mut value = param_q(rest).unwrap_or(0.0);
    let mut width = Width::Q;
    if !kind.is_shelf()
        && let Some(bw) = param_bw(rest)
    {
        value = bw;
        width = Width::Octaves;
    }
    if raw.contains("BW/60") && value == 0.0 {
        return FilterLine::Broken(format!(
            "Line {no}: Room EQ Wizard’s DSP1124P bandwidth (BW/60) is not a width Equalizer APO or this equalizer reads; export the filters with REW’s Generic equaliser (“{}”)",
            raw.trim()
        ));
    }
    if kind.is_shelf()
        && let Some(s) = param_slope(rest)
    {
        value = s;
        width = Width::Slope;
    }
    let mut corner = false;
    if value == 0.0 {
        match kind {
            Kind::Peak | Kind::AllPass => return FilterLine::Broken(format!("Line {no}: the filter has no Q or bandwidth, so it plays nothing (“{}”)", raw.trim())),
            Kind::LowPass | Kind::HighPass | Kind::BandPass => value = std::f64::consts::FRAC_1_SQRT_2,
            Kind::LowShelf | Kind::HighShelf => {
                value = 0.9;
                width = Width::Slope;
            }
            Kind::Notch => value = 30.0,
        }
    } else if kind.is_shelf() {
        if width == Width::Slope {
            value /= 12.0;
        }
        corner = !ty.ends_with('C');
    }
    if value < 0.0 {
        return FilterLine::Broken(format!("Line {no}: a negative width (“{}”)", raw.trim()));
    }
    let mut freq = freq;
    if corner {
        // Equalizer APO's move from the corner to the centre (for the DCX2496's shelves).
        let a = 10f64.powf(gain / 40.0);
        let s = if width == Width::Q { 1.0 / ((1.0 / (value * value) - 2.0) / (a + 1.0 / a) + 1.0) } else { value };
        let factor = 10f64.powf(gain.abs() / 80.0 / s);
        if !factor.is_finite() || factor <= 0.0 || s <= 0.0 {
            return FilterLine::Broken(format!("Line {no}: this shelf’s corner frequency and Q do not give a shelf (“{}”)", raw.trim()));
        }
        let centre = if kind == Kind::LowShelf { freq * factor } else { freq / factor };
        notes.push(Note::new(
            NoteKind::Info,
            format!("Line {no}: a shelf given by its corner frequency ({}), here by its midpoint, {} (as Equalizer APO places it)", hz(freq), hz(centre)),
        ));
        freq = centre;
    }
    if width == Width::Slope {
        let a = 10f64.powf(gain / 40.0);
        if (a + 1.0 / a) * (1.0 / value - 1.0) + 2.0 <= 0.0 {
            return FilterLine::Broken(format!("Line {no}: this slope is steeper than a shelf of this gain can be (“{}”)", raw.trim()));
        }
    }
    let band = Band::with_width(kind, freq as f32, gain as f32, value as f32, width);
    let off = |a: f64, b: f32| (a - f64::from(b)).abs() > 1e-3 * a.abs().max(1.0);
    if off(freq, band.freq) || (kind.has_gain() && off(gain, band.gain_db)) || off(value, band.q) {
        let (lo, hi) = width.range();
        let what = match width {
            Width::Q => format!("Q {lo} to {hi}"),
            Width::Octaves => format!("{lo} to {hi} octaves"),
            Width::Slope => format!("slopes {:.1} to {:.0} dB per octave", lo * 12.0, hi * 12.0),
        };
        notes.push(Note::new(NoteKind::Approximated, format!("Line {no}: held within the equalizer’s range (10 Hz to 24 kHz, ±24 dB, {what})")));
    }
    FilterLine::Band(band, notes)
}

/// The keywords as Equalizer APO's reference writes them (it reads no other case).
fn apo_case(args: &str) -> bool {
    const TYPES: [&str; 16] = ["PK", "PEQ", "Modal", "LP", "LPQ", "HP", "HPQ", "BP", "LS", "HS", "LSC", "HSC", "NO", "AP", "None", "IIR"];
    let w: Vec<&str> = args.split_whitespace().collect();
    let state = w.first().is_none_or(|s| *s == "ON" || *s == "OFF");
    let ty = w.get(1).is_none_or(|t| TYPES.contains(t) || !t.chars().all(|c| c.is_ascii_alphabetic()));
    let words = w.iter().all(|x| match x.to_ascii_lowercase().as_str() {
        "fc" => *x == "Fc",
        "hz" => *x == "Hz",
        "gain" => *x == "Gain",
        "db" => *x == "dB",
        "q" => *x == "Q",
        "bw" => *x == "BW",
        "oct" => *x == "Oct",
        _ => true,
    });
    state && ty && words
}

/// "105 Hz", "1.2 kHz".
fn hz(f: f64) -> String {
    if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0).replace(".00 ", " ") } else { format!("{f:.0} Hz") }
}

/// A `GraphicEQ:` line's points, sorted by frequency, as Equalizer APO reads them.
pub fn graphic_points(args: &str) -> Result<Vec<(f64, f64)>, String> {
    let value = if args.contains('.') { args.to_owned() } else { args.replace(',', ".") };
    let mut nums = Vec::new();
    let mut cur = String::new();
    for c in value.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E') {
            cur.push(c);
        } else if !cur.is_empty() {
            nums.push(float_prefix(&cur).unwrap_or(0.0));
            cur.clear();
            if nums.len() > 2 * super::fit::MAX_POINTS {
                return Err(format!("more than {} points", super::fit::MAX_POINTS));
            }
        }
    }
    let mut pts: Vec<(f64, f64)> = nums.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
    if pts.iter().any(|&(f, g)| f <= 0.0 || !f.is_finite() || !g.is_finite()) {
        return Err("a point at 0 Hz or below".into());
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(pts)
}

/// Read Equalizer APO text (and the formats that are it). `rew`: the text is Room EQ
/// Wizard's export (its header lines say nothing to read).
pub fn read(text: &str) -> Result<Import, Refusal> {
    let mut bands = Vec::new();
    let mut notes: Vec<Note> = Vec::new();
    let mut preamp_db: Option<f64> = None;
    let mut auto = false;
    let mut off_copy = false;
    let mut name = None;
    let mut rew: Option<String> = None;
    let mut graphic: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut understood = false;
    let mut channel = Scope::Common;
    let mut device = Scope::Common;
    let mut lowercase = false;
    let mut off_filters = 0usize;
    let refuse = |reason: String| Refusal { format: Some(Format::Apo), reason };
    for (i, raw) in text.lines().enumerate() {
        let no = i + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if let Some(rest) = line.strip_prefix(HEADER) {
            let n = rest.trim();
            if !n.is_empty() {
                name = Some(n.to_owned());
            }
            continue;
        }
        if line.eq_ignore_ascii_case(AUTO) {
            auto = true;
            continue;
        }
        if line.eq_ignore_ascii_case(OFF_NOTE) {
            off_copy = true;
            continue;
        }
        if line.eq_ignore_ascii_case("Filter Settings file") || line.starts_with("Room EQ V") {
            rew.get_or_insert_with(String::new);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((cmd, args)) = line.split_once(':') else { continue };
        let cmd = cmd.trim();
        let lc = cmd.to_ascii_lowercase();
        if lc == "equaliser" || lc == "equalizer" {
            rew = Some(args.trim().to_owned());
            continue;
        }
        let here = match (&channel, &device) {
            (Scope::Common, Scope::Common) => None,
            (Scope::Elsewhere(w, k), Scope::Common) | (_, Scope::Elsewhere(w, k)) => Some((w.clone(), *k)),
        };
        let applies = |what: &str, notes: &mut Vec<Note>| -> bool {
            match &here {
                None => true,
                Some((w, k)) => {
                    notes.push(Note::new(*k, format!("Line {no}: {what} for {w}: left out (this equalizer treats both channels alike, on every device)")));
                    false
                }
            }
        };
        if (lc.starts_with("filter") && (!cmd.starts_with("Filter") || !apo_case(args))) || (lc == "preamp" && cmd != "Preamp") || (lc == "graphiceq" && cmd != "GraphicEQ") {
            lowercase = true;
        }
        match lc.as_str() {
            "preamp" => {
                understood = true;
                match float_prefix(args.replace(',', ".").trim()) {
                    // Equalizer APO ignores a preamp it cannot read.
                    None => notes.push(Note::new(NoteKind::Left, format!("Line {no}: the preamp is not a number, so it plays nothing (“{}”)", raw.trim()))),
                    Some(v) => {
                        if applies("a preamp", &mut notes) {
                            preamp_db = Some(preamp_db.unwrap_or(0.0) + v);
                        }
                    }
                }
            }
            "channel" => {
                understood = true;
                channel = channel_scope(args);
            }
            "device" => {
                understood = true;
                device = if args.split(';').any(|p| p.trim().eq_ignore_ascii_case("all")) { Scope::Common } else { Scope::Elsewhere(format!("the device “{}” only", args.trim()), NoteKind::Left) };
            }
            "graphiceq" => {
                understood = true;
                let pts = graphic_points(args).map_err(|e| refuse(format!("Line {no}: this GraphicEQ curve cannot be read ({e})")))?;
                if pts.is_empty() {
                    notes.push(Note::new(NoteKind::Info, format!("Line {no}: a GraphicEQ line without points")));
                } else if applies("a GraphicEQ curve", &mut notes) {
                    graphic.push(pts);
                }
            }
            "include" => return Err(refuse(format!("Line {no} includes another file (“{}”), which is not in this text: import or paste that file instead.", args.trim()))),
            "convolution" => return Err(refuse(format!("Line {no} is a convolution with the impulse response in “{}”, which this equalizer cannot play: use AutoEq’s ParametricEQ.txt for the same correction.", args.trim()))),
            "if" | "elseif" | "else" | "endif" | "eval" => {
                return Err(refuse(format!("Line {no} is an expression (“{}:”), worked out by Equalizer APO as it runs; this equalizer cannot follow it.", cmd)));
            }
            "copy" => {
                understood = true;
                notes.push(Note::new(NoteKind::Left, format!("Line {no}: channel mixing (“Copy: {}”) has no equivalent here: left out", args.trim())));
            }
            "delay" => {
                understood = true;
                notes.push(Note::new(NoteKind::Left, format!("Line {no}: a delay (“Delay: {}”) has no equivalent here: left out", args.trim())));
            }
            "loudnesscorrection" => {
                understood = true;
                notes.push(Note::new(NoteKind::Left, format!("Line {no}: Equalizer APO’s loudness correction has no equivalent here: left out")));
            }
            "stage" => {
                understood = true;
                if !args.split_whitespace().all(|w| w.eq_ignore_ascii_case("post-mix")) {
                    notes.push(Note::new(NoteKind::Info, format!("Line {no}: “Stage: {}” is not this equalizer’s concern: what follows is read as it is", args.trim())));
                }
            }
            _ if lc.starts_with("filter") => {
                understood = true;
                if args.contains('`') {
                    return Err(refuse(format!("Line {no} has an inline expression (`…`), worked out by Equalizer APO as it runs; this equalizer cannot follow it.")));
                }
                match filter_line(no, raw, args) {
                    FilterLine::Band(b, n) => {
                        if applies("a filter", &mut notes) {
                            bands.push(b);
                            notes.extend(n);
                        }
                    }
                    FilterLine::Nothing(n) => {
                        if n.is_some() {
                            off_filters += 1;
                        }
                        if !off_copy {
                            notes.extend(n);
                        }
                    }
                    FilterLine::Broken(why) => notes.push(Note::new(NoteKind::Left, why)),
                    FilterLine::Refuse(why) => return Err(refuse(why)),
                }
            }
            // Room EQ Wizard's header lines, and anything else Equalizer APO ignores.
            _ => {}
        }
    }
    if !understood {
        return Err(Refusal { format: None, reason: "This text has no equalizer settings in it.".into() });
    }
    let format = if name.is_some() || auto || off_copy {
        Format::Marquee(1)
    } else if let Some(eq) = rew {
        Format::Rew(eq)
    } else if !graphic.is_empty() && bands.is_empty() {
        Format::GraphicEq
    } else {
        Format::Apo
    };
    if lowercase {
        notes.push(Note::new(NoteKind::Info, "Keywords in another letter case are read here; Equalizer APO itself reads them only as its reference writes them (Filter, ON, PK, Fc…)".into()));
    }
    if bands.is_empty() && graphic.is_empty() {
        let why = if off_filters > 0 { format!("All {off_filters} of its filters are OFF: there is nothing to play.") } else { "No filters in this text.".to_owned() };
        return Err(Refusal { format: Some(format), reason: why });
    }
    let preamp = if auto { Some(Preamp::Auto) } else { preamp_db.map(|d| Preamp::Manual(d as f32)) };
    Ok(Import { format, bands, preamp, name, notes, fit: None, graphic })
}

/// Trim a number's needless zeros: 105, 1.41, -2.7.
pub fn num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
    if s == "-0" { "0".into() } else { s }
}

/// One band as an Equalizer APO filter's parameters (after `ON `/`OFF `), in the form its
/// reference, AutoEq and Room EQ Wizard write: `PK Fc 105 Hz Gain -2.7 dB Q 0.7`. A
/// frequency has at most two decimals, so Equalizer APO's thousands-separator rule never
/// reads it as a thousand times more.
pub fn filter_text(b: &Band) -> String {
    let fc = num(f64::from(b.freq), 2);
    let gain = num(f64::from(b.gain_db), 2);
    let w = match b.width {
        Width::Q => format!("Q {}", num(f64::from(b.q), 3)),
        Width::Octaves => format!("BW Oct {}", num(f64::from(b.q), 3)),
        Width::Slope => String::new(),
    };
    match b.kind {
        Kind::Peak => format!("PK Fc {fc} Hz Gain {gain} dB {w}"),
        Kind::LowShelf | Kind::HighShelf => {
            let ty = if b.kind == Kind::LowShelf { "LSC" } else { "HSC" };
            if b.width == Width::Slope { format!("{ty} {} dB Fc {fc} Hz Gain {gain} dB", num(f64::from(b.q) * 12.0, 2)) } else { format!("{ty} Fc {fc} Hz Gain {gain} dB {w}") }
        }
        Kind::LowPass | Kind::HighPass => {
            let (plain, with_q) = if b.kind == Kind::LowPass { ("LP", "LPQ") } else { ("HP", "HPQ") };
            if b.width == Width::Q && b.q == std::f32::consts::FRAC_1_SQRT_2 { format!("{plain} Fc {fc} Hz") } else { format!("{with_q} Fc {fc} Hz {w}") }
        }
        Kind::BandPass => format!("BP Fc {fc} Hz {w}"),
        Kind::Notch => format!("NO Fc {fc} Hz {w}"),
        Kind::AllPass => format!("AP Fc {fc} Hz {w}"),
    }
}

/// A setting as Equalizer APO text: the name comment, `Preamp:` with the preamp that plays
/// (the auto one's value now, marked auto so this app reads it back as auto), and one
/// `Filter n:` line per band. Off, every filter is `OFF` and the preamp 0 dB: Equalizer
/// APO then plays what this app plays, nothing.
pub fn write(name: &str, bands: &[Band], preamp: Preamp, auto_db: f64, enabled: bool) -> String {
    let mut s = format!("{HEADER} {}\n", name.replace(['\r', '\n'], " "));
    if !enabled {
        s.push_str(OFF_NOTE);
        s.push('\n');
    }
    let db = match preamp {
        Preamp::Auto => {
            s.push_str(AUTO);
            s.push('\n');
            auto_db
        }
        Preamp::Manual(db) => f64::from(db),
    };
    s.push_str(&format!("Preamp: {} dB\n", num(if enabled { db } else { 0.0 }, 2)));
    for (i, b) in bands.iter().enumerate() {
        s.push_str(&format!("Filter {}: {} {}\n", i + 1, if enabled { "ON" } else { "OFF" }, filter_text(b)));
    }
    s
}

/// A preset name from a file's name: AutoEq's "Sennheiser HD 650 ParametricEQ.txt" is
/// "Sennheiser HD 650".
pub fn name_from_file(stem: &str) -> String {
    let t = stem.trim();
    let lower = t.to_ascii_lowercase();
    for suffix in ["parametriceq", "fixedbandeq", "graphiceq"] {
        if lower.ends_with(suffix) {
            let cut = t.len() - suffix.len();
            return t[..cut].trim_end_matches([' ', '_', '-']).trim().to_owned();
        }
    }
    t.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_ok(text: &str) -> Import {
        read(text).unwrap_or_else(|e| panic!("{text}: {}", e.reason))
    }

    #[test]
    fn reads_an_autoeq_file() {
        let text = "Preamp: -6.4 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.2 dB Q 0.70\nFilter 2: ON PK Fc 156 Hz Gain -3.4 dB Q 0.41\nFilter 3: ON HSC Fc 10000 Hz Gain 2.1 dB Q 0.70\n";
        let p = read_ok(text);
        assert_eq!(p.format, Format::Apo);
        assert_eq!(p.preamp, Some(Preamp::Manual(-6.4)));
        assert_eq!(p.bands.len(), 3);
        assert_eq!(p.bands[0], Band::new(Kind::LowShelf, 105.0, 6.2, 0.70));
        assert_eq!(p.bands[1], Band::new(Kind::Peak, 156.0, -3.4, 0.41));
        assert_eq!(p.bands[2].kind, Kind::HighShelf);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
    }

    /// Every example of Equalizer APO's configuration reference, as it reads them.
    #[test]
    fn reads_the_reference_s_examples() {
        let cases: &[(&str, Band)] = &[
            ("Filter  1: ON  PK       Fc     50 Hz   Gain  -3.0 dB  Q 10.00", Band::new(Kind::Peak, 50.0, -3.0, 10.0)),
            ("Filter  2: ON  PEQ      Fc     100 Hz  Gain   1.0 dB  BW Oct 0.167", Band::with_width(Kind::Peak, 100.0, 1.0, 0.167, Width::Octaves)),
            ("Filter: ON  Modal    Fc     100 Hz  Gain   3.0 dB  Q  5.41  T60 target   100 ms", Band::new(Kind::Peak, 100.0, 3.0, 5.41)),
            ("Filter: ON  LP       Fc   8000 Hz", Band::new(Kind::LowPass, 8000.0, 0.0, std::f32::consts::FRAC_1_SQRT_2)),
            ("Filter  2: ON  LPQ      Fc     10000 Hz  Q  0.400", Band::new(Kind::LowPass, 10_000.0, 0.0, 0.4)),
            ("Filter  1: ON  HP       Fc     30 Hz", Band::new(Kind::HighPass, 30.0, 0.0, std::f32::consts::FRAC_1_SQRT_2)),
            ("Filter: ON  HPQ      Fc    20.0 Hz  Q  0.500", Band::new(Kind::HighPass, 20.0, 0.0, 0.5)),
            ("Filter: ON BP Fc 1000 Hz Q 0.100", Band::new(Kind::BandPass, 1000.0, 0.0, 0.1)),
            ("Filter  1: ON  LS       Fc     300 Hz  Gain   5.0 dB", Band::with_width(Kind::LowShelf, 300.0, 5.0, 0.9, Width::Slope)),
            ("Filter: ON  LSC 10.8 dB Fc     300 Hz  Gain   5.0 dB", Band::with_width(Kind::LowShelf, 300.0, 5.0, 0.9, Width::Slope)),
            ("Filter: ON  LSC Fc     300 Hz  Gain   5.0 dB Q 0.6473", Band::new(Kind::LowShelf, 300.0, 5.0, 0.6473)),
            ("Filter: ON  HS       Fc   1000 Hz  Gain  -3.0 dB", Band::with_width(Kind::HighShelf, 1000.0, -3.0, 0.9, Width::Slope)),
            ("Filter: ON  HSC 6 dB Fc     100 Hz  Gain   -6.0 dB", Band::with_width(Kind::HighShelf, 100.0, -6.0, 0.5, Width::Slope)),
            ("Filter: ON HSC Fc 100 Hz Gain -6.0 dB Q 0.4272", Band::new(Kind::HighShelf, 100.0, -6.0, 0.4272)),
            ("Filter: ON  NO       Fc     800 Hz", Band::new(Kind::Notch, 800.0, 0.0, 30.0)),
            ("Filter: ON  AP       Fc     900 Hz  Q  0.707", Band::new(Kind::AllPass, 900.0, 0.0, 0.707)),
        ];
        for (line, want) in cases {
            let p = read_ok(line);
            assert_eq!(p.bands, vec![*want], "{line}");
        }
        // The corner-frequency shelves: moved to their midpoint by 10^(|g| / 80 / S).
        let p = read_ok("Filter: ON  LS 6dB   Fc    50.0 Hz  Gain   7.2 dB");
        let want = 50.0 * 10f64.powf(7.2 / 80.0 / 0.5);
        assert!((f64::from(p.bands[0].freq) - want).abs() < 0.01, "{:?}", p.bands);
        assert_eq!((p.bands[0].width, p.bands[0].q), (Width::Slope, 0.5));
        let p = read_ok("Filter: ON  LS 12dB  Fc   2000 Hz  Gain  -5.0 dB");
        assert!((f64::from(p.bands[0].freq) - 2000.0 * 10f64.powf(5.0 / 80.0)).abs() < 0.1);
        let p = read_ok("Filter: ON  HS 12dB  Fc     500 Hz  Gain   5.0 dB");
        assert!((f64::from(p.bands[0].freq) - 500.0 / 10f64.powf(5.0 / 80.0)).abs() < 0.01);
        let p = read_ok("Filter: ON  HS 6dB   Fc  12000 Hz  Gain  10.0 dB");
        assert!((f64::from(p.bands[0].freq) - 12_000.0 / 10f64.powf(10.0 / 40.0)).abs() < 0.1);
        // A plain LS with a Q is at its corner too: S from the Q, the gain and the cookbook.
        let p = read_ok("Filter: ON LS Fc 300 Hz Gain 6 dB Q 0.7071");
        assert!((f64::from(p.bands[0].freq) - 300.0 * 10f64.powf(6.0 / 80.0 / 1.0)).abs() < 0.05, "{:?}", p.bands);
        assert_eq!(p.bands[0].width, Width::Q);
    }

    #[test]
    fn reads_the_variants_equalizer_apo_allows() {
        let text = "# headphones\r\npreamp: -3 db\nPreamp: -1,5 dB\nFilter: on pk fc 1000 hz gain 3 db bw oct 1\nFilter  2: OFF PK Fc 50 Hz Gain -10 dB Q 2.5\nFilter3: ON PK Fc 1,000 Hz Gain -1,5 dB Q 2\nChannel: all\nFilter 4: ON None\nFilter 5: ON PK Fc 2 000 Hz Gain 1 dB Q 1\n";
        let p = read_ok(text);
        assert_eq!(p.preamp, Some(Preamp::Manual(-4.5)));
        assert_eq!(p.bands.len(), 2, "{:?}", p.bands);
        assert_eq!((p.bands[0].width, p.bands[0].q), (Width::Octaves, 1.0));
        assert_eq!((p.bands[1].freq, p.bands[1].gain_db), (1000.0, -1.5), "1,000 is 1.000, read as a thousand");
        // The OFF filter is said (info), the space-separated frequency is no number, the
        // lower case keywords are said.
        assert!(p.notes.iter().any(|n| n.kind == NoteKind::Info && n.text.contains("OFF")), "{:?}", p.notes);
        assert!(p.notes.iter().any(|n| n.kind == NoteKind::Left && n.text.contains("Line 9")), "{:?}", p.notes);
        assert!(p.notes.iter().any(|n| n.text.contains("letter case")), "{:?}", p.notes);
        assert!(p.notes.iter().any(|n| n.text.contains("thousands")), "{:?}", p.notes);
    }

    #[test]
    fn per_channel_and_per_device_parts_are_left_out_and_said() {
        let text = "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nChannel: L\nPreamp: -5 dB\nFilter 2: ON PK Fc 200 Hz Gain 2 dB Q 1\nChannel: C LFE\nFilter 3: ON HP Fc 30 Hz\nChannel: L R\nFilter 4: ON PK Fc 400 Hz Gain 1 dB Q 1\nDevice: Speakers\nFilter 5: ON PK Fc 800 Hz Gain 1 dB Q 1\nDevice: all\nFilter 6: ON PK Fc 1600 Hz Gain 1 dB Q 1\n";
        let p = read_ok(text);
        let freqs: Vec<f32> = p.bands.iter().map(|b| b.freq).collect();
        assert_eq!(freqs, vec![100.0, 400.0, 1600.0]);
        assert_eq!(p.preamp, Some(Preamp::Manual(-3.0)));
        let left: Vec<&Note> = p.notes.iter().filter(|n| n.kind == NoteKind::Left).collect();
        assert_eq!(left.len(), 3, "{:?}", p.notes);
        assert!(left[0].text.contains("left channel"));
        assert!(left[2].text.contains("Speakers"));
        assert!(p.notes.iter().any(|n| n.kind == NoteKind::Info && n.text.contains("does not have")));
    }

    #[test]
    fn refuses_what_it_cannot_play_and_says_what_it_left_out() {
        for (text, says) in [
            ("Filter: ON IIR Order 2 Coefficients 0.0380602 0.0761205 0.0380602 1.2706 -1.84776 0.729402", "coefficients"),
            ("Include: other.txt", "includes another file"),
            ("Convolution: church.wav", "convolution"),
            ("If: sampleRate == 44100\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1\nEndIf:", "expression"),
            ("Filter: ON PK Fc 1000 Hz Gain `20*log10(0.5)` dB Q 10.0", "inline expression"),
            ("Preamp: -3 dB", "No filters"),
            ("Filter 1: OFF PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: OFF PK Fc 200 Hz Gain 3 dB Q 1", "OFF"),
        ] {
            let e = read(text).unwrap_err();
            assert!(e.reason.contains(says), "{text}: {}", e.reason);
            assert!(e.format.is_some(), "{text}");
        }
        assert!(read("hello\nworld").unwrap_err().format.is_none(), "not an equalizer at all");
        assert!(read("Time: 10:30\nhttp://example.com/a:b").unwrap_err().format.is_none(), "colons are not settings");
        let p = read_ok("Preamp: loud\nFilter: ON PK Fc 100 Hz Gain 3 dB Q 1");
        assert!(p.preamp.is_none() && p.notes.iter().any(|n| n.kind == NoteKind::Left && n.text.contains("not a number")));
        // Left out with the line named, the rest read.
        let p = read_ok("Filter 1: ON PK Fc 100 Hz Gain 3 dB\nFilter 2: ON XYZ Fc 100 Hz\nFilter 3: ON PK Gain 3 dB Q 1\nFilter 4: ON PK Fc 100 Hz Gain 3 dB Q 1\nCopy: L=R\nDelay: 5 ms\nFilter 5: ON PA Fc 129.1Hz ( 125 +2 ) Gain -18.5dB BW/60 4.0");
        assert_eq!(p.bands.len(), 1);
        let left: Vec<String> = p.notes.iter().filter(|n| n.kind == NoteKind::Left).map(|n| n.text.clone()).collect();
        assert_eq!(left.len(), 6, "{left:?}");
        assert!(left[0].contains("Line 1") && left[0].contains("no Q"));
        assert!(left[1].contains("XYZ"));
        assert!(left[2].contains("no frequency"));
        assert!(left[5].contains("“PA”") || left[5].contains("BW/60"), "{left:?}");
    }

    #[test]
    fn copied_text_reads_back_the_same() {
        let bands = vec![
            Band::new(Kind::LowShelf, 105.0, 6.0, 0.7),
            Band::new(Kind::Peak, 31.5, 5.4, 1.41),
            Band::with_width(Kind::Peak, 1234.56, -3.25, 0.5, Width::Octaves),
            Band::with_width(Kind::HighShelf, 8000.0, -4.0, 0.75, Width::Slope),
            Band::new(Kind::LowPass, 18_000.0, 0.0, std::f32::consts::FRAC_1_SQRT_2),
            Band::new(Kind::HighPass, 25.0, 0.0, 0.5),
            Band::new(Kind::BandPass, 900.0, 0.0, 2.0),
            Band::new(Kind::Notch, 50.0, 0.0, 30.0),
            Band::new(Kind::AllPass, 300.0, 0.0, 0.707),
        ];
        let text = write("Mine", &bands, Preamp::Auto, -6.123, true);
        let p = read_ok(&text);
        assert_eq!(p.format, Format::Marquee(1));
        assert_eq!(p.name.as_deref(), Some("Mine"));
        assert_eq!(p.preamp, Some(Preamp::Auto));
        assert_eq!(p.bands, bands, "{text}");
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert!(text.contains("Preamp: -6.12 dB\n"), "{text}");
        assert!(text.contains("Filter 1: ON LSC Fc 105 Hz Gain 6 dB Q 0.7\n"), "{text}");
        assert!(text.contains("Filter 3: ON PK Fc 1234.56 Hz Gain -3.25 dB BW Oct 0.5\n"), "{text}");
        assert!(text.contains("Filter 4: ON HSC 9 dB Fc 8000 Hz Gain -4 dB\n"), "{text}");
        assert!(text.contains("Filter 5: ON LP Fc 18000 Hz\n"), "{text}");
        assert!(text.contains("Filter 8: ON NO Fc 50 Hz Q 30\n"), "{text}");
        let text = write("x", &[Band::new(Kind::LowShelf, 105.0, 6.0, 0.7)], Preamp::Manual(-6.0), 0.0, true);
        assert!(text.contains("Preamp: -6 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6 dB Q 0.7"), "{text}");
        assert_eq!(read_ok(&text).preamp, Some(Preamp::Manual(-6.0)));
        // Off: every filter OFF and no preamp, and still read back by this app.
        let text = write("x", &bands[..2], Preamp::Auto, -6.0, false);
        assert!(text.contains("Preamp: 0 dB\nFilter 1: OFF LSC"), "{text}");
        let e = read(&text).unwrap_err();
        assert!(e.reason.contains("OFF"), "{}", e.reason);
        // An older build's export imports.
        let old = "# Marquee Player equalizer preset: Bass boost (mine)\n# Marquee Player: preamp auto\nPreamp: -4.9 dB\nFilter 1: ON PK Fc 31.5 Hz Gain 5.4 dB Q 1.41\n";
        let p = read_ok(old);
        assert_eq!((p.format, p.name.as_deref(), p.preamp), (Format::Marquee(1), Some("Bass boost (mine)"), Some(Preamp::Auto)));
        assert_eq!(name_from_file("Sennheiser HD 650 ParametricEQ"), "Sennheiser HD 650");
        assert_eq!(name_from_file("FiiO FT1 FixedBandEQ"), "FiiO FT1");
    }

    #[test]
    fn reads_room_eq_wizard_s_export() {
        // The layout of REW's help ("Export filter settings as text"), Generic equaliser
        // lines in Equalizer APO's reference form.
        let text = "Filter Settings file\n\nRoom EQ V5.31\nDated: 16-Nov-2026 10:00:00\n\nNotes:Desk speakers\n\nEqualiser: Generic\nleft 1m.mdat\nTarget level: 75.0dB\nFilter  1: ON  PK       Fc   63.70 Hz  Gain  -5.00 dB  Q  4.000\nFilter  2: ON  LS       Fc   200.0 Hz  Gain   3.00 dB  Q  0.707\nFilter  3: ON  None\nFilter  4: ON  None\n";
        let p = read_ok(text);
        assert_eq!(p.format, Format::Rew("Generic".into()));
        assert_eq!(p.bands.len(), 2);
        assert_eq!(p.bands[0], Band::new(Kind::Peak, 63.7, -5.0, 4.0));
        assert!(p.notes.iter().all(|n| n.kind == NoteKind::Info), "{:?}", p.notes);
    }

    #[test]
    fn hostile_text_is_bounded_and_refused_plainly() {
        let many: String = (0..5000).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1e308 dB Q -1e-308\n", 100 + i)).collect();
        let p = read(&many);
        // Read (each held within range or left out), never a panic.
        if let Ok(p) = p {
            assert!(p.bands.iter().all(|b| b.gain_db.abs() <= 24.0));
        }
        for junk in ["Filter: ON PK Fc 1e999 Hz Gain nan dB Q inf", "Filter: ON PK Fc -- Hz Gain .. dB Q ..", "Filter: ON", "Filter:", "GraphicEQ: 0 3; 100 2", "Preamp: 1e400 dB"] {
            let _ = read(junk);
        }
        assert!(read("GraphicEQ: -20 1; 30 2").is_err());
    }
}
