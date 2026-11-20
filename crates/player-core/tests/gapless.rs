//! Gapless and seek verification through the real engine (decode thread, chunk queue,
//! output thread), with a capture device standing in for WASAPI.
//!
//! Fixtures are generated with ffmpeg (must be on PATH; the tests skip otherwise):
//! a continuous two-tone signal is cut at sample positions that do not fall on codec
//! frame boundaries, and each piece is encoded to FLAC and to Opus (libopus). Played
//! back to back, the pieces must reproduce the original signal:
//!
//! - FLAC: bit-exact, sample for sample, across every boundary.
//! - Opus: exactly the original length (pre-skip and end trim honoured), zero
//!   alignment offset at every boundary, and no error spike at a boundary relative to
//!   the codec's normal error level. A single dropped or inserted sample shows up as a
//!   collapse in SNR on the 5 kHz component for the rest of the stream.
//!
//! Optional real-library check: set `MP_REAL_ALBUM` to a folder of Opus files; each
//! track is decoded through `Source` and compared against ffmpeg's libopus decode.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use player_core::output::Device;
use player_core::source::Source;
use player_core::{Event, GainConfig, GainMode, Player, TrackRequest};

const RATE: usize = 48_000;
/// Cut points (frames) for the 4 pieces of a 16 s signal. None is a multiple of 960
/// (Opus 20 ms frame) or 4096 (FLAC block).
const CUTS: [usize; 3] = [173_311, 350_017, 571_003];
const TOTAL: usize = 16 * RATE + 77;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().unwrap();
    assert!(out.status.success(), "ffmpeg {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

/// Decode any file to interleaved stereo f32 with ffmpeg, forcing the libopus decoder
/// for Opus so both sides use the same reference decoder. The synthetic WAV and FLAC
/// fixtures are 16-bit, so their f32 decode is exact.
fn ffmpeg_decode(path: &Path) -> Vec<f32> {
    let is_opus = path.extension().is_some_and(|e| e == "opus");
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if is_opus {
        // Float output: ffmpeg's libopus wrapper defaults to 16-bit, which soft-clips
        // hot masters (this library peaks at +2.8 dBFS after decode).
        cmd.args(["-c:a", "libopus", "-request_sample_fmt", "flt"]);
    }
    let out = cmd.arg("-i").arg(path).args(["-f", "f32le", "-ac", "2", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

struct Fixtures {
    reference: Vec<f32>,
    flac: Vec<PathBuf>,
    opus: Vec<PathBuf>,
}

fn fixtures() -> Option<Fixtures> {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gapless-fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("reference.wav");
    if !wav.exists() {
        // 440 Hz + 5 kHz on the left, 660 Hz + 7 kHz on the right, at moderate level.
        let expr = "0.35*sin(2*PI*440*t)+0.25*sin(2*PI*5000*t)|0.35*sin(2*PI*660*t)+0.25*sin(2*PI*7000*t)";
        let dur = format!("{}", TOTAL as f64 / RATE as f64);
        ffmpeg(&["-f", "lavfi", "-i", &format!("aevalsrc={expr}:s=48000:d={dur}"), "-c:a", "pcm_s16le", wav.to_str().unwrap()]);
    }
    let reference: Vec<f32> = ffmpeg_decode(&wav);
    assert_eq!(reference.len(), TOTAL * 2, "reference length");

    let mut bounds = vec![0];
    bounds.extend(CUTS);
    bounds.push(TOTAL);
    let mut flac = Vec::new();
    let mut opus = Vec::new();
    for (i, w) in bounds.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let filter = format!("atrim=start_sample={a}:end_sample={b},asetpts=PTS-STARTPTS");
        let f = dir.join(format!("piece{i}.flac"));
        let o = dir.join(format!("piece{i}.opus"));
        if !f.exists() {
            ffmpeg(&["-i", wav.to_str().unwrap(), "-af", &filter, "-c:a", "flac", f.to_str().unwrap()]);
        }
        if !o.exists() {
            ffmpeg(&["-i", wav.to_str().unwrap(), "-af", &filter, "-c:a", "libopus", "-b:a", "160k", o.to_str().unwrap()]);
        }
        flac.push(f);
        opus.push(o);
    }
    Some(Fixtures { reference, flac, opus })
}

/// Offline device: records everything written, never inserts silence, and takes audio as
/// fast as it is decoded, up to `until` frames in all: there it takes no more until the test
/// raises the mark (`play_through_engine_with` holds it one frame into each track but the
/// last two, until it has named the track after).
#[derive(Clone)]
struct Capture {
    samples: Arc<Mutex<Vec<f32>>>,
    opens: Arc<Mutex<Vec<u32>>>,
    until: Arc<AtomicUsize>,
}

impl Default for Capture {
    fn default() -> Self {
        Capture { samples: Arc::default(), opens: Arc::default(), until: Arc::new(AtomicUsize::new(usize::MAX)) }
    }
}

impl Capture {
    /// The frames it still takes before its mark.
    fn room(&self) -> usize {
        self.until.load(Ordering::Acquire).saturating_sub(self.samples.lock().unwrap().len() / 2)
    }
}

impl Device for Capture {
    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.opens.lock().unwrap().push(rate);
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {
        // Held at its mark: asleep a moment, not spinning.
        if self.room() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn available(&mut self) -> Result<usize, String> {
        Ok(self.room().min(480))
    }
    fn queued(&mut self) -> usize {
        0
    }
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.samples.lock().unwrap().extend_from_slice(samples);
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

/// Play `files` back to back the way the app does: play the first and name the second at
/// once, then on every later TrackStarted hand the engine the following one. Returns the
/// captured stream.
fn play_through_engine(files: &[PathBuf], seek: Option<(usize, f64)>) -> (Vec<f32>, Vec<u32>) {
    play_through_engine_with(files, seek, GainConfig { mode: GainMode::Off, ..Default::default() })
}

fn play_through_engine_with(files: &[PathBuf], seek: Option<(usize, f64)>, cfg: GainConfig) -> (Vec<f32>, Vec<u32>) {
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(cfg);
    // The engine keeps one "next", named by the test: the second track right after `play`,
    // each later one when the track before it starts. This device takes audio as fast as it
    // is decoded, so on a loaded machine a track could be over before this thread has named
    // the one after it (the queue would end there, which is not what is tested). So the
    // device takes nothing until the second track is named, and then stops one frame into
    // each track until the next one is named: at every track's start while each plays whole
    // (no seek), else at the second's only.
    let whole = seek.is_none();
    let starts: Vec<usize> = if files.len() > 1 {
        let mut at = 0;
        files
            .iter()
            .map(|f| {
                let start = at;
                at += decode_all(f).len() / 2;
                start
            })
            .collect()
    } else {
        Vec::new()
    };
    // One frame into track `k`, where the device waits; none past the track before the last.
    let hold = |k: usize| if k + 1 < files.len() { starts[k] + 1 } else { usize::MAX };
    cap.until.store(0, Ordering::Release);
    let req = |i: usize| TrackRequest::new(i as u64, files[i].clone());
    player.play(req(0));
    // The second track at once, not at the first one's TrackStarted. (Not when the first
    // track is the one to seek in: the seek comes first.)
    let preset = files.len() > 1 && seek.is_none_or(|(at, _)| at != 0);
    if preset {
        player.set_next(Some(req(1)));
    }
    let held_at_second = preset && (whole || seek.is_some_and(|(at, _)| at == 1));
    cap.until.store(if held_at_second { hold(1) } else { usize::MAX }, Ordering::Release);

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seeked = false;
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::TrackStarted { token }) => {
                let i = token as usize;
                if let Some((at, secs)) = seek
                    && at == i && !seeked
                {
                    seeked = true;
                    player.seek(secs);
                }
                if i > 0 || !preset {
                    player.set_next((i + 1 < files.len()).then(|| req(i + 1)));
                }
                if i > 0 {
                    // Named: on to one frame into the track after, or to the end.
                    cap.until.store(if whole { hold(i + 1) } else { usize::MAX }, Ordering::Release);
                }
            }
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => panic!("engine error: {message}"),
            Ok(Event::Output(_) | Event::NoOutput(_) | Event::Buffering { .. }) => {}
            Err(_) => assert!(Instant::now() < deadline, "timed out"),
        }
    }
    drop(player);
    let s = cap.samples.lock().unwrap().clone();
    let o = cap.opens.lock().unwrap().clone();
    (s, o)
}

/// SNR in dB of `got` against `want` over a window.
fn snr(want: &[f32], got: &[f32]) -> f64 {
    let sig: f64 = want.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let err: f64 = want.iter().zip(got).map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2)).sum();
    10.0 * (sig / err.max(1e-20)).log10()
}

/// Lag (in frames, -max..=max) that best aligns `got` to `want` around `center`.
fn best_lag(want: &[f32], got: &[f32], center: usize, max: isize) -> isize {
    let win = 2048usize;
    let start = center.saturating_sub(win / 2);
    let mut best = (f64::MIN, 0);
    for lag in -max..=max {
        let mut acc = 0.0f64;
        for f in start..start + win {
            let g = f as isize + lag;
            if g < 0 || (g as usize) * 2 + 1 >= got.len() || f * 2 + 1 >= want.len() {
                continue;
            }
            let g = g as usize;
            acc += f64::from(want[f * 2]) * f64::from(got[g * 2]) + f64::from(want[f * 2 + 1]) * f64::from(got[g * 2 + 1]);
        }
        if acc > best.0 {
            best = (acc, lag);
        }
    }
    best.1
}

#[test]
fn flac_gapless_is_bit_exact() {
    let Some(fx) = fixtures() else { return };
    let (got, opens) = play_through_engine(&fx.flac, None);
    assert_eq!(opens, vec![48_000], "device must open once (no reopen between tracks)");
    assert_eq!(got.len(), fx.reference.len(), "total length");
    let first_diff = got.iter().zip(&fx.reference).position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "FLAC output differs from the source at sample {first_diff:?}");
}

#[test]
fn opus_gapless_length_alignment_and_boundaries() {
    let Some(fx) = fixtures() else { return };
    let (got, opens) = play_through_engine(&fx.opus, None);
    assert_eq!(opens, vec![48_000]);
    let want = &fx.reference;
    eprintln!("opus: captured {} frames, source {} frames", got.len() / 2, want.len() / 2);
    assert_eq!(got.len(), want.len(), "pre-skip / end trim: total length must match exactly");

    // Baseline codec error: SNR of mid-piece windows.
    let win = 960 * 2;
    let mut bounds = vec![0];
    bounds.extend(CUTS);
    bounds.push(TOTAL);
    let mid_snrs: Vec<f64> = bounds
        .windows(2)
        .map(|w| {
            let m = (w[0] + w[1]) / 2 * 2;
            snr(&want[m..m + win], &got[m..m + win])
        })
        .collect();
    let baseline = mid_snrs.iter().cloned().fold(f64::MAX, f64::min);
    eprintln!("opus: mid-piece SNR {mid_snrs:.1?} dB");

    for &cut in &CUTS {
        let lag = best_lag(want, &got, cut, 8);
        // SNR over 10 ms either side of the cut, and the 20 ms right after it.
        let a = (cut - 480) * 2;
        let around = snr(&want[a..a + win], &got[a..a + win]);
        let after = snr(&want[cut * 2..cut * 2 + win * 2], &got[cut * 2..cut * 2 + win * 2]);
        eprintln!("opus: boundary at {cut}: lag {lag}, SNR around {around:.1} dB, after {after:.1} dB");
        assert_eq!(lag, 0, "boundary at {cut} is misaligned by {lag} frames");
        // A 1-sample slip on a 5-7 kHz tone gives ~10 dB SNR; codec noise at 160 kb/s is
        // far above that. Allow the boundary to be up to 12 dB worse than mid-track
        // (Opus restarts from silence at each piece, so the first ms are a little worse).
        assert!(around > baseline - 12.0 && around > 20.0, "boundary at {cut}: SNR {around:.1} dB vs baseline {baseline:.1}");
    }
    // Whole-stream alignment check at the end too: a slip anywhere accumulates.
    let end = TOTAL - 4096;
    assert_eq!(best_lag(want, &got, end, 8), 0, "end of stream misaligned");
}

#[test]
fn opus_matches_ffmpeg_libopus_decode() {
    let Some(fx) = fixtures() else { return };
    for p in &fx.opus {
        let ours = decode_all(p);
        let theirs = ffmpeg_decode(p);
        assert_eq!(ours.len(), theirs.len(), "{}: frame count differs from ffmpeg", p.display());
        let max = ours.iter().zip(&theirs).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("{}: {} frames, max |diff| vs ffmpeg libopus {max:e}", p.file_name().unwrap().to_string_lossy(), ours.len() / 2);
        assert!(max < 1e-6, "max diff {max}");
    }
}

fn decode_all(p: &Path) -> Vec<f32> {
    let mut src = Source::open(p).unwrap();
    let mut out = Vec::new();
    while src.next_chunk(&mut out).unwrap() {}
    out
}

#[test]
fn seek_is_sample_accurate() {
    let Some(fx) = fixtures() else { return };
    // FLAC: exact.
    let piece = &fx.flac[1];
    let full = decode_all(piece);
    for &frame in &[0usize, 1, 4095, 4096, 77_777, 150_000] {
        let mut src = Source::open(piece).unwrap();
        src.seek(frame as u64).unwrap();
        let mut out = Vec::new();
        while out.len() < 4096 && src.next_chunk(&mut out).unwrap() {}
        let n = out.len().min(4096);
        assert_eq!(&out[..n], &full[frame * 2..frame * 2 + n], "FLAC seek to {frame}");
    }
    // Opus: aligned, and close to a straight decode (preroll lets libopus converge).
    let piece = &fx.opus[2];
    let full = decode_all(piece);
    for &frame in &[0usize, 1, 959, 960, 3840, 100_001, 200_000] {
        let mut src = Source::open(piece).unwrap();
        src.seek(frame as u64).unwrap();
        let mut out = Vec::new();
        while out.len() < 8192 && src.next_chunk(&mut out).unwrap() {}
        let s = snr(&full[frame * 2..frame * 2 + 8192], &out[..8192]);
        let lag = best_lag(&full[frame * 2..], &out, 2048, 8);
        eprintln!("opus seek to {frame}: lag {lag}, SNR vs straight decode {s:.1} dB");
        assert_eq!(lag, 0, "Opus seek to {frame} misaligned");
        assert!(s > 100.0, "Opus seek to {frame}: SNR {s:.1} dB");
    }
}

#[test]
fn seek_through_engine_then_continue_gapless() {
    let Some(fx) = fixtures() else { return };
    // Seek inside piece 1 at 2.0 s, then play on through pieces 2 and 3.
    let (got, _) = play_through_engine(&fx.flac, Some((1, 2.0)));
    let seek_frame = CUTS[0] + 2 * RATE;
    // Whatever was captured of piece 0/1 before the seek is variable; the tail after the
    // seek must be exactly the reference from the seek point to the end.
    let tail_len = (TOTAL - seek_frame) * 2;
    assert!(got.len() >= tail_len);
    let tail = &got[got.len() - tail_len..];
    assert!(tail == &fx.reference[seek_frame * 2..], "post-seek stream is not the reference from the seek point");
}

/// A resumed session: `play_from` starts the track at the offset, and the very first
/// sample the device gets is the one there (the track's start is never written).
#[test]
fn play_from_starts_exactly_at_the_offset() {
    let Some(fx) = fixtures() else { return };
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, ..Default::default() });
    player.play_from(TrackRequest::new(1, fx.flac[0].clone()), 1.5);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => panic!("engine error: {message}"),
            Ok(Event::Output(_) | Event::NoOutput(_) | Event::Buffering { .. }) => {}
            Ok(_) => {}
            Err(_) => assert!(Instant::now() < deadline, "timed out"),
        }
    }
    drop(player);
    let got = cap.samples.lock().unwrap().clone();
    let from = RATE * 3 / 2;
    assert!(got == fx.reference[from * 2..CUTS[0] * 2], "the stream is the reference from 1.5 s to the end of the piece, and nothing before");
}

#[test]
fn sample_rate_change_reopens_device_without_losing_samples() {
    let Some(fx) = fixtures() else { return };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gapless-fixtures");
    let f441 = dir.join("piece0-44k.flac");
    if !f441.exists() {
        ffmpeg(&["-i", fx.flac[0].to_str().unwrap(), "-ar", "44100", "-c:a", "flac", f441.to_str().unwrap()]);
    }
    let files = vec![f441.clone(), fx.opus[1].clone(), fx.flac[2].clone()];
    let (got, opens) = play_through_engine(&files, None);
    assert_eq!(opens, vec![44_100, 48_000], "one reopen at the 44.1 -> 48 kHz boundary, none at 48 -> 48");
    let expect: usize = files.iter().map(|f| decode_all(f).len()).sum();
    assert_eq!(got.len(), expect, "no samples lost or added around the reopen");
}

#[test]
fn real_album_matches_ffmpeg() {
    let Ok(dir) = std::env::var("MP_REAL_ALBUM") else {
        eprintln!("MP_REAL_ALBUM not set: skipping");
        return;
    };
    if !have_ffmpeg() {
        return;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("opus") || e.eq_ignore_ascii_case("flac")))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let mut ours_all = Vec::new();
    let mut theirs_all = Vec::new();
    for p in &files {
        let ours = decode_all(p);
        let theirs = ffmpeg_decode(p);
        let max = ours.iter().zip(&theirs).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("{}: ours {} frames, ffmpeg {} frames, max |diff| {max:e}", p.file_name().unwrap().to_string_lossy(), ours.len() / 2, theirs.len() / 2);
        assert_eq!(ours.len(), theirs.len(), "{}", p.display());
        assert!(max < 1e-6);
        ours_all.extend(ours);
        theirs_all.extend(theirs);
    }
    // Through the engine, back to back: identical to the concatenated reference decodes.
    let (got, opens) = play_through_engine(&files, None);
    eprintln!("engine: {} frames, device opens {opens:?}", got.len() / 2);
    assert_eq!(got.len(), theirs_all.len());
    let max = got.iter().zip(&theirs_all).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    eprintln!("engine vs ffmpeg concatenation: max |diff| {max:e}");
    assert!(max < 1e-6);
}

/// A hot master: a float WAV peaking near +5 dBFS (lavfi sine at 1/8 scale, +26 dB).
/// With the limiter (the default) no sample leaves the engine above the ceiling;
/// without it, the overs go straight to the device.
#[test]
fn hot_masters_are_limited_by_default() {
    if !have_ffmpeg() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("mp-hot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hot = dir.join("hot.wav");
    let hot_s = hot.to_string_lossy().into_owned();
    ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3", "-af", "volume=26dB", "-ac", "2", "-c:a", "pcm_f32le", &hot_s]);
    let peak = |v: &[f32]| v.iter().fold(0f32, |m, x| m.max(x.abs()));
    let off = GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() };
    let (raw, _) = play_through_engine_with(std::slice::from_ref(&hot), None, off);
    assert!(peak(&raw) > 1.5, "the fixture really is hot: {}", peak(&raw));
    let (limited, _) = play_through_engine(std::slice::from_ref(&hot), None);
    assert_eq!(limited.len(), raw.len(), "same length: nothing added or dropped");
    assert!(peak(&limited) <= player_core::limiter::CEILING, "peak {}", peak(&limited));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression: the first chunk after a play (or seek) was sometimes cut to its first
/// write when the output took it before noticing the new generation, so ~4,000 frames
/// (~85 ms) went missing right after the start. It only showed under load, so this
/// plays the Opus pieces from 8 threads at once, 10 times each; every capture must be
/// exactly the files decoded back to back.
#[test]
fn playback_start_keeps_the_first_chunk_whole_under_load() {
    let Some(fx) = fixtures() else { return };
    let mut expect = Vec::new();
    for f in &fx.opus {
        expect.extend(decode_all(f));
    }
    let short = Arc::new(Mutex::new(Vec::new()));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (files, expect, short) = (fx.opus.clone(), expect.clone(), short.clone());
            std::thread::spawn(move || {
                for _ in 0..10 {
                    let (got, _) = play_through_engine(&files, None);
                    if got != expect {
                        short.lock().unwrap().push((got.len() / 2, expect.len() / 2));
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let short = short.lock().unwrap();
    assert!(short.is_empty(), "{} of 80 runs differed (got, want frames): {:?}", short.len(), &short[..short.len().min(5)]);
}

/// Play a queue the way the app does (`playback.rs`): play the first, hand the engine
/// the following item on every TrackStarted, move on to the next item on Ended, and
/// only report an Error. Returns which items started, which failed, and the capture.
fn play_queue_like_the_app(files: &[PathBuf]) -> (Vec<usize>, Vec<usize>, Vec<f32>) {
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, ..Default::default() });
    // Tokens are fresh per request, as the app's are; token -> queue index.
    let tokens = std::cell::RefCell::new(Vec::<usize>::new());
    let req = |i: usize| {
        let mut t = tokens.borrow_mut();
        t.push(i);
        TrackRequest::new(t.len() as u64 - 1, files[i].clone())
    };
    let index = |token: u64| tokens.borrow()[token as usize];
    let mut current = 0;
    // The second item at once, the device taking nothing until it is named, as above.
    cap.until.store(0, Ordering::Release);
    player.play(req(0));
    if files.len() > 1 {
        player.set_next(Some(req(1)));
    }
    cap.until.store(usize::MAX, Ordering::Release);
    let (mut started, mut failed) = (Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::TrackStarted { token }) => {
                current = index(token);
                started.push(current);
                if token > 0 {
                    player.set_next((current + 1 < files.len()).then(|| req(current + 1)));
                }
            }
            Ok(Event::Ended) => {
                if current + 1 < files.len() {
                    current += 1;
                    player.play(req(current));
                } else {
                    break;
                }
            }
            Ok(Event::Error { token, .. }) => failed.push(index(token)),
            Ok(Event::Output(_) | Event::NoOutput(_) | Event::Buffering { .. }) => {}
            Err(_) => assert!(Instant::now() < deadline, "timed out: started {started:?}, failed {failed:?}"),
        }
    }
    drop(player);
    let s = cap.samples.lock().unwrap().clone();
    (started, failed, s)
}

/// An unplayable track is skipped once: the track after it plays, whether the bad one
/// was played directly (it is first, or follows another bad one) or was the gapless
/// next track (pre-opened while the one before played). Before, a failed play sent
/// Error and Ended, the app skipped on both, and the track after the bad one never
/// played; or, when the output thread noticed the new generation late, it lost the
/// Ended and the queue stalled. Repeated, since both depended on thread timing.
#[test]
fn an_unplayable_track_is_skipped_exactly_once() {
    let Some(fx) = fixtures() else { return };
    let bad = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist.flac");
    let (a, b) = (fx.flac[0].clone(), fx.flac[1].clone());
    let len = |files: &[&PathBuf]| files.iter().map(|f| decode_all(f).len()).sum::<usize>();
    // The queue; the items that play; the ones reported unplayable, in order; the frames.
    type Case = (Vec<PathBuf>, Vec<usize>, Vec<usize>, usize);
    let cases: Vec<Case> = vec![
        // Played directly: the first item.
        (vec![bad.clone(), a.clone(), b.clone()], vec![1, 2], vec![0], len(&[&a, &b])),
        // As the gapless next track: reported when set, then again when played.
        (vec![a.clone(), bad.clone(), b.clone()], vec![0, 2], vec![1, 1], len(&[&a, &b])),
        // Two in a row.
        (vec![a.clone(), bad.clone(), bad.clone(), b.clone()], vec![0, 3], vec![1, 1, 2], len(&[&a, &b])),
        // Last in the queue: playback just ends.
        (vec![a.clone(), bad.clone()], vec![0], vec![1, 1], len(&[&a])),
    ];
    for round in 0..5 {
        for (files, want_started, want_failed, want_len) in &cases {
            let (started, failed, got) = play_queue_like_the_app(files);
            assert_eq!(&started, want_started, "round {round}: started");
            assert_eq!(&failed, want_failed, "round {round}: failures reported");
            assert_eq!(got.len(), *want_len, "round {round}: every good track played whole, once");
        }
    }
}
