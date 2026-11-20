//! Which fields can be edited, how a value is checked, and how a change to a field
//! (by its canonical key, `meta::FIELDS`) becomes changes to the items of a file's tag
//! blocks, in each format's own terms (`plan`).
//!
//! A field is written to the block the library reads (lofty's primary tag of the format,
//! else the first one the file has) and to every other block of the file that already
//! holds it, so they keep agreeing; it is never added to a secondary block. A file whose
//! only tags are in a block that holds few fields (ID3v1, RIFF INFO, AIFF text chunks)
//! gets the format's main tag (ID3v2.3), with everything that block held copied into it,
//! so the library reads what it read before and the edit. Multi-valued fields use each
//! format's own form: Vorbis comments and APE repeat or NUL-separate, ID3v2.4 separates
//! with NUL, MP4 keeps one data atom per value; ID3v2.3, RIFF INFO and ID3v1 hold one
//! value, so several are joined with "; " (which the scanner splits again).

use crate::meta::{self, BlockKind, FileTags};

use super::Container;

/// What the editor lets a person do with a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editability {
    /// Type a value (or several).
    Edit,
    /// Remove only: identifiers nobody types by hand (MusicBrainz ids, ISRC, barcode).
    Remove,
    /// Shown, never changed: loudness, encoder details, lyrics, unknown raw items.
    View,
}

pub fn editability(key: &str) -> Editability {
    match meta::by_key(key).map(|d| d.group) {
        Some(meta::Group::Main) if key == "RATING" => Editability::View,
        Some(meta::Group::Main | meta::Group::Sort) => Editability::Edit,
        Some(meta::Group::Ids) => Editability::Remove,
        _ => Editability::View,
    }
}

/// Fields written whole by another part of the app, never typed in View metadata (where they
/// stay `Editability::View`): the lyrics, which the Lyrics tab's "Save into the file's tags"
/// writes as one text (LRC or plain).
pub const WHOLE: &[&str] = &["LYRICS"];

/// Whether `plan` takes an edit of the field: what View metadata edits, and `WHOLE`.
fn plannable(key: &str) -> bool {
    editability(key) != Editability::View || WHOLE.contains(&key)
}

/// How a field's values are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Digits: track, disc and their totals, BPM.
    Number,
    /// A date the scanner reads: "2019", "2019-03", "2019-03-08" (and "2019.03.08",
    /// "2019/3/8", "20190308", which become ISO).
    Date,
    Year,
    /// Yes or no: the compilation flag.
    Flag,
}

pub fn kind(key: &str) -> Kind {
    match key {
        "TRACKNUMBER" | "TRACKTOTAL" | "DISCNUMBER" | "DISCTOTAL" | "BPM" => Kind::Number,
        "DATE" | "ORIGINALDATE" | "RELEASEDATE" => Kind::Date,
        "YEAR" | "ORIGINALYEAR" => Kind::Year,
        "COMPILATION" => Kind::Flag,
        _ => Kind::Text,
    }
}

/// Fields that often hold several values: the editor offers "Add a value" on them.
pub fn multi_valued(key: &str) -> bool {
    matches!(
        key,
        "ARTIST" | "ARTISTS" | "ALBUMARTIST" | "GENRE" | "COMPOSER" | "LYRICIST" | "CONDUCTOR" | "PERFORMER" | "REMIXER" | "PRODUCER" | "ENGINEER" | "MIXER" | "ARRANGER" | "LABEL" | "CATALOGNUMBER" | "RELEASETYPE" | "MOOD"
    )
}

/// The fields every track view offers, present in the file or not, in this order: what
/// people fix most.
pub const COMMON: &[&str] = &["TITLE", "ARTIST", "ALBUM", "ALBUMARTIST", "TRACKNUMBER", "TRACKTOTAL", "DISCNUMBER", "DISCTOTAL", "DATE", "GENRE", "COMPOSER", "COMMENT"];

/// The fields an album view edits once for all its tracks.
pub const ALBUM_FIELDS: &[&str] = &["ALBUM", "ALBUMARTIST", "DATE", "GENRE", "LABEL", "DISCTOTAL", "COMPILATION"];

/// One value checked and put in the form it is written in, or why it cannot be.
pub fn check_value(key: &str, value: &str) -> Result<String, String> {
    let v = value.trim();
    match kind(key) {
        Kind::Text => {
            if v.contains('\0') {
                return Err("no NUL characters".into());
            }
            Ok(v.to_owned())
        }
        Kind::Number => {
            if !v.is_empty() && v.len() <= 6 && v.bytes().all(|b| b.is_ascii_digit()) {
                Ok(v.to_owned())
            } else {
                Err("a number, digits only".into())
            }
        }
        Kind::Year => {
            if v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit()) {
                Ok(v.to_owned())
            } else {
                Err("a year, four digits".into())
            }
        }
        Kind::Date => {
            let d = crate::tags::normalize_date(v);
            if iso_date(&d) { Ok(d) } else { Err("a date: 2019, 2019-03 or 2019-03-08 (2019.03.08 and 2019/3/8 are fine too)".into()) }
        }
        Kind::Flag => match v.to_ascii_lowercase().as_str() {
            "1" | "yes" | "true" | "on" | "y" => Ok("1".into()),
            "0" | "no" | "false" | "off" | "n" => Ok("0".into()),
            _ => Err("yes or no".into()),
        },
    }
}

/// "2019", "2019-03" or "2019-03-08", with a real month and day.
pub fn iso_date(d: &str) -> bool {
    let b = d.as_bytes();
    let digits = |r: std::ops::Range<usize>| d.get(r).is_some_and(|x| x.bytes().all(|c| c.is_ascii_digit()));
    match b.len() {
        4 => digits(0..4),
        7 => digits(0..4) && b[4] == b'-' && digits(5..7) && (1..=12).contains(&d[5..7].parse::<u32>().unwrap_or(0)),
        10 => {
            digits(0..4)
                && b[4] == b'-'
                && b[7] == b'-'
                && digits(5..7)
                && digits(8..10)
                && (1..=12).contains(&d[5..7].parse::<u32>().unwrap_or(0))
                && (1..=31).contains(&d[8..10].parse::<u32>().unwrap_or(0))
        }
        _ => false,
    }
}

/// A field's new values, checked: empties dropped, each in its written form. No values
/// means the field is removed.
pub fn check(key: &str, values: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for v in values {
        if v.trim().is_empty() {
            continue;
        }
        let c = check_value(key, v)?;
        if !out.contains(&c) {
            out.push(c);
        }
    }
    if out.len() > 1 && matches!(kind(key), Kind::Number | Kind::Date | Kind::Year | Kind::Flag) {
        return Err("one value only".into());
    }
    Ok(out)
}

/// One change to an item of a tag block, in the block's own terms (as `meta::read` with
/// `ReadOptions::RAW` names it): its values, or none to remove it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Change {
    pub block: BlockKind,
    pub key: String,
    pub values: Vec<String>,
}

/// What a set of field changes comes to in one file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub changes: Vec<Change>,
    /// A block the file did not have and gets (its main tag).
    pub creates: Option<BlockKind>,
    /// What a person should know: "ID3v1 holds 30 characters: its title is cut".
    pub notes: Vec<String>,
}

/// The block the library reads: lofty's primary tag of the format when the file has it,
/// else the first the file has (lofty's order). None: no tags.
pub fn main_block(container: Container, blocks: &[BlockKind]) -> Option<BlockKind> {
    let primary = container.primary();
    if let Some(b) = blocks.iter().find(|b| same_kind(**b, primary)) {
        return Some(*b);
    }
    let order: &[fn(BlockKind) -> bool] = match container {
        Container::Mpeg => &[|b| b == BlockKind::Id3v1, |b| b == BlockKind::Ape],
        Container::Aac => &[|b| b == BlockKind::Id3v1],
        Container::Wav => &[|b| b == BlockKind::RiffInfo],
        Container::Aiff => &[|b| b == BlockKind::AiffText],
        Container::Flac => &[|b| matches!(b, BlockKind::Id3v2(_))],
        _ => &[],
    };
    order.iter().find_map(|f| blocks.iter().copied().find(|b| f(*b)))
}

/// Two blocks of one kind (any ID3v2 version is ID3v2).
pub fn same_kind(a: BlockKind, b: BlockKind) -> bool {
    match (a, b) {
        (BlockKind::Id3v2(_), BlockKind::Id3v2(_)) => true,
        _ => a == b,
    }
}

/// Whether the editor writes this block in this container. FLAC's ID3v2 (lofty reads it
/// only) is left as it is.
pub fn writable(container: Container, block: BlockKind) -> bool {
    match block {
        BlockKind::Id3v2(v) => v >= 3 && matches!(container, Container::Mpeg | Container::Aac | Container::Wav | Container::Aiff),
        BlockKind::Id3v1 => matches!(container, Container::Mpeg | Container::Aac),
        BlockKind::Ape => container == Container::Mpeg,
        BlockKind::Vorbis => matches!(container, Container::Flac | Container::Opus | Container::Vorbis),
        BlockKind::Mp4 => container == Container::Mp4,
        BlockKind::RiffInfo => container == Container::Wav,
        BlockKind::AiffText => container == Container::Aiff,
    }
}

/// Blocks that hold only a few fields: a file whose main tag is one of them gets the
/// format's main tag.
fn limited(block: BlockKind) -> bool {
    matches!(block, BlockKind::Id3v1 | BlockKind::RiffInfo | BlockKind::AiffText | BlockKind::Ape)
}

/// The key a field has in a block kind when the file does not have it there yet. None:
/// the block has no place for it.
pub fn default_key(block: BlockKind, key: &str) -> Option<String> {
    let s = |x: &str| Some(x.to_owned());
    match block {
        BlockKind::Vorbis => s(key),
        BlockKind::Id3v2(v) => {
            let frame = match key {
                "TITLE" => "TIT2",
                "ARTIST" => "TPE1",
                "ALBUM" => "TALB",
                "ALBUMARTIST" => "TPE2",
                "TRACKNUMBER" | "TRACKTOTAL" => "TRCK",
                "DISCNUMBER" | "DISCTOTAL" => "TPOS",
                "DATE" => {
                    if v == 3 {
                        "TYER"
                    } else {
                        "TDRC"
                    }
                }
                "ORIGINALDATE" => {
                    if v == 3 {
                        "TORY"
                    } else {
                        "TDOR"
                    }
                }
                "RELEASEDATE" if v == 4 => "TDRL",
                "GENRE" => "TCON",
                "COMPOSER" => "TCOM",
                "LYRICIST" => "TEXT",
                "CONDUCTOR" => "TPE3",
                "REMIXER" => "TPE4",
                "LABEL" => "TPUB",
                "MEDIA" => "TMED",
                "LANGUAGE" => "TLAN",
                "GROUPING" => "TIT1",
                "SUBTITLE" => "TIT3",
                "DISCSUBTITLE" if v == 4 => "TSST",
                "MOOD" if v == 4 => "TMOO",
                "COMPILATION" => "TCMP",
                "BPM" => "TBPM",
                "KEY" => "TKEY",
                "COPYRIGHT" => "TCOP",
                "COMMENT" => "COMM",
                // Unsynchronised lyrics (LRC text in it is what players read as synced).
                "LYRICS" => "USLT",
                "ARTISTSORT" => "TSOP",
                "ALBUMARTISTSORT" => "TSO2",
                "ALBUMSORT" => "TSOA",
                "TITLESORT" => "TSOT",
                "COMPOSERSORT" => "TSOC",
                "RELEASETYPE" => return s("TXXX:MusicBrainz Album Type"),
                "RELEASESTATUS" => return s("TXXX:MusicBrainz Album Status"),
                "RELEASECOUNTRY" => return s("TXXX:MusicBrainz Album Release Country"),
                // Identifiers and the rest: Picard's TXXX names.
                other => return meta::by_key(other).map(|_| format!("TXXX:{other}")),
            };
            s(frame)
        }
        BlockKind::Mp4 => {
            let atom = match key {
                "TITLE" => "\u{a9}nam",
                "ARTIST" => "\u{a9}ART",
                "ALBUM" => "\u{a9}alb",
                "ALBUMARTIST" => "aART",
                "TRACKNUMBER" | "TRACKTOTAL" => "trkn",
                "DISCNUMBER" | "DISCTOTAL" => "disk",
                "DATE" => "\u{a9}day",
                "GENRE" => "\u{a9}gen",
                "COMPOSER" => "\u{a9}wrt",
                "COMMENT" => "\u{a9}cmt",
                "LYRICS" => "\u{a9}lyr",
                "COPYRIGHT" => "cprt",
                "BPM" => "tmpo",
                "GROUPING" => "\u{a9}grp",
                "COMPILATION" => "cpil",
                "ARTISTSORT" => "soar",
                "ALBUMARTISTSORT" => "soaa",
                "ALBUMSORT" => "soal",
                "TITLESORT" => "sonm",
                "COMPOSERSORT" => "soco",
                "RELEASETYPE" => return s("----:com.apple.iTunes:MusicBrainz Album Type"),
                "RELEASESTATUS" => return s("----:com.apple.iTunes:MusicBrainz Album Status"),
                "RELEASECOUNTRY" => return s("----:com.apple.iTunes:MusicBrainz Album Release Country"),
                "KEY" => return s("----:com.apple.iTunes:initialkey"),
                other => return meta::by_key(other).map(|_| format!("----:com.apple.iTunes:{other}")),
            };
            s(atom)
        }
        BlockKind::Id3v1 => match key {
            "TITLE" => s("Title"),
            "ARTIST" => s("Artist"),
            "ALBUM" => s("Album"),
            "DATE" => s("Year"),
            "COMMENT" => s("Comment"),
            "TRACKNUMBER" => s("Track"),
            "GENRE" => s("Genre"),
            _ => None,
        },
        BlockKind::RiffInfo => match key {
            "TITLE" => s("INAM"),
            "ARTIST" => s("IART"),
            "ALBUM" => s("IPRD"),
            "TRACKNUMBER" => s("ITRK"),
            "DATE" => s("ICRD"),
            "GENRE" => s("IGNR"),
            "COMMENT" => s("ICMT"),
            "COPYRIGHT" => s("ICOP"),
            "COMPOSER" => s("IMUS"),
            _ => None,
        },
        BlockKind::AiffText => match key {
            "TITLE" => s("NAME"),
            "ARTIST" => s("AUTH"),
            "COPYRIGHT" => s("(c) "),
            "COMMENT" => s("ANNO"),
            _ => None,
        },
        BlockKind::Ape => None,
    }
}

/// "3/12" -> (Some("3"), Some("12")); "3" -> (Some("3"), None).
fn pair(v: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else { return (None, None) };
    match v.split_once('/') {
        Some((n, t)) => (Some(n.trim().to_owned()).filter(|x| !x.is_empty()), Some(t.trim().to_owned()).filter(|x| !x.is_empty())),
        None => (Some(v.to_owned()), None),
    }
}

/// ID3v1 text: Latin-1 only (None when the value has other characters), cut to `max`.
fn latin1(v: &str, max: usize) -> Option<String> {
    if v.chars().all(|c| (c as u32) < 256) { Some(v.chars().take(max).collect()) } else { None }
}

/// The values of a field in a block, as the block stores them now (by its first item).
fn current<'a>(fields: &'a [meta::Field], block: usize, key: &str) -> Option<&'a meta::Source> {
    fields.iter().filter(|f| f.def.is_some_and(|d| d.key == key)).flat_map(|f| f.sources.iter()).find(|s| s.block == block)
}

/// Every item of a field in a block (a field can be under two aliases).
fn sources<'a>(fields: &'a [meta::Field], block: usize, key: &str) -> Vec<&'a meta::Source> {
    fields.iter().filter(|f| f.def.is_some_and(|d| d.key == key)).flat_map(|f| f.sources.iter()).filter(|s| s.block == block).collect()
}

/// The values to write for a field in a block kind (as the block's reader gives them back).
fn encode(block: BlockKind, raw_key: &str, field: &str, values: &[String]) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    let joined = || vec![values.join("; ")];
    match block {
        BlockKind::Vorbis | BlockKind::Ape => values.to_vec(),
        BlockKind::Id3v2(v) => {
            if raw_key == "COMM" || raw_key.starts_with("COMM:") {
                vec![values.join("\n")]
            } else if v == 3 {
                joined()
            } else {
                values.to_vec()
            }
        }
        BlockKind::Mp4 => match raw_key {
            "cpil" => vec![if values[0] == "1" { "Yes".into() } else { "No".into() }],
            "tmpo" => vec![values[0].trim_start_matches('0').to_owned()].into_iter().map(|s| if s.is_empty() { "0".into() } else { s }).collect(),
            _ => values.to_vec(),
        },
        BlockKind::RiffInfo => joined(),
        BlockKind::AiffText => {
            if matches!(raw_key, "ANNO" | "COMT") {
                values.to_vec()
            } else {
                joined()
            }
        }
        BlockKind::Id3v1 => {
            let _ = field;
            joined()
        }
    }
}

/// Plan field changes (canonical key, new checked values; none removes the field) for a
/// file whose tags (read with `ReadOptions::RAW`) are `tags`.
pub fn plan(container: Container, tags: &FileTags, edits: &[(String, Vec<String>)]) -> Result<Plan, String> {
    let mut p = Plan::default();
    let fields = meta::fields(tags);
    let primary = container.primary();
    let main = main_block(container, &tags.blocks);
    // The block every edit goes to, and its index (a new one past the file's blocks).
    let (target, target_index) = match main {
        Some(b) if !limited(b) && writable(container, b) => (b, tags.blocks.iter().position(|x| *x == b).unwrap_or(usize::MAX)),
        Some(b) if same_kind(b, primary) && !writable(container, b) => return Err(format!("its {} tag cannot be written", b.name())),
        _ => {
            p.creates = Some(primary);
            (primary, usize::MAX)
        }
    };
    let mut out: Vec<Change> = Vec::new();
    let mut push = |c: Change, out: &mut Vec<Change>| {
        if let Some(e) = out.iter_mut().find(|x| x.block == c.block && x.key.eq_ignore_ascii_case(&c.key)) {
            *e = c;
        } else {
            out.push(c);
        }
    };

    // A file that gets its main tag: what its other block held, copied in first, so the
    // library reads what it read before (and then the edit).
    if p.creates.is_some()
        && let Some(from) = main
    {
        let from_index = tags.blocks.iter().position(|x| *x == from).unwrap_or(usize::MAX);
        let mut seen = Vec::new();
        for f in &fields {
            let Some(d) = f.def else { continue };
            let Some(s) = f.sources.iter().find(|s| s.block == from_index) else { continue };
            if seen.contains(&d.key) || s.values.is_empty() {
                continue;
            }
            seen.push(d.key);
            let (key, mut values) = (d.key, s.values.clone());
            if from == BlockKind::Ape && matches!(key, "TRACKNUMBER" | "DISCNUMBER") {
                // APE keeps "3/12": the number and the total are two fields here.
                let (n, t) = pair(values.first().map(String::as_str));
                values = n.into_iter().collect();
                if let Some(t) = t {
                    let tk = if key == "TRACKNUMBER" { "TRACKTOTAL" } else { "DISCTOTAL" };
                    write_field(container, target, target_index, tk, &[t], tags, &fields, &mut out, &mut push, &mut p.notes)?;
                }
            }
            let values = match kind(key) {
                Kind::Date => values.iter().map(|v| crate::tags::normalize_date(v)).filter(|v| iso_date(v)).collect(),
                _ => values,
            };
            if !values.is_empty() {
                write_field(container, target, target_index, key, &values, tags, &fields, &mut out, &mut push, &mut p.notes)?;
            }
        }
        p.notes.push(format!("The file gets an {} tag (the library's), with what its {} tag holds; that tag is kept and changed with it", target_name(target), from.name()));
    }

    // Picard's ARTISTS (each artist of the credit) is what the library credits when a file
    // has it: when the artist changes and ARTISTS is not edited too, it follows, or the
    // library would go on crediting the old artists.
    let mut edits: Vec<(String, Vec<String>)> = edits.to_vec();
    if let Some((_, artist)) = edits.iter().find(|(k, _)| k == "ARTIST")
        && !edits.iter().any(|(k, _)| k == "ARTISTS")
        && fields.iter().any(|f| f.def.is_some_and(|d| d.key == "ARTISTS"))
    {
        let mut names: Vec<String> = Vec::new();
        for v in artist {
            for c in crate::names::credits(v) {
                if !names.iter().any(|n| crate::names::norm(n) == crate::names::norm(&c.name)) {
                    names.push(c.name.clone());
                }
            }
        }
        if !names.is_empty() {
            p.notes.push(format!("Artists (each artist of the credit, which the library reads first) follows: {}", names.iter().map(|n| format!("\u{201c}{n}\u{201d}")).collect::<Vec<_>>().join(", ")));
        }
        edits.push(("ARTISTS".into(), names));
    }

    for (key, values) in &edits {
        let values = check(key, values).map_err(|e| format!("{}: {e}", meta::by_key(key).map_or(key.as_str(), |d| d.label)))?;
        if !plannable(key) {
            return Err(format!("{key} cannot be changed here"));
        }
        if editability(key) == Editability::Remove && !values.is_empty() {
            return Err(format!("{key} can only be removed"));
        }
        // The main block, then every other writable block that holds the field.
        write_field(container, target, target_index, key, &values, tags, &fields, &mut out, &mut push, &mut p.notes)?;
        for (i, b) in tags.blocks.iter().enumerate() {
            if i == target_index || !writable(container, *b) {
                continue;
            }
            let holds = !sources(&fields, i, key).is_empty() || matches!(key.as_str(), "TRACKTOTAL" | "DISCTOTAL") && combined(*b) && !sources(&fields, i, number_of(key)).is_empty();
            if holds {
                write_field(container, *b, i, key, &values, tags, &fields, &mut out, &mut push, &mut p.notes)?;
            }
        }
        if tags.blocks.iter().any(|b| matches!(b, BlockKind::Id3v2(_))) && container == Container::Flac {
            let i = tags.blocks.iter().position(|b| matches!(b, BlockKind::Id3v2(_))).unwrap_or(0);
            if !sources(&fields, i, key).is_empty() {
                p.notes.push("The file's ID3v2 tag (rare in FLAC, and read-only for the editor) is left as it is".into());
            }
        }
    }
    p.notes.dedup();
    p.changes = out;
    Ok(p)
}

fn target_name(b: BlockKind) -> String {
    match b {
        BlockKind::Id3v2(3) => "ID3v2.3".into(),
        other => other.name(),
    }
}

/// Blocks where a number and its total are one item.
fn combined(b: BlockKind) -> bool {
    matches!(b, BlockKind::Id3v2(_) | BlockKind::Mp4 | BlockKind::Ape)
}

fn number_of(total: &str) -> &str {
    if total == "TRACKTOTAL" { "TRACKNUMBER" } else { "DISCNUMBER" }
}

#[allow(clippy::too_many_arguments)]
fn write_field(
    container: Container,
    block: BlockKind,
    index: usize,
    key: &str,
    values: &[String],
    tags: &FileTags,
    fields: &[meta::Field],
    out: &mut Vec<Change>,
    push: &mut impl FnMut(Change, &mut Vec<Change>),
    notes: &mut Vec<String>,
) -> Result<(), String> {
    let _ = container;
    // MP4's numeric genre (`gnre`) is not where a text genre goes: `©gen` is, and `gnre` is removed below.
    let existing: Vec<&meta::Source> = sources(fields, index, key).into_iter().filter(|s| !(block == BlockKind::Mp4 && s.key == "gnre")).collect();
    let numbers = matches!(key, "TRACKNUMBER" | "TRACKTOTAL" | "DISCNUMBER" | "DISCTOTAL");
    // A number and its total in one item ("3/12"): the other half as it is (or as
    // already changed in this plan).
    if numbers && combined(block) {
        let number_key = if key.starts_with("TRACK") { "TRACKNUMBER" } else { "DISCNUMBER" };
        let raw_key = current(fields, index, number_key).map(|s| s.key.clone()).or_else(|| default_key(block, number_key)).ok_or("no place for numbers")?;
        let now = out.iter().find(|c| c.block == block && c.key == raw_key).map(|c| c.values.first().cloned()).unwrap_or_else(|| current(fields, index, number_key).and_then(|s| s.values.first().cloned()));
        let (mut n, mut t) = pair(now.as_deref());
        let v = values.first().cloned();
        if key.ends_with("NUMBER") {
            n = v;
        } else {
            t = v;
        }
        let value = match (n, t) {
            (Some(n), Some(t)) => vec![format!("{n}/{t}")],
            (Some(n), None) => vec![n],
            (None, Some(t)) if block == BlockKind::Mp4 => vec![format!("0/{t}")],
            (None, Some(t)) => vec![format!("0/{t}")],
            (None, None) => Vec::new(),
        };
        // MP4 stores numbers: "03/12" reads back "3/12".
        let value = if block == BlockKind::Mp4 {
            value
                .into_iter()
                .map(|v| {
                    let (n, t) = pair(Some(&v));
                    let num = |x: Option<String>| x.and_then(|x| x.parse::<u16>().ok()).unwrap_or(0);
                    let (n, t) = (num(n), num(t));
                    if t > 0 { format!("{n}/{t}") } else { n.to_string() }
                })
                .collect()
        } else {
            value
        };
        push(Change { block, key: raw_key, values: value }, out);
        return Ok(());
    }
    if block == BlockKind::Id3v1 {
        let Some(raw) = default_key(block, key) else { return Ok(()) };
        let first = values.join("; ");
        let value: Vec<String> = match key {
            "DATE" => values.first().map(|d| d[..4.min(d.len())].to_owned()).into_iter().collect(),
            "TRACKNUMBER" => values.first().and_then(|n| n.parse::<u8>().ok()).filter(|n| *n > 0).map(|n| n.to_string()).into_iter().collect(),
            "GENRE" => {
                let g = values.first().and_then(|g| lofty::id3::v1::GENRES.iter().find(|x| x.eq_ignore_ascii_case(g.trim())));
                if g.is_none() && !values.is_empty() {
                    notes.push(format!("ID3v1 has a fixed list of genres without \u{201c}{}\u{201d}: its genre is cleared", values[0]));
                }
                g.map(|g| (*g).to_owned()).into_iter().collect()
            }
            _ => {
                let max = if key == "COMMENT" { 28 } else { 30 };
                match latin1(&first, max) {
                    Some(v) => {
                        if first.chars().count() > max {
                            notes.push(format!("ID3v1 holds {max} characters: its {} is cut", meta::by_key(key).map_or(key, |d| d.label).to_lowercase()));
                        }
                        vec![v].into_iter().filter(|v| !v.is_empty()).collect()
                    }
                    None => {
                        notes.push(format!("ID3v1 holds only Western European letters: its {} is cleared", meta::by_key(key).map_or(key, |d| d.label).to_lowercase()));
                        Vec::new()
                    }
                }
            }
        };
        push(Change { block, key: raw, values: value }, out);
        return Ok(());
    }
    // ID3v2.3 dates: TYER holds the year, TDAT the day and month ("DDMM"); TIME goes
    // with the date it timed.
    if let BlockKind::Id3v2(3) = block
        && key == "DATE"
    {
        let d = values.first();
        push(Change { block, key: "TYER".into(), values: d.map(|d| d[..4].to_owned()).into_iter().collect() }, out);
        let ddmm = d.filter(|d| d.len() == 10).map(|d| format!("{}{}", &d[8..10], &d[5..7]));
        push(Change { block, key: "TDAT".into(), values: ddmm.into_iter().collect() }, out);
        for k in ["TIME", "TDRC"] {
            if tags.items.iter().any(|i| i.block == index && i.key == k) {
                push(Change { block, key: k.into(), values: Vec::new() }, out);
            }
        }
        if d.is_some_and(|d| d.len() == 7) {
            notes.push("ID3v2.3 keeps a year, or a full date: the month alone is not written".into());
        }
        return Ok(());
    }
    if let BlockKind::Id3v2(3) = block
        && key == "ORIGINALDATE"
        && existing.is_empty()
    {
        let d = values.first();
        if d.is_some_and(|d| d.len() > 4) {
            notes.push("ID3v2.3 keeps the original year only".into());
        }
        push(Change { block, key: "TORY".into(), values: d.map(|d| d[..4].to_owned()).into_iter().collect() }, out);
        return Ok(());
    }
    let raw_key = match existing.first() {
        Some(s) => s.key.clone(),
        None => match default_key(block, key) {
            Some(k) => k,
            None if values.is_empty() => return Ok(()),
            None => return Err(format!("{} has no place for {}", block.name(), meta::by_key(key).map_or(key, |d| d.label))),
        },
    };
    // ID3v2.3's TORY holds a year.
    let values: Vec<String> = if raw_key == "TORY" { values.iter().map(|d| d[..4.min(d.len())].to_owned()).collect() } else { values.to_vec() };
    let encoded = encode(block, &raw_key, key, &values);
    if matches!(block, BlockKind::Id3v2(3) | BlockKind::RiffInfo) && values.len() > 1 {
        notes.push(format!("{} keeps one value per field: several are written joined with \u{201c}; \u{201d}", target_name(block)));
    }
    push(Change { block, key: raw_key, values: encoded }, out);
    // Other items of the same field in this block (an alias): removed, so one is left.
    for s in existing.iter().skip(1) {
        push(Change { block, key: s.key.clone(), values: Vec::new() }, out);
    }
    // MP4's numeric genre goes when the text genre is written.
    if block == BlockKind::Mp4 && key == "GENRE" && tags.items.iter().any(|i| i.block == index && i.key == "gnre") {
        push(Change { block, key: "gnre".into(), values: Vec::new() }, out);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::RawItem;

    fn tags(blocks: Vec<BlockKind>, items: &[(usize, &str, &[&str])]) -> FileTags {
        FileTags {
            blocks,
            items: items.iter().map(|(b, k, v)| RawItem { block: *b, key: (*k).into(), values: v.iter().map(|s| (*s).to_owned()).collect(), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    fn e(k: &str, v: &[&str]) -> (String, Vec<String>) {
        (k.to_owned(), v.iter().map(|s| (*s).to_owned()).collect())
    }

    fn ch(b: BlockKind, k: &str, v: &[&str]) -> Change {
        Change { block: b, key: k.into(), values: v.iter().map(|s| (*s).to_owned()).collect() }
    }

    #[test]
    fn values_are_checked_forgivingly() {
        assert_eq!(check_value("DATE", "2019.03.08"), Ok("2019-03-08".into()));
        assert_eq!(check_value("DATE", "2019/3/8"), Ok("2019-03-08".into()));
        assert_eq!(check_value("DATE", " 2019 "), Ok("2019".into()));
        assert!(check_value("DATE", "Summer 1999").is_err());
        assert!(check_value("DATE", "2019-13-01").is_err());
        assert_eq!(check_value("TRACKNUMBER", "07"), Ok("07".into()));
        assert!(check_value("TRACKNUMBER", "A1").is_err());
        assert_eq!(check_value("COMPILATION", "Yes"), Ok("1".into()));
        assert_eq!(check("ARTIST", &["A".into(), " ".into(), "B".into(), "A".into()]), Ok(vec!["A".into(), "B".into()]));
        assert!(check("DATE", &["2019".into(), "2020".into()]).is_err());
        assert_eq!(editability("ALBUMARTIST"), Editability::Edit);
        assert_eq!(editability("MUSICBRAINZ_ALBUMID"), Editability::Remove);
        assert_eq!(editability("REPLAYGAIN_TRACK_GAIN"), Editability::View);
        assert_eq!(editability("TXXX:Odd"), Editability::View);
    }

    #[test]
    fn vorbis_edits_keep_their_keys_and_aliases_go() {
        let t = tags(vec![BlockKind::Vorbis], &[(0, "title", &["Old"]), (0, "TOTALTRACKS", &["9"]), (0, "TRACKTOTAL", &["9"]), (0, "ARTIST", &["A"])]);
        let p = plan(Container::Flac, &t, &[e("TITLE", &["New"]), e("TRACKTOTAL", &["10"]), e("ARTIST", &["A", "B"])]).unwrap();
        assert_eq!(p.creates, None);
        assert_eq!(
            p.changes,
            [ch(BlockKind::Vorbis, "title", &["New"]), ch(BlockKind::Vorbis, "TOTALTRACKS", &["10"]), ch(BlockKind::Vorbis, "TRACKTOTAL", &[]), ch(BlockKind::Vorbis, "ARTIST", &["A", "B"])]
        );
    }

    #[test]
    fn id3_numbers_dates_and_the_id3v1_block() {
        let v3 = BlockKind::Id3v2(3);
        let t = tags(vec![v3, BlockKind::Id3v1], &[(0, "TRCK", &["2/12"]), (0, "TYER", &["2022"]), (0, "TIT2", &["A long title"]), (1, "Title", &["A long title"]), (1, "Track", &["2"]), (1, "Year", &["2022"])]);
        let p = plan(Container::Mpeg, &t, &[e("TRACKNUMBER", &["3"]), e("DATE", &["2023-04-05"]), e("TITLE", &["A title well over thirty characters long"]), e("ARTIST", &["X", "Y"])]).unwrap();
        let has = |c: Change| assert!(p.changes.contains(&c), "{c:?} in {:#?}", p.changes);
        has(ch(v3, "TRCK", &["3/12"]));
        has(ch(BlockKind::Id3v1, "Track", &["3"]));
        has(ch(v3, "TYER", &["2023"]));
        has(ch(v3, "TDAT", &["0504"]));
        has(ch(BlockKind::Id3v1, "Year", &["2023"]));
        has(ch(BlockKind::Id3v1, "Title", &["A title well over thirty chara"]));
        has(ch(v3, "TPE1", &["X; Y"]));
        assert!(!p.changes.iter().any(|c| c.block == BlockKind::Id3v1 && c.key == "Artist"), "ID3v1 had no artist: none added");
        assert!(p.notes.iter().any(|n| n.contains("30 characters")));
        assert!(p.notes.iter().any(|n| n.contains("joined")));
        // ID3v2.4: NUL-separated values, TDRC.
        let v4 = BlockKind::Id3v2(4);
        let t = tags(vec![v4], &[(0, "TPE1", &["A"])]);
        let p = plan(Container::Mpeg, &t, &[e("ARTIST", &["A", "B"]), e("DATE", &["2020"]), e("DISCTOTAL", &["2"])]).unwrap();
        assert_eq!(p.changes, [ch(v4, "TPE1", &["A", "B"]), ch(v4, "TDRC", &["2020"]), ch(v4, "TPOS", &["0/2"])]);
    }

    #[test]
    fn a_file_without_tags_gets_the_main_one_and_a_limited_one_is_copied_in() {
        let p = plan(Container::Wav, &FileTags::default(), &[e("TITLE", &["T"])]).unwrap();
        assert_eq!(p.creates, Some(BlockKind::Id3v2(3)));
        assert_eq!(p.changes, [ch(BlockKind::Id3v2(3), "TIT2", &["T"])]);
        let t = tags(vec![BlockKind::RiffInfo], &[(0, "INAM", &["Old"]), (0, "IART", &["Band"]), (0, "ICRD", &["2001"])]);
        let p = plan(Container::Wav, &t, &[e("TITLE", &["New"])]).unwrap();
        let v3 = BlockKind::Id3v2(3);
        assert_eq!(p.creates, Some(v3));
        assert!(p.changes.contains(&ch(v3, "TIT2", &["New"])));
        assert!(p.changes.contains(&ch(v3, "TPE1", &["Band"])), "the artist copied in");
        assert!(p.changes.contains(&ch(v3, "TYER", &["2001"])));
        assert!(p.changes.contains(&ch(BlockKind::RiffInfo, "INAM", &["New"])), "the INFO title kept in step");
        assert!(!p.changes.iter().any(|c| c.block == BlockKind::RiffInfo && c.key == "IART"));
        let p = plan(Container::Flac, &FileTags::default(), &[e("ALBUM", &["A"])]).unwrap();
        assert_eq!(p.creates, Some(BlockKind::Vorbis));
    }

    #[test]
    fn mp4_atoms() {
        let t = tags(vec![BlockKind::Mp4], &[(0, "trkn", &["2/2"]), (0, "gnre", &["Rock"]), (0, "cpil", &["No"])]);
        let p = plan(Container::Mp4, &t, &[e("TRACKNUMBER", &["03"]), e("GENRE", &["Ambient", "Drone"]), e("COMPILATION", &["yes"]), e("LABEL", &["L"])]).unwrap();
        assert_eq!(
            p.changes,
            [
                ch(BlockKind::Mp4, "trkn", &["3/2"]),
                ch(BlockKind::Mp4, "\u{a9}gen", &["Ambient", "Drone"]),
                ch(BlockKind::Mp4, "gnre", &[]),
                ch(BlockKind::Mp4, "cpil", &["Yes"]),
                ch(BlockKind::Mp4, "----:com.apple.iTunes:LABEL", &["L"])
            ]
        );
    }

    #[test]
    fn lyrics_are_written_whole_to_each_formats_lyrics_item() {
        let text = "[00:01.00]One\n[00:02.50]Two";
        assert_eq!(editability("LYRICS"), Editability::View, "View metadata still only shows them");
        let p = plan(Container::Flac, &tags(vec![BlockKind::Vorbis], &[(0, "UNSYNCEDLYRICS", &["old"])]), &[e("LYRICS", &[text])]).unwrap();
        assert_eq!(p.changes, [ch(BlockKind::Vorbis, "UNSYNCEDLYRICS", &[text])], "where the file keeps them");
        let p = plan(Container::Flac, &tags(vec![BlockKind::Vorbis], &[]), &[e("LYRICS", &[text])]).unwrap();
        assert_eq!(p.changes, [ch(BlockKind::Vorbis, "LYRICS", &[text])]);
        let v3 = BlockKind::Id3v2(3);
        let p = plan(Container::Mpeg, &tags(vec![v3, BlockKind::Id3v1], &[(0, "TIT2", &["T"]), (1, "Title", &["T"])]), &[e("LYRICS", &[text])]).unwrap();
        assert_eq!(p.changes, [ch(v3, "USLT", &[text])], "ID3v1 has no place for them, and is left alone");
        let p = plan(Container::Mp4, &tags(vec![BlockKind::Mp4], &[]), &[e("LYRICS", &[text])]).unwrap();
        assert_eq!(p.changes, [ch(BlockKind::Mp4, "\u{a9}lyr", &[text])]);
        let p = plan(Container::Wav, &tags(vec![BlockKind::RiffInfo], &[(0, "INAM", &["T"])]), &[e("LYRICS", &[text])]).unwrap();
        assert!(p.changes.contains(&ch(v3, "USLT", &[text])), "a WAV with only INFO gets ID3v2.3: {:?}", p.changes);
        assert!(plan(Container::Flac, &tags(vec![BlockKind::Vorbis], &[]), &[e("SYNCEDLYRICS", &["x"])]).is_err(), "only the lyrics item");
    }

    #[test]
    fn what_cannot_be_changed_is_refused() {
        let t = tags(vec![BlockKind::Vorbis], &[(0, "MUSICBRAINZ_ALBUMID", &["x"])]);
        assert!(plan(Container::Flac, &t, &[e("REPLAYGAIN_TRACK_GAIN", &["1 dB"])]).is_err());
        assert!(plan(Container::Flac, &t, &[e("MUSICBRAINZ_ALBUMID", &["y"])]).is_err());
        let p = plan(Container::Flac, &t, &[e("MUSICBRAINZ_ALBUMID", &[])]).unwrap();
        assert_eq!(p.changes, [ch(BlockKind::Vorbis, "MUSICBRAINZ_ALBUMID", &[])]);
        assert!(plan(Container::Flac, &t, &[e("DATE", &["later"])]).is_err());
    }
}
