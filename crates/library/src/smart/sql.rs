//! A rule set as one SQL query over the library's tables. Everything the user typed or
//! picked goes in as a bound parameter; the SQL text is made only of this file's own
//! fragments, so no value can change what the query does. Multi-valued fields and
//! favourites are non-correlated `IN (...)` subqueries (evaluated once, then looked up);
//! plays, last played and ratings are correlated lookups on their indexes
//! (`plays_track_time`, schema 9; `track_ratings`' key).

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use super::{Ctx, Field, Group, Kind, Limiting, MAX_DEPTH, Node, Op, Smart, Sort, SortKey, Value, ago};
use crate::query::{TRACK_COLS, Track, track_row};

/// What a smart playlist holds now, and what in it could not be evaluated (in words).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub tracks: Vec<Track>,
    pub problems: Vec<String>,
}

/// SQL text with its parameters, in order.
#[derive(Default)]
struct Frag {
    sql: String,
    params: Vec<Sql>,
}

impl Frag {
    fn of(sql: &str) -> Frag {
        Frag { sql: sql.to_owned(), params: Vec::new() }
    }

    fn s(&mut self, sql: &str) -> &mut Self {
        self.sql.push_str(sql);
        self
    }

    fn p(&mut self, v: impl Into<Sql>) -> &mut Self {
        self.sql.push('?');
        self.params.push(v.into());
        self
    }

    fn f(&mut self, other: Frag) -> &mut Self {
        self.sql.push_str(&other.sql);
        self.params.extend(other.params);
        self
    }
}

const FROM: &str = " FROM tracks t JOIN albums a ON a.id = t.album_id";
const YEAR: &str = "(CASE WHEN a.date GLOB '[0-9][0-9][0-9][0-9]*' THEN CAST(substr(a.date, 1, 4) AS INTEGER) END)";
const PLAYS: &str = "(SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id)";
const LAST: &str = "(SELECT MAX(p.played_at) FROM plays p WHERE p.track_id = t.id)";
const RATING: &str = "COALESCE((SELECT r.rating FROM track_ratings r WHERE r.track_id = t.id), 0)";
const LOSSLESS: &str = "t.codec IN ('FLAC', 'ALAC', 'WAV', 'AIFF')";
const LOSSY: &str = "t.codec IN ('MP3', 'MP2', 'AAC', 'Opus', 'Vorbis')";
/// The tie-breakers after every order: an album's tracks in their order, then the id, so
/// the same library always gives the same list.
const TIES: &str = "a.id, COALESCE(t.disc_no, 1), t.track_no, t.id";

/// LIKE pattern pieces: the user's text with LIKE's own characters escaped.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

struct Compiler<'c> {
    conn: &'c Connection,
    ctx: Ctx,
    /// The smart playlists being compiled, outermost first (cycle refusal).
    stack: Vec<i64>,
    problems: Vec<String>,
}

impl Compiler<'_> {
    fn problem(&mut self, p: String) {
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    /// The ids `smart` holds, in no particular order unless limited (then the limit's).
    fn selection(&mut self, smart: &Smart) -> Frag {
        let mut f = Frag::default();
        match &smart.limit {
            None => {
                f.s("SELECT t.id").s(FROM).s(" WHERE ");
                let w = self.group(&smart.root, true);
                f.f(w);
            }
            Some(Limiting::Unknown(_)) => {
                f.s("SELECT t.id").s(FROM).s(" WHERE 0");
            }
            Some(Limiting::Known(l)) => match l.seconds() {
                None => {
                    f.s("SELECT t.id").s(FROM).s(" WHERE ");
                    let w = self.group(&smart.root, true);
                    f.f(w).s(" ORDER BY ");
                    let o = self.order(&l.by, smart.seed);
                    f.f(o).s(" LIMIT ").p(i64::from(l.n));
                }
                Some(secs) => {
                    f.s("SELECT id FROM (SELECT t.id AS id, SUM(t.duration) OVER (ORDER BY ");
                    let o = self.order(&l.by, smart.seed);
                    f.f(o).s(" ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS run").s(FROM).s(" WHERE ");
                    let w = self.group(&smart.root, true);
                    f.f(w).s(") WHERE run <= ").p(secs);
                }
            },
        }
        f
    }

    /// `SELECT <cols> ... WHERE <what it holds>`, without its ORDER BY.
    fn query(&mut self, smart: &Smart, cols: &str) -> Frag {
        let mut f = Frag::default();
        f.s("SELECT ").s(cols).s(FROM).s(" WHERE ");
        if smart.limit.is_none() {
            let w = self.group(&smart.root, true);
            f.f(w);
        } else {
            f.s("t.id IN (");
            let sel = self.selection(smart);
            f.f(sel).s(")");
        }
        f
    }

    fn group(&mut self, g: &Group, top: bool) -> Frag {
        if g.nodes.is_empty() {
            // No rules: everything (the whole library, at the top); an empty group nested
            // in another matches what "all of nothing" / "any of nothing" does.
            return Frag::of(if top || g.all { "1" } else { "0" });
        }
        let mut f = Frag::of("(");
        for (i, n) in g.nodes.iter().enumerate() {
            if i > 0 {
                f.s(if g.all { " AND " } else { " OR " });
            }
            let part = match n {
                Node::Cond(c) => self.cond(c),
                Node::Group(inner) => self.group(inner, false),
                Node::Unknown { .. } => Frag::of("0"),
            };
            f.f(part);
        }
        f.s(")");
        f
    }

    fn cond(&mut self, c: &super::Cond) -> Frag {
        if c.check().is_err() {
            // A value that does not fit (a damaged file): matches nothing, never everything.
            self.problem(format!("A rule on {} that is not complete: it matches nothing.", c.field.label().to_lowercase()));
            return Frag::of("0");
        }
        match c.field.kind() {
            Kind::Text | Kind::Choice => self.text(c),
            Kind::Number => self.number(c),
            Kind::Date => self.date(c),
            Kind::Bool => Frag::of(match (c.field, c.op == Op::Yes) {
                (Field::Played, true) => "EXISTS (SELECT 1 FROM plays p WHERE p.track_id = t.id)",
                (Field::Played, false) => "NOT EXISTS (SELECT 1 FROM plays p WHERE p.track_id = t.id)",
                (Field::Favourite, true) => "t.id IN (SELECT track_id FROM favourite_tracks)",
                (Field::Favourite, false) => "t.id NOT IN (SELECT track_id FROM favourite_tracks)",
                (Field::AlbumFavourite, true) => "t.album_id IN (SELECT album_id FROM favourite_albums)",
                (Field::AlbumFavourite, false) => "t.album_id NOT IN (SELECT album_id FROM favourite_albums)",
                (Field::Lossless, true) => LOSSLESS,
                (Field::Lossless, false) => LOSSY,
                (Field::Remote, true) => "t.path LIKE 'navidrome:%'",
                (Field::Remote, false) => "t.path NOT LIKE 'navidrome:%'",
                _ => "0",
            }),
            Kind::Playlist => self.membership(c),
        }
    }

    fn text(&mut self, c: &super::Cond) -> Frag {
        let Value::Text(raw) = &c.value else { return Frag::of("0") };
        let mut v = crate::fold::fold(&super::clean_text(raw));
        if c.field == Field::Path {
            v = v.replace('/', "\\");
        }
        // The positive form of the operator, on a folded column `col`.
        let positive = |col: &str, op: Op| -> Frag {
            let mut f = Frag::default();
            match op {
                Op::Is | Op::IsNot => f.s(&format!("fold({col}) = ")).p(v.clone()),
                Op::Contains | Op::NotContains => f.s(&format!("fold({col}) LIKE ")).p(format!("%{}%", like_escape(&v))).s(" ESCAPE '\\'"),
                _ => f.s(&format!("fold({col}) LIKE ")).p(format!("{}%", like_escape(&v))).s(" ESCAPE '\\'"),
            };
            f
        };
        let not = if c.op.negative() { "NOT " } else { "" };
        let multi = |key: &str, table: &str, col: &str| -> Frag {
            let mut f = Frag::of(&format!("{key} {not}IN (SELECT x.{col} FROM {table} x WHERE "));
            f.f(positive("x.name", c.op)).s(")");
            f
        };
        match c.field {
            Field::Artist => multi("t.id", "track_artists", "track_id"),
            Field::AlbumArtist => multi("t.album_id", "album_artists", "album_id"),
            Field::Genre => multi("t.id", "track_genres", "track_id"),
            single => {
                let col = match single {
                    Field::Title => "t.title",
                    Field::Album => "a.title",
                    Field::Label => "a.label",
                    Field::Format => "t.codec",
                    // A server's own track has no file path: "" (`remote`).
                    Field::Path => "CASE WHEN t.path LIKE 'navidrome:%' THEN '' ELSE replace(t.path, '/', '\\') END",
                    _ => return Frag::of("0"),
                };
                if c.op.negative() {
                    // A missing value (no label) is not the value either.
                    let mut f = Frag::of("NOT COALESCE(");
                    f.f(positive(col, c.op)).s(", 0)");
                    f
                } else {
                    positive(col, c.op)
                }
            }
        }
    }

    /// The number a numeric field compares, as SQL.
    fn number_expr(&self, c: &super::Cond) -> Frag {
        match c.field {
            Field::Year => Frag::of(YEAR),
            Field::Plays => Frag::of(PLAYS),
            Field::PlaysIn => {
                let (n, unit) = c.within.unwrap_or((30, super::Unit::Days));
                let mut f = Frag::of("(SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id AND p.played_at >= ");
                f.p(ago(self.ctx, n, unit)).s(")");
                f
            }
            Field::Rating => Frag::of(RATING),
            // Whole seconds, as lengths are shown.
            Field::Duration => Frag::of("CAST(t.duration AS INTEGER)"),
            Field::Bitrate => Frag::of("t.bitrate"),
            Field::SampleRate => Frag::of("t.sample_rate"),
            Field::BitDepth => Frag::of("t.bit_depth"),
            // No disc number is disc 1, as the album page has it.
            Field::Disc => Frag::of("COALESCE(t.disc_no, 1)"),
            Field::TrackNo => Frag::of("t.track_no"),
            _ => Frag::of("NULL"),
        }
    }

    fn number(&mut self, c: &super::Cond) -> Frag {
        let e = self.number_expr(c);
        let mut f = Frag::default();
        let num = |v: f64| -> Sql { if v.fract() == 0.0 { Sql::Integer(v as i64) } else { Sql::Real(v) } };
        match (&c.value, c.op) {
            (Value::Num(v), Op::Is) => {
                f.s("(").f(e).s(" = ").p(num(*v)).s(")");
            }
            (Value::Num(v), Op::IsNot) => {
                let e2 = self.number_expr(c);
                f.s("(").f(e).s(" IS NULL OR ").f(e2).s(" <> ").p(num(*v)).s(")");
            }
            (Value::Num(v), Op::AtLeast) => {
                f.s("(").f(e).s(" >= ").p(num(*v)).s(")");
            }
            (Value::Num(v), Op::AtMost) => {
                f.s("(").f(e).s(" <= ").p(num(*v)).s(")");
            }
            (Value::Range(a, b), Op::Between) => {
                f.s("(").f(e).s(" BETWEEN ").p(num(*a)).s(" AND ").p(num(*b)).s(")");
            }
            _ => {
                f.s("0");
            }
        }
        f
    }

    fn date(&mut self, c: &super::Cond) -> Frag {
        // Added is kept in milliseconds, plays in seconds.
        let (col, k) = match c.field {
            Field::Added => ("a.added_at", 1000),
            _ => (LAST, 1),
        };
        let day = |d: i64| self.ctx.day_start(d) * k;
        let mut f = Frag::default();
        match (&c.value, c.op) {
            (Value::Ago(n, unit), Op::InLast) => {
                f.s(&format!("({col} >= ")).p(ago(self.ctx, *n, *unit) * k).s(")");
            }
            (Value::Ago(n, unit), Op::NotInLast) => {
                // Never played is not played in the last N either.
                f.s(&format!("({col} IS NULL OR {col} < ")).p(ago(self.ctx, *n, *unit) * k).s(")");
            }
            (Value::Date(d), Op::Before) => {
                f.s(&format!("({col} < ")).p(day(d.first_day())).s(")");
            }
            (Value::Date(d), Op::After) => {
                f.s(&format!("({col} >= ")).p(day(d.end_day())).s(")");
            }
            (Value::Dates(a, b), Op::Between) => {
                f.s(&format!("({col} >= ")).p(day(a.first_day())).s(&format!(" AND {col} < ")).p(day(b.end_day())).s(")");
            }
            _ => {
                f.s("0");
            }
        }
        f
    }

    fn membership(&mut self, c: &super::Cond) -> Frag {
        let Value::Id(id) = c.value else { return Frag::of("0") };
        let not = if c.op == Op::NotIn { "NOT " } else { "" };
        let found: Option<(String, Option<String>)> = self
            .conn
            .prepare_cached("SELECT name, rules FROM playlists WHERE id = ?1")
            .and_then(|mut st| st.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).optional())
            .unwrap_or(None);
        match found {
            None => {
                self.problem("A rule names a playlist that no longer exists: it matches nothing.".into());
                Frag::of("0")
            }
            Some((_, None)) => {
                let mut f = Frag::of(&format!("t.id {not}IN (SELECT i.track_id FROM playlist_items i WHERE i.playlist_id = "));
                f.p(id).s(")");
                f
            }
            Some((name, Some(rules))) => {
                if self.stack.contains(&id) {
                    self.problem(format!("“{name}” comes back to this playlist through its rules: that rule matches nothing."));
                    return Frag::of("0");
                }
                if self.stack.len() >= MAX_DEPTH {
                    self.problem(format!("Smart playlists refer to each other too deeply at “{name}”: that rule matches nothing."));
                    return Frag::of("0");
                }
                let inner = Smart::from_json(&rules);
                self.stack.push(id);
                let sel = self.selection(&inner);
                self.stack.pop();
                let mut f = Frag::of(&format!("t.id {not}IN ("));
                f.f(sel).s(")");
                f
            }
        }
    }

    /// ORDER BY terms for `s`: its key (missing values last, whichever way round), then the
    /// tie-breakers.
    fn order(&self, s: &Sort, seed: i64) -> Frag {
        let dir = if s.desc { " DESC" } else { "" };
        let mut f = Frag::default();
        let nullable = |f: &mut Frag, e: &str| {
            f.s(&format!("({e}) IS NULL, {e}{dir}, "));
        };
        match s.key {
            SortKey::ArtistAlbum => {
                f.s(&format!("COALESCE(a.sort_artist, a.album_artist) COLLATE NOCASE{dir}, "));
                nullable(&mut f, "a.date");
                f.s(&format!("a.title COLLATE NOCASE{dir}, "));
            }
            SortKey::Title => {
                f.s(&format!("t.title COLLATE NOCASE{dir}, "));
            }
            SortKey::Artist => {
                f.s(&format!("t.artist COLLATE NOCASE{dir}, "));
            }
            SortKey::Album => {
                f.s(&format!("a.title COLLATE NOCASE{dir}, "));
            }
            SortKey::Year => nullable(&mut f, YEAR),
            SortKey::Added => {
                f.s(&format!("a.added_at{dir}, "));
            }
            SortKey::LastPlayed => nullable(&mut f, LAST),
            SortKey::Plays => {
                f.s(&format!("{PLAYS}{dir}, "));
            }
            SortKey::PlaysIn(n, unit) => {
                f.s("(SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id AND p.played_at >= ").p(ago(self.ctx, n, unit)).s(&format!("){dir}, "));
            }
            SortKey::Rating => {
                f.s(&format!("{RATING}{dir}, "));
            }
            SortKey::Duration => {
                f.s(&format!("t.duration{dir}, "));
            }
            SortKey::Random => {
                f.s("seeded_rand(").p(seed).s(", t.id), ");
            }
        }
        f.s(TIES);
        f
    }
}

fn compiler(conn: &Connection, me: Option<i64>, ctx: Ctx) -> Compiler<'_> {
    Compiler { conn, ctx, stack: me.into_iter().collect(), problems: Vec::new() }
}

fn problems(smart: &Smart, c: Compiler<'_>) -> Vec<String> {
    let mut v = smart.problems();
    for p in c.problems {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// The tracks `smart` holds now, in its order. `me` is the playlist it is (refused if its
/// rules come back to it); `ctx` the time it is evaluated at.
pub fn evaluate(conn: &Connection, smart: &Smart, me: Option<i64>, ctx: Ctx) -> rusqlite::Result<Outcome> {
    let mut c = compiler(conn, me, ctx);
    let mut f = c.query(smart, TRACK_COLS);
    f.s(" ORDER BY ");
    let o = c.order(&smart.order(), smart.seed);
    f.f(o);
    let mut st = conn.prepare_cached(&f.sql)?;
    let tracks = st.query_map(params_from_iter(f.params), track_row)?.collect::<rusqlite::Result<Vec<Track>>>()?;
    Ok(Outcome { tracks, problems: problems(smart, c) })
}

/// The ids `smart` holds now, in its order.
pub fn evaluate_ids(conn: &Connection, smart: &Smart, me: Option<i64>, ctx: Ctx) -> rusqlite::Result<(Vec<i64>, Vec<String>)> {
    let mut c = compiler(conn, me, ctx);
    let mut f = c.query(smart, "t.id");
    f.s(" ORDER BY ");
    let o = c.order(&smart.order(), smart.seed);
    f.f(o);
    let mut st = conn.prepare_cached(&f.sql)?;
    let ids = st.query_map(params_from_iter(f.params), |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok((ids, problems(smart, c)))
}

/// How many tracks `smart` holds now and how long they are in all (seconds).
pub fn count(conn: &Connection, smart: &Smart, me: Option<i64>, ctx: Ctx) -> rusqlite::Result<(usize, f64)> {
    let mut c = compiler(conn, me, ctx);
    let f = c.query(smart, "COUNT(*), COALESCE(SUM(t.duration), 0)");
    let mut st = conn.prepare_cached(&f.sql)?;
    st.query_row(params_from_iter(f.params), |r| Ok((r.get::<_, i64>(0)? as usize, r.get(1)?)))
}

/// The albums with a track `smart` holds, as a subquery (`SELECT DISTINCT t.album_id ...`) with
/// its parameters in order, to go inside `IN (...)` of a query of albums (search by operators).
pub fn album_selection(conn: &Connection, smart: &Smart, ctx: Ctx) -> (String, Vec<Sql>) {
    let mut c = compiler(conn, None, ctx);
    let f = c.query(smart, "DISTINCT t.album_id");
    (f.sql, f.params)
}

/// A playlist's stored rules: None if there is no such playlist, Some(None) for an ordinary
/// one.
pub fn playlist_rules(conn: &Connection, id: i64) -> rusqlite::Result<Option<Option<String>>> {
    conn.query_row("SELECT rules FROM playlists WHERE id = ?1", [id], |r| r.get(0)).optional()
}

/// The playlists `smart`'s rules name.
pub fn references(smart: &Smart) -> Vec<i64> {
    fn walk(g: &Group, out: &mut Vec<i64>) {
        for n in &g.nodes {
            match n {
                Node::Cond(c) if c.field == Field::Playlist => {
                    if let Value::Id(id) = c.value
                        && !out.contains(&id)
                    {
                        out.push(id);
                    }
                }
                Node::Group(g) => walk(g, out),
                _ => {}
            }
        }
    }
    let mut v = Vec::new();
    walk(&smart.root, &mut v);
    v
}

/// Saving `smart` as playlist `me` would make it include itself (through the playlists it
/// names, and theirs), or nest deeper than `MAX_DEPTH`: the name of the playlist it names
/// that leads there. None when it is fine. `me` None: a new playlist, which nothing can name
/// yet (only depth is checked).
pub fn would_cycle(conn: &Connection, me: Option<i64>, smart: &Smart) -> Option<String> {
    fn reaches(conn: &Connection, me: Option<i64>, id: i64, depth: usize, seen: &mut Vec<i64>) -> bool {
        if Some(id) == me || depth > MAX_DEPTH {
            return true;
        }
        if seen.contains(&id) {
            return false;
        }
        seen.push(id);
        let Ok(Some(Some(rules))) = playlist_rules(conn, id) else { return false };
        references(&Smart::from_json(&rules)).into_iter().any(|r| reaches(conn, me, r, depth + 1, seen))
    }
    for id in references(smart) {
        if reaches(conn, me, id, 1, &mut Vec::new()) {
            let name = conn.query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap_or_else(|_| "a playlist".into());
            return Some(name);
        }
    }
    None
}
