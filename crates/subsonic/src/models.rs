//! What the server answers, typed: Subsonic API 1.16.1 with the OpenSubsonic additions, as
//! Navidrome sends them (and other servers, read tolerantly: `de`). Unknown fields are
//! ignored; a missing one is its default (empty text, None, 0, false, an empty list). Free
//! text (titles, names, the display strings) is a `String`, empty when absent; ids, dates and
//! numbers that may be unknown are `Option`s.
//!
//! Dates are as the server sends them: `created`, `starred` and `played` are ISO 8601 UTC
//! timestamps ("2024-05-01T10:00:00Z", Navidrome adds nanoseconds); release dates are
//! [`ItemDate`]s. Durations are seconds, bit rates kbps, sizes bytes.

use serde::{Deserialize, Serialize};

use crate::de;

/// The envelope every answer carries; `ping` answers with only this.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(deserialize_with = "de::string")]
    pub status: String,
    /// The Subsonic API version the server speaks ("1.16.1").
    #[serde(deserialize_with = "de::string")]
    pub version: String,
    /// OpenSubsonic: the server software ("navidrome"); empty on a plain Subsonic server.
    #[serde(rename = "type", deserialize_with = "de::string")]
    pub kind: String,
    /// OpenSubsonic: the server software's version ("0.53.3 (13af8ed4)").
    #[serde(deserialize_with = "de::string")]
    pub server_version: String,
    #[serde(deserialize_with = "de::flag")]
    pub open_subsonic: bool,
}

/// An OpenSubsonic extension the server supports (`songLyrics`, `transcodeOffset`, ...).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Extension {
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::list")]
    pub versions: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScanStatus {
    #[serde(deserialize_with = "de::flag")]
    pub scanning: bool,
    /// Files scanned (so far, while scanning).
    #[serde(deserialize_with = "de::opt_num")]
    pub count: Option<u64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub folder_count: Option<u64>,
    /// When the last scan ended: a change since the last sync is worth a look.
    #[serde(deserialize_with = "de::opt_string")]
    pub last_scan: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MusicFolder {
    /// A number on most servers, read as text.
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
}

/// An artist as named on a song or album (OpenSubsonic `artists`, `albumArtists`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArtistRef {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
}

/// A name in a list: OpenSubsonic `genres` (`{"name": "Rock"}`) and `recordLabels`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Named {
    #[serde(deserialize_with = "de::string")]
    pub name: String,
}

/// A contributor other than the artist (OpenSubsonic `contributors`): composer, producer...
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Contributor {
    #[serde(deserialize_with = "de::string")]
    pub role: String,
    #[serde(deserialize_with = "de::string")]
    pub sub_role: String,
    pub artist: ArtistRef,
}

/// An OpenSubsonic date: a year, maybe a month, maybe a day. Navidrome sends `{}` when it has
/// none, which reads as all None ([`ItemDate::is_empty`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItemDate {
    #[serde(deserialize_with = "de::opt_num")]
    pub year: Option<i32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub month: Option<u32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub day: Option<u32>,
}

impl ItemDate {
    pub fn is_empty(&self) -> bool {
        self.year.is_none()
    }
}

/// The title of one disc of an album (OpenSubsonic `discTitles`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscTitle {
    #[serde(deserialize_with = "de::num")]
    pub disc: u32,
    #[serde(deserialize_with = "de::string")]
    pub title: String,
}

/// ReplayGain in dB, peaks as fractions of full scale (OpenSubsonic `replayGain`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplayGain {
    #[serde(deserialize_with = "de::opt_num")]
    pub track_gain: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub album_gain: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub track_peak: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub album_peak: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub base_gain: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub fallback_gain: Option<f64>,
}

/// A song (Subsonic's "Child", with OpenSubsonic's fields).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Song {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub parent: Option<String>,
    #[serde(deserialize_with = "de::flag")]
    pub is_dir: bool,
    #[serde(deserialize_with = "de::string")]
    pub title: String,
    #[serde(deserialize_with = "de::string")]
    pub album: String,
    /// The artist as one display string ("A feat. B").
    #[serde(deserialize_with = "de::string")]
    pub artist: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub album_id: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub artist_id: Option<String>,
    #[serde(deserialize_with = "de::opt_num")]
    pub track: Option<u32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub disc_number: Option<u32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub year: Option<i32>,
    /// The first genre (Subsonic); `genres` has them all.
    #[serde(deserialize_with = "de::string")]
    pub genre: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub cover_art: Option<String>,
    /// Bytes of the original file.
    #[serde(deserialize_with = "de::opt_num")]
    pub size: Option<u64>,
    /// Of the original file ("audio/flac").
    #[serde(deserialize_with = "de::string")]
    pub content_type: String,
    /// Of the original file ("flac").
    #[serde(deserialize_with = "de::string")]
    pub suffix: String,
    /// What a plain `stream` would send, when the server's settings transcode it.
    #[serde(deserialize_with = "de::opt_string")]
    pub transcoded_content_type: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub transcoded_suffix: Option<String>,
    /// Seconds.
    #[serde(deserialize_with = "de::opt_num")]
    pub duration: Option<u32>,
    /// kbps.
    #[serde(deserialize_with = "de::opt_num")]
    pub bit_rate: Option<u32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub bit_depth: Option<u32>,
    /// Hz.
    #[serde(deserialize_with = "de::opt_num")]
    pub sampling_rate: Option<u32>,
    #[serde(deserialize_with = "de::opt_num")]
    pub channel_count: Option<u32>,
    /// The file's path on the server, relative to its music folder (Navidrome can be set to
    /// hide it). Not a path on this machine.
    #[serde(deserialize_with = "de::string")]
    pub path: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub created: Option<String>,
    /// When the user starred it; None when not starred.
    #[serde(deserialize_with = "de::opt_string")]
    pub starred: Option<String>,
    /// 1-5; None when not rated.
    #[serde(deserialize_with = "de::opt_num")]
    pub user_rating: Option<u8>,
    #[serde(deserialize_with = "de::opt_num")]
    pub average_rating: Option<f64>,
    #[serde(deserialize_with = "de::opt_num")]
    pub play_count: Option<u64>,
    /// When it was last played (OpenSubsonic).
    #[serde(deserialize_with = "de::opt_string")]
    pub played: Option<String>,
    /// "music", "podcast", "audiobook", "video".
    #[serde(rename = "type", deserialize_with = "de::string")]
    pub kind: String,
    /// OpenSubsonic: "song", "album", "artist".
    #[serde(deserialize_with = "de::string")]
    pub media_type: String,
    #[serde(deserialize_with = "de::opt_num")]
    pub bpm: Option<u32>,
    #[serde(deserialize_with = "de::string")]
    pub comment: String,
    #[serde(deserialize_with = "de::string")]
    pub sort_name: String,
    /// The MusicBrainz recording id; empty when untagged.
    #[serde(deserialize_with = "de::string")]
    pub music_brainz_id: String,
    #[serde(deserialize_with = "de::list")]
    pub isrc: Vec<String>,
    #[serde(deserialize_with = "de::list")]
    pub genres: Vec<Named>,
    #[serde(deserialize_with = "de::list")]
    pub artists: Vec<ArtistRef>,
    #[serde(deserialize_with = "de::string")]
    pub display_artist: String,
    #[serde(deserialize_with = "de::list")]
    pub album_artists: Vec<ArtistRef>,
    #[serde(deserialize_with = "de::string")]
    pub display_album_artist: String,
    #[serde(deserialize_with = "de::list")]
    pub contributors: Vec<Contributor>,
    #[serde(deserialize_with = "de::string")]
    pub display_composer: String,
    #[serde(deserialize_with = "de::list")]
    pub moods: Vec<String>,
    #[serde(deserialize_with = "de::opt_object")]
    pub replay_gain: Option<ReplayGain>,
    /// "explicit", "clean" or empty.
    #[serde(deserialize_with = "de::string")]
    pub explicit_status: String,
}

/// An album (Subsonic's "AlbumID3", with OpenSubsonic's fields). `song` is filled by
/// `getAlbum` only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Album {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    /// OpenSubsonic: the edition ("Deluxe Edition").
    #[serde(deserialize_with = "de::string")]
    pub version: String,
    /// The album artist as one display string.
    #[serde(deserialize_with = "de::string")]
    pub artist: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub artist_id: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub cover_art: Option<String>,
    #[serde(deserialize_with = "de::num")]
    pub song_count: u32,
    /// Seconds.
    #[serde(deserialize_with = "de::num")]
    pub duration: u32,
    #[serde(deserialize_with = "de::opt_num")]
    pub play_count: Option<u64>,
    #[serde(deserialize_with = "de::opt_string")]
    pub created: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub starred: Option<String>,
    #[serde(deserialize_with = "de::opt_num")]
    pub year: Option<i32>,
    #[serde(deserialize_with = "de::string")]
    pub genre: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub played: Option<String>,
    #[serde(deserialize_with = "de::opt_num")]
    pub user_rating: Option<u8>,
    #[serde(deserialize_with = "de::list")]
    pub record_labels: Vec<Named>,
    /// The MusicBrainz release id; empty when untagged.
    #[serde(deserialize_with = "de::string")]
    pub music_brainz_id: String,
    #[serde(deserialize_with = "de::list")]
    pub genres: Vec<Named>,
    /// The album artists.
    #[serde(deserialize_with = "de::list")]
    pub artists: Vec<ArtistRef>,
    #[serde(deserialize_with = "de::string")]
    pub display_artist: String,
    /// "album", "compilation", "live", "ep"... (MusicBrainz release types, lowercase on
    /// Navidrome).
    #[serde(deserialize_with = "de::list")]
    pub release_types: Vec<String>,
    #[serde(deserialize_with = "de::list")]
    pub moods: Vec<String>,
    #[serde(deserialize_with = "de::string")]
    pub sort_name: String,
    #[serde(deserialize_with = "de::opt_object")]
    pub original_release_date: Option<ItemDate>,
    #[serde(deserialize_with = "de::opt_object")]
    pub release_date: Option<ItemDate>,
    #[serde(deserialize_with = "de::flag")]
    pub is_compilation: bool,
    #[serde(deserialize_with = "de::string")]
    pub explicit_status: String,
    #[serde(deserialize_with = "de::list")]
    pub disc_titles: Vec<DiscTitle>,
    #[serde(deserialize_with = "de::list")]
    pub song: Vec<Song>,
}

/// An artist (Subsonic's "ArtistID3"). `album` is filled by `getArtist` only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Artist {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub cover_art: Option<String>,
    /// An image URL of the server's (Navidrome: its own `/share/img/...` link).
    #[serde(deserialize_with = "de::opt_string")]
    pub artist_image_url: Option<String>,
    #[serde(deserialize_with = "de::num")]
    pub album_count: u32,
    #[serde(deserialize_with = "de::opt_string")]
    pub starred: Option<String>,
    #[serde(deserialize_with = "de::opt_num")]
    pub user_rating: Option<u8>,
    #[serde(deserialize_with = "de::string")]
    pub music_brainz_id: String,
    #[serde(deserialize_with = "de::string")]
    pub sort_name: String,
    /// OpenSubsonic: what the artist is in the library ("albumartist", "artist", "composer").
    #[serde(deserialize_with = "de::list")]
    pub roles: Vec<String>,
    #[serde(deserialize_with = "de::list")]
    pub album: Vec<Album>,
}

/// One letter of `getArtists`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArtistIndex {
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::list")]
    pub artist: Vec<Artist>,
}

/// `getArtists`: the artists by letter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Artists {
    /// "The El La Los Las Le Les": the articles the server ignores when sorting.
    #[serde(deserialize_with = "de::string")]
    pub ignored_articles: String,
    #[serde(deserialize_with = "de::list")]
    pub index: Vec<ArtistIndex>,
}

impl Artists {
    /// Every artist, letter by letter.
    pub fn all(&self) -> impl Iterator<Item = &Artist> {
        self.index.iter().flat_map(|i| &i.artist)
    }
}

/// `search3` and `getStarred2`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchResult {
    #[serde(deserialize_with = "de::list")]
    pub artist: Vec<Artist>,
    #[serde(deserialize_with = "de::list")]
    pub album: Vec<Album>,
    #[serde(deserialize_with = "de::list")]
    pub song: Vec<Song>,
}

/// A playlist. `entry` is filled by `getPlaylist` only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Playlist {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::string")]
    pub comment: String,
    #[serde(deserialize_with = "de::string")]
    pub owner: String,
    #[serde(deserialize_with = "de::flag")]
    pub public: bool,
    #[serde(deserialize_with = "de::num")]
    pub song_count: u32,
    /// Seconds.
    #[serde(deserialize_with = "de::num")]
    pub duration: u32,
    #[serde(deserialize_with = "de::opt_string")]
    pub created: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub changed: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub cover_art: Option<String>,
    /// OpenSubsonic: the user may not change it (a smart playlist, someone else's).
    #[serde(deserialize_with = "de::flag")]
    pub readonly: bool,
    #[serde(deserialize_with = "de::list")]
    pub entry: Vec<Song>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Genre {
    /// The name.
    #[serde(deserialize_with = "de::string")]
    pub value: String,
    #[serde(deserialize_with = "de::num")]
    pub song_count: u32,
    #[serde(deserialize_with = "de::num")]
    pub album_count: u32,
}

/// One line of lyrics; `start` in milliseconds when synced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LyricLine {
    #[serde(deserialize_with = "de::opt_num")]
    pub start: Option<i64>,
    #[serde(deserialize_with = "de::string")]
    pub value: String,
}

/// One entry of OpenSubsonic `getLyricsBySongId`: Navidrome gives one per lyrics tag or
/// `.lrc` file it found, synced or not, maybe in several languages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StructuredLyrics {
    /// ISO 639 ("eng"), "xxx" or "und" when unknown.
    #[serde(deserialize_with = "de::string")]
    pub lang: String,
    #[serde(deserialize_with = "de::flag")]
    pub synced: bool,
    #[serde(deserialize_with = "de::list")]
    pub line: Vec<LyricLine>,
    /// Milliseconds to add to each start.
    #[serde(deserialize_with = "de::opt_num")]
    pub offset: Option<i64>,
    #[serde(deserialize_with = "de::string")]
    pub display_artist: String,
    #[serde(deserialize_with = "de::string")]
    pub display_title: String,
}
