//! Hearing the beat: where the kicks are, and the tempo they keep.
//!
//! The spectrum the visualizer draws is a 170 ms window, smoothed again as it moves: a kick
//! in it is a slow swell, told late and vaguely. This listens to the sound itself, the
//! newest 35 ms of it each frame:
//!
//! - **Kicks.** The energy at five frequencies from 45 to 110 Hz (Goertzel, under a Hann
//!   window: no state between frames, so it does not matter that successive frames overlap
//!   or by how much). A kick is that energy rising sharply (`Riser`): the rise summed over
//!   the frames it lasts, told the moment it passes what a kick needs (so a frame or two
//!   after the kick, not after its peak), no weaker than a share of what recent kicks'
//!   rises were, and reaching near the loudest the band has lately been (a bass note between
//!   two kicks is not one, nor is the band swelling back after two low notes cancelled for
//!   a moment). Its strength is how far past what it needed the rise went.
//! - **Snaps.** The same for the sound's sharp end (the energy of its sample-to-sample
//!   change): a snare, a clap, a rim shot; a hi-hat under a snare is too quiet to be one.
//!   Shown as sparks, not as beats; and they help find the tempo.
//! - **The tempo and the beats** are not read off the kicks (a bass line's notes are told
//!   as kicks, a kick under a rolling bass is missed): `meter::Meter` hears them in
//!   everything that starts, and tells each beat as it happens.
//!
//! No allocation after the first frames, a few thousand multiplications a frame.

use crate::visualizer::meter::{Meter, Metered};

/// The kick band's frequencies (Hz).
const KICK_HZ: [f32; 5] = [45.0, 58.0, 72.0, 90.0, 110.0];
/// The window listened to (seconds).
const WINDOW: f32 = 0.035;
/// What a rise must reach to be a kick (natural log of the energy's growth: 1.0 is 2.7
/// times the energy, 4.3 dB), and the share of recent kicks' rises it must reach as well.
const RISE: f32 = 1.0;
const SHARE: f32 = 0.42;
/// The least time between two kicks (seconds).
const APART: f32 = 0.11;
/// For this long after one, another must be as loud as it was (seconds).
const AGAIN: f32 = 0.3;
/// How far under the band's recent loudest a kick may be (natural log of the energy: 1.6 is
/// 7 dB), and how fast that loudest is forgotten (per second: a quieter passage's kicks are
/// kicks again within a second or two).
const UNDER: f32 = 1.6;
const FORGET: f32 = 0.4;
/// The most a rise counts for (the first sound after silence is a huge one, and no measure
/// of the kicks that follow).
const MOST: f32 = 4.5;
/// Below these energies nothing is a kick (about -62 dBFS) or a snap.
const FLOOR: f32 = 6e-7;
const SNAP_FLOOR: f32 = 2e-7;
/// How long after it started a kick is told (seconds: the window filling, a frame).
const TOLD: f32 = 0.025;

/// What was heard this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Heard {
    /// A kick: its strength (0.3 to 1); 0 for none.
    pub hit: f32,
    /// It was a kick (as `hit` says: kept for what reads it by name).
    pub kick: bool,
    /// A snare, a clap: its strength (0.3 to 1), 0 for none.
    pub snap: f32,
    /// The beat's length in seconds, 0 while the music has no beat to speak of (`Meter`).
    pub period: f32,
    /// Where in the beat this frame is (0 at the beat, toward 1 before the next).
    pub phase: f32,
    /// One of the music's beats fell in this frame (told as it happens, kick or no kick).
    pub beat: bool,
    /// The low notes' onset in this frame over what is usual lately (`Metered::low`).
    pub low: f32,
    /// How bright the sound is now: the energy of its sample-to-sample change over its
    /// energy above 300 Hz (the low notes left out of it: a kick's body under a clap is
    /// not what tells the clap). A snare's or a clap's noise is bright for as long as it
    /// lasts (0.1 to 1.2); a kick's sharp edge only for a few milliseconds; a plucked or
    /// sung note, a pad, are not (under 0.05).
    pub bright: f32,
}

impl Heard {
    /// Beats a minute, 0 while no tempo is known.
    #[cfg_attr(not(any(test, feature = "bench")), allow(dead_code))]
    pub fn bpm(&self) -> f32 {
        if self.period > 0.0 { 60.0 / self.period } else { 0.0 }
    }
}

/// A band's energy over time, and the sharp rises in it.
struct Riser {
    /// Last frame's energy (its log), and the rise under way.
    last: f32,
    rise: f32,
    /// The rise under way has been told.
    told: bool,
    /// What recent rises came to (a slow average).
    usual: f32,
    /// The loudest the band has lately been (its log), forgotten slowly.
    top: f32,
    /// Since the last one told, and how loud that one got (its log, followed for the 80 ms
    /// after it was told: it is told before its peak).
    since: f32,
    peak: f32,
    started: bool,
}

impl Default for Riser {
    fn default() -> Self {
        Riser { last: 0.0, rise: 0.0, told: false, usual: 0.0, top: f32::MIN, since: 9.0, peak: f32::MIN, started: false }
    }
}

impl Riser {
    /// This frame's energy: the strength of a rise told now (0.3 to 1), or 0.
    fn step(&mut self, energy: f32, floor: f32, dt: f32) -> f32 {
        self.since += dt;
        // Never under the floor: a rise out of silence is counted from there.
        let level = energy.max(floor * 0.5).ln();
        let grew = if self.started { level - self.last } else { 0.0 };
        (self.started, self.last) = (true, level);
        self.top = level.max(self.top - dt * FORGET);
        if self.since < 0.08 {
            self.peak = self.peak.max(level);
        }
        // Right after one, only another as loud: a kick's tail swelling back as it beats
        // against a bass note is not a second kick, a second kick is.
        let again = self.since >= AGAIN || level >= self.peak - 0.35;
        let mut told = 0.0;
        if grew > 0.02 {
            // The rise: summed while the energy keeps growing.
            self.rise = (self.rise + grew).min(MOST);
            let needs = RISE.max(self.usual * SHARE);
            if !self.told && self.rise >= needs && level > floor.ln() && level >= self.top - UNDER && self.since >= APART && again {
                self.told = true;
                self.since = 0.0;
                self.peak = level;
                told = (0.4 + (self.rise - needs) * 0.3).clamp(0.3, 1.0);
            }
        } else {
            if self.told {
                // What this one's rise came to, for judging the next ones.
                self.usual += (self.rise - self.usual) * if self.usual == 0.0 { 1.0 } else { 0.3 };
            }
            (self.rise, self.told) = (0.0, false);
        }
        // Too rare to judge by: the usual rise eases back, so a quieter passage's are heard.
        self.usual *= (-dt / 6.0).exp();
        told
    }

    /// Nothing playing: no rise carries over.
    fn rest(&mut self, dt: f32) {
        self.since += dt;
        (self.rise, self.told, self.started) = (0.0, false, false);
    }
}

/// Whether the beat is on the meter's pulse or halfway between two. The meter hears
/// everything that starts, and what it tells is a pulse the music keeps: an open hat or a
/// bass note on every half beat can outweigh the kicks in it, and the pulse is then halfway
/// between the beats. The kicks say, when they say it plainly. (They do not say whether the
/// pulse is the beat or twice it: a slow song's eighth notes are as steady as its beats,
/// and the slow tracks of a real library were told at twice their tempo. Counting the
/// drums on the even and the odd pulses was tried and went back and forth within a song,
/// which is worse than a pulse twice as fast.)
#[derive(Default)]
struct Count {
    /// How much kick has lately fallen on the pulses, and halfway between them.
    on: f32,
    half: f32,
    halfway: bool,
    /// The meter's phase last frame.
    before: f32,
}

impl Count {
    /// A frame: what the meter said and the kick told (0: none). Where in the beat the
    /// frame is, and whether a beat fell in it.
    fn step(&mut self, m: &Metered, kick: f32) -> (f32, bool) {
        if m.period <= 0.0 {
            *self = Count::default();
            return (0.0, false);
        }
        if kick > 0.0 {
            // Where the kick started (it is told a frame or two after).
            let at = (m.phase - TOLD / m.period).rem_euclid(1.0);
            (self.on, self.half) = (self.on * 0.85, self.half * 0.85);
            if (at - 0.5).abs() <= 0.15 {
                self.half += kick;
            } else if at <= 0.15 || at >= 0.85 {
                self.on += kick;
            }
            // The meter is believed unless the kicks plainly say otherwise: four in five of
            // the last several halfway between its pulses (where a bass line plays on the
            // beats and between them alike, the kicks told fall on both sides, and say
            // nothing). As soon as they do not, it is believed again.
            if self.half > 3.0 && self.half > 4.0 * self.on {
                self.halfway = true;
            } else if self.on > self.half {
                self.halfway = false;
            }
        }
        let before = std::mem::replace(&mut self.before, m.phase);
        if self.halfway {
            ((m.phase + 0.5) % 1.0, before < 0.5 && m.phase >= 0.5 && m.phase < 1.0)
        } else {
            (m.phase, m.beat)
        }
    }
}

pub struct Tracker {
    /// Seconds listened to.
    now: f64,
    /// The Hann window for the length in use.
    window: Vec<f32>,
    low: Riser,
    high: Riser,
    /// The tempo and the beats.
    meter: Meter,
    /// The meter tells a pulse: on the beat, or halfway between two (`Count`).
    count: Count,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker { now: 0.0, window: Vec::new(), low: Riser::default(), high: Riser::default(), meter: Meter::default(), count: Count::default() }
    }
}

impl Tracker {
    /// Forget everything (the visualizer stopped, another track).
    pub fn reset(&mut self) {
        let window = std::mem::take(&mut self.window);
        let mut meter = std::mem::take(&mut self.meter);
        meter.reset();
        *self = Tracker { window, meter, ..Tracker::default() };
    }

    /// A frame with nothing playing: time passes, nothing is heard.
    pub fn rest(&mut self, dt: f32) -> Heard {
        let dt = dt.max(0.0);
        self.now += f64::from(dt);
        self.low.rest(dt);
        self.high.rest(dt);
        // A pause of some seconds: what comes after need not go on in time with what was.
        if self.low.since > 3.0 && self.meter.period() > 0.0 {
            self.meter.reset();
        }
        Heard::default()
    }

    /// A frame: `samples` are the newest being heard (mono, oldest first), `dt` seconds
    /// after the last frame's.
    pub fn update(&mut self, samples: &[f32], rate: u32, dt: f32) -> Heard {
        let dt = dt.clamp(0.0, 0.25);
        self.now += f64::from(dt);
        let (low, high, bright) = self.energies(samples, rate);
        let kick = self.low.step(low, FLOOR, dt);
        let snap = self.high.step(high, SNAP_FLOOR, dt);
        let m = self.meter.update(samples, rate, dt);
        let (phase, beat) = self.count.step(&m, kick);
        Heard { hit: kick, kick: kick > 0.0, snap, period: m.period, phase, beat, low: m.low, bright }
    }

    /// In the newest `WINDOW` of `samples`: the energy at the kick band's frequencies (a
    /// full-scale sine at one of them: 1), the energy of the sample-to-sample change
    /// (the sound's sharp end: a snare's, a clap's), and how bright the sound is
    /// (`Heard::bright`).
    fn energies(&mut self, samples: &[f32], rate: u32) -> (f32, f32, f32) {
        let n = ((rate as f32 * WINDOW) as usize).min(samples.len());
        if n < 64 {
            return (0.0, 0.0, 0.0);
        }
        if self.window.len() != n {
            self.window = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        }
        let src = &samples[samples.len() - n..];
        let norm = 16.0 / (n as f32 * n as f32);
        let low = KICK_HZ
            .iter()
            .map(|&hz| {
                let c = 2.0 * (std::f32::consts::TAU * hz / rate as f32).cos();
                let (mut s1, mut s2) = (0.0f32, 0.0f32);
                for (&x, &w) in src.iter().zip(&self.window) {
                    let s0 = x * w + c * s1 - s2;
                    s2 = s1;
                    s1 = s0;
                }
                (s1 * s1 + s2 * s2 - c * s1 * s2) * norm
            })
            .sum::<f32>()
            .max(0.0);
        // The newer half only: a snap is short, and half the window tells it sooner.
        let recent = &src[n / 2..];
        let high = recent.windows(2).map(|w| (w[1] - w[0]) * (w[1] - w[0])).sum::<f32>() / recent.len() as f32;
        // The sound above 300 Hz (one pole, run over the whole window from rest, the newer
        // half counted: no state between frames here either).
        let k = (-std::f32::consts::TAU * 300.0 / rate as f32).exp();
        let (mut y, mut before, mut body) = (0.0f32, src[0], 0.0f32);
        for (i, &x) in src.iter().enumerate() {
            y = k * (y + x - before);
            before = x;
            if i >= n / 2 {
                body += y * y;
            }
        }
        let bright = high / (body / recent.len() as f32 + 1e-9);
        (low, high, bright)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A made-up track: a kick on every beat (a sine falling from 120 to 50 Hz), a held
    /// bass note under it, a hat between the beats, a pad. `skip`: beats without their kick.
    fn track(bpm: f32, secs: f32, kick: f32, skip: &[usize]) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let beat = 60.0 / bpm;
        let mut seed = 0x1234_5678u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let (b, ph) = ((t / beat).floor() as usize, t % beat);
                let k = if skip.contains(&b) { 0.0 } else { kick * (-ph / 0.11).exp() * (std::f32::consts::TAU * (50.0 * ph + 70.0 * 0.05 * (1.0 - (-ph / 0.05).exp()))).sin() };
                let bass = 0.12 * (std::f32::consts::TAU * 55.0 * t).sin();
                let off = (t + beat / 2.0) % beat;
                let hat = 0.15 * (-off / 0.02).exp() * noise();
                let pad = 0.08 * (std::f32::consts::TAU * 440.0 * t).sin() + 0.05 * (std::f32::consts::TAU * 660.0 * t).sin();
                k + bass + hat + pad
            })
            .collect()
    }

    /// The tracker run over a track at `fps` frames a second: every frame's `Heard`, timed.
    fn listen(sound: &[f32], fps: f32) -> Vec<(f32, Heard)> {
        let mut tr = Tracker::default();
        let (dt, size) = (1.0 / fps, 8192usize);
        let mut out = Vec::new();
        let mut t = 0.0f32;
        while ((t * RATE as f32) as usize) < sound.len() {
            let end = (t * RATE as f32) as usize;
            let mut buf = vec![0.0; size];
            let from = end.saturating_sub(size);
            buf[size - (end - from)..].copy_from_slice(&sound[from..end]);
            out.push((t, tr.update(&buf, RATE, dt)));
            t += dt;
        }
        out
    }

    #[test]
    fn every_kick_is_heard_once_and_soon() {
        for (bpm, fps) in [(120.0, 60.0), (92.0, 60.0), (150.0, 60.0), (174.0, 60.0), (120.0, 30.0), (128.0, 144.0)] {
            let sound = track(bpm, 10.0, 0.8, &[]);
            let heard = listen(&sound, fps);
            let kicks: Vec<f32> = heard.iter().filter(|h| h.1.kick).map(|h| h.0).collect();
            let beat = 60.0 / bpm;
            let expected = (10.0 / beat).floor() as usize;
            assert!((expected - 1..=expected + 1).contains(&kicks.len()), "{bpm} at {fps}: {} kicks for {expected} beats", kicks.len());
            // Each within 70 ms after its kick began (the window filling, a frame or two).
            for k in &kicks {
                let late = k % beat;
                assert!(late < 0.07 + 1.0 / fps, "{bpm} at {fps}: a kick told {late} s after the beat at {k}");
            }
        }
    }

    #[test]
    fn the_tempo_is_the_meters_and_the_phase_is_small_at_a_kick() {
        for bpm in [100.0, 120.0, 128.0, 140.0, 174.0] {
            // (With a little hiss under it: where nothing else is as high as the hats, each
            // counts for as much as a kick, and every half beat is as good a beat.)
            let mut sound = track(bpm, 14.0, 0.8, &[]);
            let mut seed = 0x0f1e_2d3cu32;
            for s in sound.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *s += 0.02 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0);
            }
            let heard = listen(&sound, 60.0);
            let at = |secs: f32| heard.iter().find(|h| h.0 >= secs).unwrap().1;
            assert!((at(13.5).bpm() - bpm).abs() < bpm * 0.015, "{bpm}: {} after 13.5 s", at(13.5).bpm());
            let last = heard.iter().rev().find(|h| h.1.kick).unwrap();
            assert!(last.1.phase < 0.2 || last.1.phase > 0.9, "{bpm}: phase {} at a kick", last.1.phase);
            // A beat told for each one played, kick or no kick.
            let beats = heard.iter().filter(|h| h.1.beat && h.0 > 8.0).count() as f32;
            assert!((beats - 6.0 * bpm / 60.0).abs() <= 1.5, "{bpm}: {beats} beats in six seconds");
        }
    }

    #[test]
    fn the_kicks_say_whether_the_beat_is_on_the_pulse_or_halfway() {
        let m = |phase: f32, beat: bool| Metered { beat, period: 0.35, phase, low: 0.0, sure: 1.0 };
        // Twenty-four pulses, twenty frames each, a kick where `kick_at` says: where in the
        // beat the kicks of the last eight are told, and how many beats are.
        let run = |kick_at: &dyn Fn(usize) -> Option<f32>| {
            let mut c = Count::default();
            let mut out = (0.0, 0);
            for pulse in 0..24usize {
                for step in 0..20 {
                    let phase = step as f32 / 20.0;
                    let kick = kick_at(pulse).is_some_and(|at| (phase - at).abs() < 0.001);
                    let (ph, beat) = c.step(&m(phase, step == 0), if kick { 0.9 } else { 0.0 });
                    if pulse >= 16 {
                        out = (if kick { ph } else { out.0 }, out.1 + usize::from(beat));
                    }
                }
            }
            out
        };
        // Kicks just after every pulse: the pulse is the beat.
        let (at_kick, beats) = run(&|_| Some(0.1));
        assert!((at_kick - 0.1).abs() < 0.01 && beats == 8, "{at_kick} {beats}");
        // Kicks halfway between the pulses: the beat is there.
        let (at_kick, beats) = run(&|_| Some(0.6));
        assert!((at_kick - 0.1).abs() < 0.01 && beats == 8, "{at_kick} {beats}");
        // Kicks on the pulses and halfway alike (a bass line told as kicks): the meter is
        // believed.
        let (at_kick, beats) = run(&|p| Some(if p % 2 == 0 { 0.1 } else { 0.6 }));
        assert!(((at_kick - 0.1).abs() < 0.01 || (at_kick - 0.6).abs() < 0.01) && beats == 8, "{at_kick} {beats}");
        let mut c = Count::default();
        for pulse in 0..24usize {
            for step in 0..20 {
                let phase = step as f32 / 20.0;
                let kick = (phase - if pulse % 2 == 0 { 0.1 } else { 0.6 }).abs() < 0.001;
                c.step(&m(phase, step == 0), if kick { 0.9 } else { 0.0 });
                assert!(!c.halfway, "pulse {pulse}");
            }
        }
    }

    #[test]
    fn a_missing_kick_is_no_kick_and_the_beat_goes_on() {
        // Beats 16 and 20 without their kick: no kick is told there, a beat is.
        let sound = track(120.0, 14.0, 0.8, &[16, 20]);
        let heard = listen(&sound, 60.0);
        for b in [16.0f32, 20.0] {
            let near: Vec<&(f32, Heard)> = heard.iter().filter(|h| (h.0 - b * 0.5).abs() < 0.1).collect();
            assert!(near.iter().all(|h| h.1.hit == 0.0) && near.iter().filter(|h| h.1.beat).count() == 1, "beat {b}");
        }
        let kicks = heard.iter().filter(|h| h.1.hit > 0.0).count();
        assert!((25..=27).contains(&kicks), "{kicks}");
    }

    /// A hip-hop bar: kicks on the one and half a beat after the three, snares (noise) on
    /// the two and the four, hats on the quarters of each beat, a long low note under it.
    fn hiphop(bpm: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let beat = 60.0 / bpm;
        let mut seed = 0x2468_ace1u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let since = |t: f32, at: f32, every: f32| (t - at).rem_euclid(every);
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let bar = beat * 4.0;
                let thump = |ph: f32, amp: f32| amp * (-ph / 0.11).exp() * (std::f32::consts::TAU * (50.0 * ph + 70.0 * 0.05 * (1.0 - (-ph / 0.05).exp()))).sin();
                let kicks = thump(since(t, 0.0, bar), 0.8) + thump(since(t, beat * 2.5, bar), 0.7);
                let sub = 0.3 * (std::f32::consts::TAU * 31.0 * t).sin();
                let nz = noise();
                let snare = 0.5 * ((-since(t, beat, bar) / 0.05).exp() + (-since(t, beat * 3.0, bar) / 0.05).exp()) * nz;
                let hat = 0.06 * (-since(t, 0.0, beat / 4.0) / 0.012).exp() * nz;
                kicks + sub + snare + hat
            })
            .collect()
    }

    #[test]
    fn kicks_off_the_beat_and_snares_on_it_still_tell_the_tempo() {
        for bpm in [88.0, 96.0] {
            let heard = listen(&hiphop(bpm, 14.0), 60.0);
            let at = |secs: f32| heard.iter().find(|h| h.0 >= secs).unwrap().1;
            let told = at(13.5).bpm();
            assert!((told - bpm).abs() < 2.0 || (told - 2.0 * bpm).abs() < 4.0, "{bpm}: {told} after 13.5 s");
            // The snares are snaps, not kicks: two a bar, on the two and the four.
            // (After the first bars: until a snare has been heard, a hat is the sharpest thing.)
            let bars = 11.0 / (240.0 / bpm);
            let snaps = heard.iter().filter(|h| h.1.snap > 0.0 && h.0 >= 3.0).count() as f32;
            let kicks = heard.iter().filter(|h| h.1.kick && h.0 >= 3.0).count() as f32;
            assert!((snaps / bars - 2.0).abs() < 0.5 && (kicks / bars - 2.0).abs() < 0.5, "{bpm}: {snaps} snaps and {kicks} kicks in {bars} bars");
        }
    }

    #[test]
    fn nothing_is_heard_in_what_has_no_kicks() {
        // A held bass note, a pad and hiss; silence; a kick far too quiet to be one.
        let sound = track(120.0, 8.0, 0.0, &[]);
        let heard = listen(&sound, 60.0);
        assert!(heard.iter().filter(|h| h.1.hit > 0.0).count() <= 1, "{:?}", heard.iter().filter(|h| h.1.hit > 0.0).collect::<Vec<_>>());
        let heard = listen(&vec![0.0; RATE as usize * 3], 60.0);
        assert!(heard.iter().all(|h| h.1 == Heard::default()));
        // Paused: nothing, and what was held is let go.
        let mut tr = Tracker::default();
        let sound = track(120.0, 6.0, 0.8, &[]);
        for (i, _) in listen(&sound, 60.0).iter().enumerate() {
            let end = ((i as f32 / 60.0) * RATE as f32) as usize;
            tr.update(&sound[end.saturating_sub(8192)..end.max(1)], RATE, 1.0 / 60.0);
        }
        assert!(tr.meter.period() > 0.0);
        for _ in 0..240 {
            assert_eq!(tr.rest(1.0 / 60.0), Heard::default());
        }
        assert_eq!(tr.meter.period(), 0.0);
    }

    #[test]
    fn a_kick_is_stronger_the_harder_it_hits_and_a_bass_note_between_kicks_is_not_one() {
        let strength = |kick: f32| {
            let heard = listen(&track(120.0, 6.0, kick, &[]), 60.0);
            let k: Vec<f32> = heard.iter().filter(|h| h.1.kick).map(|h| h.1.hit.max(0.3)).collect();
            k.iter().sum::<f32>() / k.len().max(1) as f32
        };
        assert!(strength(0.9) >= strength(0.25), "{} {}", strength(0.9), strength(0.25));
        // Small bumps of bass between the kicks (a tenth of a kick's energy): not kicks.
        let mut sound = track(120.0, 8.0, 0.8, &[]);
        for (i, s) in sound.iter_mut().enumerate() {
            let t = i as f32 / RATE as f32;
            let ph = (t + 0.25) % 0.5;
            *s += 0.1 * (-ph / 0.08).exp() * (std::f32::consts::TAU * 80.0 * t).sin();
        }
        let kicks = listen(&sound, 60.0).iter().filter(|h| h.1.kick && h.0 > 2.0).count();
        assert!((11..=13).contains(&kicks), "{kicks} kicks in 6 s at 120");
    }

    #[test]
    fn a_claps_noise_stays_bright_a_kicks_edge_and_a_plucked_note_do_not() {
        // The track's kicks, each with a sharp edge (noise gone in a few milliseconds); a
        // clap on every other one (noise for a tenth of a second); a plucked note an eighth
        // of a second after each kick (the track's hats are on the half beats).
        let mut sound = track(120.0, 8.0, 0.8, &[]);
        let mut seed = 0x0bad_5eedu32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        for (i, s) in sound.iter_mut().enumerate() {
            let t = i as f32 / RATE as f32;
            let (beat, ph) = ((t / 0.5).floor() as usize, t % 0.5);
            let n = noise();
            *s += 0.5 * (-ph / 0.003).exp() * n;
            if beat % 2 == 1 {
                *s += 0.3 * (-ph / 0.06).exp() * n;
            }
            let note = (t + 0.375) % 0.5;
            *s += 0.4 * (-note / 0.15).exp() * (std::f32::consts::TAU * 330.0 * note).sin();
        }
        let heard = listen(&sound, 60.0);
        // How bright the sound is a frame or two after a moment has been told (itself a
        // frame after it happened).
        let after = |at: f32| {
            let v: Vec<f32> = heard.iter().filter(|h| h.0 > at + 0.025 && h.0 < at + 0.055).map(|h| h.1.bright).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        let (mut claps, mut edges, mut notes) = (f32::MAX, 0.0f32, 0.0f32);
        for beat in 4..15 {
            let t = beat as f32 * 0.5;
            if beat % 2 == 1 {
                claps = claps.min(after(t));
            } else {
                edges = edges.max(after(t));
            }
            notes = notes.max(after(t + 0.125));
        }
        assert!(claps > 0.1 && edges < 0.04 && notes < 0.04, "claps {claps}, kicks' edges {edges}, notes {notes}");
    }
}
