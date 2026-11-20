//! Settings > Playback > Crossfade: when one track fades into the next, and when the change
//! stays gapless. The engine does the mixing (`player_core::crossfade`); this decides it
//! per transition (`playback::sync_next`) and for a skip (`playback::play_current`).
//!
//! The rule, with crossfade on:
//! - A track that ends goes into the next one with a crossfade of the chosen length (1 to
//!   12 s), equal power, the engine keeping it within a fifth of either track (so a short
//!   track overlaps less, and one under 2.5 s not at all).
//! - Except, with "Keep albums gapless" on (the default): the next track is the one after
//!   it on the same album, in the album's order (disc, number, file name, as the album page
//!   lists it). That is how albums mixed to run into each other are played: live albums,
//!   DJ mixes, concept albums, classical movements. Two tracks of one album out of order
//!   (shuffle, a playlist that skips around) crossfade like any others. Files opened from
//!   outside the library count as one album when they are in the same folder with the
//!   same album tag and numbered one after the other.
//! - Repeat one plays the track again gaplessly: a loop of itself, not a new track.
//! - A skip (next, previous, a track chosen in a list or the queue) while playing
//!   crossfades briefly, half a second, from what is being heard, with "Crossfade when
//!   skipping" on (the default with crossfade). Pause, seek and stop keep their short fades.
//!
//! Off, every change is exactly as before: gapless, the engine told nothing new.

use library::Track;
use player_core::Transition;

use crate::queue::Item;
use crate::settings::CrossfadeSettings;

/// The crossfade a skip gets (seconds), at most.
pub const SKIP_SECS: f32 = 0.5;
pub const MIN_SECS: f32 = 1.0;
pub const MAX_SECS: f32 = 12.0;

/// How the engine goes from `cur` to `next` when `cur` ends. `album_next`: `next` is the
/// track after `cur` on their album, in its order (`album_successor`).
pub fn transition(cfg: &CrossfadeSettings, cur: &Item, next: &Item, album_next: bool) -> Transition {
    if !cfg.enabled || next.uid == cur.uid || (cfg.album_gapless && album_next) {
        return Transition::Gapless;
    }
    Transition::Crossfade { secs: cfg.secs.clamp(MIN_SECS, MAX_SECS) }
}

/// The crossfade of a skip, if it gets one.
pub fn skip_secs(cfg: &CrossfadeSettings) -> Option<f32> {
    (cfg.enabled && cfg.on_skip).then(|| SKIP_SECS.min(cfg.secs))
}

/// Whether `next` comes right after `cur` on their album. `album`: the album's tracks in
/// its order, for library tracks (`query::album_tracks`).
pub fn album_successor(cur: &Track, next: &Track, album: &[Track]) -> bool {
    if cur.id > 0 && next.id > 0 {
        if cur.album_id != next.album_id {
            return false;
        }
        let Some(i) = album.iter().position(|t| t.id == cur.id) else { return false };
        return album.get(i + 1).is_some_and(|t| t.id == next.id);
    }
    // Files from outside the library: one folder, one album tag, numbered in a row.
    let folder = |t: &Track| std::path::Path::new(&t.path).parent().map(|p| p.to_string_lossy().to_lowercase());
    if cur.album.trim().is_empty() || !cur.album.trim().eq_ignore_ascii_case(next.album.trim()) || folder(cur) != folder(next) {
        return false;
    }
    let (Some(a), Some(b)) = (cur.track_no, next.track_no) else { return false };
    let (da, db) = (cur.disc_no.unwrap_or(1), next.disc_no.unwrap_or(1));
    (da == db && b == a + 1) || (db == da + 1 && b == 1)
}

/// "6 s"
pub fn secs_text(secs: f32) -> String {
    format!("{} s", secs.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, album_id: i64, disc: Option<u32>, no: Option<u32>) -> Track {
        Track {
            id,
            album_id,
            path: format!("D:/Music/A/{id}.flac"),
            title: format!("T{id}"),
            artist: "A".into(),
            album: "Album".into(),
            album_artist: "A".into(),
            disc_no: disc,
            track_no: no,
            duration: 200.0,
            art: None,
            audio: Default::default(),
            remote_id: None,
        }
    }

    fn item(uid: u64, t: Track) -> Item {
        Item { uid, track: t }
    }

    #[test]
    fn albums_in_order_stay_gapless_and_everything_else_crossfades() {
        let album = vec![track(1, 10, Some(1), Some(1)), track(2, 10, Some(1), Some(2)), track(3, 10, Some(2), Some(1))];
        let other = track(7, 11, Some(1), Some(1));
        let on = CrossfadeSettings { enabled: true, ..CrossfadeSettings::default() };
        let xf = Transition::Crossfade { secs: 6.0 };
        let (a1, a2, a3) = (item(1, album[0].clone()), item(2, album[1].clone()), item(3, album[2].clone()));
        let succ = |x: &Item, y: &Item| album_successor(&x.track, &y.track, &album);
        assert!(succ(&a1, &a2) && succ(&a2, &a3), "track 2 follows 1, disc 2 follows disc 1");
        assert!(!succ(&a1, &a3) && !succ(&a3, &a1) && !succ(&a2, &a1), "skipping ahead or going back is not album order");
        assert_eq!(transition(&on, &a1, &a2, succ(&a1, &a2)), Transition::Gapless);
        assert_eq!(transition(&on, &a1, &a3, succ(&a1, &a3)), xf, "shuffle within an album crossfades");
        let o = item(9, other.clone());
        assert!(!album_successor(&a3.track, &other, &album));
        assert_eq!(transition(&on, &a3, &o, false), xf, "another album crossfades");
        // The setting off: albums crossfade too.
        let all = CrossfadeSettings { album_gapless: false, ..on };
        assert_eq!(transition(&all, &a1, &a2, true), xf);
        // Repeat one: the same entry again, gapless.
        assert_eq!(transition(&on, &a1, &a1, false), Transition::Gapless);
        // Crossfade off: always gapless.
        assert_eq!(transition(&CrossfadeSettings::default(), &a3, &o, false), Transition::Gapless);
        // The length is held within 1 to 12 s.
        let long = CrossfadeSettings { secs: 40.0, ..on };
        assert_eq!(transition(&long, &a3, &o, false), Transition::Crossfade { secs: 12.0 });
    }

    #[test]
    fn files_from_outside_count_as_an_album_by_folder_tag_and_number() {
        let f = |no: u32, disc: Option<u32>, folder: &str, album: &str| Track {
            id: -(no as i64),
            album_id: 0,
            path: format!("C:/Down/{folder}/{no}.flac"),
            album: album.into(),
            disc_no: disc,
            track_no: Some(no),
            ..track(0, 0, None, None)
        };
        assert!(album_successor(&f(3, None, "x", "Live"), &f(4, None, "x", "live "), &[]));
        assert!(!album_successor(&f(3, None, "x", "Live"), &f(5, None, "x", "Live"), &[]));
        assert!(!album_successor(&f(3, None, "x", "Live"), &f(4, None, "y", "Live"), &[]));
        assert!(!album_successor(&f(3, None, "x", ""), &f(4, None, "x", ""), &[]), "no album tag: no album");
        assert!(album_successor(&f(9, Some(1), "x", "Live"), &f(1, Some(2), "x", "Live"), &[]));
    }

    #[test]
    fn skips_crossfade_briefly() {
        let on = CrossfadeSettings { enabled: true, ..CrossfadeSettings::default() };
        assert_eq!(skip_secs(&on), Some(0.5));
        assert_eq!(skip_secs(&CrossfadeSettings { on_skip: false, ..on }), None);
        assert_eq!(skip_secs(&CrossfadeSettings::default()), None);
    }
}
