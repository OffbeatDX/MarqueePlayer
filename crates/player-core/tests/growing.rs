//! Files still being written (`Feed`), through the real engine with a capture device: a writer
//! thread copies a fixture into a temporary file in slices, with delays, telling the feed as it
//! goes, the way the app's downloader will; the engine plays the file as it grows.
//!
//! What must hold: the audio is sample for sample what the complete file plays (the capture
//! device writes nothing while the engine waits, so the two captures are equal outright), in
//! every format played here, whether the writer fills the file from its start only or also
//! fetches what a reader waits for first (`Feed::wanted`, as a downloader with ranges would);
//! `Buffering` comes in on/off pairs; commands are served within tens of milliseconds while
//! the engine waits; two growing files in a row are still gapless when the second is there in
//! time, and a crossfade into one that is not does not happen (gapless after the wait); stopping, skipping or changing the next track lets go of the file (its reader count
//! drops to 0, every waiting read having returned, and the engine's threads join); a failed
//! download is the track's `Error`, then `Ended`, before its first byte, in its middle, and
//! while a seek waits; a seek past what is on disk waits for it and tells the writer where.
//! Needs ffmpeg on PATH (skips without).

mod common;

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use common::Capture;
use player_core::source::Source;
use player_core::{Event, Feed, PlayState, Player, TrackRequest, Transition};

const RATE: usize = 48_000;
/// 10 s and a few frames (an MP4 over 128 KB, which `symphonia` seeks past rather than reads
/// through), cut for the gapless pair at a frame that is no codec's boundary.
const TOTAL: usize = 10 * RATE + 77;
const CUT: usize = 7 * RATE + 1_111;
/// A slow command is one not served within this. "Tens of milliseconds" with room for a
/// loaded machine and a debug build; the timings measured are printed.
/// A CI runner gets twice as long (`common::allowance`).
fn prompt() -> Duration {
    Duration::from_secs_f64(0.150 * common::allowance())
}

/// The formats, by file name and encoder arguments.
const FORMATS: [(&str, &[&str]); 7] = [
    ("flac.flac", &["-c:a", "flac"]),
    ("mp3.mp3", &["-c:a", "libmp3lame", "-b:a", "192k"]),
    ("opus.opus", &["-c:a", "libopus", "-b:a", "128k"]),
    ("vorbis.ogg", &["-c:a", "libvorbis", "-q:a", "5"]),
    // The mp4 muxers put `moov` after `mdat` unless asked.
    ("moov-end.m4a", &["-c:a", "aac", "-b:a", "192k"]),
    ("moov-start.m4a", &["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]),
    ("wav.wav", &["-c:a", "pcm_s16le"]),
];

struct Fixtures {
    /// By file name, as in `FORMATS`.
    files: Vec<(&'static str, PathBuf)>,
    /// The signal in two FLAC pieces, and the whole of it decoded.
    a: PathBuf,
    b: PathBuf,
    reference: Vec<f32>,
}

fn fixtures() -> Option<Fixtures> {
    if !common::have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let d = common::dir("growing-fixtures");
    let wav = d.join("reference.wav");
    if !wav.exists() {
        let expr = "0.3*sin(2*PI*440*t)+0.1*sin(2*PI*3000*t)|0.3*sin(2*PI*660*t)+0.05*sin(2*PI*5000*t)";
        let src = format!("aevalsrc={expr}:s=48000:d={}", TOTAL as f64 / RATE as f64);
        common::ffmpeg(&["-f", "lavfi", "-i", &src, "-c:a", "pcm_s16le", wav.to_str().unwrap()]);
    }
    let mut files = Vec::new();
    for (name, args) in FORMATS {
        let out = d.join(name);
        if !out.exists() {
            let mut v = vec!["-i", wav.to_str().unwrap()];
            v.extend(args);
            v.push(out.to_str().unwrap());
            common::ffmpeg(&v);
        }
        files.push((name, out));
    }
    let (a, b) = (d.join("a.flac"), d.join("b.flac"));
    for (out, filter) in [(&a, format!("atrim=end_sample={CUT}")), (&b, format!("atrim=start_sample={CUT},asetpts=PTS-STARTPTS"))] {
        if !out.exists() {
            common::ffmpeg(&["-i", wav.to_str().unwrap(), "-af", &filter, "-c:a", "flac", out.to_str().unwrap()]);
        }
    }
    let reference = common::decode(&wav);
    assert_eq!(reference.len(), TOTAL * 2);
    Some(Fixtures { files, a, b, reference })
}

/// A temporary path for one growing copy.
fn temp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = common::dir("growing-copies");
    d.join(format!("{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// How the writer writes.
#[derive(Clone)]
struct Plan {
    /// The file comes in this many slices from its start, one per `delay`.
    slices: u64,
    delay: Duration,
    /// Serve `Feed::wanted` first, a stretch of `JUMP` bytes there, as a downloader with ranges
    /// would. Without, the file only grows from its start (after its tail, with `tail`).
    jump: bool,
    /// Write the last `JUMP` bytes first, as a downloader might for the probe's sake (an MP4's
    /// `moov` at the end, an Ogg's last page).
    tail: bool,
    /// Stop once the part from the start reaches this share of the file: then stall (until the
    /// test asks for a failure, `Writer::fail`), or fail with `fail` at once.
    stop_at: Option<f64>,
    fail: Option<&'static str>,
    /// Nothing is written before this (the length is said at once).
    start_after: Duration,
    /// Nor before this holds (asked every millisecond).
    start_when: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// Well under the smallest fixture (~100 KB), so a jump never fills the whole file.
const JUMP: u64 = 16 * 1024;

impl Plan {
    fn steady(jump: bool) -> Self {
        Plan { slices: 40, delay: Duration::from_millis(3), jump, tail: false, stop_at: None, fail: None, start_after: Duration::ZERO, start_when: None }
    }
}

/// The writer thread and what it saw.
struct Writer {
    feed: Feed,
    path: PathBuf,
    len: u64,
    /// Ends the thread (a stall too) when the test is done with it.
    quit: Arc<AtomicBool>,
    /// A stall fails with this.
    fail_now: Arc<Mutex<Option<String>>>,
    /// The offsets `Feed::wanted` said, with how far the file was from its start then.
    wanted: Arc<Mutex<Vec<(u64, u64)>>>,
    /// Stalled (`Plan::stop_at`).
    stalled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    fn start(src: &Path, plan: Plan) -> Self {
        let data = std::fs::read(src).unwrap();
        let len = data.len() as u64;
        let path = temp(src.file_name().unwrap().to_str().unwrap());
        let mut f = File::create(&path).unwrap();
        let feed = Feed::new();
        // As a downloader would: the length (Content-Length) before the first byte.
        feed.set_length(Some(len));
        let (quit, fail_now, wanted) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None::<String>)), Arc::new(Mutex::new(Vec::<(u64, u64)>::new())));
        let stalled = Arc::new(AtomicBool::new(false));
        let thread = {
            let (feed, quit, fail_now, wanted, stalled) = (feed.clone(), quit.clone(), fail_now.clone(), wanted.clone(), stalled.clone());
            std::thread::spawn(move || {
                let slice = len.div_ceil(plan.slices);
                let mut prefix = 0u64;
                let mut put = |at: u64, end: u64| {
                    f.seek(SeekFrom::Start(at)).unwrap();
                    f.write_all(&data[at as usize..end as usize]).unwrap();
                };
                std::thread::sleep(plan.start_after);
                if let Some(when) = &plan.start_when {
                    while !when() && !quit.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                if plan.tail {
                    let at = len.saturating_sub(JUMP);
                    put(at, len);
                    feed.fill(at..len);
                }
                loop {
                    if quit.load(Ordering::Acquire) {
                        return;
                    }
                    let w = feed.wanted();
                    if let Some(w) = w {
                        let mut seen = wanted.lock().unwrap();
                        if seen.last().is_none_or(|l| l.0 != w) {
                            seen.push((w, prefix));
                        }
                    }
                    let stall = plan.stop_at.is_some_and(|s| prefix >= (len as f64 * s) as u64);
                    stalled.store(stall, Ordering::Release);
                    if stall {
                        if let Some(m) = plan.fail.map(str::to_owned).or_else(|| fail_now.lock().unwrap().take()) {
                            feed.fail(m);
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    if plan.jump
                        && let Some(w) = w.filter(|&w| w > prefix + slice && w < len)
                    {
                        let end = (w + JUMP).min(len);
                        put(w, end);
                        feed.fill(w..end);
                    } else {
                        let end = (prefix + slice).min(len);
                        put(prefix, end);
                        prefix = end;
                        feed.advance(end);
                        if prefix == len {
                            feed.complete();
                            return;
                        }
                    }
                    std::thread::sleep(plan.delay);
                }
            })
        };
        Writer { feed, path, len, quit, fail_now, wanted, stalled, thread: Some(thread) }
    }

    fn request(&self, token: u64) -> TrackRequest {
        TrackRequest::growing(token, self.path.clone(), self.feed.clone())
    }

    fn fail(&self, message: &str) {
        *self.fail_now.lock().unwrap() = Some(message.to_owned());
    }

    /// Wait until the writer has stalled and the engine has played all it had and waits.
    fn wait_stalled(&self, run: &Run) {
        within(Duration::from_secs(20), "the writer stalls", || self.stalled.load(Ordering::Acquire));
        let mut frames = usize::MAX;
        within(Duration::from_secs(20), "the engine waits", || {
            std::thread::sleep(Duration::from_millis(50));
            let now = run.cap.frames();
            let still = now == frames && run.p().status().buffering;
            frames = now;
            still
        });
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The engine on a capture device, and every event it sent, with when.
struct Run {
    player: Option<Player>,
    events: Receiver<Event>,
    cap: Capture,
    log: Vec<(Instant, Event)>,
}

impl Run {
    fn new(cap: Capture) -> Self {
        let dev = cap.clone();
        let (player, events) = Player::with_device(move || Box::new(dev));
        player.set_gain_config(common::off());
        Run { player: Some(player), events, cap, log: Vec::new() }
    }

    fn p(&self) -> &Player {
        self.player.as_ref().unwrap()
    }

    /// Take events until one matches, failing after `timeout`.
    fn until(&mut self, timeout: Duration, mut pred: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) => {
                    self.log.push((Instant::now(), e.clone()));
                    if pred(&e) {
                        return e;
                    }
                }
                Err(_) => panic!("timed out; events so far: {:?}", self.log.iter().map(|l| &l.1).collect::<Vec<_>>()),
            }
        }
    }

    fn until_ended(&mut self) {
        self.until(Duration::from_secs(60), |e| matches!(e, Event::Ended));
    }

    /// Take what is there without waiting.
    fn drain(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.log.push((Instant::now(), e));
        }
    }

    fn errors(&self) -> Vec<(u64, String)> {
        self.log.iter().filter_map(|(_, e)| if let Event::Error { token, message } = e { Some((*token, message.clone())) } else { None }).collect()
    }

    /// The `Buffering` events: on and off, in pairs, each pair for one token. Returns the pairs.
    fn buffering_pairs(&self) -> Vec<u64> {
        let mut pairs = Vec::new();
        let mut open: Option<u64> = None;
        for (_, e) in &self.log {
            if let Event::Buffering { token, on } = e {
                match (open, on) {
                    (None, true) => open = Some(*token),
                    (Some(t), false) if t == *token => {
                        pairs.push(t);
                        open = None;
                    }
                    _ => panic!("buffering out of pairs: {:?}", self.log.iter().map(|l| &l.1).collect::<Vec<_>>()),
                }
            }
        }
        assert_eq!(open, None, "a buffering on without its off");
        pairs
    }

    /// Drop the engine (joining its threads), and say how long that took.
    fn close(&mut self) -> Duration {
        let at = Instant::now();
        drop(self.player.take());
        at.elapsed()
    }
}

/// Poll `cond` until it holds; how long it took. Fails after `limit`.
fn within(limit: Duration, what: &str, mut cond: impl FnMut() -> bool) -> Duration {
    let at = Instant::now();
    while !cond() {
        assert!(at.elapsed() < limit, "{what}: not within {limit:?}");
        std::thread::sleep(Duration::from_micros(200));
    }
    at.elapsed()
}

/// One track played to its end on an unpaced capture: what was written.
fn play_through(req: TrackRequest) -> (Vec<f32>, Run) {
    let mut run = Run::new(Capture::default());
    run.p().play(req);
    run.until_ended();
    run.drain();
    (run.cap.take(), run)
}

/// The first frame at which two captures differ, if they do.
fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x.to_bits() != y.to_bits()).map(|i| i / 2).or((a.len() != b.len()).then(|| a.len().min(b.len()) / 2))
}

#[test]
fn every_format_plays_as_it_grows_sample_for_sample() {
    let Some(fx) = fixtures() else { return };
    for (name, path) in &fx.files {
        let (whole, _) = play_through(TrackRequest::new(0, path.clone()));
        assert!(whole.len() > RATE * 2 * 3, "{name}: the complete file plays");
        // Ranges as wanted; the tail first then from the start; from the start only.
        for (jump, tail) in [(true, false), (false, true), (false, false)] {
            let w = Writer::start(path, Plan { tail, ..Plan::steady(jump) });
            let still_growing_at_play = !w.feed.is_complete();
            let started = Instant::now();
            let (grown, run) = play_through(w.request(0));
            let first_audio = run.log.iter().find(|(_, e)| matches!(e, Event::TrackStarted { .. })).map(|l| l.0 - started);
            assert!(run.errors().is_empty(), "{name}: {:?}", run.errors());
            assert_eq!(first_difference(&grown, &whole), None, "{name} (jump {jump}, tail {tail}): differs from the complete file");
            // Whether (and how often) it waits depends on how fast this machine decodes against
            // the writer: printed, not asserted. That it played a growing file is.
            let pairs = run.buffering_pairs();
            assert!(pairs.iter().all(|&t| t == 0), "{name}: {pairs:?}");
            assert!(still_growing_at_play, "{name}: the file was still being written when played");
            assert_eq!(w.feed.readers(), 0, "{name}: let go at the end");
            let wanted = w.wanted.lock().unwrap().clone();
            eprintln!(
                "{name} (jump {jump}, tail {tail}): {} bytes, first audio after {first_audio:?}, {} waits, wanted (offset, start part) {:?}",
                w.len,
                pairs.len(),
                &wanted[..wanted.len().min(4)]
            );
            if *name == "moov-end.m4a" && !tail {
                // The `moov` box is after the audio: the open waits for it, and says so.
                assert!(wanted.iter().any(|&(o, p)| o > p && o > w.len / 2), "{name}: asks for the end first: {wanted:?}");
            }
        }
    }
}

#[test]
fn two_growing_files_in_a_row_stay_gapless() {
    let Some(fx) = fixtures() else { return };
    // Paced at ~10x real time: the second file (written within ~0.1 s) is there before the
    // first (~0.3 s at that pace) ends.
    let mut run = Run::new(Capture::paced());
    let wa = Writer::start(&fx.a, Plan { slices: 20, delay: Duration::from_millis(4), ..Plan::steady(true) });
    let wb = Writer::start(&fx.b, Plan { slices: 20, delay: Duration::from_millis(4), ..Plan::steady(true) });
    run.p().play(wa.request(0));
    run.p().set_next(Some(wb.request(1)));
    assert!(!wb.feed.is_complete(), "the second file is still being written when it is set next");
    run.until_ended();
    run.drain();
    let got = run.cap.take();
    assert!(run.errors().is_empty(), "{:?}", run.errors());
    assert_eq!(first_difference(&got, &fx.reference), None, "the two pieces make the signal, bit for bit");
    let pairs = run.buffering_pairs();
    assert!(!pairs.contains(&1), "the second track was there in time: {pairs:?}");
}

#[test]
fn a_crossfade_into_a_track_not_there_in_time_does_not_happen() {
    let Some(fx) = fixtures() else { return };
    // The second track's first byte comes once the device has all of the first (`CUT` frames):
    // too late for a crossfade over its last 2 s, however fast or slow the machine plays.
    let mut run = Run::new(Capture::paced());
    let cap = run.cap.clone();
    let wb = Writer::start(&fx.b, Plan { start_when: Some(Arc::new(move || cap.frames() >= CUT)), ..Plan::steady(true) });
    run.p().play(TrackRequest::new(0, fx.a.clone()));
    run.p().set_next_with(Some(wb.request(1)), Transition::Crossfade { secs: 2.0 });
    run.until_ended();
    run.drain();
    assert!(run.errors().is_empty(), "{:?}", run.errors());
    // No overlap and no fade: the first track whole, then (after the wait) the second.
    assert_eq!(first_difference(&run.cap.take(), &fx.reference), None);
    assert_eq!(run.buffering_pairs(), vec![1], "the wait is for the second track");
}

#[test]
fn a_stalled_download_waits_in_silence_and_commands_stay_prompt() {
    let Some(fx) = fixtures() else { return };
    let flac = &fx.files[0].1;
    let mut run = Run::new(Capture::default());
    let w = Writer::start(flac, Plan { stop_at: Some(0.4), ..Plan::steady(true) });
    run.p().play(w.request(0));
    run.until(Duration::from_secs(20), |e| matches!(e, Event::TrackStarted { token: 0 }));
    w.wait_stalled(&run);
    run.drain();
    // Waiting: the position holds, nothing is written, and what the container says is known.
    let st = run.p().status();
    assert!(st.buffering && st.state == PlayState::Playing && st.token == Some(0), "{st:?}");
    assert!(st.duration.is_some_and(|d| (d - TOTAL as f64 / RATE as f64).abs() < 0.01), "{st:?}");
    let frames = run.cap.frames();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(run.p().status().position, st.position, "the position holds");
    assert_eq!(run.cap.frames(), frames, "nothing is written while waiting");
    assert!(run.p().stream_info(0).is_some_and(|i| i.codec == Some("FLAC") && i.sample_rate == RATE as u32));
    assert!(w.feed.wanted().is_some(), "the writer is told what is waited for");

    let pause = {
        run.p().set_paused(true);
        within(prompt(), "pause", || run.p().status().state == PlayState::Paused)
    };
    run.p().set_paused(false);
    run.p().set_volume(0.5);
    let next = {
        run.p().set_next(Some(TrackRequest::new(7, fx.b.clone())));
        within(prompt(), "set_next", || run.p().stream_info(7).is_some())
    };
    within(prompt(), "resumed while waiting", || run.p().status().state == PlayState::Playing);
    // A seek back: taken at once (its reader waits elsewhere now: FLAC's seek bisects over the
    // whole file, `stream.rs`), and waiting on, as the download is stalled.
    let before = w.feed.wanted();
    let seek = {
        run.p().seek(0.5);
        within(prompt(), "seek", || w.feed.wanted().is_some_and(|o| Some(o) != before))
    };
    assert!(run.p().status().buffering);
    // Then a stop: the wait ends, and the reader is let go.
    let stop = {
        run.p().stop();
        within(prompt(), "stop", || run.p().status().state == PlayState::Stopped && run.p().status().token.is_none())
    };
    run.until(prompt(), |e| matches!(e, Event::Buffering { token: 0, on: false }));
    let released = within(prompt(), "the reader let go", || w.feed.readers() == 0);
    assert_eq!(w.feed.wanted(), None);
    let closed = run.close();
    eprintln!("while waiting: pause {pause:?}, set_next {next:?}, seek {seek:?}, stop {stop:?}, reader released {released:?}, engine joined {closed:?}");
    assert!(closed < Duration::from_secs(2), "the engine's threads join: {closed:?}");
    run.drain();
    run.buffering_pairs();
    assert!(run.errors().is_empty(), "{:?}", run.errors());
}

#[test]
fn skipping_or_changing_the_next_track_lets_go_of_the_file() {
    let Some(fx) = fixtures() else { return };
    let flac = &fx.files[0].1;
    let mut run = Run::new(Capture::paced());
    // Skipped while it waits.
    let w = Writer::start(flac, Plan { stop_at: Some(0.3), ..Plan::steady(true) });
    run.p().play(w.request(0));
    run.until(Duration::from_secs(20), |e| matches!(e, Event::TrackStarted { token: 0 }));
    w.wait_stalled(&run);
    assert_eq!(w.feed.readers(), 1);
    let at = Instant::now();
    run.p().play(TrackRequest::new(1, fx.a.clone()));
    let skip = within(prompt(), "skip", || w.feed.readers() == 0);
    run.until(prompt(), |e| matches!(e, Event::Buffering { token: 0, on: false }));
    run.until(prompt(), |e| matches!(e, Event::TrackStarted { token: 1 }));
    let started = at.elapsed();
    // A next track whose file has not even its first bytes: opening, then dropped.
    let w2 = Writer::start(flac, Plan { stop_at: Some(0.0), ..Plan::steady(true) });
    run.p().set_next(Some(w2.request(2)));
    within(Duration::from_secs(2), "the next track opening", || w2.feed.readers() == 1 && w2.feed.wanted().is_some());
    run.p().set_next(None);
    let dropped = within(prompt(), "set_next(None)", || w2.feed.readers() == 0);
    // And one replaced by another.
    run.p().set_next(Some(w2.request(3)));
    within(Duration::from_secs(2), "the next track opening", || w2.feed.readers() == 1);
    run.p().set_next(Some(TrackRequest::new(4, fx.b.clone())));
    let replaced = within(prompt(), "set_next(other)", || w2.feed.readers() == 0);
    let closed = run.close();
    eprintln!("released on skip {skip:?}, the new track heard {started:?}, next dropped {dropped:?}, next replaced {replaced:?}, engine joined {closed:?}");
    run.drain();
    run.buffering_pairs();
}

#[test]
fn a_download_that_fails_before_its_first_byte_is_the_tracks_error() {
    let Some(fx) = fixtures() else { return };
    let w = Writer::start(&fx.files[0].1, Plan { stop_at: Some(0.0), fail: Some("connection refused"), ..Plan::steady(true) });
    let (got, run) = play_through(w.request(0));
    assert!(got.is_empty());
    let errors = run.errors();
    assert!(errors.len() == 1 && errors[0].0 == 0 && errors[0].1.contains("connection refused"), "{errors:?}");
    run.buffering_pairs();
    assert_eq!(w.feed.readers(), 0);
}

#[test]
fn a_download_that_fails_midway_plays_what_came_then_errs() {
    let Some(fx) = fixtures() else { return };
    let (name, path) = (&fx.files[0].0, &fx.files[0].1);
    let (whole, _) = play_through(TrackRequest::new(0, path.clone()));
    let w = Writer::start(path, Plan { stop_at: Some(0.5), fail: Some("connection reset"), ..Plan::steady(true) });
    let (got, run) = play_through(w.request(0));
    let errors = run.errors();
    assert!(errors.len() == 1 && errors[0].0 == 0 && errors[0].1.contains("connection reset"), "{name}: {errors:?}");
    // Everything decodable from the half on disk played, exactly as from the whole file.
    assert!(got.len() > whole.len() * 2 / 5 && got.len() < whole.len() * 3 / 5, "{name}: {} of {}", got.len(), whole.len());
    assert_eq!(first_difference(&got, &whole[..got.len()]), None);
    run.buffering_pairs();
    // The error comes before the end.
    let at = |f: &dyn Fn(&Event) -> bool| run.log.iter().position(|(_, e)| f(e)).unwrap();
    assert!(at(&|e| matches!(e, Event::Error { .. })) < at(&|e| matches!(e, Event::Ended)));
    assert_eq!(w.feed.readers(), 0);
}

#[test]
fn a_download_that_fails_while_a_seek_waits_is_the_tracks_error() {
    let Some(fx) = fixtures() else { return };
    let mut run = Run::new(Capture::default());
    let w = Writer::start(&fx.files[0].1, Plan { stop_at: Some(0.3), tail: true, ..Plan::steady(false) });
    run.p().play(w.request(0));
    run.until(Duration::from_secs(20), |e| matches!(e, Event::TrackStarted { token: 0 }));
    w.wait_stalled(&run);
    run.p().seek(3.0);
    // The seek's own wait: a new pair.
    run.until(prompt(), |e| matches!(e, Event::Buffering { token: 0, on: false }));
    run.until(Duration::from_secs(5), |e| matches!(e, Event::Buffering { token: 0, on: true }));
    within(Duration::from_secs(2), "the seek's offset asked for", || w.feed.wanted().is_some_and(|o| o > w.len / 2));
    w.fail("timed out");
    let e = run.until(Duration::from_secs(5), |e| matches!(e, Event::Error { .. }));
    assert!(matches!(&e, Event::Error { token: 0, message } if message.contains("timed out")), "{e:?}");
    run.until_ended();
    run.drain();
    run.buffering_pairs();
    within(prompt(), "the reader let go", || w.feed.readers() == 0);
}

#[test]
fn a_seek_past_the_download_waits_for_it_and_says_where() {
    let Some(fx) = fixtures() else { return };
    let path = &fx.files[0].1;
    // The rest of the track from the seek, as the complete file plays it.
    let to = 3.0;
    let expected = {
        let mut s = Source::open(path).unwrap();
        s.seek((to * RATE as f64) as u64).unwrap();
        let mut v = Vec::new();
        while s.next_chunk(&mut v).unwrap() {}
        v
    };
    for jump in [false, true] {
        let mut run = Run::new(Capture::default());
        let w = Writer::start(path, Plan { slices: 60, delay: Duration::from_millis(8), tail: true, ..Plan::steady(jump) });
        run.p().play(w.request(0));
        run.until(Duration::from_secs(20), |e| matches!(e, Event::TrackStarted { token: 0 }));
        run.until(Duration::from_secs(20), |e| matches!(e, Event::Buffering { token: 0, on: true }));
        let ahead = w.wanted.lock().unwrap().len();
        run.p().seek(to);
        run.until_ended();
        run.drain();
        let got = run.cap.take();
        assert!(got.len() >= expected.len());
        assert_eq!(first_difference(&got[got.len() - expected.len()..], &expected), None, "jump {jump}: the audio after the seek");
        let all = w.wanted.lock().unwrap().clone();
        let wanted = all[ahead.min(all.len())..].to_vec();
        assert!(wanted.iter().any(|&(o, p)| o > p + w.len / 4), "jump {jump}: the writer was told where the seek waits: {wanted:?}");
        run.buffering_pairs();
        assert!(run.errors().is_empty(), "{:?}", run.errors());
        eprintln!("jump {jump}: wanted after the seek {:?}", &wanted[..wanted.len().min(3)]);
    }
}
