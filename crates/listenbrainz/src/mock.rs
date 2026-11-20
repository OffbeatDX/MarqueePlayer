//! A stand-in for ListenBrainz on 127.0.0.1, for the crate's and the app's tests and the app's
//! bench (feature `mock`; never in a release build): no request made against it leaves the
//! machine.
//!
//! It answers `GET /1/validate-token`, `POST /1/submit-listens` and `POST
//! /1/feedback/recording-feedback` as the API documentation says, checking each request against
//! the documented rules (`Authorization: Token <token>`; `listen_type` `single` or `playing_now`
//! with exactly one listen, `import` with 1 to 1000; no `listened_at` for `playing_now`, an
//! integer from `LISTEN_MINIMUM_TS` on otherwise; `artist_name` and `track_name`; well-formed
//! MusicBrainz ids; each listen within 10 KB, the body within 10 MB): a request that breaks one is
//! answered 400 and kept in [`Server::violations`]. It stores listens as ListenBrainz does, once
//! per user, time and track ([`Server::listens`]), sends the rate-limit headers on every answer
//! and can be made to misbehave ([`Trouble`]). It keeps what each request was
//! ([`Server::requests`]), never the token.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::limits::*;
use crate::api::mbid;

/// How the stand-in misbehaves, until told otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Trouble {
    #[default]
    None,
    /// Every request: the connection closed without an answer (as if offline).
    Offline,
    /// Every request: 503.
    Down,
    /// The next `times` requests: 429, with `X-RateLimit-Remaining: 0` and
    /// `X-RateLimit-Reset-In: reset_in`.
    RateLimited { times: u32, reset_in: u64 },
    /// Every request: 401, whatever the token.
    BadToken,
    /// Every submission: 401 with ListenBrainz's words for an account without a verified e-mail
    /// address; validate-token still says the token is valid.
    NoEmail,
    /// The next `times` submissions are stored, then the connection is closed without an answer
    /// (the answer lost on the way).
    LoseAnswer { times: u32 },
    /// Every request: this status, the body's `error` repeating the whole request head
    /// (`Authorization` included), as a careless server or proxy might.
    Echo(u16),
    /// Every request: read, then no answer at all, the connection held open (a server that
    /// hangs: the client waits for its timeout).
    Silent,
    /// Every submission: 400, whatever it holds (a server that refuses every listen).
    RefuseListens,
}

/// One request as the stand-in saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// The `Authorization` header was `Token <a token it knows>`.
    pub token_ok: bool,
    /// `listen_type` of a submission ("" for another request).
    pub listen_type: String,
    /// Listens in a submission.
    pub listens: usize,
    /// The body's size in bytes.
    pub bytes: usize,
    pub status: u16,
    pub user_agent: String,
    pub at: Instant,
}

/// What the stand-in knows and has been sent.
#[derive(Debug, Default)]
pub struct Server {
    /// Token → user name.
    pub tokens: HashMap<String, String>,
    pub trouble: Trouble,
    /// A submission holding a listen with this `track_name` is refused with a 400.
    pub reject_track: Option<String>,
    /// The rate-limit window every answer reports: requests allowed, seconds until it resets.
    /// Running out of it is a 429 until it resets.
    pub window: (u64, u64),
    used: u64,
    window_start: Option<Instant>,
    /// Every listen stored (`single`, `import`), as sent, once per user, time and track.
    pub listens: Vec<Value>,
    /// Submissions that were all listens already stored.
    pub duplicates: usize,
    /// `playing_now` listens, as sent.
    pub playing_now: Vec<Value>,
    /// Feedback bodies, as sent.
    pub feedback: Vec<Value>,
    pub requests: Vec<Seen>,
    /// Documented rules a request broke.
    pub violations: Vec<String>,
    keys: Vec<(String, i64, String)>,
    /// The rate-limit headers of the next answer, instead of the window's.
    headers_once: Option<(u64, u64)>,
}

pub struct Mock {
    pub url: String,
    pub server: Arc<Mutex<Server>>,
}

impl Mock {
    /// Start it on a free port of 127.0.0.1, knowing `token` as `user`.
    pub fn start(token: &str, user: &str) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port for the ListenBrainz stand-in");
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = Arc::new(Mutex::new(Server { window: (1000, 60), ..Server::default() }));
        server.lock().unwrap().tokens.insert(token.into(), user.into());
        let s = server.clone();
        std::thread::Builder::new()
            .name("mp-listenbrainz-mock".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    let s = s.clone();
                    // One at a time, in order: what the app sends is sequential anyway.
                    handle(conn, &s);
                }
            })
            .expect("spawn the ListenBrainz stand-in");
        Mock { url, server }
    }

    pub fn set_trouble(&self, t: Trouble) {
        self.server.lock().unwrap().trouble = t;
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Server) -> T) -> T {
        f(&mut self.server.lock().unwrap())
    }

    /// The stored listens' track names, in the order they came.
    pub fn tracks(&self) -> Vec<String> {
        self.with(|s| s.listens.iter().map(|l| l["track_metadata"]["track_name"].as_str().unwrap_or("").to_owned()).collect())
    }
}

/// A port nothing listens on: every request fails at once, as if offline.
pub fn offline_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    head: String,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn read(conn: &TcpStream) -> Option<Request> {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(conn);
    let mut first = String::new();
    r.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.split('?').next()?.to_owned();
    let mut headers = Vec::new();
    let mut head = first.clone();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        if h == "\r\n" || h.is_empty() {
            break;
        }
        head.push_str(&h);
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0usize);
    let mut body = vec![0; len.min(MAX_LISTEN_PAYLOAD_SIZE * 2)];
    r.read_exact(&mut body).ok()?;
    Some(Request { method, path, headers, head, body })
}

fn answer(mut conn: &TcpStream, status: u16, body: &Value, s: &mut Server) {
    let body = body.to_string();
    let (remaining, reset) = s.headers_once.take().unwrap_or_else(|| (s.window.0.saturating_sub(s.used), s.window_start.map_or(s.window.1, |t| s.window.1.saturating_sub(t.elapsed().as_secs()))));
    let resp = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nX-RateLimit-Limit: {}\r\nX-RateLimit-Remaining: {remaining}\r\nX-RateLimit-Reset-In: {reset}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        s.window.0,
        body.len()
    );
    let _ = conn.write_all(resp.as_bytes());
}

fn handle(conn: TcpStream, shared: &Mutex<Server>) {
    let Some(req) = read(&conn) else { return };
    let mut s = shared.lock().unwrap();
    let token = req.header("Authorization").and_then(|a| a.strip_prefix("Token ")).map(str::trim).unwrap_or("").to_owned();
    let user = s.tokens.get(&token).cloned();
    let body: Option<Value> = serde_json::from_slice(&req.body).ok();
    let mut seen = Seen {
        method: req.method.clone(),
        path: req.path.clone(),
        token_ok: user.is_some(),
        listen_type: body.as_ref().and_then(|b| b.get("listen_type")).and_then(Value::as_str).unwrap_or("").to_owned(),
        listens: body.as_ref().and_then(|b| b.get("payload")).and_then(Value::as_array).map_or(0, Vec::len),
        bytes: req.body.len(),
        status: 0,
        user_agent: req.header("User-Agent").unwrap_or("").to_owned(),
        at: Instant::now(),
    };
    if s.trouble == Trouble::Silent {
        s.requests.push(seen);
        drop(s);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            drop(conn);
        });
        return;
    }
    let (status, out, lose) = respond(&mut s, &req, user, body);
    seen.status = status;
    s.requests.push(seen);
    match status {
        0 => {}
        _ if lose => {}
        _ => answer(&conn, status, &out, &mut s),
    }
}

/// (status, body, the answer is lost): status 0 closes the connection unanswered.
fn respond(s: &mut Server, req: &Request, user: Option<String>, body: Option<Value>) -> (u16, Value, bool) {
    match s.trouble.clone() {
        Trouble::Offline => return (0, Value::Null, false),
        Trouble::Down => return (503, json!({"code": 503, "error": "down for maintenance"}), false),
        Trouble::Echo(status) => return (status, json!({"code": status, "error": format!("cannot handle {}", req.head)}), false),
        Trouble::RateLimited { times, reset_in } if times > 0 => {
            s.trouble = if times > 1 { Trouble::RateLimited { times: times - 1, reset_in } } else { Trouble::None };
            // The headers say none left, `reset_in` to go.
            s.headers_once = Some((0, reset_in));
            return (429, json!({"code": 429, "error": "Too many requests"}), false);
        }
        Trouble::BadToken => return (401, json!({"code": 401, "error": "Invalid authorization token."}), false),
        Trouble::NoEmail if req.method == "POST" => {
            return (401, json!({"code": 401, "error": "The listens were rejected because the user does not have an email. Please visit https://musicbrainz.org/account/edit to add an email address."}), false);
        }
        _ => {}
    }
    // The rate-limit window.
    let now = Instant::now();
    if s.window_start.is_none_or(|t| now.duration_since(t).as_secs() >= s.window.1) {
        s.window_start = Some(now);
        s.used = 0;
    }
    if s.used >= s.window.0 {
        return (429, json!({"code": 429, "error": "Too many requests"}), false);
    }
    s.used += 1;
    let auth = req.header("Authorization").unwrap_or("");
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/1/validate-token") => {
            if !auth.starts_with("Token ") {
                return (400, json!({"code": 400, "error": "You need to provide an Authorization token."}), false);
            }
            match user {
                Some(u) => (200, json!({"code": 200, "message": "Token valid.", "valid": true, "user_name": u}), false),
                None => (200, json!({"code": 200, "message": "Token invalid.", "valid": false}), false),
            }
        }
        ("POST", "/1/submit-listens") => {
            let Some(user) = user else { return (401, json!({"code": 401, "error": "Invalid authorization token."}), false) };
            if let Err(why) = check_submission(req, body.as_ref()) {
                s.violations.push(why.clone());
                return (400, json!({"code": 400, "error": why}), false);
            }
            let body = body.unwrap_or_default();
            let kind = body["listen_type"].as_str().unwrap_or("").to_owned();
            let listens = body["payload"].as_array().cloned().unwrap_or_default();
            if s.trouble == Trouble::RefuseListens {
                return (400, json!({"code": 400, "error": "listen refused"}), false);
            }
            if let Some(bad) = &s.reject_track
                && listens.iter().any(|l| l["track_metadata"]["track_name"].as_str() == Some(bad))
            {
                return (400, json!({"code": 400, "error": format!("listen refused: {bad}")}), false);
            }
            if kind == "playing_now" {
                s.playing_now.extend(listens);
            } else {
                let mut added = 0;
                for l in listens {
                    let key = (user.clone(), l["listened_at"].as_i64().unwrap_or(0), l["track_metadata"]["track_name"].as_str().unwrap_or("").to_owned());
                    if !s.keys.contains(&key) {
                        s.keys.push(key);
                        s.listens.push(l);
                        added += 1;
                    }
                }
                if added == 0 {
                    s.duplicates += 1;
                }
            }
            let lose = match s.trouble {
                Trouble::LoseAnswer { times } if times > 0 => {
                    s.trouble = if times > 1 { Trouble::LoseAnswer { times: times - 1 } } else { Trouble::None };
                    true
                }
                _ => false,
            };
            (200, json!({"status": "ok"}), lose)
        }
        ("POST", "/1/feedback/recording-feedback") => {
            if user.is_none() {
                return (401, json!({"code": 401, "error": "Invalid authorization token."}), false);
            }
            let b = body.unwrap_or_default();
            let ok = b.get("recording_mbid").and_then(Value::as_str).and_then(mbid).is_some() && matches!(b.get("score").and_then(Value::as_i64), Some(-1..=1));
            if !ok {
                s.violations.push(format!("bad feedback {b}"));
                return (400, json!({"code": 400, "error": "bad feedback"}), false);
            }
            s.feedback.push(b);
            (200, json!({"status": "ok"}), false)
        }
        _ => (404, json!({"code": 404, "error": "Not found"}), false),
    }
}

/// The documented rules of a submission; Err with the first one broken.
fn check_submission(req: &Request, body: Option<&Value>) -> Result<(), String> {
    if !req.header("Content-Type").is_some_and(|c| c.starts_with("application/json")) {
        return Err("not sent as application/json".into());
    }
    if req.body.len() > MAX_LISTEN_PAYLOAD_SIZE {
        return Err(format!("payload of {} bytes", req.body.len()));
    }
    let body = body.ok_or("not JSON")?;
    let kind = body.get("listen_type").and_then(Value::as_str).ok_or("no listen_type")?;
    let list = body.get("payload").and_then(Value::as_array).ok_or("no payload")?;
    match kind {
        "single" | "playing_now" if list.len() != 1 => return Err(format!("{kind} with {} listens", list.len())),
        "import" if list.is_empty() || list.len() > MAX_LISTENS_PER_REQUEST => return Err(format!("import of {} listens", list.len())),
        "single" | "playing_now" | "import" => {}
        k => return Err(format!("listen_type {k}")),
    }
    for l in list {
        if l.to_string().len() > MAX_LISTEN_SIZE {
            return Err("a listen over MAX_LISTEN_SIZE".into());
        }
        match (kind, l.get("listened_at")) {
            ("playing_now", Some(_)) => return Err("playing_now with listened_at".into()),
            ("playing_now", None) => {}
            (_, Some(t)) if t.as_i64().is_some_and(|t| t >= LISTEN_MINIMUM_TS) => {}
            (_, t) => return Err(format!("listened_at {t:?}")),
        }
        let meta = l.get("track_metadata").and_then(Value::as_object).ok_or("no track_metadata")?;
        for k in ["artist_name", "track_name"] {
            if meta.get(k).and_then(Value::as_str).is_none_or(|v| v.trim().is_empty()) {
                return Err(format!("no {k}"));
            }
        }
        if meta.get("release_name").is_some_and(|v| !v.is_string()) {
            return Err("release_name not a string".into());
        }
        if let Some(info) = meta.get("additional_info") {
            let info = info.as_object().ok_or("additional_info not an object")?;
            for k in ["recording_mbid", "track_mbid", "release_mbid", "release_group_mbid"] {
                if let Some(v) = info.get(k)
                    && v.as_str().and_then(mbid).is_none()
                {
                    return Err(format!("{k} {v}"));
                }
            }
            if let Some(v) = info.get("artist_mbids")
                && !v.as_array().is_some_and(|a| a.iter().all(|x| x.as_str().and_then(mbid).is_some()))
            {
                return Err(format!("artist_mbids {v}"));
            }
            if info.get("duration_ms").is_some_and(|v| !v.as_u64().is_some_and(|d| d <= MAX_DURATION_MS_LIMIT)) {
                return Err("duration_ms".into());
            }
            if info.contains_key("duration_ms") && info.contains_key("duration") {
                return Err("both duration and duration_ms".into());
            }
            for k in ["media_player", "media_player_version", "submission_client", "submission_client_version", "tracknumber"] {
                if info.get(k).is_some_and(|v| !v.is_string()) {
                    return Err(format!("{k} not a string"));
                }
            }
        }
    }
    Ok(())
}
