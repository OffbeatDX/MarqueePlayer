//! The downloader and the stream cache against a local mock of the server (`streammock`), with
//! the real engine on a device that writes nowhere where playing matters. What must hold: each
//! format starts well before its download ends on a slow link (Ogg and MP4 with `moov` at the end
//! through a range for their end); a seek ahead is a range request; a stall and a dropped
//! connection go on from where they stopped; a missing file fails at once with words for it; a
//! cached file plays with no request at all; a partial file after a crash is completed, never
//! played as whole; two server tracks are gapless; eviction keeps the cap and the kept tracks;
//! skipping and quitting leave no thread or connection behind. Needs ffmpeg (skips without).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use player_core::output::Device;
use player_core::{Event, Player};

use super::*;
use crate::server::streammock::{self, FORMATS, Mock};

/// A device that takes audio at about ten times real time and keeps nothing.
#[derive(Clone, Default)]
struct Null {
    frames: Arc<AtomicUsize>,
}

impl Device for Null {
    fn open(&mut self, _rate: u32) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {
        std::thread::sleep(Duration::from_millis(1));
    }
    fn available(&mut self) -> Result<usize, String> {
        Ok(441)
    }
    fn queued(&mut self) -> usize {
        0
    }
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.frames.fetch_add(samples.len() / 2, Ordering::Relaxed);
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self) {}
    fn flush(&mut self) {}
    fn realtime(&self) -> bool {
        false
    }
}

fn player() -> (Player, Receiver<Event>) {
    Player::with_device(|| Box::new(Null::default()))
}

fn dir(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join("mp-stream-tests").join(format!("{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn fixtures(seconds: u32) -> Option<Vec<PathBuf>> {
    if !streammock::have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    Some(streammock::fixtures(&std::env::temp_dir().join("mp-stream-fixtures"), seconds))
}

fn info(id: &str, bytes: &[u8], suffix: &str) -> StreamInfo {
    StreamInfo { song_id: id.to_owned(), suffix: Some(suffix.to_owned()), content_type: None, size: bytes.len() as i64 }
}

/// A streamer on a fresh folder, signed in to the mock, quick to notice a stall.
fn streamer(mock: &Mock, name: &str) -> Arc<Streamer> {
    let mut cfg = Config::new(dir(name), DEFAULT_CAP);
    cfg.read_timeout_ms = 700;
    cfg.backoff = vec![Duration::from_millis(100), Duration::from_millis(200)];
    cfg.attempts = 4;
    let s = Streamer::new(cfg);
    s.set_client(Some(mock.client()));
    s
}

fn wait_until(limit: Duration, what: &str, mut f: impl FnMut() -> bool) -> Duration {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < limit, "{what}: not within {limit:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    t.elapsed()
}

/// Events until `f` matches one (which is returned), within `limit`.
fn until(rx: &Receiver<Event>, limit: Duration, what: &str, f: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(e) if f(&e) => return e,
            Ok(Event::Error { message, .. }) => panic!("{what}: engine error {message}"),
            Ok(_) => {}
            Err(_) => panic!("{what}: not within {limit:?}"),
        }
    }
}

fn feed_of(r: &TrackRequest) -> Feed {
    r.feed.clone().expect("a growing request")
}

#[test]
fn stretches_and_gaps() {
    let mut f = Vec::new();
    add_range(&mut f, 10..20);
    add_range(&mut f, 30..40);
    add_range(&mut f, 20..25);
    assert_eq!(f, vec![10..25, 30..40]);
    assert_eq!(gaps(&f, 50), vec![0..10, 25..30, 40..50]);
    // A reader's offset first, from there to the end of its gap.
    assert_eq!(next_stretch(&f, 50, Some(27), 0), Some(27..30));
    // Else on from where the last stretch ended, then round to the start.
    assert_eq!(next_stretch(&f, 50, None, 25), Some(25..30));
    assert_eq!(next_stretch(&f, 50, None, 45), Some(45..50));
    assert_eq!(next_stretch(&f, 50, None, 50), Some(0..10));
    add_range(&mut f, 0..50);
    assert_eq!(next_stretch(&f, 50, None, 0), None);
}

#[test]
fn keys_name_the_song_and_its_size() {
    let i = StreamInfo { song_id: "3f2a9cDe_x-1".into(), suffix: Some("FLAC".into()), content_type: None, size: 1234 };
    assert_eq!(key(&i), "3f2a9cDe_x-1-1234");
    assert_eq!(extension(&i), "flac");
    // A changed file under the same id is another file.
    assert_ne!(key(&i), key(&StreamInfo { size: 1235, ..i.clone() }));
    // Ids that cannot be file names are hashed.
    let odd = StreamInfo { song_id: "a/b:c".into(), suffix: Some("../x".into()), ..i };
    assert!(key(&odd).starts_with('x') && !key(&odd).contains(['/', ':']));
    assert_eq!(extension(&odd), "audio");
}

/// The time to first sound of each format on a link that takes ~4 s for the whole file: well
/// under half of it, each format opening as it must (Opus and the MP4 with `moov` at the end
/// through a range for their end).
#[test]
fn each_format_starts_well_before_its_download_ends() {
    let Some(files) = fixtures(20) else { return };
    let mut report = String::new();
    for (i, (name, _, ct)) in FORMATS.iter().enumerate() {
        let bytes = fs::read(&files[i]).unwrap();
        let mock = Mock::start();
        let id = format!("s{i}");
        mock.add(&id, bytes.clone(), ct);
        // About 4 s for the whole file.
        let rate = bytes.len() as u64 / 4;
        mock.set(|b| b.rate = rate);
        let s = streamer(&mock, name);
        let (p, rx) = player();
        let t = Instant::now();
        let req = s.request(1, &info(&id, &bytes, name.rsplit('.').next().unwrap())).unwrap();
        let feed = feed_of(&req);
        p.play(req);
        until(&rx, Duration::from_secs(15), name, |e| matches!(e, Event::TrackStarted { token: 1 }));
        let started = t.elapsed();
        // What had come from the server by then (the folder's sizes lag for a file being written).
        let sent_at_start = mock.sent();
        // Said in full when it does not come: which format, how far the server got, what it
        // was asked and what it is doing.
        let since = Instant::now();
        while !feed.is_complete() {
            assert!(
                since.elapsed() < Duration::from_secs(20),
                "{name}: not whole within 20 s of its first sound ({:.0} ms): the server sent {} of {} bytes at {rate} B/s, was asked from {:?}, is doing {:?}; the reader waits at {:?}",
                started.as_secs_f64() * 1000.0,
                mock.sent(),
                bytes.len(),
                mock.streams().iter().map(|r| r.from).collect::<Vec<_>>(),
                mock.doing(),
                feed.wanted()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let whole = t.elapsed();
        let ranges: Vec<u64> = mock.streams().iter().map(|r| r.from).filter(|&f| f > 0).collect();
        let line = format!(
            "{name}: {} KB at {} KB/s: first sound {:.0} ms, whole {:.0} ms ({:.0}% sent by the start), {} requests, ranges from {ranges:?}\n",
            bytes.len() / 1024,
            rate / 1024,
            started.as_secs_f64() * 1000.0,
            whole.as_secs_f64() * 1000.0,
            100.0 * sent_at_start as f64 / bytes.len() as f64,
            mock.streams().len()
        );
        eprint!("{line}");
        report.push_str(&line);
        assert!(started.as_secs_f64() < whole.as_secs_f64() * 0.4, "{name} started late: {line}");
        if name.ends_with(".opus") || name.contains("moov-end") {
            assert!(!ranges.is_empty(), "{name}: its end was fetched first, by a range");
        }
        // The file on disk is the server's, byte for byte.
        let cached = fs::read(data_path(s.dir(), &info(&id, &bytes, name.rsplit('.').next().unwrap()))).unwrap();
        assert!(cached == bytes, "{name}: the cached file differs");
        p.stop();
        drop(p);
        assert!(s.shutdown_and_wait(Duration::from_secs(5)));
    }
    let _ = fs::write(std::env::temp_dir().join("mp-stream-tests").join("first-sound.txt"), report);
}

#[test]
fn a_seek_ahead_is_a_range_request() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[0]).unwrap();
    let mock = Mock::start();
    mock.add("f", bytes.clone(), "audio/flac");
    // 10 s for the whole file: a seek to 15 s is far past the download.
    mock.set(|b| b.rate = bytes.len() as u64 / 10);
    let s = streamer(&mock, "seek");
    let (p, rx) = player();
    let req = s.request(1, &info("f", &bytes, "flac")).unwrap();
    let feed = feed_of(&req);
    p.play(req);
    until(&rx, Duration::from_secs(10), "start", |e| matches!(e, Event::TrackStarted { token: 1 }));
    let t = Instant::now();
    p.seek(15.0);
    wait_until(Duration::from_secs(6), "playing past 15 s", || p.status().position > 15.2);
    let took = t.elapsed();
    let far: Vec<u64> = mock.streams().iter().map(|r| r.from).filter(|&f| f > bytes.len() as u64 / 2).collect();
    eprintln!("seek to 15 s of 20: playing there after {took:?}; ranges from {far:?}; whole: {}", feed.is_complete());
    assert!(!far.is_empty(), "a range from the seek's place");
    assert!(!feed.is_complete(), "played there before the download got there");
    p.stop();
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
}

#[test]
fn a_stall_and_a_drop_go_on_from_where_they_stopped() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[1]).unwrap();
    let len = bytes.len() as u64;
    for drop in [false, true] {
        let mock = Mock::start();
        mock.add("m", bytes.clone(), "audio/mpeg");
        let at = len / 2 + 777;
        mock.set(|b| {
            b.rate = len / 2;
            if drop {
                b.drop_at = Some(at);
            } else {
                // Until the streamer gives up on the connection (its read timeout), however long
                // the system takes to: the stall always ends in a retry.
                b.stall_at = Some((at, Duration::from_secs(60)));
            }
        });
        let s = streamer(&mock, "stall");
        let req = s.request(1, &info("m", &bytes, "mp3")).unwrap();
        let feed = feed_of(&req);
        // Kept (the current track) though nothing reads it.
        s.keep(&[info("m", &bytes, "mp3")], None);
        wait_until(Duration::from_secs(20), "whole", || feed.is_complete());
        let froms: Vec<u64> = mock.streams().iter().map(|r| r.from).collect();
        eprintln!("{}: requests from {froms:?}; {} bytes sent for {len}", if drop { "drop" } else { "stall" }, mock.sent());
        // One retry at the stall or the drop; a machine under load may add one where the paced
        // body came too slowly for the read timeout, which must go on from where it stopped too.
        assert!(froms.len() >= 2 && froms[0] == 0, "retried: {froms:?}");
        assert!(froms.windows(2).all(|w| w[1] > w[0]), "each retry goes on from further on: {froms:?}");
        assert!(froms.iter().any(|&f| f >= at - 64 * 1024 && f <= at), "went on from where it stopped: {froms:?}");
        let retries = froms.len() as u64 - 1;
        assert!((mock.sent() as u64) < len + retries * 64 * 1024, "nothing fetched twice but a chunk a retry");
        assert_eq!(fs::read(data_path(s.dir(), &info("m", &bytes, "mp3"))).unwrap(), bytes);
        // The feed is whole once every byte is filled; the download then syncs the file and
        // removes its sidecar, a moment later (sooner than a test thread looks, on Windows).
        let sidecar = sidecar_path(s.dir(), &key(&info("m", &bytes, "mp3")));
        wait_until(Duration::from_secs(5), "whole: no sidecar", || !sidecar.exists());
        assert!(s.shutdown_and_wait(Duration::from_secs(5)));
    }
}

/// A server that sends the whole file whatever range is asked: the Opus file (whose open wants
/// its end) still plays, read in order once the server is seen to ignore ranges, never asked
/// from the start again and again.
#[test]
fn a_server_that_ignores_ranges_is_read_in_order() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[2]).unwrap();
    let mock = Mock::start();
    mock.add("o", bytes.clone(), "audio/ogg");
    mock.set(|b| {
        b.ignore_ranges = true;
        b.rate = bytes.len() as u64 / 3;
    });
    let s = streamer(&mock, "noranges");
    let (p, rx) = player();
    let req = s.request(1, &info("o", &bytes, "opus")).unwrap();
    let feed = feed_of(&req);
    p.play(req);
    until(&rx, Duration::from_secs(15), "start", |e| matches!(e, Event::TrackStarted { token: 1 }));
    wait_until(Duration::from_secs(15), "whole", || feed.is_complete());
    let n = mock.streams().len();
    eprintln!("ranges ignored: {n} requests");
    assert!(n <= 3, "asked {n} times");
    assert_eq!(fs::read(data_path(s.dir(), &info("o", &bytes, "opus"))).unwrap(), bytes);
    p.stop();
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
}

#[test]
fn a_server_that_keeps_failing_is_given_up_on_and_said() {
    let mock = Mock::start();
    let bytes = vec![7u8; 100_000];
    mock.add("x", bytes.clone(), "audio/flac");
    mock.set(|b| b.fail_next = 100);
    let s = streamer(&mock, "503");
    let req = s.request(1, &info("x", &bytes, "flac")).unwrap();
    let feed = feed_of(&req);
    s.keep(&[info("x", &bytes, "flac")], None);
    wait_until(Duration::from_secs(10), "failed", || feed.failure().is_some());
    assert_eq!(mock.streams().len(), 4, "the attempts set");
    assert_eq!(feed.failure().as_deref(), Some("the server failed (HTTP 503)"));
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
    assert_eq!(s.failure("x").as_deref(), Some("the server failed (HTTP 503)"));
}

#[test]
fn a_missing_file_fails_at_once_and_the_engine_moves_on() {
    let mock = Mock::start();
    let s = streamer(&mock, "gone");
    let (p, rx) = player();
    let bytes = vec![0u8; 5000];
    let req = s.request(1, &info("gone", &bytes, "flac")).unwrap();
    p.play(req);
    let t = Instant::now();
    let e = loop {
        if let e @ Event::Error { .. } = rx.recv_timeout(Duration::from_secs(5)).expect("an event") {
            break e;
        }
    };
    eprintln!("404: {e:?} after {:?}", t.elapsed());
    assert!(matches!(&e, Event::Error { token: 1, message } if message.contains("no longer on the server")), "{e:?}");
    until(&rx, Duration::from_secs(5), "ended", |e| matches!(e, Event::Ended));
    assert_eq!(mock.streams().len(), 1, "not retried");
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
    assert_eq!(s.failure("gone").as_deref(), Some("the file is no longer on the server"));
    assert_eq!(s.used(), 0, "nothing left in the cache");
}

#[test]
fn a_cached_file_plays_with_no_request_signed_in_or_not() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[0]).unwrap();
    let mock = Mock::start();
    mock.add("c", bytes.clone(), "audio/flac");
    let s = streamer(&mock, "cached");
    let i = info("c", &bytes, "flac");
    let feed = feed_of(&s.request(1, &i).unwrap());
    s.keep(std::slice::from_ref(&i), None);
    wait_until(Duration::from_secs(10), "whole", || feed.is_complete());
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
    let asked = mock.seen().len();
    // A new start of the app on the same folder, signed out.
    let mut cfg = Config::new(s.dir().to_path_buf(), DEFAULT_CAP);
    cfg.read_timeout_ms = 700;
    let again = Streamer::new(cfg);
    let req = again.request(2, &i).unwrap();
    assert!(req.feed.is_none(), "a plain file");
    let (p, rx) = player();
    p.play(req);
    until(&rx, Duration::from_secs(5), "start", |e| matches!(e, Event::TrackStarted { token: 2 }));
    again.set_client(Some(mock.client()));
    assert!(again.request(3, &i).unwrap().feed.is_none());
    assert_eq!(mock.seen().len(), asked, "no request at all");
}

#[test]
fn a_partial_file_after_a_crash_is_completed_never_played_as_whole() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[0]).unwrap();
    let len = bytes.len() as u64;
    let mock = Mock::start();
    mock.add("p", bytes.clone(), "audio/flac");
    let i = info("p", &bytes, "flac");
    let d = dir("crash");
    // What a crash leaves: the first 40% and a stretch near the end on disk, listed in the
    // sidecar; and a claim past what the file holds (never written), which must not count.
    let path = data_path(&d, &i);
    let mut f = fs::File::create(&path).unwrap();
    let head = len * 2 / 5;
    f.write_all(&bytes[..head as usize]).unwrap();
    f.seek(SeekFrom::Start(len - 50_000)).unwrap();
    f.write_all(&bytes[(len - 50_000) as usize..(len - 20_000) as usize]).unwrap();
    drop(f);
    write_sidecar(&sidecar_path(&d, &key(&i)), len, &[0..head, len - 50_000..len]).unwrap();
    let mut cfg = Config::new(d.clone(), DEFAULT_CAP);
    cfg.read_timeout_ms = 700;
    let s = Streamer::new(cfg);
    // Signed out: a partial file is no file to play.
    assert_eq!(s.request(1, &i).unwrap_err(), Refusal::NoClient);
    s.set_client(Some(mock.client()));
    let req = s.request(2, &i).unwrap();
    let feed = feed_of(&req);
    s.keep(std::slice::from_ref(&i), None);
    wait_until(Duration::from_secs(10), "whole", || feed.is_complete());
    let froms: Vec<u64> = mock.streams().iter().map(|r| r.from).collect();
    eprintln!("crash: requests from {froms:?}, {} bytes fetched of {len}", mock.sent());
    assert!(!froms.contains(&0), "nothing asked from the start again: {froms:?}");
    // What was missing, and at most the end of the chunk that crossed into what was there.
    assert!((mock.sent() as u64) < len - head + 64 * 1024, "only what was missing");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));

    // A file left without its sidecar whose size is not the catalog's: deleted, fetched again.
    fs::write(&path, &bytes[..1000]).unwrap();
    let _ = fs::remove_file(sidecar_path(&d, &key(&i)));
    let req = s.request(3, &i).unwrap();
    assert!(req.feed.is_some(), "not taken for a whole file");
    s.keep(std::slice::from_ref(&i), None);
    wait_until(Duration::from_secs(10), "whole again", || req.feed.as_ref().unwrap().is_complete());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
}

#[test]
fn two_server_tracks_are_gapless() {
    let Some(files) = fixtures(8) else { return };
    let (a, b) = (fs::read(&files[4]).unwrap(), fs::read(&files[5]).unwrap());
    let mock = Mock::start();
    mock.add("a", a.clone(), "audio/flac");
    mock.add("b", b.clone(), "audio/flac");
    // Each in about 1.5 s: the second is there long before the first ends (8 s at ~10x).
    mock.set(|x| x.rate = (a.len() as u64 * 2) / 3);
    let s = streamer(&mock, "gapless");
    let (ia, ib) = (info("a", &a, "flac"), info("b", &b, "flac"));
    let (p, rx) = player();
    p.play(s.request(1, &ia).unwrap());
    p.set_next(Some(s.request(2, &ib).unwrap()));
    s.keep(&[ia, ib], None);
    until(&rx, Duration::from_secs(10), "first", |e| matches!(e, Event::TrackStarted { token: 1 }));
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let e = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the second track");
        if matches!(e, Event::TrackStarted { token: 2 }) {
            break;
        }
        seen.push(e);
    }
    assert!(!seen.iter().any(|e| matches!(e, Event::Ended | Event::Error { .. } | Event::Buffering { token: 2, on: true })), "gapless: {seen:?}");
    p.stop();
    assert!(s.shutdown_and_wait(Duration::from_secs(5)));
}

#[test]
fn eviction_keeps_the_cap_and_the_tracks_kept() {
    let d = dir("evict");
    let mut cfg = Config::new(d.clone(), 10_000);
    let changes = Arc::new(Mutex::new(0));
    let c = changes.clone();
    cfg.changed = Some(Arc::new(move || *c.lock().unwrap() += 1));
    let s = Streamer::new(cfg);
    // Five whole files of 3,000 bytes, played in order 0..5 (0 the longest ago).
    let infos: Vec<StreamInfo> = (0..5).map(|n| StreamInfo { song_id: format!("e{n}"), suffix: Some("flac".into()), content_type: None, size: 3000 }).collect();
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (n, i) in infos.iter().enumerate() {
        let p = data_path(&d, i);
        fs::write(&p, vec![0u8; 3000]).unwrap();
        OpenOptions::new().write(true).open(&p).unwrap().set_modified(base + Duration::from_secs(n as u64 * 60)).unwrap();
    }
    // The oldest is the one playing: kept.
    s.keep(std::slice::from_ref(&infos[0]), None);
    s.evict();
    let left: Vec<bool> = infos.iter().map(|i| data_path(&d, i).exists()).collect();
    assert_eq!(left, vec![true, false, false, true, true], "the least recently played went, not the one kept");
    assert!(s.used() <= 10_000);
    // Played again: the newest now; a lower cap takes the others first.
    assert!(s.request(9, &infos[3]).unwrap().feed.is_none());
    s.keep(&[], None);
    s.set_cap(3000);
    let left: Vec<bool> = infos.iter().map(|i| data_path(&d, i).exists()).collect();
    assert_eq!(left, vec![false, false, false, true, false]);
    assert!(*changes.lock().unwrap() >= 1);
    // Clear keeps the kept; clearing all leaves nothing.
    s.keep(std::slice::from_ref(&infos[3]), None);
    s.clear(false);
    assert!(data_path(&d, &infos[3]).exists());
    s.clear(true);
    assert_eq!(s.used(), 0);
}

#[test]
fn skipping_signing_out_and_quitting_leave_nothing_behind() {
    let Some(files) = fixtures(20) else { return };
    let bytes = fs::read(&files[0]).unwrap();
    let mock = Mock::start();
    mock.add("k", bytes.clone(), "audio/flac");
    mock.add("l", bytes.clone(), "audio/flac");
    // Slow: 20 s for the file.
    mock.set(|b| b.rate = bytes.len() as u64 / 20);
    let s = streamer(&mock, "skip");
    let (ik, il) = (info("k", &bytes, "flac"), info("l", &bytes, "flac"));
    let (p, rx) = player();
    p.play(s.request(1, &ik).unwrap());
    s.keep(std::slice::from_ref(&ik), None);
    until(&rx, Duration::from_secs(10), "start", |e| matches!(e, Event::TrackStarted { token: 1 }));
    assert_eq!(s.running.load(Ordering::SeqCst), 1);
    // Skipped to another: the engine lets go of the first, which is no longer kept.
    let t = Instant::now();
    p.play(s.request(2, &il).unwrap());
    s.keep(std::slice::from_ref(&il), None);
    wait_until(Duration::from_secs(3), "the first download stops", || s.running.load(Ordering::SeqCst) == 1 && !s.lock().downloads.contains_key(&key(&ik)));
    wait_until(Duration::from_secs(3), "its connection closes", || mock.open() == 1);
    eprintln!("skip: the first download and its connection gone after {:?}", t.elapsed());
    // What it wrote stays, to go on from.
    assert!(sidecar_path(s.dir(), &key(&ik)).exists());

    // Sign-out: the playing download fails with words for it, and ends.
    let feed = s.lock().downloads.get(&key(&il)).unwrap().feed.clone();
    s.set_client(None);
    wait_until(Duration::from_secs(3), "signed out", || feed.failure().as_deref() == Some(SIGNED_OUT));
    until(&rx, Duration::from_secs(5), "the track's error", |e| matches!(e, Event::Error { token: 2, .. }) || matches!(e, Event::Ended));
    wait_until(Duration::from_secs(3), "no download", || s.running.load(Ordering::SeqCst) == 0);
    wait_until(Duration::from_secs(3), "no connection", || mock.open() == 0);
    assert_eq!(s.request(3, &ik).unwrap_err(), Refusal::NoClient);

    // Quit while downloading: the thread and the connection go.
    s.set_client(Some(mock.client()));
    let _ = s.request(4, &ik).unwrap();
    wait_until(Duration::from_secs(3), "connected", || mock.open() == 1);
    assert!(s.shutdown_and_wait(Duration::from_secs(3)), "quit: every download ended");
    wait_until(Duration::from_secs(3), "no connection after quit", || mock.open() == 0);
    drop(p);
}
