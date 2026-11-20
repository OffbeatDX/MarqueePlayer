//! `cargo xtask release`: a numbered release for the system it runs on (docs/releases.md,
//! "Version and releases"): on Windows the portable zip and the installer, on Linux the AppImage,
//! on macOS the disk image, in `dist/`.
//!
//! The steps the systems share are here: the up-front refusals, the tests and clippy (both
//! ways, the release profile), the remapped `dist` build that `crates/app/build.rs` numbers, the
//! version check and the leaked-path check. What each system packages is in `windows.rs`,
//! `linux.rs` and `macos.rs`; all are compiled (and linted, and tested) on every system, and
//! the one for this system runs. Nothing is packaged when a step fails, and the exit code is then non-zero.

mod linux;
mod macos;
mod notices;
mod paths;
mod remap;
mod version;
mod windows;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::args::{COMMON_HELP, Common, Parser};
use crate::cmd;
use crate::report::{self, Fail};
use crate::workspace::Workspace;

fn help() -> String {
    format!(
        "\
cargo xtask release [--skip-checks] [options]

Makes a numbered release (YYYY.M.D.count, crates/app/build.rs) for the system it runs on, in
dist/ in the checkout:
  Windows  MarqueePlayer-<version>-portable.zip and MarqueePlayer-<version>-setup.exe, the
           exe copied beside Cargo.toml, its PDB in dist/symbols/<version>/
  Linux    MarqueePlayer-<version>-x86_64.AppImage, its debug symbols in
           dist/symbols/<version>/linux/
  macOS    MarqueePlayer-<version>-macos-<arch>.dmg (arm64 or x86_64, this Mac's), holding
           MarqueePlayer.app signed ad hoc; the binary and its .dSYM in
           dist/symbols/<version>/macos/
Runs the tests and clippy (release profile, without and with the bench harness), builds the
`dist` profile with this machine's paths remapped, checks the version and that no local path
or user name is in the binary, then packages and checks the packages. Any failure stops it
before anything is packaged, with a non-zero exit code. No prompts: it runs unattended.

Options:
  --skip-checks      no tests and no clippy (for trying the packaging, and for CI, which runs
                     them as a job of their own); the output says so
{COMMON_HELP}
                     (Linux: when neither is set, ~/.cache/mp-release-target, on the Linux file
                     system, as building under /mnt/c is slow)

Environment:
  MP_INNO_SETUP       Windows: Inno Setup 6's folder (or its ISCC.exe), when it is not
                      installed where its installer puts it
  MP_VC_REDIST        Windows: the folder of the Visual C++ runtime DLLs to ship, instead of
                      the newest Visual Studio's Redist folder
  MP_PIPEWIRE_PREFIX  Linux: the PipeWire to build against (default
                      ~/.local/share/mp-build/pipewire-0.3.65, packaging/linux/pipewire-sdk.sh)
  MP_APPIMAGE_TOOLS   Linux: where appimagetool and the AppImage runtime are downloaded
                      (default ~/.local/share/mp-build/appimage-tools)
  MP_CODESIGN_IDENTITY  macOS: a Developer ID Application identity in the keychain to sign
                      with (hardened runtime, timestamp) instead of the ad-hoc signature;
                      notarising is not done (docs/macos.md, \"The package\")
  MP_RELEASE_VERSION  the release number to build (YYYY.M.D.count), instead of the one
                      crates/app/build.rs would make from the date and its state files (CI
                      gives each system's build the number it made once for the release)"
    )
}

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut p = Parser::new("release", args);
    let mut common = Common::default();
    let mut skip_checks = false;
    while let Some(a) = p.next()? {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return Ok(());
            }
            "--skip-checks" => skip_checks = true,
            _ if common.take(&a, &mut p)? => {}
            _ => return Err(p.unknown(&a)),
        }
    }
    let result = version::given(std::env::var(version::GIVEN_VAR).ok().as_deref()).map_err(Fail::msg).and_then(|given| {
        if let Some(v) = given {
            println!("release number {v}, from {}", version::GIVEN_VAR);
        }
        Workspace::find(&common)
    });
    let result = result.and_then(|ws| {
        let target = match (ws.chosen, home()) {
            (false, Some(home)) if cfg!(target_os = "linux") => home.join(".cache").join("mp-release-target"),
            _ => ws.target.clone(),
        };
        let mut release = Release { ws, target, skip_checks, start: Instant::now(), env: Vec::new() };
        if cfg!(windows) {
            windows::run(&mut release)
        } else if cfg!(target_os = "linux") {
            linux::run(&mut release)
        } else if cfg!(target_os = "macos") {
            macos::run(&mut release)
        } else {
            Err(Fail::msg("releases are made on Windows, Linux and macOS only"))
        }
    });
    // The reason first, then the plain verdict, both where a CI log shows them.
    result.map_err(|mut fail| {
        if let Some(m) = fail.message.take() {
            eprintln!();
            eprintln!("error: {m}");
        }
        eprintln!("error: no release was made.");
        fail
    })
}

/// One release run: where it builds and what every cargo it starts gets in its environment.
pub struct Release {
    ws: Workspace,
    /// Where cargo builds (the tests, clippy and the `dist` build alike).
    target: PathBuf,
    skip_checks: bool,
    start: Instant,
    /// Set (`Some`) or removed (`None`) for every program this run starts.
    env: Vec<(String, Option<OsString>)>,
}

impl Release {
    fn root(&self) -> &Path {
        &self.ws.root
    }

    fn dist(&self) -> PathBuf {
        self.root().join("dist")
    }

    /// The `dist` profile's folder in the target folder.
    fn profile_dir(&self) -> PathBuf {
        self.target.join("dist")
    }

    fn set_env(&mut self, key: &str, value: impl Into<OsString>) {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.to_owned(), Some(value.into())));
    }

    /// `program` with this run's environment.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        self.apply_env(&mut c);
        c
    }

    fn apply_env(&self, c: &mut Command) {
        for (k, v) in &self.env {
            match v {
                Some(v) => c.env(k, v),
                None => c.env_remove(k),
            };
        }
    }

    /// cargo in the checkout and the target folder, at below-normal priority, with this run's
    /// environment and never numbering a release (`MP_RELEASE_BUILD` is only for [`build`]).
    fn cargo(&self, args: &[&str]) -> Command {
        let mut c = self.ws.cargo(&self.target, args);
        self.apply_env(&mut c);
        c.env_remove("MP_RELEASE_BUILD");
        cmd::below_normal(c)
    }

    /// A step that runs one program: `failed` is the error when it exits non-zero (with its
    /// exit code).
    fn step(&self, what: &str, failed: &str, run: impl FnOnce() -> Result<i32, Fail>) -> Result<(), Fail> {
        report::step(what, run).map_err(|f| Fail { code: f.code, message: f.message.or_else(|| Some(failed.to_owned())) })
    }

    /// The tests and clippy, on the release profile, without and with the bench harness, so
    /// that code that compiles only one way is caught: nothing is built or packaged from a tree
    /// that fails them. Before the remapped build: these share the release profile's folder
    /// with `cargo xtask build`, and the remap flags would rebuild everything there.
    fn checks(&self) -> Result<(), Fail> {
        if self.skip_checks {
            report::heading("tests and clippy");
            println!("SKIPPED: --skip-checks: the tests and clippy were not run for this release.");
            return Ok(());
        }
        const TESTS: &str = "Tests failed - see above. Nothing was built or packaged.";
        const CLIPPY: &str = "Clippy failed - see above. Nothing was built or packaged.";
        let bench = ["--features", "marquee-player/bench"];
        let test = ["test", "--workspace", "--release", "--locked"];
        let clippy_args = ["clippy", "--workspace", "--all-targets", "--locked"];
        let lints = ["--", "-D", "warnings"];
        self.step("tests (release profile)", TESTS, || cmd::status(&mut self.cargo(&test)))?;
        self.step("tests (release profile, with the bench harness)", TESTS, || cmd::status(&mut self.cargo(&[&test[..], &bench].concat())))?;
        self.step("clippy", CLIPPY, || cmd::status(&mut self.cargo(&[&clippy_args[..], &lints].concat())))?;
        self.step("clippy (with the bench harness)", CLIPPY, || cmd::status(&mut self.cargo(&[&clippy_args[..], &bench, &lints].concat())))?;
        Ok(())
    }

    /// The remapped `dist` build that `build.rs` numbers (`MP_RELEASE_BUILD=1`), and the
    /// release number it gave, checked against what the binary itself says. `rustflags` are
    /// the system's own besides the remapping (Linux: the RUNPATH); `examples` are built
    /// too.
    fn build(&self, extra_rustflags: &[&str], examples: &[&str]) -> Result<String, Fail> {
        let home = home().ok_or_else(|| Fail::msg("no home folder (USERPROFILE / HOME is not set)"))?;
        let windows = cfg!(windows);
        let prefixes = remap::prefixes(&home, std::env::var_os("RUSTUP_HOME").map(PathBuf::from), std::env::var_os("CARGO_HOME").map(PathBuf::from), self.root());
        let mut rustflags = remap::inherited_rustflags(std::env::var("CARGO_ENCODED_RUSTFLAGS").ok(), std::env::var("RUSTFLAGS").ok());
        rustflags.extend(remap::rustflags(&prefixes));
        rustflags.extend(extra_rustflags.iter().map(|s| (*s).to_owned()));
        let c_flags = remap::c_flags(windows, &prefixes);
        let config = self.target.join("release-remap.toml");
        let text = remap::config_toml(&rustflags, c_flags.as_deref(), std::env::var("CFLAGS").ok().as_deref(), std::env::var("CXXFLAGS").ok().as_deref());
        write(&config, text.as_bytes())?;
        println!("path remapping written to {}", config.display());

        // libopus's CMake build keeps the C flags of its first build: when they changed, it is
        // built afresh (seconds), so it takes these.
        let stamp = self.profile_dir().join(".release-c-flags");
        if std::fs::read_to_string(&stamp).ok().as_deref() != Some(text.as_str()) {
            self.step("opusic-sys afresh (the C flags changed)", "cargo clean failed - see above. Nothing was packaged.", || {
                cmd::status(&mut self.cargo(&["clean", "-p", "opusic-sys", "--profile", "dist", "--locked"]))
            })?;
        }
        let config_arg = config.to_string_lossy().into_owned();
        // Examples the packaging runs (Windows: the installer's icon), in cargo runs of their own
        // (built with the app, their dev-dependencies' features would be unified into it), and
        // before the app: each such build may run build.rs again, which numbers a release when
        // a source changed meanwhile, so the app's build must be the last to number it.
        for example in examples {
            self.step(&format!("the {example} example (dist profile)"), "Building an example the packaging needs failed - see above. Nothing was packaged.", || {
                cmd::status(&mut self.release_cargo(&["build", "-p", "marquee-player", "--profile", "dist", "--locked", "--config", &config_arg, "--example", example]))
            })?;
        }
        self.step(
            "release build (dist profile, paths remapped; everything is compiled the first time: minutes)",
            "The release build failed - see above. Nothing was packaged.",
            || cmd::status(&mut self.release_cargo(&["build", "-p", "marquee-player", "--profile", "dist", "--locked", "--config", &config_arg])),
        )?;
        write(&stamp, text.as_bytes())?;

        // The number build.rs gave, from its state file, and the binary's own word for it.
        let state = self.root().join("crates").join("app").join(".app-version");
        let saved = std::fs::read_to_string(&state).map_err(|e| Fail::msg(format!("could not read {}: {e}", state.display())))?;
        let ver = version::release(&saved).map_err(|e| Fail::msg(format!("{}: {e}", state.display())))?;
        let built = self.built();
        let out = cmd::output(self.command(&built).arg("--version"))?;
        let says = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !out.status.success() {
            return Err(Fail::msg(format!("{} --version failed: {}{}", built.display(), says, String::from_utf8_lossy(&out.stderr).trim())));
        }
        if says != version::says(&ver) {
            return Err(Fail::msg(format!("{} says \"{says}\", but {} says {ver}", built.display(), state.display())));
        }
        println!("ok: built Marquee Player {ver}");
        Ok(ver)
    }

    /// Refuses a release whose number changed after it was built (a source edited while it
    /// was being made, and something rebuilt meanwhile): the packages would not match the
    /// numbering state.
    fn recheck(&self, ver: &str) -> Result<(), Fail> {
        let state = self.root().join("crates").join("app").join(".app-version");
        let now = std::fs::read_to_string(&state).unwrap_or_default();
        if now.trim() != ver {
            return Err(Fail::msg(format!("{} says {} now, not {ver}: a source changed during the release. Run it again.", state.display(), now.trim())));
        }
        Ok(())
    }

    /// cargo for the numbered build: `MP_RELEASE_BUILD=1`, and no RUSTFLAGS variables (they
    /// would replace the config file's flags entirely; [`remap::inherited_rustflags`] carried
    /// them into it).
    fn release_cargo(&self, args: &[&str]) -> Command {
        let mut c = self.cargo(args);
        c.env("MP_RELEASE_BUILD", "1").env_remove("RUSTFLAGS").env_remove("CARGO_ENCODED_RUSTFLAGS");
        c
    }

    /// The binary the `dist` build makes.
    fn built(&self) -> PathBuf {
        self.profile_dir().join(Workspace::exe("MarqueePlayer"))
    }

    /// Refuses a binary that carries this machine's home folder, the checkout's path or the
    /// user name.
    fn check_paths(&self, file: &Path) -> Result<(), Fail> {
        report::heading("this machine's paths in what ships");
        let bytes = std::fs::read(file).map_err(|e| Fail::msg(format!("could not read {} for the path check: {e}", file.display())))?;
        let home = home().unwrap_or_default();
        let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).ok().or_else(linux::user_name);
        let needles = paths::needles(cfg!(windows), &home, self.root(), user.as_deref());
        let finds = paths::scan(&bytes, &needles);
        for f in &finds {
            println!("found \"{}\" ({}, {} times), first at byte {}: {}", f.needle, f.encoding, f.count, f.first, f.context);
        }
        if !finds.is_empty() {
            return Err(Fail::msg(format!(
                "{} contains this machine's home folder, the checkout's path or the user name (above). Nothing was packaged.",
                file.display()
            )));
        }
        println!("ok: no home folder, checkout path or user name in {} (UTF-8 and UTF-16, case ignored)", file.display());
        Ok(())
    }

    /// The last lines: what was made, where, how big, and how long it took.
    fn summary(&self, ver: &str, rows: &[(&str, PathBuf)], notes: &[String]) {
        println!();
        println!("  Version    {ver}");
        for (what, path) in rows {
            match std::fs::metadata(path) {
                Ok(m) if m.is_file() => println!("  {what:<10} {}  ({} MB, {} bytes)", path.display(), m.len() / 1_048_576, m.len()),
                _ => println!("  {what:<10} {}", path.display()),
            }
        }
        for n in notes {
            println!("  {n}");
        }
        let s = self.start.elapsed().as_secs();
        println!("  Time       {}m {:02}s", s / 60, s % 60);
        println!();
        if self.skip_checks {
            println!("NOTE: the tests and clippy were skipped (--skip-checks): hand this release out only if they passed for this commit.");
        }
        println!("ok: release {ver} is ready.");
    }
}

/// Writes the notices (`notices.rs`) to `out`, with Windows line endings for Notepad when
/// `crlf`; refuses incomplete ones.
fn write_notices(r: &Release, triple: &str, ver: &str, extras: &[notices::Extra], out: &Path, crlf: bool) -> Result<(), Fail> {
    report::heading("third-party notices");
    let n = notices::generate(r.root(), triple, ver, &r.profile_dir(), extras)?;
    let text = if crlf { n.text.replace('\n', "\r\n") } else { n.text };
    write(out, text.as_bytes())?;
    println!("third-party notices: {} packages written to {}", n.packages, out.display());
    for b in &n.bare {
        println!("warning: no licence text found for {b}");
    }
    for m in &n.missing {
        println!("not found: {m}");
    }
    if !n.missing.is_empty() {
        return Err(Fail::msg("The third-party notices are incomplete (above). Nothing was packaged."));
    }
    Ok(())
}

/// The user's home folder: USERPROFILE on Windows, HOME elsewhere.
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Fail> {
    if let Some(dir) = path.parent() {
        create_dir(dir)?;
    }
    std::fs::write(path, bytes).map_err(|e| Fail::msg(format!("could not write {}: {e}", path.display())))
}

fn copy(from: &Path, to: &Path) -> Result<(), Fail> {
    std::fs::copy(from, to).map(drop).map_err(|e| Fail::msg(format!("could not copy {} to {}: {e}", from.display(), to.display())))
}

fn create_dir(dir: &Path) -> Result<(), Fail> {
    std::fs::create_dir_all(dir).map_err(|e| Fail::msg(format!("could not create {}: {e}", dir.display())))
}

/// Removes a folder and everything in it, if it is there.
fn remove_dir(dir: &Path) -> Result<(), Fail> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Fail::msg(format!("could not remove {}: {e}", dir.display()))),
        _ => Ok(()),
    }
}

fn remove_file(file: &Path) -> Result<(), Fail> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Fail::msg(format!("could not remove {}: {e}", file.display()))),
        _ => Ok(()),
    }
}

/// The program `name` on PATH, as a shell would find it.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(Workspace::exe(name))).find(|p| p.is_file())
}
