//! Last.fm API 2.0 calls: signed form POSTs, JSON answers, errors sorted into what the
//! scrobbler does about them. Nothing here logs a key, secret, token or session key.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use http_client as http;

pub const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
pub const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";
/// Scrobbles per `track.scrobble` call (Last.fm's limit).
pub const BATCH: usize = 50;
const TIMEOUT_MS: u32 = 20_000;

/// One play, as sent to Last.fm (and kept in the offline queue).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scrobble {
    pub artist: String,
    pub track: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_artist: String,
    /// Seconds.
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub track_number: Option<u32>,
    /// MusicBrainz recording id, when tagged.
    #[serde(default)]
    pub mbid: Option<String>,
    /// When the track started, Unix seconds (UTC).
    pub timestamp: i64,
    /// The file, to read its MusicBrainz id from when `mbid` is not given. Not sent.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// No answer (offline, DNS, timeout): try again later.
    Network(String),
    /// Last.fm is down or busy (8 operation failed, 11 offline, 16 unavailable, 29 rate
    /// limit), or an answer that is not Last.fm's (a 5xx, a proxy's or CDN's page): try again later.
    Transient(i64, String),
    /// 9: the session key is no longer valid. The user must connect again.
    InvalidSession(String),
    /// 4, 10, 13, 26: the API key or secret is wrong or suspended. The user must fix it.
    Credentials(i64, String),
    /// 14: the token is not authorised yet (the user has not allowed it in the browser).
    NotAuthorised,
    /// 15: the token expired before it was authorised.
    TokenExpired,
    /// Anything else (6 invalid parameters, ...): this request will never succeed.
    Rejected(i64, String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Network(m) => write!(f, "no connection ({m})"),
            ApiError::Transient(c, m) | ApiError::Credentials(c, m) | ApiError::Rejected(c, m) => write!(f, "Last.fm error {c}: {m}"),
            ApiError::InvalidSession(m) => write!(f, "session no longer valid: {m}"),
            ApiError::NotAuthorised => f.write_str("not authorised yet"),
            ApiError::TokenExpired => f.write_str("the authorisation request expired"),
        }
    }
}

impl ApiError {
    pub fn from_code(code: i64, message: String) -> Self {
        match code {
            9 => ApiError::InvalidSession(message),
            4 | 10 | 13 | 26 => ApiError::Credentials(code, message),
            14 => ApiError::NotAuthorised,
            15 => ApiError::TokenExpired,
            8 | 11 | 16 | 29 => ApiError::Transient(code, message),
            _ => ApiError::Rejected(code, message),
        }
    }

    /// Worth trying again later, unchanged.
    pub fn is_transient(&self) -> bool {
        matches!(self, ApiError::Network(_) | ApiError::Transient(..))
    }
}

/// `api_sig`: every parameter but `format` and `callback`, sorted by name, each name
/// followed by its value, then the secret; the MD5 of that, as hex.
pub fn sign(params: &[(String, String)], secret: &str) -> String {
    let mut p: Vec<&(String, String)> = params.iter().filter(|(k, _)| k != "format" && k != "callback").collect();
    p.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::new();
    for (k, v) in p {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    md5::hex(s.as_bytes())
}

/// What became of one scrobble in a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// Last.fm took the request but ignored this play (too old, bad artist...): never
    /// sent again.
    Ignored(String),
    /// Ignored for now: the account's daily scrobble limit is reached (ignored code 5). Sent
    /// again later.
    Limited,
}

#[derive(Clone, Debug)]
pub struct Client {
    pub root: String,
    pub key: String,
    pub secret: String,
    /// The client name sent with each call ("MarqueePlayer/<version>").
    pub agent: String,
}

impl Client {
    pub fn new(root: &str, key: &str, secret: &str, agent: &str) -> Self {
        Self { root: root.into(), key: key.into(), secret: secret.into(), agent: agent.into() }
    }

    fn call(&self, method: &str, mut params: Vec<(String, String)>, session: Option<&str>) -> Result<Value, ApiError> {
        params.push(("method".into(), method.into()));
        params.push(("api_key".into(), self.key.clone()));
        if let Some(sk) = session {
            params.push(("sk".into(), sk.into()));
        }
        let sig = sign(&params, &self.secret);
        params.push(("api_sig".into(), sig));
        params.push(("format".into(), "json".into()));
        let (status, body) = http::post(&self.root, &http::form_encode(&params), &self.agent, TIMEOUT_MS).map_err(|e| ApiError::Network(e.to_string()))?;
        let json: Option<Value> = serde_json::from_str(&body).ok();
        if let Some(j) = &json
            && let Some(code) = j.get("error").and_then(Value::as_i64)
        {
            let message = j.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
            log::info!("last.fm {method}: error {code} ({message})");
            return Err(ApiError::from_code(code, message));
        }
        // Without Last.fm's own error code, only a 400 in JSON is Last.fm refusing the request.
        // Anything else (a proxy's 407, a CDN's 403 or 429, an HTML page with a 200, a 5xx) is
        // not an answer from Last.fm: tried again later, the plays kept.
        match json {
            Some(j) if (200..300).contains(&status) => Ok(j),
            Some(_) if status == 400 => Err(ApiError::Rejected(i64::from(status), format!("HTTP {status}"))),
            _ => Err(ApiError::Transient(i64::from(status), format!("HTTP {status}"))),
        }
    }

    /// auth.getToken: a token for the user to authorise in the browser.
    pub fn get_token(&self) -> Result<String, ApiError> {
        let j = self.call("auth.getToken", Vec::new(), None)?;
        j.get("token").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| ApiError::Transient(0, "no token in the answer".into()))
    }

    /// The page where the user allows this app the token.
    pub fn auth_url(&self, token: &str) -> String {
        format!("{AUTH_PAGE}?api_key={}&token={}", self.key, token)
    }

    /// auth.getSession: (user name, session key), once the token is authorised.
    pub fn get_session(&self, token: &str) -> Result<(String, String), ApiError> {
        let j = self.call("auth.getSession", vec![("token".into(), token.into())], None)?;
        let s = j.get("session").ok_or_else(|| ApiError::Transient(0, "no session in the answer".into()))?;
        let name = s.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let key = s.get("key").and_then(Value::as_str).ok_or_else(|| ApiError::Transient(0, "no session key".into()))?;
        Ok((name, key.to_owned()))
    }

    fn track_params(s: &Scrobble, index: Option<usize>) -> Vec<(String, String)> {
        let k = |name: &str| match index {
            Some(i) => format!("{name}[{i}]"),
            None => name.to_owned(),
        };
        let mut p = vec![(k("artist"), s.artist.clone()), (k("track"), s.track.clone())];
        if !s.album.is_empty() {
            p.push((k("album"), s.album.clone()));
        }
        if !s.album_artist.is_empty() && s.album_artist != s.artist {
            p.push((k("albumArtist"), s.album_artist.clone()));
        }
        if s.duration > 0 {
            p.push((k("duration"), s.duration.to_string()));
        }
        if let Some(n) = s.track_number {
            p.push((k("trackNumber"), n.to_string()));
        }
        if let Some(m) = s.mbid.as_ref().filter(|m| !m.is_empty()) {
            p.push((k("mbid"), m.clone()));
        }
        if index.is_some() {
            p.push((k("timestamp"), s.timestamp.to_string()));
        }
        p
    }

    /// track.updateNowPlaying.
    pub fn now_playing(&self, session: &str, s: &Scrobble) -> Result<(), ApiError> {
        self.call("track.updateNowPlaying", Self::track_params(s, None), Some(session)).map(|_| ())
    }

    /// track.scrobble, up to `BATCH` at once: what became of each, in order.
    pub fn scrobble(&self, session: &str, batch: &[Scrobble]) -> Result<Vec<Outcome>, ApiError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let params: Vec<(String, String)> = batch.iter().enumerate().flat_map(|(i, s)| Self::track_params(s, Some(i))).collect();
        let j = self.call("track.scrobble", params, Some(session))?;
        parse_scrobble_outcomes(&j, batch.len())
    }
}

/// Parse `scrobbles.scrobble` from a `track.scrobble` reply. Last.fm returns an object rather
/// than an array for a single play. A missing list or a count that does not match what was
/// sent is treated as a transient error so nothing is dropped from the queue.
fn parse_scrobble_outcomes(j: &Value, expected: usize) -> Result<Vec<Outcome>, ApiError> {
    let list: &[Value] = match j.get("scrobbles").and_then(|s| s.get("scrobble")) {
        Some(Value::Array(a)) => a.as_slice(),
        Some(v @ Value::Object(_)) => std::slice::from_ref(v),
        _ => return Err(ApiError::Transient(0, "no scrobbles in the answer".into())),
    };
    if list.len() != expected {
        return Err(ApiError::Transient(0, format!("expected {expected} scrobble outcomes, got {}", list.len())));
    }
    list.iter()
        .map(|entry| {
            if !entry.is_object() {
                return Err(ApiError::Transient(0, "unexpected scrobble outcome in the answer".into()));
            }
            let ignored = entry.get("ignoredMessage");
            let code = ignored.and_then(|m| m.get("code")).map(|c| c.as_str().map(str::to_owned).unwrap_or_else(|| c.to_string()));
            Ok(match code.as_deref() {
                None | Some("0") | Some("") => Outcome::Accepted,
                Some("5") => Outcome::Limited,
                Some(c) => {
                    let text = ignored.and_then(|m| m.get("#text")).and_then(Value::as_str).unwrap_or("");
                    Outcome::Ignored(format!("code {c}: {text}"))
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect()
    }

    #[test]
    fn signatures_follow_the_documented_recipe() {
        // Last.fm's authentication spec, section 6 ("Sign your calls"): for
        // api_key xxxxxxxxxx, method auth.getSession, token yyyyyy the signature is the MD5
        // of "api_keyxxxxxxxxxxmethodauth.getSessiontokenyyyyyy" followed by the secret.
        // Expected value computed independently (Python hashlib) for secret "ilovecher".
        let params = p(&[("token", "yyyyyy"), ("method", "auth.getSession"), ("api_key", "xxxxxxxxxx"), ("format", "json")]);
        assert_eq!(sign(&params, "ilovecher"), "b87d61da3cda91a8b6746c4aef55d6f8");
        // Array parameters sort by their full name, bytewise: "artist[0]" < "artist[1]" < "method"
        // (expected: MD5 of "api_keykartist[0]Aartist[1]Bmethodtrack.scrobblesksz", by hashlib).
        let params = p(&[("method", "track.scrobble"), ("artist[1]", "B"), ("artist[0]", "A"), ("sk", "s"), ("api_key", "k")]);
        assert_eq!(sign(&params, "z"), "86791a07b26a69e119d6254b0af6395f");
    }

    #[test]
    fn error_codes_sort_into_what_to_do() {
        assert_eq!(ApiError::from_code(9, "x".into()), ApiError::InvalidSession("x".into()));
        assert!(ApiError::from_code(11, "x".into()).is_transient());
        assert!(ApiError::from_code(16, "x".into()).is_transient());
        assert!(ApiError::from_code(29, "x".into()).is_transient());
        assert!(matches!(ApiError::from_code(10, "x".into()), ApiError::Credentials(10, _)));
        assert!(matches!(ApiError::from_code(13, "x".into()), ApiError::Credentials(13, _)));
        assert!(matches!(ApiError::from_code(6, "x".into()), ApiError::Rejected(6, _)));
        assert_eq!(ApiError::from_code(14, "x".into()), ApiError::NotAuthorised);
    }

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn scrobble_reply_parses_full_list_and_single_object_form() {
        let full = json(
            r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":2},"scrobble":[
                {"ignoredMessage":{"code":"0","#text":""}},
                {"ignoredMessage":{"code":"3","#text":"Timestamp too old"}},
                {"ignoredMessage":{"code":5,"#text":"Daily scrobble limit exceeded"}}
            ]}}"##,
        );
        assert_eq!(
            parse_scrobble_outcomes(&full, 3),
            Ok(vec![Outcome::Accepted, Outcome::Ignored("code 3: Timestamp too old".into()), Outcome::Limited])
        );

        // Last.fm encodes a single-scrobble reply as an object instead of a 1-element array.
        let single_obj = json(r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":0},"scrobble":{"ignoredMessage":{"code":"0","#text":""}}}}"##);
        assert_eq!(parse_scrobble_outcomes(&single_obj, 1), Ok(vec![Outcome::Accepted]));

        let single_arr = json(r##"{"scrobbles":{"@attr":{"accepted":1,"ignored":0},"scrobble":[{"ignoredMessage":{"code":"0","#text":""}}]}}"##);
        assert_eq!(parse_scrobble_outcomes(&single_arr, 1), Ok(vec![Outcome::Accepted]));
    }

    #[test]
    fn scrobble_reply_missing_or_mismatched_list_is_transient() {
        for raw in [
            "{}",
            r#"{"scrobbles":{}}"#,
            r#"{"scrobbles":{"@attr":{"accepted":1,"ignored":0}}}"#,
            r#"{"scrobbles":{"scrobble":null}}"#,
            r#"{"scrobbles":{"scrobble":[]}}"#,
            r#"{"scrobbles":{"scrobble":[null]}}"#,
        ] {
            let err = parse_scrobble_outcomes(&json(raw), 1).unwrap_err();
            assert!(err.is_transient(), "{raw} -> {err:?}");
        }

        // Short list (1 entry for a 2-play batch), in both array and single-object forms.
        let short_arr = json(r##"{"scrobbles":{"scrobble":[{"ignoredMessage":{"code":"0","#text":""}}]}}"##);
        assert!(parse_scrobble_outcomes(&short_arr, 2).unwrap_err().is_transient());
        let short_obj = json(r##"{"scrobbles":{"scrobble":{"ignoredMessage":{"code":"0","#text":""}}}}"##);
        assert!(parse_scrobble_outcomes(&short_obj, 2).unwrap_err().is_transient());

        // Overlong list (3 entries for a 2-play batch).
        let long_arr = json(
            r##"{"scrobbles":{"scrobble":[
                {"ignoredMessage":{"code":"0","#text":""}},
                {"ignoredMessage":{"code":"0","#text":""}},
                {"ignoredMessage":{"code":"0","#text":""}}
            ]}}"##,
        );
        assert!(parse_scrobble_outcomes(&long_arr, 2).unwrap_err().is_transient());
    }
}
