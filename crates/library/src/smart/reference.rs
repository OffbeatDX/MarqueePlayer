//! A second, deliberately simple evaluation of smart playlist rules, in memory and in plain
//! Rust, written from the rules' meaning rather than from the SQL. The tests run both on the
//! same library and compare the results (members and order), so each checks the other.

use std::cmp::Ordering;
use std::collections::HashMap;

use rusqlite::Connection;

use super::{Ctx, Field, Group, Kind, Limiting, MAX_DEPTH, Node, Op, Smart, Sort, SortKey, Value, ago};
use crate::fold::fold;

#[derive(Clone, Debug, Default)]
pub struct RTrack {
    pub id: i64,
    pub album_id: i64,
    pub title: String,
    pub artist: String,
    pub path: String,
    pub duration: f64,
    pub disc: Option<i64>,
    pub track_no: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub bitrate: Option<i64>,
    pub album: String,
    pub album_artist: String,
    pub sort_artist: Option<String>,
    pub date: Option<String>,
    pub label: Option<String>,
    pub added_ms: i64,
    pub credited: Vec<String>,
    pub album_artists: Vec<String>,
    pub genres: Vec<String>,
    pub plays: Vec<i64>,
    pub favourite: bool,
    pub album_favourite: bool,
    pub rating: i64,
}

/// (playlist, the playlists around it) -> what it holds.
type Memo = HashMap<(i64, Vec<i64>), Vec<i64>>;

pub struct Lib {
    pub tracks: Vec<RTrack>,
    pub items: HashMap<i64, Vec<i64>>,
    /// Every playlist: its rules (None: ordinary).
    pub playlists: HashMap<i64, Option<String>>,
    /// What a nested smart playlist holds, by (its id, the playlists around it), within one
    /// evaluation (else every track would evaluate it again).
    memo: std::cell::RefCell<Memo>,
}

fn strings(conn: &Connection, sql: &str) -> HashMap<i64, Vec<String>> {
    let mut m: HashMap<i64, Vec<String>> = HashMap::new();
    let mut st = conn.prepare(sql).unwrap();
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (k, v) = row.unwrap();
        m.entry(k).or_default().push(v);
    }
    m
}

impl Lib {
    /// Everything the rules look at, read from the database in the plainest queries.
    pub fn load(conn: &Connection) -> Lib {
        let credited = strings(conn, "SELECT track_id, name FROM track_artists ORDER BY track_id, position");
        let album_artists = strings(conn, "SELECT album_id, name FROM album_artists ORDER BY album_id, position");
        let genres = strings(conn, "SELECT track_id, name FROM track_genres ORDER BY track_id, position");
        let mut plays: HashMap<i64, Vec<i64>> = HashMap::new();
        {
            let mut st = conn.prepare("SELECT track_id, played_at FROM plays").unwrap();
            for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).unwrap() {
                let (t, at) = r.unwrap();
                plays.entry(t).or_default().push(at);
            }
        }
        let set = |sql: &str| -> std::collections::HashSet<i64> { conn.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
        let fav = set("SELECT track_id FROM favourite_tracks");
        let afav = set("SELECT album_id FROM favourite_albums");
        let ratings: HashMap<i64, i64> =
            conn.prepare("SELECT track_id, rating FROM track_ratings").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        let mut st = conn
            .prepare(
                "SELECT t.id, t.album_id, t.title, t.artist, t.path, t.duration, t.disc_no, t.track_no, t.codec, t.sample_rate, t.bit_depth, t.bitrate,
                        a.title, a.album_artist, a.sort_artist, a.date, a.label, a.added_at
                 FROM tracks t, albums a WHERE a.id = t.album_id",
            )
            .unwrap();
        let tracks = st
            .query_map([], |r| {
                Ok(RTrack {
                    id: r.get(0)?,
                    album_id: r.get(1)?,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    path: r.get(4)?,
                    duration: r.get(5)?,
                    disc: r.get(6)?,
                    track_no: r.get(7)?,
                    codec: r.get(8)?,
                    sample_rate: r.get(9)?,
                    bit_depth: r.get(10)?,
                    bitrate: r.get(11)?,
                    album: r.get(12)?,
                    album_artist: r.get(13)?,
                    sort_artist: r.get(14)?,
                    date: r.get(15)?,
                    label: r.get(16)?,
                    added_ms: r.get(17)?,
                    ..RTrack::default()
                })
            })
            .unwrap()
            .map(Result::unwrap)
            .map(|mut t: RTrack| {
                t.credited = credited.get(&t.id).cloned().unwrap_or_default();
                t.album_artists = album_artists.get(&t.album_id).cloned().unwrap_or_default();
                t.genres = genres.get(&t.id).cloned().unwrap_or_default();
                t.plays = plays.get(&t.id).cloned().unwrap_or_default();
                t.favourite = fav.contains(&t.id);
                t.album_favourite = afav.contains(&t.album_id);
                t.rating = ratings.get(&t.id).copied().unwrap_or(0);
                t
            })
            .collect();
        let mut items: HashMap<i64, Vec<i64>> = HashMap::new();
        for r in conn.prepare("SELECT playlist_id, track_id FROM playlist_items").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() {
            let (p, t): (i64, i64) = r.unwrap();
            items.entry(p).or_default().push(t);
        }
        let playlists = conn.prepare("SELECT id, rules FROM playlists").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        Lib { tracks, items, playlists, memo: Default::default() }
    }

    /// The ids `smart` holds, in its order.
    pub fn eval(&self, smart: &Smart, me: Option<i64>, ctx: Ctx) -> Vec<i64> {
        self.memo.borrow_mut().clear();
        let mut stack: Vec<i64> = me.into_iter().collect();
        self.select(smart, &mut stack, ctx, true)
    }

    fn select(&self, smart: &Smart, stack: &mut Vec<i64>, ctx: Ctx, ordered: bool) -> Vec<i64> {
        let mut matched: Vec<&RTrack> = self.tracks.iter().filter(|t| self.group(&smart.root, t, stack, ctx, true)).collect();
        match &smart.limit {
            None => {}
            Some(Limiting::Unknown(_)) => matched.clear(),
            Some(Limiting::Known(l)) => {
                matched.sort_by(|a, b| self.cmp(a, b, &l.by, smart.seed, ctx));
                match l.seconds() {
                    None => matched.truncate(l.n as usize),
                    Some(limit) => {
                        let mut run = 0.0;
                        let mut keep = Vec::new();
                        for t in matched {
                            run += t.duration;
                            if run <= limit {
                                keep.push(t);
                            }
                        }
                        matched = keep;
                    }
                }
            }
        }
        if ordered {
            let order = smart.order();
            matched.sort_by(|a, b| self.cmp(a, b, &order, smart.seed, ctx));
        }
        matched.iter().map(|t| t.id).collect()
    }

    fn group(&self, g: &Group, t: &RTrack, stack: &mut Vec<i64>, ctx: Ctx, top: bool) -> bool {
        if g.nodes.is_empty() {
            return top || g.all;
        }
        let mut each = g.nodes.iter().map(|n| match n {
            Node::Cond(c) => self.cond(c, t, stack, ctx),
            Node::Group(inner) => self.group(inner, t, stack, ctx, false),
            Node::Unknown { .. } => false,
        });
        if g.all { each.all(|x| x) } else { each.any(|x| x) }
    }

    fn cond(&self, c: &super::Cond, t: &RTrack, stack: &mut Vec<i64>, ctx: Ctx) -> bool {
        if c.check().is_err() {
            return false;
        }
        match c.field.kind() {
            Kind::Text | Kind::Choice => {
                let Value::Text(raw) = &c.value else { return false };
                let path = c.field == Field::Path;
                let norm = |s: &str| if path { fold(s).replace('/', "\\") } else { fold(s) };
                let v = norm(&super::clean_text(raw));
                let positive = |s: &str| -> bool {
                    let s = norm(s);
                    match c.op {
                        Op::Is | Op::IsNot => s == v,
                        Op::Contains | Op::NotContains => s.contains(&v),
                        _ => s.starts_with(&v),
                    }
                };
                let values: Vec<&str> = match c.field {
                    Field::Artist => t.credited.iter().map(String::as_str).collect(),
                    Field::AlbumArtist => t.album_artists.iter().map(String::as_str).collect(),
                    Field::Genre => t.genres.iter().map(String::as_str).collect(),
                    Field::Title => vec![t.title.as_str()],
                    Field::Album => vec![t.album.as_str()],
                    Field::Label => t.label.as_deref().into_iter().collect(),
                    Field::Format => t.codec.as_deref().into_iter().collect(),
                    Field::Path => vec![if crate::remote::is_remote(&t.path) { "" } else { t.path.as_str() }],
                    _ => Vec::new(),
                };
                let any = values.iter().any(|s| positive(s));
                if c.op.negative() { !any } else { any }
            }
            Kind::Number => {
                let e: Option<f64> = match c.field {
                    Field::Year => year(t.date.as_deref()).map(|y| y as f64),
                    Field::Plays => Some(t.plays.len() as f64),
                    Field::PlaysIn => {
                        let (n, u) = c.within.unwrap();
                        let since = ago(ctx, n, u);
                        Some(t.plays.iter().filter(|&&p| p >= since).count() as f64)
                    }
                    Field::Rating => Some(t.rating as f64),
                    Field::Duration => Some((t.duration as i64) as f64),
                    Field::Bitrate => t.bitrate.map(|v| v as f64),
                    Field::SampleRate => t.sample_rate.map(|v| v as f64),
                    Field::BitDepth => t.bit_depth.map(|v| v as f64),
                    Field::Disc => Some(t.disc.unwrap_or(1) as f64),
                    Field::TrackNo => t.track_no.map(|v| v as f64),
                    _ => None,
                };
                match (c.op, &c.value) {
                    (Op::Is, Value::Num(v)) => e == Some(*v),
                    (Op::IsNot, Value::Num(v)) => e != Some(*v),
                    (Op::AtLeast, Value::Num(v)) => e.is_some_and(|e| e >= *v),
                    (Op::AtMost, Value::Num(v)) => e.is_some_and(|e| e <= *v),
                    (Op::Between, Value::Range(a, b)) => e.is_some_and(|e| *a <= e && e <= *b),
                    _ => false,
                }
            }
            Kind::Date => {
                let (e, k) = match c.field {
                    Field::Added => (Some(t.added_ms), 1000),
                    _ => (t.plays.iter().copied().max(), 1),
                };
                let start = |day: i64| ctx.day_start(day) * k;
                match (c.op, &c.value) {
                    (Op::InLast, Value::Ago(n, u)) => e.is_some_and(|e| e >= ago(ctx, *n, *u) * k),
                    (Op::NotInLast, Value::Ago(n, u)) => e.is_none_or(|e| e < ago(ctx, *n, *u) * k),
                    (Op::Before, Value::Date(d)) => e.is_some_and(|e| e < start(d.first_day())),
                    (Op::After, Value::Date(d)) => e.is_some_and(|e| e >= start(d.end_day())),
                    (Op::Between, Value::Dates(a, b)) => e.is_some_and(|e| e >= start(a.first_day()) && e < start(b.end_day())),
                    _ => false,
                }
            }
            Kind::Bool => {
                let lossless = matches!(t.codec.as_deref(), Some("FLAC" | "ALAC" | "WAV" | "AIFF"));
                let lossy = matches!(t.codec.as_deref(), Some("MP3" | "MP2" | "AAC" | "Opus" | "Vorbis"));
                let yes = c.op == Op::Yes;
                match c.field {
                    Field::Played => !t.plays.is_empty() == yes,
                    Field::Favourite => t.favourite == yes,
                    Field::AlbumFavourite => t.album_favourite == yes,
                    Field::Remote => t.path.starts_with("navidrome:") == yes,
                    Field::Lossless => {
                        if yes {
                            lossless
                        } else {
                            lossy
                        }
                    }
                    _ => false,
                }
            }
            Kind::Playlist => {
                let Value::Id(id) = c.value else { return false };
                let members: Vec<i64> = match self.playlists.get(&id) {
                    None => return false,
                    Some(None) => self.items.get(&id).cloned().unwrap_or_default(),
                    Some(Some(rules)) => {
                        if stack.contains(&id) || stack.len() >= MAX_DEPTH {
                            return false;
                        }
                        let key = (id, stack.clone());
                        if let Some(m) = self.memo.borrow().get(&key) {
                            return m.contains(&t.id) == (c.op == Op::In);
                        }
                        stack.push(id);
                        let inner = self.select(&Smart::from_json(rules), stack, ctx, false);
                        stack.pop();
                        self.memo.borrow_mut().insert(key, inner.clone());
                        inner
                    }
                };
                members.contains(&t.id) == (c.op == Op::In)
            }
        }
    }

    fn cmp(&self, a: &RTrack, b: &RTrack, s: &Sort, seed: i64, ctx: Ctx) -> Ordering {
        let dir = |o: Ordering| if s.desc { o.reverse() } else { o };
        // Missing values last, whichever way round; then the value in the order's direction.
        let nullable = |x: Option<f64>, y: Option<f64>| -> Ordering {
            x.is_none().cmp(&y.is_none()).then_with(|| match (x, y) {
                (Some(x), Some(y)) => dir(x.partial_cmp(&y).unwrap()),
                _ => Ordering::Equal,
            })
        };
        let nullable_text = |x: Option<&str>, y: Option<&str>| -> Ordering {
            x.is_none().cmp(&y.is_none()).then_with(|| match (x, y) {
                (Some(x), Some(y)) => dir(x.cmp(y)),
                _ => Ordering::Equal,
            })
        };
        let plays_since = |t: &RTrack, n: u32, u: super::Unit| t.plays.iter().filter(|&&p| p >= ago(ctx, n, u)).count();
        let primary = match s.key {
            SortKey::ArtistAlbum => dir(nocase(a.sort_artist.as_deref().unwrap_or(&a.album_artist), b.sort_artist.as_deref().unwrap_or(&b.album_artist)))
                .then_with(|| nullable_text(a.date.as_deref(), b.date.as_deref()))
                .then_with(|| dir(nocase(&a.album, &b.album))),
            SortKey::Title => dir(nocase(&a.title, &b.title)),
            SortKey::Artist => dir(nocase(&a.artist, &b.artist)),
            SortKey::Album => dir(nocase(&a.album, &b.album)),
            SortKey::Year => nullable(year(a.date.as_deref()).map(|y| y as f64), year(b.date.as_deref()).map(|y| y as f64)),
            SortKey::Added => dir(a.added_ms.cmp(&b.added_ms)),
            SortKey::LastPlayed => nullable(a.plays.iter().max().map(|&p| p as f64), b.plays.iter().max().map(|&p| p as f64)),
            SortKey::Plays => dir(a.plays.len().cmp(&b.plays.len())),
            SortKey::PlaysIn(n, u) => dir(plays_since(a, n, u).cmp(&plays_since(b, n, u))),
            SortKey::Rating => dir(a.rating.cmp(&b.rating)),
            SortKey::Duration => dir(a.duration.partial_cmp(&b.duration).unwrap()),
            SortKey::Random => crate::fold::seeded_rand(seed, a.id).cmp(&crate::fold::seeded_rand(seed, b.id)),
        };
        primary
            .then_with(|| a.album_id.cmp(&b.album_id))
            .then_with(|| a.disc.unwrap_or(1).cmp(&b.disc.unwrap_or(1)))
            .then_with(|| a.track_no.cmp(&b.track_no))
            .then_with(|| a.id.cmp(&b.id))
    }
}

/// SQLite's NOCASE: ASCII letters folded, then byte order.
fn nocase(a: &str, b: &str) -> Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

fn year(date: Option<&str>) -> Option<i64> {
    let d = date?;
    let head = d.get(..4)?;
    head.bytes().all(|b| b.is_ascii_digit()).then(|| head.parse().ok()).flatten()
}
