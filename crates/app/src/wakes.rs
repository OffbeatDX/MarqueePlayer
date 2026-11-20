//! With `MP_LOG=debug`: once a minute, a line of how often each part of the app woke and how
//! much CPU the process used, to tell what keeps a quiet player busy (a report of high CPU
//! without a measurement: docs/testing.md has the counters' meaning). Off otherwise: nothing
//! is started, and the counters are one relaxed add where they are counted.
//!
//! The UI's frames are counted as winit asks for them (`RedrawRequested`); on macOS Slint also
//! draws from its display link without asking winit, which this does not see: what the
//! visualizer and the ticks change is drawn once each, so their counts stand for those frames.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use player_core::wakes as core;

/// Runs of `playback::tick` (the timer's and the engine events').
pub static TICKS: AtomicU64 = AtomicU64::new(0);
/// Visualizer frames (each reads the tap, analyses and sets the paths).
pub static VIZ_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Frames of the tray icon's animation.
pub static TRAY_FRAMES: AtomicU64 = AtomicU64::new(0);
/// The window's winit events, and of them the redraws winit was asked for.
pub static WINDOW_EVENTS: AtomicU64 = AtomicU64::new(0);
pub static REDRAWS: AtomicU64 = AtomicU64::new(0);

const EVERY: Duration = Duration::from_secs(60);

pub fn note(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// Start the minute's line, when the log takes debug lines.
pub fn start() {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }
    let spawned = std::thread::Builder::new().name("mp-wakes".into()).spawn(|| {
        let counters = || {
            [
                core::get(&core::OUTPUT_LOOP),
                core::get(&core::CALLBACKS),
                TICKS.load(Ordering::Relaxed),
                VIZ_FRAMES.load(Ordering::Relaxed),
                TRAY_FRAMES.load(Ordering::Relaxed),
                WINDOW_EVENTS.load(Ordering::Relaxed),
                REDRAWS.load(Ordering::Relaxed),
            ]
        };
        let (mut at, mut cpu, mut last) = (Instant::now(), crate::platform::system::cpu_time(), counters());
        loop {
            std::thread::sleep(EVERY);
            let (now, cpu_now, c) = (Instant::now(), crate::platform::system::cpu_time(), counters());
            log::debug!("{}", line(now - at, cpu_now.saturating_sub(cpu), &last, &c));
            (at, cpu, last) = (now, cpu_now, c);
        }
    });
    if let Err(e) = spawned {
        log::warn!("no wake-up counts: {e}");
    }
}

fn line(span: Duration, cpu: Duration, before: &[u64; 7], now: &[u64; 7]) -> String {
    let secs = span.as_secs_f64().max(0.001);
    let r = |i: usize| now[i].saturating_sub(before[i]) as f64 / secs;
    format!(
        "wake-ups a second over {secs:.0} s: CPU {:.1}% of one core; output loop {:.1}, device callbacks {:.1}; UI ticks {:.1}, visualizer frames {:.1}, tray icon frames {:.1}, window events {:.1}, redraws asked of winit {:.1}",
        cpu.as_secs_f64() / secs * 100.0,
        r(0),
        r(1),
        r(2),
        r(3),
        r(4),
        r(5),
        r(6)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_says_rates_a_second_and_cpu_as_a_share_of_one_core() {
        let l = line(Duration::from_secs(60), Duration::from_secs(6), &[0; 7], &[5640, 5640, 600, 3600, 300, 60, 0]);
        assert_eq!(
            l,
            "wake-ups a second over 60 s: CPU 10.0% of one core; output loop 94.0, device callbacks 94.0; UI ticks 10.0, visualizer frames 60.0, tray icon frames 5.0, window events 1.0, redraws asked of winit 0.0"
        );
    }
}
