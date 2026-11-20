//! SQLite schema and connection setup.
//!
//! Track identity is the file path; album identity is `albums.key` (see
//! `scan::album_key`). Both ids stay stable across rescans, so plays, favourites and
//! playlists survive; a file that moves or is renamed keeps its row (the scan re-points
//! it, `scan::match_moves`), and an album whose key changes passes its date and
//! favourite on (`scan::inherit_album`). Bump `SCHEMA_VERSION` and add a step to `migrate` for changes.

use std::path::Path;

use rusqlite::{Connection, params};

/// The schema this build writes (`PRAGMA user_version`); Settings > About shows it.
pub const SCHEMA_VERSION: i64 = 13;

const SCHEMA_V1: &str = r#"
CREATE TABLE albums (
    id              INTEGER PRIMARY KEY,
    key             TEXT NOT NULL UNIQUE,
    title           TEXT NOT NULL,
    album_artist    TEXT NOT NULL,
    album_artist_sort TEXT,
    -- Release date as tagged: YYYY, YYYY-MM or YYYY-MM-DD. ISO order sorts as text.
    date            TEXT,
    genre           TEXT,
    label           TEXT,
    release_type    TEXT,
    folder          TEXT NOT NULL,
    -- Stem of the cached art files (see art.rs), NULL when the album has none.
    art             TEXT,
    -- Unix seconds the scanner first saw this album ("recently added"). Milliseconds
    -- from schema 2 on (see SCHEMA_V2).
    added_at        INTEGER NOT NULL
);
CREATE INDEX albums_added ON albums(added_at DESC);
CREATE INDEX albums_date ON albums(date DESC);
CREATE INDEX albums_artist ON albums(album_artist);

CREATE TABLE tracks (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    album_id    INTEGER NOT NULL REFERENCES albums(id),
    title       TEXT NOT NULL,
    artist      TEXT NOT NULL,
    disc_no     INTEGER,
    track_no    INTEGER,
    duration    REAL NOT NULL,
    genre       TEXT,
    mtime       INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    added_at    INTEGER NOT NULL
);
CREATE INDEX tracks_album ON tracks(album_id, disc_no, track_no);
CREATE INDEX tracks_artist ON tracks(artist);

CREATE TABLE plays (
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    played_at   INTEGER NOT NULL
);
CREATE INDEX plays_track ON plays(track_id);
CREATE INDEX plays_time ON plays(played_at DESC);

CREATE TABLE favourite_albums (
    album_id    INTEGER PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
    added_at    INTEGER NOT NULL
);
CREATE TABLE favourite_tracks (
    track_id    INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    added_at    INTEGER NOT NULL
);

CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, position)
);
"#;

/// Schema 2: "recently added" in milliseconds (dates imported in bulk keep their
/// order, which within one bulk import is sub-second), with the scanner's own
/// first-seen time kept apart:
/// - `added_at`: what "Recently added" sorts by, Unix milliseconds.
/// - `first_seen`: when this library first saw the album, Unix milliseconds. Never
///   changed by an import.
/// - `added_source`: NULL when `added_at` is the scanner's, else where it came from
///   (`navidrome`: the Navidrome import earlier versions had; those dates are kept).
const SCHEMA_V2: &str = r#"
UPDATE albums SET added_at = added_at * 1000;
ALTER TABLE albums ADD COLUMN first_seen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE albums ADD COLUMN added_source TEXT;
UPDATE albums SET first_seen = added_at;
"#;

/// Schema 3: an album's original release date (ORIGINALDATE) and this edition's
/// (RELEASEDATE), which "Recently released" sorts by the way Navidrome does. Every
/// file is re-read once on the next scan to fill them (mtime -1 never matches).
const SCHEMA_V3: &str = r#"
ALTER TABLE albums ADD COLUMN original_date TEXT;
ALTER TABLE albums ADD COLUMN release_date TEXT;
UPDATE tracks SET mtime = -1;
"#;

/// Schema 5: `albums.art_checked`, the album folders' modification times when art was
/// last looked for and none found (`scan::art_fingerprint`), so a scan looks again only
/// when they change. NULL (look) for every album at first. Only adds a column.
const SCHEMA_V5: &str = r#"
ALTER TABLE albums ADD COLUMN art_checked TEXT;
"#;

/// Schema 6: track ratings, 1 to 5 stars (no row: not rated). A table of their own, as
/// favourites are, so the scanner's writes to `tracks` can never touch them; a track that
/// leaves the library takes its rating with it. Only adds a table.
const SCHEMA_V6: &str = r#"
CREATE TABLE track_ratings (
    track_id    INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    rating      INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
    rated_at    INTEGER NOT NULL
);
"#;

/// Schema 7: each track's technical details (`audio.rs`): its format as people name it
/// ("FLAC", "MP3", "Opus"...), sample rate (Hz), bit depth (lossless formats only),
/// bitrate (kbps, the average for a variable one) and channels. Only adds columns; every
/// existing row, its id, plays, dates and everything else stay as they are.
///
/// `codec` NULL means "not read yet": the next full scan fills these columns for such
/// rows in the background (`scan::fill_details`), touching nothing else, so the tracks
/// are not re-read as changed (unlike schema 3's `mtime = -1`, which re-read every file
/// and with it re-made every album's art). "" means read, format not known.
const SCHEMA_V7: &str = r#"
ALTER TABLE tracks ADD COLUMN codec TEXT;
ALTER TABLE tracks ADD COLUMN sample_rate INTEGER;
ALTER TABLE tracks ADD COLUMN bit_depth INTEGER;
ALTER TABLE tracks ADD COLUMN bitrate INTEGER;
ALTER TABLE tracks ADD COLUMN channels INTEGER;
"#;

/// Schema 8: several artists and genres per track and album, and what each track's own tags
/// say about its album, so album values can be worked out from all of an album's tracks.
///
/// - `artists`, `track_artists`, `album_artists`, `genres`, `track_genres`, `album_genres`:
///   one row per credit (`relations.rs`).
/// - `track_tags`, one row per track: the album fields as its file tags them (`album_title`,
///   `album_artist` with `album_artist_mbids`, `album_artist_sort`, `artist_sort`, the three
///   dates, `label`, `release_type`, `mb_album_id`, `compilation`), and `group_folder`, the
///   folder its album is grouped in (`scan::group_folder`). Only the scanner reads them: a
///   table of their own keeps `tracks`, which every list reads, as narrow as it was.
/// - `tracks.from_name`: what the scanner took from the file's name for want of a tag (1
///   the title, 2 the track number, 4 the disc number; `tags::NameUse`).
/// - `tracks.tags_read`: the scanner version that read the file's tags (8). Rows from
///   before have 0: the next full scan re-reads their tags in the background
///   (`scan::fill_tags`), a batch at a time, keeping every id, date and favourite.
/// - On `albums`, `label_key` (the label's `norm`, for the label shelf and index),
///   `compilation` (grouped as one "Various Artists" album) and `sort_artist`, what the
///   Albums page sorts "by artist" by: the album's ALBUMARTISTSORT, else its first album
///   artist's sort name (kept by `relations::settle_artist`), else NULL (its album artist).
///
/// The migration (`fill_v8`) fills all of it from what the database already has, so
/// nothing looks emptier after upgrading: each track's album fields from its album, its
/// artists split from its artist as the artist page always split it (";", " / ", "feat."),
/// its genres from its genre, each album's artists and genres likewise. No album changes
/// identity here, no file is read.
const SCHEMA_V8: &str = r#"
ALTER TABLE tracks ADD COLUMN tags_read INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tracks ADD COLUMN from_name INTEGER NOT NULL DEFAULT 0;
CREATE TABLE track_tags (
    track_id            INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    -- The folder its album is grouped in (scan::group_folder), lower case.
    group_folder        TEXT,
    -- As the file tags them; NULL: not tagged.
    album_title         TEXT,
    album_artist        TEXT,
    album_artist_mbids  TEXT,
    album_artist_sort   TEXT,
    artist_sort         TEXT,
    date                TEXT,
    original_date       TEXT,
    release_date        TEXT,
    label               TEXT,
    release_type        TEXT,
    mb_album_id         TEXT,
    compilation         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX track_tags_group ON track_tags(group_folder);
ALTER TABLE albums ADD COLUMN label_key TEXT;
ALTER TABLE albums ADD COLUMN compilation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE albums ADD COLUMN sort_artist TEXT;
CREATE INDEX albums_label ON albums(label_key);

CREATE TABLE artists (
    id          INTEGER PRIMARY KEY,
    -- "mb:<MusicBrainz artist id>" or "n:<name, names::norm>".
    key         TEXT NOT NULL UNIQUE,
    -- names::norm of `name`, to find an artist by name.
    norm        TEXT NOT NULL,
    -- The most common spelling among its credits.
    name        TEXT NOT NULL,
    sort_name   TEXT
);
CREATE INDEX artists_norm ON artists(norm);
CREATE TABLE track_artists (
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL REFERENCES artists(id),
    -- As credited here, and its norm.
    name        TEXT NOT NULL,
    norm        TEXT NOT NULL,
    mbid        TEXT,
    featured    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, position)
) WITHOUT ROWID;
CREATE INDEX track_artists_artist ON track_artists(artist_id);
CREATE INDEX track_artists_norm ON track_artists(norm);
CREATE TABLE album_artists (
    album_id    INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL REFERENCES artists(id),
    name        TEXT NOT NULL,
    norm        TEXT NOT NULL,
    mbid        TEXT,
    PRIMARY KEY (album_id, position)
) WITHOUT ROWID;
CREATE INDEX album_artists_artist ON album_artists(artist_id);
CREATE INDEX album_artists_norm ON album_artists(norm);
CREATE TABLE genres (
    id          INTEGER PRIMARY KEY,
    -- names::norm of the name.
    key         TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL
);
CREATE TABLE track_genres (
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    genre_id    INTEGER NOT NULL REFERENCES genres(id),
    name        TEXT NOT NULL,
    PRIMARY KEY (track_id, position)
) WITHOUT ROWID;
CREATE INDEX track_genres_genre ON track_genres(genre_id);
CREATE TABLE album_genres (
    album_id    INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    genre_id    INTEGER NOT NULL REFERENCES genres(id),
    PRIMARY KEY (album_id, position)
) WITHOUT ROWID;
CREATE INDEX album_genres_genre ON album_genres(genre_id);

INSERT INTO track_tags (track_id, album_title, album_artist, album_artist_sort, date, original_date, release_date, label, release_type, mb_album_id)
SELECT t.id, a.title, a.album_artist, a.album_artist_sort, a.date, a.original_date, a.release_date, a.label, a.release_type,
       CASE WHEN a.key LIKE 'mb:%' THEN substr(a.key, 4) END
FROM tracks t JOIN albums a ON a.id = t.album_id;
-- An album keyed without an album title (no ALBUM tag) or without any artist: say so, as
-- the tracks' tags did, so grouping them again makes the same key.
UPDATE track_tags SET album_title = NULL
WHERE track_id IN (SELECT t.id FROM tracks t JOIN albums a ON a.id = t.album_id
                   WHERE a.key LIKE 't:%' AND rtrim(a.key, replace(a.key, '|', '')) LIKE '%||');
UPDATE track_tags SET album_artist = NULL
WHERE track_id IN (SELECT t.id FROM tracks t JOIN albums a ON a.id = t.album_id WHERE a.key LIKE 't:|%');
"#;

/// Schema 8's fill, in the migration's transaction: what SQL alone cannot do (`norm` is
/// Unicode-aware, the group folder needs the path's parts).
fn fill_v8(conn: &Connection) -> rusqlite::Result<()> {
    let tracks: Vec<(i64, String)> = conn.prepare("SELECT id, path FROM tracks")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut folder = conn.prepare("UPDATE track_tags SET group_folder = ?2 WHERE track_id = ?1")?;
    for (id, path) in &tracks {
        folder.execute(params![id, crate::scan::group_folder(std::path::Path::new(path)).to_string_lossy().to_lowercase()])?;
    }
    crate::relations::rebuild_from_strings(conn)
}

/// Schema 9: smart playlists (`smart.rs`).
/// - `playlists.rules`: NULL for an ordinary playlist (its tracks are its `playlist_items`);
///   for a smart one, its rule set as JSON (`smart::Smart`, versioned by its own `v`), and no
///   items: what it holds is worked out from the rules whenever it is shown. Smart and
///   ordinary playlists share ids, so a home shelf or a history entry naming one works for
///   both, and renaming and deleting are the same.
/// - `plays_track_time` (track, time) in place of `plays_track` (track), which it covers: a
///   track's last play and its plays since a date are then read from the index alone, as
///   rules on last played and on plays in a recent period ask for every track.
///
/// Only adds a column (NULL: every existing playlist stays an ordinary one) and swaps an
/// index; no row changes.
const SCHEMA_V9: &str = r#"
ALTER TABLE playlists ADD COLUMN rules TEXT;
CREATE INDEX plays_track_time ON plays(track_id, played_at);
DROP INDEX plays_track;
"#;

/// Schema 10: a full scan that has written part of the library and not finished (quit, a crash,
/// the PC asleep, the drive gone). The scan writes a batch at a time (`scan::batch`); this row
/// goes in with its first batch and out with its last, so the next scan knows it picks one up
/// ("resuming") and whether that one began a first import (`first_import`: its new albums are
/// dated by their files, `scan::SyncCtx::dated`). Only adds a table; no row changes.
const SCHEMA_V10: &str = r#"
CREATE TABLE scan_state (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    started_at   INTEGER NOT NULL,
    first_import INTEGER NOT NULL DEFAULT 0
);
"#;

/// Schema 11: a Navidrome (Subsonic / OpenSubsonic) server as a library source beside the music
/// folders (`remote.rs` has the whole design). One row per track and per album whatever its
/// sources, so every query, shelf, search and smart playlist reads them as it reads local ones.
/// - `tracks.remote_id`: the server's song id; NULL for a track the server does not have. A
///   track with a local file keeps its path; one the server alone has gets a path no file can
///   have (`remote::song_path`, `navidrome:song/<id>`). At most one track per song.
/// - `albums.remote_id`, `albums.remote_art`: the server album it shows, and that album's
///   cover art id. A server album with no local copy has a folder no folder can have
///   (`navidrome:album/<id>`) and the key `mb:<release id>` (the same a local copy keys by) or
///   `nd:<album id>`.
/// - `track_tags.mb_track_id`: the MusicBrainz recording id as tagged (MUSICBRAINZ_TRACKID),
///   which a server track is matched by. NULL until a file is next read (nothing is re-read
///   for it: numbers and titles match without it).
/// - `remote_server`: the one server (its identity, URL and user, so that another server's
///   catalog is never mixed in), what it said of its own last scan, the last complete sync.
/// - `remote_albums`, `remote_songs`: the server's catalog as the last sync saw it (each
///   entry as JSON, its hash to tell a change, and what lookups need as columns), so a track
///   whose file goes away becomes the server's again, and a file that comes back is matched
///   again, with no request to the server.
/// - `remote_pending`: albums a scan wrote since the last `remote::reconcile`.
///
/// Only adds columns and tables: every existing row stays as it was, nothing is re-read.
const SCHEMA_V11: &str = r#"
ALTER TABLE tracks ADD COLUMN remote_id TEXT;
CREATE UNIQUE INDEX tracks_remote ON tracks(remote_id) WHERE remote_id IS NOT NULL;
ALTER TABLE albums ADD COLUMN remote_id TEXT;
ALTER TABLE albums ADD COLUMN remote_art TEXT;
CREATE UNIQUE INDEX albums_remote ON albums(remote_id) WHERE remote_id IS NOT NULL;
ALTER TABLE track_tags ADD COLUMN mb_track_id TEXT;
CREATE TABLE remote_server (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    -- Who it is: the server's URL and the user, as the app words them.
    identity     TEXT NOT NULL,
    connected_at INTEGER NOT NULL,
    -- The server's own last scan, as it said it (getScanStatus), at the last sync.
    server_scan  TEXT,
    -- Unix milliseconds: the last sync that saw the whole catalog, and a sync under way.
    synced_at    INTEGER,
    sync_started INTEGER
);
CREATE TABLE remote_albums (
    id           TEXT PRIMARY KEY,
    hash         INTEGER NOT NULL,
    -- The album artist's and title's identity (`remote::match_key`), and its MusicBrainz
    -- release id (lower case): how a local copy is looked for.
    match_key    TEXT NOT NULL,
    mb_album_id  TEXT,
    data         TEXT NOT NULL
);
CREATE INDEX remote_albums_match ON remote_albums(match_key);
CREATE INDEX remote_albums_mb ON remote_albums(mb_album_id);
CREATE TABLE remote_songs (
    id           TEXT PRIMARY KEY,
    album_id     TEXT NOT NULL,
    hash         INTEGER NOT NULL,
    -- What its server-only track was last written from (`remote::placement`; NULL: none, or
    -- matched to a file).
    placed       INTEGER,
    suffix       TEXT,
    content_type TEXT,
    size         INTEGER,
    data         TEXT NOT NULL
);
CREATE INDEX remote_songs_album ON remote_songs(album_id);
CREATE TABLE remote_pending (
    album_id     INTEGER PRIMARY KEY
);
"#;

/// Schema 12: playlist entries looked up by track. A track removed (a scan's prune, a folder
/// removed from the library) takes its playlist entries with it (`ON DELETE CASCADE`), and
/// without this every one of them read all of `playlist_items`. Only adds an index.
const SCHEMA_V12: &str = r#"
CREATE INDEX playlist_items_track ON playlist_items(track_id);
"#;

/// Schema 13: favourites, ratings and plays written back to the server (`library::marks`).
/// - `remote_marks`: per server song, the favourite and the rating last agreed with the server
///   (NULL: never agreed, as before this schema; the first sync then takes the union of the
///   stars and keeps a local rating), when each was agreed, the server's star time at the
///   agreement (Unix ms) when it said one, and when the app last changed each itself (Unix ms;
///   a change made in the app since the agreement is the user's, even when it came back to the
///   agreed value). Keyed by the server's song id, so it follows the song through a merge with
///   a file and back.
/// - `remote_plays`: plays counted of tracks the server has, not yet reported to it (oldest
///   first): the song and when it was played (Unix ms).
///
/// Only adds tables: every existing row stays as it was.
const SCHEMA_V13: &str = r#"
CREATE TABLE remote_marks (
    song_id        TEXT PRIMARY KEY,
    starred        INTEGER,
    starred_at     INTEGER,
    star_agreed    INTEGER,
    star_local     INTEGER,
    rating         INTEGER CHECK (rating BETWEEN 0 AND 5),
    rating_agreed  INTEGER,
    rating_local   INTEGER
);
CREATE TABLE remote_plays (
    id           INTEGER PRIMARY KEY,
    song_id      TEXT NOT NULL,
    played_at    INTEGER NOT NULL
);
"#;

/// Schema 4: the play queue, so a restart resumes where it left off (`session.rs`).
/// Only adds tables; nothing existing changes.
/// - `queue_items`: the queue in order. `original` is the item's place in the order from
///   before shuffle while shuffle is on (NULL otherwise, and for items added while
///   shuffled). A track that leaves the library leaves the queue with it.
/// - `queue_state`: one row, the current item (a `queue_items.position`) and how far
///   into it playback was, in seconds.
const SCHEMA_V4: &str = r#"
CREATE TABLE queue_items (
    position    INTEGER PRIMARY KEY,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    original    INTEGER
);
CREATE INDEX queue_items_track ON queue_items(track_id);
CREATE TABLE queue_state (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    current     INTEGER,
    seconds     REAL NOT NULL DEFAULT 0
);
"#;

/// Beside the database: an earlier run's `check` found it damaged. `open` refuses it as
/// damaged until it is set aside (`set_aside`) or the marker removed.
pub fn damaged_marker(path: &Path) -> std::path::PathBuf {
    path.with_extension("db.damaged")
}

/// SQLite's own consistency check of the whole file (`PRAGMA quick_check`), on a read-only
/// connection of its own: `Err` with what it found when the file is damaged inside (a page
/// of it overwritten, a disk error) though its header still reads. Reads the whole file: run
/// it off the UI thread.
pub fn check(path: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(10));
    let mut st = conn.prepare("PRAGMA quick_check(3)").map_err(|e| e.to_string())?;
    let found: Vec<String> = st.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if found.len() == 1 && found[0] == "ok" { Ok(()) } else { Err(found.join("; ")) }
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if damaged_marker(path).exists() && path.exists() {
        let why = std::fs::read_to_string(damaged_marker(path)).unwrap_or_default();
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some(format!("an earlier check found it damaged: {}", why.trim())),
        ));
    }
    let conn = Connection::open(path)?;
    // WAL lets the UI read while the scanner writes from its own connection.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    // Waits for another connection's write at a fine grain, and says so (`let_waiting_in`).
    set_wait(&conn, Wait::Long)?;
    crate::fold::register(&conn)?;
    backup_before_migration(&conn, path);
    if conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap_or(0) < SCHEMA_VERSION {
        log::info!("library.db: migrating to schema {SCHEMA_VERSION}");
    }
    migrate(&conn)?;
    Ok(conn)
}

/// A connection of this process is waiting for another's write transaction to end (`busy`).
static WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SQLite's busy handler on every connection `open` makes: try again after 1 ms (10 ms after
/// the first hundred tries), for about `MS` milliseconds, and say a writer is waiting. SQLite's
/// own handler waits longer and longer between tries (up to 100 ms), and a scan writing one
/// batch after another could keep taking the database in those gaps: the app's own write (a
/// play counted, a rating, on the UI thread) would wait on and on.
fn busy<const MS: i32>(n: i32) -> bool {
    WAITING.store(true, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    TRIED.with(|t| t.set(t.get() + 1));
    std::thread::sleep(std::time::Duration::from_millis(if n < 100 { 1 } else { 10 }));
    n < if MS <= 100 { MS } else { 100 + (MS - 100) / 10 }
}

#[cfg(test)]
thread_local! {
    /// How many times `busy` waited on this thread.
    static TRIED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `busy`, as SQLite calls it.
unsafe extern "C" fn busy_handler<const MS: i32>(_: *mut std::ffi::c_void, n: std::ffi::c_int) -> std::ffi::c_int {
    std::ffi::c_int::from(busy::<MS>(n))
}

/// How long a connection waits for another's write (`set_wait`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// About 30 ms: for a write that is tried again later rather than keep the UI waiting on a
    /// scan (the session's position).
    Brief,
    /// About 5 s: what `open` sets.
    Long,
}

/// How long `conn` (one `open` made) waits for another connection's write, its busy handler
/// kept (`busy`). Not `Connection::busy_timeout`: that replaces the handler with SQLite's own,
/// which does not say it waits, so a scan would no longer let this connection's writes in
/// (`let_waiting_in`).
pub fn set_wait(conn: &Connection, wait: Wait) -> rusqlite::Result<()> {
    let handler = match wait {
        Wait::Brief => busy_handler::<30>,
        Wait::Long => busy_handler::<5000>,
    };
    // Through SQLite itself, as `Connection::busy_handler` does, which takes the connection
    // `&mut` (the app's is shared). SAFETY: the handle is `conn`'s own, open while it lives; the
    // handler is a plain function that reads no user data and cannot panic.
    let rc = unsafe { rusqlite::ffi::sqlite3_busy_handler(conn.handle(), Some(handler), std::ptr::null_mut()) };
    if rc == rusqlite::ffi::SQLITE_OK { Ok(()) } else { Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rc), None)) }
}

/// A write transaction on a shared connection (`Connection::unchecked_transaction`), begun
/// IMMEDIATE: it takes the write lock at once, waiting for another connection's write through the
/// busy handler. Every write transaction is begun so (`transaction_with_behavior(Immediate)` on a
/// `&mut Connection`): one begun deferred whose first statement reads holds a snapshot, and its
/// first write then fails at once with "database is locked" while another connection writes, the
/// busy handler never asked (waiting could not help: the snapshot would be stale).
pub fn write(conn: &Connection) -> rusqlite::Result<rusqlite::Transaction<'_>> {
    rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
}

/// For a run of write transactions (a scan's batches), before each: if another connection of this
/// process has been waiting to write meanwhile, give it the database first (it tries every
/// millisecond: a few are plenty).
pub fn let_waiting_in() {
    if WAITING.swap(false, std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// A migration is one-way: older builds refuse a newer schema. Before migrating an
/// existing database, keep a copy beside it as `library.v<old>.bak.db` (once per old
/// version), so going back to an older build means renaming one file. `VACUUM INTO`
/// writes a consistent copy including what is still in the WAL.
fn backup_before_migration(conn: &Connection, path: &Path) {
    let Ok(version) = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)) else { return };
    if !(1..SCHEMA_VERSION).contains(&version) {
        return;
    }
    let backup = path.with_extension(format!("v{version}.bak.db"));
    if backup.exists() {
        return;
    }
    // Written under a temporary name and renamed when whole: a copy cut short by a crash or a
    // kill must never be taken for the backup (the next start would skip making one).
    let partial = path.with_extension(format!("v{version}.bak.partial"));
    let _ = std::fs::remove_file(&partial);
    let target = partial.to_string_lossy().replace('\'', "''");
    match conn.execute_batch(&format!("VACUUM INTO '{target}'")).map_err(|e| e.to_string()).and_then(|()| {
        if let Ok(f) = std::fs::File::open(&partial) {
            let _ = f.sync_all();
        }
        std::fs::rename(&partial, &backup).map_err(|e| e.to_string())
    }) {
        Ok(()) => log::info!("library.db is schema {version}; kept a copy at {}", backup.display()),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            log::warn!("could not back up library.db before migrating: {e}");
        }
    }
}

/// Why `open` failed, in the terms the user can act on (`main` words each one).
#[derive(Clone, Debug, PartialEq)]
pub enum OpenProblem {
    /// A newer build's schema: this build cannot read it.
    Newer { found: i64 },
    /// Not a database, or damaged inside.
    Damaged(String),
    /// Another process holds it (a second copy of the app on the same data folder, a backup
    /// or sync tool).
    Busy,
    /// Read-only file, folder or disk; no permission.
    ReadOnly(String),
    /// The disk is full.
    Full,
    Other(String),
}

const NEWER: &str = "library.db is schema ";

/// What `e`, from `open`, means for the user.
pub fn problem(e: &rusqlite::Error) -> OpenProblem {
    use rusqlite::ErrorCode as C;
    if let rusqlite::Error::InvalidParameterName(m) = e
        && let Some(rest) = m.strip_prefix(NEWER)
    {
        return OpenProblem::Newer { found: rest.split(',').next().and_then(|n| n.trim().parse().ok()).unwrap_or(0) };
    }
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => {
            let text = msg.clone().unwrap_or_else(|| f.to_string());
            match f.code {
                C::NotADatabase | C::DatabaseCorrupt => OpenProblem::Damaged(text),
                C::DatabaseBusy | C::DatabaseLocked => OpenProblem::Busy,
                C::ReadOnly | C::PermissionDenied | C::CannotOpen => OpenProblem::ReadOnly(text),
                C::DiskFull => OpenProblem::Full,
                _ => OpenProblem::Other(text),
            }
        }
        other => OpenProblem::Other(other.to_string()),
    }
}

/// The copies kept before migrations (`library.v<n>.bak.db`) beside `path`, newest schema first.
pub fn backups(path: &Path) -> Vec<std::path::PathBuf> {
    let mut found: Vec<(i64, std::path::PathBuf)> = (1..SCHEMA_VERSION + 50)
        .map(|v| (v, path.with_extension(format!("v{v}.bak.db"))))
        .filter(|(_, p)| p.exists())
        .collect();
    found.sort_by_key(|(v, _)| -v);
    found.into_iter().map(|(_, p)| p).collect()
}

/// Move a damaged database (and its WAL and shared-memory files) aside as
/// `library.damaged-<unix seconds>.db`, so the next `open` starts a new library. The damaged
/// file is kept, never deleted. Returns where it went.
pub fn set_aside(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let aside = path.with_extension(format!("damaged-{}.db", now()));
    std::fs::rename(path, &aside)?;
    let _ = std::fs::remove_file(damaged_marker(path));
    for ext in ["db-wal", "db-shm"] {
        let side = path.with_extension(ext);
        if side.exists() {
            let _ = std::fs::rename(&side, aside.with_extension(format!("db{}", &ext[2..])));
        }
    }
    Ok(aside)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    crate::fold::register(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, SCHEMA_VERSION)
}

/// Each step runs in its own transaction, so a failed step leaves the database at the
/// version before it, data intact.
pub(crate) fn migrate_to(conn: &Connection, target: i64) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidParameterName(format!("{NEWER}{version}, this build knows {SCHEMA_VERSION}")));
    }
    // Each step's SQL, and what it fills in Rust in the same transaction.
    type Fill = fn(&Connection) -> rusqlite::Result<()>;
    let steps: [(&str, Option<Fill>); 13] = [
        (SCHEMA_V1, None),
        (SCHEMA_V2, None),
        (SCHEMA_V3, None),
        (SCHEMA_V4, None),
        (SCHEMA_V5, None),
        (SCHEMA_V6, None),
        (SCHEMA_V7, None),
        (SCHEMA_V8, Some(fill_v8)),
        (SCHEMA_V9, None),
        (SCHEMA_V10, None),
        (SCHEMA_V11, None),
        (SCHEMA_V12, None),
        (SCHEMA_V13, None),
    ];
    for (i, (sql, fill)) in steps.iter().enumerate() {
        let to = i as i64 + 1;
        if version < to && to <= target {
            let step = || -> rusqlite::Result<()> {
                conn.execute_batch(&format!("BEGIN IMMEDIATE; {sql}"))?;
                if let Some(fill) = fill {
                    fill(conn)?;
                }
                conn.execute_batch(&format!("PRAGMA user_version = {to}; COMMIT;"))
            };
            step().inspect_err(|_| {
                let _ = conn.execute_batch("ROLLBACK;");
            })?;
        }
    }
    Ok(())
}

/// Unix milliseconds, for `albums.added_at` and `first_seen`.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema-3 database with plays, favourites and a playlist, as an existing user's
    /// library.db is, migrates to the current schema with all of it intact.
    #[test]
    fn schema_3_migrates_with_its_data() {
        let tmp = crate::testdir::TestDir::new("migrate");
        let path = tmp.join("library.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            migrate_to(&conn, 3).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen, added_source, original_date)
                     VALUES (1, 'mb:x', 'Album', 'Artist', 'M:/a', 5000, 4000, 'navidrome', '2001');
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'Artist', 1, 1, 200.5, 123, 456, 7),
                            (11, 'M:/a/02.flac', 1, 'Two', 'Artist', 1, 2, 180.0, 124, 457, 8);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000);
                 INSERT INTO favourite_albums VALUES (1, 9);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO playlists VALUES (1, 'Mix', 11);
                 INSERT INTO playlist_items VALUES (1, 0, 11), (1, 1, 10);",
            )
            .unwrap();
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 3);
        }
        let backup = path.with_extension("v3.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // The schema-3 copy is kept beside it, with its data.
        {
            let old = Connection::open(&backup).unwrap();
            let v: i64 = old.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 3);
            let plays: i64 = old.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap();
            assert_eq!(plays, 3);
        }
        let _ = std::fs::remove_file(&backup);
        let q = |sql: &str| -> String {
            let mut st = conn.prepare(sql).unwrap();
            let n = st.column_count();
            let rows: Vec<String> = st
                .query_map([], |r| {
                    Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(","))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.join(" | ")
        };
        assert_eq!(q("SELECT COUNT(*) FROM plays WHERE track_id = 10"), "Integer(2)");
        assert_eq!(q("SELECT track_id, played_at FROM plays ORDER BY played_at"), "Integer(10),Integer(1000) | Integer(10),Integer(2000) | Integer(11),Integer(3000)");
        assert_eq!(q("SELECT * FROM favourite_albums"), "Integer(1),Integer(9)");
        assert_eq!(q("SELECT * FROM favourite_tracks"), "Integer(11),Integer(10)");
        assert_eq!(q("SELECT position, track_id FROM playlist_items ORDER BY position"), "Integer(0),Integer(11) | Integer(1),Integer(10)");
        assert_eq!(q("SELECT name FROM playlists"), "Text(\"Mix\")");
        assert_eq!(
            q("SELECT added_at, first_seen, added_source, original_date FROM albums"),
            "Integer(5000),Integer(4000),Text(\"navidrome\"),Text(\"2001\")"
        );
        assert_eq!(q("SELECT id, path, mtime, size, duration FROM tracks ORDER BY id"), "Integer(10),Text(\"M:/a/01.flac\"),Integer(123),Integer(456),Real(200.5) | Integer(11),Text(\"M:/a/02.flac\"),Integer(124),Integer(457),Real(180.0)");
        // The new tables exist and are empty.
        assert_eq!(q("SELECT COUNT(*) FROM queue_items"), "Integer(0)");
        assert_eq!(q("SELECT COUNT(*) FROM queue_state"), "Integer(0)");
        // Opening again changes nothing.
        drop(conn);
        let conn = open(&path).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 3);
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// A schema-3 library of realistic size: 3,000 albums, 30,000 tracks with several artists
    /// and genres in their credits, 60,000 plays, favourites and playlists.
    fn schema_3_library(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        migrate_to(&conn, 3).unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for a in 0..3_000i64 {
            conn.execute(
                "INSERT INTO albums (id, key, title, album_artist, genre, folder, added_at, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![a + 1, format!("k{a}"), format!("Album {a}"), format!("Artist {}", a % 400), if a % 3 == 0 { "Rock; Pop" } else { "Jazz" }, format!("M:/m/{a}"), 1_000 + a],
            )
            .unwrap();
            for t in 0..10i64 {
                let id = a * 10 + t + 1;
                let artist = if t % 4 == 0 { format!("Artist {} feat. Guest {}", a % 400, t) } else { format!("Artist {}", a % 400) };
                conn.execute(
                    "INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 200.0, ?7, 1, 2, 3)",
                    params![id, format!("M:/m/{a}/{t:02}.flac"), a + 1, format!("Track {t}"), artist, t + 1, if t % 2 == 0 { "Rock / Pop" } else { "Jazz" }],
                )
                .unwrap();
                if id % 5 == 0 {
                    conn.execute("INSERT INTO plays VALUES (?1, ?2), (?1, ?3), (?1, ?4)", params![id, id * 7, id * 7 + 1, id * 7 + 2]).unwrap();
                }
                if id % 50 == 0 {
                    conn.execute("INSERT INTO favourite_tracks VALUES (?1, 1)", [id]).unwrap();
                }
            }
        }
        conn.execute_batch("INSERT INTO playlists VALUES (1, 'Mix', 1); INSERT INTO playlist_items SELECT 1, id, id FROM tracks WHERE id % 97 = 0; COMMIT;").unwrap();
    }

    fn counts(conn: &Connection) -> (i64, i64, i64, i64, i64) {
        let n = |t: &str| conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap();
        (n("albums"), n("tracks"), n("plays"), n("favourite_tracks"), n("playlist_items"))
    }

    /// The child of `a_migration_killed_at_any_moment_loses_nothing`: open (and so migrate) the
    /// database it is given. Run by that test only.
    #[test]
    #[ignore]
    fn migration_child() {
        if let Some(p) = std::env::var_os("MP_MIGRATE_CHILD") {
            let _ = open(Path::new(&p));
        }
    }

    /// The app killed while it migrates a schema-3 library (at every moment from the backup's
    /// copy through the last step): the next start finishes the migration with every album,
    /// track, play, favourite and playlist entry, and the backup, when there is one, is whole.
    #[test]
    fn a_migration_killed_at_any_moment_loses_nothing() {
        let dir = crate::testdir::TestDir::new("migrate-kill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let template = dir.join("template.db");
        schema_3_library(&template);
        let want = counts(&Connection::open(&template).unwrap());
        assert_eq!(want.1, 30_000);
        // How long an uninterrupted migration takes, to spread the kills over it.
        let path = dir.join("library.db");
        std::fs::copy(&template, &path).unwrap();
        let t0 = std::time::Instant::now();
        drop(open(&path).unwrap());
        let whole = t0.elapsed();
        let mut killed_midway = 0;
        for i in 0..16u32 {
            for f in std::fs::read_dir(&dir).unwrap().flatten() {
                if f.path() != template {
                    let _ = std::fs::remove_file(f.path());
                }
            }
            std::fs::copy(&template, &path).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "db::tests::migration_child", "--ignored", "--quiet"])
                .env("MP_MIGRATE_CHILD", &path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // From before the copy starts to past the end (the process takes a while to start).
            std::thread::sleep(whole.mul_f64(f64::from(i) / 10.0) + std::time::Duration::from_millis(15));
            let done = child.try_wait().unwrap().is_some();
            let _ = child.kill();
            let _ = child.wait();
            let v: i64 = Connection::open(&path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            if !done && v < SCHEMA_VERSION {
                killed_midway += 1;
            }
            assert!(!path.with_extension("v3.bak.partial").exists() || !path.with_extension("v3.bak.db").exists() || v >= 3);
            let conn = open(&path).unwrap_or_else(|e| panic!("kill {i} (at schema {v}): the next start cannot open the library: {e}"));
            let got: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(got, SCHEMA_VERSION);
            assert_eq!(counts(&conn), want, "kill {i} at schema {v}");
            let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
            assert_eq!(ok, "ok");
            let backup = path.with_extension("v3.bak.db");
            if backup.exists() {
                let old = Connection::open(&backup).unwrap();
                let ok: String = old.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
                assert_eq!(ok, "ok", "kill {i}: the backup is damaged");
                assert_eq!(counts(&old), want, "kill {i}: the backup is not whole");
            }
        }
        eprintln!("migration of 30,000 tracks: {whole:?}; {killed_midway} of 16 kills landed before it finished");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_problems_are_told_apart() {
        let dir = crate::testdir::TestDir::new("open-problems");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Not a database at all.
        let junk = dir.join("junk.db");
        std::fs::write(&junk, vec![0x5a; 8192]).unwrap();
        let e = open(&junk).unwrap_err();
        assert!(matches!(problem(&e), OpenProblem::Damaged(_)), "{e:?}");
        // A newer build's.
        let newer = dir.join("newer.db");
        Connection::open(&newer).unwrap().execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 3)).unwrap();
        let e = open(&newer).unwrap_err();
        assert_eq!(problem(&e), OpenProblem::Newer { found: SCHEMA_VERSION + 3 });
        // A damaged one is set aside whole, and a new one opens in its place.
        let aside = set_aside(&junk).unwrap();
        assert_eq!(std::fs::read(&aside).unwrap(), vec![0x5a; 8192]);
        assert!(open(&junk).is_ok());
        // Held by another process (here another connection with a write lock).
        let held = dir.join("held.db");
        drop(open(&held).unwrap());
        let other = Connection::open(&held).unwrap();
        other.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;").unwrap();
        let c = Connection::open(&held).unwrap();
        c.busy_timeout(std::time::Duration::from_millis(50)).unwrap();
        let e = c.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap_err();
        assert_eq!(problem(&e), OpenProblem::Busy, "{e:?}");
        drop((c, other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A connection told to wait briefly, then long again (as the session's save does) still
    /// waits through `open`'s handler, which says it waits (`let_waiting_in`): the scan gives
    /// way to it.
    #[test]
    fn a_wait_set_and_reset_keeps_the_busy_handler() {
        let tmp = crate::testdir::TestDir::new("busy");
        let path = tmp.join("library.db");
        let _ = std::fs::remove_file(&path);
        let ui = open(&path).unwrap();
        let scan = open(&path).unwrap();
        let tried = || TRIED.with(std::cell::Cell::get);
        // Briefly: gives up soon while the scan holds the database, having waited by `busy`.
        set_wait(&ui, Wait::Brief).unwrap();
        scan.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (before, t0) = (tried(), std::time::Instant::now());
        let e = ui.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap_err();
        assert_eq!(problem(&e), OpenProblem::Busy, "{e:?}");
        assert!(tried() > before, "waited through the handler");
        assert!(t0.elapsed() < std::time::Duration::from_secs(2), "{:?}", t0.elapsed());
        // Back to long: waits for the scan's write to end, through the handler again.
        set_wait(&ui, Wait::Long).unwrap();
        let before = tried();
        std::thread::scope(|s| {
            s.spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                scan.execute_batch("COMMIT").unwrap();
            });
            ui.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap();
        });
        assert!(tried() > before + 10, "waited through the handler: {} tries", tried() - before);
        drop(ui);
        for ext in ["db", "db-wal", "db-shm"] {
            let _ = std::fs::remove_file(path.with_extension(ext));
        }
    }

    /// A write that reads first (rewriting a playlist reads what it holds now) waits for another
    /// connection's write like any other, through the busy handler: begun deferred, its first
    /// write found the database locked and failed at once, the handler never asked.
    #[test]
    fn a_write_that_reads_first_waits_for_another_write() {
        let dir = crate::testdir::TestDir::new("busy-read-first");
        let path = dir.join("library.db");
        let ui = open(&path).unwrap();
        let scan = open(&path).unwrap();
        let pl = crate::query::create_playlist(&ui, "mix").unwrap();
        scan.execute_batch("BEGIN IMMEDIATE; INSERT INTO playlists (name, created_at) VALUES ('other', 0);").unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            scan.execute_batch("COMMIT").unwrap();
        });
        let done = crate::query::rewrite_playlist(&ui, pl, &[], &[]);
        writer.join().unwrap();
        assert!(done.unwrap());
        let n: i64 = ui.query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    /// A schema-5 database (the last release's) gains ratings with everything else intact,
    /// and its copy is kept as library.v5.bak.db.
    #[test]
    fn schema_5_migrates_to_ratings_with_its_data() {
        let tmp = crate::testdir::TestDir::new("migrate5");
        let path = tmp.join("library.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            migrate_to(&conn, 5).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (1, 'k', 'A', 'B', 'M:/a', 5, 5);
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'B', 1, 1, 200.5, 123, 456, 7);
                 INSERT INTO plays VALUES (10, 1000);
                 INSERT INTO favourite_tracks VALUES (10, 10);
                 INSERT INTO queue_items VALUES (0, 10, NULL);
                 INSERT INTO queue_state VALUES (1, 0, 12.5);",
            )
            .unwrap();
        }
        let backup = path.with_extension("v5.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-5 database is kept");
        let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(one("SELECT COUNT(*) FROM plays"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM favourite_tracks"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM queue_items"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM track_ratings"), 0);
        crate::query::set_rating(&conn, 10, 4).unwrap();
        assert_eq!(crate::query::rating(&conn, 10).unwrap(), 4);
        assert!(crate::query::set_rating(&conn, 10, 9).is_err() || crate::query::rating(&conn, 10).unwrap() == 5, "clamped or refused");
        crate::query::set_rating(&conn, 10, 0).unwrap();
        assert_eq!(crate::query::rating(&conn, 10).unwrap(), 0, "0 clears");
        crate::query::set_rating(&conn, 10, 3).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 10", []).unwrap();
        assert_eq!(one("SELECT COUNT(*) FROM track_ratings"), 0, "goes with its track");
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// A schema-6 database (the last release's) with a year of use in it gains the
    /// technical details columns, in one step, with every row of every table as it was:
    /// ids, paths, mtimes (no re-read is forced), dates, plays, favourites, ratings,
    /// playlists, the queue. The new columns start NULL (not read yet), for the next scan
    /// to fill. Its copy is kept as library.v6.bak.db.
    #[test]
    fn schema_6_migrates_to_details_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate6");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = ["albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings"];
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY rowid")).unwrap();
            let n = st.column_count();
            st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let before: Vec<String>;
        {
            let conn = Connection::open(&path).unwrap();
            migrate_to(&conn, 6).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked)
                     VALUES (1, 'mb:x', 'Album', 'Artist', 'Artist, The', '2001-05-02', 'Rock', 'Lbl', 'album', 'M:/a', 'abc123', 5000, 4000, 'navidrome', '1999', '2001-05-02', NULL),
                            (2, 't:b|c|m:/b', 'Other', 'B', NULL, NULL, NULL, NULL, NULL, 'M:/b', NULL, 7000, 7000, NULL, NULL, NULL, '123|456');
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'Artist', 1, 1, 200.5, 'Rock', 123, 456, 7),
                            (11, 'M:/a/02.flac', 1, 'Two', 'Artist', 1, 2, 180.0, NULL, 124, 457, 8),
                            (12, 'M:/b/x.opus', 2, 'Three', 'B', NULL, NULL, 90.25, NULL, 125, 458, 9);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000), (12, 4000);
                 INSERT INTO favourite_albums VALUES (1, 9);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11), (12, 2, 12);
                 INSERT INTO playlists VALUES (1, 'Mix', 11), (2, 'Empty', 12);
                 INSERT INTO playlist_items VALUES (1, 0, 11), (1, 1, 10), (1, 2, 12);
                 INSERT INTO queue_items VALUES (0, 12, NULL), (1, 10, 0);
                 INSERT INTO queue_state VALUES (1, 1, 42.5);",
            )
            .unwrap();
            before = tables.iter().map(|t| dump(&conn, t, "*")).collect();
        }
        let backup = path.with_extension("v6.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-6 database is kept");
        // Every table as it was; for tracks, every column it had.
        let old_track_cols = "id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at";
        let old_album_cols = "id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked";
        let cols_of = |t: &str| match t {
            "tracks" => old_track_cols,
            "albums" => old_album_cols,
            "playlists" => "id, name, created_at",
            _ => "*",
        };
        for (t, b) in tables.iter().zip(&before) {
            let cols = cols_of(t);
            assert_eq!(&dump(&conn, t, cols), b, "{t} unchanged");
        }
        assert_eq!(dump(&conn, "tracks", "codec, sample_rate, bit_depth, bitrate, channels"), ["Null,Null,Null,Null,Null"; 3].join(" | "), "details not read yet");
        // The queries read the rows, details unknown.
        let t = crate::query::track(&conn, 10).unwrap().unwrap();
        assert_eq!((t.title.as_str(), t.audio), ("One", crate::audio::AudioInfo::default()));
        assert_eq!(crate::query::playlist_tracks(&conn, 1).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [11, 10, 12]);
        assert_eq!(crate::session::load_queue(&conn).unwrap().items.len(), 2);
        // Opening again changes nothing, and makes no second copy.
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert!(!backup.exists());
        for (t, b) in tables.iter().zip(&before) {
            assert_eq!(&dump(&conn, t, cols_of(t)), b);
        }
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// The last release's database (schema 7), with years of use in it, gains schema 8 in one
    /// step: every row of every table as it was (ids, paths, mtimes: no re-read is forced,
    /// dates, plays, favourites, ratings, playlists, the queue), and the artists and genres
    /// made from what it held, so the artist pages, the Artists index and the genre pages
    /// show what they showed. Its tracks are marked for the next scan to read their tags
    /// again. Its copy is kept as library.v7.bak.db.
    #[test]
    fn schema_7_migrates_to_artists_and_genres_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate7");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = ["albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings"];
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY rowid")).unwrap();
            let n = st.column_count();
            st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let old_cols = |conn: &Connection, table: &str| -> String {
            let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(", ")
        };
        let before: Vec<String>;
        let cols: Vec<String>;
        {
            let conn = Connection::open(&path).unwrap();
            migrate_to(&conn, 7).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked)
                     VALUES (1, 'mb:x', 'Album', 'The Band', 'Band, The', '2001-05-02', 'Rock', 'Lbl', 'album', 'M:/a', 'abc123', 5000, 4000, 'navidrome', '1999', '2001-05-02', NULL),
                            (2, 't:yuta imai;kasane teto|take over|m:/b', 'TAKE OVER', 'Yuta Imai;Kasane Teto', NULL, NULL, 'Vocaloid; Rock', NULL, NULL, 'M:/b', NULL, 7000, 7000, NULL, NULL, NULL, '123|456'),
                            (3, 't:host||m:/c', 'c', 'Host', NULL, '2010', NULL, '[no label]', NULL, 'M:/c', 'def', 8000, 8000, NULL, NULL, NULL, NULL);
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'The Band', 1, 1, 200.5, 'Rock', 123, 456, 7, 'FLAC', 44100, 16, 900, 2),
                            (11, 'M:/a/02.flac', 1, 'Two', 'The Band feat. Guest Singer', 1, 2, 180.0, NULL, 124, 457, 8, 'FLAC', 44100, 16, 910, 2),
                            (12, 'M:/b/x.opus', 2, 'Three', 'Yuta Imai;Kasane Teto', NULL, NULL, 90.25, 'Vocaloid; Rock', 125, 458, 9, NULL, NULL, NULL, NULL, NULL),
                            (13, 'M:/c/y.mp3', 3, 'Four', 'Host', NULL, 1, 60.0, NULL, 126, 459, 10, 'MP3', 44100, NULL, 320, 2);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000), (12, 4000);
                 INSERT INTO favourite_albums VALUES (1, 9), (2, 19);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11), (12, 2, 12);
                 INSERT INTO playlists VALUES (1, 'Mix', 11), (2, 'Empty', 12);
                 INSERT INTO playlist_items VALUES (1, 0, 11), (1, 1, 10), (1, 2, 12);
                 INSERT INTO queue_items VALUES (0, 12, NULL), (1, 10, 0);
                 INSERT INTO queue_state VALUES (1, 1, 42.5);",
            )
            .unwrap();
            before = tables.iter().map(|t| dump(&conn, t, "*")).collect();
            cols = tables.iter().map(|t| old_cols(&conn, t)).collect();
        }
        let backup = path.with_extension("v7.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-7 database is kept");
        // Every table as it was, in every column it had.
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b, "{t} unchanged");
        }
        // Each track's album fields from its album; the tags to be read again.
        assert_eq!(
            dump(
                &conn,
                "track_tags g",
                "g.track_id, g.album_title, g.album_artist, g.date, g.mb_album_id, (SELECT tags_read FROM tracks t WHERE t.id = g.track_id), (SELECT from_name FROM tracks t WHERE t.id = g.track_id)"
            ),
            "Integer(10),Text(\"Album\"),Text(\"The Band\"),Text(\"2001-05-02\"),Text(\"x\"),Integer(0),Integer(0) \
             | Integer(11),Text(\"Album\"),Text(\"The Band\"),Text(\"2001-05-02\"),Text(\"x\"),Integer(0),Integer(0) \
             | Integer(12),Text(\"TAKE OVER\"),Text(\"Yuta Imai;Kasane Teto\"),Null,Null,Integer(0),Integer(0) \
             | Integer(13),Null,Text(\"Host\"),Text(\"2010\"),Null,Integer(0),Integer(0)",
            "an album keyed without a title says so"
        );
        assert_eq!(dump(&conn, "track_tags", "group_folder"), ["Text(\"m:/a\")", "Text(\"m:/a\")", "Text(\"m:/b\")", "Text(\"m:/c\")"].join(" | "));
        assert_eq!(dump(&conn, "tracks", "COUNT(*)"), "Integer(4)", "tracks keeps its columns; the new ones are a table of their own");
        // The artist pages, the Artists index and the genres show what they showed.
        use crate::query;
        let id = |n: &str| query::find_artist(&conn, n).unwrap().unwrap().id;
        let albums = |n: &str| {
            let (own, appears) = query::artist_albums(&conn, id(n)).unwrap();
            (own.iter().map(|a| a.id).collect::<Vec<_>>(), appears.iter().map(|a| a.id).collect::<Vec<_>>())
        };
        assert_eq!(albums("The Band"), (vec![1], vec![]));
        assert_eq!(albums("Guest Singer"), (vec![], vec![1]), "a featured credit appears on the album");
        assert_eq!(albums("Kasane Teto"), (vec![2], vec![]), "each artist of a shared album owns it");
        assert_eq!(albums("Yuta Imai"), (vec![2], vec![]));
        assert_eq!(query::artist_top_tracks(&conn, id("The Band"), 10).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![10, 11], "by plays");
        let index: Vec<(String, i64)> = query::album_artists(&conn).unwrap().into_iter().map(|g| (g.name, g.albums)).collect();
        assert_eq!(index, vec![("The Band".to_owned(), 1), ("Host".into(), 1), ("Kasane Teto".into(), 1), ("Yuta Imai".into(), 1)], "by sort name: Band, The");
        let genres: Vec<(String, i64)> = query::album_groups(&conn, false).unwrap().into_iter().map(|g| (g.name, g.albums)).collect();
        assert_eq!(genres, vec![("Rock".to_owned(), 2), ("Vocaloid".into(), 1)], "a shared genre string is two genres");
        assert_eq!(query::shelf(&conn, &crate::ShelfQuery::Genre { name: "rock".into() }, 10).unwrap().len(), 2);
        assert_eq!(query::shelf(&conn, &crate::ShelfQuery::Label { name: "LBL".into() }, 10).unwrap().len(), 1);
        assert_eq!(query::search(&conn, "teto", 10).unwrap().artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Kasane Teto"]);
        // Opening again changes nothing, and makes no second copy.
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert!(!backup.exists());
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b);
        }
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// The last release's database (schema 8), with years of use in it, gains smart playlists
    /// in one step: every row of every table as it was, in every column it had (plays,
    /// favourites, ratings, playlists and their order, the queue, artists and genres), every
    /// playlist still an ordinary one with its tracks, the plays index swapped for the one
    /// that also covers time. A smart playlist can then be made beside them. Its copy is kept
    /// as library.v8.bak.db.
    #[test]
    fn schema_8_migrates_to_smart_playlists_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate8");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = [
            "albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings",
            "track_tags", "artists", "track_artists", "album_artists", "genres", "track_genres", "album_genres",
        ];
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY {cols}")).unwrap();
            let n = st.column_count();
            st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let cols_of = |conn: &Connection, table: &str| -> String {
            let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(", ")
        };
        let indexes = |conn: &Connection| -> Vec<String> {
            let mut st = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'plays' ORDER BY name").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        let (before, cols): (Vec<String>, Vec<String>);
        {
            let conn = Connection::open(&path).unwrap();
            crate::fold::register(&conn).unwrap();
            migrate_to(&conn, 8).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked, label_key, compilation, sort_artist)
                     VALUES (1, 'mb:x', 'Album', 'The Band', 'Band, The', '2001-05-02', 'Rock', 'Lbl', 'album', 'M:/a', 'abc123', 5000, 4000, 'navidrome', '1999', '2001-05-02', NULL, 'lbl', 0, 'Band, The'),
                            (2, 't:b|c|m:/b', 'Other', 'B', NULL, NULL, 'Jazz', NULL, NULL, 'M:/b', NULL, 7000, 7000, NULL, NULL, NULL, '123|456', NULL, 1, NULL);
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels, tags_read, from_name)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'The Band', 1, 1, 200.5, 'Rock', 123, 456, 7, 'FLAC', 44100, 16, 900, 2, 8, 0),
                            (11, 'M:/a/02.flac', 1, 'Two', 'The Band feat. Guest', 1, 2, 180.0, NULL, 124, 457, 8, 'FLAC', 44100, 16, 910, 2, 8, 2),
                            (12, 'M:/b/x.opus', 2, 'Three', 'B', NULL, NULL, 90.25, 'Jazz', 125, 458, 9, 'Opus', 48000, NULL, 128, 2, 0, 1);
                 INSERT INTO track_tags (track_id, group_folder, album_title, album_artist, date, compilation) VALUES (10, 'm:/a', 'Album', 'The Band', '2001', 0), (12, 'm:/b', NULL, NULL, NULL, 1);
                 INSERT INTO artists VALUES (1, 'n:the band', 'the band', 'The Band', 'Band, The'), (2, 'n:guest', 'guest', 'Guest', NULL), (3, 'n:b', 'b', 'B', NULL);
                 INSERT INTO track_artists VALUES (10, 0, 1, 'The Band', 'the band', NULL, 0), (11, 0, 1, 'The Band', 'the band', NULL, 0), (11, 1, 2, 'Guest', 'guest', NULL, 1), (12, 0, 3, 'B', 'b', NULL, 0);
                 INSERT INTO album_artists VALUES (1, 0, 1, 'The Band', 'the band', NULL), (2, 0, 3, 'B', 'b', NULL);
                 INSERT INTO genres VALUES (1, 'rock', 'Rock'), (2, 'jazz', 'Jazz');
                 INSERT INTO track_genres VALUES (10, 0, 1, 'Rock'), (12, 0, 2, 'Jazz');
                 INSERT INTO album_genres VALUES (1, 0, 1), (2, 0, 2);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000), (12, 4000), (10, 5000);
                 INSERT INTO favourite_albums VALUES (1, 9);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11), (12, 2, 12);
                 INSERT INTO playlists VALUES (1, 'Mix', 11), (2, 'Empty', 12);
                 INSERT INTO playlist_items VALUES (1, 0, 12), (1, 1, 10), (1, 2, 12);
                 INSERT INTO queue_items VALUES (0, 12, NULL), (1, 10, 0);
                 INSERT INTO queue_state VALUES (1, 1, 42.5);",
            )
            .unwrap();
            assert_eq!(indexes(&conn), vec!["plays_time".to_owned(), "plays_track".into()]);
            cols = tables.iter().map(|t| cols_of(&conn, t)).collect();
            before = tables.iter().zip(&cols).map(|(t, c)| dump(&conn, t, c)).collect();
        }
        let backup = path.with_extension("v8.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-8 database is kept");
        {
            let old = Connection::open(&backup).unwrap();
            assert_eq!(old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 8);
            assert_eq!(old.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get::<_, i64>(0)).unwrap(), 5);
        }
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b, "{t} unchanged");
        }
        assert_eq!(dump(&conn, "playlists", "rules"), "Null | Null", "every playlist stays an ordinary one");
        assert_eq!(indexes(&conn), vec!["plays_time".to_owned(), "plays_track_time".into()]);
        // The playlists read as before, ordinary, their tracks in order (a track twice).
        let lists = crate::query::playlists(&conn).unwrap();
        assert_eq!(lists.iter().map(|p| (p.name.as_str(), p.tracks, p.rules.is_none())).collect::<Vec<_>>(), vec![("Empty", 0, true), ("Mix", 3, true)]);
        assert_eq!(crate::query::playlist_tracks(&conn, 1).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [12, 10, 12]);
        assert_eq!(crate::query::play_count(&conn, 10).unwrap(), 3);
        // A smart playlist beside them: "in Mix, rated 3+".
        let rules = r#"{"v":1,"match":"all","rules":[{"field":"playlist","op":"in","value":1},{"field":"rating","op":"at_least","value":3}]}"#;
        let id = crate::query::create_smart_playlist(&conn, "Best of Mix", rules).unwrap();
        let smart = crate::smart::Smart::from_json(&crate::smart::playlist_rules(&conn, id).unwrap().unwrap().unwrap());
        let ids = crate::smart::evaluate_ids(&conn, &smart, Some(id), crate::smart::Ctx::new(10_000, 0)).unwrap().0;
        assert_eq!(ids, vec![10]);
        assert!(crate::query::add_to_playlist(&conn, id, &[11]).is_err(), "a smart playlist holds no tracks of its own");
        // Opening again changes nothing, and makes no second copy.
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert!(!backup.exists());
        assert_eq!(dump(&conn, "plays", "track_id, played_at"), before[2]);
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// Schema 9 -> 10 adds the scan's resume record (`scan_state`) and nothing else: a populated
    /// schema-9 library (every table with rows, a smart playlist among the playlists) reads the
    /// same after, its copy kept as library.v9.bak.db, and no scan is taken to be under way.
    #[test]
    fn schema_9_migrates_to_the_scan_record_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate9");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = [
            "albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings",
            "track_tags", "artists", "track_artists", "album_artists", "genres", "track_genres", "album_genres",
        ];
        let cols_of = |conn: &Connection, table: &str| -> String {
            let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(", ")
        };
        // In the columns schema 9 had (later schemas add some).
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY {cols}")).unwrap();
            let n = st.column_count();
            let rows: Vec<String> = st
                .query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            format!("{cols}: {}", rows.join(" | "))
        };
        let (before, cols): (Vec<String>, Vec<String>);
        {
            let conn = Connection::open(&path).unwrap();
            crate::fold::register(&conn).unwrap();
            migrate_to(&conn, 9).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked, label_key, compilation, sort_artist)
                     VALUES (1, 'mb:x', 'Album', 'The Band', 'Band, The', '2001-05-02', 'Rock', 'Lbl', 'album', 'M:/a', 'abc123', 5000, 4000, 'navidrome', '1999', '2001-05-02', NULL, 'lbl', 0, 'Band, The'),
                            (2, 't:b|c|m:/b', 'Other', 'B', NULL, NULL, 'Jazz', NULL, NULL, 'M:/b', NULL, 7000, 7000, NULL, NULL, NULL, '123|456', NULL, 1, NULL);
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels, tags_read, from_name)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'The Band', 1, 1, 200.5, 'Rock', 123, 456, 7, 'FLAC', 44100, 16, 900, 2, 8, 0),
                            (11, 'M:/a/02.flac', 1, 'Two', 'The Band feat. Guest', 1, 2, 180.0, NULL, 124, 457, 8, 'FLAC', 44100, 16, 910, 2, 8, 2),
                            (12, 'M:/b/x.opus', 2, 'Three', 'B', NULL, NULL, 90.25, 'Jazz', 125, 458, 9, 'Opus', 48000, NULL, 128, 2, 8, 1);
                 INSERT INTO track_tags (track_id, group_folder, album_title, album_artist, date, compilation) VALUES (10, 'm:/a', 'Album', 'The Band', '2001', 0), (12, 'm:/b', NULL, NULL, NULL, 1);
                 INSERT INTO artists VALUES (1, 'n:the band', 'the band', 'The Band', 'Band, The'), (2, 'n:guest', 'guest', 'Guest', NULL), (3, 'n:b', 'b', 'B', NULL);
                 INSERT INTO track_artists VALUES (10, 0, 1, 'The Band', 'the band', NULL, 0), (11, 0, 1, 'The Band', 'the band', NULL, 0), (11, 1, 2, 'Guest', 'guest', NULL, 1), (12, 0, 3, 'B', 'b', NULL, 0);
                 INSERT INTO album_artists VALUES (1, 0, 1, 'The Band', 'the band', NULL), (2, 0, 3, 'B', 'b', NULL);
                 INSERT INTO genres VALUES (1, 'rock', 'Rock'), (2, 'jazz', 'Jazz');
                 INSERT INTO track_genres VALUES (10, 0, 1, 'Rock'), (12, 0, 2, 'Jazz');
                 INSERT INTO album_genres VALUES (1, 0, 1), (2, 0, 2);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000), (12, 4000), (10, 5000);
                 INSERT INTO favourite_albums VALUES (1, 9);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11), (12, 2, 12);
                 INSERT INTO playlists (id, name, created_at, rules) VALUES (1, 'Mix', 11, NULL), (2, 'Best', 12, '{\"v\":1,\"match\":\"all\",\"rules\":[{\"field\":\"rating\",\"op\":\"at_least\",\"value\":3}]}');
                 INSERT INTO playlist_items VALUES (1, 0, 12), (1, 1, 10), (1, 2, 12);
                 INSERT INTO queue_items VALUES (0, 12, NULL), (1, 10, 0);
                 INSERT INTO queue_state VALUES (1, 1, 42.5);",
            )
            .unwrap();
            cols = tables.iter().map(|t| cols_of(&conn, t)).collect();
            before = tables.iter().zip(&cols).map(|(t, c)| dump(&conn, t, c)).collect();
        }
        let backup = path.with_extension("v9.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-9 database is kept");
        assert_eq!(Connection::open(&backup).unwrap().query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 9);
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b, "{t} unchanged");
        }
        let scans: i64 = conn.query_row("SELECT COUNT(*) FROM scan_state", [], |r| r.get(0)).unwrap();
        assert_eq!(scans, 0, "no scan under way");
        assert!(conn.execute("INSERT INTO scan_state (id, started_at) VALUES (2, 1)", []).is_err(), "one row at most");
        assert_eq!(crate::query::playlist_tracks(&conn, 1).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [12, 10, 12]);
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let _ = std::fs::remove_file(&backup);
    }

    /// Schema 10 -> 11 adds the server's columns and tables (`remote`) and nothing else: a populated
    /// schema-10 library (every table with rows, a scan under way) reads the same after in every
    /// column it had, its copy kept as library.v10.bak.db; every track and album is local (no
    /// remote id), no server is connected, nothing is pending, and the queries read it as before.
    #[test]
    fn schema_10_migrates_to_server_sources_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate10");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = [
            "albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings",
            "track_tags", "artists", "track_artists", "album_artists", "genres", "track_genres", "album_genres", "scan_state",
        ];
        let cols_of = |conn: &Connection, table: &str| -> String {
            let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(", ")
        };
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY {cols}")).unwrap();
            let n = st.column_count();
            st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let (before, cols): (Vec<String>, Vec<String>);
        {
            let conn = Connection::open(&path).unwrap();
            crate::fold::register(&conn).unwrap();
            migrate_to(&conn, 10).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, release_type, folder, art, added_at, first_seen, added_source, original_date, release_date, art_checked, label_key, compilation, sort_artist)
                     VALUES (1, 'mb:x', 'Album', 'The Band', 'Band, The', '2001-05-02', 'Rock', 'Lbl', 'album', 'M:/a', 'abc123', 5000, 4000, 'navidrome', '1999', '2001-05-02', NULL, 'lbl', 0, 'Band, The'),
                            (2, 't:b|c|m:/b', 'Other', 'B', NULL, NULL, 'Jazz', NULL, NULL, 'M:/b', NULL, 7000, 7000, 'files', NULL, NULL, '123|456', NULL, 1, NULL);
                 INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels, tags_read, from_name)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'The Band', 1, 1, 200.5, 'Rock', 123, 456, 7, 'FLAC', 44100, 16, 900, 2, 8, 0),
                            (11, 'M:/a/02.flac', 1, 'Two', 'The Band feat. Guest', 1, 2, 180.0, NULL, 124, 457, 8, 'FLAC', 44100, 16, 910, 2, 8, 2),
                            (12, 'M:/b/x.opus', 2, 'Three', 'B', NULL, NULL, 90.25, 'Jazz', 125, 458, 9, 'Opus', 48000, NULL, 128, 2, 8, 1);
                 INSERT INTO track_tags (track_id, group_folder, album_title, album_artist, date, mb_album_id, compilation) VALUES (10, 'm:/a', 'Album', 'The Band', '2001', 'x', 0), (12, 'm:/b', NULL, NULL, NULL, NULL, 1);
                 INSERT INTO artists VALUES (1, 'n:the band', 'the band', 'The Band', 'Band, The'), (2, 'n:guest', 'guest', 'Guest', NULL), (3, 'n:b', 'b', 'B', NULL);
                 INSERT INTO track_artists VALUES (10, 0, 1, 'The Band', 'the band', NULL, 0), (11, 0, 1, 'The Band', 'the band', NULL, 0), (11, 1, 2, 'Guest', 'guest', NULL, 1), (12, 0, 3, 'B', 'b', NULL, 0);
                 INSERT INTO album_artists VALUES (1, 0, 1, 'The Band', 'the band', NULL), (2, 0, 3, 'B', 'b', NULL);
                 INSERT INTO genres VALUES (1, 'rock', 'Rock'), (2, 'jazz', 'Jazz');
                 INSERT INTO track_genres VALUES (10, 0, 1, 'Rock'), (12, 0, 2, 'Jazz');
                 INSERT INTO album_genres VALUES (1, 0, 1), (2, 0, 2);
                 INSERT INTO plays VALUES (10, 1000), (10, 2000), (11, 3000), (12, 4000), (10, 5000);
                 INSERT INTO favourite_albums VALUES (1, 9);
                 INSERT INTO favourite_tracks VALUES (11, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11), (12, 2, 12);
                 INSERT INTO playlists (id, name, created_at, rules) VALUES (1, 'Mix', 11, NULL), (2, 'Best', 12, '{\"v\":1,\"match\":\"all\",\"rules\":[{\"field\":\"rating\",\"op\":\"at_least\",\"value\":3}]}');
                 INSERT INTO playlist_items VALUES (1, 0, 12), (1, 1, 10), (1, 2, 12);
                 INSERT INTO queue_items VALUES (0, 12, NULL), (1, 10, 0);
                 INSERT INTO queue_state VALUES (1, 1, 42.5);
                 INSERT INTO scan_state VALUES (1, 77, 1);",
            )
            .unwrap();
            cols = tables.iter().map(|t| cols_of(&conn, t)).collect();
            before = tables.iter().zip(&cols).map(|(t, c)| dump(&conn, t, c)).collect();
        }
        let backup = path.with_extension("v10.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-10 database is kept");
        assert_eq!(Connection::open(&backup).unwrap().query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 10);
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b, "{t} unchanged");
        }
        assert_eq!(dump(&conn, "tracks", "remote_id"), "Null | Null | Null");
        assert_eq!(dump(&conn, "albums", "remote_id, remote_art"), "Null,Null | Null,Null");
        assert_eq!(dump(&conn, "track_tags", "mb_track_id"), "Null | Null");
        for t in ["remote_server", "remote_albums", "remote_songs", "remote_pending"] {
            assert_eq!(dump(&conn, t, "COUNT(*)"), "Integer(0)", "{t} empty");
        }
        assert_eq!(crate::remote::counts(&conn).unwrap(), crate::remote::Counts { local: 3, server: 0, both: 0 });
        assert!(!crate::remote::connected(&conn).unwrap());
        let t = crate::query::track(&conn, 10).unwrap().unwrap();
        assert!(!t.is_remote() && t.remote_id.is_none());
        let a = crate::query::album(&conn, 1).unwrap().unwrap();
        assert!(!a.remote_only && a.remote_id.is_none());
        assert_eq!(crate::query::playlist_tracks(&conn, 1).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [12, 10, 12]);
        assert_eq!(crate::session::load_queue(&conn).unwrap().items.len(), 2);
        // Opening again changes nothing, and makes no second copy.
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert!(!backup.exists());
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b);
        }
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// Schema 11 gains the index of playlist entries by track, every row as it was, and a track
    /// removed is found in the playlists through it.
    #[test]
    fn schema_11_migrates_to_playlist_entries_by_track() {
        let tmp = crate::testdir::TestDir::new("migrate11");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let rows = |conn: &Connection| -> Vec<(i64, i64, i64)> {
            let mut st = conn.prepare("SELECT playlist_id, position, track_id FROM playlist_items ORDER BY playlist_id, position").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
        };
        let before;
        {
            let conn = Connection::open(&path).unwrap();
            crate::fold::register(&conn).unwrap();
            migrate_to(&conn, 11).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (1, 'k', 'A', 'B', 'M:/a', 5, 5);
                 INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'B', 200.5, 123, 456, 7), (11, 'M:/a/02.flac', 1, 'Two', 'B', 180.0, 124, 457, 8);
                 INSERT INTO playlists (id, name, created_at) VALUES (1, 'Mix', 11), (2, 'Other', 12);
                 INSERT INTO playlist_items VALUES (1, 0, 11), (1, 1, 10), (1, 2, 11), (2, 0, 10);",
            )
            .unwrap();
            before = rows(&conn);
        }
        let backup = path.with_extension("v11.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-11 database is kept");
        assert_eq!(rows(&conn), before);
        let plan: String = conn
            .prepare("EXPLAIN QUERY PLAN SELECT 1 FROM playlist_items WHERE track_id = 10")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(plan.contains("playlist_items_track"), "{plan}");
        conn.execute("DELETE FROM tracks WHERE id = 10", []).unwrap();
        assert_eq!(rows(&conn), vec![(1, 0, 11), (1, 2, 11)], "a removed track leaves its playlists");
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let _ = std::fs::remove_file(&backup);
    }

    /// Schema 12 (a library synced with a server) gains the tables of what was agreed with the
    /// server and the plays not yet reported, both empty: every row of every table as it was, in
    /// every column it had, the server's catalog and its links to the files included. Its copy is
    /// kept as library.v12.bak.db.
    #[test]
    fn schema_12_migrates_to_server_marks_with_everything_else_untouched() {
        let tmp = crate::testdir::TestDir::new("migrate12");
        let path = tmp.join("library.db");
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
        let tables = [
            "albums", "tracks", "plays", "favourite_albums", "favourite_tracks", "playlists", "playlist_items", "queue_items", "queue_state", "track_ratings",
            "track_tags", "remote_server", "remote_albums", "remote_songs", "remote_pending",
        ];
        let cols_of = |conn: &Connection, table: &str| -> String {
            let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(", ")
        };
        let dump = |conn: &Connection, table: &str, cols: &str| -> String {
            let mut st = conn.prepare(&format!("SELECT {cols} FROM {table} ORDER BY {cols}")).unwrap();
            let n = st.column_count();
            st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let (before, cols): (Vec<String>, Vec<String>);
        {
            let conn = Connection::open(&path).unwrap();
            crate::fold::register(&conn).unwrap();
            migrate_to(&conn, 12).unwrap();
            conn.execute_batch(
                "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen, remote_id, remote_art) VALUES (1, 'k', 'A', 'B', 'M:/a', 5, 5, 'al1', 'c1'), (2, 'nd:al2', 'S', 'B', 'navidrome:album/al2', 6, 6, 'al2', NULL);
                 INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at, remote_id)
                     VALUES (10, 'M:/a/01.flac', 1, 'One', 'B', 200.5, 123, 456, 7, 's1'), (11, 'M:/a/02.flac', 1, 'Two', 'B', 180.0, 124, 457, 8, NULL),
                            (12, 'navidrome:song/s3', 2, 'Three', 'B', 90.0, 0, 999, 9, 's3');
                 INSERT INTO plays VALUES (10, 1000), (12, 2000);
                 INSERT INTO favourite_tracks VALUES (12, 10);
                 INSERT INTO track_ratings VALUES (10, 4, 11);
                 INSERT INTO playlists (id, name, created_at) VALUES (1, 'Mix', 11);
                 INSERT INTO playlist_items VALUES (1, 0, 12), (1, 1, 10);
                 INSERT INTO remote_server VALUES (1, 'http://nas (ann)', 100, '2026-11-01T00:00:00Z', 200, NULL);
                 INSERT INTO remote_albums VALUES ('al1', 1, 'b|a', NULL, '{}'), ('al2', 2, 'b|s', NULL, '{}');
                 INSERT INTO remote_songs VALUES ('s1', 'al1', 3, NULL, 'flac', 'audio/flac', 456, '{}'), ('s3', 'al2', 4, 7, 'flac', 'audio/flac', 999, '{}');",
            )
            .unwrap();
            cols = tables.iter().map(|t| cols_of(&conn, t)).collect();
            before = tables.iter().zip(&cols).map(|(t, c)| dump(&conn, t, c)).collect();
        }
        let backup = path.with_extension("v12.bak.db");
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        assert!(backup.exists(), "a copy of the schema-12 database is kept");
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b, "{t} unchanged");
        }
        for t in ["remote_marks", "remote_plays"] {
            assert_eq!(dump(&conn, t, "COUNT(*)"), "Integer(0)", "{t} empty: nothing agreed yet, nothing to report");
        }
        // Opening again changes nothing, and makes no second copy.
        drop(conn);
        let _ = std::fs::remove_file(&backup);
        let conn = open(&path).unwrap();
        assert!(!backup.exists());
        for ((t, b), c) in tables.iter().zip(&before).zip(&cols) {
            assert_eq!(&dump(&conn, t, c), b);
        }
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// A step that fails leaves the database as it was, at the version before it.
    #[test]
    fn a_failed_step_rolls_back() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_to(&conn, 6).unwrap();
        // A column that already exists makes schema 7's second ALTER fail half-way.
        conn.execute_batch("ALTER TABLE tracks ADD COLUMN sample_rate INTEGER;").unwrap();
        assert!(migrate(&conn).is_err());
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 6);
        assert!(conn.prepare("SELECT codec FROM tracks").is_err(), "the first ALTER of the failed step is rolled back too");
    }

    #[test]
    fn a_newer_schema_is_refused_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        assert!(migrate(&conn).is_err());
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 99);
    }
}
