//! What happens when the app meets an error it cannot recover from: a panic nobody catches, or
//! a start that cannot go on (no window, no database). Never a silent disappearance (a release
//! build has no console): the log gets what happened, with a backtrace for a panic, and the
//! user gets a short message saying what to do and where the log is. The previous run's log is
//! kept beside this one (`logging::open_file`), so it survives the restart that follows.
//!
//! Panics that the library and the player catch and recover from themselves (one damaged file
//! in a scan, a decoder failing on one track: `library::guard`, `player_core::guard`) are
//! logged with their backtrace and nothing else happens. Any other panic is fatal: on the UI
//! thread the app is going down anyway; on another thread the work it did (search, covers, the
//! engine) is gone, so the app says so and closes the normal way, saving the queue and settings,
//! rather than sit there half working ("playing" with no sound).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static LOG: OnceLock<PathBuf> = OnceLock::new();
static FATAL: AtomicBool = AtomicBool::new(false);
/// A fatal panic's message on the UI thread, shown by `main` once the panic has unwound.
static PENDING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// `main` has taken the message to show: the watchdog of a UI-thread panic leaves the ending to it.
static MAIN_SHOWS: AtomicBool = AtomicBool::new(false);

/// The message of a fatal panic of the UI thread, if there was one, for `main` to show (and
/// then end the process).
pub fn pending() -> Option<String> {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    MAIN_SHOWS.store(true, Ordering::SeqCst);
    p.take()
}

/// Install the panic hook. `log` is where this run logs (shown in the message).
pub fn install(log: PathBuf) {
    let _ = LOG.set(log);
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed").to_owned();
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let caught = library::guard::active() || player_core::guard::active();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(
            "{} panicked in thread '{name}' at {at}: {what}\nbacktrace:\n{backtrace}",
            if caught { "caught and recovered: code" } else { "FATAL: code" }
        );
        eprintln!("thread '{name}' panicked at {at}: {what}");
        if caught || FATAL.swap(true, Ordering::SeqCst) {
            return;
        }
        // The tray's wheel hook (if the pointer is over the icon) goes first: the system's
        // mouse input must not wait on a process that is going down.
        crate::platform::tray::release();
        let message = message(&format!("Something went wrong inside Marquee Player ({what})."));
        if name == "main" {
            // Unwinding out of the event loop ends the process: `main` shows this once it has.
            *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
            // The toolkit catches a panic in a window callback and may run on: ask its loop to end
            // (the panic then comes out of it into `main`), and if the process is still here a
            // few seconds later, say it and end it from another thread.
            let _ = slint::quit_event_loop();
            let _ = std::thread::Builder::new().name("mp-crash".into()).spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let m = {
                    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                    if p.is_none() && MAIN_SHOWS.load(Ordering::SeqCst) {
                        // `main` is showing it (a message box the user reads): it ends the process.
                        return;
                    }
                    p.take()
                };
                if let Some(m) = m {
                    show(&m);
                }
                die(101);
            });
        } else {
            // Another thread: the UI is alive. It says so, then quits the normal way.
            let m = message.clone();
            if slint::invoke_from_event_loop(move || {
                show(&m);
                let _ = slint::quit_event_loop();
            })
            .is_err()
            {
                show(&message);
                die(101);
            }
        }
    }));
}

/// The message for a fatal error: what happened, that the app closes, where the log is, how to
/// send it.
pub fn message(what: &str) -> String {
    let log = LOG.get().map(|p| p.display().to_string()).unwrap_or_else(|| "marquee-player.log in the app's data folder".into());
    format!(
        "{what}\n\nMarquee Player has to close. Your library, playlists and settings are kept.\n\n\
         What happened is written in its log:\n{log}\n\
         (When Marquee Player starts again, this log is kept as marquee-player.previous.log.)\n\n\
         To report it: start Marquee Player again, open Settings > About and press \u{201c}Copy diagnostics\u{201d}, \
         then paste that into an issue on the project's page (Settings > About gives its address)."
    )
}

/// Show `text` in a message box and log it. Never in a bench run (nobody would close it): the
/// run's report and exit code carry it.
pub fn show(text: &str) {
    log::error!("{text}");
    if crate::bench_run() {
        #[cfg(feature = "bench")]
        crate::bench::count_fail();
        return;
    }
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Marquee Player").set_description(text).show();
}

/// End the process at once with `code`, after a fatal panic: the log is written (unbuffered),
/// and the normal exit would run thread-local destructors over state the panic left half
/// unwound (measured: one panics, and the runtime aborts).
pub fn die(code: i32) -> ! {
    crate::platform::tray::release();
    #[cfg(windows)]
    // SAFETY: ends this process; nothing runs after it.
    unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(windows_sys::Win32::System::Threading::GetCurrentProcess(), code as u32);
    }
    #[cfg(unix)]
    // SAFETY: as above.
    unsafe {
        libc::_exit(code);
    }
    #[allow(unreachable_code)]
    std::process::exit(code)
}

/// A start that cannot go on: say why and what to do, then exit.
pub fn fatal_at_start(text: &str) -> ! {
    show(text);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_message_says_what_to_do_and_where_the_log_is() {
        let m = super::message("Something went wrong.");
        assert!(m.starts_with("Something went wrong."));
        assert!(m.contains("has to close"));
        assert!(m.contains("marquee-player.log"));
        assert!(m.contains("previous.log"));
        assert!(m.contains("Copy diagnostics"));
    }
}
