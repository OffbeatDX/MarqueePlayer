//! The first scan of a large library (`MP_BENCH=bigscan`), and a long run (`soak`).
//!
//! - `MP_BENCH=bigscan` starts at once (not after the start-up scan, as the other modes do):
//!   every second while the first scan runs it notes the status line, the process's memory and
//!   handles and the longest the UI thread was late for a 16 ms timer (a stall a user would feel
//!   as a frozen window); a snapshot during the scan shows its progress. When the scan is over:
//!   how long it took, how long until the library showed albums, the peak memory, the longest
//!   stall. `MP_BENCH_QUIT_AFTER=<s>` quits that many seconds in, scan or not (the runner then
//!   starts it again to see the next start finish the job).
//! - `MP_BENCH=soak`: plays the whole library on repeat, moving to the next track every 1.5 s
//!   (a seek near each track's end, so every hand-over is the real gapless one) and changing
//!   page every 10 s, for `MP_BENCH_SOAK_SECS` (default 120); memory, handles and GDI and USER
//!   objects every 10 s, checked for growth after a warm-up.
//!
//! Reports: `bench-bigscan.txt`, `bench-soak.txt`, with `[ok]` / `[FAIL]` checks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, Timer, TimerMode};

use crate::bench::{snapshot, verdict, write_report};
use crate::nav::Route;
use crate::{Page, playback, views};

/// The longest the UI thread was late for a 16 ms timer since the last call.
struct Lag {
    last: Cell<Instant>,
    worst: Cell<Duration>,
}

fn lag_probe() -> (Rc<Lag>, Timer) {
    let lag = Rc::new(Lag { last: Cell::new(Instant::now()), worst: Cell::new(Duration::ZERO) });
    let t = Timer::default();
    let l = lag.clone();
    t.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let now = Instant::now();
        let late = now.duration_since(l.last.get()).saturating_sub(Duration::from_millis(16));
        if late > l.worst.get() {
            l.worst.set(late);
        }
        l.last.set(now);
    });
    (lag, t)
}

/// One stage of the scan, snapshotted once: navigate there at a tick, snapshot at the next.
struct Stage {
    name: &'static str,
    /// From this far through the scan (Library.progress), or this many seconds in.
    at: f32,
    secs: f64,
    page: Option<Route>,
    /// The window's client size for it (the minimum, 960 x 620), if not as it is.
    size: Option<(i32, i32)>,
    done: bool,
}

pub fn run_bigscan() {
    let t0 = Instant::now();
    let quit_after: Option<u64> = std::env::var("MP_BENCH_QUIT_AFTER").ok().and_then(|v| v.parse().ok());
    let quit_at: Option<f32> = std::env::var("MP_BENCH_QUIT_AT").ok().and_then(|v| v.parse().ok());
    // Stop pressed this far through (Settings > Library's Stop), then Resume once it has stopped.
    let stop_at: Option<f32> = std::env::var("MP_BENCH_STOP_AT").ok().and_then(|v| v.parse().ok());
    let stopping: Rc<Cell<u8>> = Rc::default();
    let slow = std::env::var("MP_SLOW_DISK_MS").ok().filter(|v| v != "0");
    let (lag, lag_timer) = lag_probe();
    let report: Rc<RefCell<String>> = Rc::default();
    if let Some(ms) = &slow {
        report.borrow_mut().push_str(&format!("a slow disk simulated: {ms} ms a file or folder, one at a time\n"));
    }
    let startup = Rc::new(Cell::new(Duration::ZERO));
    let worst = Rc::new(Cell::new(Duration::ZERO));
    let peak = Rc::new(Cell::new(0.0f64));
    let seen_scan = Rc::new(Cell::new(false));
    let first_albums: Rc<Cell<Option<f64>>> = Rc::default();
    // What a play counted or a favourite would wait for: an empty write on the UI thread's own
    // connection every 200 ms, timed, while the scan writes from its thread.
    let writes: Rc<RefCell<Vec<f64>>> = Rc::default();
    let second_writes: Rc<Cell<f64>> = Rc::default();
    let write_timer = Rc::new(Timer::default());
    {
        let (writes, second_writes) = (writes.clone(), second_writes.clone());
        write_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            crate::state::with(|st| {
                if !st.scanning.get() {
                    return;
                }
                let t = Instant::now();
                if st.db.execute_batch("BEGIN IMMEDIATE; COMMIT;").is_ok() {
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    writes.borrow_mut().push(ms);
                    second_writes.set(second_writes.get().max(ms));
                }
            });
        });
    }
    // The first albums on the screen: Home's shelves, looked at every 50 ms.
    let shown_timer = Rc::new(Timer::default());
    {
        let (first, me) = (first_albums.clone(), shown_timer.clone());
        shown_timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            crate::state::with(|st| {
                let Some(app) = st.app.upgrade() else { return };
                if app.global::<crate::Home>().get_shelves().row_count() > 0 && first.get().is_none() {
                    first.set(Some(t0.elapsed().as_secs_f64()));
                    snapshot(&app, "bigscan-first-albums");
                    me.stop();
                }
            });
        });
    }
    // The very start, before the first album: what a newcomer sees first.
    Timer::single_shot(Duration::from_millis(250), || {
        crate::state::with(|st| {
            if let Some(app) = st.app.upgrade()
                && app.global::<crate::Home>().get_empty()
            {
                snapshot(&app, "bigscan-before-albums");
            }
        });
    });
    let tracks_at_start = crate::state::with(|st| library::query::counts(&st.db).map(|c| c.1).unwrap_or(0)).unwrap_or(0);
    if tracks_at_start > 0 {
        report.borrow_mut().push_str(&format!("{tracks_at_start} tracks already in the library at the start (a scan stopped before)\n"));
    }
    let titles: Rc<RefCell<Vec<String>>> = Rc::default();
    let stages = Rc::new(RefCell::new(vec![
        Stage { name: "bigscan-start", at: f32::MAX, secs: 1.0, page: None, size: None, done: false },
        Stage { name: "bigscan-25-home", at: 0.25, secs: f64::MAX, page: Some(Route::Page(Page::Home)), size: None, done: false },
        Stage { name: "bigscan-50-settings", at: 0.5, secs: f64::MAX, page: Some(Route::Settings(crate::settings::nav::LIBRARY)), size: None, done: false },
        Stage { name: "bigscan-60-min-home", at: 0.6, secs: f64::MAX, page: Some(Route::Page(Page::Home)), size: Some((960, 620)), done: false },
        Stage { name: "bigscan-65-min-settings", at: 0.65, secs: f64::MAX, page: Some(Route::Settings(crate::settings::nav::LIBRARY)), size: None, done: false },
        Stage { name: "bigscan-70-albums", at: 0.7, secs: f64::MAX, page: Some(Route::Page(Page::Albums)), size: None, done: false },
        Stage { name: "bigscan-85-home", at: 0.85, secs: f64::MAX, page: Some(Route::Page(Page::Home)), size: Some((1920, 1032)), done: false },
    ]));
    let pending: Rc<Cell<Option<usize>>> = Rc::default();
    let sampler = Rc::new(Timer::default());
    let s2 = sampler.clone();
    sampler.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let _keep = (&lag_timer, &write_timer, &shown_timer);
        crate::state::with(|st| {
            let Some(app) = st.app.upgrade() else { return };
            let secs = t0.elapsed().as_secs_f64();
            let scanning = st.scanning.get();
            seen_scan.set(seen_scan.get() || scanning);
            let (mem, peak_ws, handles) = crate::platform::system::resources();
            peak.set(peak.get().max(peak_ws));
            let late = lag.worst.replace(Duration::ZERO);
            // The first seconds are the window's own start (the renderer, the first frame): said
            // apart; the scan is judged from then on.
            if secs > 3.0 {
                worst.set(worst.get().max(late));
            } else {
                startup.set(startup.get().max(late));
            }
            let l = app.global::<crate::Library>();
            let (status, title, detail, note, progress) = (l.get_status().to_string(), l.get_status_title().to_string(), l.get_status_detail().to_string(), l.get_status_note().to_string(), l.get_progress());
            if !title.is_empty() && !titles.borrow().contains(&title) {
                titles.borrow_mut().push(title.clone());
            }
            let albums = library::query::counts(&st.db).map(|c| c.0).unwrap_or(0);
            let write = second_writes.replace(0.0);
            report.borrow_mut().push_str(&format!(
                "{secs:>6.1} s  {:<5} {status:<34} albums {albums:>5}  mem {mem:>5.0} MB (peak {peak_ws:.0})  handles {handles}  UI late {:>3.0} ms  write {write:>4.1} ms\n",
                if scanning { "scan" } else { "idle" },
                late.as_secs_f64() * 1000.0
            ));
            if scanning && (!detail.is_empty() || !note.is_empty()) {
                report.borrow_mut().push_str(&format!("          {title}: {detail}{}{note}\n", if note.is_empty() { "" } else { " | " }));
            }
            // Snapshots of the scan as it goes: a stage navigated to at one tick, taken at the next.
            if let Some(i) = pending.take() {
                snapshot(&app, stages.borrow()[i].name);
            } else if scanning {
                let mut s = stages.borrow_mut();
                if let Some((i, stage)) = s.iter_mut().enumerate().find(|(_, g)| !g.done && (progress >= g.at || secs >= g.secs)) {
                    stage.done = true;
                    match &stage.page {
                        Some(r) => {
                            if let Some((w, h)) = stage.size {
                                crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
                            }
                            views::navigate(st, r.clone());
                            pending.set(Some(i));
                        }
                        None => {
                            snapshot(&app, stage.name);
                        }
                    }
                }
            }
            // Stop and Resume, as a person would from Settings > Library: 0 not yet, 1 stop pressed,
            // 2 stopped and snapshotted (Resume pressed), 3 resumed and seen scanning.
            match (stop_at, stopping.get()) {
                (Some(f), 0) if scanning && progress >= f => {
                    views::navigate(st, Route::Settings(crate::settings::nav::LIBRARY));
                    crate::scanning::stop(st);
                    stopping.set(1);
                    report.borrow_mut().push_str(&format!("          Stop pressed at {:.0}%
", progress * 100.0));
                }
                (Some(_), 1) if !scanning => {
                    snapshot(&app, "bigscan-stopped");
                    let resumable = l.get_resumable();
                    report.borrow_mut().push_str(&format!("          stopped: \"{status}\", Resume offered {resumable}
"));
                    verdict(&mut report.borrow_mut(), "Stop ends the scan, keeps what it read and offers Resume", resumable && library::query::counts(&st.db).is_ok_and(|c| c.1 > 0), &status);
                    crate::scanning::rescan(st);
                    stopping.set(2);
                    return;
                }
                (Some(_), 2) if scanning => {
                    stopping.set(3);
                    verdict(&mut report.borrow_mut(), "Resume picks the scan up", title.starts_with("Picking up"), &title);
                }
                _ => {}
            }
            if stop_at.is_some() && stopping.get() < 2 && !(stopping.get() == 1 && !scanning) {
                if !scanning && stopping.get() == 0 && secs > 2.0 && seen_scan.get() {
                    verdict(&mut report.borrow_mut(), "Stop was pressed before the scan ended", false, "it ended first");
                } else {
                    return;
                }
            }
            let quit = quit_after.is_some_and(|q| secs >= q as f64) || quit_at.is_some_and(|q| scanning && progress >= q);
            // Over: seen running and now not, or (a library already read) over before the first look.
            let done = !scanning && secs > 2.0 && (seen_scan.get() || library::query::counts(&st.db).is_ok_and(|c| c.1 > 0));
            if !(quit || done) {
                return;
            }
            s2.stop();
            let mut r = report.borrow_mut();
            let counts = library::query::counts(&st.db).unwrap_or((0, 0));
            r.push_str(&format!(
                "\n{} after {secs:.1} s ({:.0}% through): {} albums, {} tracks\n",
                if done { "scan over" } else { "quitting mid-scan" },
                progress.max(0.0) * 100.0,
                counts.0,
                counts.1
            ));
            r.push_str(&format!("albums first on the screen (Home) at {:?} s; peak working set {:.0} MB\n", first_albums.get().map(|s| (s * 10.0).round() / 10.0), peak.get()));
            r.push_str(&format!("the window's start (its first 3 s): UI late by up to {:.0} ms\n", startup.get().as_secs_f64() * 1000.0));
            r.push_str(&format!("what the scan called itself: {:?}\n", titles.borrow()));
            let mut w = writes.borrow().clone();
            w.sort_by(f64::total_cmp);
            let pct = |p: f64| w.get(((w.len().saturating_sub(1)) as f64 * p).round() as usize).copied().unwrap_or(0.0);
            r.push_str(&format!(
                "UI-thread database writes during the scan: {} timed, median {:.1} ms, p99 {:.1} ms, max {:.1} ms, {} over 50 ms\n",
                w.len(),
                pct(0.5),
                pct(0.99),
                pct(1.0),
                w.iter().filter(|&&x| x > 50.0).count()
            ));
            let worst_ms = worst.get().as_secs_f64() * 1000.0;
            verdict(&mut r, "the window stays responsive during the scan (UI thread never late by 250 ms or more)", worst_ms < 250.0, format!("{worst_ms:.0} ms at worst"));
            verdict(&mut r, "a write of the app's own never waits long for the scan (under 150 ms)", pct(1.0) < 150.0, format!("{:.1} ms at worst", pct(1.0)));
            if tracks_at_start > 0 && seen_scan.get() {
                verdict(&mut r, "the restart picks up the stopped scan, and says so", titles.borrow().iter().any(|t| t.starts_with("Picking up")), format!("{:?}", titles.borrow()));
            }
            if done {
                verdict(&mut r, "the scan finished and the library holds the music", counts.1 > 0, counts.1);
                verdict(&mut r, "memory stays reasonable (peak working set under 1.5 GB)", peak.get() < 1536.0, format!("{:.0} MB", peak.get()));
                // On a slow disk, the music comes in long before the end.
                if tracks_at_start == 0 && secs > 10.0 {
                    let first = first_albums.get().unwrap_or(f64::MAX);
                    verdict(&mut r, "the first albums are on the screen within the first tenth of the scan", first <= secs * 0.1 + 1.0, format!("at {first:.1} s of {secs:.1} s"));
                }
            } else if let Some(q) = quit_at {
                verdict(&mut r, "albums were on the screen before the quit", first_albums.get().is_some(), format!("quit at {:.0}%", q * 100.0));
            }
            snapshot(&app, "bigscan-end");
            write_report("bigscan", &r);
            let _ = slint::quit_event_loop();
        });
    });
    std::mem::forget(sampler);
}

/// (seconds in, memory MB, handles, GDI objects, USER objects)
type Sample = (f64, f64, u32, u32, u32);

pub fn run_soak() {
    let secs: u64 = std::env::var("MP_BENCH_SOAK_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let t0 = Instant::now();
    let report: Rc<RefCell<String>> = Rc::default();
    let samples: Rc<RefCell<Vec<Sample>>> = Rc::default();
    let (lag, lag_timer) = lag_probe();
    let worst = Rc::new(Cell::new(Duration::ZERO));
    crate::state::with(|st| {
        let tracks: Vec<library::Track> = library::query::all_albums(&st.db, library::query::AlbumSort::Artist, false)
            .unwrap_or_default()
            .iter()
            .flat_map(|a| library::query::album_tracks(&st.db, a.id).unwrap_or_default())
            .filter(|t| t.duration >= 2.5)
            .collect();
        report.borrow_mut().push_str(&format!("soak: {} tracks on repeat for {secs} s\n", tracks.len()));
        playback::play_tracks(st, tracks, 0);
        playback::cycle_repeat(st);
    });
    let pages = [Page::NowPlaying, Page::Albums, Page::Home, Page::Artists, Page::Search];
    let tick = Rc::new(Cell::new(0u64));
    let timer = Rc::new(Timer::default());
    let t2 = timer.clone();
    timer.start(TimerMode::Repeated, Duration::from_millis(1500), move || {
        let _keep = &lag_timer;
        let n = tick.get() + 1;
        tick.set(n);
        worst.set(worst.get().max(lag.worst.replace(Duration::ZERO)));
        crate::state::with(|st| {
            let s = st.player.status();
            if let Some(d) = s.duration {
                st.player.seek((d - 0.8).max(0.0));
            }
            if n.is_multiple_of(7) {
                views::go(st, pages[(n / 7) as usize % pages.len()]);
            }
            let elapsed = t0.elapsed().as_secs_f64();
            if n.is_multiple_of(7) || elapsed >= secs as f64 {
                let (mem, _, handles) = crate::platform::system::resources();
                let (gdi, user) = gui_objects();
                samples.borrow_mut().push((elapsed, mem, handles, gdi, user));
                report.borrow_mut().push_str(&format!("{elapsed:>6.0} s  memory {mem:>6.1} MB  handles {handles:>5}  GDI {gdi:>4}  USER {user:>4}  track {:?} at {:.1} s\n", s.token, s.position));
            }
            if elapsed < secs as f64 {
                return;
            }
            t2.stop();
            playback::set_paused(st, true);
            let v = samples.borrow();
            // After a warm-up (caches filling, the first covers decoded): growth from there.
            let warm = v.iter().find(|s| s.0 >= (secs as f64 * 0.3).min(40.0)).copied().unwrap_or(v[0]);
            let last = *v.last().unwrap();
            let mut r = report.borrow_mut();
            verdict(&mut r, "memory does not keep growing", last.1 - warm.1 < 60.0, format!("{:.1} -> {:.1} MB", warm.1, last.1));
            verdict(&mut r, "handles do not leak", i64::from(last.2) - i64::from(warm.2) < 60, format!("{} -> {}", warm.2, last.2));
            verdict(&mut r, "GDI and USER objects do not leak", i64::from(last.3) - i64::from(warm.3) < 20 && i64::from(last.4) - i64::from(warm.4) < 20, format!("GDI {} -> {}, USER {} -> {}", warm.3, last.3, warm.4, last.4));
            let w = worst.get().as_secs_f64() * 1000.0;
            verdict(&mut r, "the UI thread is never stalled 250 ms or more", w < 250.0, format!("{w:.0} ms at worst"));
            verdict(&mut r, "still playing after every hand-over", st.play_failures.get() == 0 && s.token.is_some(), format!("failures {}", st.play_failures.get()));
            write_report("soak", &r);
            let _ = slint::quit_event_loop();
        });
    });
    std::mem::forget(timer);
}

#[cfg(windows)]
fn gui_objects() -> (u32, u32) {
    crate::platform::system::gui_objects()
}

#[cfg(not(windows))]
fn gui_objects() -> (u32, u32) {
    (0, 0)
}
