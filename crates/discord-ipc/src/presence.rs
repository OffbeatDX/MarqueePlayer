//! The presence thread: it owns the pipe to Discord and keeps what the profile shows in
//! step with what the app last asked for.
//!
//! - Off, or without an application ID: no pipe is opened, and an open one is closed
//!   (which clears the profile).
//! - Discord not running: tried again every `retry`; the activity wanted is shown once it
//!   answers.
//! - The pipe breaks (Discord quit or restarted): noticed at the next activity sent, then
//!   as above. So is a Discord that does not answer within `ipc::TIMEOUT` (starting,
//!   frozen, or a bridge that never replies): the thread never waits on it for longer.
//! - An application ID Discord does not know: said, and not tried again until it changes.
//!
//! Discord takes five activity updates in twenty seconds. Only the latest one wanted is
//! ever sent, and no more than `burst` within `window`: skipping through tracks shows the
//! one that stays.
//!
//! The thread is never joined: dropping the `Presence` closes its channel and the thread
//! ends, and with it the pipe. The app's exit never waits for Discord.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::ipc::{self, Activity, Error, Pipe, Session};

/// Where the connection stands, for Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    /// On, without an application ID yet.
    NoId,
    /// Discord is not running here, or not answering: tried again by itself.
    Waiting,
    /// `user` is who is signed in to Discord there ("" when it did not say).
    Connected { user: String },
    /// Something the user must fix (the application ID).
    Refused(String),
}

/// Opens the pipe to Discord (`ipc::system`, or a mock's in tests).
pub type Connect = Box<dyn Fn() -> io::Result<Box<dyn Pipe>> + Send>;
pub type Notify = Box<dyn Fn(State) + Send>;

pub struct Options {
    pub connect: Connect,
    /// How often to look for Discord while it is not there.
    pub retry: Duration,
    /// At most `burst` activities sent within `window`.
    pub burst: usize,
    pub window: Duration,
}

impl Options {
    /// The Discord app on this machine.
    pub fn system() -> Self {
        Self { connect: Box::new(ipc::system), retry: Duration::from_secs(15), burst: 4, window: Duration::from_secs(20) }
    }
}

enum Cmd {
    Configure { enabled: bool, app_id: String },
    Activity(Option<Activity>),
}

pub struct Presence {
    tx: Sender<Cmd>,
}

impl Presence {
    /// Start the thread, off. `notify` is called (on that thread) whenever the state changes.
    pub fn start(opts: Options, notify: Notify) -> Self {
        let (tx, rx) = unbounded();
        std::thread::Builder::new().name("mp-discord".into()).spawn(move || Worker::new(opts, notify).run(&rx)).expect("spawn discord thread");
        Self { tx }
    }

    /// The setting and the application ID, at start and whenever either changes.
    pub fn configure(&self, enabled: bool, app_id: &str) {
        let _ = self.tx.send(Cmd::Configure { enabled, app_id: app_id.trim().to_owned() });
    }

    /// What plays now; `None` when nothing does (paused, stopped).
    pub fn set_activity(&self, activity: Option<Activity>) {
        let _ = self.tx.send(Cmd::Activity(activity));
    }
}

/// An application ID is a number (a Discord snowflake).
pub fn valid_id(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

struct Worker {
    opts: Options,
    notify: Notify,
    enabled: bool,
    app_id: String,
    /// The application ID was refused: nothing is tried until it changes.
    refused: bool,
    session: Option<Session>,
    /// What the app wants shown, and what Discord was last told.
    wanted: Option<Activity>,
    shown: Option<Activity>,
    /// When to look for Discord again.
    retry_at: Instant,
    sent: VecDeque<Instant>,
    state: State,
}

impl Worker {
    fn new(opts: Options, notify: Notify) -> Self {
        Self { opts, notify, enabled: false, app_id: String::new(), refused: false, session: None, wanted: None, shown: None, retry_at: Instant::now(), sent: VecDeque::new(), state: State::Off }
    }

    fn run(mut self, rx: &Receiver<Cmd>) {
        loop {
            let cmd = match self.wake_in() {
                Some(wait) => match rx.recv_timeout(wait) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
            };
            // Everything asked for so far: only the latest activity matters.
            for c in cmd.into_iter().chain(rx.try_iter()) {
                self.apply(c);
            }
            self.step();
        }
    }

    fn active(&self) -> bool {
        self.enabled && !self.app_id.is_empty() && !self.refused
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Activity(a) => self.wanted = a,
            Cmd::Configure { enabled, app_id } => {
                if app_id != self.app_id {
                    // Another application: the open session is the old one's.
                    self.close();
                    self.refused = false;
                    self.app_id = app_id;
                }
                self.enabled = enabled;
                self.retry_at = Instant::now();
            }
        }
    }

    fn close(&mut self) {
        self.session = None;
        self.shown = None;
    }

    fn set_state(&mut self, state: State) {
        if state != self.state {
            self.state = state.clone();
            (self.notify)(state);
        }
    }

    /// How long until there is something to do without being asked; `None`: nothing.
    fn wake_in(&mut self) -> Option<Duration> {
        if !self.active() {
            return None;
        }
        let now = Instant::now();
        if self.session.is_none() {
            return Some(self.retry_at.saturating_duration_since(now));
        }
        if self.wanted == self.shown {
            return None;
        }
        Some(self.may_send_at(now).saturating_duration_since(now))
    }

    /// When the next activity may go out: now, or once the oldest of the last `burst` is
    /// `window` old.
    fn may_send_at(&mut self, now: Instant) -> Instant {
        while self.sent.front().is_some_and(|t| now.duration_since(*t) >= self.opts.window) {
            self.sent.pop_front();
        }
        if self.sent.len() < self.opts.burst { now } else { self.sent[0] + self.opts.window }
    }

    fn step(&mut self) {
        if !self.enabled {
            self.close();
            return self.set_state(State::Off);
        }
        if self.app_id.is_empty() {
            self.close();
            return self.set_state(State::NoId);
        }
        if !self.refused && !valid_id(&self.app_id) {
            self.refused = true;
            return self.set_state(State::Refused("An application ID is a number of 17 to 20 digits.".into()));
        }
        if self.refused {
            return;
        }
        let now = Instant::now();
        if self.session.is_none() {
            if now < self.retry_at {
                return;
            }
            match (self.opts.connect)().map_err(Error::Io).and_then(|pipe| Session::open(pipe, &self.app_id)) {
                Ok(s) => {
                    log::info!("Discord: connected");
                    let user = s.user.clone();
                    self.session = Some(s);
                    self.shown = None;
                    self.set_state(State::Connected { user });
                }
                Err(e) => return self.failed(&e, now),
            }
        }
        if self.wanted != self.shown && self.may_send_at(now) <= now {
            let wanted = self.wanted.clone();
            let Some(session) = self.session.as_mut() else { return };
            match session.set_activity(wanted.as_ref()) {
                Ok(()) => {
                    self.sent.push_back(now);
                    self.shown = wanted;
                }
                Err(e) => self.failed(&e, now),
            }
        }
    }

    fn failed(&mut self, e: &Error, now: Instant) {
        self.close();
        match e {
            Error::InvalidId => {
                log::warn!("Discord: {e}");
                self.refused = true;
                self.set_state(State::Refused("Discord does not know this application ID. Check it against the one on the application's page.".into()));
            }
            _ => {
                // Discord not running is the ordinary case: said once, not every retry.
                if self.state != State::Waiting {
                    log::info!("Discord: not connected ({e}); looking again every {} s", self.opts.retry.as_secs());
                }
                self.retry_at = now + self.opts.retry;
                self.set_state(State::Waiting);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_id_is_a_number() {
        assert!(valid_id("1234567890123456789"));
        assert!(!valid_id("12345"));
        assert!(!valid_id("12345678901234567a"));
        assert!(!valid_id(""));
    }
}
