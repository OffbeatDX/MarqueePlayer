//! `cargo xtask bench`: every bench mode on fresh fixtures, one pass/fail table (docs/testing.md,
//! "The bench runner").

use crate::args::{COMMON_HELP, Common, Parser};
use crate::cmd;
use crate::report::Fail;
use crate::workspace::Workspace;

fn help() -> String {
    format!(
        "\
cargo xtask bench [--profile release|fast] [options] [-- <runner arguments>]

Builds the app with the bench harness (--features bench), the fixture tools (the library's
examples) and the bench runner, at below-normal priority, then runs the runner with the
arguments after `--` and exits with its exit code (non-zero when a mode did not pass). The
runner makes the fixtures in <temp>/mp-bench-fixtures if they are not there and runs each
mode with its data, music copy, reports and snapshots in <temp>/mp-bench-runs. On Linux, run
it inside a session with a display and PipeWire (docs/linux.md, \"Tested in WSL, and what was not\").

In cargo's default target folder the bench builds go to target/bench, so that they and the
release build (other features) do not rebuild each other; a target folder that was chosen
(--target-dir, CARGO_TARGET_DIR) is used as it is.

Options:
  --profile P        release (default) or fast: the `fast` profile (Cargo.toml) rebuilds in
                     seconds; what a mode checks is the same, its timings are not the release's
{COMMON_HELP}

The runner's arguments (after `--`): --only a,b  --skip a,b  --quick (no long modes)
  --keep  --list  --exe PATH  --fixtures DIR  --out DIR

  cargo xtask bench                              every mode (about half an hour)
  cargo xtask bench -- --only skip,gapless
  cargo xtask bench --profile fast -- --quick    while working"
    )
}

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut p = Parser::new("bench", args);
    let mut common = Common::default();
    let mut profile = "release".to_owned();
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
                    return Err(Fail::usage(format!("--profile is release or fast, not `{profile}`")));
                }
            }
            "--" => runner_args = p.rest(),
            _ if common.take(&a, &mut p)? => {}
            _ if a.starts_with('-') => return Err(Fail::usage(format!("`cargo xtask bench` has no option `{a}`; the runner's arguments go after `--`: cargo xtask bench -- {a}"))),
            _ => return Err(p.unknown(&a)),
        }
    }

    let ws = Workspace::find(&common)?;
    let target = if ws.chosen { ws.target.clone() } else { ws.target.join("bench") };
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
    let code = cmd::status(std::process::Command::new(&runner).args(&runner_args).current_dir(&ws.root))?;
    if code != 0 {
        return Err(Fail::code(code));
    }
    Ok(())
}
