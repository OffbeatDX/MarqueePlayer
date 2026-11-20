//! `cargo xtask check`: the checks to pass before a push (CI runs the same on Windows and
//! Ubuntu, and on macOS when asked, `.github/workflows/desktop.yml`), and the opt-in pre-push
//! hook that runs them.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::args::{COMMON_HELP, Common, Parser};
use crate::cmd;
use crate::report::{self, Fail};
use crate::workspace::Workspace;

fn help() -> String {
    format!(
        "\
cargo xtask check [--base REV] [--strict-format] [options]
cargo xtask check --install-hook [--force]

The checks to pass before a push, in order, stopping at the first failure:
  - whitespace damage in the commits checked (`git diff --check`: trailing spaces, a space
    before a tab, leftover conflict markers), android/ left out;
  - formatting (`cargo fmt --check`): reported, but failing the run only with
    --strict-format or MP_CHECK_FMT=strict, as the tree is not rustfmt-formatted;
  - clippy over the workspace with -D warnings, without and with the bench harness
    (--features marquee-player/bench);
  - the workspace's tests.
Builds on the `fast` profile (Cargo.toml). Exits with the failing step's exit code. CI runs
the same checks on Windows and Ubuntu, and on macOS when asked (.github/workflows/desktop.yml).

The commits checked: from REV (--base), else from where HEAD left master, else (HEAD on
master) the last commit. Where git cannot read the checkout at all, the whitespace step is
skipped and said so; where it refuses it as another user's (\"dubious ownership\"), it is
trusted for this run's own git commands only.

Options:
  --base REV         check the whitespace of REV..HEAD
  --strict-format    also fail when rustfmt would change a file
  --install-hook     install the checks as this clone's git pre-push hook (opt-in; a hook
                     that is there already is kept unless --force); `git push --no-verify`
                     skips it once
  --force            with --install-hook: replace the hook that is there
  --hook             what the hook runs: the ranges pushed come on standard input
{COMMON_HELP}"
    )
}

/// The pre-push hook: git runs it with its own sh, on Windows too.
const HOOK: &str = "\
#!/bin/sh
# Installed by `cargo xtask check --install-hook`: the checks before a push.
# Skip once with `git push --no-verify`; remove this file to uninstall.
cd \"$(git rev-parse --show-toplevel)\" || exit 1
exec cargo xtask check --hook
";

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut p = Parser::new("check", args);
    let mut common = Common::default();
    let (mut base, mut strict, mut install, mut force, mut hook) = (None, std::env::var("MP_CHECK_FMT").is_ok_and(|v| v == "strict"), false, false, false);
    while let Some(a) = p.next()? {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return Ok(());
            }
            "--base" => base = Some(p.value()?),
            "--strict-format" => strict = true,
            "--install-hook" => install = true,
            "--force" => force = true,
            "--hook" => hook = true,
            _ if common.take(&a, &mut p)? => {}
            _ => return Err(p.unknown(&a)),
        }
    }
    if force && !install {
        return Err(Fail::usage("--force goes with --install-hook"));
    }
    if hook && base.is_some() {
        return Err(Fail::usage("--hook takes the ranges pushed from standard input, not --base"));
    }

    let ws = Workspace::find(&common)?;
    let git = Git::open(&ws.root);
    if install {
        return install_hook(git.map_err(|why| Fail::msg(format!("cannot install the hook: {why}")))?, force);
    }

    println!("target folder: {}", ws.target.display());
    // The whitespace step comes first, before anything builds (a bad --base fails it at once).
    match &git {
        Ok(g) => {
            let ranges = if hook { pushed(g)? } else { g.ranges(base.as_deref()) };
            for r in ranges {
                report::step(&format!("whitespace in {r} (git diff --check)"), || cmd::status(g.cmd().args(["--no-pager", "diff", "--check", &r, "--", ".", ":(exclude)android"])))?;
            }
        }
        Err(why) => {
            report::heading("whitespace (git diff --check)");
            println!("skipped: git cannot be used on this checkout ({why}); the other checks run. Check the whitespace where git works (a clone of its own).");
        }
    }

    formatting(&ws, strict)?;
    let fast = ["--profile", "fast"];
    let clippy = |extra: &[&str]| {
        let mut a = vec!["clippy", "--workspace", "--all-targets"];
        a.extend(fast);
        a.extend(extra);
        a.extend(["--", "-D", "warnings"]);
        cmd::status(&mut ws.cargo(&ws.target, &a))
    };
    report::step("clippy (without the bench harness)", || clippy(&[]))?;
    report::step("clippy (with the bench harness)", || clippy(&["--features", "marquee-player/bench"]))?;
    report::step("tests (fast profile)", || cmd::status(&mut ws.cargo(&ws.target, &["test", "--workspace", "--profile", "fast", "--no-fail-fast"])))?;
    println!();
    println!("All checks passed.");
    Ok(())
}

/// `cargo fmt --check`: a failure only when strict.
fn formatting(ws: &Workspace, strict: bool) -> Result<(), Fail> {
    report::heading("formatting (cargo fmt --check)");
    let out = cmd::output(&mut ws.cargo(&ws.target, &["fmt", "--all", "--", "--check"]))?;
    if out.status.success() {
        println!("ok: rustfmt changes nothing");
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let changed = text.lines().filter(|l| l.starts_with("Diff in ")).count();
    if changed == 0 {
        // Not a difference: rustfmt itself did not run (not installed, a file it cannot parse).
        let code = out.status.code().unwrap_or(1);
        for l in text.lines().take(20) {
            println!("{l}");
        }
        if strict {
            println!("FAILED: cargo fmt could not check the tree (exit {code})");
            return Err(Fail::code(code));
        }
        println!("note: cargo fmt could not check the tree (exit {code}; not enforced, --strict-format enforces it)");
        return Ok(());
    }
    if strict {
        for l in text.lines().take(60) {
            println!("{l}");
        }
        println!("FAILED: rustfmt would change {changed} places");
        return Err(Fail::code(1));
    }
    println!("note: rustfmt would change {changed} places (not enforced: the tree is not rustfmt-formatted; --strict-format enforces it)");
    Ok(())
}

fn install_hook(git: Git, force: bool) -> Result<(), Fail> {
    let hooks = git.out(&["rev-parse", "--path-format=absolute", "--git-path", "hooks"]).ok_or_else(|| Fail::msg("git did not name this clone's hooks folder"))?;
    let hooks = PathBuf::from(hooks);
    let file = hooks.join("pre-push");
    if file.exists() && !force {
        println!("{} is there already; not replaced (--force replaces it).", file.display());
        return Err(Fail::code(1));
    }
    let io = |e: std::io::Error| Fail::msg(format!("could not write {}: {e}", file.display()));
    std::fs::create_dir_all(&hooks).map_err(io)?;
    std::fs::write(&file, HOOK).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
    }
    println!("Installed {}. git push now runs `cargo xtask check` first (git push --no-verify skips it).", file.display());
    Ok(())
}

/// The ranges a push sends, from the hook's standard input: a line
/// "<local ref> <local sha> <remote ref> <remote sha>" each.
fn pushed(g: &Git) -> Result<Vec<String>, Fail> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| Fail::msg(format!("could not read what is pushed: {e}")))?;
    Ok(input.lines().filter_map(|l| pushed_range(g, l)).collect())
}

fn pushed_range(g: &Git, line: &str) -> Option<String> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let (local, remote) = (*f.get(1)?, *f.get(3)?);
    // A branch deleted: nothing to check.
    if is_zero(local) {
        return None;
    }
    if !is_zero(remote) {
        return Some(format!("{remote}..{local}"));
    }
    // A new branch: what it adds to master, or its last commit.
    Some(g.since_trunk(local))
}

fn is_zero(sha: &str) -> bool {
    !sha.is_empty() && sha.bytes().all(|b| b == b'0')
}

/// git on this checkout.
struct Git {
    root: PathBuf,
    /// Set when git refuses the checkout as another user's: trusted for these commands only
    /// (`-c safe.directory`, on the command line: nobody's git configuration changes).
    safe: Option<String>,
}

impl Git {
    /// git, if it can read this checkout; why not, otherwise. Run as another user than the
    /// checkout's owner (root in WSL on /mnt/c, say), git refuses it ("dubious ownership"):
    /// then it is trusted for this run. Where git cannot read it at all (a worktree made on
    /// Windows, whose .git names a Windows path; no git), there is no git.
    fn open(root: &Path) -> Result<Self, String> {
        let mut g = Self { root: root.to_path_buf(), safe: None };
        let out = match g.cmd().args(["rev-parse", "--verify", "-q", "HEAD"]).stdin(Stdio::null()).output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("git is not installed".into()),
            Err(e) => return Err(format!("git could not run: {e}")),
        };
        if out.status.success() {
            return Ok(g);
        }
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        if err.contains("dubious ownership") {
            g.safe = Some(root.to_string_lossy().replace('\\', "/"));
            if g.out(&["rev-parse", "--verify", "-q", "HEAD"]).is_some() {
                println!("note: git refuses this checkout as this user (it belongs to another user); the checks trust it for this run only (git -c safe.directory={})", g.safe.as_deref().unwrap_or_default());
                return Ok(g);
            }
            return Err("git refuses this checkout (dubious ownership) even trusted for this run".into());
        }
        let first = err.lines().next().unwrap_or("").trim();
        Err(if first.is_empty() { "no commit to check".into() } else { first.to_owned() })
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("git");
        c.current_dir(&self.root);
        if let Some(s) = &self.safe {
            c.arg("-c").arg(format!("safe.directory={s}"));
        }
        c
    }

    /// What a git command printed, trimmed, when it succeeded.
    fn out(&self, args: &[&str]) -> Option<String> {
        let o = self.cmd().args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
        (o.status.success() && !s.is_empty()).then_some(s)
    }

    /// The ranges checked outside the hook: from `base`, else from where HEAD left master.
    fn ranges(&self, base: Option<&str>) -> Vec<String> {
        match base {
            Some(b) => vec![format!("{b}..HEAD")],
            None => vec![self.since_trunk("HEAD")],
        }
    }

    /// `rev`'s commits since it left master (`main` in a clone without master); `rev` on
    /// master itself: its last commit (everything in it, for a first commit).
    fn since_trunk(&self, rev: &str) -> String {
        let trunk = ["master", "main"].into_iter().find(|b| self.out(&["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")]).is_some());
        let sha = self.out(&["rev-parse", rev]);
        if let (Some(t), Some(sha)) = (trunk, &sha)
            && let Some(mb) = self.out(&["merge-base", rev, t])
            && &mb != sha
        {
            return format!("{mb}..{rev}");
        }
        if self.out(&["rev-parse", "-q", "--verify", &format!("{rev}~1")]).is_some() {
            return format!("{rev}~1..{rev}");
        }
        // A first commit has no parent: everything in it, from the empty tree.
        match self.out(&["hash-object", "-t", "tree", "--stdin"]) {
            Some(empty) => format!("{empty}..{rev}"),
            None => format!("{rev}~1..{rev}"),
        }
    }
}
