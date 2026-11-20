//! "View metadata": a track's or an album's menu (every track list, the queue, album tiles,
//! the album page) opens a wide panel over the window (ui/metaview.slint) with everything
//! known about it. It replaces the earlier Properties panel: a track's view holds what that
//! showed (the file's technical details, size, date added, plays) under "File".
//!
//! Track: what the library made of the file (and why, where that differs from the tags),
//! then every item of every tag block in the file (`library::meta`), in a fixed order with
//! friendly names, identifiers, sort names, loudness, encoding, the embedded pictures with
//! thumbnails, whether it has lyrics, the file's details, and last every other item under
//! its raw key. Album: the album as the library sees it, its cover and where that comes
//! from, what its tracks disagree on, and a table of its tracks; a row opens that track's
//! view, and Back returns to the album.
//!
//! What the database knows shows at once; what is read from files (always off the UI
//! thread, read-only) fills in as it arrives. A result for a view that was closed or left
//! is dropped, and an album's reads stop when its viewer closes. Nothing here runs while
//! the panel is closed.
//!
//! The rows are built as `Line`s here, from data, and only turned into strings for the
//! panel at the end: each tag row knows its field's canonical key, the block it came from
//! and the raw key (`FieldRef`), which is what editing in place would need later.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use library::audio::{self, AudioInfo, FormatKey};
use library::names;
use library::meta::{self, BlockKind, Brief, Field, FileTags, Group};
use library::query::{AlbumFacts, TrackExtras};
use library::tags::TrackTags;
use library::Track;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::metadata::details::{self, Known};
use crate::state::{self, State};
use crate::metadata;
use crate::{Dialog, Meta, MetaRow};

// ---- The line model ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Heading,
    #[default]
    Field,
    Picture,
    Note,
    TableHead,
    Track,
}

/// What Enter (or a click) on a line does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Act {
    #[default]
    None,
    /// Open the album's track at this index.
    OpenTrack(usize),
    /// List more of a long section.
    More(Section),
    /// Tag editing: add the fix of an album inconsistency (`Fix`) to the draft.
    Fix(usize),
    /// Tag editing: the front cover.
    Cover(CoverAct),
    /// Tag editing: undo a save in the Edit history (its index there).
    Undo(usize),
    /// Tag editing: offer every field that can be edited, not only the common ones.
    AllFields,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverAct {
    /// Pick an image to replace (or add) the front cover.
    Pick,
    Remove,
    /// Take the draft's cover change back.
    Keep,
}

/// What editing a line edits (tag editing on, the file editable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A track's field, by its canonical key.
    Field(String),
    /// An identifier: it can only be removed (or kept).
    Remove(String),
    /// An album's field, on every track.
    Album(String),
    /// A track of the album's table: its number, title and artist.
    Row(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    OtherTags,
    Tracks,
}

/// Which tag item a line shows: enough to change it in place later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldRef {
    /// The field's canonical key ("ALBUMARTIST"), or the raw key of one the app does not name.
    pub key: String,
    /// The tag block (index into the file's blocks).
    pub block: usize,
    /// The key in that block's own terms ("TPE2", "aART", "ALBUMARTIST").
    pub raw_key: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub label: String,
    /// The values in full (each shown on its own line; a long one is cut until expanded).
    pub values: Vec<String>,
    /// A quieter line under the values: where it came from, why it differs.
    pub note: String,
    /// Something to look at: blocks that disagree, an inconsistency, a file not read.
    pub attention: bool,
    /// What Copy puts on the clipboard ("" when there is nothing to copy).
    pub copy: String,
    /// Stable within a view, for a value's expanded state.
    pub id: String,
    /// A picture's thumbnail (index into the view's images).
    pub thumb: Option<usize>,
    pub act: Act,
    /// A track row's cells: disc, number, title, artist, length, format.
    pub cells: Vec<String>,
    pub number_attention: bool,
    pub format_attention: bool,
    pub field: Option<FieldRef>,
    /// Tag editing: what Enter, F2 or a click on the value edits.
    pub edit: Option<Edit>,
    /// A draft value (not saved yet): shown marked, with what it was in `note`.
    pub changed: bool,
    /// No value (a common field the file does not have): "Not set".
    pub empty: bool,
    /// Several values can be added (artists, genres...).
    pub multi: bool,
    /// Table rows: which cells (number, title, artist) the draft changes.
    pub cell_changed: [bool; 3],
}

impl Line {
    fn heading(title: &str) -> Line {
        Line { kind: Kind::Heading, label: title.to_owned(), ..Default::default() }
    }

    fn field(label: &str, value: impl Into<String>) -> Line {
        let v: String = value.into();
        Line { label: label.to_owned(), copy: v.clone(), values: vec![v], id: format!("l:{label}"), ..Default::default() }
    }

    fn note(text: impl Into<String>, attention: bool) -> Line {
        Line { kind: Kind::Note, values: vec![text.into()], attention, ..Default::default() }
    }

    fn with_note(mut self, note: impl Into<String>) -> Line {
        self.note = note.into();
        self
    }

    fn attention(mut self, on: bool) -> Line {
        self.attention = on;
        self
    }

    /// Up, Down and Enter stop here.
    pub fn selectable(&self) -> bool {
        match self.kind {
            Kind::Field | Kind::Picture | Kind::Track => true,
            Kind::Note => self.act != Act::None,
            Kind::Heading | Kind::TableHead => false,
        }
    }

    fn action(label: &str, text: impl Into<String>, act: Act) -> Line {
        Line { kind: Kind::Note, label: label.to_owned(), values: vec![text.into()], act, id: format!("a:{act:?}"), ..Default::default() }
    }
}

/// Quoted, for notes: “value”.
fn q(s: &str) -> String {
    format!("\u{201c}{s}\u{201d}")
}

// ---- Values as shown -------------------------------------------------------------------

/// A value longer than this many characters, or lines, is cut until expanded.
pub const PREVIEW_CHARS: usize = 320;
pub const PREVIEW_LINES: usize = 6;
/// An expanded value shows at most this much; Copy has the rest.
pub const EXPANDED_CHARS: usize = 24_000;
/// A field with more values lists this many, then how many more.
pub const MAX_VALUES: usize = 40;
/// Long sections list this many rows at first, and this many more on each "Show more".
pub const PAGE_ROWS: usize = 300;

fn cut(s: &str, chars: usize, lines: usize) -> Option<String> {
    let mut end = s.len();
    if let Some((at, _)) = s.char_indices().nth(chars) {
        end = at;
    }
    if let Some((at, _)) = s.match_indices('\n').nth(lines.saturating_sub(1)) {
        end = end.min(at);
    }
    (end < s.len()).then(|| s[..end].trim_end().to_owned())
}

/// Long unbroken runs (ids, fingerprints, base64) get a zero-width break every 24
/// characters so they wrap: the panel's text wraps only between words.
pub fn breakable(s: &str) -> String {
    if !s.split_whitespace().any(|w| w.chars().count() > 40) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    let mut run = 0;
    for c in s.chars() {
        if c.is_whitespace() {
            run = 0;
        } else {
            run += 1;
            if run > 24 && run % 24 == 1 {
                out.push('\u{200b}');
            }
        }
        out.push(c);
    }
    out
}

/// The values as the panel shows them, and whether there is more to expand.
pub fn shown(values: &[String], expanded: bool) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut expandable = false;
    for v in values.iter().take(MAX_VALUES) {
        let total = v.chars().count();
        if expanded {
            match cut(v, EXPANDED_CHARS, usize::MAX) {
                Some(c) => out.push(format!("{c}\u{2026}\n({} more characters; Copy has them all)", audio::thousands((total - c.chars().count()) as u64))),
                None => out.push(v.clone()),
            }
            expandable |= cut(v, PREVIEW_CHARS, PREVIEW_LINES).is_some();
        } else {
            match cut(v, PREVIEW_CHARS, PREVIEW_LINES) {
                Some(c) => {
                    out.push(format!("{c}\u{2026}"));
                    expandable = true;
                }
                None => out.push(v.clone()),
            }
        }
    }
    if values.len() > MAX_VALUES {
        out.push(format!("and {} more values", values.len() - MAX_VALUES));
    }
    (out.iter().map(|v| breakable(v)).collect(), expandable)
}

/// "1,234 characters" for a long value, the notes of expandable rows.
fn length_note(values: &[String]) -> String {
    let n: usize = values.iter().map(|v| v.chars().count()).sum();
    format!("{} characters", audio::thousands(n as u64))
}

// ---- Copy all --------------------------------------------------------------------------

/// The view as readable text: a title block, then each group's lines as "Label: value".
pub fn copy_all(head: &[String], lines: &[Line]) -> String {
    let mut out = String::new();
    for h in head.iter().filter(|h| !h.is_empty()) {
        out.push_str(h);
        out.push('\n');
    }
    for l in lines {
        match l.kind {
            Kind::Heading => {
                out.push('\n');
                out.push_str(&l.label);
                out.push('\n');
            }
            Kind::Field | Kind::Picture => {
                let value = if l.copy.is_empty() { l.values.join("; ") } else { l.copy.clone() };
                let value = value.replace('\n', "\n      ");
                out.push_str(&format!("  {}: {value}", l.label));
                if !l.note.is_empty() {
                    out.push_str(&format!("  ({})", l.note.replace('\n', " ")));
                }
                out.push('\n');
            }
            Kind::Note if l.act == Act::None => {
                out.push_str(&format!("  {}\n", l.values.join(" ")));
            }
            Kind::Note | Kind::TableHead => {}
            Kind::Track => {
                let c = |i: usize| l.cells.get(i).map(String::as_str).unwrap_or("");
                let no = match (c(0), c(1)) {
                    ("", n) => n.to_owned(),
                    (d, n) => format!("{d}.{n}"),
                };
                out.push_str(&format!("  {no:>5}  {} \u{2014} {}  {}  {}", c(2), c(3), c(4), c(5)));
                if !l.note.is_empty() {
                    out.push_str(&format!("  [{}]", l.note));
                }
                out.push('\n');
            }
        }
    }
    out
}

// ---- A track -------------------------------------------------------------------------

/// What was read from the file for a track's view.
#[derive(Clone, Debug, Default)]
pub enum FileRead {
    #[default]
    Pending,
    /// Not on disk (moved or deleted since the last scan, a drive not there).
    Missing,
    /// There, but not readable as audio.
    Unreadable(String),
    Read(Box<ReadFile>),
    /// Only on the server (`library::remote`): there is no file here to read or edit; what the
    /// library knows of it (from the server's catalog) is all there is.
    Server,
}

/// What View metadata says of a track only the server has, where a file's tags would be.
pub const FROM_SERVER: &str = "From the server: this track has no file here, so there are no tags to read or edit. What is shown is what the library knows of it from the server's catalog.";

#[derive(Clone, Debug)]
pub struct ReadFile {
    pub tags: FileTags,
    pub fields: Vec<Field>,
    /// The tags as the scanner reads them (what the library would make of the file now).
    pub scanned: Option<TrackTags>,
    pub report: Option<library::tags::FileReport>,
    pub size: Option<u64>,
    /// A `.lrc` beside the file (the app shows it before any embedded lyrics).
    pub lrc: Option<String>,
    /// Tag editing: the file as read (what it must still be when saved), and whether it
    /// can be edited (why not).
    pub expect: Option<library::edit::Expect>,
    pub editable: Result<(), String>,
}

/// Tag editing's part of a view (None: editing is off, the view is read-only).
#[derive(Clone, Debug, Default)]
pub struct EditView {
    /// The draft of this file (a track's view) or of every file (an album's), by path.
    pub draft: metadata::tagedit::Draft,
    /// Every editable field offered, not only the common ones.
    pub all_fields: bool,
    /// The Edit history of these files: (when, what, files of these it wrote, undone).
    pub history: Vec<(String, String, usize, bool)>,
}

/// Everything a track's view is made from.
#[derive(Clone, Debug, Default)]
pub struct TrackInput {
    pub track: Track,
    /// The album as stored (library tracks).
    pub album: Option<AlbumFacts>,
    pub genre: Option<String>,
    /// The artists the library credits the track to, in order.
    pub credits: Vec<library::query::ArtistRef>,
    /// Its genres as the library keeps them.
    pub genres: Vec<String>,
    pub extras: Option<TrackExtras>,
    pub stream: Option<player_core::StreamInfo>,
    pub file: FileRead,
    /// Rows of "Other tags" to list (`PAGE_ROWS` more each time).
    pub other_limit: usize,
    pub edit: Option<EditView>,
}

impl Default for ReadFile {
    fn default() -> Self {
        Self { tags: FileTags::default(), fields: Vec::new(), scanned: None, report: None, size: None, lrc: None, expect: None, editable: Err(String::new()) }
    }
}

/// How the album a track belongs to is told apart from others.
pub fn grouped_by(key: &str) -> (String, String) {
    // An album only a server has, without a MusicBrainz id: the server's own album
    // (`library::remote`), which has no folder or tags here.
    if key.starts_with("nd:") {
        return ("The server's album".into(), "No MusicBrainz release id in the server's catalog".into());
    }
    match key.strip_prefix("mb:") {
        Some(id) => ("Its MusicBrainz release".into(), id.to_owned()),
        None if key.starts_with("t:various artists|") => ("A compilation: its title and folder".into(), "No MusicBrainz release id in its tags".into()),
        None => ("Album artist, title and folder".into(), "No MusicBrainz release id in its tags".into()),
    }
}

/// Where the scanner's date for a file comes from: (raw key, raw value).
fn date_source(fields: &[Field]) -> Option<(&'static str, String)> {
    ["DATE", "RELEASEDATE", "ORIGINALDATE", "YEAR"].iter().find_map(|k| meta::first(fields, k).map(|v| (*k, v)))
}

/// "What the library shows" for a track, with why where it is not simply the tag.
pub fn library_lines(t: &TrackInput) -> Vec<Line> {
    let tr = &t.track;
    let read = match &t.file {
        FileRead::Read(r) => Some(r.as_ref()),
        _ => None,
    };
    let path = Path::new(&tr.path);
    // With a draft: what the library will make of the file once it is saved.
    let draft = t.edit.as_ref().and_then(|e| e.draft.file(&tr.path));
    let drafted = read.and_then(|r| r.scanned.as_ref()).zip(draft).map(|(s, d)| with_draft(s, d, path));
    let scanned = drafted.as_ref().or_else(|| read.and_then(|r| r.scanned.as_ref()));
    let derived = scanned.map(|s| library::scan::display_names(s, path));
    let mut v = Vec::new();
    let changed = |now: &str| if drafted.is_some() { format!("Once saved, the library shows {}", q(now)) } else { format!("The file now says {}; the next scan takes it", q(now)) };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let compilation = t.album.as_ref().is_some_and(|a| a.compilation);

    let mut title = Line::field("Title", tr.title.clone());
    let mut artist = Line::field("Artist", tr.artist.clone());
    let mut album = Line::field("Album", tr.album.clone());
    let mut album_artist = Line::field("Album artist", tr.album_artist.clone());
    if let (Some(s), Some((d_title, d_artist, d_album))) = (scanned, &derived) {
        if s.from_name.title {
            title.note = format!("No title tag: from the file's name, {}", q(&stem));
        } else if d_title != &tr.title {
            title = title.with_note(changed(d_title)).attention(true);
        }
        if s.artist.is_none() {
            artist.note = if s.album_artist.is_some() { "No artist tag: the album artist".into() } else { "No artist tags".into() };
        } else if d_artist != &tr.artist {
            artist = artist.with_note(changed(d_artist)).attention(true);
        }
        if s.album.is_none() {
            album.note = "No album tag: the folder's name".into();
        } else if names::norm(d_album) != names::norm(&tr.album) {
            album = album.with_note(format!("The album's title, as most of its tracks have it: this file says {}", q(d_album))).attention(true);
        } else if d_album != &tr.album {
            album.note = format!("This file spells it {}: the album takes its tracks' most common spelling", q(d_album));
        }
        let own = s.album_artist.clone();
        if compilation && !own.as_deref().is_some_and(names::is_various) {
            album_artist.note = if s.compilation {
                "A compilation: this file is flagged as part of one, so its album is one album under Various Artists".into()
            } else {
                "A compilation: its tracks have no album artist tag and name different artists, so they are one album under Various Artists".into()
            };
        } else if let Some(own) = own {
            if names::norm(&own) != names::norm(&tr.album_artist) {
                album_artist = album_artist.with_note(format!("The album's, as most of its tracks have it: this file says {}", q(&own))).attention(true);
            }
        } else if names::norm(&library::scan::album_artist_of(s)) == names::norm(&tr.album_artist) {
            album_artist.note = if s.artist.is_some() { "No album artist tag: its tracks' artist".into() } else { "No artist tags at all".into() };
        } else {
            album_artist.note = "No album artist tag on this file: the album's other tracks name it".into();
        }
    }
    // Who the library credits: each artist of a credit that names several, featured ones too.
    if t.credits.len() > 1 && artist.note.is_empty() {
        let names: Vec<String> = t.credits.iter().map(|c| if c.featured { format!("{} (featured)", c.name) } else { c.name.clone() }).collect();
        artist.note = format!("Credits {}: the track is on each one's page", names.join(", "));
        if scanned.is_some_and(|s| s.title.as_deref().is_some_and(|ti| !names::title_featured(ti).is_empty())) {
            artist.note.push_str(" (the title names a featured artist)");
        }
    }
    v.extend([title, artist, album, album_artist]);

    // The album's date as stored, and where a scan of this file takes it from.
    if let Some(a) = &t.album {
        let mut date = Line::field("Date", a.date.clone().unwrap_or_else(|| "None".into()));
        if let (Some(r), Some(s)) = (read, scanned) {
            match (date_source(&r.fields), &s.date) {
                (Some((key, raw)), Some(d)) => {
                    let from = if key == "DATE" { String::new() } else { format!("From {key} (no DATE tag)") };
                    let norm = if &raw != d { format!("read from {} as {}", q(&raw), q(d)) } else { String::new() };
                    let mut note = [from, norm].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("; ");
                    if a.date.as_deref() != Some(d.as_str()) {
                        let same_year = a.date.as_deref().and_then(|x| x.get(..4)) == d.get(..4);
                        note = if same_year {
                            format!("The album's: this file says {}; of its tracks' dates the fullest of the most common year is taken", q(d))
                        } else {
                            format!("The album's, as most of its tracks have it: this file says {}", q(d))
                        };
                        date.attention = !same_year;
                    }
                    if let Some(first) = note.get(..1) {
                        note = first.to_uppercase() + &note[1..];
                    }
                    date.note = note;
                }
                _ if a.date.is_some() => {
                    date.note = "This file has no date: the album's, from its other tracks".into();
                }
                _ => date.note = "No date tags".into(),
            }
        }
        v.push(date);
    }
    let genre_text = if t.genres.is_empty() { t.genre.clone().unwrap_or_else(|| "None".into()) } else { t.genres.join(", ") };
    let mut genre = Line::field(if t.genres.len() > 1 { "Genres" } else { "Genre" }, genre_text);
    genre.values = if t.genres.len() > 1 { t.genres.clone() } else { genre.values };
    if let Some(s) = scanned
        && s.genre != t.genre
    {
        genre = genre.with_note(changed(s.genre.as_deref().unwrap_or("no genre"))).attention(true);
    } else if t.genres.len() > 1 {
        genre.note = "The track, and its album, is under each of these genres".into();
    }
    v.push(genre);
    let mut place = Vec::new();
    if let Some(d) = tr.disc_no {
        place.push(format!("Disc {d}"));
    }
    place.push(match tr.track_no {
        Some(n) => format!("track {n}"),
        None => "no track number".into(),
    });
    let mut pos = Line::field("Disc and track", {
        let s = place.join(", ");
        s[..1].to_uppercase() + &s[1..]
    });
    if let Some(s) = scanned {
        if s.track_no != tr.track_no || s.disc_no != tr.disc_no {
            pos = pos.with_note("The file's numbers changed since the last scan").attention(true);
        } else if s.from_name.track {
            pos.note = format!("No track number tag: from the file's name, {}", q(&stem));
        } else if let Some(raw) = &s.track_raw {
            pos.note = format!("The track number tag is {}, not a number (a vinyl side and position?): the album orders it by file name", q(raw));
        }
    }
    v.push(pos);
    if let Some(a) = &t.album {
        let (how, detail) = grouped_by(&a.key);
        let mut g = Line::field("Album grouped by", how).with_note(detail);
        if let (Some(s), true) = (scanned, !a.key.is_empty())
            && !same_album(&a.key, s, path)
        {
            g = if drafted.is_some() {
                g.with_note("Once saved, this file belongs to another album: the library moves it there (its plays, favourites and playlists go with it)")
            } else {
                g.with_note("By its tags now, this file belongs to another album; the next scan moves it").attention(true)
            };
        }
        v.push(g);
    }
    // A draft's consequences are marked as such, not as something to look into.
    if drafted.is_some() {
        for l in &mut v {
            if l.attention || l.note.starts_with("Once saved") {
                l.attention = false;
                l.changed = l.note.starts_with("Once saved");
            }
        }
    }
    v
}

/// What the scanner will read from a file once the draft is saved: its tags as read now,
/// with the draft's values (the scanner's own rules, `library::tags`).
pub fn with_draft(s: &TrackTags, d: &metadata::tagedit::FileDraft, path: &Path) -> TrackTags {
    let mut t = s.clone();
    let get = |k: &str| d.fields.get(k).map(|c| c.new.iter().map(|v| names::tidy(v)).filter(|v| !v.is_empty()).collect::<Vec<_>>());
    let joined = |v: &[String]| if v.is_empty() { None } else { Some(v.join("; ")) };
    if let Some(v) = get("TITLE") {
        t.title = v.first().cloned();
        t.from_name.title = false;
    }
    if let Some(v) = get("ARTIST") {
        t.artist = joined(&v);
        let mut credits: Vec<names::Credit> = Vec::new();
        for c in v.iter().flat_map(|x| names::credits(x)) {
            names::push_credit(&mut credits, c);
        }
        if let Some(ti) = &t.title {
            for n in names::title_featured(ti) {
                names::push_credit(&mut credits, names::Credit::new(&n, true));
            }
        }
        t.artists = credits;
    }
    if let Some(v) = get("ALBUM") {
        t.album = v.first().cloned();
    }
    if let Some(v) = get("ALBUMARTIST") {
        t.album_artist = joined(&v);
    }
    if let Some(v) = get("DATE") {
        t.date = v.first().map(|x| library::tags::normalize_date(x)).or(t.release_date.clone()).or(t.original_date.clone());
    }
    if let Some(v) = get("GENRE") {
        t.genre = joined(&v);
        t.genres = Vec::new();
        for g in v.iter().flat_map(|x| names::split_genres(x)) {
            if !t.genres.iter().any(|x| names::norm(x) == names::norm(g)) {
                t.genres.push(g.to_owned());
            }
        }
    }
    if let Some(v) = get("TRACKNUMBER") {
        t.track_no = v.first().and_then(|n| names::leading_number(n));
        t.track_raw = None;
        t.from_name.track = false;
    }
    if let Some(v) = get("DISCNUMBER") {
        t.disc_no = v.first().and_then(|n| names::leading_number(n));
        t.from_name.disc = false;
    }
    if let Some(v) = get("COMPILATION") {
        t.compilation = v.first().is_some_and(|f| f == "1");
    }
    if let Some(v) = get("LABEL") {
        t.label = v.first().cloned();
    }
    if t.title.is_none() || t.track_no.is_none() {
        library::tags::apply_file_name(&mut t, path);
    }
    t
}

/// Whether a file's tags, as read now, still put it in the album with this key, as far as
/// one file can tell: its MusicBrainz release, or its album title and folder (who the
/// album is by is decided with all of the folder's tracks).
fn same_album(key: &str, s: &library::tags::TrackTags, path: &Path) -> bool {
    match key.strip_prefix("mb:") {
        Some(id) => s.mb_album_id.as_deref().is_some_and(|m| m.trim().eq_ignore_ascii_case(id)),
        None if s.mb_album_id.as_deref().is_some_and(|m| !m.trim().is_empty()) => false,
        None => {
            let Some((rest, folder)) = key.rsplit_once('|') else { return true };
            let title = rest.split_once('|').map_or("", |(_, t)| t);
            let my_title = s.album.as_deref().map(names::norm).unwrap_or_default();
            let my_folder = library::scan::group_folder(path).to_string_lossy().to_lowercase();
            title == my_title && folder == my_folder
        }
    }
}

/// R128 gains are Q7.8 fixed point: "−4.82 dB".
pub fn r128_db(raw: &str) -> Option<String> {
    let n: i32 = raw.trim().parse().ok()?;
    let db = f64::from(n) / 256.0;
    Some(format!("{}{:.2} dB, relative to the Opus header gain", if db < 0.0 { "\u{2212}" } else { "+" }, db.abs()))
}

/// "3/12" as "3 of 12".
fn of_total(v: &str) -> String {
    match v.split_once('/') {
        Some((n, t)) if !n.trim().is_empty() && !t.trim().is_empty() => format!("{} of {}", n.trim(), t.trim()),
        _ => v.to_owned(),
    }
}

/// Lines for one field: one, or one per block when the blocks disagree.
fn field_lines(f: &Field, blocks: &[BlockKind]) -> Vec<Line> {
    let several = blocks.len() > 1;
    let differs = f.differs();
    let names = |s: &meta::Source| blocks.get(s.block).map(|b| b.name()).unwrap_or_default();
    let make = |s: &meta::Source, note: String| {
        let key = f.def.map(|d| d.key).unwrap_or("");
        let values: Vec<String> = match s.binary {
            Some(n) if s.values.is_empty() => vec![format!("Binary data, {}", details::size_text(n as u64))],
            _ => match key {
                "TRACKNUMBER" | "DISCNUMBER" => s.values.iter().map(|v| of_total(v)).collect(),
                _ => s.values.clone(),
            },
        };
        let mut extra = match key {
            "R128_TRACK_GAIN" | "R128_ALBUM_GAIN" => s.values.first().and_then(|v| r128_db(v)).unwrap_or_default(),
            _ => String::new(),
        };
        if f.def.is_none() && s.values.is_empty() && s.binary.is_none() {
            extra = "Empty".into();
        }
        let note = [extra, note].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
        Line {
            label: f.label(),
            copy: s.values.join("; "),
            values,
            note,
            id: format!("f:{}:{}:{}", f.key(), s.block, s.key),
            field: Some(FieldRef { key: f.key(), block: s.block, raw_key: s.key.clone() }),
            ..Default::default()
        }
    };
    if differs {
        let all: Vec<String> = f.sources.iter().map(names).collect();
        f.sources
            .iter()
            .map(|s| {
                let others: Vec<&String> = all.iter().filter(|n| **n != names(s)).collect();
                let note = if others.is_empty() { format!("{}, twice with different values", names(s)) } else { format!("{}: differs from {}", names(s), others.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")) };
                make(s, note).attention(true)
            })
            .collect()
    } else {
        let note = if several || f.def.is_none() && f.sources[0].key != f.label() {
            f.sources.iter().map(names).collect::<Vec<_>>().join(", ")
        } else {
            String::new()
        };
        vec![make(&f.sources[0], note)]
    }
}

/// The thumbnail index of the draft's new cover (the viewer keeps that image apart).
pub const NEW_COVER: usize = usize::MAX - 1;

/// "“A”, “B”", or "nothing".
fn quoted(v: &[String]) -> String {
    if v.is_empty() { "nothing".into() } else { v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", ") }
}

/// A field's lines with tag editing on: as `field_lines`, each saying what editing it does;
/// one line with the new values (marked, with what it was) when the draft changes it; one
/// "Not set" line for a common field the file does not have.
fn edit_lines(f: &Field, blocks: &[BlockKind], fields: &[Field], draft: Option<&metadata::tagedit::FileDraft>) -> Vec<Line> {
    let key = f.key();
    let edit = match f.def.map(|d| library::edit::editability(d.key)) {
        Some(library::edit::Editability::Edit) => Some(Edit::Field(key.clone())),
        Some(library::edit::Editability::Remove) => Some(Edit::Remove(key.clone())),
        _ => None,
    };
    let multi = library::edit::multi_valued(&key);
    let base = |values: Vec<String>| Line {
        label: f.label(),
        copy: values.join("; "),
        empty: values.is_empty(),
        values,
        id: format!("e:{key}"),
        edit: edit.clone(),
        multi,
        field: f.sources.first().map(|s| FieldRef { key: key.clone(), block: s.block, raw_key: s.key.clone() }),
        ..Default::default()
    };
    if let Some(c) = draft.and_then(|d| d.fields.get(&key)) {
        let mut l = base(c.new.clone());
        l.changed = true;
        l.note = match (c.old.is_empty(), c.new.is_empty()) {
            (_, true) => format!("Removed when you save (it was {})", quoted(&c.old)),
            (true, false) => "Not set before: added when you save".into(),
            _ => format!("Was {}", quoted(&c.old)),
        };
        return vec![l];
    }
    if f.sources.is_empty() {
        // A common field the file does not have (or holds within another: a total in "3/12").
        let mut l = base(metadata::tagedit::values_of(fields, &key));
        if !l.empty {
            l.note = "Held with the number".into();
        }
        return vec![l];
    }
    let mut lines = field_lines(f, blocks);
    for l in &mut lines {
        l.edit = edit.clone();
        l.multi = multi;
        if edit.is_some() && f.sources.len() > 1 && f.differs() {
            l.note.push_str(" \u{b7} Editing it sets every tag block of the file to the same value");
        }
    }
    lines
}

/// The Edit history: saves that wrote these files, newest first.
fn history_lines(e: &EditView) -> Vec<Line> {
    let mut v = vec![Line::heading("Edit history")];
    if e.history.is_empty() {
        v.push(Line::note("No saves of these files are kept. Every save you make here can be undone from this list while it is kept (Settings > Library).", false));
    }
    for (i, (when, what, n, undone)) in e.history.iter().enumerate() {
        let mut l = Line::field(when, what.clone());
        l.id = format!("h:{i}");
        if *undone {
            l.note = "Undone".into();
        } else {
            l.note = if *n == 1 { "Undo\u{2026}: puts back what it changed, if the file is still as it left it".into() } else { format!("Undo\u{2026}: puts back what it changed in these {n} files, if they are still as it left them") };
            l.act = Act::Undo(i);
        }
        v.push(l);
    }
    v
}

/// "1,234 characters · 40 lines · synced".
pub fn lyrics_summary(text: &str) -> String {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let synced = text.lines().any(|l| {
        let l = l.trim_start();
        l.len() > 6 && l.starts_with('[') && l.as_bytes()[1].is_ascii_digit() && l[1..].contains(':')
    });
    format!(
        "{} characters \u{b7} {} \u{b7} {}",
        audio::thousands(text.chars().count() as u64),
        if lines == 1 { "1 line".to_owned() } else { format!("{lines} lines") },
        if synced { "synced (time stamps)" } else { "plain text" }
    )
}

/// "PNG · 1,200 × 1,200 px · 23.7 KB".
pub fn picture_text(format: &str, size: Option<(u32, u32)>, bytes: u64) -> String {
    let mut parts = vec![format.to_owned()];
    if let Some((w, h)) = size {
        parts.push(format!("{} \u{d7} {} px", audio::thousands(u64::from(w)), audio::thousands(u64::from(h))));
    }
    parts.push(short_size(bytes));
    parts.join(" \u{b7} ")
}

/// "23.7 KB", "4.1 MB".
pub fn short_size(bytes: u64) -> String {
    let full = details::size_text(bytes);
    full.split(" (").next().unwrap_or(&full).to_owned()
}

/// Every line of a track's view.
pub fn track_lines(t: &TrackInput) -> Vec<Line> {
    let mut v = vec![Line::heading("In the library")];
    if t.track.id > 0 {
        v.extend(library_lines(t));
    } else {
        v.push(Line::note("Not in the library: a file opened from outside it.", false));
    }
    let read = match &t.file {
        FileRead::Pending => {
            v.push(Line::heading("Tags"));
            v.push(Line::note("Reading the file\u{2026}", false));
            None
        }
        FileRead::Missing => {
            v.push(Line::heading("Tags"));
            v.push(Line::note("The file is not there: it was moved or deleted after the last scan, or its drive is not connected. What the library knows is above.", true));
            None
        }
        FileRead::Unreadable(e) => {
            v.push(Line::heading("Tags"));
            v.push(Line::note(format!("The file could not be read: {e}"), true));
            None
        }
        FileRead::Server => {
            v.push(Line::heading("From the server"));
            v.push(Line::note(FROM_SERVER, false));
            None
        }
        FileRead::Read(r) => Some(r.as_ref()),
    };
    if let Some(r) = read {
        let blocks = &r.tags.blocks;
        // Tag editing: on, and this file can be edited.
        let edit = t.edit.as_ref().filter(|_| r.editable.is_ok());
        let draft = edit.and_then(|e| e.draft.file(&t.track.path));
        if let (Some(_), Err(why)) = (&t.edit, &r.editable) {
            v.push(Line::heading("Tags"));
            v.push(Line::note(format!("This file cannot be edited: {why}."), false));
        }
        if r.fields.is_empty() && edit.is_none() {
            v.push(Line::heading("Tags"));
            v.push(Line::note("This file has no tags.", false));
        }
        for g in [Group::Main, Group::Ids, Group::Sort, Group::Loudness, Group::Encoding] {
            let mut fields: Vec<Field> = r.fields.iter().filter(|f| f.group() == g).cloned().collect();
            if let Some(e) = edit {
                // The fields people fix most, there or not (all of them on request), and
                // any the draft adds.
                let mut keys: Vec<&str> = match g {
                    Group::Main if e.all_fields => meta::FIELDS.iter().filter(|d| d.group == g && library::edit::editability(d.key) == library::edit::Editability::Edit).map(|d| d.key).collect(),
                    Group::Main => library::edit::COMMON.to_vec(),
                    Group::Sort if e.all_fields => meta::FIELDS.iter().filter(|d| d.group == g).map(|d| d.key).collect(),
                    _ => Vec::new(),
                };
                keys.extend(draft.into_iter().flat_map(|d| d.fields.keys()).filter(|k| meta::by_key(k).is_some_and(|d| d.group == g)).map(String::as_str));
                for k in keys {
                    if !fields.iter().any(|f| f.def.is_some_and(|d| d.key == k)) {
                        fields.push(Field { def: meta::by_key(k), sources: Vec::new() });
                    }
                }
                fields.sort_by_key(|f| f.def.and_then(|d| meta::FIELDS.iter().position(|x| std::ptr::eq(x, d))).unwrap_or(usize::MAX));
            }
            let mut first = true;
            for f in &fields {
                if first {
                    v.push(Line::heading(if g == Group::Main && fields.is_empty() { "Tags" } else { g.title() }));
                    first = false;
                }
                match edit {
                    Some(_) => v.extend(edit_lines(f, blocks, &r.fields, draft)),
                    None => v.extend(field_lines(f, blocks)),
                }
            }
            if g == Group::Main && edit.is_some_and(|e| !e.all_fields) {
                v.push(Line::action("", "Show every field you can edit", Act::AllFields));
            }
        }
        // Pictures.
        v.push(Line::heading("Pictures"));
        let new_cover = draft.and_then(|d| d.cover.as_ref());
        if r.tags.pictures.is_empty() && new_cover.is_none() {
            v.push(Line::note("No embedded pictures.", false));
        }
        for (i, p) in r.tags.pictures.iter().enumerate() {
            let mut note = Vec::new();
            if !p.description.is_empty() {
                note.push(q(&p.description));
            }
            if blocks.len() > 1 {
                note.push(blocks[p.block].name());
            }
            let front = p.front || blocks.get(p.block) == Some(&BlockKind::Mp4);
            let going = new_cover.is_some() && front;
            if going {
                note.push(if matches!(new_cover, Some(metadata::tagedit::Cover::Remove)) { "Removed when you save".into() } else { "Replaced when you save".into() });
            }
            let text = picture_text(&p.format, p.size, p.bytes as u64);
            v.push(Line {
                kind: Kind::Picture,
                label: p.kind.clone(),
                copy: text.clone(),
                values: vec![text],
                note: note.join(" \u{b7} "),
                thumb: p.thumb.as_ref().map(|_| i),
                id: format!("p:{i}"),
                changed: going,
                ..Default::default()
            });
        }
        if let Some(e) = edit {
            let has_front = r.tags.pictures.iter().any(|p| p.front || blocks.get(p.block) == Some(&BlockKind::Mp4));
            match new_cover {
                Some(metadata::tagedit::Cover::Set(p)) => {
                    let (text, from) = p.describe();
                    v.push(Line {
                        kind: Kind::Picture,
                        label: "New front cover".into(),
                        values: vec![text],
                        note: [from.unwrap_or_default(), "Written when you save".into()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} "),
                        thumb: Some(NEW_COVER),
                        changed: true,
                        id: "p:new".into(),
                        ..Default::default()
                    });
                    v.push(Line::action("", "Keep the cover as it is", Act::Cover(CoverAct::Keep)));
                }
                Some(metadata::tagedit::Cover::Remove) => v.push(Line::action("", "Keep the cover as it is", Act::Cover(CoverAct::Keep))),
                None => {
                    v.push(Line::action("", if has_front { "Replace the front cover\u{2026}" } else { "Add a front cover\u{2026}" }, Act::Cover(CoverAct::Pick)));
                    if has_front {
                        v.push(Line::action("", "Remove the front cover", Act::Cover(CoverAct::Remove)));
                    }
                }
            }
            let _ = e;
        }
        // Lyrics: present or not, how long; the text only on Copy.
        v.push(Line::heading("Lyrics"));
        let lyric_fields: Vec<&Field> = r.fields.iter().filter(|f| f.group() == Group::Lyrics).collect();
        let lrc = r.lrc.as_ref().map(|name| format!("The app shows {} beside the file instead", q(name)));
        if lyric_fields.is_empty() {
            let mut l = Line::field("Lyrics", "None in the tags");
            l.copy.clear();
            if let Some(name) = &r.lrc {
                l.note = format!("The app shows {} beside the file", q(name));
            }
            v.push(l);
        }
        for f in lyric_fields {
            for s in &f.sources {
                let text = s.values.join("\n");
                let summary = if s.binary.is_some() && s.values.is_empty() { format!("Binary data, {}", details::size_text(s.binary.unwrap_or(0) as u64)) } else { lyrics_summary(&text) };
                let mut note = vec![blocks.get(s.block).map(|b| b.name()).unwrap_or_default() + " " + &s.key];
                note.extend(lrc.clone());
                v.push(Line {
                    label: f.label(),
                    values: vec![summary],
                    copy: text,
                    note: note.join(" \u{b7} "),
                    id: format!("y:{}:{}", s.block, s.key),
                    field: Some(FieldRef { key: f.key(), block: s.block, raw_key: s.key.clone() }),
                    ..Default::default()
                });
            }
        }
        if r.tags.dropped > 0 {
            v.push(Line::note(format!("{} more items are in the file than are read here.", audio::thousands(r.tags.dropped as u64)), true));
        }
    }
    if let Some(e) = &t.edit
        && matches!(t.file, FileRead::Read(_))
    {
        v.extend(history_lines(e));
    }
    // The file: its details, size and history (what the Properties panel showed). A track only
    // the server has: the server's file, as its catalog describes it, with no path here.
    let remote = matches!(t.file, FileRead::Server);
    v.push(Line::heading(if remote { "The server's file" } else { "File" }));
    if let Some(why) = audio::unplayable_format(&t.track.audio, None) {
        let what = if remote { "The server's file cannot be played here" } else { "This file cannot be played" };
        v.push(Line::note(format!("{what}: {why}."), true));
    }
    if !remote {
        v.push(Line::field("Path", t.track.path.clone()));
    }
    let known = Known {
        report: read.and_then(|r| r.report.clone()),
        size: read.and_then(|r| r.size),
        extras: t.extras.clone(),
        stream: t.stream.clone(),
    };
    for (label, value) in details::property_rows(&t.track, &known) {
        let mut l = Line::field(&label, value.clone());
        l.values = value.split('\n').map(str::to_owned).collect();
        v.push(l);
    }
    // Everything else, under its raw key, last: it can run to thousands.
    if let Some(r) = read {
        let other: Vec<&Field> = r.fields.iter().filter(|f| f.group() == Group::Other).collect();
        if !other.is_empty() {
            v.push(Line::heading(&format!("{} ({})", Group::Other.title(), audio::thousands(other.len() as u64))));
            let limit = t.other_limit.max(PAGE_ROWS);
            for f in other.iter().take(limit) {
                v.extend(field_lines(f, &r.tags.blocks));
            }
            if other.len() > limit {
                let mut more = Line::note(more_text(other.len() - limit), false);
                more.act = Act::More(Section::OtherTags);
                v.push(more);
            }
        }
    }
    v
}

/// "Show the other 100", "Show 300 more (2,700 not listed yet)".
fn more_text(rest: usize) -> String {
    if rest <= PAGE_ROWS {
        format!("Show the other {}", audio::thousands(rest as u64))
    } else {
        format!("Show {PAGE_ROWS} more ({} not listed yet)", audio::thousands(rest as u64))
    }
}

/// Everything not listed (Other tags past the limit) still goes into Copy all.
pub fn track_copy_lines(t: &TrackInput) -> Vec<Line> {
    track_lines(&TrackInput { other_limit: usize::MAX, ..t.clone() })
}

// ---- An album ------------------------------------------------------------------------

/// One track of an album's table, as far as known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackFacts {
    pub disc: Option<u32>,
    pub number: Option<u32>,
    pub audio: AudioInfo,
    pub file: FileBrief,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum FileBrief {
    #[default]
    Pending,
    Missing,
    Unreadable(String),
    Read(Brief),
    /// Only on the server: no file here (nothing to read, check or edit).
    Server,
}

/// What an album's tracks disagree on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Checks {
    /// Per track: what is off about it.
    pub rows: Vec<Vec<String>>,
    pub number: Vec<bool>,
    pub format: Vec<bool>,
    /// (what, how): "Album", "“Patchwork” on 5 tracks, “Patchwork (Deluxe)” on 1".
    pub summary: Vec<(String, String)>,
}

fn tracks_n(n: usize) -> String {
    if n == 1 { "1 track".into() } else { format!("{n} tracks") }
}

/// The values of one tag across the album: the most common (ties: the first seen) is the
/// album's; the others are flagged on their rows.
fn spread(c: &mut Checks, what: &str, label_missing: &str, values: &[(usize, Option<String>)]) {
    let mut counts: Vec<(Option<String>, usize)> = Vec::new();
    for (_, v) in values {
        match counts.iter_mut().find(|(x, _)| x == v) {
            Some(e) => e.1 += 1,
            None => counts.push((v.clone(), 1)),
        }
    }
    if counts.len() < 2 {
        return;
    }
    let top = counts.iter().enumerate().max_by_key(|(i, (_, n))| (*n, std::cmp::Reverse(*i))).map(|(_, (v, _))| v.clone()).unwrap_or_default();
    let show = |v: &Option<String>| v.as_deref().map(q).unwrap_or_else(|| "none".into());
    c.summary.push((what.to_owned(), counts.iter().map(|(v, n)| format!("{} on {}", show(v), tracks_n(*n))).collect::<Vec<_>>().join(", ")));
    for (i, v) in values {
        if *v != top {
            c.rows[*i].push(match v {
                Some(v) => format!("{what} {}", q(v)),
                None => label_missing.to_owned(),
            });
        }
    }
}

pub fn check_album(tracks: &[TrackFacts]) -> Checks {
    let n = tracks.len();
    let mut c = Checks { rows: vec![Vec::new(); n], number: vec![false; n], format: vec![false; n], summary: Vec::new() };
    // The files themselves.
    let missing = tracks.iter().filter(|t| t.file == FileBrief::Missing).count();
    let unreadable = tracks.iter().filter(|t| matches!(t.file, FileBrief::Unreadable(_))).count();
    for (i, t) in tracks.iter().enumerate() {
        match &t.file {
            FileBrief::Missing => c.rows[i].push("File missing".into()),
            FileBrief::Unreadable(e) => c.rows[i].push(format!("Could not be read ({e})")),
            _ => {}
        }
    }
    if missing + unreadable > 0 {
        let mut parts = Vec::new();
        if missing > 0 {
            parts.push(format!("{} missing", tracks_n(missing)));
        }
        if unreadable > 0 {
            parts.push(format!("{} not readable", tracks_n(unreadable)));
        }
        c.summary.push(("Files".into(), parts.join(", ")));
    }
    // Tags, over the files read.
    let read: Vec<(usize, &Brief)> = tracks.iter().enumerate().filter_map(|(i, t)| if let FileBrief::Read(b) = &t.file { Some((i, b)) } else { None }).collect();
    let col = |f: fn(&Brief) -> Option<String>| read.iter().map(|(i, b)| (*i, f(b))).collect::<Vec<_>>();
    spread(&mut c, "Album", "No album tag", &col(|b| b.album.clone()));
    spread(&mut c, "Album artist", "No album artist tag", &col(|b| b.album_artist.clone()));
    spread(&mut c, "Date", "No date", &col(|b| b.date.clone()));
    spread(&mut c, "Genre", "No genre", &col(|b| b.genre.clone()));
    spread(&mut c, "MusicBrainz release", "No MusicBrainz release id", &col(|b| b.mb_album.clone()));
    let with_rg = read.iter().filter(|(_, b)| b.replaygain).count();
    if with_rg > 0 && with_rg < read.len() {
        c.summary.push(("ReplayGain".into(), format!("Missing on {} of {}", read.len() - with_rg, tracks_n(read.len()))));
        for (i, b) in &read {
            if !b.replaygain {
                c.rows[*i].push("No ReplayGain".into());
            }
        }
    }
    // Numbers, per disc.
    let mut discs: HashMap<u32, Vec<(usize, u32)>> = HashMap::new();
    let mut unnumbered = 0;
    for (i, t) in tracks.iter().enumerate() {
        match t.number {
            Some(no) => discs.entry(t.disc.unwrap_or(1)).or_default().push((i, no)),
            None => {
                unnumbered += 1;
                c.number[i] = true;
                c.rows[i].push("No track number".into());
            }
        }
    }
    let several_discs = discs.len() > 1;
    let mut keys: Vec<u32> = discs.keys().copied().collect();
    keys.sort_unstable();
    let mut gaps = Vec::new();
    let mut twice = Vec::new();
    for d in keys {
        let nums = &discs[&d];
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for (_, no) in nums {
            *seen.entry(*no).or_default() += 1;
        }
        for (i, no) in nums {
            if seen[no] > 1 {
                c.number[*i] = true;
                c.rows[*i].push(format!("Track number {no} used {} times", seen[no]));
            }
        }
        let mut dup: Vec<u32> = seen.iter().filter(|(_, n)| **n > 1).map(|(no, _)| *no).collect();
        dup.sort_unstable();
        twice.extend(dup.iter().map(|no| if several_discs { format!("{no} (disc {d})") } else { no.to_string() }));
        let max = nums.iter().map(|(_, n)| *n).max().unwrap_or(0);
        // Counted, and only the first 12 listed: a tag can say track 4000000000. Every number not
        // missing is one of the tracks, so the listing stops after at most `nums.len() + 12` steps.
        let absent = max as usize - seen.keys().filter(|&&k| (1..=max).contains(&k)).count();
        if absent > 0 {
            let list = (1..=max).filter(|k| !seen.contains_key(k)).take(12).map(|k| k.to_string()).collect::<Vec<_>>().join(", ") + if absent > 12 { ", \u{2026}" } else { "" };
            gaps.push(if several_discs { format!("{list} on disc {d}") } else { list });
        }
    }
    let mut numbering = Vec::new();
    if !gaps.is_empty() {
        numbering.push(format!("No track {}", gaps.join("; ")));
    }
    if !twice.is_empty() {
        numbering.push(format!("used twice: {}", twice.join(", ")));
    }
    if unnumbered > 0 {
        numbering.push(format!("{} without a number", tracks_n(unnumbered)));
    }
    if !numbering.is_empty() {
        let s = numbering.join("; ");
        c.summary.push(("Track numbers".into(), s[..1].to_uppercase() + &s[1..]));
    }
    let with_disc = tracks.iter().filter(|t| t.disc.is_some()).count();
    if with_disc > 0 && with_disc < n {
        c.summary.push(("Disc numbers".into(), format!("Missing on {} of {}", n - with_disc, tracks_n(n))));
        for (i, t) in tracks.iter().enumerate() {
            if t.disc.is_none() {
                c.rows[i].push("No disc number".into());
            }
        }
    }
    // Formats.
    let mut formats: Vec<(FormatKey, String, Vec<usize>)> = Vec::new();
    for (i, t) in tracks.iter().enumerate().filter(|(_, t)| t.audio.is_known()) {
        let k = FormatKey::of(&t.audio);
        match formats.iter_mut().find(|(x, _, _)| *x == k) {
            Some(e) => e.2.push(i),
            None => formats.push((k, format_name(&t.audio), vec![i])),
        }
    }
    if formats.len() > 1 {
        formats.sort_by_key(|(_, _, v)| std::cmp::Reverse(v.len()));
        c.summary.push(("Formats".into(), formats.iter().map(|(_, name, v)| format!("{name} on {}", tracks_n(v.len()))).collect::<Vec<_>>().join(", ")));
        for (_, _, v) in formats.iter().skip(1) {
            for i in v {
                c.format[*i] = true;
            }
        }
    }
    c
}

/// A format without its bitrate ("MP3", "FLAC 16/44.1"): lossy tracks of one codec are one
/// format, as on the album page.
fn format_name(a: &AudioInfo) -> String {
    if a.codec.lossless() { audio::compact(a) } else { a.codec.name().to_owned() }
}

/// The album's cover and where it comes from, as found now.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CoverFound {
    #[default]
    Pending,
    /// Embedded in this file (its name), with its picture type, format and size.
    Embedded { file: String, kind: String, text: String },
    /// An image file in the album's folder (its name).
    Folder { file: String, text: String },
    None { first: Option<String> },
}

/// Everything an album's view is made from.
#[derive(Clone, Debug, Default)]
pub struct AlbumInput {
    pub facts: AlbumFacts,
    pub tracks: Vec<Track>,
    pub files: Vec<FileBrief>,
    pub cover: CoverFound,
    pub has_thumb: bool,
    pub table_limit: usize,
    pub edit: Option<EditView>,
    /// Tag editing: per track, what its file holds of the fields an album edits, and the
    /// file as read (read only with editing on).
    pub briefs: Vec<Option<EditBrief>>,
}

/// One track's file as album editing needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct EditBrief {
    /// Field key -> values, as an editor starts from them (`metadata::tagedit::values_of`).
    pub values: std::collections::BTreeMap<String, Vec<String>>,
    /// Front covers embedded.
    pub fronts: usize,
    pub expect: Option<library::edit::Expect>,
    pub editable: Result<(), String>,
}

/// The fields album editing reads from each file.
pub const BRIEF_KEYS: &[&str] = &["TITLE", "ARTIST", "ALBUM", "ALBUMARTIST", "TRACKNUMBER", "DISCNUMBER", "DATE", "GENRE", "LABEL", "DISCTOTAL", "COMPILATION"];

/// An inconsistency's obvious fix, offered as a draft to review.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub label: String,
    pub key: String,
    /// (track index, new values).
    pub tracks: Vec<(usize, Vec<String>)>,
}

fn length_text(secs: f64) -> String {
    if secs >= 3600.0 {
        format!("{} h {} min", (secs / 3600.0) as u64, ((secs % 3600.0) / 60.0) as u64)
    } else {
        crate::rows::format_time(secs)
    }
}

/// The value most of the files read say (ties: the first seen), and on how many.
fn majority(values: impl Iterator<Item = Option<String>>) -> Option<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for v in values.flatten() {
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some(e) => e.1 += 1,
            None => counts.push((v, 1)),
        }
    }
    counts.into_iter().enumerate().max_by_key(|(i, (_, n))| (*n, std::cmp::Reverse(*i))).map(|(_, c)| c)
}

pub fn album_lines(a: &AlbumInput) -> Vec<Line> {
    let f = &a.facts;
    let mut v = vec![Line::heading("Album")];
    let opt = |label: &str, value: &Option<String>| value.as_ref().map(|x| Line::field(label, x.clone()));
    // The album's values are the most common among its tracks (scan::album_values): say so
    // where its files now mostly say something else (changed since the last scan).
    let briefs: Vec<&Brief> = a.files.iter().filter_map(|b| if let FileBrief::Read(b) = b { Some(b) } else { None }).collect();
    let most = |stored: Option<&str>, pick: fn(&Brief) -> Option<String>| -> Option<String> {
        let (top, n) = majority(briefs.iter().map(|b| pick(b)))?;
        let same = |x: &str| stored.is_some_and(|s| names::norm(s) == names::norm(x));
        let stored_n = briefs.iter().filter(|b| pick(b).is_some_and(|x| same(&x))).count();
        (!same(&top) && n > stored_n).then(|| format!("{n} of its {} now say {}; the next scan takes it", tracks_n(briefs.len()), q(&top)))
    };
    let mark = |mut l: Line, note: Option<String>| {
        if let Some(n) = note {
            l.note = n;
            l.attention = true;
        }
        l
    };
    v.push(mark(Line::field("Title", f.title.clone()), most(Some(&f.title), |b| b.album.clone())));
    let mut aa = Line::field(if f.artists.len() > 1 { "Album artists" } else { "Album artist" }, f.album_artist.clone());
    if f.compilation {
        aa.note = "A compilation: its tracks name different artists without an album artist tag, or are flagged as one".into();
    } else if f.artists.len() > 1 {
        aa.note = format!("{}: the album is on each one's page", f.artists.join(", "));
    } else if !briefs.is_empty() && briefs.iter().all(|b| b.album_artist.is_none()) {
        aa.note = "No album artist tag: its tracks' artist".into();
    }
    v.push(if f.compilation { aa } else { mark(aa, most(Some(&f.album_artist), |b| b.album_artist.clone())) });
    v.extend(opt("Album artist sort", &f.album_artist_sort));
    v.push(mark(f.date.as_ref().map_or_else(|| Line::field("Date", "None"), |d| Line::field("Date", d.clone())), most(f.date.as_deref(), |b| b.date.clone())));
    v.extend(opt("Original date", &f.original_date));
    v.extend(opt("Release date", &f.release_date));
    if f.genres.len() > 1 {
        let mut g = Line::field("Genres", f.genres.join(", ")).with_note("Every genre of its tracks, the most common first: the album is under each");
        g.values = f.genres.clone();
        v.push(g);
    } else {
        v.extend(opt("Genre", &f.genre).map(|l| mark(l, most(f.genre.as_deref(), |b| b.genre.clone()))));
    }
    v.extend(opt("Label", &f.label));
    v.extend(opt("Release type", &f.release_type));
    if let Some(e) = &a.edit {
        album_edit_lines(a, e, &mut v);
    }

    v.push(Line::heading("In the library"));
    let (how, detail) = grouped_by(&f.key);
    v.push(Line::field("Grouped by", how).with_note(detail.clone()));
    if library::remote::is_remote(&f.folder) {
        // An album only the server has: no folder here.
        v.push(Line::field("Source", "From the server").with_note("No folder here: its tracks are the server's files"));
    } else {
        v.push(Line::field("Folder", f.folder.clone()));
    }
    let added = details::date_text(f.added_at / 1000);
    let mut added_line = Line::field("Added", added);
    if f.added_source.as_deref() == Some("navidrome") {
        added_line.note = format!("Imported from Navidrome; first seen here {}", details::date_text(f.first_seen / 1000));
    }
    v.push(added_line);
    v.push(Line::field("Favourite", f.favourite_since.map_or_else(|| "No".to_owned(), |t| format!("Yes, since {}", details::date_text(t)))));
    v.push(Line::field(
        "Plays",
        match (f.plays, f.last_played) {
            (0, _) => "Never played".to_owned(),
            (n, Some(last)) => format!("{} \u{b7} last {}", if n == 1 { "1 play".into() } else { format!("{} plays", audio::thousands(n as u64)) }, details::date_text(last)),
            (n, None) => format!("{n} plays"),
        },
    ));
    let discs: HashSet<u32> = a.tracks.iter().map(|t| t.disc_no.unwrap_or(1)).collect();
    v.push(Line::field("Tracks", if discs.len() > 1 { format!("{} on {} discs", tracks_n(a.tracks.len()), discs.len()) } else { tracks_n(a.tracks.len()) }));
    v.push(Line::field("Length", length_text(a.tracks.iter().map(|t| t.duration).sum())));
    v.push(Line::field("Size", details::size_text(f.size.max(0) as u64)));
    let summary = audio::album_summary(&a.tracks.iter().map(|t| (t.audio, t.duration)).collect::<Vec<_>>());
    v.push(Line::field("Format", if summary.line.is_empty() { "Not read yet".into() } else { summary.line }));

    v.push(Line::heading("Cover"));
    let cover = match &a.cover {
        CoverFound::Pending => Line::field("Cover", "Looking\u{2026}"),
        CoverFound::Embedded { file, kind, text } => {
            let mut l = Line::field("Cover", text.clone()).with_note(format!("{kind}, embedded in {}", q(file)));
            l.copy = format!("{text}, {kind} embedded in {file}");
            l
        }
        CoverFound::Folder { file, text } => {
            let mut l = Line::field("Cover", text.clone()).with_note(format!("{} in the album's folder", q(file)));
            l.copy = format!("{text}, {file} in the album's folder");
            l
        }
        CoverFound::None { first } => Line::field("Cover", "None found").with_note(match first {
            Some(f) => format!("No picture in {} and no image in the folder", q(f)),
            None => "No image in the folder".into(),
        }),
    };
    v.push(Line { kind: Kind::Picture, thumb: a.has_thumb.then_some(0), ..cover });
    if a.edit.is_some() && !a.files.contains(&FileBrief::Pending) {
        let tracks = editable_tracks(a);
        let draft = a.edit.as_ref().map(|e| &e.draft);
        let covers: Vec<Option<&metadata::tagedit::Cover>> = tracks.iter().map(|i| draft.and_then(|d| d.file(&a.tracks[*i].path)).and_then(|f| f.cover.as_ref())).collect();
        let fronts = tracks.iter().filter(|i| a.briefs[**i].as_ref().is_some_and(|b| b.fronts > 0)).count();
        let n = tracks.len();
        match covers.iter().flatten().next() {
            Some(metadata::tagedit::Cover::Set(p)) => {
                let (text, from) = p.describe();
                v.push(Line {
                    kind: Kind::Picture,
                    label: "New front cover".into(),
                    values: vec![text],
                    note: [from.unwrap_or_default(), format!("Embedded in {} when you save, in place of their front covers", tracks_n(covers.iter().flatten().count()))].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} "),
                    thumb: Some(NEW_COVER),
                    changed: true,
                    id: "p:new".into(),
                    ..Default::default()
                });
                v.push(Line::action("", "Keep the covers as they are", Act::Cover(CoverAct::Keep)));
            }
            Some(metadata::tagedit::Cover::Remove) => {
                let mut l = Line::note(format!("The front cover is removed from {} when you save", tracks_n(covers.iter().flatten().count())), false);
                l.changed = true;
                v.push(l);
                v.push(Line::action("", "Keep the covers as they are", Act::Cover(CoverAct::Keep)));
            }
            None if n > 0 => {
                let what = if fronts > 0 { format!("Replace the front cover of {}\u{2026}", if n == a.tracks.len() { format!("all {}", tracks_n(n)) } else { tracks_n(n) }) } else { format!("Embed a front cover in {}\u{2026}", if n == a.tracks.len() { format!("all {}", tracks_n(n)) } else { tracks_n(n) }) };
                v.push(Line::action("", what, Act::Cover(CoverAct::Pick)));
                if fronts > 0 {
                    v.push(Line::action("", format!("Remove the embedded front cover from {}", tracks_n(fronts)), Act::Cover(CoverAct::Remove)));
                }
            }
            None => {}
        }
    }
    if a.facts.art.is_none() && matches!(a.cover, CoverFound::Embedded { .. } | CoverFound::Folder { .. }) {
        v.push(Line::note("Not shown yet: the next scan of this folder takes it.", false));
    }

    // What the tracks disagree on: with a draft, as they will once it is saved.
    let draft = a.edit.as_ref().map(|e| &e.draft);
    let path_draft = |i: usize| draft.and_then(|d| a.tracks.get(i).and_then(|t| d.file(&t.path)));
    let new_of = |i: usize, k: &str| path_draft(i).and_then(|d| d.fields.get(k)).map(|c| c.new.clone());
    let facts = draft_facts(a);
    let checks = check_album(&facts);
    v.push(Line::heading("Consistency"));
    if draft.is_some_and(|d| !d.is_empty()) {
        v.push(Line::note("As the tracks will be once your changes are saved.", false));
    }
    let pending = facts.iter().filter(|t| t.file == FileBrief::Pending).count();
    let fixes = if a.edit.is_some() && pending == 0 { album_fixes(a, &facts) } else { Vec::new() };
    for (what, how) in &checks.summary {
        v.push(Line::field(what, how.clone()).attention(true));
        for (i, f) in fixes.iter().enumerate().filter(|(_, f)| fix_for(&f.key) == what.as_str()) {
            v.push(Line::action("", f.label.clone(), Act::Fix(i)));
        }
    }
    if pending > 0 {
        v.push(Line::note(format!("Reading the tags of {}\u{2026} {} of {} done", tracks_n(facts.len()), facts.len() - pending, facts.len()), false));
    } else if checks.summary.is_empty() {
        v.push(Line::note("The tracks agree: one album, album artist, date, genre and release; numbered without gaps; one format; ReplayGain on all or none.", false));
    }

    v.push(Line::heading("Tracks"));
    v.push(Line { kind: Kind::TableHead, cells: ["Disc", "#", "Title", "Artist", "Length", "Format"].map(str::to_owned).to_vec(), ..Default::default() });
    let several_discs = facts.iter().map(|t| t.disc.unwrap_or(1)).collect::<HashSet<u32>>().len() > 1;
    let limit = a.table_limit.max(PAGE_ROWS);
    let editable = |i: usize| a.edit.is_some() && a.briefs.get(i).and_then(Option::as_ref).is_some_and(|b| b.editable.is_ok());
    for (i, t) in a.tracks.iter().enumerate().take(limit) {
        let issues = checks.rows[i].join(" \u{b7} ");
        let (number, title, artist) = (new_of(i, "TRACKNUMBER"), new_of(i, "TITLE"), new_of(i, "ARTIST"));
        let disc = facts[i].disc;
        let mut was = Vec::new();
        if number.is_some() {
            was.push(t.track_no.map_or_else(|| "no number".to_owned(), |n| format!("number {n}")));
        }
        if title.is_some() {
            was.push(format!("title {}", q(&t.title)));
        }
        if artist.is_some() {
            was.push(format!("artist {}", q(&t.artist)));
        }
        if new_of(i, "DISCNUMBER").is_some() {
            was.push(t.disc_no.map_or_else(|| "no disc number".to_owned(), |n| format!("disc {n}")));
        }
        let changed_note = if was.is_empty() { String::new() } else { format!("Before: {}", was.join(", ")) };
        let note = [changed_note, issues.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
        v.push(Line {
            kind: Kind::Track,
            label: title.as_ref().and_then(|v| v.first().cloned()).unwrap_or_else(|| t.title.clone()),
            cells: vec![
                if several_discs { disc.map(|d| d.to_string()).unwrap_or_else(|| "\u{2013}".into()) } else { disc.map(|d| d.to_string()).unwrap_or_default() },
                facts[i].number.map(|n| n.to_string()).unwrap_or_else(|| "\u{2013}".into()),
                title.map(|v| v.join("; ")).unwrap_or_else(|| t.title.clone()),
                artist.map(|v| v.join("; ")).unwrap_or_else(|| t.artist.clone()),
                crate::rows::format_time(t.duration),
                if t.audio.is_known() { audio::compact(&t.audio) } else { String::new() },
            ],
            note,
            attention: !issues.is_empty(),
            number_attention: checks.number[i],
            format_attention: checks.format[i],
            copy: t.path.clone(),
            act: Act::OpenTrack(i),
            id: format!("t:{}", t.id),
            edit: editable(i).then_some(Edit::Row(i)),
            changed: !was.is_empty(),
            cell_changed: [new_of(i, "TRACKNUMBER").is_some(), new_of(i, "TITLE").is_some(), new_of(i, "ARTIST").is_some()],
            ..Default::default()
        });
    }
    if a.tracks.len() > limit {
        let mut more = Line::note(more_text(a.tracks.len() - limit), false);
        more.act = Act::More(Section::Tracks);
        v.push(more);
    }
    if let Some(e) = &a.edit {
        v.extend(history_lines(e));
    }
    v
}

/// The tracks an album edit writes to: those whose files were read and can be edited.
pub fn editable_tracks(a: &AlbumInput) -> Vec<usize> {
    (0..a.tracks.len()).filter(|i| a.briefs.get(*i).and_then(Option::as_ref).is_some_and(|b| b.editable.is_ok())).collect()
}

/// An album field's value across its tracks: what most of them hold (with the draft),
/// how many tracks the draft changes it on, and what it was on those.
pub fn album_value(a: &AlbumInput, key: &str) -> (Vec<String>, usize, Vec<String>) {
    let draft = a.edit.as_ref().map(|e| &e.draft);
    let mut counts: Vec<(Vec<String>, usize)> = Vec::new();
    let mut changed = 0;
    let mut was: Vec<(Vec<String>, usize)> = Vec::new();
    for (i, t) in a.tracks.iter().enumerate() {
        let Some(b) = a.briefs.get(i).and_then(Option::as_ref) else { continue };
        let now = b.values.get(key).cloned().unwrap_or_default();
        let value = match draft.and_then(|d| d.field(&t.path, key)) {
            Some(c) => {
                changed += 1;
                match was.iter_mut().find(|(v, _)| *v == now) {
                    Some(e) => e.1 += 1,
                    None => was.push((now, 1)),
                }
                c.new.clone()
            }
            None => now,
        };
        match counts.iter_mut().find(|(v, _)| *v == value) {
            Some(e) => e.1 += 1,
            None => counts.push((value, 1)),
        }
    }
    let top = |c: &[(Vec<String>, usize)]| c.iter().enumerate().max_by_key(|(i, (_, n))| (*n, std::cmp::Reverse(*i))).map(|(_, (v, _))| v.clone()).unwrap_or_default();
    (top(&counts), changed, top(&was))
}

/// The album's fields with tag editing on: each edits every track's file.
fn album_edit_lines(a: &AlbumInput, _e: &EditView, v: &mut Vec<Line>) {
    let start = v.iter().position(|l| l.kind == Kind::Heading && l.label == "Album").map_or(0, |i| i + 1);
    let n = editable_tracks(a).len();
    let pending = a.files.contains(&FileBrief::Pending);
    let key_of = |label: &str| match label {
        "Title" => Some("ALBUM"),
        "Album artist" | "Album artists" => Some("ALBUMARTIST"),
        "Date" => Some("DATE"),
        "Genre" | "Genres" => Some("GENRE"),
        "Label" => Some("LABEL"),
        _ => None,
    };
    // Rows the album has no value for yet: there to be filled in.
    for (key, label, after) in [("GENRE", "Genre", "Release date"), ("LABEL", "Label", "Genre"), ("DISCTOTAL", "Total discs", "Label"), ("COMPILATION", "Compilation", "Total discs")] {
        let present = v[start..].iter().take_while(|l| l.kind != Kind::Heading).any(|l| key_of(&l.label) == Some(key) || l.label == label);
        if !present {
            let at = v[start..].iter().take_while(|l| l.kind != Kind::Heading).position(|l| l.label == after || key_of(&l.label) == key_of(after)).map_or(start + v[start..].iter().take_while(|l| l.kind != Kind::Heading).count(), |p| start + p + 1);
            v.insert(at, Line { label: label.into(), id: format!("l:{label}"), ..Default::default() });
        }
    }
    let end = start + v[start..].iter().take_while(|l| l.kind != Kind::Heading).count();
    for l in &mut v[start..end] {
        let key = match key_of(&l.label) {
            Some(k) => k,
            None if l.label == "Total discs" => "DISCTOTAL",
            None if l.label == "Compilation" => "COMPILATION",
            None => continue,
        };
        if pending {
            continue;
        }
        let (value, changed, was) = album_value(a, key);
        l.edit = (n > 0).then(|| Edit::Album(key.to_owned()));
        l.multi = library::edit::multi_valued(key);
        if changed > 0 {
            l.values = value.clone();
            l.changed = true;
            l.attention = false;
            l.note = match (changed, was.is_empty()) {
                (1, true) => "On 1 track when you save; it had none before".to_owned(),
                (n, true) => format!("On {} when you save; they had none before", tracks_n(n)),
                (1, false) => format!("On 1 track when you save; it was {}", quoted(&was)),
                (n, false) => format!("On {} when you save; most had {}", tracks_n(n), quoted(&was)),
            };
        } else if matches!(key, "DISCTOTAL" | "COMPILATION") || l.values.is_empty() {
            l.values = value.clone();
        }
        if key == "COMPILATION" {
            l.values = l.values.iter().map(|x| if x == "1" { "Yes".into() } else { "No".into() }).collect();
        }
        l.empty = l.values.is_empty() || l.values.iter().all(|x| x == "None");
        if l.empty {
            l.values.clear();
        }
    }
    // Said once, under the heading: what editing these does.
    if n > 0 && !pending {
        let what = if n == a.tracks.len() { format!("all {}", tracks_n(n)) } else { format!("the {} that can be edited (of {})", tracks_n(n), a.tracks.len()) };
        v.insert(start, Line::note(format!("Editing a field here sets it on {what}."), false));
    }
}

/// The check a fix belongs under.
fn fix_for(key: &str) -> &'static str {
    match key {
        "ALBUM" => "Album",
        "ALBUMARTIST" => "Album artist",
        "DATE" => "Date",
        "GENRE" => "Genre",
        "TRACKNUMBER" => "Track numbers",
        "DISCNUMBER" => "Disc numbers",
        _ => "",
    }
}

/// The obvious fixes of an album's inconsistencies: the value most of its tracks have, on
/// the tracks that differ; numbers in file order; disc 1 where there is none. Only for
/// tracks whose files can be edited.
pub fn album_fixes(a: &AlbumInput, facts: &[TrackFacts]) -> Vec<Fix> {
    let editable: Vec<usize> = (0..a.tracks.len()).filter(|i| a.briefs.get(*i).and_then(Option::as_ref).is_some_and(|b| b.editable.is_ok())).collect();
    let briefs: Vec<(usize, &Brief)> = facts.iter().enumerate().filter_map(|(i, t)| if let FileBrief::Read(b) = &t.file { Some((i, b)) } else { None }).collect();
    let mut out = Vec::new();
    let mut tag = |key: &str, label: &str, pick: fn(&Brief) -> Option<String>| {
        let values: Vec<(usize, Option<String>)> = briefs.iter().map(|(i, b)| (*i, pick(b))).collect();
        let Some((top, _)) = majority(values.iter().map(|(_, v)| v.clone())) else { return };
        let differ: Vec<usize> = values.iter().filter(|(i, v)| v.as_deref() != Some(top.as_str()) && editable.contains(i)).map(|(i, _)| *i).collect();
        if differ.is_empty() || differ.len() == values.len() {
            return;
        }
        out.push(Fix {
            label: format!("Set {label} to {} on the {} that differ{}", q(&top), tracks_n(differ.len()), if differ.len() == 1 { "s" } else { "" }),
            key: key.to_owned(),
            tracks: differ.into_iter().map(|i| (i, vec![top.clone()])).collect(),
        });
    };
    tag("ALBUM", "the album", |b| b.album.clone());
    tag("ALBUMARTIST", "the album artist", |b| b.album_artist.clone());
    tag("DATE", "the date", |b| b.date.clone());
    tag("GENRE", "the genre", |b| b.genre.clone());
    // Numbers: 1, 2, 3... on each disc, in the order of the files' names.
    let mut by_disc: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, f) in facts.iter().enumerate() {
        by_disc.entry(f.disc.unwrap_or(1)).or_default().push(i);
    }
    let mut numbering = Vec::new();
    for idx in by_disc.values_mut() {
        idx.sort_by_key(|i| a.tracks[*i].path.to_lowercase());
        for (n, i) in idx.iter().enumerate() {
            let want = n as u32 + 1;
            if facts[*i].number != Some(want) && editable.contains(i) {
                numbering.push((*i, vec![want.to_string()]));
            }
        }
    }
    let numbers_off = facts.iter().any(|f| f.number.is_none()) || {
        let mut seen = HashSet::new();
        facts.iter().any(|f| !seen.insert((f.disc.unwrap_or(1), f.number)))
    } || by_disc.values().any(|idx| idx.iter().filter_map(|i| facts[*i].number).max().unwrap_or(0) as usize != idx.len());
    if numbers_off && !numbering.is_empty() {
        out.push(Fix { label: format!("Number the tracks in file order ({} to change)", tracks_n(numbering.len())), key: "TRACKNUMBER".into(), tracks: numbering });
    }
    let without: Vec<usize> = (0..facts.len()).filter(|i| facts[*i].disc.is_none() && editable.contains(i)).collect();
    if !without.is_empty() && without.len() < facts.len() {
        out.push(Fix { label: format!("Fill in disc 1 on the {} without a disc number", tracks_n(without.len())), key: "DISCNUMBER".into(), tracks: without.into_iter().map(|i| (i, vec!["1".into()])).collect() });
    }
    out
}

// ---- The viewer ----------------------------------------------------------------------

/// Bumped when the viewer opens or closes: work for an older session drops its result
/// (and an album's reading stops).
static SESSION: AtomicU64 = AtomicU64::new(0);
/// Bumped when a track's view is opened: a late read of another track is dropped.
static TRACK_SEQ: AtomicU64 = AtomicU64::new(0);

struct TrackPage {
    input: TrackInput,
    thumbs: Vec<Image>,
    seq: u64,
}

struct AlbumPage {
    input: AlbumInput,
    thumb: Option<Image>,
    cursor: i32,
    scroll: f32,
}

#[derive(Default)]
struct Viewer {
    album: Option<AlbumPage>,
    track: Option<TrackPage>,
    /// The lines on screen.
    lines: Vec<Line>,
    expanded: HashSet<String>,
    // ---- Tag editing (all empty with it off) ----
    /// Every unsaved change, of every file this panel has shown (a track and its album
    /// share it).
    draft: metadata::tagedit::Draft,
    /// The field (or table row) being edited.
    editing: Option<Editing>,
    stage: Stage,
    all_fields: bool,
    /// The Edit history of what is shown.
    history: Vec<(library::edit::journal::Save, usize, bool)>,
    /// The new cover's thumbnail.
    new_cover: Option<Image>,
    /// The album's fixes, as offered on screen.
    fixes: Vec<Fix>,
    /// What Review shows and Save writes.
    review: Option<Review>,
    outcome: Option<metadata::tagedit::Outcome>,
    /// A save wrote files: the next scan's news is shown (`library_changed`).
    awaiting_scan: bool,
}

/// Where the panel is with tag editing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    View,
    Review,
    Saving,
    Result,
    /// Closing with unsaved changes: asked first.
    Confirm,
}

impl Stage {
    fn index(self) -> i32 {
        match self {
            Stage::View => 0,
            Stage::Review => 1,
            Stage::Saving => 2,
            Stage::Result => 3,
            Stage::Confirm => 4,
        }
    }
}

struct Editing {
    row: i32,
    edit: Edit,
    values: Vec<String>,
    slot: usize,
}

struct Review {
    edits: Vec<library::edit::FileEdit>,
    label: String,
    undoes: Option<String>,
}

thread_local! {
    static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) };
    static MODEL: Rc<VecModel<MetaRow>> = Rc::new(VecModel::default());
}

fn with<R>(f: impl FnOnce(&mut Viewer) -> R) -> Option<R> {
    VIEWER.with(|v| v.borrow_mut().as_mut().map(f))
}

fn session() -> u64 {
    SESSION.load(Ordering::SeqCst)
}

fn start(st: &Rc<State>) -> bool {
    let Some(app) = st.app.upgrade() else { return false };
    let d = app.global::<Dialog>();
    if d.get_open() {
        return false;
    }
    SESSION.fetch_add(1, Ordering::SeqCst);
    VIEWER.with(|v| *v.borrow_mut() = Some(Viewer::default()));
    let m = app.global::<Meta>();
    m.set_rows(ModelRc::from(MODEL.with(Rc::clone)));
    m.set_scroll(0.0);
    m.set_flash("".into());
    d.set_returns(false);
    true
}

/// A track's view (from a track list or the queue).
pub fn open_track(st: &Rc<State>, track: Track) {
    if !start(st) {
        return;
    }
    show_track(st, track);
}

/// An album's view (from its tile, the album page).
pub fn open_album(st: &Rc<State>, id: i64) {
    let Ok(Some(facts)) = library::query::album_facts(&st.db, id) else {
        crate::views::flash(st, "Album not found");
        return;
    };
    if !start(st) {
        return;
    }
    let tracks = library::query::album_tracks(&st.db, id).unwrap_or_default();
    let n = tracks.len();
    let input = AlbumInput { facts: facts.clone(), tracks: tracks.clone(), files: vec![FileBrief::Pending; n], briefs: vec![None; n], ..Default::default() };
    with(|v| v.album = Some(AlbumPage { input, thumb: None, cursor: -1, scroll: 0.0 }));
    header_cover(st, facts.art.as_deref());
    render(st, true);
    open_panel(st);
    read_album(facts, tracks, metadata::tagedit::enabled(st));
    refresh_history(st);
}

fn open_panel(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        app.global::<Dialog>().set_meta_open(true);
    }
}

fn show_track(st: &Rc<State>, track: Track) {
    let stream = details::playing_stream(st, &track);
    let (album, genre, extras) = if track.id > 0 {
        (
            library::query::album_facts(&st.db, track.album_id).ok().flatten(),
            library::query::track_genre(&st.db, track.id).ok().flatten(),
            library::query::track_extras(&st.db, track.id).ok().flatten(),
        )
    } else {
        (None, None, None)
    };
    let (credits, genres) = if track.id > 0 {
        (library::query::track_artists(&st.db, track.id).unwrap_or_default(), library::query::track_genre_names(&st.db, track.id).unwrap_or_default())
    } else {
        (Vec::new(), Vec::new())
    };
    let seq = TRACK_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let art = track.art.clone();
    let path = track.path.clone();
    let input = TrackInput { track, album, genre, credits, genres, extras, stream, ..Default::default() };
    with(|v| {
        v.track = Some(TrackPage { input, thumbs: Vec::new(), seq });
        v.expanded.clear();
    });
    header_cover(st, art.as_deref());
    render(st, true);
    open_panel(st);
    read_track(path, session(), seq, metadata::tagedit::enabled(st));
    refresh_history(st);
}

/// The header's cover, from the art cache (already made by the scan).
fn header_cover(st: &Rc<State>, stem: Option<&str>) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    m.set_cover(Image::default());
    m.set_has_cover(stem.is_some());
    if let Some(stem) = stem {
        let weak = st.app.clone();
        let s = session();
        crate::covers::with_image(stem, crate::covers::Kind::Thumb, move |img| {
            if session() == s
                && let Some(app) = weak.upgrade()
            {
                app.global::<Meta>().set_cover(img);
            }
        });
    }
}

fn pixels(t: &meta::Thumb) -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&t.rgba, t.width, t.height)
}

/// Read a track's file off the UI thread: its every tag, its pictures' thumbnails, the
/// tags as the scanner reads them, and its details. Never written to. With tag editing on
/// (`editing`), also whether it can be edited and what it is now (for the save's check).
fn read_track(path: String, session_id: u64, seq: u64, editing: bool) {
    let spawned = std::thread::Builder::new().name("mp-meta".into()).spawn(move || {
        library::background_thread();
        let p = Path::new(&path);
        let file = if library::remote::is_remote(&path) {
            FileRead::Server
        } else if !p.exists() {
            FileRead::Missing
        } else {
            match meta::read(p, meta::ReadOptions::ALL) {
                Err(e) => FileRead::Unreadable(e),
                Ok(mut tags) => {
                    let fields = meta::fields(&tags);
                    let scanned = library::tags::read(p, false).ok().map(|(t, _)| t);
                    let report = library::tags::file_report(p).ok();
                    let size = std::fs::metadata(p).ok().map(|m| m.len());
                    let lrc = p.with_extension("lrc");
                    let lrc = lrc.is_file().then(|| lrc.file_name().map(|n| n.to_string_lossy().into_owned())).flatten();
                    let (expect, editable) = if editing { edit_state(p, &tags) } else { (None, Err(String::new())) };
                    // Thumbnails become images on the UI thread; the rest stays here.
                    let buffers: Vec<Option<SharedPixelBuffer<Rgba8Pixel>>> = tags.pictures.iter_mut().map(|pic| pic.thumb.take().map(|t| pixels(&t))).collect();
                    for (pic, b) in tags.pictures.iter_mut().zip(&buffers) {
                        if b.is_some() {
                            pic.thumb = Some(meta::Thumb { width: 0, height: 0, rgba: Vec::new() });
                        }
                    }
                    let read = ReadFile { tags, fields, scanned, report, size, lrc, expect, editable };
                    return post_track(session_id, seq, FileRead::Read(Box::new(read)), buffers);
                }
            }
        };
        post_track(session_id, seq, file, Vec::new());
    });
    if let Err(e) = spawned {
        log::warn!("metadata: {e}");
    }
}

/// Whether a file can be edited (why not), and what it is now: what a save checks it still
/// is (`library::edit::Expect`, from the tags read the way the save reads them).
fn edit_state(p: &Path, tags: &FileTags) -> (Option<library::edit::Expect>, Result<(), String>) {
    let editable = library::edit::support(p).and_then(|c| match library::edit::blocked(c, tags) {
        Some(why) => Err(why),
        None => Ok(()),
    });
    let expect = meta::read(p, meta::ReadOptions::TAGS).ok().and_then(|t| library::edit::expect_now(p, &t));
    let lower = |w: String| {
        let w = w.trim_end_matches('.').to_owned();
        match w.chars().next() {
            Some(c) => c.to_lowercase().collect::<String>() + &w[c.len_utf8()..],
            None => w,
        }
    };
    let editable = match (&expect, editable) {
        (None, Ok(())) => Err("it could not be read again".into()),
        (_, e) => e.map_err(lower),
    };
    (expect, editable)
}

fn post_track(session_id: u64, seq: u64, file: FileRead, buffers: Vec<Option<SharedPixelBuffer<Rgba8Pixel>>>) {
    if session() != session_id {
        return;
    }
    state::post(move |st| {
        if session() != session_id {
            return;
        }
        let applied = with(|v| {
            let Some(t) = v.track.as_mut().filter(|t| t.seq == seq) else { return false };
            t.input.file = file;
            // Thumbnails: one image per picture (a blank where none could be made).
            t.thumbs = buffers.into_iter().map(|b| b.map(Image::from_rgba8).unwrap_or_default()).collect();
            true
        });
        if applied == Some(true) {
            render(st, false);
        }
    });
}

/// Read an album's files off the UI thread, one at a time, posting what is found in
/// batches; stops when the viewer closes. With tag editing on (`editing`), also what each
/// file holds of the fields an album edits, its front covers, and what it is now.
fn read_album(facts: AlbumFacts, tracks: Vec<Track>, editing: bool) {
    let s = session();
    let spawned = std::thread::Builder::new().name("mp-meta-album".into()).spawn(move || {
        library::background_thread();
        // The cover first: it is one file.
        let cover = find_cover(&facts);
        let (found, thumb) = cover;
        let thumb = thumb.as_ref().map(pixels);
        if session() != s {
            return;
        }
        state::post(move |st| {
            if session() != s {
                return;
            }
            with(|v| {
                if let Some(a) = v.album.as_mut() {
                    a.input.cover = found;
                    a.input.has_thumb = thumb.is_some();
                    a.thumb = thumb.map(Image::from_rgba8);
                }
            });
            render(st, false);
        });
        let mut batch: Vec<(usize, FileBrief, Option<EditBrief>)> = Vec::new();
        let mut last = std::time::Instant::now();
        for (i, t) in tracks.iter().enumerate() {
            if session() != s {
                return;
            }
            let p = Path::new(&t.path);
            let mut edit = None;
            let brief = if library::remote::is_remote(&t.path) {
                FileBrief::Server
            } else if !p.exists() {
                FileBrief::Missing
            } else {
                match meta::read(p, meta::ReadOptions::TAGS) {
                    Ok(tags) => {
                        let fields = meta::fields(&tags);
                        if editing {
                            let values = BRIEF_KEYS.iter().map(|k| ((*k).to_owned(), metadata::tagedit::values_of(&fields, k))).collect();
                            let pics = meta::ReadOptions { pictures: true, thumbnails: false, raw: false };
                            let fronts = meta::read(p, pics).map(|t| t.pictures.iter().filter(|x| x.front || t.blocks.get(x.block) == Some(&BlockKind::Mp4)).count()).unwrap_or(0);
                            let (expect, editable) = edit_state(p, &tags);
                            edit = Some(EditBrief { values, fronts, expect, editable });
                        }
                        FileBrief::Read(meta::brief(&fields))
                    }
                    Err(e) => FileBrief::Unreadable(e),
                }
            };
            batch.push((i, brief, edit));
            if batch.len() >= 64 || last.elapsed().as_millis() >= 400 || i + 1 == tracks.len() {
                let done = std::mem::take(&mut batch);
                last = std::time::Instant::now();
                state::post(move |st| {
                    if session() != s {
                        return;
                    }
                    with(|v| {
                        if let Some(a) = v.album.as_mut() {
                            for (i, b, e) in done {
                                if let Some(slot) = a.input.files.get_mut(i) {
                                    *slot = b;
                                }
                                if let Some(slot) = a.input.briefs.get_mut(i) {
                                    *slot = e;
                                }
                            }
                        }
                    });
                    render(st, false);
                });
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("metadata: {e}");
    }
}

/// Where the album's cover comes from now, by the scanner's rule (`scan::album_art`): the
/// embedded picture of its first track (the front cover, else the first), else an image in
/// the first track's folder, else in the album's folder.
fn find_cover(f: &AlbumFacts) -> (CoverFound, Option<meta::Thumb>) {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let first = f.first_track.as_ref().map(std::path::PathBuf::from);
    if let Some(track) = &first
        && let Ok(mut tags) = meta::read(track, meta::ReadOptions::ALL)
    {
        let at = tags.pictures.iter().position(|p| p.front).or(if tags.pictures.is_empty() { None } else { Some(0) });
        if let Some(i) = at {
            let p = tags.pictures.swap_remove(i);
            let text = picture_text(&p.format, p.size, p.bytes as u64);
            return (CoverFound::Embedded { file: name(track), kind: p.kind, text }, p.thumb);
        }
    }
    let folder = Path::new(&f.folder);
    let dir = first.as_ref().and_then(|t| t.parent().map(Path::to_path_buf)).unwrap_or_else(|| folder.to_path_buf());
    if let Some(img) = library::art::find_folder_cover(&dir).or_else(|| library::art::find_folder_cover(folder)) {
        let format = img.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default().replace("JPG", "JPEG");
        return match meta::image_file(&img, true) {
            Ok(f) => (CoverFound::Folder { file: name(&img), text: picture_text(&format, f.size, f.bytes) }, f.thumb),
            Err(e) => (CoverFound::Folder { file: name(&img), text: format!("Could not be read: {e}") }, None),
        };
    }
    (CoverFound::None { first: first.as_ref().map(|p| name(p)) }, None)
}

fn to_row(l: &Line, expanded: bool, thumbs: &[Image], new_cover: Option<&Image>) -> MetaRow {
    let (values, expandable) = shown(&l.values, expanded);
    let mut note = l.note.clone();
    if expandable && note.is_empty() {
        note = length_note(&l.values);
    }
    let thumb = l.thumb.and_then(|i| if i == NEW_COVER { new_cover } else { thumbs.get(i) }).cloned();
    MetaRow {
        kind: match l.kind {
            Kind::Heading => 0,
            Kind::Field => 1,
            Kind::Picture => 2,
            Kind::Note => 3,
            Kind::TableHead => 4,
            Kind::Track => 5,
        },
        label: l.label.as_str().into(),
        values: ModelRc::new(VecModel::from(values.into_iter().map(SharedString::from).collect::<Vec<_>>())),
        note: note.into(),
        attention: l.attention,
        expandable,
        expanded,
        copyable: !l.copy.is_empty(),
        has_thumb: thumb.is_some(),
        thumb: thumb.unwrap_or_default(),
        cells: ModelRc::new(VecModel::from(l.cells.iter().map(|c| SharedString::from(c.as_str())).collect::<Vec<_>>())),
        number_attention: l.number_attention,
        format_attention: l.format_attention,
        selectable: l.selectable(),
        action: l.act != Act::None,
        edit: match &l.edit {
            None => 0,
            Some(Edit::Field(_)) => 1,
            Some(Edit::Remove(_)) => 2,
            Some(Edit::Album(_)) => 3,
            Some(Edit::Row(_)) => 4,
        },
        changed: l.changed,
        empty: l.empty,
        multi: l.multi,
        cell_changed: ModelRc::new(VecModel::from(l.cell_changed.to_vec())),
    }
}

/// Rows of the same shape are compared cheaply; only changed ones are replaced, so the
/// panel does not rebuild a long list for one file read.
fn same(a: &MetaRow, b: &MetaRow) -> bool {
    a.kind == b.kind
        && a.label == b.label
        && a.note == b.note
        && a.attention == b.attention
        && a.expanded == b.expanded
        && a.expandable == b.expandable
        && a.has_thumb == b.has_thumb
        && !b.has_thumb
        && a.number_attention == b.number_attention
        && a.format_attention == b.format_attention
        && a.selectable == b.selectable
        && a.edit == b.edit
        && a.changed == b.changed
        && a.empty == b.empty
        && a.values.iter().eq(b.values.iter())
        && a.cells.iter().eq(b.cells.iter())
        && a.cell_changed.iter().eq(b.cell_changed.iter())
}

/// Tag editing's part of a view, from the viewer's state (None with editing off).
fn edit_view(st: &State, v: &Viewer) -> Option<EditView> {
    if !metadata::tagedit::enabled(st) {
        return None;
    }
    let history = v
        .history
        .iter()
        .map(|(s, n, undone)| {
            let when = details::date_text(s.header.time / 1000);
            let what = s.header.label.clone();
            (when, what, *n, *undone)
        })
        .collect();
    Some(EditView { draft: v.draft.clone(), all_fields: v.all_fields, history })
}

/// Build the current page's lines and show them. `fresh`: a new page (the cursor goes to
/// its first row, the scroll to the top).
fn render(st: &Rc<State>, fresh: bool) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    let Some((lines, head, rows)) = with(|v| {
        let ev = edit_view(st, v);
        if let Some(t) = v.track.as_mut() {
            t.input.edit = ev.clone();
        }
        if let Some(a) = v.album.as_mut() {
            a.input.edit = ev.clone();
        }
        let (lines, head, thumbs): (Vec<Line>, [String; 4], &[Image]) = if let Some(t) = &v.track {
            let i = &t.input;
            let sub = [i.track.artist.as_str(), i.track.album.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" \u{b7} ");
            let blocks = match &i.file {
                FileRead::Read(r) if r.tags.blocks.is_empty() => "No tags".to_owned(),
                FileRead::Read(r) => format!("Tags: {}", r.tags.blocks.iter().map(|b| b.name()).collect::<Vec<_>>().join(", ")),
                FileRead::Pending => "Reading the file\u{2026}".into(),
                FileRead::Missing => "File missing".into(),
                FileRead::Unreadable(_) => "File not readable".into(),
                FileRead::Server => "From the server".into(),
            };
            (track_lines(i), ["TRACK".into(), i.track.title.clone(), sub, blocks], &t.thumbs)
        } else if let Some(a) = &v.album {
            let f = &a.input.facts;
            let thumbs = a.thumb.as_slice();
            (album_lines(&a.input), ["ALBUM".into(), f.title.clone(), f.album_artist.clone(), f.date.clone().unwrap_or_else(|| "No date".into())], thumbs)
        } else {
            return None;
        };
        if v.track.is_none()
            && let Some(a) = &v.album
        {
            v.fixes = if a.input.edit.is_some() { album_fixes_now(&a.input) } else { Vec::new() };
        }
        let rows: Vec<MetaRow> = lines.iter().map(|l| to_row(l, v.expanded.contains(&l.id), thumbs, v.new_cover.as_ref())).collect();
        v.lines = lines.clone();
        Some((lines, head, rows))
    })
    .flatten() else {
        return;
    };
    let can_back = with(|v| v.track.is_some() && v.album.is_some()).unwrap_or(false);
    m.set_eyebrow(head[0].as_str().into());
    m.set_title(head[1].as_str().into());
    m.set_subtitle(head[2].as_str().into());
    m.set_blocks(head[3].as_str().into());
    m.set_no_file(path_of_view().is_some_and(|p| library::remote::is_remote(&p)));
    m.set_can_back(can_back);
    let (status, attention) = with(|v| match v.track.as_ref().map(|t| &t.input.file) {
        Some(FileRead::Missing) => ("The file is missing: what the library knows is shown.".to_owned(), true),
        Some(FileRead::Unreadable(_)) => ("The file could not be read: what the library knows is shown.".to_owned(), true),
        Some(FileRead::Server) => ("On the server only: what the library knows is shown.".to_owned(), false),
        _ => (String::new(), false),
    })
    .unwrap_or_default();
    m.set_status(status.into());
    m.set_status_attention(attention);
    MODEL.with(|model| {
        if fresh || model.row_count() != rows.len() {
            model.set_vec(rows);
        } else {
            for (i, r) in rows.into_iter().enumerate() {
                if model.row_data(i).is_none_or(|old| !same(&old, &r)) {
                    model.set_row_data(i, r);
                }
            }
        }
    });
    if fresh {
        m.set_cursor(lines.iter().position(Line::selectable).map_or(-1, |i| i as i32));
        m.set_scroll(0.0);
    } else if m.get_cursor() >= lines.len() as i32 {
        m.set_cursor(lines.iter().rposition(Line::selectable).map_or(-1, |i| i as i32));
    }
    sync_edit(st);
}

/// The album's fixes as `album_lines` offers them (the same numbering).
fn album_fixes_now(a: &AlbumInput) -> Vec<Fix> {
    if a.files.contains(&FileBrief::Pending) {
        return Vec::new();
    }
    album_fixes(a, &draft_facts(a))
}

/// The album's tracks as the checks see them: with the draft, as they will be once saved.
fn draft_facts(a: &AlbumInput) -> Vec<TrackFacts> {
    let draft = a.edit.as_ref().map(|e| &e.draft);
    let new_of = |i: usize, k: &str| draft.and_then(|d| a.tracks.get(i).and_then(|t| d.field(&t.path, k))).map(|c| c.new.clone());
    a.tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut file = a.files.get(i).cloned().unwrap_or_default();
            if let FileBrief::Read(b) = &mut file {
                let first = |k: &str| new_of(i, k).map(|v| v.first().cloned());
                if let Some(x) = first("ALBUM") {
                    b.album = x;
                }
                if let Some(x) = first("ALBUMARTIST") {
                    b.album_artist = x;
                }
                if let Some(x) = first("DATE") {
                    b.date = x;
                }
                if let Some(x) = first("GENRE") {
                    b.genre = x;
                }
            }
            let num = |k: &str, now: Option<u32>| new_of(i, k).map(|v| v.first().and_then(|n| names::leading_number(n))).unwrap_or(now);
            TrackFacts { disc: num("DISCNUMBER", t.disc_no), number: num("TRACKNUMBER", t.track_no), audio: t.audio, file }
        })
        .collect()
}

/// The panel's editing state: the draft bar, the field being edited, the stage.
fn sync_edit(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    let on = metadata::tagedit::enabled(st);
    m.set_edit_on(on);
    let Some((draft, stage, row, values, slot, kind, multi)) = with(|v| {
        let (row, values, slot, kind, multi) = match &v.editing {
            Some(e) => (e.row, e.values.clone(), e.slot, i32::from(matches!(e.edit, Edit::Row(_))), v.lines.get(e.row.max(0) as usize).is_some_and(|l| l.multi)),
            None => (-1, Vec::new(), 0, 0, false),
        };
        (if v.draft.is_empty() { String::new() } else { v.draft.status() }, v.stage, row, values, slot, kind, multi)
    }) else {
        return;
    };
    m.set_draft_text(draft.into());
    m.set_stage(stage.index());
    if m.get_edit_row() != row || m.get_edit_values().row_count() != values.len() {
        m.set_edit_values(ModelRc::new(VecModel::from(values.into_iter().map(SharedString::from).collect::<Vec<_>>())));
    }
    m.set_edit_row(row);
    m.set_edit_slot(slot as i32);
    m.set_edit_kind(kind);
    m.set_edit_multi(multi);
}

pub fn close(st: &Rc<State>) {
    SESSION.fetch_add(1, Ordering::SeqCst);
    VIEWER.with(|v| v.borrow_mut().take());
    MODEL.with(|m| m.set_vec(Vec::new()));
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        m.set_cover(Image::default());
        m.set_edit_row(-1);
        m.set_stage(0);
        m.set_draft_text("".into());
        app.global::<Dialog>().set_meta_open(false);
    }
}

/// From a track back to its album, where it was left.
pub fn back(st: &Rc<State>) {
    if with(|v| v.editing.is_some() || v.stage != Stage::View) == Some(true) && (with(|v| v.stage != Stage::View) == Some(true) || !edit_commit(st)) {
        return;
    }
    let Some((cursor, scroll)) = with(|v| {
        if v.album.is_none() || v.track.is_none() {
            return None;
        }
        v.track = None;
        v.expanded.clear();
        v.album.as_ref().map(|a| (a.cursor, a.scroll))
    })
    .flatten() else {
        return;
    };
    TRACK_SEQ.fetch_add(1, Ordering::SeqCst);
    let art = with(|v| v.album.as_ref().and_then(|a| a.input.facts.art.clone())).flatten();
    header_cover(st, art.as_deref());
    render(st, true);
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        m.set_cursor(cursor);
        m.set_scroll(scroll);
        m.set_focus_seq(m.get_focus_seq() + 1);
    }
}

/// Enter or a click on a row: a track row opens the track; "Show more" lists more; a long
/// value expands or folds.
pub fn activate(st: &Rc<State>, i: i32) {
    let Some(line) = with(|v| v.lines.get(i.max(0) as usize).cloned()).flatten() else { return };
    match line.act {
        Act::OpenTrack(t) => {
            // A field being edited is kept first (and stays open when it cannot be).
            if with(|v| v.editing.is_some()) == Some(true) && !edit_commit(st) {
                return;
            }
            let Some(app) = st.app.upgrade() else { return };
            let m = app.global::<Meta>();
            let (cursor, scroll) = (m.get_cursor(), m.get_scroll());
            let track = with(|v| {
                let a = v.album.as_mut()?;
                a.cursor = cursor;
                a.scroll = scroll;
                a.input.tracks.get(t).cloned()
            })
            .flatten();
            if let Some(track) = track {
                show_track(st, track);
                m.set_focus_seq(m.get_focus_seq() + 1);
            }
        }
        Act::More(section) => {
            with(|v| match section {
                Section::OtherTags => {
                    if let Some(t) = v.track.as_mut() {
                        t.input.other_limit = t.input.other_limit.max(PAGE_ROWS) + PAGE_ROWS;
                    }
                }
                Section::Tracks => {
                    if let Some(a) = v.album.as_mut() {
                        a.input.table_limit = a.input.table_limit.max(PAGE_ROWS) + PAGE_ROWS;
                    }
                }
            });
            render(st, false);
        }
        Act::Fix(k) => apply_fix(st, k),
        Act::Cover(c) => cover(st, c),
        Act::Undo(k) => review_undo(st, k),
        Act::AllFields => {
            with(|v| v.all_fields = true);
            render(st, false);
        }
        Act::None => {
            // With tag editing on, Enter on a field edits it (Ctrl+C and the copy button copy).
            if line.edit.is_some() && metadata::tagedit::enabled(st) {
                edit_start(st, i);
            } else if with(|v| to_row(&line, v.expanded.contains(&line.id), &[], None).expandable) == Some(true) {
                toggle(st, i);
            } else {
                copy(st, i);
            }
        }
    }
}

/// Show a long value in full, or fold it again.
pub fn toggle(st: &Rc<State>, i: i32) {
    let changed = with(|v| {
        let id = v.lines.get(i.max(0) as usize)?.id.clone();
        if !v.expanded.remove(&id) {
            v.expanded.insert(id);
        }
        Some(())
    })
    .flatten();
    if changed.is_some() {
        render(st, false);
    }
}

fn set_expanded(st: &Rc<State>, i: i32, on: bool) {
    let changed = with(|v| {
        let id = v.lines.get(i.max(0) as usize)?.id.clone();
        Some(if on { v.expanded.insert(id) } else { v.expanded.remove(&id) })
    })
    .flatten();
    if changed == Some(true) {
        render(st, false);
    }
}

fn flash(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    m.set_flash(text.into());
    m.set_flash_seq(m.get_flash_seq() + 1);
}

/// Copy one row's value.
pub fn copy(st: &Rc<State>, i: i32) {
    let Some(line) = with(|v| v.lines.get(i.max(0) as usize).cloned()).flatten() else { return };
    if line.copy.is_empty() {
        return;
    }
    let what = if line.kind == Kind::Track { "Path".to_owned() } else { line.label.clone() };
    if crate::explorer::copy_text(&line.copy) {
        flash(st, &format!("Copied {}", what.to_lowercase().replace("musicbrainz", "MusicBrainz").replace("isrc", "ISRC")));
    } else {
        flash(st, "Could not copy");
    }
}

/// The whole view as text.
pub fn copy_all_text(st: &Rc<State>) -> Option<String> {
    let app = st.app.upgrade()?;
    let m = app.global::<Meta>();
    let head = vec![format!("{}: {}", if m.get_eyebrow() == "TRACK" { "Track" } else { "Album" }, m.get_title()), m.get_subtitle().to_string(), path_of_view().unwrap_or_default()];
    with(|v| {
        let lines = match (&v.track, &v.album) {
            (Some(t), _) => track_copy_lines(&t.input),
            (None, Some(a)) => album_lines(&AlbumInput { table_limit: usize::MAX, ..a.input.clone() }),
            _ => Vec::new(),
        };
        copy_all(&head, &lines)
    })
}

pub fn copy_everything(st: &Rc<State>) {
    let Some(text) = copy_all_text(st) else { return };
    if crate::explorer::copy_text(&text) {
        flash(st, "Copied everything");
    } else {
        flash(st, "Could not copy");
    }
}

/// The file (a track) or the folder (an album) the view is about.
fn path_of_view() -> Option<String> {
    with(|v| match (&v.track, &v.album) {
        (Some(t), _) => Some(t.input.track.path.clone()),
        (None, Some(a)) => Some(a.input.facts.folder.clone()),
        _ => None,
    })
    .flatten()
}

pub fn explore(st: &Rc<State>) {
    let path = path_of_view().unwrap_or_default();
    if path.is_empty() || !crate::explorer::show(&path) {
        flash(st, "Not found on disk");
    }
}

/// The list's keys: the cursor moves over the rows Enter can act on.
pub fn key(st: &Rc<State>, k: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    let Some(sel) = with(|v| v.lines.iter().map(Line::selectable).collect::<Vec<bool>>()) else { return };
    let cur = m.get_cursor();
    let next = |from: i32, dir: i32| -> i32 {
        let mut i = from;
        loop {
            i += dir;
            if i < 0 || i >= sel.len() as i32 {
                return from;
            }
            if sel[i as usize] {
                return i;
            }
        }
    };
    let to = match k {
        "down" => next(cur, 1),
        "up" => next(cur, -1),
        "home" => next(-1, 1),
        "end" => next(sel.len() as i32, -1),
        "page-down" => (0..12).fold(cur, |c, _| next(c, 1)),
        "page-up" => (0..12).fold(cur, |c, _| next(c, -1)),
        "right" => {
            set_expanded(st, cur, true);
            cur
        }
        "left" => {
            set_expanded(st, cur, false);
            cur
        }
        _ => cur,
    };
    if to != cur && to >= 0 {
        m.set_cursor(to);
    }
}

/// A click on a row puts the cursor there.
pub fn select(st: &Rc<State>, i: i32) {
    if let Some(app) = st.app.upgrade()
        && with(|v| v.lines.get(i.max(0) as usize).is_some_and(Line::selectable)) == Some(true)
    {
        app.global::<Meta>().set_cursor(i);
    }
}

// ---- Tag editing -------------------------------------------------------------------------
//
// Nothing here acts with tag editing off: every entry point checks `metadata::tagedit::enabled`, and
// the lines carry no `edit` then, so there is nothing to start from either.

/// The file a track's view is about, its name and what it is now (for the draft).
fn track_file(v: &Viewer) -> Option<(String, String, Option<library::edit::Expect>, Vec<Field>)> {
    let t = v.track.as_ref()?;
    let FileRead::Read(r) = &t.input.file else { return None };
    r.editable.as_ref().ok()?;
    let name = Path::new(&t.input.track.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some((t.input.track.path.clone(), name, r.expect.clone(), r.fields.clone()))
}

/// The values an editor of a line starts from: the draft's, else the file's.
fn start_values(v: &Viewer, edit: &Edit) -> Option<Vec<String>> {
    Some(match edit {
        Edit::Field(k) | Edit::Remove(k) => {
            let (path, _, _, fields) = track_file(v)?;
            v.draft.field(&path, k).map(|c| c.new.clone()).unwrap_or_else(|| metadata::tagedit::values_of(&fields, k))
        }
        Edit::Album(k) => {
            let a = v.album.as_ref()?;
            let mut x = album_value(&a.input, k).0;
            if k == "COMPILATION" {
                x = x.iter().map(|f| if f == "1" { "Yes".into() } else { "No".into() }).collect();
            }
            x
        }
        Edit::Row(i) => {
            let a = v.album.as_ref()?;
            let t = a.input.tracks.get(*i)?;
            let b = a.input.briefs.get(*i)?.as_ref()?;
            let get = |k: &str| v.draft.field(&t.path, k).map(|c| c.new.clone()).unwrap_or_else(|| b.values.get(k).cloned().unwrap_or_default());
            vec![get("TRACKNUMBER").join("; "), get("TITLE").join("; "), get("ARTIST").join("; ")]
        }
    })
}

/// Start editing row `i` (Enter, F2, a click on its value). An identifier is not typed:
/// it toggles between kept and removed.
pub fn edit_start(st: &Rc<State>, i: i32) {
    if !metadata::tagedit::enabled(st) || with(|v| v.stage != Stage::View).unwrap_or(true) {
        return;
    }
    // A field being edited is committed first (and stays when it cannot be).
    if with(|v| v.editing.is_some()) == Some(true) && !edit_commit(st) {
        return;
    }
    let Some(line) = with(|v| v.lines.get(i.max(0) as usize).cloned()).flatten() else { return };
    let Some(edit) = line.edit.clone() else { return };
    if let Edit::Remove(k) = &edit {
        toggle_removed(st, k);
        return;
    }
    let started = with(|v| {
        let mut values = start_values(v, &edit)?;
        if values.is_empty() {
            values.push(String::new());
        }
        v.editing = Some(Editing { row: i, edit, values, slot: 0 });
        Some(())
    })
    .flatten();
    if started.is_some()
        && let Some(app) = st.app.upgrade()
    {
        let m = app.global::<Meta>();
        m.set_cursor(i);
        m.set_edit_error("".into());
        sync_edit(st);
        bump_focus(st);
    }
}

fn toggle_removed(st: &Rc<State>, key: &str) {
    with(|v| {
        let (path, name, expect, fields) = track_file(v)?;
        let now = metadata::tagedit::values_of(&fields, key);
        let removed = v.draft.field(&path, key).is_some_and(|c| c.new.is_empty());
        v.draft.set(&path, &name, expect, key, now.clone(), if removed { now } else { Vec::new() });
        Some(())
    });
    render(st, false);
}

/// A value typed into slot `slot`: kept, and checked as it is typed.
pub fn edit_text(st: &Rc<State>, slot: i32, text: &str) {
    let err = with(|v| {
        let e = v.editing.as_mut()?;
        if let Some(x) = e.values.get_mut(slot.max(0) as usize) {
            *x = text.to_owned();
        }
        e.slot = slot.max(0) as usize;
        Some(check_editing(e).err().unwrap_or_default())
    })
    .flatten()
    .unwrap_or_default();
    if let Some(app) = st.app.upgrade() {
        app.global::<Meta>().set_edit_error(err.into());
    }
}

/// What the editor's values come to: (field, values) to set, or why they cannot be.
fn check_editing(e: &Editing) -> Result<Vec<(String, Vec<String>)>, String> {
    let named = |k: &str, r: Result<Vec<(String, Vec<String>)>, String>| r.map_err(|x| format!("{}: {x}", metadata::tagedit::label(k)));
    match &e.edit {
        Edit::Field(k) | Edit::Album(k) | Edit::Remove(k) => named(k, metadata::tagedit::read_values(k, &e.values)),
        Edit::Row(_) => {
            let split = |s: &str| s.split("; ").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect::<Vec<_>>();
            let mut out = named("TRACKNUMBER", metadata::tagedit::read_values("TRACKNUMBER", &[e.values.first().cloned().unwrap_or_default()]))?;
            out.extend(named("TITLE", metadata::tagedit::read_values("TITLE", &[e.values.get(1).cloned().unwrap_or_default()]))?);
            out.extend(named("ARTIST", metadata::tagedit::read_values("ARTIST", &split(e.values.get(2).map(String::as_str).unwrap_or(""))))?);
            Ok(out)
        }
    }
}

/// Put the field being edited in the draft. False (and the field stays open, with why)
/// when its values cannot be written.
pub fn edit_commit(st: &Rc<State>) -> bool {
    let result = with(|v| {
        let e = v.editing.as_ref()?;
        Some(match check_editing(e) {
            Err(why) => Err(why),
            Ok(sets) => {
                let edit = e.edit.clone();
                apply_sets(v, &edit, sets);
                v.editing = None;
                Ok(())
            }
        })
    })
    .flatten();
    match result {
        Some(Err(why)) => {
            if let Some(app) = st.app.upgrade() {
                app.global::<Meta>().set_edit_error(why.into());
            }
            false
        }
        Some(Ok(())) => {
            render(st, false);
            true
        }
        None => true,
    }
}

/// A track an album edit goes to: its path, file name, the file as read, its values.
type AlbumTarget = (String, String, Option<library::edit::Expect>, std::collections::BTreeMap<String, Vec<String>>);
/// A fix for one track: path, file name, the file as read, old values, new values.
type FixSet = (String, String, Option<library::edit::Expect>, Vec<String>, Vec<String>);

/// Field values into the draft: a track's file, or every editable track of the album.
fn apply_sets(v: &mut Viewer, edit: &Edit, sets: Vec<(String, Vec<String>)>) {
    match edit {
        Edit::Field(_) | Edit::Remove(_) => {
            let Some((path, name, expect, fields)) = track_file(v) else { return };
            for (k, new) in sets {
                v.draft.set(&path, &name, expect.clone(), &k, metadata::tagedit::values_of(&fields, &k), new);
            }
        }
        Edit::Album(_) => {
            let Some(a) = v.album.as_ref() else { return };
            let targets: Vec<AlbumTarget> = editable_tracks(&a.input)
                .into_iter()
                .filter_map(|i| {
                    let t = &a.input.tracks[i];
                    let b = a.input.briefs[i].as_ref()?;
                    Some((t.path.clone(), file_name(&t.path), b.expect.clone(), b.values.clone()))
                })
                .collect();
            for (path, name, expect, values) in targets {
                for (k, new) in &sets {
                    v.draft.set(&path, &name, expect.clone(), k, values.get(k).cloned().unwrap_or_default(), new.clone());
                }
            }
        }
        Edit::Row(i) => {
            let Some(a) = v.album.as_ref() else { return };
            let Some((path, expect, values)) = a.input.tracks.get(*i).zip(a.input.briefs.get(*i).and_then(Option::as_ref)).map(|(t, b)| (t.path.clone(), b.expect.clone(), b.values.clone())) else { return };
            let name = file_name(&path);
            for (k, new) in sets {
                v.draft.set(&path, &name, expect.clone(), &k, values.get(&k).cloned().unwrap_or_default(), new);
            }
        }
    }
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Esc in a field: it closes, as it was.
pub fn edit_cancel(st: &Rc<State>) {
    with(|v| v.editing = None);
    if let Some(app) = st.app.upgrade() {
        app.global::<Meta>().set_edit_error("".into());
        app.global::<Meta>().set_focus_seq(app.global::<Meta>().get_focus_seq() + 1);
    }
    render(st, false);
}

/// Tab and Shift+Tab in a field: the next value, else the next field (committing this
/// one; it stays when it cannot be).
pub fn edit_next(st: &Rc<State>, slot: i32, dir: i32) {
    let moved = with(|v| {
        let e = v.editing.as_mut()?;
        let next = slot + dir;
        (next >= 0 && (next as usize) < e.values.len()).then(|| {
            e.slot = next as usize;
        })
    })
    .flatten();
    if moved.is_some() {
        sync_edit(st);
        bump_focus(st);
        return;
    }
    let Some(row) = with(|v| v.editing.as_ref().map(|e| e.row)).flatten() else { return };
    if !edit_commit(st) {
        return;
    }
    let next = with(|v| {
        let mut i = row + dir;
        while i >= 0 && (i as usize) < v.lines.len() {
            let l = &v.lines[i as usize];
            if matches!(l.edit, Some(Edit::Field(_) | Edit::Album(_) | Edit::Row(_))) {
                return Some(i);
            }
            i += dir;
        }
        None
    })
    .flatten();
    match next {
        Some(i) => {
            edit_start(st, i);
            if dir < 0 {
                with(|v| {
                    if let Some(e) = v.editing.as_mut() {
                        e.slot = e.values.len().saturating_sub(1);
                    }
                });
                sync_edit(st);
                bump_focus(st);
            }
        }
        None => {
            if let Some(app) = st.app.upgrade() {
                app.global::<Meta>().set_focus_seq(app.global::<Meta>().get_focus_seq() + 1);
            }
        }
    }
}

/// Up and Down in a table row being edited: the same cell of the row above or below.
pub fn edit_vertical(st: &Rc<State>, dir: i32) {
    let Some((row, slot)) = with(|v| v.editing.as_ref().filter(|e| matches!(e.edit, Edit::Row(_))).map(|e| (e.row, e.slot))).flatten() else { return };
    let target = with(|v| {
        let mut i = row + dir;
        while i >= 0 && (i as usize) < v.lines.len() {
            if matches!(v.lines[i as usize].edit, Some(Edit::Row(_))) {
                return Some(i);
            }
            if v.lines[i as usize].kind != Kind::Track {
                return None;
            }
            i += dir;
        }
        None
    })
    .flatten();
    let Some(i) = target else { return };
    if !edit_commit(st) {
        return;
    }
    edit_start(st, i);
    with(|v| {
        if let Some(e) = v.editing.as_mut() {
            e.slot = slot.min(e.values.len().saturating_sub(1));
        }
    });
    sync_edit(st);
    bump_focus(st);
}

/// Give the value being edited the keyboard: now, and once more after this turn of the
/// event loop (a field that replaces another, as Tab or Down open the next one, is made
/// while the one that had the keyboard is still going away).
fn bump_focus(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    m.set_edit_focus_seq(m.get_edit_focus_seq() + 1);
    let weak = st.app.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(app) = weak.upgrade() {
            let m = app.global::<Meta>();
            if m.get_edit_row() >= 0 {
                m.set_edit_focus_seq(m.get_edit_focus_seq() + 1);
            }
        }
    });
}

fn refresh_values(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        let values = with(|v| v.editing.as_ref().map(|e| e.values.clone())).flatten().unwrap_or_default();
        app.global::<Meta>().set_edit_values(ModelRc::new(VecModel::from(values.into_iter().map(SharedString::from).collect::<Vec<_>>())));
    }
    sync_edit(st);
    bump_focus(st);
}

/// Ctrl+Enter or "Add a value": another value after this one.
pub fn edit_add_value(st: &Rc<State>, slot: i32) {
    with(|v| {
        if let Some(e) = v.editing.as_mut().filter(|e| !matches!(e.edit, Edit::Row(_))) {
            let at = (slot.max(0) as usize + 1).min(e.values.len());
            e.values.insert(at, String::new());
            e.slot = at;
        }
    });
    refresh_values(st);
}

/// The × beside a value.
pub fn edit_remove_value(st: &Rc<State>, slot: i32) {
    with(|v| {
        if let Some(e) = v.editing.as_mut().filter(|e| !matches!(e.edit, Edit::Row(_))) {
            let s = slot.max(0) as usize;
            if s < e.values.len() {
                e.values.remove(s);
            }
            if e.values.is_empty() {
                e.values.push(String::new());
            }
            e.slot = s.min(e.values.len() - 1);
        }
    });
    refresh_values(st);
}

/// Alt+Up and Alt+Down on a value: move it.
pub fn edit_move_value(st: &Rc<State>, slot: i32, dir: i32) {
    with(|v| {
        if let Some(e) = v.editing.as_mut().filter(|e| !matches!(e.edit, Edit::Row(_))) {
            let (a, b) = (slot, slot + dir);
            if a >= 0 && b >= 0 && (a as usize) < e.values.len() && (b as usize) < e.values.len() {
                e.values.swap(a as usize, b as usize);
                e.slot = b as usize;
            }
        }
    });
    refresh_values(st);
}

/// Delete on a field: removed when saved (an album field: from every track). Again: kept.
pub fn remove_field(st: &Rc<State>, i: i32) {
    if !metadata::tagedit::enabled(st) || with(|v| v.stage != Stage::View || v.editing.is_some()).unwrap_or(true) {
        return;
    }
    let Some(line) = with(|v| v.lines.get(i.max(0) as usize).cloned()).flatten() else { return };
    match line.edit {
        Some(Edit::Field(k) | Edit::Remove(k)) => toggle_removed(st, &k),
        Some(Edit::Album(k)) => {
            with(|v| {
                let removed = v.album.as_ref().map(|a| album_value(&a.input, &k)).is_some_and(|(now, changed, _)| changed > 0 && now.is_empty());
                let sets = if removed {
                    // Back to what each file holds.
                    let Some(a) = v.album.as_ref() else { return };
                    let paths: Vec<String> = a.input.tracks.iter().map(|t| t.path.clone()).collect();
                    for p in paths {
                        if let Some(c) = v.draft.field(&p, &k).cloned() {
                            let name = file_name(&p);
                            v.draft.set(&p, &name, None, &k, c.old.clone(), c.old);
                        }
                    }
                    return;
                } else {
                    vec![(k.clone(), Vec::new())]
                };
                apply_sets(v, &Edit::Album(k.clone()), sets);
            });
            render(st, false);
        }
        _ => {}
    }
}

/// The front cover: pick a new one, remove it, or keep it as it is.
pub fn cover(st: &Rc<State>, act: CoverAct) {
    if !metadata::tagedit::enabled(st) || with(|v| v.stage != Stage::View).unwrap_or(true) {
        return;
    }
    match act {
        CoverAct::Keep => set_cover(st, None),
        CoverAct::Remove => set_cover(st, Some(metadata::tagedit::Cover::Remove)),
        CoverAct::Pick => {
            let Some(path) = metadata::tagedit::pick_image() else { return };
            flash(st, "Reading the image\u{2026}");
            let s = session();
            let spawned = std::thread::Builder::new().name("mp-cover-pick".into()).spawn(move || {
                library::background_thread();
                let prepared = library::edit::cover::prepare(&path);
                let thumb = prepared.as_ref().ok().and_then(|p| meta::thumbnail(&p.picture.data)).map(|t| pixels(&t));
                state::post(move |st| {
                    if session() != s {
                        return;
                    }
                    match prepared {
                        Ok(p) => {
                            with(|v| v.new_cover = thumb.map(Image::from_rgba8));
                            set_cover(st, Some(metadata::tagedit::Cover::Set(Rc::new(p))));
                            flash(st, "");
                        }
                        Err(e) => flash(st, &e),
                    }
                });
            });
            if let Err(e) = spawned {
                log::warn!("cover: {e}");
            }
        }
    }
}

fn set_cover(st: &Rc<State>, c: Option<metadata::tagedit::Cover>) {
    with(|v| {
        let targets: Vec<(String, Option<library::edit::Expect>)> = if let Some(t) = v.track.as_ref() {
            match &t.input.file {
                FileRead::Read(r) if r.editable.is_ok() => vec![(t.input.track.path.clone(), r.expect.clone())],
                _ => Vec::new(),
            }
        } else if let Some(a) = v.album.as_ref() {
            editable_tracks(&a.input).into_iter().map(|i| (a.input.tracks[i].path.clone(), a.input.briefs[i].as_ref().and_then(|b| b.expect.clone()))).collect()
        } else {
            Vec::new()
        };
        for (path, expect) in targets {
            let name = file_name(&path);
            v.draft.set_cover(&path, &name, expect, c.clone());
        }
        if c.is_none() {
            v.new_cover = None;
        }
    });
    render(st, false);
}

/// An album fix into the draft (to review before saving, like any edit).
fn apply_fix(st: &Rc<State>, i: usize) {
    with(|v| {
        let Some(fix) = v.fixes.get(i).cloned() else { return };
        let Some(a) = v.album.as_ref() else { return };
        let sets: Vec<FixSet> = fix
            .tracks
            .iter()
            .filter_map(|(t, new)| {
                let track = a.input.tracks.get(*t)?;
                let b = a.input.briefs.get(*t)?.as_ref()?;
                Some((track.path.clone(), file_name(&track.path), b.expect.clone(), b.values.get(&fix.key).cloned().unwrap_or_default(), new.clone()))
            })
            .collect();
        for (path, name, expect, old, new) in sets {
            v.draft.set(&path, &name, expect, &fix.key, old, new);
        }
    });
    render(st, false);
}

/// Review and save (Ctrl+S, the draft bar's button): every change, file by file.
pub fn review(st: &Rc<State>) {
    if !metadata::tagedit::enabled(st) || metadata::tagedit::saving() {
        return;
    }
    if with(|v| v.editing.is_some()) == Some(true) && !edit_commit(st) {
        return;
    }
    let ready = with(|v| {
        if v.draft.is_empty() || v.stage != Stage::View {
            return false;
        }
        let what = match (&v.track, &v.album) {
            (Some(t), _) => format!("\u{201c}{}\u{201d}", t.input.track.title),
            (None, Some(a)) => format!("\u{201c}{}\u{201d}", a.input.facts.title),
            _ => String::new(),
        };
        v.review = Some(Review { edits: v.draft.edits(), label: v.draft.label(&what), undoes: None });
        v.stage = Stage::Review;
        true
    });
    match ready {
        Some(true) => show_review(st),
        _ => flash(st, "Nothing to save"),
    }
}

/// Undo a save in the Edit history: its review first.
fn review_undo(st: &Rc<State>, i: usize) {
    if !metadata::tagedit::enabled(st) || metadata::tagedit::saving() {
        return;
    }
    let j = metadata::tagedit::journal(st);
    let Some((save, _, _)) = with(|v| v.history.get(i).cloned()).flatten() else { return };
    match j.undo_edits(&save) {
        Ok(edits) => {
            with(|v| {
                v.review = Some(Review { edits, label: format!("Undo: {}", save.header.label), undoes: Some(save.header.id.clone()) });
                v.stage = Stage::Review;
            });
            show_review(st);
        }
        Err(e) => flash(st, &e),
    }
}

/// The review's rows: a heading per file, a line per change.
fn show_review(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    let Some((title, note, rows, n)) = with(|v| {
        let r = v.review.as_ref()?;
        let n = r.edits.len();
        let mut lines = Vec::new();
        for e in &r.edits {
            lines.push(Line::heading(&file_name(&e.path.to_string_lossy())));
            for s in &e.summary {
                lines.push(Line { kind: Kind::Note, values: vec![s.clone()], ..Default::default() });
            }
        }
        let files = if n == 1 { "1 file".to_owned() } else { format!("{n} files") };
        let undo = r.undoes.is_some();
        let title = if undo { format!("Undo this save? {files} change back") } else { format!("Save these changes? {files} will change") };
        let note = if undo {
            "Each file is put back as it was before that save, if it is still as the save left it; one changed since is left alone and named.".to_owned()
        } else {
            "Each file is written as a copy beside it and checked (its other tags, pictures and audio unchanged) before it takes the file's place; a file that does not check out is left as it is. Every save can be undone from Edit history.".to_owned()
        };
        Some((title, note, lines.iter().map(|l| to_row(l, false, &[], None)).collect::<Vec<_>>(), n))
    })
    .flatten() else {
        return;
    };
    log::info!("tag editor: {title}");
    m.set_review_title(title.into());
    m.set_review_note(note.into());
    m.set_review_rows(ModelRc::new(VecModel::from(rows)));
    m.set_save_label(if with(|v| v.review.as_ref().is_some_and(|r| r.undoes.is_some())) == Some(true) { format!("Undo in {}", if n == 1 { "1 file".into() } else { format!("{n} files") }) } else if n == 1 { "Save 1 file".into() } else { format!("Save {n} files") }.into());
    sync_edit(st);
    bump_overlay(&m);
}

/// Give the review, progress, result or question the keyboard: now, and once more after
/// this turn of the event loop (the stage before it, which had the keyboard, is still
/// going away).
fn bump_overlay(m: &Meta<'_>) {
    m.set_overlay_seq(m.get_overlay_seq() + 1);
    let weak = state::with(|st| st.app.clone());
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(app) = weak.and_then(|w| w.upgrade()) {
            let m = app.global::<Meta>();
            m.set_overlay_seq(m.get_overlay_seq() + 1);
        }
    });
}

/// Back from the review to editing.
pub fn back_to_edit(st: &Rc<State>) {
    with(|v| {
        if matches!(v.stage, Stage::Review | Stage::Confirm) {
            v.stage = Stage::View;
            v.review = None;
        }
    });
    sync_edit(st);
    if let Some(app) = st.app.upgrade() {
        app.global::<Meta>().set_focus_seq(app.global::<Meta>().get_focus_seq() + 1);
    }
}

/// Save what the review lists.
pub fn save(st: &Rc<State>) {
    let Some(r) = with(|v| if v.stage == Stage::Review { v.review.take() } else { None }).flatten() else { return };
    let total = r.edits.len();
    log::info!("tag editor: saving {total} files ({})", r.label);
    let started = metadata::tagedit::save(st, r.edits, r.label, r.undoes, on_saving);
    if !started {
        flash(st, "Saving is not possible now");
        return;
    }
    with(|v| v.stage = Stage::Saving);
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        m.set_progress(0.0);
        m.set_progress_text(format!("Saving 1 of {total}\u{2026}").into());
        m.set_can_cancel(total > 1);
    }
    sync_edit(st);
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        bump_overlay(&m);
    }
}

/// Cancel: stop after the file being written.
pub fn cancel_save(st: &Rc<State>) {
    metadata::tagedit::cancel();
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        m.set_can_cancel(false);
        m.set_progress_text("Stopping after the file being saved\u{2026}".into());
    }
}

fn on_saving(st: &Rc<State>, msg: metadata::tagedit::Saving) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<Meta>();
    match msg {
        metadata::tagedit::Saving::Progress { done, total, name } => {
            if total > 0 {
                m.set_progress(done as f32 / total as f32);
            }
            if done < total && m.get_can_cancel() {
                m.set_progress_text(format!("Saving {} of {total}: {name}", done + 1).into());
            }
        }
        metadata::tagedit::Saving::Done(o) => {
            let (title, text, attention) = metadata::tagedit::outcome_text(&o);
            log::info!("tag editor: {title} {text}");
            let written: Vec<PathBuf> = o.report.saved.iter().map(|s| s.path.clone()).chain(o.report.unchanged.iter().cloned()).collect();
            let open = with(|v| {
                // What was written is no longer a draft; what was not stays to try again.
                for p in &written {
                    v.draft.files.remove(&*p.to_string_lossy());
                }
                if v.draft.files.values().all(|f| f.cover.is_none()) {
                    v.new_cover = None;
                }
                v.stage = Stage::Result;
                v.outcome = Some((*o).clone());
            })
            .is_some();
            m.set_result_title(title.into());
            m.set_result_text(text.into());
            m.set_result_attention(attention);
            m.set_result_can_undo(attention && !o.report.saved.is_empty() && o.id.is_some());
            m.set_progress(1.0);
            metadata::tagedit::sync_settings(st);
            if !o.report.saved.is_empty() {
                let saved: Vec<PathBuf> = o.report.saved.iter().map(|s| s.path.clone()).collect();
                with(|v| v.awaiting_scan = true);
                metadata::tagedit::rescan_saved(st, &saved);
                if open {
                    reread(st);
                }
            }
            if open {
                refresh_history(st);
                sync_edit(st);
                bump_overlay(&m);
            }
        }
    }
}

/// Read the files shown again (after a save): the view shows what they hold now.
fn reread(st: &Rc<State>) {
    let editing = metadata::tagedit::enabled(st);
    let what = with(|v| {
        if let Some(t) = v.track.as_mut() {
            t.input.file = FileRead::Pending;
            let seq = TRACK_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
            t.seq = seq;
            return Some((Some((t.input.track.path.clone(), seq)), None));
        }
        let a = v.album.as_mut()?;
        let n = a.input.tracks.len();
        a.input.files = vec![FileBrief::Pending; n];
        a.input.briefs = vec![None; n];
        Some((None, Some((a.input.facts.clone(), a.input.tracks.clone()))))
    })
    .flatten();
    render(st, false);
    match what {
        Some((Some((path, seq)), _)) => read_track(path, session(), seq, editing),
        Some((None, Some((facts, tracks)))) => read_album(facts, tracks, editing),
        _ => {}
    }
}

/// The result's Done (Enter).
pub fn result_done(st: &Rc<State>) {
    with(|v| {
        if v.stage == Stage::Result {
            v.stage = Stage::View;
            v.outcome = None;
        }
    });
    sync_edit(st);
    if let Some(app) = st.app.upgrade() {
        app.global::<Meta>().set_focus_seq(app.global::<Meta>().get_focus_seq() + 1);
    }
}

/// After a save that stopped half way: undo what it did write.
pub fn undo_result(st: &Rc<State>) {
    let Some(id) = with(|v| v.outcome.as_ref().and_then(|o| o.id.clone())).flatten() else { return };
    // Straight from the result to the review (the list does not take the keyboard between).
    with(|v| {
        v.stage = Stage::View;
        v.outcome = None;
    });
    let j = metadata::tagedit::journal(st);
    let Some(save) = j.get(&id) else { return };
    match j.undo_edits(&save) {
        Ok(edits) => {
            with(|v| {
                v.review = Some(Review { edits, label: format!("Undo: {}", save.header.label), undoes: Some(save.header.id.clone()) });
                v.stage = Stage::Review;
            });
            show_review(st);
        }
        Err(e) => flash(st, &e),
    }
}

/// The Edit history of what is shown, from the journal.
fn refresh_history(st: &Rc<State>) {
    if !metadata::tagedit::enabled(st) {
        return;
    }
    let paths = with(|v| match (&v.track, &v.album) {
        (Some(t), _) => vec![t.input.track.path.clone()],
        (None, Some(a)) => a.input.tracks.iter().map(|t| t.path.clone()).collect(),
        _ => Vec::new(),
    })
    .unwrap_or_default();
    let h = metadata::tagedit::history_for(&metadata::tagedit::journal(st), &paths);
    with(|v| v.history = h);
    render(st, false);
}

/// After a scan changed the library: what the open panel says the library holds is read
/// again (the track's row, its album; an album found again by its tracks when a retag gave
/// it a new identity). The files are not read again here; the draft stays. Nothing runs
/// with the panel closed.
pub fn library_changed(st: &Rc<State>) {
    // Only after a save from the panel: otherwise the viewer is as it always was.
    let Some((track_id, album_track)) = with(|v| {
        let ids = (v.track.as_ref().map(|t| t.input.track.id).filter(|id| *id > 0), v.album.as_ref().and_then(|a| a.input.tracks.first().map(|t| t.id)));
        std::mem::take(&mut v.awaiting_scan).then_some(ids)
    })
    .flatten() else {
        return;
    };
    let mut changed = false;
    if let Some(id) = track_id
        && let Ok(Some(track)) = library::query::track(&st.db, id)
    {
        let album = library::query::album_facts(&st.db, track.album_id).ok().flatten();
        let genre = library::query::track_genre(&st.db, id).ok().flatten();
        let credits = library::query::track_artists(&st.db, id).unwrap_or_default();
        let genres = library::query::track_genre_names(&st.db, id).unwrap_or_default();
        with(|v| {
            if let Some(t) = v.track.as_mut().filter(|t| t.input.track.id == id) {
                t.input.track = Track { art: t.input.track.art.clone(), ..track };
                t.input.album = album;
                t.input.genre = genre;
                t.input.credits = credits;
                t.input.genres = genres;
                changed = true;
            }
        });
    }
    if let Some(first) = album_track
        && let Ok(Some(t)) = library::query::track(&st.db, first)
        && let Ok(Some(facts)) = library::query::album_facts(&st.db, t.album_id)
    {
        let tracks = library::query::album_tracks(&st.db, t.album_id).unwrap_or_default();
        let reread = with(|v| {
            let a = v.album.as_mut()?;
            let same = a.input.tracks.iter().map(|t| t.id).eq(tracks.iter().map(|t| t.id));
            a.input.facts = facts.clone();
            if same {
                a.input.tracks = tracks.clone();
                None
            } else {
                // Other tracks now (a regroup): their files are read for the table.
                let n = tracks.len();
                a.input.tracks = tracks.clone();
                a.input.files = vec![FileBrief::Pending; n];
                a.input.briefs = vec![None; n];
                Some((facts.clone(), tracks.clone()))
            }
        })
        .flatten();
        changed = true;
        if let Some((facts, tracks)) = reread {
            read_album(facts, tracks, metadata::tagedit::enabled(st));
        }
    }
    if changed {
        render(st, false);
    }
}

/// Give the list the keyboard (after a field closes, a button that went away).
pub fn focus_list(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        let m = app.global::<Meta>();
        m.set_focus_seq(m.get_focus_seq() + 1);
    }
}

/// Discard every unsaved change.
pub fn discard(st: &Rc<State>) {
    with(|v| {
        v.draft = metadata::tagedit::Draft::default();
        v.editing = None;
        v.new_cover = None;
        if v.stage == Stage::Confirm {
            v.stage = Stage::View;
        }
    });
    render(st, false);
    flash(st, "Changes discarded");
    focus_list(st);
}

/// Esc, Close, a click outside: with unsaved changes, asked first; while saving, not now.
pub fn close_request(st: &Rc<State>) {
    let stage = with(|v| (v.stage, !v.draft.is_empty())).unwrap_or((Stage::View, false));
    match stage {
        (Stage::Saving, _) => flash(st, "Saving: Cancel stops after the file being saved"),
        (Stage::Review | Stage::Confirm, _) => back_to_edit(st),
        (Stage::Result, _) => result_done(st),
        (Stage::View, true) => {
            with(|v| {
                v.editing = None;
                v.stage = Stage::Confirm;
            });
            sync_edit(st);
            if let Some(app) = st.app.upgrade() {
                let m = app.global::<Meta>();
                bump_overlay(&m);
            }
        }
        (Stage::View, false) => close(st),
    }
}

/// Why quitting now would lose work in this panel: unsaved tag edits, or a save under way.
/// `None` when nothing would be lost.
pub fn blocks_quit() -> Option<&'static str> {
    with(|v| match (v.stage, v.draft.is_empty()) {
        (Stage::Saving, _) => Some("Tags are being saved: quit again when the save is done (or Cancel it)"),
        (_, false) => Some("You have unsaved tag changes: save or discard them, then quit again"),
        _ => None,
    })
    .flatten()
}

/// "Discard and close".
pub fn discard_and_close(st: &Rc<State>) {
    with(|v| v.draft = metadata::tagedit::Draft::default());
    close(st);
}

/// A key the panel did not take: the tag editing shortcuts (Settings > Keys).
pub fn shortcut(st: &Rc<State>, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    if !metadata::tagedit::enabled(st) {
        return false;
    }
    let action = st.keymap.borrow().action_for(text, ctrl, shift, alt);
    match action {
        Some(crate::keys::map::Action::SaveTags) => {
            review(st);
            true
        }
        Some(crate::keys::map::Action::EditField) => {
            let cursor = st.app.upgrade().map(|a| a.global::<Meta>().get_cursor()).unwrap_or(-1);
            edit_start(st, cursor);
            true
        }
        _ => false,
    }
}


#[cfg(feature = "bench")]
/// The lines on screen, for the bench.
pub fn lines() -> Vec<Line> {
    with(|v| v.lines.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::audio::Codec;
    use library::meta::{RawItem, ReadOptions};

    fn flac() -> AudioInfo {
        AudioInfo { codec: Codec::Flac, sample_rate: Some(44_100), bit_depth: Some(16), bitrate: Some(900), channels: Some(2) }
    }

    fn items(block: usize, kv: &[(&str, &[&str])]) -> Vec<RawItem> {
        kv.iter().map(|(k, v)| RawItem { block, key: (*k).into(), values: v.iter().map(|s| (*s).to_owned()).collect(), ..Default::default() }).collect()
    }

    fn read_file(blocks: Vec<BlockKind>, items: Vec<RawItem>, scanned: Option<TrackTags>) -> FileRead {
        let tags = FileTags { blocks, items, ..Default::default() };
        let fields = meta::fields(&tags);
        FileRead::Read(Box::new(ReadFile { tags, fields, scanned, ..Default::default() }))
    }

    fn labels(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| if l.kind == Kind::Heading { format!("# {}", l.label) } else { l.label.clone() }).collect()
    }

    #[test]
    fn long_values_are_cut_until_expanded() {
        let short = vec!["One".to_owned()];
        assert_eq!(shown(&short, false), (short.clone(), false));
        let long = vec!["x".repeat(1000)];
        let (v, more) = shown(&long, false);
        assert!(more && v[0].replace('\u{200b}', "").chars().count() == PREVIEW_CHARS + 1 && v[0].ends_with('\u{2026}'));
        let (v, more) = shown(&long, true);
        assert!(more, "still expandable (to fold it)");
        assert_eq!(v[0].replace('\u{200b}', ""), long[0], "unbroken: breaks added, nothing else");
        assert_eq!(breakable("a few words"), "a few words");
        assert_eq!(breakable(&"x".repeat(50)).chars().filter(|c| *c == '\u{200b}').count(), 2);
        let lines = vec![(1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")];
        assert_eq!(shown(&lines, false).0[0].lines().count(), PREVIEW_LINES);
        let huge = vec!["y".repeat(EXPANDED_CHARS + 50)];
        assert!(shown(&huge, true).0[0].contains("50 more characters"));
        let many: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        let (v, _) = shown(&many, false);
        assert_eq!(v.len(), MAX_VALUES + 1);
        assert_eq!(v.last().unwrap(), "and 10 more values");
    }

    #[test]
    fn a_track_view_in_order_with_blocks_that_disagree() {
        let file = read_file(
            vec![BlockKind::Id3v2(3), BlockKind::Id3v1],
            [
                items(0, &[("TIT2", &["A Title Longer Than Thirty Characters"]), ("TPE1", &["A", "B"]), ("TRCK", &["3/12"]), ("TXXX:MusicBrainz Album Id", &["abc"]), ("TXXX:Odd", &["?"]), ("TSOP", &["A, The"])]),
                items(1, &[("Title", &["A Title Longer Than Thirty Cha"]), ("Artist", &["A"])]),
            ]
            .concat(),
            None,
        );
        let t = TrackInput { track: Track { id: -1, title: "A Title".into(), path: "x.mp3".into(), ..Default::default() }, file, ..Default::default() };
        let lines = track_lines(&t);
        let l = labels(&lines);
        let pos = |s: &str| l.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} in {l:?}"));
        assert!(pos("# In the library") < pos("# Tags") && pos("# Tags") < pos("# Identifiers") && pos("# Identifiers") < pos("# Sort names"));
        assert!(pos("# Sort names") < pos("# Pictures") && pos("# Pictures") < pos("# Lyrics") && pos("# Lyrics") < pos("# File"));
        assert!(pos("# File") < pos("# Other tags (1)"), "the long tail last");
        // The title: one line per block, both marked.
        let titles: Vec<&Line> = lines.iter().filter(|x| x.label == "Title" && x.field.is_some()).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.iter().all(|x| x.attention));
        assert_eq!(titles[1].note, "ID3v1: differs from ID3v2.3");
        assert_eq!(titles[1].field, Some(FieldRef { key: "TITLE".into(), block: 1, raw_key: "Title".into() }));
        // Artist: also in both blocks, but they differ (two values against one).
        assert_eq!(lines.iter().filter(|x| x.label == "Artist" && x.field.is_some()).count(), 2);
        let track = lines.iter().find(|x| x.label == "Track").unwrap();
        assert_eq!(track.values, ["3 of 12"]);
        assert_eq!(track.copy, "3/12", "Copy gives the value as tagged");
        assert_eq!(track.note, "ID3v2.3", "several blocks: where it is");
        assert!(lines.iter().any(|x| x.label == "TXXX:Odd"));
        assert!(lines.iter().any(|x| x.kind == Kind::Note && x.values[0] == "No embedded pictures."));
        assert!(lines.iter().any(|x| x.label == "Lyrics" && x.values == ["None in the tags"]));
    }

    #[test]
    fn the_library_says_where_it_differs_from_the_tags() {
        let scanned = TrackTags {
            title: Some("Song".into()),
            artist: Some("Guest".into()),
            album: Some("Album (Deluxe)".into()),
            album_artist: None,
            date: Some("2019-03-08".into()),
            genre: Some("Pop".into()),
            track_no: Some(2),
            ..Default::default()
        };
        let file = read_file(vec![BlockKind::Vorbis], items(0, &[("TITLE", &["Song"]), ("ARTIST", &["Guest"]), ("ALBUM", &["Album (Deluxe)"]), ("DATE", &["2019.03.08"]), ("GENRE", &["Pop"])]), Some(scanned));
        let album = AlbumFacts { key: "mb:abc".into(), title: "Album".into(), album_artist: "Band".into(), date: Some("2019-03-08".into()), ..Default::default() };
        let t = TrackInput {
            track: Track { id: 5, title: "Song".into(), artist: "Guest".into(), album: "Album".into(), album_artist: "Band".into(), track_no: Some(2), path: "M:/a/02.flac".into(), ..Default::default() },
            album: Some(album),
            genre: Some("Pop".into()),
            file,
            ..Default::default()
        };
        let l = library_lines(&t);
        let get = |s: &str| l.iter().find(|x| x.label == s).unwrap_or_else(|| panic!("{s}"));
        assert_eq!(get("Title").note, "");
        assert!(get("Album").attention && get("Album").note.contains("\u{201c}Album (Deluxe)\u{201d}"));
        assert!(!get("Album artist").attention, "its album's other tracks name the album artist: that holds for this one");
        assert_eq!(get("Album artist").note, "No album artist tag on this file: the album's other tracks name it");
        assert_eq!(get("Date").note, "Read from \u{201c}2019.03.08\u{201d} as \u{201c}2019-03-08\u{201d}");
        assert_eq!(get("Disc and track").values, ["Track 2"]);
        assert_eq!(get("Album grouped by").values, ["Its MusicBrainz release"]);
        assert!(get("Album grouped by").attention, "no MusicBrainz id in this file: it would land elsewhere");
    }

    #[test]
    fn a_missing_file_shows_what_the_library_knows() {
        let t = TrackInput { track: Track { id: 3, title: "Gone".into(), path: "M:/gone.flac".into(), duration: 61.0, ..Default::default() }, file: FileRead::Missing, ..Default::default() };
        let lines = track_lines(&t);
        assert!(lines.iter().any(|l| l.kind == Kind::Note && l.attention && l.values[0].starts_with("The file is not there")));
        assert!(lines.iter().any(|l| l.label == "Path" && l.values == ["M:/gone.flac"]));
        assert!(lines.iter().any(|l| l.label == "Duration" && l.values == ["1:01"]));
    }

    #[test]
    fn many_other_tags_come_a_page_at_a_time() {
        let many: Vec<RawItem> = (0..700).map(|i| RawItem { block: 0, key: format!("X_{i:04}"), values: vec!["v".into()], ..Default::default() }).collect();
        let t = TrackInput { track: Track { id: -1, ..Default::default() }, file: read_file(vec![BlockKind::Vorbis], many, None), ..Default::default() };
        let lines = track_lines(&t);
        assert_eq!(lines.iter().filter(|l| l.label.starts_with("X_")).count(), PAGE_ROWS);
        let more = lines.last().unwrap();
        assert_eq!((more.act, more.values[0].as_str()), (Act::More(Section::OtherTags), "Show 300 more (400 not listed yet)"));
        assert_eq!(more_text(12), "Show the other 12");
        assert!(more.selectable());
        let all = track_copy_lines(&t);
        assert_eq!(all.iter().filter(|l| l.label.starts_with("X_")).count(), 700, "Copy all has every one");
    }

    fn rg_brief(album: &str, aa: Option<&str>, date: &str, genre: &str, rg: bool) -> FileBrief {
        FileBrief::Read(Brief { album: Some(album.into()), album_artist: aa.map(str::to_owned), date: Some(date.into()), genre: Some(genre.into()), mb_album: Some("mb".into()), replaygain: rg })
    }

    #[test]
    fn inconsistent_tagging_is_spotted() {
        let mp3 = AudioInfo { codec: Codec::Mp3, sample_rate: Some(44_100), bitrate: Some(192), channels: Some(2), bit_depth: None };
        let row = |no: Option<u32>, audio, file| TrackFacts { disc: None, number: no, audio, file };
        let t = vec![
            row(Some(1), flac(), rg_brief("P", Some("M"), "2021", "Indie", true)),
            row(Some(2), flac(), rg_brief("P", Some("M"), "2021", "Indie", true)),
            row(Some(3), flac(), rg_brief("P (Deluxe)", Some("M"), "2021", "Indie", true)),
            row(Some(5), flac(), rg_brief("P", Some("M"), "2020", "Indie", true)),
            row(Some(6), flac(), rg_brief("P", None, "2021", "Pop", false)),
            row(None, flac(), rg_brief("P", Some("M"), "2021", "Indie", true)),
            row(Some(7), mp3, rg_brief("P", Some("M"), "2021", "Indie", true)),
            row(Some(8), flac(), FileBrief::Missing),
        ];
        let c = check_album(&t);
        let s: HashMap<&str, &str> = c.summary.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        assert_eq!(s["Album"], "\u{201c}P\u{201d} on 6 tracks, \u{201c}P (Deluxe)\u{201d} on 1 track");
        assert_eq!(s["Album artist"], "\u{201c}M\u{201d} on 6 tracks, none on 1 track");
        assert_eq!(s["Track numbers"], "No track 4; 1 track without a number");
        assert_eq!(s["ReplayGain"], "Missing on 1 of 7 tracks");
        assert_eq!(s["Formats"], "FLAC 16/44.1 on 7 tracks, MP3 on 1 track");
        assert_eq!(s["Files"], "1 track missing");
        assert_eq!(c.rows[2], ["Album \u{201c}P (Deluxe)\u{201d}"]);
        assert_eq!(c.rows[3], ["Date \u{201c}2020\u{201d}"]);
        assert_eq!(c.rows[4], ["No album artist tag", "Genre \u{201c}Pop\u{201d}", "No ReplayGain"]);
        assert_eq!(c.rows[5], ["No track number"]);
        assert_eq!(c.rows[7], ["File missing"]);
        assert!(c.number[5] && !c.number[0]);
        assert!(c.format[6] && !c.format[0]);
        // An album that agrees: nothing.
        let ok = vec![row(Some(1), flac(), rg_brief("P", Some("M"), "2021", "Indie", true)), row(Some(2), flac(), rg_brief("P", Some("M"), "2021", "Indie", true))];
        assert_eq!(check_album(&ok).summary, Vec::<(String, String)>::new());
        // Numbers used twice, on two discs.
        let d = |disc, no| TrackFacts { disc: Some(disc), number: Some(no), audio: flac(), file: FileBrief::Pending };
        let c = check_album(&[d(1, 1), d(1, 1), d(2, 1), d(2, 3)]);
        assert_eq!(c.summary, [("Track numbers".to_owned(), "No track 2 on disc 2; used twice: 1 (disc 1)".to_owned())]);
        assert!(c.number[0] && c.number[1] && !c.number[2]);
    }

    #[test]
    fn a_huge_track_number_lists_its_gaps_without_walking_them() {
        // A mistyped tag: track 4000000000. Listing every missing number would take minutes
        // and gigabytes; only the first 12 are shown.
        let d = |no| TrackFacts { disc: None, number: Some(no), audio: flac(), file: FileBrief::Pending };
        let started = std::time::Instant::now();
        let c = check_album(&[d(1), d(3), d(4_000_000_000)]);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
        assert_eq!(c.summary, [("Track numbers".to_owned(), "No track 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, \u{2026}".to_owned())]);
        // Exactly 12 missing: no ellipsis.
        let c = check_album(&[d(1), d(14)]);
        assert_eq!(c.summary, [("Track numbers".to_owned(), "No track 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13".to_owned())]);
    }

    #[test]
    fn the_album_says_when_its_value_is_not_what_most_tracks_say() {
        let t = |title: &str| Track { title: title.into(), audio: flac(), ..Default::default() };
        let a = AlbumInput {
            facts: AlbumFacts { key: "mb:x".into(), title: "P (Deluxe)".into(), album_artist: "M".into(), date: Some("2021".into()), ..Default::default() },
            tracks: vec![t("1"), t("2"), t("3")],
            files: vec![rg_brief("P", Some("M"), "2021", "Indie", true), rg_brief("P", Some("M"), "2021", "Indie", true), rg_brief("P (Deluxe)", Some("M"), "2021", "Indie", true)],
            ..Default::default()
        };
        let lines = album_lines(&a);
        let title = lines.iter().find(|l| l.label == "Title").unwrap();
        assert!(title.attention);
        assert_eq!(title.note, "2 of its 3 tracks now say \u{201c}P\u{201d}; the next scan takes it");
        assert!(!lines.iter().find(|l| l.label == "Album artist").unwrap().attention);
    }

    #[test]
    fn copy_all_reads_as_text() {
        let lines = vec![
            Line::heading("Tags"),
            Line::field("Artist", "A; B"),
            Line { label: "Comment".into(), values: vec!["one\ntwo".into()], copy: "one\ntwo".into(), note: "ID3v1".into(), ..Default::default() },
            Line { kind: Kind::Track, cells: ["1", "3", "Song", "Band", "3:12", "FLAC 16/44.1"].map(str::to_owned).to_vec(), note: "No ReplayGain".into(), ..Default::default() },
            Line { kind: Kind::Note, values: vec!["Show 3 more".into()], act: Act::More(Section::Tracks), ..Default::default() },
        ];
        let text = copy_all(&["Track: Song".into(), String::new(), "M:/x.flac".into()], &lines);
        assert_eq!(text, "Track: Song\nM:/x.flac\n\nTags\n  Artist: A; B\n  Comment: one\n      two  (ID3v1)\n    1.3  Song \u{2014} Band  3:12  FLAC 16/44.1  [No ReplayGain]\n");
    }

    #[test]
    fn helpers() {
        assert_eq!(r128_db("-1234").as_deref(), Some("\u{2212}4.82 dB, relative to the Opus header gain"));
        assert_eq!(r128_db("x"), None);
        assert_eq!(of_total("3/12"), "3 of 12");
        assert_eq!(of_total("3"), "3");
        assert_eq!(lyrics_summary("[00:01.00]a\n[00:02.00]b"), "23 characters \u{b7} 2 lines \u{b7} synced (time stamps)");
        assert_eq!(lyrics_summary("just one"), "8 characters \u{b7} 1 line \u{b7} plain text");
        assert_eq!(picture_text("PNG", Some((1200, 1200)), 24_254), "PNG \u{b7} 1,200 \u{d7} 1,200 px \u{b7} 24 KB");
        assert_eq!(grouped_by("t:a|b|c").0, "Album artist, title and folder");
        assert_eq!(grouped_by("nd:al-1"), (String::from("The server's album"), String::from("No MusicBrainz release id in the server's catalog")), "no folder, no tags");
        assert_eq!(grouped_by("mb:abc").0, "Its MusicBrainz release");
        let _ = ReadOptions::TAGS;
    }

    // ---- Tag editing -------------------------------------------------------------------

    fn editable_file(items: Vec<RawItem>, scanned: Option<TrackTags>) -> FileRead {
        let FileRead::Read(mut r) = read_file(vec![BlockKind::Vorbis], items, scanned) else { unreachable!() };
        r.editable = Ok(());
        r.expect = Some(library::edit::Expect::Any);
        FileRead::Read(r)
    }

    fn song(path: &str) -> Track {
        Track { id: 9, title: "Old".into(), artist: "Band".into(), album: "Al".into(), album_artist: "Band".into(), path: path.into(), ..Default::default() }
    }

    #[test]
    fn with_editing_off_the_view_is_the_viewer() {
        let file = read_file(vec![BlockKind::Vorbis], items(0, &[("TITLE", &["Old"]), ("MUSICBRAINZ_ALBUMID", &["x"])]), None);
        let t = TrackInput { track: song("M:/a/1.flac"), file, ..Default::default() };
        let lines = track_lines(&t);
        assert!(lines.iter().all(|l| l.edit.is_none() && !l.changed && !l.empty), "nothing editable, nothing marked");
        assert!(!lines.iter().any(|l| matches!(l.act, Act::AllFields | Act::Cover(_) | Act::Undo(_) | Act::Fix(_))));
        assert!(!lines.iter().any(|l| l.label == "Edit history"));
        // A file that can be edited, with editing off: the same.
        let t2 = TrackInput { file: editable_file(items(0, &[("TITLE", &["Old"]), ("MUSICBRAINZ_ALBUMID", &["x"])]), None), ..t.clone() };
        assert_eq!(track_lines(&t2), lines);
    }

    #[test]
    fn editing_marks_what_can_be_changed_and_offers_the_common_fields() {
        let file = editable_file(items(0, &[("TITLE", &["Old"]), ("ARTIST", &["Band"]), ("MUSICBRAINZ_ALBUMID", &["x"]), ("REPLAYGAIN_TRACK_GAIN", &["-1 dB"]), ("MY_TAG", &["y"])]), None);
        let t = TrackInput { track: song("M:/a/1.flac"), file, edit: Some(EditView::default()), ..Default::default() };
        let lines = track_lines(&t);
        let get = |label: &str| lines.iter().find(|l| l.label == label && l.kind == Kind::Field && l.field.is_some() || l.label == label && l.empty).cloned();
        assert_eq!(get("Title").unwrap().edit, Some(Edit::Field("TITLE".into())));
        assert!(get("Artist").unwrap().multi, "several artists can be added");
        let composer = lines.iter().find(|l| l.label == "Composer").unwrap();
        assert!(composer.empty && composer.edit == Some(Edit::Field("COMPOSER".into())), "a common field the file lacks: there to fill in");
        assert_eq!(lines.iter().find(|l| l.label == "MusicBrainz release").unwrap().edit, Some(Edit::Remove("MUSICBRAINZ_ALBUMID".into())), "an identifier is only removed");
        assert_eq!(lines.iter().find(|l| l.label == "ReplayGain track gain").unwrap().edit, None, "loudness: view only");
        assert_eq!(lines.iter().find(|l| l.label == "MY_TAG").unwrap().edit, None, "unknown raw items: view only");
        assert!(lines.iter().any(|l| l.act == Act::AllFields));
        assert!(lines.iter().any(|l| l.act == Act::Cover(CoverAct::Pick)), "a cover can be added");
        assert!(lines.iter().any(|l| l.label == "Edit history"));
        // Every field on request.
        let all = track_lines(&TrackInput { edit: Some(EditView { all_fields: true, ..Default::default() }), ..t });
        assert!(all.iter().any(|l| l.label == "Conductor" && l.empty));
        assert!(all.iter().any(|l| l.label == "Album artist sort" && l.empty));
        assert!(!all.iter().any(|l| l.act == Act::AllFields));
    }

    #[test]
    fn a_draft_shows_in_place_and_in_what_the_library_will_show() {
        let scanned = TrackTags { title: Some("Old".into()), artist: Some("Band".into()), album: Some("Al".into()), album_artist: Some("Band".into()), ..Default::default() };
        let file = editable_file(items(0, &[("TITLE", &["Old"]), ("ARTIST", &["Band"]), ("ALBUM", &["Al"]), ("ALBUMARTIST", &["Band"])]), Some(scanned));
        let mut draft = metadata::tagedit::Draft::default();
        draft.set("M:/a/1.flac", "1.flac", None, "TITLE", vec!["Old".into()], vec!["New (feat. Guest)".into()]);
        draft.set("M:/a/1.flac", "1.flac", None, "COMMENT", vec![], vec!["Hello".into()]);
        let album = AlbumFacts { key: "t:band|al|m:/a".into(), title: "Al".into(), album_artist: "Band".into(), ..Default::default() };
        let t = TrackInput { track: song("M:/a/1.flac"), album: Some(album), file, edit: Some(EditView { draft, ..Default::default() }), ..Default::default() };
        let lines = track_lines(&t);
        let title = lines.iter().find(|l| matches!(&l.edit, Some(Edit::Field(k)) if k == "TITLE")).unwrap();
        assert!(title.changed && title.values == ["New (feat. Guest)"] && title.note == "Was \u{201c}Old\u{201d}");
        let comment = lines.iter().find(|l| matches!(&l.edit, Some(Edit::Field(k)) if k == "COMMENT")).unwrap();
        assert_eq!(comment.note, "Not set before: added when you save");
        // In the library: the title once saved, and the featured guest credited.
        let lib: Vec<&Line> = lines.iter().take_while(|l| l.label != "Tags").collect();
        let ltitle = lib.iter().find(|l| l.label == "Title").unwrap();
        assert!(ltitle.changed && !ltitle.attention && ltitle.note == "Once saved, the library shows \u{201c}New (feat. Guest)\u{201d}", "{ltitle:?}");
        let d = metadata::tagedit::Draft::default();
        let _ = d;
    }

    #[test]
    fn a_file_that_cannot_be_edited_says_why_and_offers_nothing() {
        let FileRead::Read(mut r) = read_file(vec![BlockKind::Id3v2(2)], items(0, &[("TT2", &["x"])]), None) else { unreachable!() };
        r.editable = Err("its tag is ID3v2.2, an old version the editor does not write".into());
        let t = TrackInput { track: song("M:/a/1.mp3"), file: FileRead::Read(r), edit: Some(EditView::default()), ..Default::default() };
        let lines = track_lines(&t);
        assert!(lines.iter().any(|l| l.kind == Kind::Note && l.values[0] == "This file cannot be edited: its tag is ID3v2.2, an old version the editor does not write."));
        assert!(lines.iter().all(|l| l.edit.is_none()));
        assert!(!lines.iter().any(|l| matches!(l.act, Act::Cover(_))));
    }

    fn edit_brief(values: &[(&str, &[&str])]) -> Option<EditBrief> {
        Some(EditBrief {
            values: values.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect(),
            fronts: 0,
            expect: Some(library::edit::Expect::Any),
            editable: Ok(()),
        })
    }

    #[test]
    fn an_album_edits_its_fields_on_every_track_and_offers_its_fixes() {
        let t = |n: Option<u32>, title: &str, path: &str| Track { title: title.into(), track_no: n, path: path.into(), audio: flac(), ..Default::default() };
        let tracks = vec![t(Some(1), "One", "M:/p/01.flac"), t(Some(1), "Two", "M:/p/02.flac"), t(None, "Three", "M:/p/03.flac")];
        let files = vec![rg_brief("P", Some("M"), "2021", "Indie", true), rg_brief("P", Some("M"), "2021", "Indie", true), rg_brief("P", None, "2021", "Indie", true)];
        let briefs = vec![
            edit_brief(&[("ALBUM", &["P"]), ("ALBUMARTIST", &["M"]), ("TRACKNUMBER", &["1"])]),
            edit_brief(&[("ALBUM", &["P"]), ("ALBUMARTIST", &["M"]), ("TRACKNUMBER", &["1"])]),
            edit_brief(&[("ALBUM", &["P"])]),
        ];
        let mut a = AlbumInput {
            facts: AlbumFacts { key: "mb:x".into(), title: "P".into(), album_artist: "M".into(), date: Some("2021".into()), ..Default::default() },
            tracks,
            files,
            briefs,
            edit: Some(EditView::default()),
            ..Default::default()
        };
        let lines = album_lines(&a);
        assert!(lines.iter().any(|l| l.kind == Kind::Note && l.values[0] == "Editing a field here sets it on all 3 tracks."));
        let title = lines.iter().find(|l| l.label == "Title").unwrap();
        assert_eq!(title.edit, Some(Edit::Album("ALBUM".into())));
        assert!(lines.iter().any(|l| l.label == "Label" && l.empty && l.edit == Some(Edit::Album("LABEL".into()))), "a field the album lacks, to fill in");
        assert!(lines.iter().any(|l| l.label == "Compilation"));
        assert!(lines.iter().filter(|l| l.kind == Kind::Track).all(|l| matches!(l.edit, Some(Edit::Row(_)))));
        // The fixes, after the checks they answer.
        let fixes = album_fixes(&a, &draft_facts(&a));
        let labels: Vec<&str> = fixes.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["Set the album artist to \u{201c}M\u{201d} on the 1 track that differs", "Number the tracks in file order (2 tracks to change)"]);
        assert_eq!(fixes[0].tracks, [(2, vec!["M".to_owned()])]);
        assert_eq!(fixes[1].tracks, [(1, vec!["2".to_owned()]), (2, vec!["3".to_owned()])], "1, 2, 3 by file name");
        let pos = |p: &dyn Fn(&Line) -> bool| lines.iter().position(p).unwrap();
        assert!(pos(&|l| l.label == "Album artist" && l.attention) < pos(&|l| l.act == Act::Fix(0)), "the fix under its check");
        // A draft on every track: the album's value, marked, and the checks as they will be.
        let mut d = metadata::tagedit::Draft::default();
        for (i, p) in ["M:/p/01.flac", "M:/p/02.flac", "M:/p/03.flac"].iter().enumerate() {
            d.set(p, p, None, "ALBUMARTIST", if i < 2 { vec!["M".into()] } else { vec![] }, vec!["New M".into()]);
        }
        d.set("M:/p/02.flac", "02", None, "TRACKNUMBER", vec!["1".into()], vec!["2".into()]);
        d.set("M:/p/03.flac", "03", None, "TRACKNUMBER", vec![], vec!["3".into()]);
        a.edit = Some(EditView { draft: d, ..Default::default() });
        let lines = album_lines(&a);
        let aa = lines.iter().find(|l| l.label == "Album artist").unwrap();
        assert!(aa.changed && aa.values == ["New M"] && aa.note == "On 3 tracks when you save; most had \u{201c}M\u{201d}", "{aa:?}");
        assert!(!lines.iter().any(|l| l.label == "Track numbers"), "numbered once saved");
        let three = lines.iter().find(|l| l.kind == Kind::Track && l.label == "Three").unwrap();
        assert!(three.changed && three.cell_changed[0] && three.cells[1] == "3" && three.note.starts_with("Before: no number"), "{three:?}");
        assert!(album_fixes(&a, &draft_facts(&a)).is_empty(), "nothing left to fix");
    }

    #[test]
    fn the_library_reads_a_draft_by_its_own_rules() {
        let s = TrackTags { title: Some("Old".into()), artist: Some("A".into()), genre: Some("Rock".into()), genres: vec!["Rock".into()], track_no: Some(4), ..Default::default() };
        let mut d = metadata::tagedit::FileDraft::default();
        d.fields.insert("ARTIST".into(), metadata::tagedit::Change { old: vec!["A".into()], new: vec!["A feat. B".into(), "C".into()] });
        d.fields.insert("GENRE".into(), metadata::tagedit::Change { old: vec!["Rock".into()], new: vec!["Hip-Hop/Rap; Jazz".into()] });
        d.fields.insert("TRACKNUMBER".into(), metadata::tagedit::Change { old: vec!["4".into()], new: vec![] });
        let t = with_draft(&s, &d, Path::new("M:/x/07 - Song.flac"));
        assert_eq!(t.artist.as_deref(), Some("A feat. B; C"));
        let credits: Vec<(&str, bool)> = t.artists.iter().map(|c| (c.name.as_str(), c.featured)).collect();
        assert_eq!(credits, [("A", false), ("B", true), ("C", false)]);
        assert_eq!(t.genres, ["Hip-Hop/Rap", "Jazz"]);
        assert_eq!((t.track_no, t.from_name.track), (Some(7), true), "no number tag: the file's name, as the scanner does");
    }
}
