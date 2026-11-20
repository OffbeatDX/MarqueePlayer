//! Smart playlist rules on a fixture library: every field with every operator it allows,
//! checked against the in-memory reference (`reference.rs`) and, where it matters, against
//! hand-worked answers; relative dates on a fixed clock; limits and orders; the stable
//! shuffle; cycles; the JSON form and what an older build does with a newer one's; user
//! text that must never reach the SQL; and thousands of random rule sets, SQL against the
//! reference.

// The tests start from a default rule set and change one part of it at a time.
#![allow(clippy::field_reassign_with_default)]

use rusqlite::{Connection, params};

use super::reference::Lib;
use super::*;

/// 2025-06-15 12:00 UTC, two hours east of UTC.
pub fn ctx() -> Ctx {
    Ctx::new(days_from_civil(2025, 6, 15) * 86_400 + 12 * 3600, 7200)
}

/// A small, fixed random number generator (xorshift64*), so fixtures and random rule sets
/// are the same on every run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

const ARTISTS: [&str; 12] = ["Beyoncé", "Sigur Rós", "ヨルシカ", "Mara Lind", "Kenji Arai", "ＡＢＣ Band", "The Band", "100% Pure", "under_score", "Back\\slash", "O'Brien", "Guest"];
const GENRES: [&str; 8] = ["Jazz", "Rock", "Électro", "Hip-Hop/Rap", "Drum & Bass", "ボカロ", "Ambient", "jazz fusion"];
const TITLES: [&str; 14] = [
    "Café Noir",
    "いただきバベル",
    "Lemonade",
    "Ágætis byrjun",
    "Robert'); DROP TABLE tracks;--",
    "50% Off",
    "snake_case",
    "Love Song",
    "LOVE",
    "Night Drive",
    "ＦＵＬＬ Width",
    "Straße",
    "a\\b",
    "Interlude",
];
const LABELS: [Option<&str>; 5] = [None, Some("Blue Note"), Some("ECM"), Some("[no label]"), Some("Warp")];
const CODECS: [Option<&str>; 9] = [Some("FLAC"), Some("MP3"), Some("Opus"), Some("AAC"), Some("ALAC"), Some("WAV"), Some("Vorbis"), None, Some("")];
const DATES: [Option<&str>; 8] = [None, Some("1999"), Some("2004-03-15"), Some("2010-07"), Some("1987-01-01"), Some("abcd"), Some("2025-06-01"), Some("2024-12-31")];

/// A library of 40 albums and 360 tracks with a believable spread of everything rules look
/// at, made with `Rng`, plus playlists: "Ordinary" (id 1, a track twice), smart "Jazz"
/// (2: genre is jazz), smart "Jazz faves" (3: in Jazz, a favourite), smart "Loop A" (4) and
/// "Loop B" (5) naming each other, and an empty ordinary "Empty" (6).
pub fn fixture() -> Connection {
    let conn = crate::db::open_in_memory().unwrap();
    let mut rng = Rng::new(7);
    let c = ctx();
    let mut artist_ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut artist = |conn: &Connection, name: &str| -> i64 {
        if let Some(&id) = artist_ids.get(name) {
            return id;
        }
        let id = artist_ids.len() as i64 + 1;
        conn.execute("INSERT INTO artists (id, key, norm, name) VALUES (?1, ?2, ?3, ?4)", params![id, format!("n:{name}"), name.to_lowercase(), name]).unwrap();
        artist_ids.insert(name.to_owned(), id);
        id
    };
    for (i, g) in GENRES.iter().enumerate() {
        conn.execute("INSERT INTO genres (id, key, name) VALUES (?1, ?2, ?3)", params![i as i64 + 1, g.to_lowercase(), g]).unwrap();
    }
    let mut track_id = 0i64;
    for album in 1..=40i64 {
        let artists: Vec<&str> = (0..1 + rng.below(2)).map(|_| *rng.pick(&ARTISTS)).collect();
        let album_artist = artists.join("; ");
        let title = format!("{} {album}", rng.pick(&TITLES));
        let date = *rng.pick(&DATES);
        let label = *rng.pick(&LABELS);
        let added = (c.now - rng.below(3 * 365 * 86_400) as i64) * 1000 + rng.below(1000) as i64;
        let sort = if rng.chance(30) { Some(format!("{}, The", artists[0])) } else { None };
        conn.execute(
            "INSERT INTO albums (id, key, title, album_artist, date, label, folder, added_at, first_seen, sort_artist) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
            params![album, format!("k{album}"), title, album_artist, date, label, format!("M:\\Music\\{album}"), added, sort],
        )
        .unwrap();
        for (pos, a) in artists.iter().enumerate() {
            let id = artist(&conn, a);
            conn.execute("INSERT OR IGNORE INTO album_artists (album_id, position, artist_id, name, norm) VALUES (?1, ?2, ?3, ?4, ?5)", params![album, pos as i64, id, a, a.to_lowercase()]).unwrap();
        }
        if rng.chance(15) {
            conn.execute("INSERT INTO favourite_albums VALUES (?1, 1)", [album]).unwrap();
        }
        let codec = *rng.pick(&CODECS);
        for n in 0..9i64 {
            track_id += 1;
            let credited: Vec<&str> = (0..1 + rng.below(3)).map(|_| *rng.pick(&ARTISTS)).collect();
            let credit = credited.join(" feat. ");
            let title = rng.pick(&TITLES).to_string();
            let duration = (30 * 4 + rng.below(570 * 4)) as f64 / 4.0;
            let disc: Option<i64> = [None, Some(1), Some(2)][rng.below(3)];
            let number: Option<i64> = if rng.chance(85) { Some(n + 1) } else { None };
            let codec = if rng.chance(10) { *rng.pick(&CODECS) } else { codec };
            let lossless = matches!(codec, Some("FLAC" | "ALAC" | "WAV"));
            let rate: Option<i64> = codec.map(|_| *rng.pick(&[44_100, 48_000, 96_000, 192_000]));
            let depth: Option<i64> = if lossless { Some(*rng.pick(&[16, 24])) } else { None };
            let bitrate: Option<i64> = codec.map(|_| *rng.pick(&[128, 192, 256, 320, 900, 1411]));
            let path = format!("M:\\Music\\{album}\\{:02} {}.{}", n + 1, title.replace(['\\', '/'], "-"), codec.unwrap_or("x").to_lowercase());
            conn.execute(
                "INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, 0, ?9, ?10, ?11, ?12, 2)",
                params![track_id, path, album, title, credit, disc, number, duration, codec, rate, depth, bitrate],
            )
            .unwrap();
            for (pos, a) in credited.iter().enumerate() {
                let id = artist(&conn, a);
                conn.execute(
                    "INSERT INTO track_artists (track_id, position, artist_id, name, norm, featured) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![track_id, pos as i64, id, a, a.to_lowercase(), pos > 0],
                )
                .unwrap();
            }
            for pos in 0..rng.below(3) {
                let g = rng.below(GENRES.len());
                conn.execute("INSERT OR IGNORE INTO track_genres (track_id, position, genre_id, name) VALUES (?1, ?2, ?3, ?4)", params![track_id, pos as i64, g as i64 + 1, GENRES[g]]).unwrap();
            }
            for _ in 0..[0, 0, 0, 1, 1, 2, 3, 5, 8][rng.below(9)] {
                let at = c.now - rng.below(2 * 365 * 86_400) as i64;
                conn.execute("INSERT INTO plays VALUES (?1, ?2)", params![track_id, at]).unwrap();
            }
            if rng.chance(20) {
                conn.execute("INSERT INTO favourite_tracks VALUES (?1, 1)", [track_id]).unwrap();
            }
            // Ratings left by a build that kept them: read by nothing now.
            if rng.chance(50) {
                conn.execute("INSERT INTO track_ratings VALUES (?1, ?2, 1)", params![track_id, 1 + rng.below(5) as i64]).unwrap();
            }
        }
    }
    conn.execute_batch(
        r#"INSERT INTO playlists (id, name, created_at) VALUES (1, 'Ordinary', 0), (6, 'Empty', 0);
           INSERT INTO playlists (id, name, created_at, rules) VALUES
             (2, 'Jazz', 0, '{"v":1,"match":"all","rules":[{"field":"genre","op":"is","value":"JAZZ"}]}'),
             (3, 'Jazz faves', 0, '{"v":1,"match":"all","rules":[{"field":"playlist","op":"in","value":2},{"field":"favourite","op":"yes"}]}'),
             (4, 'Loop A', 0, '{"v":1,"rules":[{"field":"playlist","op":"in","value":5}]}'),
             (5, 'Loop B', 0, '{"v":1,"rules":[{"field":"playlist","op":"not_in","value":4}]}');"#,
    )
    .unwrap();
    for (pos, t) in [5i64, 17, 40, 41, 99, 5, 200, 333].iter().enumerate() {
        conn.execute("INSERT INTO playlist_items VALUES (1, ?1, ?2)", params![pos as i64, t]).unwrap();
    }
    // The last album plays from the server (`Field::Remote`): its tracks have no file here.
    conn.execute("UPDATE tracks SET path = 'navidrome:song/' || id WHERE album_id = 40", []).unwrap();
    conn
}

fn sql_ids(conn: &Connection, s: &Smart) -> Vec<i64> {
    evaluate_ids(conn, s, None, ctx()).unwrap().0
}

fn rules(all: bool, conds: Vec<Cond>) -> Smart {
    Smart { root: Group { all, nodes: conds.into_iter().map(Node::Cond).collect() }, ..Smart::default() }
}

fn one(c: Cond) -> Smart {
    rules(true, vec![c])
}

fn check_both(conn: &Connection, lib: &Lib, s: &Smart, what: &str) -> Vec<i64> {
    let sql = sql_ids(conn, s);
    let mem = lib.eval(s, None, ctx());
    assert_eq!(sql, mem, "{what}: {}", s.to_json());
    // The full rows come back in the same order, and the count agrees.
    let full: Vec<i64> = evaluate(conn, s, None, ctx()).unwrap().tracks.iter().map(|t| t.id).collect();
    assert_eq!(full, sql, "{what}: rows");
    let (n, _) = count(conn, s, None, ctx()).unwrap();
    assert_eq!(n, sql.len(), "{what}: count");
    sql
}

/// Example values for a field with an operator: a few that match something, one that
/// matches nothing.
fn values(field: Field, op: Op) -> Vec<(Value, Option<(u32, Unit)>)> {
    let d = |s: &str| DateSpec::parse(s).unwrap();
    let text = |v: &[&str]| v.iter().map(|s| (Value::Text((*s).into()), None)).collect::<Vec<_>>();
    match (field.kind(), op) {
        (Kind::Text | Kind::Choice, _) => match field {
            Field::Title => text(&["love", "LOVE", "cafe", "ばべる", "full", "strasse", "50%", "_", "a\\b", "zzz"]),
            Field::Album => text(&["lemonade 1", "noir", "agaetis", "zzz"]),
            Field::Artist | Field::AlbumArtist => text(&["beyonce", "Sigur Ros", "よるしか", "abc band", "100%", "under_score", "back\\slash", "o'brien", "zzz"]),
            Field::Genre => text(&["jazz", "JAZZ", "electro", "hip-hop/rap", "ぼかろ", "zzz"]),
            Field::Label => text(&["blue note", "ecm", "[no label]", "warp", "zzz"]),
            Field::Format => text(&["flac", "MP3", "opus", "a", "zzz"]),
            Field::Path => text(&["m:\\music\\1\\", "M:/Music/12/", "flac", ".mp3", "zzz"]),
            _ => unreachable!(),
        },
        (Kind::Number, Op::Between) => match field {
            Field::Year => vec![(Value::Range(1990.0, 2010.0), None), (Value::Range(2025.0, 2025.0), None)],
            Field::Duration => vec![(Value::Range(60.0, 180.0), None), (Value::Range(0.0, 10.0), None)],
            Field::SampleRate => vec![(Value::Range(44_100.0, 48_000.0), None)],
            Field::PlaysIn => vec![(Value::Range(1.0, 3.0), Some((6, Unit::Months))), (Value::Range(0.0, 0.0), Some((1, Unit::Weeks)))],
            _ => vec![(Value::Range(1.0, 3.0), None), (Value::Range(0.0, 100_000.0), None)],
        },
        (Kind::Number, _) => match field {
            Field::Year => vec![(Value::Num(1999.0), None), (Value::Num(2004.0), None), (Value::Num(0.0), None)],
            Field::Duration => vec![(Value::Num(120.0), None), (Value::Num(300.0), None), (Value::Num(30.0), None)],
            Field::SampleRate => vec![(Value::Num(44_100.0), None), (Value::Num(96_000.0), None)],
            Field::BitDepth => vec![(Value::Num(16.0), None), (Value::Num(24.0), None)],
            Field::Bitrate => vec![(Value::Num(320.0), None), (Value::Num(900.0), None)],
            Field::Plays => vec![(Value::Num(0.0), None), (Value::Num(1.0), None), (Value::Num(3.0), None)],
            Field::PlaysIn => vec![(Value::Num(1.0), Some((30, Unit::Days))), (Value::Num(2.0), Some((1, Unit::Years))), (Value::Num(0.0), Some((3, Unit::Months)))],
            _ => vec![(Value::Num(1.0), None), (Value::Num(2.0), None), (Value::Num(9.0), None)],
        },
        (Kind::Date, Op::InLast | Op::NotInLast) => {
            vec![(Value::Ago(30, Unit::Days), None), (Value::Ago(2, Unit::Weeks), None), (Value::Ago(6, Unit::Months), None), (Value::Ago(1, Unit::Years), None)]
        }
        (Kind::Date, Op::Between) => vec![(Value::Dates(d("2024"), d("2024-12")), None), (Value::Dates(d("2025-01-15"), d("2025-06-15")), None)],
        (Kind::Date, _) => vec![(Value::Date(d("2024")), None), (Value::Date(d("2025-03")), None), (Value::Date(d("2025-06-14")), None)],
        (Kind::Bool, _) => vec![(Value::None, None)],
        (Kind::Playlist, _) => vec![(Value::Id(1), None), (Value::Id(2), None), (Value::Id(3), None), (Value::Id(6), None)],
    }
}

#[test]
fn every_field_with_every_operator_agrees_with_the_reference() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    let total = lib.tracks.len();
    let mut checked = 0;
    for field in Field::ALL {
        let mut matched_something = false;
        for &op in field.ops() {
            for (value, within) in values(field, op) {
                let c = Cond { field, op, value, within: within.or(if field == Field::PlaysIn { Some((30, Unit::Days)) } else { None }) };
                assert!(c.check().is_ok(), "{c:?}: {:?}", c.check());
                let ids = check_both(&conn, &lib, &one(c.clone()), &format!("{field:?} {op:?}"));
                matched_something |= !ids.is_empty() && ids.len() < total;
                checked += 1;
            }
        }
        assert!(matched_something, "{field:?}: some condition picks some tracks but not all");
    }
    assert!(checked > 200, "{checked}");
}

#[test]
fn text_matches_fold_case_width_kana_and_accents() {
    let conn = fixture();
    let titles = |s: &Smart| -> Vec<String> {
        let mut v: Vec<String> = evaluate(&conn, s, None, ctx()).unwrap().tracks.into_iter().map(|t| t.title).collect();
        v.sort();
        v.dedup();
        v
    };
    let t = |op, v: &str| one(Cond::new(Field::Title, op, Value::Text(v.into())));
    assert_eq!(titles(&t(Op::Contains, "cafe")), vec!["Café Noir"]);
    assert_eq!(titles(&t(Op::Is, "CAFÉ NOIR")), vec!["Café Noir"]);
    assert_eq!(titles(&t(Op::Is, "ばべる")), Vec::<String>::new(), "is: the whole title");
    assert_eq!(titles(&t(Op::Contains, "バベル")), vec!["いただきバベル"], "katakana finds hiragana");
    assert_eq!(titles(&t(Op::StartsWith, "full")), vec!["ＦＵＬＬ Width"], "full width");
    assert_eq!(titles(&t(Op::Contains, "strasse")), vec!["Straße"]);
    assert_eq!(titles(&t(Op::Is, "love")), vec!["LOVE"]);
    assert_eq!(titles(&t(Op::Contains, "love")), vec!["LOVE", "Love Song"]);
    assert_eq!(titles(&t(Op::StartsWith, "  love s ")), vec!["Love Song"], "trimmed, inner space kept");
    // LIKE's own characters are only themselves.
    assert_eq!(titles(&t(Op::Contains, "%")), vec!["50% Off"]);
    assert_eq!(titles(&t(Op::Contains, "_")), vec!["snake_case"]);
    assert_eq!(titles(&t(Op::Contains, "\\")), vec!["a\\b"]);
    assert_eq!(titles(&t(Op::Contains, "e_c")), vec!["snake_case"]);
    assert_eq!(titles(&t(Op::Contains, "e%c")), Vec::<String>::new());
    // Folder paths with either slash.
    let p = |v: &str| sql_ids(&conn, &one(Cond::new(Field::Path, Op::StartsWith, Value::Text(v.into()))));
    assert_eq!(p("M:/Music/12/"), p("m:\\MUSIC\\12\\"));
    assert_eq!(p("M:/Music/12/").len(), 9);
}

#[test]
fn several_artists_or_genres_match_if_any_does_and_is_not_means_none_does() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    for (field, value) in [(Field::Artist, "beyonce"), (Field::AlbumArtist, "Mara Lind"), (Field::Genre, "jazz")] {
        let is = sql_ids(&conn, &one(Cond::new(field, Op::Is, Value::Text(value.into()))));
        let is_not = sql_ids(&conn, &one(Cond::new(field, Op::IsNot, Value::Text(value.into()))));
        assert!(!is.is_empty() && !is_not.is_empty());
        // Together they are every track, apart they share none.
        let mut all = [is.clone(), is_not.clone()].concat();
        all.sort();
        assert_eq!(all, (1..=360).collect::<Vec<_>>(), "{field:?}");
        // A track credited with this artist and another is in "is" and not in "is not".
        let both = lib.tracks.iter().find(|t| {
            let values = match field {
                Field::Artist => &t.credited,
                Field::AlbumArtist => &t.album_artists,
                _ => &t.genres,
            };
            values.len() > 1 && values.iter().any(|v| crate::fold::fold(v) == crate::fold::fold(value)) && values.iter().any(|v| crate::fold::fold(v) != crate::fold::fold(value))
        });
        let both = both.unwrap_or_else(|| panic!("the fixture has a {field:?} with two values")).id;
        assert!(is.contains(&both) && !is_not.contains(&both), "{field:?}");
    }
    // A track with no genre is not jazz either.
    let untagged = lib.tracks.iter().find(|t| t.genres.is_empty()).unwrap().id;
    assert!(sql_ids(&conn, &one(Cond::new(Field::Genre, Op::IsNot, Value::Text("jazz".into())))).contains(&untagged));
    assert!(sql_ids(&conn, &one(Cond::new(Field::Genre, Op::NotContains, Value::Text("a".into())))).contains(&untagged));
    // No label is not "Blue Note"; nor does it contain anything.
    let no_label = lib.tracks.iter().find(|t| t.label.is_none()).unwrap().id;
    assert!(sql_ids(&conn, &one(Cond::new(Field::Label, Op::IsNot, Value::Text("Blue Note".into())))).contains(&no_label));
    assert!(!sql_ids(&conn, &one(Cond::new(Field::Label, Op::Contains, Value::Text("e".into())))).contains(&no_label));
}

/// A library of a few tracks with plays at known times, for relative dates by hand.
fn clock_library() -> Connection {
    let conn = crate::db::open_in_memory().unwrap();
    let c = ctx();
    conn.execute("INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (1, 'k', 'A', 'B', 'M:/', ?1, ?1)", [(c.now - 10 * 86_400) * 1000]).unwrap();
    conn.execute("INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (2, 'k2', 'Old', 'B', 'M:/o', ?1, ?1)", [(c.now - 400 * 86_400) * 1000]).unwrap();
    for (id, album) in [(1, 1), (2, 1), (3, 1), (4, 2), (5, 2)] {
        conn.execute(
            "INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES (?1, ?2, ?3, 'T', 'B', 100.0, 0, 0, 0)",
            params![id, format!("M:/{id}.flac"), album],
        )
        .unwrap();
    }
    // 1: played 1 day ago and 40 days ago; 2: 200 days ago; 3: 6 months ago less an hour
    // (still in the last 6 months); 4: 6 months ago and an hour (not); 5: never.
    let six_months = ago(c, 6, Unit::Months);
    for (t, at) in [(1, c.now - 86_400), (1, c.now - 40 * 86_400), (2, c.now - 200 * 86_400), (3, six_months + 3600), (4, six_months - 3600)] {
        conn.execute("INSERT INTO plays VALUES (?1, ?2)", params![t, at]).unwrap();
    }
    conn
}

/// Central European time in 2025: UTC+2 from 2025-03-30 01:00 UTC to 2025-10-26 01:00 UTC,
/// else UTC+1.
fn cet_2025(t: i64) -> i64 {
    let (spring, autumn) = (days_from_civil(2025, 3, 30) * 86_400 + 3600, days_from_civil(2025, 10, 26) * 86_400 + 3600);
    if (spring..autumn).contains(&t) { 7200 } else { 3600 }
}

/// Dates on the other side of a daylight-saving change from now: a winter day starts at its own
/// midnight (UTC+1) though now is summer time (UTC+2), and "the last 7 days" across the change
/// keeps the local time of day. SQL and the reference agree.
#[test]
fn date_rules_across_a_daylight_saving_change() {
    let conn = crate::db::open_in_memory().unwrap();
    // Now: 2025-04-02 12:00 summer time.
    let now = days_from_civil(2025, 4, 2) * 86_400 + 12 * 3600 - 7200;
    let ctx = Ctx::with_zone(now, Zone::Rules(cet_2025));
    // Album 1 added at 2025-01-09 23:30 local (winter: 22:30 UTC, the 10th at a fixed UTC+2); album 2 at
    // 2025-03-26 12:30 local (winter), 7 days back less half an hour; album 3 at 11:30 that
    // day, 7 days and half an hour back.
    let winter = |d: u32, h: i64, m: i64| (days_from_civil(2025, if d > 20 { 3 } else { 1 }, d) * 86_400 + h * 3600 + m * 60 - 3600) * 1000;
    for (id, at) in [(1, winter(9, 23, 30)), (2, winter(26, 12, 30)), (3, winter(26, 11, 30))] {
        conn.execute("INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (?1, ?2, 'A', 'B', ?2, ?3, ?3)", params![id, format!("M:/{id}"), at]).unwrap();
        conn.execute("INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES (?1, ?2, ?1, 'T', 'B', 100.0, 0, 0, 0)", params![id, format!("M:/{id}.flac")]).unwrap();
    }
    let lib = Lib::load(&conn);
    let both = |c: Cond| {
        let s = one(c);
        let sql = evaluate_ids(&conn, &s, None, ctx).unwrap().0;
        let mut mem = lib.eval(&s, None, ctx);
        mem.sort_unstable();
        let mut sorted = sql.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, mem, "SQL and the reference");
        sorted
    };
    let d = |s| DateSpec::parse(s).unwrap();
    assert_eq!(both(Cond::new(Field::Added, Op::Before, Value::Date(d("2025-01-10")))), vec![1], "added on the 9th, local winter time");
    assert_eq!(both(Cond::new(Field::Added, Op::Between, Value::Dates(d("2025-01-09"), d("2025-01-09")))), vec![1]);
    assert_eq!(both(Cond::new(Field::Added, Op::After, Value::Date(d("2025-01-09")))), vec![2, 3]);
    assert_eq!(both(Cond::new(Field::Added, Op::InLast, Value::Ago(7, Unit::Days))), vec![2], "7 days back to 12:00 local, across the change");
    assert_eq!(both(Cond::new(Field::Added, Op::InLast, Value::Ago(1, Unit::Weeks))), vec![2]);
}

#[test]
fn relative_dates_count_back_from_the_clock_given() {
    let conn = clock_library();
    let ids = |c: Cond| sql_ids(&conn, &one(c));
    let lp = |op, v| Cond::new(Field::LastPlayed, op, v);
    assert_eq!(ids(lp(Op::InLast, Value::Ago(7, Unit::Days))), vec![1]);
    assert_eq!(ids(lp(Op::InLast, Value::Ago(6, Unit::Months))), vec![1, 3]);
    assert_eq!(ids(lp(Op::NotInLast, Value::Ago(6, Unit::Months))), vec![2, 4, 5], "never played is not played lately either");
    assert_eq!(ids(lp(Op::InLast, Value::Ago(1, Unit::Years))), vec![1, 2, 3, 4]);
    assert_eq!(ids(Cond::new(Field::Added, Op::InLast, Value::Ago(2, Unit::Weeks))), vec![1, 2, 3]);
    assert_eq!(ids(Cond::new(Field::Added, Op::NotInLast, Value::Ago(1, Unit::Years))), vec![4, 5]);
    let plays_in = |n: f64, within| ids(Cond { within: Some(within), ..Cond::new(Field::PlaysIn, Op::AtLeast, Value::Num(n)) });
    assert_eq!(plays_in(2.0, (2, Unit::Months)), vec![1]);
    assert_eq!(plays_in(1.0, (1, Unit::Weeks)), vec![1]);
    assert_eq!(plays_in(1.0, (1, Unit::Years)), vec![1, 2, 3, 4]);
    // The same rules a day later: the play of a day ago is now two days old.
    let later = Ctx::with_zone(ctx().now + 86_400, ctx().zone);
    let s = one(lp(Op::InLast, Value::Ago(1, Unit::Days)));
    assert_eq!(evaluate_ids(&conn, &s, None, ctx()).unwrap().0, vec![1]);
    assert_eq!(evaluate_ids(&conn, &s, None, later).unwrap().0, Vec::<i64>::new());
    // Absolute dates in local time: added 10 days ago is 2025-06-05 (UTC+2).
    let d = |s| DateSpec::parse(s).unwrap();
    assert_eq!(ids(Cond::new(Field::Added, Op::After, Value::Date(d("2025-06-04")))), vec![1, 2, 3]);
    assert_eq!(ids(Cond::new(Field::Added, Op::After, Value::Date(d("2025-06-05")))), Vec::<i64>::new(), "after the day, not on it");
    assert_eq!(ids(Cond::new(Field::Added, Op::Before, Value::Date(d("2025-06")))), vec![4, 5]);
    assert_eq!(ids(Cond::new(Field::Added, Op::Between, Value::Dates(d("2025-06-05"), d("2025-06-05")))), vec![1, 2, 3], "between is inclusive");
    assert_eq!(ids(Cond::new(Field::Added, Op::Between, Value::Dates(d("2024"), d("2024")))), vec![4, 5]);
    assert_eq!(ids(Cond::new(Field::LastPlayed, Op::Before, Value::Date(d("2025")))), vec![2, 3, 4], "a track whose latest play is before");
    assert_eq!(ids(Cond::new(Field::Played, Op::No, Value::None)), vec![5]);
    assert_eq!(ids(Cond::new(Field::Plays, Op::Is, Value::Num(0.0))), vec![5]);
}

#[test]
fn limits_take_the_first_by_their_own_order_then_the_list_is_sorted() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    let mut s = Smart::default();
    // 25 most played, shown by title.
    s.limit = Some(Limiting::Known(Limit { n: 25, unit: LimitUnit::Tracks, by: Sort::new(SortKey::Plays) }));
    s.sort = Sorting::Known(Sort::new(SortKey::Title));
    let ids = check_both(&conn, &lib, &s, "25 most played");
    assert_eq!(ids.len(), 25);
    let most = lib.tracks.iter().map(|t| t.plays.len()).max().unwrap();
    let kept: Vec<usize> = ids.iter().map(|id| lib.tracks.iter().find(|t| t.id == *id).unwrap().plays.len()).collect();
    assert!(kept.contains(&most) && kept.iter().all(|&n| n >= 3), "{kept:?}");
    let titles: Vec<String> = evaluate(&conn, &s, None, ctx()).unwrap().tracks.into_iter().map(|t| t.title.to_lowercase()).collect();
    assert!(titles.windows(2).all(|w| w[0] <= w[1]), "shown by title");
    // An hour of tracks: whole tracks only, the total within the hour, the next one would pass it.
    s.limit = Some(Limiting::Known(Limit { n: 1, unit: LimitUnit::Hours, by: Sort::new(SortKey::Random) }));
    let ids = check_both(&conn, &lib, &s, "an hour at random");
    let total: f64 = ids.iter().map(|id| lib.tracks.iter().find(|t| t.id == *id).unwrap().duration).sum();
    assert!(total <= 3600.0 && total > 3000.0, "{total}");
    let (n, secs) = count(&conn, &s, None, ctx()).unwrap();
    assert_eq!((n, secs), (ids.len(), total));
    s.limit = Some(Limiting::Known(Limit { n: 90, unit: LimitUnit::Minutes, by: Sort { key: SortKey::Duration, desc: true } }));
    check_both(&conn, &lib, &s, "90 minutes, longest");
    // Limits combine with rules, and a limit larger than the match is all of it.
    let mut j = one(Cond::new(Field::Genre, Op::Is, Value::Text("jazz".into())));
    let all_jazz = sql_ids(&conn, &j).len();
    j.limit = Some(Limiting::Known(Limit { n: 10_000, unit: LimitUnit::Tracks, by: Sort::new(SortKey::Added) }));
    assert_eq!(check_both(&conn, &lib, &j, "jazz, a big limit").len(), all_jazz);
}

#[test]
fn every_order_both_ways_agrees_with_the_reference() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    for key in SortKey::CHOICES.into_iter().chain([SortKey::PlaysIn(1, Unit::Years)]) {
        for desc in [false, true] {
            let mut s = Smart::default();
            s.sort = Sorting::Known(Sort { key, desc });
            check_both(&conn, &lib, &s, &format!("{key:?} desc {desc}"));
            s.limit = Some(Limiting::Known(Limit { n: 40, unit: LimitUnit::Tracks, by: Sort { key, desc: !desc } }));
            check_both(&conn, &lib, &s, &format!("{key:?} limited"));
        }
    }
    // Undated albums come last whichever way round.
    let mut s = Smart::default();
    for desc in [false, true] {
        s.sort = Sorting::Known(Sort { key: SortKey::Year, desc });
        let ids = sql_ids(&conn, &s);
        let last = lib.tracks.iter().find(|t| t.id == *ids.last().unwrap()).unwrap();
        assert!(last.date.as_deref().is_none_or(|d| !d.starts_with(|c: char| c.is_ascii_digit())), "{desc}");
    }
}

#[test]
fn the_shuffle_is_stable_for_a_seed_and_new_for_another() {
    let conn = fixture();
    let mut s = Smart::default();
    s.sort = Sorting::Known(Sort::new(SortKey::Random));
    let a = sql_ids(&conn, &s);
    assert_eq!(a, sql_ids(&conn, &s), "the same seed, the same order");
    assert_eq!(a.len(), 360);
    let mut sorted = a.clone();
    sorted.sort();
    assert_ne!(a, sorted, "shuffled");
    s.seed = 2;
    let b = sql_ids(&conn, &s);
    assert_ne!(a, b, "a new seed, a new order");
    // A new track does not move the others: each keeps its place in the shuffle.
    conn.execute("INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES (999, 'M:/new.flac', 1, 'New', 'X', 10, 0, 0, 0)", []).unwrap();
    let c: Vec<i64> = sql_ids(&conn, &s).into_iter().filter(|&id| id != 999).collect();
    assert_eq!(c, b);
    // A random selection too: 20 at random, the same 20 until reshuffled.
    s.seed = 5;
    s.sort = Sorting::Known(Sort::new(SortKey::Title));
    s.limit = Some(Limiting::Known(Limit { n: 20, unit: LimitUnit::Tracks, by: Sort::new(SortKey::Random) }));
    let mut x = sql_ids(&conn, &s);
    x.sort();
    let mut y = sql_ids(&conn, &s);
    y.sort();
    assert_eq!(x, y);
    s.seed = 6;
    let mut z = sql_ids(&conn, &s);
    z.sort();
    assert_ne!(x, z);
    assert!(s.random());
}

#[test]
fn playlists_name_playlists_and_cycles_are_refused() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    // In an ordinary playlist (a track twice is there once).
    let mut ord = check_both(&conn, &lib, &one(Cond::new(Field::Playlist, Op::In, Value::Id(1))), "in Ordinary");
    ord.sort();
    assert_eq!(ord, vec![5, 17, 40, 41, 99, 200, 333]);
    // In a smart one, and in one that names another.
    let jazz = sql_ids(&conn, &one(Cond::new(Field::Genre, Op::Is, Value::Text("jazz".into()))));
    assert_eq!(check_both(&conn, &lib, &one(Cond::new(Field::Playlist, Op::In, Value::Id(2))), "in Jazz"), jazz);
    let faves = check_both(&conn, &lib, &one(Cond::new(Field::Playlist, Op::In, Value::Id(3))), "in Jazz faves");
    assert!(!faves.is_empty() && faves.len() < jazz.len() && faves.iter().all(|id| jazz.contains(id)));
    // A cycle: Loop A names Loop B which names Loop A. Evaluated as A, the rule that comes
    // back matches nothing, and it says so; nothing hangs or overflows.
    let a = Smart::from_json(&playlist_rules(&conn, 4).unwrap().unwrap().unwrap());
    let out = evaluate(&conn, &a, Some(4), ctx()).unwrap();
    assert!(out.tracks.is_empty());
    assert!(out.problems.iter().any(|p| p.contains("comes back")), "{:?}", out.problems);
    assert_eq!(lib.eval(&a, Some(4), ctx()), Vec::<i64>::new());
    // Saving: naming itself, or one that comes back to it, is refused; naming others is fine.
    let names = |id: i64| one(Cond::new(Field::Playlist, Op::In, Value::Id(id)));
    assert_eq!(would_cycle(&conn, Some(2), &names(2)), Some("Jazz".into()));
    assert_eq!(would_cycle(&conn, Some(2), &names(3)), Some("Jazz faves".into()), "Jazz faves names Jazz");
    assert_eq!(would_cycle(&conn, Some(3), &names(2)), None);
    assert_eq!(would_cycle(&conn, None, &names(3)), None, "a new playlist");
    assert_eq!(would_cycle(&conn, Some(6), &names(1)), None);
    // A playlist that is gone matches nothing (also "is not in" it), and says so.
    let gone = evaluate(&conn, &one(Cond::new(Field::Playlist, Op::NotIn, Value::Id(77))), None, ctx()).unwrap();
    assert!(gone.tracks.is_empty() && gone.problems.iter().any(|p| p.contains("no longer exists")));
    // Too deep: a chain of eleven smart playlists, each naming the next.
    for i in 0..11i64 {
        let next = if i == 10 { r#"{"rules":[]}"#.to_owned() } else { format!(r#"{{"rules":[{{"field":"playlist","op":"in","value":{}}}]}}"#, 101 + i) };
        conn.execute("INSERT INTO playlists (id, name, created_at, rules) VALUES (?1, ?2, 0, ?3)", params![100 + i, format!("Chain {i}"), next]).unwrap();
    }
    let deep = evaluate(&conn, &names(100), None, ctx()).unwrap();
    assert!(deep.tracks.is_empty() && deep.problems.iter().any(|p| p.contains("too deeply")), "{:?}", deep.problems);
    assert!(would_cycle(&conn, None, &names(100)).is_some());
    assert_eq!(evaluate(&conn, &names(105), None, ctx()).unwrap().tracks.len(), 360, "a shorter chain is fine");
}

/// Smart playlist `id` naming each of `names` ("is in").
fn smart_naming(conn: &Connection, id: i64, names: &[i64]) {
    let rules: Vec<String> = names.iter().map(|n| format!(r#"{{"field":"playlist","op":"in","value":{n}}}"#)).collect();
    let json = format!(r#"{{"rules":[{}]}}"#, rules.join(","));
    conn.execute("INSERT INTO playlists (id, name, created_at, rules) VALUES (?1, ?2, 0, ?3)", params![id, format!("Nest {id}"), json]).unwrap();
}

#[test]
fn nesting_depth_is_the_longest_path_and_matches_evaluation() {
    let conn = fixture();
    let names = |id: i64| one(Cond::new(Field::Playlist, Op::In, Value::Id(id)));
    let too_deep = |s: &Smart, me: Option<i64>| evaluate(&conn, s, me, ctx()).unwrap().problems.iter().any(|p| p.contains("too deeply"));

    // A diamond: 500 names 501 (short way) and then 510 (long way), and both come down to
    // 520, which has two below it. The short way is walked first.
    let diamond = |top: i64, long: i64| {
        smart_naming(&conn, top + 20, &[top + 30]);
        smart_naming(&conn, top + 30, &[top + 31]);
        smart_naming(&conn, top + 31, &[]);
        smart_naming(&conn, top + 1, &[top + 20]);
        for i in 0..long {
            let next = if i + 1 == long { top + 20 } else { top + 11 + i };
            smart_naming(&conn, top + 10 + i, &[next]);
        }
        smart_naming(&conn, top, &[top + 1, top + 10]);
    };
    // Long way: 500, five, then 520 and its two: nine, one more than evaluation enters.
    diamond(500, 5);
    assert!(too_deep(&names(500), None), "evaluation stops on the long way");
    assert_eq!(would_cycle(&conn, None, &names(500)), Some("Nest 500".into()), "so saving is refused");
    // One shorter: eight, which evaluation enters; but saved, the new playlist is evaluated
    // as itself, a ninth, so saving it is refused too.
    diamond(600, 4);
    assert!(!too_deep(&names(600), None));
    assert_eq!(would_cycle(&conn, None, &names(600)), Some("Nest 600".into()));
    assert_eq!(would_cycle(&conn, None, &names(601)), None, "the short way alone is fine");

    // A deep chain: eight smart playlists under a saved one (which counts itself when
    // evaluated) is one too many, both ways; a new one is refused alike, as it will count
    // itself once saved; seven under either is fine.
    for i in 0..8 {
        let below: Vec<i64> = if i == 7 { vec![] } else { vec![701 + i] };
        smart_naming(&conn, 700 + i, &below);
    }
    assert!(!too_deep(&names(700), None), "evaluated unsaved (the editor's preview)");
    assert!(would_cycle(&conn, None, &names(700)).is_some(), "saving it new");
    assert_eq!(would_cycle(&conn, None, &names(701)), None);
    smart_naming(&conn, 799, &[]);
    assert!(too_deep(&names(700), Some(799)));
    assert!(would_cycle(&conn, Some(799), &names(700)).is_some());
    assert_eq!(would_cycle(&conn, Some(799), &names(701)), None);

    // A real cycle, through a diamond's far side: 800 names 801 and 802, both name 803,
    // and 803 names 800.
    smart_naming(&conn, 803, &[800]);
    smart_naming(&conn, 801, &[803]);
    smart_naming(&conn, 802, &[803]);
    assert_eq!(would_cycle(&conn, Some(800), &one(Cond::new(Field::Playlist, Op::In, Value::Id(802)))), Some("Nest 802".into()));
    let both = Smart::from_json(r#"{"rules":[{"field":"playlist","op":"in","value":801},{"field":"playlist","op":"in","value":802}]}"#);
    assert_eq!(would_cycle(&conn, Some(800), &both), Some("Nest 801".into()));
    assert_eq!(would_cycle(&conn, Some(801), &names(802)), None, "803 names 800, not 801");
}

#[test]
fn a_playlist_named_many_times_over_is_compiled_once() {
    let conn = fixture();
    let jazz = sql_ids(&conn, &one(Cond::new(Field::Genre, Op::Is, Value::Text("jazz".into()))));
    let add = |id: i64, json: &str| {
        conn.execute("INSERT INTO playlists (id, name, created_at, rules) VALUES (?1, ?2, 0, ?3)", params![id, format!("Many {id}"), json]).unwrap();
    };
    let any_of = |names: &[i64]| {
        let rules: Vec<String> = names.iter().map(|n| format!(r#"{{"field":"playlist","op":"in","value":{n}}}"#)).collect();
        format!(r#"{{"match":"any","rules":[{}]}}"#, rules.join(","))
    };
    // A chain of six, each naming the next four times over: 4^6 paths to the last.
    add(1006, r#"{"rules":[{"field":"genre","op":"is","value":"JAZZ"}]}"#);
    for k in (1000..1006).rev() {
        add(k, &any_of(&[k + 1; 4]));
    }
    // A lattice: two playlists a layer, each naming both of the next layer's (2^6 paths).
    add(2060, r#"{"rules":[{"field":"genre","op":"is","value":"JAZZ"}]}"#);
    add(2061, r#"{"rules":[{"field":"favourite","op":"yes"}]}"#);
    for k in (0..6).rev() {
        let below = if k == 5 { [2060, 2061] } else { [2000 + (k + 1) * 10, 2001 + (k + 1) * 10] };
        add(2000 + k * 10, &any_of(&below));
        add(2001 + k * 10, &any_of(&below));
    }
    let lib = Lib::load(&conn);
    let names = |id: i64| one(Cond::new(Field::Playlist, Op::In, Value::Id(id)));
    assert_eq!(check_both(&conn, &lib, &names(1000), "chain"), jazz);
    let lattice = check_both(&conn, &lib, &names(2000), "lattice");
    let faves = sql_ids(&conn, &one(Cond::new(Field::Favourite, Op::Yes, Value::None)));
    assert_eq!(lattice.len(), lattice.iter().filter(|id| jazz.contains(id) || faves.contains(id)).count());
    // The SQL grows with the playlists named, not with the paths to them.
    let size = |id: i64| super::sql::sql_size(&conn, &names(id), None, ctx());
    assert!(size(1000) < 8_000, "{}", size(1000));
    assert!(size(2000) < 12_000, "{}", size(2000));
    // "Is not in" one of them, and the albums of one, read the same.
    check_both(&conn, &lib, &one(Cond::new(Field::Playlist, Op::NotIn, Value::Id(2000))), "not in the lattice");
    let (sel, params) = album_selection(&conn, &names(1000), ctx()).unwrap();
    let albums: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM albums a WHERE a.id IN ({sel})"), rusqlite::params_from_iter(params), |r| r.get(0)).unwrap();
    let want: i64 = conn.query_row("SELECT COUNT(DISTINCT album_id) FROM tracks t WHERE t.id IN (SELECT track_id FROM track_genres WHERE fold(name) = 'jazz')", [], |r| r.get(0)).unwrap();
    assert!(albums > 0);
    assert_eq!(albums, want);
}

#[test]
fn groups_nest_and_empty_rules_mean_every_track() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    assert_eq!(check_both(&conn, &lib, &Smart::default(), "no rules").len(), 360);
    let mut any = Smart::default();
    any.root.all = false;
    assert_eq!(sql_ids(&conn, &any).len(), 360, "no rules at the top, any or all: everything");
    // Jazz and (favourite or lossless).
    let s = Smart {
        root: Group {
            all: true,
            nodes: vec![
                Node::Cond(Cond::new(Field::Genre, Op::Contains, Value::Text("jazz".into()))),
                Node::Group(Group {
                    all: false,
                    nodes: vec![Node::Cond(Cond::new(Field::Favourite, Op::Yes, Value::None)), Node::Cond(Cond::new(Field::Lossless, Op::Yes, Value::None))],
                }),
            ],
        },
        ..Smart::default()
    };
    let ids = check_both(&conn, &lib, &s, "nested");
    for id in &ids {
        let t = lib.tracks.iter().find(|t| t.id == *id).unwrap();
        assert!(t.genres.iter().any(|g| g.to_lowercase().contains("jazz")) && (t.favourite || t.codec.as_deref().is_some_and(|c| ["FLAC", "ALAC", "WAV", "AIFF"].contains(&c))));
    }
    // An empty nested group: "all of nothing" holds, "any of nothing" does not; the editor
    // refuses to save either.
    let empty = |all| Smart { root: Group { all: true, nodes: vec![Node::Group(Group { all, nodes: vec![] })] }, ..Smart::default() };
    assert_eq!(sql_ids(&conn, &empty(true)).len(), 360);
    assert_eq!(sql_ids(&conn, &empty(false)).len(), 0);
    assert_eq!(empty(false).first_invalid().map(|e| e.1), Some("This group has no rules yet".into()));
}

#[test]
fn values_are_checked_and_explained() {
    let bad = |c: Cond| c.check().unwrap_err();
    assert_eq!(bad(Cond::new(Field::Genre, Op::Is, Value::Text("  ".into()))), "Choose a genre");
    assert_eq!(bad(Cond::new(Field::Title, Op::Contains, Value::Text("".into()))), "Type some text");
    assert_eq!(bad(Cond::new(Field::Year, Op::Between, Value::Range(2010.0, 1990.0))), "The first number should be the smaller one");
    assert_eq!(bad(Cond::new(Field::Duration, Op::AtLeast, Value::None)), "Type a length, like 3:30");
    assert_eq!(bad(Cond::new(Field::Added, Op::InLast, Value::Ago(0, Unit::Days))), "Type how many");
    let d = |s| DateSpec::parse(s).unwrap();
    assert_eq!(bad(Cond::new(Field::Added, Op::Between, Value::Dates(d("2025"), d("2024")))), "The first date should be the earlier one");
    assert_eq!(bad(Cond::new(Field::Playlist, Op::In, Value::Id(0))), "Choose a playlist");
    assert_eq!(bad(Cond { within: None, ..Cond::new(Field::PlaysIn, Op::AtLeast, Value::Num(1.0)) }), "Say how far back to count plays");
    assert!(bad(Cond::new(Field::Title, Op::AtLeast, Value::Num(1.0))).contains("cannot be used"));
    assert!(Cond::new(Field::Favourite, Op::Yes, Value::None).check().is_ok());
    // An incomplete rule from a damaged file matches nothing, never everything.
    let conn = fixture();
    let s = Smart::from_json(r#"{"rules":[{"field":"plays","op":"at_least"}]}"#);
    let out = evaluate(&conn, &s, None, ctx()).unwrap();
    assert!(out.tracks.is_empty() && !out.problems.is_empty());
    // A zero limit is refused before saving.
    let mut z = Smart::default();
    z.limit = Some(Limiting::Known(Limit { n: 0, unit: LimitUnit::Tracks, by: Sort::default() }));
    assert!(z.first_invalid().is_some());
}

#[test]
fn rule_sets_round_trip_through_json() {
    for (name, t) in templates() {
        let again = Smart::from_json(&t.to_json());
        assert_eq!(again, t, "{name}");
        assert!(t.first_invalid().is_none(), "{name}: {:?}", t.first_invalid());
        assert!(t.problems().is_empty(), "{name}");
    }
    let mut rng = Rng::new(99);
    let conn = fixture();
    let pools = Pools::load(&conn);
    for _ in 0..500 {
        let s = random_smart(&mut rng, &pools);
        let json = s.to_json();
        assert_eq!(Smart::from_json(&json), s, "{json}");
        assert_eq!(Smart::from_json(&json).to_json(), json, "stable text");
    }
    // The stored form is small and readable.
    let s = one(Cond::new(Field::Genre, Op::Is, Value::Text("Jazz".into())));
    assert_eq!(s.to_json(), r#"{"match":"all","rules":[{"field":"genre","op":"is","value":"Jazz"}],"seed":1,"sort":{"by":"artist_album","desc":false},"v":1}"#);
}

#[test]
fn what_a_newer_build_wrote_is_kept_and_never_widens_the_result() {
    let conn = fixture();
    let json = r#"{"v":3,"match":"all","rules":[
        {"field":"genre","op":"is","value":"jazz"},
        {"field":"mood","op":"is","value":"happy"},
        {"field":"plays","op":"is_roughly","value":4},
        {"group":"none_of","rules":[{"field":"title","op":"contains","value":"a"}]},
        {"group":"any","rules":[{"field":"bpm","op":"at_least","value":120},{"field":"favourite","op":"yes"}]}
      ],"sort":{"by":"energy","desc":true},"seed":4,"future":{"x":1}}"#;
    let s = Smart::from_json(json);
    assert_eq!(s.version, 3);
    let problems = s.problems();
    assert_eq!(problems.len(), 5, "{problems:?}");
    assert!(problems[0].contains("the field “mood”"), "{problems:?}");
    assert!(problems[1].contains("play count “is_roughly”"), "{problems:?}");
    assert!(problems.last().unwrap().contains("order"));
    // Unknown rules match nothing: "all of" with one is empty...
    let out = evaluate(&conn, &s, None, ctx()).unwrap();
    assert!(out.tracks.is_empty());
    // ...in "any of" the rest still count: the any-group here is its favourites.
    let any = Smart::from_json(r#"{"match":"any","rules":[{"field":"bpm","op":"at_least","value":120},{"field":"favourite","op":"yes"}]}"#);
    let mut favs = sql_ids(&conn, &one(Cond::new(Field::Favourite, Op::Yes, Value::None)));
    favs.sort();
    let mut got = sql_ids(&conn, &any);
    got.sort();
    assert_eq!(got, favs);
    // Written back, every unknown part is there as it was, and the version is not lowered.
    let back: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
    let orig: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(back["rules"], orig["rules"]);
    assert_eq!(back["sort"], orig["sort"]);
    assert_eq!(back["future"], orig["future"]);
    assert_eq!(back["v"], 3);
    // An unknown limit: nothing rather than too much. An unknown way of joining at the top: nothing.
    let lim = Smart::from_json(r#"{"rules":[],"limit":{"n":10,"unit":"gigabytes"}}"#);
    assert!(sql_ids(&conn, &lim).is_empty());
    assert!(lim.problems()[0].contains("limit"));
    let join = Smart::from_json(r#"{"match":"most","rules":[{"field":"favourite","op":"yes"}]}"#);
    assert!(sql_ids(&conn, &join).is_empty());
    // Not a rule set at all.
    for garbage in ["", "null", "[1,2]", "{\"rules\":7}", "not json", "{\"rules\":[{\"field\":\"title\",\"op\":\"is\",\"value\":5}]}"] {
        let g = Smart::from_json(garbage);
        assert!(sql_ids(&conn, &g).is_empty(), "{garbage:?}");
        assert!(!g.problems().is_empty(), "{garbage:?}");
    }
}

#[test]
fn hostile_text_is_only_ever_a_value() {
    let conn = fixture();
    let hostile = [
        "'); DROP TABLE tracks; --",
        "\" OR 1=1 --",
        "' OR '1'='1",
        "%",
        "_",
        "\\",
        "%' OR 1=1 --",
        "?",
        "?1",
        ":name",
        "$1",
        "\0",
        "x\u{0}y",
        "fold(t.title)",
        ") OR (1=1",
        "*/ SELECT * FROM plays /*",
        "\u{202e}gnp.exe",
        &"a".repeat(10_000),
    ];
    let before: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();
    for h in hostile {
        for field in Field::ALL.into_iter().filter(|f| matches!(f.kind(), Kind::Text | Kind::Choice)) {
            for &op in field.ops() {
                let c = Cond::new(field, op, Value::Text(h.to_owned()));
                let valid = c.check().is_ok();
                let s = one(c);
                let ids = evaluate_ids(&conn, &s, None, ctx()).unwrap_or_else(|e| panic!("{h:?} {field:?} {op:?}: {e}")).0;
                // Nothing matches what is not in any value; the negatives match everything. A
                // value that is nothing at all ("\0") is an incomplete rule: it matches nothing.
                if !matches!(h, "%" | "_" | "\\") {
                    let expect = if op.negative() && valid { 360 } else { 0 };
                    assert_eq!(ids.len(), expect, "{h:?} {field:?} {op:?}");
                }
            }
        }
        // As a JSON value too: a hostile name for a playlist reference, a hostile sort key.
        let j = serde_json::json!({ "rules": [{ "field": h, "op": h, "value": h }], "sort": { "by": h } }).to_string();
        assert!(sql_ids(&conn, &Smart::from_json(&j)).is_empty());
    }
    let after: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after, "the library is as it was");
    // The title that is itself an injection attempt is found as plain text.
    let s = one(Cond::new(Field::Title, Op::Contains, Value::Text("DROP TABLE".into())));
    assert!(!sql_ids(&conn, &s).is_empty());
}

#[test]
fn rule_sets_read_as_plain_words() {
    let names = |id: i64| if id == 1 { Some("Road trip".to_owned()) } else { None };
    let s = Smart {
        root: Group {
            all: true,
            nodes: vec![
                Node::Cond(Cond::new(Field::Genre, Op::Is, Value::Text("Jazz".into()))),
                Node::Cond(Cond::new(Field::Lossless, Op::Yes, Value::None)),
            ],
        },
        limit: Some(Limiting::Known(Limit { n: 50, unit: LimitUnit::Tracks, by: Sort::new(SortKey::Plays) })),
        ..Smart::default()
    };
    assert_eq!(summary(&s, &names), "Genre is Jazz \u{b7} lossless \u{b7} 50 tracks, most played");
    let words = |c: Cond| cond_words(&c, &names);
    assert_eq!(words(Cond::new(Field::Title, Op::Contains, Value::Text("love".into()))), "Title contains “love”");
    assert_eq!(words(Cond::new(Field::Favourite, Op::Yes, Value::None)), "favourite");
    assert_eq!(words(Cond::new(Field::Played, Op::No, Value::None)), "never played");
    assert_eq!(words(Cond::new(Field::LastPlayed, Op::NotInLast, Value::Ago(6, Unit::Months))), "not played in the last 6 months");
    assert_eq!(words(Cond::new(Field::Added, Op::InLast, Value::Ago(1, Unit::Weeks))), "date added in the last week");
    assert_eq!(words(Cond::new(Field::Year, Op::Between, Value::Range(1990.0, 1999.0))), "from 1990\u{2013}1999");
    assert_eq!(words(Cond::new(Field::Duration, Op::AtLeast, Value::Num(330.0))), "length is at least 5:30");
    assert_eq!(words(Cond::new(Field::SampleRate, Op::AtLeast, Value::Num(96_000.0))), "sample rate is at least 96 kHz");
    assert_eq!(words(Cond::new(Field::Playlist, Op::In, Value::Id(1))), "in “Road trip”");
    assert_eq!(words(Cond { within: Some((12, Unit::Months)), ..Cond::new(Field::PlaysIn, Op::AtLeast, Value::Num(3.0)) }), "3+ plays in the last 12 months");
    assert_eq!(words(Cond::new(Field::Lossless, Op::No, Value::None)), "lossy");
    let any = Smart { root: Group { all: false, nodes: vec![Node::Cond(Cond::new(Field::Favourite, Op::Yes, Value::None)), Node::Cond(Cond::new(Field::Played, Op::No, Value::None))] }, ..Smart::default() };
    assert_eq!(summary(&any, &names), "Any of: favourite, never played");
    let mut every = Smart::default();
    every.limit = Some(Limiting::Known(Limit { n: 2, unit: LimitUnit::Hours, by: Sort::new(SortKey::Random) }));
    assert_eq!(summary(&every, &names), "Every track \u{b7} 2 hours, random");
    let names_of: Vec<&str> = templates().iter().map(|t| t.0).collect();
    assert_eq!(names_of, ["Recently added and unplayed", "Favourites not heard in 6 months", "Most played this year", "Lossless only", "Two hours at random"]);
    assert_eq!(summary(&templates()[0].1, &names), "Date added in the last 2 months \u{b7} never played");
}

// ---- random rule sets, SQL against the reference ---------------------------------------------

/// Values the library holds, for random conditions that match something.
pub struct Pools {
    words: Vec<String>,
    artists: Vec<String>,
    genres: Vec<String>,
    labels: Vec<String>,
}

impl Pools {
    pub fn load(conn: &Connection) -> Pools {
        let col = |sql: &str| -> Vec<String> { conn.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
        let mut words: Vec<String> = col("SELECT title FROM tracks").iter().flat_map(|t| t.split_whitespace().map(str::to_owned).collect::<Vec<_>>()).collect();
        words.extend(["love", "e", "%", "_", "zz", "1"].map(str::to_owned));
        Pools {
            words,
            artists: col("SELECT name FROM artists"),
            genres: col("SELECT name FROM genres"),
            labels: col("SELECT DISTINCT label FROM albums WHERE label IS NOT NULL"),
        }
    }
}

fn random_cond(rng: &mut Rng, p: &Pools) -> Cond {
    let field = *rng.pick(&Field::ALL);
    let op = *rng.pick(field.ops());
    let units = Unit::ALL;
    let d = |rng: &mut Rng| {
        let y = 2023 + rng.below(3) as i32;
        match rng.below(3) {
            0 => DateSpec { y, m: None, d: None },
            1 => DateSpec { y, m: Some(1 + rng.below(12) as u8), d: None },
            _ => DateSpec { y, m: Some(1 + rng.below(12) as u8), d: Some(1 + rng.below(28) as u8) },
        }
    };
    let n = |rng: &mut Rng, field: Field| -> f64 {
        match field {
            Field::Year => *rng.pick(&[1987.0, 1999.0, 2004.0, 2010.0, 2024.0, 2025.0]),
            Field::Plays | Field::PlaysIn => rng.below(6) as f64,
            Field::Duration => (30 + rng.below(570)) as f64,
            Field::SampleRate => *rng.pick(&[44_100.0, 48_000.0, 96_000.0]),
            Field::BitDepth => *rng.pick(&[16.0, 24.0]),
            Field::Bitrate => *rng.pick(&[128.0, 320.0, 900.0]),
            _ => (1 + rng.below(10)) as f64,
        }
    };
    let value = match (field.kind(), op) {
        (Kind::Text | Kind::Choice, _) => {
            let pool = match field {
                Field::Artist | Field::AlbumArtist => &p.artists,
                Field::Genre => &p.genres,
                Field::Label => &p.labels,
                Field::Format => &["flac".to_owned(), "MP3".into(), "a".into(), "o".into()].to_vec(),
                Field::Path => &["m:\\music\\1".to_owned(), "M:/Music/2".into(), ".flac".into(), "love".into()].to_vec(),
                _ => &p.words,
            };
            let mut v = rng.pick(pool).clone();
            if matches!(op, Op::Contains | Op::NotContains | Op::StartsWith) && rng.chance(50) {
                let chars: Vec<char> = v.chars().collect();
                let len = 1 + rng.below(chars.len().max(1));
                v = chars.into_iter().take(len).collect();
            }
            if rng.chance(30) {
                v = v.to_uppercase();
            }
            if v.trim().is_empty() {
                v = "e".into();
            }
            Value::Text(v)
        }
        (Kind::Number, Op::Between) => {
            let (a, b) = (n(rng, field), n(rng, field));
            Value::Range(a.min(b), a.max(b))
        }
        (Kind::Number, _) => Value::Num(n(rng, field)),
        (Kind::Date, Op::InLast | Op::NotInLast) => Value::Ago(1 + rng.below(24) as u32, *rng.pick(&units)),
        (Kind::Date, Op::Between) => {
            let (a, b) = (d(rng), d(rng));
            if a.first_day() <= b.first_day() { Value::Dates(a, b) } else { Value::Dates(b, a) }
        }
        (Kind::Date, _) => Value::Date(d(rng)),
        (Kind::Bool, _) => Value::None,
        (Kind::Playlist, _) => Value::Id(*rng.pick(&[1, 2, 3, 4, 6, 77])),
    };
    let within = (field == Field::PlaysIn).then(|| (1 + rng.below(18) as u32, *rng.pick(&units)));
    Cond { field, op, value, within }
}

fn random_group(rng: &mut Rng, p: &Pools, depth: usize) -> Group {
    let n = rng.below(if depth == 0 { 5 } else { 4 }) + usize::from(depth > 0);
    let nodes = (0..n)
        .map(|_| {
            if depth < 2 && rng.chance(20) {
                Node::Group(random_group(rng, p, depth + 1))
            } else if rng.chance(3) {
                Node::Unknown { raw: serde_json::json!({ "field": "mood", "op": "is", "value": "x" }), why: "the field “mood”".into() }
            } else {
                Node::Cond(random_cond(rng, p))
            }
        })
        .collect();
    Group { all: rng.chance(60), nodes }
}

fn random_sort(rng: &mut Rng) -> Sort {
    let key = match *rng.pick(&SortKey::CHOICES) {
        SortKey::PlaysIn(..) => SortKey::PlaysIn(1 + rng.below(12) as u32, *rng.pick(&Unit::ALL)),
        k => k,
    };
    Sort { key, desc: rng.chance(50) }
}

pub fn random_smart(rng: &mut Rng, p: &Pools) -> Smart {
    Smart {
        version: FORMAT,
        root: random_group(rng, p, 0),
        sort: Sorting::Known(random_sort(rng)),
        limit: rng.chance(35).then(|| {
            let unit = *rng.pick(&LimitUnit::ALL);
            let n = match unit {
                LimitUnit::Tracks => 1 + rng.below(120) as u32,
                LimitUnit::Minutes => 1 + rng.below(300) as u32,
                LimitUnit::Hours => 1 + rng.below(6) as u32,
            };
            Limiting::Known(Limit { n, unit, by: random_sort(rng) })
        }),
        seed: rng.next() as i64,
        extra: Default::default(),
    }
}

#[test]
fn random_rule_sets_agree_with_the_reference() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    let pools = Pools::load(&conn);
    let mut rng = Rng::new(2025);
    let (mut empty, mut full, mut some) = (0, 0, 0);
    for i in 0..3000 {
        let s = random_smart(&mut rng, &pools);
        let ids = check_both(&conn, &lib, &s, &format!("random {i}"));
        match ids.len() {
            0 => empty += 1,
            360 => full += 1,
            _ => some += 1,
        }
    }
    // The generator exercises more than the trivial answers.
    assert!(some > 1500 && empty > 100 && full > 20, "some {some}, empty {empty}, full {full}");
}

#[test]
fn rules_about_ratings_from_an_older_build_are_dropped_only_where_that_never_widens() {
    let conn = fixture();
    let lib = Lib::load(&conn);
    let jazz = || Cond::new(Field::Genre, Op::Is, Value::Text("jazz".into()));
    let gone = |raw: serde_json::Value| Node::Unknown { raw, why: RATINGS_GONE.into() };
    let flagged = |s: &Smart| s.problems().iter().any(|p| p.contains("ratings"));
    // Among other rules of "all of" (also a group of nothing but ratings): dropped, the
    // others kept; an order and a limit's choice by rating become the default order.
    let json = r#"{"v":1,"match":"all","rules":[
        {"field":"genre","op":"is","value":"jazz"},
        {"field":"rating","op":"at_least","value":3},
        {"group":"any","rules":[{"field":"rating","op":"is","value":0}]}
      ],"sort":{"by":"rating","desc":true},"limit":{"n":20,"unit":"tracks","by":{"by":"rating","desc":true}},"seed":7}"#;
    let s = Smart::from_json(json);
    assert!(s.problems().is_empty(), "nothing unknown is left: {:?}", s.problems());
    let want = Smart {
        root: Group { all: true, nodes: vec![Node::Cond(jazz())] },
        sort: Sorting::Known(Sort::default()),
        limit: Some(Limiting::Known(Limit { n: 20, unit: LimitUnit::Tracks, by: Sort::default() })),
        seed: 7,
        ..Smart::default()
    };
    assert_eq!(s, want);

    // Only a rating (the old "Top rated"): not every track, but nothing, and it says why.
    let top = r#"{"v":1,"match":"all","rules":[{"field":"rating","op":"at_least","value":4}],"sort":{"by":"rating","desc":true}}"#;
    let only = Smart::from_json(top);
    assert_eq!(only.root.nodes, vec![gone(serde_json::json!({"field":"rating","op":"at_least","value":4}))]);
    assert_eq!(only.order(), Sort::default());
    assert!(flagged(&only), "{:?}", only.problems());
    assert!(check_both(&conn, &lib, &only, "ratings only").is_empty());
    let two = Smart::from_json(r#"{"rules":[{"field":"rating","op":"at_least","value":2},{"field":"rating","op":"at_most","value":4}]}"#);
    assert!(two.root.nodes.iter().all(|n| matches!(n, Node::Unknown { .. })) && sql_ids(&conn, &two).is_empty(), "two ratings, nothing else");

    // One choice of "any of": kept (it matches nothing), so the others decide, and it says so.
    let any = Smart::from_json(r#"{"match":"any","rules":[{"field":"genre","op":"is","value":"jazz"},{"field":"rating","op":"is","value":5}]}"#);
    assert_eq!(any.root.nodes.len(), 2);
    assert!(flagged(&any));
    assert_eq!(check_both(&conn, &lib, &any, "jazz or rated 5"), sql_ids(&conn, &one(jazz())));
    // In a group: in "jazz and (rated 5 or favourite)", jazz favourites, not jazz.
    let nested = Smart::from_json(r#"{"rules":[{"field":"genre","op":"is","value":"jazz"},{"group":"any","rules":[{"field":"rating","op":"is","value":5},{"field":"favourite","op":"yes"}]}]}"#);
    assert!(flagged(&nested));
    let faves = sql_ids(&conn, &rules(true, vec![jazz(), Cond::new(Field::Favourite, Op::Yes, Value::None)]));
    assert!(!faves.is_empty());
    assert_eq!(check_both(&conn, &lib, &nested, "jazz and (rated 5 or favourite)"), faves);
    // A group of nothing but ratings, alone: matches nothing (it used to leave every track).
    let group = Smart::from_json(r#"{"rules":[{"group":"any","rules":[{"field":"rating","op":"is","value":5}]}]}"#);
    assert!(flagged(&group) && check_both(&conn, &lib, &group, "group of a rating").is_empty());

    // Named by another playlist: "is in" it holds nothing, and "is not in" it is refused
    // rather than every track; both say so.
    conn.execute("INSERT INTO playlists (id, name, created_at, rules) VALUES (900, 'Top rated', 0, ?1)", [top]).unwrap();
    let lib = Lib::load(&conn);
    for op in [Op::In, Op::NotIn] {
        let s = one(Cond::new(Field::Playlist, op, Value::Id(900)));
        let out = evaluate(&conn, &s, None, ctx()).unwrap();
        assert!(out.tracks.is_empty() && out.problems.iter().any(|p| p.contains("Top rated")), "{op:?}: {:?}", out.problems);
        assert!(lib.eval(&s, None, ctx()).is_empty(), "{op:?}");
    }

    // Taking them out again changes nothing; nothing about ratings changes nothing.
    for raw in [json, top, r#"{"match":"any","rules":[{"field":"genre","op":"is","value":"jazz"},{"field":"rating","op":"is","value":5}]}"#] {
        let mut m = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw).unwrap();
        strip_ratings(&mut m);
        let once = m.clone();
        assert!(!strip_ratings(&mut m), "{raw}");
        assert_eq!(m, once);
    }
    let mut m = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(r#"{"rules":[{"field":"plays","op":"at_least","value":1}]}"#).unwrap();
    assert!(!strip_ratings(&mut m));
}
