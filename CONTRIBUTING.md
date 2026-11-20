# Contributing to Marquee Player

This repository holds two apps: the desktop player (Rust and Slint, in `crates/`) and the
Android client (Kotlin, in `android/`, a Gradle project of its own with its own
[README](android/README.md) and [design notes](android/docs/DESIGN.md)). This file is about the
desktop app; for Android, follow `android/README.md`, and keep a change to one app out of the
other's files.

## Building

The prerequisites and every development command are in [docs/building.md](docs/building.md) (and
[docs/linux.md](docs/linux.md#building) for the Linux packages, [docs/macos.md](docs/macos.md#building)
for macOS). In short:

```sh
cargo run -p marquee-player --release   # run from source
cargo build --profile fast -p marquee-player   # quicker rebuilds while working
cargo xtask --help                      # build, check, bench, release and their options
```

`rust-toolchain.toml` pins the toolchain (rustup picks it up). The first build downloads Skia's
prebuilt binaries; tests that generate audio need `ffmpeg` on PATH and skip without it.

## Before proposing a change

Run the checks a push must pass:

```sh
cargo xtask check
```

It runs, on the `fast` profile: `git diff --check` over the commits since the branch left
`master` (`--base REV` for another start), `cargo fmt --check` (reported, not enforced: see
below), clippy over the workspace with `-D warnings` without and with the bench harness, and
`cargo test --workspace`. It stops at the first failure.

`cargo xtask check --install-hook` installs it as a git pre-push hook in your clone (opt-in;
`git push --no-verify` skips it once, deleting the hook uninstalls it).

Nothing runs on GitHub for a push or a pull request, so run them before you propose a change.
[.github/workflows/desktop.yml](.github/workflows/desktop.yml) runs the same
`cargo xtask check` on Windows, Ubuntu 24.04 and macOS when a version is tagged, and when a
maintainer starts it by hand (Actions > Desktop > Run workflow). If your change touches
macOS code (`platform/macos/`, `platform/unix/`, Core Audio, FSEvents), say so in the pull
request so that the macOS check is run on it.
[.github/workflows/release.yml](.github/workflows/release.yml) builds the packages of all three
systems on a release tag, or the ones chosen by hand.
[.github/workflows/android.yml](.github/workflows/android.yml) runs the Android unit tests, the
API scan, Lint and the debug and release builds on a release tag too, or by hand. Details are in
[docs/building.md](docs/building.md#checks-before-a-push).

A change to behaviour you can see in the app is also worth a run of the benches that cover it
(below).

## Conventions the code follows

- **Do not reformat.** The tree has never been run through rustfmt: its lines are longer than any
  rustfmt setting keeps, so `cargo fmt` would change a great many lines. Keep the surrounding style and
  line length of the file you edit, and do not run `cargo fmt` on files you touch.
  `.editorconfig` states the rest: UTF-8, LF line endings (CRLF for `.bat`), a final newline,
  4-space indents in Rust and Slint, 2 in YAML.
- **The operating system goes through the platform seam.** Everything the app asks of the system
  is one interface in `crates/app/src/platform/mod.rs`, implemented in `platform/windows/`,
  `platform/linux/` and `platform/macos/` (with what Linux and macOS share in `platform/unix/`);
  nothing outside `platform/` names a system. Sound devices are behind
  `player_core::output`, folder watching behind `library::dirwatch`, HTTP behind `crates/http`.
  See [docs/architecture.md](docs/architecture.md), [docs/linux.md](docs/linux.md) and
  [docs/macos.md](docs/macos.md).
- **A new behaviour comes with a test or a bench check.** Logic is unit-tested where it lives
  (the docs name the tests for each area); what only shows in the running app is checked by a
  bench mode (`crates/app/src/bench/`), which counts its failed checks and fails the run.
  Failure handling is tested the same way ([docs/troubleshooting.md](docs/troubleshooting.md):
  each case is a test, a fuzz test or a bench runner scenario).
- **The docs describe the behaviour.** [docs/](docs/README.md) is the specification of what the
  app does. A change in behaviour updates the section that describes it, and comments point to
  it as `docs/<file>.md, "<Heading>"`.
- **Comments say what the code does**, not when or for whom it was written.
- **Commit messages**: an imperative subject that starts with the area
  (`Library: drop remove_from_playlist and move_in_playlist by stored position`,
  `Zoom: the window's minimum is given again only when it changes`), a blank line, and a short
  body saying what changed and why. See `git log` for the style.

## Checking UI behaviour with the bench harness

The scripted benches drive the real app: they click, type and press keys through the window's
own dispatch, take snapshots, and check what the pages, the settings files and the library say.
The harness is the cargo feature `bench`, never in a release build.

```sh
cargo xtask bench -- --list                     # the modes
cargo xtask bench -- --only playlist,metadata   # some of them
cargo xtask bench --profile fast -- --quick     # while working: leaves out the long modes
cargo xtask bench                               # everything (about half an hour)
```

The runner makes its fixtures in the system's temporary folder, gives each mode a fresh data
folder and local stand-ins for every online service, and prints one pass/fail table. Bench runs
play at volume 0 and open the window on another monitor without taking the focus
(`MP_WINDOW=secondary`), and a separate data folder means they never touch a running copy of the
app. On Linux, run them in a desktop session or in `packaging/linux/headless-session.sh`. The
modes, the fixtures and every environment variable are in [docs/testing.md](docs/testing.md).

## Rules that protect users

- **Music folders are read-only** unless the user turns on tag editing (Settings > Library >
  Edit tags). The scanner and View metadata never write to music files; the tag editor writes
  only through its checked save ([docs/features/metadata.md](docs/features/metadata.md#tag-editing)).
  Tests and bench builds may write only inside a folder marked `.mp-edit-fixtures` in the
  temporary folder (`library::edit::atomic::set_guard` refuses every other path).
- **No network request without a service the user set up.** The Navidrome server, Last.fm,
  ListenBrainz and LRCLIB (off by default) are reached only once the user has connected or turned
  them on; Discord is a local pipe. The API overrides (`MP_LASTFM_API`, `MP_LISTENBRAINZ_API`,
  `MP_LRCLIB_API`) work only in bench and test builds, and tests use stand-ins on 127.0.0.1.
- **What goes back to a server is opt-in per kind.** The Navidrome client is read-only for
  syncing, covers and streaming; favourites, ratings and plays are written only behind their own
  switches (`subsonic::Writes`; [docs/features/navidrome.md](docs/features/navidrome.md)).
- **Secrets stay out of logs and reports.** Passwords are never kept, tokens and session keys are
  sealed (DPAPI on Windows, mode 0600 on Linux and macOS), and neither they nor the server's address or
  user name go into the log or Copy diagnostics.

## Licence

Marquee Player is under the GNU General Public License, version 3 or later ([LICENSE](LICENSE),
the [README](README.md#licence)). By proposing a change you agree that it is published under
that licence. A file taken from elsewhere keeps its own licence: say where it comes from and
under which terms, and keep its licence file beside it (as `crates/patches/blocking` and the
AutoEq fixtures do); the terms must be compatible with GPL-3.0.
