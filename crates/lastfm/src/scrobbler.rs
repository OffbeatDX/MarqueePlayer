//! The scrobbler: one background thread that owns every network call, a queue of plays
//! kept on disk until Last.fm has them, and the credentials file.
//!
//! - `lastfm.json` (the data folder, `Options::config_dir`): the API key and shared secret the user entered, and
//!   the session key and user name once connected. Never in the database, never logged.
//!   The shared secret and session key are sealed (`Options::sealer`: the app's DPAPI on Windows, a file only
//!   the user can read on Linux); a file from before, with either in plain text, is written
//!   again sealed when read. A secret or session that does not open here is cleared.
//! - `scrobbles.json` (the same folder): plays not yet accepted, oldest first. A play is
//!   written there by the caller's thread before `scrobble` returns, so one made just
//!   before exit is not lost; the thread submits them in batches of up to 50.
//!
//! Errors: no connection or a Last.fm outage keeps the queue and tries again later (30 s,
//! doubling to 30 min; at once when the user asks). A play Last.fm ignores, or a request
//! it rejects for good, is dropped and logged; one past the account's daily limit is kept
//! and tried again at the longest retry. An invalid session stops submitting and
//! asks the user to reconnect; wrong credentials ask them to check the key and secret.
//!
//! The thread is never joined: dropping the `Scrobbler` closes its channel and the thread
//! ends after any request in flight; the app's exit never waits for the network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, BATCH, Client, Outcome, Scrobble};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
    pub session_key: String,
    pub username: String,
}

/// How the API secret and session key are kept on disk: the app passes its own (`platform::secret`: DPAPI for
/// the Windows user; on Linux a file only the user can read). `PLAIN` is for tests.
#[derive(Clone, Copy, Debug)]
pub struct Sealer {
    /// Named in the file: a file sealed another way does not open.
    pub scheme: &'static str,
    pub seal: fn(&[u8]) -> Option<Vec<u8>>,
    /// None when it cannot be opened here (another user or PC, damage).
    pub open: fn(&[u8]) -> Option<Vec<u8>>,
    /// Writes a file only this user should read.
    pub write: fn(&Path, &[u8]) -> std::io::Result<()>,
}

impl Sealer {
    /// Nothing sealed, an ordinary file.
    pub const PLAIN: Sealer = Sealer { scheme: "plain", seal: |b| Some(b.to_vec()), open: |b| Some(b.to_vec()), write: |p, b| std::fs::write(p, b) };
}

/// `lastfm.json` as stored: the API secret and session key sealed (`Sealer`), as hex. A file
/// from before carries either or both as plain `api_secret` / `session_key`: read, then
/// written again sealed.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    api_secret: String,
    username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    session_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    sealed_with: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    secret: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    session: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

fn open_sealed(sealed_with: &str, value: &str, sealer: &Sealer) -> Option<String> {
    (sealed_with == sealer.scheme).then(|| unhex(value)).flatten().and_then(|b| (sealer.open)(&b)).and_then(|p| String::from_utf8(p).ok())
}

impl Credentials {
    /// Read the file, and write it again sealed if it held the API secret or session key in plain text.
    pub fn load(path: &Path, sealer: &Sealer) -> Self {
        let (creds, plain) = Self::read(path, sealer);
        if plain {
            log::info!("{}: the API secret and session key are kept sealed from now on", path.display());
            creds.save(path, sealer);
        }
        creds
    }

    /// Read the file without writing it; true when it held the API secret or session key in plain
    /// text. A secret or session that does not open here (another PC or user, damage) is cleared;
    /// the API key is kept.
    pub fn read(path: &Path, sealer: &Sealer) -> (Self, bool) {
        let s: Stored = read_json(path).unwrap_or_default();
        let plain_secret = !s.api_secret.is_empty();
        let plain_session = !s.session_key.is_empty();
        let api_secret = if plain_secret {
            s.api_secret
        } else if s.secret.is_empty() {
            String::new()
        } else {
            open_sealed(&s.sealed_with, &s.secret, sealer).unwrap_or_else(|| {
                log::warn!("{}: the saved API secret cannot be opened here (made for another PC or user, or damaged); enter it again", path.display());
                String::new()
            })
        };
        let session_key = if plain_session {
            s.session_key
        } else if s.session.is_empty() {
            String::new()
        } else {
            open_sealed(&s.sealed_with, &s.session, sealer).unwrap_or_else(|| {
                log::warn!("{}: the saved session cannot be opened here (made for another PC or user, or damaged); connect again", path.display());
                String::new()
            })
        };
        let username = if session_key.is_empty() { String::new() } else { s.username };
        (Self { api_key: s.api_key, api_secret, session_key, username }, plain_secret || plain_session)
    }

    pub fn save(&self, path: &Path, sealer: &Sealer) {
        let mut s = Stored { api_key: self.api_key.clone(), username: self.username.clone(), ..Stored::default() };
        if !self.api_secret.is_empty() {
            let Some(sealed) = (sealer.seal)(self.api_secret.as_bytes()) else {
                log::warn!("{}: the API secret could not be sealed; not saved", path.display());
                return;
            };
            s.sealed_with = sealer.scheme.into();
            s.secret = hex(&sealed);
        }
        if !self.session_key.is_empty() {
            let Some(sealed) = (sealer.seal)(self.session_key.as_bytes()) else {
                log::warn!("{}: the session key could not be sealed; not saved", path.display());
                return;
            };
            s.sealed_with = sealer.scheme.into();
            s.session = hex(&sealed);
        }
        let text = serde_json::to_string_pretty(&s).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        let written = (sealer.write)(&tmp, text.as_bytes()).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            log::warn!("{} cannot be saved: {e}", path.display());
        }
    }
}

/// A JSON file of ours: `None` when it is not there. One that cannot be read (damaged, from a
/// newer build) is kept beside itself as `<name>.bad` before anything replaces it, and logged.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            let mut bad = path.file_name().unwrap_or_default().to_owned();
            bad.push(".bad");
            let _ = std::fs::copy(path, path.with_file_name(bad));
            log::error!("{} cannot be read ({e}); kept as .bad and started afresh", path.display());
            None
        }
    }
}

/// Written to a temporary file, flushed to the disk and renamed over the old one: a crash
/// leaves the old file or the new, never an empty one (the scrobbles waiting are in it).
fn write_atomic(path: &Path, text: &str) {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("{} cannot be saved: {e}", path.display());
    }
}

/// Plays waiting for Last.fm, oldest first, on disk.
pub struct Queue {
    path: PathBuf,
    items: Vec<Scrobble>,
}

impl Queue {
    pub fn load(path: &Path) -> Self {
        let items = read_json(path).unwrap_or_default();
        Self { path: path.to_owned(), items }
    }

    fn save(&self) {
        write_atomic(&self.path, &serde_json::to_string(&self.items).unwrap_or_default());
    }

    pub fn push(&mut self, s: Scrobble) {
        self.items.push(s);
        self.items.sort_by_key(|s| s.timestamp);
        self.save();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn oldest(&self, n: usize) -> Vec<Scrobble> {
        self.items.iter().take(n).cloned().collect()
    }

    /// Drop these, as `oldest` gave them (they were sent, or will never be accepted): each one
    /// once, wherever it is now. A play queued meanwhile can sort before them (the clock set
    /// back), so they are not always the oldest any more.
    pub fn remove(&mut self, sent: &[Scrobble]) {
        for s in sent {
            if let Some(i) = self.items.iter().position(|q| q == s) {
                self.items.remove(i);
            }
        }
        self.save();
    }
}

/// Where the connection stands, for Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// No API key and secret yet.
    NoCredentials,
    /// Key and secret entered, not connected.
    Disconnected,
    /// Waiting for the user to allow access on this page.
    Authorising { url: String },
    Connected { user: String },
    /// The session is no longer valid: connect again.
    Reconnect,
    /// Something the user must fix (the key or secret), or the last thing that went wrong.
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Plays waiting to be sent.
    pub queued: usize,
    /// The last thing that happened, for the status line ("Sent 3 scrobbles").
    pub note: String,
    /// Plays are taken (queued, to send now or once the user has acted): connected, or the
    /// session or the key refused since (until Disconnect or another key).
    pub accepting: bool,
}

enum Cmd {
    Credentials { key: String, secret: String },
    Connect,
    Cancel,
    Disconnect,
    NowPlaying(Scrobble),
    /// A play was queued: send it (and anything waiting) now.
    Flush,
    Retry,
}

/// Fills in the MusicBrainz recording id from the file's tags, on the scrobbler's thread.
pub type MbidLookup = Box<dyn Fn(&str) -> Option<String> + Send>;
pub type Notify = Box<dyn Fn(Status) + Send>;

pub struct Options {
    /// The API root (Last.fm's, or a mock server's in tests).
    pub root: String,
    /// The client name sent with each call; the app puts its version in it.
    pub agent: String,
    /// Where `lastfm.json` and `scrobbles.json` are kept (the app passes its data folder).
    pub config_dir: PathBuf,
    /// How often to ask whether the user has authorised the token.
    pub auth_poll: Duration,
    /// Give up on an authorisation after this long.
    pub auth_timeout: Duration,
    /// The first retry delay after a failure (doubles to `retry_max`).
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// How the API secret and session key are kept in `lastfm.json` (`Sealer::PLAIN` unless the app sets its own).
    pub sealer: Sealer,
}

impl Options {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            root: crate::api::API_ROOT.into(),
            agent: "MarqueePlayer".into(),
            config_dir: config_dir.to_owned(),
            auth_poll: Duration::from_secs(3),
            auth_timeout: Duration::from_secs(300),
            retry_min: Duration::from_secs(30),
            retry_max: Duration::from_secs(1800),
            sealer: Sealer::PLAIN,
        }
    }
}

pub struct Scrobbler {
    tx: Sender<Cmd>,
    queue: Arc<Mutex<Queue>>,
}

impl Scrobbler {
    /// Start the thread. `notify` is called (on that thread) whenever the status changes;
    /// `open_url` when the user must authorise in the browser.
    pub fn start(opts: Options, mbid: MbidLookup, notify: Notify, open_url: Box<dyn Fn(&str) + Send>) -> Self {
        let _ = std::fs::create_dir_all(&opts.config_dir);
        let queue = Arc::new(Mutex::new(Queue::load(&opts.config_dir.join("scrobbles.json"))));
        let (tx, rx) = unbounded();
        let q = queue.clone();
        std::thread::Builder::new()
            .name("mp-lastfm".into())
            .spawn(move || Worker::new(opts, q, mbid, notify, open_url).run(rx))
            .expect("spawn last.fm thread");
        Self { tx, queue }
    }

    pub fn set_credentials(&self, key: &str, secret: &str) {
        let _ = self.tx.send(Cmd::Credentials { key: key.trim().into(), secret: secret.trim().into() });
    }

    pub fn connect(&self) {
        let _ = self.tx.send(Cmd::Connect);
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    /// "Now playing". Sent only while connected.
    pub fn now_playing(&self, s: Scrobble) {
        let _ = self.tx.send(Cmd::NowPlaying(s));
    }

    /// Queue a play (written to disk before this returns) and send it when possible.
    /// Queued also while not connected: sent once connected.
    pub fn scrobble(&self, s: Scrobble) {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).push(s);
        let _ = self.tx.send(Cmd::Flush);
    }

    /// Try sending the queue now (instead of at the next retry).
    pub fn retry(&self) {
        let _ = self.tx.send(Cmd::Retry);
    }

    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

struct Worker {
    opts: Options,
    creds: Credentials,
    creds_path: PathBuf,
    queue: Arc<Mutex<Queue>>,
    mbid: MbidLookup,
    notify: Notify,
    open_url: Box<dyn Fn(&str) + Send>,
    state: State,
    note: String,
    /// After a failure: no sending before this.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Sending is held until the user changes something (bad credentials, invalid session).
    held: bool,
    /// Connected once and not disconnected since (`Status::accepting`).
    accepting: bool,
}

impl Worker {
    fn new(opts: Options, queue: Arc<Mutex<Queue>>, mbid: MbidLookup, notify: Notify, open_url: Box<dyn Fn(&str) + Send>) -> Self {
        let creds_path = opts.config_dir.join("lastfm.json");
        let creds = Credentials::load(&creds_path, &opts.sealer);
        let backoff = opts.retry_min;
        let accepting = !creds.api_key.is_empty() && !creds.api_secret.is_empty() && !creds.session_key.is_empty();
        let mut w = Self {
            opts,
            creds,
            creds_path,
            queue,
            mbid,
            notify,
            open_url,
            state: State::NoCredentials,
            note: String::new(),
            retry_at: None,
            backoff,
            held: false,
            accepting,
        };
        w.state = w.idle_state();
        w
    }

    fn idle_state(&self) -> State {
        if self.creds.api_key.is_empty() || self.creds.api_secret.is_empty() {
            State::NoCredentials
        } else if self.creds.session_key.is_empty() {
            State::Disconnected
        } else {
            State::Connected { user: self.creds.username.clone() }
        }
    }

    fn client(&self) -> Client {
        Client::new(&self.opts.root, &self.creds.api_key, &self.creds.api_secret, &self.opts.agent)
    }

    fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn report(&self) {
        (self.notify)(Status { state: self.state.clone(), queued: self.queued(), note: self.note.clone(), accepting: self.accepting });
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        self.report();
        self.send_queue();
        loop {
            let cmd = match self.retry_at {
                Some(at) if !self.held => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                _ => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
            };
            match cmd {
                None => {
                    self.retry_at = None;
                    self.send_queue();
                }
                Some(Cmd::Credentials { key, secret }) => {
                    self.set_credentials(key, secret);
                }
                Some(Cmd::Connect) => {
                    if self.connect(&rx) {
                        return;
                    }
                }
                Some(Cmd::Cancel) => {}
                Some(Cmd::Disconnect) => {
                    self.creds.session_key.clear();
                    self.creds.username.clear();
                    self.creds.save(&self.creds_path, &self.opts.sealer);
                    self.accepting = false;
                    self.state = self.idle_state();
                    self.note = "Disconnected".into();
                    self.report();
                }
                Some(Cmd::NowPlaying(mut s)) => {
                    if matches!(self.state, State::Connected { .. }) {
                        self.fill_mbid(std::slice::from_mut(&mut s));
                        match self.client().now_playing(&self.creds.session_key, &s) {
                            Ok(()) => log::debug!("last.fm: now playing sent"),
                            Err(e) => self.failed(&e, false),
                        }
                    }
                }
                Some(Cmd::Flush) => {
                    if self.retry_at.is_none() {
                        self.send_queue();
                    } else {
                        self.report();
                    }
                }
                Some(Cmd::Retry) => {
                    self.retry_at = None;
                    self.backoff = self.opts.retry_min;
                    self.send_queue();
                }
            }
        }
    }

    /// The key and secret the user entered; true if they changed.
    fn set_credentials(&mut self, key: String, secret: String) -> bool {
        if key == self.creds.api_key && secret == self.creds.api_secret {
            return false;
        }
        // Another key: the old session does not belong to it.
        self.creds = Credentials { api_key: key, api_secret: secret, ..Credentials::default() };
        self.creds.save(&self.creds_path, &self.opts.sealer);
        self.held = false;
        self.accepting = false;
        self.state = self.idle_state();
        self.note.clear();
        self.report();
        true
    }

    /// The MusicBrainz recording id from the file's tags, where not given.
    fn fill_mbid(&self, list: &mut [Scrobble]) {
        for s in list.iter_mut().filter(|s| s.mbid.is_none()) {
            s.mbid = s.path.as_deref().and_then(|p| (self.mbid)(p));
        }
    }

    /// Something failed: decide what follows. `queue`: the failure was sending the queue.
    fn failed(&mut self, e: &ApiError, queue: bool) {
        match e {
            ApiError::InvalidSession(_) => {
                log::warn!("last.fm: the session is no longer valid; asking to reconnect");
                self.creds.session_key.clear();
                self.creds.save(&self.creds_path, &self.opts.sealer);
                self.state = State::Reconnect;
                self.held = true;
            }
            ApiError::Credentials(code, m) => {
                log::warn!("last.fm: credentials refused (error {code})");
                self.state = State::Error(format!("Last.fm refused the API key or secret ({m}). Check them and connect again."));
                self.held = true;
            }
            e if e.is_transient() => {
                if queue {
                    log::info!("last.fm: {e}; trying again in {} s", self.backoff.as_secs());
                    self.retry_at = Some(Instant::now() + self.backoff);
                    self.backoff = (self.backoff * 2).min(self.opts.retry_max);
                    self.note = format!("Last.fm could not be reached ({e}); trying again later");
                } else {
                    log::info!("last.fm: now playing not sent: {e}");
                }
            }
            e => log::warn!("last.fm: request rejected: {e}"),
        }
        self.report();
    }

    /// Send the queue, oldest first, in batches, until it is empty or something fails.
    fn send_queue(&mut self) {
        if self.held || !matches!(self.state, State::Connected { .. }) {
            self.report();
            return;
        }
        let mut sent = 0usize;
        let mut dropped = 0usize;
        let mut limited = false;
        while !limited {
            let taken = self.queue.lock().unwrap_or_else(|e| e.into_inner()).oldest(BATCH);
            if taken.is_empty() {
                break;
            }
            let mut batch = taken.clone();
            self.fill_mbid(&mut batch);
            match self.client().scrobble(&self.creds.session_key, &batch) {
                Ok(outcomes) => {
                    let mut done = Vec::new();
                    for (o, s) in outcomes.iter().zip(&taken) {
                        match o {
                            Outcome::Accepted => sent += 1,
                            Outcome::Ignored(why) => {
                                dropped += 1;
                                log::warn!("last.fm: a scrobble was ignored ({why}); dropped");
                            }
                            Outcome::Limited => {
                                limited = true;
                                continue;
                            }
                        }
                        done.push(s.clone());
                    }
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&done);
                    self.backoff = self.opts.retry_min;
                }
                Err(ApiError::Rejected(code, m)) if batch.len() > 1 => {
                    // One bad play must not cost the others: send them one at a time.
                    log::warn!("last.fm: batch rejected (error {code}: {m}); sending one by one");
                    match self.send_one_by_one(batch.len(), &mut sent, &mut dropped) {
                        Some(false) => {}
                        Some(true) => limited = true,
                        None => break,
                    }
                }
                Err(ApiError::Rejected(code, m)) => {
                    log::warn!("last.fm: scrobble rejected (error {code}: {m}); dropped");
                    dropped += 1;
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&taken);
                }
                Err(e) => {
                    self.failed(&e, true);
                    return;
                }
            }
        }
        if sent > 0 || dropped > 0 {
            self.note = match (sent, dropped) {
                (s, 0) => format!("Sent {s} scrobble{}", if s == 1 { "" } else { "s" }),
                (s, d) => format!("Sent {s}, {d} not accepted by Last.fm"),
            };
            log::info!("last.fm: {}", self.note);
        }
        if limited {
            // Last.fm counts the limit over a day: the plays past it are kept and tried again
            // at the longest retry, not at once.
            self.retry_at = Some(Instant::now() + self.opts.retry_max);
            self.note = format!("{}Last.fm's daily scrobble limit is reached; the rest are kept and sent later", if self.note.is_empty() { String::new() } else { format!("{}. ", self.note) });
            log::info!("last.fm: the daily scrobble limit is reached; {} kept, trying again in {} min", self.queued(), self.opts.retry_max.as_secs() / 60);
        }
        self.report();
    }

    /// Some(true): stopped at the daily limit; None: stopped on a failure.
    fn send_one_by_one(&mut self, n: usize, sent: &mut usize, dropped: &mut usize) -> Option<bool> {
        for _ in 0..n {
            let one = self.queue.lock().unwrap_or_else(|e| e.into_inner()).oldest(1);
            if one.is_empty() {
                return Some(false);
            }
            match self.client().scrobble(&self.creds.session_key, &one) {
                Ok(o) if o.first() == Some(&Outcome::Limited) => return Some(true),
                Ok(o) => {
                    if o.first() == Some(&Outcome::Accepted) {
                        *sent += 1;
                    } else {
                        *dropped += 1;
                    }
                }
                Err(ApiError::Rejected(code, m)) => {
                    log::warn!("last.fm: scrobble rejected (error {code}: {m}); dropped");
                    *dropped += 1;
                }
                Err(e) => {
                    self.failed(&e, true);
                    return None;
                }
            }
            self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&one);
        }
        Some(false)
    }

    /// The desktop authentication flow: a token, the browser, then the session once the
    /// user allowed it. Returns true if the channel closed (the app is exiting).
    fn connect(&mut self, rx: &Receiver<Cmd>) -> bool {
        if self.creds.api_key.is_empty() || self.creds.api_secret.is_empty() {
            self.state = State::NoCredentials;
            self.report();
            return false;
        }
        let client = self.client();
        let token = match client.get_token() {
            Ok(t) => t,
            Err(e) => {
                self.state = match &e {
                    ApiError::Credentials(_, m) => State::Error(format!("Last.fm refused the API key or secret ({m}).")),
                    e => State::Error(format!("Could not connect: {e}")),
                };
                self.report();
                return false;
            }
        };
        let url = client.auth_url(&token);
        self.state = State::Authorising { url: url.clone() };
        self.note.clear();
        self.report();
        (self.open_url)(&url);
        let deadline = Instant::now() + self.opts.auth_timeout;
        loop {
            match rx.recv_timeout(self.opts.auth_poll) {
                Ok(Cmd::Cancel) | Ok(Cmd::Disconnect) => {
                    self.state = self.idle_state();
                    self.report();
                    return false;
                }
                // Another key or secret: this token was asked for with the old one. Kept, and
                // the wait given up (a Connect after it starts again with the new one).
                Ok(Cmd::Credentials { key, secret }) => {
                    if self.set_credentials(key, secret) {
                        return false;
                    }
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => return true,
                Err(RecvTimeoutError::Timeout) => {}
            }
            match client.get_session(&token) {
                Ok((user, key)) => {
                    log::info!("last.fm: connected");
                    self.creds.session_key = key;
                    self.creds.username = user.clone();
                    self.creds.save(&self.creds_path, &self.opts.sealer);
                    self.state = State::Connected { user };
                    self.held = false;
                    self.accepting = true;
                    self.retry_at = None;
                    self.note = "Connected".into();
                    self.send_queue();
                    return false;
                }
                Err(ApiError::NotAuthorised) | Err(ApiError::Network(_)) | Err(ApiError::Transient(..)) if Instant::now() < deadline => {}
                Err(e) => {
                    self.state = State::Error(match e {
                        ApiError::NotAuthorised | ApiError::TokenExpired => "Access was not allowed in time. Connect again to retry.".into(),
                        e => format!("Could not connect: {e}"),
                    });
                    self.report();
                    return false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the app's sealing: reversible, and not the plain text.
    const XOR: Sealer = Sealer {
        scheme: "xor-test",
        seal: |b| Some(b.iter().map(|c| c ^ 0x5a).collect()),
        open: |b| (b.first() != Some(&0)).then(|| b.iter().map(|c| c ^ 0x5a).collect()),
        write: |p, b| std::fs::write(p, b),
    };

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mp-lastfm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn what_was_sent_is_dropped_even_when_an_earlier_play_came_in_meanwhile() {
        let d = dir("queue");
        let s = |t: i64, name: &str| Scrobble { artist: "A".into(), track: name.into(), timestamp: t, ..Scrobble::default() };
        let mut q = Queue::load(&d.join("scrobbles.json"));
        q.push(s(20, "b"));
        q.push(s(30, "c"));
        let sent = q.oldest(2);
        // The clock stepped back: a play made while those were on their way sorts before them.
        q.push(s(10, "a"));
        q.remove(&sent);
        assert_eq!(q.oldest(9), [s(10, "a")], "the new play is kept, and neither sent one is sent again");
        assert_eq!(Queue::load(&d.join("scrobbles.json")).len(), 1, "on disk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_api_secret_and_session_key_are_kept_sealed_and_plain_ones_are_sealed_when_read() {
        let d = dir("seal");
        let p = d.join("lastfm.json");
        // Migration from a fixture with both fields in plain text.
        std::fs::write(&p, r#"{"api_key":"k","api_secret":"sec-plain","session_key":"sk-plain","username":"ann"}"#).unwrap();
        let c = Credentials::load(&p, &XOR);
        assert_eq!((c.api_key.as_str(), c.api_secret.as_str(), c.session_key.as_str(), c.username.as_str()), ("k", "sec-plain", "sk-plain", "ann"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("sec-plain") && !text.contains("sk-plain") && text.contains("xor-test"), "written again sealed: {text}");
        assert_eq!(Credentials::read(&p, &XOR), (c.clone(), false), "reads back the same, nothing plain");
        assert_eq!(Credentials::load(&p, &XOR), c, "file written by new code still loads");

        // Migration from an intermediate file: session already sealed, api_secret still plain.
        let sealed_sk = hex(&(XOR.seal)(b"sk-sealed").unwrap());
        std::fs::write(&p, format!(r#"{{"api_key":"k","api_secret":"sec-old","username":"ann","sealed_with":"xor-test","session":"{sealed_sk}"}}"#)).unwrap();
        let (c_mid, plain_mid) = Credentials::read(&p, &XOR);
        assert!(plain_mid, "plain api_secret triggers migration");
        assert_eq!((c_mid.api_secret.as_str(), c_mid.session_key.as_str()), ("sec-old", "sk-sealed"));
        let c_mid = Credentials::load(&p, &XOR);
        assert_eq!((c_mid.api_secret.as_str(), c_mid.session_key.as_str()), ("sec-old", "sk-sealed"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("sec-old") && !text.contains("sk-sealed") && text.contains("xor-test"), "migrated: {text}");
        assert_eq!(Credentials::read(&p, &XOR), (c_mid, false));

        // Round trip: saving new secret and session key seals both.
        let updated = Credentials { api_key: "k2".into(), api_secret: "sec-new".into(), session_key: "sk-new".into(), username: "bob".into() };
        updated.save(&p, &XOR);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("sec-new") && !text.contains("sk-new"), "both sealed on save: {text}");
        assert_eq!(Credentials::read(&p, &XOR), (updated.clone(), false));
        assert_eq!(Credentials::load(&p, &XOR), updated);

        // Round trip with api_secret only (before connecting, no session key yet).
        let creds_only = Credentials { api_key: "k3".into(), api_secret: "sec-only".into(), session_key: String::new(), username: String::new() };
        creds_only.save(&p, &XOR);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("sec-only") && text.contains("xor-test"), "secret sealed even without session: {text}");
        assert_eq!(Credentials::read(&p, &XOR), (creds_only.clone(), false));
        assert_eq!(Credentials::load(&p, &XOR), creds_only);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sealed_credentials_that_do_not_open_here_are_cleared() {
        let d = dir("locked");
        let p = d.join("lastfm.json");
        Credentials { api_key: "k".into(), api_secret: "s".into(), session_key: "sk".into(), username: "ann".into() }.save(&p, &XOR);
        // Sealed another way (another system's build): secret and session cleared, API key kept.
        let (c, plain) = Credentials::read(&p, &Sealer::PLAIN);
        assert!(!plain);
        assert_eq!((c.api_key.as_str(), c.api_secret.as_str(), c.session_key.as_str(), c.username.as_str()), ("k", "", "", ""));
        // A seal that does not open (another user or PC), and one damaged.
        std::fs::write(&p, r#"{"api_key":"k","username":"ann","sealed_with":"xor-test","secret":"00ff","session":"00ff"}"#).unwrap();
        let c = Credentials::load(&p, &XOR);
        assert_eq!((c.api_key.as_str(), c.api_secret.as_str(), c.session_key.as_str()), ("k", "", ""));
        std::fs::write(&p, r#"{"api_key":"k","username":"ann","sealed_with":"xor-test","secret":"zz","session":"zz"}"#).unwrap();
        let c = Credentials::load(&p, &XOR);
        assert_eq!((c.api_key.as_str(), c.api_secret.as_str(), c.session_key.as_str()), ("k", "", ""));
        assert!(!d.join("lastfm.json.bad").exists(), "not a damaged file: nothing kept aside");
        // If sealing itself fails, nothing is written in plain text.
        let broken = Sealer { scheme: "broken", seal: |_| None, open: |_| None, write: |p, b| std::fs::write(p, b) };
        let p2 = d.join("unsealed.json");
        Credentials { api_key: "k".into(), api_secret: "plain-secret".into(), session_key: String::new(), username: String::new() }.save(&p2, &broken);
        assert!(!p2.exists(), "a failed seal never writes a plain secret");
        let _ = std::fs::remove_dir_all(&d);
    }
}
