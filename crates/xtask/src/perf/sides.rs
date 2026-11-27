//! The two builds `cargo xtask perf` compares: the working tree as it is, and the base in a
//! checkout of its own (a detached `git worktree` under the target folder, kept between
//! runs), each in its own target folder seeded from one already built, both on the `fast`
//! profile with the bench feature, built at the same time.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::cmd;
use crate::report::Fail;
use crate::workspace::Workspace;

pub const PROFILE: &str = "fast";

pub struct Side {
    pub name: &'static str,
    /// The checkout built.
    pub src: PathBuf,
    /// Its own target folder.
    pub target: PathBuf,
    /// What was done to make it buildable or comparable, for the report.
    pub notes: Vec<String>,
}

impl Side {
    pub fn exe(&self) -> PathBuf {
        self.target.join(PROFILE).join(Workspace::exe("MarqueePlayer"))
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Fail> {
    let out = cmd::output(Command::new("git").current_dir(dir).args(args))?;
    if !out.status.success() {
        return Err(Fail::msg(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The commit the base is: `--base` (any ref), else where this branch left master.
pub fn resolve_base(root: &Path, given: Option<&str>) -> Result<(String, String), Fail> {
    let name = match given {
        Some(r) => r.to_owned(),
        None => {
            let main = ["master", "origin/master", "main", "origin/main"]
                .into_iter()
                .find(|b| git(root, &["rev-parse", "--verify", "--quiet", &format!("{b}^{{commit}}")]).is_ok())
                .ok_or_else(|| Fail::msg("no master branch to take the merge base with: give --base <ref>"))?;
            git(root, &["merge-base", "HEAD", main])?
        }
    };
    let sha = git(root, &["rev-parse", "--verify", &format!("{name}^{{commit}}")]).map_err(|_| Fail::usage(format!("--base: `{name}` is not a commit here")))?;
    let label = if given.is_some() { name } else { format!("merge base with master ({})", &sha[..sha.len().min(10)]) };
    Ok((sha, label))
}

/// The base's checkout, at `sha`: made the first time (`git worktree add --detach`), moved
/// after (only when it is elsewhere, so an unchanged base costs no rebuild); then the perf
/// bench grafted in where the base has none.
pub fn prepare_base(root: &Path, dir: &Path, sha: &str) -> Result<Vec<String>, Fail> {
    let mut notes = Vec::new();
    if !dir.join(".git").exists() {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| Fail::msg(format!("could not clear {}: {e}", dir.display())))?;
        }
        let _ = git(root, &["worktree", "prune"]);
        git(root, &["worktree", "add", "--detach", "--force", &dir.to_string_lossy(), sha])?;
        notes.push(format!("checked out the base in {}", dir.display()));
    } else {
        let head = git(dir, &["rev-parse", "HEAD"])?;
        let dirty = git(dir, &["status", "--porcelain", "--untracked-files=all"])?;
        // The graft's files, and the lock file its feature makes cargo write.
        let graft_only = dirty.lines().all(|l| GRAFTED.iter().chain(&["Cargo.lock"]).any(|g| l.ends_with(g)));
        if head != sha || !graft_only {
            git(dir, &["checkout", "--force", "--detach", sha])?;
            git(dir, &["clean", "-fdq"])?;
        }
    }
    notes.extend(graft(root, dir)?);
    Ok(notes)
}

/// The files a graft writes into the base's checkout.
const GRAFTED: [&str; 3] = ["crates/app/src/bench/perf.rs", "crates/app/src/bench/mod.rs", "crates/app/Cargo.toml"];

/// A base from before `MP_BENCH=perf` gets this checkout's `bench/perf.rs`, registered in
/// `bench/mod.rs`, and the bench feature's `slint/unstable-wgpu-30` (the window's frames
/// counted): bench-only code, so what is compared is the app as the base had it. Files are
/// written only when they differ, so a second run rebuilds nothing.
fn graft(root: &Path, dir: &Path) -> Result<Vec<String>, Fail> {
    let mut notes = Vec::new();
    let perf = dir.join(GRAFTED[0]);
    let ours = std::fs::read_to_string(root.join(GRAFTED[0])).map_err(|e| Fail::msg(format!("this checkout has no {}: {e}", GRAFTED[0])))?;
    let base_mod = std::fs::read_to_string(dir.join(GRAFTED[1])).map_err(|e| Fail::msg(format!("the base has no {}: {e}", GRAFTED[1])))?;
    let native = git(dir, &["ls-files", "--error-unmatch", GRAFTED[0]]).is_ok();
    if native {
        return Ok(notes);
    }
    let patched = graft_mod(&base_mod).ok_or_else(|| Fail::msg("the base is too old for the perf bench to be grafted in (its bench/mod.rs has no place for it): give a newer --base"))?;
    write_if_changed(&perf, &ours)?;
    write_if_changed(&dir.join(GRAFTED[1]), &patched)?;
    notes.push("the base has no perf bench: this checkout's bench/perf.rs was grafted in (bench-only code)".into());
    let toml_path = dir.join(GRAFTED[2]);
    let toml = std::fs::read_to_string(&toml_path).unwrap_or_default();
    match graft_feature(&toml) {
        Some(t) => {
            write_if_changed(&toml_path, &t)?;
            notes.push("and the bench feature's slint/unstable-wgpu-30, so its window's frames are counted".into());
        }
        None if toml.contains("slint/unstable-wgpu-30") => {}
        None => notes.push("the base's bench feature could not take slint/unstable-wgpu-30: its window's frames are not counted".into()),
    }
    Ok(notes)
}

/// `bench/mod.rs` with `mod perf;` and its `MP_BENCH=perf` arm added; None when the places
/// they go are not there.
pub fn graft_mod(text: &str) -> Option<String> {
    if text.contains("mod perf;") {
        return Some(text.to_owned());
    }
    let at = text.find("mod player_bar;")?;
    let mut s = format!("{}pub(crate) mod perf; // perf: grafted by cargo xtask perf\n{}", &text[..at], &text[at..]);
    let arm = "            _ => tour(),\n";
    let at = s.find(arm)?;
    s.insert_str(at, "            \"perf\" => crate::bench::perf::run(frames.clone()),\n");
    Some(s)
}

/// The app's Cargo.toml with `slint/unstable-wgpu-30` in its bench feature; None when the
/// feature is not written as this tool knows it, or has it already.
pub fn graft_feature(toml: &str) -> Option<String> {
    let line = toml.lines().find(|l| l.trim_start().starts_with("bench = ["))?;
    if line.contains("unstable-wgpu-30") {
        return None;
    }
    let close = line.rfind(']')?;
    let new = format!("{}, \"slint/unstable-wgpu-30\"]{}", &line[..close], &line[close + 1..]);
    Some(toml.replacen(line, &new, 1))
}

fn write_if_changed(path: &Path, text: &str) -> Result<(), Fail> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| Fail::msg(format!("could not write {}: {e}", path.display())))
}

/// A target folder for `side`, seeded the first time from `from` (a target folder whose
/// `fast` profile is built: copy-on-write on APFS, so instant and taking no space); then the
/// build-script outputs of the workspace's own crates that name another checkout's files are
/// removed, so they run again here. (slint-build records the absolute paths of the `.slint`
/// files it read: kept, cargo would see those files unchanged and keep the other
/// checkout's UI.)
pub fn seed_target(target: &Path, from: &[PathBuf], src: &Path, members: &[String]) -> Result<Vec<String>, Fail> {
    let mut notes = Vec::new();
    let profile = target.join(PROFILE);
    if !profile.exists() {
        match from.iter().find(|f| f.join(PROFILE).join(".fingerprint").is_dir()) {
            Some(f) => {
                std::fs::create_dir_all(target).map_err(|e| Fail::msg(format!("could not make {}: {e}", target.display())))?;
                let t = Instant::now();
                clone_dir(&f.join(PROFILE), &profile)?;
                notes.push(format!("seeded {} from {} in {:.1} s", target.display(), f.join(PROFILE).display(), t.elapsed().as_secs_f64()));
            }
            None => notes.push(format!("no built target to seed {} from: the first build is a whole one", target.display())),
        }
    }
    let purged = purge_foreign_build_outputs(&profile, src, members);
    if purged > 0 {
        notes.push(format!("{purged} build-script outputs of the workspace's crates named another checkout's files: removed, to run again here"));
    }
    Ok(notes)
}

/// A copy of a folder: `cp -c -R` on macOS (APFS clones), `cp -R` elsewhere on Unix,
/// `robocopy`-free plain recursion on Windows.
fn clone_dir(from: &Path, to: &Path) -> Result<(), Fail> {
    let ok = if cfg!(target_os = "macos") {
        Command::new("cp").args(["-c", "-R"]).arg(from).arg(to).status().is_ok_and(|s| s.success())
    } else if cfg!(unix) {
        Command::new("cp").args(["-R"]).arg(from).arg(to).status().is_ok_and(|s| s.success())
    } else {
        copy_tree(from, to).is_ok()
    };
    if ok { Ok(()) } else { Err(Fail::msg(format!("could not copy {} to {}", from.display(), to.display()))) }
}

pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Removes `build/<member>-<hash>` and `.fingerprint/<member>-<hash>` where the build
/// script's output names a file by an absolute path outside `src`. The count removed.
fn purge_foreign_build_outputs(profile: &Path, src: &Path, members: &[String]) -> usize {
    let Ok(dirs) = std::fs::read_dir(profile.join("build")) else { return 0 };
    let mut n = 0;
    for d in dirs.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        let Some((crate_name, _hash)) = name.rsplit_once('-') else { continue };
        if !members.iter().any(|m| m == crate_name) {
            continue;
        }
        let Ok(out) = std::fs::read_to_string(d.path().join("output")) else { continue };
        if names_foreign_file(&out, src) {
            let _ = std::fs::remove_dir_all(d.path());
            let _ = std::fs::remove_dir_all(profile.join(".fingerprint").join(&name));
            n += 1;
        }
    }
    n
}

/// Whether a build script's output has a `rerun-if-changed` of an absolute path outside `src`.
pub fn names_foreign_file(output: &str, src: &Path) -> bool {
    output.lines().filter_map(|l| l.strip_prefix("cargo:rerun-if-changed=").or_else(|| l.strip_prefix("cargo::rerun-if-changed="))).any(|p| {
        let p = Path::new(p.trim());
        p.is_absolute() && !p.starts_with(src)
    })
}

/// The workspace's package names (the crates of a checkout), from `cargo metadata`, with
/// `-` as cargo writes them in the build folder's names (`marquee-ui`).
pub fn members(ws: &Workspace) -> Vec<String> {
    let out = cmd::output(Command::new(cmd::cargo_program()).current_dir(&ws.root).args(["metadata", "--no-deps", "--format-version", "1"]));
    let Ok(out) = out else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return Vec::new() };
    v["packages"].as_array().map(|p| p.iter().filter_map(|p| p["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// Builds the sides at the same time (each `cargo build -p marquee-player --profile fast
/// --features bench` in its own target folder, below normal priority, its output in a log);
/// the seconds each took, or the first failure with the end of its log.
pub fn build_all(ws: &Workspace, sides: &[&Side], logs: &Path) -> Result<Vec<f64>, Fail> {
    let mut running = Vec::new();
    for s in sides {
        let log = logs.join(format!("build-{}.log", s.name));
        let file = std::fs::File::create(&log).map_err(|e| Fail::msg(format!("could not write {}: {e}", log.display())))?;
        let mut c = ws.cargo(&s.target, &["build", "-p", "marquee-player", "--profile", PROFILE, "--features", "bench"]);
        c.current_dir(&s.src);
        // (`below_normal` makes a new command: its output is set after.)
        let mut c = cmd::below_normal(c);
        c.stdin(Stdio::null()).stdout(file.try_clone().map_err(|e| Fail::msg(e.to_string()))?).stderr(file);
        let child = c.spawn().map_err(|e| Fail::msg(format!("could not start cargo: {e}")))?;
        running.push((s, child, log, Instant::now()));
    }
    // Each one's time to its own end, whichever ends first.
    let mut ended: Vec<Option<(bool, f64)>> = vec![None; running.len()];
    while ended.iter().any(Option::is_none) {
        for (i, (_, child, _, start)) in running.iter_mut().enumerate() {
            if ended[i].is_none()
                && let Ok(Some(st)) = child.try_wait()
            {
                ended[i] = Some((st.success(), start.elapsed().as_secs_f64()));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let mut times = Vec::new();
    let mut failed = None;
    for ((s, _, log, _), end) in running.into_iter().zip(ended) {
        let (ok, secs) = end.unwrap_or((false, 0.0));
        times.push(secs);
        if !ok && failed.is_none() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(25)..].join("\n");
            failed = Some(Fail::msg(format!("the {} failed to build ({}):\n{tail}\n(the whole log: {})", s.name, s.src.display(), log.display())));
        }
    }
    match failed {
        Some(f) => Err(f),
        None => Ok(times),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graft_registers_the_mode_where_the_base_has_its_places() {
        let base = "mod output_devices; // x\nmod player_bar; // y\n\nfn f() {\n    match m {\n            \"viz-cost\" => a(),\n            _ => tour(),\n        }\n}\n";
        let g = graft_mod(base).unwrap();
        assert!(g.contains("mod output_devices; // x\npub(crate) mod perf; // perf: grafted by cargo xtask perf\nmod player_bar;"));
        assert!(g.contains("\"perf\" => crate::bench::perf::run(frames.clone()),\n            _ => tour(),"));
        // Grafted twice: the same.
        assert_eq!(graft_mod(&g).unwrap(), g);
        assert_eq!(graft_mod("fn nothing() {}"), None);
    }

    #[test]
    fn the_graft_adds_the_wgpu_feature_to_the_bench_feature_once() {
        let toml = "[features]\nbench = [\"subsonic/mock\", \"listenbrainz/mock\"]\nbench-opengl = [\"bench\"]\n";
        let t = graft_feature(toml).unwrap();
        assert!(t.contains("bench = [\"subsonic/mock\", \"listenbrainz/mock\", \"slint/unstable-wgpu-30\"]\nbench-opengl"));
        assert_eq!(graft_feature(&t), None);
    }

    #[test]
    fn a_build_output_naming_another_checkouts_files_is_foreign() {
        let (main, worktree) = if cfg!(windows) { (Path::new(r"C:\a\Main"), Path::new(r"C:\a\worktree")) } else { (Path::new("/Users/a/Main"), Path::new("/Users/a/worktree")) };
        let file = main.join("crates").join("app").join("ui").join("app.slint");
        let out = format!("cargo:rerun-if-changed={}\ncargo:rustc-env=X=1\n", file.display());
        assert!(names_foreign_file(&out, worktree));
        assert!(!names_foreign_file(&out, main));
        assert!(!names_foreign_file("cargo:rerun-if-changed=build.rs\n", worktree));
    }
}
