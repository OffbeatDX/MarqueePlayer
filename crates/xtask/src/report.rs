//! How a command fails, and the "== step" / "ok:" / "FAILED:" lines of a run.

use std::time::Instant;

/// Why a command stops: the exit code to leave with, and a message for standard error (none
/// when what failed has said why already, as cargo and git do).
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub message: Option<String>,
}

impl Fail {
    /// Leave with `code`, saying nothing more.
    pub fn code(code: i32) -> Self {
        Self { code, message: None }
    }

    /// Something went wrong: exit code 1.
    pub fn msg(message: impl Into<String>) -> Self {
        Self { code: 1, message: Some(message.into()) }
    }

    /// The command line was wrong: exit code 2.
    pub fn usage(message: impl Into<String>) -> Self {
        Self { code: 2, message: Some(message.into()) }
    }
}

/// A heading for what runs next.
pub fn heading(what: &str) {
    println!();
    println!("== {what}");
}

/// One step: its heading, then `ok: <what> (<n> s)`, or `FAILED: <what> (exit <code>)` and a
/// [`Fail`] carrying that exit code. `run` gives the exit code of what it ran.
pub fn step(what: &str, run: impl FnOnce() -> Result<i32, Fail>) -> Result<(), Fail> {
    heading(what);
    let start = Instant::now();
    let code = run()?;
    if code != 0 {
        println!("FAILED: {what} (exit {code})");
        return Err(Fail::code(code));
    }
    println!("ok: {what} ({} s)", start.elapsed().as_secs());
    Ok(())
}
