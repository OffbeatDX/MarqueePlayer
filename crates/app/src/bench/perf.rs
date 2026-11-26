//! `MP_BENCH=perf`: the scenes `cargo xtask perf` compares two builds on (docs/testing.md,
//! "Comparing two builds: cargo xtask perf"). Each scene is a steady state of the real app,
//! entered, left to settle for `MP_PERF_SETTLE_MS` (default 2500) and measured for
//! `MP_PERF_SECS` (default 8) seconds; `MP_PERF_SCENES` names them in order (comma-separated):
//!
//! - `home`: music playing on Home, nothing else moving;
//! - `np-off`: Now playing with the visualizer off;
//! - `bars`: the default visualizer (Classic, bars) behind the page;
//! - `halo`: the halo behind the page;
//! - `grid`: All albums scrolled up and down at 2,400 px a second;
//! - `prep`: nothing measured: every album's cover brought on screen once and Home shown, so
//!   a data folder copied after it has its covers in the cache (the command's seed).
//!
//! It writes one line of JSON per event to standard output, each after `MP-PERF ` (the log's
//! lines are on standard error): `hello` (the window, the display's refresh), `start` and
//! `end` of each measured span (`end`: the visualizer's frames, their spacing and work, the
//! window's own frames when the rendering notifier is called, the process's CPU time and
//! memory), then `done`. The command reads the window server and the GPU from outside over
//! the same span, between `start` and `end`.
//!
//! `MP_PERF_SOUND=made-up`: the visualizer hears a sound made up from its frame number instead
//! of the tap, a frame's time step fixed at 1/60 s (`made_up`, `frame_dt`, called from
//! `visualizer::frame`), so two builds draw the same frames whatever the timing; the frames
//! that were made up are counted in `end` (none: the build's visualizer does not call it).
//! `MP_PERF_SNAP=<n>`: in the visualizer's scenes, the window's own picture
//! (`take_snapshot`) after the n-th frame of made-up sound, `perf-<scene>-<n>.png` and the
//! same pixels raw (`.rgba`, for the command to compare) in `MP_BENCH_OUT`.
//!
//! The command builds this file into a base that has none (`cargo xtask perf`, "grafted"), so
//! it uses only what the app had when it was written: keep it that way, or the command's
//! graft into older bases stops compiling.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::bench::{Frames, Step, run_steps, start_recording, stop_recording};
use crate::state::State;
use crate::visualizer::look::{Placement, Preset};
use crate::{AppWindow, Grid, Page, Visualizer, playback, views};

/// The version of the lines written, for the command to refuse a build it cannot read.
const VERSION: u32 = 1;

fn emit(v: serde_json::Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "MP-PERF {v}");
    let _ = out.flush();
}

fn now_unix() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

// ---- The made-up sound ----------------------------------------------------------------------

static MADE_UP: AtomicBool = AtomicBool::new(false);
/// Frames of made-up sound since the scene began (the frame number the sound is made from).
static MADE_UP_N: AtomicU64 = AtomicU64::new(0);
/// The frame after which `MP_PERF_SNAP` takes the window's picture, and whether it was taken.
static SNAP_AT: AtomicU64 = AtomicU64::new(u64::MAX);
static SNAP_ARMED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static SNAP_NAME: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The visualizer's time step for a frame: 1/60 s with the made-up sound, so the looks move
/// by the frame and not by the clock; `dt` otherwise.
pub(crate) fn frame_dt(dt: f32) -> f32 {
    if MADE_UP.load(Ordering::Relaxed) { 1.0 / 60.0 } else { dt }
}

/// With the made-up sound on, overwrites the samples the visualizer read from the tap with
/// the made-up sound's window ending at this frame (a function of the frame number and the
/// rate only), and counts the frame. Off: leaves them as they are.
pub(crate) fn made_up(samples: &mut [f32], rate: u32) {
    if !MADE_UP.load(Ordering::Relaxed) {
        return;
    }
    let n = MADE_UP_N.fetch_add(1, Ordering::Relaxed);
    let rate = if rate == 0 { 48_000 } else { rate };
    let hop = u64::from(rate) / 60;
    let len = samples.len() as u64;
    let end = (n + 1) * hop;
    for (i, s) in samples.iter_mut().enumerate() {
        // The window ends at this frame's time; before the start, silence.
        let t = (end + i as u64).checked_sub(len);
        *s = t.map_or(0.0, |t| sound_at(t, rate));
    }
    if n == SNAP_AT.load(Ordering::Relaxed) && SNAP_ARMED.swap(false, Ordering::Relaxed) {
        // After this frame is drawn into the window's properties, before the next one.
        Timer::single_shot(Duration::ZERO, || {
            let name = SNAP_NAME.with(|n| n.borrow_mut().take());
            if let (Some(name), Some(app)) = (name, crate::state::with(|st| st.app.upgrade()).flatten())
                && let Some(img) = super::snapshot(&app, &name)
            {
                // And the pixels as they are, for the command to compare without a PNG
                // decoder: `RGBA`, the width and height (u32, little-endian), the pixels.
                let mut raw = b"RGBA".to_vec();
                raw.extend(img.width().to_le_bytes());
                raw.extend(img.height().to_le_bytes());
                raw.extend_from_slice(img.as_raw());
                let _ = std::fs::write(super::out_dir().join(format!("{name}.rgba")), raw);
            }
        });
    }
}

/// The made-up sound at sample `t`: a kick every half second (120 a minute), a bass line, a
/// chord that changes every two seconds and hi-hat noise on the off-beats, every part a
/// function of `t` alone (the noise a hash of it), so any build hears the same at frame n.
pub(crate) fn sound_at(t: u64, rate: u32) -> f32 {
    use std::f32::consts::TAU;
    let r = rate as f32;
    let secs = t as f32 / r;
    let beat = (t % (u64::from(rate) / 2)) as f32 / r;
    let kick = (-beat * 18.0).exp() * (TAU * (50.0 + 60.0 * (-beat * 30.0).exp()) * beat).sin();
    let bar = (t / (u64::from(rate) * 2)) % 4;
    let root = [110.0, 98.0, 130.8, 87.3][bar as usize];
    let bass = 0.35 * (TAU * root * secs).sin() * (0.6 + 0.4 * (TAU * 2.0 * secs).sin());
    let chord: f32 = [1.0, 1.26, 1.5, 2.0].iter().map(|m| (TAU * root * 4.0 * m * secs).sin()).sum::<f32>() * 0.04;
    let off = ((t + u64::from(rate) / 4) % (u64::from(rate) / 2)) as f32 / r;
    let mut h = t.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    let noise = (h & 0xFFFF) as f32 / 32_768.0 - 1.0;
    let hat = 0.12 * (-off * 60.0).exp() * noise;
    (0.7 * kick + bass + chord + hat).clamp(-1.0, 1.0)
}

// ---- The scenes -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Scene {
    Home,
    NowPlayingOff,
    Bars,
    Halo,
    Grid,
    Prep,
}

impl Scene {
    pub(crate) const ALL: [Scene; 6] = [Scene::Home, Scene::NowPlayingOff, Scene::Bars, Scene::Halo, Scene::Grid, Scene::Prep];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Scene::Home => "home",
            Scene::NowPlayingOff => "np-off",
            Scene::Bars => "bars",
            Scene::Halo => "halo",
            Scene::Grid => "grid",
            Scene::Prep => "prep",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Scene> {
        Scene::ALL.into_iter().find(|s| s.id() == id.trim())
    }

    fn visualizer(self) -> Option<Preset> {
        match self {
            Scene::Bars => Some(Preset::Classic),
            Scene::Halo => Some(Preset::Halo),
            _ => None,
        }
    }
}

/// `MP_PERF_SCENES`, in order; unknown names are said in the `hello` line and left out.
pub(crate) fn scenes_from(v: &str) -> (Vec<Scene>, Vec<String>) {
    let (mut ok, mut unknown) = (Vec::new(), Vec::new());
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match Scene::from_id(part) {
            Some(s) => ok.push(s),
            None => unknown.push(part.to_owned()),
        }
    }
    (ok, unknown)
}

fn album(st: &Rc<State>, title: &str) -> Vec<library::Track> {
    let id: Option<i64> = st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok();
    id.map(|a| library::query::album_tracks(&st.db, a).unwrap_or_default()).unwrap_or_default()
}

fn set_visualizer(st: &Rc<State>, on: bool) {
    if st.settings.borrow().visualizer != on {
        crate::toggle_visualizer(st);
    }
}

fn show(st: &Rc<State>, app: &AppWindow, preset: Preset) {
    let i = Preset::ALL.iter().position(|p| *p == preset).unwrap_or(0);
    crate::visualizer::choose_preset(st, i);
    let pi = Placement::ALL.iter().position(|p| *p == Placement::Behind).unwrap_or(0);
    crate::visualizer::choose(st, "placement", pi);
    app.global::<Visualizer>().set_tab(false);
    crate::visualizer::wake(st);
}

/// The display's refresh rate (Hz), as the visualizer's timer reads it.
fn refresh_hz(app: &AppWindow) -> Option<u32> {
    app.window().with_winit_window(|w| w.current_monitor().and_then(|m| m.refresh_rate_millihertz())).flatten().map(|mhz| (mhz + 500) / 1000)
}

/// The process's memory now, in MB: the physical footprint on macOS (what Activity Monitor
/// shows), else what `platform::system::resources` gives (private bytes on Windows, the
/// resident size on Linux).
fn memory_mb() -> f64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pid_rusage fills the struct it is given for this process's own pid.
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast()) };
        if rc == 0 {
            return info.ri_phys_footprint as f64 / 1_048_576.0;
        }
    }
    crate::platform::system::resources().0
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// The figures of a span of frames (when each began or was presented, and the work or render
/// time it took): count, a second, the spacing's p50, p95 and max, the frames later than
/// one and a half refreshes after the one before, the work's p50 and p95 and sum (ms).
pub(crate) fn frame_figures(prefix: &str, frames: &[(Instant, Duration)], secs: f64, refresh: f64) -> serde_json::Map<String, serde_json::Value> {
    let mut iv: Vec<f64> = frames.windows(2).map(|w| (w[1].0 - w[0].0).as_secs_f64() * 1000.0).collect();
    let mut work: Vec<f64> = frames.iter().map(|f| f.1.as_secs_f64() * 1000.0).collect();
    iv.sort_by(f64::total_cmp);
    work.sort_by(f64::total_cmp);
    let late_ms = 1.5 * 1000.0 / refresh.max(1.0);
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: serde_json::Value| {
        m.insert(format!("{prefix}{k}"), v);
    };
    put("frames", frames.len().into());
    put("fps", (frames.len() as f64 / secs.max(1e-9)).into());
    put("interval_p50_ms", pct(&iv, 0.5).into());
    put("interval_p95_ms", pct(&iv, 0.95).into());
    put("interval_max_ms", iv.last().copied().unwrap_or(0.0).into());
    put("late", iv.iter().filter(|&&i| i > late_ms).count().into());
    put("work_p50_ms", pct(&work, 0.5).into());
    put("work_p95_ms", pct(&work, 0.95).into());
    put("work_sum_ms", work.iter().sum::<f64>().into());
    m
}

/// See the module's comment.
pub(super) fn run(frames: Rc<RefCell<Frames>>) {
    let (scenes, unknown) = scenes_from(&std::env::var("MP_PERF_SCENES").unwrap_or_else(|_| "home,bars".into()));
    let settle = env_u64("MP_PERF_SETTLE_MS", 2500);
    let secs = env_u64("MP_PERF_SECS", 8);
    let made_up_sound = std::env::var("MP_PERF_SOUND").as_deref() == Ok("made-up");
    let refresh: Rc<Cell<f64>> = Rc::new(Cell::new(60.0));
    let names: Vec<&'static str> = scenes.iter().map(|s| s.id()).collect();
    let mut steps: Vec<Step> = Vec::new();
    {
        let refresh = refresh.clone();
        steps.push((1200, Box::new(move |st, app| {
            let (size, scale) = (app.window().size(), app.window().scale_factor());
            let hz = refresh_hz(app);
            refresh.set(f64::from(hz.unwrap_or(60)));
            emit(serde_json::json!({
                "event": "hello", "version": VERSION, "unix": now_unix(),
                "window_w": size.width, "window_h": size.height, "scale": scale,
                "refresh_hz": hz, "made_up_sound": made_up_sound, "scenes": names,
                "unknown_scenes": unknown, "albums": app.global::<Grid>().get_total(),
                "settle_ms": settle, "secs": secs,
            }));
            let tracks = album(st, "Night Shift");
            if tracks.is_empty() {
                emit(serde_json::json!({"event": "error", "what": "the album \"Night Shift\" is not in the library (MP_MUSIC: the visualizer's fixtures)"}));
            }
            playback::play_tracks(st, tracks, 0);
            // Repeat the album: the music never runs out, however many scenes.
            playback::cycle_repeat(st);
        })));
    }
    let scroller = Rc::new(Timer::default());
    for scene in scenes {
        let s1 = scroller.clone();
        steps.push((300, Box::new(move |st, app| enter(st, app, scene, &s1))));
        if scene == Scene::Prep {
            prep(&mut steps);
            continue;
        }
        let t0 = Rc::new(Cell::new((Instant::now(), Duration::ZERO, 0u64)));
        {
            let (t0, frames) = (t0.clone(), frames.clone());
            steps.push((settle, Box::new(move |_st, _app| {
                emit(serde_json::json!({"event": "start", "scene": scene.id(), "unix": now_unix()}));
                crate::visualizer::record(true);
                start_recording(&frames);
                t0.set((Instant::now(), crate::platform::system::cpu_time(), MADE_UP_N.load(Ordering::Relaxed)));
            })));
        }
        let (frames, refresh, scroller) = (frames.clone(), refresh.clone(), scroller.clone());
        steps.push((secs * 1000, Box::new(move |_st, app| {
            let (at, cpu, made0) = t0.get();
            let wall = at.elapsed().as_secs_f64();
            let cpu_s = (crate::platform::system::cpu_time().saturating_sub(cpu)).as_secs_f64();
            let viz = crate::visualizer::take_stats();
            crate::visualizer::record(false);
            let win = stop_recording(&frames);
            let mut m = serde_json::Map::new();
            m.insert("event".into(), "end".into());
            m.insert("scene".into(), scene.id().into());
            m.insert("unix".into(), now_unix().into());
            m.insert("secs".into(), wall.into());
            m.insert("cpu_s".into(), cpu_s.into());
            m.insert("mem_mb".into(), memory_mb().into());
            m.insert("made_up_frames".into(), (MADE_UP_N.load(Ordering::Relaxed) - made0).into());
            m.extend(frame_figures("viz_", &viz, wall, refresh.get()));
            m.extend(frame_figures("win_", &win, wall, refresh.get()));
            if scene == Scene::Grid {
                scroller.stop();
                m.insert("grid_y".into(), f64::from(app.global::<Grid>().get_content_y()).into());
                app.global::<Grid>().set_content_y(0.0);
            }
            if SNAP_ARMED.swap(false, Ordering::Relaxed) {
                m.insert("snap_missed".into(), true.into());
            }
            emit(serde_json::Value::Object(m));
        })));
    }
    steps.push((300, Box::new(|_st, _app| {
        emit(serde_json::json!({"event": "done", "unix": now_unix()}));
        let _ = slint::quit_event_loop();
    })));
    run_steps(steps);
}

fn enter(st: &Rc<State>, app: &AppWindow, scene: Scene, scroller: &Rc<Timer>) {
    match scene {
        Scene::Home => views::go(st, Page::Home),
        Scene::NowPlayingOff => {
            set_visualizer(st, false);
            views::go(st, Page::NowPlaying);
        }
        Scene::Bars | Scene::Halo => {
            // Off and on again: the visualizer starts from rest (its analysis, its beat, its
            // looks), so with the made-up sound its frame n is the same in any build.
            set_visualizer(st, false);
            MADE_UP.store(std::env::var("MP_PERF_SOUND").as_deref() == Ok("made-up"), Ordering::Relaxed);
            MADE_UP_N.store(0, Ordering::Relaxed);
            // The frame to take the window's picture after, counted from here (the settling
            // included, so frame n is the same frame in any build).
            if let Some(n) = std::env::var("MP_PERF_SNAP").ok().and_then(|v| v.trim().parse().ok()) {
                SNAP_NAME.set(Some(format!("perf-{}-{n}", scene.id())));
                SNAP_AT.store(n, Ordering::Relaxed);
                SNAP_ARMED.store(true, Ordering::Relaxed);
            }
            set_visualizer(st, true);
            views::go(st, Page::NowPlaying);
            if let Some(p) = scene.visualizer() {
                show(st, app, p);
            }
        }
        Scene::Grid => {
            views::go(st, Page::Albums);
            app.global::<Grid>().set_content_y(0.0);
            // Up and down between the top and a little short of the end (never more than
            // 4,000 px), at 2,400 px a second: new rows, and their covers, on every frame.
            let n = app.global::<Grid>().get_count().max(0) as usize;
            let reach = super::grid_max_scroll(app, n).clamp(1.0, 4000.0);
            let weak = app.as_weak();
            let start = Instant::now();
            scroller.start(TimerMode::Repeated, Duration::from_millis(1), move || {
                if let Some(app) = weak.upgrade() {
                    let d = (start.elapsed().as_secs_f32() * 2400.0) % (2.0 * reach);
                    app.global::<Grid>().set_content_y(-(if d > reach { 2.0 * reach - d } else { d }));
                }
            });
        }
        Scene::Prep => views::go(st, Page::Albums),
    }
    if scene.visualizer().is_none() {
        MADE_UP.store(false, Ordering::Relaxed);
    }
}

/// `prep`: down the whole album grid a screen at a time (every cover asked for and cached),
/// back to the top, then Home.
fn prep(steps: &mut Vec<Step>) {
    for k in 1..=40 {
        steps.push((if k == 1 { 800 } else { 150 }, Box::new(move |_st, app| {
            let n = app.global::<Grid>().get_count().max(0) as usize;
            let end = super::grid_max_scroll(app, n);
            app.global::<Grid>().set_content_y(-(end * k as f32 / 40.0));
        })));
    }
    steps.push((1500, Box::new(|st, app| {
        app.global::<Grid>().set_content_y(0.0);
        views::go(st, Page::Home);
        emit(serde_json::json!({"event": "prepared", "unix": now_unix()}));
    })));
    steps.push((1500, Box::new(|_st, _app| {})));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_made_up_sound_is_the_same_for_the_same_sample_and_never_clips() {
        for t in [0u64, 1, 23_999, 24_000, 96_000, 1_000_003] {
            assert_eq!(sound_at(t, 48_000).to_bits(), sound_at(t, 48_000).to_bits());
            assert!(sound_at(t, 48_000).abs() <= 1.0);
        }
        // Not silence: a kick at the start of each half second.
        let peak = (0..2_400).map(|t| sound_at(t, 48_000).abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.3, "{peak}");
    }

    #[test]
    fn a_frame_of_made_up_sound_is_its_window_of_the_sound_and_counts() {
        MADE_UP.store(true, Ordering::Relaxed);
        MADE_UP_N.store(5, Ordering::Relaxed);
        let mut a = vec![9.0f32; 4096];
        made_up(&mut a, 48_000);
        assert_eq!(MADE_UP_N.load(Ordering::Relaxed), 6);
        // Frame 5's window ends at sample 6 * 800 = 4800.
        assert_eq!(a[4095].to_bits(), sound_at(4799, 48_000).to_bits());
        assert_eq!(a[0].to_bits(), sound_at(4800 - 4096, 48_000).to_bits());
        // Before the sound began: silence.
        MADE_UP_N.store(0, Ordering::Relaxed);
        made_up(&mut a, 48_000);
        assert_eq!(a[0], 0.0);
        assert_eq!(frame_dt(0.03), 1.0 / 60.0);
        MADE_UP.store(false, Ordering::Relaxed);
        let mut b = vec![9.0f32; 16];
        made_up(&mut b, 48_000);
        assert!(b.iter().all(|&x| x == 9.0));
        assert_eq!(frame_dt(0.03), 0.03);
    }

    #[test]
    fn scenes_are_read_in_order_and_unknown_names_kept_apart() {
        let (s, u) = scenes_from(" bars, home ,nope,,grid");
        assert_eq!(s, [Scene::Bars, Scene::Home, Scene::Grid]);
        assert_eq!(u, ["nope"]);
    }

    #[test]
    fn frame_figures_count_late_frames_against_the_refresh() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let f = [(ms(0), Duration::from_millis(1)), (ms(17), Duration::from_millis(2)), (ms(33), Duration::from_millis(1)), (ms(83), Duration::from_millis(3))];
        let m = frame_figures("viz_", &f, 1.0, 60.0);
        assert_eq!(m["viz_frames"], 4);
        assert_eq!(m["viz_late"], 1);
        assert_eq!(m["viz_interval_max_ms"].as_f64().unwrap().round(), 50.0);
        assert_eq!(m["viz_work_sum_ms"].as_f64().unwrap().round(), 7.0);
    }
}
