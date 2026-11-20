//! The checkout and where cargo builds in it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::Common;
use crate::cmd;
use crate::report::Fail;

pub struct Workspace {
    /// The folder of the workspace's Cargo.toml.
    pub root: PathBuf,
    /// Where cargo builds: `--target-dir`, else `CARGO_TARGET_DIR`, else cargo's default.
    pub target: PathBuf,
    /// Whether the target folder was chosen (`--target-dir`, `CARGO_TARGET_DIR`) rather than
    /// cargo's default.
    pub chosen: bool,
    jobs: Vec<String>,
}

impl Workspace {
    pub fn find(common: &Common) -> Result<Self, Fail> {
        let here = std::env::current_dir().map_err(|e| Fail::msg(format!("no current folder: {e}")))?;
        let out = cmd::output(Command::new(cmd::cargo_program()).args(["locate-project", "--workspace", "--message-format", "plain"]))?;
        if !out.status.success() {
            return Err(Fail::msg(format!("not in Marquee Player's checkout: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        let manifest = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let root = manifest.parent().map(Path::to_path_buf).ok_or_else(|| Fail::msg("cargo named no workspace folder"))?;

        let given = common.target_dir.clone().map(PathBuf::from).or_else(|| std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()).map(PathBuf::from));
        let (target, chosen) = match given {
            // Relative to where the command was run, as cargo takes it.
            Some(dir) => (here.join(dir), true),
            None => (default_target(&root)?, false),
        };
        Ok(Self { root, target, chosen, jobs: common.jobs_args() })
    }

    /// `cargo <args>` in the checkout, building in `target`, with `-j` when one was given (for
    /// a command that builds).
    pub fn cargo(&self, target: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(cmd::cargo_program());
        c.current_dir(&self.root).env("CARGO_TARGET_DIR", target);
        match args.split_first() {
            // Right after the subcommand: anything after a `--` is not cargo's.
            Some((sub @ &("build" | "test" | "clippy"), rest)) => c.arg(sub).args(&self.jobs).args(rest),
            _ => c.args(args),
        };
        c
    }

    /// The name of a program cargo builds (`MarqueePlayer.exe` on Windows).
    pub fn exe(name: &str) -> String {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    }
}

/// cargo's own target folder for this checkout (`target/`, or what a cargo configuration
/// says), from `cargo metadata`.
fn default_target(root: &Path) -> Result<PathBuf, Fail> {
    let out = cmd::output(Command::new(cmd::cargo_program()).current_dir(root).env_remove("CARGO_TARGET_DIR").args(["metadata", "--no-deps", "--format-version", "1"]))?;
    if !out.status.success() {
        return Err(Fail::msg(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let json = String::from_utf8_lossy(&out.stdout);
    json_string(&json, "target_directory").map(PathBuf::from).ok_or_else(|| Fail::msg("cargo metadata named no target folder"))
}

/// The string value of the first `"key":"..."` in a JSON text (enough for cargo's metadata,
/// without a JSON parser).
fn json_string(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = json[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::json_string;

    #[test]
    fn reads_a_string_with_escapes() {
        let json = r#"{"packages":[],"target_directory":"C:\\Users\\a \"b\"\\target","version":1}"#;
        assert_eq!(json_string(json, "target_directory").as_deref(), Some(r#"C:\Users\a "b"\target"#));
        assert_eq!(json_string(json, "workspace_root"), None);
    }
}
