//! The submitter against the ListenBrainz stand-in (`listenbrainz::mock`, on 127.0.0.1: no
//! request leaves this machine): connecting with a token, now playing and single listens with
//! every field, the offline queue across a restart in batches, the rate limits and 429, a token
//! refused, a listen refused, an answer lost (no duplicate), feedback, and that no log line or
//! status carries the token. The stand-in checks every request against the documented rules;
//! each test ends with none broken.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use listenbrainz::mock::{Mock, Trouble, offline_url};
use listenbrainz::{Account, Ids, Listen, Options, Sealer, State, Status, Submitter};

const TOKEN: &str = "6f1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const USER: &str = "alice";
const REC: &str = "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae";
const RELEASE: &str = "0383dadf-2a4e-4d10-a46a-e9e041da8eb3";

// ---- a log that keeps every line, to look for the token in ------------------------------

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Keep;

impl log::Log for Keep {
    /// Every line of this crate and the HTTP client, down to trace; other crates' down to
    /// debug. ureq (the client's backend off Windows) dumps each request, headers and all, only
    /// at its own trace level, which a logger asks for knowingly: the app's never does (it keeps
    /// other crates' warnings and errors only). (As subsonic's test logger.)
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Debug || m.target().starts_with("listenbrainz") || m.target().starts_with("http_client")
    }
    fn log(&self, r: &log::Record) {
        // The log macros call this without asking `enabled` first.
        if !self.enabled(r.metadata()) {
            return;
        }
        LINES.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
    }
    fn flush(&self) {}
}

fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        log::set_logger(&Keep).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// A stand-in for the app's sealing: reversible, and not the plain text.
const XOR: Sealer = Sealer {
    scheme: "xor-test",
    seal: |b| Some(b.iter().map(|c| c ^ 0x5a).collect()),
    open: |b| Some(b.iter().map(|c| c ^ 0x5a).collect()),
    write: |p, b| std::fs::write(p, b),
};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("listenbrainz").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Connected as `alice` (at whatever root the test gives).
fn connected(d: &std::path::Path) {
    Account { server: String::new(), user: USER.into(), token: TOKEN.into() }.save(&d.join("listenbrainz.json"), &XOR);
}

struct Run {
    s: Submitter,
    status: Receiver<Status>,
    last: Option<Status>,
}

fn opts(d: &std::path::Path, root: &str) -> Options {
    Options {
        root: Some(root.into()),
        agent: "MarqueePlayer/test".into(),
        version: "2026.11.18".into(),
        retry_min: Duration::from_millis(150),
        min_gap: Duration::from_millis(10),
        sealer: XOR,
        ..Options::new(d)
    }
}

fn start_with(o: Options) -> Run {
    capture_logs();
    let (tx, status) = unbounded();
    let s = Submitter::start(
        o,
        Box::new(|p| if p == "M:/music/01.flac" { Ids { recording: Some(REC.into()), release: Some(RELEASE.into()), ..Ids::default() } } else { Ids::default() }),
        Box::new(move |st| {
            let _ = tx.send(st);
        }),
    );
    Run { s, status, last: None }
}

fn start(d: &std::path::Path, root: &str) -> Run {
    start_with(opts(d, root))
}

impl Run {
    fn wait(&mut self, what: &str, f: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(20);
        if let Some(l) = &self.last
            && f(l)
        {
            return l.clone();
        }
        loop {
            match self.status.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(st) => {
                    self.last = Some(st.clone());
                    if f(&st) {
                        return st;
                    }
                }
                Err(_) => panic!("timed out waiting for {what}; last {:?}", self.last),
            }
        }
    }
}

fn play(n: i64, track: &str) -> Listen {
    Listen {
        artist: "Aurora Lines".into(),
        track: track.into(),
        release: "Night Harbour".into(),
        duration_ms: 245_000,
        track_number: Some(n as u32),
        ids: Ids::default(),
        listened_at: 1_760_000_000 + n * 300,
        path: Some(format!("M:/music/{n:02}.flac")),
    }
}

fn queue_file(d: &std::path::Path) -> Vec<Listen> {
    serde_json::from_str(&std::fs::read_to_string(d.join("listens.json")).unwrap_or("[]".into())).unwrap()
}

fn no_violations(m: &Mock) {
    let v = m.with(|s| s.violations.clone());
    assert!(v.is_empty(), "documented rules broken: {v:?}");
}

fn is_connected(s: &Status) -> bool {
    matches!(s.state, State::Connected { .. })
}

#[test]
fn a_token_connects_as_its_user_and_is_kept_sealed() {
    let d = dir("connect");
    let m = Mock::start(TOKEN, USER);
    let mut run = start(&d, &m.url);
    run.wait("disconnected", |s| s.state == State::Disconnected && !s.accepting);
    // A token it does not know: said, nothing kept.
    run.s.connect("0000aaaa-0000-0000-0000-000000000000", "");
    let st = run.wait("refused", |s| matches!(s.state, State::Error(_)));
    assert!(matches!(&st.state, State::Error(e) if e.contains("did not accept this token")), "{st:?}");
    // The right one, pasted with spaces around it.
    run.s.connect(&format!("  {TOKEN}\n"), "");
    let st = run.wait("connected", |s| s.state == State::Connected { user: USER.into() });
    assert!(st.accepting && st.note == "Connected");
    let file = std::fs::read_to_string(d.join("listenbrainz.json")).unwrap();
    assert!(!file.contains(TOKEN) && file.contains("xor-test"), "sealed: {file}");
    assert_eq!(Account::read(&d.join("listenbrainz.json"), &XOR).0, Account { server: String::new(), user: USER.into(), token: TOKEN.into() });
    let seen = m.with(|s| s.requests.clone());
    assert!(seen.iter().all(|r| r.method == "GET" && r.path == "/1/validate-token" && r.user_agent == "MarqueePlayer/test"), "{seen:?}");
    assert!(seen.last().unwrap().token_ok, "Authorization: Token <token>");
    // Restarted: connected, without asking.
    drop(run);
    let mut run = start(&d, &m.url);
    run.wait("connected after a restart", |s| s.state == State::Connected { user: USER.into() });
    run.s.disconnect();
    let st = run.wait("disconnected", |s| s.state == State::Disconnected);
    assert!(!st.accepting);
    assert!(Account::read(&d.join("listenbrainz.json"), &XOR).0.token.is_empty(), "disconnect forgets the token");
    no_violations(&m);
}

#[test]
fn a_server_address_of_ones_own_is_used_and_kept() {
    let d = dir("server");
    let m = Mock::start(TOKEN, USER);
    // No forced root: the address typed is where requests go.
    let mut run = start_with(Options { root: None, ..opts(&d, "") });
    // A server that does not speak the API, then none at all.
    run.s.connect(TOKEN, &format!("{}/not/here", m.url));
    let st = run.wait("not a ListenBrainz API", |s| matches!(s.state, State::Error(_)));
    assert!(matches!(&st.state, State::Error(e) if e.contains("Check the server address")), "{st:?}");
    run.last = None;
    run.s.connect(TOKEN, &offline_url());
    let st = run.wait("unreachable", |s| matches!(&s.state, State::Error(e) if e.contains("Could not reach")));
    assert!(!st.accepting);
    run.s.connect(TOKEN, &format!("{}/1/", m.url));
    run.wait("connected", |s| s.state == State::Connected { user: USER.into() });
    assert_eq!(Account::read(&d.join("listenbrainz.json"), &XOR).0.server, m.url, "kept without the API's own /1/");
    run.s.submit(play(2, "Own server"));
    run.wait("sent", |s| s.note == "Sent 1 listen");
    assert_eq!(m.tracks(), ["Own server"]);
    no_violations(&m);
}

#[test]
fn now_playing_and_a_single_listen_carry_every_field() {
    let d = dir("single");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    run.s.now_playing(play(1, "Lantern"));
    run.s.submit(play(1, "Lantern"));
    run.wait("sent", |s| s.note == "Sent 1 listen" && s.queued == 0);
    // Both requests, in either order: the worker says it is connected before its first look at
    // the queue, so the listen can go out in that look, before the "now playing" waiting behind
    // it, and the status can say "Sent" before that one has gone.
    let t0 = Instant::now();
    while m.with(|s| s.requests.len()) < 2 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (np, listens, seen) = m.with(|s| (s.playing_now.clone(), s.listens.clone(), s.requests.clone()));
    let mut kinds = seen.iter().map(|r| r.listen_type.as_str()).collect::<Vec<_>>();
    kinds.sort_unstable();
    assert_eq!(kinds, ["playing_now", "single"]);
    assert!(seen.iter().all(|r| r.token_ok));
    let info = serde_json::json!({
        "media_player": "Marquee Player",
        "media_player_version": "2026.11.18",
        "submission_client": "Marquee Player",
        "submission_client_version": "2026.11.18",
        "duration_ms": 245_000,
        "tracknumber": "1",
        "recording_mbid": REC,
        "release_mbid": RELEASE,
    });
    let meta = serde_json::json!({ "artist_name": "Aurora Lines", "track_name": "Lantern", "release_name": "Night Harbour", "additional_info": info });
    assert_eq!(np, [serde_json::json!({ "track_metadata": meta })], "now playing: no time");
    assert_eq!(listens, [serde_json::json!({ "listened_at": 1_760_000_300, "track_metadata": meta })]);
    assert!(queue_file(&d).is_empty());
    no_violations(&m);
}

#[test]
fn offline_listens_survive_a_restart_and_go_in_batches_oldest_first() {
    let d = dir("offline");
    connected(&d);
    let mut run = start_with(Options { max_listens: 10, ..opts(&d, &offline_url()) });
    run.wait("connected", is_connected);
    // Made out of order on purpose: the queue keeps them by time.
    for n in [3, 1, 2] {
        run.s.submit(play(n, &format!("T{n}")));
    }
    for n in 4..=25 {
        run.s.submit(play(n, &format!("T{n}")));
    }
    let st = run.wait("failure noted, every listen queued", |s| s.note.contains("could not be reached") && s.queued == 25);
    assert!(st.accepting);
    assert_eq!(queue_file(&d).len(), 25, "on disk while offline");
    drop(run);

    let m = Mock::start(TOKEN, USER);
    let mut run = start_with(Options { max_listens: 10, ..opts(&d, &m.url) });
    run.wait("all sent", |s| s.queued == 0 && s.note.starts_with("Sent"));
    let seen = m.with(|s| s.requests.clone());
    assert_eq!(seen.iter().map(|r| (r.listen_type.as_str(), r.listens)).collect::<Vec<_>>(), [("import", 10), ("import", 10), ("import", 5)]);
    let tracks = m.tracks();
    assert_eq!(tracks.len(), 25);
    assert_eq!(&tracks[..3], ["T1", "T2", "T3"], "oldest first");
    assert_eq!(tracks[24], "T25");
    assert!(queue_file(&d).is_empty());
    no_violations(&m);
}

#[test]
fn batches_keep_within_the_payload_size() {
    let d = dir("size");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.set_trouble(Trouble::Offline);
    let max_payload = 3000;
    let mut run = start_with(Options { max_payload, ..opts(&d, &m.url) });
    run.wait("connected", is_connected);
    for n in 1..=12 {
        run.s.submit(Listen { release: "x".repeat(400), ..play(n, &format!("Long {n}")) });
    }
    run.wait("queued", |s| s.queued == 12 && s.note.contains("could not be reached"));
    m.with(|s| {
        s.trouble = Trouble::None;
        s.requests.clear();
    });
    run.s.retry();
    run.wait("sent", |s| s.queued == 0);
    let seen = m.with(|s| s.requests.clone());
    assert!(seen.len() > 2 && seen.iter().all(|r| r.bytes <= max_payload && r.status == 200), "{:?}", seen.iter().map(|r| (r.listens, r.bytes)).collect::<Vec<_>>());
    assert_eq!(seen.iter().map(|r| r.listens).sum::<usize>(), 12);
    no_violations(&m);
}

/// A server that refuses every listen: three are dropped, then the rest is kept and tried
/// again later, nothing more dropped while it goes on refusing; once it takes them, they go.
#[test]
fn a_server_that_refuses_every_listen_does_not_drain_the_queue() {
    let d = dir("refuses-all");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.set_trouble(Trouble::RefuseListens);
    let mut run = start_with(Options { retry_max: Duration::from_millis(400), ..opts(&d, &m.url) });
    run.wait("connected", is_connected);
    for n in 1..=10 {
        run.s.submit(play(n, &format!("T{n}")));
    }
    run.wait("three dropped", |s| s.note == "Sent 0, 3 not accepted by ListenBrainz" && s.queued == 7);
    // Tried again after the wait, refused again: kept.
    let t0 = Instant::now();
    while m.with(|s| s.requests.iter().filter(|r| r.status == 400).count()) < 6 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(queue_file(&d).len(), 7, "nothing more dropped");
    m.set_trouble(Trouble::None);
    run.wait("the rest sent", |s| s.queued == 0 && s.note == "Sent 7 listens");
    assert_eq!(m.tracks(), ["T4", "T5", "T6", "T7", "T8", "T9", "T10"]);
    no_violations(&m);
}

#[test]
fn a_server_error_is_retried_and_a_refused_listen_is_dropped_alone() {
    let d = dir("errors");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.with(|s| s.reject_track = Some("bad".into()));
    m.set_trouble(Trouble::Down);
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    // Queued before the thread looks, so all go in one batch.
    for (n, t) in [(1, "A"), (2, "B"), (3, "bad"), (4, "D"), (5, "E")] {
        run.s.submit(play(n, t));
    }
    run.wait("an outage noted", |s| s.note.contains("could not be reached") && s.queued == 5);
    m.set_trouble(Trouble::None);
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 4, 1 not accepted by ListenBrainz");
    assert_eq!(m.tracks(), ["A", "B", "D", "E"], "the others, in order");
    let statuses: Vec<u16> = m.with(|s| s.requests.iter().map(|r| r.status).collect());
    assert!(statuses.starts_with(&[503]) && statuses.contains(&400), "{statuses:?}");
    assert!(queue_file(&d).is_empty(), "the refused one is not kept");
    // A listen that can never be sent goes without a request.
    let before = m.with(|s| s.requests.len());
    run.s.submit(Listen { listened_at: 1_000, ..play(9, "Too old") });
    run.wait("dropped", |s| s.note == "Sent 0, 1 not accepted by ListenBrainz" && s.queued == 0);
    assert_eq!(m.with(|s| s.requests.len()), before);
    no_violations(&m);
}

#[test]
fn rate_limits_wait_instead_of_hammering() {
    let d = dir("ratelimit");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    // A 429: sent again after X-RateLimit-Reset-In, not after the (long) backoff.
    m.set_trouble(Trouble::RateLimited { times: 1, reset_in: 2 });
    let mut run = start_with(Options { retry_min: Duration::from_secs(600), min_gap: Duration::from_millis(300), max_listens: 1, ..opts(&d, &m.url) });
    run.wait("connected", is_connected);
    run.s.submit(play(1, "One"));
    let st = run.wait("waiting", |s| s.note.contains("rate limit"));
    assert!(st.note.contains("2 s"), "{}", st.note);
    run.wait("sent", |s| s.note == "Sent 1 listen");
    let seen = m.with(|s| s.requests.clone());
    assert_eq!(seen.iter().map(|r| r.status).collect::<Vec<_>>(), [429, 200]);
    let gap = seen[1].at - seen[0].at;
    assert!(gap >= Duration::from_millis(1900) && gap < Duration::from_secs(10), "waited {gap:?}");

    // The window used up (X-RateLimit-Remaining: 0): the next request waits for it to reset,
    // and none is refused.
    m.with(|s| {
        s.requests.clear();
        s.window = (2, 2);
    });
    // Let the window the mock starts now be the one counted.
    std::thread::sleep(Duration::from_millis(2100));
    for n in 2..=4 {
        run.s.submit(play(n, &format!("W{n}")));
    }
    let t0 = Instant::now();
    while m.tracks().len() < 4 && t0.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = m.with(|s| s.requests.clone());
    assert!(seen.iter().all(|r| r.status == 200), "{:?}", seen.iter().map(|r| r.status).collect::<Vec<_>>());
    for w in seen.windows(2) {
        assert!(w[1].at - w[0].at >= Duration::from_millis(290), "a second apart (here {:?})", w[1].at - w[0].at);
    }
    assert!(seen.len() >= 3 && seen[2].at - seen[0].at >= Duration::from_millis(1500), "the third waited for the window");
    no_violations(&m);
}

#[test]
fn a_token_refused_stops_sending_keeps_the_queue_and_connecting_again_sends_it() {
    let d = dir("token");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.set_trouble(Trouble::BadToken);
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    run.s.submit(play(1, "A"));
    let st = run.wait("connect again", |s| s.state == State::Reconnect);
    assert!(st.accepting && st.queued == 1, "kept for after connecting again: {st:?}");
    std::thread::sleep(Duration::from_millis(600));
    run.s.submit(play(2, "B"));
    run.s.retry();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(m.with(|s| s.requests.len()), 2, "the listen and the check of the token, then no retrying for ever");
    assert_eq!(queue_file(&d).len(), 2);
    assert!(Account::read(&d.join("listenbrainz.json"), &XOR).0.token.is_empty());
    m.set_trouble(Trouble::None);
    run.s.connect(TOKEN, "");
    run.wait("sent after connecting again", |s| is_connected(s) && s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(m.tracks(), ["A", "B"]);
    no_violations(&m);
}

/// ListenBrainz answers 401 to listens from an account without a verified e-mail address: the
/// token is still valid, so it is kept, the server's words are shown, and the listens wait.
#[test]
fn listens_refused_with_a_valid_token_keep_the_token_and_say_why() {
    let d = dir("noemail");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.set_trouble(Trouble::NoEmail);
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    run.s.submit(play(1, "A"));
    let st = run.wait("the server's words", |s| s.refused.contains("does not have an email"));
    assert!(is_connected(&st) && st.accepting && st.queued == 1, "still connected, the listen kept: {st:?}");
    assert!(!Account::read(&d.join("listenbrainz.json"), &XOR).0.token.is_empty(), "the token is kept");
    m.set_trouble(Trouble::None);
    run.s.retry();
    run.wait("sent once the account is in order", |s| is_connected(s) && s.queued == 0 && s.note.starts_with("Sent") && s.refused.is_empty());
    assert_eq!(m.tracks(), ["A"]);
    no_violations(&m);
}

#[test]
fn an_answer_lost_on_the_way_does_not_make_a_duplicate() {
    let d = dir("lost");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    m.set_trouble(Trouble::LoseAnswer { times: 1 });
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    run.s.submit(play(1, "Once"));
    run.wait("the answer lost", |s| s.note.contains("could not be reached") && s.queued == 1);
    // On disk as it went: its ids read in, its file forgotten.
    let q = queue_file(&d);
    assert_eq!((q[0].ids.recording.as_deref(), q[0].path.as_deref()), (Some(REC), None));
    run.wait("sent again", |s| s.queued == 0 && s.note == "Sent 1 listen");
    let (listens, dups, seen) = m.with(|s| (s.listens.len(), s.duplicates, s.requests.clone()));
    assert_eq!((listens, dups, seen.len()), (1, 1, 2), "stored once; the second submission was the same listen");
    no_violations(&m);
}

#[test]
fn a_wrong_address_keeps_the_queue() {
    let d = dir("wrong");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    let mut run = start(&d, &format!("{}/elsewhere", m.url));
    run.wait("connected", is_connected);
    run.s.submit(play(1, "Kept"));
    let st = run.wait("noted", |s| s.note.contains("unexpected answer (HTTP 404"));
    assert_eq!(st.queued, 1, "a 404 drops nothing");
    assert_eq!(queue_file(&d).len(), 1);
}

#[test]
fn love_goes_as_feedback_by_recording_id() {
    let d = dir("love");
    connected(&d);
    let m = Mock::start(TOKEN, USER);
    let mut run = start(&d, &m.url);
    run.wait("connected", is_connected);
    run.s.feedback(None, Some("M:/music/01.flac".into()), 1);
    run.s.feedback(None, Some("M:/music/02.flac".into()), 1);
    run.s.feedback(Some(REC.to_uppercase()), None, 0);
    let t0 = Instant::now();
    while m.with(|s| s.feedback.len()) < 2 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let fb = m.with(|s| s.feedback.clone());
    assert_eq!(fb, [serde_json::json!({"recording_mbid": REC, "score": 1}), serde_json::json!({"recording_mbid": REC, "score": 0})], "the file without an id sends nothing");
    no_violations(&m);
}

#[test]
fn no_log_line_or_status_carries_the_token() {
    let d = dir("secret");
    let m = Mock::start(TOKEN, USER);
    let mut run = start(&d, &m.url);
    let mut texts = Vec::new();
    // Connecting against a server that echoes the request (Authorization and all) back.
    for status in [400, 404, 500] {
        m.set_trouble(Trouble::Echo(status));
        run.s.connect(TOKEN, "");
        texts.push(format!("{:?}", run.wait("refused", |s| matches!(s.state, State::Error(_)))));
        run.last = None;
    }
    m.set_trouble(Trouble::None);
    run.s.connect(TOKEN, "");
    run.wait("connected", is_connected);
    for (status, n) in [(400, 1), (404, 2), (500, 3)] {
        m.set_trouble(Trouble::Echo(status));
        run.s.submit(play(n, &format!("S{n}")));
        run.s.retry();
        std::thread::sleep(Duration::from_millis(300));
        texts.push(format!("{:?}", run.status.try_iter().last()));
    }
    let seen = m.with(|s| s.requests.len());
    assert!(seen >= 7, "{seen} requests");
    let lines = LINES.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l.contains("cannot handle")), "the server's message is kept (scrubbed)");
    for t in texts.iter().chain(&lines) {
        assert!(!t.contains(TOKEN) && !t.contains(&TOKEN[..12]), "the token in: {t}");
    }
}

#[test]
fn a_redirect_to_another_server_is_refused_without_sending_the_token() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    let trap_port = trap.local_addr().unwrap().port();
    let trapped: Arc<Mutex<Vec<String>>> = Arc::default();
    let t = trapped.clone();
    std::thread::spawn(move || {
        for conn in trap.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut buf = [0u8; 4096];
            let n = conn.read(&mut buf).unwrap_or(0);
            t.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let body = r#"{"valid":true,"user_name":"alice","status":"ok"}"#;
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = conn.write_all(resp.as_bytes());
        }
    });

    let redir = TcpListener::bind("127.0.0.1:0").unwrap();
    let redir_url = format!("http://127.0.0.1:{}", redir.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let mut n = 0usize;
        for conn in redir.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let _ = r.read_exact(&mut vec![0; len]);
            let status = if n == 0 { "302 Found" } else { "307 Temporary Redirect" };
            n += 1;
            let resp = format!("HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:{trap_port}/1/validate-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = conn.write_all(resp.as_bytes());
        }
    });

    let client = listenbrainz::Client::new(&redir_url, TOKEN, "MarqueePlayer/test", listenbrainz::api::Player::default());
    assert!(matches!(client.validate_token(), Err(listenbrainz::ApiError::Network(_))));
    assert!(matches!(client.playing_now(&play(1, "A")), Err(listenbrainz::ApiError::Network(_))));
    assert!(matches!(client.recording_feedback(REC, 1), Err(listenbrainz::ApiError::Network(_))));
    assert!(trapped.lock().unwrap().is_empty(), "other server received a redirected token: {:?}", trapped.lock().unwrap());
}
