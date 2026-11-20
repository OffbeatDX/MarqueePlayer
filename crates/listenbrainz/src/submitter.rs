//! The submitter: one background thread that owns every request to ListenBrainz, a queue of
//! listens kept on disk until ListenBrainz has them, and the account file.
//!
//! - `listenbrainz.json` (data folder): the server address (empty: ListenBrainz's own), the user
//!   name and the token, sealed (`Options::sealer`: the app's DPAPI on Windows, a file only the
//!   user can read on Linux). A file with the token in plain text (written by hand, or by the
//!   bench) is written again sealed when read; a token that does not open here is no token
//!   (disconnected). Never in the database, never logged.
//! - `listens.json` (data folder): listens not yet accepted, oldest first. A listen is written
//!   there by the caller's thread before `submit` returns, so one made just before exit is not
//!   lost; the thread sends them in batches (`import`, at most 1000 and 10 MB a request, as
//!   documented; `single` for one).
//!
//! **No duplicates.** A listen leaves the queue only once ListenBrainz answered 200 for it. Its
//! MusicBrainz ids are read from the file and written into the queue before it is first sent,
//! so a listen sent again (its answer lost to a timeout or a dropped connection) is the same
//! request, byte for byte: the same user, `listened_at` and metadata, which ListenBrainz stores
//! once (its listen store keeps one listen per user, time and recording; a listen submitted
//! again is not added twice). A listen is queued once per play (the app's rule), and one queued
//! twice (the same time, artist and title) is kept once.
//!
//! **Errors.** No connection, a 5xx or an unexpected answer (a 404 at an address that is not a
//! ListenBrainz API) keeps the queue and tries again later (30 s, doubling to 30 min; at once
//! when the user asks). 429 waits as long as `X-RateLimit-Reset-In` says, then sends again, no
//! backoff; and every answer's `X-RateLimit-Remaining` / `X-RateLimit-Reset-In` are honoured
//! before the next request, which is never sooner than a second after the last (the documented
//! "one call per second"). A 400 for a batch halves it until the listen it refuses is alone;
//! that one is dropped with a log line, the others sent. A listen that can never be sent (no
//! artist or title, a time before ListenBrainz's minimum, over 10 KB) is dropped the same way. A
//! 401 (the token no longer accepted) stops sending and asks to connect again; the queue stays.
//!
//! The thread is never joined: dropping the `Submitter` closes its channel and the thread ends
//! after any request in flight; the app's exit never waits for the network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{self, ApiError, Client, Ids, Listen, ListenType, Player, limits};

/// `listenbrainz.json`.
pub const ACCOUNT_FILE: &str = "listenbrainz.json";
/// `listens.json`.
pub const QUEUE_FILE: &str = "listens.json";

/// How the token is kept on disk: the app passes its own (`platform::secret`), as it does for
/// Last.fm's session key. `PLAIN` is for tests.
#[derive(Clone, Copy, Debug)]
pub struct Sealer {
    /// Named in the file: a file sealed another way does not open (disconnected).
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

/// The account: where, who, and the token (empty: not connected).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Account {
    /// The API root as the user set it (`api::normalize_server`); empty: ListenBrainz's own.
    pub server: String,
    pub user: String,
    pub token: String,
}

impl std::fmt::Debug for Account {
    /// Never the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account").field("server", &self.server).field("user", &self.user).field("token", &if self.token.is_empty() { "" } else { "(set)" }).finish()
    }
}

/// `listenbrainz.json` as stored: the token sealed (`Sealer`), as hex. A plain `token` is read,
/// then written again sealed.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    #[serde(skip_serializing_if = "String::is_empty")]
    server: String,
    user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    sealed_with: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    sealed_token: String,
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

impl Account {
    /// The API root requests go to.
    pub fn root(&self) -> &str {
        if self.server.is_empty() { api::API_ROOT } else { &self.server }
    }

    /// Read the file, and write it again sealed if it held the token in plain text.
    pub fn load(path: &Path, sealer: &Sealer) -> Self {
        let (a, plain) = Self::read(path, sealer);
        if plain {
            log::info!("{}: the token is kept sealed from now on", path.display());
            a.save(path, sealer);
        }
        a
    }

    /// Read the file without writing it; true when it held the token in plain text. A token
    /// that does not open here (another PC or user, damage) is no token: disconnected, the
    /// server address kept.
    pub fn read(path: &Path, sealer: &Sealer) -> (Self, bool) {
        let s: Stored = read_json(path).unwrap_or_default();
        let plain = !s.token.trim().is_empty();
        let token = if plain {
            s.token.trim().to_owned()
        } else if s.sealed_token.is_empty() {
            String::new()
        } else {
            let opened = (s.sealed_with == sealer.scheme).then(|| unhex(&s.sealed_token)).flatten().and_then(|b| (sealer.open)(&b)).and_then(|p| String::from_utf8(p).ok());
            opened.unwrap_or_else(|| {
                log::warn!("{}: the saved token cannot be opened here (made for another PC or user, or damaged); connect again", path.display());
                String::new()
            })
        };
        let user = if token.is_empty() { String::new() } else { s.user };
        (Self { server: s.server, user, token }, plain)
    }

    pub fn save(&self, path: &Path, sealer: &Sealer) {
        let mut s = Stored { server: self.server.clone(), user: self.user.clone(), ..Stored::default() };
        if !self.token.is_empty() {
            let Some(sealed) = (sealer.seal)(self.token.as_bytes()) else {
                log::warn!("{}: the token could not be sealed; not saved", path.display());
                return;
            };
            s.sealed_with = sealer.scheme.into();
            s.sealed_token = hex(&sealed);
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
/// leaves the old file or the new, never an empty one (the listens waiting are in it).
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

/// Listens waiting for ListenBrainz, oldest first, on disk.
pub struct Queue {
    path: PathBuf,
    items: Vec<Listen>,
}

impl Queue {
    pub fn load(path: &Path) -> Self {
        let items = read_json(path).unwrap_or_default();
        Self { path: path.to_owned(), items }
    }

    fn save(&self) {
        write_atomic(&self.path, &serde_json::to_string(&self.items).unwrap_or_default());
    }

    /// Add a listen, in time order; one already queued (the same time, artist and title) is not
    /// added again.
    pub fn push(&mut self, l: Listen) {
        if self.items.iter().any(|q| q.listened_at == l.listened_at && q.artist == l.artist && q.track == l.track) {
            log::info!("listenbrainz: that listen is queued already");
            return;
        }
        let at = self.items.partition_point(|q| q.listened_at <= l.listened_at);
        self.items.insert(at, l);
        self.save();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn oldest(&self, n: usize) -> Vec<Listen> {
        self.items.iter().take(n).cloned().collect()
    }

    /// Whether `q` is `l`: the same time, artist and title (one of each is queued).
    fn same(q: &Listen, l: &Listen) -> bool {
        q.listened_at == l.listened_at && q.artist == l.artist && q.track == l.track
    }

    /// Listens `oldest` gave, as they will be sent (their ids filled in), each written back over
    /// itself. A listen queued meanwhile can sort before them (the clock set back), so they are
    /// found by who they are, not where they were.
    fn update(&mut self, listens: &[Listen]) {
        for l in listens {
            if let Some(q) = self.items.iter_mut().find(|q| Self::same(q, l)) {
                *q = l.clone();
            }
        }
        self.save();
    }

    /// Drop these (they were accepted, or will never be), wherever they are now.
    pub fn remove(&mut self, listens: &[Listen]) {
        self.items.retain(|q| !listens.iter().any(|l| Self::same(q, l)));
        self.save();
    }
}

/// Where the connection stands, for Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// No token.
    Disconnected,
    /// Checking a token.
    Connecting,
    Connected { user: String },
    /// The token is no longer accepted: connect again (the queue is kept).
    Reconnect,
    /// Connecting did not work: why, in words.
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Listens waiting to be sent.
    pub queued: usize,
    /// The last thing that happened, for the status line ("Sent 3 listens").
    pub note: String,
    /// Listens are taken (queued, to send now or once connected again): connected, or the token
    /// refused since (until Disconnect).
    pub accepting: bool,
    /// The server's words when it accepts the token but refuses the listens (an account
    /// without a verified e-mail address); empty otherwise. The listens are kept.
    pub refused: String,
}

enum Cmd {
    Connect { token: String, server: String },
    Disconnect,
    NowPlaying(Listen),
    /// A listen was queued: send it (and anything waiting) now.
    Flush,
    Retry,
    /// Feedback on a recording (1 loved, 0 none): by its id, or the file to read it from.
    Feedback { recording: Option<String>, path: Option<String>, score: i8 },
}

/// The MusicBrainz ids of a file's tags, read on the submitter's thread.
pub type IdLookup = Box<dyn Fn(&str) -> Ids + Send>;
pub type Notify = Box<dyn Fn(Status) + Send>;

pub struct Options {
    /// The API root for every request, whatever the account's server says (a mock in tests and
    /// the bench). None: the account's server, else ListenBrainz's own.
    pub root: Option<String>,
    /// The `User-Agent`; the app puts its version in it.
    pub agent: String,
    /// `media_player` / `submission_client` and their version.
    pub player: String,
    pub version: String,
    pub data_dir: PathBuf,
    /// The first retry delay after a failure (doubles to `retry_max`).
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// The least time between two requests (the documented one per second).
    pub min_gap: Duration,
    /// Listens per request and bytes per request (the documented maximums).
    pub max_listens: usize,
    pub max_payload: usize,
    /// How the token is kept in `listenbrainz.json` (`Sealer::PLAIN` unless the app sets its own).
    pub sealer: Sealer,
}

impl Options {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: None,
            agent: "MarqueePlayer".into(),
            player: "Marquee Player".into(),
            version: String::new(),
            data_dir: data_dir.to_owned(),
            retry_min: Duration::from_secs(30),
            retry_max: Duration::from_secs(1800),
            min_gap: Duration::from_secs(1),
            max_listens: limits::MAX_LISTENS_PER_REQUEST,
            max_payload: limits::MAX_LISTEN_PAYLOAD_SIZE,
            sealer: Sealer::PLAIN,
        }
    }
}

pub struct Submitter {
    tx: Sender<Cmd>,
    queue: Arc<Mutex<Queue>>,
}

impl Submitter {
    /// Start the thread. `notify` is called (on that thread) whenever the status changes.
    pub fn start(opts: Options, ids: IdLookup, notify: Notify) -> Self {
        let _ = std::fs::create_dir_all(&opts.data_dir);
        let queue = Arc::new(Mutex::new(Queue::load(&opts.data_dir.join(QUEUE_FILE))));
        let (tx, rx) = unbounded();
        let q = queue.clone();
        std::thread::Builder::new().name("mp-listenbrainz".into()).spawn(move || Worker::new(opts, q, ids, notify).run(rx)).expect("spawn listenbrainz thread");
        Self { tx, queue }
    }

    /// Check `token` against the server (`server` as typed, `api::normalize_server`; empty:
    /// ListenBrainz's own) and connect as its user.
    pub fn connect(&self, token: &str, server: &str) {
        let _ = self.tx.send(Cmd::Connect { token: token.trim().into(), server: server.into() });
    }

    /// Forget the token (the server address and the queue stay).
    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    /// "Now playing". Sent only while connected, and only when the rate limit allows it now.
    pub fn now_playing(&self, l: Listen) {
        let _ = self.tx.send(Cmd::NowPlaying(l));
    }

    /// Queue a listen (written to disk before this returns) and send it when possible.
    pub fn submit(&self, l: Listen) {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).push(l);
        let _ = self.tx.send(Cmd::Flush);
    }

    /// Try sending the queue now (instead of at the next retry).
    pub fn retry(&self) {
        let _ = self.tx.send(Cmd::Retry);
    }

    /// Love (`score` 1) or no feedback (0) for a recording, by its MusicBrainz id or the file to
    /// read it from. Best effort: kept until it could be sent while the app runs, not on disk.
    pub fn feedback(&self, recording: Option<String>, path: Option<String>, score: i8) {
        let _ = self.tx.send(Cmd::Feedback { recording, path, score });
    }

    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// When the next request may go: a second after the last (`min_gap`), and not before a rate
/// limit's window ends.
struct Pacer {
    min_gap: Duration,
    last: Option<Instant>,
    not_before: Option<Instant>,
}

impl Pacer {
    /// How long until a request may go (zero: now).
    fn wait(&self) -> Duration {
        let now = Instant::now();
        let gap = self.last.map(|l| (l + self.min_gap).saturating_duration_since(now)).unwrap_or_default();
        let limit = self.not_before.map(|t| t.saturating_duration_since(now)).unwrap_or_default();
        gap.max(limit)
    }

    /// A request was answered with these rate-limit headers.
    fn answered(&mut self, rate: http_client::RateLimit) {
        let now = Instant::now();
        self.last = Some(now);
        self.not_before = match (rate.remaining, rate.reset_in) {
            (Some(0), Some(s)) => Some(now + Duration::from_secs(s.min(3600))),
            _ => self.not_before.filter(|t| *t > now),
        };
    }

    /// 429: nothing before `d` from now.
    fn hold(&mut self, d: Duration) {
        self.not_before = Some(Instant::now() + d);
    }
}

struct Worker {
    opts: Options,
    account: Account,
    account_path: PathBuf,
    queue: Arc<Mutex<Queue>>,
    ids: IdLookup,
    notify: Notify,
    state: State,
    note: String,
    refused: String,
    /// After a failure: no sending before this.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Sending is held until the user connects again (the token refused).
    held: bool,
    pacer: Pacer,
    /// Feedback not sent yet (rate limit, offline), oldest first.
    feedback: Vec<(Option<String>, Option<String>, i8)>,
    /// Connected once and not disconnected since (`Status::accepting`).
    accepting: bool,
}

/// A request may go now, after a wait this short (slept on the thread); longer, it waits for the
/// loop instead, so a command (Disconnect) is never kept waiting.
const SHORT_WAIT: Duration = Duration::from_millis(1500);

impl Worker {
    fn new(opts: Options, queue: Arc<Mutex<Queue>>, ids: IdLookup, notify: Notify) -> Self {
        let account_path = opts.data_dir.join(ACCOUNT_FILE);
        let account = Account::load(&account_path, &opts.sealer);
        let backoff = opts.retry_min;
        let pacer = Pacer { min_gap: opts.min_gap, last: None, not_before: None };
        let accepting = !account.token.is_empty();
        let mut w = Self { opts, account, account_path, queue, ids, notify, state: State::Disconnected, note: String::new(), refused: String::new(), retry_at: None, backoff, held: false, pacer, feedback: Vec::new(), accepting };
        w.state = w.idle_state();
        w
    }

    fn idle_state(&self) -> State {
        if self.account.token.is_empty() { State::Disconnected } else { State::Connected { user: self.account.user.clone() } }
    }

    fn client(&self) -> Client {
        let root = self.opts.root.as_deref().unwrap_or_else(|| self.account.root());
        Client::new(root, &self.account.token, &self.opts.agent, Player { name: self.opts.player.clone(), version: self.opts.version.clone() })
    }

    fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn report(&self) {
        (self.notify)(Status { state: self.state.clone(), queued: self.queued(), note: self.note.clone(), accepting: self.accepting, refused: self.refused.clone() });
    }

    fn connected(&self) -> bool {
        !self.held && matches!(self.state, State::Connected { .. })
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        self.report();
        self.send_all();
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
                    self.send_all();
                }
                Some(Cmd::Connect { token, server }) => self.connect(token, &server),
                Some(Cmd::Disconnect) => {
                    self.account.token.clear();
                    self.account.user.clear();
                    self.account.save(&self.account_path, &self.opts.sealer);
                    self.held = false;
                    self.retry_at = None;
                    self.feedback.clear();
                    self.accepting = false;
                    self.state = State::Disconnected;
                    self.note = "Disconnected".into();
                    log::info!("listenbrainz: disconnected");
                    self.report();
                }
                Some(Cmd::NowPlaying(mut l)) => {
                    if self.connected() {
                        self.fill_ids(std::slice::from_mut(&mut l));
                        match self.request(|c| c.playing_now(&l)) {
                            Ok(()) => {
                                log::debug!("listenbrainz: now playing sent");
                                if !std::mem::take(&mut self.refused).is_empty() {
                                    self.report();
                                }
                            }
                            Err(e) => self.failed(&e, false),
                        }
                    }
                }
                Some(Cmd::Flush) => {
                    if self.retry_at.is_none() {
                        self.send_all();
                    } else {
                        self.report();
                    }
                }
                Some(Cmd::Retry) => {
                    self.retry_at = None;
                    self.backoff = self.opts.retry_min;
                    self.send_all();
                }
                Some(Cmd::Feedback { recording, path, score }) => {
                    if matches!(self.state, State::Connected { .. } | State::Reconnect) {
                        self.feedback.push((recording, path, score));
                        if self.retry_at.is_none() {
                            self.send_all();
                        }
                    }
                }
            }
        }
    }

    /// One request, after the pacer's wait when it is short; its rate-limit headers noted. A
    /// longer wait is not slept here: `RateLimited` with what is left, nothing sent.
    fn request<T>(&mut self, f: impl FnOnce(&Client) -> Result<T, ApiError>) -> Result<T, ApiError> {
        let w = self.pacer.wait();
        if w > SHORT_WAIT {
            return Err(ApiError::RateLimited(Duration::from_secs(w.as_secs_f64().ceil() as u64)));
        }
        if !w.is_zero() {
            std::thread::sleep(w);
        }
        let c = self.client();
        let r = f(&c);
        self.pacer.answered(c.rate());
        if let Err(ApiError::RateLimited(d)) = &r {
            self.pacer.hold(*d);
        }
        r
    }

    /// The MusicBrainz ids from the file's tags, where the listen has none yet; the file is then
    /// forgotten (the listen is as it will be sent).
    fn fill_ids(&self, list: &mut [Listen]) -> bool {
        let mut changed = false;
        for l in list.iter_mut() {
            if let Some(p) = l.path.take() {
                if l.ids.is_empty() {
                    l.ids = (self.ids)(&p).tidy();
                }
                changed = true;
            }
        }
        changed
    }

    /// Something failed: decide what follows. `queue`: the failure was sending the queue (or
    /// feedback), which then waits.
    fn failed(&mut self, e: &ApiError, queue: bool) {
        // A 401 to a listen: the token, or the account (ListenBrainz refuses listens from an
        // account without a verified e-mail address the same way)? validate-token says which.
        let checked;
        let e = match e {
            ApiError::Unauthorized(said) => match self.request(Client::validate_token) {
                Ok(_) => {
                    log::warn!("listenbrainz: listens refused though the token is valid: {e}; trying again in {} s", self.opts.retry_max.as_secs());
                    self.retry_at = Some(Instant::now() + self.opts.retry_max);
                    self.refused = if said.is_empty() { "HTTP 401, with no reason given.".to_owned() } else { said.clone() };
                    self.note.clear();
                    self.report();
                    return;
                }
                Err(c) => {
                    checked = c;
                    &checked
                }
            },
            e => e,
        };
        match e {
            ApiError::InvalidToken => {
                log::warn!("listenbrainz: the token is no longer accepted; asking to connect again");
                self.account.token.clear();
                self.account.save(&self.account_path, &self.opts.sealer);
                self.state = State::Reconnect;
                self.held = true;
                self.refused.clear();
                self.note.clear();
            }
            ApiError::RateLimited(d) => {
                if queue {
                    log::info!("listenbrainz: rate limited; sending again in {} s", d.as_secs());
                    self.retry_at = Some(Instant::now() + *d);
                    self.note = format!("Waiting for ListenBrainz's rate limit; sending again in {} s", d.as_secs());
                } else {
                    log::info!("listenbrainz: now playing not sent (rate limited)");
                }
            }
            e if e.is_transient() => {
                if queue {
                    log::info!("listenbrainz: {e}; trying again in {} s", self.backoff.as_secs());
                    self.retry_at = Some(Instant::now() + self.backoff);
                    self.backoff = (self.backoff * 2).min(self.opts.retry_max);
                    self.note = format!("ListenBrainz could not be reached ({e}); trying again later");
                } else {
                    log::info!("listenbrainz: now playing not sent: {e}");
                }
            }
            e => log::warn!("listenbrainz: request refused: {e}"),
        }
        self.report();
    }

    /// The queue, then any feedback waiting.
    fn send_all(&mut self) {
        if self.send_queue() {
            self.send_feedback();
        }
    }

    /// Send the queue, oldest first, in batches, until it is empty or something fails. False
    /// when it stopped on a failure (or a wait).
    fn send_queue(&mut self) -> bool {
        if !self.connected() {
            self.report();
            return false;
        }
        let max = self.opts.max_listens.clamp(1, limits::MAX_LISTENS_PER_REQUEST);
        let max_bytes = self.opts.max_payload.min(limits::MAX_LISTEN_PAYLOAD_SIZE);
        let player = Player { name: self.opts.player.clone(), version: self.opts.version.clone() };
        // At most this many per request: halved after a 400, until the listen it refuses is alone.
        let mut window = max;
        let (mut sent, mut dropped) = (0usize, 0usize);
        let ok = loop {
            let mut batch = self.queue.lock().unwrap_or_else(|e| e.into_inner()).oldest(window);
            if batch.is_empty() {
                break true;
            }
            if self.fill_ids(&mut batch) {
                self.queue.lock().unwrap_or_else(|e| e.into_inner()).update(&batch);
            }
            let jsons: Vec<Value> = batch.iter().map(|l| api::listen_json(l, &player, ListenType::Import)).collect();
            let sizes: Vec<usize> = jsons.iter().map(|j| j.to_string().len()).collect();
            if let Some(why) = api::unsendable(&batch[0], sizes[0]) {
                log::warn!("listenbrainz: a listen cannot be sent ({why}); dropped");
                dropped += 1;
                self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&batch[..1]);
                continue;
            }
            // Up to the first that can never be sent (dropped when it is first).
            let good = batch.iter().zip(&sizes).take_while(|(l, s)| api::unsendable(l, **s).is_none()).count();
            let n = api::batch_len(&sizes[..good], window, max_bytes);
            let kind = if n == 1 { ListenType::Single } else { ListenType::Import };
            let payload: Vec<Value> = jsons.into_iter().take(n).collect();
            match self.request(|c| c.submit(kind, &payload)) {
                Ok(()) => {
                    self.refused.clear();
                    sent += n;
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&batch[..n]);
                    self.backoff = self.opts.retry_min;
                    window = (window * 2).min(max);
                }
                Err(ApiError::Rejected(status, m)) if n > 1 => {
                    log::warn!("listenbrainz: a batch of {n} was refused (HTTP {status}: {m}); sending it in halves");
                    window = n / 2;
                }
                Err(ApiError::Rejected(status, m)) => {
                    log::warn!("listenbrainz: a listen was refused (HTTP {status}: {m}); dropped");
                    dropped += 1;
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).remove(&batch[..1]);
                }
                Err(e) => {
                    self.failed(&e, true);
                    break false;
                }
            }
        };
        if sent > 0 || dropped > 0 {
            self.note = match (sent, dropped) {
                (s, 0) => format!("Sent {s} listen{}", if s == 1 { "" } else { "s" }),
                (s, d) => format!("Sent {s}, {d} not accepted by ListenBrainz"),
            };
            log::info!("listenbrainz: {}", self.note);
        }
        self.report();
        ok
    }

    /// Feedback waiting, oldest first; what has no recording id is dropped.
    fn send_feedback(&mut self) {
        while self.connected() && !self.feedback.is_empty() {
            let (recording, path, score) = self.feedback[0].clone();
            let id = recording.as_deref().and_then(api::mbid).or_else(|| path.as_deref().and_then(|p| (self.ids)(p).tidy().recording));
            let Some(id) = id else {
                log::info!("listenbrainz: a favourite without a MusicBrainz recording id; no feedback sent");
                self.feedback.remove(0);
                continue;
            };
            match self.request(|c| c.recording_feedback(&id, score)) {
                Ok(()) => {
                    log::info!("listenbrainz: feedback {score} sent");
                    self.feedback.remove(0);
                }
                Err(ApiError::Rejected(status, m)) => {
                    log::warn!("listenbrainz: feedback refused (HTTP {status}: {m}); dropped");
                    self.feedback.remove(0);
                }
                Err(e) => {
                    self.failed(&e, true);
                    return;
                }
            }
        }
    }

    /// Check the token, and connect as its user.
    fn connect(&mut self, token: String, server: &str) {
        let server = match api::normalize_server(server) {
            Ok(s) => s,
            Err(why) => {
                self.state = State::Error(why);
                self.report();
                return;
            }
        };
        if token.is_empty() {
            self.state = State::Error("Paste your user token from your ListenBrainz settings page.".into());
            self.report();
            return;
        }
        self.state = State::Connecting;
        self.refused.clear();
        self.note.clear();
        self.report();
        let before = std::mem::replace(&mut self.account, Account { server: server.clone(), user: String::new(), token });
        let r = self.request(Client::validate_token);
        match r {
            Ok(user) => {
                log::info!("listenbrainz: connected");
                self.account.user = user.clone();
                self.account.save(&self.account_path, &self.opts.sealer);
                self.state = State::Connected { user };
                self.accepting = true;
                self.held = false;
                self.retry_at = None;
                self.backoff = self.opts.retry_min;
                self.note = "Connected".into();
                self.send_all();
            }
            Err(e) => {
                log::info!("listenbrainz: not connected: {e}");
                let words = match e {
                    ApiError::InvalidToken => "ListenBrainz did not accept this token. Copy it again from your ListenBrainz settings page.".to_owned(),
                    ApiError::Network(_) | ApiError::Busy(_) => format!("Could not reach the server: {e}."),
                    ApiError::RateLimited(d) => format!("The server asked to wait {} s. Try again then.", d.as_secs()),
                    e => format!("That server did not answer as ListenBrainz does ({e}). Check the server address."),
                };
                // The account as it was (a reconnect that failed keeps the queue held).
                let keep_server = before.token.is_empty();
                self.account = before;
                if keep_server {
                    self.account.server = server;
                }
                self.state = State::Error(words);
                self.report();
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
        let d = std::env::temp_dir().join(format!("mp-listenbrainz-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_token_is_kept_sealed_and_a_plain_one_is_sealed_when_read() {
        let d = dir("seal");
        let p = d.join(ACCOUNT_FILE);
        std::fs::write(&p, r#"{"server":"http://127.0.0.1:9","user":"ann","token":" tok-plain "}"#).unwrap();
        let a = Account::load(&p, &XOR);
        assert_eq!(a, Account { server: "http://127.0.0.1:9".into(), user: "ann".into(), token: "tok-plain".into() });
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("tok-plain") && text.contains("xor-test"), "written again sealed: {text}");
        assert_eq!(Account::read(&p, &XOR), (a.clone(), false));
        assert!(!format!("{a:?}").contains("tok-plain"), "never in a debug print");
        // Sealed another way, or a seal that does not open: disconnected, the server kept.
        let (b, _) = Account::read(&p, &Sealer::PLAIN);
        assert_eq!((b.server.as_str(), b.user.as_str(), b.token.as_str()), ("http://127.0.0.1:9", "", ""));
        std::fs::write(&p, r#"{"user":"ann","sealed_with":"xor-test","sealed_token":"00ff"}"#).unwrap();
        assert_eq!(Account::load(&p, &XOR).token, "");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_queue_keeps_time_order_and_one_of_each() {
        let d = dir("queue");
        let p = d.join(QUEUE_FILE);
        let l = |t: i64, name: &str| Listen { artist: "A".into(), track: name.into(), listened_at: t, ..Listen::default() };
        let mut q = Queue::load(&p);
        q.push(l(30, "c"));
        q.push(l(10, "a"));
        q.push(l(20, "b"));
        q.push(l(10, "a"));
        assert_eq!(q.oldest(9).iter().map(|l| l.track.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        let q2 = Queue::load(&p);
        assert_eq!(q2.len(), 3, "on disk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn what_was_sent_is_dropped_even_when_an_earlier_listen_came_in_meanwhile() {
        let d = dir("queue-meanwhile");
        let p = d.join(QUEUE_FILE);
        let l = |t: i64, name: &str| Listen { artist: "A".into(), track: name.into(), listened_at: t, path: Some(format!("M:/{name}.flac")), ..Listen::default() };
        let mut q = Queue::load(&p);
        q.push(l(20, "b"));
        q.push(l(30, "c"));
        let mut batch = q.oldest(2);
        for b in &mut batch {
            b.path = None;
        }
        // The clock stepped back: a listen made while those were on their way sorts before them.
        q.push(l(10, "a"));
        q.update(&batch);
        assert_eq!(q.oldest(9), [l(10, "a"), batch[0].clone(), batch[1].clone()], "each written back over itself, the new one untouched");
        q.remove(&batch[..1]);
        q.remove(&batch[1..]);
        assert_eq!(q.oldest(9), [l(10, "a")], "the new listen is kept, and neither sent one is sent again");
        assert_eq!(Queue::load(&p).len(), 1, "on disk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_pacer_spaces_requests_and_honours_the_headers() {
        let mut p = Pacer { min_gap: Duration::from_millis(1000), last: None, not_before: None };
        assert!(p.wait().is_zero(), "the first goes at once");
        p.answered(http_client::RateLimit { remaining: Some(5), reset_in: Some(30) });
        assert!(p.wait() > Duration::from_millis(900) && p.wait() <= Duration::from_millis(1000), "a second after the last");
        p.answered(http_client::RateLimit { remaining: Some(0), reset_in: Some(30) });
        assert!(p.wait() > Duration::from_secs(29), "the window is used up: until it ends");
        p.hold(Duration::from_secs(5));
        assert!(p.wait() <= Duration::from_secs(5) && p.wait() > Duration::from_secs(4));
    }
}
