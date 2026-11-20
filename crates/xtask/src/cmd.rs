//! Running cargo, git and the programs built.

use std::ffi::OsString;
use std::process::{Command, Output, Stdio};

use crate::report::Fail;

/// The cargo that runs this tool (`cargo xtask` sets `CARGO`), so every build uses the same
/// toolchain; plain `cargo` otherwise.
pub fn cargo_program() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Runs `cmd` with this terminal, and gives its exit code (1 for a process ended by a
/// signal). Fails only when it could not be started at all.
pub fn status(cmd: &mut Command) -> Result<i32, Fail> {
    let s = cmd.status().map_err(|e| Fail::msg(format!("could not run {}: {e}", describe(cmd))))?;
    Ok(s.code().unwrap_or(1))
}

/// Runs `cmd` with no input and gives what it printed.
pub fn output(cmd: &mut Command) -> Result<Output, Fail> {
    cmd.stdin(Stdio::null()).output().map_err(|e| Fail::msg(format!("could not run {}: {e}", describe(cmd))))
}

/// The program and its arguments, as for a shell.
pub fn describe(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().into_owned();
    for a in cmd.get_args() {
        let a = a.to_string_lossy();
        if a.is_empty() || a.contains([' ', '\t', '"']) {
            s.push_str(&format!(" \"{a}\""));
        } else {
            s.push(' ');
            s.push_str(&a);
        }
    }
    s
}

/// Starts `cmd` (and everything it starts) at below-normal priority, so a long build leaves
/// the machine usable: on Windows the priority class, which child processes inherit; elsewhere
/// through `nice`.
pub fn below_normal(cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        let mut cmd = cmd;
        cmd.creation_flags(BELOW_NORMAL_PRIORITY_CLASS);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut nice = Command::new("nice");
        nice.arg("-n").arg("10").arg(cmd.get_program()).args(cmd.get_args());
        if let Some(dir) = cmd.get_current_dir() {
            nice.current_dir(dir);
        }
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => nice.env(k, v),
                None => nice.env_remove(k),
            };
        }
        nice
    }
}
