//! Smart playlists: a playlist defined by rules instead of a hand-made list. It always holds
//! the tracks that match right now: evaluating it is one SQL query over the library's tables
//! (`sql.rs`), parameterised, never text pasted from what the user typed.
//!
//! A rule set (`Smart`) is conditions joined by "all of" or "any of" (`Group`); a condition
//! can itself be a group with its own all / any (the editor makes them one level deep; the
//! format and the evaluation take any depth). A condition is a field, an operator and a
//! value (`Cond`). Text matches fold case, width, kana and Latin accents as search does
//! (`fold.rs`); multi-valued fields (artists, album artists, genres) match when any value
//! does, and their negative operators ("is not", "does not contain") when none does. After
//! the conditions: an order (`Sort`, or a stable random order by `seed`) and an optional
//! limit by tracks or total length, selected by an order of its own (`Limit`).
//!
//! Stored as JSON in `playlists.rules` (schema 9), versioned (`FORMAT`). A rule, an order or
//! a limit this build does not know (from a newer build, or damaged) is kept as it was
//! (`Node::Unknown`, `Sorting::Unknown`, `Limiting::Unknown`) and written back unchanged; it
//! never crashes and never widens the result: an unknown condition matches nothing, an
//! unknown limit leaves the list empty, an unknown order falls back to the default one, and
//! each says so (`Smart::problems`).

pub mod search;
mod sql;
mod time;
mod words;

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;

use serde_json::{Map, Value as Json, json};

pub use sql::{Outcome, album_selection, count, evaluate, evaluate_ids, playlist_rules, references, would_cycle};
pub use time::{Ctx, DateSpec, Unit, Zone, ago, civil_from_days, days_from_civil, set_local_zone};
pub use words::{cond_words, length, shown_number, sort_words, summary};

/// The rule format this build writes.
pub const FORMAT: u32 = 1;

/// How deep smart playlists may refer to each other ("is in playlist" of a smart playlist
/// that refers to another...). Deeper is refused like a cycle.
pub const MAX_DEPTH: usize = 8;

/// What a field holds, which decides its operators and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Typed text (title, album, folder path).
    Text,
    /// Text whose "is" / "is not" value is picked from what the library holds (artists,
    /// album artists, genres, labels, formats); the other operators take typed text.
    Choice,
    Number,
    Date,
    Bool,
    /// Membership of another playlist.
    Playlist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Label,
    Year,
    Added,
    LastPlayed,
    Plays,
    /// Plays within a recent period (`Cond::within`).
    PlaysIn,
    Played,
    Favourite,
    AlbumFavourite,
    Duration,
    Format,
    Lossless,
    /// Plays from the Navidrome server: a track only the server has (no file here).
    Remote,
    Bitrate,
    SampleRate,
    BitDepth,
    Disc,
    TrackNo,
    Path,
    Playlist,
}

impl Field {
    /// In the order the field picker lists them.
    pub const ALL: [Field; 25] = [
        Field::Title,
        Field::Artist,
        Field::AlbumArtist,
        Field::Album,
        Field::Genre,
        Field::Label,
        Field::Year,
        Field::Favourite,
        Field::AlbumFavourite,
        Field::Plays,
        Field::PlaysIn,
        Field::Played,
        Field::LastPlayed,
        Field::Added,
        Field::Duration,
        Field::Format,
        Field::Lossless,
        Field::Remote,
        Field::Bitrate,
        Field::SampleRate,
        Field::BitDepth,
        Field::Disc,
        Field::TrackNo,
        Field::Path,
        Field::Playlist,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "album_artist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Label => "label",
            Field::Year => "year",
            Field::Added => "added",
            Field::LastPlayed => "last_played",
            Field::Plays => "plays",
            Field::PlaysIn => "plays_in",
            Field::Played => "played",
            Field::Favourite => "favourite",
            Field::AlbumFavourite => "album_favourite",
            Field::Duration => "duration",
            Field::Format => "format",
            Field::Lossless => "lossless",
            Field::Remote => "remote",
            Field::Bitrate => "bitrate",
            Field::SampleRate => "sample_rate",
            Field::BitDepth => "bit_depth",
            Field::Disc => "disc",
            Field::TrackNo => "track",
            Field::Path => "path",
            Field::Playlist => "playlist",
        }
    }

    pub fn from_key(s: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.key() == s)
    }

    /// As the editor names it.
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Artist => "Artist",
            Field::AlbumArtist => "Album artist",
            Field::Album => "Album",
            Field::Genre => "Genre",
            Field::Label => "Label",
            Field::Year => "Year",
            Field::Added => "Date added",
            Field::LastPlayed => "Last played",
            Field::Plays => "Play count",
            Field::PlaysIn => "Plays in the last",
            Field::Played => "Played",
            Field::Favourite => "Favourite",
            Field::AlbumFavourite => "Favourite album",
            Field::Duration => "Length",
            Field::Format => "Format",
            Field::Lossless => "Lossless",
            Field::Remote => "From the server",
            Field::Bitrate => "Bitrate",
            Field::SampleRate => "Sample rate",
            Field::BitDepth => "Bit depth",
            Field::Disc => "Disc number",
            Field::TrackNo => "Track number",
            Field::Path => "File path",
            Field::Playlist => "Playlist",
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Field::Title | Field::Album | Field::Path => Kind::Text,
            Field::Artist | Field::AlbumArtist | Field::Genre | Field::Label | Field::Format => Kind::Choice,
            Field::Year
            | Field::Plays
            | Field::PlaysIn
            | Field::Duration
            | Field::Bitrate
            | Field::SampleRate
            | Field::BitDepth
            | Field::Disc
            | Field::TrackNo => Kind::Number,
            Field::Added | Field::LastPlayed => Kind::Date,
            Field::Played | Field::Favourite | Field::AlbumFavourite | Field::Lossless | Field::Remote => Kind::Bool,
            Field::Playlist => Kind::Playlist,
        }
    }

    /// A track can have several (any one matches; "is not" means none does).
    pub fn multi(self) -> bool {
        matches!(self, Field::Artist | Field::AlbumArtist | Field::Genre)
    }

    pub fn ops(self) -> &'static [Op] {
        match self.kind() {
            Kind::Text | Kind::Choice => &[Op::Is, Op::IsNot, Op::Contains, Op::NotContains, Op::StartsWith],
            Kind::Number => &[Op::Is, Op::IsNot, Op::AtLeast, Op::AtMost, Op::Between],
            Kind::Date => &[Op::InLast, Op::NotInLast, Op::Before, Op::After, Op::Between],
            Kind::Bool => &[Op::Yes, Op::No],
            Kind::Playlist => &[Op::In, Op::NotIn],
        }
    }

    /// The operator as the editor words it for this field.
    pub fn op_label(self, op: Op) -> &'static str {
        match (self, op) {
            (Field::Played, Op::Yes) => "at least once",
            (Field::Played, Op::No) => "never",
            (Field::Lossless, Op::No) => "no (lossy)",
            (Field::Remote, Op::No) => "no (a file here)",
            (_, Op::Yes) => "yes",
            (_, Op::No) => "no",
            (Field::Duration, Op::AtLeast) => "is at least",
            (_, op) => op.label(),
        }
    }

    /// The operator a new condition on this field starts with.
    pub fn default_op(self) -> Op {
        match self {
            Field::Plays | Field::PlaysIn | Field::Bitrate | Field::SampleRate | Field::BitDepth => Op::AtLeast,
            Field::Duration => Op::AtLeast,
            Field::Played => Op::No,
            _ => self.ops()[0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    AtLeast,
    AtMost,
    /// Inclusive at both ends.
    Between,
    Before,
    After,
    InLast,
    NotInLast,
    Yes,
    No,
    In,
    NotIn,
}

impl Op {
    const ALL: [Op; 16] = [
        Op::Is,
        Op::IsNot,
        Op::Contains,
        Op::NotContains,
        Op::StartsWith,
        Op::AtLeast,
        Op::AtMost,
        Op::Between,
        Op::Before,
        Op::After,
        Op::InLast,
        Op::NotInLast,
        Op::Yes,
        Op::No,
        Op::In,
        Op::NotIn,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Op::Is => "is",
            Op::IsNot => "is_not",
            Op::Contains => "contains",
            Op::NotContains => "not_contains",
            Op::StartsWith => "starts_with",
            Op::AtLeast => "at_least",
            Op::AtMost => "at_most",
            Op::Between => "between",
            Op::Before => "before",
            Op::After => "after",
            Op::InLast => "in_last",
            Op::NotInLast => "not_in_last",
            Op::Yes => "yes",
            Op::No => "no",
            Op::In => "in",
            Op::NotIn => "not_in",
        }
    }

    pub fn from_key(s: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.key() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::Is => "is",
            Op::IsNot => "is not",
            Op::Contains => "contains",
            Op::NotContains => "does not contain",
            Op::StartsWith => "starts with",
            Op::AtLeast => "is at least",
            Op::AtMost => "is at most",
            Op::Between => "is between",
            Op::Before => "is before",
            Op::After => "is after",
            Op::InLast => "is in the last",
            Op::NotInLast => "is not in the last",
            Op::Yes => "yes",
            Op::No => "no",
            Op::In => "is in",
            Op::NotIn => "is not in",
        }
    }

    /// The negative operators of a multi-valued field hold when no value matches.
    pub fn negative(self) -> bool {
        matches!(self, Op::IsNot | Op::NotContains)
    }
}

/// A condition's value, in the field's own units: durations in seconds, sample rates in Hz,
/// bitrates in kbps, dates added and plays by the calendar.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Text(String),
    Num(f64),
    Range(f64, f64),
    Date(DateSpec),
    Dates(DateSpec, DateSpec),
    Ago(u32, Unit),
    /// A playlist, by id.
    Id(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub field: Field,
    pub op: Op,
    pub value: Value,
    /// `Field::PlaysIn`: the period plays are counted in.
    pub within: Option<(u32, Unit)>,
}

impl Cond {
    pub fn new(field: Field, op: Op, value: Value) -> Cond {
        Cond { field, op, value, within: (field == Field::PlaysIn).then_some((30, Unit::Days)) }
    }

    /// Whether its value fits its field and operator; the reason, in a few words, when not.
    pub fn check(&self) -> Result<(), String> {
        if !self.field.ops().contains(&self.op) {
            return Err(format!("“{}” cannot be used with {}", self.op.label(), self.field.label().to_lowercase()));
        }
        if self.field == Field::PlaysIn {
            match self.within {
                Some((n, _)) if (1..=100_000).contains(&n) => {}
                _ => return Err("Say how far back to count plays".into()),
            }
        }
        let number = |v: f64| -> Result<(), String> {
            if !v.is_finite() || v < 0.0 {
                return Err("Type a number".into());
            }
            Ok(())
        };
        match (self.field.kind(), self.op, &self.value) {
            (Kind::Text | Kind::Choice, _, Value::Text(t)) if !clean_text(t).is_empty() => Ok(()),
            (Kind::Text | Kind::Choice, Op::Is | Op::IsNot, _) if self.field.kind() == Kind::Choice => Err(format!("Choose {}", choose_what(self.field))),
            (Kind::Text | Kind::Choice, _, _) => Err("Type some text".into()),
            (Kind::Number, Op::Between, Value::Range(a, b)) => {
                number(*a)?;
                number(*b)?;
                if a > b { Err("The first number should be the smaller one".into()) } else { Ok(()) }
            }
            (Kind::Number, Op::Between, _) => Err("Type two numbers".into()),
            (Kind::Number, _, Value::Num(v)) => number(*v),
            (Kind::Number, _, _) => Err(match self.field {
                Field::Duration => "Type a length, like 3:30".into(),
                _ => "Type a number".into(),
            }),
            (Kind::Date, Op::InLast | Op::NotInLast, Value::Ago(n, _)) if (1..=100_000).contains(n) => Ok(()),
            (Kind::Date, Op::InLast | Op::NotInLast, _) => Err("Type how many".into()),
            (Kind::Date, Op::Before | Op::After, Value::Date(_)) => Ok(()),
            (Kind::Date, Op::Between, Value::Dates(a, b)) => {
                if a.first_day() > b.first_day() { Err("The first date should be the earlier one".into()) } else { Ok(()) }
            }
            (Kind::Date, _, _) => Err("Type a date, like 2024-03-15 (or 2024-03, or 2024)".into()),
            (Kind::Bool, _, _) => Ok(()),
            (Kind::Playlist, _, Value::Id(id)) if *id > 0 => Ok(()),
            (Kind::Playlist, _, _) => Err("Choose a playlist".into()),
        }
    }
}

fn choose_what(f: Field) -> &'static str {
    match f {
        Field::Artist | Field::AlbumArtist => "an artist",
        Field::Genre => "a genre",
        Field::Label => "a label",
        Field::Format => "a format",
        _ => "a value",
    }
}

/// A condition, a group of them, or something this build cannot read.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Cond(Cond),
    Group(Group),
    /// Kept as stored and written back as it was; matches nothing here. `why` says what it
    /// is in a few words.
    Unknown { raw: Json, why: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// "All of" (true) or "any of".
    pub all: bool,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// Album artist (by sort name), album date and title, then disc and track: an album's
    /// tracks in their order, the albums as the Albums page orders them.
    ArtistAlbum,
    Title,
    Artist,
    Album,
    Year,
    Added,
    LastPlayed,
    Plays,
    /// Plays within a recent period.
    PlaysIn(u32, Unit),
    Duration,
    /// A stable shuffle: the same for one seed (`Smart::seed`), new when reshuffled.
    Random,
}

impl SortKey {
    pub const CHOICES: [SortKey; 11] = [
        SortKey::ArtistAlbum,
        SortKey::Title,
        SortKey::Artist,
        SortKey::Album,
        SortKey::Year,
        SortKey::Added,
        SortKey::LastPlayed,
        SortKey::Plays,
        SortKey::PlaysIn(30, Unit::Days),
        SortKey::Duration,
        SortKey::Random,
    ];

    pub fn key(self) -> &'static str {
        match self {
            SortKey::ArtistAlbum => "artist_album",
            SortKey::Title => "title",
            SortKey::Artist => "artist",
            SortKey::Album => "album",
            SortKey::Year => "year",
            SortKey::Added => "added",
            SortKey::LastPlayed => "last_played",
            SortKey::Plays => "plays",
            SortKey::PlaysIn(..) => "plays_in",
            SortKey::Duration => "duration",
            SortKey::Random => "random",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::ArtistAlbum => "Artist and album",
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::Year => "Year",
            SortKey::Added => "Date added",
            SortKey::LastPlayed => "Last played",
            SortKey::Plays => "Play count",
            SortKey::PlaysIn(..) => "Plays in the last",
            SortKey::Duration => "Length",
            SortKey::Random => "Random",
        }
    }

    /// Names A to Z, everything else most / newest first.
    pub fn default_desc(self) -> bool {
        !matches!(self, SortKey::ArtistAlbum | SortKey::Title | SortKey::Artist | SortKey::Album | SortKey::Random)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

impl Sort {
    pub fn new(key: SortKey) -> Sort {
        Sort { key, desc: key.default_desc() }
    }
}

impl Default for Sort {
    fn default() -> Self {
        Sort::new(SortKey::ArtistAlbum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitUnit {
    Tracks,
    Minutes,
    Hours,
}

impl LimitUnit {
    pub const ALL: [LimitUnit; 3] = [LimitUnit::Tracks, LimitUnit::Minutes, LimitUnit::Hours];

    pub fn key(self) -> &'static str {
        match self {
            LimitUnit::Tracks => "tracks",
            LimitUnit::Minutes => "minutes",
            LimitUnit::Hours => "hours",
        }
    }

    pub fn from_key(s: &str) -> Option<LimitUnit> {
        LimitUnit::ALL.into_iter().find(|u| u.key() == s)
    }

    pub fn words(self, n: u32) -> &'static str {
        match (self, n == 1) {
            (LimitUnit::Tracks, true) => "track",
            (LimitUnit::Tracks, false) => "tracks",
            (LimitUnit::Minutes, true) => "minute",
            (LimitUnit::Minutes, false) => "minutes",
            (LimitUnit::Hours, true) => "hour",
            (LimitUnit::Hours, false) => "hours",
        }
    }
}

/// At most `n` tracks, or tracks up to `n` minutes or hours in all (each track whole: the
/// first that would pass the total, and all after it, are left out), chosen in the order
/// `by` from everything that matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Limit {
    pub n: u32,
    pub unit: LimitUnit,
    pub by: Sort,
}

impl Limit {
    /// The total length allowed, in seconds (a duration limit).
    pub fn seconds(&self) -> Option<f64> {
        match self.unit {
            LimitUnit::Tracks => None,
            LimitUnit::Minutes => Some(f64::from(self.n) * 60.0),
            LimitUnit::Hours => Some(f64::from(self.n) * 3600.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sorting {
    Known(Sort),
    Unknown(Json),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Limiting {
    Known(Limit),
    Unknown(Json),
}

/// A smart playlist's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Smart {
    /// The format it was written in (`FORMAT`, or a newer build's).
    pub version: u32,
    pub root: Group,
    pub sort: Sorting,
    pub limit: Option<Limiting>,
    /// The shuffle of a random order or a random selection: the same for one seed.
    pub seed: i64,
    /// Keys a newer build wrote at the top level, kept as they were.
    pub extra: Map<String, Json>,
}

impl Default for Smart {
    fn default() -> Self {
        Smart { version: FORMAT, root: Group { all: true, nodes: Vec::new() }, sort: Sorting::Known(Sort::default()), limit: None, seed: 1, extra: Map::new() }
    }
}

impl Smart {
    /// The playlists this rule set's conditions name ("is in", "is not in"), any depth.
    pub fn playlists_used(&self) -> Vec<i64> {
        fn walk(g: &Group, out: &mut Vec<i64>) {
            for n in &g.nodes {
                match n {
                    Node::Cond(Cond { field: Field::Playlist, value: Value::Id(id), .. }) => out.push(*id),
                    Node::Group(g) => walk(g, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Read a stored rule set. Never fails: what cannot be read is kept as unknown (and
    /// matches nothing); JSON that is not a rule set at all becomes one unknown rule.
    pub fn from_json(s: &str) -> Smart {
        match serde_json::from_str::<Json>(s) {
            Ok(Json::Object(m)) => Smart::from_map(m),
            Ok(other) => Smart::unreadable(other),
            Err(_) => Smart::unreadable(Json::String(s.to_owned())),
        }
    }

    fn unreadable(raw: Json) -> Smart {
        let mut s = Smart::default();
        s.root.nodes.push(Node::Unknown { raw, why: "rules that could not be read".into() });
        s
    }

    fn from_map(mut m: Map<String, Json>) -> Smart {
        strip_ratings(&mut m);
        let version = m.remove("v").and_then(|v| v.as_u64()).map_or(FORMAT, |v| v.min(u64::from(u32::MAX)) as u32);
        let all = match m.remove("match") {
            Some(Json::String(s)) if s == "any" => false,
            Some(Json::String(s)) if s == "all" => true,
            None => true,
            // A way of joining this build does not know: an unknown root, which matches nothing.
            Some(other) => {
                let mut s = Smart::default();
                s.root.nodes.push(Node::Unknown { raw: json!({ "match": other, "rules": m.get("rules").cloned().unwrap_or(Json::Null) }), why: "a way of joining rules this version does not know".into() });
                m.remove("rules");
                s.sort = sort_from(m.remove("sort"));
                s.limit = limit_from(m.remove("limit"));
                s.seed = m.remove("seed").and_then(|v| v.as_i64()).unwrap_or(1);
                s.extra = m;
                return s;
            }
        };
        let nodes = match m.remove("rules") {
            Some(Json::Array(a)) => a.into_iter().map(|n| node_from(n, 1)).collect(),
            None | Some(Json::Null) => Vec::new(),
            Some(other) => vec![Node::Unknown { raw: other, why: "rules that could not be read".into() }],
        };
        let sort = sort_from(m.remove("sort"));
        let limit = limit_from(m.remove("limit"));
        let seed = m.remove("seed").and_then(|v| v.as_i64()).unwrap_or(1);
        Smart { version, root: Group { all, nodes }, sort, limit, seed, extra: m }
    }

    pub fn to_json(&self) -> String {
        let mut m = self.extra.clone();
        m.insert("v".into(), json!(self.version.max(FORMAT)));
        m.insert("match".into(), json!(if self.root.all { "all" } else { "any" }));
        m.insert("rules".into(), Json::Array(self.root.nodes.iter().map(node_to).collect()));
        m.insert(
            "sort".into(),
            match &self.sort {
                Sorting::Known(s) => sort_to(s),
                Sorting::Unknown(raw) => raw.clone(),
            },
        );
        if let Some(l) = &self.limit {
            m.insert(
                "limit".into(),
                match l {
                    Limiting::Known(l) => json!({ "n": l.n, "unit": l.unit.key(), "by": sort_to(&l.by) }),
                    Limiting::Unknown(raw) => raw.clone(),
                },
            );
        }
        m.insert("seed".into(), json!(self.seed));
        Json::Object(m).to_string()
    }

    /// The order the list is shown in (the default for an order this build does not know).
    pub fn order(&self) -> Sort {
        match &self.sort {
            Sorting::Known(s) => *s,
            Sorting::Unknown(_) => Sort::default(),
        }
    }

    /// Whether a random order or a random selection is in it (Reshuffle is offered).
    pub fn random(&self) -> bool {
        self.order().key == SortKey::Random || matches!(&self.limit, Some(Limiting::Known(l)) if l.by.key == SortKey::Random)
    }

    /// What this build cannot evaluate in it, in words (empty when everything can be).
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        fn walk(g: &Group, v: &mut Vec<String>) {
            for n in &g.nodes {
                match n {
                    Node::Unknown { why, .. } if why == RATINGS_GONE => {
                        v.push("A rule about ratings, which are no longer kept: it matches nothing. Edit the playlist to take it out, or delete the playlist.".into())
                    }
                    Node::Unknown { why, .. } => v.push(format!("A rule this version of Marquee Player cannot use ({why}): it matches nothing here.")),
                    Node::Group(g) => walk(g, v),
                    Node::Cond(_) => {}
                }
            }
        }
        walk(&self.root, &mut v);
        if matches!(self.sort, Sorting::Unknown(_)) {
            v.push("An order this version does not know: shown by artist and album instead.".into());
        }
        if matches!(self.limit, Some(Limiting::Unknown(_))) {
            v.push("A limit this version does not know: nothing is shown rather than more than it allows.".into());
        }
        v
    }

    /// Whether a rule or the limit is one this build cannot use (it then holds less than the
    /// rules say: a rule matches nothing, a limit leaves it empty).
    pub fn incomplete(&self) -> bool {
        fn walk(g: &Group) -> bool {
            g.nodes.iter().any(|n| match n {
                Node::Unknown { .. } => true,
                Node::Group(g) => walk(g),
                Node::Cond(_) => false,
            })
        }
        walk(&self.root) || matches!(self.limit, Some(Limiting::Unknown(_)))
    }

    /// The first condition whose value does not fit (for refusing a save), with its reason.
    pub fn first_invalid(&self) -> Option<(Vec<usize>, String)> {
        fn walk(g: &Group, path: &mut Vec<usize>, top: bool) -> Option<(Vec<usize>, String)> {
            if !top && g.nodes.is_empty() {
                return Some((path.clone(), "This group has no rules yet".into()));
            }
            for (i, n) in g.nodes.iter().enumerate() {
                path.push(i);
                let r = match n {
                    Node::Cond(c) => c.check().err().map(|e| (path.clone(), e)),
                    Node::Group(g) => walk(g, path, false),
                    Node::Unknown { .. } => None,
                };
                path.pop();
                if r.is_some() {
                    return r;
                }
            }
            None
        }
        if let Some(r) = walk(&self.root, &mut Vec::new(), true) {
            return Some(r);
        }
        if let Some(Limiting::Known(l)) = &self.limit
            && l.n == 0
        {
            return Some((Vec::new(), "A limit needs a number above 0".into()));
        }
        None
    }
}

/// Why a rule about ratings is kept as one this build cannot use (`Node::Unknown`).
pub const RATINGS_GONE: &str = "ratings are no longer kept";

/// Take out of a stored rule set what was about ratings, which the app no longer keeps, where
/// that leaves the other rules meaning what they did less the rating: a rating condition (or a
/// group of nothing but rating conditions) among other rules of an "all of" group is dropped,
/// and an order or a limit's selection by rating becomes the default order. Anywhere else (the
/// only rules of the playlist or of a group, or one choice of an "any of" group) it stays, read
/// as a rule this build cannot use (`RATINGS_GONE`): it matches nothing and the playlist says
/// so, so a list that was only about ratings is empty and flagged, never every track. Taking
/// out again changes nothing. Whether anything was taken out.
pub fn strip_ratings(m: &mut Map<String, Json>) -> bool {
    /// A rating condition, or a group (of rules) of nothing else.
    fn rating(n: &Json) -> bool {
        if n.get("group").is_some() {
            return matches!(n.get("rules"), Some(Json::Array(a)) if !a.is_empty() && a.iter().all(rating));
        }
        n.get("field").and_then(Json::as_str) == Some("rating")
    }
    fn rules(nodes: &mut Vec<Json>, all: bool) -> bool {
        let mut changed = false;
        for n in nodes.iter_mut() {
            if !rating(n)
                && let Some(join) = n.get("group").and_then(Json::as_str).map(|j| j == "all")
                && let Some(Json::Array(inner)) = n.get_mut("rules")
            {
                changed |= rules(inner, join);
            }
        }
        if all && nodes.iter().any(|n| !rating(n)) {
            let before = nodes.len();
            nodes.retain(|n| !rating(n));
            changed |= nodes.len() != before;
        }
        changed
    }
    fn by_rating(s: Option<&Json>) -> bool {
        s.and_then(|s| s.get("by")).and_then(Json::as_str) == Some("rating")
    }
    let all = match m.get("match") {
        None => true,
        Some(j) => j.as_str() == Some("all"),
    };
    let mut changed = match m.get_mut("rules") {
        Some(Json::Array(a)) => rules(a, all),
        _ => false,
    };
    if by_rating(m.get("sort")) {
        m.remove("sort");
        changed = true;
    }
    if let Some(Json::Object(l)) = m.get_mut("limit")
        && by_rating(l.get("by"))
    {
        l.remove("by");
        changed = true;
    }
    changed
}

fn sort_from(j: Option<Json>) -> Sorting {
    match j {
        None | Some(Json::Null) => Sorting::Known(Sort::default()),
        Some(j) => sort_parse(&j).map_or(Sorting::Unknown(j), Sorting::Known),
    }
}

fn sort_parse(j: &Json) -> Option<Sort> {
    let by = j.get("by")?.as_str()?;
    let desc = j.get("desc").and_then(Json::as_bool).unwrap_or(false);
    let key = match by {
        "plays_in" => {
            let (n, unit) = ago_parse(j.get("within")?)?;
            SortKey::PlaysIn(n, unit)
        }
        other => SortKey::CHOICES.into_iter().find(|k| k.key() == other && !matches!(k, SortKey::PlaysIn(..)))?,
    };
    Some(Sort { key, desc })
}

fn sort_to(s: &Sort) -> Json {
    match s.key {
        SortKey::PlaysIn(n, unit) => json!({ "by": "plays_in", "desc": s.desc, "within": { "n": n, "unit": unit.key() } }),
        k => json!({ "by": k.key(), "desc": s.desc }),
    }
}

fn limit_from(j: Option<Json>) -> Option<Limiting> {
    match j {
        None | Some(Json::Null) => None,
        Some(j) => Some(limit_parse(&j).map_or(Limiting::Unknown(j), Limiting::Known)),
    }
}

fn limit_parse(j: &Json) -> Option<Limit> {
    let n = u32::try_from(j.get("n")?.as_u64()?).ok()?;
    let unit = LimitUnit::from_key(j.get("unit")?.as_str()?)?;
    let by = match j.get("by") {
        None => Sort::default(),
        Some(b) => sort_parse(b)?,
    };
    Some(Limit { n, unit, by })
}

fn ago_parse(j: &Json) -> Option<(u32, Unit)> {
    let n = u32::try_from(j.get("n")?.as_u64()?).ok()?;
    Some((n, Unit::from_key(j.get("unit")?.as_str()?)?))
}

fn ago_to(n: u32, unit: Unit) -> Json {
    json!({ "n": n, "unit": unit.key() })
}

/// A stored node at nesting `depth` (1: the root's own).
fn node_from(j: Json, depth: usize) -> Node {
    if let Some(join) = j.get("group") {
        let all = match join.as_str() {
            Some("all") => true,
            Some("any") => false,
            _ => return Node::Unknown { raw: j, why: "a group joined in a way this version does not know".into() },
        };
        if depth > 16 {
            return Node::Unknown { raw: j, why: "groups nested too deep".into() };
        }
        let nodes = match j.get("rules") {
            Some(Json::Array(a)) => a.iter().cloned().map(|n| node_from(n, depth + 1)).collect(),
            _ => return Node::Unknown { raw: j, why: "a group that could not be read".into() },
        };
        return Node::Group(Group { all, nodes });
    }
    match cond_parse(&j) {
        Ok(c) => Node::Cond(c),
        Err(why) => Node::Unknown { raw: j, why },
    }
}

fn cond_parse(j: &Json) -> Result<Cond, String> {
    let fkey = j.get("field").and_then(Json::as_str).unwrap_or("");
    if fkey == "rating" {
        return Err(RATINGS_GONE.into());
    }
    let field = Field::from_key(fkey).ok_or_else(|| format!("the field “{fkey}”"))?;
    let okey = j.get("op").and_then(Json::as_str).unwrap_or("");
    let op = Op::from_key(okey).filter(|o| field.ops().contains(o)).ok_or_else(|| format!("{} “{okey}”", field.label().to_lowercase()))?;
    let bad = || format!("a value of {} it cannot read", field.label().to_lowercase());
    let v = j.get("value");
    let num = |v: &Json| v.as_f64().filter(|f| f.is_finite());
    let date = |v: &Json| v.as_str().and_then(DateSpec::parse);
    let value = match (field.kind(), op) {
        (Kind::Text | Kind::Choice, _) => Value::Text(v.and_then(Json::as_str).ok_or_else(bad)?.to_owned()),
        (Kind::Number, Op::Between) => {
            let a = v.and_then(Json::as_array).filter(|a| a.len() == 2).ok_or_else(bad)?;
            Value::Range(num(&a[0]).ok_or_else(bad)?, num(&a[1]).ok_or_else(bad)?)
        }
        (Kind::Number, _) => Value::Num(v.and_then(num).ok_or_else(bad)?),
        (Kind::Date, Op::InLast | Op::NotInLast) => {
            let (n, u) = v.and_then(ago_parse).ok_or_else(bad)?;
            Value::Ago(n, u)
        }
        (Kind::Date, Op::Between) => {
            let a = v.and_then(Json::as_array).filter(|a| a.len() == 2).ok_or_else(bad)?;
            Value::Dates(date(&a[0]).ok_or_else(bad)?, date(&a[1]).ok_or_else(bad)?)
        }
        (Kind::Date, _) => Value::Date(v.and_then(date).ok_or_else(bad)?),
        (Kind::Bool, _) => Value::None,
        (Kind::Playlist, _) => Value::Id(v.and_then(Json::as_i64).ok_or_else(bad)?),
    };
    let within = if field == Field::PlaysIn { Some(j.get("within").and_then(ago_parse).ok_or_else(bad)?) } else { None };
    Ok(Cond { field, op, value, within })
}

fn node_to(n: &Node) -> Json {
    match n {
        Node::Unknown { raw, .. } => raw.clone(),
        Node::Group(g) => json!({ "group": if g.all { "all" } else { "any" }, "rules": g.nodes.iter().map(node_to).collect::<Vec<_>>() }),
        Node::Cond(c) => {
            let mut m = Map::new();
            m.insert("field".into(), json!(c.field.key()));
            m.insert("op".into(), json!(c.op.key()));
            let value = match &c.value {
                Value::None => None,
                Value::Text(t) => Some(json!(t)),
                Value::Num(v) => Some(num_json(*v)),
                Value::Range(a, b) => Some(json!([num_json(*a), num_json(*b)])),
                Value::Date(d) => Some(json!(d.iso())),
                Value::Dates(a, b) => Some(json!([a.iso(), b.iso()])),
                Value::Ago(n, u) => Some(ago_to(*n, *u)),
                Value::Id(id) => Some(json!(id)),
            };
            if let Some(v) = value {
                m.insert("value".into(), v);
            }
            if let Some((n, u)) = c.within {
                m.insert("within".into(), ago_to(n, u));
            }
            Json::Object(m)
        }
    }
}

/// Whole numbers as integers ("4", not "4.0"), the rest as they are.
fn num_json(v: f64) -> Json {
    if v.fract() == 0.0 && v.abs() < 9e15 { json!(v as i64) } else { json!(v) }
}

/// Starting points for a new smart playlist: ordinary rule sets the user picks and then
/// edits, not lists of their own.
pub fn templates() -> Vec<(&'static str, Smart)> {
    let rules = |all: bool, nodes: Vec<Cond>, sort: Sort, limit: Option<Limit>| Smart {
        root: Group { all, nodes: nodes.into_iter().map(Node::Cond).collect() },
        sort: Sorting::Known(sort),
        limit: limit.map(Limiting::Known),
        ..Smart::default()
    };
    vec![
        (
            "Recently added and unplayed",
            rules(
                true,
                vec![Cond::new(Field::Added, Op::InLast, Value::Ago(2, Unit::Months)), Cond::new(Field::Played, Op::No, Value::None)],
                Sort::new(SortKey::Added),
                None,
            ),
        ),
        (
            "Favourites not heard in 6 months",
            rules(
                true,
                vec![Cond::new(Field::Favourite, Op::Yes, Value::None), Cond::new(Field::LastPlayed, Op::NotInLast, Value::Ago(6, Unit::Months))],
                Sort::new(SortKey::Random),
                None,
            ),
        ),
        (
            "Most played this year",
            rules(
                true,
                vec![Cond { within: Some((12, Unit::Months)), ..Cond::new(Field::PlaysIn, Op::AtLeast, Value::Num(1.0)) }],
                Sort::new(SortKey::PlaysIn(12, Unit::Months)),
                Some(Limit { n: 50, unit: LimitUnit::Tracks, by: Sort::new(SortKey::PlaysIn(12, Unit::Months)) }),
            ),
        ),
        ("Lossless only", rules(true, vec![Cond::new(Field::Lossless, Op::Yes, Value::None)], Sort::new(SortKey::ArtistAlbum), None)),
        ("Two hours at random", rules(true, Vec::new(), Sort::new(SortKey::Random), Some(Limit { n: 2, unit: LimitUnit::Hours, by: Sort::new(SortKey::Random) }))),
    ]
}

/// What a text value means: trimmed, without NUL characters (which SQLite's LIKE would read
/// as the end of the pattern, and which cannot be typed anyway).
pub fn clean_text(s: &str) -> String {
    s.replace('\0', "").trim().to_owned()
}
