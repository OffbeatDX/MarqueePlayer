//! Time a search on a library database (read-only use): `-- <library.db> <query>...`
//! Each query runs 25 times (the first, cold, is reported apart); the median and the
//! fastest of the rest are printed. `search_fixture` makes a database to time on.
fn main() {
    let mut args = std::env::args().skip(1);
    let db = std::path::PathBuf::from(args.next().expect("db path"));
    let conn = library::db::open(&db).expect("open");
    for q in args {
        let t = std::time::Instant::now();
        let r = library::query::search(&conn, &q, 24).unwrap();
        let cold = t.elapsed().as_secs_f64() * 1000.0;
        let mut ms: Vec<f64> = (0..24)
            .map(|_| {
                let t = std::time::Instant::now();
                let _ = library::query::search(&conn, &q, 24).unwrap();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        println!(
            "{q:>12}: median {:6.2} ms  min {:6.2} ms  (first {:6.2} ms)  albums {} artists {} tracks {}",
            ms[ms.len() / 2],
            ms[0],
            cold,
            r.albums.len(),
            r.artists.len(),
            r.tracks.len()
        );
    }
}
