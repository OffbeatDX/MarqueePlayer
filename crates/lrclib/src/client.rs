//! The two LRCLIB calls and the rule for taking a search result.

use serde::Deserialize;

use crate::{Answer, TrackInfo, fold};

/// How far a search result's length may be from the track's (seconds). The exact
/// lookup applies the same tolerance on the service's side.
pub const DURATION_SLACK: f64 = 2.0;
/// Per request: resolve, connect, send, receive each.
const TIMEOUT_MS: u32 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No connection, DNS, timeout: offline for now.
    Network(String),
    /// Too many requests (429) or overloaded (503): wait this long (seconds) first.
    Busy(Option<u64>),
    /// Another status, or a body that is not what the API returns.
    Server(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(m) => write!(f, "{m}"),
            Error::Busy(Some(s)) => write!(f, "busy, retry after {s} s"),
            Error::Busy(None) => write!(f, "busy"),
            Error::Server(m) => write!(f, "{m}"),
        }
    }
}

/// A lyrics record, as both calls return it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: i64,
    #[serde(default)]
    pub track_name: String,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub album_name: Option<String>,
    /// Seconds (a number: whole in the documentation's examples, not promised).
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(default)]
    pub plain_lyrics: Option<String>,
    #[serde(default)]
    pub synced_lyrics: Option<String>,
}

impl Record {
    fn has_words(&self) -> bool {
        let some = |s: &Option<String>| s.as_deref().is_some_and(|t| !t.trim().is_empty());
        some(&self.synced_lyrics) || some(&self.plain_lyrics)
    }

    /// The answer this record gives: synced lyrics when it has them and they parse with
    /// time stamps, else the plain ones.
    pub fn answer(&self) -> Answer {
        if self.instrumental {
            return Answer::Instrumental { id: self.id };
        }
        if let Some(s) = self.synced_lyrics.as_deref() {
            let l = library::lyrics::parse(s, "lrclib");
            if l.synced && !l.lines.is_empty() {
                return Answer::Found { id: self.id, lyrics: l };
            }
        }
        if let Some(p) = self.plain_lyrics.as_deref() {
            let mut l = library::lyrics::parse(p, "lrclib");
            if !l.lines.is_empty() {
                // Plain text with something that looks like a stamp is still shown plain.
                if l.synced {
                    l.synced = false;
                    for line in &mut l.lines {
                        line.time_ms = None;
                    }
                }
                return Answer::Found { id: self.id, lyrics: l };
            }
        }
        Answer::NotFound
    }
}

/// Same artist and title after folding, length within the slack.
pub fn matches(t: &TrackInfo, r: &Record) -> bool {
    fold(&t.artist) == fold(&r.artist_name) && fold(&t.title) == fold(&r.track_name) && (t.duration - r.duration).abs() <= DURATION_SLACK
}

/// A search result that may be shown for `t`: it matches, was not rejected, and has
/// words (or is flagged instrumental).
pub fn acceptable(t: &TrackInfo, r: &Record, rejected: &[i64]) -> bool {
    matches(t, r) && !rejected.contains(&r.id) && (r.instrumental || r.has_words())
}

pub struct Client {
    /// API root, "https://lrclib.net" (a local mock in tests).
    root: String,
    agent: String,
}

impl Client {
    pub fn new(root: impl Into<String>, agent: impl Into<String>) -> Self {
        Self { root: root.into().trim_end_matches('/').to_owned(), agent: agent.into() }
    }

    fn get(&self, path: &str, params: &[(String, String)]) -> Result<http_client::Response, Error> {
        let url = format!("{}{path}?{}", self.root, http_client::form_encode(params));
        let r = http_client::get(&url, &self.agent, TIMEOUT_MS).map_err(|e| Error::Network(e.to_string()))?;
        match r.status {
            429 | 503 => Err(Error::Busy(r.retry_after)),
            _ => Ok(r),
        }
    }

    /// `GET /api/get`: the record for exactly this track, or `None` (404).
    pub fn exact(&self, t: &TrackInfo) -> Result<Option<Record>, Error> {
        let mut p = vec![("artist_name".to_owned(), t.artist.clone()), ("track_name".to_owned(), t.title.clone())];
        if !t.album.trim().is_empty() {
            p.push(("album_name".to_owned(), t.album.clone()));
        }
        p.push(("duration".to_owned(), format!("{}", t.duration.round() as i64)));
        let r = self.get("/api/get", &p)?;
        match r.status {
            200 => serde_json::from_str(&r.body).map(Some).map_err(|e| Error::Server(format!("unexpected answer: {e}"))),
            404 => Ok(None),
            s => Err(Error::Server(format!("HTTP {s}"))),
        }
    }

    /// `GET /api/search` by title and artist (structured, not free text): up to 20 records.
    pub fn search(&self, t: &TrackInfo) -> Result<Vec<Record>, Error> {
        let p = [("track_name".to_owned(), t.title.clone()), ("artist_name".to_owned(), t.artist.clone())];
        let r = self.get("/api/search", &p)?;
        match r.status {
            200 => serde_json::from_str(&r.body).map_err(|e| Error::Server(format!("unexpected answer: {e}"))),
            404 => Ok(Vec::new()),
            s => Err(Error::Server(format!("HTTP {s}"))),
        }
    }

    /// The exact lookup, then the search; `rejected` are records the user turned down.
    pub fn lookup(&self, t: &TrackInfo, rejected: &[i64]) -> Result<Answer, Error> {
        if let Some(r) = self.exact(t)?
            && !rejected.contains(&r.id)
        {
            match r.answer() {
                Answer::NotFound => {}
                a => return Ok(a),
            }
        }
        let results = self.search(t)?;
        let album = fold(&t.album);
        // Of the acceptable ones: the same album first, then synced, then the closest length.
        let best = results
            .iter()
            .filter(|r| acceptable(t, r, rejected))
            .min_by(|a, b| {
                let key = |r: &Record| {
                    (
                        r.album_name.as_deref().map(fold) != Some(album.clone()),
                        r.synced_lyrics.as_deref().is_none_or(|s| s.trim().is_empty()),
                    )
                };
                key(a).cmp(&key(b)).then((a.duration - t.duration).abs().total_cmp(&(b.duration - t.duration).abs()))
            });
        Ok(best.map_or(Answer::NotFound, Record::answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> TrackInfo {
        TrackInfo { artist: "Beyoncé".into(), title: "Halo".into(), album: "I Am... Sasha Fierce".into(), duration: 261.4 }
    }

    fn rec(artist: &str, title: &str, duration: f64) -> Record {
        Record {
            id: 1,
            track_name: title.into(),
            artist_name: artist.into(),
            album_name: None,
            duration,
            instrumental: false,
            plain_lyrics: Some("words".into()),
            synced_lyrics: None,
        }
    }

    #[test]
    fn matching_folds_and_checks_the_length() {
        assert!(matches(&t(), &rec("BEYONCE", "halo", 262.0)));
        assert!(matches(&t(), &rec("beyoncé", "  Halo ", 259.5)));
        assert!(!matches(&t(), &rec("Beyoncé", "Halo", 264.0)), "a different edit");
        assert!(!matches(&t(), &rec("Beyoncé", "Halo (Remix)", 261.0)));
        assert!(!matches(&t(), &rec("Someone Else", "Halo", 261.0)));
        let mut r = rec("Beyonce", "Halo", 261.0);
        assert!(acceptable(&t(), &r, &[]));
        assert!(!acceptable(&t(), &r, &[1]), "rejected");
        r.plain_lyrics = None;
        assert!(!acceptable(&t(), &r, &[]), "no words");
        r.instrumental = true;
        assert!(acceptable(&t(), &r, &[]), "instrumental");
    }

    #[test]
    fn records_parse_as_the_api_sends_them() {
        let body = r#"{"id":3396226,"name":"I Want to Live","trackName":"I Want to Live","artistName":"Borislav Slavov","albumName":"Baldur's Gate 3","duration":233,"instrumental":false,"hasWordSync":false,"plainLyrics":"I feel your breath upon my neck\nA soft caress","syncedLyrics":"[00:17.12] I feel your breath upon my neck\n[00:20.85] A soft caress","lyricsfile":"version: '1.0'"}"#;
        let r: Record = serde_json::from_str(body).unwrap();
        assert_eq!((r.id, r.duration), (3396226, 233.0));
        match r.answer() {
            Answer::Found { id, lyrics } => {
                assert_eq!(id, 3396226);
                assert!(lyrics.synced);
                assert_eq!(lyrics.lines[1].time_ms, Some(20_850));
                assert_eq!(lyrics.lines[0].text, "I feel your breath upon my neck");
            }
            a => panic!("{a:?}"),
        }
        let inst: Record = serde_json::from_str(r#"{"id":5,"trackName":"x","artistName":"y","albumName":null,"duration":100.5,"instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#).unwrap();
        assert_eq!(inst.answer(), Answer::Instrumental { id: 5 });
    }
}
