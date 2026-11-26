# Testing

The bench runner, the scripted benches and the environment variables for measurement and tests.

## The bench runner

`cargo xtask bench` (on Linux inside a desktop session or
`packaging/linux/headless-session.sh`) builds the app with the bench harness, the fixture tools and
`crates/bench-runner` at below-normal priority, then runs the bench modes and prints one pass/fail
table; the arguments after `--` go to the runner, and it exits with the runner's exit code:

    cargo xtask bench                                    # quick working loop (fast profile, parallel)
    cargo xtask bench -- --only skip,gapless             # named modes (overrides --quick)
    cargo xtask bench -- --changed                       # modes for uncommitted changes
    cargo xtask bench -- --for crates/app/src/visualizer # modes for a path
    cargo xtask bench -- --list
    cargo xtask bench --profile release --all            # full release pass (including long modes)

By default `cargo xtask bench` is the working loop: the `fast` profile (`Cargo.toml`, shared with
`cargo build --profile fast` and `cargo xtask check` in `target/fast`, so dependencies are compiled
once), the modes that are long by design (`soak`, `bigscan`, `slowscan`) left out (`--quick`),
pre-scanned library templates reused across modes, and several parallel-safe modes running at once
(`--jobs`, scaled from the core count, capped at 6). Pass `--profile release` (built into
`target/bench` so it never overwrites or LTO-relinks the release binary) and `--all` for the full
pass before a release. What a mode checks is the same on either profile; frame and CPU timings, the
exe's size and the bit-exact audio tests are only meaningful on the release profile, which the full
pass and `cargo xtask release` use.

The runner (`crates/bench-runner`, never shipped):

- **Fixtures.** `cargo run --release -p library --example bench_fixtures -- <dir>` makes every set
  any mode needs, in one folder marked `.mp-bench-fixtures` inside the temporary folder (it refuses
  any other place): `core` (2,550 tracks: the albums the first benches name, "Lemonade", "Night
  Harbour", "Sampler", "Groove", 300 grid albums with covers, a 5.1 AAC track and a damaged FLAC
  in albums of their own, unreadable files), `formats` (the technical details' albums), `words` (the
  lyrics album), and through the other tools `viz`, `messy` (240 albums), `messy120` (the smart
  playlist bench's 120), `meta` and `edit` (marked `.mp-edit-fixtures`). The runner makes them in
  `%TEMP%\mp-bench-fixtures` the first time, and `big` (about 29,000 tracks, `messy_library` with
  2,600 albums, 600 MB) the first time `bigscan` runs.
- **Zero-copy read-only sets, CoW clones, and pre-scanned templates.** Single-set read-only modes
  point `MP_MUSIC` straight at `<fixtures>/<set>` with no copy; modes that mutate their music
  (`editing`, `metadata`, `lyrics`, `guard`, `session`), multi-set modes, and `--cold` runs get
  their own copy cloned copy-on-write where the file system allows (`clonefile` on APFS, `FICLONE`
  reflinks on Linux, falling back to a plain copy; plain copy on Windows). Each shared fixture set
  is scanned once into `<fixtures>/.mp-templates/<set>` (stamped with the app executable's and
  fixtures' sizes and mtimes so a rebuild or fixture change invalidates it automatically); eligible
  modes start from a clone of `library.db` and `cache/art` only (fresh `settings.json`, logs and
  session). Modes whose subject is the scan, grouping, guard, session moves, damaged files, or
  mutated fixtures still scan from nothing. Pass `--cold --jobs 1` to reproduce the original serial
  cold-scan run.
- **Parallel execution and `alone` modes.** Each mode gets its own data folder (and single-instance
  socket), output folder, and local mocks of LRCLIB and Last.fm on ephemeral loopback ports
  (`MP_LRCLIB_API`, `MP_LASTFM_API`: nothing leaves the machine), always with `MP_VOLUME=0` and
  `MP_WINDOW=secondary` (on macOS, parallel workers also pass `MP_WINDOW_SLOT=k/n` to stagger
  windows so none completely covers another and triggers `WindowEvent::Occluded`). Modes that
  measure frame intervals, CPU %, idle wake-ups, UI lag, real-time audio/beat timing, or change the
  system output device (`grid`, `sidebar`, `covers`, `playlist`, `gapless`, `skip`, `skip-damaged`,
  `idle-paused`, `idle-stopped`, `autoscroll`, `tray`, `visualizer`, `viz-idle`, `viz-styles`,
  `viz-cost`, `viz-look`, `output-default`, `soak`, `xfade-rates`, `streaming`, `bigscan`,
  `slowscan`, `two-instances`) are marked `alone` in `MODES` and run strictly one at a time after
  the parallel batch finishes.
- **Filtering by path.** `--for <path>` (repeatable or comma-separated) and `--changed [<git ref>]`
  map changed source paths to the modes whose subject lives there (`PATH_RULES` beside `MODES`);
  any unmapped or shared path falls back to running all selected modes.
- **Verdicts.** Every bench module counts its failed checks in one place (`bench::count_fail`, and
  `bench::verdict` for the modes that only reported text before); the app exits with code 3 when any
  failed. The runner: exit 0 and no `[FAIL]` line is PASS, 3 is FAIL, 101 or "panicked" in the log
  is PANIC, any other code CRASH, a run outliving its limit HANG (stopped by its handle). Reports,
  snapshots, the log and stderr of each mode are kept in `%TEMP%\mp-bench-runs\<mode>`, and the
  summary table records each mode's total, prep and scan seconds alongside the whole run's wall
  time against the sum of mode times.
- **`playlist`** (`bench/mod.rs`, then `bench/playlist.rs` and `bench/selection.rs`; the `core` set): a playlist of the library's
  first 3,000 tracks (2,550), how fast it opens, its scroll kept through Back; then Shuffle on it
  and playlist name clashes (`bench/playlist.rs`); then its selection
  by clicks with Ctrl and Shift held (key events of their own, then the click, all through the
  window's own dispatch), rows found by right clicks down the page: ranges, toggles, the moving
  anchor, Ctrl+A and Shift+End on every row (timed), Esc, the arrows; the menu of several tracks
  and each of its actions; a real drag of two chosen rows within the playlist; Delete; the
  selection gone when the list or the page changes and a menu opened before then doing nothing;
  the album page's list; the queue's Delete, block move and Esc. Snapshots `select-*.png`.
- **`server`** (`bench/server.rs`, the `formats` set): a Navidrome server against a stand-in the app
  starts on 127.0.0.1 (`subsonic::mock`): sign-in, the sync and its covers, two albums matched by
  their files, the server's own tracks, favourites and plays written back (never a rating, and none
  with the switches off), Stop, the server gone, the shrink guard, a refused sign-in, signing
  out, the cloud on the server's music, the top right of Home for every sync and rescan, an
  empty library's Home (see [Measurement and test hooks](#measurement-and-test-hooks)).
- **`listenbrainz`** (`bench/listenbrainz.rs`, the `core` set): Settings > Connections > ListenBrainz against a
  stand-in the app starts on 127.0.0.1 (`listenbrainz::mock`), reached through the card's own
  server address (plain http, noted): "Find a setting", the token page asked for and not opened, a
  token refused, the right one (the field emptied, `listenbrainz.json` without it), now playing
  and the listen of a track played, the server down then back (one `import`), a 429 waited out,
  the token refused while connected (the section marked, the queue kept) and connecting again,
  Copy diagnostics' line, love on and off, Disconnect, the log without the token; every request
  checked against the documented rules. Snapshots `lb-*.png`.
- **`streaming`** (`bench/streaming.rs`, no set): server-only tracks against a local mock of a Navidrome
  server inside the app (see [Measurement and test hooks](#measurement-and-test-hooks)).
- **`lyrics`** (`bench/lyrics.rs`, the `core` set): the Lyrics tab's timing (the buttons' callback and
  `G` through the window's key dispatch, the line lit, a click on a line, kept across a change of
  track and in `lyrics-offsets.json`), its menu with tag editing off and on, Copy (to the bench's
  stand-in clipboard), Save as .lrc over Night Harbour's `.lrc` (asked, untouched, then replaced
  with the offset in its stamps), Save as .txt for Paper Moons' plain lyrics, Save into the tags
  (one save in the journal, the `.lrc` still read first), and a server's track refused. It marks
  its own copy of the set `.mp-edit-fixtures` (only inside the temporary folder), so the write
  guard lets it write there and nowhere else.
- **`viz-look`** (`bench/viz_look.rs`, the `viz` set): the halo, the pinned preview (and the
  equalizer's curve), `Shift+V`, opacity and the user's presets (see [Measurement and test
  hooks](#measurement-and-test-hooks)). The runner makes the `viz` set again, in place, when it is one from before its bright
  and monochrome covers ("Paper", "Graphite").
- **`row-hover`**, **`row-hover-125`**, **`row-hover-150`** (`bench/row_hover.rs`, the `core` set; scale 1,
  1.25 and 1.5 through `SLINT_SCALE_FACTOR`): the pointer resting on a track row's play button
  on an album page, moved a quarter pixel back and forth through the window's own dispatch, the
  row drawn after each move (`take_snapshot`): it must stay hovered (no change in eight moves)
  and a click there must play that row's track, on six rows. It fails on the old row, where
  the button, a sibling over the row's TouchArea, took the hover from the row that showed it.
  **`row-hover-z70`**, **`row-hover-z200`**: the same at the interface size's ends
  (`MP_BENCH_ZOOM`; a window too short for the album's rows scrolls the page and tests those
  wholly shown). Each then, where the window is frameless (Linux), sizes the window down and
  checks its edge line is drawn on the left, right and bottom edges.
- **`output-default`** (`bench/output_default.rs`, the `core` set; Linux): PipeWire's default moved between
  the headless session's two null sinks with `wpctl set-default` before anything played,
  while playing, paused and stopped; each time Settings > Output's list, the equalizer's device
  and its setting, the stream's real link (`pw-dump`) and "Playing on ..." must agree with the
  default PipeWire names. It changes the default only between "Test Speakers" and "Test
  Headphones" and puts the first one back; elsewhere (a real desktop) it checks the start only.
  On Windows it says so and ends.
- **`eq-paste`** (`bench/eq_paste.rs`, the `messy` set): the equalizer's Copy and Paste, every format, the
  preview applied and cancelled (see [Measurement and test hooks](#measurement-and-test-hooks)).
- **`zoom`** (`bench/zoom.rs`, the `core` set, two starts): the interface size at 70%, 100%, 150% and
  200% (the scale factor applied, the caption buttons and the player bar inside the window, the
  grids' covers at the size drawn; `zoom-070.png`, `zoom-100.png`, `zoom-150.png`,
  `zoom-200.png`), its keys through the window's key dispatch (remapped and reset in the key
  editor), Settings' row and "Find a setting", Ctrl and the wheel and a pinch through their
  handlers (the pinch's first finger pressing Back: not clicked), another monitor's scale factor,
  the minimum size at 200% and 100%; then `zoom-check`, the next start on the same data folder:
  150% restored as the window comes up (`zoom-restored-150.png`).
- **`home-shelves`** (`bench/home_shelves.rs`, the `core` set): Home's hero and grid shelves by the
  pointer and the keys through the window's dispatch. The hero's play button: not drawn without
  the pointer or with it elsewhere on the card, drawn with it on the cover and on every move
  resting on the button and across its edge, a click there playing the album and one elsewhere
  on the cover opening it; the strip's single click featuring a cover and playing nothing, its
  double click (at once, and 150 ms apart) playing the cover clicked. The shelf editor's rows,
  clicked up to 6 and down to 1 and held there. Grid shelves of 1, 2 and 6 rows at window widths
  from 960 to 1920 and at 70%, 100% and 150%: the page as tall as the rows wanted (its furthest
  scroll), the first and last rows full; a shelf of fewer albums than a row holds in one row;
  End and the arrows stopping at the last album shown. Snapshots `hs-*.png`.
- **`xfade-rates`** (`bench/xfade_rates.rs`, the `formats` set): the 48 kHz Opus track crossfading into a
  44.1 kHz FLAC album in the real app: the device not reopened, the details, Settings > Output and
  the player bar saying the track is converted from 44.1 kHz, the album's next track gapless and
  still converted, and after a pause the track at its own 44.1 kHz with the details saying so.
- **Modes beyond `MP_BENCH`'s own:** `skip-damaged` (the skip bench on the damaged FLAC), `session`
  (session-save, the history track's folder moved, a second folder added, session-check), `guard`
  (a scan, then nineteen in twenty files deleted), `soak` (the whole library on repeat, a track change
  every 1.5 s for 3 minutes: memory, handles, GDI and USER objects, UI stalls), `bigscan` (the first
  scan of `big`, quit 3 s in, then started again to finish: progress, UI stalls, the app's own
  writes waiting on the scan, when albums were first on the screen, memory, snapshots as it goes
  and at the minimum window size), `slowscan` (the same on the messy library with a slow disk
  simulated, `MP_SLOW_DISK_MS=8`: quit 40% through, then started again, which must say it picks
  the scan up and finish it), and the
  scenarios of the app's own files misbehaving, checked by the runner itself: `data-files`
  (settings.json with values it cannot read, damaged lastfm.json and scrobbles.json, a broken
  theme.toml, a junk tag-edit record), `db-garbage`, `db-newer`, `db-flipped` (a library damaged
  inside: found by the start-up check), `data-readonly` (Windows: the data folder denied writing by
  an ACL, removed again after), `two-instances` and `crash` (a deliberate panic on the UI thread and
  on another: the message, the backtrace in the log, the log kept as the previous one).

## Measurement and test hooks

Environment variables, for development only. `MP_BENCH` and its companions, and the three that
send requests to another host (`MP_LASTFM_API`, `MP_LISTENBRAINZ_API`, `MP_LRCLIB_API`), need a build with the `bench`
feature (`--features bench`) or a test build; the rest work in every build:

| Variable | Effect |
| --- | --- |
| `MP_DATA_DIR=<dir>` | Keep config, database and cache in one folder instead of `%APPDATA%` / `%LOCALAPPDATA%` |
| `MP_MUSIC=<dir>` | Add a library folder at start |
| `MP_VOLUME=0` | Start at this volume |
| `--minimized` (argument) | Start in the tray, the window made but not shown ("Start with Windows" passes it). Ignored without a tray |
| `--version` (argument) | Print `Marquee Player <version>` and exit (the AppImage's launcher uses it to check the libraries load) |
| `MP_BENCH=grid\|sidebar\|tour\|gapless\|theme\|skip\|session-save\|session-check\|guard\|idle-paused\|idle-stopped\|covers\|playlist\|phase4\|phase5\|cleanup\|visualizer\|viz-idle\|settings\|autoscroll\|tray\|details\|metadata\|about\|home\|home-shelves\|library\|editing\|smart\|eq\|eq-paste\|viz-styles\|viz-cost\|perf\|play-cost\|viz-look\|server\|listenbrainz\|soak\|bigscan\|xfade-rates\|streaming\|zoom\|zoom-check` | Run a scripted measurement, write results and PNG snapshots to `MP_BENCH_OUT`, then quit (see `bench/mod.rs`). The frame timings (`grid`, `sidebar`, `playlist`) come from Slint's rendering notifier, set for those three modes only (Skia draws a window with one without partial rendering, so they measure with it off and the other modes as the app draws without the bench); on Skia's wgpu surface (Metal, Direct3D 12, Vulkan) the notifier is called only with Slint's `unstable-wgpu-30` feature, which the `bench` feature turns on. `SLINT_BACKEND=winit-skia-opengl` with a build with `--features bench-opengl` times them on OpenGL instead. A run that saw no frame says so and why, rather than reporting zeros. Memory in the bench's reports (`soak`, `bigscan`, `viz-cost`) is the physical footprint on macOS (Activity Monitor's "Memory", GPU memory included on Apple silicon; its lifetime peak as the peak), private bytes on Windows, the resident size on Linux |
| `MP_BENCH_GRID_PAGE=genres` | With `grid`: scroll the Genres page instead of Albums (the same grid with other tiles, to compare; it needs a library with a few hundred genres to have anywhere to scroll) |
| `MP_BENCH_GRID_SIZE=<w>x<h>` | With `grid`: resize the window's client area to this many physical pixels first (`2560x1392`: a maximized window on a 1440p monitor), for the columns and rows of a monitor larger than the one the bench's window opens on. The slow frames of a grid grow with the tiles on screen, so a stutter on a large window can be invisible at 1080p |
| `MP_BENCH_IDLE_SECS=<s>` | How long `idle-paused` / `idle-stopped` stay idle, hidden, before quitting (default 90) |
| `MP_WINDOW=secondary` | Open the window maximized on a monitor other than the primary one, without ever taking the foreground or keyboard focus (also when shown again from the tray); click into it to use it. One monitor: normal placement, still never activated. Bench runs honour it too | On Linux (no monitor to keep clear under a headless compositor): the window opens maximized and never asks for the focus, and the benches size it themselves |
| `MP_WINDOW_SLOT=k/n` | macOS with `MP_WINDOW=secondary`: stagger parallel bench windows (`0 <= k < n`, `n > 1`) so each window keeps its top-left corner uncovered by every other slot and macOS never marks it occluded (`ticker.rs`) |
| `MP_AUDIO=alsa` | Linux: play through ALSA's `default` device instead of PipeWire (the fallback where there is no PipeWire) |
| `MP_BENCH_PART=open` | With `phase5`: only files from outside (command line, a second launch, posted `WM_DROPFILES` of `MP_BENCH_DROP`, `;`-separated paths) |
| `MP_LASTFM_API=<url>` | Bench and test builds only: send Last.fm calls to this API root instead (a local mock). A release build ignores it, so nothing in the environment can send the session key elsewhere |
| `MP_LISTENBRAINZ_API=<url>` | Bench and test builds only: send every ListenBrainz request to this API root instead, whatever Settings' server address says (a local mock). A release build ignores it: the user's own "Server address" is the supported way to send elsewhere |
| `MP_LRCLIB_API=<url>` | Bench and test builds only: send lyrics lookups to this API root instead of `https://lrclib.net` (a local mock) |
| `MP_BENCH_VIZ_SECS=<s>` | How long `viz-idle` lets the visualizer run before pausing and hiding (default 20; then `MP_BENCH_IDLE_SECS`, default 60) |
| `MP_BENCH_EDITION=installed\|installed-all` | Bench builds only: act as the installed edition (for you, for everyone) without any registration: the installer's mutex and quit event, the Default apps button, About's line |
| `MP_BENCH_DETAILS=off` | With `details`: the same views with the setting off (`off-*` snapshots), and no name prompts |
| `MP_BENCH_TRAY_SECS=<s>` | How long `tray` plays with the tray animation off, then on (default 30 each) |
| `MP_VIZ_DRAW=none\|minimal` | Measurement only: analyse without drawing, or redraw a 1 px rectangle per frame instead of the bars |
| `MP_VIZ_AA=all\|fills\|strokes\|none` | Measurement and comparison only: which of the visualizer's paths ARE anti-aliased (`all` the drawing on Windows, Linux and 1x; `strokes` only the line and its glow, the macOS policy at a scale factor of 2 or more; `fills` the fills, the ripples and the rings; `none`). Unset: the policy (`visualizer::aa_policy`). Bars stay without anti-aliasing either way. Works in every build |
| `MP_BENCH_VIZ_PART=<parts>` | With `viz-styles`: only these parts (comma-separated: presets, colours, min, settings, menu, moments, sheet, idle; `none` for just the start) |
| `MP_BENCH_VIZ_CASES=<words>` | With `viz-cost`: only the cases whose name contains one of these (comma-separated). One case's whole name (`halo, behind`) is that case alone; with a semicolon, the parts between semicolons, each a name or a piece of one (`halo, behind; saturn, panel`) |
| `MP_PERF_SCENES=<scenes>`, `MP_PERF_SECS=<s>`, `MP_PERF_SETTLE_MS=<ms>` | With `perf`: the scenes in order (`home`, `np-off`, `bars`, `halo`, `grid`, `prep`; default `home,bars`), seconds measured (default 8) and settling (default 2500 ms) each; lines of JSON after `MP-PERF ` on standard output (see [Comparing two builds](#comparing-two-builds-cargo-xtask-perf)) |
| `MP_PERF_SOUND=made-up`, `MP_PERF_SNAP=<n>` | With `perf`: the visualizer hears a sound made up from its frame number, its time step 1/60 s (two builds draw the same frames); the window's picture after frame n of a visualizer scene (`perf-<scene>-<n>.png` and `.rgba`) |
| `MP_BENCH_LOOK_PART=<parts>` | With `viz-look`: only these parts (comma-separated: halo, pin, keys, opacity, presets) |
| `MP_HALO_SHAPE=square` | Measurement and comparison only: the halo follows the cover's rounded square instead of a circle (the cover drawn square), as tried and left out. Works in every build |
| `MP_HALO_WITHOUT=<words>` | Comparison only: the halo's ripples without some of what moves them (comma-separated: `throw`, a kick throwing its ripple; `push`, kicks pushing those on their way; `swells`, a ripple on a swell where there are no kicks, a timer instead). Works in every build |
| `MP_HALO_WITH=rings` | The halo's snare rings, which are off: a thin white ring on a snare or a clap on the beat. Tried on real music and left out (a ring could not be connected to a sound heard); kept for another try at tuning them. Works in every build |
| `MP_BARS_WITH=flash` | The bars' flash, which is off: the bars' area and reflection brighter on the beat (their alpha 1.3 times at a full one, settled before the next). Watched on real music and left out (too jarring, and at times with nothing much heard); kept for another try. Works in every build |
| `MP_POWER=battery\|mains\|low-power` | macOS, bench and test builds only: the power the Mac runs on, for the visualizer's automatic frame rate (`battery` or `low-power`, which is Low Power Mode on the adapter: 30 a second; `mains`: up to 60; `battery,low-power` both), in place of the system's, which is then not watched. `viz-idle` checks the frames come at the rate in effect. Ignored on Windows and Linux, which have no automatic rate |
| `MP_REDUCED_MOTION=1\|0` | Stand in for Windows' "Show animations in Windows" (off: the visualizer's default look is Calm). Works in every build |
| `MP_BENCH_SKIP_ALBUM=<title>` | With `skip`: the album to play (default: the first of four tracks or more) |
| `MP_BENCH_MOVED=<text>` | With `session-check`: the moved history track's path must contain this |
| `MP_BENCH_SOAK_SECS=<s>` | How long `soak` plays (default 120) |
| `MP_BENCH_QUIT_AFTER=<s>` | With `bigscan`: quit this many seconds in, scan or not |
| `MP_BENCH_QUIT_AT=<fraction>` | With `bigscan`: quit once the scan is this far through (`0.4`) |
| `MP_BENCH_STOP_AT=<fraction>` | With `bigscan`: press Settings > Library's Stop once the scan is this far through, check it stopped with Resume offered (`bigscan-stopped.png`), press Resume and check it picks the scan up |
| `MP_SLOW_DISK_MS=<ms>` | Simulate a slow disk: every folder the scanner lists and file it opens takes this much longer, one at a time (see [Scanning](features/library.md#scanning)). Works in every build |
| `MP_SCAN_TIMING=1` | Print when each part of a scan ended, and how long each batch held the database (stderr) |
| `MP_BENCH_ANSWER=yes` | A bench run's answer to the "start with a new library?" question for a damaged database (none: no) |
| `MP_BENCH_REAL_CLIPBOARD=1` | Linux only, with `eq-paste`: also copy and paste through the toolkit's real clipboard (for the private headless session, whose clipboard is nobody's; never set it on a desktop someone uses) |
| `MP_LOG=debug` | More logging, and once a minute a `wake-ups a second` line (`wakes.rs`): the process's CPU as a share of one core, and per second the output thread's loop turns, a pull device's callbacks (Core Audio, PipeWire; each wakes the output thread), UI ticks, visualizer frames, tray icon frames, the window's winit events and the redraws asked of winit (not those Slint's macOS display link draws by itself). The window being covered or seen again (macOS, X11) is logged as it happens |

Only one instance runs per data folder: launching the app again brings the running
one's window back (from the tray too) and exits. An instance on another `MP_DATA_DIR` is a
separate one, so test and bench runs never touch, nor are stopped by, the real one.

`MP_BENCH=metadata` runs on the metadata viewer's fixtures (FLAC with a full Picard tag set,
MP3 with ID3v2.4 and with ID3v2.3 plus ID3v1, Opus with R128, M4A with iTunes atoms, an
untagged WAV, an inconsistent album, a long comment with 3,000 extra items and a large
picture, a 400-track album; it deletes and breaks one file each in the folder after the
scan). Make them in an empty folder (never a music folder) and point `MP_MUSIC` at it:

    cargo run --release -p library --example meta_fixtures -- %TEMP%\mp-meta\music
    cargo run --release -p library --example meta_dump -- "<a file>"

`meta_dump` prints what the viewer reads from any file (read-only).

`MP_BENCH=editing` runs on the tag editor's fixtures (the metadata viewer's set, plus Ogg
Vorbis, ALAC, M4B, MP2, MP3 with three tag blocks and with unusual frames, WAV and AIFF with
and without their own text tags, untagged files, raw AAC and a 20-second FLAC to save while it
plays), made in a folder marked `.mp-edit-fixtures` inside the temporary folder (the tool refuses
any other place; every bench build refuses to write anything outside such a folder):

    cargo run --release -p library --example edit_fixtures -- %TEMP%\mp-edit\music

It checks the read-only viewer with editing off, turns editing on in Settings, edits a track by
the keyboard (a title, a second artist moved first, an invalid number, "1/7"), reviews, saves and
undoes it (the file checked on disk, its audio unchanged), edits by the pointer, asks before
closing with a draft, applies an album's fixes, an album field and a table row, replaces the
album's cover from an enormous picked image (the bench supplies the pick: no dialog), saves seven
files and checks the library after the rescan, stops a save at a read-only file and undoes what it
wrote, saves a track while it plays, and snapshots each state (`e-*.png`, also at the minimum
window size). The fixtures are changed by the run: make them again before the next.

`MP_BENCH=home` and `MP_BENCH=library` run on a messy fixture library: 2,754 short tracks in
mixed formats, most tagged in full, many not, the way real libraries are not (compilations
without an album artist, untagged "01 - Title" files, case and spacing variants, disc numbers
on some tracks only, "A1" numbers, two spellings of a title, loose singles, featured artists in
titles and artist fields, multi-valued tags in each format's own way, a 5.1 AAC file). Make it
in an empty folder (never a music folder):

    cargo run --release -p library --example messy_library -- %TEMP%\mp-messy\music

`library` checks and snapshots the compilation, the regrouped albums, the artist pages, the
disc headers, the album page at the minimum width with the sidebar pinned, View metadata of
the odd files and the 5.1 AAC note; `home` snapshots the home page at 1920 x 1080 and at the
minimum size.

`MP_BENCH=smart` runs on the messy library too (made with any number of albums, e.g. `--
%TEMP%\mp-smart-bench\music 120`), with favourites and plays seeded in its own
database (`bench/smart.rs`): it builds a smart playlist in the editor by the keyboard (the name,
Tab to the first rule, its field from the list by typing and Enter, its value, "Add
rule" by Enter) and by the pointer (an entry clicked in a list), a genre picked by typing, a
value typed wrong and explained, a save refused and fixed, a group, the limit; saves it and
checks the page against the rules; checks live updating (a "never played" list loses a track
once its play counts while the queue keeps it, a "most played this year" list holds what its
rules do, a favourites list follows `L`), Reshuffle, a rule from a newer build kept and
shown, a playlist that cannot name itself, Delete's hint, "Add to playlist" offering ordinary
playlists only, a home shelf, Save as ordinary playlist, Export, the Playlists page and the
sidebar; then search operators (`plays:>=1 -is:remote` typed into the search field: tracks only,
all played; the hint for `re:`; the syntax help; `year:` listing albums too; a plain query
still plain; both paths timed; Save as smart playlist opening the editor with the rules, and the
saved playlist holding what the search found); `s-*.png` at 1920 x 1080 and 960 x 620, and a count of failed checks at the end of
`bench-smart.txt`.

`MP_BENCH=viz-styles` and `viz-cost` (`bench/viz_styles.rs`) run on the visualizer's fixtures: three
albums of synthetic audio, each with its own cover (so accents and palettes differ): the test
signals `MP_BENCH=visualizer` and `viz-idle` use ("Spectrum": sweeps at 48 and 96 kHz, three
tones, pink noise, a groove that drops 20 dB after 10 s), and mixes with the spectrum of real
music ("Night Shift": house, hip-hop, a quiet intro, build, drop and breakdown; "Daylight":
rock, a quiet ballad, ambient 30 dB down), each a kick-like low pulse, bass, pads or chords
and hi-hat-like noise bursts, synthesised in Rust and encoded to FLAC by ffmpeg. Make them in
an empty folder:

    cargo run --release -p library --example viz_fixtures -- %TEMP%\mp-viz\music

`viz-styles` snapshots every preset in every placement at 1920 x 1032 and at 960 x 620 (with
the sidebars pinned, with auto-hide on), the colour schemes by height and across, moments of
the realistic fixtures in three looks, a strip of twelve consecutive frames for three looks
(`vs-sheet-*.png`, one frame each 17 ms), Now playing's menu (by the keyboard, into Style),
`V`, `C` and `Y`, the panel's tabs, Settings > Visualizer (live, still, scrolled, by the
keyboard, Reset, at the minimum size) and whether anything runs when it should not; `vs-*.png`
and a count of failed checks at the end of `bench-viz-styles.txt`. `viz-cost` measures every
style in every placement (`MP_BENCH_VIZ_SECS`, default 5, a case) and two at 30 frames a
second: each case's line has its frames, frame intervals, work per frame, the process's CPU and
its memory at the case's end, under a first line with the window's physical size and scale
factor.
`MP_BENCH=play-cost` (beside it) is for a profiler outside the app to line up with: the same
album playing on Home, on Albums, on Now playing with the visualizer off, with the bars behind
the page (the default look), with Halo behind it, and with the window hidden,
`MP_BENCH_VIZ_SECS` each (`MP_BENCH_VIZ_CASES` picks phases by name). Each phase's start is
logged with the time ("play-cost phase <name> starts at unix <t>"); its end with the process's
CPU and memory, and under it every frame the window drew, whatever asked for it (the ticks'
as much as the visualizer's sixty) and how long the renderer took over one
(`bench-play-cost.txt`). On Apple silicon read the CPU figures with the machine's clock in mind:
light periodic work runs several times slower there than the same work back to back, and the
share of a core counts time, not work ([Visualizer](features/visualizer.md), "What the share of
a core means on a Mac"). The tick redraws the player bar only when its seek bar moves a
physical pixel or the time turns over (`playback::bar_moves`), and on macOS with the time or 3
logical pixels on (`playback::bar_due`); the album's tracks are 30 to 36 s, which move it every
tick, so the phases without the visualizer draw about ten frames a second (about four on
macOS) whatever that saves on a long track (a folder of one's own with a longer "Night Shift"
in `settings.json`'s `folders` shows it: a 4 minute one draws one frame a second on macOS).

`MP_BENCH=viz-look` (`bench/viz_look.rs`) runs on the visualizer's fixtures, which now have two more
albums of one track each under a bright cover ("Paper": yellow, coral and sky on cream) and a
nearly monochrome one ("Graphite": cool greys), beside the dark "Night Shift": the halo in each
placement under the three covers at 1920 x 1080 and 960 x 620 (`vl-halo-*.png`; with
`MP_HALO_SHAPE=square`, `vl-halo-square-*`), its edge moving and closed; the pinned preview
beside the column and held at the top of a narrow one, scrolled, by Tab, paused (the still
frame), under "Find a setting" (none), on entering the section (its top), and the equalizer's
curve the same way; `Shift+V` through the three placements (the panel's tab chosen), with the
visualizer off and on another page, `V` alone, Settings > Keys and the key editor; opacity by
the keyboard and the pointer, behind the page and elsewhere apart, the quick menu's Opacity ▸
by the keyboard, saved; the user's presets: Save as by the keyboard, the picker, `C` into them,
Rename by the pointer (a built-in name refused), Update, Copy (the menu, `Ctrl+C`), Paste of
the same preset ("Use it"), of a friend's line inside a message (shown in the preview, added by
Enter), of a taken name (Keep both, Replace), refused pastes (no preset, a newer format, an
unknown style, 40 KB), Export, Delete, Import (Esc, then Add by the pointer), a binary file
refused, the quick menu's Your presets ▸, and settings.json read back after each change.
`vl-*.png` and a count of failed checks at the end of `bench-viz-look.txt`. It never touches
the real clipboard and opens no file dialog.

**The halo, traced** (`visualizer/trace.rs`, an ignored test: a tool, not a check). Nobody can watch the window move from a terminal, so this runs the real beat tracker, analysis and drawing over a stretch of sound at 60 frames a second and writes what happened: `trace.jsonl` (every frame: the kick, snap, tempo (`per`, the beat's length), beat, phase, the low notes' onset (`low`) and brightness told, what the looks were given as the beat after `beat_hit` and `one_hit` (`given`, 0 for nothing), where each ripple and ring is) and the frames of chosen moments as SVG (the frame's own paths under its own brushes, to scale: what is too faint there is too faint in the window).

    ffmpeg -i track.flac -f f32le -ac 1 -ar 48000 track.pcm
    MP_TRACE_PCM=track.pcm MP_TRACE_OUT=<folder> MP_TRACE_FRAMES=12-12.7 cargo test -p marquee-player halo_trace -- --ignored

`MP_TRACE_STAGE=1300x1300x340` (width, height, hub), `MP_TRACE_STRENGTH=0.85` (the look's opacity) and `MP_HALO_WITHOUT` apply. A timeline of the ripples' distance against time, with the kicks and snaps on it, shows skipped beats, ripples by the clock and rings on the wrong sounds at a glance.

**Saturn, traced** (`saturn_trace`, beside it). Saturn's frames over made-up music (120 and 170 a minute with a kick on every beat, 72 a minute with soft beats only, weak kicks the low notes bear out, and 120 a minute going quiet for four seconds; `MP_TRACE_CASES` picks some: `120`, `170`, `slow`, `quiet`), 60 a second for 12 seconds each, behind the page's size (a cover 300 px across): every grain's place, size and layer each frame (`<case>.bin`, the format in its comment) and the brushes (`saturn.json`), for an animation of it as drawn.

    MP_TRACE_OUT=<folder> cargo test -p marquee-player saturn_trace -- --ignored

**The path text, timed** (`path_text_cost` in `visualizer/draw.rs`, an ignored test: a measure, not a check). Ten seconds of made-up music (a kick on every beat) drawn by Classic and by Halo on the page's stage in a 1470 x 922 window, the paths written as they were (`write!` with `{:.1}`, kept for the tests behind `AS_BEFORE`) and as they are (`draw::tenth`), back to back, and each frame's paths parsed as Slint parses them (lyon's SVG parser, a dev-dependency at the version Slint uses). Prints the nanoseconds a frame to build the paths (the whole of `Draw::draw`) and to parse them, the best of five rounds, and the bytes a frame. Beside it, `every_look_reads_back_as_the_path_it_was_written_as_before` holds every preset on five kinds of stage, two seconds each, to the old text: the parser builds the same path from both, point for point (but for the small dots, which are curves where they were arcs: the same dots, each starting where it did, and `a_small_dot_is_two_curves_no_more_than_a_twentieth_of_a_pixel_from_its_circle` holds each to its circle).

    cargo test -p marquee-player --profile fast path_text_cost -- --ignored --nocapture

`MP_BENCH=server` (`bench/server.rs`) runs on any library of two albums of two tracks or more (the
runner gives it the `formats` set). It starts a Navidrome stand-in on 127.0.0.1 itself
(`subsonic::mock`; no request leaves the machine) serving two of the library's albums as the same
files under other titles, and 30 albums of its own (26 with covers), and drives Settings > Library
> Server: the signed-out form, "Find a setting" by "navidrome", the plain http note, a wrong
password refused in words, the right one (the field emptied, `navidrome.json` without the password
and, on Windows, without the token readable), the sync's progress (sidebar and Settings, the
stand-in slowed), the two albums one each with the files' titles, the covers in the cache, the log
and Copy diagnostics naming neither the address nor the user, no Show in Explorer and View
metadata "From the server" for the server's tracks, an M3U export leaving them out, About's
counts, Sync now listing again, a start's sync asking two questions (its scan, its stars), what
goes back to the server (the three switches, a heart sent at once and never a rating, a play
reported with its time, the server's heart here after Sync now, both switches off: not one
write, on again: what changed meanwhile sent, the Last.fm switch), Stop, the server gone (a
quiet line), the shrink guard held then confirmed, a refused sign-in (signed out, marked, the
music kept), signing in again and out. On the way: the cloud on a server album's page and its
tracks, not on a local album the server also has, on the server's tiles only in Albums, and on a
playlist's server tracks only (`sv-icon-dark.png`, and `sv-icon-light.png` in light colour
tokens); the tooltip's host without the scheme; the top right of Home (`activity::take_log`) for
the start-up scan, the sync after signing in, Sync now, a start's sync, one back from the tray, a
rescan of the scanned library and a rescan with a sync right after it, each at least 700 ms,
ending at full, one after the other without a gap. Then the folders are removed: an empty Home
offers both sources, Connect a server opens Settings > Library with the sign-in focused, and the
first sync of an empty library (102 albums, the stand-in slowed) shows on Home in its large words,
its albums coming in before it ends. `sv-*.png` and a count of failed checks at the end of
`bench-server.txt`.

`MP_BENCH=eq` runs on any library of two albums of three tracks or more (the messy fixture
library will do): Settings > Equalizer off, on, with a preset, by the keyboard (Tab to the
sliders, Right, Up, Page Up, Home, Delete), the parametric view, an AutoEq file imported (its
preview, then applied) and a file of IIR coefficients refused, the preset menu, "Save as
preset…" typed, export as Equalizer APO text and as this app's own file read back, a track
playing at volume 0 (the details naming the equalizer, a device's own setting on and off), the
player bar's indicator on and off and its button, the crossfade settings and the transitions
handed to the engine (an album in order gapless, the next album crossfading, repeat one
gapless); `q-*.png` at the window's size and at 960 x 620, and a count of failed checks at the
end of `bench-eq.txt`. It writes its AutoEq fixture and export into `MP_BENCH_OUT`.

`MP_BENCH=eq-paste` (`bench/eq_paste.rs`, any library) drives Copy and Paste: Copy by its button and
by `Ctrl+C` on the page, "Copy as" GraphicEQ by the keyboard and EasyEffects by the pointer,
each read back; Paste of each format (AutoEq's FiiO FT1 files: ParametricEQ by
`Ctrl+V`, FixedBandEQ imported as a file, GraphicEQ fitted; Room EQ Wizard's export,
EasyEffects with a band left out, PulseEffects, this app's own file with every kind) with the
preview checked, applied by Enter and by the pointer, cancelled by Esc and by Cancel (the
equalizer untouched), shown again before the last preview had gone; text that is no equalizer
(the pill) and IIR coefficients (refused in the preview); the graphic view of a setting it
cannot show (nothing changes); a band's kind from its menu by the keyboard and the pointer,
stepped; "Find a setting". `p-*.png` at 1920 x 1080 and 960 x 620, failed checks at the end of
`bench-eq-paste.txt`. It never touches the real clipboard (the stand-in, `explorer::
bench_set_clipboard`); on Linux, `MP_BENCH_REAL_CLIPBOARD=1` adds a round trip through the
toolkit's clipboard, for the private headless session only.

`MP_BENCH=streaming` (`bench/streaming.rs`) needs no music folder: it makes its own fixtures with ffmpeg
(20 s of FLAC, MP3, Opus and an M4A with `moov` at the end, and two FLAC halves of one 8 s signal,
in `%TEMP%\mp-stream-fixtures`), starts a local mock of a Navidrome server on 127.0.0.1 in the app
(`server/streammock.rs`: the original file with ranges, a throttled link, stalls, drops, 503s, 404s), writes
a small catalog of them into the library as server-only tracks (`library::remote::sync`) and signs
the streamer in to the mock. It times the first sound of each format on a link that takes 6 s for
the file (each well before its download ends; Opus and the M4A by a range for their end), checks the
loading ring and, after a second, the pill, a seek far past the download (a range there), a resumed
position, two halves gapless (the second fetched as it became next, never waited for), a cached file
played with no request at all with its server lyrics synced and the details saying "from the
server", a song gone from the server (said, not retried, the queue on), signed out (one skipped,
then stopped with one reason; a track chosen not skipped), the cap (the playing track kept), a skip
from a download under way (its thread and connection gone within a moment), Settings > Playback's
card and quitting with nothing left running; `st-*.png` and a count of failed checks at the end of
`bench-streaming.txt`. Nothing leaves the machine.

To time smart playlists at scale without a music folder, make a fixture database (20,000 tracks
by default, a play history, the starting points and a smart playlist naming another) and time
every smart playlist in it and a set of heavier rule sets:

    cargo run --release -p library --example smart_fixture -- %TEMP%\mp-smart\library.db 20000
    cargo run --release -p library --example smart_time -- %TEMP%\mp-smart\library.db

`browse_time` times the browse paths (the Albums page in its orders, the Artists, Genres and
Labels indexes, a genre shelf, an artist's page, a 3,000-track playlist, search) on a library
database; `artist_report` prints every album artist's page, for comparing a library before
and after a migration:

    cargo run --release -p library --example browse_time -- <library.db> [artist]
    cargo run --release -p library --example artist_report -- <library.db>

To time search without a music folder, make a fixture database of mixed Latin, accented,
kana and featured-artist names and time queries on it:

    cargo run --release -p library --example search_fixture -- %TEMP%\fixture.db 4000
    cargo run --release -p library --example search_time -- %TEMP%\fixture.db beyonce kameria "kasane teto"

`cargo test -p player-core --release` checks gapless and seek against generated
fixtures. With `MP_REAL_ALBUM=<folder of .opus/.flac>` it also decodes a real album
and compares it, sample by sample, to ffmpeg's libopus decode.

## Comparing two builds: cargo xtask perf

`cargo xtask perf` answers one question about the working tree: is it cheaper to draw than its
base, by how much, and how sure is that. It builds both, runs a few short scenes of the real app
on each, interleaved, and prints one table with a verdict for each scene:

    cargo xtask perf                              home and bars, against where this branch left master
    cargo xtask perf --base HEAD --scenes all     the uncommitted change, every scene
    cargo xtask perf --snap 300 --scenes bars     and whether both draw the same picture

**The builds.** The change is the working tree as it is; the base is `--base <ref>` (any commit,
branch or tag), else the merge base with master, checked out in a detached `git worktree` of its
own under `<target>/xtask-perf/base-src` and kept between runs (the user's checkout and branches
are never touched; an unchanged base is not checked out again, so it costs no rebuild). Both
build at once, on the `fast` profile with the bench feature, each in its own target folder
(`<target>/xtask-perf/change`, `.../base`): one target folder shared by two checkouts keeps the
first one's UI, as slint-build records the absolute paths of the `.slint` files it read. The
first time, each folder is seeded by cloning one already built (`target/bench/fast`, else
`target/fast`; `cp -c -R`, copy-on-write on APFS, instant), and the build-script outputs of the
workspace's own crates that name another checkout's files are removed, so they run again here
(once). A base from before the perf bench existed gets this checkout's `bench/perf.rs` grafted in,
registered in `bench/mod.rs`, with the bench feature's `slint/unstable-wgpu-30`: bench-only code,
said in the output.

**The scenes** (`MP_BENCH=perf`, `crates/app/src/bench/perf.rs`), each a steady state of the real
app, left to settle (2.5 s) and measured (8 s): `home` (music playing on Home, nothing else
moving), `np-off` (Now playing, the visualizer off), `bars` (the default visualizer behind the
page), `halo` (the halo behind the page), `grid` (All albums scrolled up and down at 2,400 px a
second). They run on the bench fixtures (`<temp>/mp-bench-fixtures`, made if missing: the core
set's 300 grid albums with covers and the visualizer's "Night Shift", cloned once into
`<temp>/mp-perf-music`), at volume 0, on `MP_WINDOW=secondary`. Each build first scans its own
copy of that library and brings every cover on screen once (both at once); each pass then starts
from a fresh copy of that folder. The app reports, for each measured span, the visualizer's frames
and their spacing and work, the window's own frames (Skia's wgpu surface calls the rendering
notifier only with Slint's `unstable-wgpu-30`, which the bench feature turns on; that surface
draws the whole window every frame either way), its CPU time and its memory (the physical
footprint on macOS); from outside the command reads the window server's CPU time (`ps` of
`WindowServer`) and the GPU's utilisation once a second (`ioreg`, "Device Utilization %") over the
same span.

**The made-up sound.** The visualizer follows the sound, so two runs never draw the same frames.
In these scenes it hears a sound made up from its frame number instead (a kick on the beat, a
bass line, a chord, hi-hats), its time step fixed at 1/60 s: two builds draw the same frames, so
the comparison is quieter, and with `--snap N` the window's own picture after frame N
(`take_snapshot`, not a capture of the screen) is saved from each build and compared pixel for
pixel (how many differ, by how much, where). N counts from the scene's start, its settling
included, so up to about 600 at 60 frames a second. The time label moves with the clock, so a few
pixels there always differ: with no change, frame 300 of bars and of the halo differed in 18 to 26
of 5.4 million pixels, all in the time label under the seek bar; the visualizer itself was the
same to the pixel. A base whose visualizer cannot hear it (from before it
existed) hears the fixtures' music on both sides, said in the output; `--real-sound` asks for that.

**Why with the clock held.** On this Mac a CPU percentage counts time, not work: light periodic
work (a frame every 16.7 ms, then idle) runs 4 to 5 times slower than the same work back to back,
on the efficiency cores or at a low clock, and the figure moves with whatever else the machine
does (bars behind the page showed 34% of a core on a quiet machine, 9.3% with two other cores kept
busy). With two busy processes (`yes`) running for the length of the measuring the clock holds,
and the app's CPU repeats from run to run to about a tenth of a point. So the measuring passes run
with two cores kept busy, and a cost is CPU time per frame (per visualizer frame for `bars` and
`halo`, per window frame for `grid`, per second for `home` and `np-off`, where little is drawn).
The figures are therefore smaller than what the machine shows in normal use, where the same work
runs slower: compare them with each other, never with Activity Monitor. The display is kept awake
(`caffeinate -d`) for the measuring only.

**Watts.** CPU alone can mislead: a change that moves work to the window server or the GPU can cut
the app's CPU and raise the machine's power (it happened: the visualizer in a layer of its own,
the app's CPU to a third, the whole machine's power up by half). So every scene also has the
window server and the GPU; and where `sudo -n` may run exactly `/usr/bin/powermetrics --samplers
cpu_power,gpu_power -i 1000 -n 10` (a sudoers rule allowing that one command without a password;
the command never asks for one), a second pass without the busy processes gives the CPU's and the
GPU's watts, ten samples a second apart over each scene, one pass a build, compared by the
samples' standard error. Where it may not, the output says so. The rule, made by the machine's
owner (`sudo visudo -f /etc/sudoers.d/powermetrics`):

    <user> ALL=(root) NOPASSWD: /usr/bin/powermetrics --samplers cpu_power,gpu_power -i 1000 -n 10

The command asks `sudo -n -l` whether that command would run, running nothing and never asking
for a password. (Its reading of `powermetrics`' text was written from the tool's documented
layout and has not yet met its real output on this Mac: check the first table that has watts.) `--confirm` adds the slow check: the battery controller's own figure of the whole
machine (`ioreg -rw0 -c AppleSmartBattery`, "PowerTelemetryData"; it refreshes once a minute, so
each scene runs about three minutes on each build), for a final word, not for every change. It
is one reading a build of everything the machine does: with no change, Home read 6.37 W and 5.58 W
while another agent worked on the same Mac, so it means something only on a machine left alone.

**What a verdict means.** The passes alternate, base then change, then change then base, and so
on, so a drift of the machine over the run weighs on both sides alike. For each measure: the
base's and the change's mean over the rounds, the difference (as a share of the base) and the
spread between rounds (the larger of the two sides' ranges, as a share of the base). A difference
counts only when it is larger than that spread, larger than the floor, and the rounds do not
overlap (every round of one side below every round of the other). The floors are the tool's own
noise, measured with no change: 7% for the app's CPU, 10% and 4 points of a core for the window
server, 10% and 10 points for the GPU (both move with whatever else draws on the screen), 10% for
memory, 5% for watts. The scene's verdict is the app's CPU's: **cheaper**, **dearer** or **within
the noise**; "moved, not saved" when the app is cheaper but the window server or the GPU is
dearer. One round gives figures but no verdict; `--rounds 4` (about twice as long) when a
difference sits near the floor.

What a verdict does not mean: that the change is cheaper everywhere (only in these scenes, on
this machine, with the clock held: a change that only helps at a low clock does not show here,
and a change of how often something wakes shows only as far as these scenes wake it), that it
looks the same (`--snap` for that), or anything under the floor (a 3% gain is within the noise
however often it repeats).

**The tool's own noise**, measured on 2026-11-26 with the base equal to the change (`--base HEAD`,
nothing changed), another agent running the app's benches on the same Mac at the time: the app's
CPU differed by -5.8% (Home), -1.7% (Now playing, off), -1.8% (bars), +1.4% (halo), +0.4% (grid)
in one run of two rounds; by -6.6% (Home) and -5.3% (bars) in another, which the 3% floor first
tried called "cheaper": hence 7%; with four rounds, bars +1.3% (single runs within ±2.5%). The
window server differed by up to 36% (3 points of a core) and the GPU by up to 33% (9 points) with
no change. A known change was found with the right sign and about its size: the position's timer
at 50 ms instead of 100 ms (Home's window frames 9.9 to 19.7 a second) read Home dearer, +66% and
+51% in two runs; the visualizer's paths built twice a frame read bars +9.8% a frame, dearer, in
one run (its own measured work +0.15 ms of the +0.19 ms found) and +6.7%, within the noise (its
rounds ±13.6% apart), in another. So with another app at work beside it, two rounds resolve a
change of about 10% only some of the time; a change of 15% or more, reliably; under 7%, never.

On a quiet machine it does far better. The same no-change run of all five scenes with nothing
else at work (2026-11-26): the app's CPU differed by -0.1% to -0.8%, its rounds ±0.3% (halo) to
±1.7% (bars, grid) apart, every scene within the noise. The floor stays at 7% by default, the
figure a shared machine needs; `--floor 3` is defensible when the run's own "other work" line
is low on both sides and the rounds are within a percent or two of each other (the first real
comparison, the branch `macos-perf` against `master`, read bars -6.9% with its rounds ±0.9%
apart: a real difference the default floor called noise).

**A rate changed on purpose** (`--rates-differ`): where the change draws at another rate than
the base (30 frames a second on the battery against 60), the cost of a frame says nothing and
the lower rate would be taken for a covered window. With this the scene's cost a second is
compared (the app's CPU as a share of a core, the window server, the GPU, watts), and a side
is refused only if it drew no frames at all.

**Other work on the machine.** The busy processes hold the clock, but they do not make the
measure immune to other work: with a `cargo build` looping in another folder, the same build read
19% dearer on bars, the window server 18%. So over each span the command reads the whole machine's
busy cores (`iostat`) and takes away the app, the two busy processes and the window server: what
is left is other work, shown in the table (0.3 to 0.6 cores while the other agent's app ran
quietly beside it). More than one core of it in any run, or half a core
more on one side than the other, and the scene's verdict is "not trusted" (what it would have
read is shown beside it); when no scene is left to trust, the exit code is 3. It caught every
false verdict seen while another agent ran benches beside it (halo and grid read 25% and 38%
cheaper with no change, with 1.8 to 3.4 other cores busy during the base's passes) and under the
looping build (bars read 21% cheaper, 3.8 other cores during the base's passes, 1.6 during the
change's); it cannot see
work that costs little CPU and much drawing (another app animating: the window server and the GPU
then move, and are judged with their own wide floors).

**It refuses rather than mislead.** A locked screen (nothing draws; `IOConsoleLocked`), before or
during the run, stops it with exit code 3 and says so. A scene where a build drew fewer than half
the frames it should (the window covered, hidden or asleep) is "not measured", without a verdict;
a busy machine makes it "not trusted"; when no scene is left, the exit code is 3. A scene that did
not hold its frame rate (under 90% of the display's) is said to be so beside its verdict. A base
that does not build stops it with the end of its build log. Watts not readable, the window's
frames not counted, the made-up sound not heard by a build: said in the output.

**What it takes**, measured on the M5 MacBook Air (another agent at work on it): the first run
against a new base, from a checkout whose target folder was cloned from another checkout, 183 s
(the base's checkout and the seeding 4 s, both builds at once 68 s, mostly the UI crate, which
runs its build script again once in each new target folder; the libraries 15 s; measuring 96 s).
A later run with the same base, only the change rebuilt: 117 to 124 s for the default two scenes
and two rounds (builds 6 to 10 s, the libraries 17 s, measuring 95 s); every scene, 243 to 265 s
(measuring 224 s). A pass is about 24 s for two scenes, 56 s for all five; watts add a pass a
build (11 s a scene), `--confirm` about six minutes a scene.

The JSON of the whole run (every pass's lines, every comparison, the notes and times) is written
to `<target>/xtask-perf/last.json`, and with `--json FILE` there too. Each pass's app log,
snapshots and the build logs are in `<temp>/mp-perf-runs` until the next run.

On Windows and Linux it builds and runs the same and compares the app's own figures; the window
server (or compositor), the GPU, watts and the screen's lock are not read there, said in the
output. Busy threads in place of `yes` on Windows. Not tried there yet.
