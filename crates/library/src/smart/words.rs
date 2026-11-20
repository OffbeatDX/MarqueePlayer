//! A rule set in plain words, for the Playlists page and the playlist's own page:
//! "Genre is Jazz · 50 tracks, most played".

use super::{Cond, Field, Group, Kind, Limiting, Node, Op, Smart, Sort, SortKey, Value};

fn num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

/// A length in seconds as the app shows lengths ("3:30", "1:02:03").
pub fn length(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// A field's number as the editor shows it: lengths "3:30", sample rates in kHz.
pub fn shown_number(field: Field, v: f64) -> String {
    match field {
        Field::Duration => length(v),
        Field::SampleRate => crate::audio::khz(v as u32),
        _ => num(v),
    }
}

fn unit_suffix(field: Field) -> &'static str {
    match field {
        Field::Bitrate => " kbps",
        Field::SampleRate => " kHz",
        Field::BitDepth => "-bit",
        _ => "",
    }
}

/// One condition in a few words. `playlist` names a playlist by id.
pub fn cond_words(c: &Cond, playlist: &dyn Fn(i64) -> Option<String>) -> String {
    let field = c.field.label();
    let v = |x: f64| format!("{}{}", shown_number(c.field, x), unit_suffix(c.field));
    match (c.field, c.op, &c.value) {
        (Field::Favourite, Op::Yes, _) => "favourite".into(),
        (Field::Favourite, Op::No, _) => "not a favourite".into(),
        (Field::AlbumFavourite, Op::Yes, _) => "on a favourite album".into(),
        (Field::AlbumFavourite, Op::No, _) => "not on a favourite album".into(),
        (Field::Played, Op::Yes, _) => "played".into(),
        (Field::Played, Op::No, _) => "never played".into(),
        (Field::Lossless, Op::Yes, _) => "lossless".into(),
        (Field::Lossless, Op::No, _) => "lossy".into(),
        (Field::Remote, Op::Yes, _) => "from the server".into(),
        (Field::Remote, Op::No, _) => "with a file here".into(),
        (Field::Plays, Op::Is, Value::Num(n)) if *n == 0.0 => "never played".into(),
        (Field::Plays, Op::AtLeast, Value::Num(n)) => format!("{}+ plays", num(*n)),
        (Field::Plays, Op::AtMost, Value::Num(n)) => format!("{} plays or fewer", num(*n)),
        (Field::Plays, Op::Is, Value::Num(n)) => format!("{} {}", num(*n), if *n == 1.0 { "play" } else { "plays" }),
        (Field::PlaysIn, op, value) => {
            let (n, unit) = c.within.unwrap_or((30, super::Unit::Days));
            let count = match (op, value) {
                (Op::AtLeast, Value::Num(x)) => format!("{}+ plays", num(*x)),
                (Op::AtMost, Value::Num(x)) => format!("{} plays or fewer", num(*x)),
                (Op::Is, Value::Num(x)) => format!("{} {}", num(*x), if *x == 1.0 { "play" } else { "plays" }),
                (Op::IsNot, Value::Num(x)) => format!("not {} plays", num(*x)),
                (Op::Between, Value::Range(a, b)) => format!("{}\u{2013}{} plays", num(*a), num(*b)),
                _ => "plays".into(),
            };
            format!("{count} in the last {}", if n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(n)) })
        }
        (Field::Year, Op::Is, Value::Num(n)) => format!("from {}", num(*n)),
        (Field::Year, Op::AtLeast, Value::Num(n)) => format!("from {} on", num(*n)),
        (Field::Year, Op::AtMost, Value::Num(n)) => format!("from {} or before", num(*n)),
        (Field::Year, Op::Between, Value::Range(a, b)) => format!("from {}\u{2013}{}", num(*a), num(*b)),
        (Field::Year, Op::IsNot, Value::Num(n)) => format!("not from {}", num(*n)),
        (Field::Playlist, op, Value::Id(id)) => {
            let name = playlist(*id).unwrap_or_else(|| "a missing playlist".into());
            format!("{} \u{201c}{name}\u{201d}", if op == Op::In { "in" } else { "not in" })
        }
        (_, Op::InLast, Value::Ago(n, unit)) => format!("{} in the last {}", field.to_lowercase(), if *n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(*n)) }),
        (Field::LastPlayed, Op::NotInLast, Value::Ago(n, unit)) => format!("not played in the last {}", if *n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(*n)) }),
        (_, Op::NotInLast, Value::Ago(n, unit)) => format!("{} not in the last {}", field.to_lowercase(), if *n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(*n)) }),
        (_, Op::Before, Value::Date(d)) => format!("{} before {}", field.to_lowercase(), d.iso()),
        (_, Op::After, Value::Date(d)) => format!("{} after {}", field.to_lowercase(), d.iso()),
        (_, Op::Between, Value::Dates(a, b)) => format!("{} {}\u{2013}{}", field.to_lowercase(), a.iso(), b.iso()),
        (_, op, Value::Text(t)) => {
            // A value picked from the library reads as a name; typed text is quoted.
            if c.field.kind() == Kind::Choice && matches!(op, Op::Is | Op::IsNot) {
                format!("{field} {} {}", op.label(), t.trim())
            } else {
                format!("{field} {} \u{201c}{}\u{201d}", op.label(), t.trim())
            }
        }
        (_, op, Value::Num(n)) => format!("{} {} {}", field.to_lowercase(), op.label(), v(*n)),
        (_, Op::Between, Value::Range(a, b)) => format!("{} {}\u{2013}{}", field.to_lowercase(), v(*a), v(*b)),
        _ => field.to_lowercase(),
    }
}

/// How an order reads after a limit or alone: "most played", "random", "by title".
pub fn sort_words(s: &Sort) -> String {
    match (s.key, s.desc) {
        (SortKey::Random, _) => "random".into(),
        (SortKey::Plays, true) => "most played".into(),
        (SortKey::Plays, false) => "least played".into(),
        (SortKey::PlaysIn(n, unit), true) => format!("most played in the last {}", if n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(n)) }),
        (SortKey::PlaysIn(n, unit), false) => format!("least played in the last {}", if n == 1 { unit.words(1).to_owned() } else { format!("{n} {}", unit.words(n)) }),
        (SortKey::Added, true) => "most recently added".into(),
        (SortKey::Added, false) => "first added".into(),
        (SortKey::LastPlayed, true) => "most recently played".into(),
        (SortKey::LastPlayed, false) => "least recently played".into(),
        (SortKey::Year, true) => "newest".into(),
        (SortKey::Year, false) => "oldest".into(),
        (SortKey::Duration, true) => "longest".into(),
        (SortKey::Duration, false) => "shortest".into(),
        (k, desc) => format!("by {}{}", k.label().to_lowercase(), if desc { ", Z to A" } else { "" }),
    }
}

fn group_words(g: &Group, playlist: &dyn Fn(i64) -> Option<String>, top: bool) -> String {
    let parts: Vec<String> = g
        .nodes
        .iter()
        .map(|n| match n {
            Node::Cond(c) => cond_words(c, playlist),
            // A group of one rule reads as that rule; of several, in brackets.
            Node::Group(inner) if inner.nodes.len() == 1 => group_words(inner, playlist, false),
            Node::Group(inner) => format!("({})", group_words(inner, playlist, false)),
            Node::Unknown { .. } => "a rule from a newer version".into(),
        })
        .collect();
    match (g.all, top, parts.len()) {
        (_, _, 0) => String::new(),
        (true, true, _) => parts.join(" \u{b7} "),
        (true, false, _) => parts.join(" and "),
        (false, _, 1) => parts[0].clone(),
        (false, true, _) => format!("any of: {}", parts.join(", ")),
        (false, false, _) => parts.join(" or "),
    }
}

/// The whole rule set in a line: its conditions joined by " · " ("any of: …" for any), then
/// its limit with what selects it, or its order when that says something ("random").
pub fn summary(smart: &Smart, playlist: &dyn Fn(i64) -> Option<String>) -> String {
    let mut parts = Vec::new();
    let rules = group_words(&smart.root, playlist, true);
    parts.push(if rules.is_empty() { "Every track".to_owned() } else { capitalise(&rules) });
    match &smart.limit {
        Some(Limiting::Known(l)) => {
            parts.push(format!("{} {}, {}", l.n, l.unit.words(l.n), sort_words(&l.by)));
        }
        Some(Limiting::Unknown(_)) => parts.push("a limit from a newer version".into()),
        None => {
            let s = smart.order();
            if s.key == SortKey::Random {
                parts.push("random order".into());
            }
        }
    }
    parts.join(" \u{b7} ")
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
