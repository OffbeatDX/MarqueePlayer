//! The lyrics lookup against a local mock of the LRCLIB API (no request leaves this
//! machine): exact hits (synced, plain, instrumental), the search fallback taking a good
//! match and refusing wrong ones, the cache (hits without a request, rejection, retry),
//! lookups turned off, the wait before the network, a slow answer for a track no longer
//! playing, and HTTP and network errors.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use lrclib::{Answer, Cache, Client, Delivery, TrackInfo, Worker};

type Query = BTreeMap<String, String>;

/// (status, extra headers, body, delay before answering)
type Reply = (u16, Vec<String>, String, Duration);
type Handler = Box<dyn FnMut(&str, &Query) -> Reply + Send>;

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    query: Query,
    agent: String,
}

struct Mock {
    url: String,
    log: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn requests(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }
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
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let l = log.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let mut first = String::new();
            if r.read_line(&mut first).is_err() {
                continue;
            }
            let mut agent = String::new();
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("user-agent:") {
                    agent = v.trim().to_owned();
                }
            }
            assert!(first.starts_with("GET "), "{first}");
            let target = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (path, q) = target.split_once('?').unwrap_or((&target, ""));
            let query: Query = q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect();
            l.lock().unwrap().push(Seen { path: path.to_owned(), query: query.clone(), agent });
            let (status, headers, body, delay) = handler(path, &query);
            std::thread::sleep(delay);
            let mut extra = String::new();
            for h in headers {
                extra.push_str(&h);
                extra.push_str("\r\n");
            }
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = conn.write_all(resp.as_bytes());
        }
    });
    Mock { url, log }
}

fn ok(body: String) -> Reply {
    (200, vec![], body, Duration::ZERO)
}

fn not_found() -> Reply {
    (404, vec![], r#"{"message":"Failed to find specified track","name":"TrackNotFound","statusCode":404}"#.into(), Duration::ZERO)
}

/// An LRCLIB record as the API returns it.
struct Rec<'a> {
    id: i64,
    artist: &'a str,
    title: &'a str,
    album: &'a str,
    duration: f64,
    plain: Option<&'a str>,
    synced: Option<&'a str>,
    instrumental: bool,
}

fn record(r: Rec) -> String {
    let Rec { id, artist, title, album, duration, plain, synced, instrumental } = r;
    serde_json::json!({
        "id": id, "name": title, "trackName": title, "artistName": artist, "albumName": album,
        "duration": duration, "instrumental": instrumental, "hasWordSync": false,
        "plainLyrics": plain, "syncedLyrics": synced, "lyricsfile": "version: '1.0'"
    })
    .to_string()
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lrclib-mock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn track() -> TrackInfo {
    TrackInfo { artist: "Hikaru Test".into(), title: "First Love".into(), album: "First Love".into(), duration: 257.4 }
}

const AGENT: &str = "lrclib-tests/0.1";

fn worker(m: &Mock, cache: &Cache, enabled: bool) -> (Worker, Receiver<(u64, Delivery)>) {
    let (tx, rx) = unbounded();
    let w = Worker::spawn(
        Client::new(m.url.clone(), AGENT),
        cache.clone(),
        Arc::new(AtomicBool::new(enabled)),
        Box::new(move |g, d| {
            let _ = tx.send((g, d));
        }),
    );
    (w, rx)
}

fn next(rx: &Receiver<(u64, Delivery)>) -> (u64, Delivery) {
    rx.recv_timeout(Duration::from_secs(10)).expect("an answer")
}

#[test]
fn exact_hit_with_synced_lyrics() {
    let m = mock(Box::new(|path, _| match path {
        "/api/get" => ok(record(Rec { id: 11, artist: "Hikaru Test", title: "First Love", album: "First Love", duration: 257.0, plain: Some("Last kiss\nTasted"), synced: Some("[00:12.50] Last kiss\n[00:15.00] Tasted"), instrumental: false })),
        _ => not_found(),
    }));
    let cache = Cache::new(temp("exact"));
    let (w, rx) = worker(&m, &cache, true);
    w.lookup(1, track(), Duration::ZERO);
    let (g, d) = next(&rx);
    assert_eq!(g, 1);
    let Delivery::Answer(Answer::Found { id, lyrics }) = d else { panic!("{d:?}") };
    assert_eq!(id, 11);
    assert!(lyrics.synced);
    assert_eq!(lyrics.lines.iter().map(|l| (l.time_ms, l.text.as_str())).collect::<Vec<_>>(), [(Some(12_500), "Last kiss"), (Some(15_000), "Tasted")]);
    let reqs = m.requests();
    assert_eq!(reqs.len(), 1, "no search after an exact hit: {reqs:?}");
    let q = &reqs[0].query;
    assert_eq!(reqs[0].path, "/api/get");
    assert_eq!(q.get("artist_name").map(String::as_str), Some("Hikaru Test"));
    assert_eq!(q.get("track_name").map(String::as_str), Some("First Love"));
    assert_eq!(q.get("album_name").map(String::as_str), Some("First Love"));
    assert_eq!(q.get("duration").map(String::as_str), Some("257"), "whole seconds");
    assert_eq!(reqs[0].agent, AGENT.to_ascii_lowercase());

    // Cached: the same track again sends nothing and comes back at once.
    w.lookup(2, track(), Duration::from_secs(5));
    let (g, d) = rx.recv_timeout(Duration::from_secs(1)).expect("a cache hit does not wait");
    assert_eq!(g, 2);
    assert!(matches!(d, Delivery::Answer(Answer::Found { id: 11, .. })), "{d:?}");
    assert_eq!(m.requests().len(), 1);
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn plain_only_and_instrumental() {
    let m = mock(Box::new(|path, q| match (path, q.get("track_name").map(String::as_str)) {
        ("/api/get", Some("Plain")) => ok(record(Rec { id: 21, artist: "A", title: "Plain", album: "", duration: 100.0, plain: Some("line one\n\nline two"), synced: None, instrumental: false })),
        ("/api/get", Some("Instrumental")) => ok(record(Rec { id: 22, artist: "A", title: "Instrumental", album: "", duration: 100.0, plain: None, synced: None, instrumental: true })),
        _ => not_found(),
    }));
    let cache = Cache::new(temp("plain"));
    let (w, rx) = worker(&m, &cache, true);
    w.lookup(1, TrackInfo { artist: "A".into(), title: "Plain".into(), album: String::new(), duration: 100.0 }, Duration::ZERO);
    let (_, d) = next(&rx);
    let Delivery::Answer(Answer::Found { lyrics, .. }) = d else { panic!("{d:?}") };
    assert!(!lyrics.synced);
    assert_eq!(lyrics.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["line one", "", "line two"]);
    assert!(!m.requests()[0].query.contains_key("album_name"), "no album: not sent");
    w.lookup(2, TrackInfo { artist: "A".into(), title: "Instrumental".into(), album: String::new(), duration: 100.0 }, Duration::ZERO);
    assert_eq!(next(&rx), (2, Delivery::Answer(Answer::Instrumental { id: 22 })));
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn search_fallback_takes_a_good_match_and_refuses_wrong_ones() {
    let m = mock(Box::new(|path, q| match (path, q.get("track_name").map(String::as_str)) {
        ("/api/get", _) => not_found(),
        ("/api/search", Some("First Love")) => ok(format!(
            "[{},{},{},{}]",
            record(Rec { id: 31, artist: "Someone Else", title: "First Love", album: "Cover", duration: 257.0, plain: Some("wrong artist"), synced: None, instrumental: false }),
            record(Rec { id: 32, artist: "Hikaru Test", title: "First Love", album: "Live", duration: 301.0, plain: Some("live edit"), synced: None, instrumental: false }),
            record(Rec { id: 33, artist: "hikaru test", title: "First Love", album: "Single", duration: 258.0, plain: Some("plain"), synced: None, instrumental: false }),
            record(Rec { id: 34, artist: "Hikaru Test", title: "First Love", album: "First Love", duration: 256.0, plain: Some("right"), synced: Some("[00:01.00]right"), instrumental: false }),
        )),
        ("/api/search", Some("Wrong Only")) => ok(format!(
            "[{},{}]",
            record(Rec { id: 41, artist: "Hikaru Test", title: "Wrong Only (Remix)", album: "", duration: 200.0, plain: Some("remix"), synced: None, instrumental: false }),
            record(Rec { id: 42, artist: "Hikaru Test", title: "Wrong Only", album: "", duration: 230.0, plain: Some("other edit"), synced: None, instrumental: false }),
        )),
        _ => ok("[]".into()),
    }));
    let cache = Cache::new(temp("search"));
    let (w, rx) = worker(&m, &cache, true);
    w.lookup(1, track(), Duration::ZERO);
    let (_, d) = next(&rx);
    let Delivery::Answer(Answer::Found { id, lyrics }) = d else { panic!("{d:?}") };
    assert_eq!(id, 34, "the same album and synced, within 2 s");
    assert!(lyrics.synced);
    let reqs = m.requests();
    assert_eq!(reqs.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/api/get", "/api/search"]);
    assert_eq!(reqs[1].query.get("artist_name").map(String::as_str), Some("Hikaru Test"));

    let wrong = TrackInfo { artist: "Hikaru Test".into(), title: "Wrong Only".into(), album: String::new(), duration: 200.0 };
    w.lookup(2, wrong.clone(), Duration::ZERO);
    assert_eq!(next(&rx), (2, Delivery::Answer(Answer::NotFound)), "a remix and another edit are refused");
    // Not found is cached: no request the second time.
    let before = m.requests().len();
    w.lookup(3, wrong, Duration::ZERO);
    assert_eq!(next(&rx), (3, Delivery::Answer(Answer::NotFound)));
    assert_eq!(m.requests().len(), before);
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn rejection_and_retry() {
    let m = mock(Box::new(|path, _| match path {
        "/api/get" => ok(record(Rec { id: 51, artist: "Hikaru Test", title: "First Love", album: "First Love", duration: 257.0, plain: Some("wrong words"), synced: None, instrumental: false })),
        "/api/search" => ok(format!("[{}]", record(Rec { id: 51, artist: "Hikaru Test", title: "First Love", album: "First Love", duration: 257.0, plain: Some("wrong words"), synced: None, instrumental: false }))),
        _ => not_found(),
    }));
    let cache = Cache::new(temp("reject"));
    let (w, rx) = worker(&m, &cache, true);
    w.lookup(1, track(), Duration::ZERO);
    assert!(matches!(next(&rx).1, Delivery::Answer(Answer::Found { id: 51, .. })));
    // The user says they are wrong: gone from the cache, and never looked up again by itself.
    w.reject(track());
    let n = m.requests().len();
    w.lookup(2, track(), Duration::ZERO);
    assert_eq!(next(&rx), (2, Delivery::Answer(Answer::Rejected)));
    assert_eq!(m.requests().len(), n, "a rejected track sends nothing");
    // Try again: it asks, but the same record is refused, from the exact lookup and the search.
    w.retry(3, track());
    assert_eq!(next(&rx), (3, Delivery::Answer(Answer::NotFound)));
    let paths: Vec<_> = m.requests()[n..].iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, ["/api/get", "/api/search"]);
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn worker_reject_after_an_in_flight_put_wins() {
    let m = mock(Box::new(|path, _| match path {
        "/api/get" => (
            200,
            vec![],
            record(Rec { id: 61, artist: "Hikaru Test", title: "First Love", album: "First Love", duration: 257.0, plain: Some("slow wrong words"), synced: None, instrumental: false }),
            Duration::from_millis(80),
        ),
        _ => not_found(),
    }));
    let cache = Cache::new(temp("reject-order"));
    let (w, rx) = worker(&m, &cache, true);
    w.lookup(1, track(), Duration::ZERO);
    // Reject sent right after lookup: must run after the lookup's cache.put and win.
    w.reject(track());
    assert!(matches!(next(&rx).1, Delivery::Answer(Answer::Found { id: 61, .. })));
    w.lookup(2, track(), Duration::ZERO);
    assert_eq!(next(&rx), (2, Delivery::Answer(Answer::Rejected)));
    assert_eq!(cache.get(&track()).rejected, vec![61]);
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn disabled_sends_nothing() {
    let m = mock(Box::new(|_, _| ok("{}".into())));
    let cache = Cache::new(temp("off"));
    let (w, rx) = worker(&m, &cache, false);
    w.lookup(1, track(), Duration::ZERO);
    assert_eq!(next(&rx), (1, Delivery::Disabled));
    w.retry(2, track());
    assert_eq!(next(&rx), (2, Delivery::Disabled));
    std::thread::sleep(Duration::from_millis(200));
    assert!(m.requests().is_empty(), "{:?}", m.requests());
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn skipping_through_tracks_sends_only_the_last() {
    let m = mock(Box::new(|_, _| not_found()));
    let cache = Cache::new(temp("skip"));
    let (w, rx) = worker(&m, &cache, true);
    for (g, title) in [(1, "One"), (2, "Two"), (3, "Three"), (4, "Four")] {
        w.lookup(g, TrackInfo { title: title.into(), ..track() }, Duration::from_millis(400));
        std::thread::sleep(Duration::from_millis(80));
    }
    let (g, d) = next(&rx);
    assert_eq!((g, d), (4, Delivery::Answer(Answer::NotFound)));
    let titles: Vec<_> = m.requests().iter().filter_map(|r| r.query.get("track_name").cloned()).collect();
    assert_eq!(titles, ["Four", "Four"], "the exact lookup and the search, for the last track only");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing for the skipped ones");
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn a_slow_answer_for_an_old_track_is_dropped_but_cached() {
    let m = mock(Box::new(|path, q| match (path, q.get("track_name").map(String::as_str)) {
        ("/api/get", Some("Slow")) => {
            let mut r = ok(record(Rec { id: 61, artist: "Hikaru Test", title: "Slow", album: "", duration: 200.0, plain: Some("slow words"), synced: None, instrumental: false }));
            r.3 = Duration::from_millis(800);
            r
        }
        ("/api/get", Some("Fast")) => ok(record(Rec { id: 62, artist: "Hikaru Test", title: "Fast", album: "", duration: 200.0, plain: Some("fast words"), synced: None, instrumental: false })),
        _ => not_found(),
    }));
    let cache = Cache::new(temp("slow"));
    let (w, rx) = worker(&m, &cache, true);
    let slow = TrackInfo { artist: "Hikaru Test".into(), title: "Slow".into(), album: String::new(), duration: 200.0 };
    let fast = TrackInfo { title: "Fast".into(), ..slow.clone() };
    w.lookup(1, slow.clone(), Duration::ZERO);
    std::thread::sleep(Duration::from_millis(200));
    // The track changed while the request was out.
    w.lookup(2, fast, Duration::ZERO);
    let (g, d) = next(&rx);
    assert_eq!(g, 2, "only the playing track's answer comes back");
    assert!(matches!(d, Delivery::Answer(Answer::Found { id: 62, .. })), "{d:?}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(matches!(cache.get(&slow).answer, Some(Answer::Found { id: 61, .. })), "the slow answer was still kept");
    let _ = std::fs::remove_dir_all(cache.dir());
}

#[test]
fn http_and_network_errors_are_not_cached() {
    let busy_count = Arc::new(Mutex::new(0));
    let bc = busy_count.clone();
    let m = mock(Box::new(move |_, q| match q.get("track_name").map(String::as_str) {
        Some("Broken") => (500, vec![], "oops".into(), Duration::ZERO),
        Some("Garbled") => ok("<html>not json</html>".into()),
        Some("Busy") => {
            *bc.lock().unwrap() += 1;
            (429, vec!["Retry-After: 2".into()], "{}".into(), Duration::ZERO)
        }
        _ => not_found(),
    }));
    let cache = Cache::new(temp("errors"));
    let (w, rx) = worker(&m, &cache, true);
    let t = |title: &str| TrackInfo { title: title.into(), ..track() };
    w.lookup(1, t("Broken"), Duration::ZERO);
    assert!(matches!(next(&rx), (1, Delivery::Failed(m)) if m.contains("500")));
    assert_eq!(cache.get(&t("Broken")).answer, None, "an error is not cached");
    w.lookup(2, t("Broken"), Duration::ZERO);
    assert!(matches!(next(&rx), (2, Delivery::Failed(_))));
    assert_eq!(m.requests().len(), 2, "so it is asked again");
    w.lookup(3, t("Garbled"), Duration::ZERO);
    assert!(matches!(next(&rx), (3, Delivery::Failed(_))));
    // 429: everything waits for Retry-After, without a request.
    w.lookup(4, t("Busy"), Duration::ZERO);
    assert!(matches!(next(&rx), (4, Delivery::Failed(_))));
    w.lookup(5, t("Busy"), Duration::ZERO);
    assert!(matches!(next(&rx), (5, Delivery::Failed(_))));
    assert_eq!(*busy_count.lock().unwrap(), 1, "no request while backing off");
    std::thread::sleep(Duration::from_millis(2700));
    w.lookup(6, t("Busy"), Duration::ZERO);
    assert!(matches!(next(&rx), (6, Delivery::Failed(_))));
    assert_eq!(*busy_count.lock().unwrap(), 2, "asked again after Retry-After");
    let _ = std::fs::remove_dir_all(cache.dir());

    // Nothing listening: offline.
    let port = { TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() };
    let cache = Cache::new(temp("offline"));
    let (tx, rx) = unbounded();
    let w = Worker::spawn(
        Client::new(format!("http://127.0.0.1:{port}"), AGENT),
        cache.clone(),
        Arc::new(AtomicBool::new(true)),
        Box::new(move |g, d| {
            let _ = tx.send((g, d));
        }),
    );
    w.lookup(1, track(), Duration::ZERO);
    assert!(matches!(next(&rx), (1, Delivery::Offline(_))));
    assert_eq!(cache.get(&track()).answer, None);
    let _ = std::fs::remove_dir_all(cache.dir());
}
