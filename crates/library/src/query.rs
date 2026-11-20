//! Read queries for the UI (shelves, album and artist pages, search) and the small
//! amount of user data the library owns (plays, favourites, playlists).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::audio::{AudioInfo, Codec};
use crate::db::now;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub release_type: Option<String>,
    /// Art cache stem.
    pub art: Option<String>,
    /// The server album it shows (`remote`), and that album's cover art id there.
    pub remote_id: Option<String>,
    pub remote_art: Option<String>,
    /// Only on the server: no track of it has a file (no folder to show, no file to edit).
    pub remote_only: bool,
}

impl Album {
    pub fn year(&self) -> Option<&str> {
        self.date.as_deref().and_then(|d| d.get(..4))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: i64,
    pub album_id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub disc_no: Option<u32>,
    pub track_no: Option<u32>,
    pub duration: f64,
    pub art: Option<String>,
    /// Format, bit depth, sample rate, bitrate, channels; unknown until read (schema 7).
    pub audio: AudioInfo,
    /// The server's song id when the server has it (`remote`).
    pub remote_id: Option<String>,
}

impl Track {
    /// Only on the server: `path` is no file's (`remote::song_path`). Played by streaming;
    /// nothing to show in Explorer, no tags to read or edit, no file details, no lyrics file.
    pub fn is_remote(&self) -> bool {
        crate::remote::is_remote(&self.path)
    }
}

/// What a home shelf shows. Persisted in the app's shelf config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShelfQuery {
    RecentlyAdded,
    RecentlyReleased,
    RecentlyPlayed,
    MostPlayed,
    Random,
    Favourites,
    AllAlbums,
    Genre { name: String },
    Label { name: String },
    Playlist { id: i64 },
}

impl ShelfQuery {
    pub fn default_title(&self) -> String {
        match self {
            Self::RecentlyAdded => "Recently added".into(),
            Self::RecentlyReleased => "Recently released".into(),
            Self::RecentlyPlayed => "Recently played".into(),
            Self::MostPlayed => "Most played".into(),
            Self::Random => "Random albums".into(),
            Self::Favourites => "Favourite albums".into(),
            Self::AllAlbums => "All albums".into(),
            Self::Genre { name } => name.clone(),
            Self::Label { name } => name.clone(),
            Self::Playlist { .. } => "Playlist".into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub albums: Vec<Album>,
    pub artists: Vec<ArtistRef>,
    pub tracks: Vec<Track>,
    /// There are more of each than the limit let through ("See all").
    pub more_albums: bool,
    pub more_artists: bool,
    pub more_tracks: bool,
    /// The query had operators (`smart::search`): it was evaluated as smart playlist rules, and
    /// can be saved as a smart playlist.
    pub operators: bool,
    /// What in the query was not understood (searched as text), a few words each.
    pub notes: Vec<String>,
}

/// One section of the search results, for its full page ("See all").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Albums,
    Artists,
    Tracks,
}

/// Which sections a search pass fills.
#[derive(Clone, Copy)]
struct Want {
    albums: bool,
    artists: bool,
    tracks: bool,
}

impl Want {
    const ALL: Want = Want { albums: true, artists: true, tracks: true };

    fn only(kind: SearchKind) -> Want {
        Want { albums: kind == SearchKind::Albums, artists: kind == SearchKind::Artists, tracks: kind == SearchKind::Tracks }
    }
}

/// A browse tile that stands for several albums: an artist, a genre, a label. `art` is
/// the cover of one of its albums (the most recently released one with art).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    /// An artist's key (`Artist::key`); a genre's or label's `names::norm`.
    pub key: String,
    pub albums: i64,
    pub art: Option<String>,
}

/// How the Albums page orders the library. Each has a natural direction (`default_desc`),
/// which choosing it applies; the direction can then be flipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlbumSort {
    /// Album artist (by sort name), then release date, then title.
    #[default]
    Artist,
    Title,
    /// Original release date when tagged, else the date; undated albums always last.
    Released,
    /// When the album was added (`added_at`).
    Added,
    /// Plays of all its tracks; never-played albums count 0.
    MostPlayed,
}

impl AlbumSort {
    pub const ALL: [AlbumSort; 5] = [AlbumSort::Artist, AlbumSort::Title, AlbumSort::Released, AlbumSort::Added, AlbumSort::MostPlayed];

    pub fn label(self) -> &'static str {
        match self {
            AlbumSort::Artist => "Artist",
            AlbumSort::Title => "Title",
            AlbumSort::Released => "Release date",
            AlbumSort::Added => "Date added",
            AlbumSort::MostPlayed => "Most played",
        }
    }

    /// Names A to Z; dates and plays newest/most first.
    pub fn default_desc(self) -> bool {
        !matches!(self, AlbumSort::Artist | AlbumSort::Title)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// Its own tracks (an ordinary playlist; a smart one has none: see `rules`).
    pub tracks: i64,
    /// A smart playlist's rule set (`smart::Smart` as JSON); None for an ordinary playlist.
    pub rules: Option<String>,
}

impl Playlist {
    pub fn smart(&self) -> bool {
        self.rules.is_some()
    }
}

const ALBUM_COLS: &str =
    "a.id, a.title, a.album_artist, a.date, a.genre, a.label, a.release_type, a.art, a.remote_id, a.remote_art, substr(a.folder, 1, 10) = 'navidrome:'";

fn album_row(r: &Row<'_>) -> rusqlite::Result<Album> {
    Ok(Album {
        id: r.get(0)?,
        title: r.get(1)?,
        artist: r.get(2)?,
        date: r.get(3)?,
        genre: r.get(4)?,
        label: r.get(5)?,
        release_type: r.get(6)?,
        art: r.get(7)?,
        remote_id: r.get(8)?,
        remote_art: r.get(9)?,
        remote_only: r.get(10)?,
    })
}

pub(crate) const TRACK_COLS: &str = "t.id, t.album_id, t.path, t.title, t.artist, a.title, a.album_artist,
    t.disc_no, t.track_no, t.duration, a.art, t.codec, t.sample_rate, t.bit_depth, t.bitrate, t.channels, t.remote_id";
/// How many columns `TRACK_COLS` has: a query's own columns after them start here.
pub(crate) const TRACK_COL_COUNT: usize = 17;

pub(crate) fn track_row(r: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        album_id: r.get(1)?,
        path: r.get(2)?,
        title: r.get(3)?,
        artist: r.get(4)?,
        album: r.get(5)?,
        album_artist: r.get(6)?,
        disc_no: r.get(7)?,
        track_no: r.get(8)?,
        duration: r.get(9)?,
        art: r.get(10)?,
        audio: audio_row(r, 11)?,
        remote_id: r.get(16)?,
    })
}

/// The details columns (`codec, sample_rate, bit_depth, bitrate, channels`) from `at`.
fn audio_row(r: &Row<'_>, at: usize) -> rusqlite::Result<AudioInfo> {
    // Borrowed, not copied: a long playlist reads thousands of rows.
    let codec = match r.get_ref(at)? {
        rusqlite::types::ValueRef::Text(t) => std::str::from_utf8(t).map(Codec::from_name).unwrap_or_default(),
        _ => Codec::Unknown,
    };
    let int = |i: usize| -> rusqlite::Result<Option<i64>> { r.get(at + i) };
    Ok(AudioInfo {
        codec,
        sample_rate: int(1)?.and_then(|v| u32::try_from(v).ok()),
        bit_depth: int(2)?.and_then(|v| u8::try_from(v).ok()),
        bitrate: int(3)?.and_then(|v| u32::try_from(v).ok()),
        channels: int(4)?.and_then(|v| u8::try_from(v).ok()),
    })
}

/// What the Properties panel shows of a library track beyond its `Track` row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackExtras {
    /// Bytes, as last scanned.
    pub size: i64,
    /// Unix seconds the scanner first saw the file.
    pub added_at: i64,
    pub plays: i64,
    /// Unix seconds of the latest counted play.
    pub last_played: Option<i64>,
}

pub fn track_extras(conn: &Connection, id: i64) -> rusqlite::Result<Option<TrackExtras>> {
    conn.query_row(
        "SELECT t.size, t.added_at, (SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id),
                (SELECT MAX(played_at) FROM plays p WHERE p.track_id = t.id)
         FROM tracks t WHERE t.id = ?1",
        [id],
        |r| Ok(TrackExtras { size: r.get(0)?, added_at: r.get(1)?, plays: r.get(2)?, last_played: r.get(3)? }),
    )
    .optional()
}

/// Everything the library keeps about an album, as stored (the metadata viewer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumFacts {
    /// The identity it is grouped by (`scan::album_key`): "mb:<release id>" or
    /// "t:<album artist>|<title>|<folder>".
    pub key: String,
    pub title: String,
    pub album_artist: String,
    pub album_artist_sort: Option<String>,
    pub date: Option<String>,
    pub original_date: Option<String>,
    pub release_date: Option<String>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub release_type: Option<String>,
    pub folder: String,
    pub art: Option<String>,
    /// Unix milliseconds ("Recently added"), where it came from (None: the scanner), and
    /// when the scanner first saw it.
    pub added_at: i64,
    pub added_source: Option<String>,
    pub first_seen: i64,
    /// Unix seconds it was made a favourite.
    pub favourite_since: Option<i64>,
    pub plays: i64,
    /// Unix seconds.
    pub last_played: Option<i64>,
    /// Bytes of all its tracks, as last scanned.
    pub size: i64,
    /// The track the scanner looks in for embedded art (its first with a file, by disc and
    /// number); none for an album only a server has.
    pub first_track: Option<String>,
    /// The server album it shows (`remote`).
    pub remote_id: Option<String>,
    /// Grouped as a compilation ("Various Artists").
    pub compilation: bool,
    /// Its album artists' names, in order.
    pub artists: Vec<String>,
    /// Its genres, the most carried first.
    pub genres: Vec<String>,
}

pub fn album_facts(conn: &Connection, id: i64) -> rusqlite::Result<Option<AlbumFacts>> {
    conn.query_row(
        "SELECT a.key, a.title, a.album_artist, a.album_artist_sort, a.date, a.original_date, a.release_date,
                a.genre, a.label, a.release_type, a.folder, a.art, a.added_at, a.added_source, a.first_seen,
                (SELECT f.added_at FROM favourite_albums f WHERE f.album_id = a.id),
                (SELECT COUNT(*) FROM plays p JOIN tracks t ON t.id = p.track_id WHERE t.album_id = a.id),
                (SELECT MAX(p.played_at) FROM plays p JOIN tracks t ON t.id = p.track_id WHERE t.album_id = a.id),
                (SELECT COALESCE(SUM(t.size), 0) FROM tracks t WHERE t.album_id = a.id),
                (SELECT path FROM tracks t WHERE t.album_id = a.id AND t.path NOT LIKE 'navidrome:%'
                 ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path LIMIT 1),
                a.compilation, a.remote_id
         FROM albums a WHERE a.id = ?1",
        [id],
        |r| {
            Ok(AlbumFacts {
                key: r.get(0)?,
                title: r.get(1)?,
                album_artist: r.get(2)?,
                album_artist_sort: r.get(3)?,
                date: r.get(4)?,
                original_date: r.get(5)?,
                release_date: r.get(6)?,
                genre: r.get(7)?,
                label: r.get(8)?,
                release_type: r.get(9)?,
                folder: r.get(10)?,
                art: r.get(11)?,
                added_at: r.get(12)?,
                added_source: r.get(13)?,
                first_seen: r.get(14)?,
                favourite_since: r.get(15)?,
                plays: r.get(16)?,
                last_played: r.get(17)?,
                size: r.get(18)?,
                first_track: r.get(19)?,
                compilation: r.get(20)?,
                remote_id: r.get(21)?,
                artists: Vec::new(),
                genres: Vec::new(),
            })
        },
    )
    .optional()?
    .map(|mut f| -> rusqlite::Result<AlbumFacts> {
        f.artists = album_artist_refs(conn, id)?.into_iter().map(|a| a.name).collect();
        f.genres = album_genre_names(conn, id)?;
        Ok(f)
    })
    .transpose()
}

/// A track's genre as stored (tracks keep their own; the album's is its last scanned
/// track's).
pub fn track_genre(conn: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT genre FROM tracks WHERE id = ?1", [id], |r| r.get(0)).optional().map(Option::flatten)
}

/// SQL LIKE pattern for a substring match, with the user's text escaped.
fn like_pattern(q: &str) -> String {
    let esc = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{esc}%")
}

/// How a shelf query runs beyond its definition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShelfOpts {
    /// The shuffle of `Random` (same seed, same order).
    pub seed: i64,
    /// Only albums with art (the hero card).
    pub art_only: bool,
}

/// What the Albums page sorts "by artist" by: the album's ALBUMARTISTSORT, else its first
/// album artist's sort name (from any of their tracks or albums; `albums.sort_artist`),
/// else the album artist.
const ARTIST_SORT: &str = "COALESCE(a.sort_artist, a.album_artist)";

/// What "Recently released" sorts by: the original release date when tagged
/// (ORIGINALDATE, so a reissue sits with its first release), else the album's date.
const RELEASED: &str = "COALESCE(NULLIF(a.original_date, ''), a.date)";

/// The albums a shelf shows, best first. The definitions follow Navidrome's album list
/// sorts (see docs/features/library.md, "Home shelves").
pub fn shelf(conn: &Connection, q: &ShelfQuery, limit: usize) -> rusqlite::Result<Vec<Album>> {
    shelf_with(conn, q, limit, ShelfOpts::default())
}

pub fn shelf_with(conn: &Connection, q: &ShelfQuery, limit: usize, opts: ShelfOpts) -> rusqlite::Result<Vec<Album>> {
    let limit = limit.min(i64::MAX as usize) as i64;
    let art = if opts.art_only { "AND a.art IS NOT NULL" } else { "" };
    let sql = match q {
        // Newest first by the date the album was added (first seen, or imported).
        ShelfQuery::RecentlyAdded => {
            format!("SELECT {ALBUM_COLS} FROM albums a WHERE 1 {art} ORDER BY a.added_at DESC, a.id DESC LIMIT ?1")
        }
        // By release (original release when tagged), then this edition's release
        // date, then title, all newest/last first as Navidrome orders them. Undated
        // albums come last.
        ShelfQuery::RecentlyReleased => format!(
            "SELECT {ALBUM_COLS} FROM albums a WHERE 1 {art}
             ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.release_date DESC,
                      a.title COLLATE NOCASE DESC, a.id DESC LIMIT ?1"
        ),
        // Albums with a play, by their latest play (any track).
        ShelfQuery::RecentlyPlayed => format!(
            "SELECT {ALBUM_COLS} FROM albums a JOIN (
                SELECT t.album_id, MAX(p.played_at) AS last FROM plays p JOIN tracks t ON t.id = p.track_id
                GROUP BY t.album_id) r ON r.album_id = a.id
             WHERE 1 {art} ORDER BY r.last DESC, a.id DESC LIMIT ?1"
        ),
        // Albums with a play, by plays of all their tracks, all time; ties by the
        // latest play.
        ShelfQuery::MostPlayed => format!(
            "SELECT {ALBUM_COLS} FROM albums a JOIN (
                SELECT t.album_id, COUNT(*) AS n, MAX(p.played_at) AS last FROM plays p
                JOIN tracks t ON t.id = p.track_id GROUP BY t.album_id) r ON r.album_id = a.id
             WHERE 1 {art} ORDER BY r.n DESC, r.last DESC, a.id DESC LIMIT ?1"
        ),
        ShelfQuery::Random => {
            return query_albums(
                conn,
                &format!(
                    "SELECT {ALBUM_COLS} FROM albums a WHERE 1 {art}
                     ORDER BY seeded_rand(?2, a.id), a.id LIMIT ?1"
                ),
                params![limit, opts.seed],
            );
        }
        ShelfQuery::Favourites => format!(
            "SELECT {ALBUM_COLS} FROM albums a JOIN favourite_albums f ON f.album_id = a.id
             WHERE 1 {art} ORDER BY f.added_at DESC, a.id DESC LIMIT ?1"
        ),
        ShelfQuery::AllAlbums => format!(
            "SELECT {ALBUM_COLS} FROM albums a WHERE 1 {art}
             ORDER BY {ARTIST_SORT} COLLATE NAMES, a.date, a.title COLLATE NAMES LIMIT ?1"
        ),
        ShelfQuery::Genre { name } => {
            return query_albums(
                conn,
                &format!(
                    "SELECT {ALBUM_COLS} FROM albums a
                     WHERE a.id IN (SELECT x.album_id FROM album_genres x JOIN genres g ON g.id = x.genre_id WHERE g.key = ?2) {art}
                     ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.title COLLATE NOCASE LIMIT ?1"
                ),
                params![limit, crate::names::norm(name)],
            );
        }
        ShelfQuery::Label { name } => {
            return query_albums(
                conn,
                &format!(
                    "SELECT {ALBUM_COLS} FROM albums a WHERE a.label_key = ?2 {art}
                     ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.title COLLATE NOCASE LIMIT ?1"
                ),
                params![limit, crate::names::norm(name)],
            );
        }
        ShelfQuery::Playlist { id } => {
            // A smart playlist: the albums of what it holds now, in its order.
            if let Some(Some(rules)) = crate::smart::playlist_rules(conn, *id)? {
                let smart = crate::smart::Smart::from_json(&rules);
                let tracks = crate::smart::evaluate(conn, &smart, Some(*id), crate::smart::Ctx::now())?.tracks;
                return albums_of_tracks(conn, &tracks, limit as usize, opts.art_only);
            }
            return query_albums(
                conn,
                &format!(
                    "SELECT {ALBUM_COLS} FROM albums a JOIN (
                        SELECT t.album_id, MIN(i.position) AS pos FROM playlist_items i
                        JOIN tracks t ON t.id = i.track_id WHERE i.playlist_id = ?2 GROUP BY t.album_id
                     ) p ON p.album_id = a.id WHERE 1 {art} ORDER BY p.pos LIMIT ?1"
                ),
                params![limit, id],
            );
        }
    };
    query_albums(conn, &sql, params![limit])
}

/// The albums of `tracks`, each once, in the order their first track comes; at most `limit`.
pub fn albums_of_tracks(conn: &Connection, tracks: &[Track], limit: usize, art_only: bool) -> rusqlite::Result<Vec<Album>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in tracks {
        if out.len() >= limit {
            break;
        }
        if !seen.insert(t.album_id) || (art_only && t.art.is_none()) {
            continue;
        }
        if let Some(a) = album(conn, t.album_id)? {
            out.push(a);
        }
    }
    Ok(out)
}

/// Every album, for the Albums page, in `sort` order (`desc` reverses the main key; ties
/// are always broken the same way, so the order is stable).
pub fn all_albums(conn: &Connection, sort: AlbumSort, desc: bool) -> rusqlite::Result<Vec<Album>> {
    let dir = if desc { "DESC" } else { "ASC" };
    let artist = format!("{ARTIST_SORT} COLLATE NAMES");
    let sql = match sort {
        AlbumSort::Artist => format!("SELECT {ALBUM_COLS} FROM albums a ORDER BY {artist} {dir}, a.date, a.title COLLATE NAMES, a.id"),
        AlbumSort::Title => format!("SELECT {ALBUM_COLS} FROM albums a ORDER BY a.title COLLATE NAMES {dir}, {artist}, a.id"),
        AlbumSort::Released => format!(
            "SELECT {ALBUM_COLS} FROM albums a ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) {dir}, a.title COLLATE NOCASE, a.id"
        ),
        AlbumSort::Added => format!("SELECT {ALBUM_COLS} FROM albums a ORDER BY a.added_at {dir}, a.id {dir}"),
        AlbumSort::MostPlayed => format!(
            "SELECT {ALBUM_COLS} FROM albums a LEFT JOIN (
                SELECT t.album_id, COUNT(*) AS n, MAX(p.played_at) AS last FROM plays p
                JOIN tracks t ON t.id = p.track_id GROUP BY t.album_id) r ON r.album_id = a.id
             ORDER BY COALESCE(r.n, 0) {dir}, COALESCE(r.last, 0) {dir}, {artist}, a.date, a.id"
        ),
    };
    query_albums(conn, &sql, [])
}

/// Every album artist, each artist of a shared album on their own, ordered by sort name
/// (their ARTISTSORT / ALBUMARTISTSORT when tagged anywhere, else their name; natural
/// order: "Artist 2" before "Artist 10", case ignored); with how many albums they have and
/// the art of their most recently released album that has any.
pub fn album_artists(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    let mut index: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    {
        let mut st = conn.prepare_cached(
            "SELECT id, key, name, COALESCE(sort_name, name) FROM artists WHERE id IN (SELECT artist_id FROM album_artists)",
        )?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))? {
            let (id, key, name, sort) = row?;
            index.insert(id, groups.len());
            groups.push((crate::fold::fold(&sort), Group { name, key, albums: 0, art: None }));
        }
    }
    let mut st = conn.prepare_cached(&format!(
        "SELECT x.artist_id, a.art FROM album_artists x JOIN albums a ON a.id = x.album_id
         ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.id"
    ))?;
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (id, art) = row?;
        let Some(&i) = index.get(&id) else { continue };
        let g = &mut groups[i].1;
        g.albums += 1;
        if g.art.is_none() {
            g.art = art;
        }
    }
    groups.sort_by(|a, b| crate::fold::natural_cmp(&a.0, &b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}

/// Genres (`label == false`) or labels, as browse tiles, by name (natural order, case
/// ignored, as `album_artists`). A genre counts every album one of whose tracks carries it;
/// a name is one entry whatever its case, spacing or Unicode form, in its most common
/// spelling. "[no label]" is MusicBrainz's placeholder for self-released records, not a
/// label.
pub fn album_groups(conn: &Connection, label: bool) -> rusqlite::Result<Vec<Group>> {
    let sql = if label {
        format!(
            "SELECT a.label_key, a.label, a.art FROM albums a WHERE a.label_key IS NOT NULL AND a.label_key <> '' AND a.label_key <> '[no label]'
             ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.id"
        )
    } else {
        format!(
            "SELECT g.key, g.name, a.art FROM album_genres x JOIN genres g ON g.id = x.genre_id JOIN albums a ON a.id = x.album_id
             ORDER BY ({RELEASED}) IS NULL, ({RELEASED}) DESC, a.id"
        )
    };
    let mut st = conn.prepare_cached(&sql)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
    // (fold key, group, spellings and their counts)
    type Spelled = (String, Group, Vec<(String, usize)>);
    let mut groups: Vec<Spelled> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows {
        let (key, name, art) = row?;
        let i = *index.entry(key.clone()).or_insert_with(|| {
            groups.push((String::new(), Group { name: String::new(), key, albums: 0, art: None }, Vec::new()));
            groups.len() - 1
        });
        let g = &mut groups[i];
        g.1.albums += 1;
        if g.1.art.is_none() {
            g.1.art = art;
        }
        match g.2.iter_mut().find(|(n, _)| *n == name) {
            Some(s) => s.1 += 1,
            None => g.2.push((name, 1)),
        }
    }
    for g in &mut groups {
        let best = g.2.iter().map(|s| s.1).max().unwrap_or(0);
        let mut spellings: Vec<&(String, usize)> = g.2.iter().filter(|s| s.1 == best).collect();
        spellings.sort();
        g.1.name = spellings.first().map(|s| s.0.clone()).unwrap_or_default();
        g.0 = crate::fold::fold(&g.1.name);
    }
    groups.sort_by(|a, b| crate::fold::natural_cmp(&a.0, &b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    Ok(groups.into_iter().map(|(_, g, _)| g).collect())
}

/// The folder an album was scanned from.
pub fn album_folder(conn: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT folder FROM albums WHERE id = ?1", [id], |r| r.get(0)).optional()
}

fn query_albums(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> rusqlite::Result<Vec<Album>> {
    let mut st = conn.prepare_cached(sql)?;
    st.query_map(p, album_row)?.collect()
}

fn query_tracks(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> rusqlite::Result<Vec<Track>> {
    let mut st = conn.prepare_cached(sql)?;
    st.query_map(p, track_row)?.collect()
}

pub fn album(conn: &Connection, id: i64) -> rusqlite::Result<Option<Album>> {
    conn.query_row(&format!("SELECT {ALBUM_COLS} FROM albums a WHERE a.id = ?1"), [id], album_row).optional()
}

pub fn album_tracks(conn: &Connection, album_id: i64) -> rusqlite::Result<Vec<Track>> {
    query_tracks(
        conn,
        &format!(
            "SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.album_id = ?1
             ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path"
        ),
        [album_id],
    )
}

/// The track that is the server's song `song_id`, if the library has it.
pub fn track_by_remote(conn: &Connection, song_id: &str) -> rusqlite::Result<Option<Track>> {
    conn.query_row(&format!("SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.remote_id = ?1"), [song_id], track_row).optional()
}

pub fn track(conn: &Connection, id: i64) -> rusqlite::Result<Option<Track>> {
    conn.query_row(
        &format!("SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.id = ?1"),
        [id],
        track_row,
    )
    .optional()
}

/// The tracks with these ids that the library has, by id, in one query (ids it does not have,
/// a file from outside's negative one among them, are not in it).
pub fn tracks_by_id(conn: &Connection, ids: &[i64]) -> rusqlite::Result<std::collections::HashMap<i64, Track>> {
    let mut wanted: Vec<i64> = ids.iter().copied().filter(|&id| id > 0).collect();
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let list = serde_json::to_string(&wanted).unwrap_or_else(|_| "[]".into());
    let tracks = query_tracks(conn, &format!("SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.id IN (SELECT value FROM json_each(?1))"), [list])?;
    Ok(tracks.into_iter().map(|t| (t.id, t)).collect())
}

/// The artists in a value that joins several (see `names::split_artists`).
pub use crate::names::split_artists;

/// An artist as the library knows it (`relations.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artist {
    pub id: i64,
    /// Its identity: "mb:<MusicBrainz id>" or "n:<name>"; what links and routes carry.
    pub key: String,
    pub name: String,
}

/// An artist credited on a track or an album, in credit order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistRef {
    pub key: String,
    pub name: String,
    /// Credited after "feat." (tracks only).
    pub featured: bool,
}

/// The artist a link, a route or a name stands for: by key ("mb:…", "n:…"), else by name
/// (its `norm`: case, spaces and Unicode form ignored), the most credited of that name.
pub fn find_artist(conn: &Connection, s: &str) -> rusqlite::Result<Option<Artist>> {
    let row = |r: &Row<'_>| Ok(Artist { id: r.get(0)?, key: r.get(1)?, name: r.get(2)? });
    if let Some(a) = conn.prepare_cached("SELECT id, key, name FROM artists WHERE key = ?1")?.query_row([s], row).optional()? {
        return Ok(Some(a));
    }
    conn.prepare_cached(
        "SELECT a.id, a.key, a.name FROM artists a WHERE a.norm = ?1
         ORDER BY (SELECT COUNT(*) FROM album_artists x WHERE x.artist_id = a.id) DESC,
                  (SELECT COUNT(*) FROM track_artists x WHERE x.artist_id = a.id) DESC, a.id LIMIT 1",
    )?
    .query_row([crate::names::norm(s)], row)
    .optional()
}

/// The artists a track credits, in order.
pub fn track_artists(conn: &Connection, track: i64) -> rusqlite::Result<Vec<ArtistRef>> {
    let mut st = conn.prepare_cached(
        "SELECT a.key, x.name, x.featured FROM track_artists x JOIN artists a ON a.id = x.artist_id
         WHERE x.track_id = ?1 ORDER BY x.position",
    )?;
    st.query_map([track], |r| Ok(ArtistRef { key: r.get(0)?, name: r.get(1)?, featured: r.get(2)? }))?.collect()
}

/// An album's artists, in order.
pub fn album_artist_refs(conn: &Connection, album: i64) -> rusqlite::Result<Vec<ArtistRef>> {
    let mut st = conn.prepare_cached(
        "SELECT a.key, x.name FROM album_artists x JOIN artists a ON a.id = x.artist_id
         WHERE x.album_id = ?1 ORDER BY x.position",
    )?;
    st.query_map([album], |r| Ok(ArtistRef { key: r.get(0)?, name: r.get(1)?, featured: false }))?.collect()
}

/// An album's genres, the most carried first.
pub fn album_genre_names(conn: &Connection, album: i64) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare_cached("SELECT g.name FROM album_genres x JOIN genres g ON g.id = x.genre_id WHERE x.album_id = ?1 ORDER BY x.position")?;
    st.query_map([album], |r| r.get(0))?.collect()
}

/// A track's genres, in order.
pub fn track_genre_names(conn: &Connection, track: i64) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare_cached("SELECT name FROM track_genres WHERE track_id = ?1 ORDER BY position")?;
    st.query_map([track], |r| r.get(0))?.collect()
}

/// The artist's albums (they are an album artist of them), and the albums they appear on
/// (credited on a track, as an artist or featured, of an album that is not theirs).
pub fn artist_albums(conn: &Connection, artist: i64) -> rusqlite::Result<(Vec<Album>, Vec<Album>)> {
    let own = query_albums(
        conn,
        &format!(
            "SELECT {ALBUM_COLS} FROM albums a WHERE a.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?1)
             ORDER BY a.date DESC, a.title"
        ),
        [artist],
    )?;
    let appears = query_albums(
        conn,
        &format!(
            "SELECT {ALBUM_COLS} FROM albums a WHERE a.id IN (
                SELECT t.album_id FROM track_artists x JOIN tracks t ON t.id = x.track_id WHERE x.artist_id = ?1)
               AND a.id NOT IN (SELECT album_id FROM album_artists WHERE artist_id = ?1)
             ORDER BY a.date DESC, a.title"
        ),
        [artist],
    )?;
    Ok((own, appears))
}

/// The artist's most played tracks: those that credit them, and those of their albums.
pub fn artist_top_tracks(conn: &Connection, artist: i64, limit: usize) -> rusqlite::Result<Vec<Track>> {
    query_tracks(
        conn,
        &format!(
            "SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id
             LEFT JOIN (SELECT track_id, COUNT(*) AS n FROM plays GROUP BY track_id) p ON p.track_id = t.id
             WHERE t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ?1)
                OR t.album_id IN (SELECT album_id FROM album_artists WHERE artist_id = ?1)
             ORDER BY COALESCE(p.n, 0) DESC, a.date DESC, t.track_no, t.id LIMIT ?2"
        ),
        params![artist, limit.min(i64::MAX as usize) as i64],
    )
}

/// Substring search, folded (`fold.rs`): case, katakana/hiragana, full/half width and
/// Latin accents are ignored on both sides; sort names match without their commas ("hanazawa kana"
/// finds "Hanazawa, Kana"). A query that reads as romaji (4 letters or more,
/// `romaji::to_kana`) also finds kana titles, artists and albums; those results come
/// after the ordinary ones, which are exactly what the Latin query alone finds.
///
/// Each section holds at most `limit` results; `more_*` says when there were more (the
/// search page's "See all", which asks for one section with no limit: `search_section`).
/// Artists are album artists first (by albums), then track artists (by tracks), each
/// artist of a multi-artist value on its own and featured artists too ("A feat. B" finds
/// B), as the artist page credits them.
pub fn search(conn: &Connection, q: &str, limit: usize) -> rusqlite::Result<SearchResults> {
    search_in(conn, q, limit, Want::ALL)
}

/// One section of `search`, up to `limit` (`usize::MAX`: all of it).
pub fn search_section(conn: &Connection, q: &str, kind: SearchKind, limit: usize) -> rusqlite::Result<SearchResults> {
    search_in(conn, q, limit, Want::only(kind))
}

fn search_in(conn: &Connection, q: &str, limit: usize, want: Want) -> rusqlite::Result<SearchResults> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(SearchResults::default());
    }
    // One more than asked, to know whether there is more.
    let fetch = limit.saturating_add(1);
    // Operators (`artist:`, `year:>2000`, `-is:remote`...) take the smart playlists' rules; a
    // query without any is plain search, exactly as before (reading it costs microseconds).
    let parsed = crate::smart::search::parse_in(conn, q);
    if parsed.has_operators() {
        let mut r = search_rules(conn, &parsed, fetch, want)?;
        finish(&mut r, limit);
        return Ok(r);
    }
    let folded = crate::fold::fold(q);
    let mut r = search_with(conn, Pass::Fold(&folded), fetch, want)?;
    let letters = q.chars().filter(|c| c.is_alphabetic()).count();
    if letters >= 4
        && let Some(kana) = crate::romaji::to_kana(q)
    {
        let key = crate::romaji::loose(&crate::fold::fold(&kana));
        if key.chars().count() >= 2 {
            let k = search_with(conn, Pass::Kana(&key), fetch, want)?;
            append(&mut r.albums, k.albums, fetch, |a| a.id);
            append(&mut r.tracks, k.tracks, fetch, |t| t.id);
            append(&mut r.artists, k.artists, fetch, |a| a.key.clone());
        }
    }
    r.notes = parsed.notes;
    finish(&mut r, limit);
    Ok(r)
}

/// Say which sections have more than `limit`, and keep `limit` of each.
fn finish(r: &mut SearchResults, limit: usize) {
    r.more_albums = r.albums.len() > limit;
    r.more_artists = r.artists.len() > limit;
    r.more_tracks = r.tracks.len() > limit;
    r.albums.truncate(limit);
    r.artists.truncate(limit);
    r.tracks.truncate(limit);
}

/// Search by operators: the query's rules (`smart::search`), evaluated as a smart playlist's are
/// (`smart::evaluate`), at most `limit` of each section. Tracks are the tracks the rules hold;
/// albums are those with a track the rules hold read album by album (shown only when every term
/// can apply to an album: not for a play count); artists only when every term is
/// about names (words, `artist:`, `albumartist:`).
fn search_rules(conn: &Connection, p: &crate::smart::search::Parsed, limit: usize, want: Want) -> rusqlite::Result<SearchResults> {
    use crate::smart::search::Level;
    use crate::smart::{Ctx, Limit, LimitUnit, Limiting, Op, evaluate};
    let ctx = Ctx::now();
    let mut r = SearchResults { operators: true, notes: p.notes.clone(), ..SearchResults::default() };
    if want.tracks
        && let Some(mut s) = p.rules(Level::Tracks)
    {
        if let Ok(n) = u32::try_from(limit) {
            s.limit = Some(Limiting::Known(Limit { n, unit: LimitUnit::Tracks, by: s.order() }));
        }
        r.tracks = evaluate(conn, &s, None, ctx)?.tracks;
    }
    let limit = limit.min(i64::MAX as usize) as i64;
    if want.albums
        && let Some(s) = p.rules(Level::Albums)
    {
        let (sel, mut params) = crate::smart::album_selection(conn, &s, ctx);
        params.push(rusqlite::types::Value::Integer(limit));
        r.albums = query_albums(
            conn,
            &format!("SELECT {ALBUM_COLS} FROM albums a WHERE a.id IN ({sel}) ORDER BY {RELEASED} DESC, a.title COLLATE NOCASE, a.id LIMIT ?"),
            rusqlite::params_from_iter(params),
        )?;
    }
    if want.artists
        && let Some(terms) = p.artist_terms()
    {
        let mut sql = String::from("SELECT r.key, r.name FROM artists r WHERE 1");
        let mut params: Vec<String> = Vec::new();
        for (album_artist, op, text) in terms {
            let v = crate::fold::fold(&crate::smart::clean_text(&text));
            let (cmp, pat) = match op {
                Op::Is | Op::IsNot => ("= ?", v),
                _ => ("LIKE ?", like_pattern(&v)),
            };
            params.push(pat);
            let n = params.len();
            let esc = if op == Op::Is || op == Op::IsNot { "" } else { " ESCAPE '\\'" };
            let name = format!("(fold(r.name) {cmp}{n}{esc} OR replace(fold(COALESCE(r.sort_name, '')), ',', '') {cmp}{n}{esc})");
            sql.push_str(&if matches!(op, Op::IsNot | Op::NotContains) { format!(" AND NOT {name}") } else { format!(" AND {name}") });
            if album_artist {
                sql.push_str(" AND r.id IN (SELECT artist_id FROM album_artists)");
            }
        }
        params.push(limit.to_string());
        sql.push_str(&format!(
            " ORDER BY (SELECT COUNT(*) FROM album_artists x WHERE x.artist_id = r.id) DESC,
                       (SELECT COUNT(*) FROM track_artists x WHERE x.artist_id = r.id) DESC, r.name LIMIT CAST(?{} AS INTEGER)",
            params.len()
        ));
        let mut st = conn.prepare_cached(&sql)?;
        r.artists = st.query_map(rusqlite::params_from_iter(params), |r| Ok(ArtistRef { key: r.get(0)?, name: r.get(1)?, featured: false }))?.collect::<rusqlite::Result<_>>()?;
    }
    Ok(r)
}

/// A search pass: the folded query (case, width, kana and accents folded), or the kana
/// reading of a romaji query (`romaji::loose`, matched only against values with kana).
#[derive(Clone, Copy)]
enum Pass<'a> {
    Fold(&'a str),
    Kana(&'a str),
}

/// Add `more` after `into`, skipping what is already there, up to `limit`.
fn append<T, K: PartialEq>(into: &mut Vec<T>, more: Vec<T>, limit: usize, key: impl Fn(&T) -> K) {
    for m in more {
        if into.len() >= limit {
            break;
        }
        if !into.iter().any(|x| key(x) == key(&m)) {
            into.push(m);
        }
    }
}

/// One pass of search. The fold pass applies `fold` to the columns and also searches album
/// artist sort names (Latin readings); the kana pass first checks that a column holds any
/// kana (a cheap GLOB), so only those values are folded.
fn search_with(conn: &Connection, pass: Pass<'_>, limit: usize, want: Want) -> rusqlite::Result<SearchResults> {
    let limit = limit.min(i64::MAX as usize) as i64;
    let (pat, kana) = match pass {
        Pass::Fold(q) => (like_pattern(q), false),
        Pass::Kana(key) => (like_pattern(key), true),
    };
    let m = |col: &str| {
        if kana {
            format!("({col} GLOB '*[ぁ-ヿ]*' AND kfold({col}) LIKE ?1 ESCAPE '\\')")
        } else {
            format!("fold({col}) LIKE ?1 ESCAPE '\\'")
        }
    };
    let sort = |col: &str| {
        if kana { String::new() } else { format!(" OR {} OR replace(fold({col}), ',', '') LIKE ?1 ESCAPE '\\'", m(col)) }
    };
    let mut r = SearchResults::default();
    if want.albums {
        r.albums = query_albums(
            conn,
            &format!(
                "SELECT {ALBUM_COLS} FROM albums a
                 WHERE {} OR {} OR {}{}
                 ORDER BY ({}) DESC, a.date DESC LIMIT ?2",
                m("a.title"),
                m("a.album_artist"),
                m("a.label"),
                sort("a.album_artist_sort"),
                m("a.title")
            ),
            params![pat, limit],
        )?;
    }
    if want.artists {
        let mut st = conn.prepare_cached(&format!(
            "SELECT r.key, r.name FROM artists r WHERE {}{}
             ORDER BY (SELECT COUNT(*) FROM album_artists x WHERE x.artist_id = r.id) DESC,
                      (SELECT COUNT(*) FROM track_artists x WHERE x.artist_id = r.id) DESC, r.name LIMIT ?2",
            m("r.name"),
            sort("r.sort_name")
        ))?;
        r.artists = st.query_map(params![pat, limit], |r| Ok(ArtistRef { key: r.get(0)?, name: r.get(1)?, featured: false }))?.collect::<rusqlite::Result<_>>()?;
    }
    if want.tracks {
        r.tracks = query_tracks(
            conn,
            &format!(
                "SELECT {TRACK_COLS} FROM tracks t JOIN albums a ON a.id = t.album_id
                 WHERE {} OR {}
                 ORDER BY ({}) DESC, a.date DESC LIMIT ?2",
                m("t.title"),
                m("t.artist"),
                m("t.title")
            ),
            params![pat, limit],
        )?;
    }
    Ok(r)
}

/// Genres and how many albums each is on, the most first (the shelf editor).
pub fn genres(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut st = conn.prepare_cached(
        "SELECT g.name, COUNT(*) FROM album_genres x JOIN genres g ON g.id = x.genre_id GROUP BY g.id ORDER BY 2 DESC, g.name",
    )?;
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

/// Labels and how many albums each has, the most first, in their most common spelling.
pub fn labels(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut v: Vec<(String, i64)> = album_groups(conn, true)?.into_iter().map(|g| (g.name, g.albums)).collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.1));
    Ok(v)
}

pub fn counts(conn: &Connection) -> rusqlite::Result<(i64, i64)> {
    conn.query_row("SELECT (SELECT COUNT(*) FROM albums), (SELECT COUNT(*) FROM tracks)", [], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
}

// ---- plays -------------------------------------------------------------------

pub fn record_play(conn: &Connection, track_id: i64) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO plays (track_id, played_at) VALUES (?1, ?2)", params![track_id, now()])?;
    Ok(())
}

/// Write plays counted earlier (track, `db::now` when counted), in order, each taking it off
/// `pending` once it is in: a call that fails part way (the database busy or not writable)
/// leaves the rest for the next, and none is written twice. A play of a track no longer in the
/// library is dropped. Returns how many were written.
pub fn record_plays(conn: &Connection, pending: &mut Vec<(i64, i64)>) -> rusqlite::Result<usize> {
    record_plays_for(conn, pending, false)
}

/// `record_plays`; with `to_server`, each play of a track the server has is also queued for it
/// (`marks::queue_play`), in the same transaction as the play: never one without the other.
pub fn record_plays_for(conn: &Connection, pending: &mut Vec<(i64, i64)>, to_server: bool) -> rusqlite::Result<usize> {
    let mut written = 0;
    while let Some(&(id, at)) = pending.first() {
        let tx = crate::db::write(conn)?;
        let n = tx.prepare_cached("INSERT INTO plays (track_id, played_at) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?1)")?.execute(params![id, at])?;
        if n > 0 && to_server {
            crate::marks::queue_play(&tx, id, at)?;
        }
        tx.commit()?;
        written += n;
        pending.remove(0);
    }
    Ok(written)
}

pub fn play_count(conn: &Connection, track_id: i64) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM plays WHERE track_id = ?1", [track_id], |r| r.get(0))
}

// ---- favourites --------------------------------------------------------------

pub fn is_favourite_album(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM favourite_albums WHERE album_id = ?1)", [id], |r| r.get(0))
}

/// Toggle; returns the new state.
pub fn toggle_favourite_album(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    if is_favourite_album(conn, id)? {
        conn.execute("DELETE FROM favourite_albums WHERE album_id = ?1", [id])?;
        Ok(false)
    } else {
        conn.execute("INSERT INTO favourite_albums (album_id, added_at) VALUES (?1, ?2)", params![id, now()])?;
        Ok(true)
    }
}

/// Every favourite track's id: one query for a whole list of tracks (a favourites table
/// is small), instead of one per track.
pub fn favourite_track_ids(conn: &Connection) -> rusqlite::Result<std::collections::HashSet<i64>> {
    let mut st = conn.prepare_cached("SELECT track_id FROM favourite_tracks")?;
    st.query_map([], |r| r.get(0))?.collect()
}

/// A playlist's name, if it exists.
pub fn playlist_name(conn: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| r.get(0)).optional()
}

pub fn is_favourite_track(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM favourite_tracks WHERE track_id = ?1)", [id], |r| r.get(0))
}

pub fn toggle_favourite_track(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    if is_favourite_track(conn, id)? {
        conn.execute("DELETE FROM favourite_tracks WHERE track_id = ?1", [id])?;
        Ok(false)
    } else {
        conn.execute("INSERT INTO favourite_tracks (track_id, added_at) VALUES (?1, ?2)", params![id, now()])?;
        Ok(true)
    }
}

/// Several tracks favourited, or not, all or none (one transaction). Tracks already so are
/// left as they are (a favourite keeps when it was made). Returns how many changed.
pub fn set_favourite_tracks(conn: &Connection, ids: &[i64], fav: bool) -> rusqlite::Result<usize> {
    let tx = crate::db::write(conn)?;
    let mut n = 0;
    if fav {
        let mut st = tx.prepare_cached("INSERT OR IGNORE INTO favourite_tracks (track_id, added_at) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?1)")?;
        let at = now();
        for id in ids {
            n += st.execute(params![id, at])?;
        }
    } else {
        let mut st = tx.prepare_cached("DELETE FROM favourite_tracks WHERE track_id = ?1")?;
        for id in ids {
            n += st.execute([id])?;
        }
    }
    tx.commit()?;
    Ok(n)
}

// ---- ratings, no longer kept ------------------------------------------------

/// Smart playlists stored with a rule about ratings (from a build that kept them) written
/// again without it (`smart::strip_ratings`): the rating conditions dropped, an order by
/// rating back to the default one, every other rule as it was. Logged, one line each. How
/// many were.
pub fn forget_rating_rules(conn: &Connection) -> rusqlite::Result<usize> {
    let found: Vec<(i64, String, String)> = conn
        .prepare("SELECT id, name, rules FROM playlists WHERE rules IS NOT NULL AND rules LIKE '%rating%'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    for (id, name, rules) in found {
        let Ok(serde_json::Value::Object(mut m)) = serde_json::from_str(&rules) else { continue };
        if crate::smart::strip_ratings(&mut m) {
            conn.execute("UPDATE playlists SET rules = ?2 WHERE id = ?1", params![id, serde_json::Value::Object(m).to_string()])?;
            log::info!("smart playlist \u{201c}{name}\u{201d}: its rules about ratings dropped (ratings are no longer kept), the others kept");
            n += 1;
        }
    }
    Ok(n)
}

// ---- playlists ---------------------------------------------------------------

/// Every track's path and id (matching playlist files, `m3u`): tracks with a file (a server's
/// own tracks have no path a playlist file could hold).
pub fn track_paths(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut st = conn.prepare_cached("SELECT path, id FROM tracks WHERE path NOT LIKE 'navidrome:%'")?;
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

pub fn playlists(conn: &Connection) -> rusqlite::Result<Vec<Playlist>> {
    let mut st = conn.prepare_cached(
        "SELECT p.id, p.name, (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id), p.rules
         FROM playlists p ORDER BY p.name COLLATE NAMES, p.id",
    )?;
    st.query_map([], |r| Ok(Playlist { id: r.get(0)?, name: r.get(1)?, tracks: r.get(2)?, rules: r.get(3)? }))?.collect()
}

/// The names of the smart playlists whose rules name playlist `id` (deleting it leaves those
/// rules matching nothing: said before it goes).
pub fn smart_playlists_using(conn: &Connection, id: i64) -> rusqlite::Result<Vec<String>> {
    Ok(playlists(conn)?
        .into_iter()
        .filter(|p| p.id != id && p.rules.as_deref().is_some_and(|r| crate::smart::Smart::from_json(r).playlists_used().contains(&id)))
        .map(|p| p.name)
        .collect())
}

/// A new smart playlist with this rule set (`smart::Smart::to_json`).
pub fn create_smart_playlist(conn: &Connection, name: &str, rules: &str) -> rusqlite::Result<i64> {
    conn.execute("INSERT INTO playlists (name, created_at, rules) VALUES (?1, ?2, ?3)", params![name, now(), rules])?;
    Ok(conn.last_insert_rowid())
}

/// Replace a smart playlist's rules (an ordinary playlist is left as it is).
pub fn set_playlist_rules(conn: &Connection, id: i64, rules: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE playlists SET rules = ?2 WHERE id = ?1 AND rules IS NOT NULL", params![id, rules])?;
    Ok(())
}

/// Refuse to give a smart playlist tracks of its own (what it holds comes from its rules).
fn ordinary(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    match crate::smart::playlist_rules(conn, id)? {
        Some(Some(_)) => Err(rusqlite::Error::InvalidParameterName("a smart playlist holds no tracks of its own".into())),
        _ => Ok(()),
    }
}

pub fn create_playlist(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute("INSERT INTO playlists (name, created_at) VALUES (?1, ?2)", params![name, now()])?;
    Ok(conn.last_insert_rowid())
}

/// A new playlist holding `track_ids`, in one step: created with its tracks, or not at all.
pub fn create_playlist_with(conn: &Connection, name: &str, track_ids: &[i64]) -> rusqlite::Result<i64> {
    let tx = crate::db::write(conn)?;
    tx.execute("INSERT INTO playlists (name, created_at) VALUES (?1, ?2)", params![name, now()])?;
    let id = tx.last_insert_rowid();
    {
        let mut st = tx.prepare_cached("INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)")?;
        for (i, t) in track_ids.iter().enumerate() {
            st.execute(params![id, i as i64, t])?;
        }
    }
    tx.commit()?;
    Ok(id)
}

pub fn rename_playlist(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE playlists SET name = ?2 WHERE id = ?1", params![id, name])?;
    Ok(())
}

pub fn delete_playlist(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
    Ok(())
}

/// Append tracks to a playlist, all or none (one transaction).
/// Replace everything in the playlist with `track_ids`, in one transaction. The playlist
/// keeps its id, so a home shelf showing it keeps showing it.
pub fn set_playlist_tracks(conn: &Connection, id: i64, track_ids: &[i64]) -> rusqlite::Result<()> {
    ordinary(conn, id)?;
    let tx = crate::db::write(conn)?;
    tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
    {
        let mut st =
            tx.prepare_cached("INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)")?;
        for (i, t) in track_ids.iter().enumerate() {
            st.execute(params![id, i as i64, t])?;
        }
    }
    tx.commit()
}

pub fn add_to_playlist(conn: &Connection, id: i64, track_ids: &[i64]) -> rusqlite::Result<()> {
    ordinary(conn, id)?;
    // `db::write` works on a shared `&Connection`; it rolls back on drop unless committed,
    // so an error part way leaves the playlist as it was.
    let tx = crate::db::write(conn)?;
    let start: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_items WHERE playlist_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    {
        let mut st =
            tx.prepare_cached("INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)")?;
        for (i, t) in track_ids.iter().enumerate() {
            st.execute(params![id, start + i as i64, t])?;
        }
    }
    tx.commit()
}

/// Replace an ordinary playlist's tracks with `track_ids`, only if it holds `expected` now (the
/// list as it was shown when the change was asked for): one transaction, and `false` with
/// nothing written when the playlist changed meanwhile. Several rows removed or moved at once.
pub fn rewrite_playlist(conn: &Connection, id: i64, expected: &[i64], track_ids: &[i64]) -> rusqlite::Result<bool> {
    ordinary(conn, id)?;
    let tx = crate::db::write(conn)?;
    let now: Vec<i64> = {
        let mut st = tx.prepare_cached("SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position")?;
        st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    if now != expected {
        return Ok(false);
    }
    tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
    {
        let mut st = tx.prepare_cached("INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)")?;
        for (i, t) in track_ids.iter().enumerate() {
            st.execute(params![id, i as i64, t])?;
        }
    }
    tx.commit()?;
    Ok(true)
}

pub fn playlist_tracks(conn: &Connection, id: i64) -> rusqlite::Result<Vec<Track>> {
    query_tracks(
        conn,
        &format!(
            "SELECT {TRACK_COLS} FROM playlist_items i JOIN tracks t ON t.id = i.track_id
             JOIN albums a ON a.id = t.album_id WHERE i.playlist_id = ?1 ORDER BY i.position"
        ),
        [id],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn a_playlist_is_created_with_its_tracks_or_not_at_all() {
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES (1, 'k', 'A', 'B', 'M:/a', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at)
                 VALUES (1, 'M:/a/1', 1, 'One', 'B', 1, 0, 0, 0), (2, 'M:/a/2', 1, 'Two', 'B', 1, 0, 0, 0);",
        )
        .unwrap();
        let id = create_playlist_with(&conn, "Mix", &[2, 1]).unwrap();
        assert_eq!(playlist_tracks(&conn, id).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(playlist_name(&conn, id).unwrap().as_deref(), Some("Mix"));
        // A track that is not there (a foreign key failure) leaves no playlist behind.
        assert!(create_playlist_with(&conn, "Broken", &[1, 99]).is_err());
        assert_eq!(playlists(&conn).unwrap().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Mix"]);
    }

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, date, genre, label, folder, added_at) VALUES
                (1, 'a', 'Blackmagik Blazing', 'かめりあ', '2019-08-12', 'Hardcore', 'かめるかめりあ', 'M:/a', 100),
                (2, 'b', 'いただきバベル', '電音部', '2023-06-29', NULL, 'ASOBINOTES', 'M:/b', 300),
                (3, 'c', 'Untagged', 'Someone', NULL, NULL, NULL, 'M:/c', 200);
             INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at) VALUES
                (10, 'M:/a/01.opus', 1, 'B L A C K - R A Y', 'かめりあ', 1, 1, 278.0, 0, 0, 0),
                (11, 'M:/a/02.opus', 1, 'BRACE FOR FRICKING IMPACT', 'かめりあ', 1, 2, 264.0, 0, 0, 0),
                (20, 'M:/b/01.opus', 2, 'いただきバベル (Aiobahn Remix)', '港白金女学院、黒鉄たま', 1, 1, 200.0, 0, 0, 0),
                (30, 'M:/c/x.opus', 3, 'x', 'Someone feat. かめりあ', NULL, NULL, 10.0, 0, 0, 0);",
        )
        .unwrap();
        crate::relations::rebuild_from_strings(conn).unwrap();
    }

    /// Rows written by hand, then their artists and genres as the migration makes them.
    fn rows(c: &Connection, sql: &str) {
        c.execute_batch(sql).unwrap();
        crate::relations::rebuild_from_strings(c).unwrap();
    }

    fn artist_id(c: &Connection, name: &str) -> i64 {
        find_artist(c, name).unwrap().unwrap_or_else(|| panic!("no artist {name}")).id
    }

    fn names_of(v: &[ArtistRef]) -> Vec<&str> {
        v.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn tracks_come_by_id_in_one_query() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let got = tracks_by_id(&c, &[30, 10, 10, 99, -4, 20]).unwrap();
        let mut ids: Vec<i64> = got.keys().copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![10, 20, 30], "each once; not one the library lacks, nor a file from outside");
        for (id, t) in &got {
            assert_eq!(Some(t), track(&c, *id).unwrap().as_ref(), "the same as one by one");
        }
        assert!(tracks_by_id(&c, &[]).unwrap().is_empty());
    }

    #[test]
    fn shelves_order() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let ids = |v: Vec<Album>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(shelf(&c, &ShelfQuery::RecentlyAdded, 10).unwrap()), vec![2, 3, 1]);
        assert_eq!(ids(shelf(&c, &ShelfQuery::RecentlyReleased, 10).unwrap()), vec![2, 1, 3], "undated last");
        assert!(shelf(&c, &ShelfQuery::RecentlyPlayed, 10).unwrap().is_empty());
        record_play(&c, 10).unwrap();
        record_play(&c, 11).unwrap();
        record_play(&c, 20).unwrap();
        assert_eq!(ids(shelf(&c, &ShelfQuery::MostPlayed, 10).unwrap()), vec![1, 2]);
        assert_eq!(ids(shelf(&c, &ShelfQuery::Label { name: "asobinotes".into() }, 10).unwrap()), vec![2]);
        assert!(toggle_favourite_album(&c, 3).unwrap());
        assert_eq!(ids(shelf(&c, &ShelfQuery::Favourites, 10).unwrap()), vec![3]);
        assert!(!toggle_favourite_album(&c, 3).unwrap());
    }

    /// Plays the database could not take are kept, in order, and written once each when it can.
    #[test]
    fn plays_not_written_are_kept_and_written_once() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let mut pending = vec![(10, 100), (11, 200), (999, 250), (20, 300)];
        c.execute_batch("PRAGMA query_only = ON").unwrap();
        assert!(record_plays(&c, &mut pending).is_err());
        assert_eq!(pending.len(), 4, "nothing written, nothing lost");
        c.execute_batch("PRAGMA query_only = OFF").unwrap();
        // A play of a track the library no longer has (999) is dropped.
        assert_eq!(record_plays(&c, &mut pending).unwrap(), 3);
        assert!(pending.is_empty());
        assert_eq!(record_plays(&c, &mut pending).unwrap(), 0, "never twice");
        let plays: Vec<(i64, i64)> = c.prepare("SELECT track_id, played_at FROM plays ORDER BY played_at").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(plays, vec![(10, 100), (11, 200), (20, 300)], "at the time they were counted");
    }

    #[test]
    fn recently_released_follows_navidrome() {
        let c = db::open_in_memory().unwrap();
        c.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, date, original_date, release_date, folder, added_at) VALUES
                (1, 'a', 'Reissue', 'X', '2025-05-01', '1999-03-03', '2025-05-01', 'M:/a', 0),
                (2, 'b', 'New', 'X', '2024-10-10', NULL, NULL, 'M:/b', 0),
                (3, 'c', 'Year only', 'X', '2024', NULL, NULL, 'M:/c', 0),
                (4, 'd', 'Old', 'X', '2001-01-01', '', NULL, 'M:/d', 0);",
        )
        .unwrap();
        let ids = |v: Vec<Album>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
        // The reissue sorts by its original date; a bare year sorts before that
        // year's full dates (text order, as Navidrome's).
        assert_eq!(ids(shelf(&c, &ShelfQuery::RecentlyReleased, 10).unwrap()), vec![2, 3, 4, 1]);
    }

    #[test]
    fn played_shelves_and_random() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let ids = |v: Vec<Album>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
        // Album 1: two plays, earlier; album 2: two plays, later. Most played ties
        // break by the latest play; never-played albums are not on either shelf.
        c.execute_batch(
            "INSERT INTO plays (track_id, played_at) VALUES (10, 100), (11, 110), (20, 200), (20, 210);",
        )
        .unwrap();
        assert_eq!(ids(shelf(&c, &ShelfQuery::MostPlayed, 10).unwrap()), vec![2, 1]);
        assert_eq!(ids(shelf(&c, &ShelfQuery::RecentlyPlayed, 10).unwrap()), vec![2, 1]);
        // Random: one seed, one order (the shelf and its full page agree); every album
        // exactly once.
        let opts = |seed| ShelfOpts { seed, art_only: false };
        let a = ids(shelf_with(&c, &ShelfQuery::Random, 10, opts(7)).unwrap());
        assert_eq!(a, ids(shelf_with(&c, &ShelfQuery::Random, 10, opts(7)).unwrap()));
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3]);
        let orders: std::collections::HashSet<Vec<i64>> =
            (0..20).map(|s| ids(shelf_with(&c, &ShelfQuery::Random, 10, opts(s)).unwrap())).collect();
        assert!(orders.len() > 2, "different seeds shuffle differently");
        // Art only: none of these have art.
        assert!(shelf_with(&c, &ShelfQuery::Random, 10, ShelfOpts { seed: 1, art_only: true }).unwrap().is_empty());
    }

    #[test]
    fn search_handles_cjk_and_case() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let r = search(&c, "バベル", 10).unwrap();
        assert_eq!(r.albums.len(), 1);
        assert_eq!(r.tracks[0].id, 20);
        let r = search(&c, "black", 10).unwrap();
        assert_eq!(r.albums[0].id, 1);
        assert!(r.tracks.is_empty(), "spaced title must not match");
        assert_eq!(search(&c, "BRACE", 10).unwrap().tracks[0].id, 11);
        assert!(search(&c, "100%", 10).unwrap().albums.is_empty());
        // Folded: katakana typed as hiragana, half-width katakana, full-width Latin.
        assert_eq!(search(&c, "ばべる", 10).unwrap().tracks[0].id, 20);
        assert_eq!(search(&c, "ﾊﾞﾍﾞﾙ", 10).unwrap().tracks[0].id, 20);
        assert_eq!(search(&c, "ＢＲＡＣＥ", 10).unwrap().tracks[0].id, 11);
        // Romaji finds kana: ばべる from "babel"... is not romaji (l), but "ibaberu" is.
        assert!(search(&c, "babel", 10).unwrap().tracks.is_empty());
        assert_eq!(search(&c, "itadakibaberu", 10).unwrap().tracks[0].id, 20);
        assert_eq!(search(&c, "kameria", 10).unwrap().albums[0].id, 1, "かめりあ");
        // Short queries are not romaji-expanded (no flood of kana matches).
        assert!(search(&c, "kame", 10).unwrap().albums.iter().all(|a| a.id != 2));
    }

    #[test]
    fn sort_names_match_without_their_commas() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        c.execute("UPDATE albums SET album_artist_sort = 'Hanazawa, Kana' WHERE id = 3", []).unwrap();
        assert_eq!(search(&c, "hanazawa kana", 10).unwrap().albums[0].id, 3);
    }

    #[test]
    fn artist_page_includes_appearances() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let (own, appears) = artist_albums(&c, artist_id(&c, "かめりあ")).unwrap();
        assert_eq!(own.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(appears.iter().map(|a| a.id).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn artist_pages_match_whole_names_only() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, date, folder, added_at) VALUES
                (1, 'a', 'Kyougen', 'Ado', '2022-01-26', 'M:/a', 0),
                (2, 'b', 'Drag Album', 'Adore Delano', '2014-01-01', 'M:/b', 0),
                (3, 'c', 'Split', 'Someone', '2020-01-01', 'M:/c', 0),
                (4, 'd', 'Guests', 'Host', '2021-01-01', 'M:/d', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, track_no, duration, mtime, size, added_at) VALUES
                (10, 'M:/a/01.flac', 1, 'Usseewa', 'Ado', 1, 200.0, 0, 0, 0),
                (20, 'M:/b/01.flac', 2, 'Party', 'Adore Delano', 1, 200.0, 0, 0, 0),
                (21, 'M:/b/02.flac', 2, 'Duet', 'Adore Delano feat. Adorable', 2, 200.0, 0, 0, 0),
                (30, 'M:/c/01.flac', 3, 'Two', 'Someone;Ado', 1, 200.0, 0, 0, 0),
                (40, 'M:/d/01.flac', 4, 'Guest', 'Host (feat. Ado)', 1, 200.0, 0, 0, 0),
                (41, 'M:/d/02.flac', 4, 'Other', 'Host ft. Adolf', 2, 200.0, 0, 0, 0);",
        );
        let ids = |v: Vec<Album>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
        let ado = artist_id(&c, "Ado");
        let (own, appears) = artist_albums(&c, ado).unwrap();
        assert_eq!(ids(own), vec![1]);
        // A split value and a featured credit count; "Adore Delano" does not.
        assert_eq!(ids(appears), vec![4, 3]);
        let top: Vec<i64> = artist_top_tracks(&c, ado, 10).unwrap().iter().map(|t| t.id).collect();
        let mut sorted = top.clone();
        sorted.sort();
        assert_eq!(sorted, vec![10, 30, 40], "no tracks by Adore Delano or Adolf");
        // The limit still applies after the exact match, not before it.
        assert_eq!(artist_top_tracks(&c, ado, 2).unwrap().len(), 2);
        assert!(find_artist(&c, "Delano").unwrap().is_none(), "no artist of part of a name");
        let top: Vec<i64> = artist_top_tracks(&c, artist_id(&c, "Adore Delano"), 10).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(top.len(), 2, "{top:?}");
    }

    #[test]
    fn artists_are_found_by_key_or_name() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let a = find_artist(&c, "かめりあ").unwrap().unwrap();
        assert_eq!(find_artist(&c, &a.key).unwrap(), Some(a.clone()), "by key");
        assert_eq!(find_artist(&c, "  かめりあ ").unwrap(), Some(a), "by name, spaces ignored");
        assert!(find_artist(&c, "かめ").unwrap().is_none());
        assert_eq!(names_of(&track_artists(&c, 30).unwrap()), vec!["Someone", "かめりあ"]);
        assert!(track_artists(&c, 30).unwrap()[1].featured);
    }

    #[test]
    fn each_artist_of_a_shared_album_owns_it() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES (4, 'd', 'TAKE OVER', 'Yuta Imai;Kasane Teto', 'M:/d', 400);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                (40, 'M:/d/01.opus', 4, 'TAKE OVER', 'Yuta Imai;Kasane Teto', 178.0, 0, 0, 0);",
        );
        for name in ["Yuta Imai", "Kasane Teto"] {
            let (own, appears) = artist_albums(&c, artist_id(&c, name)).unwrap();
            assert_eq!(own.iter().map(|a| a.id).collect::<Vec<_>>(), vec![4], "{name}");
            assert!(appears.is_empty(), "{name}");
        }
        // A name that is only a substring of another artist does not own the album.
        assert!(find_artist(&c, "Yuta").unwrap().is_none());
    }

    #[test]
    fn accents_fold_on_both_sides() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES
                (1, 'a', 'Lemonade', 'Beyoncé', 'M:/a', 0),
                (2, 'b', 'Ágætis byrjun', 'Sigur Rós', 'M:/b', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                (10, 'M:/a/01.flac', 1, 'Formation', 'Beyoncé', 200.0, 0, 0, 0),
                (20, 'M:/b/01.flac', 2, 'Svefn-g-englar', 'Sigur Rós', 200.0, 0, 0, 0);",
        );
        let r = search(&c, "beyonce", 10).unwrap();
        assert_eq!(r.albums[0].id, 1);
        assert_eq!(names_of(&r.artists), vec!["Beyoncé"]);
        assert_eq!(r.tracks[0].id, 10);
        assert_eq!(search(&c, "BEYONCÉ", 10).unwrap().albums[0].id, 1, "typed with the accent too");
        assert_eq!(names_of(&search(&c, "sigur ros", 10).unwrap().artists), vec!["Sigur Rós"]);
        assert_eq!(search(&c, "agaetis", 10).unwrap().albums[0].id, 2, "æ is ae");
    }

    #[test]
    fn artists_include_track_artists_split_and_featured() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES
                (1, 'a', 'TAKE OVER', 'Yuta Imai;Kasane Teto', 'M:/a', 0),
                (2, 'b', 'Host Album', 'Host', 'M:/b', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                (10, 'M:/a/01.flac', 1, 'TAKE OVER', 'Yuta Imai;Kasane Teto', 200.0, 0, 0, 0),
                (20, 'M:/b/01.flac', 2, 'One', 'Host feat. Guest Star', 200.0, 0, 0, 0),
                (21, 'M:/b/02.flac', 2, 'Two', 'Host;Other Guest', 200.0, 0, 0, 0);",
        );
        // The matching artist of a shared album, not the whole value.
        assert_eq!(names_of(&search(&c, "teto", 10).unwrap().artists), vec!["Kasane Teto"]);
        // Only on tracks: a featured artist, and one of a split value.
        assert_eq!(names_of(&search(&c, "guest", 10).unwrap().artists), vec!["Guest Star", "Other Guest"]);
        // Album artists come first; "Host" is not repeated for its tracks.
        assert_eq!(names_of(&search(&c, "host", 10).unwrap().artists), vec!["Host"]);
    }

    #[test]
    fn sections_say_when_there_is_more() {
        let c = db::open_in_memory().unwrap();
        let mut sql = String::new();
        for i in 0..30 {
            sql.push_str(&format!(
                "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES ({i}, 'k{i}', 'Night {i}', 'Artist {i}', 'M:/{i}', 0);
                 INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES ({i}, 'M:/{i}/1.flac', {i}, 'Night track {i}', 'Artist {i}', 1.0, 0, 0, 0);"
            ));
        }
        rows(&c, &sql);
        let r = search(&c, "night", 24).unwrap();
        assert_eq!((r.albums.len(), r.tracks.len()), (24, 24));
        assert!(r.more_albums && r.more_tracks && !r.more_artists);
        let r = search(&c, "artist", 24).unwrap();
        assert!(r.more_artists);
        let all = search_section(&c, "night", SearchKind::Tracks, usize::MAX).unwrap();
        assert_eq!(all.tracks.len(), 30);
        assert!(all.albums.is_empty() && all.artists.is_empty(), "only the section asked for");
        assert!(!all.more_tracks);
        assert_eq!(search_section(&c, "artist", SearchKind::Artists, usize::MAX).unwrap().artists.len(), 30);
        let r = search(&c, "night 29", 24).unwrap();
        assert_eq!(r.albums.len(), 1);
        assert!(!r.more_albums);
    }

    #[test]
    fn albums_and_playlists_sort_as_people_sort_names() {
        let c = db::open_in_memory().unwrap();
        rows(
            &c,
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES
                (1, 'a', 'Vol. 10', 'Artist 10', 'M:/a', 1),
                (2, 'b', 'Vol. 2', 'artist 2', 'M:/b', 1),
                (3, 'c', 'vol. 1', 'Artist 9', 'M:/c', 1);",
        );
        let ids = |s| all_albums(&c, s, false).unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(AlbumSort::Artist), vec![2, 3, 1], "Artist 2, Artist 9, Artist 10, case ignored");
        assert_eq!(ids(AlbumSort::Title), vec![3, 2, 1], "Vol. 1, Vol. 2, Vol. 10");
        for name in ["Mix 10", "mix 2", "Mix 1"] {
            create_playlist(&c, name).unwrap();
        }
        let names: Vec<String> = playlists(&c).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Mix 1", "mix 2", "Mix 10"]);
    }

    #[test]
    fn smart_playlists_naming_a_playlist_are_found() {
        let c = db::open_in_memory().unwrap();
        let plain = create_playlist(&c, "Road trip").unwrap();
        let rules = format!(r#"{{"match":"all","rules":[{{"group":"any","rules":[{{"field":"playlist","op":"in","value":{plain}}}]}}],"v":1}}"#);
        c.execute("INSERT INTO playlists (name, created_at, rules) VALUES ('From the trip', 1, ?1)", [rules]).unwrap();
        c.execute("INSERT INTO playlists (name, created_at, rules) VALUES ('Jazz', 1, ?1)", [r#"{"match":"all","rules":[{"field":"genre","op":"is","value":"Jazz"}],"v":1}"#]).unwrap();
        assert_eq!(smart_playlists_using(&c, plain).unwrap(), vec!["From the trip".to_owned()]);
        assert!(smart_playlists_using(&c, plain + 100).unwrap().is_empty());
    }

    #[test]
    fn album_sorts() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, folder, added_at) VALUES
                (1, 'a', 'Beta', 'The Band', 'Band, The', '2001-01-01', 'M:/a', 300),
                (2, 'b', 'alpha', 'Zed', NULL, '1999', 'M:/b', 100),
                (3, 'c', 'Gamma', 'Abba', NULL, NULL, 'M:/c', 200);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                (10, 'M:/a/1.flac', 1, 'x', 'The Band', 1.0, 0, 0, 0),
                (20, 'M:/b/1.flac', 2, 'y', 'Zed', 1.0, 0, 0, 0),
                (30, 'M:/c/1.flac', 3, 'z', 'Abba', 1.0, 0, 0, 0);
             INSERT INTO plays (track_id, played_at) VALUES (20, 5), (20, 6), (10, 7);",
        );
        let ids = |s, d| all_albums(&c, s, d).unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(AlbumSort::Artist, false), vec![3, 1, 2], "by sort name: Abba, Band, Zed");
        assert_eq!(ids(AlbumSort::Artist, true), vec![2, 1, 3]);
        assert_eq!(ids(AlbumSort::Title, false), vec![2, 1, 3], "case ignored");
        assert_eq!(ids(AlbumSort::Released, true), vec![1, 2, 3], "undated last");
        assert_eq!(ids(AlbumSort::Released, false), vec![2, 1, 3], "undated last either way");
        assert_eq!(ids(AlbumSort::Added, true), vec![1, 3, 2]);
        assert_eq!(ids(AlbumSort::Added, false), vec![2, 3, 1]);
        assert_eq!(ids(AlbumSort::MostPlayed, true), vec![2, 1, 3], "two plays, one, none");
        assert_eq!(ids(AlbumSort::MostPlayed, false), vec![3, 1, 2]);
        assert!(!AlbumSort::Artist.default_desc() && AlbumSort::Added.default_desc());
    }

    #[test]
    fn browse_groups() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, album_artist_sort, date, genre, label, folder, art, added_at) VALUES
                (1, 'a', 'Old', 'Hanazawa Kana', 'Hanazawa, Kana', '2001', 'Pop', 'Label A', 'M:/a', 'old', 0),
                (2, 'b', 'New', 'Hanazawa Kana', 'Hanazawa, Kana', '2020', 'pop', '[no label]', 'M:/b', 'new', 0),
                (3, 'c', 'Shared', 'Yuta Imai;Kasane Teto', NULL, '2010', 'Vocaloid', NULL, 'M:/c', NULL, 0),
                (4, 'd', 'Solo', 'Abba', NULL, NULL, '', 'label a', 'M:/d', NULL, 0);",
        );
        let a = album_artists(&c).unwrap();
        let names: Vec<&str> = a.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Abba", "Hanazawa Kana", "Kasane Teto", "Yuta Imai"]);
        let kana = &a[1];
        assert_eq!((kana.albums, kana.art.as_deref()), (2, Some("new")), "art of the latest release");
        assert_eq!(a[2].albums, 1);
        let g = album_groups(&c, false).unwrap();
        assert_eq!(g.iter().map(|g| (g.name.as_str(), g.albums)).collect::<Vec<_>>(), vec![("Pop", 2), ("Vocaloid", 1)], "one genre whatever its case, in its most common spelling (ties: code point order)");
        let l = album_groups(&c, true).unwrap();
        assert_eq!(l.iter().map(|g| (g.name.as_str(), g.albums)).collect::<Vec<_>>(), vec![("Label A", 2)], "no placeholder, case merged");
        assert_eq!(album_folder(&c, 2).unwrap().as_deref(), Some("M:/b"));
    }

    #[test]
    fn browse_groups_sort_naturally() {
        let c = db::open_in_memory().unwrap();
        rows(&c,
            "INSERT INTO albums (id, key, title, album_artist, genre, label, folder, added_at) VALUES
                (1, 'a', 'A', 'Artist 10', 'Genre 10', 'Label 10', 'M:/a', 0),
                (2, 'b', 'B', 'artist 2', 'Genre 2', 'label 2', 'M:/b', 0),
                (3, 'c', 'C', 'Artist 1', 'genre 1', 'Label 1', 'M:/c', 0),
                (4, 'd', 'D', 'Band', 'Genre 9', 'Label 9', 'M:/d', 0);",
        );
        let names = |g: Vec<Group>| g.into_iter().map(|g| g.name).collect::<Vec<_>>();
        assert_eq!(names(album_artists(&c).unwrap()), vec!["Artist 1", "artist 2", "Artist 10", "Band"]);
        assert_eq!(names(album_groups(&c, false).unwrap()), vec!["genre 1", "Genre 2", "Genre 9", "Genre 10"]);
        assert_eq!(names(album_groups(&c, true).unwrap()), vec!["Label 1", "label 2", "Label 9", "Label 10"]);
    }

    #[test]
    fn a_playlist_can_be_replaced_whole() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let p = create_playlist(&c, "Mix").unwrap();
        add_to_playlist(&c, p, &[10, 20, 30]).unwrap();
        set_playlist_tracks(&c, p, &[11, 10]).unwrap();
        let ids: Vec<i64> = playlist_tracks(&c, p).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![11, 10]);
    }

    /// A smart playlist saved by a build that kept ratings: its rating rules are written away
    /// once, the others kept as they were; one without any is left untouched.
    #[test]
    fn smart_playlists_lose_their_rating_rules_and_keep_the_others() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let best = create_smart_playlist(
            &c,
            "Best",
            r#"{"v":1,"match":"all","rules":[{"field":"genre","op":"is","value":"Jazz"},{"field":"rating","op":"at_least","value":4}],"sort":{"by":"rating","desc":true},"seed":3}"#,
        )
        .unwrap();
        let plain = r#"{"v":1,"match":"all","rules":[{"field":"title","op":"contains","value":"rating"}],"seed":1}"#;
        let other = create_smart_playlist(&c, "Titles", plain).unwrap();
        assert_eq!(forget_rating_rules(&c).unwrap(), 1);
        let rules = |id| crate::smart::playlist_rules(&c, id).unwrap().unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&rules(best)).unwrap();
        assert_eq!(v, serde_json::json!({"v":1,"match":"all","rules":[{"field":"genre","op":"is","value":"Jazz"}],"seed":3}));
        assert_eq!(rules(other), plain, "nothing about ratings: not written");
        assert_eq!(forget_rating_rules(&c).unwrap(), 0, "once only");
        assert_eq!(playlists(&c).unwrap().len(), 2, "no playlist lost");
    }

    #[test]
    fn several_tracks_are_favourited_at_once() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        toggle_favourite_track(&c, 20).unwrap();
        assert_eq!(set_favourite_tracks(&c, &[10, 20, 30, 999], true).unwrap(), 2, "20 already was; 999 is no track");
        assert!([10, 20, 30].iter().all(|&t| is_favourite_track(&c, t).unwrap()));
        assert_eq!(set_favourite_tracks(&c, &[10, 30], false).unwrap(), 2);
        assert!(!is_favourite_track(&c, 10).unwrap() && is_favourite_track(&c, 20).unwrap());
    }

    #[test]
    fn a_playlist_is_rewritten_only_as_it_was_shown() {
        let c = db::open_in_memory().unwrap();
        seed(&c);
        let p = create_playlist(&c, "Mix").unwrap();
        add_to_playlist(&c, p, &[10, 20, 30, 11]).unwrap();
        let ids = |c: &Connection| -> Vec<i64> { playlist_tracks(c, p).unwrap().iter().map(|t| t.id).collect() };
        // Two rows removed at once.
        assert!(rewrite_playlist(&c, p, &[10, 20, 30, 11], &[10, 11]).unwrap());
        assert_eq!(ids(&c), vec![10, 11]);
        // The list on screen was older than the playlist: nothing written.
        assert!(!rewrite_playlist(&c, p, &[10, 20, 30, 11], &[30]).unwrap());
        assert_eq!(ids(&c), vec![10, 11]);
        assert_eq!(playlists(&c).unwrap()[0].tracks, 2);
    }
}
