//! The play queue kept across restarts (schema 4: `queue_items`, `queue_state`).
//!
//! The app writes the whole queue when it changes and only `queue_state` (the current
//! item and how far into it) as playback moves on, so a long queue is not rewritten
//! every few seconds. Shuffle and repeat themselves are settings (settings.json); what
//! is here is the order shuffle turns back to (`SavedItem::original`).

use rusqlite::{Connection, OptionalExtension, params};

use crate::Track;
use crate::query::{TRACK_COL_COUNT, TRACK_COLS, track_row};

/// One queue entry: the track, and its place in the order before shuffle (while shuffle
/// is on; `None` otherwise, or for an item added while shuffled).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedItem {
    pub track: Track,
    pub original: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedQueue {
    pub items: Vec<SavedItem>,
    /// Index into `items`.
    pub current: Option<usize>,
    /// Seconds into the current item.
    pub seconds: f64,
}

/// Replace the saved queue: `items` are (track id, original place), in queue order.
pub fn save_queue(conn: &Connection, items: &[(i64, Option<i64>)], current: Option<usize>, seconds: f64) -> rusqlite::Result<()> {
    let tx = crate::db::write(conn)?;
    tx.execute("DELETE FROM queue_items", [])?;
    {
        // Track ids that are no longer in the library (removed by a scan while queued)
        // are left out rather than failing the whole save on the foreign key.
        let mut ins = tx.prepare_cached(
            "INSERT INTO queue_items (position, track_id, original)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?2)",
        )?;
        for (i, (id, original)) in items.iter().enumerate() {
            ins.execute(params![i as i64, id, original])?;
        }
    }
    put_state(&tx, current, seconds)?;
    tx.commit()
}

/// Update only the current item and the position in it.
pub fn save_position(conn: &Connection, current: Option<usize>, seconds: f64) -> rusqlite::Result<()> {
    put_state(conn, current, seconds)
}

fn put_state(conn: &Connection, current: Option<usize>, seconds: f64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO queue_state (id, current, seconds) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET current = excluded.current, seconds = excluded.seconds",
        params![current.map(|c| c as i64), seconds.max(0.0)],
    )?;
    Ok(())
}

/// The saved queue, without the tracks that are no longer in the library. If the
/// current item is one of those, the next one that is left becomes current, from its
/// start (or the last one, if none follows).
pub fn load_queue(conn: &Connection) -> rusqlite::Result<SavedQueue> {
    let state: Option<(Option<i64>, f64)> =
        conn.query_row("SELECT current, seconds FROM queue_state WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let (saved_current, seconds) = state.unwrap_or((None, 0.0));
    let mut st = conn.prepare(&format!(
        "SELECT {TRACK_COLS}, q.position, q.original FROM queue_items q
         JOIN tracks t ON t.id = q.track_id JOIN albums a ON a.id = t.album_id
         ORDER BY q.position"
    ))?;
    let mut items = Vec::new();
    let mut current = None;
    let mut exact = false;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, i64>(TRACK_COL_COUNT)?, SavedItem { track: track_row(r)?, original: r.get(TRACK_COL_COUNT + 1)? }))
    })?;
    for row in rows {
        let (position, item) = row?;
        if let Some(c) = saved_current
            && current.is_none()
            && position >= c
        {
            current = Some(items.len());
            exact = position == c;
        }
        items.push(item);
    }
    if saved_current.is_some() && current.is_none() && !items.is_empty() {
        current = Some(items.len() - 1);
    }
    Ok(SavedQueue { items, current, seconds: if exact { seconds } else { 0.0 } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn seed(c: &Connection) {
        c.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES (1, 'a', 'Al', 'Ar', 'M:/a', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                (1, 'M:/a/1', 1, 'one', 'Ar', 10, 0, 0, 0), (2, 'M:/a/2', 1, 'two', 'Ar', 20, 0, 0, 0),
                (3, 'M:/a/3', 1, 'three', 'Ar', 30, 0, 0, 0), (4, 'M:/a/4', 1, 'four', 'Ar', 40, 0, 0, 0);",
        )
        .unwrap();
    }

    fn ids(q: &SavedQueue) -> Vec<i64> {
        q.items.iter().map(|i| i.track.id).collect()
    }

    #[test]
    fn round_trips_and_drops_tracks_that_are_gone() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        assert_eq!(load_queue(&c).unwrap(), SavedQueue::default());
        save_queue(&c, &[(3, Some(2)), (1, Some(0)), (2, None), (4, Some(1))], Some(2), 12.5).unwrap();
        let q = load_queue(&c).unwrap();
        assert_eq!(ids(&q), vec![3, 1, 2, 4]);
        assert_eq!(q.items.iter().map(|i| i.original).collect::<Vec<_>>(), vec![Some(2), Some(0), None, Some(1)]);
        assert_eq!((q.current, q.seconds), (Some(2), 12.5));
        assert_eq!(q.items[0].track.title, "three");
        save_position(&c, Some(3), 7.0).unwrap();
        let q = load_queue(&c).unwrap();
        assert_eq!((q.current, q.seconds), (Some(3), 7.0));

        // A queued track leaves the library: it leaves the queue; current stays on its item.
        c.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        let q = load_queue(&c).unwrap();
        assert_eq!(ids(&q), vec![3, 2, 4]);
        assert_eq!((q.current, q.seconds), (Some(2), 7.0));
        // The current one goes: the next one left is current, from its start.
        c.execute("DELETE FROM tracks WHERE id = 4", []).unwrap();
        let q = load_queue(&c).unwrap();
        assert_eq!(ids(&q), vec![3, 2]);
        assert_eq!((q.current, q.seconds), (Some(1), 0.0));
        // Saving ids that are gone skips them.
        save_queue(&c, &[(99, None), (2, None)], Some(1), 3.0).unwrap();
        let q = load_queue(&c).unwrap();
        assert_eq!(ids(&q), vec![2]);
    }

    #[test]
    fn an_empty_queue_saves_as_empty() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        save_queue(&c, &[(1, None)], Some(0), 1.0).unwrap();
        save_queue(&c, &[], None, 0.0).unwrap();
        assert_eq!(load_queue(&c).unwrap(), SavedQueue::default());
    }
}
