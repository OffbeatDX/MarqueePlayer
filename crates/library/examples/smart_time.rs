//! Time smart playlists on a library database (read-only use): `-- <library.db>`. Every
//! smart playlist stored in it, then a set of heavier rule sets (text matches over every
//! title, several artists and genres, play counts in a period, nested playlists, a wide "any
//! of"), each evaluated as the page fills it (every row, in order) and as the Playlists page
//! counts it. Each runs 15 times; the first (cold) is reported apart, then the median and
//! the slowest of the rest. `smart_fixture` makes a database to time on.
use library::smart::{Ctx, Smart, count, evaluate};

const EXTRA: &[(&str, &str)] = &[
    ("title contains “love”", r#"{"rules":[{"field":"title","op":"contains","value":"love"}],"sort":{"by":"title","desc":false}}"#),
    ("artist is Beyonce (folded)", r#"{"rules":[{"field":"artist","op":"is","value":"beyonce"}]}"#),
    ("genre is not jazz, by year", r#"{"rules":[{"field":"genre","op":"is_not","value":"jazz"}],"sort":{"by":"year","desc":true}}"#),
    ("album artist starts with “m”", r#"{"rules":[{"field":"album_artist","op":"starts_with","value":"m"}]}"#),
    ("path contains “\\0”", r#"{"rules":[{"field":"path","op":"contains","value":"\\0"}]}"#),
    ("2+ plays in 30 days, by them", r#"{"rules":[{"field":"plays_in","op":"at_least","value":2,"within":{"n":30,"unit":"days"}}],"sort":{"by":"plays_in","desc":true,"within":{"n":30,"unit":"days"}}}"#),
    ("not played in 6 months, by last played", r#"{"rules":[{"field":"last_played","op":"not_in_last","value":{"n":6,"unit":"months"}}],"sort":{"by":"last_played","desc":false}}"#),
    ("every track, by artist and album", r#"{"rules":[]}"#),
    ("every track, by play count", r#"{"rules":[],"sort":{"by":"plays","desc":true}}"#),
    ("100 most played, by title", r#"{"rules":[],"sort":{"by":"title","desc":false},"limit":{"n":100,"unit":"tracks","by":{"by":"plays","desc":true}}}"#),
    ("10 hours by length", r#"{"rules":[],"limit":{"n":10,"unit":"hours","by":{"by":"duration","desc":true}}}"#),
    ("in the 3,000-track playlist, played", r#"{"rules":[{"field":"playlist","op":"in","value":3},{"field":"plays","op":"at_least","value":1}]}"#),
    (
        "any of ten",
        r#"{"match":"any","rules":[{"field":"title","op":"contains","value":"night"},{"field":"artist","op":"contains","value":"davis"},{"field":"genre","op":"is","value":"ambient"},{"field":"label","op":"is","value":"warp"},{"field":"year","op":"between","value":[1990,1995]},{"field":"lossless","op":"yes"},{"field":"favourite","op":"yes"},{"field":"plays","op":"at_least","value":20},{"field":"added","op":"in_last","value":{"n":2,"unit":"weeks"}},{"field":"bit_depth","op":"at_least","value":24}],"sort":{"by":"random"}}"#,
    ),
];

fn time(conn: &rusqlite::Connection, name: &str, smart: &Smart, me: Option<i64>) {
    let ctx = Ctx::now();
    let run = |f: &mut dyn FnMut()| -> (f64, f64, f64) {
        let t = std::time::Instant::now();
        f();
        let first = t.elapsed().as_secs_f64() * 1000.0;
        let mut ms: Vec<f64> = (0..14)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        (first, ms[ms.len() / 2], ms[ms.len() - 1])
    };
    let mut n = 0;
    let (f1, m1, x1) = run(&mut || n = evaluate(conn, smart, me, ctx).unwrap().tracks.len());
    let (_, m2, x2) = run(&mut || {
        count(conn, smart, me, ctx).unwrap();
    });
    println!("{name:>42}: {n:>6} tracks  rows median {m1:6.1} ms  max {x1:6.1}  (first {f1:6.1})  count median {m2:6.1} ms  max {x2:6.1}");
}

fn main() {
    let db = std::path::PathBuf::from(std::env::args().nth(1).expect("db path"));
    let conn = library::db::open(&db).expect("open");
    let (albums, tracks) = library::query::counts(&conn).unwrap();
    let plays: i64 = conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap();
    println!("{albums} albums, {tracks} tracks, {plays} plays");
    for p in library::query::playlists(&conn).unwrap() {
        if let Some(rules) = &p.rules {
            time(&conn, &p.name, &Smart::from_json(rules), Some(p.id));
        }
    }
    for (name, json) in EXTRA {
        time(&conn, name, &Smart::from_json(json), None);
    }
}
