//! The client against a local mock server on 127.0.0.1 (no request leaves this machine):
//! binary bodies, chunked and sized bodies arriving piece by piece, a Range request, a
//! server that stops mid-body, a stall that hits the timeout, a stop from the sink, a
//! redirect. Each test writes the raw HTTP it needs, so the client sees exactly what a
//! server could send.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_client::{ContentRange, Download, Head, HttpError, Request, Sink};

const AGENT: &str = "MarqueePlayer-test/1";

/// What the mock read of one request.
#[derive(Clone, Debug, Default)]
struct Seen {
    line: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn path(&self) -> &str {
        self.line.split(' ').nth(1).unwrap_or("")
    }
}

type Handler = Arc<dyn Fn(&Seen, &mut TcpStream) + Send + Sync>;

/// A server that answers each connection with `handler`, on its own thread; the URL of
/// `path` on it, and what it saw.
fn serve(path: &str, handler: impl Fn(&Seen, &mut TcpStream) + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}{path}", listener.local_addr().unwrap().port());
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let s = seen.clone();
    let handler: Handler = Arc::new(handler);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let (s, handler) = (s.clone(), handler.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut req = Seen::default();
                if r.read_line(&mut req.line).unwrap_or(0) == 0 {
                    return;
                }
                req.line = req.line.trim_end().to_owned();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        req.headers.push((k.trim().to_owned(), v.trim().to_owned()));
                    }
                }
                s.lock().unwrap().push(req.clone());
                handler(&req, &mut conn);
                let _ = conn.flush();
            });
        }
    });
    (url, seen)
}

fn get<'a>(url: &'a str, headers: &'a [&'a str], timeout_ms: u32) -> Request<'a> {
    Request { method: "GET", url, headers, body: &[], user_agent: AGENT, timeout_ms }
}

/// Every byte value, `n` bytes long: a body that text handling would spoil.
fn binary(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 256) as u8).collect()
}

/// A sink that keeps everything, and optionally stops.
#[derive(Default)]
struct Keep {
    head: Option<Head>,
    body: Vec<u8>,
    chunks: usize,
    stop_at_head: bool,
    /// Stop once this many bytes are in.
    stop_after: Option<usize>,
    /// Told of each chunk.
    tell: Option<mpsc::Sender<usize>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Sink for Keep {
    fn head(&mut self, head: &Head) -> ControlFlow<()> {
        self.head = Some(head.clone());
        if self.stop_at_head { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }
    fn chunk(&mut self, bytes: &[u8]) -> ControlFlow<()> {
        self.body.extend_from_slice(bytes);
        self.chunks += 1;
        if let Some(t) = &self.tell {
            let _ = t.send(self.body.len());
        }
        if self.stop_after.is_some_and(|n| self.body.len() >= n) || self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

fn write_head(conn: &mut TcpStream, status: &str, headers: &[String]) {
    let mut s = format!("HTTP/1.1 {status}\r\n");
    for h in headers {
        s.push_str(h);
        s.push_str("\r\n");
    }
    s.push_str("Connection: close\r\n\r\n");
    conn.write_all(s.as_bytes()).unwrap();
}

#[test]
fn a_binary_body_comes_back_intact_with_its_type() {
    let data = binary(300_000);
    let body = data.clone();
    let (url, seen) = serve("/cover", move |_, c| {
        write_head(c, "200 OK", &["Content-Type: image/png".into(), format!("Content-Length: {}", body.len())]);
        c.write_all(&body).unwrap();
    });
    let r = http_client::request_bytes(&get(&url, &[], 5000)).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type.as_deref(), Some("image/png"));
    assert!(r.body == data, "the bytes changed ({} came back)", r.body.len());
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].header("User-Agent"), Some(AGENT));
    // Nothing asks for a compressed body: it must come as sent.
    assert!(seen[0].header("Accept-Encoding").is_none_or(|v| !v.contains("gzip")));
}

#[test]
fn text_calls_still_read_whole_answers() {
    let (url, _) = serve("/api", |_, c| {
        write_head(c, "429 Too Many Requests", &["Retry-After: 7".into(), "X-RateLimit-Remaining: 0".into(), "x-ratelimit-reset-in: 12".into(), "Content-Length: 2".into()]);
        c.write_all(b"{}").unwrap();
    });
    let r = http_client::get(&url, AGENT, 5000).unwrap();
    assert_eq!((r.status, r.body.as_str(), r.retry_after), (429, "{}", Some(7)));
    // ListenBrainz's rate-limit headers, whatever their case.
    assert_eq!(r.rate_limit, http_client::RateLimit { remaining: Some(0), reset_in: Some(12) });
}

#[test]
fn a_chunked_body_arrives_as_it_is_sent() {
    let data = binary(200_000);
    let body = data.clone();
    // The server sends half, then waits until the client has some of it: a client that
    // waited for the whole body would never get there.
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let (url, _) = serve("/stream", move |_, c| {
        write_head(c, "200 OK", &["Content-Type: audio/ogg".into(), "Transfer-Encoding: chunked".into()]);
        let (a, b) = body.split_at(body.len() / 2);
        for part in a.chunks(7000) {
            c.write_all(format!("{:x}\r\n", part.len()).as_bytes()).unwrap();
            c.write_all(part).unwrap();
            c.write_all(b"\r\n").unwrap();
        }
        c.flush().unwrap();
        go_rx.lock().unwrap().recv_timeout(Duration::from_secs(10)).expect("the client never had the first half");
        for part in b.chunks(9000) {
            c.write_all(format!("{:x}\r\n", part.len()).as_bytes()).unwrap();
            c.write_all(part).unwrap();
            c.write_all(b"\r\n").unwrap();
        }
        c.write_all(b"0\r\n\r\n").unwrap();
    });
    let (tell, told) = mpsc::channel();
    std::thread::spawn(move || {
        if told.recv().is_ok() {
            let _ = go_tx.send(());
        }
        while told.recv().is_ok() {}
    });
    let mut sink = Keep { tell: Some(tell), ..Keep::default() };
    let d = http_client::download(&get(&url, &[], 5000), &mut sink).unwrap();
    assert!(!d.stopped);
    assert_eq!(d.head.status, 200);
    assert_eq!(d.head.content_length, None, "chunked: no length");
    assert_eq!(d.head.content_type.as_deref(), Some("audio/ogg"));
    assert_eq!(d.received, data.len() as u64);
    assert!(sink.body == data);
    assert!(sink.chunks > 1);
}

#[test]
fn a_sized_body_reports_its_length_and_streams() {
    let data = binary(1_000_000);
    let body = data.clone();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let (url, _) = serve("/file", move |_, c| {
        write_head(c, "200 OK", &["Content-Type: audio/flac".into(), format!("Content-Length: {}", body.len()), "Accept-Ranges: bytes".into()]);
        c.write_all(&body[..100_000]).unwrap();
        c.flush().unwrap();
        go_rx.lock().unwrap().recv_timeout(Duration::from_secs(10)).expect("the client never had the start");
        c.write_all(&body[100_000..]).unwrap();
    });
    let (tell, told) = mpsc::channel();
    std::thread::spawn(move || {
        if told.recv().is_ok() {
            let _ = go_tx.send(());
        }
        while told.recv().is_ok() {}
    });
    let mut sink = Keep { tell: Some(tell), ..Keep::default() };
    let d = http_client::download(&get(&url, &[], 5000), &mut sink).unwrap();
    assert_eq!(d.head.content_length, Some(data.len() as u64));
    assert_eq!(d.head.range(), None);
    assert_eq!(d.received, data.len() as u64);
    assert!(sink.body == data);
    assert!(sink.chunks >= 16, "chunks of at most 64 KiB, {} came", sink.chunks);
}

#[test]
fn a_range_request_gets_the_rest_of_the_file() {
    let data = binary(50_000);
    let file = data.clone();
    let (url, seen) = serve("/file", move |req, c| {
        let start: usize = req.header("Range").and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.strip_suffix('-')).map_or(0, |s| s.parse().unwrap());
        let part = &file[start..];
        write_head(
            c,
            "206 Partial Content",
            &["Content-Type: audio/mpeg".into(), format!("Content-Length: {}", part.len()), format!("Content-Range: bytes {start}-{}/{}", file.len() - 1, file.len())],
        );
        c.write_all(part).unwrap();
    });
    let mut sink = Keep::default();
    let d = http_client::download(&get(&url, &["Range: bytes=12345-"], 5000), &mut sink).unwrap();
    assert_eq!(seen.lock().unwrap()[0].header("Range"), Some("bytes=12345-"));
    assert_eq!(d.head.status, 206);
    assert_eq!(d.head.range(), Some(ContentRange { first: 12345, last: 49_999, total: Some(50_000) }));
    assert_eq!(d.head.content_length, Some(50_000 - 12345));
    assert!(sink.body == data[12345..]);
}

#[test]
fn a_body_cut_short_is_an_error_after_the_bytes_that_came() {
    let data = binary(10_000);
    let body = data.clone();
    let (url, _) = serve("/file", move |_, c| {
        write_head(c, "200 OK", &["Content-Length: 10000".into()]);
        c.write_all(&body[..4000]).unwrap();
        c.flush().unwrap();
        let _ = c.shutdown(std::net::Shutdown::Both);
    });
    let mut sink = Keep::default();
    let e = http_client::download(&get(&url, &[], 5000), &mut sink).unwrap_err();
    assert!(matches!(e, HttpError::Network(_)));
    assert_eq!(sink.body, data[..4000], "every byte that came was given");
    // The buffered call fails too, instead of handing back a short body.
    assert!(http_client::request_bytes(&get(&url, &[], 5000)).is_err());
}

#[test]
fn a_chunked_body_cut_short_is_an_error() {
    let (url, _) = serve("/stream", |_, c| {
        write_head(c, "200 OK", &["Transfer-Encoding: chunked".into()]);
        c.write_all(b"1000\r\n").unwrap();
        c.write_all(&[7u8; 0x1000]).unwrap();
        c.write_all(b"\r\n1000\r\n").unwrap();
        c.write_all(&[8u8; 100]).unwrap();
        c.flush().unwrap();
        let _ = c.shutdown(std::net::Shutdown::Both);
    });
    let mut sink = Keep::default();
    let r = http_client::download(&get(&url, &[], 5000), &mut sink);
    assert!(r.is_err(), "{r:?}");
    assert!(sink.body.len() >= 0x1000);
}

/// A server that sends a little every 20 ms for as long as anyone listens.
fn endless() -> String {
    serve("/endless", |_, c| {
        write_head(c, "200 OK", &["Content-Type: audio/ogg".into(), "Transfer-Encoding: chunked".into()]);
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if c.write_all(b"400\r\n").and_then(|_| c.write_all(&[1u8; 0x400])).and_then(|_| c.write_all(b"\r\n")).and_then(|_| c.flush()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })
    .0
}

#[test]
fn the_sink_stops_a_download_at_once() {
    let url = endless();
    let t = Instant::now();
    let mut sink = Keep { stop_after: Some(5 * 0x400), ..Keep::default() };
    let d: Download = http_client::download(&get(&url, &[], 20_000), &mut sink).unwrap();
    assert!(d.stopped);
    assert!(d.received >= 5 * 0x400);
    assert!(t.elapsed() < Duration::from_secs(3), "stopped after {:?}", t.elapsed());
}

#[test]
fn a_cancel_flag_from_another_thread_stops_it() {
    let url = endless();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c.store(true, Ordering::Relaxed);
    });
    let t = Instant::now();
    let mut sink = Keep { cancel: Some(cancel), ..Keep::default() };
    let d = http_client::download(&get(&url, &[], 20_000), &mut sink).unwrap();
    assert!(d.stopped);
    assert!(t.elapsed() < Duration::from_secs(3), "stopped after {:?}", t.elapsed());
}

#[test]
fn the_head_can_refuse_the_body() {
    let url = endless();
    let mut sink = Keep { stop_at_head: true, ..Keep::default() };
    let d = http_client::download(&get(&url, &[], 20_000), &mut sink).unwrap();
    assert!(d.stopped);
    assert_eq!(d.received, 0);
    assert_eq!(d.head.status, 200);
}

#[test]
fn a_stall_ends_in_an_error_after_the_timeout() {
    let (url, _) = serve("/stall", |_, c| {
        write_head(c, "200 OK", &["Content-Length: 100000".into()]);
        c.write_all(&[3u8; 1000]).unwrap();
        c.flush().unwrap();
        std::thread::sleep(Duration::from_secs(8));
    });
    let t = Instant::now();
    let mut sink = Keep::default();
    let e = http_client::download(&get(&url, &[], 600), &mut sink).unwrap_err();
    let took = t.elapsed();
    assert!(matches!(e, HttpError::Network(_)));
    assert_eq!(sink.body.len(), 1000);
    assert!(took >= Duration::from_millis(500) && took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn a_silent_server_ends_in_an_error_after_the_timeout() {
    let (url, _) = serve("/silent", |_, _| std::thread::sleep(Duration::from_secs(8)));
    let t = Instant::now();
    assert!(http_client::request_bytes(&get(&url, &[], 600)).is_err());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

/// Requests share one session (the same user agent) but not their timeouts: a long wait after a
/// short timeout still succeeds, and a short timeout after a long one still ends the wait. (How
/// soon WinHTTP gives up on a silent server is its own: seconds, not the milliseconds asked.)
#[test]
fn each_request_keeps_its_own_timeout() {
    let (url, _) = serve("/x", |req, c| {
        if req.path() == "/silent" {
            std::thread::sleep(Duration::from_secs(12));
            return;
        }
        std::thread::sleep(Duration::from_millis(1500));
        write_head(c, "200 OK", &["Content-Length: 2".into()]);
        let _ = c.write_all(b"ok");
    });
    let silent = url.replace("/x", "/silent");
    let short = || {
        let t = Instant::now();
        let r = http_client::request_bytes(&get(&silent, &[], 600));
        (r.is_err(), t.elapsed())
    };
    let long = || http_client::request_bytes(&get(&url, &[], 10_000)).map(|r| r.body);
    let (failed, took) = short();
    assert!(failed && took < Duration::from_secs(9), "{took:?}");
    assert_eq!(long().unwrap(), b"ok", "the short timeout did not stay with the session");
    let (failed, took) = short();
    assert!(failed && took < Duration::from_secs(9), "nor the long one: {took:?}");
}

#[test]
fn redirects_are_followed() {
    let (url, seen) = serve("/old", |req, c| {
        if req.path() == "/old" {
            write_head(c, "302 Found", &["Location: /new?x=1".into(), "Content-Length: 0".into()]);
        } else {
            write_head(c, "200 OK", &["Content-Type: image/jpeg".into(), "Content-Length: 3".into()]);
            c.write_all(&[0xff, 0xd8, 0xff]).unwrap();
        }
    });
    let r = http_client::request_bytes(&get(&url, &[], 5000)).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &[0xff, 0xd8, 0xff][..]));
    let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| s.path().to_owned()).collect();
    assert_eq!(paths, ["/old", "/new?x=1"]);
}

#[test]
fn an_unreachable_server_is_a_network_error_without_the_url() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let url = format!("http://127.0.0.1:{port}/rest/ping?t=secrettoken&s=secretsalt");
    let e = http_client::request_bytes(&get(&url, &[], 3000)).unwrap_err();
    let text = e.to_string();
    assert!(!text.contains("secret"), "{text}");
}

#[test]
fn a_sign_in_follows_a_redirect_on_the_same_server_only() {
    // The same scheme, host and port: followed, the Authorization header sent again.
    let (url, seen) = serve("/1/submit-listens", |req, c| {
        if req.path() == "/1/submit-listens" {
            write_head(c, "308 Permanent Redirect", &["Location: /1/submit-listens/".into(), "Content-Length: 0".into()]);
        } else {
            write_head(c, "200 OK", &["Content-Length: 2".into()]);
            c.write_all(b"ok").unwrap();
        }
    });
    let r = http_client::request(&get(&url, &["Authorization: Token secret-token"], 5000)).unwrap();
    assert_eq!((r.status, r.body.as_str()), (200, "ok"));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.iter().map(|s| s.path()).collect::<Vec<_>>(), ["/1/submit-listens", "/1/submit-listens/"]);
    assert!(seen.iter().all(|s| s.header("Authorization") == Some("Token secret-token")), "{seen:?}");

    // Another port (another server): not followed, an error, and the token never goes there.
    let (other, other_seen) = serve("/elsewhere", |_, c| write_head(c, "200 OK", &["Content-Length: 0".into()]));
    let (url, _) = serve("/1/validate-token", move |_, c| write_head(c, "301 Moved Permanently", &[format!("Location: {other}"), "Content-Length: 0".into()]));
    let e = http_client::request(&get(&url, &["Authorization: Token secret-token"], 5000)).unwrap_err();
    assert!(e.to_string().contains("another address") && !e.to_string().contains("secret"), "{e}");
    assert!(other_seen.lock().unwrap().is_empty(), "the other server was never asked");

    // Without a sign-in, a redirect anywhere is followed as before.
    let (other, _) = serve("/x", |_, c| {
        write_head(c, "200 OK", &["Content-Length: 1".into()]);
        c.write_all(b"y").unwrap();
    });
    let (url, _) = serve("/cover", move |_, c| write_head(c, "302 Found", &[format!("Location: {other}"), "Content-Length: 0".into()]));
    assert_eq!(http_client::request(&get(&url, &[], 5000)).unwrap().body, "y");
}
