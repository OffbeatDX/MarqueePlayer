//! `MP_BENCH=visualizer`: the spectrum visualizer and the online lyrics, each part
//! snapshotted and reported (`bench-visualizer.txt`). Needs `MP_MUSIC` with the two
//! fixture albums: "Spectrum" (a log sweep at 48 kHz, three tones, pink noise at 44.1
//! kHz, a synthetic groove that drops 20 dB after 10 s, a sweep at 96 kHz) and "Words"
//! (tracks named for what a mock LRCLIB at `MP_LRCLIB_API` answers for them).
//!
//! - The visualizer: snapshots during each signal, the frame timer's intervals and the
//!   time each frame takes, the process CPU while it runs, which bars a tone lights,
//!   the fall to rest on a pause, and that the timer and the tap stop when paused, off
//!   the page and switched off (the `V` key, dispatched in the app).
//! - The lyrics: looking up, synced, plain, instrumental, not found, an HTTP error,
//!   offline, local lyrics winning, skipping through tracks, a cached answer, rejecting
//!   and trying again, and the lookup turned off. What the mock received is in its log.
//!
//! `MP_BENCH=viz-idle`: the visualizer runs for `MP_BENCH_VIZ_SECS` (default 20), its frames
//! checked to come at the rate in effect (the user's, or the power's: `MP_POWER`); then for 2 s
//! at the other rate, changed while it draws as an unplugged Mac changes it (checked for a
//! lost frame), then the track is paused, the window hidden and the app left idle for `MP_BENCH_IDLE_SECS`
//! (default 60); the report gives the process CPU time of each phase and when each began,
//! for an outside count of context switches.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model};

use crate::bench::{Step, run_steps, snapshot, type_text, write_report};
use crate::state::State;
use crate::{AppWindow, Nav, Page, Visualizer, playback, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("visualizer bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

/// This process's CPU time (user + kernel; `platform::system`).
fn cpu_time() -> Duration {
    crate::platform::system::cpu_time()
}

fn album_tracks(st: &Rc<State>, title: &str) -> Vec<library::Track> {
    let id: Option<i64> = st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok();
    id.map(|a| library::query::album_tracks(&st.db, a).unwrap_or_default()).unwrap_or_default()
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// The frame timer's intervals and the work per frame since recording began.
fn frame_report(name: &str, stats: &[(Instant, Duration)], cpu: Duration, wall: Duration) -> String {
    let mut iv: Vec<f64> = stats.windows(2).map(|w| (w[1].0 - w[0].0).as_secs_f64() * 1000.0).collect();
    let mut work: Vec<f64> = stats.iter().map(|s| s.1.as_secs_f64() * 1000.0).collect();
    iv.sort_by(f64::total_cmp);
    work.sort_by(f64::total_cmp);
    format!(
        "{name}: {} frames in {:.2} s ({:.1} fps)\n  frame interval ms: p50 {:.2}  p95 {:.2}  p99 {:.2}  max {:.2}\n  analysis + paths per frame ms: p50 {:.3}  p95 {:.3}  max {:.3}\n  process CPU: {:.1}% of one core ({:.0} ms in {:.1} s)",
        stats.len(),
        wall.as_secs_f64(),
        stats.len() as f64 / wall.as_secs_f64().max(1e-9),
        pct(&iv, 0.5),
        pct(&iv, 0.95),
        pct(&iv, 0.99),
        iv.last().copied().unwrap_or(0.0),
        pct(&work, 0.5),
        pct(&work, 0.95),
        work.last().copied().unwrap_or(0.0),
        100.0 * cpu.as_secs_f64() / wall.as_secs_f64().max(1e-9),
        cpu.as_secs_f64() * 1000.0,
        wall.as_secs_f64(),
    )
}

/// The loudest bars, as "bar (centre Hz) level".
fn top_bars(n: usize) -> String {
    let levels = crate::visualizer::levels();
    let centres = player_core::spectrum::Analyzer::new(Default::default()).centres(levels.len());
    let mut idx: Vec<usize> = (0..levels.len()).collect();
    idx.sort_by(|a, b| levels[*b].total_cmp(&levels[*a]));
    idx.iter().take(n).map(|&i| format!("bar {i} ({:.0} Hz) {:.2}", centres[i], levels[i])).collect::<Vec<_>>().join(", ")
}

/// Whether the loudest bar is within a third of an octave of the sweep's frequency at the
/// position being heard (the engine's position, not the time since the steps began: the
/// device starts a few hundred milliseconds after the call, and a 96 kHz track reopens it).
fn sweep_check(r: &Report, what: &str, pos: f64, secs: f64) {
    let want = 20.0 * 1000f64.powf(pos / secs);
    let levels = crate::visualizer::levels();
    let centres = player_core::spectrum::Analyzer::new(Default::default()).centres(levels.len());
    let top = (0..levels.len()).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).map(|i| f64::from(centres[i])).unwrap_or(0.0);
    let octaves = (top / want).log2().abs();
    let ok = top > 0.0 && octaves <= 1.0 / 3.0;
    if !ok {
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: the loudest bar at {top:.0} Hz, the sweep at {want:.0} Hz ({pos:.2} s in), {octaves:.2} octaves apart", if ok { "ok" } else { "FAIL" }));
}

fn mean_level() -> f32 {
    let l = crate::visualizer::levels();
    l.iter().sum::<f32>() / l.len().max(1) as f32
}

fn state_line(app: &AppWindow) -> String {
    let (timer, tap) = crate::visualizer::running();
    format!("timer {timer}, tap {tap}, drawing {}", app.global::<Visualizer>().get_active())
}

fn lyrics_line(app: &AppWindow) -> String {
    let l = app.global::<crate::Lyrics>();
    format!("online {:?}, {} lines, synced {}, status {:?}", l.get_online().to_string(), l.get_lines().row_count(), l.get_synced(), l.get_status().to_string())
}

pub fn run() {
    let r: Report = Rc::default();
    let t0 = Rc::new(RefCell::new((Instant::now(), Duration::ZERO)));
    let mut steps: Vec<Step> = Vec::new();
    let mark = |t0: &Rc<RefCell<(Instant, Duration)>>| {
        *t0.borrow_mut() = (Instant::now(), cpu_time());
        crate::visualizer::record(true);
    };

    // ---- The visualizer -------------------------------------------------------------
    {
        let (r, t0) = (r.clone(), t0.clone());
        steps.push((500, Box::new(move |st, app| {
            let tracks = album_tracks(st, "Spectrum");
            line(&r, format!("[setup] Spectrum: {} tracks; window {:?} at scale {}", tracks.len(), app.window().size(), app.window().scale_factor()));
            playback::play_tracks(st, tracks, 0);
            views::go(st, Page::NowPlaying);
            mark(&t0);
        })));
    }
    steps.push((1500, Box::new(|_, app| {
        snapshot(app, "viz-01-sweep-1.5s");
    })));
    steps.push((3500, Box::new(|_, app| {
        snapshot(app, "viz-02-sweep-5s");
    })));
    {
        let (r, t0) = (r.clone(), t0.clone());
        steps.push((3000, Box::new(move |st, app| {
            snapshot(app, "viz-03-sweep-8s");
            sweep_check(&r, "the 48 kHz sweep lights the bars of its frequency", st.player.status().position, 20.0);
            let (start, cpu) = *t0.borrow();
            let stats = crate::visualizer::take_stats();
            line(&r, frame_report("[viz] sweep, 48 kHz, 8 s", &stats, cpu_time() - cpu, start.elapsed()));
            line(&r, format!("[viz] sweep at 8 s (about {:.0} Hz): {}", 20.0 * 1000f64.powf(8.0 / 20.0), top_bars(3)));
            line(&r, format!("[viz] {}", state_line(app)));
        })));
    }
    steps.push((100, Box::new(|st, _| playback::next(st))));
    {
        let r = r.clone();
        steps.push((2500, Box::new(move |_, app| {
            snapshot(app, "viz-04-three-tones");
            line(&r, format!("[viz] three tones (100 Hz -12 dB, 1 kHz -18 dB, 5 kHz -24 dB): loudest {}", top_bars(6)));
        })));
    }
    steps.push((100, Box::new(|st, _| playback::next(st))));
    {
        let r = r.clone();
        steps.push((2500, Box::new(move |_, app| {
            snapshot(app, "viz-05-pink-noise-44k");
            let l = crate::visualizer::levels();
            let (min, max) = l.iter().fold((1f32, 0f32), |(a, b), &v| (a.min(v), b.max(v)));
            let third = l.len() / 3;
            let avg = |s: &[f32]| s.iter().sum::<f32>() / s.len().max(1) as f32;
            line(&r, format!(
                "[viz] pink noise, 44.1 kHz: levels min {min:.2} max {max:.2}; mean of the low / mid / high third {:.2} / {:.2} / {:.2}",
                avg(&l[..third]),
                avg(&l[third..2 * third]),
                avg(&l[2 * third..])
            ));
        })));
    }
    steps.push((100, Box::new(|st, _| playback::next(st))));
    let loud = Rc::new(RefCell::new(Vec::<f32>::new()));
    for (delay, name) in [(1500, "viz-06-groove-a"), (150, "viz-06-groove-b"), (150, "viz-06-groove-c"), (2000, "viz-06-groove-d")] {
        let loud = loud.clone();
        steps.push((delay, Box::new(move |_, app| {
            snapshot(app, name);
            loud.borrow_mut().push(mean_level());
        })));
    }
    // Sample the quiet passage (after 10 s) a few times too.
    let quiet = Rc::new(RefCell::new(Vec::<f32>::new()));
    steps.push((7500, Box::new(|_, app| {
        snapshot(app, "viz-07-groove-quiet");
    })));
    for _ in 0..6 {
        let quiet = quiet.clone();
        steps.push((150, Box::new(move |_, _| quiet.borrow_mut().push(mean_level()))));
    }
    {
        let (r, loud, quiet) = (r.clone(), loud.clone(), quiet.clone());
        steps.push((10, Box::new(move |_, _| {
            let f = |v: &[f32]| v.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(" ");
            line(&r, format!("[viz] groove mean level, loud part: {}; 20 dB quieter part: {}", f(&loud.borrow()), f(&quiet.borrow())));
        })));
    }
    steps.push((100, Box::new(|st, _| playback::next(st))));
    {
        let (r2, t02) = (r.clone(), t0.clone());
        let t01 = t0.clone();
        steps.push((300, Box::new(move |_, _| mark(&t01))));
        steps.push((2500, Box::new(move |st, app| {
            snapshot(app, "viz-08-sweep-96k");
            sweep_check(&r2, "the 96 kHz sweep lights the bars of its frequency", st.player.status().position, 10.0);
            let (start, cpu) = *t02.borrow();
            let stats = crate::visualizer::take_stats();
            line(&r2, frame_report("[viz] sweep, 96 kHz, 2.5 s", &stats, cpu_time() - cpu, start.elapsed()));
            line(&r2, format!("[viz] tap rate {} Hz; output {:?}; at 2.8 s (about {:.0} Hz): {}", st.player.spectrum().rate(), st.player.output_info().map(|o| o.device), 20.0 * 1000f64.powf(2.8 / 10.0), top_bars(3)));
        })));
    }
    // Pause: the bars fall, then everything stops.
    {
        let r = r.clone();
        let t_pause = Rc::new(RefCell::new(Instant::now()));
        let tp = t_pause.clone();
        steps.push((100, Box::new(move |st, _| {
            *tp.borrow_mut() = Instant::now();
            playback::set_paused(st, true);
        })));
        steps.push((150, Box::new(|_, app| {
            snapshot(app, "viz-09-paused-150ms");
        })));
        steps.push((400, Box::new(|_, app| {
            snapshot(app, "viz-10-paused-550ms");
        })));
        // Poll until it stops (each poll is this bench's own timer, not the app's).
        let r2 = r.clone();
        let tp2 = t_pause.clone();
        steps.push((2000, Box::new(move |_, app| {
            line(&r2, format!("[viz] paused 2.55 s: {}", state_line(app)));
            let _ = tp2;
        })));
        let r3 = r.clone();
        let t_pause2 = t_pause.clone();
        steps.push((10, Box::new(move |_, _| {
            let c = cpu_time();
            *t_pause2.borrow_mut() = Instant::now();
            let r3 = r3.clone();
            let tp = t_pause2.clone();
            slint::Timer::single_shot(Duration::from_secs(5), move || {
                line(&r3, format!("[viz] paused, window shown, on now playing, 5 s: process CPU {:.0} ms ({:.2}% of one core)", (cpu_time() - c).as_secs_f64() * 1000.0, 100.0 * (cpu_time() - c).as_secs_f64() / tp.borrow().elapsed().as_secs_f64()));
            });
        })));
    }
    steps.push((5200, Box::new(|st, _| playback::set_paused(st, false))));
    {
        let r1 = r.clone();
        steps.push((1000, Box::new(move |st, app| {
            line(&r1, format!("[viz] resumed: {}", state_line(app)));
            views::go(st, Page::Home);
        })));
        let r2 = r.clone();
        steps.push((500, Box::new(move |st, app| {
            line(&r2, format!("[viz] on Home, playing: {}", state_line(app)));
            views::go(st, Page::NowPlaying);
        })));
        let r3 = r.clone();
        steps.push((500, Box::new(move |_, app| {
            line(&r3, format!("[viz] back on now playing: {}", state_line(app)));
            // The V key, through Slint's own dispatch.
            type_text(app, "v");
        })));
        let r4 = r.clone();
        steps.push((500, Box::new(move |st, app| {
            line(&r4, format!("[viz] after V: setting {}, {}; pill {:?}", st.settings.borrow().visualizer, state_line(app), app.global::<Nav>().get_flash().to_string()));
            snapshot(app, "viz-11-off");
            type_text(app, "v");
        })));
        let r5 = r.clone();
        steps.push((900, Box::new(move |st, app| {
            line(&r5, format!("[viz] V again: setting {}, {}", st.settings.borrow().visualizer, state_line(app)));
            snapshot(app, "viz-12-on-again");
            app.global::<crate::Lyrics>().set_shown(true);
        })));
        let r6 = r.clone();
        steps.push((800, Box::new(move |_, app| {
            snapshot(app, "viz-13-with-lyrics-tab");
            line(&r6, format!("[lyrics] a Spectrum track, lookup off: {}", lyrics_line(app)));
        })));
    }

    // ---- Online lyrics (against the mock) ---------------------------------------------
    {
        let r = r.clone();
        steps.push((300, Box::new(move |st, app| {
            st.settings.borrow_mut().lyrics_online = true;
            views::refresh_player_settings(st);
            crate::lyrics::set_enabled(st, true);
            let words = album_tracks(st, "Words");
            line(&r, format!("[lyrics] lookup on; Words: {} tracks; MP_LRCLIB_API {:?}", words.len(), std::env::var("MP_LRCLIB_API").ok()));
            playback::play_tracks(st, words, 0);
            app.global::<crate::Lyrics>().set_shown(true);
        })));
    }
    {
        let r = r.clone();
        steps.push((1000, Box::new(move |_, app| {
            line(&r, format!("[lyrics] Night Drive at 1 s: {}", lyrics_line(app)));
            snapshot(app, "lyr-01-looking");
        })));
    }
    let names: [(&'static str, &'static str); 6] = [
        ("Night Drive", "lyr-02-synced"),
        ("Paper Lantern", "lyr-03-plain"),
        ("Interlude", "lyr-04-instrumental"),
        ("Lost Signal", "lyr-05-not-found"),
        ("Static", "lyr-06-http-error"),
        ("Dropout", "lyr-07-offline"),
    ];
    for (i, (name, snap)) in names.into_iter().enumerate() {
        let r = r.clone();
        steps.push((if i == 0 { 2800 } else { 3600 }, Box::new(move |st, app| {
            line(&r, format!("[lyrics] {name} at 3.6 s: {}", lyrics_line(app)));
            let want = ["found", "found", "instrumental", "not-found", "failed", "offline"][i];
            let got = app.global::<crate::Lyrics>().get_online().to_string();
            let ok = got == want && (i != 0 || app.global::<crate::Lyrics>().get_synced());
            if !ok {
                crate::bench::count_fail();
            }
            line(&r, format!("  [{}] the mock's answer for {name} shows as {want}: {got}", if ok { "ok" } else { "FAIL" }));
            snapshot(app, snap);
            playback::next(st);
        })));
    }
    {
        let r = r.clone();
        steps.push((1500, Box::new(move |st, app| {
            line(&r, format!("[lyrics] With Its Own (an .lrc beside it): {}", lyrics_line(app)));
            snapshot(app, "lyr-08-local-wins");
            playback::next(st);
        })));
    }
    // Skipping through: Skip One, Two, Three, 300 ms apart; Skip Four stays.
    for _ in 0..3 {
        steps.push((300, Box::new(|st, _| playback::next(st))));
    }
    {
        let r = r.clone();
        steps.push((3600, Box::new(move |st, app| {
            line(&r, format!("[lyrics] Skip Four after skipping (see the mock's log: only Skip Four asked): {}", lyrics_line(app)));
            // Night Drive again: cached, so at once and without a request.
            let words = album_tracks(st, "Words");
            playback::play_tracks(st, words, 0);
        })));
    }
    {
        let r = r.clone();
        steps.push((400, Box::new(move |st, app| {
            line(&r, format!("[lyrics] Night Drive again, 0.4 s in (cached): {}", lyrics_line(app)));
            crate::lyrics::reject(st);
        })));
    }
    {
        let r = r.clone();
        steps.push((400, Box::new(move |st, app| {
            line(&r, format!("[lyrics] after Wrong lyrics?: {}", lyrics_line(app)));
            snapshot(app, "lyr-09-rejected");
            crate::lyrics::retry(st);
        })));
    }
    {
        let r = r.clone();
        steps.push((2500, Box::new(move |st, app| {
            line(&r, format!("[lyrics] after Look up again (the mock offers only the rejected record): {}", lyrics_line(app)));
            snapshot(app, "lyr-10-retried");
            st.settings.borrow_mut().lyrics_online = false;
            views::refresh_player_settings(st);
            crate::lyrics::set_enabled(st, false);
            let words = album_tracks(st, "Words");
            let last = words.len().saturating_sub(1);
            playback::play_tracks(st, words, last);
        })));
    }
    {
        let r = r.clone();
        steps.push((3600, Box::new(move |st, app| {
            line(&r, format!("[lyrics] Quiet Off with the lookup off, 3.6 s in (the mock must not see it): {}", lyrics_line(app)));
            snapshot(app, "lyr-11-lookup-off");
            views::go(st, Page::Settings);
        })));
    }
    // Settings > Visualizer (the lyrics switch is in Settings > Lyrics now).
    steps.push((500, Box::new(move |st, _| {
        views::settings_section(st, crate::settings::nav::VISUALIZER);
    })));
    steps.push((400, Box::new(move |_, app| {
        snapshot(app, "set-01-now-playing");
    })));
    steps.push((300, Box::new(move |_, _| {
        write_report("visualizer", &r.borrow());
        let _ = slint::quit_event_loop();
    })));
    run_steps(steps);
}

/// The frames came at the rate in effect: the user's, or under Auto the power's (`MP_POWER`
/// states it: `battery` draws 30 a second on macOS). The median interval near 33 ms at 30;
/// under 25 ms at up to 60 (the display's rate, at most 60).
fn rate_check(r: &Report, st: &Rc<State>, stats: &[(Instant, Duration)]) {
    use crate::visualizer::look::{Fps, fps_in_effect};
    let s = crate::visualizer::settings(st);
    let power = crate::platform::system::power();
    let fps = fps_in_effect(s.chosen_fps(), power);
    let mut iv: Vec<f64> = stats.windows(2).map(|w| (w[1].0 - w[0].0).as_secs_f64() * 1000.0).collect();
    iv.sort_by(f64::total_cmp);
    let p50 = pct(&iv, 0.5);
    let ok = !iv.is_empty() && if fps == Fps::Half { (28.0..40.0).contains(&p50) } else { p50 < 25.0 };
    if !ok {
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] the frames come at the rate in effect: {fps:?} (chosen {:?}, power {power:?}); interval p50 {p50:.2} ms", if ok { "ok" } else { "FAIL" }, s.chosen_fps()));
}

pub fn run_idle() {
    let viz: u64 = std::env::var("MP_BENCH_VIZ_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let idle: u64 = std::env::var("MP_BENCH_IDLE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let r: Report = Rc::default();
    let begin = Instant::now();
    let unix = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let c0 = Rc::new(RefCell::new(Duration::ZERO));
    let was = Rc::new(RefCell::new(None));
    let mut steps: Vec<Step> = Vec::new();
    {
        let (r, c0) = (r.clone(), c0.clone());
        steps.push((500, Box::new(move |st, _| {
            playback::play_tracks(st, album_tracks(st, "Spectrum"), 3);
            views::go(st, Page::NowPlaying);
            *c0.borrow_mut() = cpu_time();
            crate::visualizer::record(true);
            line(&r, format!("phase running: starts at unix {:.1}, pid {}", unix(), std::process::id()));
        })));
    }
    {
        let (r, c0, was) = (r.clone(), c0.clone(), was.clone());
        steps.push((viz * 1000, Box::new(move |st, app| {
            let stats = crate::visualizer::take_stats();
            let wall = stats.last().zip(stats.first()).map_or(Duration::from_secs(1), |(l, f)| l.0 - f.0);
            line(&r, frame_report("visualizer running, playing, now playing shown", &stats, cpu_time() - *c0.borrow(), wall));
            rate_check(&r, st, &stats);
            // The rate in effect changes while it draws, as when the Mac is unplugged or
            // plugged in: the other rate, through the handler the power's notification calls.
            let s = crate::visualizer::settings(st);
            let now = crate::visualizer::look::fps_in_effect(s.chosen_fps(), crate::platform::system::power());
            let other = if now == crate::visualizer::look::Fps::Half { crate::visualizer::look::Fps::Full } else { crate::visualizer::look::Fps::Half };
            *was.borrow_mut() = Some(st.settings.borrow().visualizer_look.clone());
            st.settings.borrow_mut().visualizer_look = Some(crate::visualizer::look::VizSettings { fps: other, fps_chosen: true, ..s });
            crate::visualizer::record(true);
            crate::visualizer::power_changed(st);
            let _ = app;
        })));
    }
    {
        let (r, was) = (r.clone(), was.clone());
        steps.push((2000, Box::new(move |st, _| {
            let stats = crate::visualizer::take_stats();
            rate_check(&r, st, &stats);
            // No frame lost at the change: every interval under two of the slower rate's.
            let max = stats.windows(2).map(|w| (w[1].0 - w[0].0).as_secs_f64() * 1000.0).fold(0.0, f64::max);
            let ok = stats.len() > 10 && max < 2.0 * 33.4;
            if !ok {
                crate::bench::count_fail();
            }
            line(&r, format!("  [{}] the rate changed while drawing without a gap: {} frames, the longest interval {max:.2} ms", if ok { "ok" } else { "FAIL" }, stats.len()));
            if let Some(v) = was.borrow_mut().take() {
                st.settings.borrow_mut().visualizer_look = v;
            }
            crate::visualizer::power_changed(st);
            playback::set_paused(st, true);
        })));
    }
    {
        let (r, c0) = (r.clone(), c0.clone());
        steps.push((3000, Box::new(move |st, app| {
            let _ = app.hide();
            *c0.borrow_mut() = cpu_time();
            line(&r, format!("phase idle (paused, window hidden): starts at unix {:.1}; engine {:?}; {}", unix(), st.player.status().state, state_line(app)));
            write_report("viz-idle", &r.borrow());
        })));
    }
    {
        let (r, c0) = (r.clone(), c0.clone());
        steps.push((idle * 1000, Box::new(move |_, _| {
            line(&r, format!("phase idle: process CPU {:.1} ms in {idle} s; ends at unix {:.1} ({:.0} s after start)", (cpu_time() - *c0.borrow()).as_secs_f64() * 1000.0, unix(), begin.elapsed().as_secs_f64()));
            write_report("viz-idle", &r.borrow());
            let _ = slint::quit_event_loop();
        })));
    }
    run_steps(steps);
}
