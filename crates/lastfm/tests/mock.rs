//! The scrobbler against a local mock of the Last.fm API (no request leaves this
//! machine): the desktop auth flow, now playing, single and batched scrobbles, the
//! offline queue across a restart, and each class of error. The mock checks every
//! request's signature with the test secret, as Last.fm does.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use lastfm::{Credentials, Options, Scrobble, Scrobbler, Sealer, State, Status};

const KEY: &str = "testkey0123456789";
const SECRET: &str = "testsecret";

type Form = BTreeMap<String, String>;
type Handler = Box<dyn FnMut(&Form) -> (u16, String) + Send>;

struct Mock {
    url: String,
    log: Arc<Mutex<Vec<Form>>>,
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

fn mock(mut handler: Handler) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/2.0/", listener.local_addr().unwrap().port());
    let log: Arc<Mutex<Vec<Form>>> = Arc::default();
    let l = log.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let mut len = 0usize;
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            assert!(first.starts_with("POST /2.0/ "), "{first}");
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let form: Form = body.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect();
            // Check the signature, as Last.fm does.
            let params: Vec<(String, String)> = form.iter().filter(|(k, _)| *k != "api_sig").map(|(k, v)| (k.clone(), v.clone())).collect();
            let (status, answer) = if form.get("api_sig") != Some(&lastfm::api::sign(&params, SECRET)) || form.get("api_key").map(String::as_str) != Some(KEY) {
                (403, r#"{"error":13,"message":"Invalid method signature supplied"}"#.to_owned())
            } else {
                assert_eq!(form.get("format").map(String::as_str), Some("json"));
                l.lock().unwrap().push(form.clone());
                handler(&form)
            };
            let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            let _ = conn.write_all(resp.as_bytes());
        }
    });
    Mock { url, log }
}

/// A port nothing listens on: every request fails as if offline.
fn offline() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}/2.0/")
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lastfm").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn connected(d: &std::path::Path) {
    Credentials { api_key: KEY.into(), api_secret: SECRET.into(), session_key: "sk-1".into(), username: "alice".into() }.save(&d.join("lastfm.json"), &Sealer::PLAIN);
}

struct Run {
    s: Scrobbler,
    status: Receiver<Status>,
    opened: Arc<Mutex<Vec<String>>>,
    last: Option<Status>,
}

fn start(d: &std::path::Path, root: &str) -> Run {
    start_sealed(d, root, Sealer::PLAIN)
}

fn start_sealed(d: &std::path::Path, root: &str, sealer: Sealer) -> Run {
    let (tx, status) = unbounded();
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let o = opened.clone();
    let opts = Options { root: root.into(), auth_poll: Duration::from_millis(30), retry_min: Duration::from_millis(150), sealer, ..Options::new(d) };
    let s = Scrobbler::start(
        opts,
        Box::new(|p| (p == "M:/music/01.flac").then(|| "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae".to_owned())),
        Box::new(move |st| {
            let _ = tx.send(st);
        }),
        Box::new(move |u| o.lock().unwrap().push(u.to_owned())),
    );
    Run { s, status, opened, last: None }
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
            let left = deadline.saturating_duration_since(Instant::now());
            match self.status.recv_timeout(left) {
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

fn play(n: i64, track: &str) -> Scrobble {
    Scrobble {
        artist: "Aurora Lines".into(),
        track: track.into(),
        album: "Night Harbour".into(),
        album_artist: "Various Artists".into(),
        duration: 245,
        track_number: Some(n as u32),
        mbid: None,
        timestamp: 1_760_000_000 + n * 300,
        path: Some(format!("M:/music/{n:02}.flac")),
    }
}

fn accept_all(f: &Form) -> (u16, String) {
    match f["method"].as_str() {
        "track.scrobble" => {
            let n = f.keys().filter(|k| k.starts_with("artist[")).count();
            let one = r##"{"track":{"#text":"t"},"ignoredMessage":{"code":"0","#text":""}}"##;
            let list = if n == 1 { one.to_owned() } else { format!("[{}]", vec![one; n].join(",")) };
            (200, format!(r#"{{"scrobbles":{{"@attr":{{"accepted":{n},"ignored":0}},"scrobble":{list}}}}}"#))
        }
        "track.updateNowPlaying" => (200, r#"{"nowplaying":{}}"#.into()),
        m => panic!("unexpected {m}"),
    }
}

fn queue_file(d: &std::path::Path) -> Vec<Scrobble> {
    serde_json::from_str(&std::fs::read_to_string(d.join("scrobbles.json")).unwrap_or("[]".into())).unwrap()
}

#[test]
fn desktop_auth_flow_connects_and_keeps_the_session_in_the_config_folder() {
    let d = dir("auth");
    let polls = Arc::new(Mutex::new(0));
    let p = polls.clone();
    let m = mock(Box::new(move |f| match f["method"].as_str() {
        "auth.getToken" => (200, r#"{"token":"tok-42"}"#.into()),
        "auth.getSession" => {
            assert_eq!(f["token"], "tok-42");
            *p.lock().unwrap() += 1;
            if *p.lock().unwrap() < 3 {
                (403, r#"{"error":14,"message":"This token has not been authorized"}"#.into())
            } else {
                (200, r#"{"session":{"name":"alice","key":"sk-secret","subscriber":0}}"#.into())
            }
        }
        m => panic!("unexpected {m}"),
    }));
    let mut run = start(&d, &m.url);
    assert!(!run.wait("no credentials", |s| s.state == State::NoCredentials).accepting);
    run.s.set_credentials(&format!(" {KEY} "), SECRET);
    assert!(!run.wait("disconnected", |s| s.state == State::Disconnected).accepting, "never connected: plays are not taken");
    run.s.connect();
    let st = run.wait("authorising", |s| matches!(s.state, State::Authorising { .. }));
    let State::Authorising { url } = st.state else { unreachable!() };
    assert_eq!(url, format!("https://www.last.fm/api/auth/?api_key={KEY}&token=tok-42"));
    assert!(run.wait("connected", |s| s.state == State::Connected { user: "alice".into() }).accepting);
    assert_eq!(*run.opened.lock().unwrap(), [url], "the browser was asked to open the page, once");
    assert_eq!(*polls.lock().unwrap(), 3, "polled until authorised");
    let saved = Credentials::read(&d.join("lastfm.json"), &Sealer::PLAIN).0;
    assert_eq!((saved.session_key.as_str(), saved.username.as_str(), saved.api_key.as_str()), ("sk-secret", "alice", KEY));
    // Restarted: still connected, without asking again.
    drop(run);
    let mut run = start(&d, &m.url);
    run.wait("connected after restart", |s| s.state == State::Connected { user: "alice".into() });
    run.s.disconnect();
    assert!(!run.wait("disconnected", |s| s.state == State::NoCredentials).accepting, "disconnected: plays are no longer taken");
    assert_eq!(Credentials::read(&d.join("lastfm.json"), &Sealer::PLAIN).0, Credentials::default(), "disconnect forgets the key, secret and session");
}

/// A stand-in for the app's sealing: reversible, and not the plain text.
const XOR: Sealer = Sealer { scheme: "xor-test", seal: |b| Some(b.iter().map(|c| c ^ 0x5a).collect()), open: |b| Some(b.iter().map(|c| c ^ 0x5a).collect()), write: |p, b| std::fs::write(p, b) };

fn hex(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

/// Every file left in the folder, with its bytes.
fn files(d: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    std::fs::read_dir(d).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap_or_default())).collect()
}

/// Whether `needle` is anywhere in the bytes.
fn holds(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Connected, with plays waiting (Last.fm unreachable) and the copies the app writes beside its
/// files: Disconnect leaves nothing of the account in the worker, in what it reports, or on
/// disk (not the key, the secret, the session, the user, nor their sealed hex), and drops the
/// plays waiting; started again, it has no credentials.
#[test]
fn disconnect_forgets_the_key_secret_session_and_the_plays_waiting() {
    let d = dir("forget");
    let session = "sk-forget-9f8e7d";
    Credentials { api_key: KEY.into(), api_secret: SECRET.into(), session_key: session.into(), username: "alice".into() }.save(&d.join("lastfm.json"), &XOR);
    let sealed = [hex(&(XOR.seal)(SECRET.as_bytes()).unwrap()), hex(&(XOR.seal)(session.as_bytes()).unwrap())];
    assert!(holds(&std::fs::read(d.join("lastfm.json")).unwrap(), &sealed[0]), "the test looks for what is really written");
    // A damaged file kept aside, a save cut short, the same for the queue.
    for (name, text) in [("lastfm.json.bad", format!("{{\"api_key\":\"{KEY}\",\"secret\":\"{}\"", sealed[0])), ("lastfm.json.tmp", format!("{KEY} {}", sealed[1])), ("scrobbles.json.bad", "[".into())] {
        std::fs::write(d.join(name), text).unwrap();
    }
    let root = offline();
    let mut run = start_sealed(&d, &root, XOR);
    run.wait("connected", |s| s.state == State::Connected { user: "alice".into() });
    run.s.scrobble(play(1, "Waiting"));
    run.s.scrobble(play(2, "Also waiting"));
    run.wait("kept for later", |s| s.queued == 2 && !s.note.is_empty());
    assert_eq!(queue_file(&d).len(), 2);

    run.s.disconnect();
    let st = run.wait("forgotten", |s| s.state == State::NoCredentials);
    assert!(!st.accepting && st.queued == 0 && run.s.queued() == 0, "{st:?}");
    assert!(!format!("{st:?}").contains("alice"));
    let left = files(&d);
    for (name, bytes) in &left {
        for needle in [KEY, SECRET, session, "alice", &sealed[0], &sealed[1]] {
            assert!(!holds(bytes, needle), "{name} still holds {needle}");
        }
    }
    assert!(left.is_empty(), "nothing of Last.fm's is left in the folder: {:?}", left.iter().map(|f| &f.0).collect::<Vec<_>>());
    // A restart has nothing to show.
    drop(run);
    let mut run = start_sealed(&d, &root, XOR);
    let st = run.wait("no credentials after a restart", |s| s.state == State::NoCredentials);
    assert!(!st.accepting && st.queued == 0);
    assert_eq!(Credentials::read(&d.join("lastfm.json"), &XOR).0, Credentials::default());
    assert!(!d.join("lastfm.json").exists());
}

/// What an older version left after its Disconnect: the key and the secret, no session. Not
/// connected, Disconnect still forgets them, and a restart shows nothing.
#[test]
fn a_key_and_secret_left_by_an_older_version_can_be_removed() {
    let d = dir("older");
    // As the older version wrote it (the secret in plain text, before sealing).
    std::fs::write(d.join("lastfm.json"), format!(r#"{{"api_key":"{KEY}","api_secret":"{SECRET}","username":""}}"#)).unwrap();
    let mut run = start_sealed(&d, &offline(), XOR);
    let st = run.wait("disconnected", |s| s.state == State::Disconnected);
    assert!(!st.accepting);
    assert_eq!(Credentials::read(&d.join("lastfm.json"), &XOR).0.api_key, KEY, "still there before");
    run.s.disconnect();
    run.wait("forgotten", |s| s.state == State::NoCredentials);
    assert!(files(&d).is_empty(), "{:?}", files(&d).iter().map(|f| &f.0).collect::<Vec<_>>());
    drop(run);
    let mut run = start_sealed(&d, &offline(), XOR);
    run.wait("no credentials after a restart", |s| s.state == State::NoCredentials);
    assert_eq!(Credentials::read(&d.join("lastfm.json"), &XOR).0, Credentials::default());
    // An empty key and secret applied (the fields cleared, then Connect): the file goes too.
    run.s.set_credentials(KEY, SECRET);
    run.wait("disconnected", |s| s.state == State::Disconnected);
    assert!(d.join("lastfm.json").exists());
    run.s.set_credentials("", "");
    run.wait("no credentials", |s| s.state == State::NoCredentials);
    assert!(files(&d).is_empty());
}

#[test]
fn a_refused_key_says_so_and_cancel_stops_waiting() {
    let d = dir("auth-errors");
    let m = mock(Box::new(|f| match f["method"].as_str() {
        "auth.getToken" => (200, r#"{"token":"t"}"#.into()),
        "auth.getSession" => (403, r#"{"error":14,"message":"not yet"}"#.into()),
        m => panic!("unexpected {m}"),
    }));
    let mut run = start(&d, &m.url);
    run.s.set_credentials(KEY, "wrong-secret");
    run.s.connect();
    let st = run.wait("error", |s| matches!(s.state, State::Error(_)));
    assert!(matches!(&st.state, State::Error(e) if e.contains("API key or secret")), "{st:?}");
    run.s.set_credentials(KEY, SECRET);
    run.s.connect();
    run.wait("authorising", |s| matches!(s.state, State::Authorising { .. }));
    run.s.cancel();
    run.wait("back to disconnected", |s| s.state == State::Disconnected);
}

#[test]
fn now_playing_and_a_single_scrobble_carry_every_field() {
    let d = dir("single");
    connected(&d);
    let m = mock(Box::new(accept_all));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.now_playing(play(1, "Lantern"));
    run.s.scrobble(play(1, "Lantern"));
    run.wait("sent", |s| s.note == "Sent 1 scrobble" && s.queued == 0);
    // Both requests, in either order: the worker says it is connected before its first look at
    // the queue, so the scrobble can go out in that look, before the "now playing" waiting
    // behind it, and the status can say "Sent" before that one has gone.
    let t0 = Instant::now();
    while m.log.lock().unwrap().len() < 2 && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let log = m.log.lock().unwrap().clone();
    assert_eq!(log.len(), 2, "now playing and the scrobble");
    let by = |method: &str| log.iter().find(|f| f["method"] == method).cloned().unwrap_or_else(|| panic!("no {method}"));
    let np = &by("track.updateNowPlaying");
    assert_eq!(np["sk"], "sk-1");
    for (k, v) in [("artist", "Aurora Lines"), ("track", "Lantern"), ("album", "Night Harbour"), ("albumArtist", "Various Artists"), ("duration", "245"), ("trackNumber", "1"), ("mbid", "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae")] {
        assert_eq!(np.get(k).map(String::as_str), Some(v), "now playing {k}");
    }
    assert!(!np.contains_key("timestamp"));
    let sc = &by("track.scrobble");
    for (k, v) in [("artist[0]", "Aurora Lines"), ("track[0]", "Lantern"), ("album[0]", "Night Harbour"), ("albumArtist[0]", "Various Artists"), ("duration[0]", "245"), ("trackNumber[0]", "1"), ("timestamp[0]", "1760000300"), ("mbid[0]", "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae")] {
        assert_eq!(sc.get(k).map(String::as_str), Some(v), "scrobble {k}");
    }
    assert!(!sc.keys().any(|k| k.starts_with("path")), "the file path is never sent");
    assert!(queue_file(&d).is_empty());
}

#[test]
fn offline_plays_survive_a_restart_and_go_in_batches_oldest_first() {
    let d = dir("offline");
    connected(&d);
    let mut run = start(&d, &offline());
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    // Made out of order on purpose: the queue keeps them by time.
    for n in [3, 1, 2] {
        run.s.scrobble(play(n, &format!("T{n}")));
    }
    for n in 4..=120 {
        run.s.scrobble(play(n, &format!("T{n}")));
    }
    // Both: where a refused connection fails at once (Linux), the failure is noted before the
    // thread has taken every play.
    let st = run.wait("failure noted, every play queued", |s| s.note.contains("could not be reached") && s.queued == 120);
    assert_eq!(st.queued, 120);
    assert_eq!(queue_file(&d).len(), 120, "on disk while offline");
    drop(run);

    let m = mock(Box::new(accept_all));
    let mut run = start(&d, &m.url);
    run.wait("all sent", |s| s.queued == 0 && s.note.starts_with("Sent"));
    let log = m.log.lock().unwrap().clone();
    let sizes: Vec<usize> = log.iter().map(|f| f.keys().filter(|k| k.starts_with("artist[")).count()).collect();
    assert_eq!(sizes, [50, 50, 20], "batches of 50");
    assert_eq!(log[0]["track[0]"], "T1");
    assert_eq!(log[0]["track[1]"], "T2");
    assert_eq!(log[0]["track[2]"], "T3", "oldest first");
    assert_eq!(log[2]["track[19]"], "T120");
    assert!(queue_file(&d).is_empty());
}

#[test]
fn transient_errors_retry_and_ignored_plays_are_dropped() {
    let d = dir("transient");
    connected(&d);
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let m = mock(Box::new(move |f| {
        *c.lock().unwrap() += 1;
        match *c.lock().unwrap() {
            1 => (200, r#"{"error":11,"message":"Service Offline"}"#.into()),
            2 => (503, "<html>busy</html>".into()),
            _ => {
                assert_eq!(f.keys().filter(|k| k.starts_with("artist[")).count(), 2);
                (
                    200,
                    r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":1},"scrobble":[{"ignoredMessage":{"code":"0","#text":""}},{"ignoredMessage":{"code":"3","#text":"Timestamp too old"}}]}}"##.into(),
                )
            }
        }
    }));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "B"));
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 1, 1 not accepted by Last.fm");
    assert_eq!(*calls.lock().unwrap(), 3, "an outage and an HTTP 503 were retried, then sent");
    assert!(queue_file(&d).is_empty(), "the ignored one is not kept");
}

#[test]
fn a_rejected_batch_is_retried_one_by_one_and_only_the_bad_play_is_dropped() {
    let d = dir("rejected");
    connected(&d);
    let m = mock(Box::new(|f| {
        if f.values().any(|v| v == "bad") {
            (400, r#"{"error":6,"message":"Invalid parameters"}"#.into())
        } else {
            accept_all(f)
        }
    }));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    // Queued before the thread looks, so all three go in one batch.
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "bad"));
    run.s.scrobble(play(3, "C"));
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    let sent: Vec<String> = m.log.lock().unwrap().iter().filter(|f| !f.values().any(|v| v == "bad")).filter_map(|f| f.get("track[0]").cloned()).collect();
    assert!(sent.contains(&"A".to_owned()) && sent.contains(&"C".to_owned()), "{sent:?}");
    assert!(st.note.contains("not accepted") || st.note == "Sent 2 scrobbles", "{}", st.note);
}

#[test]
fn one_by_one_fallback_carries_musicbrainz_ids_and_clears_the_queue() {
    let d = dir("fallback-mbid");
    connected(&d);
    let m = mock(Box::new(|f| {
        if f.values().any(|v| v == "bad") {
            (400, r#"{"error":6,"message":"Invalid parameters"}"#.into())
        } else {
            accept_all(f)
        }
    }));
    // Queue while offline so both plays are sent together in one batch, then rejected and
    // retried one by one. play(1, "A") has path M:/music/01.flac, which start() maps to an MBID.
    let mut run = start(&d, &offline());
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "bad"));
    run.wait("both queued offline", |s| s.note.contains("could not be reached") && s.queued == 2);
    drop(run);

    let mut run = start(&d, &m.url);
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 1, 1 not accepted by Last.fm");
    let log = m.log.lock().unwrap().clone();
    assert_eq!(log[0].keys().filter(|k| k.starts_with("artist[")).count(), 2, "first try was the rejected batch");
    let fallback = log.iter().find(|f| f.get("track[0]").map(String::as_str) == Some("A") && !f.contains_key("track[1]")).expect("one-by-one fallback for A");
    assert_eq!(fallback.get("mbid[0]").map(String::as_str), Some("8f3471b5-7e6a-48da-86a9-c1c07a0f47ae"), "fallback carries the MBID");
    assert!(queue_file(&d).is_empty(), "original queued item removed");
}

#[test]
fn an_invalid_session_asks_to_reconnect_and_keeps_the_queue() {
    let d = dir("session");
    connected(&d);
    let m = mock(Box::new(|_| (403, r#"{"error":9,"message":"Invalid session key - Please re-authenticate"}"#.into())));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    let st = run.wait("reconnect", |s| s.state == State::Reconnect);
    assert_eq!(st.queued, 1, "kept for after reconnecting");
    assert!(st.accepting, "plays made meanwhile are queued too");
    std::thread::sleep(Duration::from_millis(600));
    run.s.scrobble(play(2, "B"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(m.log.lock().unwrap().len(), 1, "no retrying for ever");
    assert!(Credentials::read(&d.join("lastfm.json"), &Sealer::PLAIN).0.session_key.is_empty());
    assert_eq!(queue_file(&d).len(), 2);
}

#[test]
fn a_suspended_key_while_connected_holds_sending() {
    let d = dir("suspended");
    connected(&d);
    let m = mock(Box::new(|_| (403, r#"{"error":26,"message":"Suspended API key"}"#.into())));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    let st = run.wait("error", |s| matches!(s.state, State::Error(_)));
    assert!(matches!(&st.state, State::Error(e) if e.contains("Suspended")), "{st:?}");
    assert!(st.accepting, "plays made until the key is fixed are queued, not lost");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(m.log.lock().unwrap().len(), 1, "held until the user changes the key");
    assert_eq!(st.queued, 1);
}

#[test]
fn an_answer_that_is_not_last_fms_keeps_the_queue() {
    // A proxy asking for its password, a CDN's block page, a captive portal's page with a 200:
    // none of them is Last.fm refusing the plays.
    let d = dir("not-lastfm");
    connected(&d);
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let m = mock(Box::new(move |f| {
        *c.lock().unwrap() += 1;
        match *c.lock().unwrap() {
            1 => (407, "<html>Proxy Authentication Required</html>".into()),
            2 => (403, "<html>Access denied</html>".into()),
            3 => (200, "<html>Sign in to the Wi-Fi</html>".into()),
            4 => (429, String::new()),
            _ => accept_all(f),
        }
    }));
    let mut run = start(&d, &m.url);
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "B"));
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 2 scrobbles");
    assert_eq!(*calls.lock().unwrap(), 5, "each was retried later, then the batch went");
    assert!(queue_file(&d).is_empty());
}

#[test]
fn a_truncated_scrobble_reply_is_retried_and_keeps_the_queue() {
    let d = dir("truncated");
    connected(&d);
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let m = mock(Box::new(move |f| {
        assert_eq!(f.keys().filter(|k| k.starts_with("artist[")).count(), 2, "neither play dropped before a full reply");
        let n = {
            let mut g = c.lock().unwrap();
            *g += 1;
            *g
        };
        match n {
            1 => (200, "{}".into()),
            2 => (200, r#"{"scrobbles":{"@attr":{"accepted":2,"ignored":0}}}"#.into()),
            3 => (200, r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":0},"scrobble":{"ignoredMessage":{"code":"0","#text":""}}}}"##.into()),
            4 => (200, r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":0},"scrobble":[{"ignoredMessage":{"code":"0","#text":""}}]}}"##.into()),
            _ => accept_all(f),
        }
    }));
    // Queue while offline so both plays go together in one batch.
    let mut run = start(&d, &offline());
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "B"));
    run.wait("both queued offline", |s| s.note.contains("could not be reached") && s.queued == 2);
    drop(run);

    let mut run = start(&d, &m.url);
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 2 scrobbles");
    assert_eq!(*calls.lock().unwrap(), 5, "missing and short replies were retried until a full reply arrived");
    assert!(queue_file(&d).is_empty());
}

#[test]
fn plays_past_the_daily_limit_are_kept_for_later() {
    let d = dir("daily-limit");
    connected(&d);
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let m = mock(Box::new(move |f| {
        *c.lock().unwrap() += 1;
        if *c.lock().unwrap() == 1 {
            assert_eq!(f.keys().filter(|k| k.starts_with("artist[")).count(), 2);
            (
                200,
                r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":1},"scrobble":[{"ignoredMessage":{"code":"0","#text":""}},{"ignoredMessage":{"code":"5","#text":"Daily scrobble limit exceeded"}}]}}"##.into(),
            )
        } else {
            assert_eq!(f["track[0]"], "B");
            accept_all(f)
        }
    }));
    // Both plays queued while offline, so that they go in one batch: queued while connected,
    // the thread may send the first before the second is there.
    let mut run = start(&d, &offline());
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "A"));
    run.s.scrobble(play(2, "B"));
    run.wait("both queued offline", |s| s.note.contains("could not be reached") && s.queued == 2);
    drop(run);
    let mut run = start(&d, &m.url);
    let st = run.wait("limited", |s| s.note.contains("daily"));
    assert_eq!(st.queued, 1, "the play past the limit is kept");
    assert_eq!(queue_file(&d)[0].track, "B");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(*calls.lock().unwrap(), 1, "not sent again soon: the limit lasts the day");
    run.s.retry();
    run.wait("sent later", |s| s.queued == 0 && s.note == "Sent 1 scrobble");
}

#[test]
fn a_new_key_while_authorising_is_kept_and_connect_starts_again_with_it() {
    let d = dir("auth-new-key");
    let m = mock(Box::new(|f| match f["method"].as_str() {
        "auth.getToken" => (200, r#"{"token":"t"}"#.into()),
        "auth.getSession" => (403, r#"{"error":14,"message":"not yet"}"#.into()),
        m => panic!("unexpected {m}"),
    }));
    let mut run = start(&d, &m.url);
    run.s.set_credentials(KEY, SECRET);
    run.s.connect();
    run.wait("authorising", |s| matches!(s.state, State::Authorising { .. }));
    // The user corrects the secret and presses Connect again while the browser is open.
    run.s.set_credentials(KEY, "corrected-secret");
    run.s.connect();
    let st = run.wait("connect again, with the new secret", |s| matches!(s.state, State::Error(_)));
    assert!(matches!(&st.state, State::Error(e) if e.contains("API key or secret")), "the mock knows only the old one: {st:?}");
    assert_eq!(Credentials::read(&d.join("lastfm.json"), &Sealer::PLAIN).0.api_secret, "corrected-secret", "saved");
}

#[test]
fn a_redirect_to_another_server_is_refused_without_sending_the_body() {
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
            let body = r#"{"token":"leaked"}"#;
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = conn.write_all(resp.as_bytes());
        }
    });

    let redir = TcpListener::bind("127.0.0.1:0").unwrap();
    let redir_url = format!("http://127.0.0.1:{}/2.0/", redir.local_addr().unwrap().port());
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
            let status = if n.is_multiple_of(2) { "307 Temporary Redirect" } else { "308 Permanent Redirect" };
            n += 1;
            let resp = format!("HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:{trap_port}/2.0/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = conn.write_all(resp.as_bytes());
        }
    });

    let client = lastfm::Client::new(&redir_url, KEY, SECRET, "MarqueePlayer-test/1");
    assert!(matches!(client.scrobble("sk-secret", &[play(1, "A")]), Err(lastfm::ApiError::Network(_))));
    assert!(matches!(client.now_playing("sk-secret", &play(1, "A")), Err(lastfm::ApiError::Network(_))));
    assert!(matches!(client.get_token(), Err(lastfm::ApiError::Network(_))));
    assert!(matches!(client.get_session("tok-1"), Err(lastfm::ApiError::Network(_))));
    assert!(trapped.lock().unwrap().is_empty(), "other server received a redirected signed request: {:?}", trapped.lock().unwrap());
}

static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CaptureLog;

impl log::Log for CaptureLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("lastfm")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            LOG_LINES.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
        }
    }
    fn flush(&self) {}
}

fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        log::set_logger(&CaptureLog).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

#[test]
fn one_by_one_fallback_logs_why_an_ignored_play_was_dropped() {
    capture_logs();
    let d = dir("fallback-ignored");
    connected(&d);
    let m = mock(Box::new(|f| {
        if f.values().any(|v| v == "bad") {
            (400, r#"{"error":6,"message":"Invalid parameters"}"#.into())
        } else if f.get("track[0]").map(String::as_str) == Some("old") {
            (200, r##"{"scrobbles":{"@attr":{"accepted":0,"ignored":1},"scrobble":{"ignoredMessage":{"code":"3","#text":"Timestamp too old in the fallback"}}}}"##.into())
        } else {
            accept_all(f)
        }
    }));
    // Queue both plays while offline so they go out in one batch, which is rejected and
    // retried one by one.
    let mut run = start(&d, &offline());
    run.wait("connected", |s| matches!(s.state, State::Connected { .. }));
    run.s.scrobble(play(1, "old"));
    run.s.scrobble(play(2, "bad"));
    run.wait("both queued offline", |s| s.note.contains("could not be reached") && s.queued == 2);
    drop(run);

    let start_idx = LOG_LINES.lock().unwrap().len();
    let mut run = start(&d, &m.url);
    let st = run.wait("done", |s| s.queued == 0 && s.note.starts_with("Sent"));
    assert_eq!(st.note, "Sent 0, 2 not accepted by Last.fm");
    let lines = LOG_LINES.lock().unwrap()[start_idx..].to_vec();
    assert!(
        lines.iter().any(|l| l.contains("WARN") && l.contains("a scrobble was ignored (code 3: Timestamp too old in the fallback); dropped")),
        "expected ignored warning in fallback logs: {lines:?}"
    );
}
