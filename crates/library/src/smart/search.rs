//! Search operators: `artist:"nina simone" year:1990..1999 -is:remote live`. A typed query is
//! read into terms (`parse`), and the terms into the smart playlists' own rules (`rules`), so a
//! query with operators is evaluated by the same SQL a smart playlist is (`sql.rs`) and can be
//! saved as one. A query without any operator never comes here: plain search keeps its own path
//! (`query::search`).
//!
//! The syntax, forgiving and case-insensitive (docs/features/navigation.md, "Search operators", has it in full):
//! - `field:value` for text fields (contains; `field:=value` is exactly), quoted for spaces;
//! - `field:N`, `field:>N`, `>=`, `<`, `<=`, `N..M` (either end open), `N+` for numbers; lengths as
//!   `3:30`, `90s`, `5m` or minutes; sample rates in kHz or Hz; `year:1990s`;
//! - `added:` and `played:` with `<30d` (within the last 30 days), `>1y` (longer ago, or never
//!   played), `2024`, `2024-03`, `>2023`, `2023..2024`; `played:never`;
//! - `is:favourite` (and `favorite`, `fav`), `is:favalbum`, `is:local`, `is:remote`, `is:lossless`,
//!   `is:lossy`, `is:played`, `is:unplayed`; `playlist:name`;
//! - a leading `-` negates a term; `OR` (upper case) between terms makes them one term that any
//!   of them satisfies; everything else is ANDed.
//!
//! What does not read as an operator (an unknown field, a value that does not fit) is searched
//! as text, never refused; `Parsed::notes` says what, for the hint under the search box.

use super::{Cond, DateSpec, Field, Group, Kind, Node, Op, Smart, Sort, SortKey, Sorting, Unit, Value, civil_from_days};

/// One term of a query.
#[derive(Clone, Debug, PartialEq)]
pub enum Atom {
    /// Words or a quoted phrase, searched as text (`neg`: not found).
    Text { neg: bool, text: String },
    /// A field operator, as the rule it means (a negation already applied).
    Rule(Node),
}

/// A query read into terms: all of `clauses` hold, each when any of its atoms does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub clauses: Vec<Vec<Atom>>,
    /// What was not understood and was searched as text instead, a few words each.
    pub notes: Vec<String>,
    /// A word was negated (`-live`): an operator of its own. A negated field that was not
    /// understood (`-composer:bach`, searched as text) is not one.
    pub negated_word: bool,
}

/// Where a query's rules are evaluated: which tracks match, or which albums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Tracks,
    Albums,
}

impl Parsed {
    /// Whether an operator was understood (a field, a flag, OR, or a negated word such as
    /// `-live`): the query then takes the rules' path; without one it is plain search, exactly
    /// as before.
    pub fn has_operators(&self) -> bool {
        self.clauses.iter().any(|c| c.len() > 1 || c.iter().any(|a| matches!(a, Atom::Rule(_)))) || self.negated_word
    }

    /// The query as smart playlist rules, evaluated at `level`; None when a term cannot apply
    /// there (a play count says nothing about an album). Text is title, artist, album or album artist
    /// for tracks, and album, album artist or label for albums, as plain search looks.
    pub fn rules(&self, level: Level) -> Option<Smart> {
        let mut root: Vec<Node> = Vec::new();
        for clause in &self.clauses {
            let mut any: Vec<Node> = Vec::new();
            for atom in clause {
                let node = match atom {
                    Atom::Text { neg, text } => {
                        let g = text_group(level, text);
                        if *neg { negate(&g)? } else { g }
                    }
                    Atom::Rule(n) => {
                        if level == Level::Albums && !fields(n).iter().all(|f| album_level(*f)) {
                            return None;
                        }
                        n.clone()
                    }
                };
                push(&mut any, false, node);
            }
            let node = if any.len() == 1 { any.pop()? } else { Node::Group(Group { all: false, nodes: any }) };
            push(&mut root, true, node);
        }
        let sort = self.sort();
        Some(Smart { root: Group { all: true, nodes: root }, sort: Sorting::Known(sort), ..Smart::default() })
    }

    /// The order results are shown in: by the first number or date asked about (most played
    /// first for `plays:`, newest first for `added:`...), else by artist and album.
    pub fn sort(&self) -> Sort {
        let key = self.clauses.iter().flatten().find_map(|a| match a {
            Atom::Rule(n) => fields(n).into_iter().find_map(|f| match f {
                Field::Plays => Some(SortKey::Plays),
                Field::Added => Some(SortKey::Added),
                Field::LastPlayed => Some(SortKey::LastPlayed),
                Field::Year => Some(SortKey::Year),
                Field::Duration => Some(SortKey::Duration),
                _ => None,
            }),
            Atom::Text { .. } => None,
        });
        Sort::new(key.unwrap_or(SortKey::ArtistAlbum))
    }

    /// When every term is about artists' names (words, `artist:`, `albumartist:`) and none is
    /// OR'ed: each as (album artists only, operator, text), for the Artists section; None
    /// otherwise (the section is then left out).
    pub fn artist_terms(&self) -> Option<Vec<(bool, Op, String)>> {
        let mut out = Vec::new();
        for clause in &self.clauses {
            let [atom] = clause.as_slice() else { return None };
            match atom {
                Atom::Text { neg, text } => out.push((false, if *neg { Op::NotContains } else { Op::Contains }, text.clone())),
                Atom::Rule(Node::Cond(Cond { field: f @ (Field::Artist | Field::AlbumArtist), op, value: Value::Text(t), .. })) => {
                    out.push((*f == Field::AlbumArtist, *op, t.clone()))
                }
                _ => return None,
            }
        }
        Some(out)
    }
}

/// Add `node` to a group joined by `all`, taking a group joined the same way apart (so the
/// rules stay as flat as they read).
fn push(into: &mut Vec<Node>, all: bool, node: Node) {
    match node {
        Node::Group(g) if g.all == all => into.extend(g.nodes),
        n => into.push(n),
    }
}

fn text_group(level: Level, text: &str) -> Node {
    let fields: &[Field] = match level {
        Level::Tracks => &[Field::Title, Field::Artist, Field::Album, Field::AlbumArtist],
        Level::Albums => &[Field::Album, Field::AlbumArtist, Field::Label],
    };
    Node::Group(Group { all: false, nodes: fields.iter().map(|f| Node::Cond(Cond::new(*f, Op::Contains, Value::Text(text.to_owned())))).collect() })
}

/// What an album's tracks share (or what search reads album by album): these can list albums.
fn album_level(f: Field) -> bool {
    matches!(f, Field::Album | Field::AlbumArtist | Field::Artist | Field::Genre | Field::Label | Field::Year | Field::Added | Field::AlbumFavourite | Field::Remote)
}

fn fields(n: &Node) -> Vec<Field> {
    match n {
        Node::Cond(c) => vec![c.field],
        Node::Group(g) => g.nodes.iter().flat_map(fields).collect(),
        Node::Unknown { .. } => Vec::new(),
    }
}

// ---- negation ------------------------------------------------------------------------

/// The rule that holds where `n` does not, in the rules' own operators (so a saved query stays
/// editable): "contains" and "does not contain", "is at least 4" and "is at most 3", "in the last"
/// and "not in the last", a group by De Morgan. Numbers are whole (every number field is), so a
/// comparison's opposite is exact; a missing value (no year) matches neither, as in the editor,
/// except that never played is the opposite of any last-played date. None when nothing could
/// match it.
pub fn negate(n: &Node) -> Option<Node> {
    match n {
        Node::Group(g) => {
            let nodes: Vec<Node> = g.nodes.iter().map(negate).collect::<Option<_>>()?;
            Some(Node::Group(Group { all: !g.all, nodes }))
        }
        Node::Unknown { .. } => None,
        Node::Cond(c) => negate_cond(c),
    }
}

fn negate_cond(c: &Cond) -> Option<Node> {
    let with = |op: Op, value: Value| Cond { op, value, ..c.clone() };
    let ok = |c: Cond| c.check().is_ok().then_some(c);
    let either = |v: Vec<Option<Cond>>| -> Option<Node> {
        let mut v: Vec<Node> = v.into_iter().flatten().map(Node::Cond).collect();
        match v.len() {
            0 => None,
            1 => v.pop(),
            _ => Some(Node::Group(Group { all: false, nodes: v })),
        }
    };
    let flipped = |op: Op| Some(Node::Cond(with(op, c.value.clone())));
    match (c.field.kind(), c.op, &c.value) {
        (Kind::Text | Kind::Choice, Op::Contains, _) => flipped(Op::NotContains),
        (Kind::Text | Kind::Choice, Op::NotContains, _) => flipped(Op::Contains),
        (Kind::Text | Kind::Choice, Op::Is, _) => flipped(Op::IsNot),
        (Kind::Text | Kind::Choice, Op::IsNot, _) => flipped(Op::Is),
        (Kind::Bool, Op::Yes, _) => flipped(Op::No),
        (Kind::Bool, Op::No, _) => flipped(Op::Yes),
        (Kind::Playlist, Op::In, _) => flipped(Op::NotIn),
        (Kind::Playlist, Op::NotIn, _) => flipped(Op::In),
        (Kind::Number, Op::Is, _) => flipped(Op::IsNot),
        (Kind::Number, Op::IsNot, _) => flipped(Op::Is),
        (Kind::Number, Op::AtLeast, Value::Num(v)) => either(vec![ok(with(Op::AtMost, Value::Num(v - 1.0)))]),
        (Kind::Number, Op::AtMost, Value::Num(v)) => either(vec![ok(with(Op::AtLeast, Value::Num(v + 1.0)))]),
        (Kind::Number, Op::Between, Value::Range(a, b)) => either(vec![ok(with(Op::AtMost, Value::Num(a - 1.0))), ok(with(Op::AtLeast, Value::Num(b + 1.0)))]),
        (Kind::Date, Op::InLast, _) => flipped(Op::NotInLast),
        (Kind::Date, Op::NotInLast, _) => flipped(Op::InLast),
        (Kind::Date, op, value) => {
            let mut v = match (op, value) {
                (Op::Before, Value::Date(d)) => vec![prev(*d).map(|p| with(Op::After, Value::Date(p)))],
                (Op::After, Value::Date(d)) => vec![next(*d).map(|n| with(Op::Before, Value::Date(n)))],
                (Op::Between, Value::Dates(a, b)) => vec![Some(with(Op::Before, Value::Date(*a))), Some(with(Op::After, Value::Date(*b)))],
                _ => return None,
            };
            if c.field == Field::LastPlayed {
                v.push(Some(Cond::new(Field::Played, Op::No, Value::None)));
            }
            either(v)
        }
        _ => None,
    }
}

/// The year, month or day before `d` (as precise as it is).
fn prev(d: DateSpec) -> Option<DateSpec> {
    let r = match (d.m, d.d) {
        (Some(_), Some(_)) => day_spec(d.first_day() - 1),
        (Some(m), None) => {
            if m == 1 { DateSpec { y: d.y - 1, m: Some(12), d: None } } else { DateSpec { y: d.y, m: Some(m - 1), d: None } }
        }
        _ => DateSpec { y: d.y - 1, m: None, d: None },
    };
    (1000..=9999).contains(&r.y).then_some(r)
}

/// The year, month or day after `d`.
fn next(d: DateSpec) -> Option<DateSpec> {
    let r = match (d.m, d.d) {
        (Some(_), Some(_)) => day_spec(d.end_day()),
        (Some(m), None) => {
            if m == 12 { DateSpec { y: d.y + 1, m: Some(1), d: None } } else { DateSpec { y: d.y, m: Some(m + 1), d: None } }
        }
        _ => DateSpec { y: d.y + 1, m: None, d: None },
    };
    (1000..=9999).contains(&r.y).then_some(r)
}

fn day_spec(days: i64) -> DateSpec {
    let (y, m, d) = civil_from_days(days);
    DateSpec { y: y.clamp(0, 99_999) as i32, m: Some(m as u8), d: Some(d as u8) }
}

// ---- reading a query -------------------------------------------------------------------

/// A piece of the query between spaces (or a quoted run).
#[derive(Debug)]
struct Tok {
    neg: bool,
    /// Before the colon of `key:value`, as typed.
    key: Option<String>,
    value: String,
    /// The value was quoted (never an operator word like OR).
    quoted: bool,
    /// The token as typed, for searching it as text.
    raw: String,
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201c}' | '\u{201d}' | '\u{201e}')
}

/// Read a quoted run starting after its opening quote; an unclosed quote runs to the end.
fn quoted(chars: &[char], i: &mut usize) -> String {
    let mut s = String::new();
    while *i < chars.len() {
        let c = chars[*i];
        *i += 1;
        if is_quote(c) {
            break;
        }
        s.push(c);
    }
    s
}

fn tokens(q: &str) -> Vec<Tok> {
    let chars: Vec<char> = q.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let mut neg = false;
        if chars[i] == '-' && chars.get(i + 1).is_some_and(|c| !c.is_whitespace() && *c != '-') {
            neg = true;
            i += 1;
        }
        let mut tok = Tok { neg, key: None, value: String::new(), quoted: false, raw: String::new() };
        if is_quote(chars[i]) {
            i += 1;
            tok.value = quoted(&chars, &mut i);
            tok.quoted = true;
        } else {
            let mut word = String::new();
            while i < chars.len() && !chars[i].is_whitespace() {
                let c = chars[i];
                if c == ':' && tok.key.is_none() && is_key(&word) {
                    tok.key = Some(std::mem::take(&mut word));
                    i += 1;
                    // A comparison before a quoted value: `album:="Kind of Blue"`.
                    let mut cmp = String::new();
                    while let Some(&c) = chars.get(i + cmp.chars().count())
                        && matches!(c, '=' | '<' | '>')
                        && cmp.len() < 2
                    {
                        cmp.push(c);
                    }
                    if chars.get(i + cmp.chars().count()).is_some_and(|c| is_quote(*c)) {
                        i += cmp.chars().count() + 1;
                        tok.value = cmp + &quoted(&chars, &mut i);
                        tok.quoted = true;
                        // What follows a closing quote without a space belongs to it.
                        while i < chars.len() && !chars[i].is_whitespace() {
                            tok.value.push(chars[i]);
                            i += 1;
                        }
                        break;
                    }
                    continue;
                }
                word.push(c);
                i += 1;
            }
            if !tok.quoted {
                tok.value = word;
            }
        }
        tok.raw = chars[start..i].iter().collect::<String>().trim().to_owned();
        out.push(tok);
    }
    out
}

/// A field name as typed: letters (and `_`, `-`), short.
fn is_key(s: &str) -> bool {
    (1..=20).contains(&s.chars().count()) && s.chars().all(|c| c.is_ascii_alphabetic() || c == '_') && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// What a field name stands for.
enum Key {
    Field(Field),
    Is,
    Has,
    Playlist,
    /// A field the library does not keep.
    Missing(&'static str),
}

fn key(k: &str) -> Option<Key> {
    let k: String = k.chars().filter(|c| *c != '_' && *c != '-').collect::<String>().to_ascii_lowercase();
    Some(Key::Field(match k.as_str() {
        "artist" | "by" => Field::Artist,
        "albumartist" | "aa" => Field::AlbumArtist,
        "album" => Field::Album,
        "title" | "song" => Field::Title,
        "genre" => Field::Genre,
        "label" | "publisher" => Field::Label,
        "format" | "codec" => Field::Format,
        "path" | "file" | "folder" => Field::Path,
        "year" | "date" => Field::Year,
        "plays" | "playcount" | "count" => Field::Plays,
        "length" | "duration" | "time" | "len" => Field::Duration,
        "bitrate" | "kbps" => Field::Bitrate,
        "samplerate" | "rate" | "khz" => Field::SampleRate,
        "bitdepth" | "bits" | "depth" => Field::BitDepth,
        "disc" | "disk" | "cd" => Field::Disc,
        "track" | "trackno" | "tracknumber" => Field::TrackNo,
        "added" => Field::Added,
        "played" | "lastplayed" => Field::LastPlayed,
        "is" => return Some(Key::Is),
        "has" => return Some(Key::Has),
        "playlist" | "in" => return Some(Key::Playlist),
        "composer" => return Some(Key::Missing("the library does not keep composers")),
        "lyrics" => return Some(Key::Missing("lyrics are read from the files only when shown")),
        "mood" | "bpm" | "comment" => return Some(Key::Missing("the library does not keep it")),
        "rating" | "rated" | "stars" => return Some(Key::Missing("the library does not keep ratings")),
        _ => return None,
    }))
}

/// Read `q`. `playlist` finds a playlist's id by its name (`playlist:`); `|_| None` when there
/// is no library to ask. Never fails: what is not understood is text, and noted.
pub fn parse(q: &str, playlist: &dyn Fn(&str) -> Option<i64>) -> Parsed {
    let mut toks = tokens(q);
    // `artist: nina`: a known field with its value after a space.
    let mut i = 0;
    while i + 1 < toks.len() {
        if toks[i].key.as_deref().is_some_and(|k| key(k).is_some()) && toks[i].value.is_empty() && !toks[i].quoted && toks[i + 1].key.is_none() && !toks[i + 1].neg && !is_or(&toks[i + 1]) {
            let next = toks.remove(i + 1);
            toks[i].raw = format!("{} {}", toks[i].raw, next.raw);
            toks[i].value = next.value;
            toks[i].quoted = next.quoted;
        }
        i += 1;
    }
    let mut p = Parsed::default();
    let mut joining = false;
    for t in toks {
        if is_or(&t) {
            joining = !p.clauses.is_empty();
            continue;
        }
        let atom = atom(&t, playlist, &mut p.notes);
        let Some(atom) = atom else { continue };
        p.negated_word |= t.neg && t.key.is_none();
        match p.clauses.last_mut() {
            Some(last) if joining => last.push(atom),
            _ => p.clauses.push(vec![atom]),
        }
        joining = false;
    }
    p
}

fn is_or(t: &Tok) -> bool {
    !t.quoted && !t.neg && t.key.is_none() && (t.value == "OR" || t.value == "|" || t.value == "||")
}

fn note(notes: &mut Vec<String>, n: String) {
    if !notes.contains(&n) && notes.len() < 4 {
        notes.push(n);
    }
}

fn atom(t: &Tok, playlist: &dyn Fn(&str) -> Option<i64>, notes: &mut Vec<String>) -> Option<Atom> {
    let as_text = |t: &Tok| {
        let text = if t.key.is_some() { t.raw.trim_start_matches('-').to_owned() } else { t.value.clone() };
        let text = super::clean_text(&text);
        (!text.is_empty()).then_some(Atom::Text { neg: t.neg, text })
    };
    let Some(k) = &t.key else { return as_text(t) };
    let value = super::clean_text(&t.value);
    let Some(what) = key(k) else {
        // A colon in a title ("Re:Stacks") is text; said only when it looked like a field.
        if !value.is_empty() {
            note(notes, format!("\u{201c}{k}:\u{201d} is not a field"));
        }
        return as_text(t);
    };
    if value.is_empty() {
        note(notes, format!("\u{201c}{k}:\u{201d} needs a value"));
        return as_text(t);
    }
    let read = match what {
        Key::Field(f) => field_rule(f, &value),
        Key::Is => flag(&value),
        Key::Has => match value.to_ascii_lowercase().as_str() {
            "rating" | "stars" => Err("the library does not keep ratings".into()),
            "lyrics" => Err("lyrics are read from the files only when shown, so search cannot tell".into()),
            _ => Err(format!("search cannot tell whether a track has \u{201c}{value}\u{201d}")),
        },
        Key::Playlist => match playlist(&value) {
            Some(id) => Ok(Node::Cond(Cond::new(Field::Playlist, Op::In, Value::Id(id)))),
            None => Err(format!("no playlist is called \u{201c}{value}\u{201d}")),
        },
        Key::Missing(why) => Err(why.to_owned()),
    };
    let read = read.and_then(|n| if t.neg { negate(&n).ok_or_else(|| "nothing could match its opposite".to_owned()) } else { Ok(n) });
    match read {
        Ok(n) => Some(Atom::Rule(n)),
        Err(why) => {
            note(notes, format!("\u{201c}{}\u{201d}: {why}", t.raw));
            as_text(t)
        }
    }
}

fn flag(v: &str) -> Result<Node, String> {
    let c = |f: Field, op: Op| Ok(Node::Cond(Cond::new(f, op, Value::None)));
    match v.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
        "favourite" | "favorite" | "fav" | "fave" | "loved" | "liked" | "heart" => c(Field::Favourite, Op::Yes),
        "favalbum" | "favouritealbum" | "favoritealbum" => c(Field::AlbumFavourite, Op::Yes),
        "local" | "file" => c(Field::Remote, Op::No),
        "remote" | "server" | "cloud" | "navidrome" | "streamed" => c(Field::Remote, Op::Yes),
        "lossless" => c(Field::Lossless, Op::Yes),
        "lossy" => c(Field::Lossless, Op::No),
        "played" => c(Field::Played, Op::Yes),
        "unplayed" | "new" | "neverplayed" => c(Field::Played, Op::No),
        "rated" | "unrated" => Err("the library does not keep ratings".into()),
        _ => Err("is: takes favourite, favalbum, local, remote, lossless, lossy, played or unplayed".into()),
    }
}

/// A comparison as typed before a value: `>=`, `>`, `<=`, `<`, `=`, or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmp {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

fn split_cmp(s: &str) -> (Cmp, &str) {
    for (p, c) in [(">=", Cmp::Ge), ("=>", Cmp::Ge), ("\u{2265}", Cmp::Ge), ("<=", Cmp::Le), ("=<", Cmp::Le), ("\u{2264}", Cmp::Le), (">", Cmp::Gt), ("<", Cmp::Lt), ("=", Cmp::Eq)] {
        if let Some(rest) = s.strip_prefix(p) {
            return (c, rest.trim());
        }
    }
    if let Some(rest) = s.strip_suffix('+') {
        return (Cmp::Ge, rest.trim());
    }
    (Cmp::Eq, s)
}

fn field_rule(f: Field, v: &str) -> Result<Node, String> {
    let cond = match f.kind() {
        Kind::Text | Kind::Choice => {
            let (op, text) = match v.strip_prefix('=') {
                Some(rest) => (Op::Is, rest.trim()),
                None => (Op::Contains, v),
            };
            if text.is_empty() {
                return Err("needs a value".into());
            }
            Cond::new(f, op, Value::Text(text.to_owned()))
        }
        Kind::Number => number_rule(f, v)?,
        Kind::Date => return date_rule(f, v),
        Kind::Bool | Kind::Playlist => return Err("is not a field to search by".into()),
    };
    cond.check()?;
    Ok(Node::Cond(cond))
}

fn number_rule(f: Field, v: &str) -> Result<Cond, String> {
    let bad = || match f {
        Field::Duration => "not a length (3:30, 90s, 5m)".to_owned(),
        Field::Year => "not a year".to_owned(),
        Field::SampleRate => "not a sample rate (44.1, 96k)".to_owned(),
        _ => "not a number".to_owned(),
    };
    let num = |s: &str| number(f, s).ok_or_else(bad);
    // A decade: 1990s.
    if f == Field::Year
        && let Some(d) = v.strip_suffix('s').or_else(|| v.strip_suffix("'s"))
        && d.len() == 4
        && d.ends_with('0')
        && let Ok(y) = d.parse::<u32>()
    {
        return Ok(Cond::new(f, Op::Between, Value::Range(f64::from(y), f64::from(y + 9))));
    }
    let range = v.split_once("..").or_else(|| {
        // 1990-1999 for years (a year is never negative).
        if f == Field::Year { v.split_once('-').filter(|(a, b)| a.len() == 4 && b.len() == 4) } else { None }
    });
    if let Some((a, b)) = range {
        let (a, b) = (a.trim(), b.trim());
        return Ok(match (a.is_empty(), b.is_empty()) {
            (true, true) => return Err(bad()),
            (false, true) => Cond::new(f, Op::AtLeast, Value::Num(num(a)?)),
            (true, false) => Cond::new(f, Op::AtMost, Value::Num(num(b)?)),
            (false, false) => {
                let (x, y) = (num(a)?, num(b)?);
                Cond::new(f, Op::Between, Value::Range(x.min(y), x.max(y)))
            }
        });
    }
    let (cmp, rest) = split_cmp(v);
    let n = num(rest)?;
    Ok(match cmp {
        Cmp::Eq => Cond::new(f, Op::Is, Value::Num(n)),
        Cmp::Ge => Cond::new(f, Op::AtLeast, Value::Num(n)),
        Cmp::Gt => Cond::new(f, Op::AtLeast, Value::Num(n + 1.0)),
        Cmp::Le => Cond::new(f, Op::AtMost, Value::Num(n)),
        Cmp::Lt => Cond::new(f, Op::AtMost, Value::Num(n - 1.0)),
    })
}

/// A number typed for field `f`, in its own units, whole (every number field is whole): a length
/// in seconds from `3:30`, `1:02:03`, `90s`, `5m`, `1h` or minutes (`5`, as the editor reads it);
/// a sample rate in Hz from kHz (`44.1`, `96k`) or Hz; a bitrate with or without `kbps`.
fn number(f: Field, s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let plain = |s: &str| -> Option<f64> {
        let v: f64 = s.trim().replace(',', ".").parse().ok()?;
        (0.0..1e12).contains(&v).then_some(v)
    };
    let whole = |s: &str| -> Option<f64> { s.trim().parse::<u64>().ok().filter(|v| *v < 1_000_000_000_000).map(|v| v as f64) };
    let v = match f {
        Field::Duration => {
            if s.contains(':') {
                let parts: Option<Vec<u64>> = s.split(':').map(|p| p.trim().parse::<u64>().ok()).collect();
                let secs = match parts?.as_slice() {
                    [m, sec] if *sec < 60 => m.checked_mul(60)?.checked_add(*sec)?,
                    [h, m, sec] if *m < 60 && *sec < 60 => h.checked_mul(3600)?.checked_add(m * 60 + sec)?,
                    _ => return None,
                };
                // As long as a plain number may be (no track is near it).
                if secs >= 1_000_000_000_000 {
                    return None;
                }
                secs as f64
            } else if let Some(x) = s.strip_suffix("sec").or_else(|| s.strip_suffix('s')) {
                plain(x)?.round()
            } else if let Some(x) = s.strip_suffix("min").or_else(|| s.strip_suffix('m')) {
                (plain(x)? * 60.0).round()
            } else if let Some(x) = s.strip_suffix('h') {
                (plain(x)? * 3600.0).round()
            } else {
                (plain(&s)? * 60.0).round()
            }
        }
        Field::SampleRate => {
            let x = s.trim_end_matches("hz").trim_end_matches('k').trim();
            let v = plain(x)?;
            if v <= 0.0 {
                return None;
            }
            if v >= 1000.0 { v.round() } else { (v * 1000.0).round() }
        }
        Field::Bitrate => whole(s.trim_end_matches("kbps").trim_end_matches("kb/s").trim_end_matches('k'))?,
        Field::BitDepth => whole(s.trim_end_matches("bits").trim_end_matches("bit").trim_end_matches('-').trim_end_matches('b'))?,
        Field::Year => {
            let y = whole(&s)?;
            if !(1000.0..=9999.0).contains(&y) {
                return None;
            }
            y
        }
        _ => whole(&s)?,
    };
    Some(v)
}

fn unit(s: &str) -> Option<Unit> {
    Some(match s.trim() {
        "d" | "day" | "days" => Unit::Days,
        "w" | "wk" | "wks" | "week" | "weeks" => Unit::Weeks,
        "m" | "mo" | "mon" | "month" | "months" => Unit::Months,
        "y" | "yr" | "yrs" | "year" | "years" => Unit::Years,
        _ => return None,
    })
}

/// `30d`, `2 weeks`, `6m`, `1y`.
fn ago(s: &str) -> Option<(u32, Unit)> {
    let s = s.trim().to_ascii_lowercase();
    let digits = s.find(|c: char| !c.is_ascii_digit())?;
    let n: u32 = s[..digits].parse().ok().filter(|n| (1..=100_000).contains(n))?;
    Some((n, unit(&s[digits..])?))
}

fn date_rule(f: Field, v: &str) -> Result<Node, String> {
    let low = v.to_ascii_lowercase();
    if f == Field::LastPlayed {
        match low.as_str() {
            "never" | "no" => return Ok(Node::Cond(Cond::new(Field::Played, Op::No, Value::None))),
            "ever" | "yes" | "any" => return Ok(Node::Cond(Cond::new(Field::Played, Op::Yes, Value::None))),
            _ => {}
        }
    }
    let bad = || "not a date (2024-03-15, 2024) or a time ago (30d, 6m, 1y)".to_owned();
    let date = |s: &str| DateSpec::parse(s).ok_or_else(bad);
    let c = |op: Op, value: Value| Ok(Node::Cond(Cond::new(f, op, value)));
    if let Some((a, b)) = v.split_once("..") {
        let (a, b) = (a.trim(), b.trim());
        return match (a.is_empty(), b.is_empty()) {
            (true, true) => Err(bad()),
            (false, true) => c(Op::After, Value::Date(prev(date(a)?).ok_or_else(bad)?)),
            (true, false) => c(Op::Before, Value::Date(next(date(b)?).ok_or_else(bad)?)),
            (false, false) => {
                let (x, y) = (date(a)?, date(b)?);
                let (x, y) = if x.first_day() <= y.first_day() { (x, y) } else { (y, x) };
                c(Op::Between, Value::Dates(x, y))
            }
        };
    }
    let (cmp, rest) = split_cmp(v);
    if let Some((n, u)) = ago(rest) {
        // Less than 30 days ago: in the last 30 days. More than a year ago: not in the last year.
        return match cmp {
            Cmp::Eq | Cmp::Lt | Cmp::Le => c(Op::InLast, Value::Ago(n, u)),
            Cmp::Gt | Cmp::Ge => c(Op::NotInLast, Value::Ago(n, u)),
        };
    }
    let d = date(rest)?;
    match cmp {
        Cmp::Eq => c(Op::Between, Value::Dates(d, d)),
        Cmp::Gt => c(Op::After, Value::Date(d)),
        Cmp::Ge => c(Op::After, Value::Date(prev(d).ok_or_else(bad)?)),
        Cmp::Lt => c(Op::Before, Value::Date(d)),
        Cmp::Le => c(Op::Before, Value::Date(next(d).ok_or_else(bad)?)),
    }
}

/// The playlist named `name` (case, width and accents aside), for `playlist:`.
pub fn playlist_named(conn: &rusqlite::Connection, name: &str) -> Option<i64> {
    let want = crate::fold::fold(name.trim());
    let mut st = conn.prepare_cached("SELECT id, name FROM playlists ORDER BY id").ok()?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).ok()?;
    rows.flatten().find(|(_, n)| crate::fold::fold(n.trim()) == want).map(|(id, _)| id)
}

/// `q` read with the playlists of `conn`.
pub fn parse_in(conn: &rusqlite::Connection, q: &str) -> Parsed {
    parse(q, &|name| playlist_named(conn, name))
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
