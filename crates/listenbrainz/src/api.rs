//! The ListenBrainz API as documented (listenbrainz.readthedocs.io, "API" and "JSON
//! documentation"): `GET /1/validate-token`, `POST /1/submit-listens` (`playing_now`, `single`,
//! `import`) and `POST /1/feedback/recording-feedback`, each with `Authorization: Token <token>`.
//! JSON in, JSON out; errors sorted into what the submitter does about them. Nothing here logs
//! the token, and a server's message is never repeated with the token in it (`scrub`).

use std::cell::Cell;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::limits::*;

/// ListenBrainz's own API root; Settings' "Server address" replaces it for a server of one's
/// own that speaks the same API.
pub const API_ROOT: &str = "https://api.listenbrainz.org";
/// Where a user finds their token (the settings page of their ListenBrainz account).
pub const TOKEN_PAGE: &str = "https://listenbrainz.org/settings/";
const TIMEOUT_MS: u32 = 20_000;

/// The documented limits (the API page's constants).
pub mod limits {
    /// "The maximum number of listens in a request."
    pub const MAX_LISTENS_PER_REQUEST: usize = 1000;
    /// "Maximum overall listen size in bytes."
    pub const MAX_LISTEN_SIZE: usize = 10240;
    /// "The maximum size of a payload in bytes."
    pub const MAX_LISTEN_PAYLOAD_SIZE: usize = 10_240_000;
    /// "The minimum acceptable value for listened_at field."
    pub const LISTEN_MINIMUM_TS: i64 = 1_033_430_400;
    /// The longest `duration_ms` taken (24 days).
    pub const MAX_DURATION_MS_LIMIT: u64 = 2_073_600_000;
}

/// The MusicBrainz ids of a listen, as tagged (`additional_info`'s `recording_mbid`,
/// `track_mbid`, `release_mbid`, `release_group_mbid`, `artist_mbids`). Only well-formed ids are
/// kept (`Ids::tidy`): ListenBrainz refuses a listen with one that is not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ids {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording: Option<String>,
    /// The release track (Picard's MUSICBRAINZ_RELEASETRACKID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_group: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<String>,
}

/// A MusicBrainz id as ListenBrainz takes it: a UUID, lower case; None for anything else.
pub fn mbid(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let b = s.as_bytes();
    let ok = b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() });
    ok.then_some(s)
}

impl Ids {
    pub fn is_empty(&self) -> bool {
        *self == Ids::default()
    }

    /// Only well-formed ids, lower case; artists without repeats. A value holding several ids
    /// ("id1; id2", "id1/id2") gives each.
    pub fn tidy(self) -> Ids {
        let one = |v: Option<String>| v.as_deref().and_then(mbid);
        let mut artists: Vec<String> = Vec::new();
        for a in self.artists.iter().flat_map(|v| v.split([';', '/', ',', '\0', ' '])).filter_map(mbid) {
            if !artists.contains(&a) {
                artists.push(a);
            }
        }
        Ids { recording: one(self.recording), track: one(self.track), release: one(self.release), release_group: one(self.release_group), artists }
    }
}

/// One listen, as sent (and kept in the offline queue).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listen {
    /// `artist_name`: the credit as tagged.
    pub artist: String,
    /// `track_name`.
    pub track: String,
    /// `release_name` (the album); empty: not sent.
    #[serde(default)]
    pub release: String,
    /// `duration_ms`; 0: unknown, not sent.
    #[serde(default)]
    pub duration_ms: u64,
    /// `tracknumber`.
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub ids: Ids,
    /// `listened_at`: when the track started, Unix seconds (UTC). Not sent with `playing_now`.
    pub listened_at: i64,
    /// The file to read the MusicBrainz ids from, before the listen is first sent (then cleared:
    /// a listen sent again is the same, byte for byte). Never sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `listen_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenType {
    /// The track has just started: one listen, no `listened_at`; kept only for a while there.
    PlayingNow,
    /// One listen just made.
    Single,
    /// Listens kept until they could be sent (the offline queue): at least one.
    Import,
}

impl ListenType {
    pub fn as_str(self) -> &'static str {
        match self {
            ListenType::PlayingNow => "playing_now",
            ListenType::Single => "single",
            ListenType::Import => "import",
        }
    }
}

/// Who is sending: `media_player` and `submission_client` (the app's name, no version, as the
/// documentation asks) and their `..._version`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    pub version: String,
}

/// One listen's JSON: `listened_at` (unless `playing_now`) and `track_metadata` with the
/// documented `additional_info` fields the listen has; a field it does not have is left out,
/// as the documentation asks.
pub fn listen_json(l: &Listen, player: &Player, kind: ListenType) -> Value {
    let mut info = Map::new();
    let mut put = |k: &str, v: Value| {
        info.insert(k.to_owned(), v);
    };
    if !player.name.is_empty() {
        put("media_player", json!(player.name));
        put("submission_client", json!(player.name));
    }
    if !player.version.is_empty() {
        put("media_player_version", json!(player.version));
        put("submission_client_version", json!(player.version));
    }
    if l.duration_ms > 0 && l.duration_ms <= MAX_DURATION_MS_LIMIT {
        put("duration_ms", json!(l.duration_ms));
    }
    if let Some(n) = l.track_number.filter(|n| *n > 0) {
        // A string, as the documentation types it.
        put("tracknumber", json!(n.to_string()));
    }
    let ids = l.ids.clone().tidy();
    for (k, v) in [("recording_mbid", ids.recording), ("track_mbid", ids.track), ("release_mbid", ids.release), ("release_group_mbid", ids.release_group)] {
        if let Some(v) = v {
            put(k, json!(v));
        }
    }
    if !ids.artists.is_empty() {
        put("artist_mbids", json!(ids.artists));
    }
    let mut meta = Map::new();
    meta.insert("artist_name".into(), json!(l.artist));
    meta.insert("track_name".into(), json!(l.track));
    if !l.release.trim().is_empty() {
        meta.insert("release_name".into(), json!(l.release));
    }
    meta.insert("additional_info".into(), Value::Object(info));
    let mut o = Map::new();
    if kind != ListenType::PlayingNow {
        o.insert("listened_at".into(), json!(l.listened_at));
    }
    o.insert("track_metadata".into(), Value::Object(meta));
    Value::Object(o)
}

/// The request body: `{"listen_type": ..., "payload": [...]}`.
pub fn payload(kind: ListenType, listens: &[Value]) -> String {
    json!({ "listen_type": kind.as_str(), "payload": listens }).to_string()
}

/// The bytes a payload adds around its listens (`{"listen_type":"import","payload":[` and `]}`).
const ENVELOPE: usize = 48;

/// How many of `sizes` (each listen's JSON, oldest first) go in one request: at most
/// `max_count`, the whole body within `max_bytes`. 0 only when there is none.
pub fn batch_len(sizes: &[usize], max_count: usize, max_bytes: usize) -> usize {
    let mut total = ENVELOPE;
    let mut n = 0;
    for &s in sizes.iter().take(max_count.max(1)) {
        let add = s + usize::from(n > 0);
        if n > 0 && total + add > max_bytes {
            break;
        }
        total += add;
        n += 1;
    }
    n
}

/// Why a listen can never be sent (it is dropped, and logged); None when it can.
pub fn unsendable(l: &Listen, json_size: usize) -> Option<String> {
    if l.artist.trim().is_empty() || l.track.trim().is_empty() {
        Some("no artist or no title".into())
    } else if l.listened_at < LISTEN_MINIMUM_TS {
        Some(format!("its time ({}) is before what ListenBrainz takes", l.listened_at))
    } else if json_size > MAX_LISTEN_SIZE {
        Some(format!("{json_size} bytes, more than the {MAX_LISTEN_SIZE} ListenBrainz takes"))
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// No answer (offline, DNS, timeout): try again later.
    Network(String),
    /// The server is down or busy (HTTP 5xx): try again later.
    Busy(u16),
    /// 429: too many requests; send again after this long.
    RateLimited(Duration),
    /// 401, or validate-token's `valid: false`: the token is not (or no longer) accepted. The
    /// user must connect again.
    InvalidToken,
    /// 401 to a call other than validate-token, with the server's words. Not always the token:
    /// ListenBrainz also answers 401 to listens from an account without a verified e-mail
    /// address. The submitter asks validate-token which it is.
    Unauthorized(String),
    /// 400 (or 413): these listens will never be accepted as they are.
    Rejected(u16, String),
    /// Another answer (a 404 from an address that is not a ListenBrainz API, a 403, a page that
    /// is not JSON): try again later, slowly; nothing is dropped for it.
    Unexpected(u16, String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Network(m) => write!(f, "no connection ({m})"),
            ApiError::Busy(s) => write!(f, "the server is busy or down (HTTP {s})"),
            ApiError::RateLimited(d) => write!(f, "asked to wait {} s (HTTP 429)", d.as_secs()),
            ApiError::InvalidToken => f.write_str("the token is not accepted"),
            ApiError::Unauthorized(m) if m.is_empty() => f.write_str("not authorised (HTTP 401)"),
            ApiError::Unauthorized(m) => write!(f, "not authorised (HTTP 401: {m})"),
            ApiError::Rejected(s, m) => write!(f, "HTTP {s}: {m}"),
            ApiError::Unexpected(s, m) if m.is_empty() => write!(f, "an unexpected answer (HTTP {s})"),
            ApiError::Unexpected(s, m) => write!(f, "an unexpected answer (HTTP {s}: {m})"),
        }
    }
}

impl ApiError {
    /// Worth trying again later, unchanged (with the backoff).
    pub fn is_transient(&self) -> bool {
        matches!(self, ApiError::Network(_) | ApiError::Busy(_) | ApiError::Unexpected(..))
    }
}

/// `text` with `secret` taken out (a server that echoes the request back must not put the
/// token in the log), cut to a line.
pub fn scrub(text: &str, secret: &str) -> String {
    let t = if secret.len() >= 4 { text.replace(secret, "…") } else { text.to_owned() };
    let t: String = t.chars().filter(|c| !c.is_control()).take(500).collect();
    t.trim().to_owned()
}

/// How long a 429 asks to wait: `X-RateLimit-Reset-In`, else `Retry-After`, else 10 s; at most
/// an hour, at least a second.
fn wait_of(rate: http_client::RateLimit, retry_after: Option<u64>) -> Duration {
    Duration::from_secs(rate.reset_in.or(retry_after).unwrap_or(10).clamp(1, 3600))
}

/// The API at one root, with one user's token. Used on the submitter's thread only.
#[derive(Debug)]
pub struct Client {
    pub root: String,
    pub token: String,
    /// The `User-Agent` ("MarqueePlayer/<version>").
    pub agent: String,
    pub player: Player,
    /// The rate-limit headers of the last answer.
    rate: Cell<http_client::RateLimit>,
}

impl Client {
    pub fn new(root: &str, token: &str, agent: &str, player: Player) -> Self {
        Self { root: root.trim_end_matches('/').to_owned(), token: token.trim().to_owned(), agent: agent.into(), player, rate: Cell::default() }
    }

    /// The rate-limit headers of the last answer (none before one).
    pub fn rate(&self) -> http_client::RateLimit {
        self.rate.get()
    }

    fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<Value, ApiError> {
        let url = format!("{}{path}", self.root);
        let auth = format!("Authorization: Token {}", self.token);
        let mut headers = vec![auth.as_str()];
        if body.is_some() {
            headers.push("Content-Type: application/json");
        }
        let r = http_client::request(&http_client::Request {
            method,
            url: &url,
            headers: &headers,
            body: body.unwrap_or("").as_bytes(),
            user_agent: &self.agent,
            timeout_ms: TIMEOUT_MS,
        })
        .map_err(|e| ApiError::Network(e.to_string()))?;
        self.rate.set(r.rate_limit);
        let json: Option<Value> = serde_json::from_str(&r.body).ok();
        let message = || {
            let m = json.as_ref().and_then(|j| j.get("error").or_else(|| j.get("message"))).and_then(Value::as_str).unwrap_or("");
            scrub(m, &self.token)
        };
        match r.status {
            200..=299 => json.ok_or_else(|| ApiError::Unexpected(r.status, "the answer is not JSON".into())),
            401 => Err(ApiError::Unauthorized(message())),
            429 => Err(ApiError::RateLimited(wait_of(r.rate_limit, r.retry_after))),
            400 | 413 if json.is_some() || r.status == 413 => Err(ApiError::Rejected(r.status, message())),
            s if s >= 500 || s == 0 => Err(ApiError::Busy(s)),
            s => Err(ApiError::Unexpected(s, message())),
        }
    }

    /// `GET /1/validate-token`: the user name the token belongs to.
    pub fn validate_token(&self) -> Result<String, ApiError> {
        if self.token.is_empty() {
            return Err(ApiError::InvalidToken);
        }
        let j = match self.call("GET", "/1/validate-token", None) {
            // 400: "No token was sent".
            Err(ApiError::Rejected(400, _) | ApiError::Unauthorized(_)) => return Err(ApiError::InvalidToken),
            r => r?,
        };
        match (j.get("valid").and_then(Value::as_bool), j.get("user_name").and_then(Value::as_str)) {
            (Some(true), Some(user)) if !user.is_empty() => Ok(user.to_owned()),
            (Some(false), _) => Err(ApiError::InvalidToken),
            _ => Err(ApiError::Unexpected(200, "not a validate-token answer".into())),
        }
    }

    /// `POST /1/submit-listens` with these listens' JSON (`listen_json`).
    pub fn submit(&self, kind: ListenType, listens: &[Value]) -> Result<(), ApiError> {
        self.call("POST", "/1/submit-listens", Some(&payload(kind, listens))).map(|_| ())
    }

    /// "Now playing": one listen, without its time.
    pub fn playing_now(&self, l: &Listen) -> Result<(), ApiError> {
        self.submit(ListenType::PlayingNow, &[listen_json(l, &self.player, ListenType::PlayingNow)])
    }

    /// `POST /1/feedback/recording-feedback`: `score` 1 loved, 0 no feedback (-1 hated is not
    /// used), for a recording by its MusicBrainz id.
    pub fn recording_feedback(&self, recording_mbid: &str, score: i8) -> Result<(), ApiError> {
        let body = json!({ "recording_mbid": recording_mbid, "score": score }).to_string();
        self.call("POST", "/1/feedback/recording-feedback", Some(&body)).map(|_| ())
    }
}

/// A server address as typed, as the API root: trimmed, `https://` when no scheme is given, no
/// trailing `/` (nor `/1`, the API's own path, when pasted with it). Empty: ListenBrainz's own
/// (returned as ""). Err with what is wrong.
pub fn normalize_server(typed: &str) -> Result<String, String> {
    let t = typed.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let t = if t.contains("://") { t.to_owned() } else { format!("https://{t}") };
    let lower = t.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("The server address must start with https:// or http://.".into());
    }
    let mut t = t.trim_end_matches('/').to_owned();
    if t.ends_with("/1") {
        t.truncate(t.len() - 2);
        t = t.trim_end_matches('/').to_owned();
    }
    let host = t.split_once("://").map(|(_, r)| r).unwrap_or("");
    if host.is_empty() || host.starts_with('/') || host.contains(char::is_whitespace) {
        return Err("That is not a server address.".into());
    }
    if t.eq_ignore_ascii_case(API_ROOT) {
        return Ok(String::new());
    }
    Ok(t)
}

/// The address is plain http: the token goes unencrypted.
pub fn is_plain_http(typed: &str) -> bool {
    typed.trim().to_ascii_lowercase().starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REC: &str = "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae";

    fn listen() -> Listen {
        Listen {
            artist: "Aurora Lines".into(),
            track: "Lantern".into(),
            release: "Night Harbour".into(),
            duration_ms: 245_400,
            track_number: Some(3),
            ids: Ids { recording: Some(REC.to_uppercase()), release: Some("not-an-id".into()), artists: vec![format!("{REC}; {REC}")], ..Ids::default() },
            listened_at: 1_760_000_000,
            path: Some("M:/a.flac".into()),
        }
    }

    fn player() -> Player {
        Player { name: "Marquee Player".into(), version: "2026.11.18".into() }
    }

    /// The documented shape (the JSON documentation's sample, with this app's fields).
    #[test]
    fn a_listen_is_the_documented_json() {
        let v = listen_json(&listen(), &player(), ListenType::Single);
        assert_eq!(
            v,
            json!({
                "listened_at": 1_760_000_000,
                "track_metadata": {
                    "artist_name": "Aurora Lines",
                    "track_name": "Lantern",
                    "release_name": "Night Harbour",
                    "additional_info": {
                        "media_player": "Marquee Player",
                        "media_player_version": "2026.11.18",
                        "submission_client": "Marquee Player",
                        "submission_client_version": "2026.11.18",
                        "duration_ms": 245_400,
                        "tracknumber": "3",
                        "recording_mbid": REC,
                        "artist_mbids": [REC],
                    }
                }
            })
        );
        assert!(!v.to_string().contains("M:/a.flac"), "the file is never sent");
        // Now playing: no time.
        let np = listen_json(&listen(), &player(), ListenType::PlayingNow);
        assert!(np.get("listened_at").is_none() && np["track_metadata"]["track_name"] == "Lantern");
        // What it does not have is left out.
        let bare = Listen { artist: "A".into(), track: "T".into(), listened_at: 1_760_000_000, ..Listen::default() };
        let v = listen_json(&bare, &Player::default(), ListenType::Single);
        assert_eq!(v, json!({ "listened_at": 1_760_000_000, "track_metadata": { "artist_name": "A", "track_name": "T", "additional_info": {} } }));
        let body = payload(ListenType::Import, &[v.clone(), v]);
        assert!(body.starts_with(r#"{"listen_type":"import","payload":[{"#), "{body}");
    }

    #[test]
    fn only_well_formed_ids_are_kept() {
        assert_eq!(mbid(&format!(" {} ", REC.to_uppercase())).as_deref(), Some(REC));
        assert_eq!(mbid("8f3471b5-7e6a-48da-86a9-c1c07a0f47a"), None);
        assert_eq!(mbid("8f3471b5x7e6a-48da-86a9-c1c07a0f47ae"), None);
        let ids = Ids { track: Some("".into()), artists: vec![format!("{REC}/0383dadf-2a4e-4d10-a46a-e9e041da8eb3"), "x".into()], ..Ids::default() }.tidy();
        assert_eq!(ids, Ids { artists: vec![REC.into(), "0383dadf-2a4e-4d10-a46a-e9e041da8eb3".into()], ..Ids::default() });
    }

    #[test]
    fn batches_keep_to_the_documented_limits() {
        // 2500 small listens: 1000, 1000, 500.
        let small = vec![300usize; 2500];
        assert_eq!(batch_len(&small, MAX_LISTENS_PER_REQUEST, MAX_LISTEN_PAYLOAD_SIZE), 1000);
        assert_eq!(batch_len(&small[2000..], MAX_LISTENS_PER_REQUEST, MAX_LISTEN_PAYLOAD_SIZE), 500);
        // Listens near the per-listen maximum: the payload's size decides first.
        let big = vec![MAX_LISTEN_SIZE; 1500];
        let n = batch_len(&big, MAX_LISTENS_PER_REQUEST, MAX_LISTEN_PAYLOAD_SIZE);
        assert!(n < 1000 && ENVELOPE + n * (MAX_LISTEN_SIZE + 1) <= MAX_LISTEN_PAYLOAD_SIZE && ENVELOPE + (n + 1) * (MAX_LISTEN_SIZE + 1) > MAX_LISTEN_PAYLOAD_SIZE, "{n}");
        // The body as built is what was counted.
        let v = listen_json(&listen(), &player(), ListenType::Import);
        let one = v.to_string().len();
        let three = payload(ListenType::Import, &[v.clone(), v.clone(), v]).len();
        // (Counted a little high, never low: what is counted for three is at least the body.)
        let counted = ENVELOPE + 3 * one + 2;
        assert!(three <= counted, "{three} > {counted}");
        assert_eq!(batch_len(&[one; 3], 1000, counted), 3);
        assert_eq!(batch_len(&[one; 3], 1000, counted - 1), 2);
        assert_eq!(batch_len(&[], 1000, 100), 0);
        // One listen always goes alone, whatever its size (`unsendable` decides).
        assert_eq!(batch_len(&[50_000, 10], 1000, 100), 1);
    }

    #[test]
    fn what_can_never_be_sent() {
        let l = listen();
        assert_eq!(unsendable(&l, 500), None);
        assert!(unsendable(&Listen { listened_at: LISTEN_MINIMUM_TS - 1, ..l.clone() }, 500).is_some());
        assert!(unsendable(&Listen { track: " ".into(), ..l.clone() }, 500).is_some());
        assert!(unsendable(&l, MAX_LISTEN_SIZE + 1).is_some());
    }

    #[test]
    fn server_addresses() {
        assert_eq!(normalize_server("").unwrap(), "");
        assert_eq!(normalize_server(" https://api.listenbrainz.org/ ").unwrap(), "", "ListenBrainz's own");
        assert_eq!(normalize_server("lb.example.org").unwrap(), "https://lb.example.org");
        assert_eq!(normalize_server("http://192.168.1.10:8100/apis/listenbrainz/1/").unwrap(), "http://192.168.1.10:8100/apis/listenbrainz");
        assert!(normalize_server("ftp://x").is_err());
        assert!(normalize_server("https://").is_err());
        assert!(is_plain_http(" HTTP://x") && !is_plain_http("https://x"));
    }

    #[test]
    fn a_server_message_never_carries_the_token() {
        let token = "0b4c33a5-1d1c-4bd2-9d2b-6a1f2e1e9f00";
        assert_eq!(scrub(&format!("bad header Authorization: Token {token}\n"), token), "bad header Authorization: Token …");
        assert_eq!(wait_of(http_client::RateLimit { remaining: Some(0), reset_in: Some(4) }, Some(9)), Duration::from_secs(4));
        assert_eq!(wait_of(http_client::RateLimit::default(), Some(9)), Duration::from_secs(9));
        assert_eq!(wait_of(http_client::RateLimit::default(), None), Duration::from_secs(10));
    }
}
