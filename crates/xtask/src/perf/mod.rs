//! `cargo xtask perf`: is this change cheaper to draw than its base, by how much, and how
//! sure is that, in about two minutes (docs/testing.md, "Comparing two builds: cargo xtask
//! perf"). `sides.rs` builds the two, `pass.rs` runs one build's scenes, `probe.rs` reads
//! what the app cannot (the window server, the GPU, watts), `stats.rs` compares.

mod pass;
mod probe;
mod sides;
mod stats;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::args::{COMMON_HELP, Common, Parser};
use crate::cmd;
use crate::report::Fail;
use crate::workspace::Workspace;
use pass::{Extra, Pass, PassResult, SceneRun};
use sides::Side;
use stats::{Compared, Sides};

/// Every scene, in the order `--scenes all` runs them, with what it is.
const SCENES: [(&str, &str); 5] = [
    ("home", "music playing on Home, nothing else moving"),
    ("np-off", "Now playing with the visualizer off"),
    ("bars", "the default visualizer (bars) behind the page"),
    ("halo", "the halo behind the page"),
    ("grid", "the album grid scrolled at 2,400 px a second"),
];
const DEFAULT_SCENES: &str = "home,bars";

/// The smallest difference that counts, as a share of the base, whatever the spread:
/// measured as this tool's own noise with no change (docs/testing.md). With the clock held the
/// app's CPU repeats to a few tenths of a percent on a quiet machine, but with another app
/// running benches beside it two rounds of the same build differed by up to 6.6%
/// (2026-11-26), and two rounds can agree by chance: 7%.
const FLOOR_APP: f64 = 0.07;
const FLOOR_OTHER: f64 = 0.10;
const FLOOR_WATTS: f64 = 0.05;
/// And for what the whole machine moves, in points: the window server (% of a core) and the
/// GPU (% busy) swung by 3 and 7 points between rounds with no change while another app drew.
const FLOOR_WINDOW_SERVER_PTS: f64 = 4.0;
const FLOOR_GPU_PTS: f64 = 10.0;
/// Other work on the machine (cores busy beyond the app, the two busy processes and the window
/// server) that makes a scene's verdict untrusted: more than this in any run, or this much
/// more on one side than the other. A `cargo build` looping in another folder made a change
/// that was none read as 19% dearer with the clock held (2026-11-26).
const OTHER_WORK_MAX: f64 = 1.0;
const OTHER_WORK_SKEW: f64 = 0.5;

fn help() -> String {
    let scenes: String = SCENES.iter().map(|(n, d)| format!("\n                     {n:<7} {d}")).collect();
    format!(
        "\
cargo xtask perf [--base REF] [--scenes a,b,...] [--rounds N] [--confirm] [options]

Compares what two builds of the app cost to draw: the working tree as it is (the change)
and REF (the base; default: where this branch left master). Builds both at once on the
`fast` profile with the bench feature, each in its own target folder under
<target>/xtask-perf (the base in a checkout of its own there, a detached git worktree kept
between runs; your checkout and branches are never touched), then runs each scene on both,
interleaved (base, change, change, base, base...), with two busy processes holding the CPU's
clock up, and prints one table: for each scene the app's CPU per frame (per second where
nothing is drawn), frames a second and late frames, the window server's CPU, the GPU,
memory and watts where they can be read, each with its difference, the spread between
rounds beside it, and a verdict: cheaper, dearer or within the noise. The whole run is also
written as JSON (<target>/xtask-perf/last.json, or --json FILE).

It refuses rather than mislead: a locked screen, a window that drew no frames, a base that
does not build stop it with a non-zero exit; a scene run while the machine did other work
(more than a core beyond the app, the busy processes and the window server, or half a core
more on one side) gets no verdict, and with none left the exit code is 3; a scene that did
not hold its frame rate, or whose spread is wider than its difference, is said to be so. A
verdict needs a difference over 7% of the app's CPU: smaller ones are within this tool's
noise (docs/testing.md).

Options:
  --base REF         the base: any commit, branch or tag
  --scenes a,b       the scenes, in order (default {DEFAULT_SCENES}; `all` for every one):{scenes}
  --rounds N         passes of each build (default 2; one gives figures but no verdict)
  --secs S           seconds measured a scene (default 8)
  --settle S         seconds a scene settles before it is measured (default 2.5)
  --real-sound       the visualizer hears the fixtures' music, not the made-up sound that
                     makes both builds draw the same frames
  --snap N           save the window's own picture after the visualizer's N-th frame in
                     each build (bars, halo) and say how many pixels differ
  --no-watts         skip the watts pass even where powermetrics is allowed
  --confirm          then a slow confirmation: the battery controller's watts, each scene
                     about three minutes on each build, no busy processes
  --json FILE        also write the JSON here
  --fixtures DIR     the bench fixtures (default <temp>/mp-bench-fixtures; made if missing)
{COMMON_HELP}

What it takes (M5 MacBook Air, measured): a first run against a new base about three
minutes (both builds at once, 68 s); a later run with the same base builds only what
changed (seconds) and takes about two minutes for the default two scenes and two rounds
(a pass about 24 s), about four and a half for every scene; --confirm about six minutes
more a scene.

  cargo xtask perf                          home and bars against the merge base
  cargo xtask perf --base HEAD --scenes all  what the uncommitted change costs, every scene
  cargo xtask perf --snap 300 --scenes bars  and whether it draws the same picture"
    )
}

struct Opts {
    base: Option<String>,
    scenes: Vec<String>,
    rounds: usize,
    secs: u64,
    settle_ms: u64,
    real_sound: bool,
    snap: Option<u64>,
    watts: bool,
    confirm: bool,
    json: Option<PathBuf>,
    fixtures: Option<PathBuf>,
}

fn parse(args: Vec<String>, common: &mut Common) -> Result<Option<Opts>, Fail> {
    let mut p = Parser::new("perf", args);
    let mut o = Opts { base: None, scenes: Vec::new(), rounds: 2, secs: 8, settle_ms: 2500, real_sound: false, snap: None, watts: true, confirm: false, json: None, fixtures: None };
    let mut scenes = DEFAULT_SCENES.to_owned();
    let num = |p: &mut Parser, what: &str| -> Result<f64, Fail> {
        let v = p.value()?;
        v.parse::<f64>().ok().filter(|n| *n > 0.0 && n.is_finite()).ok_or_else(|| Fail::usage(format!("{what} needs a number above 0, not `{v}`")))
    };
    while let Some(a) = p.next()? {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return Ok(None);
            }
            "--base" => o.base = Some(p.value()?),
            "--scenes" => scenes = p.value()?,
            "--rounds" => o.rounds = num(&mut p, "--rounds")?.round() as usize,
            "--secs" => o.secs = num(&mut p, "--secs")?.round().max(1.0) as u64,
            "--settle" => o.settle_ms = (num(&mut p, "--settle")? * 1000.0) as u64,
            "--real-sound" => o.real_sound = true,
            "--snap" => o.snap = Some(num(&mut p, "--snap")?.round() as u64),
            "--no-watts" => o.watts = false,
            "--confirm" => o.confirm = true,
            "--json" => o.json = Some(PathBuf::from(p.value()?)),
            "--fixtures" => o.fixtures = Some(PathBuf::from(p.value()?)),
            _ if common.take(&a, &mut p)? => {}
            _ => return Err(p.unknown(&a)),
        }
    }
    o.scenes = parse_scenes(&scenes)?;
    Ok(Some(o))
}

fn parse_scenes(s: &str) -> Result<Vec<String>, Fail> {
    if s.trim() == "all" {
        return Ok(SCENES.iter().map(|(n, _)| (*n).to_owned()).collect());
    }
    let mut out = Vec::new();
    for n in s.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !SCENES.iter().any(|(k, _)| *k == n) {
            return Err(Fail::usage(format!("no scene is called `{n}`: {} or all", SCENES.iter().map(|s| s.0).collect::<Vec<_>>().join(", "))));
        }
        if !out.iter().any(|o| o == n) {
            out.push(n.to_owned());
        }
    }
    if out.is_empty() {
        return Err(Fail::usage("--scenes names none"));
    }
    Ok(out)
}

/// Where the times of a run went, for the report.
#[derive(Default)]
struct Times(Vec<(String, f64)>);

impl Times {
    fn add(&mut self, what: &str, secs: f64) {
        self.0.push((what.to_owned(), secs));
    }
}

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut common = Common::default();
    let Some(o) = parse(args, &mut common)? else { return Ok(()) };
    let all_start = Instant::now();
    let mut times = Times::default();
    let ws = Workspace::find(&common)?;
    let root = ws.target.join("xtask-perf");
    let runs = std::env::temp_dir().join("mp-perf-runs");
    let _ = std::fs::remove_dir_all(&runs);
    std::fs::create_dir_all(&runs).map_err(|e| Fail::msg(format!("could not make {}: {e}", runs.display())))?;
    let mut notes: Vec<String> = Vec::new();

    // ---- The builds ----
    let t = Instant::now();
    let (sha, base_label) = sides::resolve_base(&ws.root, o.base.as_deref())?;
    println!("base: {base_label} = {}", &sha[..12.min(sha.len())]);
    println!("change: the working tree of {}", ws.root.display());
    let members = sides::members(&ws);
    let base_src = root.join("base-src");
    let mut base = Side { name: "base", src: base_src.clone(), target: root.join("base"), notes: sides::prepare_base(&ws.root, &base_src, &sha)? };
    let mut change = Side { name: "change", src: ws.root.clone(), target: root.join("change"), notes: Vec::new() };
    change.notes = sides::seed_target(&change.target, &[ws.target.join("bench"), ws.target.clone()], &change.src, &members)?;
    let seeds = [change.target.clone(), ws.target.join("bench"), ws.target.clone()];
    base.notes.extend(sides::seed_target(&base.target, &seeds, &base.src, &members)?);
    times.add("checkout and seeding", t.elapsed().as_secs_f64());
    for s in [&base, &change] {
        for n in &s.notes {
            println!("  {}: {n}", s.name);
        }
    }
    println!("building both (fast profile, bench feature; logs in {})...", runs.display());
    let t = Instant::now();
    let built = sides::build_all(&ws, &[&base, &change], &runs)?;
    times.add("builds (both at once)", t.elapsed().as_secs_f64());
    println!("  built: base {:.0} s, change {:.0} s", built[0], built[1]);
    notes.extend(base.notes.iter().map(|n| format!("base: {n}")));
    notes.extend(change.notes.iter().map(|n| format!("change: {n}")));

    // ---- What they play ----
    let fixtures = o.fixtures.clone().unwrap_or_else(|| std::env::temp_dir().join("mp-bench-fixtures"));
    let t = Instant::now();
    ensure_fixtures(&ws, &fixtures)?;
    let music = pass::music(&fixtures)?;
    times.add("fixtures", t.elapsed().as_secs_f64());
    let hook = |s: &Side| std::fs::read_to_string(s.src.join("crates/app/src/visualizer/mod.rs")).is_ok_and(|t| t.contains("bench::perf::made_up"));
    let made_up = !o.real_sound && hook(&base) && hook(&change);
    if !o.real_sound && !made_up {
        notes.push("made-up sound: off, as the base's visualizer cannot hear it; both builds hear the fixtures' music, so their frames differ and the comparison is noisier".into());
    }
    for n in probe::not_read_here() {
        notes.push(format!("not read on this system: {n}"));
    }

    if probe::screen_locked() {
        return Err(refused("the screen is locked: nothing draws, so nothing would be measured. Unlock it and run again."));
    }
    // ---- Seed data folders: each build's own library, scanned and its covers cached ----
    let t = Instant::now();
    let limit = |scenes: usize, secs: u64| Duration::from_secs(60 + scenes as u64 * (secs + o.settle_ms / 1000 + 4));
    {
        let _hold = probe::Hold::new(false);
        let prep = ["prep".to_owned()];
        let results: Vec<Result<PassResult, Fail>> = std::thread::scope(|sc| {
            let hs: Vec<_> = [&base, &change]
                .into_iter()
                .map(|s| {
                    let (seed, out, exe, music, prep) = (runs.join(format!("seed-{}", s.name)), runs.join(format!("prep-{}", s.name)), s.exe(), music.clone(), &prep);
                    sc.spawn(move || pass::run(&Pass { exe: &exe, music: &music, data: &seed, out: &out, scenes: prep, secs: 1, settle_ms: 0, made_up: false, snap: None, extra: Extra::None, limit: Duration::from_secs(120) }))
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(Fail::msg("a preparing pass panicked")))).collect()
        });
        for (s, r) in [&base, &change].iter().zip(results) {
            let r = r?;
            if !r.errors.is_empty() {
                return Err(refused(&format!("the {} could not prepare its library: {}", s.name, r.errors.join("; "))));
            }
        }
    }
    times.add("libraries scanned, covers cached (both at once)", t.elapsed().as_secs_f64());

    // ---- The measuring passes, interleaved, the clock held ----
    let t = Instant::now();
    let mut rounds: Vec<(&str, PassResult)> = Vec::new();
    {
        let _hold = probe::Hold::new(true);
        for r in 0..o.rounds {
            // Base first, then change first, and so on (ABBA): a drift in the machine over
            // the run weighs on both sides alike.
            let order = if r % 2 == 0 { [&base, &change] } else { [&change, &base] };
            for s in order {
                let res = measure(s, &runs, &music, &o, format!("r{}-{}", r + 1, s.name), made_up, if r == 0 { o.snap } else { None }, Extra::None, o.secs, &o.scenes, limit(o.scenes.len(), o.secs))?;
                println!("  round {} {}: {:.0} s{}", r + 1, s.name, res.secs, if res.errors.is_empty() { String::new() } else { format!(" ({})", res.errors.join("; ")) });
                if res.scenes.iter().any(|s| s.locked) || probe::screen_locked() {
                    return Err(refused("the screen locked during the run: nothing drew, nothing was measured. Unlock it and run again."));
                }
                rounds.push((s.name, res));
            }
        }
    }
    times.add(&format!("measuring: {} rounds of {} scenes", o.rounds, o.scenes.len()), t.elapsed().as_secs_f64());

    // ---- Watts, without the busy processes ----
    let mut watts: Vec<(&str, PassResult)> = Vec::new();
    if o.watts && probe::powermetrics_allowed() {
        let t = Instant::now();
        let _hold = probe::Hold::new(false);
        let secs = o.secs.max(11);
        for s in [&base, &change] {
            let res = measure(s, &runs, &music, &o, format!("watts-{}", s.name), made_up, None, Extra::Watts, secs, &o.scenes, limit(o.scenes.len(), secs))?;
            println!("  watts {}: {:.0} s", s.name, res.secs);
            watts.push((s.name, res));
        }
        times.add("watts pass (powermetrics, one each)", t.elapsed().as_secs_f64());
    } else if o.watts && cfg!(target_os = "macos") {
        notes.push("watts: not read; `sudo -n /usr/bin/powermetrics --samplers cpu_power,gpu_power -i 1000 -n 10` is not allowed without a password here (docs/testing.md says how to allow exactly that)".into());
    }

    // ---- The slow confirmation ----
    let mut battery: Vec<(&str, PassResult)> = Vec::new();
    if o.confirm {
        if probe::battery_accumulator().is_none() {
            notes.push("--confirm: no battery controller to read here".into());
        } else {
            let t = Instant::now();
            let _hold = probe::Hold::new(false);
            for scene in &o.scenes {
                for s in [&base, &change] {
                    let one = [scene.clone()];
                    let res = measure(s, &runs, &music, &o, format!("confirm-{scene}-{}", s.name), made_up, None, Extra::Battery, 170, &one, limit(1, 170))?;
                    println!("  confirm {scene} {}: {:.0} s", s.name, res.secs);
                    battery.push((s.name, res));
                }
            }
            times.add("confirmation (battery controller)", t.elapsed().as_secs_f64());
        }
    }

    // ---- The table ----
    let refresh = rounds.iter().find_map(|(_, r)| r.hello.as_ref()?.get("refresh_hz")?.as_f64()).unwrap_or(60.0);
    let mut out_scenes = Vec::new();
    let mut measured_any = false;
    println!();
    println!("== the change against {base_label}: {} rounds, {} s a scene, the clock held{}", o.rounds, o.secs, if made_up { ", made-up sound" } else { "" });
    for scene in &o.scenes {
        let (text, v, measured) = scene_report(scene, &rounds, &watts, &battery, refresh, made_up);
        measured_any |= measured;
        print!("{text}");
        out_scenes.push(v);
    }
    if let Some(n) = o.snap {
        println!();
        for scene in o.scenes.iter().filter(|s| *s == "bars" || *s == "halo") {
            println!("snap {scene}, frame {n}: {}", snap_compare(&runs, scene, n));
        }
    }
    times.add("whole run", all_start.elapsed().as_secs_f64());
    println!();
    for n in &notes {
        println!("note: {n}");
    }
    println!("times: {}", times.0.iter().map(|(w, s)| format!("{w} {s:.0} s")).collect::<Vec<_>>().join("; "));

    let report = json!({
        "base": {"ref": base_label, "sha": sha}, "change": ws.root.display().to_string(),
        "rounds": o.rounds, "secs": o.secs, "settle_ms": o.settle_ms, "made_up_sound": made_up,
        "refresh_hz": refresh, "floors": {"app": FLOOR_APP, "other": FLOOR_OTHER, "watts": FLOOR_WATTS, "window_server_pts": FLOOR_WINDOW_SERVER_PTS, "gpu_pts": FLOOR_GPU_PTS},
        "scenes": out_scenes, "notes": notes, "times_s": times.0.iter().map(|(w, s)| json!({"what": w, "secs": s})).collect::<Vec<_>>(),
        "passes": rounds.iter().chain(&watts).chain(&battery).map(|(side, r)| pass_json(side, r)).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    let last = root.join("last.json");
    let _ = std::fs::write(&last, &text);
    println!("json: {}", last.display());
    if let Some(j) = &o.json {
        std::fs::write(j, &text).map_err(|e| Fail::msg(format!("could not write {}: {e}", j.display())))?;
        println!("json: {}", j.display());
    }
    if !measured_any {
        return Err(refused("no scene was measured on both builds on a quiet enough machine: no comparison to trust (see each scene above)"));
    }
    Ok(())
}

fn refused(why: &str) -> Fail {
    Fail { code: 3, message: Some(format!("not measured: {why}")) }
}

#[allow(clippy::too_many_arguments)]
fn measure(s: &Side, runs: &Path, music: &Path, o: &Opts, name: String, made_up: bool, snap: Option<u64>, extra: Extra, secs: u64, scenes: &[String], limit: Duration) -> Result<PassResult, Fail> {
    let data = runs.join("data");
    let _ = std::fs::remove_dir_all(&data);
    sides::copy_tree(&runs.join(format!("seed-{}", s.name)), &data).map_err(|e| Fail::msg(format!("could not copy the seeded data folder: {e}")))?;
    pass::run(&Pass { exe: &s.exe(), music, data: &data, out: &runs.join(name), scenes, secs, settle_ms: o.settle_ms, made_up, snap, extra, limit })
}

fn ensure_fixtures(ws: &Workspace, dir: &Path) -> Result<(), Fail> {
    if dir.join("core").join("100 Grid").is_dir() && dir.join("viz").join("Fixture Band").join("Night Shift").is_dir() {
        return Ok(());
    }
    println!("making the bench fixtures in {} (once)...", dir.display());
    let target = ws.target.join("bench");
    let code = cmd::status(&mut cmd::below_normal(ws.cargo(&target, &["build", "--release", "-p", "library", "--examples"])))?;
    if code != 0 {
        return Err(Fail::code(code));
    }
    let tool = target.join("release").join("examples").join(Workspace::exe("bench_fixtures"));
    let code = cmd::status(std::process::Command::new(&tool).arg(dir))?;
    if code != 0 {
        return Err(Fail::msg(format!("bench_fixtures failed (exit {code})")));
    }
    Ok(())
}

// ---- Reading the passes ----------------------------------------------------------------------

/// What a scene's main cost is counted per.
#[derive(Clone, Copy, PartialEq)]
enum Unit {
    VizFrame,
    WindowFrame,
    Second,
}

/// The scene's runs on one side, in round order.
fn runs_of<'a>(passes: &'a [(&str, PassResult)], side: &str, scene: &str) -> Vec<&'a SceneRun> {
    passes.iter().filter(|(s, _)| *s == side).filter_map(|(_, r)| r.scenes.iter().find(|x| x.scene == scene)).collect()
}

fn frames_expected(scene: &str) -> bool {
    matches!(scene, "bars" | "halo" | "grid")
}

struct Measure {
    label: &'static str,
    sides: Sides,
    floor: f64,
    floor_abs: f64,
    fmt: fn(f64) -> String,
    /// Whether it gets a verdict (lower is cheaper) or is shown for what it is.
    judged: bool,
}

fn scene_report(scene: &str, rounds: &[(&str, PassResult)], watts: &[(&str, PassResult)], battery: &[(&str, PassResult)], refresh: f64, made_up: bool) -> (String, Value, bool) {
    let (b, c) = (runs_of(rounds, "base", scene), runs_of(rounds, "change", scene));
    let mut text = format!("\n{scene}: {}\n", SCENES.iter().find(|s| s.0 == scene).map_or("", |s| s.1));
    let mut problems: Vec<String> = Vec::new();
    // Frames: the scenes that draw every refresh must have drawn, and held the rate.
    let viz = matches!(scene, "bars" | "halo");
    let win_counted = b.iter().chain(&c).all(|r| r.num("win_frames").unwrap_or(0.0) > 0.0);
    let (fkey, lkey) = if viz { ("viz_fps", "viz_late") } else { ("win_fps", "win_late") };
    let mut refused_scene = b.is_empty() || c.is_empty();
    if b.is_empty() || c.is_empty() {
        problems.push("not measured on both builds (see the passes' errors)".into());
    }
    if frames_expected(scene) && (viz || win_counted) {
        for (side, runs) in [("base", &b), ("change", &c)] {
            for (i, r) in runs.iter().enumerate() {
                let fps = r.num(fkey).unwrap_or(0.0);
                if fps < 0.5 * refresh {
                    refused_scene = true;
                    problems.push(format!("the {side} drew {fps:.0} frames a second in round {} where it should have drawn {refresh:.0}: the window covered, hidden or asleep? Not measured", i + 1));
                } else if fps < 0.9 * refresh {
                    problems.push(format!("the {side} did not hold its frame rate in round {} ({fps:.1} of {refresh:.0} a second): a cost per frame is not comparable", i + 1));
                }
            }
        }
    }
    if viz && made_up && b.iter().chain(&c).any(|r| r.num("made_up_frames").unwrap_or(0.0) < 1.0) {
        problems.push("the made-up sound was asked for but a build's visualizer did not hear it".into());
    }
    if scene == "grid" && !win_counted {
        problems.push("the window's frames were not counted on a build: its CPU is per second of scrolling, not per frame".into());
    }
    // Other work on the machine over each span: what was busy beyond the app, the two busy
    // processes and the window server.
    let other = |r: &&SceneRun| -> Option<f64> { Some(r.machine? - r.num("cpu_s")? / r.num("secs")? - 2.0 - r.window_server.unwrap_or(0.0)) };
    let (ob, oc): (Vec<f64>, Vec<f64>) = (b.iter().filter_map(other).collect(), c.iter().filter_map(other).collect());
    let mut busy_machine = false;
    if let (Some(mb), Some(mc)) = (stats::mean(&ob), stats::mean(&oc)) {
        let most = ob.iter().chain(&oc).copied().fold(f64::NEG_INFINITY, f64::max);
        if most > OTHER_WORK_MAX || (mb - mc).abs() > OTHER_WORK_SKEW {
            busy_machine = true;
            problems.push(format!(
                "the machine was doing other work: {mb:.1} cores busy beyond the app, the busy processes and the window server during the base's runs, {mc:.1} during the change's (at most {OTHER_WORK_MAX:.1}, and no more than {OTHER_WORK_SKEW:.1} apart, to be trusted): its verdict is not to be trusted; run again on a quieter machine"
            ));
        }
    }
    let unit = if viz { Unit::VizFrame } else if scene == "grid" && win_counted { Unit::WindowFrame } else { Unit::Second };
    let app_cost = |r: &&SceneRun| -> Option<f64> {
        let cpu = r.num("cpu_s")?;
        match unit {
            Unit::VizFrame => Some(cpu * 1000.0 / r.num("viz_frames").filter(|n| *n > 0.0)?),
            Unit::WindowFrame => Some(cpu * 1000.0 / r.num("win_frames").filter(|n| *n > 0.0)?),
            Unit::Second => Some(100.0 * cpu / r.num("secs")?),
        }
    };
    let collect = |f: &dyn Fn(&&SceneRun) -> Option<f64>| Sides { base: b.iter().filter_map(f).collect(), change: c.iter().filter_map(f).collect() };
    let ms = |v: f64| format!("{v:.3} ms");
    let pct_core = |v: f64| format!("{v:.1}% core");
    let mut measures = vec![Measure {
        label: match unit {
            Unit::VizFrame => "app CPU a viz frame",
            Unit::WindowFrame => "app CPU a frame",
            Unit::Second => "app CPU",
        },
        sides: collect(&app_cost),
        floor: FLOOR_APP,
        floor_abs: 0.0,
        fmt: if unit == Unit::Second { pct_core } else { ms },
        judged: true,
    }];
    if unit != Unit::Second {
        measures.push(Measure { label: "app CPU", sides: collect(&|r| Some(100.0 * r.num("cpu_s")? / r.num("secs")?)), floor: FLOOR_APP, floor_abs: 0.0, fmt: pct_core, judged: false });
    }
    if viz || win_counted {
        measures.push(Measure { label: if viz { "viz frames a second" } else { "frames a second" }, sides: collect(&|r| r.num(fkey)), floor: 0.0, floor_abs: 0.0, fmt: |v| format!("{v:.1}"), judged: false });
        // Late against the display's refresh: only where every refresh should draw (Home
        // draws ten frames a second, each "late").
        if frames_expected(scene) {
            measures.push(Measure { label: "late frames", sides: collect(&|r| r.num(lkey)), floor: 0.0, floor_abs: 0.0, fmt: |v| format!("{v:.1}"), judged: false });
        }
    }
    if viz && win_counted {
        measures.push(Measure { label: "window frames a second", sides: collect(&|r| r.num("win_fps")), floor: 0.0, floor_abs: 0.0, fmt: |v| format!("{v:.1}"), judged: false });
    }
    measures.push(Measure { label: "window server", sides: collect(&|r| r.window_server.map(|w| 100.0 * w)), floor: FLOOR_OTHER, floor_abs: FLOOR_WINDOW_SERVER_PTS, fmt: pct_core, judged: true });
    measures.push(Measure { label: "GPU", sides: collect(&|r| stats::mean(&r.gpu)), floor: FLOOR_OTHER, floor_abs: FLOOR_GPU_PTS, fmt: |v| format!("{v:.1}%"), judged: true });
    measures.push(Measure { label: "other work (cores)", sides: collect(&other), floor: 0.0, floor_abs: 0.0, fmt: |v| format!("{v:.2}"), judged: false });
    measures.push(Measure { label: "memory at the end", sides: collect(&|r| r.num("mem_mb")), floor: 0.05, floor_abs: 0.0, fmt: |v| format!("{v:.0} MB"), judged: true });

    let mut rows = Vec::new();
    let mut by_label: Vec<(&str, Option<Compared>)> = Vec::new();
    text.push_str(&format!("  {:<24} {:>13} {:>13} {:>16} {:>12}\n", "", "base", "change", "difference", "spread"));
    for m in &measures {
        let cmp = stats::compare(&m.sides, m.floor, m.floor_abs);
        by_label.push((m.label, cmp));
        let Some(cm) = cmp else {
            if !m.sides.base.is_empty() || !m.sides.change.is_empty() {
                text.push_str(&format!("  {:<24} not measured on both\n", m.label));
            }
            continue;
        };
        let verdict = if m.judged && !refused_scene { cm.verdict.word() } else { "" };
        text.push_str(&format!(
            "  {:<24} {:>13} {:>13} {:>16} {:>12}  {verdict}\n",
            m.label,
            (m.fmt)(cm.base),
            (m.fmt)(cm.change),
            format!("{:+.1}%", 100.0 * cm.rel),
            if cm.verdict == stats::Verdict::NoSpread { "-".to_owned() } else { format!("±{:.1}%", 100.0 * cm.spread_rel) },
        ));
        rows.push(json!({"measure": m.label, "base_rounds": m.sides.base, "change_rounds": m.sides.change, "base": cm.base, "change": cm.change, "diff": cm.diff, "rel": cm.rel, "spread": cm.spread, "spread_rel": cm.spread_rel, "verdict": if m.judged { cm.verdict.word() } else { "" }}));
    }
    // Watts: one run each side, ten samples a second apart.
    let samples = |side: &str, f: fn(&SceneRun) -> &Vec<f64>| runs_of(watts, side, scene).first().map(|r| f(r).clone()).unwrap_or_default();
    for (label, f) in [("CPU watts (powermetrics)", (|r: &SceneRun| &r.cpu_watts) as fn(&SceneRun) -> &Vec<f64>), ("GPU watts (powermetrics)", |r: &SceneRun| &r.gpu_watts)] {
        if let Some(cm) = stats::compare_samples(&samples("base", f), &samples("change", f), FLOOR_WATTS) {
            text.push_str(&format!("  {:<24} {:>13} {:>13} {:>16} {:>12}  {}\n", label, format!("{:.2} W", cm.base), format!("{:.2} W", cm.change), format!("{:+.1}%", 100.0 * cm.rel), format!("±{:.1}%", 100.0 * cm.spread_rel), cm.verdict.word()));
            rows.push(json!({"measure": label, "base": cm.base, "change": cm.change, "rel": cm.rel, "spread_rel": cm.spread_rel, "verdict": cm.verdict.word()}));
        }
    }
    let bw = |side: &str| runs_of(battery, side, scene).first().and_then(|r| r.battery_watts);
    match (bw("base"), bw("change")) {
        (Some(x), Some(y)) => {
            text.push_str(&format!("  {:<24} {:>13} {:>13} {:>16} {:>12}  one reading each: no spread\n", "whole machine (battery)", format!("{x:.2} W"), format!("{y:.2} W"), format!("{:+.1}%", 100.0 * (y - x) / x), "-"));
            rows.push(json!({"measure": "whole machine watts (battery controller)", "base": x, "change": y}));
        }
        (None, None) => {}
        _ => text.push_str("  whole machine (battery): the controller did not refresh twice during a span: no figure\n"),
    }
    let get = |l: &str| by_label.iter().find(|(k, _)| *k == l).and_then(|(_, c)| *c);
    let app_label = measures[0].label;
    let verdict = if refused_scene {
        "not measured".to_owned()
    } else if busy_machine {
        format!("not trusted, the machine was busy (it read: {})", stats::scene_verdict(get(app_label), get("window server"), get("GPU")))
    } else {
        stats::scene_verdict(get(app_label), get("window server"), get("GPU"))
    };
    let noise = get(app_label).map_or(String::new(), |c| format!(" (the app's CPU {:+.1}%, its rounds ±{:.1}% apart, the floor {:.0}%)", 100.0 * c.rel, 100.0 * c.spread_rel, 100.0 * FLOOR_APP));
    text.push_str(&format!("  verdict: {verdict}{noise}\n"));
    for p in &problems {
        text.push_str(&format!("  ! {p}\n"));
    }
    let v = json!({"scene": scene, "other_work_cores": {"base": ob, "change": oc}, "unit": match unit { Unit::VizFrame => "ms a visualizer frame", Unit::WindowFrame => "ms a window frame", Unit::Second => "% of a core" }, "verdict": verdict, "problems": problems, "measures": rows});
    (text, v, !refused_scene && !busy_machine)
}

fn pass_json(side: &str, r: &PassResult) -> Value {
    json!({
        "side": side, "secs": r.secs, "errors": r.errors, "hello": r.hello,
        "scenes": r.scenes.iter().map(|s| json!({"scene": s.scene, "app": s.app, "window_server_core": s.window_server, "machine_busy_cores": s.machine, "gpu_pct": s.gpu, "cpu_watts": s.cpu_watts, "gpu_watts": s.gpu_watts, "battery_watts": s.battery_watts})).collect::<Vec<_>>(),
    })
}

// ---- --snap ------------------------------------------------------------------------------------

/// The two builds' pictures of the same frame, compared: how many pixels differ, by how much
/// at most and on average, and where.
fn snap_compare(runs: &Path, scene: &str, n: u64) -> String {
    let path = |side: &str| runs.join(format!("r1-{side}")).join(format!("perf-{scene}-{n}.rgba"));
    let (pb, pc) = (path("base"), path("change"));
    let (Ok(a), Ok(b)) = (std::fs::read(&pb), std::fs::read(&pc)) else {
        return format!("a picture is missing ({}, {}): was the frame reached in both builds?", pb.display(), pc.display());
    };
    match diff_rgba(&a, &b) {
        Ok(d) => format!("{d} (pictures: {} and {})", pb.with_extension("png").display(), pc.with_extension("png").display()),
        Err(e) => e,
    }
}

/// Two pictures in the app's `.rgba` form (`RGBA`, width and height as little-endian u32,
/// then the pixels), compared.
pub fn diff_rgba(a: &[u8], b: &[u8]) -> Result<String, String> {
    let head = |x: &[u8]| -> Option<(u32, u32)> {
        (x.len() >= 12 && &x[..4] == b"RGBA").then(|| (u32::from_le_bytes(x[4..8].try_into().unwrap_or_default()), u32::from_le_bytes(x[8..12].try_into().unwrap_or_default())))
    };
    let (Some((w, h)), Some(hb)) = (head(a), head(b)) else { return Err("not the app's .rgba pictures".into()) };
    if (w, h) != hb {
        return Err(format!("the pictures differ in size: {w} x {h} and {} x {}", hb.0, hb.1));
    }
    let n = (w as usize) * (h as usize);
    let (pa, pb) = (&a[12..], &b[12..]);
    if pa.len() < n * 4 || pb.len() < n * 4 {
        return Err("a picture is cut short".into());
    }
    let (mut differ, mut most, mut sum) = (0usize, 0u8, 0u64);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for i in 0..n {
        let d = (0..4).map(|k| pa[i * 4 + k].abs_diff(pb[i * 4 + k])).max().unwrap_or(0);
        if d > 0 {
            differ += 1;
            most = most.max(d);
            sum += u64::from(d);
            let (x, y) = ((i % w as usize) as u32, (i / w as usize) as u32);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if differ == 0 {
        return Ok(format!("identical ({w} x {h})"));
    }
    Ok(format!(
        "{differ} of {n} pixels differ ({:.3}%), by at most {most} and {:.1} on average (of 255), within x {x0}..{x1}, y {y0}..{y1}",
        100.0 * differ as f64 / n as f64,
        sum as f64 / differ as f64
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: u32, h: u32, px: &[[u8; 4]]) -> Vec<u8> {
        let mut v = b"RGBA".to_vec();
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        px.iter().for_each(|p| v.extend(p));
        v
    }

    #[test]
    fn pictures_compared_say_how_many_pixels_differ_and_where() {
        let a = rgba(2, 2, &[[0, 0, 0, 255]; 4]);
        assert_eq!(diff_rgba(&a, &a).unwrap(), "identical (2 x 2)");
        let b = rgba(2, 2, &[[0, 0, 0, 255], [0, 0, 0, 255], [0, 10, 0, 255], [0, 0, 0, 255]]);
        let d = diff_rgba(&a, &b).unwrap();
        assert!(d.starts_with("1 of 4 pixels differ (25.000%), by at most 10"), "{d}");
        assert!(d.ends_with("within x 0..0, y 1..1"), "{d}");
        assert!(diff_rgba(&a, &rgba(1, 1, &[[0; 4]])).is_err());
    }

    #[test]
    fn scenes_are_named_in_order_once_and_all_is_every_one() {
        assert_eq!(parse_scenes("bars, home,bars").unwrap(), ["bars", "home"]);
        assert_eq!(parse_scenes("all").unwrap().len(), SCENES.len());
        assert!(parse_scenes("bars,nope").is_err());
        assert!(parse_scenes(" , ").is_err());
    }
}
