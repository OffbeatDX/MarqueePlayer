//! Artists and genres as tables of their own (schema 8): each credit of a track or an
//! album is a row (`track_artists`, `album_artists`, `track_genres`, `album_genres`) that
//! points at one artist or genre, so a track with two artists is on both their pages and an
//! album with three genres is under all three.
//!
//! An artist is identified (`artists.key`) by its MusicBrainz artist id when credits carry
//! one ("mb:<id>"), else by its name's `names::norm` ("n:<name>"). A credit without an id
//! goes to the one artist with an id whose credits have the same name, when there is
//! exactly one: "Radiohead" from an untagged rip joins the Picard-tagged Radiohead. When
//! two artists with ids share a name, credits without an id stay apart under the name
//! (which of the two they mean cannot be known). This is worked out per name from all the
//! credits there are (`resolve`), so it never depends on the order files were scanned.
//!
//! An artist's or genre's name is the most common spelling among its credits (ties: the
//! first in code point order); an artist's sort name the most common ARTISTSORT of tracks
//! that credit only them, or ALBUMARTISTSORT of albums that are only theirs. Artists and
//! genres nothing credits any more are deleted.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::names::{self, Credit};

/// What a write touched, for `resolve`.
#[derive(Debug, Default)]
pub struct Touched {
    /// Artist names (`norm`) whose credits changed.
    pub norms: HashSet<String>,
    /// Artists that may have lost or gained credits.
    pub artists: HashSet<i64>,
    pub genres: HashSet<i64>,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        self.norms.is_empty() && self.artists.is_empty() && self.genres.is_empty()
    }
}

/// The artist with this key, made if there is none (named `name` until `resolve`).
fn ensure_artist(c: &Connection, key: &str, name: &str) -> rusqlite::Result<i64> {
    let mut find = c.prepare_cached("SELECT id FROM artists WHERE key = ?1")?;
    if let Some(id) = find.query_row([key], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    c.prepare_cached("INSERT INTO artists (key, norm, name) VALUES (?1, ?2, ?3)")?.execute(params![key, names::norm(name), name])?;
    Ok(c.last_insert_rowid())
}

fn ensure_genre(c: &Connection, name: &str) -> rusqlite::Result<i64> {
    let key = names::norm(name);
    let mut find = c.prepare_cached("SELECT id FROM genres WHERE key = ?1")?;
    if let Some(id) = find.query_row([&key], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    c.prepare_cached("INSERT INTO genres (key, name) VALUES (?1, ?2)")?.execute(params![key, name])?;
    Ok(c.last_insert_rowid())
}

/// The key a credit gets before `resolve` looks at its name's other credits.
fn first_key(c: &Credit) -> String {
    match &c.mbid {
        Some(id) => format!("mb:{id}"),
        None => format!("n:{}", names::norm(&c.name)),
    }
}

/// Replace a track's artist credits.
pub fn set_track_artists(c: &Connection, track: i64, credits: &[Credit], t: &mut Touched) -> rusqlite::Result<()> {
    forget_credits(c, "track_artists", "track_id", track, t)?;
    let mut ins = c.prepare_cached("INSERT INTO track_artists (track_id, position, artist_id, name, norm, mbid, featured) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
    for (i, cr) in credits.iter().enumerate() {
        let n = names::norm(&cr.name);
        if n.is_empty() {
            continue;
        }
        let id = ensure_artist(c, &first_key(cr), &cr.name)?;
        ins.execute(params![track, i as i64, id, cr.name, n, cr.mbid, cr.featured])?;
        t.norms.insert(n);
        t.artists.insert(id);
    }
    Ok(())
}

/// Replace an album's album artist credits.
pub fn set_album_artists(c: &Connection, album: i64, credits: &[Credit], t: &mut Touched) -> rusqlite::Result<()> {
    forget_credits(c, "album_artists", "album_id", album, t)?;
    let mut ins = c.prepare_cached("INSERT INTO album_artists (album_id, position, artist_id, name, norm, mbid) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
    for (i, cr) in credits.iter().enumerate() {
        let n = names::norm(&cr.name);
        if n.is_empty() {
            continue;
        }
        let id = ensure_artist(c, &first_key(cr), &cr.name)?;
        ins.execute(params![album, i as i64, id, cr.name, n, cr.mbid])?;
        t.norms.insert(n);
        t.artists.insert(id);
    }
    Ok(())
}

/// Note what a track's or album's credits were (their names and artists may need
/// resolving again once they are gone), and delete them.
fn forget_credits(c: &Connection, table: &str, col: &str, id: i64, t: &mut Touched) -> rusqlite::Result<()> {
    let mut st = c.prepare_cached(&format!("SELECT norm, artist_id FROM {table} WHERE {col} = ?1"))?;
    for row in st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (n, a) = row?;
        t.norms.insert(n);
        t.artists.insert(a);
    }
    c.prepare_cached(&format!("DELETE FROM {table} WHERE {col} = ?1"))?.execute([id])?;
    Ok(())
}

/// Before a track row is deleted (its credits go with it, by cascade): note them.
pub fn forget_track(c: &Connection, track: i64, t: &mut Touched) -> rusqlite::Result<()> {
    forget_credits(c, "track_artists", "track_id", track, t)?;
    let mut st = c.prepare_cached("SELECT genre_id FROM track_genres WHERE track_id = ?1")?;
    for g in st.query_map([track], |r| r.get::<_, i64>(0))? {
        t.genres.insert(g?);
    }
    Ok(())
}

/// Before an album row is deleted: note its credits.
pub fn forget_album(c: &Connection, album: i64, t: &mut Touched) -> rusqlite::Result<()> {
    forget_credits(c, "album_artists", "album_id", album, t)?;
    let mut st = c.prepare_cached("SELECT genre_id FROM album_genres WHERE album_id = ?1")?;
    for g in st.query_map([album], |r| r.get::<_, i64>(0))? {
        t.genres.insert(g?);
    }
    Ok(())
}

/// Replace a track's genres.
pub fn set_track_genres(c: &Connection, track: i64, genres: &[String], t: &mut Touched) -> rusqlite::Result<()> {
    {
        let mut st = c.prepare_cached("SELECT genre_id FROM track_genres WHERE track_id = ?1")?;
        for g in st.query_map([track], |r| r.get::<_, i64>(0))? {
            t.genres.insert(g?);
        }
    }
    c.prepare_cached("DELETE FROM track_genres WHERE track_id = ?1")?.execute([track])?;
    let mut ins = c.prepare_cached("INSERT OR IGNORE INTO track_genres (track_id, position, genre_id, name) VALUES (?1, ?2, ?3, ?4)")?;
    for (i, g) in genres.iter().enumerate() {
        if names::norm(g).is_empty() {
            continue;
        }
        let id = ensure_genre(c, g)?;
        ins.execute(params![track, i as i64, id, g])?;
        t.genres.insert(id);
    }
    Ok(())
}

/// An album's genres: every genre of its tracks, the one most of them carry first (ties:
/// the order the tracks name them in, album order).
pub fn set_album_genres(c: &Connection, album: i64, t: &mut Touched) -> rusqlite::Result<()> {
    {
        let mut st = c.prepare_cached("SELECT genre_id FROM album_genres WHERE album_id = ?1")?;
        for g in st.query_map([album], |r| r.get::<_, i64>(0))? {
            t.genres.insert(g?);
        }
    }
    c.prepare_cached("DELETE FROM album_genres WHERE album_id = ?1")?.execute([album])?;
    let mut order: Vec<(i64, usize)> = Vec::new();
    {
        // A server's tracks (`remote`) count only for an album with no file of its own: a file's
        // tags win, as for every other album value (`scan::album_values`).
        let mut st = c.prepare_cached(
            "SELECT tg.genre_id FROM track_genres tg JOIN tracks t ON t.id = tg.track_id
             WHERE t.album_id = ?1 AND (t.path NOT LIKE 'navidrome:%'
                   OR NOT EXISTS(SELECT 1 FROM tracks u WHERE u.album_id = ?1 AND u.path NOT LIKE 'navidrome:%'))
             ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path, tg.position",
        )?;
        for g in st.query_map([album], |r| r.get::<_, i64>(0))? {
            let g = g?;
            match order.iter_mut().find(|(id, _)| *id == g) {
                Some(o) => o.1 += 1,
                None => order.push((g, 1)),
            }
        }
    }
    // Stable: equal counts keep the order they were first named in.
    order.sort_by_key(|o| std::cmp::Reverse(o.1));
    let mut ins = c.prepare_cached("INSERT INTO album_genres (album_id, position, genre_id) VALUES (?1, ?2, ?3)")?;
    for (i, (g, _)) in order.iter().enumerate() {
        ins.execute(params![album, i as i64, g])?;
        t.genres.insert(*g);
    }
    Ok(())
}

/// Settle what a write touched: each name's credits to the right artist (see the module
/// doc), every touched artist's and genre's name and sort name, and delete the ones
/// nothing credits.
pub fn resolve(c: &Connection, t: &mut Touched) -> rusqlite::Result<()> {
    for n in std::mem::take(&mut t.norms) {
        resolve_name(c, &n, &mut t.artists)?;
    }
    for id in std::mem::take(&mut t.artists) {
        settle_artist(c, id)?;
    }
    for id in std::mem::take(&mut t.genres) {
        settle_genre(c, id)?;
    }
    Ok(())
}

/// The first half of `resolve`: each name's credits to the right artist. The artists and genres
/// it touched stay in `t` for `settle_artists_and_genres`, which a scan writing in batches runs
/// in transactions of its own (their names are worked out from all their credits, which grows
/// with the library: not something to hold the database for in every batch).
pub fn resolve_names(c: &Connection, t: &mut Touched) -> rusqlite::Result<()> {
    for n in std::mem::take(&mut t.norms) {
        resolve_name(c, &n, &mut t.artists)?;
    }
    Ok(())
}

/// The second half of `resolve`: these artists' and genres' names and sort names from their
/// credits, and the ones nothing credits deleted.
pub fn settle_artists_and_genres(c: &Connection, artists: &[i64], genres: &[i64]) -> rusqlite::Result<()> {
    for &id in artists {
        settle_artist(c, id)?;
    }
    for &id in genres {
        settle_genre(c, id)?;
    }
    Ok(())
}

/// Point every credit named `n` at its artist.
fn resolve_name(c: &Connection, n: &str, touched: &mut HashSet<i64>) -> rusqlite::Result<()> {
    let mut ids: Vec<String> = Vec::new();
    {
        let mut st = c.prepare_cached(
            "SELECT mbid FROM track_artists WHERE norm = ?1 AND mbid IS NOT NULL
             UNION SELECT mbid FROM album_artists WHERE norm = ?1 AND mbid IS NOT NULL",
        )?;
        for id in st.query_map([n], |r| r.get::<_, String>(0))? {
            ids.push(id?);
        }
    }
    let name_of = |c: &Connection| -> rusqlite::Result<String> {
        c.query_row(
            "SELECT name FROM (SELECT name FROM track_artists WHERE norm = ?1 UNION ALL SELECT name FROM album_artists WHERE norm = ?1) LIMIT 1",
            [n],
            |r| r.get(0),
        )
        .optional()
        .map(|o| o.unwrap_or_else(|| n.to_owned()))
    };
    let unnamed = match ids.as_slice() {
        [only] => format!("mb:{only}"),
        _ => format!("n:{n}"),
    };
    for (table, col) in [("track_artists", "track_id"), ("album_artists", "album_id")] {
        // Credits without an id; then each id's.
        let mut targets: Vec<(Option<String>, String)> = vec![(None, unnamed.clone())];
        targets.extend(ids.iter().map(|id| (Some(id.clone()), format!("mb:{id}"))));
        for (mbid, key) in targets {
            let wrong: Vec<(i64, i64, i64)> = {
                let mut st = c.prepare_cached(&format!(
                    "SELECT x.{col}, x.position, x.artist_id FROM {table} x JOIN artists a ON a.id = x.artist_id
                     WHERE x.norm = ?1 AND x.mbid IS ?2 AND a.key <> ?3"
                ))?;
                st.query_map(params![n, mbid, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
            };
            if wrong.is_empty() {
                continue;
            }
            let to = ensure_artist(c, &key, &name_of(c)?)?;
            touched.insert(to);
            let mut set = c.prepare_cached(&format!("UPDATE {table} SET artist_id = ?3 WHERE {col} = ?1 AND position = ?2"))?;
            for (owner, pos, from) in wrong {
                set.execute(params![owner, pos, to])?;
                touched.insert(from);
            }
        }
    }
    Ok(())
}

/// An artist's name and sort name from its credits; deleted when it has none.
fn settle_artist(c: &Connection, id: i64) -> rusqlite::Result<()> {
    let name: Option<String> = c
        .prepare_cached(
            "SELECT name FROM (SELECT name FROM track_artists WHERE artist_id = ?1
                               UNION ALL SELECT name FROM album_artists WHERE artist_id = ?1)
             GROUP BY name ORDER BY COUNT(*) DESC, name LIMIT 1",
        )?
        .query_row([id], |r| r.get(0))
        .optional()?;
    let Some(name) = name else {
        c.prepare_cached("DELETE FROM artists WHERE id = ?1")?.execute([id])?;
        return Ok(());
    };
    let sort: Option<String> = c
        .prepare_cached(
            "SELECT s FROM (
                SELECT g.artist_sort AS s FROM track_artists x JOIN track_tags g ON g.track_id = x.track_id
                 WHERE x.artist_id = ?1 AND g.artist_sort IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM track_artists y WHERE y.track_id = x.track_id AND y.position <> x.position)
                UNION ALL
                SELECT a.album_artist_sort FROM album_artists x JOIN albums a ON a.id = x.album_id
                 WHERE x.artist_id = ?1 AND a.album_artist_sort IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM album_artists y WHERE y.album_id = x.album_id AND y.position <> x.position))
             GROUP BY s ORDER BY COUNT(*) DESC, s LIMIT 1",
        )?
        .query_row([id], |r| r.get(0))
        .optional()?;
    c.prepare_cached("UPDATE artists SET name = ?2, norm = ?3, sort_name = ?4 WHERE id = ?1")?.execute(params![id, name, names::norm(&name), sort])?;
    // The albums it heads sort by it (unless they have a sort name of their own).
    c.prepare_cached(
        "UPDATE albums SET sort_artist = COALESCE(album_artist_sort, ?2)
         WHERE id IN (SELECT album_id FROM album_artists WHERE artist_id = ?1 AND position = 0)
           AND sort_artist IS NOT COALESCE(album_artist_sort, ?2)",
    )?
    .execute(params![id, sort])?;
    Ok(())
}

fn settle_genre(c: &Connection, id: i64) -> rusqlite::Result<()> {
    let name: Option<String> = c
        .prepare_cached("SELECT name FROM track_genres WHERE genre_id = ?1 GROUP BY name ORDER BY COUNT(*) DESC, name LIMIT 1")?
        .query_row([id], |r| r.get(0))
        .optional()?;
    let used: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM album_genres WHERE genre_id = ?1)")?.query_row([id], |r| r.get(0))?;
    match name {
        Some(name) => {
            c.prepare_cached("UPDATE genres SET name = ?2 WHERE id = ?1")?.execute(params![id, name])?;
        }
        None if used => {}
        None => {
            c.prepare_cached("DELETE FROM genres WHERE id = ?1")?.execute([id])?;
        }
    }
    Ok(())
}

/// Every credit made again from the stored strings, as earlier versions read them: each
/// track's artists split from its artist (";", " / ", "feat."), its genres from its genre;
/// each album's artists split from its album artist, its genres those of its tracks and its
/// own stored genre; the label's key. What schema 8's migration fills (and tests use, after
/// writing rows by hand). Reads no file.
pub fn rebuild_from_strings(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM track_artists; DELETE FROM album_artists; DELETE FROM track_genres; DELETE FROM album_genres;
         DELETE FROM artists; DELETE FROM genres;",
    )?;
    let mut t = Touched::default();
    let tracks: Vec<(i64, String, Option<String>)> =
        conn.prepare("SELECT id, artist, genre FROM tracks")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    for (id, artist, genre) in &tracks {
        set_track_artists(conn, *id, &names::credits(artist), &mut t)?;
        let genres: Vec<String> = genre.as_deref().map(names::split_genres).unwrap_or_default().into_iter().map(names::tidy).collect();
        set_track_genres(conn, *id, &genres, &mut t)?;
    }
    let albums: Vec<(i64, String, Option<String>, Option<String>)> =
        conn.prepare("SELECT id, album_artist, genre, label FROM albums")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut label = conn.prepare("UPDATE albums SET label_key = ?2 WHERE id = ?1")?;
    for (id, artist, genre, l) in &albums {
        label.execute(params![id, l.as_deref().map(names::norm)])?;
        let credits: Vec<Credit> = names::split_artists(artist).into_iter().map(|n| Credit::new(n, false)).collect();
        set_album_artists(conn, *id, &credits, &mut t)?;
        set_album_genres(conn, *id, &mut t)?;
        // The album's own genre as it was stored, if its tracks do not say it.
        for g in genre.as_deref().map(names::split_genres).unwrap_or_default() {
            let gid = ensure_genre(conn, &names::tidy(g))?;
            conn.prepare_cached(
                "INSERT INTO album_genres (album_id, position, genre_id)
                 SELECT ?1, (SELECT COALESCE(MAX(position) + 1, 0) FROM album_genres WHERE album_id = ?1), ?2
                 WHERE NOT EXISTS (SELECT 1 FROM album_genres WHERE album_id = ?1 AND genre_id = ?2)",
            )?
            .execute(params![id, gid])?;
            t.genres.insert(gid);
        }
    }
    resolve(conn, &mut t)
}
