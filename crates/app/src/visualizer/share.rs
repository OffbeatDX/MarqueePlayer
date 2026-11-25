//! A visualizer preset as text, to send in a chat message or keep in a file: one line,
//! versioned, readable, every value by name.
//!
//!     MarqueeViz/1 name="Night halo" style=ring colours=cover by=frequency detail=medium
//!       motion=smooth sensitivity=+3 peaks=on mirror=off reflection=off
//!       reflection-height=45% reflection-strength=55% opacity=100% behind=85%
//!       custom=3c64ff,a050f0,ff6eaa
//!
//! (one line when written). Reading is forgiving where it can be and refuses plainly where
//! it cannot: the line may sit inside other text (a chat message: "try this: MarqueeViz/1
//! …"), keys come in any order, a missing key takes the default look's value, an unknown key
//! is left out and said, a number out of range is brought into it and said. Refused, with a
//! reason: no preset in the text; text far too long or not text; a newer format than this
//! build reads; a style, colour scheme, detail or speed this build does not have; a value
//! that is not a number or a colour. Nothing here panics on any input (the tests feed it
//! hostile text). Pure and unit-tested; `visualizer/mod.rs` copies, pastes, exports and imports.

use crate::visualizer::look::{self, ColourBy, Colours, Detail, Look, Motion, Style, ThemeViz};

/// What the line starts with, before its format number.
pub const MARK: &str = "MarqueeViz/";
/// The format this build writes and reads. A later build that only adds keys keeps it (this
/// build leaves those keys out and says so); one that changes what a key means raises it.
pub const VERSION: u32 = 1;
/// Longer text is refused before it is read (a preset is about 250 bytes).
pub const MAX_BYTES: usize = 16 * 1024;

/// A preset read from text.
#[derive(Clone, Debug, PartialEq)]
pub struct Shared {
    /// Its name as given (cleaned), if it had one.
    pub name: Option<String>,
    pub look: Look,
    /// What was left out or brought into range, in words.
    pub notes: Vec<String>,
}

/// Why text is not a preset this build can add.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// No preset in it at all (said as "no visualizer preset").
    NotAPreset,
    /// A preset, or something claiming to be one, that cannot be read: why.
    Unreadable(String),
}

fn style_name(s: Style) -> &'static str {
    match s {
        Style::Bars => "bars",
        Style::Line => "line",
        Style::Radial => "ring",
        Style::Bonfire => "bonfire",
        Style::Saturn => "saturn",
        Style::Eclipse => "eclipse",
    }
}

fn colours_name(c: Colours) -> &'static str {
    match c {
        Colours::Accent => "accent",
        Colours::Cover => "cover",
        Colours::Classic => "classic",
        Colours::Prism => "prism",
        Colours::Rainbow => "rainbow",
        Colours::Ember => "ember",
        Colours::Ice => "ice",
        Colours::Sunset => "sunset",
        Colours::Custom => "custom",
    }
}

fn detail_name(d: Detail) -> &'static str {
    match d {
        Detail::Fine => "fine",
        Detail::Medium => "medium",
        Detail::Coarse => "coarse",
    }
}

fn motion_name(m: Motion) -> &'static str {
    match m {
        Motion::Smooth => "smooth",
        Motion::Normal => "normal",
        Motion::Snappy => "snappy",
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn pct(v: f32) -> String {
    format!("{}%", (v * 100.0).round() as i32)
}

/// A name inside quotes: `"` and `\` escaped.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The preset as one line of text.
pub fn to_text(name: &str, look: &Look) -> String {
    let l = look.clone().clamped();
    let custom: Vec<String> = l.custom.iter().map(|c| c.trim().trim_start_matches('#').to_lowercase()).collect();
    format!(
        "{MARK}{VERSION} name={} style={} colours={} by={} detail={} motion={} sensitivity={} intensity={} cover-pulse={} peaks={} mirror={} reflection={} reflection-height={} reflection-strength={} opacity={} behind={} custom={}",
        quote(&look::clean_name(name)),
        style_name(l.style),
        colours_name(l.colours),
        if l.colour_by == ColourBy::Frequency { "frequency" } else { "height" },
        detail_name(l.detail),
        motion_name(l.motion),
        if l.sensitivity == 0 { "0".to_owned() } else { format!("{:+}", l.sensitivity) },
        pct(l.intensity),
        on(l.cover_pulse),
        on(l.peaks),
        on(l.mirror),
        on(l.reflection),
        pct(l.reflection_size),
        pct(l.reflection_strength),
        pct(l.opacity),
        pct(l.prominence),
        custom.join(","),
    )
}

/// A file of one preset: a comment saying what it is, then the line.
pub fn to_file(name: &str, look: &Look) -> String {
    format!("# Marquee Player visualizer preset. Import it in Settings > Visualizer, or paste this line there.\n{}\n", to_text(name, look))
}

/// The tokens of a line: `key=value`, the value quoted or not.
fn tokens(line: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut it = line.chars().peekable();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        if it.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = it.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            it.next();
        }
        if it.peek() != Some(&'=') {
            // A word without a value (a chat's own words after the preset): the line ends.
            break;
        }
        it.next();
        let mut value = String::new();
        match it.peek() {
            Some(&q @ ('"' | '\u{201c}' | '\u{201d}')) => {
                it.next();
                let close = if q == '"' { '"' } else { '\u{201d}' };
                let mut closed = false;
                while let Some(c) = it.next() {
                    if c == '\\' && close == '"' {
                        if let Some(n) = it.next() {
                            value.push(n);
                        }
                    } else if c == close || (close == '\u{201d}' && c == '"') {
                        closed = true;
                        break;
                    } else {
                        value.push(c);
                    }
                }
                if !closed {
                    return Err(format!("the {key} has no closing quote"));
                }
            }
            _ => {
                while let Some(&c) = it.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    value.push(c);
                    it.next();
                }
            }
        }
        out.push((key.to_ascii_lowercase(), value));
    }
    Ok(out)
}

/// A number as written: "45", "45%", "+3", "-3 dB" (the unit dropped), or why not.
fn number(key: &str, v: &str) -> Result<f64, String> {
    let t = v.trim().trim_end_matches('%').trim_end_matches("dB").trim_end_matches("db").trim();
    if t.len() > 32 {
        return Err(format!("{key} is not a number"));
    }
    match t.parse::<f64>() {
        Ok(x) if x.is_finite() => Ok(x),
        _ => Err(format!("{key} is not a number (\u{201c}{}\u{201d})", short(v))),
    }
}

fn short(v: &str) -> String {
    let s: String = v.chars().filter(|c| !c.is_control()).take(24).collect();
    if v.chars().count() > 24 { format!("{s}\u{2026}") } else { s }
}

fn switch(key: &str, v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "on" | "yes" | "true" | "1" => Ok(true),
        "off" | "no" | "false" | "0" => Ok(false),
        _ => Err(format!("{key} must be on or off, not \u{201c}{}\u{201d}", short(v))),
    }
}

/// A percentage brought into `lo..=hi` (as fractions), saying so when it had to be.
fn fraction(key: &str, v: &str, lo: f32, hi: f32, notes: &mut Vec<String>) -> Result<f32, String> {
    let x = number(key, v)?;
    let f = (x / 100.0) as f32;
    let c = if f.is_finite() { f.clamp(lo, hi) } else { hi };
    if (f64::from(c) - x / 100.0).abs() > 0.0005 {
        notes.push(format!("{key} {}% is out of range: {}% instead", short(&format_num(x)), (c * 100.0).round()));
    }
    Ok(c)
}

fn format_num(x: f64) -> String {
    if x.abs() >= 1e6 { format!("{x:e}") } else { format!("{}", (x * 10.0).round() / 10.0) }
}

/// Read a preset from text (the clipboard, a file, a chat message).
pub fn parse(text: &str) -> Result<Shared, Refusal> {
    if text.len() > MAX_BYTES {
        return Err(Refusal::Unreadable(format!("it is too long to be a visualizer preset ({} KB)", text.len() / 1024)));
    }
    if text.contains('\0') {
        return Err(Refusal::Unreadable("it is not text".to_owned()));
    }
    let lower = text.to_ascii_lowercase();
    let Some(at) = lower.find(&MARK.to_ascii_lowercase()) else {
        return Err(Refusal::NotAPreset);
    };
    let rest = &text[at + MARK.len()..];
    let line = rest.lines().next().unwrap_or("");
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return Err(Refusal::Unreadable("it has no format number after \u{201c}MarqueeViz/\u{201d}".to_owned()));
    }
    let version: u32 = digits.parse().unwrap_or(u32::MAX);
    if version > VERSION {
        return Err(Refusal::Unreadable(format!("it is from a newer version of Marquee Player (format {}); this version reads format {VERSION}", if version == u32::MAX { digits.chars().take(12).collect() } else { version.to_string() })));
    }
    if version == 0 {
        return Err(Refusal::Unreadable("its format number is 0, which no version wrote".to_owned()));
    }
    let body = &line[digits.len()..];
    if !body.is_empty() && !body.starts_with(char::is_whitespace) {
        return Err(Refusal::Unreadable("its format number is not followed by a space".to_owned()));
    }
    if body.chars().any(|c| c.is_control() && c != '\t' && c != '\r') {
        return Err(Refusal::Unreadable("it is not text".to_owned()));
    }
    let toks = tokens(body).map_err(Refusal::Unreadable)?;
    let mut look = Look::classic(&ThemeViz::default());
    let mut name = None;
    let mut notes = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for (k, v) in toks {
        if seen.contains(&k) {
            notes.push(format!("{} was given twice: the first is used", short(&k)));
            continue;
        }
        seen.push(k.clone());
        let bad = |what: &str| Refusal::Unreadable(format!("it uses the {what} \u{201c}{}\u{201d}, which this version does not have", short(&v)));
        match k.as_str() {
            "name" => {
                let n = look::clean_name(&v);
                if v.chars().count() > look::NAME_MAX + 8 {
                    notes.push(format!("its name was cut to {} characters", look::NAME_MAX));
                }
                name = (!n.is_empty()).then_some(n);
            }
            "style" => {
                look.style = match v.to_ascii_lowercase().as_str() {
                    "bars" => Style::Bars,
                    "line" => Style::Line,
                    // Shockwave and Fireworks were taken out: a preset shared before reads
                    // as its nearest relative.
                    "ring" | "radial" | "halo" | "shockwave" => Style::Radial,
                    "bonfire" | "embers" | "sparks" | "fireworks" => Style::Bonfire,
                    "saturn" => Style::Saturn,
                    // Galaxy and Swarm were taken out on 2026-11-25: a preset shared before
                    // shows as Saturn, dust round the cover too, and says so.
                    old @ ("galaxy" | "swarm") => {
                        let was = if old == "galaxy" { "Galaxy" } else { "Swarm" };
                        notes.push(format!("its look, {was}, is no longer in this version: Saturn is shown instead"));
                        Style::Saturn
                    }
                    "eclipse" => Style::Eclipse,
                    _ => return Err(bad("style")),
                }
            }
            "colours" | "colors" => {
                look.colours = Colours::ALL.into_iter().find(|c| colours_name(*c) == v.to_ascii_lowercase()).ok_or_else(|| bad("colour scheme"))?;
            }
            "by" => {
                look.colour_by = match v.to_ascii_lowercase().as_str() {
                    "height" => ColourBy::Height,
                    "frequency" | "across" => ColourBy::Frequency,
                    _ => return Err(bad("way of colouring")),
                }
            }
            "detail" => look.detail = Detail::ALL.into_iter().find(|d| detail_name(*d) == v.to_ascii_lowercase()).ok_or_else(|| bad("detail"))?,
            "motion" | "speed" => look.motion = Motion::ALL.into_iter().find(|m| motion_name(*m) == v.to_ascii_lowercase()).ok_or_else(|| bad("speed"))?,
            "sensitivity" => {
                let x = number("sensitivity", &v).map_err(Refusal::Unreadable)?;
                let c = x.round().clamp(-12.0, 12.0) as i32;
                if (f64::from(c) - x).abs() > 0.01 {
                    notes.push(format!("sensitivity {} dB is out of range: {c:+} dB instead", format_num(x)));
                }
                look.sensitivity = c;
            }
            "intensity" => look.intensity = fraction("intensity", &v, 0.5, 2.0, &mut notes).map_err(Refusal::Unreadable)?,
            "cover-pulse" => look.cover_pulse = switch("cover-pulse", &v).map_err(Refusal::Unreadable)?,
            "peaks" => look.peaks = switch("peaks", &v).map_err(Refusal::Unreadable)?,
            "mirror" => look.mirror = switch("mirror", &v).map_err(Refusal::Unreadable)?,
            "reflection" => look.reflection = switch("reflection", &v).map_err(Refusal::Unreadable)?,
            "reflection-height" => look.reflection_size = fraction("reflection-height", &v, 0.1, 1.0, &mut notes).map_err(Refusal::Unreadable)?,
            "reflection-strength" => look.reflection_strength = fraction("reflection-strength", &v, 0.05, 1.0, &mut notes).map_err(Refusal::Unreadable)?,
            "opacity" => look.opacity = fraction("opacity", &v, 0.1, 1.0, &mut notes).map_err(Refusal::Unreadable)?,
            "behind" => look.prominence = fraction("behind", &v, 0.1, 1.0, &mut notes).map_err(Refusal::Unreadable)?,
            "custom" => {
                let parts: Vec<&str> = v.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
                if parts.is_empty() || parts.len() > 3 {
                    return Err(Refusal::Unreadable("custom must be one to three colours, like 3c64ff,a050f0,ff6eaa".to_owned()));
                }
                let mut cs = Vec::new();
                for p in parts {
                    let Some(rgb) = look::parse_hex(p) else {
                        return Err(Refusal::Unreadable(format!("custom has \u{201c}{}\u{201d}, which is not a colour like 3c64ff", short(p))));
                    };
                    cs.push(look::hex(rgb));
                }
                look.custom = cs;
            }
            other => {
                if unknown.len() < 5 {
                    unknown.push(short(other));
                }
            }
        }
    }
    if !unknown.is_empty() {
        notes.push(format!("left out what this version does not know: {}", unknown.join(", ")));
    }
    let look = look.clamped();
    Ok(Shared { name, look, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualizer::look::Preset;

    fn halo() -> Look {
        Look {
            style: Style::Radial,
            colours: Colours::Custom,
            colour_by: ColourBy::Frequency,
            custom: vec!["#3c64ff".into(), "#a050f0".into(), "#ff6eaa".into()],
            motion: Motion::Smooth,
            sensitivity: 3,
            opacity: 0.7,
            prominence: 0.85,
            ..Look::default()
        }
    }

    #[test]
    fn every_look_round_trips() {
        let t = ThemeViz::default();
        let mut looks: Vec<Look> = Preset::ALL.iter().map(|p| p.look(&t)).collect();
        looks.push(halo());
        looks.push(Look { reflection: false, mirror: true, peaks: false, detail: Detail::Fine, sensitivity: -12, reflection_size: 0.1, reflection_strength: 0.05, opacity: 0.1, prominence: 1.0, intensity: 1.5, ..halo() });
        for (i, l) in looks.iter().enumerate() {
            let name = format!("Look {i} \u{201c}quoted\u{201d} \"and\" back\\slash");
            let text = to_text(&name, l);
            assert!(text.starts_with("MarqueeViz/1 name="), "{text}");
            assert!(!text.contains('\n') && text.len() < 400, "one line, short: {text}");
            let back = parse(&text).unwrap_or_else(|e| panic!("{e:?} for {text}"));
            assert!(back.look.same(l), "{text}\n{:?}\n{:?}", back.look, l);
            assert_eq!(back.name.as_deref(), Some(name.as_str()));
            assert!(back.notes.is_empty(), "{:?}", back.notes);
            // As a file, and inside a chat message.
            assert!(parse(&to_file(&name, l)).unwrap().look.same(l));
            let chat = format!("hey, try this one:\n> {text}   it looks great with jazz");
            assert!(parse(&chat).unwrap().look.same(l), "{chat}");
        }
    }

    #[test]
    fn the_example_reads() {
        let t = "MarqueeViz/1 name=\"Night halo\" style=ring colours=cover by=frequency detail=medium motion=smooth sensitivity=+3 peaks=on mirror=off reflection=off reflection-height=45% reflection-strength=55% opacity=100% behind=85% custom=3c64ff,a050f0,ff6eaa";
        let s = parse(t).unwrap();
        assert_eq!(s.name.as_deref(), Some("Night halo"));
        assert_eq!((s.look.style, s.look.colours, s.look.colour_by, s.look.sensitivity), (Style::Radial, Colours::Cover, ColourBy::Frequency, 3));
        assert!((s.look.prominence - 0.85).abs() < 1e-6 && (s.look.opacity - 1.0).abs() < 1e-6);
        // Lenient forms: any case, any order, smart quotes, a missing value taking the default's.
        let s = parse("marqueeviz/1 STYLE=Bars name=\u{201c}Mine\u{201d}").unwrap();
        assert_eq!((s.name.as_deref(), s.look.style), (Some("Mine"), Style::Bars));
        assert_eq!(s.look.colours, Look::classic(&ThemeViz::default()).colours);
        let s = parse("MarqueeViz/1").unwrap();
        assert_eq!(s.name, None);
    }

    #[test]
    fn a_look_taken_out_shows_as_saturn_and_says_so() {
        for (old, was) in [("galaxy", "Galaxy"), ("Swarm", "Swarm")] {
            let text = format!("MarqueeViz/1 name=\"Old dust\" style={old} colours=ice by=frequency motion=snappy sensitivity=-2 opacity=60% behind=70% cover-pulse=off");
            let s = parse(&text).unwrap_or_else(|e| panic!("{e:?} for {text}"));
            assert_eq!(s.name.as_deref(), Some("Old dust"));
            assert_eq!((s.look.style, s.look.colours, s.look.colour_by, s.look.motion, s.look.sensitivity, s.look.cover_pulse), (Style::Saturn, Colours::Ice, ColourBy::Frequency, Motion::Snappy, -2, false));
            assert!((s.look.opacity - 0.6).abs() < 1e-6 && (s.look.prominence - 0.7).abs() < 1e-6);
            assert_eq!(s.notes.len(), 1, "{:?}", s.notes);
            assert!(s.notes[0].contains(was) && s.notes[0].contains("no longer") && s.notes[0].contains("Saturn"), "{:?}", s.notes);
            // As a file, too; and written out again it names Saturn.
            assert_eq!(parse(&to_file("Old dust", &s.look)).unwrap().look.style, Style::Saturn);
            assert!(to_text("Old dust", &s.look).contains("style=saturn"));
        }
    }

    #[test]
    fn what_is_not_a_preset_says_so() {
        for t in ["", "   ", "hello there", "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 2 dB Q 1", "{\"preset\":\"halo\"}", "MarqueeVi/1 style=ring"] {
            assert_eq!(parse(t), Err(Refusal::NotAPreset), "{t:?}");
        }
    }

    fn refused(t: &str) -> String {
        match parse(t) {
            Err(Refusal::Unreadable(r)) => r,
            other => panic!("{t:?} should be refused: {other:?}"),
        }
    }

    #[test]
    fn malformed_presets_are_refused_with_a_reason() {
        assert!(refused("MarqueeViz/ style=ring").contains("no format number"));
        assert!(refused("MarqueeViz/x style=ring").contains("no format number"));
        assert!(refused("MarqueeViz/0 style=ring").contains("0"));
        assert!(refused("MarqueeViz/1style=ring").contains("not followed by a space"));
        assert!(refused("MarqueeViz/1 style=plasma").contains("\u{201c}plasma\u{201d}"));
        assert!(refused("MarqueeViz/1 colours=aurora").contains("colour scheme"));
        assert!(refused("MarqueeViz/1 detail=ultra").contains("detail"));
        assert!(refused("MarqueeViz/1 motion=warp").contains("speed"));
        assert!(refused("MarqueeViz/1 by=mood").contains("colouring"));
        assert!(refused("MarqueeViz/1 opacity=lots").contains("not a number"));
        assert!(refused("MarqueeViz/1 opacity=NaN").contains("not a number"));
        assert!(refused("MarqueeViz/1 opacity=inf").contains("not a number"));
        assert!(refused("MarqueeViz/1 sensitivity=1e999").contains("not a number"));
        assert!(refused("MarqueeViz/1 peaks=maybe").contains("on or off"));
        assert!(refused("MarqueeViz/1 custom=zzzzzz").contains("not a colour"));
        assert!(refused("MarqueeViz/1 custom=").contains("one to three colours"));
        assert!(refused("MarqueeViz/1 custom=111111,222222,333333,444444").contains("one to three"));
        assert!(refused("MarqueeViz/1 name=\"never closed").contains("closing quote"));
        assert!(refused("MarqueeViz/1 name=a\u{1}b").contains("not text"));
    }

    #[test]
    fn newer_formats_are_refused() {
        let r = refused("MarqueeViz/2 style=ring name=\"From the future\"");
        assert!(r.contains("newer version") && r.contains("format 2") && r.contains("format 1"), "{r}");
        let r = refused("MarqueeViz/99999999999999999999 style=ring");
        assert!(r.contains("newer version"), "{r}");
    }

    #[test]
    fn hostile_input_never_panics() {
        assert!(refused(&"x".repeat(MAX_BYTES + 1)).contains("too long"));
        assert!(refused("MarqueeViz/1 \0\0\0").contains("not text"));
        assert_eq!(parse(&String::from_utf8_lossy(&[0xff, 0xfe, 0x00, 0x41])).map(|_| ()), Err(Refusal::Unreadable("it is not text".into())));
        // Absurd numbers are brought into range and said.
        let s = parse("MarqueeViz/1 opacity=500% behind=-40 sensitivity=99 reflection-height=1e300 reflection-strength=0").unwrap();
        assert_eq!((s.look.opacity, s.look.prominence, s.look.sensitivity), (1.0, 0.1, 12));
        assert!((s.look.reflection_size - 1.0).abs() < 1e-6 && (s.look.reflection_strength - 0.05).abs() < 1e-6);
        assert_eq!(s.notes.len(), 5, "{:?}", s.notes);
        // Unknown keys are left out and said; a key given twice keeps the first.
        let s = parse("MarqueeViz/1 style=line sparkle=on twinkle=7 style=ring").unwrap();
        assert_eq!(s.look.style, Style::Line);
        assert!(s.notes.iter().any(|n| n.contains("sparkle") && n.contains("twinkle")) && s.notes.iter().any(|n| n.contains("twice")), "{:?}", s.notes);
        // A very long name is cut, control characters go.
        let s = parse(&format!("MarqueeViz/1 name=\"{}\"", "N".repeat(300))).unwrap();
        assert_eq!(s.name.as_ref().map(|n| n.chars().count()), Some(look::NAME_MAX));
        // Random bytes after the mark, many times: never a panic.
        let mut seed = 0x9e37_79b9u32;
        for _ in 0..3000 {
            let n = (seed % 200) as usize;
            let mut t = String::from("MarqueeViz/1 ");
            for _ in 0..n {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let c = char::from_u32(seed % 0x3000).unwrap_or('?');
                t.push(if seed.is_multiple_of(7) { '=' } else if seed.is_multiple_of(11) { '"' } else { c });
            }
            let _ = parse(&t);
        }
    }
}
