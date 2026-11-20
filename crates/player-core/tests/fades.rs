//! Fades on pause, resume, seek and stop, and an output thread that does nothing while
//! paused or stopped, through the real engine with a capture device that asks for
//! fades (as a real device does) and paces itself at ~10x real time.
//!
//! What must hold: gapless boundaries and a track played from its start are never
//! touched (the capture is bit-exact); a seek's new audio fades in over `FADE_MS`; on
//! a cut, the device's unplayed audio is replaced by its first `FADE_MS`, faded out; a
//! pause fades out and the resume fades back in, losing nothing; and while paused or
//! stopped the output never touches the device. Needs ffmpeg on PATH (skips without).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use player_core::output::{Device, FADE_MS};
use player_core::{Event, GainConfig, GainMode, Player, TrackRequest};

const RATE: usize = 48_000;
const CUT: usize = 101_111;
const TOTAL: usize = 4 * RATE + 33;
const FADE: usize = RATE * FADE_MS as usize / 1000;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Two FLAC pieces of one continuous signal, and the signal itself.
fn fixtures() -> Option<(Vec<PathBuf>, Vec<f32>)> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fade-fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("reference.wav");
    if !wav.exists() {
        let expr = "0.4*sin(2*PI*440*t)+0.2*sin(2*PI*3000*t)|0.4*sin(2*PI*550*t)+0.2*sin(2*PI*4000*t)";
        let src = format!("aevalsrc={expr}:s=48000:d={}", TOTAL as f64 / RATE as f64);
        if !ffmpeg(&["-f", "lavfi", "-i", &src, "-c:a", "pcm_s16le", wav.to_str().unwrap()]) {
            eprintln!("ffmpeg not on PATH: skipping");
            return None;
        }
    }
    let mut pieces = Vec::new();
    for (i, (a, b)) in [(0, CUT), (CUT, TOTAL)].into_iter().enumerate() {
        let f = dir.join(format!("piece{i}.flac"));
        if !f.exists() {
            let filter = format!("atrim=start_sample={a}:end_sample={b},asetpts=PTS-STARTPTS");
            assert!(ffmpeg(&["-i", wav.to_str().unwrap(), "-af", &filter, "-c:a", "flac", f.to_str().unwrap()]));
        }
        pieces.push(f);
    }
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&wav)
        .args(["-f", "f32le", "-ac", "2", "-"])
        .output()
        .unwrap();
    let reference: Vec<f32> = out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    assert_eq!(reference.len(), TOTAL * 2);
    Some((pieces, reference))
}

/// Records everything written. Asks for fades; never inserts silence. `queued` is what
/// the test says the device still holds (a flush empties it). Counts every call, so a
/// test can see the output leave it alone.
#[derive(Clone, Default)]
struct FadingCapture {
    samples: Arc<Mutex<Vec<f32>>>,
    queued: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl FadingCapture {
    fn touch(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}

impl Device for FadingCapture {
    fn open(&mut self, _rate: u32) -> Result<(), String> {
        self.touch();
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {
        self.touch();
        // 480 frames a millisecond: ~10x real time.
        std::thread::sleep(Duration::from_millis(1));
    }
    fn available(&mut self) -> Result<usize, String> {
        self.touch();
        Ok(480)
    }
    fn queued(&mut self) -> usize {
        self.touch();
        self.queued.load(Ordering::Relaxed)
    }
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.touch();
        self.samples.lock().unwrap().extend_from_slice(samples);
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        self.touch();
        Ok(())
    }
    fn stop(&mut self) {
        self.touch();
    }
    fn flush(&mut self) {
        self.touch();
        self.queued.store(0, Ordering::Relaxed);
    }
    fn realtime(&self) -> bool {
        false
    }
    fn fades(&self) -> bool {
        true
    }
}

fn curve(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// Play the pieces back to back as the app does; `on_start(player, cap, index)` runs on
/// each TrackStarted. Returns the capture once the queue has ended, and the device
/// calls made in the 300 ms after the end (should be none: the output sleeps).
fn play(files: &[PathBuf], mut on_start: impl FnMut(&Player, &FadingCapture, usize)) -> (Vec<f32>, usize) {
    let cap = FadingCapture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, ..Default::default() });
    let req = |i: usize| TrackRequest::new(i as u64, files[i].clone());
    player.play(req(0));
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::TrackStarted { token }) => {
                let i = token as usize;
                player.set_next((i + 1 < files.len()).then(|| req(i + 1)));
                on_start(&player, &cap, i);
            }
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => panic!("engine error: {message}"),
            Ok(Event::Output(_) | Event::NoOutput(_) | Event::Buffering { .. }) => {}
            Err(_) => assert!(Instant::now() < deadline, "timed out"),
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    let before = cap.calls.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    let idle_calls = cap.calls.load(Ordering::Relaxed) - before;
    drop(player);
    let s = cap.samples.lock().unwrap().clone();
    (s, idle_calls)
}

#[test]
fn fades_leave_gapless_boundaries_and_track_starts_alone() {
    let Some((files, reference)) = fixtures() else { return };
    let (got, idle_calls) = play(&files, |_, _, _| {});
    assert_eq!(got.len(), reference.len());
    assert!(got == reference, "with fades on, playing straight through is still bit-exact");
    assert_eq!(idle_calls, 0, "stopped at the end of the queue, the output leaves the device alone");
}

#[test]
fn a_seek_fades_out_what_was_queued_and_fades_the_new_audio_in() {
    let Some((files, reference)) = fixtures() else { return };
    let seek_secs = 1.0;
    let (got, _) = play(&files, |player, cap, i| {
        if i == 1 {
            // The device "still holds" 10 ms when the seek cuts it.
            cap.queued.store(480, Ordering::Relaxed);
            player.seek(seek_secs);
        }
    });
    let seek_frame = CUT + (seek_secs * RATE as f64) as usize;
    let after = TOTAL - seek_frame;
    let f = got.len() / 2;
    assert!(f > after + FADE + 480);
    // The new audio: the reference from the seek point to the end, the first FADE_MS
    // faded in, the rest exact.
    let new = &got[(f - after) * 2..];
    for i in 0..FADE {
        let g = curve((i + 1) as f32 / FADE as f32);
        for c in 0..2 {
            let want = reference[(seek_frame + i) * 2 + c] * g;
            assert!((new[i * 2 + c] - want).abs() < 1e-6, "fade-in frame {i}: {} vs {want}", new[i * 2 + c]);
        }
    }
    assert!(new[FADE * 2..] == reference[(seek_frame + FADE) * 2..], "after the fade-in, the reference exactly");
    // Just before it: the tail, i.e. the first FADE_MS of the 480 frames the device
    // still held (the last 480 written before the cut), faded out.
    let k = f - after - FADE;
    let held = &got[(k - 480) * 2..(k - 480 + FADE) * 2];
    let tail = &got[k * 2..(k + FADE) * 2];
    for i in 0..FADE {
        let g = curve(1.0 - (i + 1) as f32 / FADE as f32);
        for c in 0..2 {
            assert!((tail[i * 2 + c] - held[i * 2 + c] * g).abs() < 1e-6, "tail frame {i}");
        }
    }
    assert_eq!(tail[(FADE - 1) * 2], 0.0, "the tail ends in silence");
}

#[test]
fn a_pause_fades_out_and_the_resume_fades_in_without_losing_anything() {
    let Some((files, reference)) = fixtures() else { return };
    let paused_calls = Arc::new(AtomicUsize::new(usize::MAX));
    let pc = paused_calls.clone();
    let (got, _) = play(&files, move |player, cap, i| {
        if i == 1 {
            player.set_paused(true);
            std::thread::sleep(Duration::from_millis(100));
            let before = cap.calls.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(300));
            pc.store(cap.calls.load(Ordering::Relaxed) - before, Ordering::Relaxed);
            player.set_paused(false);
        }
    });
    assert_eq!(paused_calls.load(Ordering::Relaxed), 0, "while paused, the output leaves the device alone");
    assert_eq!(got.len(), reference.len(), "nothing lost or repeated across the pause");
    let diff: Vec<usize> = (0..TOTAL).filter(|&i| got[i * 2] != reference[i * 2] || got[i * 2 + 1] != reference[i * 2 + 1]).collect();
    assert!(!diff.is_empty(), "the pause left a fade");
    let (first, last) = (diff[0], *diff.last().unwrap());
    assert!(last - first < 2 * FADE, "only the fade-out and fade-in differ: frames {first}..={last}");
    // Out over FADE frames down to silence, then in over FADE frames.
    let p = (first..=last).find(|&i| got[i * 2] == 0.0 && got[i * 2 + 1] == 0.0).expect("silent at the pause");
    for i in p.saturating_sub(FADE - 1)..(p + FADE).min(TOTAL) {
        for c in 0..2 {
            assert!(got[i * 2 + c].abs() <= reference[i * 2 + c].abs() + 1e-6, "frame {i} is not louder than the source");
        }
    }
}

#[test]
fn a_stop_cuts_with_a_tail_and_the_output_then_sleeps() {
    let Some((files, _)) = fixtures() else { return };
    let cap = FadingCapture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, ..Default::default() });
    player.play(TrackRequest::new(0, files[0].clone()));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::TrackStarted { .. }) => break,
            Ok(e) => panic!("unexpected {e:?}"),
            Err(_) => assert!(Instant::now() < deadline, "timed out"),
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    cap.queued.store(480, Ordering::Relaxed);
    let before_stop = cap.samples.lock().unwrap().len() / 2;
    player.stop();
    // The tail plays, then the device is stopped (after the cut's hold), then nothing.
    std::thread::sleep(Duration::from_millis(400));
    let before = cap.calls.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(cap.calls.load(Ordering::Relaxed) - before, 0, "stopped, the output leaves the device alone");
    let got = cap.samples.lock().unwrap().clone();
    let f = got.len() / 2;
    assert!(f >= before_stop, "stop never removes what was written");
    assert_eq!(got[(f - 1) * 2], 0.0, "the last thing written after a stop is the end of a fade-out");
    drop(player);
}
