//! Print every album artist's page as the library answers it, one line each, sorted:
//! name, own albums, appears-on albums, top tracks with their plays; then the favourites
//! and the total plays. `-- <library.db>` (opened read-write: it may migrate).
fn main() {
    let db = std::path::PathBuf::from(std::env::args().nth(1).expect("db"));
    let conn = library::db::open(&db).expect("open");
    let mut lines = Vec::new();
    for g in library::query::album_artists(&conn).unwrap() {
        let id = library::query::find_artist(&conn, &g.key).unwrap().unwrap().id;
        let (own, appears) = library::query::artist_albums(&conn, id).unwrap();
        let top = library::query::artist_top_tracks(&conn, id, 8).unwrap();
        let plays: Vec<String> = top.iter().map(|t| format!("{}:{}", t.id, library::query::play_count(&conn, t.id).unwrap())).collect();
        let mut own: Vec<String> = own.iter().map(|a| a.title.clone()).collect();
        own.sort();
        let mut appears: Vec<String> = appears.iter().map(|a| a.title.clone()).collect();
        appears.sort();
        lines.push(format!("{} | own {:?} | appears {:?} | top {:?}", g.name, own, appears, plays));
    }
    lines.sort();
    for l in &lines {
        println!("{l}");
    }
    let favs: i64 = conn.query_row("SELECT COUNT(*) FROM favourite_albums", [], |r| r.get(0)).unwrap();
    let fav_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM favourite_tracks", [], |r| r.get(0)).unwrap();
    let plays: i64 = conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap();
    println!("favourite albums {favs}, favourite tracks {fav_tracks}, plays {plays}");
}
