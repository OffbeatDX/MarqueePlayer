//! Every answer on disk: `<dir>/<key>.json`, one per track. The key is a hash of the
//! folded artist, title and album and the length in whole seconds; the file also holds
//! those, and an entry whose fields do not match the track is ignored (a hash collision
//! never shows another song's words).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Answer, TrackInfo, fold};

/// Per-write counter so concurrent writers never share a temp file name.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A "not found" is asked again after this long: lyrics added to the service later are
/// found then.
pub const NOT_FOUND_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    artist: String,
    title: String,
    album: String,
    duration: i64,
    /// Unix seconds.
    fetched: u64,
    /// "found", "instrumental", "not_found" or "rejected".
    kind: String,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    synced: bool,
    /// The lyrics as the service sent them (LRC when synced).
    #[serde(default)]
    text: Option<String>,
    /// Records the user rejected for this track: never shown for it again.
    #[serde(default)]
    rejected: Vec<i64>,
}

/// What the cache holds for a track.
#[derive(Clone, Debug, PartialEq)]
pub struct Cached {
    /// `None`: nothing usable (missing, or a "not found" that has expired).
    pub answer: Option<Answer>,
    /// Records rejected for this track, to refuse on the next lookup.
    pub rejected: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// FNV-1a, 64 bits: stable across builds and platforms (std's hasher is not).
fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(Some(0)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn fields(t: &TrackInfo) -> (String, String, String, i64) {
        (fold(&t.artist), fold(&t.title), fold(&t.album), t.duration.round() as i64)
    }

    /// The file for this track.
    pub fn path(&self, t: &TrackInfo) -> PathBuf {
        let (a, ti, al, d) = Self::fields(t);
        self.dir.join(format!("{:016x}.json", fnv(&[&a, &ti, &al, &d.to_string()])))
    }

    fn read(&self, t: &TrackInfo) -> Option<Entry> {
        let e: Entry = serde_json::from_str(&std::fs::read_to_string(self.path(t)).ok()?).ok()?;
        let (a, ti, al, d) = Self::fields(t);
        (e.artist == a && e.title == ti && e.album == al && e.duration == d).then_some(e)
    }

    fn write(&self, t: &TrackInfo, mut e: Entry) {
        let (a, ti, al, d) = Self::fields(t);
        (e.artist, e.title, e.album, e.duration) = (a, ti, al, d);
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.path(t);
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
        match serde_json::to_string(&e) {
            Ok(text) => {
                if std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            Err(e) => log::warn!("lyrics cache: {e}"),
        }
    }

    pub fn get(&self, t: &TrackInfo) -> Cached {
        self.get_at(t, now())
    }

    /// As `get`, at a given time (Unix seconds), for the expiry.
    pub fn get_at(&self, t: &TrackInfo, now: u64) -> Cached {
        let Some(e) = self.read(t) else { return Cached { answer: None, rejected: Vec::new() } };
        let answer = match e.kind.as_str() {
            "found" => e.text.as_deref().map(|text| {
                let mut l = library::lyrics::parse(text, "lrclib");
                if !e.synced {
                    l.synced = false;
                    l.lines.iter_mut().for_each(|x| x.time_ms = None);
                }
                Answer::Found { id: e.id.unwrap_or(0), lyrics: l }
            }),
            "instrumental" => Some(Answer::Instrumental { id: e.id.unwrap_or(0) }),
            "not_found" if now.saturating_sub(e.fetched) < NOT_FOUND_TTL.as_secs() => Some(Answer::NotFound),
            "rejected" => Some(Answer::Rejected),
            _ => None,
        };
        Cached { answer, rejected: e.rejected }
    }

    /// Keep an answer (from the network). The track's rejected records stay with it.
    pub fn put(&self, t: &TrackInfo, a: &Answer) {
        self.put_at(t, a, now());
    }

    pub fn put_at(&self, t: &TrackInfo, a: &Answer, fetched: u64) {
        let rejected = self.read(t).map(|e| e.rejected).unwrap_or_default();
        let e = match a {
            Answer::Found { id, lyrics } => Entry {
                kind: "found".into(),
                id: Some(*id),
                synced: lyrics.synced,
                text: Some(to_text(lyrics)),
                ..Entry::default()
            },
            Answer::Instrumental { id } => Entry { kind: "instrumental".into(), id: Some(*id), ..Entry::default() },
            Answer::NotFound => Entry { kind: "not_found".into(), ..Entry::default() },
            Answer::Rejected => Entry { kind: "rejected".into(), ..Entry::default() },
        };
        self.write(t, Entry { fetched, rejected, ..e });
    }

    /// The user rejected what is shown for this track: its words go, and the track is not
    /// looked up again until `retry`.
    pub fn reject(&self, t: &TrackInfo) {
        let old = self.read(t).unwrap_or_default();
        let mut rejected = old.rejected;
        if let Some(id) = old.id.filter(|_| old.kind == "found" || old.kind == "instrumental")
            && !rejected.contains(&id)
        {
            rejected.push(id);
        }
        self.write(t, Entry { kind: "rejected".into(), fetched: now(), rejected, ..Entry::default() });
    }

    /// Before a lookup the user asked for: forget a "not found" or a rejection (the
    /// rejected records stay refused), so the next lookup goes to the network.
    pub fn retry(&self, t: &TrackInfo) -> Vec<i64> {
        let Some(e) = self.read(t) else { return Vec::new() };
        if e.rejected.is_empty() {
            let _ = std::fs::remove_file(self.path(t));
        } else {
            self.write(t, Entry { kind: "retry".into(), fetched: now(), rejected: e.rejected.clone(), ..Entry::default() });
        }
        e.rejected
    }
}

/// Lyrics back to text for the cache: LRC stamps when synced.
fn to_text(l: &library::lyrics::Lyrics) -> String {
    l.lines
        .iter()
        .map(|line| match line.time_ms {
            Some(ms) if l.synced => format!("[{:02}:{:02}.{:02}]{}", ms / 60_000, (ms / 1000) % 60, (ms % 1000) / 10, line.text),
            _ => line.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lrclib-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn t() -> TrackInfo {
        TrackInfo { artist: "Artist".into(), title: "Song".into(), album: "Album".into(), duration: 200.2 }
    }

    #[test]
    fn answers_round_trip_and_retags_miss() {
        let c = Cache::new(dir("rt"));
        assert_eq!(c.get(&t()).answer, None);
        let lyrics = library::lyrics::parse("[00:01.50]one\n[01:02.03]two", "lrclib");
        c.put(&t(), &Answer::Found { id: 7, lyrics: lyrics.clone() });
        assert_eq!(c.get(&t()).answer, Some(Answer::Found { id: 7, lyrics: lyrics.clone() }));
        // Folding: the same track however the case is written.
        let same = TrackInfo { artist: "ARTIST".into(), title: "song ".into(), ..t() };
        assert!(c.get(&same).answer.is_some());
        // A retag, another album, another edit: a different entry.
        for other in [
            TrackInfo { title: "Song (Live)".into(), ..t() },
            TrackInfo { album: "Single".into(), ..t() },
            TrackInfo { duration: 230.0, ..t() },
        ] {
            assert_eq!(c.get(&other).answer, None, "{other:?}");
        }
        let plain = library::lyrics::parse("a\nb", "lrclib");
        c.put(&t(), &Answer::Found { id: 8, lyrics: plain.clone() });
        assert_eq!(c.get(&t()).answer, Some(Answer::Found { id: 8, lyrics: plain }));
        let _ = std::fs::remove_dir_all(c.dir());
    }

    #[test]
    fn not_found_expires_and_rejection_sticks() {
        let c = Cache::new(dir("ttl"));
        c.put_at(&t(), &Answer::NotFound, 1_000);
        assert_eq!(c.get_at(&t(), 1_000 + 3600).answer, Some(Answer::NotFound));
        assert_eq!(c.get_at(&t(), 1_000 + NOT_FOUND_TTL.as_secs()).answer, None, "expired: look again");
        let lyrics = library::lyrics::parse("x", "lrclib");
        c.put(&t(), &Answer::Found { id: 42, lyrics });
        c.reject(&t());
        let got = c.get(&t());
        assert_eq!(got.answer, Some(Answer::Rejected));
        assert_eq!(got.rejected, vec![42]);
        // Asking again keeps the rejected record refused, and the next answer keeps it too.
        assert_eq!(c.retry(&t()), vec![42]);
        assert_eq!(c.get(&t()).answer, None);
        c.put(&t(), &Answer::NotFound);
        assert_eq!(c.get(&t()).rejected, vec![42]);
        let _ = std::fs::remove_dir_all(c.dir());
    }

    #[test]
    fn concurrent_put_and_reject_leave_a_valid_cache_file() {
        let c = Cache::new(dir("race"));
        let track = t();
        let long_text: String = (0..120).map(|i| format!("[{:02}:{:02}.00]Line {i} with enough words to span writes", i / 60, i % 60)).collect::<Vec<_>>().join("\n");
        let lyrics = library::lyrics::parse(&long_text, "lrclib");
        // Seed the file so readers always see a file on disk.
        c.put(&track, &Answer::Found { id: 1, lyrics: lyrics.clone() });

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let iters = 150;

        let c1 = c.clone();
        let t1 = track.clone();
        let b1 = barrier.clone();
        let putter = std::thread::spawn(move || {
            b1.wait();
            for i in 1..=iters {
                c1.put(&t1, &Answer::Found { id: i, lyrics: lyrics.clone() });
            }
        });

        let c2 = c.clone();
        let t2 = track.clone();
        let b2 = barrier.clone();
        let rejecter = std::thread::spawn(move || {
            b2.wait();
            for _ in 0..iters {
                c2.reject(&t2);
            }
        });

        let path = c.path(&track);
        let d3 = done.clone();
        let b3 = barrier.clone();
        let reader = std::thread::spawn(move || {
            b3.wait();
            while !d3.load(std::sync::atomic::Ordering::Acquire) {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    let parsed: Result<Entry, _> = serde_json::from_str(&raw);
                    assert!(parsed.is_ok(), "corrupt cache file read during concurrent writes: {raw:?}");
                }
            }
        });

        putter.join().unwrap();
        rejecter.join().unwrap();
        done.store(true, std::sync::atomic::Ordering::Release);
        reader.join().unwrap();

        let raw = std::fs::read_to_string(c.path(&track)).expect("cache file must exist");
        let _: Entry = serde_json::from_str(&raw).expect("final cache file must be valid JSON");
        let got = c.get(&track);
        assert!(matches!(got.answer, Some(Answer::Found { .. } | Answer::Rejected)), "unexpected final entry: {got:?}");
        let leftover: Vec<_> = std::fs::read_dir(c.dir())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("tmp"))
            .collect();
        assert!(leftover.is_empty(), "leftover temp files: {leftover:?}");
        let _ = std::fs::remove_dir_all(c.dir());
    }
}
