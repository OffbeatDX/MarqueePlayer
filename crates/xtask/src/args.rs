//! A small hand-written argument parser: `--name`, `--name VALUE`, `--name=VALUE`, `-j N`, and
//! `--` before arguments handed on untouched.

use crate::report::Fail;

pub struct Parser {
    command: &'static str,
    args: std::vec::IntoIter<String>,
    /// The value given as `--name=VALUE`, until the option takes it.
    inline: Option<String>,
    current: String,
}

impl Parser {
    pub fn new(command: &'static str, args: Vec<String>) -> Self {
        Self { command, args: args.into_iter(), inline: None, current: String::new() }
    }

    /// The next option's name (`--base` for `--base=x` too), `--` itself, or a plain argument.
    /// Errors when the previous option was given a value it does not take.
    pub fn next(&mut self) -> Result<Option<String>, Fail> {
        if self.inline.is_some() {
            return Err(Fail::usage(format!("{} takes no value (cargo xtask {} --help)", self.current, self.command)));
        }
        let Some(arg) = self.args.next() else { return Ok(None) };
        let name = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") && name.len() > 2 => {
                self.inline = Some(value.to_owned());
                name.to_owned()
            }
            _ => arg,
        };
        self.current = name.clone();
        Ok(Some(name))
    }

    /// The value of the option [`next`](Self::next) just gave.
    pub fn value(&mut self) -> Result<String, Fail> {
        if let Some(v) = self.inline.take() {
            return Ok(v);
        }
        match self.args.next() {
            Some(v) if !v.starts_with("--") => Ok(v),
            _ => Err(Fail::usage(format!("{} needs a value (cargo xtask {} --help)", self.current, self.command))),
        }
    }

    /// Everything after `--`, untouched.
    pub fn rest(&mut self) -> Vec<String> {
        self.args.by_ref().collect()
    }

    /// The error for an argument this command does not take.
    pub fn unknown(&self, arg: &str) -> Fail {
        Fail::usage(format!("`cargo xtask {}` has no option `{arg}` (cargo xtask {} --help lists them)", self.command, self.command))
    }
}

/// The options every command takes.
#[derive(Default)]
pub struct Common {
    pub target_dir: Option<String>,
    pub jobs: Option<String>,
}

impl Common {
    /// Takes `arg` if it is one of the common options; false if it is not.
    pub fn take(&mut self, arg: &str, p: &mut Parser) -> Result<bool, Fail> {
        match arg {
            "--target-dir" => self.target_dir = Some(p.value()?),
            "-j" | "--jobs" => {
                let n = p.value()?;
                if n.parse::<u32>().map_or(true, |n| n == 0) {
                    return Err(Fail::usage(format!("{arg} needs a number of jobs, not `{n}`")));
                }
                self.jobs = Some(n);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `-j N` for cargo, when one was given.
    pub fn jobs_args(&self) -> Vec<String> {
        self.jobs.iter().flat_map(|n| ["-j".to_owned(), n.clone()]).collect()
    }
}

/// The help lines for the options every command takes.
pub const COMMON_HELP: &str = "\
  --target-dir DIR   where cargo builds (default: CARGO_TARGET_DIR if set, else cargo's
                     default, target/ in the checkout)
  -j, --jobs N       parallel build jobs (default: cargo's)";
