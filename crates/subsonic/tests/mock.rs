//! The client against a local mock of a Navidrome server on 127.0.0.1 (no request leaves
//! this machine). The mock checks every request's authentication as Navidrome does (token =
//! md5(password + salt), never a password) and answers with JSON shaped like Navidrome's
//! (0.5x, OpenSubsonic): sign-in, every read call, paging a whole catalog through an
//! empty-query search3, cover bytes, ranged and transcoded streams, each class of error,
//! the read-only guard, and that no error text or log line carries a secret.

// The song in `json!` is a large literal.
#![recursion_limit = "256"]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use subsonic::{AlbumList, Client, Credentials, Error, PlaylistUpdate, Quality, Search, StreamHead, StreamSink};

const USER: &str = "alice";
/// With characters that need escaping, as a real one might.
const PASSWORD: &str = "s3same & open/?=%";

// ---- a log that keeps every line, to look for secrets in ------------------------------

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Keep;

impl log::Log for Keep {
    /// Every line of this crate and the HTTP client, down to trace; other crates' down to
    /// debug. ureq (the client's backend off Windows) writes a request's whole URL only when
    /// its own trace level is on (else `/******`), which a logger asks for knowingly: the
    /// app's never does (it keeps other crates' warnings and errors only).
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Debug || m.target().starts_with("subsonic") || m.target().starts_with("http_client")
    }
    fn log(&self, r: &log::Record) {
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

// ---- the mock -----------------------------------------------------------------------

/// One request as the mock read it.
#[derive(Clone, Debug, Default)]
struct Req {
    path: String,
    endpoint: String,
    params: Vec<(String, String)>,
    headers: BTreeMap<String, String>,
    raw: String,
    auth_ok: bool,
}

impl Req {
    fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    fn params(&self, k: &str) -> Vec<&str> {
        self.params.iter().filter(|(n, _)| n == k).map(|(_, v)| v.as_str()).collect()
    }
    fn num(&self, k: &str) -> usize {
        self.param(k).map_or(20, |v| v.parse().unwrap())
    }
}

struct Reply {
    status: u16,
    content_type: String,
    headers: Vec<String>,
    body: Vec<u8>,
    chunked: bool,
}

const ENVELOPE: &str = r#""status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.53.3 (13af8ed4)","openSubsonic":true"#;

fn ok(inner: &str) -> Reply {
    let sep = if inner.is_empty() { "" } else { "," };
    json(200, &format!(r#"{{"subsonic-response":{{{ENVELOPE}{sep}{inner}}}}}"#))
}

fn failed(code: i64, message: &str) -> Reply {
    let body = serde_json::json!({"subsonic-response": {"status": "failed", "version": "1.16.1", "type": "navidrome", "serverVersion": "0.53.3", "openSubsonic": true, "error": {"code": code, "message": message}}});
    json(200, &body.to_string())
}

fn json(status: u16, body: &str) -> Reply {
    Reply { status, content_type: "application/json".into(), headers: vec![], body: body.as_bytes().to_vec(), chunked: false }
}

fn raw(status: u16, content_type: &str, body: &[u8]) -> Reply {
    Reply { status, content_type: content_type.into(), headers: vec![], body: body.to_vec(), chunked: false }
}

struct Mock {
    url: String,
    seen: Arc<Mutex<Vec<Req>>>,
    connections: Arc<AtomicUsize>,
}

impl Mock {
    fn seen(&self) -> Vec<Req> {
        self.seen.lock().unwrap().clone()
    }
    fn last(&self) -> Req {
        self.seen().last().cloned().expect("no request")
    }
    fn client(&self, read_only: bool) -> Client {
        Client::new(Credentials::from_password(&self.url, USER, PASSWORD).unwrap(), read_only).unwrap()
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

/// A server under `prefix` ("" or "/navidrome") answering with `handler` once a request's
/// authentication checks out, and with Navidrome's error 40 when it does not.
fn mock_at(prefix: &str, handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}{prefix}", listener.local_addr().unwrap().port());
    let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
    let connections = Arc::new(AtomicUsize::new(0));
    let (s, n) = (seen.clone(), connections.clone());
    let handler = Arc::new(handler);
    let prefix = prefix.to_owned();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            n.fetch_add(1, Ordering::SeqCst);
            let (s, handler, prefix) = (s.clone(), handler.clone(), prefix.clone());
            std::thread::spawn(move || answer(conn, &prefix, &s, &*handler));
        }
    });
    Mock { url, seen, connections }
}

fn mock(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> Mock {
    mock_at("", handler)
}

fn answer(mut conn: TcpStream, prefix: &str, seen: &Mutex<Vec<Req>>, handler: &dyn Fn(&Req) -> Reply) {
    let mut r = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut req = Req { raw: line.trim_end().to_owned(), ..Req::default() };
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            req.headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let target = req.raw.split(' ').nth(1).unwrap_or("").to_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    req.path = path.to_owned();
    req.params = query.split('&').filter(|kv| !kv.is_empty()).map(|kv| kv.split_once('=').unwrap_or((kv, ""))).map(|(k, v)| (decode(k), decode(v))).collect();
    req.endpoint = path.strip_prefix(prefix).and_then(|p| p.strip_prefix("/rest/")).unwrap_or("").trim_end_matches(".view").to_owned();
    // Navidrome's check: the user, and md5(password + salt) for the token; never a password.
    let (salt, token) = (req.param("s").unwrap_or(""), req.param("t").unwrap_or(""));
    req.auth_ok = req.param("u") == Some(USER) && !salt.is_empty() && token == subsonic::token(PASSWORD, salt) && req.param("p").is_none();
    seen.lock().unwrap().push(req.clone());
    let reply = if !req.auth_ok {
        failed(40, "Wrong username or password")
    } else {
        assert_eq!(req.param("v"), Some("1.16.1"));
        assert_eq!(req.param("c"), Some("MarqueePlayer"));
        assert_eq!(req.param("f"), Some("json"));
        handler(&req)
    };
    let mut head = format!("HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n", reply.status, reply.content_type);
    for h in &reply.headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    if reply.chunked {
        head.push_str("Transfer-Encoding: chunked\r\n\r\n");
        let _ = conn.write_all(head.as_bytes());
        for part in reply.body.chunks(5000) {
            let _ = conn.write_all(format!("{:x}\r\n", part.len()).as_bytes());
            let _ = conn.write_all(part);
            let _ = conn.write_all(b"\r\n");
        }
        let _ = conn.write_all(b"0\r\n\r\n");
    } else {
        head.push_str(&format!("Content-Length: {}\r\n\r\n", reply.body.len()));
        let _ = conn.write_all(head.as_bytes());
        let _ = conn.write_all(&reply.body);
    }
    let _ = conn.flush();
}

// ---- a small Navidrome library -------------------------------------------------------

const SONGS: usize = 7;

fn song(i: usize) -> serde_json::Value {
    let album = i / 3;
    serde_json::json!({
        "id": format!("s{i}"), "parent": format!("al{album}"), "isDir": false, "title": format!("Song {i}"),
        "album": format!("Album {album}"), "artist": "Ann & Bob", "track": i % 3 + 1, "year": 1999, "genre": "Rock",
        "coverArt": format!("mf-s{i}_6650f2a7"), "size": 31_416_000 + i, "contentType": "audio/flac", "suffix": "flac",
        "duration": 245, "bitRate": 1011, "path": format!("Ann/Album {album}/0{} - Song {i}.flac", i % 3 + 1),
        "discNumber": 1, "created": "2024-05-01T10:00:00.123456789Z", "albumId": format!("al{album}"), "artistId": "ar1",
        "type": "music", "isVideo": false, "bpm": 120, "comment": "", "sortName": format!("song {i}"), "mediaType": "song",
        "musicBrainzId": "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae", "isrc": ["USRC17607839"], "genres": [{"name": "Rock"}, {"name": "Pop"}],
        "replayGain": {"trackGain": -6.5, "albumGain": -7.1, "trackPeak": 0.98, "albumPeak": 1.0},
        "channelCount": 2, "samplingRate": 44100, "bitDepth": 16, "moods": [],
        "artists": [{"id": "ar1", "name": "Ann"}, {"id": "ar2", "name": "Bob"}], "displayArtist": "Ann & Bob",
        "albumArtists": [{"id": "ar1", "name": "Ann"}], "displayAlbumArtist": "Ann",
        "contributors": [{"role": "composer", "artist": {"id": "ar3", "name": "Cy"}}], "displayComposer": "Cy", "explicitStatus": "",
        "playCount": i, "played": "2024-06-01T08:00:00Z", "userRating": 4, "starred": if i == 0 { "2024-06-02T00:00:00Z" } else { "" }
    })
}

fn album(i: usize, with_songs: bool) -> serde_json::Value {
    let mut a = serde_json::json!({
        "id": format!("al{i}"), "name": format!("Album {i}"), "artist": "Ann", "artistId": "ar1", "coverArt": format!("al-al{i}_0"),
        "songCount": 3, "duration": 735, "playCount": 12, "created": "2024-05-01T10:00:00Z", "year": 1999, "genre": "Rock",
        "played": "2024-06-01T08:00:00Z", "userRating": 0, "genres": [{"name": "Rock"}], "musicBrainzId": "",
        "isCompilation": i == 2, "sortName": format!("album {i}"), "discTitles": [{"disc": 1, "title": ""}],
        "originalReleaseDate": {"year": 1997, "month": 3, "day": 21}, "releaseDate": {}, "releaseTypes": ["album"],
        "recordLabels": [{"name": "Label One"}], "moods": [], "artists": [{"id": "ar1", "name": "Ann"}], "displayArtist": "Ann",
        "explicitStatus": "", "version": if i == 1 { "Deluxe" } else { "" }
    });
    if with_songs {
        a["song"] = (i * 3..(i * 3 + 3).min(SONGS)).map(song).collect();
    }
    a
}

fn artist(i: usize) -> serde_json::Value {
    let name = ["", "Ann", "Bob"][i];
    serde_json::json!({"id": format!("ar{i}"), "name": name, "coverArt": format!("ar-ar{i}_0"), "albumCount": 3,
        "artistImageUrl": "https://example.invalid/share/img/x", "musicBrainzId": "", "sortName": "", "roles": ["albumartist", "artist"]})
}

/// The file `stream` sends: every byte value, so a shift shows.
fn file() -> Vec<u8> {
    (0..200_000u32).map(|i| (i % 251) as u8).collect()
}

const COVER: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 \x18\0\0\0\x30\x01\0\x9d\x01\x2a\x01\0\x01\0\x02\0\x34\x25\xa4\0\x03\x70\0\xfe\xfb\x94\0\0";

fn navidrome(r: &Req) -> Reply {
    let page = |count: &str, offset: &str, total: usize, f: &dyn Fn(usize) -> serde_json::Value| -> serde_json::Value {
        let (c, o) = (r.num(count), r.param(offset).map_or(0, |v| v.parse().unwrap()));
        (o..(o + c).min(total)).map(f).collect()
    };
    match r.endpoint.as_str() {
        "ping" => ok(""),
        "getOpenSubsonicExtensions" => ok(r#""openSubsonicExtensions":[{"name":"transcodeOffset","versions":[1]},{"name":"formPost","versions":[1]},{"name":"songLyrics","versions":[1]}]"#),
        "getScanStatus" => ok(r#""scanStatus":{"scanning":false,"count":12345,"folderCount":300,"lastScan":"2024-06-01T10:00:00Z"}"#),
        "getMusicFolders" => ok(r#""musicFolders":{"musicFolder":[{"id":1,"name":"Music Library"}]}"#),
        "getArtists" => ok(&format!(r#""artists":{{"index":[{{"name":"A","artist":[{}]}},{{"name":"B","artist":[{}]}}],"lastModified":1717236000000,"ignoredArticles":"The El La Los Las Le Les Os As O A"}}"#, artist(1), artist(2))),
        "getArtist" => {
            let mut a = artist(1);
            a["album"] = (0..3).map(|i| album(i, false)).collect();
            ok(&format!(r#""artist":{a}"#))
        }
        "getAlbumList2" => ok(&format!(r#""albumList2":{{"album":{}}}"#, page("size", "offset", 3, &|i| album(i, false)))),
        "getAlbum" => match r.param("id") {
            Some("al-forbidden") => failed(50, "User not authorized"),
            Some("al-empty") => ok(""),
            Some(id) if id.starts_with("al") && id[2..].parse::<usize>().is_ok_and(|i| i < 3) => ok(&format!(r#""album":{}"#, album(id[2..].parse().unwrap(), true))),
            _ => failed(70, "Album not found"),
        },
        "getSong" => ok(&format!(r#""song":{}"#, song(r.param("id").unwrap()[1..].parse().unwrap()))),
        "search3" => {
            assert!(r.param("query").is_some(), "search3 without a query");
            ok(&format!(
                r#""searchResult3":{{"artist":{},"album":{},"song":{}}}"#,
                page("artistCount", "artistOffset", 2, &|i| artist(i + 1)),
                page("albumCount", "albumOffset", 3, &|i| album(i, false)),
                page("songCount", "songOffset", SONGS, &song)
            ))
        }
        "getPlaylists" => ok(r#""playlists":{"playlist":[{"id":"pl1","name":"Mix","comment":"","owner":"alice","public":false,"songCount":2,"duration":490,"created":"2024-05-01T10:00:00Z","changed":"2024-05-02T10:00:00Z","coverArt":"pl-pl1_0","readonly":false}]}"#),
        "getPlaylist" => ok(&format!(r#""playlist":{{"id":"pl1","name":"Mix","owner":"alice","songCount":2,"duration":490,"entry":[{},{}]}}"#, song(0), song(5))),
        "getGenres" => ok(r#""genres":{"genre":[{"value":"Rock","songCount":12,"albumCount":3},{"value":"Jazz","songCount":1,"albumCount":1}]}"#),
        "getStarred2" => ok(&format!(r#""starred2":{{"song":[{}]}}"#, song(0))),
        "getLyricsBySongId" => ok(r#""lyricsList":{"structuredLyrics":[{"displayArtist":"Ann","displayTitle":"Song 0","lang":"eng","offset":-100,"synced":true,"line":[{"start":0,"value":"La"},{"start":2500,"value":"Di"}]},{"lang":"xxx","synced":false,"line":[{"value":"La"}]}]}"#),
        "getCoverArt" => match r.param("id") {
            Some("missing") => raw(404, "text/plain", b"404 page not found"),
            _ => raw(200, "image/webp", COVER),
        },
        "stream" => stream(r),
        _ => failed(0, "not implemented"),
    }
}

/// Navidrome's stream: the original with ranges, a live transcode chunked from the start.
fn stream(r: &Req) -> Reply {
    if r.param("id") == Some("gone") {
        return failed(70, "Song not found");
    }
    let file = file();
    if r.param("format") != Some("raw") {
        let mut t = raw(200, "audio/ogg", &file[..60_000]);
        t.chunked = true;
        return t;
    }
    let start: usize = r.headers.get("range").and_then(|v| v.strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok()).unwrap_or(0);
    if start == 0 {
        let mut f = raw(200, "audio/flac", &file);
        f.headers.push("Accept-Ranges: bytes".into());
        return f;
    }
    if start >= file.len() {
        let mut f = raw(416, "text/plain", b"");
        f.headers.push(format!("Content-Range: bytes */{}", file.len()));
        return f;
    }
    let mut f = raw(206, "audio/flac", &file[start..]);
    f.headers.push(format!("Content-Range: bytes {start}-{}/{}", file.len() - 1, file.len()));
    f
}

/// A sink that keeps what it is given.
#[derive(Default)]
struct Keep2 {
    head: Option<StreamHead>,
    body: Vec<u8>,
    stop_after: Option<usize>,
}

impl StreamSink for Keep2 {
    fn start(&mut self, head: &StreamHead) -> ControlFlow<()> {
        self.head = Some(head.clone());
        ControlFlow::Continue(())
    }
    fn chunk(&mut self, bytes: &[u8]) -> ControlFlow<()> {
        self.body.extend_from_slice(bytes);
        if self.stop_after.is_some_and(|n| self.body.len() >= n) { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }
}

/// A port nothing listens on.
fn offline() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}

// ---- tests ---------------------------------------------------------------------------

#[test]
fn sign_in_sends_a_token_and_never_the_password() {
    capture_logs();
    let m = mock(navidrome);
    let (creds, info) = subsonic::sign_in(&format!(" {}/ ", m.url), " alice ", PASSWORD).unwrap();
    assert_eq!(creds.url, m.url);
    assert_eq!(creds.user, USER);
    assert_eq!(creds.token, subsonic::token(PASSWORD, &creds.salt));
    assert_eq!((info.status.as_str(), info.version.as_str(), info.kind.as_str()), ("ok", "1.16.1", "navidrome"));
    assert_eq!(info.server_version, "0.53.3 (13af8ed4)");
    assert!(info.open_subsonic);
    let r = m.last();
    assert_eq!(r.path, "/rest/ping.view");
    assert!(r.auth_ok);
    assert_eq!(r.param("s"), Some(creds.salt.as_str()));
    assert!(!r.raw.contains("s3same") && !r.raw.contains(&http_client::encode(PASSWORD)), "the password went out");
    assert!(r.headers.get("user-agent").is_some_and(|a| a.starts_with("MarqueePlayer/")));
    // Each sign-in makes a new salt.
    let (again, _) = subsonic::sign_in(&m.url, USER, PASSWORD).unwrap();
    assert_ne!(again.salt, creds.salt);
}

#[test]
fn a_path_before_rest_is_kept() {
    let m = mock_at("/navidrome", navidrome);
    let (creds, _) = subsonic::sign_in(&format!("{}/rest/", m.url), USER, PASSWORD).unwrap();
    assert!(creds.url.ends_with("/navidrome"));
    assert_eq!(m.last().path, "/navidrome/rest/ping.view");
}

#[test]
fn every_read_parses_navidromes_answers_and_is_signed() {
    capture_logs();
    let m = mock(navidrome);
    let c = m.client(true);

    let ext = c.open_subsonic_extensions().unwrap();
    assert_eq!(ext.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["transcodeOffset", "formPost", "songLyrics"]);
    assert_eq!(ext[2].versions, [1]);

    let scan = c.scan_status().unwrap();
    assert_eq!((scan.scanning, scan.count, scan.folder_count), (false, Some(12345), Some(300)));
    assert_eq!(scan.last_scan.as_deref(), Some("2024-06-01T10:00:00Z"));

    let folders = c.music_folders().unwrap();
    assert_eq!((folders[0].id.as_str(), folders[0].name.as_str()), ("1", "Music Library"));

    let artists = c.artists(None).unwrap();
    assert_eq!(artists.all().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Ann", "Bob"]);
    assert!(artists.ignored_articles.starts_with("The El"));
    let ann = c.artist("ar1").unwrap();
    assert_eq!((ann.album_count, ann.album.len(), ann.roles.len()), (3, 3, 2));
    assert_eq!(m.last().param("id"), Some("ar1"));

    let al = c.album("al0").unwrap();
    assert_eq!((al.name.as_str(), al.artist.as_str(), al.song_count, al.duration), ("Album 0", "Ann", 3, 735));
    assert_eq!(al.original_release_date.unwrap().year, Some(1997));
    assert_eq!(al.original_release_date.unwrap().day, Some(21));
    assert!(al.release_date.unwrap().is_empty(), "Navidrome's {{}} is no date");
    assert_eq!((al.record_labels[0].name.as_str(), al.release_types.as_slice()), ("Label One", &["album".to_owned()][..]));
    assert_eq!(al.artists[0].name, "Ann");
    assert_eq!(al.disc_titles[0].disc, 1);
    assert!(!al.is_compilation && c.album("al2").unwrap().is_compilation);
    assert_eq!(c.album("al1").unwrap().version, "Deluxe");
    assert_eq!(al.song.len(), 3);

    let s = &al.song[0];
    assert_eq!((s.id.as_str(), s.title.as_str(), s.album.as_str(), s.artist.as_str()), ("s0", "Song 0", "Album 0", "Ann & Bob"));
    assert_eq!((s.track, s.disc_number, s.year, s.duration), (Some(1), Some(1), Some(1999), Some(245)));
    assert_eq!((s.display_artist.as_str(), s.display_album_artist.as_str()), ("Ann & Bob", "Ann"));
    assert_eq!(s.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Ann", "Bob"]);
    assert_eq!(s.album_artists[0].id, "ar1");
    assert_eq!(s.genres.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Rock", "Pop"]);
    assert_eq!((s.size, s.suffix.as_str(), s.content_type.as_str()), (Some(31_416_000), "flac", "audio/flac"));
    assert_eq!((s.bit_rate, s.bit_depth, s.sampling_rate, s.channel_count), (Some(1011), Some(16), Some(44100), Some(2)));
    assert_eq!(s.path, "Ann/Album 0/01 - Song 0.flac");
    assert_eq!(s.replay_gain.unwrap().track_gain, Some(-6.5));
    assert_eq!(s.replay_gain.unwrap().album_peak, Some(1.0));
    assert_eq!((s.bpm, s.user_rating, s.play_count), (Some(120), Some(4), Some(0)));
    assert_eq!(s.music_brainz_id, "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae");
    assert_eq!((s.starred.as_deref(), al.song[1].starred.as_deref()), (Some("2024-06-02T00:00:00Z"), None));
    assert_eq!(s.cover_art.as_deref(), Some("mf-s0_6650f2a7"));
    assert_eq!(s.created.as_deref(), Some("2024-05-01T10:00:00.123456789Z"));
    assert_eq!(s.contributors[0].artist.name, "Cy");
    assert_eq!(s.isrc, ["USRC17607839"]);

    assert_eq!(c.song("s4").unwrap().title, "Song 4");

    let newest = c.album_list2(&AlbumList::Newest, 2, 0, None).unwrap();
    assert_eq!(newest.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["al0", "al1"]);
    let r = m.last();
    assert_eq!((r.param("type"), r.param("size"), r.param("offset"), r.param("musicFolderId")), (Some("newest"), Some("2"), Some("0"), None));
    assert_eq!(c.album_list2(&AlbumList::Newest, 2, 2, Some("1")).unwrap().len(), 1);
    assert_eq!(m.last().param("musicFolderId"), Some("1"));
    c.album_list2(&AlbumList::ByYear { from: 1990, to: 2000 }, 10, 0, None).unwrap();
    assert_eq!((m.last().param("type"), m.last().param("fromYear"), m.last().param("toYear")), (Some("byYear"), Some("1990"), Some("2000")));
    c.album_list2(&AlbumList::ByGenre("Hip-Hop & Rap".into()), 10, 0, None).unwrap();
    assert_eq!(m.last().param("genre"), Some("Hip-Hop & Rap"));
    c.album_list2(&AlbumList::AlphabeticalByArtist, 10, 0, None).unwrap();
    assert_eq!(m.last().param("type"), Some("alphabeticalByArtist"));

    let pls = c.playlists().unwrap();
    assert_eq!((pls[0].name.as_str(), pls[0].song_count, pls[0].owner.as_str()), ("Mix", 2, "alice"));
    let pl = c.playlist("pl1").unwrap();
    assert_eq!(pl.entry.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s0", "s5"]);

    let genres = c.genres().unwrap();
    assert_eq!((genres[0].value.as_str(), genres[0].song_count, genres[0].album_count), ("Rock", 12, 3));

    let starred = c.starred2(None).unwrap();
    assert_eq!((starred.song.len(), starred.album.len(), starred.artist.len()), (1, 0, 0));

    let lyrics = c.lyrics_by_song_id("s0").unwrap();
    assert_eq!(lyrics.len(), 2);
    assert!(lyrics[0].synced);
    assert_eq!((lyrics[0].lang.as_str(), lyrics[0].offset), ("eng", Some(-100)));
    assert_eq!(lyrics[0].line[1].start, Some(2500));
    assert_eq!(lyrics[1].line[0].start, None);

    let seen = m.seen();
    assert_eq!(seen.len(), 19, "one request per call");
    for r in &seen {
        assert!(r.auth_ok, "{} was not signed right", r.endpoint);
        assert!(r.path.ends_with(".view"));
    }
}

#[test]
fn an_empty_search_pages_through_the_whole_catalog() {
    let m = mock(navidrome);
    let c = m.client(true);
    let mut songs = Vec::new();
    let mut pages = 0;
    loop {
        let page = c.search3(&Search::songs("", 3, songs.len() as u32)).unwrap();
        assert!(page.album.is_empty() && page.artist.is_empty(), "counts of 0 ask for none");
        pages += 1;
        let n = page.song.len();
        songs.extend(page.song);
        if n < 3 {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(songs.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), (0..SONGS).map(|i| format!("s{i}")).collect::<Vec<_>>());
    let r = m.last();
    assert_eq!(r.param("query"), Some(""));
    assert_eq!((r.param("songCount"), r.param("songOffset"), r.param("albumCount"), r.param("artistCount")), (Some("3"), Some("6"), Some("0"), Some("0")));

    // All kinds in one page, with offsets and counts.
    let all = c.search3(&Search { query: "", artist_count: 10, album_count: 2, album_offset: 1, song_count: 1, song_offset: 0, ..Search::default() }).unwrap();
    assert_eq!((all.artist.len(), all.album.len(), all.song.len()), (2, 2, 1));
    assert_eq!(all.album[0].id, "al1");
    assert_eq!(all.artist[0].artist_image_url.as_deref(), Some("https://example.invalid/share/img/x"));
    // Paging albums and artists alone.
    assert_eq!(c.search3(&Search::albums("", 2, 2)).unwrap().album.len(), 1);
    assert_eq!(c.search3(&Search::artists("", 1, 1)).unwrap().artist[0].name, "Bob");
}

#[test]
fn covers_come_as_bytes() {
    let m = mock(navidrome);
    let c = m.client(true);
    let cover = c.cover_art("al-al0_0", Some(300)).unwrap();
    assert_eq!(cover.bytes, COVER);
    assert_eq!(cover.content_type.as_deref(), Some("image/webp"));
    assert_eq!((m.last().param("id"), m.last().param("size")), (Some("al-al0_0"), Some("300")));
    c.cover_art("al-al0_0", None).unwrap();
    assert_eq!(m.last().param("size"), None);
    assert!(matches!(c.cover_art("missing", None), Err(Error::NotFound(_))));
}

#[test]
fn the_original_streams_whole_or_from_a_byte() {
    let m = mock(navidrome);
    let c = m.client(true);
    let file = file();

    let mut sink = Keep2::default();
    let s = c.stream("s1", &Quality::Original, 0, &mut sink).unwrap();
    assert!(sink.body == file);
    assert_eq!((s.received, s.stopped), (file.len() as u64, false));
    let h = sink.head.unwrap();
    assert_eq!((h.start, h.range_honoured, h.length, h.total), (0, true, Some(file.len() as u64), Some(file.len() as u64)));
    assert_eq!(h.content_type.as_deref(), Some("audio/flac"));
    let r = m.last();
    assert_eq!((r.param("format"), r.param("id"), r.headers.get("range")), (Some("raw"), Some("s1"), None));

    let mut sink = Keep2::default();
    let s = c.stream("s1", &Quality::Original, 150_000, &mut sink).unwrap();
    assert_eq!(m.last().headers.get("range").map(String::as_str), Some("bytes=150000-"));
    let h = s.head;
    assert_eq!((h.start, h.range_honoured, h.length, h.total), (150_000, true, Some(50_000), Some(file.len() as u64)));
    assert!(sink.body == file[150_000..]);

    // Asked from the very end: nothing more, and not an error.
    let mut sink = Keep2::default();
    let s = c.stream("s1", &Quality::Original, file.len() as u64, &mut sink).unwrap();
    assert_eq!((s.received, s.stopped, s.head.length, s.head.total), (0, false, Some(0), Some(file.len() as u64)));
    assert!(sink.head.is_some() && sink.body.is_empty());

    // The sink can stop it.
    let mut sink = Keep2 { stop_after: Some(10_000), ..Keep2::default() };
    let s = c.stream("s1", &Quality::Original, 0, &mut sink).unwrap();
    assert!(s.stopped);
    assert!(sink.body.len() < file.len());
}

#[test]
fn a_transcode_has_no_length_and_ignores_the_range() {
    let m = mock(navidrome);
    let c = m.client(true);
    let mut sink = Keep2::default();
    let s = c.stream("s1", &Quality::Transcode { format: "opus".into(), max_bit_rate: 128 }, 5000, &mut sink).unwrap();
    let r = m.last();
    assert_eq!((r.param("format"), r.param("maxBitRate")), (Some("opus"), Some("128")));
    let h = s.head;
    assert_eq!((h.start, h.range_honoured, h.length, h.total), (0, false, None, None), "the body starts at 0, whatever was asked");
    assert_eq!(sink.body, file()[..60_000]);
}

#[test]
fn a_stream_error_answer_never_reaches_the_sink() {
    let m = mock(navidrome);
    let c = m.client(true);
    let mut sink = Keep2::default();
    assert_eq!(c.stream("gone", &Quality::Original, 0, &mut sink), Err(Error::NotFound("Song not found".into())));
    assert!(sink.head.is_none() && sink.body.is_empty());
    let m = mock(|_| raw(200, "text/html", b"<html>Log in to the proxy</html>"));
    let mut sink = Keep2::default();
    assert!(matches!(m.client(true).stream("s1", &Quality::Original, 0, &mut sink), Err(Error::NotSubsonic(_))));
    assert!(sink.head.is_none());
}

#[test]
fn each_error_has_its_class() {
    capture_logs();
    let m = mock(navidrome);
    let c = m.client(true);
    // Wrong password: the token does not check out.
    let wrong = Client::new(Credentials::from_password(&m.url, USER, "nope").unwrap(), true).unwrap();
    assert!(matches!(wrong.ping(), Err(Error::WrongCredentials { code: 40, .. })));
    assert!(matches!(subsonic::sign_in(&m.url, USER, "nope"), Err(Error::WrongCredentials { code: 40, .. })));
    let other_user = Client::new(Credentials::from_password(&m.url, "mallory", PASSWORD).unwrap(), true).unwrap();
    assert!(matches!(other_user.ping(), Err(Error::WrongCredentials { .. })));
    assert_eq!(c.album("al-forbidden"), Err(Error::NotPermitted("User not authorized".into())));
    assert_eq!(c.album("al9"), Err(Error::NotFound("Album not found".into())));
    assert!(matches!(c.album("al-empty"), Err(Error::BadResponse(_))));
    assert_eq!(c.scan_status().map(|_| ()), Ok(()));

    let generic = mock(|_| failed(0, "Something broke"));
    let e = generic.client(true).ping().unwrap_err();
    assert_eq!(e, Error::Server { code: Some(0), http_status: None, message: "Something broke".into() });
    assert!(!e.is_retryable());

    let busy = mock(|_| raw(503, "text/html", b"<html>Service Unavailable</html>"));
    let e = busy.client(true).ping().unwrap_err();
    assert_eq!(e, Error::Server { code: None, http_status: Some(503), message: String::new() });
    assert!(e.is_retryable());

    for (reply, what) in [
        (raw(200, "text/html", b"<!doctype html><html><body>Welcome to nginx</body></html>"), "a web page"),
        (raw(404, "text/plain", b"404 page not found"), "HTTP 404"),
        (json(200, r#"{"hello":"world"}"#), "JSON of another kind"),
        (raw(200, "application/octet-stream", b"\x89PNG\r\n"), "not JSON"),
    ] {
        let reply = Arc::new(Mutex::new(Some(reply)));
        let site = mock(move |_| reply.lock().unwrap().take().unwrap());
        match site.client(true).ping() {
            Err(Error::NotSubsonic(m)) => assert!(m.contains(what), "{m:?} for {what}"),
            other => panic!("{what}: {other:?}"),
        }
    }

    let e = Client::new(Credentials { url: offline(), user: USER.into(), salt: "abcd1234".into(), token: "t0k3nt0k3n".into() }, true).unwrap().ping().unwrap_err();
    assert!(matches!(e, Error::Network(_)) && e.is_retryable(), "{e:?}");

    assert_eq!(Client::new(Credentials { url: "ftp://nas".into(), user: USER.into(), salt: String::new(), token: String::new() }, true).unwrap_err(), Error::BadUrl);
    assert_eq!(subsonic::sign_in("not a url", USER, PASSWORD).unwrap_err(), Error::BadUrl);
}

#[test]
fn a_read_only_client_sends_no_write() {
    let m = mock(navidrome);
    let c = m.client(true);
    type Call = Box<dyn Fn(&Client) -> Result<(), Error>>;
    let calls: Vec<(&str, Call)> = vec![
        ("star", Box::new(|c| c.star(&["s1"], &[], &[]))),
        ("unstar", Box::new(|c| c.unstar(&[], &["al1"], &[]))),
        ("setRating", Box::new(|c| c.set_rating("s1", 4))),
        ("scrobble", Box::new(|c| c.scrobble("s1", Some(1_717_236_000_000), true))),
        ("scrobble", Box::new(|c| c.scrobble("s1", None, false))),
        ("scrobble", Box::new(|c| c.scrobble_plays(&[("s1", 1), ("s2", 2)]))),
        ("createPlaylist", Box::new(|c| c.create_playlist("New", &["s1"]).map(drop))),
        ("updatePlaylist", Box::new(|c| c.update_playlist("pl1", &PlaylistUpdate { add: &["s2"], ..PlaylistUpdate::default() }))),
        ("deletePlaylist", Box::new(|c| c.delete_playlist("pl1"))),
    ];
    for (endpoint, call) in &calls {
        assert_eq!(call(&c), Err(Error::ReadOnly(endpoint)), "{endpoint}");
        assert_eq!(m.connections.load(Ordering::SeqCst), 0, "{endpoint} reached the server");
    }
    // The same calls on a client that may write do go out, as asked.
    let w = m.client(false);
    for (endpoint, call) in &calls {
        let before = m.connections.load(Ordering::SeqCst);
        // The mock does not implement the writes ("not implemented", code 0): sent is enough.
        let _ = call(&w);
        assert_eq!(m.connections.load(Ordering::SeqCst), before + 1, "{endpoint}");
        assert_eq!(m.last().endpoint, *endpoint);
    }
}

/// A client allowed one kind of write sends that kind only: the others are refused before a
/// connection is made, as a read-only client refuses them all.
#[test]
fn a_client_sends_only_the_kinds_of_write_it_is_allowed() {
    let m = mock(|_| ok(""));
    let marks = m.client(true).with_writes(subsonic::Writes { marks: true, ..subsonic::Writes::NONE });
    assert!(!marks.is_read_only());
    marks.star(&["s1"], &[], &[]).unwrap();
    marks.unstar(&["s1"], &[], &[]).unwrap();
    marks.set_rating("s1", 3).unwrap();
    assert_eq!(m.connections.load(Ordering::SeqCst), 3);
    assert_eq!(marks.scrobble("s1", None, true), Err(Error::ReadOnly("scrobble")));
    assert_eq!(marks.create_playlist("x", &[]).map(drop), Err(Error::ReadOnly("createPlaylist")));
    assert_eq!(m.connections.load(Ordering::SeqCst), 3, "refused before a connection");
    let plays = m.client(true).with_writes(subsonic::Writes { plays: true, ..subsonic::Writes::NONE });
    plays.scrobble("s1", Some(5), true).unwrap();
    assert_eq!(plays.star(&["s1"], &[], &[]), Err(Error::ReadOnly("star")));
    assert_eq!(plays.set_rating("s1", 1), Err(Error::ReadOnly("setRating")));
    assert_eq!(plays.delete_playlist("p"), Err(Error::ReadOnly("deletePlaylist")));
    assert_eq!(m.connections.load(Ordering::SeqCst), 4);
    assert!(m.client(true).is_read_only() && !m.client(false).is_read_only());
}

#[test]
fn writes_send_what_they_say() {
    let m = mock(|r| match r.endpoint.as_str() {
        "createPlaylist" => ok(r#""playlist":{"id":"pl9","name":"New","songCount":2}"#),
        _ => ok(""),
    });
    let c = m.client(false);
    c.star(&["s1", "s2"], &["al1"], &["ar1"]).unwrap();
    let r = m.last();
    assert_eq!((r.params("id"), r.params("albumId"), r.params("artistId")), (vec!["s1", "s2"], vec!["al1"], vec!["ar1"]));
    c.set_rating("s1", 9).unwrap();
    assert_eq!(m.last().param("rating"), Some("5"));
    c.scrobble("s1", Some(1_717_236_000_000), true).unwrap();
    let r = m.last();
    assert_eq!((r.param("time"), r.param("submission")), (Some("1717236000000"), Some("true")));
    c.scrobble("s1", None, false).unwrap();
    assert_eq!((m.last().param("time"), m.last().param("submission")), (None, Some("false")));
    c.scrobble_plays(&[("s1", 10), ("s2", 20)]).unwrap();
    assert_eq!((m.last().params("id"), m.last().params("time")), (vec!["s1", "s2"], vec!["10", "20"]));
    let pl = c.create_playlist("New", &["s1", "s2"]).unwrap().unwrap();
    assert_eq!((pl.id.as_str(), m.last().params("songId")), ("pl9", vec!["s1", "s2"]));
    c.update_playlist("pl9", &PlaylistUpdate { name: Some("Renamed"), public: Some(true), add: &["s3"], remove: &[0, 2], ..PlaylistUpdate::default() }).unwrap();
    let r = m.last();
    assert_eq!((r.param("playlistId"), r.param("name"), r.param("public"), r.param("comment")), (Some("pl9"), Some("Renamed"), Some("true"), None));
    assert_eq!((r.params("songIdToAdd"), r.params("songIndexToRemove")), (vec!["s3"], vec!["0", "2"]));
    c.delete_playlist("pl9").unwrap();
    assert_eq!((m.last().endpoint.as_str(), m.last().param("id")), ("deletePlaylist", Some("pl9")));
}

#[test]
fn servers_that_send_odd_types_still_read() {
    // Ids as numbers, numbers as strings, a lone object for a list, nulls, an item that does
    // not read at all: what other Subsonic servers have been seen to send.
    let m = mock(|r| match r.endpoint.as_str() {
        "getAlbum" => ok(r#""album":{"id":17,"name":"X","songCount":"2","year":null,"isCompilation":"true","song":{"id":5,"title":"Only","track":"3","duration":201.6,"starred":null}}"#),
        _ => ok(r#""albumList2":{"album":[{"id":"a1","name":"Fine"},"garbage",{"id":"a2","name":"Also fine","genres":null}]}"#),
    });
    let c = m.client(true);
    let a = c.album("17").unwrap();
    assert_eq!((a.id.as_str(), a.song_count, a.year, a.is_compilation), ("17", 2, None, true));
    assert_eq!((a.song[0].id.as_str(), a.song[0].track, a.song[0].duration, a.song[0].starred.clone()), ("5", Some(3), Some(201), None));
    let list = c.album_list2(&AlbumList::Random, 10, 0, None).unwrap();
    assert_eq!(list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["a1", "a2"]);
}

#[test]
fn no_error_or_log_line_carries_a_secret() {
    capture_logs();
    // A server (or a proxy) that echoes the whole request line, token and salt included,
    // in its error message.
    let m = mock(|r| failed(0, &format!("cannot handle {}", r.raw)));
    let c = m.client(true);
    let creds = c.credentials().clone();
    let mut texts = Vec::new();
    let mut keep = |e: Error| texts.push(format!("{e} / {e:?}"));
    keep(c.ping().unwrap_err());
    keep(c.album("x").unwrap_err());
    keep(c.cover_art("x", None).unwrap_err());
    keep(c.stream("x", &Quality::Original, 0, &mut Keep2::default()).unwrap_err());
    keep(c.star(&["x"], &[], &[]).unwrap_err());
    let wrong = Client::new(Credentials::from_password(&mock(navidrome).url, USER, "nope").unwrap(), true).unwrap();
    let wrong_creds = wrong.credentials().clone();
    keep(wrong.ping().unwrap_err());
    let off = Client::new(Credentials::from_password(&offline(), USER, PASSWORD).unwrap(), true).unwrap();
    let off_creds = off.credentials().clone();
    keep(off.ping().unwrap_err());
    keep(off.stream("x", &Quality::Original, 10, &mut Keep2::default()).unwrap_err());
    texts.push(format!("{c:?} {creds:?}"));
    assert!(texts[0].contains("cannot handle"), "the server's message is kept: {}", texts[0]);

    let lines = LINES.lock().unwrap().clone();
    assert!(!lines.is_empty());
    for t in texts.iter().chain(&lines) {
        for secret in [&creds.token, &creds.salt, &wrong_creds.token, &wrong_creds.salt, &off_creds.token, &off_creds.salt] {
            assert!(!t.contains(secret.as_str()), "a secret in: {t}");
        }
        assert!(!t.contains(PASSWORD) && !t.contains("s3same"), "the password in: {t}");
        assert!(!t.contains("/rest/") || t.contains("cannot handle GET /rest/"), "a URL in: {t}");
    }
}
