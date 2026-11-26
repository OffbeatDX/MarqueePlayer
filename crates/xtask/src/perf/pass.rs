//! One pass: one build of the app started on a fresh copy of the seeded data folder, its
//! scenes run (`MP_BENCH=perf`, `crates/app/src/bench/perf.rs`), its lines read as they come,
//! and over each measured span the window server, the GPU and, in a watts pass,
//! `powermetrics` read from outside.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::probe;
use crate::report::Fail;

/// The version of the app's lines this reads (`VERSION` in bench/perf.rs).
pub const VERSION: u64 = 1;

/// A line of the app's, after `MP-PERF `.
#[derive(Debug, PartialEq)]
pub enum Line {
    Hello(Value),
    Start { scene: String },
    End(Value),
    Prepared,
    Done,
    Error(String),
}

/// One line of the app's standard output: a perf line, or None for anything else.
pub fn parse_line(line: &str) -> Option<Result<Line, String>> {
    let json = line.trim().strip_prefix("MP-PERF ")?;
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("a line the app wrote is not JSON ({e}): {json}"))),
    };
    let scene = || v["scene"].as_str().unwrap_or_default().to_owned();
    Some(Ok(match v["event"].as_str() {
        Some("hello") => Line::Hello(v),
        Some("start") => Line::Start { scene: scene() },
        Some("end") => Line::End(v),
        Some("prepared") => Line::Prepared,
        Some("done") => Line::Done,
        Some("error") => Line::Error(v["what"].as_str().unwrap_or("?").to_owned()),
        other => return Some(Err(format!("an event the app wrote is not known here: {other:?}"))),
    }))
}

/// What one scene measured in one pass.
#[derive(Clone, Debug, Default)]
pub struct SceneRun {
    pub scene: String,
    /// The app's `end` line.
    pub app: Value,
    /// The window server's CPU over the span, as a share of one core (1.0 = a whole core).
    pub window_server: Option<f64>,
    /// The GPU's utilisation, sampled once a second over the span (%).
    pub gpu: Vec<f64>,
    /// `powermetrics`' samples over the span (W), in a watts pass.
    pub cpu_watts: Vec<f64>,
    pub gpu_watts: Vec<f64>,
    /// The battery controller's mean power over the span (W), in a confirmation pass.
    pub battery_watts: Option<f64>,
    /// The whole machine's busy cores over the span (`iostat`).
    pub machine: Option<f64>,
    /// The screen was locked at the span's end.
    pub locked: bool,
}

impl SceneRun {
    pub fn num(&self, key: &str) -> Option<f64> {
        self.app.get(key).and_then(Value::as_f64)
    }
}

pub struct PassResult {
    pub hello: Option<Value>,
    pub scenes: Vec<SceneRun>,
    pub errors: Vec<String>,
    pub secs: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Extra {
    None,
    /// `powermetrics` over each span (its ten seconds).
    Watts,
    /// The battery controller over each span: from its first refresh to its last, at least
    /// two a minute apart.
    Battery,
}

pub struct Pass<'a> {
    pub exe: &'a Path,
    pub music: &'a Path,
    pub data: &'a Path,
    pub out: &'a Path,
    pub scenes: &'a [String],
    pub secs: u64,
    pub settle_ms: u64,
    pub made_up: bool,
    pub snap: Option<u64>,
    pub extra: Extra,
    pub limit: Duration,
}

/// Runs a pass. Fails only when the app could not be started; what went wrong inside is in
/// `errors`.
pub fn run(p: &Pass) -> Result<PassResult, Fail> {
    let started = Instant::now();
    std::fs::create_dir_all(p.out).map_err(|e| Fail::msg(format!("could not make {}: {e}", p.out.display())))?;
    let log = std::fs::File::create(p.out.join("app.log")).map_err(|e| Fail::msg(format!("could not write the app's log: {e}")))?;
    let mut c = Command::new(p.exe);
    c.env("MP_BENCH", "perf")
        .env("MP_DATA_DIR", p.data)
        .env("MP_MUSIC", p.music)
        .env("MP_VOLUME", "0")
        .env("MP_WINDOW", "secondary")
        .env("MP_BENCH_OUT", p.out)
        .env("MP_PERF_SCENES", p.scenes.join(","))
        .env("MP_PERF_SECS", p.secs.to_string())
        .env("MP_PERF_SETTLE_MS", p.settle_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log);
    if p.made_up {
        c.env("MP_PERF_SOUND", "made-up");
    }
    if let Some(n) = p.snap {
        c.env("MP_PERF_SNAP", n.to_string());
    }
    let child = c.spawn().map_err(|e| Fail::msg(format!("could not start {}: {e}", p.exe.display())))?;
    let child = Arc::new(Mutex::new(child));
    // A pass that outlives its limit is stopped (the app hung, or the window never showed).
    let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (child, killed, limit) = (child.clone(), killed.clone(), p.limit);
        std::thread::spawn(move || {
            let until = Instant::now() + limit;
            while Instant::now() < until {
                if child.lock().map(|mut c| c.try_wait().ok().flatten().is_some()).unwrap_or(true) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if let Ok(mut c) = child.lock() {
                killed.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = c.kill();
            }
        });
    }
    let stdout = child.lock().ok().and_then(|mut c| c.stdout.take()).ok_or_else(|| Fail::msg("no output from the app"))?;
    let ws = probe::window_server();
    let mut res = PassResult { hello: None, scenes: Vec::new(), errors: Vec::new(), secs: 0.0 };
    // What is being read over the span now.
    struct Span {
        ws0: Option<f64>,
        at: Instant,
        gpu: Option<probe::GpuSampler>,
        power: Option<Child>,
        battery: Option<std::thread::JoinHandle<Option<f64>>>,
        battery_stop: Arc<std::sync::atomic::AtomicBool>,
        machine: Option<Child>,
    }
    let mut span: Option<Span> = None;
    let mut done = false;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Some(parsed) = parse_line(&line) else { continue };
        match parsed {
            Err(e) => res.errors.push(e),
            Ok(Line::Hello(v)) => {
                if v["version"].as_u64() != Some(VERSION) {
                    res.errors.push(format!("the app writes version {} of the perf lines, this reads {VERSION}", v["version"]));
                }
                res.hello = Some(v);
            }
            Ok(Line::Start { .. }) => {
                let battery_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                span = Some(Span {
                    ws0: ws.and_then(probe::cpu_seconds),
                    at: Instant::now(),
                    gpu: probe::GpuSampler::start(),
                    power: if p.extra == Extra::Watts { probe::start_powermetrics() } else { None },
                    battery: (p.extra == Extra::Battery).then(|| {
                        let stop = battery_stop.clone();
                        std::thread::spawn(move || battery_span(&stop))
                    }),
                    battery_stop,
                    machine: probe::start_machine_cpu(p.secs),
                });
            }
            Ok(Line::End(v)) => {
                let Some(s) = span.take() else {
                    res.errors.push("the app ended a span it had not started".into());
                    continue;
                };
                let wall = s.at.elapsed().as_secs_f64();
                let ws1 = ws.and_then(probe::cpu_seconds);
                let mut run = SceneRun { scene: v["scene"].as_str().unwrap_or_default().to_owned(), app: v, ..Default::default() };
                run.window_server = s.ws0.zip(ws1).map(|(a, b)| (b - a) / wall.max(1e-9));
                run.gpu = s.gpu.map(probe::GpuSampler::finish).unwrap_or_default();
                if let Some(mut pm) = s.power {
                    // Its ten samples take ten seconds from the span's start: wait for them.
                    let mut text = String::new();
                    if let Some(mut o) = pm.stdout.take() {
                        let _ = std::io::Read::read_to_string(&mut o, &mut text);
                    }
                    let _ = pm.wait();
                    let (cpu, gpu) = probe::parse_powermetrics(&text);
                    run.cpu_watts = cpu;
                    run.gpu_watts = gpu;
                }
                if let Some(b) = s.battery {
                    s.battery_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    run.battery_watts = b.join().ok().flatten();
                }
                if let Some(mut m) = s.machine {
                    // Its second line comes as the span ends: give it a moment, no more.
                    let until = Instant::now() + Duration::from_millis(1500);
                    while Instant::now() < until && m.try_wait().ok().flatten().is_none() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = m.kill();
                    let mut text = String::new();
                    if let Some(mut o) = m.stdout.take() {
                        let _ = std::io::Read::read_to_string(&mut o, &mut text);
                    }
                    let _ = m.wait();
                    run.machine = probe::parse_iostat(&text, std::thread::available_parallelism().map_or(1, |n| n.get()));
                }
                run.locked = probe::screen_locked();
                res.scenes.push(run);
            }
            Ok(Line::Prepared) => {}
            Ok(Line::Done) => done = true,
            Ok(Line::Error(e)) => res.errors.push(format!("the app: {e}")),
        }
    }
    let status = child.lock().ok().and_then(|mut c| c.wait().ok());
    if killed.load(std::sync::atomic::Ordering::Relaxed) {
        res.errors.push(format!("the app was stopped after {} s, its limit: it hung, or never got as far as its scenes (log: {})", p.limit.as_secs(), p.out.join("app.log").display()));
    } else if !done {
        res.errors.push(format!("the app ended before its scenes did ({}; log: {})", status.map_or("no exit status".into(), |s| s.to_string()), p.out.join("app.log").display()));
    }
    res.secs = started.elapsed().as_secs_f64();
    Ok(res)
}

/// Polls the battery's accumulator every two seconds until `stop`: the mean power from the
/// first refresh seen to the last, once at least two refreshes a minute apart were.
fn battery_span(stop: &std::sync::atomic::AtomicBool) -> Option<f64> {
    let mut last = probe::battery_accumulator()?;
    let mut first_refresh: Option<(u64, u64)> = None;
    let mut latest: Option<(u64, u64)> = None;
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(2));
        let Some(now) = probe::battery_accumulator() else { continue };
        if now.1 != last.1 {
            if first_refresh.is_none() {
                first_refresh = Some(now);
            } else {
                latest = Some(now);
            }
            last = now;
        }
    }
    probe::battery_watts(first_refresh?, latest?)
}

/// The music a pass plays and shows: the core set's grid albums (with covers) and the
/// visualizer's "Night Shift", cloned once into `<temp>/mp-perf-music`.
pub fn music(fixtures: &Path) -> Result<PathBuf, Fail> {
    let dir = std::env::temp_dir().join("mp-perf-music");
    let parts = [(fixtures.join("core").join("100 Grid"), dir.join("100 Grid")), (fixtures.join("viz").join("Fixture Band").join("Night Shift"), dir.join("Night Shift"))];
    for (from, to) in &parts {
        if to.is_dir() {
            continue;
        }
        if !from.is_dir() {
            return Err(Fail::msg(format!("the fixtures have no {}", from.display())));
        }
        super::sides::copy_tree(from, to).map_err(|e| Fail::msg(format!("could not copy {} to {}: {e}", from.display(), to.display())))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_lines_are_read_and_the_rest_ignored() {
        // As the app wrote them on 2026-11-26 (bench/perf.rs, version 1).
        let hello = r#"MP-PERF {"event":"hello","made_up_sound":true,"refresh_hz":60,"scale":2.0,"scenes":["prep","home","bars","grid"],"secs":4,"settle_ms":2500,"unix":1795715048.908926,"unknown_scenes":[],"version":1,"window_h":1844,"window_w":2940}"#;
        let Some(Ok(Line::Hello(v))) = parse_line(hello) else { panic!() };
        assert_eq!(v["refresh_hz"], 60);
        assert_eq!(parse_line(r#"MP-PERF {"event":"start","scene":"home","unix":1795715061.721443}"#), Some(Ok(Line::Start { scene: "home".into() })));
        let end = r#"MP-PERF {"cpu_s":1.392936,"event":"end","made_up_frames":231,"mem_mb":643.6574859619141,"scene":"bars","secs":4.00157325,"unix":1795715072.524933,"viz_fps":57.727295133232914,"viz_frames":231,"viz_interval_max_ms":156.963917,"viz_interval_p50_ms":16.663042,"viz_interval_p95_ms":17.548208,"viz_late":1,"viz_work_p50_ms":1.344792,"viz_work_p95_ms":1.785,"viz_work_sum_ms":287.51313600000003,"win_fps":57.727295133232914,"win_frames":231,"win_interval_max_ms":170.010834,"win_interval_p50_ms":16.6665,"win_interval_p95_ms":17.853417,"win_late":2,"win_work_p50_ms":2.54025,"win_work_p95_ms":4.348417,"win_work_sum_ms":573.1157069999999}"#;
        let Some(Ok(Line::End(v))) = parse_line(end) else { panic!() };
        let run = SceneRun { scene: "bars".into(), app: v, ..Default::default() };
        assert_eq!(run.num("viz_frames"), Some(231.0));
        assert_eq!(run.num("cpu_s"), Some(1.392936));
        assert_eq!(parse_line(r#"MP-PERF {"event":"done","unix":1}"#), Some(Ok(Line::Done)));
        assert_eq!(parse_line("    0.005 [INFO ] MarqueePlayer: config ..."), None);
        assert!(matches!(parse_line("MP-PERF {not json"), Some(Err(_))));
        assert!(matches!(parse_line(r#"MP-PERF {"event":"later"}"#), Some(Err(_))));
    }
}
