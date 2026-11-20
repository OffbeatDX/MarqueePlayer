//! Favourites, ratings and plays of the tracks a server has, kept in step with the server
//! (docs/features/navidrome.md). The database side and the rules only; the app asks the server and
//! sends what is decided here (no network code here).
//!
//! **What is compared.** Per server song (`remote_marks`, keyed by its id, so it follows the
//! song through a merge with a file and back): the favourite and the rating last agreed with
//! the server. Each sync that read the server's own values (`Seen`) compares three things: the
//! local value now, the server's now and the agreed one. Only the server's changed: the
//! server's is taken. Only the local one changed: it is sent. Both changed (to different
//! values): for a rating the local one wins; for a favourite the newer change wins when both
//! times are known (the server's star time, the app's own change time), else the local one. A
//! change made in the app since the agreement (`star_local`, `rating_local`) is a local change
//! even when it came back to the agreed value.
//!
//! **Never agreed** (the first sync after this came, or a song new to the library): favourites
//! are the union (starred on either side: starred on both), a local rating other than none is
//! kept (and sent), else the server's is taken. A value the user changed in the app before any
//! agreement wins as a local change does.
//!
//! **The scale** is the same on both sides: 1 to 5 whole stars, 0 for none (`setRating` 0 removes
//! a rating). Nothing is rounded, so a value sent and read back is the value that was sent, and
//! a local value is written only when it is to change.
//!
//! **Plays** of tracks the server has are queued (`remote_plays`, written with the play) until
//! the server took them: offline, they wait for the next sync.
//!
//! What is pending needs no queue of its own for favourites and ratings: a local value that
//! differs from the agreed one is what is still to send, so a change made offline goes at the
//! next sync, and one made while the app was closed too.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::now_ms;

/// What was last agreed with the server of one song, and when the app last changed it itself.
/// Times are Unix milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Agreed {
    /// None: never agreed.
    pub starred: Option<bool>,
    /// The server's star time at the agreement, when it said one.
    pub starred_at: Option<i64>,
    pub star_agreed: Option<i64>,
    pub star_local: Option<i64>,
    pub rating: Option<u8>,
    pub rating_agreed: Option<i64>,
    pub rating_local: Option<i64>,
}

/// What the server says of a song now, when it was read (None: not known this time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// Some(Some(time)): starred then (Unix ms; 0 when it gave no time); Some(None): not starred.
    pub starred: Option<Option<i64>>,
    pub rating: Option<u8>,
}

/// What to do with one value of a song: write it locally, send it to the server, record it as
/// agreed. A value sent is agreed once the server took it (`push` and `agree` are never both
/// set). `at` is the server's star time to record with an agreement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plan<T> {
    pub set_local: Option<T>,
    pub push: Option<T>,
    pub agree: Option<T>,
    pub at: Option<i64>,
}

impl<T: Copy> Plan<T> {
    fn push(v: T) -> Self {
        Plan { set_local: None, push: Some(v), agree: None, at: None }
    }
    fn agree(v: T, at: Option<i64>) -> Self {
        Plan { set_local: None, push: None, agree: Some(v), at }
    }
    fn take(v: T, at: Option<i64>) -> Self {
        Plan { set_local: Some(v), push: None, agree: Some(v), at }
    }
    fn none() -> Self {
        Plan { set_local: None, push: None, agree: None, at: None }
    }
}

/// A change made in the app after the agreement.
fn touched_since(local: Option<i64>, agreed: Option<i64>) -> bool {
    match (local, agreed) {
        (Some(l), Some(a)) => l > a,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The favourite. `local`: the local favourite's time (Unix ms) when it is one.
pub fn decide_star(local: Option<i64>, m: &Agreed, seen: Option<Option<i64>>) -> Plan<bool> {
    let l = local.is_some();
    let touched = touched_since(m.star_local, m.star_agreed);
    match (m.starred, seen) {
        (None, None) => {
            if l || m.star_local.is_some() {
                Plan::push(l)
            } else {
                Plan::none()
            }
        }
        (None, Some(s)) => {
            if m.star_local.is_some() {
                if l == s.is_some() { Plan::agree(l, s) } else { Plan::push(l) }
            } else if l || s.is_some() {
                // The union: starred on either side, starred on both.
                match (l, s) {
                    (true, None) => Plan::push(true),
                    (false, Some(at)) => Plan::take(true, Some(at)),
                    (_, s) => Plan::agree(true, s),
                }
            } else {
                Plan::agree(false, None)
            }
        }
        (Some(a), None) => {
            if l != a || touched {
                Plan::push(l)
            } else {
                Plan::none()
            }
        }
        (Some(a), Some(s)) => {
            let sv = s.is_some();
            let server_changed = sv != a || (sv && matches!((m.starred_at, s), (Some(x), Some(y)) if x != y));
            let local_changed = l != a || touched;
            match (server_changed, local_changed) {
                (false, false) if sv && m.starred_at.is_none() => Plan::agree(a, s),
                (false, false) => Plan::none(),
                _ if l == sv => {
                    if sv == a && m.starred_at == s && !touched { Plan::none() } else { Plan::agree(l, s) }
                }
                (true, false) => Plan::take(sv, s),
                (false, true) => Plan::push(l),
                (true, true) => {
                    // When each changed: the server's star time (none for an unstar), the app's
                    // own change (or the local favourite's time).
                    let local_time = m.star_local.or(local);
                    let server_newer = matches!((s, local_time), (Some(st), Some(lt)) if st > lt);
                    if server_newer { Plan::take(sv, s) } else { Plan::push(l) }
                }
            }
        }
    }
}

/// The rating (0: none).
pub fn decide_rating(local: u8, m: &Agreed, seen: Option<u8>) -> Plan<u8> {
    let touched = touched_since(m.rating_local, m.rating_agreed);
    match (m.rating, seen) {
        (None, None) => {
            if local != 0 || m.rating_local.is_some() {
                Plan::push(local)
            } else {
                Plan::none()
            }
        }
        (None, Some(s)) => {
            if local != 0 || m.rating_local.is_some() {
                if local == s { Plan::agree(local, None) } else { Plan::push(local) }
            } else if s != 0 {
                Plan::take(s, None)
            } else {
                Plan::agree(0, None)
            }
        }
        (Some(a), None) => {
            if local != a || touched {
                Plan::push(local)
            } else {
                Plan::none()
            }
        }
        (Some(a), Some(s)) => {
            let local_changed = local != a || touched;
            if local == s {
                if s == a && !touched { Plan::none() } else { Plan::agree(local, None) }
            } else if local_changed {
                // Only local changed, or both: the local rating wins.
                Plan::push(local)
            } else {
                Plan::take(s, None)
            }
        }
    }
}

// ---- the database -----------------------------------------------------------------

/// A track the server has, as compared: its song, its local favourite and rating, what was
/// agreed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub song_id: String,
    pub track_id: i64,
    /// The local favourite's time (Unix ms) when it is one.
    pub starred: Option<i64>,
    pub rating: u8,
    pub agreed: Agreed,
}

const ROWS: &str = "SELECT t.remote_id, t.id, f.added_at, r.rating,
        m.starred, m.starred_at, m.star_agreed, m.star_local, m.rating, m.rating_agreed, m.rating_local
     FROM tracks t
     LEFT JOIN favourite_tracks f ON f.track_id = t.id
     LEFT JOIN track_ratings r ON r.track_id = t.id
     LEFT JOIN remote_marks m ON m.song_id = t.remote_id
     WHERE t.remote_id IS NOT NULL";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        song_id: r.get(0)?,
        track_id: r.get(1)?,
        starred: r.get::<_, Option<i64>>(2)?.map(|s| s * 1000),
        rating: r.get::<_, Option<i64>>(3)?.unwrap_or(0).clamp(0, 5) as u8,
        agreed: Agreed {
            starred: r.get::<_, Option<bool>>(4)?,
            starred_at: r.get(5)?,
            star_agreed: r.get(6)?,
            star_local: r.get(7)?,
            rating: r.get::<_, Option<i64>>(8)?.map(|v| v.clamp(0, 5) as u8),
            rating_agreed: r.get(9)?,
            rating_local: r.get(10)?,
        },
    })
}

/// Every track the server has, ordered by song id.
pub fn rows(conn: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut st = conn.prepare(&format!("{ROWS} ORDER BY t.remote_id"))?;
    st.query_map([], row)?.collect()
}

/// One track, when the server has it.
pub fn row_of(conn: &Connection, track_id: i64) -> rusqlite::Result<Option<Row>> {
    conn.prepare_cached(&format!("{ROWS} AND t.id = ?1"))?.query_row([track_id], row).optional()
}

/// Which value the app changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Favourite,
    Rating,
}

/// The app changed a track's favourite or rating (the user did): noted, with the time, for a
/// track the server has. Its song, when it has one.
pub fn touched(conn: &Connection, track_id: i64, what: Mark) -> rusqlite::Result<Option<String>> {
    let Some(song): Option<String> = conn.prepare_cached("SELECT remote_id FROM tracks WHERE id = ?1")?.query_row([track_id], |r| r.get(0)).optional()?.flatten() else {
        return Ok(None);
    };
    let col = match what {
        Mark::Favourite => "star_local",
        Mark::Rating => "rating_local",
    };
    conn.prepare_cached(&format!("INSERT INTO remote_marks (song_id, {col}) VALUES (?1, ?2) ON CONFLICT(song_id) DO UPDATE SET {col} = excluded.{col}"))?
        .execute(params![song, now_ms()])?;
    Ok(Some(song))
}

/// The local favourite as the server has it (a server's change taken): not a change of the
/// app's own. `at`: the server's star time (Unix ms; 0 or None: now).
pub fn set_local_star(conn: &Connection, track_id: i64, starred: bool, at: Option<i64>) -> rusqlite::Result<()> {
    if starred {
        let secs = at.filter(|t| *t > 0).map_or_else(crate::db::now, |t| t / 1000);
        conn.prepare_cached("INSERT INTO favourite_tracks (track_id, added_at) VALUES (?1, ?2) ON CONFLICT(track_id) DO NOTHING")?.execute(params![track_id, secs])?;
    } else {
        conn.prepare_cached("DELETE FROM favourite_tracks WHERE track_id = ?1")?.execute([track_id])?;
    }
    Ok(())
}

/// The local rating as the server has it (0: none).
pub fn set_local_rating(conn: &Connection, track_id: i64, rating: u8) -> rusqlite::Result<()> {
    if rating == 0 {
        conn.prepare_cached("DELETE FROM track_ratings WHERE track_id = ?1")?.execute([track_id])?;
    } else {
        conn.prepare_cached(
            "INSERT INTO track_ratings (track_id, rating, rated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id) DO UPDATE SET rating = excluded.rating, rated_at = excluded.rated_at WHERE rating != excluded.rating",
        )?
        .execute(params![track_id, i64::from(rating.min(5)), crate::db::now()])?;
    }
    Ok(())
}

/// The favourite agreed with the server, now.
pub fn agree_star(conn: &Connection, song: &str, starred: bool, at: Option<i64>) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO remote_marks (song_id, starred, starred_at, star_agreed) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(song_id) DO UPDATE SET starred = excluded.starred, starred_at = excluded.starred_at, star_agreed = excluded.star_agreed",
    )?
    .execute(params![song, starred, at, now_ms()])?;
    Ok(())
}

/// The rating agreed with the server, now.
pub fn agree_rating(conn: &Connection, song: &str, rating: u8) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO remote_marks (song_id, rating, rating_agreed) VALUES (?1, ?2, ?3)
         ON CONFLICT(song_id) DO UPDATE SET rating = excluded.rating, rating_agreed = excluded.rating_agreed",
    )?
    .execute(params![song, i64::from(rating.min(5)), now_ms()])?;
    Ok(())
}

// ---- plays ------------------------------------------------------------------------

/// A play of track `track_id` counted at `played_at` (Unix seconds), queued for the server when
/// it has the track. Whether it was.
pub fn queue_play(conn: &Connection, track_id: i64, played_at: i64) -> rusqlite::Result<bool> {
    let n = conn
        .prepare_cached("INSERT INTO remote_plays (song_id, played_at) SELECT remote_id, ?2 FROM tracks WHERE id = ?1 AND remote_id IS NOT NULL")?
        .execute(params![track_id, played_at * 1000])?;
    Ok(n > 0)
}

/// A play queued for the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedPlay {
    pub id: i64,
    pub song_id: String,
    /// Unix milliseconds.
    pub played_at: i64,
}

/// The oldest `limit` plays not yet reported.
pub fn queued_plays(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<QueuedPlay>> {
    let mut st = conn.prepare_cached("SELECT id, song_id, played_at FROM remote_plays ORDER BY played_at, id LIMIT ?1")?;
    st.query_map([limit as i64], |r| Ok(QueuedPlay { id: r.get(0)?, song_id: r.get(1)?, played_at: r.get(2)? }))?.collect()
}

/// A play reported (or one the server cannot take): off the queue.
pub fn unqueue_play(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.prepare_cached("DELETE FROM remote_plays WHERE id = ?1")?.execute([id])?;
    Ok(())
}

/// Every queued play forgotten (reporting plays turned off).
pub fn clear_plays(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM remote_plays", [])
}

pub fn queued_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM remote_plays", [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agreed(starred: bool, at: Option<i64>) -> Agreed {
        Agreed { starred: Some(starred), starred_at: at, star_agreed: Some(1_000), ..Agreed::default() }
    }

    #[test]
    fn a_favourite_changed_on_one_side_goes_to_the_other() {
        // Only the server: taken.
        assert_eq!(decide_star(None, &agreed(false, None), Some(Some(5_000))), Plan::take(true, Some(5_000)));
        assert_eq!(decide_star(Some(500), &agreed(true, Some(400)), Some(None)), Plan::take(false, None));
        // Only local: sent.
        assert_eq!(decide_star(Some(2_000), &agreed(false, None), Some(None)), Plan::push(true));
        assert_eq!(decide_star(None, &agreed(true, Some(400)), Some(Some(400))), Plan::push(false));
        // Nothing changed: nothing to do.
        assert_eq!(decide_star(Some(500), &agreed(true, Some(400)), Some(Some(400))), Plan::none());
        assert_eq!(decide_star(None, &agreed(false, None), Some(None)), Plan::none());
        // The server not read this time: a local change still goes.
        assert_eq!(decide_star(None, &agreed(true, Some(400)), None), Plan::push(false));
        assert_eq!(decide_star(Some(1), &agreed(true, Some(400)), None), Plan::none());
        // Both changed to the same: agreed, nothing sent.
        assert_eq!(decide_star(None, &agreed(true, Some(400)), Some(None)), Plan::agree(false, None));
        // A star sent: its time is recorded once the server says it.
        assert_eq!(decide_star(Some(500), &agreed(true, None), Some(Some(900))), Plan::agree(true, Some(900)));
    }

    #[test]
    fn both_changed_the_newer_favourite_wins_when_both_times_are_known() {
        // Agreed starred; starred again on the server at 9000; unstarred in the app at 5000.
        let mut m = agreed(true, Some(400));
        m.star_local = Some(5_000);
        assert_eq!(decide_star(None, &m, Some(Some(9_000))), Plan::take(true, Some(9_000)), "the server's is newer");
        m.star_local = Some(12_000);
        assert_eq!(decide_star(None, &m, Some(Some(9_000))), Plan::push(false), "the app's is newer");
        // The app's change time not known (made while the switch was off): local wins.
        m.star_local = None;
        assert_eq!(decide_star(None, &m, Some(Some(9_000))), Plan::push(false));
        // Unstarred on the server (no time), unstarred and starred again in the app: local wins.
        let mut m = agreed(true, Some(400));
        m.star_local = Some(3_000);
        assert_eq!(decide_star(Some(3_000), &m, Some(None)), Plan::push(true));
    }

    #[test]
    fn the_first_sync_takes_the_union_of_favourites() {
        let never = Agreed::default();
        assert_eq!(decide_star(Some(1), &never, Some(None)), Plan::push(true));
        assert_eq!(decide_star(None, &never, Some(Some(7))), Plan::take(true, Some(7)));
        assert_eq!(decide_star(Some(1), &never, Some(Some(7))), Plan::agree(true, Some(7)));
        assert_eq!(decide_star(None, &never, Some(None)), Plan::agree(false, None));
        assert_eq!(decide_star(None, &never, None), Plan::none(), "nothing known of the server, nothing local");
        // Unstarred in the app before any agreement: the user's change, sent as it is.
        let touched = Agreed { star_local: Some(5), ..Agreed::default() };
        assert_eq!(decide_star(None, &touched, Some(Some(7))), Plan::push(false));
        assert_eq!(decide_star(None, &touched, None), Plan::push(false));
    }

    fn rated(r: u8) -> Agreed {
        Agreed { rating: Some(r), rating_agreed: Some(1_000), ..Agreed::default() }
    }

    #[test]
    fn ratings_go_both_ways_and_local_wins_when_both_changed() {
        assert_eq!(decide_rating(3, &rated(3), Some(5)), Plan::take(5, None), "only the server");
        assert_eq!(decide_rating(4, &rated(3), Some(3)), Plan::push(4), "only local");
        assert_eq!(decide_rating(0, &rated(3), Some(3)), Plan::push(0), "cleared locally");
        assert_eq!(decide_rating(4, &rated(3), Some(1)), Plan::push(4), "both: local wins");
        assert_eq!(decide_rating(4, &rated(3), Some(4)), Plan::agree(4, None), "both, the same");
        assert_eq!(decide_rating(3, &rated(3), Some(3)), Plan::none());
        assert_eq!(decide_rating(3, &rated(3), Some(0)), Plan::take(0, None), "cleared on the server");
        assert_eq!(decide_rating(2, &rated(3), None), Plan::push(2), "the server not read: a local change goes");
        assert_eq!(decide_rating(3, &rated(3), None), Plan::none());
        // Rated 4 then 3 again in the app after the agreement; 5 on the server: the app's.
        let mut m = rated(3);
        m.rating_local = Some(2_000);
        assert_eq!(decide_rating(3, &m, Some(5)), Plan::push(3));
    }

    #[test]
    fn the_first_sync_keeps_a_local_rating_else_takes_the_server_s() {
        let never = Agreed::default();
        assert_eq!(decide_rating(4, &never, Some(2)), Plan::push(4));
        assert_eq!(decide_rating(4, &never, Some(4)), Plan::agree(4, None));
        assert_eq!(decide_rating(0, &never, Some(2)), Plan::take(2, None));
        assert_eq!(decide_rating(0, &never, Some(0)), Plan::agree(0, None));
        assert_eq!(decide_rating(4, &never, None), Plan::push(4));
        assert_eq!(decide_rating(0, &never, None), Plan::none());
    }

    #[test]
    fn rows_touches_and_agreements() {
        let conn = crate::db::open_in_memory().unwrap();
        conn.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (1, 'k', 'A', 'B', 'M:/a', 5, 5);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at, remote_id)
                 VALUES (10, 'M:/a/01.flac', 1, 'One', 'B', 200.5, 123, 456, 7, 's1'), (11, 'M:/a/02.flac', 1, 'Two', 'B', 180.0, 124, 457, 8, NULL),
                        (12, 'navidrome:song/s3', 1, 'Three', 'B', 90.0, 0, 999, 9, 's3');
             INSERT INTO favourite_tracks VALUES (12, 10);
             INSERT INTO track_ratings VALUES (10, 4, 11);",
        )
        .unwrap();
        let rows = rows(&conn).unwrap();
        assert_eq!(rows.iter().map(|r| (r.song_id.as_str(), r.track_id, r.starred, r.rating)).collect::<Vec<_>>(), [("s1", 10, None, 4), ("s3", 12, Some(10_000), 0)], "only the server's tracks");
        assert_eq!(rows[0].agreed, Agreed::default());
        assert_eq!(touched(&conn, 11, Mark::Favourite).unwrap(), None, "a track the server does not have");
        assert_eq!(touched(&conn, 10, Mark::Rating).unwrap().as_deref(), Some("s1"));
        agree_star(&conn, "s1", true, Some(77)).unwrap();
        agree_rating(&conn, "s1", 4).unwrap();
        let r = row_of(&conn, 10).unwrap().unwrap();
        assert!(r.agreed.rating_local.is_some() && r.agreed.rating == Some(4) && r.agreed.starred == Some(true) && r.agreed.starred_at == Some(77));
        set_local_star(&conn, 10, true, Some(5_000)).unwrap();
        set_local_rating(&conn, 12, 3).unwrap();
        set_local_star(&conn, 12, false, None).unwrap();
        let r = row_of(&conn, 10).unwrap().unwrap();
        assert_eq!((r.starred, r.rating), (Some(5_000), 4), "starred at the server's time");
        let r = row_of(&conn, 12).unwrap().unwrap();
        assert_eq!((r.starred, r.rating), (None, 3));
        // Plays: only of the server's tracks, oldest first, off once reported.
        assert!(queue_play(&conn, 12, 200).unwrap());
        assert!(queue_play(&conn, 10, 100).unwrap());
        assert!(!queue_play(&conn, 11, 50).unwrap());
        let q = queued_plays(&conn, 10).unwrap();
        assert_eq!(q.iter().map(|p| (p.song_id.as_str(), p.played_at)).collect::<Vec<_>>(), [("s1", 100_000), ("s3", 200_000)]);
        unqueue_play(&conn, q[0].id).unwrap();
        assert_eq!(queued_count(&conn).unwrap(), 1);
        assert_eq!(clear_plays(&conn).unwrap(), 1);
    }
}
