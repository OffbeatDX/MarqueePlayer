//! `cargo xtask build`: the release build of the app, copied beside Cargo.toml.

use std::path::Path;

use crate::args::{COMMON_HELP, Common, Parser};
use crate::report::{self, Fail};
use crate::workspace::Workspace;
use crate::cmd;

fn help() -> String {
    format!(
        "\
cargo xtask build [--test] [options]

Builds the app (release profile) and copies it beside Cargo.toml: MarqueePlayer.exe on
Windows, MarqueePlayer on Linux and macOS (a bare binary, not an app bundle: that is
`cargo xtask release`'s). First checks that neither that copy nor the built one is in
use (quit Marquee Player first), so a running app does not fail the build minutes in.
The build is a development build, numbered YYYY.M.D-dev; numbered releases come from
`cargo xtask release`.

Options:
  --test             run the workspace's tests (release profile) first
{COMMON_HELP}"
    )
}

pub fn run(args: Vec<String>) -> Result<(), Fail> {
    let mut p = Parser::new("build", args);
    let mut common = Common::default();
    let mut test = false;
    while let Some(a) = p.next()? {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", help());
                return Ok(());
            }
            "--test" => test = true,
            _ if common.take(&a, &mut p)? => {}
            _ => return Err(p.unknown(&a)),
        }
    }

    let ws = Workspace::find(&common)?;
    let name = Workspace::exe("MarqueePlayer");
    let copy = ws.root.join(&name);
    let built = ws.target.join("release").join(&name);
    // A running exe cannot be overwritten: say so before spending minutes on a build.
    for f in [&copy, &built] {
        if in_use(f) {
            return Err(Fail::msg(format!("{} is in use. Quit Marquee Player (tray icon, Quit) and run `cargo xtask build` again.", f.display())));
        }
    }
    println!("target folder: {}", ws.target.display());

    if test {
        report::step("tests (release profile)", || cmd::status(&mut ws.cargo(&ws.target, &["test", "--workspace", "--release"])))?;
    }
    report::step("build (release profile)", || cmd::status(&mut ws.cargo(&ws.target, &["build", "-p", "marquee-player", "--release"])))?;

    std::fs::copy(&built, &copy).map_err(|e| Fail::msg(format!("could not copy {} to {}: {e}", built.display(), copy.display())))?;
    println!();
    println!("Done: {}", copy.display());
    Ok(())
}

/// Whether a program is running from `path`: opening it to write fails then (Windows: a
/// sharing violation; Linux: "text file busy"). Opening it changes nothing in it (no
/// truncation, nothing written). Not append-only: Windows lets a running exe be opened to
/// append. A file that is not there is not in use.
pub fn in_use(path: &Path) -> bool {
    path.is_file() && std::fs::OpenOptions::new().write(true).open(path).is_err()
}
