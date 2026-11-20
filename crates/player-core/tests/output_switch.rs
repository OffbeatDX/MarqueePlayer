//! Output device changes through the real engine, with a fake device that records what
//! it was asked to do: choosing another device mid-track, the chosen device coming back
//! (a route change), and an exclusive device let go during a long pause. In every case
//! the captured stream must be the track, sample for sample: playback continues at the
//! same position, nothing skipped or repeated.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use player_core::output::Device;
use player_core::source::Source;
use player_core::{Event, GainConfig, GainMode, OutputConfig, OutputInfo, Player, TrackRequest};

fn fixture() -> Option<PathBuf> {
    if !Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("output-switch");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("tone.flac");
    if !f.exists() {
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("aevalsrc=0.3*sin(2*PI*440*t)+0.2*sin(2*PI*3000*t)|0.3*sin(2*PI*660*t):s=48000:d=6")
            .args(["-c:a", "flac"])
            .arg(&f)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    Some(f)
}

fn decode(path: &Path) -> Vec<f32> {
    let mut s = Source::open(path).unwrap();
    let mut v = Vec::new();
    while s.next_chunk(&mut v).unwrap() {}
    v
}

#[derive(Default)]
struct Log {
    samples: Vec<f32>,
    /// (rate, the device configured at the time of the open)
    opens: Vec<(u32, Option<String>)>,
    configured: Vec<OutputConfig>,
    released: usize,
}

/// Offline, slowed to ~100x real time so the test can act mid-track.
#[derive(Clone, Default)]
struct Fake {
    log: Arc<Mutex<Log>>,
    cfg: OutputConfig,
    /// Set by the test: the device "came back" (a route change).
    route: Arc<AtomicBool>,
    open: bool,
    writes: Arc<AtomicUsize>,
    /// Set by the test: no device can be opened (none connected, or it refuses).
    refuse: Arc<AtomicBool>,
}

impl Device for Fake {
    fn open(&mut self, rate: u32) -> Result<(), String> {
        if self.refuse.load(Ordering::Acquire) {
            return Err("no audio output device".into());
        }
        self.log.lock().unwrap().opens.push((rate, self.cfg.device.clone()));
        self.open = true;
        Ok(())
    }
    fn wait(&mut self, _timeout: Duration) {}
    fn available(&mut self) -> Result<usize, String> {
        Ok(480)
    }
    fn queued(&mut self) -> usize {
        0
    }
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.log.lock().unwrap().samples.extend_from_slice(samples);
        self.writes.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(Duration::from_micros(500));
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
    fn route_changed(&mut self) -> bool {
        self.route.swap(false, Ordering::AcqRel)
    }
    fn configure(&mut self, cfg: &OutputConfig) {
        self.cfg = cfg.clone();
        self.log.lock().unwrap().configured.push(cfg.clone());
    }
    fn info(&mut self) -> Option<OutputInfo> {
        let device = self.cfg.device.clone().unwrap_or_else(|| "Default".into());
        let exclusive = self.cfg.exclusive.then(|| "24-bit / 48 kHz".to_owned());
        Some(OutputInfo { id: device.clone(), device, missing: false, exclusive, note: None, ..OutputInfo::default() })
    }
    fn release(&mut self) -> bool {
        if self.cfg.exclusive && self.open {
            self.open = false;
            self.log.lock().unwrap().released += 1;
            return true;
        }
        false
    }
    fn holds_device(&self) -> bool {
        self.cfg.exclusive && self.open
    }
}

struct Run {
    player: Player,
    events: crossbeam_channel::Receiver<Event>,
    fake: Fake,
    infos: Vec<OutputInfo>,
    no_output: Vec<Option<String>>,
}

impl Run {
    fn start(path: &Path) -> Run {
        let fake = Fake::default();
        let dev = fake.clone();
        let (player, events) = Player::with_device(move || Box::new(dev));
        player.set_gain_config(GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() });
        player.play(TrackRequest::new(1, path.to_path_buf()));
        Run { player, events, fake, infos: Vec::new(), no_output: Vec::new() }
    }

    /// Pump events until `f` says stop (or the track ends: then returns false).
    fn until(&mut self, mut f: impl FnMut(&Run) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if f(self) {
                return true;
            }
            match self.events.recv_timeout(Duration::from_millis(5)) {
                Ok(Event::Output(i)) => self.infos.push(i),
                Ok(Event::NoOutput(why)) => self.no_output.push(why),
                Ok(Event::Ended) => return false,
                Ok(Event::Error { message, .. }) => panic!("{message}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "timed out");
        }
    }

    fn written(&self) -> usize {
        self.fake.writes.load(Ordering::Relaxed)
    }

    fn finish(mut self) -> (Log, Vec<OutputInfo>) {
        assert!(!self.until(|_| false), "plays to the end");
        drop(self.player);
        let log = std::mem::take(&mut *self.fake.log.lock().unwrap());
        (log, self.infos)
    }
}

#[test]
fn choosing_another_device_mid_track_continues_at_the_same_sample() {
    let Some(f) = fixture() else { return };
    let want = decode(&f);
    let mut run = Run::start(&f);
    assert!(run.until(|r| r.written() > 100), "started");
    run.player.set_output(OutputConfig { device: Some("Headphones".into()), exclusive: false });
    assert!(run.until(|r| r.written() > 300));
    // The same choice again changes nothing.
    run.player.set_output(OutputConfig { device: Some("Headphones".into()), exclusive: false });
    let (log, infos) = run.finish();
    assert_eq!(log.samples.len(), want.len(), "nothing lost or repeated");
    assert!(log.samples == want, "bit-exact across the switch");
    assert_eq!(log.opens, [(48_000, None), (48_000, Some("Headphones".into()))], "reopened once, on the chosen device");
    assert_eq!(log.configured.len(), 1);
    assert_eq!(infos.iter().map(|i| i.device.as_str()).collect::<Vec<_>>(), ["Default", "Headphones"], "the app hears of both");
}

#[test]
fn a_route_change_reopens_without_losing_samples() {
    let Some(f) = fixture() else { return };
    let want = decode(&f);
    let mut run = Run::start(&f);
    assert!(run.until(|r| r.written() > 150));
    run.fake.route.store(true, Ordering::Release);
    let (log, _) = run.finish();
    assert_eq!(log.opens.len(), 2, "reopened when the route changed");
    assert!(log.samples == want, "bit-exact across the reopen");
}

#[test]
fn an_exclusive_device_is_released_in_a_long_pause_and_reopened_on_resume() {
    let Some(f) = fixture() else { return };
    let want = decode(&f);
    let mut run = Run::start(&f);
    run.player.set_output(OutputConfig { device: None, exclusive: true });
    assert!(run.until(|r| r.written() > 150));
    run.player.set_paused(true);
    let paused = Instant::now();
    assert!(run.until(|r| r.fake.log.lock().unwrap().released == 1 || paused.elapsed() > Duration::from_secs(8)));
    let held = paused.elapsed();
    assert_eq!(run.fake.log.lock().unwrap().released, 1, "let go");
    assert!(held >= player_core::output::RELEASE_AFTER, "not before {:?} ({held:?})", player_core::output::RELEASE_AFTER);
    let opens = run.fake.log.lock().unwrap().opens.len();
    run.player.set_paused(false);
    let (log, infos) = run.finish();
    assert_eq!(log.opens.len(), opens + 1, "opened again on resume");
    assert_eq!(log.released, 2, "and let go again when the track ended");
    assert_eq!(log.samples.len(), want.len(), "the pause lost nothing");
    assert!(log.samples == want, "bit-exact through the pause and reopen");
    assert!(infos.iter().all(|i| i.exclusive.is_some()), "{infos:?}");
}

/// No device can be opened (none connected, or it refuses): the app is told once, nothing is
/// written meanwhile, and when a device opens again it is told that too and the track plays on
/// from where it was, never sitting "playing" in silence without a word.
#[test]
fn no_device_is_said_and_playback_resumes_when_one_opens() {
    let Some(f) = fixture() else { return };
    let fake = Fake::default();
    fake.refuse.store(true, Ordering::Release);
    let dev = fake.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() });
    player.play(TrackRequest::new(1, f.clone()));
    let mut r = Run { player, events, fake, infos: Vec::new(), no_output: Vec::new() };
    assert!(r.until(|r| !r.no_output.is_empty()), "the failure is reported");
    assert!(r.no_output[0].as_deref().is_some_and(|w| w.contains("no audio output device")), "{:?}", r.no_output);
    // A while later: still one report, nothing written, the position where it was.
    let t0 = Instant::now();
    assert!(r.until(|_| t0.elapsed() > Duration::from_millis(1500)));
    assert_eq!(r.no_output.len(), 1, "said once, not every retry");
    assert_eq!(r.fake.writes.load(Ordering::Relaxed), 0);
    assert!(r.player.status().position < 0.1);
    // A device comes: it is said, and the track plays.
    r.fake.refuse.store(false, Ordering::Release);
    assert!(r.until(|r| r.no_output.len() == 2 && r.fake.writes.load(Ordering::Relaxed) > 50), "playback resumes");
    assert_eq!(r.no_output[1], None);
}
