//! Minimal logger: stderr plus `marquee-player.log` in the log folder (`Paths::log`), each line
//! with the seconds since the start. The previous run's log is kept as
//! `marquee-player.previous.log`, so the log of a run that crashed survives the restart that
//! follows (Settings > About's Copy diagnostics reads both). `MP_LOG=debug` for more.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

struct Logger {
    file: Mutex<Option<std::fs::File>>,
    level: log::LevelFilter,
    start: Instant,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        let t = m.target();
        let ours = t.starts_with("MarqueePlayer") || t.starts_with("player_core") || t.starts_with("library");
        m.level() <= self.level && (ours || m.level() <= log::Level::Warn)
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{:>9.3} [{:<5}] {}: {}\n", self.start.elapsed().as_secs_f64(), r.level(), r.target(), r.args());
        eprint!("{line}");
        // A panic while the lock was held must not silence the log for the rest of the run
        // (least of all the panic's own report).
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = f.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

static LOGGER: std::sync::OnceLock<&'static Logger> = std::sync::OnceLock::new();

/// Log to stderr from now on; the file comes with `open_file`.
pub fn init() {
    let level = match std::env::var("MP_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    let logger: &'static Logger = Box::leak(Box::new(Logger { file: Mutex::new(None), level, start: Instant::now() }));
    let _ = LOGGER.set(logger);
    let _ = log::set_logger(logger);
    log::set_max_level(level);
}

/// The previous run's log, beside this one's.
pub fn previous(file: &Path) -> PathBuf {
    file.with_file_name("marquee-player.previous.log")
}

/// Also log to `file`, started afresh; the log that was there becomes `previous(file)`. Only
/// the instance that runs does (after `instance::acquire`): a second launch must not touch the
/// log of the one it hands over to.
pub fn open_file(file: &Path) {
    if file.exists() {
        let _ = std::fs::rename(file, previous(file));
    }
    if let Some(l) = LOGGER.get() {
        let opened = std::fs::File::create(file);
        if let Err(e) = &opened {
            eprintln!("cannot write the log {}: {e}", file.display());
        }
        *l.file.lock().unwrap_or_else(|e| e.into_inner()) = opened.ok();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_previous_log_is_kept_beside_the_new_one() {
        let dir = std::env::temp_dir().join(format!("mp-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("marquee-player.log");
        std::fs::write(&log, "the crashed run\n").unwrap();
        super::open_file(&log);
        assert_eq!(std::fs::read_to_string(super::previous(&log)).unwrap(), "the crashed run\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
