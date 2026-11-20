//! The less common formats (AIFF and AIFF-C, raw AAC in ADTS, M4B, MP2), each from
//! an ffmpeg-made fixture: played from the start against ffmpeg's own decode, a seek
//! against a straight decode, and a gapless hand-over through the engine where the format
//! carries what gapless needs (PCM: always; MP4: encoder delay and padding). ADTS and MP2
//! carry no delay information, so their boundaries are not checked.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use player_core::output::Device;
use player_core::source::Source;
use player_core::{Event, GainConfig, GainMode, Player, TrackRequest};

const RATE: usize = 44_100;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().unwrap();
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("format-fixtures");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The format's encoder arguments and extension.
const FORMATS: [(&str, &[&str]); 5] = [
    ("aiff", &["-c:a", "pcm_s16be"]),
    ("aifc", &["-f", "aiff", "-c:a", "pcm_s16le"]),
    ("aac", &["-c:a", "aac", "-b:a", "192k", "-f", "adts"]),
    ("m4b", &["-c:a", "aac", "-b:a", "192k", "-f", "ipod"]),
    ("mp2", &["-c:a", "mp2", "-b:a", "256k"]),
];

/// A 6 s two-tone signal, and the same cut into two pieces at a frame that is no codec's
/// frame boundary; each encoded as `ext`.
fn fixture(ext: &str, args: &[&str]) -> (PathBuf, PathBuf, PathBuf) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let d = dir();
    let wav = d.join("ref.wav");
    if !wav.exists() {
        ffmpeg(&["-f", "lavfi", "-i", "aevalsrc=0.3*sin(2*PI*440*t)+0.1*sin(2*PI*3000*t)|0.3*sin(2*PI*660*t):s=44100:d=6", "-c:a", "pcm_s16le", wav.to_str().unwrap()]);
    }
    let whole = d.join(format!("whole.{ext}"));
    let a = d.join(format!("a.{ext}"));
    let b = d.join(format!("b.{ext}"));
    for (out, filter) in [(&whole, None), (&a, Some("atrim=end_sample=131071")), (&b, Some("atrim=start_sample=131071,asetpts=PTS-STARTPTS"))] {
        if out.exists() {
            continue;
        }
        let mut v: Vec<&str> = vec!["-i", wav.to_str().unwrap()];
        if let Some(f) = filter {
            v.extend(["-af", f]);
        }
        v.extend(args);
        v.push(out.to_str().unwrap());
        ffmpeg(&v);
    }
    (whole, a, b)
}

fn decode(p: &Path) -> Vec<f32> {
    let mut s = Source::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    assert_eq!(s.sample_rate, RATE as u32);
    let mut v = Vec::new();
    while s.next_chunk(&mut v).unwrap() {}
    v
}

fn ffmpeg_decode(p: &Path) -> Vec<f32> {
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(p).args(["-f", "f32le", "-ac", "2", "-"]).output().unwrap();
    assert!(out.status.success());
    out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// The MP4 pieces carry exact gapless data: ffmpeg's own decode of each is exactly its
/// part of the 6 s signal.
fn exact_mp4_fixtures(a: &Path, b: &Path) -> bool {
    let (na, nb) = (ffmpeg_decode(a).len() / 2, ffmpeg_decode(b).len() / 2);
    na == 131_071 && nb == 6 * RATE - 131_071
}

fn snr(want: &[f32], got: &[f32]) -> f64 {
    let sig: f64 = want.iter().map(|&x| f64::from(x).powi(2)).sum();
    let err: f64 = want.iter().zip(got).map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2)).sum();
    10.0 * (sig / err.max(1e-20)).log10()
}

/// The lag (frames) that best lines `got` up with `want`, and the SNR there.
fn aligned(want: &[f32], got: &[f32], max: isize) -> (isize, f64) {
    let n = (want.len().min(got.len()) / 2).saturating_sub(max.unsigned_abs() + 1).min(RATE * 2);
    let mut best = (0, f64::MIN);
    for lag in -max..=max {
        let w0 = (RATE / 2) as isize;
        let g0 = w0 + lag;
        if g0 < 0 {
            continue;
        }
        let w = &want[w0 as usize * 2..(w0 as usize + n) * 2];
        let g = &got[g0 as usize * 2..(g0 as usize + n) * 2];
        let s = snr(w, g);
        if s > best.1 {
            best = (lag, s);
        }
    }
    best
}

/// The lag that best aligns `got` with `want` over 2048 frames from `center`.
fn lag_at(want: &[f32], got: &[f32], center: usize, max: isize) -> isize {
    let n = 2048;
    (-max..=max)
        .max_by(|&a, &b| {
            let s = |lag: isize| {
                let g = (center as isize + lag) as usize;
                snr(&want[center * 2..(center + n) * 2], &got[g * 2..(g + n) * 2])
            };
            s(a).total_cmp(&s(b))
        })
        .unwrap()
}

#[test]
fn each_new_format_plays_from_the_start_like_ffmpeg() {
    if !have_ffmpeg() {
        return;
    }
    for (ext, args) in FORMATS {
        let (whole, _, _) = fixture(ext, args);
        let ours = decode(&whole);
        let theirs = ffmpeg_decode(&whole);
        let frames = ours.len() / 2;
        let (lag, s) = aligned(&theirs, &ours, 3000);
        eprintln!("{ext}: {frames} frames (ffmpeg {}), lag {lag}, SNR vs ffmpeg {s:.1} dB", theirs.len() / 2);
        let pcm = ext.starts_with("aif");
        if pcm {
            assert!(ours == theirs, "{ext}: PCM is bit-exact");
        } else {
            assert!(s > 60.0, "{ext}: same decode as ffmpeg (SNR {s:.1} dB)");
        }
        // Length: within one codec frame of the 6 s signal (exact where the container says).
        let want = 6 * RATE;
        let slack = if pcm || ext == "m4b" { 0 } else { 2112 };
        assert!(frames.abs_diff(want) <= slack, "{ext}: {frames} frames, want {want} ±{slack}");
    }
}

#[test]
fn each_new_format_seeks_accurately() {
    if !have_ffmpeg() {
        return;
    }
    for (ext, args) in FORMATS {
        let (whole, _, _) = fixture(ext, args);
        let straight = decode(&whole);
        let mut s = Source::open(&whole).unwrap();
        let at = 3 * RATE as u64 + 777;
        s.seek(at).unwrap();
        let mut v = Vec::new();
        while v.len() < RATE * 2 && s.next_chunk(&mut v).unwrap() {}
        let at = at as usize;
        let want = &straight[at * 2..(at + RATE / 2) * 2];
        let got = &v[..RATE];
        let pcm = ext.starts_with("aif");
        if pcm {
            assert!(got == want, "{ext}: the sample after a seek is the sample asked for");
        } else {
            // A lossy decoder restarts at the seek: its first frame or two differ, then it
            // must be the same audio at the same position (no offset).
            let tail = 8192;
            let s = snr(&want[tail..], &got[tail..]);
            eprintln!("{ext}: after a seek, SNR vs straight decode {s:.1} dB");
            assert!(s > 30.0, "{ext}: seek lands at the asked frame (SNR {s:.1} dB)");
        }
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<f32>>>);

impl Device for Capture {
    fn open(&mut self, _rate: u32) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {}
    fn available(&mut self) -> Result<usize, String> {
        Ok(480)
    }
    fn queued(&mut self) -> usize {
        0
    }
    fn write(&mut self, s: &[f32]) -> Result<(), String> {
        self.0.lock().unwrap().extend_from_slice(s);
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

fn gapless(a: &Path, b: &Path) -> Vec<f32> {
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() });
    player.play(TrackRequest::new(0, a));
    // Named at once, not when the first track says it started: on a device that takes audio
    // as fast as it is decoded, three seconds of PCM can be over before that event is read.
    player.set_next(Some(TrackRequest::new(1, b)));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            _ => assert!(Instant::now() < deadline),
        }
    }
    drop(player);
    cap.0.lock().unwrap().clone()
}

#[test]
fn pcm_and_mp4_formats_are_gapless() {
    if !have_ffmpeg() {
        return;
    }
    for (ext, args) in FORMATS.into_iter().filter(|(e, _)| e.starts_with("aif") || *e == "m4b") {
        let (whole, a, b) = fixture(ext, args);
        let joined = gapless(&a, &b);
        let reference = decode(&whole);
        let frames = joined.len() / 2;
        eprintln!("{ext}: two pieces gapless: {frames} frames (whole file {})", reference.len() / 2);
        if ext.starts_with("aif") {
            assert!(joined == reference, "{ext}: the two pieces play back as the whole, bit-exact");
        } else if !exact_mp4_fixtures(&a, &b) {
            // ffmpeg before 8 writes edit lists one frame off for these cuts (its own decode
            // of the pieces is too long too): such fixtures cannot show sample-exact gapless.
            // ffmpeg 8 and newer write them exactly (Windows' 9.0 here).
            eprintln!("{ext}: skipped: this ffmpeg does not write exact gapless data for the pieces (ffmpeg 8 or newer does)");
        } else {
            assert_eq!(frames, 6 * RATE, "{ext}: encoder delay and padding trimmed, nothing added");
            // Around the cut (131071): no offset, and no more error than elsewhere.
            let cut = 131_071usize;
            let w = 2048;
            let at = snr(&reference[(cut - w) * 2..(cut + w) * 2], &joined[(cut - w) * 2..(cut + w) * 2]);
            let away = snr(&reference[(cut + 20_000) * 2..(cut + 20_000 + 2 * w) * 2], &joined[(cut + 20_000) * 2..(cut + 20_000 + 2 * w) * 2]);
            eprintln!("{ext}: SNR at the boundary {at:.1} dB, away from it {away:.1} dB");
            // Each piece's edge is encoded on its own, so the error rises right at the cut;
            // what must hold is the alignment: no offset just before or just after it.
            for center in [cut - 4096, cut + 4096] {
                let lag = lag_at(&reference, &joined, center, 64);
                eprintln!("{ext}: lag at {center}: {lag}");
                assert_eq!(lag, 0, "{ext}: no offset around the boundary");
            }
            assert!(away > 25.0, "{ext}: the second piece is the same audio (SNR {away:.1} dB)");
        }
    }
}
