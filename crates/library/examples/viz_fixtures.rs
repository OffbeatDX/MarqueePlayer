//! Fixtures for the visualizer bench (`MP_BENCH=visualizer`, `viz-idle`, `viz-styles`):
//! synthetic music, so stills and measurements are the same on every run and nothing of
//! anyone's real library is needed. `-- <empty folder>`. Writes only inside the folder it
//! is given, and refuses a folder that holds anything.
//!
//!     cargo run --release -p library --example viz_fixtures -- %TEMP%\mp-viz\music
//!
//! Three albums, each with its own cover (so the accent and the cover's palette differ):
//!
//! - "Spectrum" (Fixture Signals): the test signals `bench/visualizer.rs` expects, in its order: a log
//!   sweep 20 Hz-20 kHz over 20 s at 48 kHz, three tones (100 Hz at -12 dB, 1 kHz at
//!   -18 dB, 5 kHz at -24 dB), pink noise at 44.1 kHz, a groove that drops 20 dB after
//!   10 s (30 s long), a sweep over 10 s at 96 kHz.
//! - "Night Shift" (Fixture Band): mixes with a spectrum like real music's: house (a kick
//!   on every beat, off-beat hats, a clap, a bass line, chords), hip-hop (a long 808 sub,
//!   a snare, fast hats), and "Dynamics" (a quiet intro, a build, the drop, a breakdown).
//! - "Daylight" (Fixture Band): rock (distorted power chords, kick and snare, a crash),
//!   a ballad (plucked chords and a pad, quiet), ambient (a low drone and sparse bells,
//!   about 30 dB under full scale).
//! - "Paper" and "Graphite" (Fixture Covers): one track each (the house and rock mixes) under a
//!   bright cover (yellow, coral and sky on cream) and a nearly monochrome one (greys with a
//!   cool tint), so the cover's palette is tried light and nearly colourless too (the halo's
//!   stills, `viz-look`).
//!
//! Each is synthesised here (kick: a sine falling from ~160 to ~48 Hz; hats: high-passed
//! noise bursts; pads: detuned saws through a low-pass), mastered to a -1 dBFS peak (the
//! quiet ones lower, on purpose), and encoded to FLAC by ffmpeg (on PATH), tagged.

use std::f32::consts::{PI, TAU};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RATE: u32 = 44_100;

/// Deterministic noise.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// A stereo buffer being mixed.
struct Mix {
    rate: u32,
    l: Vec<f32>,
    r: Vec<f32>,
    rng: Rng,
}

impl Mix {
    fn new(secs: f32, rate: u32) -> Self {
        let n = (secs * rate as f32) as usize;
        Self { rate, l: vec![0.0; n], r: vec![0.0; n], rng: Rng(0x9e37_79b9_7f4a_7c15) }
    }

    fn secs(&self) -> f32 {
        self.l.len() as f32 / self.rate as f32
    }

    /// Add `f(t)` (mono, t from the start of the event) for `dur` seconds from `at`, panned
    /// (-1 left .. 1 right).
    fn add(&mut self, at: f32, dur: f32, pan: f32, mut f: impl FnMut(f32, &mut Rng) -> f32) {
        let rate = self.rate as f32;
        let start = (at * rate) as usize;
        let n = (dur * rate) as usize;
        let (gl, gr) = (((1.0 - pan) * 0.5).sqrt(), ((1.0 + pan) * 0.5).sqrt());
        for i in 0..n {
            let k = start + i;
            if k >= self.l.len() {
                break;
            }
            let v = f(i as f32 / rate, &mut self.rng);
            self.l[k] += v * gl;
            self.r[k] += v * gr;
        }
    }

    /// Scale so the peak is `peak` (linear).
    fn master(&mut self, peak: f32) {
        let m = self.l.iter().chain(&self.r).fold(0f32, |a, &v| a.max(v.abs())).max(1e-9);
        let g = peak / m;
        self.l.iter_mut().chain(self.r.iter_mut()).for_each(|v| *v *= g);
    }

    /// A gain curve over time (dB at t), for arrangement dynamics.
    fn shape(&mut self, db: impl Fn(f32) -> f32) {
        let rate = self.rate as f32;
        for i in 0..self.l.len() {
            let g = 10f32.powf(db(i as f32 / rate) / 20.0);
            self.l[i] *= g;
            self.r[i] *= g;
        }
    }
}

/// One-pole low-pass, as a closure state.
fn lowpass(cut: f32, rate: f32) -> impl FnMut(f32) -> f32 {
    let a = 1.0 - (-TAU * cut / rate).exp();
    let mut y = 0.0;
    move |x| {
        y += a * (x - y);
        y
    }
}

fn highpass(cut: f32, rate: f32) -> impl FnMut(f32) -> f32 {
    let mut lp = lowpass(cut, rate);
    move |x| x - lp(x)
}

fn saw(phase: f32) -> f32 {
    2.0 * (phase - phase.floor()) - 1.0
}

fn note(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

// ---- Instruments ---------------------------------------------------------------------

fn kick(m: &mut Mix, at: f32, gain: f32) {
    let mut phase = 0.0f32;
    let rate = m.rate as f32;
    let mut click = highpass(2000.0, rate);
    m.add(at, 0.45, 0.0, move |t, rng| {
        let f = 48.0 + 115.0 * (-t * 32.0).exp();
        phase += f / rate;
        let body = (TAU * phase).sin() * (-t * 6.5).exp();
        let c = click(rng.next()) * (-t * 300.0).exp() * 0.6;
        (body + c) * gain
    });
}

fn snare(m: &mut Mix, at: f32, gain: f32) {
    let rate = m.rate as f32;
    let (mut hp, mut lp) = (highpass(900.0, rate), lowpass(7000.0, rate));
    m.add(at, 0.3, 0.05, move |t, rng| {
        let tone = (TAU * 185.0 * t).sin() * (-t * 28.0).exp() * 0.5;
        let noise = lp(hp(rng.next())) * (-t * 15.0).exp() * 1.4;
        (tone + noise) * gain
    });
}

fn clap(m: &mut Mix, at: f32, gain: f32) {
    let rate = m.rate as f32;
    let (mut hp, mut lp) = (highpass(1200.0, rate), lowpass(5000.0, rate));
    m.add(at, 0.25, -0.1, move |t, rng| {
        // Three quick bursts, then the tail.
        let bursts = [0.0f32, 0.011, 0.022].iter().map(|&o| if t >= o { (-(t - o) * 180.0).exp() } else { 0.0 }).sum::<f32>();
        let env = bursts * 0.6 + (-t * 18.0).exp() * 0.5;
        lp(hp(rng.next())) * env * gain
    });
}

fn hat(m: &mut Mix, at: f32, gain: f32, open: bool, pan: f32) {
    let rate = m.rate as f32;
    let mut hp = highpass(7000.0, rate);
    let mut hp2 = highpass(5000.0, rate);
    let decay = if open { 7.0 } else { 55.0 };
    m.add(at, if open { 0.5 } else { 0.12 }, pan, move |t, rng| hp2(hp(rng.next())) * (-t * decay).exp() * gain)
}

fn crash(m: &mut Mix, at: f32, gain: f32) {
    let rate = m.rate as f32;
    let mut hp = highpass(3500.0, rate);
    m.add(at, 2.2, 0.3, move |t, rng| hp(rng.next()) * (-t * 1.8).exp() * gain)
}

/// A saw bass through a low-pass that opens on each note.
fn bass(m: &mut Mix, at: f32, dur: f32, midi: f32, gain: f32) {
    let rate = m.rate as f32;
    let f = note(midi);
    let mut phase = 0.0f32;
    let mut y = 0.0f32;
    m.add(at, dur, 0.0, move |t, _| {
        phase += f / rate;
        let cut = 180.0 + 900.0 * (-t * 9.0).exp();
        let a = 1.0 - (-TAU * cut / rate).exp();
        y += a * (saw(phase) - y);
        let env = (t * 200.0).min(1.0) * if t > dur - 0.02 { ((dur - t) / 0.02).max(0.0) } else { 1.0 };
        (y + 0.4 * (TAU * phase).sin()) * env * gain
    });
}

/// An 808: a long sine with a slight drop, softly saturated.
fn sub808(m: &mut Mix, at: f32, dur: f32, midi: f32, gain: f32) {
    let rate = m.rate as f32;
    let f = note(midi);
    let mut phase = 0.0f32;
    m.add(at, dur, 0.0, move |t, _| {
        phase += f * (1.0 + 0.6 * (-t * 25.0).exp()) / rate;
        let env = (t * 300.0).min(1.0) * (-t * 1.2).exp() * if t > dur - 0.03 { ((dur - t) / 0.03).max(0.0) } else { 1.0 };
        ((TAU * phase).sin() * 1.6).tanh() * env * gain
    });
}

/// A chord of detuned saws through a low-pass, slow in and out.
fn pad(m: &mut Mix, at: f32, dur: f32, chord: &[f32], gain: f32, cut: f32) {
    let rate = m.rate as f32;
    for (k, &midi) in chord.iter().enumerate() {
        for (d, pan) in [(-0.09f32, -0.6f32), (0.0, 0.0), (0.08, 0.6)] {
            let f = note(midi + d);
            let mut phase = (k as f32 * 0.37 + d).fract();
            let mut lp = lowpass(cut, rate);
            let mut lp2 = lowpass(cut * 1.5, rate);
            m.add(at, dur, pan, move |t, _| {
                phase += f / rate;
                let env = (t / 0.25).min(1.0) * ((dur - t) / 0.4).clamp(0.0, 1.0);
                lp2(lp(saw(phase))) * env * gain / chord.len() as f32
            });
        }
    }
}

/// A plucked string or piano-ish note: harmonics, the higher ones dying faster.
fn pluck(m: &mut Mix, at: f32, midi: f32, gain: f32, pan: f32) {
    let f = note(midi);
    m.add(at, 2.5, pan, move |t, _| {
        let mut v = 0.0;
        for h in 1..=10 {
            let hf = h as f32;
            if f * hf > 18_000.0 {
                break;
            }
            v += (TAU * f * hf * t).sin() * (-t * (1.5 + hf * 1.1)).exp() / hf;
        }
        v * (t * 400.0).min(1.0) * gain
    });
}

fn bell(m: &mut Mix, at: f32, midi: f32, gain: f32, pan: f32) {
    let f = note(midi);
    m.add(at, 5.0, pan, move |t, _| {
        let v = (TAU * f * t).sin() + 0.5 * (TAU * f * 2.76 * t).sin() * (-t * 1.5).exp() + 0.3 * (TAU * f * 5.4 * t).sin() * (-t * 3.0).exp();
        v * (-t * 0.8).exp() * (t * 500.0).min(1.0) * gain
    });
}

/// A distorted power chord (root, fifth, octave) for `dur`.
fn guitar(m: &mut Mix, at: f32, dur: f32, root: f32, gain: f32, pan: f32) {
    let rate = m.rate as f32;
    let fs = [note(root), note(root + 7.0), note(root + 12.0)];
    let mut ph = [0.0f32; 3];
    let mut lp = lowpass(3200.0, rate);
    let mut hp = highpass(90.0, rate);
    m.add(at, dur, pan, move |t, _| {
        let mut v = 0.0;
        for k in 0..3 {
            ph[k] += fs[k] * (1.0 + 0.002 * k as f32) / rate;
            v += saw(ph[k]);
        }
        let env = (t * 150.0).min(1.0) * ((dur - t) / 0.03).clamp(0.0, 1.0) * (0.75 + 0.25 * (-t * 3.0).exp());
        lp(hp((v * 3.0).tanh())) * env * gain
    });
}

// ---- Songs ---------------------------------------------------------------------------

fn house() -> Mix {
    let bpm = 124.0;
    let beat = 60.0 / bpm;
    let mut m = Mix::new(32.0, RATE);
    let bars = (m.secs() / (beat * 4.0)) as usize;
    let chords: [[f32; 4]; 4] = [[57.0, 60.0, 64.0, 67.0], [53.0, 57.0, 60.0, 64.0], [55.0, 59.0, 62.0, 66.0], [52.0, 55.0, 59.0, 62.0]];
    let roots = [33.0f32, 29.0, 31.0, 28.0];
    for b in 0..bars {
        let t0 = b as f32 * beat * 4.0;
        let c = b % 4;
        pad(&mut m, t0, beat * 4.0, &chords[c], 0.22, 1800.0);
        for q in 0..4 {
            let t = t0 + q as f32 * beat;
            kick(&mut m, t, 0.95);
            hat(&mut m, t + beat * 0.5, 0.16, q == 3, 0.35);
            hat(&mut m, t + beat * 0.25, 0.05, false, -0.3);
            hat(&mut m, t + beat * 0.75, 0.05, false, -0.3);
            if q % 2 == 1 {
                clap(&mut m, t, 0.45);
            }
            // Off-beat bass.
            bass(&mut m, t + beat * 0.5, beat * 0.45, roots[c] + if q == 2 { 12.0 } else { 0.0 }, 0.42);
        }
    }
    m.master(0.89);
    m
}

fn hiphop() -> Mix {
    let bpm = 88.0;
    let beat = 60.0 / bpm;
    let mut m = Mix::new(30.0, RATE);
    let bars = (m.secs() / (beat * 4.0)) as usize;
    for b in 0..bars {
        let t0 = b as f32 * beat * 4.0;
        kick(&mut m, t0, 0.8);
        kick(&mut m, t0 + beat * 2.5, 0.7);
        sub808(&mut m, t0, beat * 2.4, 31.0, 0.9);
        sub808(&mut m, t0 + beat * 2.5, beat * 1.4, if b % 2 == 0 { 34.0 } else { 29.0 }, 0.8);
        snare(&mut m, t0 + beat, 0.55);
        snare(&mut m, t0 + beat * 3.0, 0.55);
        for s in 0..16 {
            let roll = b % 4 == 3 && s >= 12;
            hat(&mut m, t0 + s as f32 * beat / 4.0, if s % 2 == 0 { 0.13 } else { 0.07 }, false, 0.2);
            if roll {
                hat(&mut m, t0 + (s as f32 + 0.5) * beat / 4.0, 0.06, false, 0.2);
            }
        }
        for (k, n) in [72.0f32, 75.0, 79.0].iter().enumerate() {
            pluck(&mut m, t0 + k as f32 * beat * 0.75, *n, 0.05, -0.4);
        }
    }
    m.master(0.89);
    m
}

/// A quiet intro, a build, the drop, a breakdown, the drop again.
fn dynamics() -> Mix {
    let bpm = 128.0;
    let beat = 60.0 / bpm;
    let mut m = Mix::new(36.0, RATE);
    let bar = beat * 4.0;
    let bars = (m.secs() / bar) as usize;
    let chord = [[60.0f32, 63.0, 67.0, 70.0], [56.0, 60.0, 63.0, 67.0], [58.0, 62.0, 65.0, 68.0], [55.0, 58.0, 62.0, 65.0]];
    for b in 0..bars {
        let t0 = b as f32 * bar;
        let section = match b {
            0..=3 => 0, // intro: pad only
            4..=7 => 1, // build: hats and a rising snare
            8..=11 => 2, // drop
            12..=13 => 3, // breakdown: pad and plucks
            _ => 2,
        };
        pad(&mut m, t0, bar, &chord[b % 4], if section == 2 { 0.2 } else { 0.28 }, if section == 1 { 900.0 + 300.0 * (b - 4) as f32 } else { 1600.0 });
        if section == 1 {
            let n = 4 << (b - 4).min(2);
            for s in 0..n {
                snare(&mut m, t0 + s as f32 * bar / n as f32, 0.15 + 0.1 * (b - 4) as f32);
            }
            for s in 0..8 {
                hat(&mut m, t0 + s as f32 * beat / 2.0, 0.08, false, 0.3);
            }
        }
        if section == 2 {
            if b == 8 || b == 14 {
                crash(&mut m, t0, 0.35);
            }
            for q in 0..4 {
                let t = t0 + q as f32 * beat;
                kick(&mut m, t, 1.0);
                hat(&mut m, t + beat * 0.5, 0.18, true, 0.3);
                if q % 2 == 1 {
                    clap(&mut m, t, 0.5);
                }
                bass(&mut m, t + beat * 0.5, beat * 0.45, 36.0 - [0.0, 4.0, 2.0, 5.0][b % 4], 0.5);
            }
        }
        if section == 3 {
            for k in 0..8 {
                pluck(&mut m, t0 + k as f32 * beat / 2.0, 72.0 + [0.0, 3.0, 7.0, 10.0][k % 4], 0.12, if k % 2 == 0 { -0.5 } else { 0.5 });
            }
        }
    }
    m.master(0.89);
    // The intro about 18 dB down, rising through the build.
    m.shape(|t| {
        let b = t / bar;
        if b < 4.0 {
            -18.0
        } else if b < 8.0 {
            -18.0 + 18.0 * (b - 4.0) / 4.0
        } else if (12.0..14.0).contains(&b) {
            -10.0
        } else {
            0.0
        }
    });
    m
}

fn rock() -> Mix {
    let bpm = 140.0;
    let beat = 60.0 / bpm;
    let mut m = Mix::new(30.0, RATE);
    let bar = beat * 4.0;
    let bars = (m.secs() / bar) as usize;
    let roots = [40.0f32, 43.0, 45.0, 38.0];
    for b in 0..bars {
        let t0 = b as f32 * bar;
        if b % 4 == 0 {
            crash(&mut m, t0, 0.3);
        }
        for e in 0..8 {
            let t = t0 + e as f32 * beat / 2.0;
            guitar(&mut m, t, beat / 2.0 - 0.01, roots[b % 4], 0.22, -0.5);
            guitar(&mut m, t, beat / 2.0 - 0.01, roots[b % 4] + 12.0, 0.12, 0.5);
            hat(&mut m, t, 0.1, false, 0.2);
        }
        for q in 0..4 {
            let t = t0 + q as f32 * beat;
            if q % 2 == 0 {
                kick(&mut m, t, 0.9);
            } else {
                snare(&mut m, t, 0.7);
            }
        }
        kick(&mut m, t0 + beat * 2.5, 0.7);
        bass(&mut m, t0, bar, roots[b % 4] - 12.0, 0.35);
    }
    m.master(0.89);
    m
}

fn ballad() -> Mix {
    let bpm = 72.0;
    let beat = 60.0 / bpm;
    let mut m = Mix::new(30.0, RATE);
    let bar = beat * 4.0;
    let bars = (m.secs() / bar) as usize;
    let chords: [[f32; 3]; 4] = [[60.0, 64.0, 67.0], [57.0, 60.0, 64.0], [53.0, 57.0, 60.0], [55.0, 59.0, 62.0]];
    for b in 0..bars {
        let t0 = b as f32 * bar;
        let c = chords[b % 4];
        pad(&mut m, t0, bar, &[c[0] - 12.0, c[1], c[2]], 0.18, 1200.0);
        for k in 0..8 {
            pluck(&mut m, t0 + k as f32 * beat / 2.0, c[k % 3] + if k >= 4 { 12.0 } else { 0.0 }, 0.18, (k as f32 / 4.0 - 1.0) * 0.6);
        }
        pluck(&mut m, t0, c[0] - 24.0, 0.25, 0.0);
    }
    // A soft swell in the middle; mastered lower: a quiet song.
    m.master(0.5);
    m.shape(|t| -6.0 + 6.0 * (PI * t / 30.0).sin());
    m
}

fn ambient() -> Mix {
    let mut m = Mix::new(30.0, RATE);
    pad(&mut m, 0.0, 30.0, &[38.0, 45.0, 50.0], 0.5, 500.0);
    let notes = [74.0f32, 79.0, 81.0, 86.0, 77.0, 72.0];
    for (k, n) in notes.iter().cycle().take(14).enumerate() {
        bell(&mut m, 0.7 + k as f32 * 2.1, *n, 0.12, ((k * 7) % 5) as f32 / 2.5 - 1.0);
    }
    // About 30 dB under full scale.
    m.master(0.05);
    m
}

// ---- Test signals (the "Spectrum" album of bench/visualizer.rs) ---------------------------

fn sweep(secs: f32, rate: u32) -> Mix {
    let mut m = Mix::new(secs, rate);
    let (f0, f1) = (20.0f32, 20_000.0f32);
    let k = (f1 / f0).ln();
    m.add(0.0, secs, 0.0, move |t, _| {
        // Phase of an exponential sweep.
        let ph = TAU * f0 * secs / k * ((t / secs * k).exp() - 1.0);
        ph.sin() * 0.5
    });
    m
}

fn tones() -> Mix {
    let mut m = Mix::new(10.0, 48_000);
    for (f, db) in [(100.0f32, -12.0f32), (1000.0, -18.0), (5000.0, -24.0)] {
        let a = 10f32.powf(db / 20.0);
        m.add(0.0, 10.0, 0.0, move |t, _| (TAU * f * t).sin() * a);
    }
    m
}

fn pink() -> Mix {
    let mut m = Mix::new(10.0, 44_100);
    // Paul Kellet's economy pink filter.
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    m.add(0.0, 10.0, 0.0, move |_, rng| {
        let w = rng.next();
        b0 = 0.99765 * b0 + w * 0.099_046;
        b1 = 0.96300 * b1 + w * 0.296_516_4;
        b2 = 0.57000 * b2 + w * 1.052_691_3;
        (b0 + b1 + b2 + w * 0.1848) * 0.06
    });
    m
}

/// Kick, bass and hats; 20 dB down after 10 s.
fn groove() -> Mix {
    let beat = 0.5;
    let mut m = Mix::new(30.0, 48_000);
    for b in 0..60 {
        let t = b as f32 * beat;
        kick(&mut m, t, 0.8);
        hat(&mut m, t + beat * 0.5, 0.15, false, 0.0);
        bass(&mut m, t + beat * 0.5, 0.2, 36.0, 0.35);
        if b % 2 == 1 {
            snare(&mut m, t, 0.4);
        }
    }
    m.master(0.8);
    m.shape(|t| if t < 10.0 { 0.0 } else { -20.0 });
    m
}

// ---- Covers and files ----------------------------------------------------------------

/// A cover of soft blobs of these colours on a dark ground.
fn cover(path: &Path, colours: &[[u8; 3]], ground: [u8; 3]) {
    let size = 600u32;
    let blobs: Vec<(f32, f32, f32)> = (0..colours.len())
        .map(|i| {
            let a = i as f32 / colours.len() as f32 * TAU + 0.6;
            (0.5 + 0.28 * a.cos(), 0.5 + 0.26 * a.sin(), 0.32)
        })
        .collect();
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
        let mut c = [f32::from(ground[0]), f32::from(ground[1]), f32::from(ground[2])];
        for (k, &(bx, by, r)) in blobs.iter().enumerate() {
            let d = ((u - bx).powi(2) + (v - by).powi(2)).sqrt() / r;
            let w = (1.0 - d).clamp(0.0, 1.0).powf(1.5);
            for j in 0..3 {
                c[j] = c[j] * (1.0 - w) + f32::from(colours[k][j]) * w;
            }
        }
        image::Rgb([c[0] as u8, c[1] as u8, c[2] as u8])
    });
    img.save(path).expect("cover");
}

fn encode(m: &Mix, path: &Path, album: &str, artist: &str, title: &str, track: usize) {
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "f32le", "-ar", &m.rate.to_string(), "-ac", "2", "-i", "-"])
        .args(["-c:a", "flac", "-sample_fmt", "s16"])
        .args(["-metadata", &format!("title={title}"), "-metadata", &format!("artist={artist}"), "-metadata", &format!("album_artist={artist}")])
        .args(["-metadata", &format!("album={album}"), "-metadata", &format!("track={track}"), "-metadata", "date=2026"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg on PATH");
    let mut bytes = Vec::with_capacity(m.l.len() * 8);
    for (l, r) in m.l.iter().zip(&m.r) {
        bytes.extend_from_slice(&l.clamp(-1.0, 1.0).to_le_bytes());
        bytes.extend_from_slice(&r.clamp(-1.0, 1.0).to_le_bytes());
    }
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    assert!(child.wait().unwrap().success(), "ffmpeg failed for {}", path.display());
    println!("{}", path.display());
}

fn album(root: &Path, name: &str, artist: &str, colours: &[[u8; 3]], ground: [u8; 3], tracks: Vec<(&str, Mix)>) {
    let dir: PathBuf = root.join(artist).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    cover(&dir.join("cover.png"), colours, ground);
    for (i, (title, m)) in tracks.iter().enumerate() {
        encode(m, &dir.join(format!("{:02} {title}.flac", i + 1)), name, artist, title, i + 1);
    }
}

fn main() {
    let Some(root) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: viz_fixtures <empty folder>");
        std::process::exit(2);
    };
    if root.exists() && std::fs::read_dir(&root).map(|mut d| d.next().is_some()).unwrap_or(true) {
        eprintln!("{} is not empty: refusing to write there", root.display());
        std::process::exit(2);
    }
    std::fs::create_dir_all(&root).unwrap();
    album(
        &root,
        "Spectrum",
        "Fixture Signals",
        &[[70, 200, 230], [40, 120, 220]],
        [10, 14, 28],
        vec![("Sweep", sweep(20.0, 48_000)), ("Three Tones", tones()), ("Pink Noise", pink()), ("Groove", groove()), ("Sweep 96k", sweep(10.0, 96_000))],
    );
    album(
        &root,
        "Night Shift",
        "Fixture Band",
        &[[230, 40, 150], [60, 210, 230], [120, 60, 220]],
        [14, 8, 24],
        vec![("House", house()), ("Hip-Hop", hiphop()), ("Dynamics", dynamics())],
    );
    album(
        &root,
        "Daylight",
        "Fixture Band",
        &[[250, 150, 40], [30, 170, 150], [240, 210, 120]],
        [30, 22, 18],
        vec![("Rock", rock()), ("Ballad", ballad()), ("Ambient", ambient())],
    );
    album(&root, "Paper", "Fixture Covers", &[[255, 196, 64], [255, 110, 90], [80, 180, 255]], [236, 228, 214], vec![("Sunny", house())]);
    album(&root, "Graphite", "Fixture Covers", &[[150, 152, 158], [90, 92, 98], [204, 205, 210]], [26, 26, 29], vec![("Steel", rock())]);
}
