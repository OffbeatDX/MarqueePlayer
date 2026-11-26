//! `cargo xtask <command>`: Marquee Player's development commands, one way to do each thing on
//! every system (docs/building.md, "Build and run"). The alias is in `.cargo/config.toml`. Standard
//! library only (and serde_json, for `release`), so it compiles in seconds.
//!
//! - `build`: the release build, copied beside Cargo.toml (`build.rs` here);
//! - `check`: the checks before a push, and the opt-in pre-push hook (`check.rs`);
//! - `bench`: every bench mode through the bench runner (`bench/mod.rs`);
//! - `perf`: what a change costs to draw against its base, two builds compared (`perf/`);
//! - `release`: a numbered release for this system, packaged (`release/`).
//!
//! Where cargo builds, for every command: `--target-dir DIR`, else `CARGO_TARGET_DIR`, else
//! cargo's own default (`target/` in the checkout) (`workspace.rs`).

mod args;
mod bench;
mod build;
mod check;
mod cmd;
mod perf;
mod release;
mod report;
mod workspace;

use report::Fail;

const HELP: &str = "\
cargo xtask <command> [options]: Marquee Player's development commands

Commands:
  build    the release build of the app, copied beside Cargo.toml
  check    the checks before a push (whitespace, formatting, clippy, tests); installs the
           opt-in pre-push hook
  bench    every bench mode on fresh fixtures, through the bench runner
  perf     what the working tree costs to draw against a base (default: where it left
           master): both built, scenes measured on each, one table with verdicts
  release  a numbered release for this system, in dist/: Windows, the portable zip and the
           installer; Linux, the AppImage; macOS, the disk image
  help     this text (`cargo xtask help <command>`, or `<command> --help`, for one command)

Options every command takes:
  --target-dir DIR   where cargo builds (default: CARGO_TARGET_DIR if set, else cargo's
                     default, target/ in the checkout)
  -j, --jobs N       parallel build jobs (default: cargo's)

Run from anywhere in the checkout.";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.is_empty() {
        eprintln!("{HELP}");
        Err(Fail::code(2))
    } else {
        let command = args.remove(0);
        run(&command, args)
    };
    if let Err(fail) = result {
        if let Some(message) = &fail.message {
            eprintln!("error: {message}");
        }
        std::process::exit(fail.code);
    }
}

fn run(command: &str, args: Vec<String>) -> Result<(), Fail> {
    match command {
        "build" => build::run(args),
        "release" => release::run(args),
        "check" => check::run(args),
        "bench" => bench::run(args),
        "perf" => perf::run(args),
        "help" | "--help" | "-h" => match args.first().map(String::as_str) {
            None => {
                println!("{HELP}");
                Ok(())
            }
            Some(c @ ("build" | "check" | "bench" | "perf" | "release")) => run(c, vec!["--help".into()]),
            Some(other) => Err(Fail::usage(format!("no command is called `{other}` (cargo xtask --help lists them)"))),
        },
        other => Err(Fail::usage(format!("no command is called `{other}` (cargo xtask --help lists them)"))),
    }
}
