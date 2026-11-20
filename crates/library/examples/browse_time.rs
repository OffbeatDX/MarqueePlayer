//! Time the browse paths on a library database: `-- <library.db> [artist]`. Each runs 25
//! times; the median and the fastest are printed. A playlist of every track (up to 3,000)
//! is made in a copy of the database, never in the one given.
fn time<T>(name: &str, mut f: impl FnMut() -> T, n: impl Fn(&T) -> usize) {
    let first = f();
    let count = n(&first);
    let mut ms: Vec<f64> = (0..25)
        .map(|_| {
            let t = std::time::Instant::now();
            let _ = f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    println!("{name:>28}: median {:7.2} ms  min {:7.2} ms  ({count} rows)", ms[ms.len() / 2], ms[0]);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let src = std::path::PathBuf::from(args.next().expect("db path"));
    let artist = args.next().unwrap_or_else(|| "Mara Lind".into());
    let copy = std::env::temp_dir().join(format!("mp-browse-time-{}.db", std::process::id()));
    {
        let c = library::db::open(&src).expect("open");
        let _ = std::fs::remove_file(&copy);
        c.execute_batch(&format!("VACUUM INTO '{}'", copy.display())).unwrap();
    }
    let conn = library::db::open(&copy).expect("open copy");
    use library::query::{self, AlbumSort};
    time("albums by artist", || query::all_albums(&conn, AlbumSort::Artist, false).unwrap(), Vec::len);
    time("albums by title", || query::all_albums(&conn, AlbumSort::Title, false).unwrap(), Vec::len);
    time("albums by most played", || query::all_albums(&conn, AlbumSort::MostPlayed, true).unwrap(), Vec::len);
    time("artist index", || query::album_artists(&conn).unwrap(), Vec::len);
    time("genre index", || query::album_groups(&conn, false).unwrap(), Vec::len);
    time("label index", || query::album_groups(&conn, true).unwrap(), Vec::len);
    time("genre shelf (Folk)", || query::shelf(&conn, &library::ShelfQuery::Genre { name: "Folk".into() }, 1000).unwrap(), Vec::len);
    time("shelf editor genres", || query::genres(&conn).unwrap(), Vec::len);
    let id = query::find_artist(&conn, &artist).unwrap().map_or(-1, |a| a.id);
    time(&format!("artist page ({artist})"), || query::artist_albums(&conn, id).unwrap(), |(a, b)| a.len() + b.len());
    time("artist top tracks", || query::artist_top_tracks(&conn, id, 8).unwrap(), Vec::len);
    let ids: Vec<i64> = conn.prepare("SELECT id FROM tracks ORDER BY id LIMIT 3000").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let pl = query::create_playlist_with(&conn, "Everything", &ids).unwrap();
    time("playlist of 3000", || query::playlist_tracks(&conn, pl).unwrap(), Vec::len);
    for q in ["mara", "lind", "harbour", "beyonce", "kameria", "night"] {
        time(&format!("search {q:?}"), || query::search(&conn, q, 24).unwrap(), |r| r.albums.len() + r.artists.len() + r.tracks.len());
    }
    drop(conn);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", copy.display()));
    }
}
