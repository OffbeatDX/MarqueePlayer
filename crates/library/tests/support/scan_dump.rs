//! A library as its user knows it, as text: everything a page can show of its tracks, albums,
//! artists and genres, and every piece of the user's own data (plays, favourites and their
//! times, playlists, the queue), keyed by what the user knows a thing by (a track by
//! its file, an album by its identity), never by row numbers, which may be handed out in
//! another order. Two libraries that dump alike are the same to the user.
//!
//! Shared by the scanner's batch tests (`src/scan.rs`) and `tests/progressive.rs`.

use rusqlite::Connection;
use rusqlite::types::Value;

/// `since_ms`: when the scan being judged began. A date stamped by it ("now") differs from one
/// run to the next and is shown as "new"; every other date (one the library had, one taken over
/// from another album, one taken from the files) is shown as it is. `ids_up_to`: tracks that were
/// in the library before (ids up to this) must keep their ids, which are shown.
///
/// "Now" reaches back `CLOCK_SLACK_MS` before `since_ms`: the wall clock a scan stamps its dates
/// by can be stepped back while it runs (a WSL guest's clock is pulled to the NTP server's time
/// and to its Windows host's in turn, half a second apart, every 32 seconds), and a date stamped
/// just after such a step lies before `since_ms`. The dates a library had in these tests are
/// decades older, and dates taken from the files are shown whatever they are.
pub fn dump(conn: &Connection, since_ms: i64, ids_up_to: i64) -> Vec<(&'static str, String)> {
    const CLOCK_SLACK_MS: i64 = 60_000;
    let since_ms = since_ms - CLOCK_SLACK_MS;
    let q = |what: &'static str, sql: &str| -> (&'static str, String) {
        let mut st = conn.prepare(sql).unwrap_or_else(|e| panic!("{what}: {e}"));
        let n = st.column_count();
        let rows: Vec<String> = st
            .query_map(rusqlite::params![since_ms, ids_up_to], |r| {
                Ok((0..n)
                    .map(|i| match r.get::<_, Value>(i).unwrap() {
                        Value::Null => "-".to_owned(),
                        Value::Integer(v) => v.to_string(),
                        Value::Real(v) => format!("{v:.3}"),
                        Value::Text(v) => v,
                        Value::Blob(v) => format!("<{} bytes>", v.len()),
                    })
                    .collect::<Vec<_>>()
                    .join(" | "))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        (what, rows.join("\n"))
    };
    vec![
        q(
            "tracks",
            "SELECT t.path, t.title, t.artist, t.disc_no, t.track_no, t.duration, t.genre, t.mtime, t.size, t.codec, t.sample_rate,
                    t.bit_depth, t.bitrate, t.channels, t.tags_read, t.from_name, a.key,
                    CASE WHEN t.added_at * 1000 < ?1 THEN t.added_at ELSE 'new' END
             FROM tracks t JOIN albums a ON a.id = t.album_id WHERE ?2 = ?2 ORDER BY t.path",
        ),
        q("track ids kept", "SELECT path, id FROM tracks WHERE id <= ?2 AND ?1 = ?1 ORDER BY path"),
        q(
            "track tags",
            "SELECT t.path, g.group_folder, g.album_title, g.album_artist, g.album_artist_mbids, g.album_artist_sort, g.artist_sort, g.date,
                    g.original_date, g.release_date, g.label, g.release_type, g.mb_album_id, g.compilation
             FROM track_tags g JOIN tracks t ON t.id = g.track_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY t.path",
        ),
        q(
            "track artists",
            "SELECT t.path, x.position, x.name, x.norm, x.mbid, x.featured, ar.key FROM track_artists x
             JOIN tracks t ON t.id = x.track_id JOIN artists ar ON ar.id = x.artist_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY t.path, x.position",
        ),
        q(
            "track genres",
            "SELECT t.path, x.position, g.key, x.name FROM track_genres x JOIN tracks t ON t.id = x.track_id JOIN genres g ON g.id = x.genre_id
             WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY t.path, x.position",
        ),
        q(
            "albums",
            "SELECT key, title, album_artist, album_artist_sort, date, original_date, release_date, genre, label, label_key, release_type,
                    folder, art, art_checked, compilation, sort_artist, added_source,
                    CASE WHEN first_seen < ?1 OR added_source IS NOT NULL THEN added_at ELSE 'new' END,
                    CASE WHEN first_seen < ?1 THEN first_seen ELSE 'new' END
             FROM albums WHERE ?2 = ?2 ORDER BY key",
        ),
        q(
            "album artists",
            "SELECT a.key, x.position, x.name, x.norm, x.mbid, ar.key FROM album_artists x JOIN albums a ON a.id = x.album_id
             JOIN artists ar ON ar.id = x.artist_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY a.key, x.position",
        ),
        q(
            "album genres",
            "SELECT a.key, x.position, g.key FROM album_genres x JOIN albums a ON a.id = x.album_id JOIN genres g ON g.id = x.genre_id
             WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY a.key, x.position",
        ),
        q("artists", "SELECT key, norm, name, sort_name FROM artists WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY key"),
        q("genres", "SELECT key, name FROM genres WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY key"),
        q("plays", "SELECT t.path, p.played_at FROM plays p JOIN tracks t ON t.id = p.track_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY 1, 2"),
        q(
            "favourite albums",
            "SELECT a.key, f.added_at FROM favourite_albums f JOIN albums a ON a.id = f.album_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY 1",
        ),
        q(
            "favourite tracks",
            "SELECT t.path, f.added_at FROM favourite_tracks f JOIN tracks t ON t.id = f.track_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY 1",
        ),
        q(
            "playlists",
            "SELECT p.name, p.rules, i.position, t.path FROM playlists p LEFT JOIN playlist_items i ON i.playlist_id = p.id
             LEFT JOIN tracks t ON t.id = i.track_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY 1, 3",
        ),
        q("queue", "SELECT q.position, t.path, q.original FROM queue_items q JOIN tracks t ON t.id = q.track_id WHERE ?1 = ?1 AND ?2 = ?2 ORDER BY 1"),
        q("orphans", "SELECT 'empty album', key FROM albums WHERE id NOT IN (SELECT album_id FROM tracks) AND ?1 = ?1 AND ?2 = ?2"),
    ]
}

/// Panics, naming the part and the first lines that differ, unless the dumps are the same.
pub fn assert_same(want: &[(&str, String)], got: &[(&str, String)], case: &str) {
    for ((what, a), (_, b)) in want.iter().zip(got) {
        if a != b {
            let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
            let at = la.iter().zip(&lb).position(|(x, y)| x != y).unwrap_or(la.len().min(lb.len()));
            panic!(
                "{case}: {what} differ ({} rows vs {}), first at row {at}:\n  want: {}\n  got:  {}",
                la.len(),
                lb.len(),
                la.get(at).unwrap_or(&"(none)"),
                lb.get(at).unwrap_or(&"(none)")
            );
        }
    }
}
