//! The progress in the top right of Home (`Activity` in stores.slint): a scan of the folders or
//! a sync with the server, whichever runs (docs/features/library.md, "Scanning", "What it says").
//!
//! Every scan shows it (the first import, a rescan, the folder watcher's, a folder's removal),
//! and every sync (after signing in, at start, Sync now, back from the tray). A quick one does
//! not flicker: once shown it stays at least `MIN_SHOWN`, and when its job ends the bar runs to
//! full and stays there `AT_FULL` before it goes. A job that comes right after (a scan and a sync
//! take turns writing the library, `server::sync::WRITER`; a rescan asked for during a scan) takes
//! its place then, with no gap. A scan that starts while a sync runs is shown at once: the sync
//! waits for it.
//!
//! Nothing runs while nothing happens: this is called when a scan or a sync starts, says how far
//! it is or ends (`scanning.rs`, `server::show`), and the only timer is one single shot, for the
//! end of the hold.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::state::{self, State};
use crate::{Activity, AppWindow, Library, Server};

/// The least time the indicator stays once shown.
pub const MIN_SHOWN: Duration = Duration::from_millis(700);
/// How long a finished job's bar stays at full (its width animates to it in 400 ms).
pub const AT_FULL: Duration = Duration::from_millis(450);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Scan,
    Sync,
}

/// What the indicator shows: a job, running or `done` (its bar at full).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub job: Job,
    pub done: bool,
}

/// When to show what, given which jobs are busy; no clock or UI of its own.
#[derive(Debug, Default)]
pub struct Indicator {
    /// The job shown, and since when.
    shown: Option<(Job, Instant)>,
    /// The job shown ended then: it is held at full until `release`.
    ended: Option<Instant>,
    /// The other job ran while this one was shown (a sync waiting for the scan shown): it gets
    /// its turn after, even when it was over by then.
    pending: Option<Job>,
}

impl Indicator {
    /// The jobs busy now (`scan`: a library job of `scanning.rs`; `sync`: `Server.busy`): what
    /// to show, and when to look again (`Some` only while a finished job is held).
    pub fn update(&mut self, scan: bool, sync: bool, now: Instant) -> (Option<Shown>, Option<Duration>) {
        let wanted = if scan {
            Some(Job::Scan)
        } else if sync {
            Some(Job::Sync)
        } else {
            None
        };
        let busy = |job: Job| match job {
            Job::Scan => scan,
            Job::Sync => sync,
        };
        let other = |job: Job| match job {
            Job::Scan => Job::Sync,
            Job::Sync => Job::Scan,
        };
        match self.shown {
            None => self.shown = wanted.map(|job| (job, now)),
            Some((job, since)) => {
                if busy(other(job)) {
                    self.pending = Some(other(job));
                }
                if self.ended.is_none() {
                    if !busy(job) {
                        self.ended = Some(now);
                    } else if wanted != Some(job) {
                        // A scan while a sync runs: the sync waits for it to finish writing,
                        // and comes back after it.
                        self.shown = wanted.map(|job| (job, now));
                        self.pending = Some(job);
                    }
                }
                if let Some(ended) = self.ended {
                    let release = (since + MIN_SHOWN).max(ended + AT_FULL);
                    if now < release {
                        return (Some(Shown { job, done: true }), Some(release - now));
                    }
                    self.ended = None;
                    let pending = self.pending.take();
                    self.shown = wanted.map(|job| (job, now));
                    if let (None, Some(p)) = (wanted, pending) {
                        // It ran and ended during the hold: its turn, at full.
                        self.shown = Some((p, now));
                        self.ended = Some(now);
                        return (Some(Shown { job: p, done: true }), Some(MIN_SHOWN));
                    }
                }
            }
        }
        (self.shown.map(|(job, _)| Shown { job, done: false }), None)
    }
}

thread_local! {
    static INDICATOR: RefCell<Indicator> = RefCell::default();
    /// Each job's last words (title, text) while it ran, for its turn after it ended.
    static WORDS: RefCell<[(String, String); 2]> = RefCell::default();
    static HOLD: Timer = Timer::default();
    /// Every change of what is shown, with when (for the bench).
    #[cfg(feature = "bench")]
    static LOG: RefCell<Vec<(Instant, Option<Shown>, f32)>> = const { RefCell::new(Vec::new()) };
}

/// A scan or a sync started, said how far it is, or ended: the indicator in step.
pub fn changed(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let (scan, sync) = (app.global::<Library>().get_scanning(), app.global::<Server>().get_busy());
    let (shown, wait) = INDICATOR.with(|i| i.borrow_mut().update(scan, sync, Instant::now()));
    let a = app.global::<Activity>();
    // The words of each job running (the one not shown may get its turn after it ended).
    let mut progress = -1.0;
    for (job, busy) in [(Job::Scan, scan), (Job::Sync, sync)] {
        if busy {
            let (title, text, p) = words(&app, job);
            if shown.is_some_and(|s| s.job == job) {
                progress = p;
            }
            WORDS.with(|w| w.borrow_mut()[job as usize] = (title, text));
        }
    }
    match shown {
        None => a.set_shown(false),
        Some(s) => {
            let was = a.get_shown() && a.get_sync() == (s.job == Job::Sync);
            if !s.done || !was {
                let (title, text) = WORDS.with(|w| w.borrow()[s.job as usize].clone());
                a.set_title(title.into());
                a.set_text(text.into());
            }
            a.set_sync(s.job == Job::Sync);
            a.set_done(s.done);
            // Done: the last words stay, the bar runs to full.
            a.set_progress(if s.done { 1.0 } else { progress });
            a.set_shown(true);
        }
    }
    #[cfg(feature = "bench")]
    LOG.with(|l| {
        let mut l = l.borrow_mut();
        if l.last().is_none_or(|(_, last, _)| *last != shown) {
            l.push((Instant::now(), shown, a.get_progress()));
        }
    });
    HOLD.with(|t| match wait {
        Some(d) => t.start(TimerMode::SingleShot, d, || {
            state::with(changed);
        }),
        None => t.stop(),
    });
}

/// What the job shown is doing and how far (0..1, below 0 while that cannot be told), from what
/// the sidebar and Settings show of it.
fn words(app: &AppWindow, job: Job) -> (String, String, f32) {
    match job {
        Job::Scan => {
            let l = app.global::<Library>();
            let title = match l.get_status_title().as_str() {
                // Removing a folder from the library (it has no title of its own).
                "" => "Updating the library".to_owned(),
                "Scanning" => "Scanning your folders".to_owned(),
                t => t.to_owned(),
            };
            let detail = l.get_status_detail();
            let text = if detail.is_empty() { l.get_status() } else { detail };
            (title, text.into(), l.get_progress())
        }
        Job::Sync => {
            let s = app.global::<Server>();
            (s.get_title().into(), s.get_status().into(), s.get_progress())
        }
    }
}

/// What was shown since the last `take_log`, with when and the bar's value (the bench).
#[cfg(feature = "bench")]
pub fn take_log() -> Vec<(Instant, Option<Shown>, f32)> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn shown(job: Job, done: bool) -> Option<Shown> {
        Some(Shown { job, done })
    }

    #[test]
    fn nothing_busy_shows_nothing_and_waits_for_nothing() {
        let mut i = Indicator::default();
        assert_eq!(i.update(false, false, Instant::now()), (None, None));
    }

    #[test]
    fn a_quick_scan_stays_its_least_time_and_ends_at_full() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        assert_eq!(i.update(true, false, t0), (shown(Job::Scan, false), None));
        // Over in 100 ms: held at full until 700 ms after it showed.
        assert_eq!(i.update(false, false, t0 + ms(100)), (shown(Job::Scan, true), Some(ms(600))));
        assert_eq!(i.update(false, false, t0 + ms(400)), (shown(Job::Scan, true), Some(ms(300))));
        assert_eq!(i.update(false, false, t0 + ms(700)), (None, None));
    }

    #[test]
    fn a_long_scan_is_held_at_full_a_moment_after_its_end() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        i.update(true, false, t0);
        assert_eq!(i.update(true, false, t0 + ms(5000)), (shown(Job::Scan, false), None));
        assert_eq!(i.update(false, false, t0 + ms(6000)), (shown(Job::Scan, true), Some(AT_FULL)));
        assert_eq!(i.update(false, false, t0 + ms(6000) + AT_FULL), (None, None));
    }

    #[test]
    fn a_scan_then_a_sync_show_one_after_the_other_without_a_gap() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        i.update(true, false, t0);
        // The sync was asked for meanwhile: it waits for the scan (both busy).
        assert_eq!(i.update(true, true, t0 + ms(200)), (shown(Job::Scan, false), None));
        // The scan ends: at full, the sync not shown yet, nothing hidden.
        assert_eq!(i.update(false, true, t0 + ms(1000)), (shown(Job::Scan, true), Some(AT_FULL)));
        // Then the sync, for its own least time.
        let at = t0 + ms(1000) + AT_FULL;
        assert_eq!(i.update(false, true, at), (shown(Job::Sync, false), None));
        assert_eq!(i.update(false, false, at + ms(100)), (shown(Job::Sync, true), Some(ms(600))));
        assert_eq!(i.update(false, false, at + ms(700)), (None, None));
    }

    #[test]
    fn a_scan_during_a_sync_is_shown_at_once_and_the_sync_comes_back() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        assert_eq!(i.update(false, true, t0), (shown(Job::Sync, false), None));
        assert_eq!(i.update(true, true, t0 + ms(2000)), (shown(Job::Scan, false), None));
        assert_eq!(i.update(false, true, t0 + ms(2100)), (shown(Job::Scan, true), Some(ms(600))));
        assert_eq!(i.update(false, true, t0 + ms(2700)), (shown(Job::Sync, false), None));
    }

    #[test]
    fn a_sync_over_during_the_scans_hold_still_gets_its_turn() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        i.update(true, true, t0);
        assert_eq!(i.update(false, true, t0 + ms(1000)), (shown(Job::Scan, true), Some(AT_FULL)));
        // The sync writes and ends within the hold: shown after it, at full, its least time.
        assert_eq!(i.update(false, false, t0 + ms(1200)), (shown(Job::Scan, true), Some(ms(250))));
        let at = t0 + ms(1000) + AT_FULL;
        assert_eq!(i.update(false, false, at), (shown(Job::Sync, true), Some(MIN_SHOWN)));
        assert_eq!(i.update(false, false, at + ms(300)), (shown(Job::Sync, true), Some(ms(400))));
        assert_eq!(i.update(false, false, at + MIN_SHOWN), (None, None));
    }

    #[test]
    fn a_rescan_asked_for_during_a_scan_follows_it_after_its_end_at_full() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        i.update(true, false, t0);
        // The scan ends and the next starts in the same moment (`scanning::next_job`).
        assert_eq!(i.update(false, false, t0 + ms(3000)), (shown(Job::Scan, true), Some(AT_FULL)));
        assert_eq!(i.update(true, false, t0 + ms(3000)), (shown(Job::Scan, true), Some(AT_FULL)));
        assert_eq!(i.update(true, false, t0 + ms(3000) + AT_FULL), (shown(Job::Scan, false), None));
    }

    #[test]
    fn a_job_ending_during_the_hold_does_not_shorten_or_stretch_it() {
        let t0 = Instant::now();
        let mut i = Indicator::default();
        i.update(false, true, t0);
        i.update(false, false, t0 + ms(50));
        // A late call (the hold's timer) after the release: gone.
        assert_eq!(i.update(false, false, t0 + ms(2000)), (None, None));
        assert_eq!(i.update(false, false, t0 + ms(2001)), (None, None));
    }
}
