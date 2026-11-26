//! `cargo xtask bench`: every bench mode on fresh fixtures, one pass/fail table (docs/testing.md,
//! "The bench runner").

use crate::args::{COMMON_HELP, Common, Parser};
use crate::cmd;
use crate::report::Fail;
use crate::workspace::Workspace;

fn help() -> String {
    format!(
        "\
cargo xtask bench [--profile fast|release] [--all] [options] [-- <runner arguments>]

Builds the app with the bench harness (--features bench), the fixture tools (the library's
examples) and the bench runner, at below-normal priority, then runs the runner and exits with
its exit code (non-zero when a mode did not pass). By default this is the working loop: the
`fast` profile, the long modes (`soak`, `bigscan`, `slowscan`) left out, pre-scanned library
templates reused across modes, and several modes at once. Pass `--all` and `--profile release`
for the full release pass.

The runner makes the fixtures in <temp>/mp-bench-fixtures if they are not there and runs each
mode with its data, reports and snapshots in <temp>/mp-bench-runs. On Linux, run it inside a
session with a display and PipeWire (docs/linux.md, \"Tested in WSL, and what was not\").

In cargo's default target folder, `--profile fast` shares `target/fast` with `cargo build
--profile fast` and `cargo xtask check` so dependencies are built once; `--profile release`
uses `target/bench` so the bench-enabled binary does not overwrite or relink the release build.
A target folder that was chosen (`--target-dir`, `CARGO_TARGET_DIR`) is used as it is.

Options:
  --profile P        fast (default) or release: `fast` rebuilds in seconds; what a mode checks
                     is the same, its frame and CPU timings are not the release's
  --all              include the long modes (`soak`, `bigscan`, `slowscan`)
{COMMON_HELP}

The runner's arguments (after `--`): --only a,b  --skip a,b  --quick  --all  --jobs N  --cold
  --for PATH...  --changed [REF]  --keep  --list  --exe PATH  --fixtures DIR  --out DIR

  cargo xtask bench                              quick loop while working (fast, parallel)
  cargo xtask bench -- --only skip,gapless       named modes only
  cargo xtask bench -- --changed                 only modes for uncommitted changes
  cargo xtask bench -- --for crates/app/src/visualizer
  cargo xtask bench --profile release --all      full release pass (including long modes)"
    )
}

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut p = Parser::new("bench", args);
    let mut common = Common::default();
    let mut profile = "fast".to_owned();
    let mut all = false;
    let mut runner_args = Vec::new();
    while let Some(a) = p.next()? {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return Ok(());
            }
            "--profile" => {
                profile = p.value()?;
                if profile != "release" && profile != "fast" {
                    return Err(Fail::usage(format!("--profile is fast or release, not `{profile}`")));
                }
            }
            "--all" => all = true,
            "--" => runner_args = p.rest(),
            _ if common.take(&a, &mut p)? => {}
            _ if a.starts_with('-') => return Err(Fail::usage(format!("`cargo xtask bench` has no option `{a}`; the runner's arguments go after `--`: cargo xtask bench -- {a}"))),
            _ => return Err(p.unknown(&a)),
        }
    }

    let ws = Workspace::find(&common)?;
    // Share `target/fast` with working builds and `xtask check` so Slint, Skia and the rest are
    // not compiled twice; keep `--profile release` in `target/bench` so `--features bench` never
    // overwrites or LTO-relinks the shippable `target/release` binary.
    let target = if ws.chosen || profile == "fast" { ws.target.clone() } else { ws.target.join("bench") };
    println!("target folder: {}", target.display());
    let builds: [&[&str]; 3] = [&["-p", "marquee-player", "--features", "bench"], &["-p", "library", "--examples"], &["-p", "bench-runner"]];
    for b in builds {
        let mut a = vec!["build", "--profile", &profile];
        a.extend(b);
        let code = cmd::status(&mut cmd::below_normal(ws.cargo(&target, &a)))?;
        if code != 0 {
            return Err(Fail::code(code));
        }
    }
    let runner = target.join(&profile).join(Workspace::exe("bench-runner"));
    let mut pass = Vec::new();
    // Skip long modes by default unless the caller asked for `--all`, `--only`, or `--list`/`--help`.
    let caller_controls_scope = all || runner_args.iter().any(|a| matches!(a.as_str(), "--all" | "--quick" | "--only" | "--list" | "-h" | "--help"));
    if !caller_controls_scope {
        pass.push("--quick".to_owned());
    }
    pass.extend(runner_args);
    let code = cmd::status(std::process::Command::new(&runner).args(&pass).current_dir(&ws.root))?;
    if code != 0 {
        return Err(Fail::code(code));
    }
    Ok(())
}
