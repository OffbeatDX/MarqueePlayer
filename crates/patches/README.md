# Patched dependencies

Copies of dependencies with a fix the app needs before upstream has it, wired in by a
`[patch]` table in the workspace `Cargo.toml`: `[patch.crates-io]` for a crates.io dependency,
`[patch."https://github.com/slint-ui/slint"]` for one of Slint's crates (which come from git).
None is a workspace member (`exclude`). Each keeps its own licence files. Remove a copy (and
its `[patch]` line) once upstream has the fix.

## blocking 1.7.0

`blocking` is the thread pool `zbus` uses (on Linux: the desktop's media controls, the tray,
the accessibility bridge). In 1.7.0 an idle pool thread retires only when the pool also has
more threads than its limit (`&&`), so the last one never does: it wakes every 500 ms for the
life of the process, also while the app is paused and hidden (measured: 120 wakeups a minute,
all from `blocking-1`). The patch makes either condition retire it (`||`: too many threads, or
500 ms without work), as the code's own comments describe; the pool starts a thread again
when work comes. One line, in `src/lib.rs` (`main_loop`), marked "Marquee Player's patch".
Upstream: https://github.com/smol-rs/blocking (the same code on master as of 2026-11).
The Windows build does not depend on this crate.

## i-slint-renderer-skia 1.20.0 (Slint 84e288a)

Slint's Skia renderer, `internal/renderers/skia` of the Slint commit the app pins
(84e288a16657bfd8b2b33b3ae2d1067f4fee8cda): its sources, `build.rs`, README and licence files
(`LICENSES/`, which upstream links to Slint's root `LICENSES/`: real copies here), without its
`tests/` and dev-dependencies, which need the rest of Slint's tree. Its `Cargo.toml` is
upstream's with what Slint's workspace gave it written out: each `workspace = true` field and
dependency is the literal of Slint's root `Cargo.toml`, and Slint's own crates (`i-slint-core`,
`i-slint-core-macros`, `i-slint-common`, `vtable`, `const-field-offset`) come from the same git
commit as the app's `slint`, so Cargo resolves exactly what it did without the copy (checked
2026-11-26: `cargo tree` for macOS, Windows and Linux and `Cargo.lock` differ only in the
renderer's own source).

The change: Skia's GPU context, which Slint creates with Skia's defaults, keeps up to 256 MB of
textures and buffers it might reuse and gives none of it back while under that limit. The
patch releases what no frame has used lately (`perform_deferred_cleanup`), at most four times a
second, after each frame and while the window is hidden (no frame is drawn then): what has a
key of its own (a cover's texture, a glyph atlas) after 10 s without use, and the scratch
resources (a layer, a path's mask made for one frame and never asked for again) after half a
second. Lowering the limit instead would make what every frame needs anew churn (tried: at
64 MB the footprint fell but Halo cost 6 points of a core more). Measured on 2026-11-26 on an
M5 MacBook Air (2940x1844 window). Scrolling the Albums grid of a 1,355-album library: the
cache held 195 to 202 MB, 173 MB of it purgeable (nothing was using it), and the process's
footprint after 24 s of scrolling was 643 MB; with the patch 520 MB, at the same 60 frames a
second (frame intervals p95 18.6 to 19.0 ms both ways). With the visualizer behind the page
(`MP_BENCH=viz-cost`, memory at each case's end), where an anti-aliased line is a new mask
texture every frame: the line 343 MB where it was 514 MB with the 10 s rule alone, Bonfire
356 MB where 596 MB, Halo 483 MB where 570 MB, the process's CPU within the runs' two points.
On Apple silicon GPU memory counts in the process's footprint. All in `wgpu_30_surface.rs`
(`WGPUSurface`: the `last_cache_trim` field and its two initialisers, `trim_gpu_cache`, and its
calls in `render`), each place marked "Marquee Player's patch" and behind
`cfg(target_vendor = "apple")`: on Windows and Linux the copy compiles to upstream's code.

To make the copy again (for another Slint commit or release): copy `internal/renderers/skia`
of that commit from Cargo's checkout (`~/.cargo/git/checkouts/slint-*/<commit>`) without
`tests/`, the files of `LICENSES/` (followed links), write out the `workspace = true` entries
of its `Cargo.toml` from that commit's root `Cargo.toml` as above, with the new commit (or, for
a release, the version from crates.io and `[patch.crates-io]`), leave out `[dev-dependencies]`,
and apply the change again. Check that `cargo tree -p i-slint-renderer-skia` (with
`--target` for each system, and `-e features`) and `Cargo.lock` change only in the renderer's
source. The copy's version must be the one the rest of Slint has, or the `[patch]` does not
apply and Cargo warns "patch was not used".

Remove it when Slint trims the cache itself or lets the app reach the Skia context (to call
`perform_deferred_cleanup` or set a smaller cache from the app).
