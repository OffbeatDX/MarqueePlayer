//! Library scans in the background, folder watching, and the folder list.
//!
//! One library job runs at a time, off the UI thread: a scan, or removing folders from
//! the library (`remove_folder`). `State::scanning` is set while either runs; a rescan
//! asked for meanwhile runs after it (`rescan_pending`, or `pending_paths` for what the
//! watcher saw), and a folder removed during a scan is dropped from the library once
//! the scan has finished (`pending_forget`), so the two never compete for the database.
//!
//! The start-up scan and Rescan look at everything; a change the folder watcher reports
//! is scanned only where it happened (`library::scan::scopes`), unless it needs more.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use library::scan::{Phase, Progress};
use slint::ComponentHandle;

use crate::state::{self, State};
use crate::{Library, views};

fn set_status(st: &Rc<State>, scanning: bool, text: &str) {
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<Library>();
        l.set_scanning(scanning);
        l.set_status(text.into());
        if !scanning {
            l.set_status_title("".into());
            l.set_status_detail("".into());
            l.set_status_note("".into());
            l.set_progress(-1.0);
            l.set_stopping(false);
        }
    }
    // The top right of Home: a job began or ended.
    crate::activity::changed(st);
}

/// The running scan, on the UI thread: its Stop, how fast it reads (for the time left), and
/// the library changes it has told of.
#[derive(Default)]
struct Live {
    cancel: Option<Arc<AtomicBool>>,
    /// The library was empty when it began: it is adding someone's music for the first time.
    first: bool,
    /// (when, files read) of the last half minute.
    samples: VecDeque<(Instant, usize)>,
    changes: usize,
}

thread_local! {
    static LIVE: RefCell<Live> = RefCell::default();
}

/// "29,047".
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "about 3 min left": from the files read per second over the last half minute, once there
/// are some seconds of it, and only while files are being read.
fn time_left(samples: &VecDeque<(Instant, usize)>, left: usize) -> Option<String> {
    let (&(t0, d0), &(t1, d1)) = (samples.front()?, samples.back()?);
    let span = t1.duration_since(t0).as_secs_f64();
    if span < 5.0 || d1 <= d0 || left == 0 {
        return None;
    }
    let secs = left as f64 / ((d1 - d0) as f64 / span);
    Some(match secs {
        s if s < 60.0 => "less than a minute left".to_owned(),
        s if s < 3600.0 => format!("about {} min left", (s / 60.0).ceil() as u64),
        s => {
            let m = (s / 60.0).round() as u64;
            format!("about {} h {} min left", m / 60, m % 60)
        }
    })
}

/// The folder of a file the scan waits on, or the folder itself.
fn folder_of(p: &Path) -> String {
    if library::scan::is_audio(p) { p.parent().unwrap_or(p).display().to_string() } else { p.display().to_string() }
}

/// What the sidebar, Settings and an empty home say of a scan in progress.
#[derive(Debug, Default, PartialEq)]
pub struct Told {
    pub short: String,
    pub title: String,
    pub line: String,
    pub note: String,
    pub progress: f32,
}

/// Words for `p` (`first`: the library was empty when the scan began; `left`: the time left,
/// when it can be told).
pub fn tell(p: &Progress, first: bool, left: Option<String>) -> Told {
    let title = if p.resumed {
        "Picking up where the last scan stopped"
    } else if first {
        "Adding your music"
    } else {
        "Scanning"
    }
    .to_owned();
    let albums = match p.albums {
        0 => String::new(),
        1 => " · 1 album in the library".to_owned(),
        n => format!(" · {} albums in the library", count(n)),
    };
    let slow = p.waiting_on.as_deref().map(folder_of);
    match p.phase {
        Phase::Walking => Told {
            short: if p.found > 0 { format!("Looking for music · {}", count(p.found)) } else { "Looking for music…".into() },
            title,
            line: if p.found > 0 { format!("Looking for music: {} files found so far{albums}", count(p.found)) } else { "Looking for music…".into() },
            note: slow.map(|f| format!("Taking a while to list {f}")).unwrap_or_default(),
            progress: -1.0,
        },
        Phase::Reading => {
            let known = !p.looking && p.total > 0;
            let pct = if known { ((p.done as f64 / p.total as f64) * 100.0).floor() as usize } else { 0 };
            let verb = if first || p.resumed { "Adding music" } else { "Scanning" };
            let short = match (p.total, known) {
                (0, _) => "Scanning…".to_owned(),
                (_, true) => format!("{verb} · {pct}%"),
                (_, false) if p.albums == 1 => format!("{verb} · 1 album"),
                (_, false) if p.albums > 0 => format!("{verb} · {} albums", count(p.albums)),
                _ => format!("{verb}…"),
            };
            let mut line = if known && p.resumed {
                format!("{} read of the {} files left{albums}", count(p.done), count(p.total))
            } else if known {
                format!("{} of {} files read{albums}", count(p.done), count(p.total))
            } else {
                format!("{} files read, still looking for more{albums}", count(p.done))
            };
            let stalled = p.idle >= Duration::from_secs(5) && p.done < p.total;
            if let Some(left) = left.filter(|_| known && !stalled) {
                line.push_str(" · ");
                line.push_str(&left);
            }
            let note = if stalled {
                let secs = p.idle.as_secs();
                let wait = if secs >= 120 { format!("{} min", secs / 60) } else { format!("{secs} s") };
                match &slow {
                    Some(f) => format!("Waiting for the disk: nothing read for {wait} (in {f}). Is the drive still connected?"),
                    None => format!("Waiting for the disk: nothing read for {wait}. Is the drive still connected?"),
                }
            } else {
                slow.map(|f| format!("Slow to read: {f}")).unwrap_or_default()
            };
            Told {
                short: if stalled { "Waiting for the disk…".into() } else { short },
                title,
                line,
                note,
                progress: if known { (p.done as f32 / p.total as f32).clamp(0.0, 1.0) } else { -1.0 },
            }
        }
        Phase::Art => Told {
            short: format!("Finding covers · {}%", (p.done * 100).checked_div(p.total).unwrap_or(0)),
            title,
            line: format!("Every file is read. Finding covers: {} of {} albums", count(p.done), count(p.total)),
            note: String::new(),
            progress: if p.total > 0 { p.done as f32 / p.total as f32 } else { -1.0 },
        },
        Phase::Details => Told {
            short: format!("Reading file details {} / {}", count(p.done), count(p.total)),
            title: "Reading file details".into(),
            line: format!("{} of {} tracks: their format, bit depth and sample rate (nothing else changes)", count(p.done), count(p.total)),
            note: String::new(),
            progress: if p.total > 0 { p.done as f32 / p.total as f32 } else { -1.0 },
        },
        Phase::Tags => Told {
            short: format!("Updating the library {} / {}", count(p.done), count(p.total)),
            title: "Updating the library".into(),
            line: format!("Reading again the tags of {} of {} tracks for this version", count(p.done), count(p.total)),
            note: String::new(),
            progress: if p.total > 0 { p.done as f32 / p.total as f32 } else { -1.0 },
        },
    }
}

/// A scan said how far it is (on the UI thread).
fn progressed(st: &Rc<State>, p: Progress) {
    if !st.scanning.get() {
        return;
    }
    let (told, changed) = LIVE.with(|l| {
        let mut l = l.borrow_mut();
        let now = Instant::now();
        if p.phase == Phase::Reading {
            l.samples.push_back((now, p.done));
            while l.samples.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(30)) {
                l.samples.pop_front();
            }
        }
        let left = time_left(&l.samples, p.total.saturating_sub(p.done));
        let changed = p.changes != l.changes;
        l.changes = p.changes;
        (tell(&p, l.first, left), changed)
    });
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<Library>();
        if !l.get_stopping() {
            l.set_status(told.short.into());
        }
        l.set_status_title(told.title.into());
        l.set_status_detail(told.line.into());
        l.set_status_note(told.note.into());
        l.set_progress(told.progress);
    }
    crate::activity::changed(st);
    if changed {
        views::library_changed_live(st);
    }
}

/// Stop the running scan (Settings): what it has read is written and stays; the next scan
/// (Rescan, or the next start) reads the rest.
pub fn stop(st: &Rc<State>) {
    let asked = LIVE.with(|l| l.borrow().cancel.as_ref().map(|c| !c.swap(true, Ordering::Relaxed)).unwrap_or(false));
    if asked && let Some(app) = st.app.upgrade() {
        let l = app.global::<Library>();
        l.set_stopping(true);
        l.set_status("Stopping…".into());
        log::info!("scan: stop asked for");
    }
}

/// The mass-removal guard's note (empty when it held nothing back).
fn set_held(st: &State, held: &[library::scan::Held]) {
    let text = held_text(held);
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<Library>();
        l.set_held(text.into());
        l.set_held_removable(held_removable(held));
    }
}

/// Whether "Remove missing tracks" would remove anything: tracks held as missing, not only
/// ones in folders that could not be read (those stay until they read again).
pub fn held_removable(held: &[library::scan::Held]) -> bool {
    held.iter().any(|h| h.unreadable.is_none() && h.missing > 0)
}

pub fn held_text(held: &[library::scan::Held]) -> String {
    let lines: Vec<String> = held
        .iter()
        .map(|h| match &h.unreadable {
            // Not seen rather than missing: "Remove missing tracks" does not remove these.
            Some(why) => format!(
                "{} {} in {} could not be checked: it could not be read ({}). They stay in the library (with their plays, favourites and playlists) until it can be read again.",
                h.missing,
                if h.missing == 1 { "track" } else { "tracks" },
                h.root.display(),
                why.trim()
            ),
            None => format!(
                "{} of {} tracks in {} were not found. They stay in the library (with their plays, favourites and playlists) until they are back or you remove them.",
                h.missing,
                h.known,
                h.root.display()
            ),
        })
        .collect();
    lines.join("\n")
}

/// The user confirmed the removal the guard held back: scan again, removing what is
/// missing.
pub fn confirm_removal(st: &Rc<State>) {
    st.confirm_removal.set(true);
    rescan(st);
}

/// Start an incremental scan of everything, unless one is running (then run once more
/// after it).
pub fn rescan(st: &Rc<State>) {
    start(st, None);
}

/// The folder watcher saw changes: scan where they happened (or everything, if that is
/// what it takes).
pub fn rescan_changed(st: &Rc<State>, change: library::watch::Change) {
    match change {
        library::watch::Change::Everything => start(st, None),
        library::watch::Change::Paths(paths) => start(st, Some(paths)),
    }
}

/// A scan of everything (`changed: None`) or of where these paths are.
fn start(st: &Rc<State>, changed: Option<Vec<PathBuf>>) {
    if st.scanning.get() {
        match changed {
            None => st.rescan_pending.set(true),
            Some(paths) => st.pending_paths.borrow_mut().extend(paths),
        }
        return;
    }
    let roots = st.settings.borrow().folders.clone();
    if roots.is_empty() {
        set_status(st, false, "No library folders.");
        return;
    }
    let only = match changed {
        None => None,
        Some(paths) => match library::scan::scopes(&paths, &roots) {
            Some(dirs) if dirs.is_empty() => return, // nothing under the library
            dirs => dirs,
        },
    };
    st.scanning.set(true);
    set_status(st, true, "Scanning…");
    let cancel = Arc::new(AtomicBool::new(false));
    let first = library::query::counts(&st.db).map(|c| c.1 == 0).unwrap_or(false);
    LIVE.with(|l| *l.borrow_mut() = Live { cancel: Some(cancel.clone()), first, ..Default::default() });
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<Library>();
        l.set_resumable(false);
        l.set_status_title(if first { "Adding your music" } else { "Scanning" }.into());
        l.set_status_detail("Looking for music…".into());
    }
    crate::activity::changed(st);
    // A confirmed removal is for a full scan; a partial one never removes anything.
    let confirm_removal = only.is_none() && st.confirm_removal.replace(false);
    let opts = library::scan::Options { confirm_removal, only, cancel: Some(cancel), ..Default::default() };
    let db = st.paths.database();
    let art = st.art.clone();
    let spawned = std::thread::Builder::new().name("mp-scan".into()).spawn(move || {
        library::background_thread();
        // Whatever happens in the scan, a panic included, the outcome is posted, so the
        // app always leaves the scanning state. (The scanner already turns a panic in one
        // file into that file's error; this catches anything else.)
        let result = library::guard::catching(|| {
            library::db::open(&db)
                .and_then(|mut conn| {
                    // A server sync's writes wait for the scan (server::sync::WRITER).
                    let _writer = crate::server::sync::writer();
                    // A few times a second and at every batch written: the UI thread does
                    // little with it (words, and a library change now and then).
                    let report = library::scan::scan_with(&mut conn, &art, &roots, opts, &|p: Progress| {
                        state::post(move |st| progressed(st, p));
                    })?;
                    // A server's catalog matched again with what the scan wrote: a file that
                    // is a server's track becomes one row with it (one query when there is no
                    // server, or nothing to do).
                    reconcile(&mut conn);
                    // Albums gone or their art looked at again: covers may be unused now.
                    sweep_art(&conn, &art, report.removed > 0 || report.albums_changed > 0);
                    Ok(report)
                })
                .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|what| {
            log::error!("scan thread panicked: {what}");
            Err(format!("internal error ({what})"))
        });
        state::post(move |st| finish(st, result));
    });
    if let Err(e) = spawned {
        log::error!("cannot start the scan thread: {e}");
        finish(st, Err(format!("cannot start ({e})")));
    }
}

/// Set by the first sweep of the art cache since start.
static ART_SWEPT: AtomicBool = AtomicBool::new(false);

/// Remove the cached covers no album uses (`library::art::sweep_unused`), on a library job's
/// thread, which holds `server::sync::WRITER` (no scan or sync writes meanwhile). `removed`:
/// the job removed albums or looked at their art again; otherwise the cache is swept only
/// once per start (by the first job to end).
pub fn sweep_art(conn: &rusqlite::Connection, art: &library::ArtCache, removed: bool) {
    if ART_SWEPT.swap(true, Ordering::AcqRel) && !removed {
        return;
    }
    let t0 = Instant::now();
    match library::art::sweep_unused(conn, art) {
        Ok(s) if s.files > 0 => log::info!("art cache: {} unused files removed ({} KB) in {} ms", s.files, s.bytes / 1024, t0.elapsed().as_millis()),
        Ok(_) => {}
        Err(e) => log::warn!("art cache: not swept ({e})"),
    }
}

/// `library::remote::reconcile` after a library job; a failure is logged (the next scan or
/// sync tries again: what is pending stays pending).
fn reconcile(conn: &mut rusqlite::Connection) {
    match library::remote::reconcile(conn) {
        Ok(r) if r.albums > 0 => log::info!("server: {} albums matched again after the scan ({} tracks with files, {} merged)", r.albums, r.attached, r.merged),
        Ok(_) => {}
        Err(e) => log::warn!("server: matching after the scan failed ({e})"),
    }
}

/// The end of a scan, on the UI thread: leave the scanning state, show the outcome, and
/// run the rescan asked for meanwhile.
fn finish(st: &Rc<State>, result: Result<library::scan::Report, String>) {
    st.scanning.set(false);
    LIVE.with(|l| *l.borrow_mut() = Live::default());
    match result {
        Ok(r) => {
            let (albums, tracks) = library::query::counts(&st.db).unwrap_or((0, 0));
            // Stopped in the covers (every file read): not finished either; Resume looks at them.
            let mut text = if r.covers_cut && !r.interrupted {
                format!("Stopped with {} albums, {} tracks in. The covers not looked at yet are looked for at the next start, or by Resume", count(albums as usize), count(tracks as usize))
            } else if r.interrupted {
                format!("Stopped with {} albums, {} tracks in. The rest is read at the next start, or by Resume", count(albums as usize), count(tracks as usize))
            } else {
                format!("{} albums, {} tracks. Last scan {:.1} s", count(albums as usize), count(tracks as usize), r.seconds)
            };
            if r.moved > 0 {
                text.push_str(&format!(", {} moved or renamed", r.moved));
            }
            for h in &r.held {
                log::warn!("scan: {} of {} tracks under {} missing; held back", h.missing, h.known, h.root.display());
            }
            // A partial scan does not look at the guard (it removes nothing), nor does a
            // stopped one: the note of the last full scan stands.
            if !r.partial && !r.interrupted {
                set_held(st, &r.held);
            }
            if !r.errors.is_empty() {
                text.push_str(&format!(", {} unreadable files", r.errors.len()));
                for (p, e) in r.errors.iter().take(20) {
                    log::warn!("unreadable: {}: {e}", p.display());
                }
            }
            // A stopped scan saw only some folders: its list adds to the last, as a partial's.
            set_unreadable(st, &r.errors, r.partial || r.interrupted);
            set_status(st, false, &text);
            if let Some(app) = st.app.upgrade() {
                app.global::<Library>().set_resumable(r.interrupted || r.covers_cut);
            }
            log::info!(
                "scan: {} files, {} read in {} batches (the longest {:.0} ms), {} removed, {} moved, {:.1} s{}{}",
                r.files,
                r.read,
                r.batches,
                r.longest_write * 1000.0,
                r.removed,
                r.moved,
                r.seconds,
                if r.resumed { ", resumed" } else { "" },
                if r.interrupted { ", stopped" } else if r.covers_cut { ", stopped in the covers" } else { "" }
            );
            if r.details > 0 {
                log::info!("scan: filled in the details of {} tracks", r.details);
            }
            if r.read > 0 || r.removed > 0 || r.moved > 0 || r.albums_changed > 0 || r.details > 0 || r.batches > 0 {
                crate::playback::refresh_queue_tracks(st);
                views::scan_finished(st);
                // An open View metadata shows what the library holds now (after a tag edit
                // above all: no stale "the next scan takes it").
                crate::metadata::view::library_changed(st);
            }
        }
        Err(e) => set_status(st, false, &format!("Scan failed: {e}")),
    }
    next_job(st);
}

/// After a library job: folders removed meanwhile first, then a rescan asked for (of
/// everything, which covers any watcher changes too, or of those).
fn next_job(st: &Rc<State>) {
    if !st.pending_forget.borrow().is_empty() {
        forget(st);
    } else if st.rescan_pending.replace(false) {
        st.pending_paths.borrow_mut().clear();
        rescan(st);
    } else if !st.pending_paths.borrow().is_empty() {
        let paths = std::mem::take(&mut *st.pending_paths.borrow_mut());
        start(st, Some(paths));
    }
}

/// Drop the folders in `pending_forget` from the library, on a thread of its own.
fn forget(st: &Rc<State>) {
    if st.scanning.get() {
        return; // runs when the current job ends (next_job)
    }
    let roots: Vec<PathBuf> = std::mem::take(&mut *st.pending_forget.borrow_mut());
    if roots.is_empty() {
        return;
    }
    st.scanning.set(true);
    let names: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    set_status(st, true, &format!("Removing {} from the library…", names.join(", ")));
    let db = st.paths.database();
    let art = st.art.clone();
    // What a folder still in the library holds too stays (one folder inside another).
    let keep = st.settings.borrow().folders.clone();
    let spawned = std::thread::Builder::new().name("mp-forget".into()).spawn(move || {
        let _writer = crate::server::sync::writer();
        let result = library::db::open(&db)
            .and_then(|mut c| {
                let n = roots.iter().map(|r| library::scan::forget_root(&mut c, r, &keep)).sum::<rusqlite::Result<usize>>()?;
                // A removed file the server has is the server's track again; matched anew.
                reconcile(&mut c);
                sweep_art(&c, &art, n > 0);
                Ok(n)
            })
            .map_err(|e| e.to_string());
        state::post(move |st| finish_forget(st, &names, result));
    });
    if let Err(e) = spawned {
        log::error!("cannot start the folder removal: {e}");
        st.scanning.set(false);
        set_status(st, false, &format!("Removing folder failed: {e}"));
    }
}

fn finish_forget(st: &Rc<State>, names: &[String], result: Result<usize, String>) {
    st.scanning.set(false);
    match result {
        Ok(n) => set_status(st, false, &format!("Removed {} ({n} tracks) from the library.", names.join(", "))),
        Err(e) => set_status(st, false, &format!("Removing folder failed: {e}")),
    }
    // What the guard held under a removed folder is gone with it.
    set_held(st, &[]);
    views::refresh_all(st);
    next_job(st);
}

/// Settings lists the files a scan could not read (all of them; the log has the first 20).
/// A full scan's list replaces the last one; a partial scan's adds to it, as it looked at
/// only some folders.
fn set_unreadable(st: &State, errors: &[(PathBuf, String)], partial: bool) {
    let Some(app) = st.app.upgrade() else { return };
    let l = app.global::<Library>();
    let fresh: Vec<String> = errors.iter().map(|(p, e)| unreadable_line(p, e)).collect();
    let lines = if partial {
        use slint::Model;
        let mut old: Vec<String> = l.get_unreadable().iter().map(|s| s.to_string()).collect();
        merge_unreadable(&mut old, fresh);
        old
    } else {
        fresh
    };
    let lines: Vec<slint::SharedString> = lines.into_iter().map(Into::into).collect();
    l.set_unreadable(slint::ModelRc::new(slint::VecModel::from(lines)));
}

/// One line of the list: the path, then why.
pub fn unreadable_line(path: &std::path::Path, error: &str) -> String {
    format!("{}: {}", path.display(), error.trim())
}

/// Add a partial scan's unreadable files to the list, each path once (the newer reason).
pub fn merge_unreadable(list: &mut Vec<String>, fresh: Vec<String>) {
    for line in fresh {
        // The path ends at the first ": " (Windows names cannot hold a colon).
        let path = line.split_once(": ").map_or(line.as_str(), |(p, _)| p).to_owned();
        list.retain(|l| l.split_once(": ").map_or(l.as_str(), |(p, _)| p) != path);
        list.push(line);
    }
}

pub fn restart_watch(st: &Rc<State>) {
    let roots = st.settings.borrow().folders.clone();
    *st.watch.borrow_mut() = None;
    let generation = WATCH_GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    if roots.is_empty() {
        return;
    }
    // Set up off the UI thread: on Linux it means a watch per folder of the library (inotify).
    let spawned = std::thread::Builder::new().name("mp-watch-setup".into()).spawn(move || {
        let w = library::watch::watch(&roots, |change| state::post(move |st| rescan_changed(st, change)));
        let note = library::dirwatch::limit_note();
        state::post(move |st| {
            // A newer restart (folders changed meanwhile) wins; this one is dropped.
            if WATCH_GEN.with(std::cell::Cell::get) != generation {
                return;
            }
            match w {
                Ok(w) => *st.watch.borrow_mut() = Some(w),
                Err(e) => log::warn!("folder watch unavailable: {e}"),
            }
            if let Some(note) = note {
                views::flash_long(st, &note);
            }
        });
    });
    if let Err(e) = spawned {
        log::warn!("folder watch unavailable: {e}");
    }
}

thread_local! {
    /// Bumped by every `restart_watch`: only the newest one's watch is kept.
    static WATCH_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The library folder that `folder` is (or is inside), if any: its music is there already.
fn covered_by<'a>(folders: &'a [PathBuf], folder: &Path) -> Option<&'a PathBuf> {
    folders.iter().find(|f| library::scan::outermost(&[(*f).clone(), folder.to_path_buf()]) == [(*f).clone()])
}

pub fn add_folder(st: &Rc<State>, folder: PathBuf) {
    {
        let mut s = st.settings.borrow_mut();
        if let Some(outer) = covered_by(&s.folders, &folder) {
            let text = if *outer == folder { format!("{} is in the library already", folder.display()) } else { format!("{} is in the library already, as part of {}", folder.display(), outer.display()) };
            drop(s);
            views::flash(st, &text);
            return;
        }
        // Removed during a scan and added back before it ended: nothing to remove after all.
        st.pending_forget.borrow_mut().retain(|r| *r != folder);
        s.folders.push(folder);
    }
    st.save_settings_soon();
    views::refresh_folders(st);
    restart_watch(st);
    rescan(st);
}

pub fn pick_and_add_folder(st: &Rc<State>) {
    if !crate::dialog_allowed("add music folder") {
        return;
    }
    if let Some(folder) = rfd::FileDialog::new().set_title("Add a music folder").pick_folder() {
        add_folder(st, folder);
    }
}

pub fn remove_folder(st: &Rc<State>, index: usize) {
    let removed = {
        let mut s = st.settings.borrow_mut();
        (index < s.folders.len()).then(|| s.folders.remove(index))
    };
    let Some(root) = removed else { return };
    st.save_settings_soon();
    views::refresh_folders(st);
    restart_watch(st);
    // Removing a folder removes its albums from the library (never from disk), in the
    // background; during a scan, once the scan is done.
    st.pending_forget.borrow_mut().push(root);
    if st.scanning.get() {
        set_status(st, true, "Scanning… (the folder is removed from the library after the scan)");
    } else {
        forget(st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!((count(0), count(999), count(1000), count(29_047), count(1_234_567)), ("0".into(), "999".into(), "1,000".into(), "29,047".into(), "1,234,567".into()));
    }

    /// What a scan says at each step: a first import's words, how far once the total is
    /// known, the time left only then, a slow folder named, a stalled disk said as such.
    #[test]
    fn a_scan_says_what_it_does_how_far_and_when_the_disk_is_slow() {
        let p = |phase, done, total, looking| Progress { phase, done, total, looking, ..Default::default() };
        let walk = Progress { found: 1204, ..p(Phase::Walking, 0, 0, true) };
        let t = tell(&walk, true, None);
        assert_eq!((t.title.as_str(), t.short.as_str()), ("Adding your music", "Looking for music · 1,204"));
        assert!(t.progress < 0.0);
        let reading = Progress { albums: 312, ..p(Phase::Reading, 4120, 29047, false) };
        let t = tell(&reading, true, Some("about 3 min left".into()));
        assert_eq!(t.short, "Adding music · 14%");
        assert_eq!(t.line, "4,120 of 29,047 files read · 312 albums in the library · about 3 min left");
        assert!((t.progress - 0.1418).abs() < 0.001 && t.note.is_empty());
        let t = tell(&Progress { albums: 3, ..p(Phase::Reading, 40, 41, true) }, true, Some("x".into()));
        assert_eq!((t.short.as_str(), t.line.as_str()), ("Adding music · 3 albums", "40 files read, still looking for more · 3 albums in the library"), "no time left while the total is not known");
        let slow = Progress { waiting_on: Some(PathBuf::from("D:/Music/Band/Album/01.flac")), ..reading.clone() };
        assert_eq!(tell(&slow, false, None).note, format!("Slow to read: {}", Path::new("D:/Music/Band/Album").display()));
        assert_eq!(tell(&slow, false, None).short, "Scanning · 14%", "a rescan says scanning");
        let stalled = Progress { idle: Duration::from_secs(12), ..slow };
        let t = tell(&stalled, true, Some("about 3 min left".into()));
        assert_eq!(t.short, "Waiting for the disk…");
        assert!(t.note.starts_with("Waiting for the disk: nothing read for 12 s (in ") && !t.line.contains("left"), "{t:?}");
        assert_eq!(tell(&Progress { resumed: true, ..reading }, false, None).title, "Picking up where the last scan stopped");
        assert_eq!(tell(&p(Phase::Reading, 0, 0, false), false, None).short, "Scanning…", "nothing to read");
    }

    #[test]
    fn the_time_left_comes_from_the_last_seconds_of_reading() {
        let t0 = Instant::now();
        let mut s = VecDeque::from([(t0, 1000), (t0 + Duration::from_secs(2), 1400)]);
        assert_eq!(time_left(&s, 5000), None, "not enough seconds yet");
        s.push_back((t0 + Duration::from_secs(10), 3000));
        // 200 files a second.
        assert_eq!(time_left(&s, 30_000).as_deref(), Some("about 3 min left"));
        assert_eq!(time_left(&s, 5_000).as_deref(), Some("less than a minute left"));
        assert_eq!(time_left(&s, 1_000_000).as_deref(), Some("about 1 h 23 min left"));
    }

    #[test]
    fn a_folder_inside_one_listed_is_said_to_be_there_already() {
        let folders = [PathBuf::from("M:/Music"), PathBuf::from("N:/More")];
        assert_eq!(covered_by(&folders, Path::new("M:/Music/Jazz")), Some(&folders[0]));
        assert_eq!(covered_by(&folders, Path::new("M:/Music")), Some(&folders[0]), "the same one again");
        assert_eq!(covered_by(&folders, Path::new("M:/Musical")), None, "a name that only begins the same");
        assert_eq!(covered_by(&folders, Path::new("M:/")), None, "one around a listed folder is added");
        assert_eq!(covered_by(&folders, Path::new("O:/Other")), None);
    }

    #[test]
    fn unreadable_files_list_each_path_once() {
        let line = unreadable_line(std::path::Path::new(r"M:\a\01.flac"), " no audio track \n");
        assert_eq!(line, r"M:\a\01.flac: no audio track");
        let mut list = vec![line, r"M:\b\02.mp3: bad header".to_owned()];
        merge_unreadable(&mut list, vec![r"M:\a\01.flac: still broken".to_owned(), r"M:\c\03.ogg: truncated".to_owned()]);
        assert_eq!(list, vec![r"M:\b\02.mp3: bad header", r"M:\a\01.flac: still broken", r"M:\c\03.ogg: truncated"]);
    }

    #[test]
    fn held_tracks_say_why_they_are_kept() {
        use library::scan::Held;
        let guard = Held { root: r"M:\Music".into(), missing: 30, known: 31, unreadable: None };
        let unread = Held { root: r"M:\Music\A".into(), missing: 1, known: 4, unreadable: Some("Access is denied. (os error 5)".into()) };
        assert!(held_removable(&[guard.clone(), unread.clone()]) && held_removable(std::slice::from_ref(&guard)));
        assert!(!held_removable(std::slice::from_ref(&unread)), "only unreadable: nothing to remove");
        assert!(!held_removable(&[]));
        let text = held_text(&[guard, unread]);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(r"30 of 31 tracks in M:\Music were not found."), "{text}");
        assert!(lines[1].starts_with(r"1 track in M:\Music\A could not be checked: it could not be read (Access is denied. (os error 5))."), "{text}");
        assert!(lines[1].ends_with("until it can be read again."), "{text}");
    }
}
