//! A scan, written as it goes: whole album folders read in parallel and written a batch at a
//! time, each batch in a transaction of its own, so the library fills in while it is read and a
//! quit, a crash or a drive gone loses at most what was not yet written (the next scan reads only
//! that again: the files written are found unchanged by their time and size).
//!
//! How it runs: the walk (`walk.rs`) hands over units, every file of one album folder at a time;
//! a pool of readers reads their tags, in walk order and only so far ahead of the writing
//! (`READ_AHEAD`); this thread takes the units whose files are all read, in order, and writes
//! them together (`BATCH_FILES`, or every `BATCH_EVERY`), then hands the albums each batch made
//! or changed to the same pool for their covers, which are written as they come.
//!
//! What a batch boundary must not change, and why it does not:
//! - Grouping: an album is decided with all the tracks of its folder (`group.rs`), and a unit is
//!   all the files to read of one group folder, so a batch never holds part of a folder. An album
//!   that spans folders (one MusicBrainz release in folders not named for their discs) keys by
//!   the release in every batch, so its identity does not depend on where the batches fall; its
//!   values are worked out again, from all its tracks, whenever a batch adds to it.
//! - Moves and renames: a file that vanished and a new file that is it under another path must
//!   keep one row (`match_moves`), which can only be decided once every new file is read. So when
//!   files vanished, a unit is held back, and written at the end with the removals in one
//!   transaction, as every scan used to be, if it has a new file that could be one of them (the
//!   same title, numbers and duration: `could_be`), or if it is the folder of a vanished track
//!   (it is grouped with them until they are moved or removed). Everything else is written as it
//!   comes: a new file that fits no vanished track is a new track whatever else is read. A first
//!   scan has nothing that vanished, and holds nothing back. A partial scan (the folder watcher)
//!   with something gone under its folders holds back everything: it may hand over to a full scan
//!   before writing anything, as before.
//! - Dates: see `SyncCtx::dated`, and docs/features/library.md, "Recently added".
//! - The missing-tracks guard and the prune look at the whole scan: they run at the end, with the
//!   held units, and only when the scan was not stopped.
//!
//! A track is removed only when its file is known to be gone. One under a folder or entry the walk
//! could not read (`Found::Unreadable`) was not seen, not found missing: it is held, named in
//! `Report::held` with the error (`Held::unreadable`), and stays until the folder reads again; and
//! before any track is removed its path is looked at once more (`try_exists`), and only a plain
//! "not there" removes it (an error holds it the same way); then its root, which must still be a
//! folder that lists (`still_there`: a drive gone mid-scan holds them all). `Options::confirm_removal` overrides the
//! mass-removal guard, not this.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};

use super::walk::{Disk, Found, Unit, Walk};
use super::*;

/// Files read ahead of the writing at most (and at least one unit): what a scan holds in memory
/// besides the walk's list of files, however large the library.
const READ_AHEAD: usize = 2048;
/// A batch is written when this many files are ready...
const BATCH_FILES: usize = 512;
/// ...or this long after the last one, with whatever is ready (the first as soon as one album is).
const BATCH_EVERY: Duration = Duration::from_millis(1000);
/// While the walk is still going in a first scan (read as it is found), only this many files are
/// read at once, so the walk, which says how much there is, is not starved on a slow disk.
const READERS_WHILE_WALKING: usize = 2;
/// How often progress is told while nothing else happens (the disk may be slow, or stalled).
const TELL_EVERY: Duration = Duration::from_millis(250);
/// A file being read (or a folder listed) this long is named in the progress.
const SLOW: Duration = Duration::from_secs(2);
/// How often the names of the artists and genres the batches touched are worked out again
/// (`Writer::settle`), and how many in one transaction.
const SETTLE_EVERY: Duration = Duration::from_secs(3);
const SETTLE_CHUNK: usize = 8;
/// How often the scan moves the WAL into the database (`Writer::write`).
const CHECKPOINT_EVERY: Duration = Duration::from_millis(750);

/// A known track, as the scan starts.
pub(super) struct Known {
    pub id: i64,
    pub mtime: i64,
    pub size: i64,
    pub album: i64,
    /// Its group folder (`track_tags.group_folder`).
    pub folder: Option<String>,
}

struct Slot {
    files: Vec<FileInfo>,
    /// How many files (`files` is emptied when the unit is taken).
    count: usize,
    results: Vec<Option<Result<TrackTags, String>>>,
    left: usize,
    /// Held back from the start (a partial scan with something gone; a vanished track's folder).
    hold: bool,
    /// Written, or held to the end: no longer counted against the read-ahead.
    out: bool,
}

/// A unit whose files are all read, out of the pipe.
pub(super) struct Taken {
    /// Its place in the pipe.
    index: usize,
    files: Vec<FileInfo>,
    results: Vec<Result<TrackTags, String>>,
    hold: bool,
}

struct ArtJob {
    album: i64,
    folder: PathBuf,
    track: PathBuf,
    stamp: String,
}

#[derive(Default)]
struct Pipe {
    units: Vec<Slot>,
    /// Units taken out (read, then written or held).
    taken: usize,
    /// Units written or held: what the reading may run ahead of.
    written: usize,
    /// The next file to read.
    next: (usize, usize),
    /// Units `..window` may be read.
    window: usize,
    readers: usize,
    reading: usize,
    stop: bool,
    over: bool,
    /// The walk: its units not yet adopted, files found, the folder being listed.
    walked: Vec<Unit>,
    found: usize,
    /// What the walk could not read, and why.
    unreadable: Vec<(PathBuf, String)>,
    listing: Option<(PathBuf, Instant)>,
    walk_done: bool,
    /// What each reader is reading, since when.
    busy: HashMap<usize, (PathBuf, Instant)>,
    read: usize,
    last_read: Option<Instant>,
    art: VecDeque<ArtJob>,
    art_busy: usize,
    arts: Vec<(i64, Option<String>, String)>,
    art_errors: Vec<(PathBuf, String)>,
    art_total: usize,
    art_done: usize,
    /// Bumped by every piece of news for the writing thread, so it never waits through one.
    events: u64,
}

impl Pipe {
    fn take_next(&mut self) -> Option<(usize, usize)> {
        while self.next.0 < self.window.min(self.units.len()) {
            let (u, f) = self.next;
            if f < self.units[u].files.len() {
                self.next.1 += 1;
                return Some((u, f));
            }
            self.next = (u + 1, 0);
        }
        None
    }

    /// Units `ids` are out of the pipe for good (written, or held to the end).
    fn out(&mut self, ids: impl IntoIterator<Item = usize>) {
        for i in ids {
            self.units[i].out = true;
        }
    }

    /// Let the reading run `ahead` files past what is written or held (at least one unit): the
    /// files of the units still in the pipe, read or not, count; a ready unit may wait in front
    /// of held ones.
    fn open_window(&mut self, ahead: usize) {
        while self.written < self.units.len() && self.units[self.written].out {
            self.written += 1;
        }
        let mut n = 0;
        let mut w = self.written;
        while w < self.units.len() && (w == self.written || n < ahead) {
            if !self.units[w].out {
                n += self.units[w].count;
            }
            w += 1;
        }
        self.window = self.window.max(w);
    }
}

fn lock(m: &Mutex<Pipe>) -> MutexGuard<'_, Pipe> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn reader(pipe: &Mutex<Pipe>, wake: &Condvar, disk: &Disk, art: &ArtCache, me: usize, art_cap: usize) {
    crate::background_thread();
    enum Job {
        Read(usize, usize, PathBuf),
        Art(ArtJob),
    }
    loop {
        let job = {
            let mut p = lock(pipe);
            loop {
                if p.over {
                    return;
                }
                if p.art_busy < art_cap
                    && let Some(j) = p.art.pop_front()
                {
                    p.art_busy += 1;
                    break Job::Art(j);
                }
                if !p.stop
                    && p.reading < p.readers
                    && let Some((u, f)) = p.take_next()
                {
                    p.reading += 1;
                    p.last_read.get_or_insert_with(Instant::now);
                    let path = p.units[u].files[f].path.clone();
                    p.busy.insert(me, (path.clone(), Instant::now()));
                    break Job::Read(u, f, path);
                }
                p = wake.wait(p).unwrap_or_else(|e| e.into_inner());
            }
        };
        match job {
            Job::Read(u, f, path) => {
                // A panic in the tag reader (lofty on a malformed file) is this file's error,
                // not the end of the scan.
                let r = guarded(|| {
                    disk.wait();
                    #[cfg(test)]
                    super::tests::reading(&path);
                    tags::read(&path, false).map(|(t, _)| t)
                });
                let mut p = lock(pipe);
                let slot = &mut p.units[u];
                slot.results[f] = Some(r);
                slot.left -= 1;
                p.reading -= 1;
                p.read += 1;
                p.last_read = Some(Instant::now());
                p.busy.remove(&me);
                p.events += 1;
                drop(p);
                wake.notify_all();
            }
            Job::Art(j) => {
                // A panic decoding one cover (the image decoder) leaves that album without art
                // and is reported; the scan goes on.
                let stem = guarded(|| Ok(album_art_on(art, &j.folder, &j.track, disk)));
                let mut p = lock(pipe);
                match stem {
                    Ok(stem) => p.arts.push((j.album, stem, j.stamp)),
                    Err(e) => {
                        p.art_errors.push((j.track.clone(), e));
                        p.arts.push((j.album, None, j.stamp));
                    }
                }
                p.art_busy -= 1;
                p.art_done += 1;
                p.events += 1;
                drop(p);
                wake.notify_all();
            }
        }
    }
}

/// The modification-time fingerprints of the albums art is asked for, and their jobs.
fn art_jobs(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<ArtJob>> {
    let mut st = conn.prepare_cached(&format!("SELECT a.folder, {FIRST_TRACK} FROM albums a WHERE a.id = ?1"))?;
    let mut jobs = Vec::new();
    for &id in ids {
        if let Some((folder, Some(track))) = st.query_row([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))).optional()? {
            let (folder, track) = (PathBuf::from(folder), PathBuf::from(track));
            let stamp = art_fingerprint(&folder, &track);
            jobs.push(ArtJob { album: id, folder, track, stamp });
        }
    }
    Ok(jobs)
}

/// One scan, start to end (see the module doc). An `Options::only` scan that found something
/// gone for good answers `None`, for the caller to scan everything instead.
pub(super) fn run(
    conn: &mut Connection,
    art: &ArtCache,
    roots: &[PathBuf],
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
) -> rusqlite::Result<Option<Report>> {
    let started = Instant::now();
    // Checkpoints after each batch instead (`Writer::write`); back to the default at the end.
    let _ = conn.execute_batch("PRAGMA wal_autocheckpoint = 0;");
    let r = run_inner(conn, art, roots, opts, progress, started);
    let _ = conn.execute_batch("PRAGMA wal_autocheckpoint = 1000;");
    r
}

#[allow(clippy::too_many_lines)]
fn run_inner(
    conn: &mut Connection,
    art: &ArtCache,
    roots: &[PathBuf],
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    started: Instant,
) -> rusqlite::Result<Option<Report>> {
    let mut report = Report::default();
    let partial = opts.only.is_some();
    report.partial = partial;
    let disk = Disk::new(opts.slow_disk);
    let never = AtomicBool::new(false);
    // Set when the scan's writing ends (a write failed, say): the walk stops too, not kept on
    // to the end of a large library (on a slow share, minutes) before the error is told.
    let halt = AtomicBool::new(false);
    let cancel: &AtomicBool = opts.cancel.as_deref().unwrap_or(&never);

    #[cfg(any(windows, target_os = "macos"))]
    if let Some(dirs) = opts.only.as_deref()
        && spelled_otherwise(conn, dirs)?
    {
        log::info!("scan: a folder to scan is spelled otherwise in the library (renamed in letter case?); scanning everything");
        return Ok(None);
    }
    // What the database already knows (under the scanned folders).
    let known = load_known(conn, opts.only.as_deref())?;
    timing("known loaded", started);
    let state: Option<bool> = if partial {
        None
    } else {
        conn.query_row("SELECT first_import FROM scan_state WHERE id = 1", [], |r| r.get::<_, bool>(0)).optional()?
    };
    report.resumed = state.is_some();
    // A first import: an empty library (or the rest of one that was stopped). New albums are dated
    // by their files then (`SyncCtx::dated`).
    let first_import = state.unwrap_or(!partial && known.is_empty());
    // Nothing is known: nothing can have vanished, so units are read as the walk finds them.
    let streaming = !partial && known.is_empty();
    let live_roots: Vec<PathBuf> = roots.iter().filter(|r| r.is_dir()).cloned().collect();
    if report.resumed {
        log::info!("scan: resuming a scan that did not finish");
    }

    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let ahead = opts.batch_files.unwrap_or(READ_AHEAD);
    let pipe = Mutex::new(Pipe { readers: if streaming { READERS_WHILE_WALKING } else { workers }, ..Default::default() });
    let wake = Condvar::new();
    let tell_walk = |f: Found| {
        let mut p = lock(&pipe);
        match f {
            // Only for the progress, which looks a few times a second: nobody is woken for it.
            Found::Listing(dir) => {
                p.listing = Some((dir, Instant::now()));
                return;
            }
            Found::Files(n) => {
                p.found += n;
                return;
            }
            Found::Unreadable(path, e) => {
                p.unreadable.push((path, e));
                return;
            }
            // Units wait for the end of the walk unless they are read as they come.
            Found::Unit(u) if !streaming => {
                p.walked.push(u);
                return;
            }
            Found::Unit(u) => p.walked.push(u),
            Found::Done => {
                p.walk_done = true;
                p.listing = None;
                p.readers = workers;
            }
        }
        p.events += 1;
        drop(p);
        wake.notify_all();
    };

    let mut w = Writer {
        conn,
        known: &known,
        first_import,
        full: !partial,
        report: &mut report,
        progress,
        state_written: state.is_some(),
        touched: HashSet::new(),
        created: HashSet::new(),
        changes: 0,
        albums: 0,
        last_tell: None,
        last_arts: None,
        phase: Phase::Walking,
        unsettled: Default::default(),
        last_settle: None,
        last_checkpoint: None,
    };
    // What the walk found: every file (path -> mtime, size), for the prune and the fills.
    let mut walked: HashMap<String, (i64, i64)> = HashMap::new();
    let mut handover = false;
    let result = std::thread::scope(|s| -> rusqlite::Result<()> {
        let walk_roots: &[PathBuf] = opts.only.as_deref().unwrap_or(roots);
        s.spawn(|| {
            crate::background_thread();
            Walk::new(&disk, cancel, &halt, &tell_walk).run(walk_roots);
        });
        let art_cap = workers.div_ceil(2);
        for me in 0..workers {
            let (pipe, wake, disk) = (&pipe, &wake, &disk);
            s.spawn(move || reader(pipe, wake, disk, art, me, art_cap));
        }
        let shut = || {
            halt.store(true, Ordering::Relaxed);
            lock(&pipe).over = true;
            wake.notify_all();
        };
        let r = (|| -> rusqlite::Result<()> {
            // The vanished tracks (`prepared` once the walk is over, when not streaming).
            let mut moves = Moves::default();
            let mut prepared = streaming;
            let mut ready: Vec<Taken> = Vec::new();
            let mut held: Vec<Taken> = Vec::new();
            let mut last_write: Option<Instant> = None;
            loop {
                // Units the walk finished: into the pipe (at once when nothing is known; otherwise
                // once the walk is over and the vanished tracks are known).
                let (adopt, walk_done, seen) = {
                    let mut p = lock(&pipe);
                    let done = p.walk_done;
                    let seen = p.events;
                    if streaming || done { (std::mem::take(&mut p.walked), done, seen) } else { (Vec::new(), done, seen) }
                };
                let mut slots = Vec::new();
                for u in adopt {
                    let mut files = Vec::new();
                    for f in u.files {
                        let path = f.path.to_string_lossy().into_owned();
                        // Walked twice (roots are `outermost`, so not as a rule): read once.
                        if walked.insert(path.clone(), (f.mtime, f.size)).is_some() {
                            continue;
                        }
                        match known.get(&path) {
                            Some(k) if k.mtime == f.mtime && k.size == f.size => {}
                            _ => files.push(f),
                        }
                    }
                    if !files.is_empty() {
                        slots.push((u.key, files));
                    }
                }
                if !prepared && walk_done {
                    prepared = true;
                    timing("walk done", started);
                    let unreadable = lock(&pipe).unreadable.clone();
                    moves.prepare(&known, &walked, &live_roots, partial, &unreadable);
                    w.report.files = walked.len();
                }
                if !slots.is_empty() {
                    let mut p = lock(&pipe);
                    for (key, files) in slots {
                        let hold = moves.hold_all || moves.folders.contains(&key);
                        let n = files.len();
                        p.units.push(Slot { results: (0..n).map(|_| None).collect(), files, count: n, left: n, hold, out: false });
                    }
                    p.open_window(ahead);
                    drop(p);
                    wake.notify_all();
                }
                // The units read through, in order.
                let taken: Vec<Taken> = {
                    let mut p = lock(&pipe);
                    let mut out = Vec::new();
                    while p.taken < p.units.len() && p.units[p.taken].left == 0 {
                        let i = p.taken;
                        let slot = &mut p.units[i];
                        out.push(Taken {
                            index: i,
                            files: std::mem::take(&mut slot.files),
                            results: std::mem::take(&mut slot.results).into_iter().map(|r| r.unwrap_or_else(|| Err("not read".into()))).collect(),
                            hold: slot.hold,
                        });
                        p.taken += 1;
                    }
                    out
                };
                let mut busy = !taken.is_empty();
                let mut gone_out: Vec<usize> = Vec::new();
                for t in taken {
                    w.report.read += t.files.len();
                    if t.hold || moves.could_be_moved(w.conn, &t, &known, &walked, &live_roots)? {
                        gone_out.push(t.index);
                        held.push(t);
                    } else {
                        ready.push(t);
                    }
                }
                // Covers that came back.
                w.write_arts(&pipe, false, art, &HashMap::new())?;
                let all_read = {
                    let p = lock(&pipe);
                    prepared && p.walk_done && p.walked.is_empty() && p.taken == p.units.len()
                };
                let ready_files: usize = ready.iter().map(|t| t.files.len()).sum();
                let stopping = cancel.load(Ordering::Relaxed);
                let due = !ready.is_empty()
                    && (stopping
                        || all_read
                        || match opts.batch_files {
                            Some(n) => ready_files >= n,
                            None => last_write.is_none_or(|t| ready_files >= BATCH_FILES || t.elapsed() >= BATCH_EVERY),
                        });
                if due {
                    // At most `BATCH_FILES` (and whole units) in one transaction, so the database is
                    // held only briefly; the rest goes in the next, at once.
                    let cap = opts.batch_files.unwrap_or(BATCH_FILES);
                    let (mut n, mut take) = (0, 0);
                    while take < ready.len() && (take == 0 || n < cap) {
                        n += ready[take].files.len();
                        take += 1;
                    }
                    let rest = ready.split_off(take);
                    let units = std::mem::replace(&mut ready, rest);
                    gone_out.extend(units.iter().map(|t| t.index));
                    w.write(units, &[], &HashMap::new(), false, &pipe, &wake)?;
                    last_write = Some(Instant::now());
                    busy = true;
                }
                w.phase = if (!prepared && !streaming) || (lock(&pipe).units.is_empty() && !walk_done) { Phase::Walking } else { Phase::Reading };
                w.tell(&pipe, due);
                // Stopped (the app's Stop, or quitting): what is read is written (above), and
                // nothing more is read. Checked before the reading may go on past this batch.
                if cancel.load(Ordering::Relaxed) {
                    if !ready.is_empty() {
                        w.write(std::mem::take(&mut ready), &[], &HashMap::new(), false, &pipe, &wake)?;
                    }
                    lock(&pipe).stop = true;
                    w.settle()?;
                    w.report.interrupted = true;
                    log::info!("scan: stopped after {} batches; the files not yet written are read at the next scan", w.report.batches);
                    return Ok(());
                }
                if !gone_out.is_empty() {
                    let mut p = lock(&pipe);
                    p.out(gone_out);
                    p.open_window(ahead);
                    drop(p);
                    wake.notify_all();
                }
                if all_read && ready.is_empty() {
                    w.tell(&pipe, true);
                    break;
                }
                // Wait for news (a file read, a cover, the walk), unless some came meanwhile.
                let p = lock(&pipe);
                if !busy && p.events == seen {
                    let _ = wake.wait_timeout(p, TELL_EVERY).unwrap_or_else(|e| e.into_inner());
                }
            }

            // Everything is read: the moves, the prune and what was held back.
            timing("all read", started);
            let ends = moves.finish(w.conn, &held, &known, &live_roots, opts.confirm_removal, w.report)?;
            if partial && ends.gone_for_good {
                // Something here is gone for good (or moved where this scan cannot see): that is
                // for a full scan to decide, with the guard. Nothing has been written yet.
                log::info!("partial scan: tracks missing under {:?}; scanning everything instead", opts.only);
                handover = true;
                return Ok(());
            }
            w.write(held, &ends.gone, &ends.repoint, true, &pipe, &wake)?;
            if w.report.resumed {
                w.settle_all()?;
            }

            // Art: for an album without art, when its folder changed since art was last looked
            // for (a cover may have been added); and, once after the cache's format changed, for
            // every album whose cached art is older.
            let mut more: Vec<i64> = Vec::new();
            {
                let mut st = w.conn.prepare("SELECT id, folder, art_checked FROM albums WHERE art IS NULL")?;
                let mut first = w.conn.prepare(&format!("SELECT {FIRST_TRACK} FROM albums a WHERE a.id = ?1"))?;
                let rows: Vec<(i64, String, Option<String>)> =
                    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
                for (id, folder, checked) in rows {
                    if w.touched.contains(&id) {
                        continue;
                    }
                    // A partial scan looks only where it scans: a folder changed anywhere else
                    // came with its own change, and its own scan.
                    let folder = Path::new(&folder);
                    if let Some(dirs) = &opts.only
                        && !dirs.iter().any(|d| folder.starts_with(d) || d.starts_with(folder))
                    {
                        continue;
                    }
                    let Some(track) = first.query_row([id], |r| r.get::<_, Option<String>>(0))? else { continue };
                    if checked.as_deref() != Some(art_fingerprint(folder, Path::new(&track)).as_str()) {
                        more.push(id);
                    }
                }
            }
            let upgrading = art.needs_upgrade();
            let mut upgrade: HashMap<i64, String> = HashMap::new();
            if upgrading {
                let mut st = w.conn.prepare("SELECT id, art FROM albums WHERE art IS NOT NULL")?;
                for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
                    let (id, stem) = row?;
                    if !w.touched.contains(&id) && !more.contains(&id) && !art.is_current(&stem) {
                        upgrade.insert(id, stem);
                        more.push(id);
                    }
                }
            }
            timing("art recheck done", started);
            w.ask_art(&more, &pipe, &wake)?;
            // The covers still to come.
            let mut art_cut = false;
            loop {
                let (left, done, total, seen) = {
                    let p = lock(&pipe);
                    (p.art.len() + p.art_busy, p.art_done, p.art_total, p.events)
                };
                w.write_arts(&pipe, false, art, &upgrade)?;
                if left == 0 {
                    break;
                }
                w.tell_art(done, total);
                // Stopped now: the library is written; the covers not found yet are looked for by the
                // next scan (their albums have none, and were never looked at).
                if cancel.load(Ordering::Relaxed) {
                    art_cut = true;
                    lock(&pipe).art.clear();
                }
                let p = lock(&pipe);
                if p.events == seen {
                    let _ = wake.wait_timeout(p, TELL_EVERY).unwrap_or_else(|e| e.into_inner());
                }
            }
            w.write_arts(&pipe, true, art, &upgrade)?;
            if upgrading && !art_cut {
                art.mark_upgraded();
            }
            // Not finished: the covers cut are for the next scan (Resume), as the report says.
            w.report.covers_cut = art_cut;
            Ok(())
        })();
        if r.is_err() || cancel.load(Ordering::Relaxed) {
            lock(&pipe).stop = true;
        }
        shut();
        r
    });
    result?;
    {
        let mut p = lock(&pipe);
        report.errors.extend(std::mem::take(&mut p.art_errors));
        let unreadable = std::mem::take(&mut p.unreadable);
        report.errors.extend(unreadable.into_iter().map(|(path, e)| (path, format!("could not be read, so what is in it was not checked: {e}"))));
    }
    if handover {
        return Ok(None);
    }
    report.albums_changed = lock(&pipe).art_total;
    report.files = report.files.max(walked.len());

    // Tracks from before details were kept: fill them in, last, so the scan's own news (new and
    // changed music, art) is in the library first. Only a full scan does it, and only for files
    // it found where they were, unchanged since.
    if !partial && !report.interrupted {
        let mut r = std::mem::take(&mut report);
        timing("fills start", started);
        let tags = fill_tags(conn, &walked, art, workers, progress, &mut r, cancel, &disk)?;
        r.tags = tags;
        if !r.interrupted {
            r.details = fill_details(conn, &walked, workers, progress, cancel, &disk)?;
        }
        report = r;
    }
    timing("end", started);
    report.seconds = started.elapsed().as_secs_f64();
    Ok(Some(report))
}

/// What the database knows under the scanned folders: path -> its row. A server's own tracks
/// (`remote`) are no file's: never known here, so never read, re-pointed, counted missing or
/// removed by a scan.
fn load_known(conn: &Connection, only: Option<&[PathBuf]>) -> rusqlite::Result<HashMap<String, Known>> {
    let mut known = HashMap::new();
    let row = |r: &rusqlite::Row| -> rusqlite::Result<(String, Known)> {
        Ok((r.get(0)?, Known { id: r.get(1)?, mtime: r.get(2)?, size: r.get(3)?, album: r.get(4)?, folder: r.get(5)? }))
    };
    const COLS: &str = "SELECT t.path, t.id, t.mtime, t.size, t.album_id, g.group_folder FROM tracks t LEFT JOIN track_tags g ON g.track_id = t.id";
    match only {
        None => {
            let mut st = conn.prepare(&format!("{COLS} WHERE t.path NOT LIKE 'navidrome:%'"))?;
            for r in st.query_map([], row)? {
                let (p, k) = r?;
                known.insert(p, k);
            }
        }
        Some(dirs) => {
            let mut st = conn.prepare_cached(&format!("{COLS} WHERE substr(t.path, 1, length(?1)) = ?1 OR substr(t.path, 1, length(?2)) = ?2"))?;
            for d in dirs {
                let (a, b) = root_prefixes(d);
                for r in st.query_map(params![a, b], row)? {
                    let (p, k) = r?;
                    known.insert(p, k);
                }
            }
        }
    }
    Ok(known)
}

/// Windows and macOS (`FOLDS_CASE`): whether a partial scan of `dirs` would look at files under
/// a spelling the library does not have for them, which only a full scan matches up
/// (`match_moves`). A folder renamed
/// only in letter case (the watcher names it in both spellings, or only the new one), or named by
/// its short 8.3 name: the file system finds the files whatever the letter case, so their new
/// paths would come in as new tracks beside the old ones (their history on those) for good.
#[cfg(any(windows, target_os = "macos"))]
fn spelled_otherwise(conn: &Connection, dirs: &[PathBuf]) -> rusqlite::Result<bool> {
    // On the disk.
    for d in dirs {
        if let Ok(c) = std::fs::canonicalize(d)
            && c.file_name() != d.file_name()
        {
            return Ok(true);
        }
    }
    // In the library: a track under a folder in another letter case.
    let lower: Vec<(String, String)> = dirs.iter().map(|d| root_prefixes(d)).map(|(a, b)| (a.to_lowercase(), b.to_lowercase())).collect();
    let exact: Vec<(String, String)> = dirs.iter().map(|d| root_prefixes(d)).collect();
    let mut st = conn.prepare("SELECT path FROM tracks WHERE path NOT LIKE 'navidrome:%'")?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        let low = path.to_lowercase();
        for ((la, lb), (a, b)) in lower.iter().zip(&exact) {
            if (low.starts_with(la.as_str()) || low.starts_with(lb.as_str())) && !path.starts_with(a.as_str()) && !path.starts_with(b.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether `path` is there, looked at once more before its track is taken for gone: `Ok(false)` is
/// the only answer that may remove it (an error, a share timing out, is not "not there").
fn is_there(path: &Path) -> std::io::Result<bool> {
    #[cfg(test)]
    if let Some(e) = super::tests::fault(super::tests::Fault::Exists, path) {
        return Err(e);
    }
    path.try_exists()
}

/// Whether the root `root` is still there at the end of a scan, as a folder that lists; with
/// `found` (the walk found some of its tracks), one that lists empty is not either: the drive
/// went from under it (a mount point left behind). Why not, when not.
fn still_there(root: &Path, found: bool) -> Result<(), String> {
    match std::fs::metadata(root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) | Err(_) => return Err("the folder is not there any more".to_owned()),
    }
    match std::fs::read_dir(root).map(|mut rd| rd.next().is_some()) {
        Ok(any) if any || !found => Ok(()),
        Ok(_) => Err("the folder is empty now, as if its drive went during the scan".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// The vanished tracks, and which new files could be them.
#[derive(Default)]
struct Moves {
    /// (path, id, album) of the known tracks not found, under roots that are still there; then
    /// (a partial scan) the move candidates found library-wide.
    vanished: Vec<(String, i64, i64)>,
    /// How many of `vanished` are under the scanned folders (the rest are candidates only).
    in_scope: usize,
    /// The group folders of the vanished tracks: a unit there is held.
    folders: HashSet<String>,
    /// A partial scan with something gone under its folders: everything is held (it may hand
    /// over to a full scan before writing anything).
    hold_all: bool,
    /// The vanished tracks as candidates (loaded when first needed), by (title, disc, track).
    old: Option<Vec<Candidate>>,
    buckets: HashMap<(String, Option<i64>, Option<i64>), Vec<usize>>,
    by_path: HashMap<String, usize>,
    /// Library-wide candidates a partial scan has looked up (track ids).
    seen: HashSet<i64>,
    partial: bool,
    /// What the walk could not read (path -> why).
    unreadable: HashMap<PathBuf, String>,
    /// Known tracks not found under each of those: held, not vanished.
    unseen: HashMap<PathBuf, usize>,
}

/// What the end of a scan decided.
struct Ends {
    repoint: HashMap<String, (i64, i64)>,
    gone: Vec<(i64, i64)>,
    gone_for_good: bool,
}

/// A new file (a path not known), as `match_moves` compares it.
fn candidate(f: &FileInfo, t: &TrackTags) -> Candidate {
    let (title, artist, album) = display_names(t, &f.path);
    Candidate {
        path: f.path.to_string_lossy().into_owned(),
        title,
        artist,
        album,
        // As tagged: a number read from a file's name changes with a rename.
        disc_no: t.disc_no.filter(|_| !t.from_name.disc).map(i64::from),
        track_no: t.track_no.filter(|_| !t.from_name.track).map(i64::from),
        duration: t.duration,
        size: f.size,
    }
}

impl Moves {
    fn prepare(
        &mut self,
        known: &HashMap<String, Known>,
        walked: &HashMap<String, (i64, i64)>,
        live_roots: &[PathBuf],
        partial: bool,
        unreadable: &[(PathBuf, String)],
    ) {
        self.partial = partial;
        self.unreadable = unreadable.iter().cloned().collect();
        let mut v: Vec<(String, i64, i64)> = Vec::new();
        for (path, k) in known {
            if walked.contains_key(path) {
                continue;
            }
            // Not seen, rather than gone: held (`finish`).
            if let Some(u) = self.unseen_under(path) {
                *self.unseen.entry(u).or_default() += 1;
            } else if root_of(path, live_roots).is_some() {
                v.push((path.clone(), k.id, k.album));
            }
        }
        v.sort();
        for (path, _, _) in &v {
            if let Some(f) = known.get(path).and_then(|k| k.folder.clone()) {
                self.folders.insert(f);
            }
        }
        self.in_scope = v.len();
        self.hold_all = partial && !v.is_empty();
        self.seen = v.iter().map(|x| x.1).collect();
        self.vanished = v;
    }

    /// The path the walk could not read that `path` is, or is under, if any.
    fn unseen_under(&self, path: &str) -> Option<PathBuf> {
        if self.unreadable.is_empty() {
            return None;
        }
        Path::new(path).ancestors().find(|a| self.unreadable.contains_key(*a)).map(Path::to_path_buf)
    }

    fn load_old(&mut self, conn: &Connection) -> rusqlite::Result<()> {
        if self.old.is_some() {
            return Ok(());
        }
        let mut old = Vec::with_capacity(self.vanished.len());
        let mut st = conn.prepare_cached(
            "SELECT t.title, t.artist, a.title, CASE WHEN t.from_name & 4 THEN NULL ELSE t.disc_no END,
                    CASE WHEN t.from_name & 2 THEN NULL ELSE t.track_no END, t.duration, t.size
             FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.id = ?1",
        )?;
        for (path, id, _) in &self.vanished {
            old.push(st.query_row([id], |r| {
                Ok(Candidate {
                    path: path.clone(),
                    title: r.get(0)?,
                    artist: r.get(1)?,
                    album: r.get(2)?,
                    disc_no: r.get(3)?,
                    track_no: r.get(4)?,
                    duration: r.get(5)?,
                    size: r.get(6)?,
                })
            })?);
        }
        for (i, o) in old.iter().enumerate() {
            self.index(i, o);
        }
        self.old = Some(old);
        Ok(())
    }

    fn index(&mut self, i: usize, o: &Candidate) {
        self.buckets.entry((o.title.trim().to_lowercase(), o.disc_no, o.track_no)).or_default().push(i);
        if super::FOLDS_CASE {
            self.by_path.insert(o.path.to_lowercase(), i);
        }
    }

    /// Whether a unit read through must wait for the end: one of its new files could be a
    /// vanished track moved or renamed (`could_be`, or on Windows the same path in another letter
    /// case). A partial scan looks for such tracks library-wide, as before.
    fn could_be_moved(
        &mut self,
        conn: &Connection,
        t: &Taken,
        known: &HashMap<String, Known>,
        walked: &HashMap<String, (i64, i64)>,
        live_roots: &[PathBuf],
    ) -> rusqlite::Result<bool> {
        if self.vanished.is_empty() && !self.partial {
            return Ok(false);
        }
        let mut fits = false;
        for (f, r) in t.files.iter().zip(&t.results) {
            let Ok(tags) = r else { continue };
            let path = f.path.to_string_lossy();
            if known.contains_key(&*path) {
                continue;
            }
            let n = candidate(f, tags);
            if self.partial {
                // A new file here may be a track moved from a folder this scan did not look at:
                // tracks anywhere of the same duration and numbers whose file is gone are
                // candidates too, as in a full scan.
                let mut st = conn.prepare_cached(
                    "SELECT id, path, album_id, title FROM tracks
                     WHERE duration BETWEEN ?1 AND ?2
                       AND (CASE WHEN from_name & 4 THEN NULL ELSE disc_no END) IS ?3
                       AND (CASE WHEN from_name & 2 THEN NULL ELSE track_no END) IS ?4",
                )?;
                let rows: Vec<(i64, String, i64, String)> = st
                    .query_map(params![n.duration - 0.05, n.duration + 0.05, n.disc_no, n.track_no], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                for (id, path, album, title) in rows {
                    // The title is required to match (`could_be`): check it before the disk.
                    if same_text(&title, &n.title)
                        && self.seen.insert(id)
                        && !walked.contains_key(&path)
                        && root_of(&path, live_roots).is_some()
                        && self.unseen_under(&path).is_none()
                        && matches!(is_there(Path::new(&path)), Ok(false))
                    {
                        self.vanished.push((path, id, album));
                        self.old = None;
                    }
                }
            }
            if self.vanished.is_empty() {
                continue;
            }
            if self.old.is_none() {
                self.buckets.clear();
                self.by_path.clear();
                self.load_old(conn)?;
            }
            let old = self.old.as_ref().expect("loaded");
            let bucket = (n.title.trim().to_lowercase(), n.disc_no, n.track_no);
            if (super::FOLDS_CASE && self.by_path.contains_key(&n.path.to_lowercase()))
                || self.buckets.get(&bucket).is_some_and(|is| is.iter().any(|&i| could_be(&old[i], &n)))
            {
                fits = true;
            }
        }
        Ok(fits)
    }

    /// The moves among the held units' new files, the tracks to remove, and whether a partial
    /// scan must hand over.
    fn finish(
        &mut self,
        conn: &Connection,
        held: &[Taken],
        known: &HashMap<String, Known>,
        live_roots: &[PathBuf],
        confirm_removal: bool,
        report: &mut Report,
    ) -> rusqlite::Result<Ends> {
        let mut repoint: HashMap<String, (i64, i64)> = HashMap::new();
        let new: Vec<Candidate> = held
            .iter()
            .flat_map(|t| t.files.iter().zip(&t.results))
            .filter_map(|(f, r)| {
                let t = r.as_ref().ok()?;
                (!known.contains_key(&*f.path.to_string_lossy())).then(|| candidate(f, t))
            })
            .collect();
        if !self.vanished.is_empty() && !new.is_empty() {
            self.load_old(conn)?;
            let old = self.old.as_ref().expect("loaded");
            for (j, i) in match_moves(old, &new) {
                let (_, id, album) = &self.vanished[i];
                repoint.insert(new[j].path.clone(), (*id, *album));
            }
        }
        let moved_ids: HashSet<i64> = repoint.values().map(|&(id, _)| id).collect();
        report.moved = repoint.len();
        let in_scope = &self.vanished[..self.in_scope];
        let gone_for_good = in_scope.iter().any(|(_, id, _)| !moved_ids.contains(id));
        // Prune files that vanished (and did not move), under roots that are still there,
        // unless nearly all of a root went missing at once: then hold them (see `GUARD_*`).
        let mut gone = Vec::new();
        let mut unseen = std::mem::take(&mut self.unseen);
        if !self.partial {
            // Per root: tracks known, and the missing ones (path, id, album).
            type Missing<'p> = Vec<(&'p str, i64, i64)>;
            let mut per_root: Vec<(usize, Missing)> = vec![(0, Vec::new()); live_roots.len()];
            for path in known.keys() {
                if let Some(r) = root_of(path, live_roots) {
                    per_root[r].0 += 1;
                }
            }
            for (path, id, album) in in_scope {
                if !moved_ids.contains(id)
                    && let Some(r) = root_of(path, live_roots)
                {
                    per_root[r].1.push((path, *id, *album));
                }
            }
            for (r, (known_n, missing)) in per_root.into_iter().enumerate() {
                let found = known_n - missing.len();
                if !confirm_removal && missing.len() >= GUARD_MIN_MISSING && found * GUARD_FOUND_RATIO < known_n {
                    report.held.push(Held { root: live_roots[r].clone(), missing: missing.len(), known: known_n, unreadable: None });
                    continue;
                }
                // Once more, file by file: only a plain "not there" removes a track.
                let mut these = Vec::new();
                for (path, id, album) in missing {
                    match is_there(Path::new(path)) {
                        Ok(false) => these.push((id, album)),
                        Ok(true) => log::warn!("scan: {path} was not found by the walk but is there; kept"),
                        Err(e) => {
                            log::warn!("scan: cannot look at {path}: {e}; kept");
                            let folder = Path::new(path).parent().unwrap_or(Path::new(path)).to_path_buf();
                            self.unreadable.entry(folder.clone()).or_insert_with(|| e.to_string());
                            *unseen.entry(folder).or_default() += 1;
                        }
                    }
                }
                // And the root once more, after its files: a drive unplugged or a share dropped
                // during the scan makes every file under it "not there" (on Windows a drive letter
                // or share that is gone is "not found", like a deleted file).
                if !these.is_empty()
                    && let Err(why) = still_there(&live_roots[r], found > 0)
                {
                    log::warn!("scan: {}: {why}; its {} missing tracks are kept", live_roots[r].display(), these.len());
                    report.held.push(Held { root: live_roots[r].clone(), missing: these.len(), known: known_n, unreadable: Some(why) });
                    continue;
                }
                gone.extend(these);
            }
        }
        // What could not be looked at, held: by path, with how many tracks are known under each.
        let mut under: HashMap<&Path, usize> = HashMap::new();
        if !unseen.is_empty() {
            for k in known.keys() {
                for a in Path::new(k).ancestors() {
                    if let Some((p, _)) = unseen.get_key_value(a) {
                        *under.entry(p.as_path()).or_default() += 1;
                    }
                }
            }
        }
        let mut held: Vec<Held> = unseen
            .iter()
            .map(|(path, &missing)| Held {
                root: path.clone(),
                missing,
                known: under.get(path.as_path()).copied().unwrap_or(0).max(missing),
                unreadable: Some(self.unreadable.get(path).cloned().unwrap_or_default()),
            })
            .collect();
        held.sort_by(|a, b| a.root.cmp(&b.root));
        report.held.extend(held);
        report.removed = gone.len();
        Ok(Ends { repoint, gone, gone_for_good })
    }
}

/// The writing side of a scan.
struct Writer<'a> {
    conn: &'a mut Connection,
    known: &'a HashMap<String, Known>,
    first_import: bool,
    full: bool,
    report: &'a mut Report,
    progress: &'a (dyn Fn(Progress) + Sync),
    state_written: bool,
    /// Albums whose art was asked for by this scan.
    touched: HashSet<i64>,
    /// Albums this scan made of new files alone (`Synced::created`).
    created: HashSet<i64>,
    changes: usize,
    albums: usize,
    last_tell: Option<Instant>,
    last_arts: Option<Instant>,
    phase: Phase,
    /// Artists and genres whose names are to be worked out again (`settle`).
    unsettled: (HashSet<i64>, HashSet<i64>),
    last_settle: Option<Instant>,
    last_checkpoint: Option<Instant>,
}

impl Writer<'_> {
    /// Write these units (and remove `gone`) in one transaction; `last`: the end of the scan
    /// (what was held back, the removals), after which nothing of it is left to resume.
    fn write(
        &mut self,
        units: Vec<Taken>,
        gone: &[(i64, i64)],
        repoint: &HashMap<String, (i64, i64)>,
        last: bool,
        pipe: &Mutex<Pipe>,
        wake: &Condvar,
    ) -> rusqlite::Result<()> {
        let mut fresh: Vec<Fresh> = Vec::new();
        for t in units {
            for (f, r) in t.files.into_iter().zip(t.results) {
                match r {
                    Ok(tags) => {
                        let path = f.path.to_string_lossy().into_owned();
                        // A known path, or a vanished track's row re-pointed here (a move or rename).
                        let row = self.known.get(&path).map(|k| (k.id, k.album)).or_else(|| repoint.get(&path).copied());
                        fresh.push(Fresh { path: f.path, mtime: f.mtime, size: f.size, born: f.born, tags, row });
                    }
                    Err(e) => self.report.errors.push((f.path, e)),
                }
            }
        }
        let any = !fresh.is_empty() || !gone.is_empty();
        crate::db::let_waiting_in();
        let t0 = Instant::now();
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let synced = if any {
            let ctx = SyncCtx { dated: self.first_import, earlier: if last { Some(&self.created) } else { None }, defer_settle: true };
            Some(sync(&tx, fresh, gone, &ctx)?)
        } else {
            None
        };
        if self.full {
            if last {
                tx.execute("DELETE FROM scan_state", [])?;
            } else if any && !self.state_written {
                tx.execute("INSERT OR IGNORE INTO scan_state (id, started_at, first_import) VALUES (1, ?1, ?2)", params![now_ms(), self.first_import])?;
                self.state_written = true;
            }
        }
        tx.commit()?;
        self.report.longest_write = self.report.longest_write.max(t0.elapsed().as_secs_f64());
        timing(&format!("batch {} written in {:.1} ms", self.report.batches + 1, t0.elapsed().as_secs_f64() * 1000.0), t0);
        // The WAL into the database, here and now, outside any transaction (`run` turned the
        // automatic one off): never inside a batch, and never left for the app's next write of
        // its own (a play counted) to do on the UI thread. Passive: it waits for nobody.
        if last || self.last_checkpoint.is_none_or(|t| t.elapsed() >= CHECKPOINT_EVERY) {
            let t1 = Instant::now();
            let _ = self.conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
            self.last_checkpoint = Some(Instant::now());
            timing(&format!("checkpoint in {:.1} ms", t1.elapsed().as_secs_f64() * 1000.0), t1);
        }
        let Some(synced) = synced else {
            if last {
                self.settle()?;
            }
            return Ok(());
        };
        self.unsettled.0.extend(synced.unsettled.artists.iter().copied());
        self.unsettled.1.extend(synced.unsettled.genres.iter().copied());
        if last || self.last_settle.is_none_or(|t| t.elapsed() >= SETTLE_EVERY) {
            self.settle()?;
        }
        self.created.extend(synced.created.iter().copied());
        self.report.batches += 1;
        self.changes += 1;
        self.albums = self.conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get::<_, i64>(0))? as usize;
        let mut ids: Vec<i64> = synced.changed.union(&synced.regrouped).copied().collect();
        ids.sort_unstable();
        self.ask_art(&ids, pipe, wake)?;
        // A batch is news: tell it now.
        self.last_tell = None;
        Ok(())
    }

    /// Work out the names of the artists and genres the batches since the last time touched,
    /// a few at a time, each few in a transaction of its own: the most common spelling and
    /// sort name over all their credits, which for a prolific artist or a common genre reads
    /// many rows (`relations::settle_artists_and_genres`). Until then a new artist shows the
    /// spelling it first came with. Run every `SETTLE_EVERY`, when stopped, and at the end.
    fn settle(&mut self) -> rusqlite::Result<()> {
        self.last_settle = Some(Instant::now());
        let (artists, genres) = std::mem::take(&mut self.unsettled);
        let mut artists: Vec<i64> = artists.into_iter().collect();
        let mut genres: Vec<i64> = genres.into_iter().collect();
        artists.sort_unstable();
        genres.sort_unstable();
        self.settle_these(&artists, &genres)
    }

    fn settle_these(&mut self, artists: &[i64], genres: &[i64]) -> rusqlite::Result<()> {
        for chunk in artists.chunks(SETTLE_CHUNK) {
            crate::db::let_waiting_in();
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            relations::settle_artists_and_genres(&tx, chunk, &[])?;
            tx.commit()?;
        }
        for chunk in genres.chunks(SETTLE_CHUNK) {
            crate::db::let_waiting_in();
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            relations::settle_artists_and_genres(&tx, &[], chunk)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Every artist and genre, settled again: after a scan that picked up one which may have
    /// ended without settling what it wrote (a crash, the PC off).
    fn settle_all(&mut self) -> rusqlite::Result<()> {
        let ids = |sql: &str| -> rusqlite::Result<Vec<i64>> { self.conn.prepare(sql)?.query_map([], |r| r.get(0))?.collect() };
        let artists = ids("SELECT id FROM artists ORDER BY id")?;
        let genres = ids("SELECT id FROM genres ORDER BY id")?;
        self.unsettled = Default::default();
        self.settle_these(&artists, &genres)
    }

    fn ask_art(&mut self, ids: &[i64], pipe: &Mutex<Pipe>, wake: &Condvar) -> rusqlite::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.touched.extend(ids.iter().copied());
        let jobs = art_jobs(self.conn, ids)?;
        let mut p = lock(pipe);
        p.art_total += jobs.len();
        p.art.extend(jobs);
        drop(p);
        wake.notify_all();
        Ok(())
    }

    /// Write the covers that came back, in a transaction of their own (as `make_art` does): at
    /// once when `all`, else when a few have gathered or a moment has passed, so the covers
    /// come in with their albums without a write for every one.
    fn write_arts(&mut self, pipe: &Mutex<Pipe>, all: bool, art: &ArtCache, upgrade: &HashMap<i64, String>) -> rusqlite::Result<()> {
        let arts = {
            let mut p = lock(pipe);
            let waiting = p.art.len() + p.art_busy;
            if p.arts.is_empty() || (!all && p.arts.len() < 32 && waiting > 0 && self.last_arts.is_some_and(|t| t.elapsed() < BATCH_EVERY / 2)) {
                return Ok(());
            }
            std::mem::take(&mut p.arts)
        };
        self.last_arts = Some(Instant::now());
        crate::db::let_waiting_in();
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut set_art = tx.prepare_cached("UPDATE albums SET art=?2, art_checked=?3 WHERE id=?1")?;
            for (id, stem, stamp) in arts {
                match (stem, upgrade.get(&id)) {
                    // Only re-caching, and the source is gone or unreadable now: keep the art as
                    // cached (lower quality beats none), and do not try it again.
                    (None, Some(old)) => art.mark_current(old),
                    // No art: remember the folders as they were before looking, to look again
                    // only when they change.
                    (None, None) => {
                        set_art.execute(params![id, Option::<String>::None, stamp])?;
                    }
                    (Some(stem), _) => {
                        set_art.execute(params![id, stem, Option::<String>::None])?;
                    }
                }
            }
        }
        tx.commit()?;
        self.changes += 1;
        Ok(())
    }

    /// Say how far the scan is: at once when `now` (a batch written), else at most every
    /// `TELL_EVERY`.
    fn tell(&mut self, pipe: &Mutex<Pipe>, now: bool) {
        if !now && self.last_tell.is_some_and(|t| t.elapsed() < TELL_EVERY) {
            return;
        }
        self.last_tell = Some(Instant::now());
        let p = lock(pipe);
        let waiting = p
            .busy
            .values()
            .min_by_key(|(_, since)| *since)
            .filter(|(_, since)| since.elapsed() >= SLOW)
            .map(|(path, _)| path.clone())
            .or_else(|| p.listing.as_ref().filter(|(_, since)| since.elapsed() >= SLOW).map(|(dir, _)| dir.clone()));
        let idle = match (p.last_read, p.reading) {
            (_, 0) if p.walk_done && p.taken == p.units.len() => Duration::ZERO,
            (Some(t), _) => t.elapsed(),
            (None, _) => Duration::ZERO,
        };
        let prog = Progress {
            phase: self.phase,
            done: p.read,
            total: self.report_total(&p),
            looking: !p.walk_done,
            found: p.found,
            albums: self.albums,
            resumed: self.report.resumed,
            waiting_on: waiting,
            idle,
            changes: self.changes,
            batches: self.report.batches,
        };
        drop(p);
        (self.progress)(prog);
    }

    /// Files to read this scan, as far as is known (it grows while a first scan's walk goes on).
    fn report_total(&self, p: &Pipe) -> usize {
        p.read + p.units[p.taken.min(p.units.len())..].iter().map(|u| u.left).sum::<usize>() + p.walked.iter().map(|u| u.files.len()).sum::<usize>()
    }

    fn tell_art(&mut self, done: usize, total: usize) {
        if self.last_tell.is_some_and(|t| t.elapsed() < TELL_EVERY) {
            return;
        }
        self.last_tell = Some(Instant::now());
        (self.progress)(Progress { phase: Phase::Art, done, total, albums: self.albums, resumed: self.report.resumed, changes: self.changes, batches: self.report.batches, ..Default::default() });
    }
}

/// Development: `MP_SCAN_TIMING=1` prints when each part of a scan ended.
fn timing(what: &str, since: Instant) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("MP_SCAN_TIMING").is_some()) {
        eprintln!("scan timing: {what} at {:.1} ms", since.elapsed().as_secs_f64() * 1000.0);
    }
}
