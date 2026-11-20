//! Fixtures for the metadata viewer (`MP_BENCH=metadata`, `tests/meta.rs`): audio made by
//! ffmpeg, tagged by lofty, all inside the folder given (it must not exist yet, or be one
//! this tool made: it is emptied first). Never point it at a real music folder.
//!
//!     cargo run --release -p library --example meta_fixtures -- %TEMP%\mp-meta\music
//!
//! - `Picard Set`: FLAC with a full MusicBrainz Picard tag set (multi-valued artists and
//!   genres, sort names, ids, ReplayGain, synced lyrics, a front cover and an artist
//!   picture), its date written "2019.03.08" (the scanner makes it ISO).
//! - `ID3 Formats`: MP3 with ID3v2.4 (multi-valued frames, UFID, COMM, USLT, APIC), and
//!   MP3 with ID3v2.3 plus an ID3v1 tag that disagrees (title cut at 30, another genre).
//! - `Other Formats`: Opus with R128 gains, AAC in MP4 with iTunes freeform atoms and a
//!   cover. `Untagged`: a WAV with no tags at all.
//! - `Inconsistent Album`: one MusicBrainz release whose tracks disagree on album, album
//!   artist, date and genre, with a gap and a missing track number, one MP3 among FLACs,
//!   and one track without ReplayGain.
//! - `Edge Cases`: a 60,000-character comment, 3,000 extra items and a large (multi-megabyte) picture in
//!   one file; `02 Gone.flac` and `03 Corrupt.flac` for the bench to delete and break after
//!   the scan.
//! - `Big Album`: 400 tracks of one release.

#[path = "../tests/support/meta_fixtures.rs"]
mod fixtures;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: meta_fixtures <empty folder>"));
    fixtures::make(&dir);
    println!("fixtures in {}", dir.display());
}
