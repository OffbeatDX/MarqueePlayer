//! Helpers the equalizer and crossfade tests share: capture devices, fixtures, playing a
//! list of files through the real engine the way the app does.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use player_core::output::Device;
use player_core::source::Source;
use player_core::{Event, GainConfig, GainMode, Player, TrackRequest, Transition};

/// How much more than their limit the tests that measure a cost or a delay allow: twice as
/// much on a CI runner (`CI` set), whose shared cores measure about half as fast as a desktop's
/// (the equalizer's 32 bands at 192 kHz: 18 ms a second here, 30 to 36 on GitHub's Windows,
/// Linux and macOS runners) and whose timers run late. Elsewhere the limit as written.
pub fn allowance() -> f64 {
    if std::env::var_os("CI").is_some() { 2.0 } else { 1.0 }
}

pub fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

pub fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().unwrap();
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A folder for one test file's fixtures, in the build's temporary folder.
pub fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 32-bit float stereo WAV (exactly these samples, no codec in the way).
pub fn write_wav(path: &Path, rate: u32, samples: &[f32]) {
    let data_len = samples.len() * 4;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 8).to_le_bytes());
    b.extend_from_slice(&8u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for x in samples {
        b.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

pub fn decode(path: &Path) -> Vec<f32> {
    let mut s = Source::open(path).unwrap();
    let mut v = Vec::new();
    while s.next_chunk(&mut v).unwrap() {}
    v
}

/// Records everything written. `pace`: sleeps 1 ms per wait (480 frames a millisecond,
/// ~10x real time at 48 kHz), so a test can act while a track plays and the decode thread
/// stays its usual second ahead. `fades`: asks for the click-free fades a real device gets.
#[derive(Clone, Default)]
pub struct Capture {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub opens: Arc<Mutex<Vec<u32>>>,
    pub queued: Arc<AtomicUsize>,
    pub pace: bool,
    pub fades: bool,
    /// While set, the output thread waits in `wait` (writes nothing): `held`.
    pub hold: Arc<AtomicBool>,
}

/// Run `f` with the device holding still, and a moment after so the engine can act on it
/// (decode a new track's first chunk); returns the frames written before. A test can then
/// say where in the stream the action lands, however loaded the machine.
pub fn held(cap: &Capture, f: impl FnOnce()) -> usize {
    cap.hold.store(true, Ordering::Release);
    // The output finishes the write it may be in, then stops at its next wait.
    std::thread::sleep(Duration::from_millis(30));
    let at = cap.frames();
    f();
    std::thread::sleep(Duration::from_millis(300));
    cap.hold.store(false, Ordering::Release);
    at
}

impl Capture {
    pub fn paced() -> Self {
        Self { pace: true, ..Self::default() }
    }

    pub fn fading() -> Self {
        Self { pace: true, fades: true, ..Self::default() }
    }

    pub fn frames(&self) -> usize {
        self.samples.lock().unwrap().len() / 2
    }

    pub fn take(&self) -> Vec<f32> {
        self.samples.lock().unwrap().clone()
    }
}

impl Device for Capture {
    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.opens.lock().unwrap().push(rate);
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {
        while self.hold.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.pace {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn available(&mut self) -> Result<usize, String> {
        Ok(480)
    }
    fn queued(&mut self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.samples.lock().unwrap().extend_from_slice(samples);
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self) {}
    /// What the device had not played is gone: the recording is what would be heard.
    fn flush(&mut self) {
        let q = self.queued.swap(0, Ordering::Relaxed);
        let mut s = self.samples.lock().unwrap();
        let keep = s.len().saturating_sub(q * 2);
        s.truncate(keep);
    }
    fn realtime(&self) -> bool {
        false
    }
    fn fades(&self) -> bool {
        self.fades
    }
}

pub fn off() -> GainConfig {
    GainConfig { mode: GainMode::Off, ..Default::default() }
}

/// Play `files` back to back as the app does, the transition before each from `how`
/// (`how(i)` for going from file i to i + 1). `on_start(player, capture, index)` runs at
/// each TrackStarted. Returns the capture once the queue has ended.
pub fn play(
    cap: Capture,
    files: &[PathBuf],
    cfg: GainConfig,
    setup: impl FnOnce(&Player),
    how: impl Fn(usize) -> Transition,
    mut on_start: impl FnMut(&Player, &Capture, usize),
) -> (Vec<f32>, Vec<u32>) {
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(cfg);
    setup(&player);
    let req = |i: usize| TrackRequest::new(i as u64, files[i].clone());
    player.play(req(0));
    // The second track at once, not at the first one's TrackStarted: unpaced, the engine can
    // decode a short first track to its end before that event is even read here.
    player.set_next_with((files.len() > 1).then(|| req(1)), how(0));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(Event::TrackStarted { token }) => {
                let i = token as usize;
                if i > 0 && i < files.len() {
                    player.set_next_with((i + 1 < files.len()).then(|| req(i + 1)), how(i));
                }
                on_start(&player, &cap, i);
            }
            Ok(Event::Ended) => break,
            Ok(Event::Error { message, .. }) => panic!("engine error: {message}"),
            Ok(Event::Output(_) | Event::NoOutput(_) | Event::Buffering { .. }) => {}
            Err(_) => assert!(Instant::now() < deadline, "timed out"),
        }
    }
    drop(player);
    let opens = cap.opens.lock().unwrap().clone();
    (cap.take(), opens)
}

/// The largest sample-to-sample step of either channel over `range` of frames.
pub fn max_step(v: &[f32], range: std::ops::Range<usize>) -> f32 {
    let mut m = 0f32;
    for i in range.start.max(1)..range.end.min(v.len() / 2) {
        for c in 0..2 {
            m = m.max((v[i * 2 + c] - v[(i - 1) * 2 + c]).abs());
        }
    }
    m
}

pub fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0f32, |m, x| m.max(x.abs()))
}
