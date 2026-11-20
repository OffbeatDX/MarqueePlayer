//! Lyrics from local sources only: a `.lrc` file beside the track with the same name,
//! else a `.txt` file of that name (what "Save as .txt" writes for plain lyrics), else the
//! lyrics in its tags (Vorbis LYRICS / UNSYNCEDLYRICS, ID3 USLT, MP4 ©lyr).
//! Read when asked (the now playing page), never stored in the database, never looked
//! up online. Also the other way: lyrics as LRC or plain text (`to_lrc`, `to_text`), with the
//! user's timing offset applied (`shifted`), written beside a track, never over a file
//! already there unless asked (`write_sidecar`).
//!
//! LRC: `[mm:ss.xx]` (also `[mm:ss]`, `[mm:ss:xx]`, `[h:mm:ss.xx]`), several stamps on one
//! line, `[offset:±ms]`, metadata tags (`[ar:...]`) ignored, and the enhanced forms'
//! word stamps (`<mm:ss.xx>`) and voice tags (`v1:`) dropped from the text. Text without
//! any stamp is unsynced lyrics, shown as it is.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// When it is sung (ms from the start), for synced lyrics.
    pub time_ms: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub synced: bool,
    pub lines: Vec<Line>,
    /// Where they came from: "lrc", "txt" or "tags" (the app's own: "lrclib", "server").
    pub source: &'static str,
}

/// `[mm:ss.xx]` and its variants, in ms.
fn stamp(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    let num = |x: &str| x.trim().parse::<u64>().ok();
    let frac_ms = |x: &str| -> Option<u64> {
        let (sec, frac) = x.split_once('.').unwrap_or((x, ""));
        let sec = num(sec)?;
        let frac = frac.trim();
        let ms = if frac.is_empty() {
            0
        } else {
            let digits: String = frac.chars().take(3).collect();
            num(&digits)? * 10u64.pow(3 - digits.len() as u32)
        };
        sec.checked_mul(1000)?.checked_add(ms)
    };
    // `n` of `unit` ms; `None` past what a time holds (untrusted text: not a time then).
    let of = |n: u64, unit: u64| n.checked_mul(unit);
    match parts.as_slice() {
        [m, s] => of(num(m)?, 60_000)?.checked_add(frac_ms(s)?),
        // mm:ss:xx (centiseconds after a colon), or h:mm:ss.xx
        [a, b, c] if c.contains('.') => of(num(a)?, 3_600_000)?.checked_add(of(num(b)?, 60_000)?)?.checked_add(frac_ms(c)?),
        [m, s, cs] => of(num(m)?, 60_000)?.checked_add(of(num(s)?, 1000)?)?.checked_add(of(num(cs)?, 10)?),
        _ => None,
    }
}

/// The most a file's `[offset:]` moves its lyrics either way, in ms: a day.
const FILE_OFFSET_MAX_MS: i64 = 86_400_000;

/// Lyrics from text: LRC when it has time stamps, plain lines otherwise.
pub fn parse(text: &str, source: &'static str) -> Lyrics {
    let mut timed: Vec<(u64, String)> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    let mut offset: i64 = 0;
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        let mut tag_line = false;
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            let body = &inner[..end];
            if let Some(t) = stamp(body) {
                times.push(t);
            } else if let Some((k, v)) = body.split_once(':') {
                if k.trim().eq_ignore_ascii_case("offset") {
                    offset = v.trim().parse::<i64>().unwrap_or(0).clamp(-FILE_OFFSET_MAX_MS, FILE_OFFSET_MAX_MS);
                }
                tag_line = true;
            } else {
                break;
            }
            rest = inner[end + 1..].trim_start();
        }
        if times.is_empty() {
            if !tag_line {
                plain.push(rest.to_owned());
            }
            continue;
        }
        let text = clean(rest);
        for t in times {
            timed.push((t, text.clone()));
        }
    }
    if timed.is_empty() {
        // Unsynced: as written, without leading or trailing blank lines.
        while plain.first().is_some_and(|l| l.trim().is_empty()) {
            plain.remove(0);
        }
        while plain.last().is_some_and(|l| l.trim().is_empty()) {
            plain.pop();
        }
        return Lyrics { synced: false, lines: plain.into_iter().map(|text| Line { time_ms: None, text }).collect(), source };
    }
    timed.sort_by_key(|(t, _)| *t);
    // A positive offset shows the lyrics sooner (the LRC convention).
    let lines = timed.into_iter().map(|(t, text)| Line { time_ms: Some(i64::try_from(t).unwrap_or(i64::MAX).saturating_sub(offset).max(0) as u64), text }).collect();
    Lyrics { synced: true, lines, source }
}

/// The enhanced LRC's word stamps and voice tags out of a line.
fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) if stamp(&rest[i + 1..i + j]).is_some() => rest = &rest[i + j + 1..],
            _ => {
                out.push('<');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // "v1: words" (voice / singer tags of the A2 extension).
    match out.split_once(':') {
        Some((v, t)) if v.len() <= 3 && v.starts_with('v') && v[1..].chars().all(|c| c.is_ascii_digit()) => t.trim().to_owned(),
        _ => out,
    }
}

/// The line being sung at `ms` (synced lyrics): the last one that has started.
pub fn current(lines: &[Line], ms: u64) -> Option<usize> {
    current_shifted(lines, ms, 0)
}

// ---- The user's timing offset ------------------------------------------------------------

/// One step of the Lyrics tab's earlier / later buttons (and their keys).
pub const OFFSET_STEP_MS: i64 = 100;
/// How far the lyrics can be moved either way.
pub const OFFSET_MAX_MS: i64 = 60_000;

/// When a line shows with the lyrics moved by `offset_ms` (positive: later), never before
/// the start.
pub fn shifted(time_ms: u64, offset_ms: i64) -> u64 {
    i64::try_from(time_ms).unwrap_or(i64::MAX).saturating_add(offset_ms).max(0) as u64
}

/// The line being sung at `ms` with the lyrics moved by `offset_ms`: the last one that has
/// started (lines moved before the start have all started at 0).
pub fn current_shifted(lines: &[Line], ms: u64, offset_ms: i64) -> Option<usize> {
    let n = lines.partition_point(|l| l.time_ms.is_some_and(|t| shifted(t, offset_ms) <= ms));
    n.checked_sub(1)
}

/// An offset moved by `by`, within `OFFSET_MAX_MS` either way.
pub fn nudge(offset_ms: i64, by: i64) -> i64 {
    offset_ms.saturating_add(by).clamp(-OFFSET_MAX_MS, OFFSET_MAX_MS)
}

/// An offset as the Lyrics tab shows it: "+0.3 s", "−1.2 s" (a minus sign), "0.0 s".
pub fn offset_text(offset_ms: i64) -> String {
    let sign = match offset_ms.signum() {
        1 => "+",
        -1 => "\u{2212}",
        _ => "",
    };
    format!("{sign}{:.1} s", offset_ms.unsigned_abs() as f64 / 1000.0)
}

// ---- Writing lyrics ------------------------------------------------------------------------

/// Synced lines as LRC, `offset_ms` applied to every time stamp (clamped at 0) so that other
/// players show them as this one does: `tags` first (`[ti:...]`; empty ones left out), then
/// `[mm:ss.xx]` and the text for each line, in centiseconds (rounded), the form every LRC
/// reader takes. No `[offset:]` tag: the offset is in the stamps. Lines without a time are
/// left out.
pub fn to_lrc(lines: &[Line], offset_ms: i64, tags: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (k, v) in tags {
        let v: String = v.chars().map(|c| if matches!(c, '\n' | '\r' | ']') { ' ' } else { c }).collect();
        if !v.trim().is_empty() {
            out.push_str(&format!("[{k}:{}]\n", v.trim()));
        }
    }
    for l in lines {
        let Some(t) = l.time_ms else { continue };
        let cs = shifted(t, offset_ms).saturating_add(5) / 10;
        out.push_str(&format!("[{:02}:{:02}.{:02}]{}\n", cs / 6000, cs / 100 % 60, cs % 100, l.text));
    }
    out
}

/// Lines as plain text, one a line (plain lyrics, or "Copy lyrics").
pub fn to_text(lines: &[Line]) -> String {
    let mut out = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    out.push('\n');
    out
}

/// The file beside `track` that its lyrics are saved to, and read from first: `<name>.lrc`
/// for synced lyrics, `<name>.txt` for plain ones (`load` reads the `.lrc`, then the `.txt`).
pub fn sidecar(track: &Path, synced: bool) -> std::path::PathBuf {
    track.with_extension(if synced { "lrc" } else { "txt" })
}

/// Why lyrics were not written beside a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// A file of that name is there already, and replacing it was not asked for.
    Exists,
    /// Not a place to write: a server's track, or a path the write guard refuses (tests and
    /// the bench).
    Refused(String),
    /// The system's error (a read-only folder, a full disk, a missing drive).
    Io(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Exists => f.write_str("a file of that name is there already"),
            SaveError::Refused(why) | SaveError::Io(why) => f.write_str(why),
        }
    }
}

/// Write `text` (UTF-8, no BOM) to `path`, a track's sidecar. A file already there is
/// replaced only with `replace` (the new one written beside it, then moved over it, so it is
/// never left half written); without, the write is refused (`SaveError::Exists`) and the
/// file left as it is.
pub fn write_sidecar(path: &Path, text: &str, replace: bool) -> Result<(), SaveError> {
    use std::io::Write;
    if crate::remote::is_remote(&path.to_string_lossy()) {
        return Err(SaveError::Refused("the track plays from the server: there is no folder to save beside it".into()));
    }
    if !crate::edit::atomic::allowed(path) {
        return Err(SaveError::Refused(format!("refused by the write guard: {}", path.display())));
    }
    let io = |e: std::io::Error| SaveError::Io(e.to_string());
    if !replace {
        let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(SaveError::Exists),
            Err(e) => return Err(io(e)),
        };
        if let Err(e) = f.write_all(text.as_bytes()).and_then(|()| f.sync_all()) {
            drop(f);
            let _ = std::fs::remove_file(path);
            return Err(io(e));
        }
        return Ok(());
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".mp-lyrics.tmp");
    let tmp = path.with_file_name(name);
    let written = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()));
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

/// The lyrics of `track`, from a `.lrc` beside it, else a `.txt`, else its tags. None for a
/// server's own track (`remote::is_remote`: no file beside it, no tags to read).
pub fn load(track: &Path) -> Option<Lyrics> {
    if crate::remote::is_remote(&track.to_string_lossy()) {
        return None;
    }
    for (ext, source) in [("lrc", "lrc"), ("txt", "txt")] {
        if let Ok(bytes) = std::fs::read(track.with_extension(ext)) {
            let l = parse(&crate::m3u::decode(&bytes), source);
            if !l.lines.is_empty() {
                return Some(l);
            }
        }
    }
    let options = ParseOptions::new().read_cover_art(false);
    let file = Probe::open(track).and_then(|p| p.options(options).read()).ok()?;
    for tag in file.tags() {
        for key in [ItemKey::Lyrics, ItemKey::UnsyncLyrics] {
            if let Some(text) = tag.get_string(key).filter(|t| !t.trim().is_empty()) {
                let l = parse(text, "tags");
                if !l.lines.is_empty() {
                    return Some(l);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(l: &Lyrics) -> Vec<(Option<u64>, &str)> {
        l.lines.iter().map(|l| (l.time_ms, l.text.as_str())).collect()
    }

    #[test]
    fn parses_lrc_with_its_variants() {
        let l = parse("[ar:Someone]\n[ti:Song]\n[00:12.34]First line\n[00:05]Intro\n[01:02:50]Colon cs\n[00:20.00][00:40.5]Chorus\n\n[1:00:00.00]An hour in\n", "lrc");
        assert!(l.synced);
        assert_eq!(
            times(&l),
            [
                (Some(5_000), "Intro"),
                (Some(12_340), "First line"),
                (Some(20_000), "Chorus"),
                (Some(40_500), "Chorus"),
                (Some(62_500), "Colon cs"),
                (Some(3_600_000), "An hour in")
            ]
        );
    }

    #[test]
    fn applies_the_offset_and_strips_enhanced_stamps() {
        let l = parse("[offset:+500]\n[00:10.00]v1: <00:10.00>Hello <00:10.50>there <00:11.00>world\n[00:12.00]a <b> c\n", "lrc");
        assert_eq!(times(&l), [(Some(9_500), "Hello there world"), (Some(11_500), "a <b> c")]);
        let l = parse("[offset:-250]\n[00:01.00]x", "lrc");
        assert_eq!(l.lines[0].time_ms, Some(1_250));
    }

    /// Lyrics come from files, tags and a server: numbers too large are not a time (and an
    /// offset is a day either way at most), never a time wrapped round, or a panic.
    #[test]
    fn stamps_and_offsets_too_large_are_not_taken() {
        let l = parse("[999999999999999:00]x\n[1:999999999999999999.5]x\n[99999999999999:00:00.0]x\n[00:01.00]y", "lrc");
        assert_eq!(times(&l), [(Some(1_000), "y")]);
        let l = parse("[offset:-9223372036854775808]\n[00:01.00]x", "lrc");
        assert_eq!(l.lines[0].time_ms, Some(1_000 + 86_400_000));
        let l = parse("[offset:9223372036854775807]\n[00:01.00]x", "lrc");
        assert_eq!(l.lines[0].time_ms, Some(0));
    }

    #[test]
    fn plain_text_is_unsynced() {
        let l = parse("\n\nVerse one\nstill one\n\nVerse two\n\n", "tags");
        assert!(!l.synced);
        assert_eq!(times(&l), [(None, "Verse one"), (None, "still one"), (None, ""), (None, "Verse two")]);
    }

    #[test]
    fn the_current_line_is_the_last_one_started() {
        let l = parse("[00:01.00]a\n[00:05.00]b\n[00:09.00]c", "lrc");
        assert_eq!(current(&l.lines, 0), None);
        assert_eq!(current(&l.lines, 1_000), Some(0));
        assert_eq!(current(&l.lines, 8_999), Some(1));
        assert_eq!(current(&l.lines, 60_000), Some(2));
    }

    #[test]
    fn the_offset_moves_every_line_and_stops_at_the_start() {
        assert_eq!(shifted(5_000, 300), 5_300);
        assert_eq!(shifted(5_000, -300), 4_700);
        assert_eq!(shifted(200, -300), 0, "never before the start");
        assert_eq!(shifted(u64::MAX, 1), i64::MAX as u64, "no overflow");
        let l = parse("[00:01.00]a\n[00:05.00]b\n[00:09.00]c", "lrc");
        // Later by 0.5 s: "b" starts at 5.5 s.
        assert_eq!(current_shifted(&l.lines, 5_400, 500), Some(0));
        assert_eq!(current_shifted(&l.lines, 5_500, 500), Some(1));
        // Earlier by 2 s: "a" (moved before the start) has started at 0, "b" at 3 s.
        assert_eq!(current_shifted(&l.lines, 0, -2_000), Some(0));
        assert_eq!(current_shifted(&l.lines, 3_000, -2_000), Some(1));
        assert_eq!(current_shifted(&l.lines, 0, 0), None);
        assert_eq!(current_shifted(&l.lines, 9_000, 0), current(&l.lines, 9_000));
    }

    #[test]
    fn offsets_step_within_bounds_and_read_as_seconds() {
        assert_eq!(nudge(0, OFFSET_STEP_MS), 100);
        assert_eq!(nudge(100, -3 * OFFSET_STEP_MS), -200);
        assert_eq!(nudge(OFFSET_MAX_MS - 50, OFFSET_STEP_MS), OFFSET_MAX_MS);
        assert_eq!(nudge(-OFFSET_MAX_MS, -OFFSET_STEP_MS), -OFFSET_MAX_MS);
        assert_eq!(nudge(i64::MAX, 1), OFFSET_MAX_MS);
        assert_eq!(offset_text(300), "+0.3 s");
        assert_eq!(offset_text(-1_200), "\u{2212}1.2 s");
        assert_eq!(offset_text(0), "0.0 s");
        assert_eq!(offset_text(12_000), "+12.0 s");
    }

    #[test]
    fn lrc_written_reads_back_with_the_offset_in_its_stamps() {
        let l = parse("[offset:+250]\n[00:00.10]Intro\n[00:12.34]First\n[00:12.34]\n[01:02.50]Colon\n[61:00.00]An hour in\n", "lrc");
        let text = to_lrc(&l.lines, 0, &[("ti", "Song"), ("ar", "Some]one\nElse"), ("al", " ")]);
        assert!(text.starts_with("[ti:Song]\n[ar:Some one Else]\n[00:"), "{text}");
        assert!(!text.contains("[al:"), "an empty tag is left out");
        assert!(!text.contains("offset"), "the source's offset is in the stamps");
        assert_eq!(parse(&text, "lrc").lines, l.lines, "the app's own parser reads back what was written");
        // Later by 0.3 s, then earlier by 0.3 s: the intro (at 0 after its offset) stays at 0.
        let later = parse(&to_lrc(&l.lines, 300, &[]), "lrc");
        assert_eq!(later.lines.iter().map(|x| x.time_ms).collect::<Vec<_>>(), l.lines.iter().map(|x| x.time_ms.map(|t| t + 300)).collect::<Vec<_>>());
        let earlier = parse(&to_lrc(&l.lines, -300, &[]), "lrc");
        assert_eq!(earlier.lines.iter().map(|x| x.time_ms).collect::<Vec<_>>(), [Some(0), Some(11_790), Some(11_790), Some(61_950), Some(3_659_450)]);
        assert!(to_lrc(&l.lines, -300, &[]).starts_with("[00:00.00]Intro\n"), "clamped at 0, not negative");
        assert_eq!(earlier.lines.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Intro", "First", "", "Colon", "An hour in"]);
        // Milliseconds round to the nearest centisecond.
        let odd = [Line { time_ms: Some(1_234), text: "x".into() }, Line { time_ms: Some(1_235), text: "y".into() }];
        assert_eq!(to_lrc(&odd, 0, &[]), "[00:01.23]x\n[00:01.24]y\n");
    }

    #[test]
    fn plain_text_written_reads_back_unsynced() {
        let l = parse("Verse one\nstill one\n\nVerse two", "tags");
        let text = to_text(&l.lines);
        assert_eq!(text, "Verse one\nstill one\n\nVerse two\n");
        assert_eq!(parse(&text, "txt").lines, l.lines);
    }

    #[test]
    fn a_sidecar_is_never_written_over_unless_asked_and_is_read_first() {
        let dir = crate::testdir::TestDir::new("lyrics-sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("01 Song.flac");
        std::fs::write(&track, b"not audio").unwrap();
        assert_eq!(sidecar(&track, true), dir.join("01 Song.lrc"));
        assert_eq!(sidecar(&track, false), dir.join("01 Song.txt"));
        assert!(load(&track).is_none());
        // Plain text first: read as the track's lyrics.
        write_sidecar(&sidecar(&track, false), "Words\nmore words\n", false).unwrap();
        let l = load(&track).unwrap();
        assert_eq!((l.source, l.synced, l.lines.len()), ("txt", false, 2));
        // A .lrc beside it wins over the .txt.
        let lrc = sidecar(&track, true);
        write_sidecar(&lrc, "[00:01.00]One\n", false).unwrap();
        assert_eq!(load(&track).unwrap().source, "lrc");
        // There already: refused and untouched, unless replacing is asked for.
        assert_eq!(write_sidecar(&lrc, "[00:02.00]Two\n", false), Err(SaveError::Exists));
        assert_eq!(std::fs::read_to_string(&lrc).unwrap(), "[00:01.00]One\n");
        write_sidecar(&lrc, "[00:02.00]Two\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&lrc).unwrap(), "[00:02.00]Two\n");
        assert_eq!(load(&track).unwrap().lines[0].time_ms, Some(2_000));
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 3, "no temporary file left: {left:?}");
        // UTF-8 as written.
        write_sidecar(&lrc, "[00:01.00]夜の港\n", true).unwrap();
        assert_eq!(load(&track).unwrap().lines[0].text, "夜の港");
        // A missing folder: the system's error; a server's track: refused.
        assert!(matches!(write_sidecar(&dir.join("gone").join("x.lrc"), "x", false), Err(SaveError::Io(_))));
        assert!(matches!(write_sidecar(Path::new("navidrome://s/1.lrc"), "x", false), Err(SaveError::Refused(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
