//! Every bench mode (`MP_BENCH`), each on its own data folder and local mocks of LRCLIB and
//! Last.fm (nothing leaves this machine), then one pass/fail table. The release's end-to-end
//! evidence.
//!
//!     cargo xtask bench                                    # fast profile, quick, parallel
//!     cargo xtask bench -- --only skip,gapless             # named modes
//!     cargo xtask bench -- --changed                       # modes for uncommitted changes
//!     cargo xtask bench -- --for crates/app/src/visualizer # modes for a path
//!     cargo xtask bench --profile release --all            # full release pass
//!
//! Options:
//!
//! - `--exe <path>`: the app built with the bench feature (default: `MarqueePlayer[.exe]` beside
//!   this runner, in the same target folder);
//! - `--fixtures <dir>`: made by `bench_fixtures` if missing (default `<temp>/mp-bench-fixtures`);
//! - `--out <dir>`: one folder per mode with its data, reports, snapshots and log
//!   (default `<temp>/mp-bench-runs`; emptied first, and must be in the temporary folder);
//! - `--only a,b`, `--skip a,b`: modes by name (`--only` overrides `--quick`);
//! - `--quick` / `--all`: skip or include the long modes (`LONG`);
//! - `--jobs N` / `-j N`: how many parallel-safe modes run at once (default from the core count;
//!   `1` runs serially); modes marked `alone` always run one at a time afterwards;
//! - `--cold`: skip pre-scanned library templates and give every mode its own copy of its sets
//!   (`--jobs 1 --cold` reproduces the original serial cold-scan run);
//! - `--for <path>` (repeatable or comma-separated): pick modes whose subject lives in those
//!   paths (any unmapped path runs everything);
//! - `--changed [<git ref>]`: like `--for` on files changed in git (against `<ref>`, or against
//!   `HEAD` plus untracked files when omitted);
//! - `--list`: print every mode with its flags; `--keep`: keep each mode's music copy.
//!
//! A mode passes when the app exits 0 (every check passed: `bench::exit_code`), fails on
//! exit 3 (failed checks; the report has `[FAIL]` lines), and is a crash on any other code
//! (101 is a panic) or a hang when it outlives its time limit (it is then killed: only that
//! child process, by its handle). The app runs with `MP_VOLUME=0`, `MP_WINDOW=secondary` and
//! every network address pointing at the mocks; the bench harness itself opens no native
//! dialog and writes nothing to the clipboard.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// A bench mode and what it needs.
struct Mode {
    name: &'static str,
    /// `MP_BENCH`'s value (several entries can run one mode with different settings).
    bench: &'static str,
    /// Fixture sets needed (`<fixtures>/<set>` or `<music>/<set>`), in this order.
    sets: &'static [&'static str],
    /// Seconds before it counts as a hang (also used to schedule long parallel modes first).
    limit: u64,
    env: &'static [(&'static str, &'static str)],
    /// Runs off Windows too, on Linux and macOS (the rest need Windows: GDI counts, the tray's
    /// Win32 icon, WASAPI's device list, a resize without activation).
    linux: bool,
    /// Must run with nothing else running: measures frame intervals, CPU %, idle wake-ups,
    /// UI lag, real-time audio/beat timing, or changes the system default output device.
    alone: bool,
    /// Must start with an empty database and scan from nothing (its subject is the scan,
    /// grouping, guard, session moves, damaged files, or it mutates its own music copy).
    cold: bool,
    /// Writes to, renames, or deletes files in its music folder: must get its own writable
    /// clone/copy rather than pointing directly at `<fixtures>/<set>`.
    mutates: bool,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Plain,
    /// `zoom` (the interface size, left at 150%), then `zoom-check`: the next start keeps it.
    Zoom,
    /// `session-save`, a folder moved, then `session-check` with a second folder to remove.
    Session,
    /// A first run scans, then nineteen in twenty files go: the guard must hold the tracks back.
    Guard,
    /// The first scan of a large library (the `big` set, scanned where it is, never copied): a
    /// run quit mid-scan, then one that must finish it.
    BigScan,
    /// The first scan of the messy library on a slow disk (simulated, `MP_SLOW_DISK_MS`): a run quit
    /// 40% through, then one that must pick it up and finish it.
    SlowScan,
    /// The app's own files misbehaving (`scenario`): launched as `idle-stopped`, checked here.
    Scenario(Scenario),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Scenario {
    /// settings.json with values it cannot read, lastfm.json and scrobbles.json damaged,
    /// theme.toml broken, a junk record in the tag editor's `inflight`.
    DataFiles,
    /// library.db is not a database: refused with a message; then "start afresh" (answered yes).
    DbGarbage,
    /// library.db from a newer build: refused with a message, left untouched.
    DbNewer,
    /// library.db with bytes flipped inside (the header intact): what the app makes of it.
    DbFlipped,
    /// The data folder cannot be written to.
    DataReadOnly,
    /// Two launches at once on one data folder: one runs, the other hands over and leaves.
    TwoInstances,
    /// A deliberate panic on the UI thread, then on another: told, logged with a backtrace, the
    /// log kept as the previous one after the next start.
    Crash,
}

/// The modes that are long by design (minutes of playing, or scanning a large library): left out
/// by `--quick`, run by `--all` before a release.
const LONG: &[&str] = &["soak", "bigscan", "slowscan"];

const MODES: &[Mode] = &[
    Mode { name: "tour", bench: "tour", sets: &["core"], limit: 120, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Measures scroll frame intervals and render times.
    Mode { name: "grid", bench: "grid", sets: &["core"], limit: 120, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Measures sidebar slide frame intervals and render times.
    Mode { name: "sidebar", bench: "sidebar", sets: &["core"], limit: 120, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Asserts visible covers decode within 3 s after a 40,000 px flick.
    Mode { name: "covers", bench: "covers", sets: &["core"], limit: 120, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Measures playlist fill/first-frame time and asserts Ctrl+A / Shift+End < 50 ms on 3,000 rows.
    Mode { name: "playlist", bench: "playlist", sets: &["core"], limit: 120, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Real-time seek-to-end and gapless transition position window.
    Mode { name: "gapless", bench: "gapless", sets: &["core"], limit: 90, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Its unplayable track is a 5.1 AAC file: the one Windows' ffmpeg (9) writes is one Symphonia
    // refuses, the one Ubuntu 24.04's (6.1) writes it decodes, so on Linux the album has nothing to
    // skip. `skip-damaged` covers the same path on both. Tight gapless skip timing.
    Mode { name: "skip", bench: "skip", sets: &["core"], limit: 90, env: &[], linux: false, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "skip-damaged", bench: "skip", sets: &["core"], limit: 90, env: &[("MP_BENCH_SKIP_ALBUM", "Damaged")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "theme", bench: "theme", sets: &["core"], limit: 90, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Moves an album folder and moves `formats` between two runs; must scan cold on its own copy.
    Mode { name: "session", bench: "session-save", sets: &["core", "formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: true, kind: Kind::Session },
    // Scans cold, deletes 19 in 20 files from its own copy, and verifies the shrink guard.
    Mode { name: "guard", bench: "guard", sets: &["core"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: true, kind: Kind::Guard },
    // Idle CPU % and wake-up measurements while hidden.
    Mode { name: "idle-paused", bench: "idle-paused", sets: &["core"], limit: 90, env: &[("MP_BENCH_IDLE_SECS", "10")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "idle-stopped", bench: "idle-stopped", sets: &["core"], limit: 90, env: &[("MP_BENCH_IDLE_SECS", "10")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "phase4", bench: "phase4", sets: &["core"], limit: 240, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "phase5", bench: "phase5", sets: &["core"], limit: 240, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "phase5-open", bench: "phase5", sets: &["core"], limit: 120, env: &[("MP_BENCH_PART", "open")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "cleanup", bench: "cleanup", sets: &["core"], limit: 240, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "settings", bench: "settings", sets: &["core"], limit: 240, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Asserts middle-click autoscroll speed matches its curve within 10% + 3 px/s.
    Mode { name: "autoscroll", bench: "autoscroll", sets: &["core"], limit: 300, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Measures tray animation CPU % and Windows GDI/USER object counts.
    Mode { name: "tray", bench: "tray", sets: &["core"], limit: 120, env: &[("MP_BENCH_TRAY_SECS", "8"), ("MP_BENCH_IDLE_SECS", "8")], linux: false, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "about", bench: "about", sets: &["core"], limit: 180, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "details", bench: "details", sets: &["formats"], limit: 300, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "details-off", bench: "details", sets: &["formats"], limit: 300, env: &[("MP_BENCH_DETAILS", "off")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Two tiny sets (23 files); asserts frequency-sweep peak timing and measures CPU/frames.
    Mode { name: "visualizer", bench: "visualizer", sets: &["viz", "words"], limit: 300, env: &[], linux: true, alone: true, cold: true, mutates: false, kind: Kind::Plain },
    // Writes a .lrc, a .txt and a tag into its own copy of the core set (marked for the editor).
    Mode { name: "lyrics", bench: "lyrics", sets: &["core"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: true, kind: Kind::Plain },
    // The interface size, then the next start on the same data folder keeping it.
    Mode { name: "zoom", bench: "zoom", sets: &["core"], limit: 180, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Zoom },
    // Measures visualizer active vs idle CPU % and wake-ups.
    Mode { name: "viz-idle", bench: "viz-idle", sets: &["viz"], limit: 120, env: &[("MP_BENCH_VIZ_SECS", "6"), ("MP_BENCH_IDLE_SECS", "10")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Checks `visualizer::running()` across `resize_client` and captures 17 ms frame strips;
    // flaked under `--jobs` on macOS (window occlusion after resize + CoreAudio underruns).
    Mode { name: "viz-styles", bench: "viz-styles", sets: &["viz"], limit: 600, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Measures per-style FPS, frame intervals, work per frame, and CPU %.
    Mode { name: "viz-cost", bench: "viz-cost", sets: &["viz"], limit: 400, env: &[("MP_BENCH_VIZ_SECS", "2")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Asserts real-time beat-tracker tempo (±3 BPM) and kick counts over 8 s + 50 ms pulse polls.
    Mode { name: "viz-look", bench: "viz-look", sets: &["viz"], limit: 600, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Its Navidrome stand-in runs inside the app (bench/server.rs), on 127.0.0.1. Checks the
    // start-up scan activity log and removes folders (`formats` scans in ~0.1 s).
    Mode { name: "server", bench: "server", sets: &["formats"], limit: 300, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Plain },
    // Its ListenBrainz stand-in runs inside the app (bench/listenbrainz.rs), on 127.0.0.1, reached through
    // the card's own server address.
    Mode { name: "listenbrainz", bench: "listenbrainz", sets: &["core"], limit: 180, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Deletes and corrupts files in its own copy of `meta` after scanning.
    Mode { name: "metadata", bench: "metadata", sets: &["meta"], limit: 300, env: &[], linux: true, alone: false, cold: true, mutates: true, kind: Kind::Plain },
    // `messy_library.rs` polls for the start-up scan (cold scan is faster than its 5 s fallback).
    Mode { name: "home", bench: "home", sets: &["messy"], limit: 180, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Plain },
    // The hero's play button and strip, the shelf editor's rows, grid shelves at several sizes.
    Mode { name: "home-shelves", bench: "home-shelves", sets: &["core"], limit: 300, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Checks how the scanner groups messy tags, compilations, decomposed Unicode and genres.
    Mode { name: "library", bench: "library", sets: &["messy"], limit: 240, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Plain },
    // Writes tags and toggles read-only flags in its own copy of `edit`.
    Mode { name: "editing", bench: "editing", sets: &["edit"], limit: 400, env: &[], linux: true, alone: false, cold: true, mutates: true, kind: Kind::Plain },
    Mode { name: "smart", bench: "smart", sets: &["messy120"], limit: 400, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "eq", bench: "eq", sets: &["messy"], limit: 300, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "eq-paste", bench: "eq-paste", sets: &["messy"], limit: 300, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Linux: moves PipeWire's default between the headless session's two null sinks (only
    // those; elsewhere it checks the start and changes nothing). On Windows it says so and ends.
    // Mutates system audio default on Linux, so it must run alone.
    Mode { name: "output-default", bench: "output-default", sets: &["core"], limit: 90, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // The pointer resting on a track row's play button, at whole and fractional scales.
    Mode { name: "row-hover", bench: "row-hover", sets: &["core"], limit: 90, env: &[], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "row-hover-125", bench: "row-hover", sets: &["core"], limit: 90, env: &[("SLINT_SCALE_FACTOR", "1.25")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "row-hover-150", bench: "row-hover", sets: &["core"], limit: 90, env: &[("SLINT_SCALE_FACTOR", "1.5")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // And at the interface sizes' ends (zoom.rs), with the frameless window's edge line.
    Mode { name: "row-hover-z70", bench: "row-hover", sets: &["core"], limit: 90, env: &[("MP_BENCH_ZOOM", "70")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    Mode { name: "row-hover-z200", bench: "row-hover", sets: &["core"], limit: 90, env: &[("MP_BENCH_ZOOM", "200")], linux: true, alone: false, cold: false, mutates: false, kind: Kind::Plain },
    // Asserts UI thread lag < 250 ms and checks memory/handle/GDI growth over 180 s.
    Mode { name: "soak", bench: "soak", sets: &["core"], limit: 400, env: &[("MP_BENCH_SOAK_SECS", "180")], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Real-time 1 s crossfade and sample-rate conversion checks at fixed wall-clock windows.
    Mode { name: "xfade-rates", bench: "xfade-rates", sets: &["formats"], limit: 120, env: &[], linux: true, alone: true, cold: false, mutates: false, kind: Kind::Plain },
    // Its own fixtures (ffmpeg) and its own mock of a Navidrome server, in the app: no set needed.
    // Asserts time-to-first-sound < 0.4x download, cancel < 2 s, and gapless streaming timing.
    Mode { name: "streaming", bench: "streaming", sets: &[], limit: 300, env: &[], linux: true, alone: true, cold: true, mutates: false, kind: Kind::Plain },
    // First scan of `big`: asserts UI lag < 250 ms, DB write < 150 ms, mid-scan quit < 20 s.
    Mode { name: "bigscan", bench: "bigscan", sets: &[], limit: 1800, env: &[], linux: true, alone: true, cold: true, mutates: false, kind: Kind::BigScan },
    // First scan of `messy` with `MP_SLOW_DISK_MS=8`: asserts UI lag < 250 ms and DB write < 150 ms.
    Mode { name: "slowscan", bench: "bigscan", sets: &["messy"], limit: 900, env: &[("MP_SLOW_DISK_MS", "8")], linux: true, alone: true, cold: true, mutates: false, kind: Kind::SlowScan },
    // Scenarios: test damaged/missing/unwritable data files, DB refusal, single-instance and panics.
    Mode { name: "data-files", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::DataFiles) },
    Mode { name: "db-garbage", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::DbGarbage) },
    Mode { name: "db-newer", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::DbNewer) },
    Mode { name: "db-flipped", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::DbFlipped) },
    Mode { name: "data-readonly", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: false, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::DataReadOnly) },
    // Sleeps 1.5 s between two launches and asserts the second hands over and exits < 10 s.
    Mode { name: "two-instances", bench: "idle-stopped", sets: &["formats"], limit: 120, env: &[], linux: true, alone: true, cold: true, mutates: false, kind: Kind::Scenario(Scenario::TwoInstances) },
    Mode { name: "crash", bench: "crash-ui", sets: &["formats"], limit: 120, env: &[], linux: true, alone: false, cold: true, mutates: false, kind: Kind::Scenario(Scenario::Crash) },
];

// ---- path-to-mode mapping (--for / --changed) -----------------------------------------------

/// Honest mapping from source paths to the bench modes whose subject lives there.
/// Matched by longest matching prefix (exact file or directory prefix ending in `/`).
/// Any path that matches nothing — or matches an empty mode list — falls back to running
/// every mode, so shared/core code (`main.rs`, `state.rs`, `views.rs`, `app.slint`,
/// `player-core/src/engine.rs`, `library/src/db.rs`, `Cargo.toml`, `bench-runner`) is never
/// skipped by accident.
const PATH_RULES: &[(&str, &[&str])] = &[
    // Individual bench modules.
    ("crates/app/src/bench/about.rs", &["about"]),
    ("crates/app/src/bench/autoscroll.rs", &["autoscroll", "tray"]),
    ("crates/app/src/bench/bigscan.rs", &["bigscan", "slowscan", "soak"]),
    ("crates/app/src/bench/details.rs", &["details", "details-off"]),
    ("crates/app/src/bench/editing.rs", &["editing"]),
    ("crates/app/src/bench/eq.rs", &["eq"]),
    ("crates/app/src/bench/eq_paste.rs", &["eq-paste"]),
    ("crates/app/src/bench/home_shelves.rs", &["home-shelves"]),
    ("crates/app/src/bench/listenbrainz.rs", &["listenbrainz"]),
    ("crates/app/src/bench/lyrics.rs", &["lyrics"]),
    ("crates/app/src/bench/messy_library.rs", &["home", "library"]),
    ("crates/app/src/bench/metadata.rs", &["metadata"]),
    ("crates/app/src/bench/output_default.rs", &["output-default"]),
    ("crates/app/src/bench/output_devices.rs", &["phase5", "phase5-open"]),
    ("crates/app/src/bench/player_bar.rs", &["cleanup"]),
    ("crates/app/src/bench/playlist.rs", &["playlist"]),
    ("crates/app/src/bench/row_hover.rs", &["row-hover", "row-hover-125", "row-hover-150", "row-hover-z70", "row-hover-z200"]),
    ("crates/app/src/bench/selection.rs", &["playlist"]),
    ("crates/app/src/bench/server.rs", &["server"]),
    ("crates/app/src/bench/settings.rs", &["settings"]),
    ("crates/app/src/bench/smart.rs", &["smart"]),
    ("crates/app/src/bench/streaming.rs", &["streaming"]),
    ("crates/app/src/bench/visualizer.rs", &["visualizer", "viz-idle"]),
    ("crates/app/src/bench/viz_look.rs", &["viz-look"]),
    ("crates/app/src/bench/viz_styles.rs", &["viz-styles", "viz-cost"]),
    ("crates/app/src/bench/xfade_rates.rs", &["xfade-rates"]),
    ("crates/app/src/bench/zoom.rs", &["zoom"]),
    // Visualizer.
    ("crates/app/src/visualizer/", &["visualizer", "viz-idle", "viz-styles", "viz-cost", "viz-look"]),
    ("crates/app/ui/vizstage.slint", &["visualizer", "viz-idle", "viz-styles", "viz-cost", "viz-look"]),
    ("crates/player-core/src/spectrum.rs", &["visualizer", "viz-idle", "viz-styles", "viz-cost", "viz-look"]),
    // Equalizer (`viz-look` also checks the pinned EQ curve).
    ("crates/app/src/eq.rs", &["eq", "eq-paste", "viz-look", "settings"]),
    ("crates/app/ui/eq.slint", &["eq", "eq-paste", "viz-look", "settings"]),
    ("crates/player-core/src/eq.rs", &["eq", "eq-paste", "viz-look", "settings"]),
    ("crates/player-core/src/eq/", &["eq", "eq-paste", "viz-look", "settings"]),
    // Lyrics & LRCLIB (`visualizer` exercises the LRCLIB mock on the "Words" album).
    ("crates/app/src/lyrics/", &["lyrics", "visualizer", "streaming", "phase4"]),
    ("crates/lrclib/", &["lyrics", "visualizer", "streaming", "phase4"]),
    // Connections: ListenBrainz, Last.fm, Discord.
    ("crates/listenbrainz/", &["listenbrainz", "settings", "about"]),
    ("crates/app/src/connections/listens.rs", &["listenbrainz", "settings", "about"]),
    ("crates/lastfm/", &["phase4", "settings", "data-files", "about"]),
    ("crates/app/src/connections/scrobble.rs", &["phase4", "settings", "data-files", "about"]),
    ("crates/discord-ipc/", &["settings", "about"]),
    ("crates/app/src/connections/presence.rs", &["settings", "about"]),
    // Server & streaming.
    ("crates/app/src/server/streaming.rs", &["streaming", "server"]),
    ("crates/app/src/server/streammock.rs", &["streaming", "server"]),
    ("crates/http/src/audio.rs", &["streaming", "server"]),
    ("crates/player-core/src/stream.rs", &["streaming", "server"]),
    ("crates/app/src/server/", &["server", "streaming", "lyrics", "smart"]),
    ("crates/subsonic/", &["server", "streaming", "lyrics", "smart"]),
    ("crates/library/src/remote.rs", &["server", "streaming", "lyrics", "smart"]),
    // About, autoscroll, tray, zoom.
    ("crates/app/src/about.rs", &["about", "settings"]),
    ("crates/app/src/release_number.rs", &["about", "settings"]),
    ("crates/app/src/autoscroll.rs", &["autoscroll"]),
    ("crates/app/ui/autoscroll.slint", &["autoscroll"]),
    ("crates/app/src/tray/", &["tray", "about", "idle-paused", "idle-stopped", "settings"]),
    ("crates/app/src/zoom.rs", &["zoom", "row-hover-z70", "row-hover-z200", "home-shelves", "settings"]),
    // Metadata, tag editing, technical details.
    ("crates/app/src/metadata/details.rs", &["details", "details-off", "xfade-rates", "streaming"]),
    ("crates/app/src/metadata/tagedit.rs", &["editing", "lyrics", "metadata", "data-files"]),
    ("crates/library/src/edit/", &["editing", "lyrics", "metadata", "data-files"]),
    ("crates/app/src/metadata/", &["metadata", "editing", "details", "details-off", "library"]),
    ("crates/app/ui/metaview.slint", &["metadata", "editing", "details", "details-off", "library"]),
    ("crates/library/src/meta/", &["metadata", "editing", "details", "details-off", "library"]),
    // Playlists, smart playlists, multi-select.
    ("crates/app/src/playlists/smart.rs", &["smart", "playlist"]),
    ("crates/app/src/playlists/smartedit.rs", &["smart", "playlist"]),
    ("crates/app/ui/smart.slint", &["smart", "playlist"]),
    ("crates/library/src/smart/", &["smart", "playlist"]),
    ("crates/app/src/playlists/", &["playlist", "smart", "phase4"]),
    ("crates/app/ui/pages/playlists.slint", &["playlist", "smart", "phase4"]),
    ("crates/app/src/selected.rs", &["playlist"]),
    ("crates/app/src/selection.rs", &["playlist"]),
    // Specific pages & covers.
    ("crates/app/ui/pages/home.slint", &["home", "home-shelves", "smart", "server", "tour"]),
    ("crates/app/ui/pages/shelves.slint", &["home", "home-shelves", "smart", "server", "tour"]),
    ("crates/app/ui/pages/album.slint", &["library", "playlist", "row-hover", "row-hover-125", "row-hover-150", "row-hover-z70", "row-hover-z200", "tour", "phase4"]),
    ("crates/app/ui/pages/artist.slint", &["library", "tour", "phase4"]),
    ("crates/app/ui/pages/search.slint", &["smart", "library", "playlist", "phase4"]),
    ("crates/app/src/search.rs", &["smart", "library", "playlist", "phase4"]),
    ("crates/app/ui/pages/browse.slint", &["grid", "covers", "library", "zoom", "tour", "phase4"]),
    ("crates/app/src/covers.rs", &["grid", "covers", "library", "zoom", "tour", "phase4"]),
    ("crates/app/ui/pages/nowplaying.slint", &["visualizer", "viz-styles", "viz-look", "viz-cost", "viz-idle", "lyrics", "details", "details-off", "cleanup", "tour"]),
    // Crossfade, resampling, audio output.
    ("crates/app/src/crossfade.rs", &["xfade-rates", "gapless", "streaming", "settings"]),
    ("crates/player-core/src/crossfade.rs", &["xfade-rates", "gapless", "streaming", "settings"]),
    ("crates/player-core/src/resample.rs", &["xfade-rates", "gapless", "streaming", "settings"]),
    ("crates/app/src/output.rs", &["phase5", "output-default", "xfade-rates", "gapless", "skip", "skip-damaged", "idle-paused", "idle-stopped"]),
    ("crates/player-core/src/output.rs", &["phase5", "output-default", "xfade-rates", "gapless", "skip", "skip-damaged", "idle-paused", "idle-stopped"]),
    ("crates/player-core/src/output/", &["phase5", "output-default", "xfade-rates", "gapless", "skip", "skip-damaged", "idle-paused", "idle-stopped"]),
    // Scanning, session, reroot.
    ("crates/app/src/scanning.rs", &["bigscan", "slowscan", "guard", "session", "library", "home", "server", "phase4", "editing", "metadata"]),
    ("crates/library/src/scan.rs", &["bigscan", "slowscan", "guard", "session", "library", "home", "server", "phase4", "editing", "metadata"]),
    ("crates/library/src/scan/", &["bigscan", "slowscan", "guard", "session", "library", "home", "server", "phase4", "editing", "metadata"]),
    ("crates/app/src/session.rs", &["session", "guard"]),
    ("crates/library/src/session.rs", &["session", "guard"]),
    ("crates/library/src/reroot.rs", &["session", "guard"]),
    // Crash, logging, theme, keys, opener/DnD.
    ("crates/app/src/crash.rs", &["crash", "data-files", "db-garbage", "db-newer", "db-flipped", "data-readonly", "two-instances"]),
    ("crates/app/src/logging.rs", &["crash", "data-files", "db-garbage", "db-newer", "db-flipped", "data-readonly", "two-instances"]),
    ("crates/app/src/theme/", &["theme", "data-files", "viz-styles", "viz-look"]),
    ("crates/app/src/keys/", &["settings", "zoom", "viz-look", "viz-styles", "phase4", "cleanup", "playlist"]),
    ("crates/app/src/opener.rs", &["phase5-open", "playlist"]),
    ("crates/app/src/dnd.rs", &["phase5-open", "playlist"]),
];

fn normalize_repo_path(raw: &str, repo_root: Option<&Path>) -> String {
    let mut s = raw.replace('\\', "/");
    if let Some(root) = repo_root {
        let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/").trim_end_matches('/'));
        if let Some(stripped) = s.strip_prefix(&prefix) {
            s = stripped.to_owned();
        }
    }
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_owned();
    }
    s
}

/// Map one repo-relative path to modes by the longest matching rule in `PATH_RULES`.
/// `None` means unmapped (caller must run all modes).
fn modes_for_one_path(norm: &str) -> Option<&'static [&'static str]> {
    let mut best: Option<(usize, &'static [&'static str])> = None;
    for &(pat, modes) in PATH_RULES {
        let matched = if pat.ends_with('/') {
            norm.starts_with(pat) || norm == pat.trim_end_matches('/')
        } else {
            norm == pat
        };
        if matched && best.is_none_or(|(len, _)| pat.len() > len) {
            best = Some((pat.len(), modes));
        }
    }
    best.and_then(|(_, m)| (!m.is_empty()).then_some(m))
}

#[derive(Debug, PartialEq, Eq)]
enum PathSelection {
    /// Every path mapped cleanly; run these mode names.
    Modes(BTreeSet<String>),
    /// At least one path is unmapped (`String` is the first such path); run everything.
    AllBecause(String),
}

fn modes_for_paths(paths: &[String], repo_root: Option<&Path>) -> PathSelection {
    let mut chosen = BTreeSet::new();
    for raw in paths {
        let norm = normalize_repo_path(raw, repo_root);
        if norm.is_empty() {
            continue;
        }
        match modes_for_one_path(&norm) {
            Some(ms) => {
                for m in ms {
                    chosen.insert((*m).to_owned());
                }
            }
            None => return PathSelection::AllBecause(norm),
        }
    }
    PathSelection::Modes(chosen)
}

fn git_root() -> Option<PathBuf> {
    let out = Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok()?;
    out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Files changed in git (against `git_ref`, or uncommitted + untracked against `HEAD`).
fn git_changed_paths(git_ref: Option<&str>) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    let diff_args: Vec<&str> = match git_ref {
        Some(r) => vec!["diff", "--name-only", r],
        None => vec!["diff", "--name-only", "HEAD"],
    };
    let out = Command::new("git").args(&diff_args).output().map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", diff_args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let t = l.trim();
        if !t.is_empty() {
            files.insert(t.to_owned());
        }
    }
    let untracked = Command::new("git").args(["ls-files", "--others", "--exclude-standard"]).output().map_err(|e| format!("running git ls-files: {e}"))?;
    if untracked.status.success() {
        for l in String::from_utf8_lossy(&untracked.stdout).lines() {
            let t = l.trim();
            if !t.is_empty() {
                files.insert(t.to_owned());
            }
        }
    }
    Ok(files.into_iter().collect())
}

// ---- the mocks ------------------------------------------------------------------------------

/// A tiny HTTP/1.1 server on 127.0.0.1: one request per connection, answered by `handle`, every
/// request logged. `None` from the handler closes the connection without an answer (offline).
struct Mock {
    url: String,
    log: Arc<Mutex<Vec<String>>>,
}

type Answer = Option<(u16, String)>;

fn serve(handle: impl Fn(&str, &str, &HashMap<String, String>) -> Answer + Send + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port for the mock");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let l = log.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
            if let Some((method, path, form)) = read_request(&conn) {
                l.lock().unwrap().push(format!("{method} {path} {form:?}"));
                if let Some((status, body)) = handle(&method, &path, &form) {
                    let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let mut c = &conn;
                    let _ = c.write_all(resp.as_bytes());
                }
            }
        }
    });
    Mock { url, log }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 3 <= b.len() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn form(s: &str) -> HashMap<String, String> {
    s.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect()
}

fn read_request(conn: &TcpStream) -> Option<(String, String, HashMap<String, String>)> {
    let mut r = BufReader::new(conn);
    let mut first = String::new();
    r.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        if h == "\r\n" || h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut f = form(query);
    f.extend(form(&String::from_utf8_lossy(&body)));
    Some((method, path.to_owned(), f))
}

fn record(id: i64, title: &str, synced: Option<&str>, plain: Option<&str>, instrumental: bool) -> String {
    let s = |v: Option<&str>| v.map_or("null".to_owned(), |t| format!("{:?}", t));
    format!(
        r#"{{"id":{id},"trackName":{title:?},"artistName":"Fixture Voices","albumName":"Words","duration":8.0,"instrumental":{instrumental},"plainLyrics":{},"syncedLyrics":{}}}"#,
        s(plain),
        s(synced)
    )
}

/// LRCLIB as `bench/visualizer.rs` expects it for the "Words" album: each title names its answer.
fn lrclib() -> Mock {
    const SYNCED: &str = "[00:00.50]The road runs on\n[00:01.50]lights in the rain\n[00:02.50]no one awake\n[00:03.50]but us\n[00:04.50]and the radio";
    serve(|_, path, q| {
        let title = q.get("track_name").cloned().unwrap_or_default();
        let found = |r: String| Some((200, r));
        let none = || Some((404, r#"{"code":404,"name":"TrackNotFound","message":"Failed to find specified track"}"#.to_owned()));
        if path == "/api/search" {
            return match title.as_str() {
                "Night Drive" => Some((200, format!("[{}]", record(101, "Night Drive", Some(SYNCED), Some("The road runs on"), false)))),
                _ => Some((200, "[]".into())),
            };
        }
        match title.as_str() {
            "Night Drive" => found(record(101, "Night Drive", Some(SYNCED), Some("The road runs on"), false)),
            "Paper Lantern" => found(record(102, "Paper Lantern", None, Some("A paper lantern\nfloats up the hill\nand the hill lets it go"), false)),
            "Interlude" => found(record(103, "Interlude", None, None, true)),
            "Static" => Some((500, r#"{"code":500,"name":"InternalServerError"}"#.into())),
            "Dropout" => None,
            "Skip Four" => found(record(104, "Skip Four", Some(SYNCED), None, false)),
            _ => none(),
        }
    })
}

/// Last.fm's API for a connected session: now playing and scrobbles accepted.
fn lastfm() -> Mock {
    let m = serve(|_, _, f| {
        let method = f.get("method").map(String::as_str).unwrap_or("");
        Some(match method {
            "track.updateNowPlaying" => (200, r##"{"nowplaying":{"ignoredMessage":{"code":"0","#text":""}}}"##.into()),
            "track.scrobble" => (200, r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":0},"scrobble":{"ignoredMessage":{"code":"0","#text":""}}}}"##.into()),
            _ => (200, r#"{"error":10,"message":"Invalid API key (mock)"}"#.into()),
        })
    });
    Mock { url: format!("{}/2.0/", m.url), log: m.log }
}

// ---- files & copy-on-write cloning ----------------------------------------------------------

/// macOS APFS `clonefile(2)`: clones a file or an entire directory hierarchy copy-on-write
/// in one syscall. Falls back to `false` across volumes or on non-APFS file systems.
#[cfg(target_os = "macos")]
fn try_clonefile(from: &Path, to: &Path) -> bool {
    use std::ffi::{CString, c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn clonefile(src: *const c_char, dst: *const c_char, flags: u32) -> c_int;
    }
    let (Ok(src), Ok(dst)) = (CString::new(from.as_os_str().as_bytes()), CString::new(to.as_os_str().as_bytes())) else {
        return false;
    };
    // SAFETY: both pointers are valid NUL-terminated C strings alive for the call.
    unsafe { clonefile(src.as_ptr(), dst.as_ptr(), 0) == 0 }
}

/// Linux `FICLONE` ioctl (btrfs, xfs, bcachefs): reflinks one file copy-on-write.
#[cfg(target_os = "linux")]
fn try_reflink(from: &Path, to: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    #[cfg(target_env = "musl")]
    type IoctlReq = std::ffi::c_int;
    #[cfg(not(target_env = "musl"))]
    type IoctlReq = std::ffi::c_ulong;
    const FICLONE: IoctlReq = 0x4004_9409;
    unsafe extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: IoctlReq, ...) -> std::ffi::c_int;
    }
    let Ok(src) = std::fs::File::open(from) else { return false };
    let Ok(dst) = std::fs::File::create(to) else { return false };
    // SAFETY: both descriptors are open for the duration of the ioctl.
    let ok = unsafe { ioctl(dst.as_raw_fd(), FICLONE, src.as_raw_fd()) == 0 };
    if ok {
        if let Ok(meta) = src.metadata() {
            let _ = std::fs::set_permissions(to, meta.permissions());
        }
    } else {
        drop(dst);
        let _ = std::fs::remove_file(to);
    }
    ok
}

/// Clone a single file copy-on-write where the file system supports it, else copy it.
fn clone_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(target_os = "macos")]
    {
        if to.exists() {
            let _ = std::fs::remove_file(to);
        }
        if try_clonefile(from, to) {
            return Ok(());
        }
    }
    #[cfg(target_os = "linux")]
    if try_reflink(from, to) {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    Ok(())
}

/// Clone a directory tree copy-on-write where supported (`clonefile` on APFS, `FICLONE`
/// per file on Linux), falling back to a plain copy (and always copying on Windows, where
/// read-only modes point directly at `<fixtures>/<set>` and only mutating/multi-set modes
/// copy so hard links can never corrupt shared fixtures).
fn clone_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if to.exists() {
            let _ = std::fs::remove_dir_all(to);
        }
        if try_clonefile(from, to) {
            return Ok(());
        }
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            clone_tree(&e.path(), &target)?;
        } else {
            clone_file(&e.path(), &target)?;
        }
    }
    Ok(())
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn in_temp(p: &Path) -> bool {
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    p.ancestors().find(|a| a.exists()).and_then(|a| std::fs::canonicalize(a).ok()).is_some_and(|a| a.starts_with(&temp))
}

// ---- pre-scanned library templates ----------------------------------------------------------

fn mtime_nanos(meta: &std::fs::Metadata) -> u128 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// Fingerprint of the app executable and a fixture set folder: any rebuild of `MarqueePlayer`,
/// any change to the fixture files, or any move of `--fixtures` invalidates the template so a
/// mode can never start from stale state.
fn template_stamp(exe: &Path, set_dir: &Path) -> std::io::Result<String> {
    let exe_canon = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let exe_meta = std::fs::metadata(&exe_canon)?;
    let set_canon = std::fs::canonicalize(set_dir).unwrap_or_else(|_| set_dir.to_path_buf());
    let mut stack = vec![set_canon.clone()];
    let (mut count, mut bytes, mut max_mtime) = (0u64, 0u64, 0u128);
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)? {
            let e = e?;
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                let m = e.metadata()?;
                count += 1;
                bytes += m.len();
                max_mtime = max_mtime.max(mtime_nanos(&m));
            }
        }
    }
    Ok(format!(
        "v1\nexe={}|{}|{}\nset={}|{}|{}|{}\n",
        exe_canon.display(),
        exe_meta.len(),
        mtime_nanos(&exe_meta),
        set_canon.display(),
        count,
        bytes,
        max_mtime
    ))
}

fn template_valid(dir: &Path, want_stamp: &str) -> bool {
    dir.join("library.db").exists()
        && !dir.join("library.db.damaged").exists()
        && std::fs::read_to_string(dir.join("stamp.txt")).is_ok_and(|s| s == want_stamp)
}

/// Copy only `library.db` (plus `-wal`/`-shm` if present) and `cache/art` from a template
/// into a fresh mode's `data` folder. Never copies `settings.json`, `session.json`, logs or
/// lock files.
fn apply_template(tmpl: &Path, data: &Path) -> std::io::Result<()> {
    for name in ["library.db", "library.db-wal", "library.db-shm"] {
        let src = tmpl.join(name);
        if src.exists() {
            clone_file(&src, &data.join(name))?;
        }
    }
    let art = tmpl.join("cache").join("art");
    if art.exists() {
        clone_tree(&art, &data.join("cache").join("art"))?;
    }
    Ok(())
}

/// Build one pre-scanned template for `set` by launching `idle-stopped` with
/// `MP_BENCH_IDLE_SECS=0` pointing straight at `<fixtures>/<set>`, then atomically saving
/// only `library.db` and `cache/art` plus `stamp.txt`.
fn build_template(exe: &Path, fixtures: &Path, tmpl_root: &Path, set: &str, stamp: &str, slot: Option<(usize, usize)>) -> Result<PathBuf, String> {
    let pid = std::process::id();
    let scratch = tmpl_root.join(format!("{set}.build-{pid}"));
    let ready = tmpl_root.join(format!("{set}.ready-{pid}"));
    let final_dir = tmpl_root.join(set);
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&ready);
    let data = scratch.join("data");
    let out = scratch.join("out");
    std::fs::create_dir_all(&data).map_err(|e| format!("template {set}: {e}"))?;
    std::fs::create_dir_all(&out).map_err(|e| format!("template {set}: {e}"))?;
    let lrc = lrclib();
    let lfm = lastfm();
    let run = Run {
        exe,
        dir: scratch.clone(),
        music: fixtures.join(set),
        data: data.clone(),
        out,
        lrclib: lrc.url.clone(),
        lastfm: lfm.url.clone(),
        slot,
    };
    let (code, secs) = run.launch("idle-stopped", &[("MP_BENCH_IDLE_SECS", "0")], 120, &[]);
    if code != Some(0) || !data.join("library.db").exists() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(format!("template scan of {set} failed (exit {code:?} after {secs:.1} s)"));
    }
    std::fs::create_dir_all(&ready).map_err(|e| format!("template {set}: {e}"))?;
    for name in ["library.db", "library.db-wal", "library.db-shm"] {
        let src = data.join(name);
        if src.exists() {
            std::fs::rename(&src, ready.join(name)).map_err(|e| format!("template {set} moving {name}: {e}"))?;
        }
    }
    let art = data.join("cache").join("art");
    if art.exists() {
        std::fs::create_dir_all(ready.join("cache")).map_err(|e| format!("template {set}: {e}"))?;
        std::fs::rename(&art, ready.join("cache").join("art")).map_err(|e| format!("template {set} moving cache/art: {e}"))?;
    }
    std::fs::write(ready.join("stamp.txt"), stamp).map_err(|e| format!("template {set} stamp: {e}"))?;
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&ready, &final_dir).map_err(|e| format!("template {set} install: {e}"))?;
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(final_dir)
}

/// Prepare templates for every single-set non-cold mode in `modes`.
/// Reuses any valid cached template immediately; builds missing ones only when at least 2
/// selected modes will share that set (so a single-mode `--only` run on a fresh build does
/// not pay an extra launch just to make a one-use template).
fn prepare_templates(exe: &Path, fixtures: &Path, root: &Path, modes: &[&Mode], jobs: usize) -> (HashMap<String, PathBuf>, Vec<String>, Vec<String>, f64) {
    let t0 = Instant::now();
    let tmpl_root = if in_temp(fixtures) { fixtures.join(".mp-templates") } else { root.join("_templates") };
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for m in modes {
        if !m.cold && m.sets.len() == 1 {
            *counts.entry(m.sets[0]).or_default() += 1;
        }
    }
    let mut ready: HashMap<String, PathBuf> = HashMap::new();
    let mut reused = Vec::new();
    let mut to_build: Vec<(&'static str, String)> = Vec::new();
    for (set, count) in counts {
        let set_dir = fixtures.join(set);
        let Ok(stamp) = template_stamp(exe, &set_dir) else { continue };
        let dir = tmpl_root.join(set);
        if template_valid(&dir, &stamp) {
            ready.insert(set.to_owned(), dir);
            reused.push(set.to_owned());
        } else if count >= 2 {
            to_build.push((set, stamp));
        }
    }
    let mut built = Vec::new();
    if !to_build.is_empty() {
        let _ = std::fs::create_dir_all(&tmpl_root);
        let workers = jobs.min(to_build.len()).max(1);
        let queue = Mutex::new(VecDeque::from(to_build));
        let done: Mutex<Vec<(String, Result<PathBuf, String>)>> = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for w in 0..workers {
                let slot = (workers > 1).then_some((w, workers));
                let queue = &queue;
                let done = &done;
                let tmpl_root = &tmpl_root;
                s.spawn(move || loop {
                    let next = queue.lock().unwrap().pop_front();
                    let Some((set, stamp)) = next else { break };
                    let res = build_template(exe, fixtures, tmpl_root, set, &stamp, slot);
                    done.lock().unwrap().push((set.to_owned(), res));
                });
            }
        });
        for (set, res) in done.into_inner().unwrap() {
            match res {
                Ok(dir) => {
                    ready.insert(set.clone(), dir);
                    built.push(set);
                }
                Err(e) => eprintln!("warning: {e}; falling back to cold scan for {set}"),
            }
        }
        built.sort();
    }
    (ready, reused, built, t0.elapsed().as_secs_f64())
}

// ---- running --------------------------------------------------------------------------------

#[derive(Clone)]
struct Outcome {
    name: String,
    verdict: String,
    fails: usize,
    secs: f64,
    prep: f64,
    scan: Option<f64>,
    note: String,
}

#[derive(Clone)]
struct Run<'a> {
    exe: &'a Path,
    dir: PathBuf,
    music: PathBuf,
    data: PathBuf,
    out: PathBuf,
    lrclib: String,
    lastfm: String,
    slot: Option<(usize, usize)>,
}

impl Run<'_> {
    /// One launch of the app; its exit code, or `None` when it had to be stopped.
    fn launch(&self, bench: &str, env: &[(&str, &str)], limit: u64, extra: &[(&str, &Path)]) -> (Option<i32>, f64) {
        let t0 = Instant::now();
        let Ok(child) = self.command(bench, env, extra, "").spawn() else { return (Some(-1), 0.0) };
        wait(child, limit, t0)
    }

    /// `launch`, its stderr kept apart (`stderr-<bench><tag>.txt`).
    fn launch_tagged(&self, bench: &str, env: &[(&str, &str)], limit: u64, extra: &[(&str, &Path)], tag: &str) -> (Option<i32>, f64) {
        let t0 = Instant::now();
        let Ok(child) = self.command(bench, env, extra, tag).spawn() else { return (Some(-1), 0.0) };
        wait(child, limit, t0)
    }

    /// The app's command line, its stderr to `stderr-<bench><tag>.txt`.
    fn command(&self, bench: &str, env: &[(&str, &str)], extra: &[(&str, &Path)], tag: &str) -> Command {
        let mut cmd = Command::new(self.exe);
        cmd.env("MP_BENCH", bench)
            .env("MP_BENCH_OUT", &self.out)
            .env("MP_DATA_DIR", &self.data)
            .env("MP_MUSIC", &self.music)
            .env("MP_VOLUME", "0")
            .env("MP_WINDOW", "secondary")
            .env("MP_LRCLIB_API", &self.lrclib)
            .env("MP_LASTFM_API", &self.lastfm)
            .env("RUST_BACKTRACE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.dir.join(format!("stderr-{bench}{tag}.txt"))).map(Stdio::from).unwrap_or(Stdio::null()));
        if let Some((k, n)) = self.slot {
            cmd.env("MP_WINDOW_SLOT", format!("{k}/{n}"));
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }
}

/// Wait for a launched app; its exit code, or `None` when it outlived `limit` and was stopped
/// (this child only, by its handle).
fn wait(mut child: std::process::Child, limit: u64, t0: Instant) -> (Option<i32>, f64) {
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            return (Some(s.code().unwrap_or(-2)), t0.elapsed().as_secs_f64());
        }
        if t0.elapsed() > Duration::from_secs(limit) {
            let _ = child.kill();
            let _ = child.wait();
            return (None, t0.elapsed().as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Fill `bytes` from `at` on, every 7th byte for `len` bytes, flipped.
fn flip(bytes: &mut [u8], at: usize, len: usize) {
    for b in bytes.iter_mut().skip(at).take(len).step_by(7) {
        *b ^= 0xFF;
    }
}

/// Parse the first start-up scan duration from `marquee-player.log` (e.g.
/// `... MarqueePlayer::scanning: scan: 2553 files, ..., 1.9 s`).
fn parse_scan_secs(log: &str) -> Option<f64> {
    let line = log.lines().find(|l| l.contains("MarqueePlayer::scanning: scan:"))?;
    let tail = line.rsplit(", ").next()?.trim();
    tail.strip_suffix(" s")?.trim().parse::<f64>().ok()
}

/// A scenario of the app's own files misbehaving: set up, launched, checked here; the checks
/// go to `bench-<scenario>.txt` as `[ok]` / `[FAIL]` lines like the app's own.
fn scenario(s: Scenario, run: &Run, m: &Mode) -> (Option<i32>, f64) {
    let data = &run.data;
    let log = || std::fs::read_to_string(data.join("marquee-player.log")).unwrap_or_default();
    let stderr = |tag: &str| std::fs::read_to_string(run.dir.join(format!("stderr-idle-stopped{tag}.txt"))).unwrap_or_default();
    let idle = [("MP_BENCH_IDLE_SECS", "2")];
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut check = |what: &str, ok: bool, detail: String| checks.push((what.to_owned(), ok, detail));
    let mut secs = 0.0;
    match s {
        Scenario::DataFiles => {
            let folder = run.music.display().to_string().replace('\\', "\\\\");
            let _ = std::fs::write(data.join("settings.json"), format!(r#"{{ "folders": ["{folder}"], "volume": "loud", "exclusive": true, "crossfade": 3 }}"#));
            let _ = std::fs::write(data.join("lastfm.json"), "{ not json");
            let _ = std::fs::write(data.join("scrobbles.json"), r#"[{"artist": "A"#);
            let _ = std::fs::write(data.join("theme.toml"), "[colors\naccent = ");
            let _ = std::fs::create_dir_all(data.join("edits").join("inflight"));
            let _ = std::fs::write(data.join("edits").join("inflight").join("junk.json"), "garbage");
            let (c, t) = run.launch("idle-stopped", &idle, m.limit, &[]);
            secs += t;
            let saved = std::fs::read_to_string(data.join("settings.json")).unwrap_or_default();
            let l = log();
            check("the app starts and runs as usual", c == Some(0), format!("exit {c:?}"));
            check("settings.json: the readable settings are kept (the folder, exclusive mode)", saved.contains(&folder) && saved.contains(r#""exclusive": true"#), saved.lines().take(4).collect::<Vec<_>>().join(" "));
            check("settings.json: the old file is kept as settings.json.bad", data.join("settings.json.bad").exists(), String::new());
            check("settings.json: what was reset is logged (and shown in the pill)", l.contains("volume") && l.contains("settings.json"), String::new());
            check("lastfm.json and scrobbles.json: kept as .bad before anything replaces them", data.join("lastfm.json.bad").exists() && data.join("scrobbles.json.bad").exists(), String::new());
            check("theme.toml: the built-in theme is used, the file left as it is", std::fs::read_to_string(data.join("theme.toml")).unwrap_or_default() == "[colors\naccent = " && l.contains("theme.toml"), String::new());
            check("a junk record in edits/inflight is dropped", !data.join("edits").join("inflight").join("junk.json").exists(), String::new());
        }
        Scenario::DbGarbage => {
            let junk = vec![0x5au8; 8192];
            let _ = std::fs::write(data.join("library.db"), &junk);
            let (c, t) = run.launch("idle-stopped", &idle, m.limit, &[]);
            secs += t;
            let e = stderr("");
            check("refused with a message, not a crash", matches!(c, Some(1) | Some(3)) && e.contains("damaged and cannot be read"), format!("exit {c:?}"));
            check("the message says what to do", e.contains("Start with a new library?") && e.contains("music files are not touched"), String::new());
            check("the file is left as it was", std::fs::read(data.join("library.db")).unwrap_or_default() == junk, String::new());
            let (c, t) = run.launch("idle-stopped", &[("MP_BENCH_IDLE_SECS", "2"), ("MP_BENCH_ANSWER", "yes")], m.limit, &[]);
            secs += t;
            let aside = std::fs::read_dir(data).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("library.damaged-")).count()).unwrap_or(0);
            check("\"start with a new library\": it starts, scans, and keeps the damaged file aside", c == Some(0) && aside == 1, format!("exit {c:?}, {aside} kept aside"));
        }
        Scenario::DbNewer => {
            let (c, t) = run.launch("idle-stopped", &idle, m.limit, &[]);
            secs += t;
            check("a first start makes the library", c == Some(0), format!("exit {c:?}"));
            let mut db = std::fs::read(data.join("library.db")).unwrap_or_default();
            if db.len() > 64 {
                db[60..64].copy_from_slice(&99u32.to_be_bytes());
                let _ = std::fs::write(data.join("library.db"), &db);
            }
            let (c, t) = run.launch_tagged("idle-stopped", &idle, m.limit, &[], "-2");
            secs += t;
            let e = stderr("-2");
            check("a newer build's library is refused with a message", matches!(c, Some(1) | Some(3)) && e.contains("newer version of Marquee Player"), format!("exit {c:?}"));
            check("and left untouched", std::fs::read(data.join("library.db")).unwrap_or_default() == db, String::new());
        }
        Scenario::DbFlipped => {
            let (c, t) = run.launch("idle-stopped", &idle, m.limit, &[]);
            secs += t;
            check("a first start makes the library", c == Some(0), format!("exit {c:?}"));
            let mut db = std::fs::read(data.join("library.db")).unwrap_or_default();
            let n = db.len();
            // The middle of the file: table and index pages. A third of the way in was one of the
            // schema's own pages from schema 12 on (58 pages, the schema on 1, 24, 25 and 45), and
            // a database whose schema cannot be read does not open at all, which is `db-garbage`'s
            // case, not this one.
            flip(&mut db, n / 2, 16 * 1024);
            let _ = std::fs::write(data.join("library.db"), &db);
            let (c, t) = run.launch_tagged("idle-stopped", &idle, m.limit, &[], "-2");
            secs += t;
            let e = stderr("-2");
            check("a library damaged inside: no crash, no hang", matches!(c, Some(0) | Some(3)), format!("exit {c:?}"));
            check("the user is told the library is damaged, and the app closes", e.contains("found its library database damaged"), e.lines().filter(|l| l.contains("malformed") || l.contains("damaged")).take(2).collect::<Vec<_>>().join(" | "));
            let (c, t) = run.launch_tagged("idle-stopped", &idle, m.limit, &[], "-3");
            secs += t;
            check("the next start offers a new library", matches!(c, Some(1) | Some(3)) && stderr("-3").contains("Start with a new library?"), format!("exit {c:?}"));
            let (c, t) = run.launch_tagged("idle-stopped", &[("MP_BENCH_IDLE_SECS", "2"), ("MP_BENCH_ANSWER", "yes")], m.limit, &[], "-4");
            secs += t;
            check("and starts with one when asked, the damaged file kept", c == Some(0) && !library_marker(data), format!("exit {c:?}"));
        }
        Scenario::DataReadOnly => {
            let (c, t) = run.launch("idle-stopped", &idle, m.limit, &[]);
            secs += t;
            check("a first start makes the data folder", c == Some(0), format!("exit {c:?}"));
            let user = std::env::var("USERNAME").unwrap_or_default();
            let deny = Command::new("icacls").arg(data).args(["/deny", &format!("{user}:(OI)(CI)(W,D,DC)"), "/Q"]).stdout(Stdio::null()).status().is_ok_and(|s| s.success());
            check("the data folder made unwritable (icacls deny write)", deny, String::new());
            let (c, t) = run.launch_tagged("idle-stopped", &idle, m.limit, &[], "-2");
            secs += t;
            let e = stderr("-2");
            let _ = Command::new("icacls").arg(data).args(["/remove:d", &user, "/T", "/Q"]).stdout(Stdio::null()).status();
            check("refused at start with a message naming the folder and what to do", matches!(c, Some(1) | Some(3)) && e.contains("cannot write to the folder"), format!("exit {c:?}: {}", e.lines().filter(|l| l.contains("ERROR")).take(1).collect::<String>()));
        }
        Scenario::Crash => {
            let (c, t) = run.launch_tagged("crash-ui", &[], m.limit, &[], "");
            secs += t;
            let e = std::fs::read_to_string(run.dir.join("stderr-crash-ui.txt")).unwrap_or_default();
            let l = log();
            check("a panic on the UI thread ends the process (no hang)", c == Some(101), format!("exit {c:?}"));
            check("the log has it, with its thread and a backtrace", l.contains("FATAL: code panicked in thread 'main'") && l.contains("backtrace:"), String::new());
            check("the user is told what happened, where the log is and how to report it", e.contains("Something went wrong inside Marquee Player") && e.contains("marquee-player.log") && e.contains("Copy diagnostics"), String::new());
            let (c, t) = run.launch_tagged("idle-stopped", &idle, m.limit, &[], "-after");
            secs += t;
            let prev = std::fs::read_to_string(data.join("marquee-player.previous.log")).unwrap_or_default();
            check("the next start runs, and keeps the crashed run's log as the previous one", c == Some(0) && prev.contains("FATAL: code panicked"), format!("exit {c:?}"));
            let (c, t) = run.launch_tagged("crash-worker", &[], m.limit, &[], "-worker");
            secs += t;
            let e = std::fs::read_to_string(run.dir.join("stderr-crash-worker-worker.txt")).unwrap_or_default();
            check("a panic on another thread: told, and the app closes the normal way", matches!(c, Some(0) | Some(3)) && e.contains("mp-bench-crash") && e.contains("has to close"), format!("exit {c:?}"));
        }
        Scenario::TwoInstances => {
            let t0 = Instant::now();
            let a = run.command("idle-stopped", &[("MP_BENCH_IDLE_SECS", "8")], &[], "-a").spawn();
            std::thread::sleep(Duration::from_millis(1500));
            let b = run.command("idle-stopped", &[("MP_BENCH_IDLE_SECS", "8")], &[], "-b").spawn();
            let (cb, tb) = b.map_or((Some(-1), 0.0), |b| wait(b, 30, Instant::now()));
            let (ca, _) = a.map_or((Some(-1), 0.0), |a| wait(a, m.limit, t0));
            secs = t0.elapsed().as_secs_f64();
            check("the second launch hands over and leaves at once", cb == Some(0) && tb < 10.0, format!("exit {cb:?} after {tb:.1} s"));
            check("the first runs to its end as usual", ca == Some(0), format!("exit {ca:?}"));
            let l = log();
            check("the running copy's log is its own (the second did not restart it)", l.contains("on Windows") || l.contains("on Linux") || l.lines().count() > 5, format!("{} lines", l.lines().count()));
        }
    }
    let ok = checks.iter().all(|c| c.1);
    let text: String = checks.iter().map(|(w, ok, d)| format!("  [{}] {w}: {d}\n", if *ok { "ok" } else { "FAIL" })).collect();
    let _ = std::fs::write(run.out.join(format!("bench-{}.txt", m.name)), text);
    (Some(if ok { 0 } else { 3 }), secs)
}

/// The snapshots of a run quit mid-scan, kept apart from the next run's: `quit-<name>.png`.
fn keep_as_quit(out: &Path) {
    for e in std::fs::read_dir(out).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("bigscan-") && name.ends_with(".png") {
            let _ = std::fs::rename(e.path(), out.join(format!("quit-{name}")));
        }
    }
}

/// `[FAIL]` and `FAIL` lines in the mode's reports.
fn failed_lines(out: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(rd) = std::fs::read_dir(out) else { return lines };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "txt") {
            for l in std::fs::read_to_string(&p).unwrap_or_default().lines() {
                if l.contains("[FAIL]") || l.trim_start().starts_with("FAIL") || l.contains("-> FAIL") || l.contains("FAIL:") {
                    lines.push(l.trim().to_owned());
                }
            }
        }
    }
    lines
}

struct ModeCtx<'a> {
    exe: &'a Path,
    fixtures: &'a Path,
    root: &'a Path,
    templates: &'a HashMap<String, PathBuf>,
    cold: bool,
    keep: bool,
}

fn run_mode(ctx: &ModeCtx, m: &Mode, slot: Option<(usize, usize)>) -> Outcome {
    let prep_t0 = Instant::now();
    let dir = ctx.root.join(m.name);
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("music");
    let data = dir.join("data");
    let out = dir.join("out");
    for d in [&data, &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    // Single-set read-only modes point straight at `<fixtures>/<set>` (zero copy, and paths
    // match the pre-scanned template verbatim). Mutating modes, multi-set modes, and `--cold`
    // clone their sets into `<out>/<mode>/music`.
    let direct = !ctx.cold && !m.mutates && m.sets.len() == 1;
    let library = if direct {
        ctx.fixtures.join(m.sets[0])
    } else {
        std::fs::create_dir_all(&music).unwrap();
        for set in m.sets {
            if let Err(e) = clone_tree(&ctx.fixtures.join(set), &music.join(set)) {
                return Outcome {
                    name: m.name.into(),
                    verdict: "ERROR".into(),
                    fails: 0,
                    secs: 0.0,
                    prep: prep_t0.elapsed().as_secs_f64(),
                    scan: None,
                    note: format!("cloning {set}: {e}"),
                };
            }
        }
        // One set: the library is that set's folder (the metadata and editing benches check
        // that it is their fixtures, marked); several: the folder holding them.
        if m.sets.len() == 1 { music.join(m.sets[0]) } else { music.clone() }
    };
    if !ctx.cold && !m.cold && m.sets.len() == 1
        && let Some(tmpl) = ctx.templates.get(m.sets[0])
        && let Err(e) = apply_template(tmpl, &data)
    {
        return Outcome {
            name: m.name.into(),
            verdict: "ERROR".into(),
            fails: 0,
            secs: 0.0,
            prep: prep_t0.elapsed().as_secs_f64(),
            scan: None,
            note: format!("seeding template {}: {e}", m.sets[0]),
        };
    }
    // A connected Last.fm session (against the mock).
    let _ = std::fs::write(data.join("lastfm.json"), r#"{"api_key":"benchkey0123456789","api_secret":"benchsecret","session_key":"bench-sk","username":"bench"}"#);
    let lrc = lrclib();
    let lfm = lastfm();
    let prep = prep_t0.elapsed().as_secs_f64();
    let run = Run { exe: ctx.exe, dir: dir.clone(), music: library, data, out: out.clone(), lrclib: lrc.url.clone(), lastfm: lfm.url.clone(), slot };
    let mut note = String::new();
    let (code, secs) = match m.kind {
        Kind::Plain => run.launch(m.bench, m.env, m.limit, &[]),
        Kind::Scenario(s) => scenario(s, &run, m),
        Kind::Session => {
            let (c1, s1) = run.launch("session-save", m.env, m.limit, &[]);
            if c1 != Some(0) {
                note = "session-save did not pass".into();
                (c1, s1)
            } else {
                // Move the folder of the track with the history between the runs: its plays,
                // favourite and playlist entry must follow it.
                let report = std::fs::read_to_string(out.join("bench-session-save.txt")).unwrap_or_default();
                let track = report.lines().find_map(|l| l.strip_prefix("history on ").and_then(|l| l.split_once(") at ")).map(|(_, p)| PathBuf::from(p.trim())));
                if let Some(album) = track.as_deref().and_then(Path::parent) {
                    let moved = album.with_file_name(format!("{} (moved)", album.file_name().unwrap_or_default().to_string_lossy()));
                    if let Err(e) = std::fs::rename(album, &moved) {
                        note = format!("could not move {}: {e}", album.display());
                    }
                } else {
                    note = "session-save named no track".into();
                }
                // A second folder, outside the first, to add and remove while a scan runs.
                let extra = dir.join("extra");
                let _ = std::fs::rename(music.join("formats"), extra.join("formats")).or_else(|_| {
                    std::fs::create_dir_all(&extra)?;
                    std::fs::rename(music.join("formats"), extra.join("formats"))
                });
                let (c2, s2) = run.launch("session-check", &[("MP_BENCH_MOVED", " (moved)")], m.limit, &[("MP_BENCH_EXTRA", &extra.join("formats"))]);
                (c2, s1 + s2)
            }
        }
        Kind::Zoom => {
            let (c1, s1) = run.launch("zoom", m.env, m.limit, &[]);
            if c1 != Some(0) {
                note = "zoom did not pass".into();
                (c1, s1)
            } else {
                let (c2, s2) = run.launch("zoom-check", m.env, m.limit, &[]);
                (c2, s1 + s2)
            }
        }
        Kind::BigScan => {
            // Scanned where it is (read only), never copied: tens of thousands of files.
            let big = ctx.fixtures.join("big");
            let run = Run { music: big, ..run.clone() };
            let (c1, s1) = run.launch("bigscan", &[("MP_BENCH_QUIT_AFTER", "3")], m.limit, &[]);
            let _ = std::fs::rename(out.join("bench-bigscan.txt"), out.join("bench-bigscan-quit.txt"));
            keep_as_quit(&out);
            if c1 != Some(0) {
                note = format!("quitting mid-scan: exit {c1:?} after {s1:.1} s");
                (c1, s1)
            } else {
                // Quit 3 s in (mid-scan, even with the files in the system's cache): the process should be
                // gone promptly, not after the scan.
                if s1 > 20.0 {
                    note = format!("quitting mid-scan took the process {s1:.1} s to end");
                }
                let (c2, s2) = run.launch("bigscan", &[], m.limit, &[]);
                (if s1 > 20.0 && c2 == Some(0) { Some(3) } else { c2 }, s1 + s2)
            }
        }
        Kind::SlowScan => {
            let quit: Vec<(&str, &str)> = m.env.iter().copied().chain([("MP_BENCH_QUIT_AT", "0.4")]).collect();
            let (c1, s1) = run.launch("bigscan", &quit, m.limit, &[]);
            let _ = std::fs::rename(out.join("bench-bigscan.txt"), out.join("bench-bigscan-quit.txt"));
            keep_as_quit(&out);
            if c1 != Some(0) {
                note = format!("quitting 40% through: exit {c1:?} after {s1:.1} s");
                (c1, s1)
            } else {
                let (c2, s2) = run.launch("bigscan", m.env, m.limit, &[]);
                (c2, s1 + s2)
            }
        }
        Kind::Guard => {
            // The first run scans everything and plays nothing.
            let (c1, s1) = run.launch("idle-stopped", &[("MP_BENCH_IDLE_SECS", "1")], m.limit, &[]);
            if c1 != Some(0) {
                note = "the first scan did not finish".into();
                (c1, s1)
            } else {
                let mut all = Vec::new();
                files(&music, &mut all);
                all.sort();
                for (i, f) in all.iter().enumerate() {
                    if i % 20 != 0 {
                        let _ = std::fs::remove_file(f);
                    }
                }
                let (c2, s2) = run.launch(m.bench, m.env, m.limit, &[]);
                (c2, s1 + s2)
            }
        }
    };
    let _ = std::fs::write(dir.join("lrclib-mock.log"), lrc.log.lock().unwrap().join("\n"));
    let _ = std::fs::write(dir.join("lastfm-mock.log"), lfm.log.lock().unwrap().join("\n"));
    let fails = failed_lines(&out);
    let log = std::fs::read_to_string(run.data.join("marquee-player.log")).unwrap_or_default();
    let scan = parse_scan_secs(&log);
    // (The crash scenario panics on purpose: its own checks say whether that went as it should.)
    let panicked = m.kind != Kind::Scenario(Scenario::Crash) && log.lines().any(|l| l.contains("panicked"));
    let verdict = match code {
        Some(0) if fails.is_empty() && !panicked => "PASS",
        Some(0) if panicked => "PANIC",
        Some(0) | Some(3) => "FAIL",
        Some(101) => "PANIC",
        None => "HANG",
        Some(_) => "CRASH",
    };
    if note.is_empty() {
        note = match code {
            Some(c) if c != 0 && c != 3 => format!("exit code {c}"),
            None => format!("stopped after {} s", m.limit),
            _ => fails.first().cloned().unwrap_or_default(),
        };
    }
    let clean_t0 = Instant::now();
    if !ctx.keep && !direct {
        let _ = std::fs::remove_dir_all(&music);
    }
    let prep = prep + clean_t0.elapsed().as_secs_f64();
    let _ = std::fs::write(dir.join("failed.txt"), fails.join("\n"));
    Outcome { name: m.name.into(), verdict: verdict.into(), fails: fails.len(), secs, prep, scan, note }
}

// ---- scheduling & selection -----------------------------------------------------------------

/// Sensible default concurrency from the machine's core count: enough to keep the cores
/// busy without starving the window server, GPU or audio mixer.
fn default_jobs_for(cpus: usize) -> usize {
    (cpus * 3 / 5).clamp(1, 6)
}

fn default_jobs() -> usize {
    default_jobs_for(std::thread::available_parallelism().map_or(1, usize::from))
}

/// Select modes in `MODES` order given `--only`, `--skip`, `--quick`, and `--for`/`--changed`.
/// Explicit `--only` overrides `--quick` so `--only soak` works even when `xtask bench` passes
/// `--quick` by default.
fn select_modes(only: &[String], skip: &[String], quick: bool, for_modes: Option<&BTreeSet<String>>) -> Vec<&'static Mode> {
    MODES
        .iter()
        .filter(|m| only.is_empty() || only.iter().any(|o| o == m.name))
        .filter(|m| for_modes.is_none_or(|set| set.contains(m.name)))
        .filter(|m| !skip.iter().any(|s| s == m.name))
        .filter(|m| !quick || !only.is_empty() || !LONG.contains(&m.name))
        .collect()
}

/// Approximate runtime (seconds) for ordering the parallel work queue longest-first (LPT)
/// so the slowest parallel modes (`cleanup`, `viz-styles`) start immediately instead of
/// trailing at the end while other workers sit idle. Falls back to `m.limit / 3` for any
/// mode not listed.
fn schedule_weight(m: &Mode) -> u64 {
    match m.name {
        "cleanup" => 215,
        "viz-styles" => 165,
        "details" => 80,
        "settings" => 77,
        "home-shelves" => 72,
        "smart" => 70,
        "metadata" => 67,
        "details-off" => 64,
        "phase5" => 62,
        "phase4" => 55,
        "editing" => 50,
        "listenbrainz" => 49,
        "library" => 43,
        "server" => 41,
        "about" => 36,
        "zoom" => 32,
        "home" => 30,
        "eq-paste" => 29,
        "tour" => 27,
        "eq" => 27,
        "lyrics" => 25,
        "phase5-open" => 18,
        "session" => 17,
        _ => m.limit / 3,
    }
}

type Scheduled = Vec<(usize, &'static Mode)>;

/// Split runnable modes into `(parallel, alone)`, keeping each mode's index in `modes` so
/// the final table stays in `MODES` order. Parallel modes are ordered longest first so a
/// long mode like `cleanup` or `viz-styles` starts immediately instead of trailing at the end.
fn schedule(modes: &[&'static Mode], windows: bool, jobs: usize) -> (Scheduled, Scheduled) {
    let mut parallel = Vec::new();
    let mut alone = Vec::new();
    for (idx, &m) in modes.iter().enumerate() {
        if !windows && !m.linux {
            continue;
        }
        if jobs <= 1 || m.alone {
            alone.push((idx, m));
        } else {
            parallel.push((idx, m));
        }
    }
    parallel.sort_by(|a, b| schedule_weight(b.1).cmp(&schedule_weight(a.1)).then_with(|| a.0.cmp(&b.0)));
    (parallel, alone)
}

fn help() -> &'static str {
    "\
bench-runner [options]

Runs Marquee Player's scripted bench modes (`MP_BENCH`) against local mocks and prints one
pass/fail table.

Options:
  --only a,b         run only the named modes (overrides --quick)
  --skip a,b         skip the named modes
  --quick            skip the long modes (`soak`, `bigscan`, `slowscan`)
  --all              include the long modes (cancels --quick)
  --jobs N, -j N     parallel workers for parallel-safe modes (default from core count, max 6;
                     1 runs everything serially); `alone` modes always run one at a time after
  --cold             skip pre-scanned library templates and copy/clone each mode's sets
                     (--jobs 1 --cold reproduces the original serial cold-scan run)
  --for PATH         pick modes for this source path (repeatable or comma-separated; any
                     unmapped path runs everything)
  --changed [REF]    like --for on git-changed files (against REF, or HEAD + untracked)
  --keep             keep each mode's music copy (for modes that make one)
  --list             list every mode with its flags and exit
  --exe PATH         app binary built with --features bench
  --fixtures DIR     fixture root (default <temp>/mp-bench-fixtures)
  --out DIR          output root inside <temp> (default <temp>/mp-bench-runs)
  -h, --help         show this help"
}

fn format_scan(scan: Option<f64>) -> String {
    scan.map_or_else(|| "-".into(), |s| format!("{s:.1}"))
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let me = std::env::current_exe().unwrap();
    let mut exe = me.with_file_name(format!("MarqueePlayer{}", std::env::consts::EXE_SUFFIX));
    let mut fixtures = std::env::temp_dir().join("mp-bench-fixtures");
    let mut root = std::env::temp_dir().join("mp-bench-runs");
    let (mut only, mut skip, mut keep) = (Vec::<String>::new(), Vec::<String>::new(), false);
    let (mut quick, mut cold, mut jobs) = (false, false, default_jobs());
    let mut for_paths: Vec<String> = Vec::new();
    let mut changed: Option<Option<String>> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return;
            }
            "--quick" => quick = true,
            "--all" => quick = false,
            "--cold" => cold = true,
            "--jobs" | "-j" => {
                let raw = args.next().expect("--jobs <N>");
                jobs = raw.parse::<usize>().ok().filter(|&n| n >= 1).unwrap_or_else(|| {
                    eprintln!("--jobs expects a positive integer, not `{raw}`");
                    std::process::exit(2);
                });
            }
            "--exe" => exe = PathBuf::from(args.next().expect("--exe <path>")),
            "--fixtures" => fixtures = PathBuf::from(args.next().expect("--fixtures <dir>")),
            "--out" => root = PathBuf::from(args.next().expect("--out <dir>")),
            "--only" => only.extend(args.next().expect("--only a,b").split(',').filter(|s| !s.is_empty()).map(str::to_owned)),
            "--skip" => skip.extend(args.next().expect("--skip a,b").split(',').filter(|s| !s.is_empty()).map(str::to_owned)),
            "--for" => {
                let raw = args.next().expect("--for <path>");
                for_paths.extend(raw.split(',').filter(|s| !s.is_empty()).map(str::to_owned));
            }
            "--changed" => {
                let git_ref = args.peek().filter(|next| !next.starts_with('-')).cloned();
                if git_ref.is_some() {
                    args.next();
                }
                changed = Some(git_ref);
            }
            "--keep" => keep = true,
            "--list" => {
                for m in MODES {
                    let mut tags = Vec::new();
                    if !m.linux {
                        tags.push("Windows only");
                    }
                    if LONG.contains(&m.name) {
                        tags.push("long");
                    }
                    if m.alone {
                        tags.push("alone");
                    }
                    if m.cold {
                        tags.push("cold");
                    }
                    if m.mutates {
                        tags.push("mutates");
                    }
                    let suffix = if tags.is_empty() { String::new() } else { format!(" ({})", tags.join(", ")) };
                    println!("{}{suffix}", m.name);
                }
                return;
            }
            other => {
                eprintln!("unknown argument {other} (--help lists options)");
                std::process::exit(2);
            }
        }
    }
    // A name that is no mode's (a list PowerShell joined with spaces when it was not quoted, a
    // misspelling) would otherwise run nothing and pass.
    if let Some(unknown) = only.iter().chain(&skip).find(|n| !MODES.iter().any(|m| m.name == n.as_str())) {
        eprintln!("no bench mode is called \"{unknown}\" (--list prints them; in PowerShell, quote a list: '--only' 'a,b')");
        std::process::exit(2);
    }
    if let Some(git_ref) = changed {
        match git_changed_paths(git_ref.as_deref()) {
            Ok(paths) if paths.is_empty() && for_paths.is_empty() => {
                println!("no changed files found (pass a ref, e.g. --changed master); nothing to run");
                return;
            }
            Ok(paths) => for_paths.extend(paths),
            Err(e) => {
                eprintln!("--changed: {e}");
                std::process::exit(2);
            }
        }
    }
    let for_modes = if for_paths.is_empty() {
        None
    } else {
        let root_dir = git_root();
        match modes_for_paths(&for_paths, root_dir.as_deref()) {
            PathSelection::Modes(set) => {
                let list: Vec<&str> = set.iter().map(String::as_str).collect();
                println!("--for/--changed ({} path{}): {}", for_paths.len(), if for_paths.len() == 1 { "" } else { "s" }, list.join(", "));
                Some(set)
            }
            PathSelection::AllBecause(path) => {
                println!("--for/--changed: `{path}` is not mapped to a subset; running all selected modes");
                None
            }
        }
    };
    assert!(exe.exists(), "{} not found: build it with cargo build --profile fast -p marquee-player --features bench", exe.display());
    assert!(in_temp(&root), "{} is not in the temporary folder: refused", root.display());
    if !fixtures.join(".mp-bench-fixtures").exists() {
        let tool = me.with_file_name("examples").join(format!("bench_fixtures{}", std::env::consts::EXE_SUFFIX));
        println!("making the fixtures in {} ...", fixtures.display());
        let ok = Command::new(&tool).arg(&fixtures).status().is_ok_and(|s| s.success());
        assert!(ok, "{} failed (cargo build --profile fast -p library --examples)", tool.display());
    }
    // The visualizer's set as made before it had its bright and monochrome covers ("Paper",
    // "Graphite"): made again, in place of the old one (inside the fixtures folder only).
    let viz = fixtures.join("viz");
    if viz.exists() && !viz.join("Fixture Covers").join("Graphite").exists() {
        let tool = me.with_file_name("examples").join(format!("viz_fixtures{}", std::env::consts::EXE_SUFFIX));
        let fresh = fixtures.join("viz.new");
        let _ = std::fs::remove_dir_all(&fresh);
        println!("making the visualizer's fixtures again in {} (its newer covers) ...", viz.display());
        let ok = Command::new(&tool).arg(&fresh).stdout(Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(ok, "{} failed (cargo build --profile fast -p library --examples)", tool.display());
        std::fs::remove_dir_all(&viz).expect("the old visualizer fixtures");
        std::fs::rename(&fresh, &viz).expect("the new visualizer fixtures in place");
    }
    let modes = select_modes(&only, &skip, quick, for_modes.as_ref());
    // The large library (about 30,000 tracks), made the first time a mode needs it, kept.
    let wants_big = modes.iter().any(|m| m.kind == Kind::BigScan);
    if wants_big && !fixtures.join("big").exists() {
        let tool = me.with_file_name("examples").join(format!("messy_library{}", std::env::consts::EXE_SUFFIX));
        println!("making the large library in {} (a few minutes, about 600 MB) ...", fixtures.join("big").display());
        let ok = Command::new(&tool).arg(fixtures.join("big")).arg("2600").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(ok, "{} failed", tool.display());
    }
    if root.exists() {
        let _ = std::fs::remove_dir_all(&root);
    }
    std::fs::create_dir_all(&root).unwrap();
    let t0 = Instant::now();
    let (parallel, alone) = schedule(&modes, cfg!(windows), jobs);
    let runnable: Vec<&Mode> = parallel.iter().chain(&alone).map(|(_, m)| *m).collect();
    let (templates, tmpl_secs) = if cold {
        (HashMap::new(), 0.0)
    } else {
        let (map, reused, built, secs) = prepare_templates(&exe, &fixtures, &root, &runnable, jobs);
        if !reused.is_empty() || !built.is_empty() {
            let mut parts = Vec::new();
            if !reused.is_empty() {
                parts.push(format!("reused {}", reused.join(", ")));
            }
            if !built.is_empty() {
                parts.push(format!("built {}", built.join(", ")));
            }
            println!("templates ({secs:.1} s): {}", parts.join("; "));
        }
        (map, secs)
    };
    let active_jobs = if parallel.is_empty() { 1 } else { jobs.min(parallel.len()) };
    println!(
        "running {} mode{} ({} parallel with --jobs {}, {} alone{})",
        runnable.len(),
        if runnable.len() == 1 { "" } else { "s" },
        parallel.len(),
        active_jobs,
        alone.len(),
        if cold { ", --cold" } else { "" }
    );
    let ctx = ModeCtx { exe: &exe, fixtures: &fixtures, root: &root, templates: &templates, cold, keep };
    let slots: Vec<Mutex<Option<Outcome>>> = (0..modes.len()).map(|_| Mutex::new(None)).collect();
    for (i, m) in modes.iter().enumerate() {
        if !cfg!(windows) && !m.linux {
            *slots[i].lock().unwrap() = Some(Outcome {
                name: m.name.into(),
                verdict: "n/a".into(),
                fails: 0,
                secs: 0.0,
                prep: 0.0,
                scan: None,
                note: "Windows only".into(),
            });
        }
    }
    let out_lock = Mutex::new(());
    let print_done = |o: &Outcome| {
        let _g = out_lock.lock().unwrap();
        println!("{:<14} {:<6} {:>6.1} s  (prep {:>4.1} s, scan {:>4} s)  {}", o.name, o.verdict, o.secs, o.prep, format_scan(o.scan), o.note);
        let _ = std::io::stdout().flush();
    };
    if !parallel.is_empty() {
        let queue = Mutex::new(VecDeque::from(parallel));
        std::thread::scope(|s| {
            for w in 0..active_jobs {
                let win_slot = (active_jobs > 1).then_some((w, active_jobs));
                let queue = &queue;
                let ctx = &ctx;
                let slots = &slots;
                let print_done = &print_done;
                s.spawn(move || loop {
                    let next = queue.lock().unwrap().pop_front();
                    let Some((idx, m)) = next else { break };
                    let o = run_mode(ctx, m, win_slot);
                    print_done(&o);
                    *slots[idx].lock().unwrap() = Some(o);
                });
            }
        });
    }
    for (idx, m) in alone {
        let o = run_mode(&ctx, m, None);
        print_done(&o);
        *slots[idx].lock().unwrap() = Some(o);
    }
    let results: Vec<Outcome> = slots.into_iter().map(|s| s.into_inner().unwrap().expect("every selected mode produced an outcome")).collect();
    let wall = t0.elapsed().as_secs_f64();
    let mode_sum: f64 = results.iter().filter(|o| o.verdict != "n/a").map(|o| o.secs + o.prep).sum();
    let speedup = if wall > 0.0 { mode_sum / wall } else { 1.0 };
    let mut table = String::new();
    table.push_str(&format!("\n{:<14} {:<6} {:>6} {:>8} {:>6} {:>6}  {}\n", "mode", "result", "fails", "seconds", "prep", "scan", "first failure / note"));
    for o in &results {
        table.push_str(&format!(
            "{:<14} {:<6} {:>6} {:>8.1} {:>6.1} {:>6}  {}\n",
            o.name,
            o.verdict,
            o.fails,
            o.secs,
            o.prep,
            format_scan(o.scan),
            o.note.chars().take(110).collect::<String>()
        ));
    }
    let passed = results.iter().filter(|o| o.verdict == "PASS").count();
    let counted = results.iter().filter(|o| o.verdict != "n/a").count();
    let tmpl_note = if tmpl_secs > 0.05 { format!(", templates {tmpl_secs:.1} s") } else { String::new() };
    table.push_str(&format!(
        "\n{passed} of {counted} passed in {wall:.0} s wall (sum of modes {mode_sum:.0} s, {speedup:.1}x; --jobs {jobs}{tmpl_note}); reports in {}\n",
        root.display()
    ));
    print!("{table}");
    let _ = std::fs::write(root.join("table.txt"), &table);
    std::process::exit(if passed == counted { 0 } else { 1 });
}

/// Whether the app's "database found damaged" marker is still beside the library.
fn library_marker(data: &Path) -> bool {
    data.join("library.db.damaged").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_name_is_unique_and_long_names_exist() {
        let mut seen = BTreeSet::new();
        for m in MODES {
            assert!(seen.insert(m.name), "duplicate mode name {}", m.name);
        }
        for &l in LONG {
            assert!(seen.contains(l), "LONG names unknown mode {l}");
        }
    }

    #[test]
    fn every_path_rule_targets_only_real_modes() {
        let known: BTreeSet<&str> = MODES.iter().map(|m| m.name).collect();
        for &(pat, modes) in PATH_RULES {
            assert!(!modes.is_empty(), "empty mode list for {pat}");
            for &m in modes {
                assert!(known.contains(m), "PATH_RULES entry {pat} names unknown mode {m}");
            }
        }
    }

    #[test]
    fn mutating_multi_set_and_scenario_modes_always_stay_cold() {
        for m in MODES {
            if m.mutates {
                assert!(m.cold, "mutating mode {} must be cold (its music path is per-run)", m.name);
            }
            if m.sets.len() != 1 {
                assert!(m.cold, "mode {} with {} sets must be cold", m.name, m.sets.len());
            }
            if !matches!(m.kind, Kind::Plain | Kind::Zoom) {
                assert!(m.cold, "non-plain mode {} must be cold", m.name);
            }
            if LONG.contains(&m.name) {
                assert!(m.alone, "long mode {} must run alone", m.name);
            }
        }
    }

    #[test]
    fn timing_and_cpu_modes_are_marked_to_run_alone() {
        let must_be_alone = [
            "grid",
            "sidebar",
            "covers",
            "playlist",
            "gapless",
            "skip",
            "skip-damaged",
            "idle-paused",
            "idle-stopped",
            "autoscroll",
            "tray",
            "visualizer",
            "viz-idle",
            "viz-styles",
            "viz-cost",
            "viz-look",
            "output-default",
            "soak",
            "xfade-rates",
            "streaming",
            "bigscan",
            "slowscan",
            "two-instances",
        ];
        for name in must_be_alone {
            let m = MODES.iter().find(|m| m.name == name).unwrap_or_else(|| panic!("missing mode {name}"));
            assert!(m.alone, "{name} measures timing/CPU/system state and must run alone");
        }
    }

    #[test]
    fn schedule_separates_parallel_and_alone_and_orders_parallel_longest_first() {
        let all = select_modes(&[], &[], true, None);
        let (par, alone) = schedule(&all, false, 4);
        assert!(!par.is_empty() && !alone.is_empty());
        assert!(par.iter().all(|(_, m)| !m.alone && m.linux));
        assert!(alone.iter().all(|(_, m)| m.alone && m.linux));
        // Longest parallel modes (`cleanup`, `details`) start first in the parallel queue.
        assert_eq!(par[0].1.name, "cleanup");
        assert_eq!(par[1].1.name, "details");
        for w in par.windows(2) {
            assert!(schedule_weight(w[0].1) >= schedule_weight(w[1].1));
        }
        // With --jobs 1 everything runs in the serial list, in MODES order.
        let (par1, alone1) = schedule(&all, false, 1);
        assert!(par1.is_empty());
        assert_eq!(alone1.len(), par.len() + alone.len());
        for w in alone1.windows(2) {
            assert!(w[0].0 < w[1].0);
        }
    }

    #[test]
    fn only_overrides_quick_so_explicit_long_modes_still_run() {
        let picked = select_modes(&["soak".into()], &[], true, None);
        assert_eq!(picked.iter().map(|m| m.name).collect::<Vec<_>>(), vec!["soak"]);
        let quick = select_modes(&[], &[], true, None);
        assert!(quick.iter().all(|m| !LONG.contains(&m.name)));
        assert_eq!(quick.iter().filter(|m| m.linux).count(), 52);
    }

    #[test]
    fn path_mapping_narrows_focused_changes_and_falls_back_for_unmapped_paths() {
        let viz = modes_for_paths(&["crates/app/src/visualizer/beat.rs".into()], None);
        assert_eq!(
            viz,
            PathSelection::Modes(["visualizer", "viz-idle", "viz-styles", "viz-cost", "viz-look"].into_iter().map(str::to_owned).collect())
        );
        // Longest prefix wins: `server/streaming.rs` narrows more tightly than `server/`.
        let stream = modes_for_paths(&["crates/app/src/server/streaming.rs".into()], None);
        assert_eq!(stream, PathSelection::Modes(["streaming", "server"].into_iter().map(str::to_owned).collect()));
        // Windows separators and leading `./` normalize cleanly.
        let win = modes_for_paths(&[r".\crates\app\src\about.rs".into()], None);
        assert_eq!(win, PathSelection::Modes(["about", "settings"].into_iter().map(str::to_owned).collect()));
        // Any unmapped or shared file forces a full run.
        let shared = modes_for_paths(&["crates/app/src/visualizer/beat.rs".into(), "crates/app/src/main.rs".into()], None);
        assert_eq!(shared, PathSelection::AllBecause("crates/app/src/main.rs".into()));
        let cargo = modes_for_paths(&["Cargo.toml".into()], None);
        assert_eq!(cargo, PathSelection::AllBecause("Cargo.toml".into()));
    }

    #[test]
    fn scan_seconds_parser_extracts_duration_from_log() {
        let log = "0.233 [INFO ] MarqueePlayer: window hooks installed\n2.136 [INFO ] MarqueePlayer::scanning: scan: 2553 files, 2553 read in 6 batches (the longest 29 ms), 0 removed, 0 moved, 1.9 s\n";
        assert_eq!(parse_scan_secs(log), Some(1.9));
        assert_eq!(parse_scan_secs("no scan here\n"), None);
    }

    #[test]
    fn default_jobs_scales_sensibly_with_core_count() {
        assert_eq!(default_jobs_for(1), 1);
        assert_eq!(default_jobs_for(2), 1);
        assert_eq!(default_jobs_for(4), 2);
        assert_eq!(default_jobs_for(8), 4);
        assert_eq!(default_jobs_for(10), 6);
        assert_eq!(default_jobs_for(32), 6);
    }

    #[test]
    fn clone_tree_produces_an_independent_writable_copy() {
        let base = std::env::temp_dir().join(format!("mp-clone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "original-a").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), "original-b").unwrap();

        clone_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "original-a");
        assert_eq!(std::fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(), "original-b");

        // Mutate and delete in the clone; the source must stay untouched.
        std::fs::write(dst.join("a.txt"), "changed").unwrap();
        std::fs::remove_file(dst.join("sub").join("b.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(src.join("a.txt")).unwrap(), "original-a");
        assert_eq!(std::fs::read_to_string(src.join("sub").join("b.txt")).unwrap(), "original-b");

        let _ = std::fs::remove_dir_all(&base);
    }
}
