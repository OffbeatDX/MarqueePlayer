//! Lyrics from LRCLIB (lrclib.net, <https://lrclib.net/docs>) for tracks that have none
//! of their own. No UI here.
//!
//! - [`Client`]: the exact lookup (`GET /api/get`: artist, title, album, duration), then,
//!   when that finds nothing, the search (`GET /api/search`), whose results are taken only
//!   when the artist and title match after the library's search folding and the length is
//!   within [`DURATION_SLACK`] seconds. A wrong lyric is worse than none.
//! - [`Cache`]: every answer on disk (one small JSON file per track, never in the music
//!   folders or the database), keyed by the folded artist, title, album and the length
//!   in whole seconds, so a retag or another edit of the song is a different entry. "Not
//!   found" expires after [`NOT_FOUND_TTL`]; a lyric the user rejected stays rejected
//!   until they ask again. Network errors are never cached.
//! - [`Worker`]: one thread that waits a moment before going to the network (skipping
//!   through tracks sends nothing), sends one request at a time, honours `Retry-After`,
//!   and hands back only the answer for the newest request (older ones are still cached).

mod cache;
mod client;
mod worker;

pub use cache::{Cache, Cached, NOT_FOUND_TTL};
pub use client::{Client, DURATION_SLACK, Error, Record, acceptable, matches};
pub use worker::{Delivery, Worker};

/// What is looked up: the track's own tags.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackInfo {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Seconds.
    pub duration: f64,
}

impl TrackInfo {
    /// Worth asking about: an artist, a title and a length the service accepts (1 s to an
    /// hour; it matches on the length, and without one a wrong edit could come back).
    pub fn lookable(&self) -> bool {
        !self.artist.trim().is_empty() && !self.title.trim().is_empty() && (1.0..=3600.0).contains(&self.duration)
    }
}

/// The answer for a track.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Lyrics, synced when the service had synced ones (`lyrics.synced`). `id` is the
    /// service's record, so a rejected one is not taken again.
    Found { id: i64, lyrics: library::lyrics::Lyrics },
    /// The service says the track has no words.
    Instrumental { id: i64 },
    NotFound,
    /// The user said these lyrics were wrong; not looked up again until they ask.
    Rejected,
}

/// Fold a name as the library's search does (case, accents, kana, width), with runs of
/// white space made one.
pub fn fold(s: &str) -> String {
    library::fold::fold(s).split_whitespace().collect::<Vec<_>>().join(" ")
}
