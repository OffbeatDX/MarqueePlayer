//! Folder scan: walk the roots, read tags of new or changed files in parallel, and write
//! them to the database a batch of whole album folders at a time, each batch in its own
//! transaction, caching the art of the albums each batch made or changed as it goes (see
//! `batch.rs`, which says what a batch boundary must not change and why it does not). A first
//! scan reads folders as the walk finds them, so the first albums are in within seconds; a
//! scan that is stopped (quit, a crash, the drive gone) keeps every batch it wrote, and the
//! next one reads only the rest (`Report::resumed`).
//!
//! Incremental: files whose mtime and size are unchanged are not re-read. Files that
//! vanished are pruned, but only under roots that still exist, so an unplugged drive
//! does not wipe its play history, and only files known to be gone: a track under a
//! folder the walk could not read (a share's error, no permission), or whose path
//! cannot be looked at, is held instead (`Held::unreadable`; see `batch.rs`).
//!
//! Moves and renames keep a track's history. A track's identity is its path, and
//! plays, favourites and playlist entries hang off its row, so a file that vanished
//! and a file that appeared in the same scan are first matched up (`match_moves`): a
//! match re-points the existing row to the new path, keeping its id. A wrong match
//! would hand one track's history to another, so the rules are strict and a match
//! has to be the only candidate both ways; anything doubtful is left unmatched (the
//! old row is pruned and the new file comes in fresh, as before).
//!
//! Mass removal is held back. A root that still exists but where nearly every known
//! track has gone missing (a share mounted empty, a drive letter pointing at an empty
//! folder, a library mid-reorganisation) is not pruned: its tracks stay, and the
//! report names the root (`Report::held`), until a later scan finds them again or a
//! scan with `Options::confirm_removal` removes them. See `GUARD_*`.
//!
//! Partial scans: the folder watcher hands over what changed, and a scan of only those
//! folders (`Options::only`, from `scopes`) reads and matches only there. A file that
//! appears in them can still be a track moved from anywhere else: vanished tracks of
//! the same duration and numbers are looked up library-wide (one query per new file,
//! and a check that the old file is really gone) and matched by the same rules. A
//! partial scan never removes a track: if anything under its folders went missing and
//! is not a move, it is dropped and a full scan runs instead, so the prune and the
//! mass-removal guard always see the whole library. The start-up scan and Rescan are
//! full scans.
//!
//! Art is looked for again only when something that could change the answer changed:
//! the album's files, or (for an album without art) the contents of its folder, by the
//! folders' modification times (`albums.art_checked`). And once after the art cache's
//! format changes (`ArtCache::needs_upgrade`), every album's art is re-cached from its
//! source; an album whose new files could not be written keeps its old ones, and the
//! next scan tries it again.
//!
//! Technical details (format, bit depth, sample rate, bitrate, channels; `audio.rs`) are
//! read with the tags of every file the scan reads. Tracks from before they were kept
//! (schema 7) have none: a full scan fills them afterwards (`fill_details`), reading only
//! the files' properties and writing only those columns, a batch at a time. It is not a
//! re-read: nothing else of the track changes, its album and art are not looked at
//! again, and the files count as found as before, so "recently added", the art and the
//! mass-removal guard see nothing different.
//!
//! "Recently added" (`albums.added_at`) is stamped once, when an album is first seen,
//! and kept through rescans and retags. An album whose identity changes (a retag that
//! adds a MusicBrainz id, an untagged album's folder renamed or moved) comes back as a
//! new row; it takes over the date, and the favourite, of the album it replaces, so it
//! does not jump to the front of Recently added. See `inherit_album`. The albums of a
//! first import (an empty library) are dated by their files instead (`SyncCtx::dated`).

mod batch;
mod walk;

// What a library is to its user, as text, for the batch tests (shared with tests/progressive.rs).
#[cfg(test)]
#[path = "../tests/support/scan_dump.rs"]
mod scan_dump;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::art::{self, ArtCache};
use crate::audio::AudioInfo;
use crate::db::{now, now_ms};
use crate::group;
use crate::names::{self, Credit};
use crate::relations::{self, Touched};
use crate::tags::{self, TrackTags};

/// What the scanner reads: every container Symphonia is built with here (Cargo.toml) and
/// lofty reads tags from. `.aac` is raw ADTS; `.m4b` an audiobook in MP4.
pub const AUDIO_EXTS: &[&str] = &["opus", "ogg", "oga", "flac", "mp3", "mp2", "wav", "aif", "aiff", "aifc", "aac", "m4a", "m4b", "mp4"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Looking for files (`Progress::found`).
    #[default]
    Walking,
    /// Reading tags and writing the library a batch at a time (`done` of `total` files).
    Reading,
    /// The covers still to come once every file is read (`done` of `total` albums).
    Art,
    /// Reading the technical details of tracks scanned before they were kept.
    Details,
    /// Reading again the tags of tracks scanned by an earlier version (`fill_tags`).
    Tags,
}

/// How far a scan is, told from its thread a few times a second and at every batch written.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
    /// The walk is still going (a first scan reads as it finds): `total` may still grow.
    pub looking: bool,
    /// Audio files the walk has found so far.
    pub found: usize,
    /// Albums in the library, as of the last batch written.
    pub albums: usize,
    /// This scan picks up one that did not finish.
    pub resumed: bool,
    /// The file being read, or the folder being listed, the longest, once that has taken a
    /// while (a slow or stalled disk).
    pub waiting_on: Option<PathBuf>,
    /// How long since a file was last read, while some are still to read.
    pub idle: Duration,
    /// Bumped whenever the scan wrote to the library (a batch, covers): views look again.
    pub changes: usize,
    /// Batches written so far.
    pub batches: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub files: usize,
    pub read: usize,
    pub removed: usize,
    /// Tracks whose file moved or was renamed, re-pointed to the new path.
    pub moved: usize,
    /// Missing tracks kept back: roots, by the mass-removal guard; folders that could not be
    /// read (`Held::unreadable`).
    pub held: Vec<Held>,
    pub albums_changed: usize,
    pub errors: Vec<(PathBuf, String)>,
    pub seconds: f64,
    /// A partial scan (`Options::only`) that stayed partial. It has nothing to say about
    /// the guard: `held` names only what it could not read, and nothing else held then
    /// means nothing.
    pub partial: bool,
    /// Tracks whose technical details were filled in (`fill_details`).
    pub details: usize,
    /// Tracks whose tags were read again for this version of the scanner (`fill_tags`).
    pub tags: usize,
    /// Stopped before the end (`Options::cancel`): what was written stays, the next scan reads
    /// the rest; nothing was removed.
    pub interrupted: bool,
    /// Stopped in the covers, after every file was read and written (and the prune done): the
    /// albums whose art was not looked at yet are looked at by the next scan. Not finished, for
    /// all that: the app offers Resume, as for `interrupted`.
    pub covers_cut: bool,
    /// Picked up a scan that had not finished.
    pub resumed: bool,
    /// Transactions the scan wrote the library in.
    pub batches: usize,
    /// The longest of them, in seconds: how long a write of the app's own (a play counted, a
    /// rating) could have waited for one.
    pub longest_write: f64,
}

/// Where missing tracks were kept in the library: a root the guard held, or a folder (or a
/// file) that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub root: PathBuf,
    /// Tracks under it that this scan did not find.
    pub missing: usize,
    /// Tracks under it before the scan.
    pub known: usize,
    /// `root` could not be read (the error): its tracks were not seen rather than found gone.
    /// They stay until it reads again; `Options::confirm_removal` does not remove them.
    /// `None`: the mass-removal guard.
    pub unreadable: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Remove missing tracks even where the mass-removal guard would keep them (the
    /// user confirmed it).
    pub confirm_removal: bool,
    /// Scan only these folders and what is below them (see `scopes`), not every root.
    /// Never removes a track: becomes a full scan instead (see the module doc).
    pub only: Option<Vec<PathBuf>>,
    /// Set to stop the scan: what is read is written, nothing more is read, nothing is removed,
    /// and the next scan picks up from there.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Tests: write a batch exactly when this many files are ready (whole folders), never by
    /// time, and read no further ahead than the batch being formed.
    pub batch_files: Option<usize>,
    /// A slow disk, simulated: every folder listed and file opened takes this much longer, one
    /// at a time (default: `MP_SLOW_DISK_MS`, else none).
    pub slow_disk: Option<Duration>,
}

/// Most folders a partial scan takes; more changed than that, and a full scan is
/// about as quick.
const MAX_SCOPES: usize = 64;

/// The folders to scan for these changed paths (from the folder watcher): a changed
/// file's folder, a changed (or vanished) folder itself; nested ones folded into their
/// parent, paths outside the library dropped. `None` when only a full scan will do: a
/// root itself changed, or too many folders did.
pub fn scopes(changed: &[PathBuf], roots: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let roots = &outermost(roots);
    let mut dirs: Vec<PathBuf> = Vec::new();
    for p in changed {
        // A path that is gone may have been a file or a folder: its extension says
        // which it was when it is audio or an image; otherwise it is taken as a folder
        // (scanning a folder that does not exist finds only what went missing in it).
        let dir = if is_audio(p) || art::is_image(p) || p.is_file() { p.parent()?.to_path_buf() } else { p.clone() };
        let Some(root) = roots.iter().find(|r| dir.starts_with(r)) else { continue };
        if dir == *root {
            return None;
        }
        dirs.push(dir);
    }
    // Sorted, a folder comes before everything inside it.
    dirs.sort();
    dirs.dedup();
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !out.iter().any(|o| d.starts_with(o)) {
            out.push(d);
        }
    }
    (out.len() <= MAX_SCOPES).then_some(out)
}

/// The mass-removal guard holds a root's missing tracks when at least this many are
/// missing...
pub const GUARD_MIN_MISSING: usize = 25;
/// ...and fewer than 1 in `GUARD_FOUND_RATIO` of the root's known tracks were found.
/// Deleting a few albums, or even most of a library, prunes as usual; only a root that
/// comes back (nearly) empty is held.
pub const GUARD_FOUND_RATIO: usize = 10;

/// A vanished track (or a new file), as `match_moves` compares them.
#[derive(Clone, Debug)]
struct Candidate {
    path: String,
    title: String,
    artist: String,
    album: String,
    disc_no: Option<i64>,
    track_no: Option<i64>,
    duration: f64,
    size: i64,
}

impl Candidate {
    fn file_name(&self) -> String {
        Path::new(&self.path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
    }
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Durations agree within 50 ms. Retagging does not touch the audio, and moving does
/// not touch anything, so the same file reads the same duration.
fn same_duration(a: f64, b: f64) -> bool {
    a > 0.0 && b > 0.0 && (a - b).abs() <= 0.05
}

/// Whether `new` can be the file `old` was, moved or renamed (and perhaps retagged).
/// Always: the same duration, title, and disc and track numbers. Then one of: the
/// same size (the file itself is unchanged, only its path), the same file name (a
/// retag plus a move into another folder), or the same artist and album (renamed and
/// retagged, as a tagger's "rename files" does).
fn could_be(old: &Candidate, new: &Candidate) -> bool {
    if !same_duration(old.duration, new.duration)
        || !same_text(&old.title, &new.title)
        || old.disc_no != new.disc_no
        || old.track_no != new.track_no
    {
        return false;
    }
    old.size == new.size
        || old.file_name() == new.file_name()
        || (same_text(&old.artist, &new.artist) && same_text(&old.album, &new.album))
}

/// Pair vanished tracks (`old`, by track id) with new files (`new`) that are the same
/// file moved or renamed. Returns new path index -> old index.
///
/// First, on Windows and macOS, a path that differs only in letter case is the same file
/// (`FOLDS_CASE`: a folder renamed "old" -> "Old"). Then `could_be`, and
/// only where the pairing is unique both ways: two vanished tracks that both fit one
/// new file, or one vanished track that fits two new files, match nothing.
fn match_moves(old: &[Candidate], new: &[Candidate]) -> HashMap<usize, usize> {
    let mut out: HashMap<usize, usize> = HashMap::new();
    let mut old_taken = vec![false; old.len()];
    if FOLDS_CASE {
        let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, o) in old.iter().enumerate() {
            by_path.entry(o.path.to_lowercase()).or_default().push(i);
        }
        let mut new_by_path: HashMap<String, Vec<usize>> = HashMap::new();
        for (j, n) in new.iter().enumerate() {
            new_by_path.entry(n.path.to_lowercase()).or_default().push(j);
        }
        for (k, js) in &new_by_path {
            if let (Some(is), [j]) = (by_path.get(k), js.as_slice())
                && let [i] = is.as_slice()
            {
                out.insert(*j, *i);
                old_taken[*i] = true;
            }
        }
    }
    // Compare only within the same title and numbers (all required by `could_be`).
    type Bucket = (String, Option<i64>, Option<i64>);
    let bucket = |c: &Candidate| -> Bucket { (c.title.trim().to_lowercase(), c.disc_no, c.track_no) };
    let mut old_by: HashMap<Bucket, Vec<usize>> = HashMap::new();
    for (i, o) in old.iter().enumerate().filter(|(i, _)| !old_taken[*i]) {
        old_by.entry(bucket(o)).or_default().push(i);
    }
    let mut fits_new: HashMap<usize, Vec<usize>> = HashMap::new(); // new -> olds
    let mut fits_old: HashMap<usize, Vec<usize>> = HashMap::new(); // old -> news
    for (j, n) in new.iter().enumerate().filter(|(j, _)| !out.contains_key(j)) {
        for &i in old_by.get(&bucket(n)).map(Vec::as_slice).unwrap_or(&[]) {
            if could_be(&old[i], n) {
                fits_new.entry(j).or_default().push(i);
                fits_old.entry(i).or_default().push(j);
            }
        }
    }
    for (j, is) in fits_new {
        if let [i] = is.as_slice()
            && fits_old.get(i).is_some_and(|js| js.len() == 1)
        {
            out.insert(j, *i);
        }
    }
    out
}

pub(crate) struct FileInfo {
    path: PathBuf,
    mtime: i64,
    size: i64,
    /// The earliest the file is known to have been on disk (its creation, or its last change
    /// when earlier), Unix milliseconds: a first import's albums are dated by it.
    born: i64,
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A folder named for one disc of an album: "CD1", "Disc 2", "disk 3", "CD 1", "Disc_02",
/// "Disc 1 - The Early Years". Not "CDs", "Discovery" or "Disco 2000".
fn is_disc_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    let Some(rest) = ["disc", "disk", "cd"].iter().find_map(|p| n.strip_prefix(p)) else { return false };
    let rest = rest.trim_start_matches([' ', '_', '-', '.']);
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    (1..=3).contains(&digits) && rest[digits..].chars().next().is_none_or(|c| matches!(c, ' ' | '-' | '_' | '.' | '(' | '['))
}

/// A folder named for one disc beside its siblings: "Live CD1", "Live (Disc 2)", "Live - CD
/// 3", "Live [CD2]" give "Live". The disc marker must follow a space, a hyphen, an
/// underscore or a bracket.
fn sibling_disc_stem(name: &str) -> Option<&str> {
    let n = name.trim_end();
    let s = n.strip_suffix([')', ']']).unwrap_or(n);
    let digits = s.bytes().rev().take_while(u8::is_ascii_digit).count();
    if !(1..=2).contains(&digits) {
        return None;
    }
    let s = s[..s.len() - digits].trim_end_matches([' ', '_', '-', '.']);
    let lower = s.to_ascii_lowercase();
    let marker = ["disc", "disk", "cd"].iter().find(|m| lower.ends_with(*m))?;
    let before = &s[..s.len() - marker.len()];
    if !before.ends_with([' ', '_', '-', '(', '[']) {
        return None;
    }
    let stem = before.trim_end_matches([' ', '_', '-', '(', '[']);
    (!stem.is_empty()).then_some(stem)
}

/// Folder that represents the album: the file's folder, or its parent when the file
/// sits in a "CD1" / "Disc 2" style subfolder. A real folder (the album's art is looked
/// for there, Explorer shows it).
pub fn album_folder(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new(""));
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if is_disc_dir(name) { dir.parent().unwrap_or(dir).to_path_buf() } else { dir.to_path_buf() }
}

/// The folder a track's album is grouped by (`group`): `album_folder`, and for a folder
/// named for its disc beside its siblings ("Live CD1" and "Live CD2") the name without the
/// disc ("Live"), so the discs are one album. Not necessarily a real folder.
pub fn group_folder(file: &Path) -> PathBuf {
    let folder = album_folder(file);
    if folder.as_path() == file.parent().unwrap_or(Path::new(""))
        && let Some(stem) = folder.file_name().and_then(|n| n.to_str()).and_then(sibling_disc_stem)
    {
        return folder.with_file_name(stem);
    }
    folder
}

/// Scan `roots` into the database. `progress` is called from the scanning thread.
pub fn scan(
    conn: &mut Connection,
    art: &ArtCache,
    roots: &[PathBuf],
    progress: &(dyn Fn(Progress) + Sync),
) -> rusqlite::Result<Report> {
    scan_with(conn, art, roots, Options::default(), progress)
}

/// The root (of `live_roots`) a path is under, if any.
fn root_of(path: &str, live_roots: &[PathBuf]) -> Option<usize> {
    live_roots.iter().position(|r| Path::new(path).starts_with(r))
}

/// Paths that differ only in letter case name the same file: NTFS on Windows, and APFS and
/// HFS+ as macOS formats them by default (case-insensitive, case-preserving). There a folder
/// renamed only in letter case is still found under its old spelling, so a vanished track and
/// a new file whose paths differ only so are the same file moved (`match_moves`), and a
/// partial scan of such a folder hands over to a full one (`batch::spelled_otherwise`). A
/// case-sensitive volume on macOS (an option when formatting) is taken as the others are.
pub(crate) const FOLDS_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// Whether `path` is `folder` or under it, by whole names (letter case aside on Windows).
fn within(path: &Path, folder: &Path) -> bool {
    if cfg!(windows) {
        let low = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        low(path).starts_with(low(folder))
    } else {
        path.starts_with(folder)
    }
}

/// The library folders without the ones inside another (or the same twice): a folder inside
/// another is part of it, its files walked, watched and stored once, as the outer one's.
pub fn outermost(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (i, r) in roots.iter().enumerate() {
        // Inside another; of two that are the same, the first is kept.
        let inside = roots.iter().enumerate().any(|(j, o)| j != i && within(r, o) && (j < i || !within(o, r)));
        if !inside {
            out.push(r.clone());
        }
    }
    out
}

/// `scan`, with options.
pub fn scan_with(
    conn: &mut Connection,
    art: &ArtCache,
    roots: &[PathBuf],
    opts: Options,
    progress: &(dyn Fn(Progress) + Sync),
) -> rusqlite::Result<Report> {
    let started = Instant::now();
    let roots = &outermost(roots);
    match batch::run(conn, art, roots, &opts, progress)? {
        Some(report) => Ok(report),
        // A partial scan that found something gone for good: everything, with the guard.
        None => {
            let full = Options { only: None, ..opts };
            let mut r = scan_with(conn, art, roots, full, progress)?;
            r.seconds = started.elapsed().as_secs_f64();
            Ok(r)
        }
    }
}

/// The first track of an album with a file, in album order: where embedded art is looked for.
/// An album only a server has has none (its cover is the server's: `albums.remote_art`).
const FIRST_TRACK: &str =
    "(SELECT path FROM tracks t WHERE t.album_id = a.id AND t.path NOT LIKE 'navidrome:%' ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path LIMIT 1)";

/// Cache the art of albums `ids` (`upgrade`: albums only re-cached for a new cache format,
/// with their art as it was). Returns the errors and how many albums were looked at.
#[allow(clippy::type_complexity)]
fn make_art(
    conn: &mut Connection,
    art: &ArtCache,
    ids: &HashSet<i64>,
    upgrade: &HashMap<i64, String>,
    workers: usize,
    progress: &(dyn Fn(Progress) + Sync),
    disk: &walk::Disk,
) -> rusqlite::Result<(Vec<(PathBuf, String)>, usize)> {
    // (album, folder, first track, folder fingerprint before looking)
    let mut jobs: Vec<(i64, PathBuf, PathBuf, String)> = Vec::new();
    {
        let mut st = conn.prepare(&format!("SELECT a.folder, {FIRST_TRACK} FROM albums a WHERE a.id = ?1"))?;
        for &id in ids {
            if let Some((folder, track)) = st
                .query_row([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
                .optional()?
                && let Some(track) = track
            {
                let (folder, track) = (PathBuf::from(folder), PathBuf::from(track));
                let stamp = art_fingerprint(&folder, &track);
                jobs.push((id, folder, track, stamp));
            }
        }
    }
    let total = jobs.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let arts: Mutex<Vec<(i64, Cover)>> = Mutex::new(Vec::new());
    let art_errors: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                crate::background_thread();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        break;
                    }
                    let (id, folder, track, _) = &jobs[i];
                    // A panic decoding one cover (the image decoder) leaves that album
                    // without art and is reported; the scan goes on.
                    let stem = match guarded(|| Ok(album_art_on(art, folder, track, disk))) {
                        Ok(stem) => stem,
                        Err(e) => {
                            art_errors.lock().unwrap_or_else(|e| e.into_inner()).push((track.clone(), e));
                            Cover::Missing
                        }
                    };
                    arts.lock().unwrap_or_else(|e| e.into_inner()).push((*id, stem));
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d.is_multiple_of(16) || d == total {
                        progress(Progress { phase: Phase::Art, done: d, total, ..Default::default() });
                    }
                }
            });
        }
    });
    let art_errors = art_errors.into_inner().unwrap_or_else(|e| e.into_inner());
    let stamps: HashMap<i64, &String> = jobs.iter().map(|(id, _, _, stamp)| (*id, stamp)).collect();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut set_art = tx.prepare_cached("UPDATE albums SET art=?2, art_checked=?3 WHERE id=?1")?;
        for (id, stem) in arts.into_inner().unwrap_or_else(|e| e.into_inner()) {
            match (stem, upgrade.get(&id)) {
                // Only re-caching, and the source is gone or unreadable now: keep the
                // art as cached (lower quality beats none), and do not try it again.
                (Cover::Missing, Some(old)) => art.mark_current(old),
                // Only re-caching, and the new files could not be written: the art stays
                // as cached, not marked current, for the next scan to try again.
                (Cover::Failed, Some(_)) => {}
                // No art: remember the folders as they were, to look again only when
                // they change.
                (Cover::Missing | Cover::Failed, None) => {
                    set_art.execute(params![id, Option::<String>::None, stamps.get(&id)])?;
                }
                (Cover::Cached(stem), _) => {
                    set_art.execute(params![id, stem, Option::<String>::None])?;
                }
            }
        }
    }
    tx.commit()?;
    Ok((art_errors, total))
}

/// The scanner's tag reading version (`tracks.tags_read`). Tracks read by an earlier one
/// (0: before schema 8) are read again by the next full scan (`fill_tags`), and their
/// folders regrouped. 9: "feat." with no space after it, full-width brackets
/// (`names::featured`); a folder's titled tracks without an album artist are one album
/// whatever their numbers (`group`).
pub const TAGS_VERSION: i64 = 9;

/// Tracks read per transaction by `fill_tags`, whole folders at a time.
const TAGS_BATCH: usize = 384;

/// Read again the tags of tracks scanned before this version of the scanner
/// (`tags_read < TAGS_VERSION`), to take in what earlier versions did not keep: several
/// artists and genres, MusicBrainz artist ids, the compilation flag, each track's own album
/// fields. Only files found where they were (`present`), unchanged since (a changed file
/// was read in full by the scan already). The rows are updated in place (ids, plays,
/// favourites, ratings, playlists and "added" dates stay), a batch of whole folders at a
/// time, each in its own transaction, so a quit keeps what was done; the albums of each
/// batch are regrouped by the rules of `group` and keep their dates and favourites when
/// they change identity (`sync`). Only albums that are new or whose tracks changed get
/// their art looked at again. A file that cannot be read keeps its row and is not tried
/// again until it changes. Returns how many tracks were read.
#[allow(clippy::too_many_arguments)]
fn fill_tags(
    conn: &mut Connection,
    by_path: &HashMap<String, (i64, i64)>,
    art: &ArtCache,
    workers: usize,
    progress: &(dyn Fn(Progress) + Sync),
    report: &mut Report,
    cancel: &AtomicBool,
    disk: &walk::Disk,
) -> rusqlite::Result<usize> {
    // (folder, id, path, album), folder by folder.
    let mut todo: Vec<(String, i64, String, i64)> = {
        let mut st = conn.prepare(
            "SELECT COALESCE(g.group_folder, ''), t.id, t.path, t.album_id FROM tracks t LEFT JOIN track_tags g ON g.track_id = t.id
             WHERE t.tags_read < ?1",
        )?;
        st.query_map([TAGS_VERSION], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .filter_map(Result::ok)
            .filter(|(_, _, path, _): &(String, i64, String, i64)| by_path.contains_key(path))
            .collect()
    };
    let total = todo.len();
    if total == 0 {
        return Ok(0);
    }
    todo.sort();
    log::info!("scan: reading again the tags of {total} tracks scanned by an earlier version");
    let started = Instant::now();
    let mut done = 0;
    let mut at = 0;
    while at < todo.len() {
        // Whole folders: a batch ends where a folder ends, past TAGS_BATCH tracks.
        let mut end = (at + TAGS_BATCH).min(todo.len());
        while end < todo.len() && todo[end].0 == todo[end - 1].0 {
            end += 1;
        }
        let batch = &todo[at..end];
        at = end;
        if cancel.load(Ordering::Relaxed) {
            report.interrupted = true;
            break;
        }
        progress(Progress { phase: Phase::Tags, done, total, ..Default::default() });
        let next = AtomicUsize::new(0);
        let read: Mutex<Vec<(usize, Result<TrackTags, String>)>> = Mutex::new(Vec::with_capacity(batch.len()));
        std::thread::scope(|s| {
            for _ in 0..workers.min(batch.len()) {
                s.spawn(|| {
                    crate::background_thread();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((_, _, path, _)) = batch.get(i) else { break };
                        let r = guarded(|| {
                            disk.wait();
                            tags::read(Path::new(path), false).map(|(t, _)| t)
                        });
                        read.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
                    }
                });
            }
        });
        let mut fresh = Vec::new();
        let mut failed = Vec::new();
        let mut read = read.into_inner().unwrap_or_else(|e| e.into_inner());
        read.sort_by_key(|(i, _)| *i);
        for (i, r) in read {
            let (_, id, path, album) = &batch[i];
            match r {
                Ok(tags) => {
                    let (mtime, size) = by_path[path];
                    fresh.push(Fresh { path: PathBuf::from(path), mtime, size, born: 0, tags, row: Some((*id, *album)) });
                }
                Err(e) => {
                    log::debug!("tags of {path}: {e}");
                    failed.push(*id);
                }
            }
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let synced = sync(&tx, fresh, &[], &SyncCtx::default())?;
        {
            let mut mark = tx.prepare_cached("UPDATE tracks SET tags_read = ?2 WHERE id = ?1")?;
            for id in failed {
                mark.execute(params![id, TAGS_VERSION])?;
            }
        }
        tx.commit()?;
        if !synced.regrouped.is_empty() {
            report.errors.extend(make_art(conn, art, &synced.regrouped, &HashMap::new(), workers, progress, disk)?.0);
        }
        done += batch.len();
    }
    progress(Progress { phase: Phase::Tags, done, total, ..Default::default() });
    log::info!("scan: tags of {done} tracks read again in {:.1} s", started.elapsed().as_secs_f64());
    Ok(done)
}

/// Tracks read per transaction by `fill_details`: a quit or a crash keeps what was done.
const DETAILS_BATCH: usize = 256;

/// Read the technical details of tracks that have none yet (`codec IS NULL`: scanned
/// before schema 7) and write only those columns. The files are read in parallel, one
/// batch at a time, each batch in its own transaction; a file that cannot be read gets
/// `codec = ''` (read, not known), so it is not tried again until it changes (a change
/// re-reads it in full anyway). Returns how many tracks were filled.
fn fill_details(
    conn: &mut Connection,
    present: &HashMap<String, (i64, i64)>,
    workers: usize,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
    disk: &walk::Disk,
) -> rusqlite::Result<usize> {
    let todo: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT id, path FROM tracks WHERE codec IS NULL ORDER BY id")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .filter(|(_, path): &(i64, String)| present.contains_key(path))
            .collect()
    };
    let total = todo.len();
    if total == 0 {
        return Ok(0);
    }
    log::info!("scan: reading the details of {total} tracks scanned before they were kept");
    let started = Instant::now();
    let mut done = 0;
    for batch in todo.chunks(DETAILS_BATCH) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(Progress { phase: Phase::Details, done, total, ..Default::default() });
        let next = AtomicUsize::new(0);
        let read: Mutex<Vec<(i64, AudioInfo)>> = Mutex::new(Vec::with_capacity(batch.len()));
        std::thread::scope(|s| {
            for _ in 0..workers.min(batch.len()) {
                s.spawn(|| {
                    crate::background_thread();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((id, path)) = batch.get(i) else { break };
                        let info = match guarded(|| {
                            disk.wait();
                            tags::read_audio(Path::new(path))
                        }) {
                            Ok(info) => info,
                            Err(e) => {
                                log::debug!("details of {path}: {e}");
                                AudioInfo::default()
                            }
                        };
                        read.lock().unwrap_or_else(|e| e.into_inner()).push((*id, info));
                    }
                });
            }
        });
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            // `codec IS NULL`: a row a scan wrote meanwhile (none can: scans run one at a
            // time) would not be overwritten.
            let mut set = tx.prepare_cached(
                "UPDATE tracks SET codec=?2, sample_rate=?3, bit_depth=?4, bitrate=?5, channels=?6
                 WHERE id=?1 AND codec IS NULL",
            )?;
            for (id, a) in read.into_inner().unwrap_or_else(|e| e.into_inner()) {
                set.execute(params![id, a.codec.name(), a.sample_rate, a.bit_depth, a.bitrate, a.channels])?;
            }
        }
        tx.commit()?;
        done += batch.len();
    }
    progress(Progress { phase: Phase::Details, done, total, ..Default::default() });
    log::info!("scan: details of {done} tracks read in {:.1} s", started.elapsed().as_secs_f64());
    Ok(done)
}

/// The modification times of an album's folders (the first track's, and the album
/// folder when that is its parent): they change when a file is added, removed or
/// renamed in them, which is when an album without art may have gained some.
fn art_fingerprint(folder: &Path, first_track: &Path) -> String {
    let stamp = |dir: &Path| {
        std::fs::metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or_else(|| "-".to_owned(), |d| d.as_nanos().to_string())
    };
    let track_dir = first_track.parent().unwrap_or(folder);
    if track_dir == folder { stamp(folder) } else { format!("{}|{}", stamp(track_dir), stamp(folder)) }
}

/// Run `f`, turning a panic inside it into an error ("panicked: ..."), so one bad file
/// cannot take a scan worker, and with it the whole scan, down.
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    crate::guard::catching(f).unwrap_or_else(|what| Err(format!("panicked: {what}")))
}

/// The album artist one track alone would give its album: ALBUMARTIST, else the track's
/// artist, else "Unknown Artist" (the album's is worked out from all its tracks:
/// `album_values`).
pub fn album_artist_of(t: &TrackTags) -> String {
    t.album_artist.clone().or_else(|| t.artist.clone()).unwrap_or_else(|| UNKNOWN_ARTIST.to_owned())
}

/// Track title, track artist and album title as stored, with their fallbacks: the file
/// name for a missing title (`tags::read` already took a title from it when it could),
/// the album artist for a missing artist, the album folder's name for a missing album.
pub fn display_names(t: &TrackTags, path: &Path) -> (String, String, String) {
    let title = t
        .title
        .clone()
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let artist = t.artist.clone().unwrap_or_else(|| album_artist_of(t));
    let album = t.album.clone().unwrap_or_else(|| {
        album_folder(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    });
    (title, artist, album)
}

/// A file read by this scan, to be written by `sync`.
struct Fresh {
    path: PathBuf,
    mtime: i64,
    size: i64,
    /// `FileInfo::born` (0: not known).
    born: i64,
    tags: TrackTags,
    /// Its row: a known path, or a vanished track re-pointed here; and the album it was in.
    row: Option<(i64, i64)>,
}

/// What `sync` changed.
#[derive(Debug, Default)]
struct Synced {
    /// Albums with a track read again or gone: their art is looked at again.
    changed: HashSet<i64>,
    /// Albums that are new, or whose set of tracks changed (a track came or left).
    regrouped: HashSet<i64>,
    /// Albums made of new files alone (no track came from another album): an album emptied
    /// later in the same scan with the same artist and title passes its date on to one
    /// (`SyncCtx::earlier`), as within one `sync`.
    created: HashSet<i64>,
    /// With `SyncCtx::defer_settle`: the artists and genres whose names are still to be
    /// worked out from their credits (`relations::settle_artists_and_genres`).
    unsettled: Touched,
}

/// What `sync` is to know beyond the files.
#[derive(Default)]
struct SyncCtx<'a> {
    /// A first import (an empty library, or the rest of one that was stopped): an album new
    /// to the library is dated by its files, the earliest any of its files is known to have
    /// been on disk (`FileInfo::born`; never later than now), `added_source` "files". So
    /// "Recently added" after a first import shows what was added to the collection last,
    /// not the order the scan happened to read it in, and the same whether the import was
    /// interrupted or not: the date is the earliest over all its tracks, whichever batch
    /// each came in. Once the import is over, dates never change again (except by `inherit`).
    dated: bool,
    /// The albums earlier batches of this scan made of new files alone: the albums emptied by
    /// the end of the scan (its removals) pass their date and favourite on to one of them of
    /// the same artist and title, as when it all was one transaction.
    earlier: Option<&'a HashSet<i64>>,
    /// Leave the touched artists' and genres' names to the caller (`Synced::unsettled`): a
    /// scan in batches settles them in short transactions of their own, and at its end.
    defer_settle: bool,
}

/// What grouping needs of a track, from its tags.
fn member_of(t: &TrackTags, path: &Path) -> group::Member {
    group::Member {
        folder: group_folder(path).to_string_lossy().to_lowercase(),
        album: t.album.clone(),
        album_artist: t.album_artist.clone(),
        main_artist: t.artists.iter().find(|c| !c.featured).map(|c| c.name.clone()),
        compilation: t.compilation,
        disc: t.disc_no,
        track: t.track_no,
        mb_album: t.mb_album_id.clone(),
    }
}

/// The album key a track would have on its own (its group being only itself), for the
/// metadata viewer: `group` decides with all of a folder's tracks.
pub fn album_key(t: &TrackTags, path: &Path) -> String {
    group::keys(&[member_of(t, path)]).remove(0)
}

/// Write the files read (`fresh`) and delete `gone` (track id, album id), then regroup
/// every track of the folders they were and are in (`group::keys`), so an album is decided
/// with all its tracks, whatever was read. Albums that appear take over the "added" date
/// and the favourite of the albums their tracks came from; an album that disappears into
/// another passes them on (`inherit`). Every album touched has its values worked out again
/// from all its tracks (`album_values`), and artists and genres are settled
/// (`relations::resolve`).
fn sync(tx: &Connection, fresh: Vec<Fresh>, gone: &[(i64, i64)], ctx: &SyncCtx) -> rusqlite::Result<Synced> {
    let now = now();
    let now_ms = now_ms();
    let mut out = Synced::default();
    let mut touched = Touched::default();
    // The folders to regroup, and the albums whose values to work out again.
    let mut folders: HashSet<String> = HashSet::new();
    let mut albums: HashSet<i64> = HashSet::new();
    let mut skip: HashSet<i64> = HashSet::new();
    {
        let mut folder_of = tx.prepare_cached("SELECT group_folder FROM track_tags WHERE track_id = ?1")?;
        let mut old = |id: i64, folders: &mut HashSet<String>| -> rusqlite::Result<()> {
            if let Some(Some(f)) = folder_of.query_row([id], |r| r.get::<_, Option<String>>(0)).optional()? {
                folders.insert(f);
            }
            Ok(())
        };
        for f in &fresh {
            folders.insert(group_folder(&f.path).to_string_lossy().to_lowercase());
            if let Some((id, album)) = f.row {
                old(id, &mut folders)?;
                skip.insert(id);
                albums.insert(album);
                out.changed.insert(album);
            }
        }
        for &(id, album) in gone {
            old(id, &mut folders)?;
            skip.insert(id);
            albums.insert(album);
            out.changed.insert(album);
            out.regrouped.insert(album);
        }
    }
    // Every other track of those folders, as stored.
    let mut members: Vec<group::Member> = fresh.iter().map(|f| member_of(&f.tags, &f.path)).collect();
    let mut stored: Vec<(i64, i64)> = Vec::new(); // (track id, album id) of members after the fresh ones
    {
        let mut st = tx.prepare_cached(
            "SELECT t.id, t.album_id, g.album_title, g.album_artist, g.compilation, t.disc_no, t.track_no, g.mb_album_id,
                    (SELECT x.name FROM track_artists x WHERE x.track_id = t.id AND x.featured = 0 ORDER BY x.position LIMIT 1)
             FROM track_tags g JOIN tracks t ON t.id = g.track_id WHERE g.group_folder = ?1",
        )?;
        let mut sorted: Vec<&String> = folders.iter().collect();
        sorted.sort();
        for folder in sorted {
            let rows = st.query_map([folder], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    group::Member {
                        folder: folder.clone(),
                        album: r.get(2)?,
                        album_artist: r.get(3)?,
                        compilation: r.get(4)?,
                        disc: r.get(5)?,
                        track: r.get(6)?,
                        mb_album: r.get(7)?,
                        main_artist: r.get(8)?,
                    },
                ))
            })?;
            for row in rows {
                let (id, album, m) = row?;
                if skip.contains(&id) {
                    continue;
                }
                albums.insert(album);
                stored.push((id, album));
                members.push(m);
            }
        }
    }
    let keys = group::keys(&members);

    // An album for every key: the one there is, or a new one.
    let mut album_of: HashMap<String, i64> = HashMap::new();
    let mut new_albums: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut find = tx.prepare_cached("SELECT id FROM albums WHERE key = ?1")?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO albums (key, title, album_artist, folder, added_at, first_seen, added_source) VALUES (?1, '', '', ?2, ?3, ?3, ?4)",
        )?;
        let source = ctx.dated.then_some("files");
        for (i, key) in keys.iter().enumerate() {
            if album_of.contains_key(key) {
                continue;
            }
            let id = match find.query_row([key], |r| r.get::<_, i64>(0)).optional()? {
                // An album a server made (`remote`: one with the release id these files have)
                // that its first local files join: from now on a local album, dated as one new
                // to the library, so the library is the same whether the files or the server
                // came first.
                Some(id) if server_made(tx, id)? => {
                    tx.prepare_cached("UPDATE albums SET added_at = ?2, first_seen = ?2, added_source = ?3 WHERE id = ?1")?.execute(params![id, now_ms, source])?;
                    out.regrouped.insert(id);
                    id
                }
                Some(id) => id,
                None => {
                    let path = match i.checked_sub(fresh.len()) {
                        None => fresh[i].path.clone(),
                        Some(j) => PathBuf::from(tx.query_row("SELECT path FROM tracks WHERE id = ?1", [stored[j].0], |r| r.get::<_, String>(0))?),
                    };
                    insert.execute(params![key, album_folder(&path).to_string_lossy(), now_ms, source])?;
                    let id = tx.last_insert_rowid();
                    new_albums.insert(id, Vec::new());
                    out.regrouped.insert(id);
                    id
                }
            };
            album_of.insert(key.clone(), id);
            albums.insert(id);
        }
    }
    // Albums that took tracks from another album (merged, split, moved).
    let mut moved_from: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut moved = |to: i64, from: i64, out: &mut Synced| {
        if to != from {
            moved_from.entry(to).or_default().push(from);
            out.regrouped.insert(to);
            out.regrouped.insert(from);
        }
    };

    // The files read.
    {
        let mut update = tx.prepare_cached(
            "UPDATE tracks SET album_id=?2, title=?3, artist=?4, disc_no=?5, track_no=?6, duration=?7, genre=?8,
             mtime=?9, size=?10, path=?11, codec=?12, sample_rate=?13, bit_depth=?14, bitrate=?15, channels=?16,
             tags_read=?17, from_name=?18 WHERE id=?1",
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO tracks (album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, path,
             codec, sample_rate, bit_depth, bitrate, channels, tags_read, from_name, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        )?;
        let mut set_tags = tx.prepare_cached(
            "INSERT OR REPLACE INTO track_tags (track_id, group_folder, album_title, album_artist, album_artist_mbids,
             album_artist_sort, artist_sort, date, original_date, release_date, label, release_type, mb_album_id, compilation, mb_track_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?;
        // In passes, the track rows first, then their tags, then their credits: rows of one
        // table written together sit together in the file, so a list read from a cold cache
        // (a long playlist, the Albums page) reads as few pages as before schema 8.
        let mut ids = Vec::with_capacity(fresh.len());
        for (i, f) in fresh.iter().enumerate() {
            let t = &f.tags;
            let album = album_of[&keys[i]];
            let (title, artist, _) = display_names(t, &f.path);
            let a = &t.audio;
            let path = f.path.to_string_lossy();
            let id = match f.row {
                Some((id, old)) => {
                    update.execute(params![
                        id, album, title, artist, t.disc_no, t.track_no, t.duration, t.genre, f.mtime, f.size, path,
                        a.codec.name(), a.sample_rate, a.bit_depth, a.bitrate, a.channels, TAGS_VERSION, t.from_name.bits()
                    ])?;
                    moved(album, old, &mut out);
                    id
                }
                None => {
                    insert.execute(params![
                        album, title, artist, t.disc_no, t.track_no, t.duration, t.genre, f.mtime, f.size, path,
                        a.codec.name(), a.sample_rate, a.bit_depth, a.bitrate, a.channels, TAGS_VERSION, t.from_name.bits(), now
                    ])?;
                    out.regrouped.insert(album);
                    tx.last_insert_rowid()
                }
            };
            out.changed.insert(album);
            ids.push(id);
        }
        for ((f, &id), m) in fresh.iter().zip(&ids).zip(&members) {
            let t = &f.tags;
            let mbids = (!t.album_artist_mbids.is_empty()).then(|| t.album_artist_mbids.join(";"));
            set_tags.execute(params![
                id, m.folder, t.album, t.album_artist, mbids, t.album_artist_sort, t.artist_sort, t.date, t.original_date,
                t.release_date, t.label, t.release_type, t.mb_album_id, t.compilation, t.mb_recording_id
            ])?;
        }
        for (f, &id) in fresh.iter().zip(&ids) {
            relations::set_track_artists(tx, id, &f.tags.artists, &mut touched)?;
        }
        for (f, &id) in fresh.iter().zip(&ids) {
            relations::set_track_genres(tx, id, &f.tags.genres, &mut touched)?;
        }
    }
    // The other tracks of those folders, where their album changed.
    {
        let mut set = tx.prepare_cached("UPDATE tracks SET album_id = ?2 WHERE id = ?1")?;
        for (j, &(id, old)) in stored.iter().enumerate() {
            let album = album_of[&keys[fresh.len() + j]];
            if album != old {
                set.execute(params![id, album])?;
                moved(album, old, &mut out);
            }
        }
    }
    // The tracks gone. One the server also has stays, as the server's (`remote::to_server_only`).
    {
        let mut delete = tx.prepare_cached("DELETE FROM tracks WHERE id = ?1")?;
        for &(id, _) in gone {
            if crate::remote::to_server_only(tx, id, &mut touched)? {
                continue;
            }
            relations::forget_track(tx, id, &mut touched)?;
            delete.execute([id])?;
        }
    }
    for (id, from) in &mut new_albums {
        if let Some(f) = moved_from.get(id) {
            from.extend(f);
        }
    }
    inherit(tx, &new_albums, &moved_from, ctx.earlier)?;
    for (id, from) in &new_albums {
        if from.iter().all(|f| new_albums.contains_key(f)) {
            out.created.insert(*id);
        }
    }
    if ctx.dated {
        // The earliest of its files, whichever batch each came in (`SyncCtx::dated`).
        let mut earliest: HashMap<i64, i64> = HashMap::new();
        for (i, f) in fresh.iter().enumerate() {
            if f.born > 0 {
                let at = earliest.entry(album_of[&keys[i]]).or_insert(i64::MAX);
                *at = (*at).min(f.born.min(now_ms));
            }
        }
        let mut set = tx.prepare_cached("UPDATE albums SET added_at = MIN(added_at, ?2) WHERE id = ?1 AND added_source = 'files'")?;
        for (album, at) in earliest {
            set.execute(params![album, at])?;
        }
    }
    albums.extend(out.regrouped.iter().copied());
    let mut sorted: Vec<i64> = albums.into_iter().collect();
    sorted.sort_unstable();
    let mut has_tracks = tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM tracks WHERE album_id = ?1)")?;
    let mut delete_album = tx.prepare_cached("DELETE FROM albums WHERE id = ?1")?;
    // A server's catalog is matched again with what this wrote (`remote::reconcile`).
    crate::remote::mark_pending(tx, sorted.iter().copied())?;
    for id in sorted {
        if has_tracks.query_row([id], |r| r.get::<_, bool>(0))? {
            album_values(tx, id, &mut touched)?;
        } else if tx.query_row("SELECT EXISTS(SELECT 1 FROM albums WHERE id = ?1)", [id], |r| r.get::<_, bool>(0))? {
            relations::forget_album(tx, id, &mut touched)?;
            delete_album.execute([id])?;
        }
    }
    if ctx.defer_settle {
        relations::resolve_names(tx, &mut touched)?;
        out.unsettled = touched;
    } else {
        relations::resolve(tx, &mut touched)?;
    }
    out.changed.retain(|id| album_exists(tx, *id));
    out.regrouped.retain(|id| album_exists(tx, *id));
    out.created.retain(|id| album_exists(tx, *id));
    Ok(out)
}

/// An album a server made (`added_source` "server") that has no local track yet.
fn server_made(tx: &Connection, id: i64) -> rusqlite::Result<bool> {
    tx.prepare_cached(
        "SELECT added_source IS 'server' AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.album_id = a.id AND t.path NOT LIKE 'navidrome:%')
         FROM albums a WHERE a.id = ?1",
    )?
    .query_row([id], |r| r.get(0))
}

fn album_exists(tx: &Connection, id: i64) -> bool {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM albums WHERE id = ?1)", [id], |r| r.get::<_, bool>(0)).unwrap_or(false)
}

/// One track of an album, for working out the album's values.
struct AlbumTrack {
    path: String,
    album: Option<String>,
    album_artist: Option<String>,
    album_artist_mbids: Option<String>,
    album_artist_sort: Option<String>,
    date: Option<String>,
    original_date: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    release_type: Option<String>,
    genre: Option<String>,
    member: group::Member,
    main_mbid: Option<String>,
}

/// An album's values from all its tracks, so they do not depend on which track a scan
/// reached last: for each, the most common value among the tracks that have one, ties
/// going to the album's first track (by disc, number and path; a value "2004" and one
/// "2004-03-15" are the same year, and the fullest date of the most common year wins). Its
/// album artist and whether it is a compilation by `group::decide_one`; its genres every
/// genre of its tracks; its folder its first track's.
///
/// An album with tracks from a server (`remote`) is worked out from its local tracks when it has
/// any (a file's tags win), else from the server's, whose folder is the server album's.
pub(crate) fn album_values(tx: &Connection, id: i64, touched: &mut Touched) -> rusqlite::Result<()> {
    let tracks: Vec<AlbumTrack> = {
        let mut st = tx.prepare_cached(
            "SELECT t.path, g.album_title, g.album_artist, g.album_artist_mbids, g.album_artist_sort, g.date,
                    g.original_date, g.release_date, g.label, g.release_type, t.genre, COALESCE(g.compilation, 0), t.disc_no,
                    t.track_no, g.mb_album_id, g.group_folder, x.name, x.mbid
             FROM tracks t LEFT JOIN track_tags g ON g.track_id = t.id LEFT JOIN track_artists x ON x.track_id = t.id AND x.position =
                  (SELECT MIN(y.position) FROM track_artists y WHERE y.track_id = t.id AND y.featured = 0)
             WHERE t.album_id = ?1 ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path",
        )?;
        st.query_map([id], |r| {
            Ok(AlbumTrack {
                path: r.get(0)?,
                album: r.get(1)?,
                album_artist: r.get(2)?,
                album_artist_mbids: r.get(3)?,
                album_artist_sort: r.get(4)?,
                date: r.get(5)?,
                original_date: r.get(6)?,
                release_date: r.get(7)?,
                label: r.get(8)?,
                release_type: r.get(9)?,
                genre: r.get(10)?,
                member: group::Member {
                    compilation: r.get(11)?,
                    disc: r.get(12)?,
                    track: r.get(13)?,
                    mb_album: r.get(14)?,
                    folder: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
                    album: r.get(1)?,
                    album_artist: r.get(2)?,
                    main_artist: r.get(16)?,
                },
                main_mbid: r.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let local = tracks.iter().any(|t| !crate::remote::is_remote(&t.path));
    let tracks: Vec<AlbumTrack> = if local { tracks.into_iter().filter(|t| !crate::remote::is_remote(&t.path)).collect() } else { tracks };
    let Some(first) = tracks.first() else { return Ok(()) };
    let folder = if local { album_folder(Path::new(&first.path)).to_string_lossy().into_owned() } else { first.member.folder.clone() };
    let common = |f: fn(&AlbumTrack) -> &Option<String>| -> Option<String> {
        names::most_common(tracks.iter().filter_map(|t| f(t).as_ref()), |v| names::norm(v)).cloned()
    };
    let exact = |f: fn(&AlbumTrack) -> &Option<String>| -> Option<String> { names::most_common(tracks.iter().filter_map(|t| f(t).as_ref()), Clone::clone).cloned() };
    let title = names::most_common(tracks.iter().filter_map(|t| t.album.as_ref()), |v| names::norm(v))
        .map(|n| {
            // The most common spelling of that title.
            let norm = names::norm(n);
            names::most_common(tracks.iter().filter_map(|t| t.album.as_ref()).filter(|v| names::norm(v) == norm), Clone::clone).cloned().unwrap_or_else(|| n.clone())
        })
        .unwrap_or_else(|| if local { Path::new(&folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { String::new() });
    let members: Vec<&group::Member> = tracks.iter().map(|t| &t.member).collect();
    let by = group::decide_one(&members);
    let (artist, credits): (String, Vec<Credit>) = match &by {
        group::By::Explicit(norm) => {
            let pick = names::most_common(tracks.iter().filter(|t| t.album_artist.as_deref().map(names::norm).as_ref() == Some(norm)), |t| t.album_artist.clone())
                .or_else(|| tracks.iter().find(|t| t.album_artist.is_some()));
            match pick.and_then(|t| t.album_artist.clone().map(|a| (a, t.album_artist_mbids.clone()))) {
                Some((a, ids)) => {
                    let mut credits: Vec<Credit> = names::split_artists(&a).into_iter().map(|n| Credit::new(n, false)).collect();
                    let ids: Vec<&str> = ids.as_deref().map(|s| s.split(';').collect()).unwrap_or_default();
                    if ids.len() == credits.len() {
                        for (c, id) in credits.iter_mut().zip(ids) {
                            c.mbid = Some(id.to_owned());
                        }
                    }
                    (a, credits)
                }
                None => (UNKNOWN_ARTIST.to_owned(), vec![Credit::new(UNKNOWN_ARTIST, false)]),
            }
        }
        group::By::Main(norm) if !norm.is_empty() => {
            let pick = names::most_common(tracks.iter().filter(|t| t.member.main_artist.as_deref().map(names::norm).as_ref() == Some(norm)), |t| t.member.main_artist.clone());
            match pick.and_then(|t| t.member.main_artist.clone().map(|a| (a, t.main_mbid.clone()))) {
                Some((a, mbid)) => (a.clone(), vec![Credit { name: a, mbid, featured: false }]),
                None => (UNKNOWN_ARTIST.to_owned(), vec![Credit::new(UNKNOWN_ARTIST, false)]),
            }
        }
        group::By::Main(_) => (UNKNOWN_ARTIST.to_owned(), vec![Credit::new(UNKNOWN_ARTIST, false)]),
        group::By::Various => {
            let tagged = names::most_common(tracks.iter().filter_map(|t| t.album_artist.as_ref()).filter(|a| names::is_various(a)), Clone::clone).cloned();
            let a = tagged.unwrap_or_else(|| names::VARIOUS_ARTISTS.to_owned());
            (a.clone(), vec![Credit::new(&a, false)])
        }
    };
    let sort = match by {
        group::By::Explicit(_) => exact(|t| &t.album_artist_sort),
        _ => None,
    };
    let date = album_date(tracks.iter().filter_map(|t| t.date.as_deref()));
    let original = album_date(tracks.iter().filter_map(|t| t.original_date.as_deref()));
    let release = album_date(tracks.iter().filter_map(|t| t.release_date.as_deref()));
    let label = common(|t| &t.label).map(|l| {
        let norm = names::norm(&l);
        names::most_common(tracks.iter().filter_map(|t| t.label.as_ref()).filter(|v| names::norm(v) == norm), Clone::clone).cloned().unwrap_or(l)
    });
    let genre = common(|t| &t.genre);
    let release_type = common(|t| &t.release_type);
    tx.prepare_cached(
        "UPDATE albums SET title=?2, album_artist=?3, album_artist_sort=?4, date=?5, original_date=?6, release_date=?7,
         genre=?8, label=?9, label_key=?10, release_type=?11, folder=?12, compilation=?13 WHERE id=?1",
    )?
    .execute(params![
        id,
        title,
        artist,
        sort,
        date,
        original,
        release,
        genre,
        label,
        label.as_deref().map(names::norm),
        release_type,
        folder,
        by == group::By::Various
    ])?;
    relations::set_album_artists(tx, id, &credits, touched)?;
    relations::set_album_genres(tx, id, touched)?;
    Ok(())
}

/// The album's date from its tracks': the most common year (ties: the first track's), and
/// of that year the fullest date given, the most common of those ("2004" on six tracks and
/// "2004-03-15" on four make "2004-03-15").
pub fn album_date<'a>(dates: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
    let year = |d: &str| d.get(..4).filter(|y| y.bytes().all(|b| b.is_ascii_digit())).map(str::to_owned).unwrap_or_else(|| d.to_owned());
    let all: Vec<&str> = dates.collect();
    let y = names::most_common(all.iter(), |d| year(d))?;
    let y = year(y);
    let longest = all.iter().filter(|d| year(d) == y).map(|d| d.len()).max()?;
    names::most_common(all.iter().filter(|d| year(d) == y && d.len() == longest), |d| (*d).to_owned()).map(|d| (*d).to_owned())
}

/// What an album without any artist is shown as.
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// Pass the user's album data on when albums change identity: an album created by this
/// scan takes over the "added" date and the favourite of the albums its tracks came from
/// (`new_albums`: new album -> albums before), or else of an album left empty by this scan
/// with the same artist and title (its files came back under new paths but could not be
/// matched one by one); an album that existed and took in the tracks of an album this
/// scan emptied (a merge: a featured credit or a compilation now grouped as one) takes them
/// from that album. The oldest date wins; the favourite is kept if any was one, with its
/// earliest time. Run before the emptied albums are deleted.
fn inherit(tx: &Connection, new_albums: &HashMap<i64, Vec<i64>>, moved_from: &HashMap<i64, Vec<i64>>, earlier: Option<&HashSet<i64>>) -> rusqlite::Result<()> {
    let empty = |id: i64| -> rusqlite::Result<bool> { tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM tracks WHERE album_id = ?1)", [id], |r| r.get(0)) };
    let mut plan: HashMap<i64, Vec<i64>> = HashMap::new();
    let new_ids: HashSet<i64> = new_albums.keys().copied().collect();
    let mut earlier: Vec<i64> = earlier.map(|e| e.iter().copied().filter(|id| !new_ids.contains(id)).collect()).unwrap_or_default();
    earlier.sort_unstable();
    if !new_albums.is_empty() || !earlier.is_empty() {
        // Albums this scan emptied (still in the table until the caller deletes them).
        let mut orphans: HashMap<(String, String), Vec<i64>> = HashMap::new();
        {
            let mut st = tx.prepare(
                "SELECT id, lower(album_artist), lower(title) FROM albums
                 WHERE id NOT IN (SELECT DISTINCT album_id FROM tracks)",
            )?;
            for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
                let (id, artist, title) = row?;
                if !new_ids.contains(&id) {
                    orphans.entry((artist, title)).or_default().push(id);
                }
            }
        }
        for (&id, from) in new_albums {
            let mut from: Vec<i64> = from.iter().copied().filter(|f| !new_ids.contains(f)).collect();
            if from.is_empty() {
                // Its values are not worked out yet: its tracks' album fields name it.
                let key: Option<(String, String)> = tx
                    .query_row(
                        "SELECT lower(COALESCE(g.album_artist, t.artist)), lower(COALESCE(g.album_title, '')) FROM tracks t
                         LEFT JOIN track_tags g ON g.track_id = t.id WHERE t.album_id = ?1
                         ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path LIMIT 1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                from = key.and_then(|k| orphans.get(&k).cloned()).unwrap_or_default();
            }
            plan.insert(id, from);
        }
        // Made of new files by an earlier batch of this scan (`SyncCtx::earlier`), as if it were
        // new here: it takes over from an album of its artist and title this scan emptied.
        if !orphans.is_empty() {
            let orphan_ids: HashSet<i64> = orphans.values().flatten().copied().collect();
            for id in earlier.into_iter().filter(|id| !orphan_ids.contains(id)) {
                let key: Option<(String, String)> = tx
                    .query_row(
                        "SELECT lower(COALESCE(g.album_artist, t.artist)), lower(COALESCE(g.album_title, '')) FROM tracks t
                         LEFT JOIN track_tags g ON g.track_id = t.id WHERE t.album_id = ?1
                         ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.path LIMIT 1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some(from) = key.and_then(|k| orphans.get(&k).cloned()) {
                    plan.insert(id, from);
                }
            }
        }
    }
    for (&to, from) in moved_from {
        if new_ids.contains(&to) {
            continue;
        }
        let mut emptied = Vec::new();
        for &f in from {
            if !new_ids.contains(&f) && empty(f)? {
                emptied.push(f);
            }
        }
        if !emptied.is_empty() {
            emptied.push(to);
            plan.entry(to).or_default().extend(emptied);
        }
    }
    let mut dates = tx.prepare("SELECT added_at, first_seen, added_source FROM albums WHERE id = ?1")?;
    let mut set = tx.prepare("UPDATE albums SET added_at = ?2, first_seen = ?3, added_source = ?4 WHERE id = ?1")?;
    let mut fav = tx.prepare("SELECT added_at FROM favourite_albums WHERE album_id = ?1")?;
    let mut set_fav = tx.prepare(
        "INSERT INTO favourite_albums (album_id, added_at) VALUES (?1, ?2)
         ON CONFLICT(album_id) DO UPDATE SET added_at = MIN(added_at, excluded.added_at)",
    )?;
    let mut ids: Vec<&i64> = plan.keys().collect();
    ids.sort();
    for &id in ids {
        let mut from = plan[&id].clone();
        from.sort_unstable();
        from.dedup();
        let mut best: Option<(i64, i64, Option<String>)> = None;
        let mut favourite: Option<i64> = None;
        for f in from {
            let Some(d) = dates.query_row([f], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))).optional()? else { continue };
            if best.as_ref().is_none_or(|b| d.0 < b.0) {
                best = Some(d);
            }
            if let Some(at) = fav.query_row([f], |r| r.get::<_, i64>(0)).optional()? {
                favourite = Some(favourite.map_or(at, |b: i64| b.min(at)));
            }
        }
        if let Some((added, first, source)) = best {
            set.execute(params![id, added, first, source])?;
        }
        if let Some(at) = favourite {
            set_fav.execute(params![id, at])?;
        }
    }
    Ok(())
}

/// What a stored path under `root` starts with: the root and a separator (either
/// kind, as the root may have been given with `/`).
fn root_prefixes(root: &Path) -> (String, String) {
    let r = root.to_string_lossy();
    if r.ends_with(['\\', '/']) {
        (r.clone().into_owned(), r.into_owned())
    } else {
        (format!("{r}\\"), format!("{r}/"))
    }
}

/// Drop everything under `root` from the library (the folder was removed from the
/// library, not from disk). Plays and favourites of those tracks go with them. One
/// delete in one transaction. A track a server also has stays, as the server's
/// (`remote::to_server_only`); a server's own tracks (no path under any folder) are never touched.
/// `keep`: the library folders that stay; what one of them holds too (one folder inside the
/// other, `outermost`) stays.
pub fn forget_root(conn: &mut Connection, root: &Path, keep: &[PathBuf]) -> rusqlite::Result<usize> {
    if root.as_os_str().is_empty() {
        return Ok(0);
    }
    let keep: Vec<&PathBuf> = keep.iter().filter(|k| within(k, root) || within(root, k)).collect();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    #[cfg(windows)]
    let mut under: Vec<(i64, i64, String, bool)> = tx
        .prepare("SELECT id, album_id, path, remote_id IS NOT NULL FROM tracks WHERE path NOT LIKE 'navidrome:%'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, _, path, _)| within(Path::new(path), root))
        .collect();
    #[cfg(not(windows))]
    let mut under: Vec<(i64, i64, String, bool)> = {
        let (a, b) = root_prefixes(root);
        const UNDER: &str = "(substr(path, 1, length(?1)) = ?1 OR substr(path, 1, length(?2)) = ?2)";
        tx.prepare(&format!("SELECT id, album_id, path, remote_id IS NOT NULL FROM tracks WHERE {UNDER} AND path NOT LIKE 'navidrome:%'"))?
            .query_map(params![a, b], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    under.retain(|(_, _, path, _)| !keep.iter().any(|k| within(Path::new(path), k)));
    let mut touched = Touched::default();
    let mut albums: Vec<i64> = Vec::new();
    let mut n = 0;
    {
        let mut delete = tx.prepare("DELETE FROM tracks WHERE id = ?1 AND path NOT LIKE 'navidrome:%'")?;
        for (id, album, _, shared) in &under {
            if *shared && crate::remote::to_server_only(&tx, *id, &mut touched)? {
                albums.push(*album);
                n += 1;
            } else {
                relations::forget_track(&tx, *id, &mut touched)?;
                n += delete.execute([id])?;
            }
        }
    }
    {
        let empty_albums: Vec<i64> = tx
            .prepare("SELECT id FROM albums WHERE id NOT IN (SELECT DISTINCT album_id FROM tracks)")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut delete_album = tx.prepare("DELETE FROM albums WHERE id = ?1")?;
        for id in empty_albums {
            relations::forget_album(&tx, id, &mut touched)?;
            delete_album.execute([id])?;
        }
    }
    // An album with tracks in a folder that stays as well: what is left of it decides its values.
    let thinned: Vec<i64> = if keep.is_empty() { Vec::new() } else { under.iter().map(|t| t.1).collect() };
    let mut values: Vec<i64> = albums.iter().chain(&thinned).copied().collect();
    values.sort_unstable();
    values.dedup();
    for &album in &values {
        if album_exists(&tx, album) {
            album_values(&tx, album, &mut touched)?;
        }
    }
    albums.sort_unstable();
    albums.dedup();
    if !albums.is_empty() {
        crate::remote::mark_pending(&tx, albums)?;
    }
    if !touched.is_empty() {
        relations::resolve(&tx, &mut touched)?;
    }
    tx.commit()?;
    Ok(n)
}

/// What looking for an album's art came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cover {
    /// Cached, by this stem.
    Cached(String),
    /// None there: no picture in the first track, no cover file.
    Missing,
    /// There, but caching it failed (it would not decode, or its files could not be
    /// written: on Windows a cached image open elsewhere cannot be replaced). Logged.
    Failed,
}

/// Embedded picture of the album's first track, else a cover file in the folder
/// (the disc subfolder first, then the album folder).
fn album_art_on(cache: &ArtCache, folder: &Path, first_track: &Path, disk: &walk::Disk) -> Cover {
    disk.wait();
    let mut failed = false;
    if let Ok((_, Some(bytes))) = tags::read(first_track, true) {
        match cache.store(&bytes) {
            Ok(stem) => return Cover::Cached(stem),
            Err(e) => {
                log::warn!("embedded art in {}: {e}", first_track.display());
                failed = true;
            }
        }
    }
    let track_dir = first_track.parent().unwrap_or(folder);
    disk.wait();
    let cover = art::find_folder_cover(track_dir).or_else(|| {
        disk.wait();
        art::find_folder_cover(folder)
    });
    let Some(cover) = cover else { return if failed { Cover::Failed } else { Cover::Missing } };
    disk.wait();
    match cache.store_file(&cover) {
        Ok(stem) => Cover::Cached(stem),
        Err(e) => {
            log::warn!("cover {}: {e}", cover.display());
            Cover::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files named `panics-here*` make the tag reader panic, as a malformed file might
    /// inside lofty.
    pub(super) fn maybe_panic(path: &Path) {
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("panics-here")) {
            panic!("simulated reader panic");
        }
    }

    /// Every file the scan's readers open, counted (tests run side by side: by full path).
    static READS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

    /// The scan's readers are about to read `path`.
    pub(super) fn reading(path: &Path) {
        *READS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(path.to_path_buf()).or_default() += 1;
        maybe_panic(path);
    }

    /// Where a test makes the disk fail: listing a folder (the walk), looking at an entry of
    /// one (the walk), looking at a path once more before its track is removed (the prune).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Fault {
        List,
        Look,
        Exists,
    }

    /// The faults set, by full path (tests run side by side).
    static FAULTS: Mutex<Vec<(Fault, PathBuf, std::io::ErrorKind)>> = Mutex::new(Vec::new());

    /// From now on, `at` on `path` fails with `kind`.
    pub(super) fn set_fault(at: Fault, path: &Path, kind: std::io::ErrorKind) {
        FAULTS.lock().unwrap_or_else(|e| e.into_inner()).push((at, path.to_path_buf(), kind));
    }

    /// Every fault under `dir` cleared.
    pub(super) fn clear_faults(dir: &Path) {
        FAULTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, p, _)| !p.starts_with(dir));
        UNPLUGS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, _, _, _)| !p.starts_with(dir));
    }

    /// Drives unplugged mid-walk: (folder whose listing unplugs it, root, where the root goes,
    /// whether an empty folder is left at the root's place, as a mount point on Linux).
    static UNPLUGS: Mutex<Vec<(PathBuf, PathBuf, PathBuf, bool)>> = Mutex::new(Vec::new());

    /// When the walk is about to list `at`, `root` goes (moved to `aside`), as a drive unplugged.
    pub(super) fn set_unplug(at: &Path, root: &Path, aside: &Path, leave_empty: bool) {
        UNPLUGS.lock().unwrap_or_else(|e| e.into_inner()).push((at.to_path_buf(), root.to_path_buf(), aside.to_path_buf(), leave_empty));
    }

    /// The walk is about to list `dir`.
    pub(super) fn listing(dir: &Path) {
        let mut unplugs = UNPLUGS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = unplugs.iter().position(|(at, _, _, _)| at == dir) {
            let (_, root, aside, leave_empty) = unplugs.remove(i);
            std::fs::rename(&root, aside).unwrap();
            if leave_empty {
                std::fs::create_dir(&root).unwrap();
            }
        }
    }

    /// The error a test set for `at` on `path`, if any.
    pub(super) fn fault(at: Fault, path: &Path) -> Option<std::io::Error> {
        let faults = FAULTS.lock().unwrap_or_else(|e| e.into_inner());
        faults.iter().find(|(a, p, _)| *a == at && p == path).map(|(_, _, kind)| std::io::Error::new(*kind, format!("simulated {at:?} failure")))
    }

    /// How many times each file under `dir` was read, and forget them.
    pub(super) fn reads_under(dir: &Path) -> HashMap<PathBuf, usize> {
        let mut all = READS.lock().unwrap_or_else(|e| e.into_inner());
        let map = all.get_or_insert_with(HashMap::new);
        let mine: Vec<PathBuf> = map.keys().filter(|p| p.starts_with(dir)).cloned().collect();
        mine.into_iter().filter_map(|p| map.remove_entry(&p)).collect()
    }

    #[test]
    fn guarded_turns_a_panic_into_an_error() {
        assert_eq!(guarded(|| Ok::<_, String>(3)), Ok(3));
        assert_eq!(guarded(|| Err::<i32, _>("no".to_owned())), Err("no".to_owned()));
        assert_eq!(guarded::<i32>(|| panic!("boom")), Err("panicked: boom".to_owned()));
        let n = 7;
        assert_eq!(guarded::<i32>(|| panic!("boom {n}")), Err("panicked: boom 7".to_owned()));
    }

    #[test]
    fn disc_subfolders_belong_to_the_album() {
        assert_eq!(album_folder(Path::new("M:/A/Album/CD2/01.flac")), PathBuf::from("M:/A/Album"));
        assert_eq!(album_folder(Path::new("M:/A/Album/Disc 1/01.flac")), PathBuf::from("M:/A/Album"));
        assert_eq!(album_folder(Path::new("M:/A/Discovery/01.flac")), PathBuf::from("M:/A/Discovery"));
        assert_eq!(album_folder(Path::new("M:/A/CDs/01.flac")), PathBuf::from("M:/A/CDs"));
    }

    /// End to end: real (tiny, silent) WAV files with ID3 tags, scanned into a fresh
    /// database, to pin down when "recently added" is stamped and when it is kept.
    mod added_dates {
        use super::super::*;
        use lofty::config::WriteOptions;
        use lofty::prelude::*;
        use lofty::tag::{ItemKey, Tag, TagType};

        struct Lib {
            dir: PathBuf,
            conn: Connection,
            art: ArtCache,
        }

        impl Lib {
            fn new(name: &str) -> Self {
                let dir = std::env::temp_dir().join(format!("mp-scan-test-{name}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(dir.join("music")).unwrap();
                let conn = crate::db::open_in_memory().unwrap();
                let art = ArtCache::new(dir.join("art"));
                Self { dir, conn, art }
            }
            fn music(&self) -> PathBuf {
                self.dir.join("music")
            }
            /// Write `rel` as a short silent WAV tagged with album artist, album, title
            /// and optionally a MusicBrainz release id. Bumps the mtime so a rescan
            /// re-reads it.
            fn file(&self, rel: &str, artist: &str, album: &str, title: &str, mbid: Option<&str>) {
                let path = self.music().join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let rate = 8000u32;
                let data = vec![0u8; 800];
                let mut w = Vec::new();
                w.extend_from_slice(b"RIFF");
                w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
                w.extend_from_slice(b"WAVEfmt ");
                w.extend_from_slice(&16u32.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&rate.to_le_bytes());
                w.extend_from_slice(&rate.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&8u16.to_le_bytes());
                w.extend_from_slice(b"data");
                w.extend_from_slice(&(data.len() as u32).to_le_bytes());
                w.extend_from_slice(&data);
                std::fs::write(&path, w).unwrap();
                let mut tag = Tag::new(TagType::Id3v2);
                tag.insert_text(ItemKey::AlbumArtist, artist.into());
                tag.insert_text(ItemKey::TrackArtist, artist.into());
                tag.insert_text(ItemKey::AlbumTitle, album.into());
                tag.insert_text(ItemKey::TrackTitle, title.into());
                if let Some(mb) = mbid {
                    tag.insert_text(ItemKey::MusicBrainzReleaseId, mb.into());
                }
                tag.save_to_path(&path, WriteOptions::default()).unwrap();
                // Different size or mtime than before, so the scanner re-reads it.
                let f = std::fs::File::options().write(true).open(&path).unwrap();
                let later = std::time::SystemTime::now() + std::time::Duration::from_secs(self.bump());
                f.set_modified(later).unwrap();
            }
            fn bump(&self) -> u64 {
                let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();
                10 + n as u64 * 3 + std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 % 7
            }
            fn scan(&mut self) {
                let roots = vec![self.music()];
                scan(&mut self.conn, &self.art, &roots, &|_| {}).unwrap();
            }
            /// Pretend every album so far was first seen long ago.
            fn age_all(&self) {
                self.conn.execute("UPDATE albums SET added_at = 1000, first_seen = 1000", []).unwrap();
            }
            fn added(&self, title: &str) -> i64 {
                self.conn.query_row("SELECT added_at FROM albums WHERE title = ?1", [title], |r| r.get(0)).unwrap()
            }
            fn albums(&self) -> i64 {
                self.conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0)).unwrap()
            }
        }

        impl Drop for Lib {
            fn drop(&mut self) {
                super::clear_faults(&self.dir);
                // A test may have opened a database file in the folder (`old_state`): closed
                // first, or Windows will not remove it.
                if let Ok(c) = Connection::open_in_memory() {
                    drop(std::mem::replace(&mut self.conn, c));
                }
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        #[test]
        fn a_panic_in_one_file_is_that_files_error() {
            let mut lib = Lib::new("panic");
            lib.file("A/Album/01.wav", "A", "Album", "one", None);
            lib.file("A/Album/panics-here.wav", "A", "Album", "two", None);
            lib.file("A/Album/03.wav", "A", "Album", "three", None);
            let roots = vec![lib.music()];
            let r = scan(&mut lib.conn, &lib.art, &roots, &|_| {}).expect("the scan completes");
            assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
            assert!(r.errors[0].0.ends_with("panics-here.wav"));
            assert!(r.errors[0].1.contains("panicked"), "{}", r.errors[0].1);
            let n: i64 = lib.conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 2, "the other files are indexed");
        }

        /// Without `want_art` the tags (and duration) are the same; only the picture is
        /// not read.
        #[test]
        fn art_is_read_only_when_wanted() {
            use lofty::picture::{MimeType, Picture, PictureType};
            let lib = Lib::new("art");
            lib.file("A/Album/01.wav", "A", "Album", "one", Some("mb-1"));
            let path = lib.music().join("A/Album/01.wav");
            let mut tagged = lofty::read_from_path(&path).unwrap();
            let tag = tagged.primary_tag_mut().unwrap();
            let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9];
            tag.push_picture(Picture::unchecked(jpeg.clone()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
            tag.save_to_path(&path, WriteOptions::default()).unwrap();
            let (with, art) = crate::tags::read(&path, true).unwrap();
            let (without, none) = crate::tags::read(&path, false).unwrap();
            assert_eq!(art.as_deref(), Some(&jpeg[..]));
            assert_eq!(none, None);
            assert_eq!(with, without);
            assert_eq!(without.title.as_deref(), Some("one"));
            assert_eq!(without.mb_album_id.as_deref(), Some("mb-1"));
            // And lofty really skips the picture with the option `tags::read` passes.
            let opts = lofty::config::ParseOptions::new().read_cover_art(false);
            let f = lofty::probe::Probe::open(&path).unwrap().options(opts).read().unwrap();
            assert!(f.primary_tag().unwrap().pictures().is_empty());
            assert_eq!(lofty::read_from_path(&path).unwrap().primary_tag().unwrap().pictures().len(), 1);
        }

        #[test]
        fn new_albums_are_stamped_and_kept() {
            let mut lib = Lib::new("stamp");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.scan();
            lib.age_all();
            lib.scan();
            assert_eq!(lib.added("Old"), 1000, "an unchanged rescan keeps the date");
            lib.file("B/New/01.wav", "B", "New", "one", None);
            lib.scan();
            assert_eq!(lib.added("Old"), 1000);
            assert!(lib.added("New") > 1_700_000_000_000, "a new album is stamped now, in milliseconds");
        }

        #[test]
        fn a_track_added_to_an_album_keeps_its_date() {
            let mut lib = Lib::new("addtrack");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.scan();
            lib.age_all();
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.scan();
            assert_eq!(lib.albums(), 1);
            assert_eq!(lib.added("Old"), 1000);
        }

        #[test]
        fn a_retag_that_changes_the_album_identity_keeps_the_date() {
            let mut lib = Lib::new("retag");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.scan();
            lib.age_all();
            // Picard adds the MusicBrainz release id: the album key changes.
            lib.file("A/Old/01.wav", "A", "Old", "one", Some("1b2c3d"));
            lib.file("A/Old/02.wav", "A", "Old", "two", Some("1b2c3d"));
            lib.scan();
            assert_eq!(lib.albums(), 1);
            let key: String = lib.conn.query_row("SELECT key FROM albums", [], |r| r.get(0)).unwrap();
            assert_eq!(key, "mb:1b2c3d");
            assert_eq!(lib.added("Old"), 1000);
        }

        impl Lib {
            /// The path as the scanner stores it (walkdir joins with `\` on Windows).
            fn p(&self, rel: &str) -> String {
                rel.split('/').fold(self.music(), |p, c| p.join(c)).to_string_lossy().into_owned()
            }
            fn id(&self, rel: &str) -> Option<i64> {
                self.conn.query_row("SELECT id FROM tracks WHERE path = ?1", [self.p(rel)], |r| r.get(0)).optional().unwrap()
            }
            fn tracks(&self) -> i64 {
                self.conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap()
            }
            fn mv(&self, from: &str, to: &str) {
                let to = PathBuf::from(self.p(to));
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::rename(self.p(from), to).unwrap();
            }
            /// Play, favourite and add to a playlist, as a user would.
            fn history(&self, id: i64) -> i64 {
                crate::query::record_play(&self.conn, id).unwrap();
                crate::query::record_play(&self.conn, id).unwrap();
                crate::query::toggle_favourite_track(&self.conn, id).unwrap();
                let pl = crate::query::create_playlist(&self.conn, "mix").unwrap();
                crate::query::add_to_playlist(&self.conn, pl, &[id]).unwrap();
                pl
            }
            fn has_history(&self, id: i64, playlist: i64) -> bool {
                let plays = crate::query::play_count(&self.conn, id).unwrap();
                let fav = crate::query::is_favourite_track(&self.conn, id).unwrap();
                let listed = crate::query::playlist_tracks(&self.conn, playlist).unwrap().iter().any(|t| t.id == id);
                plays == 2 && fav && listed
            }
            fn plays_total(&self) -> i64 {
                self.conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap()
            }
            fn scan_report(&mut self, opts: Options) -> Report {
                let roots = vec![self.music()];
                scan_with(&mut self.conn, &self.art, &roots, opts, &|_| {}).unwrap()
            }
        }

        #[test]
        fn a_moved_folder_keeps_track_history_and_the_album_favourite() {
            let mut lib = Lib::new("move-history");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            let pl = lib.history(id);
            let album: i64 = lib.conn.query_row("SELECT album_id FROM tracks WHERE id = ?1", [id], |r| r.get(0)).unwrap();
            crate::query::toggle_favourite_album(&lib.conn, album).unwrap();
            // Untagged album (no MusicBrainz id): the album key has the folder in it.
            lib.mv("A/Old", "B/Elsewhere/Old (2020)");
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (2, 0), "{r:?}");
            assert_eq!(lib.id("B/Elsewhere/Old (2020)/01.wav"), Some(id), "same row, new path");
            assert!(lib.has_history(id, pl));
            assert_eq!(lib.tracks(), 2);
            assert_eq!(lib.albums(), 1);
            let new_album: i64 = lib.conn.query_row("SELECT album_id FROM tracks WHERE id = ?1", [id], |r| r.get(0)).unwrap();
            assert_ne!(new_album, album, "a new album row (its key changed)");
            assert!(crate::query::is_favourite_album(&lib.conn, new_album).unwrap(), "that kept the favourite");
        }

        #[test]
        fn a_renamed_file_keeps_its_history() {
            let mut lib = Lib::new("rename");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            let pl = lib.history(id);
            lib.mv("A/Old/01.wav", "A/Old/01 - One.wav");
            let r = lib.scan_report(Options::default());
            assert_eq!(r.moved, 1);
            assert_eq!(lib.id("A/Old/01 - One.wav"), Some(id));
            assert!(lib.has_history(id, pl));
        }

        #[test]
        fn a_retag_plus_move_keeps_its_history() {
            let mut lib = Lib::new("retag-move");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            let pl = lib.history(id);
            // A tagger writes a MusicBrainz id and a corrected album artist (the size
            // changes), and files the album under a new folder (same file name).
            lib.mv("A/Old", "Artist A/Old");
            lib.file("Artist A/Old/01.wav", "Artist A", "Old", "one", Some("mb-old"));
            lib.file("Artist A/Old/02.wav", "Artist A", "Old", "two", Some("mb-old"));
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (2, 0), "{r:?}");
            assert_eq!(lib.id("Artist A/Old/01.wav"), Some(id));
            assert!(lib.has_history(id, pl));
            // And renamed as well as retagged: same artist and album still pair them.
            lib.mv("Artist A/Old/01.wav", "Artist A/Old/01 One.wav");
            lib.file("Artist A/Old/01 One.wav", "Artist A", "Old", "one", Some("mb-old"));
            lib.conn.execute("UPDATE tracks SET size = size + 1 WHERE id = ?1", [id]).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!(r.moved, 1, "{r:?}");
            assert_eq!(lib.id("Artist A/Old/01 One.wav"), Some(id));
        }

        #[cfg(windows)]
        #[test]
        fn a_folder_renamed_only_in_letter_case_keeps_its_history() {
            let mut lib = Lib::new("case");
            lib.file("A/old album/01.wav", "A", "Old", "one", None);
            lib.scan();
            let id = lib.id("A/old album/01.wav").unwrap();
            let pl = lib.history(id);
            lib.mv("A/old album", "A/Old Album");
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (1, 0), "{r:?}");
            assert_eq!(lib.id("A/Old Album/01.wav"), Some(id));
            assert!(lib.has_history(id, pl));
            assert_eq!(lib.tracks(), 1);
        }

        /// The same seen by the folder watcher, which names the folder in both spellings (or only
        /// the new one): its partial scan hands over to a full one, which takes the rename for a
        /// move. It took the new spelling's files for new tracks beside the old ones, for good.
        #[cfg(any(windows, target_os = "macos"))]
        #[test]
        fn a_folder_renamed_only_in_letter_case_seen_by_the_watcher_keeps_its_history() {
            for reported in [&["A/old album", "A/Old Album"][..], &["A/Old Album/01.wav"][..]] {
                let mut lib = Lib::new("case-watched");
                lib.file("A/old album/01.wav", "A", "Old", "one", None);
                lib.file("A/old album/02.wav", "A", "Old", "two", None);
                if !Path::new(&lib.p("A/OLD ALBUM/01.wav")).exists() {
                    eprintln!("the temp folder's volume is case-sensitive: nothing to test");
                    return;
                }
                lib.scan();
                let id = lib.id("A/old album/01.wav").unwrap();
                let pl = lib.history(id);
                lib.mv("A/old album", "A/Old Album");
                let r = lib.scan_changed(reported);
                assert_eq!((r.partial, r.removed), (false, 0), "{reported:?}: {r:?}");
                assert_eq!(lib.tracks(), 2, "{reported:?}");
                assert_eq!(lib.id("A/Old Album/01.wav"), Some(id));
                assert!(lib.has_history(id, pl));
            }
        }

        /// Linux counterpart: paths are case-sensitive there, so the folder renamed only in
        /// letter case is another folder; the move rules take the history along. (macOS takes
        /// it for the same files moved, `FOLDS_CASE`, with the same outcome.)
        #[cfg(not(windows))]
        #[test]
        fn a_folder_renamed_only_in_letter_case_is_a_move_that_keeps_its_history() {
            let mut lib = Lib::new("case");
            lib.file("A/old album/01.wav", "A", "Old", "one", None);
            lib.scan();
            let id = lib.id("A/old album/01.wav").unwrap();
            let pl = lib.history(id);
            lib.mv("A/old album", "A/Old Album");
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (1, 0), "{r:?}");
            assert_eq!(lib.id("A/Old Album/01.wav"), Some(id));
            assert_eq!(lib.id("A/old album/01.wav"), None, "the old spelling is gone");
            assert!(lib.has_history(id, pl));
            assert_eq!(lib.tracks(), 1);
        }

        #[test]
        fn identical_looking_files_are_not_matched() {
            let mut lib = Lib::new("ambiguous");
            // Two copies of the same file (same tags, size and duration) in two folders.
            lib.file("A/Copy 1/01.wav", "A", "Same", "one", None);
            lib.file("A/Copy 2/01.wav", "A", "Same", "one", None);
            lib.file("A/Other/01.wav", "A", "Other", "x", None);
            lib.scan();
            let one = lib.id("A/Copy 1/01.wav").unwrap();
            let two = lib.id("A/Copy 2/01.wav").unwrap();
            lib.history(one);
            // Both move: which new file is which cannot be told, so neither matches.
            lib.mv("A/Copy 1", "B/X");
            lib.mv("A/Copy 2", "B/Y");
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (0, 2), "{r:?}");
            assert!(lib.id("B/X/01.wav").is_some_and(|i| i != one && i != two));
            assert_eq!(lib.plays_total(), 0, "the history did not go to either copy");

            // One vanished track and two new files that both fit it: no match either.
            let mut lib = Lib::new("ambiguous2");
            lib.file("A/Orig/01.wav", "A", "Same", "one", None);
            lib.file("A/Other/01.wav", "A", "Other", "x", None);
            lib.scan();
            let id = lib.id("A/Orig/01.wav").unwrap();
            lib.history(id);
            lib.mv("A/Orig", "B/X");
            std::fs::create_dir_all(PathBuf::from(lib.p("B/Y"))).unwrap();
            std::fs::copy(lib.p("B/X/01.wav"), lib.p("B/Y/01.wav")).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!(r.moved, 0, "{r:?}");
            assert_eq!(lib.plays_total(), 0);
        }

        #[test]
        fn a_different_track_is_not_taken_for_a_moved_one() {
            let mut lib = Lib::new("different");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Keep/01.wav", "A", "Keep", "k", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            lib.history(id);
            // Deleted, and an unrelated track (another title) added the same scan.
            std::fs::remove_file(lib.p("A/Old/01.wav")).unwrap();
            lib.file("A/New/01.wav", "A", "Old", "two", None);
            let r = lib.scan_report(Options::default());
            assert_eq!((r.moved, r.removed), (0, 1), "{r:?}");
            assert_ne!(lib.id("A/New/01.wav"), Some(id));
            assert_eq!(lib.plays_total(), 0);
        }

        fn many(lib: &Lib, n: usize) {
            for i in 0..n {
                lib.file(&format!("Art {}/Album {}/{i:02}.wav", i % 3, i / 4), "A", &format!("Album {}", i / 4), &format!("t{i}"), None);
            }
        }

        #[test]
        fn a_root_found_empty_is_held_until_confirmed() {
            let mut lib = Lib::new("guard");
            many(&lib, 30);
            lib.scan();
            let id = lib.id("Art 0/Album 0/00.wav").unwrap();
            let pl = lib.history(id);
            // The folder is still there but everything in it is gone (an empty mount).
            let aside = lib.dir.join("aside");
            std::fs::rename(lib.music(), &aside).unwrap();
            std::fs::create_dir_all(lib.music()).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!(r.removed, 0);
            assert_eq!(r.held, vec![Held { root: lib.music(), missing: 30, known: 30, unreadable: None }]);
            assert_eq!(lib.tracks(), 30, "kept");
            // It comes back: everything as it was, nothing held.
            std::fs::remove_dir_all(lib.music()).unwrap();
            std::fs::rename(&aside, lib.music()).unwrap();
            let r = lib.scan_report(Options::default());
            assert!(r.held.is_empty() && r.removed == 0, "{r:?}");
            assert!(lib.has_history(id, pl));
            // Gone again, and this time the user confirms: removed.
            std::fs::remove_dir_all(lib.music()).unwrap();
            std::fs::create_dir_all(lib.music()).unwrap();
            assert_eq!(lib.scan_report(Options::default()).held.len(), 1);
            let r = lib.scan_report(Options { confirm_removal: true, ..Default::default() });
            assert_eq!((r.removed, r.held.len()), (30, 0));
            assert_eq!((lib.tracks(), lib.albums()), (0, 0));
        }

        #[test]
        fn deleting_some_albums_is_not_held() {
            let mut lib = Lib::new("guard-some");
            many(&lib, 40);
            lib.scan();
            // Most of the library deleted on purpose: 36 of 40 gone, 4 left (10%).
            for i in 0..36 {
                std::fs::remove_file(lib.p(&format!("Art {}/Album {}/{i:02}.wav", i % 3, i / 4))).unwrap();
            }
            let r = lib.scan_report(Options::default());
            assert!(r.held.is_empty(), "{r:?}");
            assert_eq!((r.removed, lib.tracks()), (36, 4));
            // A small library emptied entirely (fewer than GUARD_MIN_MISSING) is pruned too.
            let mut lib = Lib::new("guard-small");
            many(&lib, 10);
            lib.scan();
            std::fs::remove_dir_all(lib.music()).unwrap();
            std::fs::create_dir_all(lib.music()).unwrap();
            let r = lib.scan_report(Options::default());
            assert!(r.held.is_empty());
            assert_eq!(lib.tracks(), 0);
        }

        /// The drive goes part way through the walk (a third of the library walked: enough for the
        /// guard to let a removal through): what was not reached is held, not removed. Gone at
        /// once (a drive letter) or leaving an empty folder behind (a mount point on Linux).
        #[test]
        fn a_drive_unplugged_mid_walk_keeps_what_was_not_walked() {
            for leave_empty in [false, true] {
                let mut lib = Lib::new(&format!("unplug-{leave_empty}"));
                many(&lib, 60);
                lib.scan();
                let id = lib.id("Art 2/Album 0/02.wav").unwrap();
                let pl = lib.history(id);
                let aside = lib.dir.join("aside");
                super::set_unplug(&lib.music().join("Art 1"), &lib.music(), &aside, leave_empty);
                let r = lib.scan_report(Options::default());
                assert_eq!(r.removed, 0, "{r:?}");
                assert_eq!(held_unread(&r), vec![(lib.music(), 40, 60)], "{r:?}");
                assert_eq!(lib.tracks(), 60);
                // Plugged back in: all there, history and all.
                if leave_empty {
                    std::fs::remove_dir(lib.music()).unwrap();
                }
                std::fs::rename(&aside, lib.music()).unwrap();
                let r = lib.scan_report(Options::default());
                assert!(r.held.is_empty() && r.removed == 0, "{r:?}");
                assert!(lib.has_history(id, pl));
            }
        }

        #[test]
        fn forgetting_a_root_removes_only_what_is_under_it() {
            let mut lib = Lib::new("forget");
            lib.file("A/X/01.wav", "A", "X", "one", None);
            lib.file("AB/Y/01.wav", "A", "Y", "two", None);
            lib.scan();
            let id = lib.id("A/X/01.wav").unwrap();
            lib.history(id);
            let root = PathBuf::from(lib.p("A"));
            let n = forget_root(&mut lib.conn, &root, &[]).unwrap();
            assert_eq!(n, 1);
            assert_eq!(lib.id("A/X/01.wav"), None);
            assert!(lib.id("AB/Y/01.wav").is_some(), "a sibling sharing the name's start stays");
            assert_eq!((lib.albums(), lib.plays_total()), (1, 0));
        }

        #[test]
        fn forgetting_a_root_drops_artists_and_genres_credited_nowhere_else() {
            let mut lib = Lib::new("forget-relations");
            lib.tagged(
                "Gone/X/01.wav",
                &[
                    (K::TrackArtist, "Solo Ghost"),
                    (K::AlbumArtist, "Solo Ghost"),
                    (K::AlbumTitle, "Ghost Album"),
                    (K::TrackTitle, "One"),
                    (K::Genre, "Spectral"),
                ],
            );
            lib.tagged(
                "Gone/X/02.wav",
                &[
                    (K::TrackArtist, "Solo Ghost feat. Shared Guest"),
                    (K::AlbumArtist, "Solo Ghost"),
                    (K::AlbumTitle, "Ghost Album"),
                    (K::TrackTitle, "Two"),
                    (K::Genre, "Spectral; Shared Genre"),
                ],
            );
            lib.tagged(
                "Keep/Y/01.wav",
                &[
                    (K::TrackArtist, "Staying Band feat. Shared Guest"),
                    (K::AlbumArtist, "Staying Band"),
                    (K::AlbumTitle, "Stay Album"),
                    (K::TrackTitle, "Three"),
                    (K::Genre, "Shared Genre"),
                ],
            );
            lib.scan();
            assert!(crate::query::find_artist(&lib.conn, "Solo Ghost").unwrap().is_some());
            assert_eq!(crate::query::search(&lib.conn, "Solo Ghost", 10).unwrap().artists.len(), 1);
            let genre_count = |lib: &Lib, key: &str| -> i64 {
                lib.conn.query_row("SELECT COUNT(*) FROM genres WHERE key = ?1", [key], |r| r.get(0)).unwrap()
            };
            assert_eq!(genre_count(&lib, "spectral"), 1);

            let root = PathBuf::from(lib.p("Gone"));
            let n = forget_root(&mut lib.conn, &root, &[]).unwrap();
            assert_eq!(n, 2);
            lib.assert_no_orphans();
            assert!(crate::query::find_artist(&lib.conn, "Solo Ghost").unwrap().is_none());
            assert!(crate::query::search(&lib.conn, "Solo Ghost", 10).unwrap().artists.is_empty());
            assert!(crate::query::search(&lib.conn, "artist:\"Solo Ghost\"", 10).unwrap().artists.is_empty());
            assert_eq!(genre_count(&lib, "spectral"), 0);
            assert!(crate::query::find_artist(&lib.conn, "Staying Band").unwrap().is_some());
            assert!(crate::query::find_artist(&lib.conn, "Shared Guest").unwrap().is_some());
            assert_eq!(genre_count(&lib, "shared genre"), 1);
        }

        #[cfg(windows)]
        #[test]
        fn forgetting_a_root_ignores_letter_case_on_windows() {
            let mut lib = Lib::new("forget-case");
            lib.file("A/X/01.wav", "Only A", "X", "one", None);
            lib.file("AB/Y/01.wav", "Only AB", "Y", "two", None);
            lib.scan();
            let flipped: String = lib
                .p("A")
                .chars()
                .map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() })
                .collect();
            let n = forget_root(&mut lib.conn, Path::new(&flipped), &[]).unwrap();
            assert_eq!(n, 1);
            assert_eq!(lib.id("A/X/01.wav"), None);
            assert!(lib.id("AB/Y/01.wav").is_some(), "a sibling sharing the name's start stays");
            lib.assert_no_orphans();
        }

        /// One library folder inside another: what is in both is walked once, as the outer
        /// one's, and the scans finish (a new file there was read twice and failed every scan).
        #[test]
        fn a_folder_inside_another_is_scanned_once() {
            let mut lib = Lib::new("nested");
            lib.file("A/X/01.wav", "A", "X", "one", None);
            lib.file("Jazz/Y/01.wav", "B", "Y", "two", None);
            let roots = vec![lib.music(), lib.music().join("Jazz")];
            scan_with(&mut lib.conn, &lib.art, &roots, Options::default(), &|_| {}).expect("a first scan");
            assert_eq!(lib.tracks(), 2);
            lib.file("Jazz/New/01.wav", "B", "New", "three", None);
            let r = scan_with(&mut lib.conn, &lib.art, &roots, Options::default(), &|_| {}).expect("a rescan");
            assert_eq!((r.files, r.removed, lib.tracks()), (3, 0, 3));
            // The watcher's scans too, whichever root comes first.
            lib.file("Jazz/New/02.wav", "B", "New", "four", None);
            let inner_first = vec![lib.music().join("Jazz"), lib.music()];
            let only = scopes(&[PathBuf::from(lib.p("Jazz/New/02.wav"))], &inner_first);
            assert_eq!(only, Some(vec![PathBuf::from(lib.p("Jazz/New"))]));
            scan_with(&mut lib.conn, &lib.art, &inner_first, Options { only, ..Default::default() }, &|_| {}).expect("a partial scan");
            assert_eq!(lib.tracks(), 4);
            assert_eq!(scopes(&[PathBuf::from(lib.p("Jazz/03.wav"))], &inner_first), Some(vec![PathBuf::from(lib.p("Jazz"))]), "the inner root is a folder of the outer one");
        }

        #[test]
        fn outermost_drops_roots_inside_others() {
            let p = PathBuf::from;
            let roots = vec![p("M:/Music/Jazz"), p("M:/Music"), p("M:/Musicals"), p("M:/Music/"), p("N:/Other")];
            assert_eq!(outermost(&roots), vec![p("M:/Music"), p("M:/Musicals"), p("N:/Other")]);
            #[cfg(windows)]
            assert_eq!(outermost(&[p(r"m:\music\jazz"), p(r"M:\Music")]), vec![p(r"M:\Music")], "letter case aside on Windows");
            #[cfg(unix)]
            assert_eq!(outermost(&[p("/m/music/jazz"), p("/m/Music")]).len(), 2);
        }

        /// Removing a folder removes nothing another library folder still holds: the inner one
        /// removes nothing, the outer one leaves what is in the inner one.
        #[test]
        fn forgetting_a_nested_root_keeps_what_another_root_holds() {
            let mut lib = Lib::new("forget-nested");
            lib.file("A/X/01.wav", "A", "X", "one", None);
            lib.file("Jazz/Y/01.wav", "B", "Y", "two", None);
            lib.scan();
            let id = lib.id("Jazz/Y/01.wav").unwrap();
            let pl = lib.history(id);
            let (outer, inner) = (lib.music(), lib.music().join("Jazz"));
            assert_eq!(forget_root(&mut lib.conn, &inner, std::slice::from_ref(&outer)).unwrap(), 0);
            assert_eq!(lib.tracks(), 2);
            assert_eq!(forget_root(&mut lib.conn, &outer, std::slice::from_ref(&inner)).unwrap(), 1);
            assert_eq!(lib.id("A/X/01.wav"), None);
            assert!(lib.has_history(id, pl));
            // The last folder removed: all of it goes.
            assert_eq!(forget_root(&mut lib.conn, &inner, &[]).unwrap(), 1);
            assert_eq!(lib.tracks(), 0);
        }

        impl Lib {
            /// A partial scan of what the watcher would report for these paths.
            fn scan_changed(&mut self, changed: &[&str]) -> Report {
                let roots = vec![self.music()];
                let paths: Vec<PathBuf> = changed.iter().map(|c| PathBuf::from(self.p(c))).collect();
                let only = scopes(&paths, &roots);
                assert!(only.is_some(), "{changed:?} needs a full scan");
                scan_with(&mut self.conn, &self.art, &roots, Options { only, ..Default::default() }, &|_| {}).unwrap()
            }
        }

        #[test]
        fn scopes_fold_nested_folders_and_send_roots_to_a_full_scan() {
            let root = PathBuf::from("M:/Music");
            let roots = vec![root.clone()];
            let p = |s: &str| root.join(s);
            let got = scopes(&[p("A/X/01.flac"), p("A/X/cover.jpg"), p("A/X/CD2/01.flac"), p("A"), p("B/Gone Vol. 2")], &roots);
            assert_eq!(got, Some(vec![p("A"), p("B/Gone Vol. 2")]));
            assert_eq!(scopes(&[p("01.flac")], &roots), None, "a file in the root itself: everything");
            assert_eq!(scopes(std::slice::from_ref(&root), &roots), None);
            assert_eq!(scopes(&[PathBuf::from("N:/Else/01.flac")], &roots), Some(vec![]), "outside the library: nothing");
            let many: Vec<PathBuf> = (0..=MAX_SCOPES).map(|i| p(&format!("F{i}"))).collect();
            assert_eq!(scopes(&many, &roots), None);
        }

        #[test]
        fn a_partial_scan_reads_only_its_folders_and_adds_what_is_new() {
            let mut lib = Lib::new("partial-add");
            lib.file("A/One/01.wav", "A", "One", "one", None);
            lib.file("B/Two/01.wav", "B", "Two", "two", None);
            lib.scan();
            lib.file("A/One/02.wav", "A", "One", "one b", None);
            // Changed too, but not reported: a partial scan of A/One does not look.
            lib.file("B/Two/01.wav", "B", "Two", "two retitled", None);
            let r = lib.scan_changed(&["A/One/02.wav"]);
            assert!(r.partial && r.read == 1 && r.files == 2, "{r:?}");
            assert_eq!(lib.tracks(), 3);
            let title: String = lib.conn.query_row("SELECT title FROM tracks WHERE path = ?1", [lib.p("B/Two/01.wav")], |r| r.get(0)).unwrap();
            assert_eq!(title, "two");
        }

        #[test]
        fn a_partial_scan_matches_a_move_from_a_folder_it_did_not_look_at() {
            let mut lib = Lib::new("partial-move");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Keep/01.wav", "A", "Keep", "k", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            let pl = lib.history(id);
            lib.mv("A/Old/01.wav", "B/New/01.wav");
            // Only the destination is reported.
            let r = lib.scan_changed(&["B/New/01.wav"]);
            assert!(r.partial && r.moved == 1 && r.removed == 0, "{r:?}");
            assert_eq!(lib.id("B/New/01.wav"), Some(id), "same row, new path");
            assert!(lib.has_history(id, pl));
            assert_eq!(lib.tracks(), 2);
        }

        #[test]
        fn a_partial_scan_never_removes_and_a_full_scan_takes_over() {
            let mut lib = Lib::new("partial-remove");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.file("A/Old/02.wav", "A", "Old", "two", None);
            lib.file("B/Other/01.wav", "B", "Other", "x", None);
            lib.scan();
            std::fs::remove_file(lib.p("A/Old/02.wav")).unwrap();
            let r = lib.scan_changed(&["A/Old/02.wav"]);
            assert!(!r.partial, "a removal makes it a full scan: {r:?}");
            assert_eq!((r.removed, lib.tracks()), (1, 2));
        }

        #[test]
        fn a_partial_scan_in_a_root_found_empty_holds_like_a_full_scan() {
            let mut lib = Lib::new("partial-guard");
            many(&lib, 30);
            lib.scan();
            // The share comes back empty, and one album is being copied back in.
            let aside = lib.dir.join("aside");
            std::fs::rename(lib.music(), &aside).unwrap();
            std::fs::create_dir_all(lib.music().join("Art 0/Album 0")).unwrap();
            std::fs::copy(aside.join("Art 0/Album 0/00.wav"), lib.music().join("Art 0/Album 0/00.wav")).unwrap();
            let r = lib.scan_changed(&["Art 0/Album 0/00.wav"]);
            assert!(!r.partial, "{r:?}");
            assert_eq!(r.removed, 0, "nothing pruned: the full scan's guard holds the root");
            assert_eq!(r.held.len(), 1);
            assert_eq!(lib.tracks(), 30);
        }

        /// What a scan held because it could not read it: (path, missing, known).
        fn held_unread(r: &Report) -> Vec<(PathBuf, usize, usize)> {
            r.held.iter().filter(|h| h.unreadable.is_some()).map(|h| (h.root.clone(), h.missing, h.known)).collect()
        }

        #[test]
        fn a_folder_that_cannot_be_listed_keeps_its_tracks() {
            use super::{Fault, set_fault};
            let mut lib = Lib::new("unlistable");
            lib.file("A/One/01.wav", "A", "One", "one", None);
            lib.file("A/One/02.wav", "A", "One", "two", None);
            lib.file("B/Two/01.wav", "B", "Two", "x", None);
            lib.scan();
            let id = lib.id("A/One/01.wav").unwrap();
            let pl = lib.history(id);
            // A share's error on one folder mid-walk; meanwhile a file elsewhere really goes.
            let one = PathBuf::from(lib.p("A/One"));
            set_fault(Fault::List, &one, std::io::ErrorKind::PermissionDenied);
            std::fs::remove_file(lib.p("B/Two/01.wav")).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!(r.removed, 1, "what is really gone goes: {r:?}");
            assert_eq!(held_unread(&r), vec![(one.clone(), 2, 2)]);
            assert!(r.errors.iter().any(|(p, _)| *p == one), "and the folder is in the unreadable list: {r:?}");
            assert_eq!(lib.tracks(), 2);
            assert!(lib.has_history(id, pl));
            // Confirming the guard's removal does not remove what was not seen.
            let r = lib.scan_report(Options { confirm_removal: true, ..Default::default() });
            assert_eq!((r.removed, lib.tracks(), held_unread(&r).len()), (0, 2, 1));
            // It reads again: nothing held, nothing lost.
            super::clear_faults(&lib.dir);
            let r = lib.scan_report(Options::default());
            assert!(r.held.is_empty() && r.removed == 0 && r.errors.is_empty(), "{r:?}");
            assert!(lib.has_history(id, pl));
            // Really gone: removed.
            std::fs::remove_dir_all(&one).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!((r.removed, lib.tracks()), (2, 0));
        }

        #[test]
        fn a_file_that_cannot_be_looked_at_keeps_its_track() {
            use super::{Fault, set_fault};
            use std::io::ErrorKind;
            let mut lib = Lib::new("unlookable");
            for (i, t) in ["one", "two", "three", "four"].iter().enumerate() {
                lib.file(&format!("A/One/0{}.wav", i + 1), "A", "One", t, None);
            }
            lib.scan();
            let one = PathBuf::from(lib.p("A/One"));
            let (two, three, four) = (one.join("02.wav"), one.join("03.wav"), one.join("04.wav"));
            // Its metadata fails.
            set_fault(Fault::Look, &two, ErrorKind::PermissionDenied);
            // The walk is told it went (deleted meanwhile), but it is there: kept.
            set_fault(Fault::Look, &three, ErrorKind::NotFound);
            // Told it went, and looking at it once more fails too (a share timing out): held.
            set_fault(Fault::Look, &four, ErrorKind::NotFound);
            set_fault(Fault::Exists, &four, ErrorKind::TimedOut);
            let r = lib.scan_report(Options::default());
            assert_eq!(r.removed, 0, "{r:?}");
            assert_eq!(held_unread(&r), vec![(one.clone(), 1, 4), (two, 1, 1)]);
            assert_eq!(lib.tracks(), 4);
            // A file really gone is still removed when nothing stands in the way.
            super::clear_faults(&lib.dir);
            std::fs::remove_file(&four).unwrap();
            let r = lib.scan_report(Options::default());
            assert!(r.held.is_empty(), "{r:?}");
            assert_eq!((r.removed, lib.tracks()), (1, 3));
        }

        #[test]
        fn a_partial_scan_keeps_what_it_could_not_read() {
            use super::{Fault, set_fault};
            let mut lib = Lib::new("partial-unreadable");
            lib.file("A/One/01.wav", "A", "One", "one", None);
            lib.file("A/One/Bonus/01.wav", "A", "One Bonus", "b", None);
            lib.file("B/Two/01.wav", "B", "Two", "x", None);
            lib.scan();
            let bonus = PathBuf::from(lib.p("A/One/Bonus"));
            set_fault(Fault::List, &bonus, std::io::ErrorKind::PermissionDenied);
            lib.file("A/One/02.wav", "A", "One", "two", None);
            let r = lib.scan_changed(&["A/One/02.wav"]);
            assert!(r.partial && r.removed == 0, "nothing gone: it stays partial: {r:?}");
            assert_eq!(held_unread(&r), vec![(bonus.clone(), 1, 1)]);
            assert_eq!(lib.tracks(), 4);
            // Something really gone as well: the full scan it hands over to removes that, and holds
            // the rest.
            std::fs::remove_file(lib.p("A/One/01.wav")).unwrap();
            let r = lib.scan_changed(&["A/One/01.wav"]);
            assert!(!r.partial, "{r:?}");
            assert_eq!(r.removed, 1);
            assert_eq!(held_unread(&r), vec![(bonus, 1, 1)]);
            assert_eq!(lib.tracks(), 3);
        }

        #[test]
        fn a_partial_scan_does_not_take_a_track_it_cannot_look_at_for_a_moved_one() {
            let mut lib = Lib::new("partial-unlookable");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.scan();
            let id = lib.id("A/Old/01.wav").unwrap();
            // A copy appears elsewhere while the original cannot be looked at: not a move.
            std::fs::create_dir_all(lib.music().join("B/New")).unwrap();
            std::fs::copy(lib.p("A/Old/01.wav"), lib.p("B/New/01.wav")).unwrap();
            super::set_fault(super::Fault::Exists, Path::new(&lib.p("A/Old/01.wav")), std::io::ErrorKind::TimedOut);
            let r = lib.scan_changed(&["B/New/01.wav"]);
            assert!(r.partial && r.moved == 0, "{r:?}");
            assert_eq!(lib.id("A/Old/01.wav"), Some(id));
            assert_eq!(lib.tracks(), 2);
        }

        /// The same through the system, not a test seam: a folder another program holds open
        /// without sharing it (Windows), or one with no permissions (Linux; not as root).
        #[test]
        fn a_folder_the_system_will_not_list_keeps_its_tracks() {
            let mut lib = Lib::new("os-unlistable");
            lib.file("A/One/01.wav", "A", "One", "one", None);
            lib.file("B/Two/01.wav", "B", "Two", "x", None);
            lib.scan();
            let one = PathBuf::from(lib.p("A/One"));
            #[cfg(windows)]
            let _lock = {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_FLAG_BACKUP_SEMANTICS: a folder can be opened at all.
                std::fs::OpenOptions::new().read(true).share_mode(0).custom_flags(0x0200_0000).open(&one).unwrap()
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&one, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            if std::fs::read_dir(&one).is_ok() {
                eprintln!("the system lists the folder anyway (running as root?): nothing to test");
                return;
            }
            let r = lib.scan_report(Options::default());
            assert_eq!(r.removed, 0, "{r:?}");
            assert_eq!(held_unread(&r), vec![(one.clone(), 1, 1)]);
            assert_eq!(lib.tracks(), 2);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&one, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        #[test]
        fn art_is_looked_for_again_only_when_the_folder_changes() {
            let mut lib = Lib::new("art-retry");
            lib.file("A/Bare/01.wav", "A", "Bare", "one", None);
            let r = lib.scan_report(Options::default());
            assert_eq!(r.albums_changed, 1, "a new album is looked at");
            let r = lib.scan_report(Options::default());
            assert_eq!(r.albums_changed, 0, "nothing changed: not looked at again");
            // A cover appears in the folder.
            let img = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 90]));
            img.save(lib.music().join("A/Bare/cover.png")).unwrap();
            let r = lib.scan_report(Options::default());
            assert_eq!(r.albums_changed, 1, "{r:?}");
            let art: Option<String> = lib.conn.query_row("SELECT art FROM albums", [], |r| r.get(0)).unwrap();
            assert!(art.is_some(), "the new cover is found");
        }

        /// A cache-format upgrade that cannot write an album's new files (on Windows, a cached
        /// image open elsewhere) leaves it not current, and the cache not marked upgraded, so
        /// the next scan tries again; a cover that is gone is kept as cached and marked.
        #[test]
        fn a_failed_art_upgrade_is_tried_again() {
            let mut lib = Lib::new("art-upgrade-retry");
            lib.file("A/Kept/01.wav", "A", "Kept", "one", None);
            lib.file("A/Gone/01.wav", "A", "Gone", "one", None);
            for (album, c) in [("Kept", 10u8), ("Gone", 200)] {
                let img = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, c]));
                img.save(lib.music().join(format!("A/{album}/cover.png"))).unwrap();
            }
            lib.scan();
            let stem = |lib: &Lib, album: &str| lib.conn.query_row("SELECT art FROM albums WHERE title = ?1", [album], |r| r.get::<_, String>(0)).unwrap();
            let (kept, gone) = (stem(&lib, "Kept"), stem(&lib, "Gone"));
            assert!(!lib.art.needs_upgrade());
            // Pretend both were cached by the previous format: the accent file without its
            // format line, and the cache's version behind.
            let art_dir = lib.dir.join("art");
            let old_format = |s: &str| {
                let accent = art_dir.join(format!("{s}_a.txt"));
                let first = std::fs::read_to_string(&accent).unwrap().lines().next().unwrap().to_owned();
                std::fs::write(&accent, first).unwrap();
            };
            old_format(&kept);
            old_format(&gone);
            std::fs::remove_file(art_dir.join("version.txt")).unwrap();
            std::fs::remove_file(lib.music().join("A/Gone/cover.png")).unwrap();
            assert!(lib.art.needs_upgrade() && !lib.art.is_current(&kept) && !lib.art.is_current(&gone));

            let full = art_dir.join(format!("{kept}_f.jpg"));
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_SHARE_READ only: the rename over it is refused, as under a viewer's lock.
                let _lock = std::fs::OpenOptions::new().read(true).share_mode(1).open(&full).unwrap();
                lib.scan();
            }
            #[cfg(not(windows))]
            {
                // A folder in its place: the rename over it is refused.
                let saved = std::fs::read(&full).unwrap();
                std::fs::remove_file(&full).unwrap();
                std::fs::create_dir(&full).unwrap();
                std::fs::write(full.join("keep"), b"x").unwrap();
                lib.scan();
                std::fs::remove_dir_all(&full).unwrap();
                std::fs::write(&full, saved).unwrap();
            }
            assert!(!lib.art.is_current(&kept), "not written: not stamped current");
            assert!(lib.art.is_current(&gone), "its source gone: kept as cached, and marked");
            assert!(lib.art.needs_upgrade(), "so the next scan upgrades again");
            assert_eq!(stem(&lib, "Kept"), kept, "the album keeps its art meanwhile");

            lib.scan();
            assert!(lib.art.is_current(&kept), "tried again, and written");
            assert!(!lib.art.needs_upgrade());
            assert_eq!(stem(&lib, "Kept"), kept);
            assert_eq!(stem(&lib, "Gone"), gone);
        }

        // ---- schema 8: several artists, grouping by all of a folder's tracks ----------------

        impl Lib {
            /// Write `rel` as a short silent WAV with these ID3v2.4 items (a key given twice
            /// is a multi-valued item, NUL-separated). Bumps the mtime.
            fn tagged(&self, rel: &str, items: &[(ItemKey, &str)]) {
                let path = self.music().join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                // Even: a RIFF chunk of odd length needs a pad byte the tag writer will not add.
                let data = vec![0u8; 800 + 2 * rel.len()];
                let mut w = Vec::new();
                w.extend_from_slice(b"RIFF");
                w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
                w.extend_from_slice(b"WAVEfmt ");
                w.extend_from_slice(&16u32.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&8000u32.to_le_bytes());
                w.extend_from_slice(&8000u32.to_le_bytes());
                w.extend_from_slice(&1u16.to_le_bytes());
                w.extend_from_slice(&8u16.to_le_bytes());
                w.extend_from_slice(b"data");
                w.extend_from_slice(&(data.len() as u32).to_le_bytes());
                w.extend_from_slice(&data);
                std::fs::write(&path, w).unwrap();
                if !items.is_empty() {
                    let mut tag = Tag::new(TagType::Id3v2);
                    for (k, v) in items {
                        tag.push(lofty::tag::TagItem::new(*k, lofty::tag::ItemValue::Text((*v).to_owned())));
                    }
                    tag.save_to_path(&path, WriteOptions::default()).unwrap();
                }
                let f = std::fs::File::options().write(true).open(&path).unwrap();
                let later = std::time::SystemTime::now() + std::time::Duration::from_secs(self.bump());
                f.set_modified(later).unwrap();
            }
            /// (title, album artist, compilation) of the album a track is in.
            fn album_of(&self, rel: &str) -> (String, String, bool) {
                self.conn
                    .query_row(
                        "SELECT a.title, a.album_artist, a.compilation FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.path = ?1",
                        [self.p(rel)],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .unwrap()
            }
            fn album_id(&self, rel: &str) -> i64 {
                self.conn.query_row("SELECT album_id FROM tracks WHERE path = ?1", [self.p(rel)], |r| r.get(0)).unwrap()
            }
            fn artist_page(&self, name: &str) -> (Vec<String>, Vec<String>) {
                let a = crate::query::find_artist(&self.conn, name).unwrap().unwrap_or_else(|| panic!("no artist {name}"));
                let (own, appears) = crate::query::artist_albums(&self.conn, a.id).unwrap();
                (own.into_iter().map(|a| a.title).collect(), appears.into_iter().map(|a| a.title).collect())
            }
            /// Albums without tracks, artists and genres nothing credits: none, ever.
            fn assert_no_orphans(&self) {
                let n = |sql: &str| -> i64 { self.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
                assert_eq!(n("SELECT COUNT(*) FROM albums WHERE id NOT IN (SELECT album_id FROM tracks)"), 0, "empty albums");
                assert_eq!(
                    n("SELECT COUNT(*) FROM artists WHERE id NOT IN (SELECT artist_id FROM track_artists UNION SELECT artist_id FROM album_artists)"),
                    0,
                    "artists nothing credits"
                );
                assert_eq!(n("SELECT COUNT(*) FROM genres WHERE id NOT IN (SELECT genre_id FROM track_genres UNION SELECT genre_id FROM album_genres)"), 0);
                assert_eq!(n("SELECT COUNT(*) FROM favourite_albums WHERE album_id NOT IN (SELECT id FROM albums)"), 0);
            }
        }

        use ItemKey as K;

        /// The tag editor's own writes come in like any retag: through a scan of the folder
        /// where they happened (what the folder watcher hands over), every track keeping its
        /// row and all that hangs off it, an album whose identity changed regrouped with its
        /// favourite and its "added" date, and nothing read twice.
        #[test]
        fn the_editors_own_writes_come_in_like_any_retag() {
            use crate::edit::{self, FileEdit};
            let mut lib = Lib::new("edits");
            std::fs::write(lib.dir.join(".mp-edit-fixtures"), b"scan test").unwrap();
            edit::atomic::set_guard(|p| edit::atomic::under_marker(p, ".mp-edit-fixtures"));
            for n in 1..=3 {
                let no = n.to_string();
                lib.tagged(&format!("Band/Old Name/{n}.wav"), &[(K::TrackArtist, "Band"), (K::AlbumArtist, "Band"), (K::AlbumTitle, "Old Name"), (K::TrackTitle, &format!("Song {n}")), (K::TrackNumber, &no)]);
            }
            lib.tagged("Band/Other/1.wav", &[(K::TrackArtist, "Band"), (K::AlbumTitle, "Other"), (K::TrackTitle, "Elsewhere")]);
            lib.scan();
            let album = lib.album_id("Band/Old Name/1.wav");
            let one = lib.id("Band/Old Name/1.wav").unwrap();
            let two = lib.id("Band/Old Name/2.wav").unwrap();
            let pl = lib.history(one);
            crate::query::set_rating(&lib.conn, two, 4).unwrap();
            crate::query::toggle_favourite_album(&lib.conn, album).unwrap();
            lib.conn.execute("UPDATE albums SET added_at = 1234 WHERE id = ?1", [album]).unwrap();
            let ids_before: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            // The album renamed in the editor, on every track; one title too.
            let journal = edit::Journal::new(&lib.dir.join("journal"));
            let edits: Vec<FileEdit> = (1..=3)
                .map(|n| {
                    let path = PathBuf::from(lib.p(&format!("Band/Old Name/{n}.wav")));
                    let tags = crate::meta::read(&path, crate::meta::ReadOptions::TAGS).unwrap();
                    let mut fields = vec![("ALBUM".to_owned(), vec!["New Name".to_owned()])];
                    if n == 2 {
                        fields.push(("TITLE".into(), vec!["Song Two".into()]));
                    }
                    FileEdit { expect: edit::expect_now(&path, &tags).unwrap(), path, fields, raw: Vec::new(), pictures: None, summary: Vec::new() }
                })
                .collect();
            let r = edit::save_all(&edits, &journal, "Rename", None, &std::sync::atomic::AtomicBool::new(false), &|_| {});
            assert_eq!(r.saved.len(), 3, "{r:?}");
            // What the watcher hands over: the paths that changed (the files, and the
            // editor's temporary files, gone by now).
            let changed: Vec<PathBuf> = (1..=3).map(|n| PathBuf::from(lib.p(&format!("Band/Old Name/{n}.wav")))).chain([PathBuf::from(lib.p("Band/Old Name/1.wav.mpedit-0000.tmp"))]).collect();
            let only = scopes(&changed, &[lib.music()]);
            assert_eq!(only.as_ref().map(Vec::len), Some(1), "one folder");
            let rep = lib.scan_report(Options { only: only.clone(), ..Default::default() });
            assert!(rep.partial);
            assert_eq!((rep.read, rep.removed, rep.moved), (3, 0, 0), "{rep:?}");
            // Every track, with its history.
            let ids_after: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            assert_eq!(ids_after, ids_before);
            assert!(lib.has_history(one, pl));
            assert_eq!(crate::query::rating(&lib.conn, two).unwrap(), 4);
            let title: String = lib.conn.query_row("SELECT title FROM tracks WHERE id = ?1", [two], |r| r.get(0)).unwrap();
            assert_eq!(title, "Song Two");
            // The album under its new name: one album, its favourite and date taken over.
            let renamed = lib.album_id("Band/Old Name/1.wav");
            assert_eq!(lib.album_of("Band/Old Name/3.wav").0, "New Name");
            assert_eq!(lib.album_id("Band/Old Name/3.wav"), renamed);
            assert!(crate::query::is_favourite_album(&lib.conn, renamed).unwrap(), "the favourite follows the album");
            let added: i64 = lib.conn.query_row("SELECT added_at FROM albums WHERE id = ?1", [renamed], |r| r.get(0)).unwrap();
            assert_eq!(added, 1234, "not new in Recently added");
            assert_eq!(lib.albums(), 2);
            lib.assert_no_orphans();
            // Asked again (the watcher saw the same writes twice): nothing to read.
            let rep = lib.scan_report(Options { only, ..Default::default() });
            assert_eq!(rep.read, 0);
            // Undone from the journal: back under the old name, still one album.
            let save = journal.list().pop().unwrap();
            let undo = journal.undo_edits(&save).unwrap();
            let r = edit::save_all(&undo, &journal, "Undo", Some(save.header.id.clone()), &std::sync::atomic::AtomicBool::new(false), &|_| {});
            assert_eq!(r.saved.len(), 3, "{r:?}");
            lib.scan_report(Options { only: scopes(&changed, &[lib.music()]), ..Default::default() });
            assert_eq!(lib.album_of("Band/Old Name/2.wav").0, "Old Name");
            assert!(crate::query::is_favourite_album(&lib.conn, lib.album_id("Band/Old Name/2.wav")).unwrap());
            assert_eq!(lib.albums(), 2);
            lib.assert_no_orphans();
        }

        #[test]
        fn a_compilation_without_an_album_artist_is_one_album() {
            let mut lib = Lib::new("comp");
            for (t, a) in ["Mara Lind", "Theo Vance", "Ines Okafor"].iter().enumerate() {
                let n = (t + 1).to_string();
                lib.tagged(&format!("Comp/Summer/{n}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Summer Hits"), (K::TrackTitle, a), (K::TrackNumber, &n)]);
            }
            // Two albums of one title in one folder, no album artist: the folder decides, one
            // compilation, whatever the numbers say.
            for a in ["Copper Monks", "Lunar Sparrows"] {
                for t in 1..=2 {
                    let n = t.to_string();
                    lib.tagged(&format!("Mixed/Hits/{a} {n}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Greatest Hits"), (K::TrackTitle, a), (K::TrackNumber, &n)]);
                }
            }
            // With album artist tags, two albums.
            for a in ["Copper Monks", "Lunar Sparrows"] {
                for t in 1..=2 {
                    let n = t.to_string();
                    lib.tagged(&format!("Tagged/Hits/{a} {n}.wav"), &[(K::TrackArtist, a), (K::AlbumArtist, a), (K::AlbumTitle, "Greatest Hits"), (K::TrackTitle, a), (K::TrackNumber, &n)]);
                }
            }
            lib.scan();
            assert_eq!(lib.album_of("Comp/Summer/1.wav"), ("Summer Hits".into(), "Various Artists".into(), true));
            assert_eq!(lib.album_id("Comp/Summer/1.wav"), lib.album_id("Comp/Summer/3.wav"));
            assert_eq!(lib.artist_page("Various Artists"), (vec!["Greatest Hits".to_owned(), "Summer Hits".to_owned()], vec![]));
            assert_eq!(lib.artist_page("Theo Vance"), (vec![], vec!["Summer Hits".to_owned()]), "appears on the compilation");
            assert_eq!(lib.album_id("Mixed/Hits/Copper Monks 1.wav"), lib.album_id("Mixed/Hits/Lunar Sparrows 2.wav"));
            assert_eq!(lib.album_of("Mixed/Hits/Copper Monks 1.wav"), ("Greatest Hits".into(), "Various Artists".into(), true));
            assert_ne!(lib.album_id("Tagged/Hits/Copper Monks 1.wav"), lib.album_id("Tagged/Hits/Lunar Sparrows 1.wav"));
            assert_eq!(lib.album_of("Tagged/Hits/Copper Monks 1.wav").1, "Copper Monks");
            assert_eq!(lib.albums(), 4);
            lib.assert_no_orphans();
        }

        /// A game soundtrack in one folder, ripped a song at a time: no album artist, one
        /// artist per song (a featured credit with no space after "feat."), every track
        /// numbered 1; another with no numbers at all. One album each, a compilation; one
        /// artist's songs of it are that artist's. Loose singles without an album tag stay
        /// apart.
        #[test]
        fn a_soundtrack_without_an_album_artist_is_one_album() {
            let mut lib = Lib::new("gst");
            let songs = [("7mai", "Song A"), ("40mP", "Song B"), ("40mP feat.\u{30b7}\u{30e3}\u{30ce}", "Song C"), ("164", "Song D"), ("1640mP\u{ff08}164\u{d7}40mP\u{ff09}", "Song E")];
            for (i, (a, t)) in songs.iter().enumerate() {
                lib.tagged(&format!("GST/{i}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "O.N.G.E.K.I. GST"), (K::TrackTitle, t), (K::TrackNumber, "1")]);
                lib.tagged(&format!("Unnumbered/{i}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Another GST"), (K::TrackTitle, t)]);
            }
            for (i, a) in ["40mP", "40mP feat.\u{30b7}\u{30e3}\u{30ce}"].iter().enumerate() {
                lib.tagged(&format!("Solo/{i}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Solo"), (K::TrackTitle, a), (K::TrackNumber, "1")]);
            }
            for a in ["Mara Lind", "Theo Vance"] {
                lib.tagged(&format!("Singles/{a}.wav"), &[(K::TrackArtist, a), (K::TrackTitle, a)]);
            }
            lib.scan();
            let gst = lib.album_id("GST/0.wav");
            for i in 1..songs.len() {
                assert_eq!(lib.album_id(&format!("GST/{i}.wav")), gst);
                assert_eq!(lib.album_id(&format!("Unnumbered/{i}.wav")), lib.album_id("Unnumbered/0.wav"));
            }
            assert_eq!(lib.album_of("GST/2.wav"), ("O.N.G.E.K.I. GST".into(), "Various Artists".into(), true));
            assert_eq!(lib.album_of("Unnumbered/2.wav").1, "Various Artists");
            assert_eq!(lib.album_id("Solo/0.wav"), lib.album_id("Solo/1.wav"));
            assert_eq!(lib.album_of("Solo/1.wav"), ("Solo".into(), "40mP".into(), false));
            assert_ne!(lib.album_id("Singles/Mara Lind.wav"), lib.album_id("Singles/Theo Vance.wav"), "no album tag: apart");
            assert_eq!(lib.albums(), 5);
            assert_eq!(lib.artist_page("\u{30b7}\u{30e3}\u{30ce}").1, vec!["Another GST".to_owned(), "O.N.G.E.K.I. GST".into(), "Solo".into()], "featured");
            lib.assert_no_orphans();
        }

        #[test]
        fn several_artists_and_genres_are_each_credited() {
            let mut lib = Lib::new("multi");
            // ID3v2.4: two values, NUL-separated; a featured credit; a title credit.
            lib.tagged("Duets/1.wav", &[(K::TrackArtist, "Mara Lind"), (K::TrackArtist, "Kenji Arai"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Duets"), (K::TrackTitle, "One"), (K::TrackNumber, "1"), (K::Genre, "Folk"), (K::Genre, "Soul")]);
            lib.tagged("Duets/2.wav", &[(K::TrackArtist, "Mara Lind feat. Noor Haddad"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Duets"), (K::TrackTitle, "Two"), (K::TrackNumber, "2"), (K::Genre, "Folk; Jazz")]);
            lib.tagged("Duets/3.wav", &[(K::TrackArtist, "Mara Lind"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Duets"), (K::TrackTitle, "Three (feat. Ruth Ellery)"), (K::TrackNumber, "3"), (K::Genre, "folk")]);
            lib.tagged("Whole/1.wav", &[(K::TrackArtist, "Earth, Wind & Fire"), (K::AlbumTitle, "Whole"), (K::TrackTitle, "x"), (K::TrackNumber, "1")]);
            lib.tagged("Whole/2.wav", &[(K::TrackArtist, "AC/DC"), (K::AlbumTitle, "Whole 2"), (K::TrackTitle, "y"), (K::TrackNumber, "1")]);
            lib.scan();
            for guest in ["Kenji Arai", "Noor Haddad", "Ruth Ellery"] {
                assert_eq!(lib.artist_page(guest), (vec![], vec!["Duets".to_owned()]), "{guest}");
            }
            assert_eq!(lib.artist_page("Mara Lind").0, vec!["Duets"]);
            let display: String = lib.conn.query_row("SELECT artist FROM tracks WHERE path = ?1", [lib.p("Duets/1.wav")], |r| r.get(0)).unwrap();
            assert_eq!(display, "Mara Lind; Kenji Arai", "several values shown joined");
            let genres: Vec<(String, i64)> = crate::query::album_groups(&lib.conn, false).unwrap().into_iter().map(|g| (g.name, g.albums)).collect();
            assert_eq!(genres, vec![("Folk".to_owned(), 1), ("Jazz".into(), 1), ("Soul".into(), 1)], "one Folk whatever its case");
            assert_eq!(crate::query::album_genre_names(&lib.conn, lib.album_id("Duets/1.wav")).unwrap(), vec!["Folk", "Soul", "Jazz"], "the most carried first");
            assert!(crate::query::find_artist(&lib.conn, "Earth, Wind & Fire").unwrap().is_some());
            assert!(crate::query::find_artist(&lib.conn, "AC/DC").unwrap().is_some());
            assert!(crate::query::find_artist(&lib.conn, "Earth").unwrap().is_none());
            lib.assert_no_orphans();
            // A retag that drops a guest drops them from the library.
            lib.tagged("Duets/1.wav", &[(K::TrackArtist, "Mara Lind"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Duets"), (K::TrackTitle, "One"), (K::TrackNumber, "1")]);
            lib.scan();
            assert!(crate::query::find_artist(&lib.conn, "Kenji Arai").unwrap().is_none());
            assert!(!crate::query::album_genre_names(&lib.conn, lib.album_id("Duets/1.wav")).unwrap().contains(&"Soul".to_owned()));
            lib.assert_no_orphans();
        }

        #[test]
        fn album_values_are_the_most_common_whatever_was_read_last() {
            let mut lib = Lib::new("values");
            for t in 1..=5 {
                let n = t.to_string();
                let date = if t <= 3 { "2004" } else { "2004-03-15" };
                let genre = if t == 5 { "Electronic" } else { "Pop" };
                let title = if t == 2 { "dates " } else { "Dates" };
                lib.tagged(&format!("GE/Dates/{n}.wav"), &[(K::TrackArtist, "Golden Engines"), (K::AlbumArtist, "Golden Engines"), (K::AlbumTitle, title), (K::TrackTitle, &n), (K::TrackNumber, &n), (K::RecordingDate, date), (K::Genre, genre)]);
            }
            lib.scan();
            let row = |lib: &Lib| -> (String, String, String) {
                lib.conn.query_row("SELECT title, date, genre FROM albums", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
            };
            assert_eq!(lib.albums(), 1, "a title in another case and spacing is the same album");
            assert_eq!(row(&lib), ("Dates".into(), "2004-03-15".into(), "Pop".into()));
            // The odd track read again (last): the album does not follow it.
            lib.tagged("GE/Dates/5.wav", &[(K::TrackArtist, "Golden Engines"), (K::AlbumArtist, "Golden Engines"), (K::AlbumTitle, "Dates"), (K::TrackTitle, "5"), (K::TrackNumber, "5"), (K::RecordingDate, "2004-03-15"), (K::Genre, "Electronic")]);
            lib.scan();
            assert_eq!(row(&lib), ("Dates".into(), "2004-03-15".into(), "Pop".into()));
        }

        #[test]
        fn untagged_files_take_numbers_and_titles_from_their_names() {
            let mut lib = Lib::new("names");
            for f in ["01 - First Light", "02. Second Wind", "1-03 Third Rail", "Untitled"] {
                lib.tagged(&format!("Rip/{f}.wav"), &[]);
            }
            lib.scan();
            let rows: Vec<(String, Option<i64>, Option<i64>, i64)> = lib
                .conn
                .prepare("SELECT title, disc_no, track_no, from_name FROM tracks ORDER BY path")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(
                rows,
                vec![
                    ("First Light".into(), None, Some(1), 3),
                    ("Second Wind".into(), None, Some(2), 3),
                    ("Third Rail".into(), Some(1), Some(3), 7),
                    ("Untitled".into(), None, None, 1)
                ]
            );
            assert_eq!(lib.albums(), 1);
            assert_eq!(lib.album_of("Rip/Untitled.wav"), ("Rip".into(), "Unknown Artist".into(), false));
        }

        #[test]
        fn discs_in_sibling_folders_are_one_album() {
            let mut lib = Lib::new("discs");
            for d in 1..=2 {
                for t in 1..=2 {
                    let (n, dn) = (t.to_string(), d.to_string());
                    lib.tagged(&format!("AT/Live CD{d}/{n}.wav"), &[(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Live"), (K::TrackTitle, &format!("{d}-{t}")), (K::TrackNumber, &n), (K::DiscNumber, &dn)]);
                    lib.tagged(&format!("AT/Double/Disc {d}/{n}.wav"), &[(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Double"), (K::TrackTitle, &format!("{d}-{t}")), (K::TrackNumber, &n), (K::DiscNumber, &dn)]);
                }
            }
            lib.scan();
            assert_eq!(lib.albums(), 2);
            assert_eq!(lib.album_id("AT/Live CD1/1.wav"), lib.album_id("AT/Live CD2/2.wav"));
            let folder: String = lib.conn.query_row("SELECT folder FROM albums WHERE title = 'Live'", [], |r| r.get(0)).unwrap();
            assert_eq!(folder, lib.p("AT/Live CD1"), "a real folder: the first disc's");
        }

        /// Write the database the previous version's scanner made of this library: schema
        /// 7, one album per (album artist or artist, title, folder), each album's values
        /// from the track scanned last, with the files' sizes and times as scanned.
        /// (album id, album artist, title, folder, its files, added) as the old scanner stored it.
        type OldAlbum<'a> = (i64, &'a str, &'a str, &'a str, &'a [&'a str], i64);

        fn old_state(lib: &mut Lib, albums: &[OldAlbum]) {
            let path = lib.dir.join("library.db");
            let conn = Connection::open(&path).unwrap();
            crate::db::migrate_to(&conn, 7).unwrap();
            let mut tid = 100;
            for (id, artist, title, folder, files, added) in albums {
                let key = format!("t:{}|{}|{}", artist.to_lowercase(), title.to_lowercase(), lib.p(folder).to_lowercase());
                conn.execute(
                    "INSERT INTO albums (id, key, title, album_artist, folder, added_at, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    params![id, key, title, artist, lib.p(folder), added],
                )
                .unwrap();
                for f in *files {
                    let p = lib.p(&format!("{folder}/{f}"));
                    let meta = std::fs::metadata(&p).unwrap();
                    let mtime = meta.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
                    let (t, _) = crate::tags::read(Path::new(&p), false).unwrap();
                    conn.execute(
                        "INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at, codec)
                         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, 1, 'WAV')",
                        params![tid, p, id, t.title, t.artist, t.track_no, t.duration, mtime, meta.len() as i64],
                    )
                    .unwrap();
                    tid += 1;
                }
            }
            drop(conn);
            lib.conn = crate::db::open(&path).unwrap();
        }

        /// A library the previous version scanned (its compilation split in one album per
        /// artist, an album split by a featured credit, with plays, favourites, a rating
        /// and a playlist on them) is migrated, and the next scan reads the tags again in
        /// the background and regroups it: one compilation, one album; every play, rating
        /// and playlist entry stays on its track (same ids), the favourites go with the
        /// albums, the "added" dates are the oldest, nothing is left orphaned.
        #[test]
        fn regrouping_an_old_library_keeps_every_piece_of_user_data() {
            let mut lib = Lib::new("regroup");
            for (f, a) in [("1.wav", "Mara Lind"), ("2.wav", "Theo Vance"), ("3.wav", "Ines Okafor")] {
                let n = &f[..1];
                lib.tagged(&format!("Comp/Summer/{f}"), &[(K::TrackArtist, a), (K::AlbumTitle, "Summer Hits"), (K::TrackTitle, a), (K::TrackNumber, n)]);
            }
            for (f, a) in [("1.wav", "Harbour Lights"), ("2.wav", "Harbour Lights feat. Mara Lind"), ("3.wav", "Harbour Lights")] {
                let n = &f[..1];
                lib.tagged(&format!("HL/Low Tide/{f}"), &[(K::TrackArtist, a), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, &format!("Low {n}")), (K::TrackNumber, n)]);
            }
            old_state(
                &mut lib,
                &[
                    (1, "Mara Lind", "Summer Hits", "Comp/Summer", &["1.wav"], 3000),
                    (2, "Theo Vance", "Summer Hits", "Comp/Summer", &["2.wav"], 2000),
                    (3, "Ines Okafor", "Summer Hits", "Comp/Summer", &["3.wav"], 4000),
                    (4, "Harbour Lights", "Low Tide", "HL/Low Tide", &["1.wav", "3.wav"], 9000),
                    (5, "Harbour Lights feat. Mara Lind", "Low Tide", "HL/Low Tide", &["2.wav"], 1000),
                ],
            );
            let ids_before: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            // The user's data, on the old albums and tracks.
            let comp2 = lib.id("Comp/Summer/2.wav").unwrap();
            let low2 = lib.id("HL/Low Tide/2.wav").unwrap();
            let pl = lib.history(comp2);
            crate::query::record_play(&lib.conn, low2).unwrap();
            crate::query::set_rating(&lib.conn, low2, 5).unwrap();
            lib.conn.execute("INSERT INTO favourite_albums VALUES (2, 50), (5, 70)", []).unwrap();
            let plays = lib.plays_total();
            // Migrated: same albums, nothing regrouped yet.
            assert_eq!(lib.albums(), 5);
            assert_eq!(lib.artist_page("Mara Lind"), (vec!["Summer Hits".to_owned()], vec!["Low Tide".to_owned()]), "as the old pages showed");
            let r = lib.scan_report(Options::default());
            assert_eq!((r.read, r.tags, r.removed, r.moved), (0, 6, 0, 0), "no file changed; the tags read again: {r:?}");
            // One compilation, one album.
            assert_eq!(lib.albums(), 2);
            let comp = lib.album_id("Comp/Summer/1.wav");
            assert_eq!(lib.album_of("Comp/Summer/3.wav"), ("Summer Hits".into(), "Various Artists".into(), true));
            assert_eq!(lib.album_id("Comp/Summer/3.wav"), comp);
            let low = lib.album_id("HL/Low Tide/1.wav");
            assert_eq!(lib.album_id("HL/Low Tide/2.wav"), low);
            assert_eq!(lib.album_of("HL/Low Tide/2.wav").1, "Harbour Lights");
            // Tracks and their history as they were.
            let ids_after: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            assert_eq!(ids_after, ids_before, "every track keeps its row");
            assert!(lib.has_history(comp2, pl));
            assert_eq!(lib.plays_total(), plays);
            assert_eq!(crate::query::rating(&lib.conn, low2).unwrap(), 5);
            // The favourites went with the albums; the dates are the oldest.
            assert!(crate::query::is_favourite_album(&lib.conn, comp).unwrap(), "a favourite part of the compilation");
            assert!(crate::query::is_favourite_album(&lib.conn, low).unwrap(), "the favourite featured split");
            let added = |id: i64| -> i64 { lib.conn.query_row("SELECT added_at FROM albums WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
            assert_eq!(added(comp), 2000);
            assert_eq!(added(low), 1000);
            let fav_at: i64 = lib.conn.query_row("SELECT added_at FROM favourite_albums WHERE album_id = ?1", [low], |r| r.get(0)).unwrap();
            assert_eq!(fav_at, 70);
            // The artist pages after the fill.
            assert_eq!(lib.artist_page("Mara Lind"), (vec![], vec!["Low Tide".to_owned(), "Summer Hits".to_owned()]));
            assert_eq!(lib.artist_page("Harbour Lights").0, vec!["Low Tide"]);
            lib.assert_no_orphans();
            let tags_read: i64 = lib.conn.query_row("SELECT MIN(tags_read) FROM tracks", [], |r| r.get(0)).unwrap();
            assert_eq!(tags_read, TAGS_VERSION);
            // And it stays so.
            let r = lib.scan_report(Options::default());
            assert_eq!((r.read, r.tags, r.albums_changed), (0, 0, 0), "{r:?}");
            assert_eq!((lib.albums(), lib.album_id("Comp/Summer/1.wav")), (2, comp));
        }

        /// A soundtrack the previous scanner split in one album per artist (every track
        /// numbered 1, "feat." with no space a credit of its own) is regrouped by the next
        /// scan: one compilation, every play, rating and favourite kept.
        #[test]
        fn a_split_soundtrack_is_regrouped_keeping_user_data() {
            let mut lib = Lib::new("regroup-gst");
            for (f, a) in [("1.wav", "40mP"), ("2.wav", "40mP feat.\u{30b7}\u{30e3}\u{30ce}"), ("3.wav", "164")] {
                lib.tagged(&format!("GST/{f}"), &[(K::TrackArtist, a), (K::AlbumTitle, "O.N.G.E.K.I. GST"), (K::TrackTitle, f), (K::TrackNumber, "1")]);
            }
            old_state(
                &mut lib,
                &[
                    (1, "40mP", "O.N.G.E.K.I. GST", "GST", &["1.wav"], 3000),
                    (2, "40mP feat.\u{30b7}\u{30e3}\u{30ce}", "O.N.G.E.K.I. GST", "GST", &["2.wav"], 2000),
                    (3, "164", "O.N.G.E.K.I. GST", "GST", &["3.wav"], 4000),
                ],
            );
            // As the previous version left it: its tags read by scanner version 8.
            lib.conn.execute("UPDATE tracks SET tags_read = 8", []).unwrap();
            let ids_before: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            let two = lib.id("GST/2.wav").unwrap();
            let three = lib.id("GST/3.wav").unwrap();
            let pl = lib.history(two);
            crate::query::record_play(&lib.conn, three).unwrap();
            crate::query::set_rating(&lib.conn, three, 4).unwrap();
            lib.conn.execute("INSERT INTO favourite_albums VALUES (3, 60)", []).unwrap();
            let plays = lib.plays_total();
            assert_eq!(lib.albums(), 3);
            let r = lib.scan_report(Options::default());
            assert_eq!((r.read, r.tags), (0, 3), "no file changed; the tags read again: {r:?}");
            assert_eq!(lib.albums(), 1);
            let gst = lib.album_id("GST/1.wav");
            assert_eq!(lib.album_of("GST/2.wav"), ("O.N.G.E.K.I. GST".into(), "Various Artists".into(), true));
            let ids_after: Vec<i64> = lib.conn.prepare("SELECT id FROM tracks ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            assert_eq!(ids_after, ids_before, "every track keeps its row");
            assert!(lib.has_history(two, pl));
            assert_eq!(lib.plays_total(), plays);
            assert_eq!(crate::query::rating(&lib.conn, three).unwrap(), 4);
            assert!(crate::query::is_favourite_album(&lib.conn, gst).unwrap(), "the favourite goes with the album");
            assert_eq!(lib.added("O.N.G.E.K.I. GST"), 2000, "the oldest date");
            assert!(crate::query::find_artist(&lib.conn, "40mP feat.\u{30b7}\u{30e3}\u{30ce}").unwrap().is_none(), "the credit split");
            assert_eq!(lib.artist_page("\u{30b7}\u{30e3}\u{30ce}").1, vec!["O.N.G.E.K.I. GST".to_owned()]);
            lib.assert_no_orphans();
            let r = lib.scan_report(Options::default());
            assert_eq!((r.read, r.tags, r.albums_changed), (0, 0, 0), "{r:?}");
        }

        /// The same regrouping when the files were retagged meanwhile (read in full, not by
        /// the background fill): an album that merges into one that exists passes its
        /// favourite and older date on.
        #[test]
        fn a_retag_that_merges_two_albums_keeps_the_favourite_and_the_older_date() {
            let mut lib = Lib::new("merge");
            lib.tagged("Band/Low/1.wav", &[(K::TrackArtist, "Band"), (K::AlbumArtist, "Band"), (K::AlbumTitle, "Low"), (K::TrackTitle, "a"), (K::TrackNumber, "1")]);
            lib.tagged("Band/Low/2.wav", &[(K::TrackArtist, "Band feat. Guest"), (K::AlbumArtist, "Guest"), (K::AlbumTitle, "Low"), (K::TrackTitle, "b"), (K::TrackNumber, "2")]);
            lib.scan();
            assert_eq!(lib.albums(), 2, "album artists as tagged: two albums");
            let band = lib.album_id("Band/Low/1.wav");
            let guest = lib.album_id("Band/Low/2.wav");
            lib.conn.execute("UPDATE albums SET added_at = CASE WHEN id = ?1 THEN 5000 ELSE 1500 END", [band]).unwrap();
            crate::query::toggle_favourite_album(&lib.conn, guest).unwrap();
            // The album artist tag dropped from the second track: it joins the first's album.
            lib.tagged("Band/Low/2.wav", &[(K::TrackArtist, "Band feat. Guest"), (K::AlbumTitle, "Low"), (K::TrackTitle, "b"), (K::TrackNumber, "2")]);
            lib.scan();
            assert_eq!(lib.albums(), 1);
            assert_eq!(lib.album_id("Band/Low/2.wav"), band, "the album there was");
            assert!(crate::query::is_favourite_album(&lib.conn, band).unwrap());
            assert_eq!(lib.added("Low"), 1500);
            lib.assert_no_orphans();
        }

        /// A MusicBrainz artist id makes an artist; credits of the same name without one
        /// join it (one artist), unless two artists with ids share the name.
        #[test]
        fn musicbrainz_artist_ids_tell_artists_apart() {
            let mut lib = Lib::new("mbid");
            let id1 = "11111111-0000-4000-8000-000000000001";
            let id2 = "22222222-0000-4000-8000-000000000002";
            lib.tagged("A/1.wav", &[(K::TrackArtist, "Nirvana"), (K::MusicBrainzArtistId, id1), (K::AlbumTitle, "Grunge"), (K::TrackTitle, "x"), (K::TrackNumber, "1")]);
            lib.tagged("B/1.wav", &[(K::TrackArtist, "nirvana"), (K::AlbumTitle, "Untagged"), (K::TrackTitle, "y"), (K::TrackNumber, "1")]);
            lib.scan();
            let keys = |lib: &Lib| -> Vec<String> { lib.conn.prepare("SELECT key FROM artists WHERE norm = 'nirvana' ORDER BY key").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
            assert_eq!(keys(&lib), vec![format!("mb:{id1}")], "the name-only credit joins the one with an id");
            lib.tagged("C/1.wav", &[(K::TrackArtist, "Nirvana"), (K::MusicBrainzArtistId, id2), (K::AlbumTitle, "Sixties"), (K::TrackTitle, "z"), (K::TrackNumber, "1")]);
            lib.scan();
            assert_eq!(keys(&lib), vec![format!("mb:{id1}"), format!("mb:{id2}"), "n:nirvana".into()], "two with ids: the one without stays apart");
            lib.assert_no_orphans();
        }

        #[test]
        fn a_moved_folder_keeps_the_date() {
            let mut lib = Lib::new("move");
            lib.file("A/Old/01.wav", "A", "Old", "one", None);
            lib.scan();
            lib.age_all();
            std::fs::rename(lib.music().join("A/Old"), lib.music().join("A/Old (2020)")).unwrap();
            lib.scan();
            assert_eq!(lib.albums(), 1);
            assert_eq!(lib.added("Old"), 1000);
        }

        // ---- batches: a scan stopped and started again ends as one that was not ----------

        use crate::scan::scan_dump;

        /// A small library with every case grouping has: albums by bands, a compilation without
        /// an album artist, two albums of one title in one folder, sibling disc folders with a
        /// folder between them, disc subfolders, one MusicBrainz release in two folders not
        /// named for their discs, featured artists, an album artist tag on some tracks only,
        /// untagged "01 - Title" files, loose files in the root, a cover file.
        fn messy(lib: &Lib) {
            let t = |rel: &str, items: &[(K, &str)]| lib.tagged(rel, items);
            for b in 0..6 {
                let band = format!("Band {b}");
                for a in 0..2 {
                    let album = format!("Album {b}{a}");
                    for n in 1..=3 {
                        let (no, title) = (n.to_string(), format!("{album} {n}"));
                        let genre = if b % 2 == 0 { "Rock" } else { "Jazz; Soul" };
                        t(&format!("{band}/{album}/{n:02}.wav"), &[(K::TrackArtist, &band), (K::AlbumArtist, &band), (K::AlbumTitle, &album), (K::TrackTitle, &title), (K::TrackNumber, &no), (K::Genre, genre)]);
                    }
                }
            }
            for (n, a) in ["Mara Lind", "Theo Vance", "Ines Okafor"].iter().enumerate() {
                let no = (n + 1).to_string();
                t(&format!("Comp/Summer/{no}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Summer Hits"), (K::TrackTitle, a), (K::TrackNumber, &no)]);
            }
            for a in ["Copper Monks", "Lunar Sparrows"] {
                for n in 1..=2 {
                    let no = n.to_string();
                    t(&format!("Mixed/Hits/{a} {no}.wav"), &[(K::TrackArtist, a), (K::AlbumTitle, "Greatest Hits"), (K::TrackTitle, a), (K::TrackNumber, &no)]);
                }
            }
            for d in 1..=2 {
                for n in 1..=2 {
                    let (no, dn, title) = (n.to_string(), d.to_string(), format!("{d}-{n}"));
                    let tags = [(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Live"), (K::TrackTitle, title.as_str()), (K::TrackNumber, &no), (K::DiscNumber, &dn)];
                    t(&format!("AT/Live CD{d}/{no}.wav"), &tags);
                    let tags = [(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Double"), (K::TrackTitle, title.as_str()), (K::TrackNumber, &no), (K::DiscNumber, &dn)];
                    t(&format!("AT/Double/Disc {d}/{no}.wav"), &tags);
                }
            }
            t("AT/Live At Home/1.wav", &[(K::TrackArtist, "Atomic Tides"), (K::AlbumTitle, "Live At Home"), (K::TrackTitle, "Home"), (K::TrackNumber, "1")]);
            for (part, n, date) in [("Part A", "1", "2001"), ("Part A", "2", "2001"), ("Part B", "3", "2001-04-02")] {
                t(&format!("MB/{part}/{n}.wav"), &[(K::TrackArtist, "Glass Owls"), (K::AlbumArtist, "Glass Owls"), (K::AlbumTitle, "Spread"), (K::TrackTitle, n), (K::TrackNumber, n), (K::MusicBrainzReleaseId, "mb-spread"), (K::RecordingDate, date)]);
            }
            t("HL/Low Tide/1.wav", &[(K::TrackArtist, "Harbour Lights"), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, "Low 1"), (K::TrackNumber, "1")]);
            t("HL/Low Tide/2.wav", &[(K::TrackArtist, "Harbour Lights feat. Mara Lind"), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, "Low 2"), (K::TrackNumber, "2")]);
            t("HL/Low Tide/3.wav", &[(K::TrackArtist, "Harbour Lights"), (K::AlbumArtist, "Harbour Lights"), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, "Low 3"), (K::TrackNumber, "3")]);
            for f in ["01 - First Light", "02. Second Wind", "Untitled"] {
                t(&format!("Rip/{f}.wav"), &[]);
            }
            t("loose one.wav", &[(K::TrackArtist, "Solo"), (K::TrackTitle, "Loose")]);
            let img = image::RgbImage::from_fn(48, 48, |x, y| image::Rgb([x as u8 * 5, y as u8 * 5, 120]));
            img.save(lib.music().join("Band 0/Album 00/cover.png")).unwrap();
        }

        /// The user's data on a scanned library: plays, favourites, a rating, a playlist (an
        /// ordinary one and a smart one), the queue.
        fn user_data(lib: &Lib) {
            let id = |rel: &str| lib.id(rel).unwrap_or_else(|| panic!("{rel}"));
            for rel in ["Band 1/Album 10/01.wav", "Comp/Summer/2.wav", "AT/Live CD2/1.wav", "Rip/Untitled.wav"] {
                crate::query::record_play(&lib.conn, id(rel)).unwrap();
            }
            crate::query::toggle_favourite_track(&lib.conn, id("HL/Low Tide/2.wav")).unwrap();
            crate::query::set_rating(&lib.conn, id("Band 2/Album 20/03.wav"), 5).unwrap();
            for rel in ["Band 1/Album 10/01.wav", "HL/Low Tide/2.wav", "Band 3/Album 31/02.wav"] {
                crate::query::toggle_favourite_album(&lib.conn, lib.album_id(rel)).unwrap();
            }
            let pl = crate::query::create_playlist(&lib.conn, "mix").unwrap();
            let ids: Vec<i64> = ["Band 1/Album 10/01.wav", "Band 3/Album 31/01.wav", "Comp/Summer/1.wav", "Band 4/Album 40/02.wav"].iter().map(|r| id(r)).collect();
            crate::query::add_to_playlist(&lib.conn, pl, &ids).unwrap();
            crate::query::create_smart_playlist(&lib.conn, "rated", r#"{"v":1,"match":"all","rules":[{"field":"rating","op":"at_least","value":3}]}"#).unwrap();
            lib.conn.execute_batch(&format!("INSERT INTO queue_items (position, track_id) VALUES (0, {}), (1, {});", ids[1], ids[3])).unwrap();
            // Dates the library had, so a date kept is told from one stamped anew.
            lib.conn.execute("UPDATE albums SET added_at = 1000 + id, first_seen = 1000 + id, added_source = NULL", []).unwrap();
            lib.conn.execute("UPDATE tracks SET added_at = 100 + id", []).unwrap();
        }

        /// The library as it stands, as a file to start each run from.
        fn snapshot(lib: &Lib, name: &str) -> PathBuf {
            let p = lib.dir.join(format!("{name}.db"));
            let _ = std::fs::remove_file(&p);
            lib.conn.execute_batch(&format!("VACUUM INTO '{}'", p.to_string_lossy().replace('\'', "''"))).unwrap();
            p
        }

        /// What a scan to the end makes of `snap` and the files as they are now: in one batch
        /// (the reference, as every scan used to be), or `batch` files at a time, stopped after
        /// every `stop_every` batches and started again until it finishes. The dump, how many
        /// times each file was read, the runs it took, and whether the last said it resumed.
        #[allow(clippy::type_complexity)]
        fn run(lib: &Lib, snap: &Path, name: &str, batch: usize, stop_every: Option<usize>) -> (Vec<(&'static str, String)>, HashMap<PathBuf, usize>, usize, bool) {
            let path = lib.dir.join(format!("{name}.db"));
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
            }
            std::fs::copy(snap, &path).unwrap();
            let mut conn = crate::db::open(&path).unwrap();
            let ids_up_to: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tracks", [], |r| r.get(0)).unwrap();
            let since = crate::db::now_ms();
            scan_dump_reads_clear(lib);
            let mut runs = 0;
            let mut resumed = false;
            let mut covers_only = false;
            let mut all_reads: HashMap<PathBuf, usize> = HashMap::new();
            loop {
                runs += 1;
                assert!(runs < 400, "{name}: the scan never finishes");
                let cancel = Arc::new(AtomicBool::new(false));
                let c = cancel.clone();
                let progress = move |p: Progress| {
                    if stop_every.is_some_and(|k| p.batches >= k) {
                        c.store(true, Ordering::Relaxed);
                    }
                };
                let opts = Options { cancel: Some(cancel), batch_files: Some(batch), ..Default::default() };
                let r = scan_with(&mut conn, &lib.art, &[lib.music()], opts, &progress).unwrap();
                let reads = super::reads_under(&lib.music());
                for (p, n) in &reads {
                    *all_reads.entry(p.clone()).or_default() += n;
                }
                // A run after one stopped in its covers only looks at those covers: it resumes
                // nothing (the files were all read), so it says nothing of resuming.
                if !covers_only {
                    resumed = r.resumed;
                }
                covers_only = r.covers_cut && !r.interrupted;
                if !r.interrupted && !r.covers_cut {
                    let state: i64 = conn.query_row("SELECT COUNT(*) FROM scan_state", [], |r| r.get(0)).unwrap();
                    assert_eq!(state, 0, "{name}: a scan that finished leaves nothing to resume");
                    break;
                }
            }
            (scan_dump::dump(&conn, since, ids_up_to), all_reads, runs, resumed)
        }

        fn scan_dump_reads_clear(lib: &Lib) {
            super::reads_under(&lib.music());
        }

        /// Every file is read exactly once, whatever the stops.
        fn read_once(reads: &HashMap<PathBuf, usize>, files: usize, case: &str) {
            let twice: Vec<_> = reads.iter().filter(|(_, n)| **n > 1).collect();
            assert!(twice.is_empty(), "{case}: read again: {twice:?}");
            assert_eq!(reads.len(), files, "{case}: every file read");
        }

        fn audio_files(dir: &Path) -> usize {
            let mut n = 0;
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    n += audio_files(&p);
                } else if is_audio(&p) {
                    n += 1;
                }
            }
            n
        }

        /// A first scan, stopped after every batch (and after every second one), for several batch
        /// sizes: in the end the same library as the one scanned in one go, row for row, albums
        /// dated by their files alike, every file read once; and the runs after the first say they
        /// resume.
        #[test]
        fn a_first_scan_stopped_after_every_batch_ends_as_one_that_was_not() {
            let lib = Lib::new("batches-first");
            messy(&lib);
            let files = audio_files(&lib.music());
            let empty = snapshot(&lib, "empty");
            let (want, reads, runs, _) = run(&lib, &empty, "reference", usize::MAX, None);
            assert_eq!(runs, 1);
            read_once(&reads, files, "reference");
            assert!(want.iter().any(|(w, rows)| *w == "albums" && rows.contains("Various Artists")), "the compilation is one");
            for (batch, every) in [(1, Some(1)), (3, Some(1)), (4, Some(2)), (7, Some(1)), (2, None)] {
                let case = format!("batches of {batch}, stopped every {every:?}");
                let (got, reads, runs, resumed) = run(&lib, &empty, &format!("b{batch}"), batch, every);
                scan_dump::assert_same(&want, &got, &case);
                read_once(&reads, files, &case);
                if every.is_some() {
                    assert!(runs > 3 && resumed, "{case}: {runs} runs, resumed {resumed}");
                }
            }
        }

        /// The same for a rescan of a library with the user's data on it, after a reorganisation:
        /// folders moved and renamed (one only in letter case), a file renamed, retags that regroup
        /// albums (an album artist dropped, a MusicBrainz id added), files and a whole album
        /// removed, an album moved and re-encoded (no file matches: it takes over the date by its
        /// name), two copies of one file (which match nothing), a new album. Every play, favourite,
        /// rating, playlist entry and the queue end on the same tracks as in one go; the tracks
        /// that moved keep their ids.
        #[test]
        fn a_rescan_with_moves_retags_and_removals_stopped_after_every_batch_ends_as_one() {
            let mut lib = Lib::new("batches-rescan");
            messy(&lib);
            lib.scan();
            user_data(&lib);
            let base = snapshot(&lib, "base");
            // The reorganisation.
            lib.mv("Band 1/Album 10", "Moved/Band 1 - Album 10");
            lib.mv("Band 3/Album 31/02.wav", "Band 3/Album 31/02 - Renamed.wav");
            lib.mv("Band 4", "band 4 tmp");
            lib.mv("band 4 tmp", "BAND 4");
            for n in 1..=3 {
                let (no, title) = (n.to_string(), format!("Album 20 {n}"));
                lib.tagged(&format!("Band 2/Album 20/{n:02}.wav"), &[(K::TrackArtist, "Band 2"), (K::AlbumArtist, "Band 2"), (K::AlbumTitle, "Album 20"), (K::TrackTitle, &title), (K::TrackNumber, &no), (K::MusicBrainzReleaseId, "mb-twenty")]);
            }
            lib.tagged("HL/Low Tide/3.wav", &[(K::TrackArtist, "Harbour Lights"), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, "Low 3"), (K::TrackNumber, "3")]);
            std::fs::remove_file(lib.p("Band 5/Album 50/02.wav")).unwrap();
            std::fs::remove_dir_all(lib.p("Band 5/Album 51")).unwrap();
            // Re-encoded (other durations) and moved: nothing matches one by one.
            for n in 1..=3 {
                std::fs::remove_file(lib.p(&format!("Band 0/Album 01/{n:02}.wav"))).unwrap();
                let (no, title) = (n.to_string(), format!("Album 01 {n}"));
                lib.tagged(&format!("Band 0/Album 01 (FLAC)/track {n:02} re-encoded.wav"), &[(K::TrackArtist, "Band 0"), (K::AlbumArtist, "Band 0"), (K::AlbumTitle, "Album 01"), (K::TrackTitle, &title), (K::TrackNumber, &no)]);
            }
            lib.mv("Comp/Summer/1.wav", "Copies/X/1.wav");
            std::fs::create_dir_all(lib.p("Copies/Y")).unwrap();
            std::fs::copy(lib.p("Copies/X/1.wav"), lib.p("Copies/Y/1.wav")).unwrap();
            for n in 1..=2 {
                let no = n.to_string();
                lib.tagged(&format!("New/Fresh/{no}.wav"), &[(K::TrackArtist, "New Band"), (K::AlbumArtist, "New Band"), (K::AlbumTitle, "Fresh"), (K::TrackTitle, &no), (K::TrackNumber, &no)]);
            }
            let (want, _, _, _) = run(&lib, &base, "reference", usize::MAX, None);
            let moved = want.iter().find(|(w, _)| *w == "track ids kept").unwrap();
            assert!(moved.1.contains("Moved") && moved.1.contains("02 - Renamed.wav"), "the moves kept their rows");
            let favs = &want.iter().find(|(w, _)| *w == "favourite albums").unwrap().1;
            assert_eq!(favs.lines().count(), 3, "every favourite album kept: {favs}");
            for (batch, every) in [(1, Some(1)), (2, Some(1)), (3, Some(2)), (5, None)] {
                let case = format!("rescan, batches of {batch}, stopped every {every:?}");
                let (got, reads, runs, _) = run(&lib, &base, &format!("r{batch}"), batch, every);
                scan_dump::assert_same(&want, &got, &case);
                // Files that could be a moved track, and the folders tracks left, wait for the
                // end; every other file is read once.
                let twice: Vec<_> = reads.iter().filter(|(p, n)| **n > 1 && !could_wait(&lib, p)).collect();
                assert!(twice.is_empty(), "{case}: read again: {twice:?}");
                assert!(every.is_none() || runs >= 2, "{case}: {runs} runs");
            }
        }

        /// The files that a rescan holds back to the end (their folder lost a track, or they
        /// may be one moved): read again by each run that stops before the end.
        fn could_wait(lib: &Lib, p: &Path) -> bool {
            let rel = p.strip_prefix(lib.music()).unwrap().to_string_lossy().replace('\\', "/");
            ["Moved/", "Band 3/Album 31/", "BAND 4/", "Copies/", "Band 0/Album 01", "Band 5/Album 50/", "Comp/Summer/"].iter().any(|w| rel.starts_with(w))
        }

        /// A first import stopped half-way keeps what it wrote (and says it is to be resumed),
        /// and dates its albums by their files: the next run resumes, reads only the rest, and
        /// ends with every album dated by its files, none removed.
        #[test]
        fn a_stopped_first_import_keeps_its_batches_and_is_resumed() {
            let mut lib = Lib::new("batches-resume");
            messy(&lib);
            let cancel = Arc::new(AtomicBool::new(false));
            let c = cancel.clone();
            let progress = move |p: Progress| {
                if p.batches >= 3 {
                    c.store(true, Ordering::Relaxed);
                }
            };
            let roots = vec![lib.music()];
            let r = scan_with(&mut lib.conn, &lib.art, &roots, Options { cancel: Some(cancel), batch_files: Some(5), ..Default::default() }, &progress).unwrap();
            assert!(r.interrupted && !r.resumed && r.batches == 3, "{r:?}");
            let first = lib.tracks();
            assert!(first >= 15 && (first as usize) < audio_files(&lib.music()), "{first} tracks in after three batches");
            let (when, first_import): (i64, bool) = lib.conn.query_row("SELECT started_at, first_import FROM scan_state", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert!(first_import && when > 1_700_000_000_000);
            super::reads_under(&lib.music());
            let r = scan_with(&mut lib.conn, &lib.art, &roots, Options::default(), &|_| {}).unwrap();
            assert!(r.resumed && !r.interrupted && r.removed == 0, "{r:?}");
            assert_eq!(r.read, audio_files(&lib.music()) - first as usize, "only the rest is read");
            assert!(super::reads_under(&lib.music()).values().all(|n| *n == 1));
            let undated: i64 = lib.conn.query_row("SELECT COUNT(*) FROM albums WHERE added_source IS NOT 'files'", [], |r| r.get(0)).unwrap();
            assert_eq!(undated, 0, "a first import's albums are dated by their files");
            // Dated by the files: the earliest of them, not the scan.
            let (added, born): (i64, i64) = lib
                .conn
                .query_row("SELECT a.added_at, a.first_seen FROM albums a WHERE a.title = 'Album 00'", [], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap();
            let file = std::fs::metadata(lib.p("Band 0/Album 00/01.wav")).unwrap();
            let created = file.created().or_else(|_| file.modified()).unwrap().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
            assert!(added <= created && added <= born, "{added} {created} {born}");
            assert_eq!(lib.conn.query_row("SELECT COUNT(*) FROM scan_state", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            // Later scans stamp new albums as before: first seen now, not by their files.
            lib.age_all();
            lib.tagged("Later/New/1.wav", &[(K::TrackArtist, "Later"), (K::AlbumTitle, "New"), (K::TrackTitle, "x")]);
            lib.scan();
            let (added, source): (i64, Option<String>) = lib.conn.query_row("SELECT added_at, added_source FROM albums WHERE title = 'New'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert!(added > crate::db::now_ms() - 60_000 && source.is_none());
            assert_eq!(lib.added("Album 00"), 1000, "and the import's dates stay");
        }

        /// A stopped scan removes nothing: the guard and the prune look at a whole scan only.
        #[test]
        fn a_stopped_scan_removes_nothing() {
            let mut lib = Lib::new("batches-noprune");
            messy(&lib);
            lib.scan();
            let before = lib.tracks();
            std::fs::remove_dir_all(lib.p("Band 5")).unwrap();
            for n in 0..4 {
                lib.tagged(&format!("Added/A{n}/1.wav"), &[(K::TrackArtist, "Added"), (K::AlbumTitle, &format!("A{n}")), (K::TrackTitle, "x")]);
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let c = cancel.clone();
            let roots = vec![lib.music()];
            let r = scan_with(&mut lib.conn, &lib.art, &roots, Options { cancel: Some(cancel), batch_files: Some(1), ..Default::default() }, &move |p: Progress| {
                if p.batches >= 1 {
                    c.store(true, Ordering::Relaxed);
                }
            })
            .unwrap();
            assert!(r.interrupted && r.removed == 0, "{r:?}");
            assert_eq!(lib.tracks(), before + 1, "one new album written, nothing removed");
            let r = lib.scan_report(Options::default());
            assert!(r.resumed && r.removed == 6, "{r:?}");
        }

        /// On a slow disk the library fills in while it is read: the first batches are written
        /// (and told) long before the last file is read, and the progress names what it waits on.
        #[test]
        fn on_a_slow_disk_albums_come_in_while_the_rest_is_read() {
            let mut lib = Lib::new("batches-slow");
            messy(&lib);
            let files = audio_files(&lib.music());
            let seen: Mutex<Vec<Progress>> = Mutex::new(Vec::new());
            let progress = |p: Progress| seen.lock().unwrap().push(p);
            let opts = Options { slow_disk: Some(Duration::from_millis(15)), ..Default::default() };
            let t0 = Instant::now();
            let roots = vec![lib.music()];
            let r = scan_with(&mut lib.conn, &lib.art, &roots, opts, &progress).unwrap();
            assert_eq!(r.read, files);
            assert!(t0.elapsed() >= Duration::from_millis(15 * files as u64), "the disk was slow");
            let seen = seen.into_inner().unwrap();
            let early = seen.iter().find(|p| p.batches >= 1).expect("a batch was told");
            assert!(early.phase == Phase::Reading && early.done < files / 2 && early.albums >= 1, "{early:?}");
            assert!(r.batches >= 3, "{r:?}");
            assert!(seen.iter().any(|p| p.looking && p.phase == Phase::Reading), "read while the walk went on");
            let last = seen.iter().rev().find(|p| p.phase == Phase::Reading).unwrap();
            assert_eq!((last.done, last.total), (files, files), "{last:?}");
        }
    }

    #[test]
    fn album_key_prefers_musicbrainz() {
        let mut t = TrackTags { album: Some("X".into()), artist: Some("Y".into()), ..Default::default() };
        t.artists = vec![Credit::new("Y", false)];
        let k1 = album_key(&t, Path::new("M:/a/01.flac"));
        assert_eq!(k1, "t:y|x|m:/a");
        t.mb_album_id = Some("ABC".into());
        assert_eq!(album_key(&t, Path::new("M:/a/01.flac")), "mb:abc");
    }
}
