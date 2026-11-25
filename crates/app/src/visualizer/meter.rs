//! The music's meter: its tempo, and where its beats are.
//!
//! `beat::Tracker` hears kicks and snaps, each as it happens. That is not the beat: on real
//! music a bass line's notes are told as kicks, a kick under a rolling bass is missed, and
//! a tempo read off the kicks alone was known for two fifths of the time over a minute each
//! of 64 tracks of every kind from a real library (and what moved with those kicks landed
//! on the music's beats hardly more often than chance would have it). This listens to
//! everything, the way beat trackers have for twenty years (Scheirer; Davies and Plumbley;
//! Stark, Davies and Plumbley's BTrack, which this follows most closely):
//!
//! - **Onsets.** Every 10 ms, the spectrum of the newest 21 ms in sixteen bands from 35 Hz
//!   to 11.5 kHz, each band's level compressed (a logarithm: a quiet passage's onsets count
//!   like a loud one's); the onset function is how much the bands rose since the hop
//!   before, summed (a note, a drum, a chord: anything that starts), and the low notes'
//!   share of it apart (the two bands under 150 Hz: a kick, a bass note).
//! - **Tempo.** The onset function's last six seconds correlated with themselves: a beat's
//!   length is a lag at which it repeats, and at twice, three and four times that lag (a
//!   comb). Tempos near 140 a minute are preferred, broadly (0.7 of an octave either way
//!   is still six tenths as likely). The scores are held and eased, the tempo held while it
//!   scores within a fifth of the best. The comb cannot always tell a beat from one and a
//!   half: a tempo newly chosen is checked against the ones at two thirds and four thirds
//!   of its beat, and twice it, by laying a train of pulses at each spacing over the onsets
//!   (the right one lands on an onset every time).
//! - **Beats.** A running score: each hop's onset plus the best score of about a beat ago
//!   (those nearer exactly one beat ago count more). Half a beat after each beat the score
//!   is run on into the future with no onsets, and its highest point about a beat after the
//!   last one is where the next beat will be: beats are told when they happen, not after.
//! - **Whether there is a beat at all.** How far the best tempo's score stands above the
//!   others': a piano played freely, a film score's swell, noise, repeat a little at some
//!   lag as anything does, and nothing is told for them.
//!
//! Measured on those 64 minutes against a trained beat tracker run offline (beat_this): the
//! tempo held 97% of the time, the beats within 70 ms of the reference's (or of its half
//! beats, or every other one: which pulse a listener taps is a matter of taste) for 84% of
//! them on average, 44 tracks over 80%.
//!
//! A few thousand multiplications per hop and a quarter of a million four times a second.

const HOPS: f32 = 100.0;
/// The seconds of onset function the tempo is read from, and how often it is read (hops).
const WINDOW: usize = 600;
const EVERY: usize = 25;
/// The beat's lengths considered (hops): 200 to 60 a minute.
const LAG_MIN: usize = 30;
const LAG_MAX: usize = 100;
/// The usual tempo, and how many octaves from it a tempo is six tenths as usual.
const CENTRE: f32 = 140.0;
const SIGMA: f32 = 0.7;
/// How much of each reading enters the held scores, and the share of the best score the
/// held tempo must keep to stay.
const EASE: f32 = 0.25;
const STAY: f32 = 0.8;
/// How far the best tempo's score must stand above the others' (their upper quartile) for
/// there to be a beat, and to go on being one. Noise never stood more than 0.19 above, nor
/// did a film score or a piano played freely; music with a beat stands 0.5 to 1.8 above.
const LOCK: f32 = 0.26;
const UNLOCK: f32 = 0.18;
/// A tempo at two thirds or four thirds of the chosen beat is taken instead when its pulses
/// land this much better; one at twice the beat when they land this much better again.
const KIN: f32 = 1.06;
const TWICE: f32 = 1.25;
/// The running score: how much of it is what came before, and how tightly "a beat ago" is
/// meant.
const CARRY: f32 = 0.9;
const TIGHT: f32 = 5.0;
/// The bands' edges (Hz); the first two are the low notes.
const EDGES: [f32; 17] = [35.0, 90.0, 150.0, 220.0, 320.0, 450.0, 620.0, 850.0, 1150.0, 1550.0, 2100.0, 2800.0, 3700.0, 4900.0, 6500.0, 8600.0, 11500.0];
/// The onset function is counted over its own running mean, and that mean never under
/// this (music's is 2 to 4: the bands' levels waver that much from hop to hop; a held
/// note's is a hundredth of it, and what little it wavers is no rhythm); under `QUIET`
/// there is no beat to speak of whatever repeats.
const FLOOR: f32 = 0.25;
const QUIET: f32 = 0.6;
/// How much of the past is kept (hops).
const KEEP: usize = 1024;

/// What the meter says of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metered {
    /// A beat fell in this frame.
    pub beat: bool,
    /// The beat's length in seconds; 0 while there is no beat to speak of.
    pub period: f32,
    /// Where in the beat this frame is (0 at the beat, toward 1 before the next).
    pub phase: f32,
    /// The strongest onset of the low notes in this frame, over what is usual lately
    /// (1: an ordinary moment; a kick is several times that).
    pub low: f32,
    /// How far the tempo's score stands above the others' (`LOCK`).
    pub sure: f32,
}

/// A radix-2 complex FFT of one size, in place.
struct Fft {
    n: usize,
    twiddles: Vec<(f32, f32)>,
    rev: Vec<u32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        Fft { n, twiddles, rev }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let (half, step) = (len / 2, n / len);
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// The past, by hop number: the newest `KEEP` values.
#[derive(Default)]
struct Past {
    v: Vec<f32>,
    /// The hop number of `v[0]`.
    base: usize,
}

impl Past {
    fn push(&mut self, x: f32) {
        if self.v.len() >= 2 * KEEP {
            self.v.drain(..KEEP);
            self.base += KEEP;
        }
        self.v.push(x);
    }

    /// The value at hop `n` (0 before the beginning, and for what is no longer kept).
    fn at(&self, n: i64) -> f32 {
        let i = n - self.base as i64;
        if i < 0 { 0.0 } else { self.v.get(i as usize).copied().unwrap_or(0.0) }
    }

    /// The newest `count` values, oldest first.
    fn last(&self, count: usize) -> &[f32] {
        &self.v[self.v.len().saturating_sub(count)..]
    }
}

pub struct Meter {
    rate: u32,
    hop: usize,
    fft: Fft,
    window: Vec<f32>,
    /// Each band's first bin and the one after its last.
    bands: Vec<(usize, usize)>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// The bands' levels a hop ago (empty: none yet).
    before: Vec<f32>,
    /// Time passed that no hop has covered yet (seconds).
    owed: f32,
    /// Hops so far.
    n: usize,
    /// The onset function over its own running mean, and the running score.
    onsets: Past,
    score: Past,
    mean: f32,
    mean_low: f32,
    /// The comb's scores, held and eased, for each lag from `LAG_MIN`; how usual each is.
    held: Vec<f32>,
    usual: Vec<f32>,
    /// The beat's length in hops (0: none yet), how well it scores, whether there is a beat.
    period: f32,
    sure: f32,
    locked: bool,
    /// The next beat and the last (hops), and when the next is worked out.
    next: f32,
    last: f32,
    predict_at: usize,
    scratch: Vec<f32>,
    acf: Vec<f32>,
}

impl Default for Meter {
    fn default() -> Self {
        Meter::new(48_000)
    }
}

impl Meter {
    fn new(rate: u32) -> Meter {
        let rate = rate.max(8000);
        // About 21 ms, a power of two: 1024 points at 44.1 and 48 kHz.
        let nfft = ((rate as f32 * 0.0213) as usize).next_power_of_two().max(256);
        let hop = (rate as f32 / HOPS).round() as usize;
        let window = (0..nfft).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / nfft as f32).cos()).collect();
        let bin = rate as f32 / nfft as f32;
        let mut bands = Vec::new();
        for w in EDGES.windows(2) {
            let (a, b) = ((w[0] / bin).ceil() as usize, ((w[1] / bin).ceil() as usize).min(nfft / 2));
            // A band narrower than a bin still has the bin nearest it.
            bands.push(if b > a { (a, b) } else { (a.min(nfft / 2 - 1), a.min(nfft / 2 - 1) + 1) });
        }
        let usual = (LAG_MIN..=LAG_MAX).map(|lag| (-0.5 * ((60.0 * HOPS / lag as f32 / CENTRE).log2() / SIGMA).powi(2)).exp()).collect();
        Meter {
            rate,
            hop,
            fft: Fft::new(nfft),
            window,
            bands,
            re: vec![0.0; nfft],
            im: vec![0.0; nfft],
            before: Vec::new(),
            owed: 0.0,
            n: 0,
            onsets: Past::default(),
            score: Past::default(),
            mean: 0.0,
            mean_low: 0.0,
            held: vec![0.0; LAG_MAX - LAG_MIN + 1],
            usual,
            period: 0.0,
            sure: 0.0,
            locked: false,
            next: -1.0,
            last: -1.0,
            predict_at: 0,
            scratch: Vec::new(),
            acf: Vec::new(),
        }
    }

    /// Forget everything (the visualizer stopped, another track, a pause).
    pub fn reset(&mut self) {
        *self = Meter::new(self.rate);
    }

    /// The beat's length in seconds, 0 while there is no beat to speak of.
    pub fn period(&self) -> f32 {
        if self.locked && self.period > 0.0 { self.period / HOPS } else { 0.0 }
    }

    /// A frame: `samples` are the newest being heard (mono, oldest first), `dt` seconds after
    /// the last frame's.
    pub fn update(&mut self, samples: &[f32], rate: u32, dt: f32) -> Metered {
        if rate != self.rate {
            *self = Meter::new(rate);
        }
        let nfft = self.fft.n;
        // A stall (the window dragged, a long frame): what was missed is not made up, and the
        // levels before it are no measure of a rise.
        if dt > 0.12 {
            self.before.clear();
            self.owed = 0.0;
        }
        self.owed += dt.max(0.0);
        let hops = (self.owed * HOPS) as usize;
        self.owed -= hops as f32 / HOPS;
        let (mut beat, mut low) = (false, 0.0f32);
        for k in (0..hops).rev() {
            let Some(end) = samples.len().checked_sub(k * self.hop) else { continue };
            if end < nfft {
                continue;
            }
            let (b, l) = self.hop(&samples[end - nfft..end]);
            beat |= b;
            low = low.max(l);
        }
        let now = self.n as f32 + self.owed * HOPS;
        let on = self.locked && self.period > 0.0;
        let phase = if on && self.last >= 0.0 { ((now - self.last) / self.period).clamp(0.0, 1.5) } else { 0.0 };
        Metered { beat: beat && on, period: self.period(), phase, low, sure: self.sure }
    }

    /// One hop: the newest `nfft` samples up to it. Whether a beat falls on it, and the low
    /// notes' onset over what is usual.
    fn hop(&mut self, seg: &[f32]) -> (bool, f32) {
        let nfft = self.fft.n;
        for ((re, &x), &w) in self.re.iter_mut().zip(seg).zip(&self.window) {
            *re = x * w;
        }
        self.im.fill(0.0);
        self.fft.run(&mut self.re, &mut self.im);
        let norm = 4.0 / nfft as f32;
        let (mut all, mut low) = (0.0f32, 0.0f32);
        let first = self.before.is_empty();
        if first {
            self.before.resize(self.bands.len(), 0.0);
        }
        for (i, &(a, b)) in self.bands.iter().enumerate() {
            let energy: f32 = (a..b).map(|k| (self.re[k] * self.re[k] + self.im[k] * self.im[k]) * norm * norm).sum();
            let level = (1000.0 * energy.sqrt()).ln_1p();
            if !first {
                let rise = (level - self.before[i]).max(0.0);
                all += rise;
                if i < 2 {
                    low += rise;
                }
            }
            self.before[i] = level;
        }
        self.step(all, low)
    }

    /// One hop's onset values.
    fn step(&mut self, all: f32, low: f32) -> (bool, f32) {
        let n = self.n;
        let k = 1.0 - (-1.0 / (3.0 * HOPS)).exp();
        self.mean += (all - self.mean) * k;
        self.mean_low += (low - self.mean_low) * k;
        let x = all / (self.mean + FLOOR);
        self.onsets.push(x);
        if n.is_multiple_of(EVERY) {
            self.tempo();
        }
        let per = self.period;
        let mut beat = false;
        if per <= 0.0 {
            self.score.push(x);
        } else {
            let past = self.best_before(n as i64, per, &self.score);
            self.score.push((1.0 - CARRY) * x + CARRY * past);
            if self.next < 0.0 {
                self.next = n as f32 + per;
                self.predict_at = n + (per / 2.0) as usize;
            }
            if n as f32 >= self.next && self.last < self.next {
                beat = true;
                self.last = self.next;
                self.predict_at = n + (per / 2.0).round() as usize;
            }
            if n == self.predict_at {
                self.predict();
            }
        }
        self.n += 1;
        (beat, low / (self.mean_low + FLOOR * 0.2))
    }

    /// The best score from half a beat to two beats before hop `n`, those nearest one beat
    /// before counting most.
    fn best_before(&self, n: i64, per: f32, score: &Past) -> f32 {
        let (far, near) = ((2.0 * per).round() as i64, ((per / 2.0).round() as i64).max(1));
        let mut best = 0.0f32;
        for back in near..=far {
            if n - back < 0 {
                break;
            }
            let w = (-0.5 * (TIGHT * (back as f32 / per).ln()).powi(2)).exp();
            best = best.max(score.at(n - back) * w);
        }
        best
    }

    /// The next beat: the running score carried on a beat into the future with no onsets,
    /// its highest point about half a beat from now (this is asked half a beat after a beat).
    fn predict(&mut self) {
        let per = self.period;
        let span = per.round() as usize;
        let n = self.n as i64;
        // The future's scores follow the past's in one run of numbers.
        let back = 2 * span + 2;
        self.scratch.clear();
        self.scratch.extend((0..back as i64).map(|i| self.score.at(n + 1 - back as i64 + i)));
        let (far, near) = ((2.0 * per).round() as usize, ((per / 2.0).round() as usize).max(1));
        for i in 0..=span {
            let m = back + i;
            let mut best = 0.0f32;
            for b in near..=far.min(m) {
                let w = (-0.5 * (TIGHT * (b as f32 / per).ln()).powi(2)).exp();
                best = best.max(self.scratch[m - b] * w);
            }
            self.scratch.push(best);
        }
        let (mut top, mut at) = (-1.0f32, span / 2);
        for i in 1..=span {
            let g = (-0.5 * ((i as f32 - per / 2.0) / (per / 2.0)).powi(2)).exp();
            let v = self.scratch[back + i - 1] * g;
            if v > top {
                (top, at) = (v, i);
            }
        }
        self.next = (self.n + at) as f32;
    }

    /// How well a train of pulses `lag` hops apart lands on the onsets `sig` (ending now),
    /// at its best place: the mean of the onsets under it; the lag refined by quarter hops.
    fn pulses(sig: &[f32], lag: f32) -> (f32, f32) {
        let mut best = (0.0f32, lag);
        for d in [-0.5f32, -0.25, 0.0, 0.25, 0.5] {
            let l = lag + d;
            let count = ((sig.len() - 1) as f32 / l) as usize;
            if count < 4 {
                continue;
            }
            let mut top = 0.0f32;
            for place in 0..l.ceil() as usize {
                let mut sum = 0.0;
                for i in 0..count {
                    let at = (sig.len() - 1) as f32 - place as f32 - i as f32 * l;
                    sum += sig[(at.round().max(0.0)) as usize];
                }
                top = top.max(sum / count as f32);
            }
            if top > best.0 {
                best = (top, l);
            }
        }
        best
    }

    /// The tempo itself, or its kin: the one at two thirds or four thirds of its beat when a
    /// train of pulses at that spacing lands clearly better, then the one at twice the beat
    /// when every other pulse of this one lands on little.
    fn kin(&self, lag: f32) -> f32 {
        let raw = self.onsets.last(WINDOW);
        // An onset counts for the hops next to it too (a pulse a hop off still lands).
        let sig: Vec<f32> = (0..raw.len()).map(|i| raw[i].max(raw[i.saturating_sub(1)]).max(raw[(i + 1).min(raw.len() - 1)])).collect();
        let within = |l: f32| l >= LAG_MIN as f32 && l <= LAG_MAX as f32;
        let own = Self::pulses(&sig, lag);
        let mut best = (own.0 * KIN, lag);
        for f in [2.0 / 3.0, 4.0 / 3.0] {
            if within(lag * f) {
                let c = Self::pulses(&sig, lag * f);
                if c.0 > best.0 {
                    best = c;
                }
            }
        }
        if within(best.1 * 2.0) {
            let base = Self::pulses(&sig, best.1);
            let twice = Self::pulses(&sig, best.1 * 2.0);
            if twice.0 > base.0 * TWICE {
                return twice.1;
            }
        }
        best.1
    }

    /// The tempo, from the onset function's last seconds.
    fn tempo(&mut self) {
        let x = self.onsets.last(WINDOW);
        let len = x.len();
        if len < WINDOW / 2 {
            return;
        }
        let mean = x.iter().sum::<f32>() / len as f32;
        // Its correlation with itself at each lag the comb reads, each over as many products
        // as it has, against the lag of none.
        let most = (4 * LAG_MAX + 4).min(len * 9 / 10);
        self.acf.clear();
        let zero: f32 = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / len as f32;
        if zero <= 1e-9 {
            return;
        }
        for lag in 0..=most {
            let s: f32 = (lag..len).map(|i| (x[i] - mean) * (x[i - lag] - mean)).sum();
            self.acf.push(s / (len - lag) as f32 / zero);
        }
        for (j, lag) in (LAG_MIN..=LAG_MAX).enumerate() {
            let mut s = 0.0f32;
            for k in 1..=4usize {
                let c = k * lag;
                if c + k >= len * 9 / 10 || c + k - 1 > most {
                    break;
                }
                let peak = self.acf[c + 1 - k..c + k].iter().copied().fold(f32::MIN, f32::max);
                s += peak / (k as f32).sqrt();
            }
            self.held[j] += (s.max(0.0) * self.usual[j] - self.held[j]) * EASE;
        }
        let top = |v: &[f32]| v.iter().enumerate().fold((0usize, f32::MIN), |b, (i, &s)| if s > b.1 { (i, s) } else { b });
        let mut best = top(&self.held).0;
        if self.period > 0.0 {
            let cur = (self.period.round() as usize).clamp(LAG_MIN, LAG_MAX) - LAG_MIN;
            let (a, b) = (cur.saturating_sub(2), (cur + 3).min(self.held.len()));
            let near = a + top(&self.held[a..b]).0;
            if self.held[near] >= STAY * self.held[best] {
                best = near;
            }
        }
        // Between the hops: a parabola through the peak.
        let mut lag = (LAG_MIN + best) as f32;
        if best > 0 && best + 1 < self.held.len() {
            let (a, b, c) = (self.held[best - 1], self.held[best], self.held[best + 1]);
            let bend = a - 2.0 * b + c;
            if bend < 0.0 {
                lag += 0.5 * (a - c) / bend;
            }
        }
        // How sure: the best score over the others' upper quartile (any sound repeats a
        // little at some lag; a beat stands out).
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.held);
        self.scratch.sort_by(f32::total_cmp);
        self.sure = self.held[best] - self.scratch[self.scratch.len() * 3 / 4];
        self.locked = self.mean >= QUIET && self.sure >= if self.locked { UNLOCK } else { LOCK };
        let same = |a: f32, b: f32| (a - b).abs() / b < 0.06;
        if self.period > 0.0 && same(lag, self.period) {
            self.period += (lag - self.period) * 0.5;
            return;
        }
        // Another tempo: itself or its kin, unless that is the one held.
        let kin = self.kin(lag);
        if self.period > 0.0 && (same(kin, self.period) || (kin != lag && [2.0 / 3.0, 4.0 / 3.0].iter().any(|f| same(lag * f, self.period)))) {
            return;
        }
        self.period = kin;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A made-up track: a kick on every beat (or as `kicks` says, in beats within a bar of
    /// four), a snare on two and four, a hat on the half beats, a bass note, a pad.
    fn track(bpm: f32, secs: f32, kicks: &[f32]) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let mut seed = 0x1357_9bdfu32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        };
        (0..(secs * RATE as f32) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let bar = (t / beat) % 4.0;
                let since = |at: f32| (bar - at).rem_euclid(4.0) * beat;
                let thump = |ph: f32| 0.8 * (-ph / 0.11).exp() * (std::f32::consts::TAU * (50.0 * ph + 3.5 * (1.0 - (-ph / 0.05).exp()))).sin();
                let kick: f32 = kicks.iter().map(|&k| thump(since(k))).sum();
                let nz = noise();
                let snare = 0.4 * ((-since(1.0) / 0.05).exp() + (-since(3.0) / 0.05).exp()) * nz;
                let hat = 0.1 * (-((t + beat / 2.0) % beat) / 0.02).exp() * nz;
                let bass = 0.12 * (std::f32::consts::TAU * 55.0 * t).sin();
                let pad = 0.08 * (std::f32::consts::TAU * 440.0 * t).sin() + 0.05 * (std::f32::consts::TAU * 660.0 * t).sin();
                kick + snare + hat + bass + pad
            })
            .collect()
    }

    /// The meter run over a sound at `fps` frames a second: every frame's time and answer.
    fn listen(sound: &[f32], fps: f32) -> Vec<(f32, Metered)> {
        let mut m = Meter::default();
        let (dt, size) = (1.0 / fps, 8192usize);
        let mut out = Vec::new();
        let mut t = 0.0f32;
        while ((t * RATE as f32) as usize) < sound.len() {
            let end = (t * RATE as f32) as usize;
            let mut buf = vec![0.0; size];
            let from = end.saturating_sub(size);
            buf[size - (end - from)..].copy_from_slice(&sound[from..end]);
            out.push((t, m.update(&buf, RATE, dt)));
            t += dt;
        }
        out
    }

    #[test]
    fn the_tempo_is_found_and_the_beats_fall_on_the_kicks() {
        for (bpm, fps) in [(92.0, 60.0), (120.0, 60.0), (128.0, 144.0), (140.0, 30.0), (174.0, 60.0)] {
            let heard = listen(&track(bpm, 16.0, &[0.0, 1.0, 2.0, 3.0]), fps);
            let at = |secs: f32| heard.iter().find(|h| h.0 >= secs).unwrap().1;
            assert!(at(7.0).period > 0.0, "{bpm} at {fps}: a beat after seven seconds");
            let told = 60.0 / at(15.5).period;
            assert!((told - bpm).abs() < bpm * 0.015, "{bpm} at {fps}: {told}");
            // Each beat told from the tenth second on, within 45 ms of a kick, or all of them
            // of the hats halfway between (a pulse is told as it happens, not a frame late
            // like something heard; which pulse is the beat is the kicks' to say:
            // `beat::Tracker`).
            let beat = 60.0 / bpm;
            let beats: Vec<f32> = heard.iter().filter(|h| h.1.beat && h.0 > 10.0).map(|h| h.0).collect();
            assert!((beats.len() as f32 - 6.0 / beat).abs() <= 1.5, "{bpm} at {fps}: {} beats in six seconds", beats.len());
            let off = |b: f32, from: f32| (b - from + beat / 2.0).rem_euclid(beat) - beat / 2.0;
            let near = |from: f32| beats.iter().all(|&b| off(b, from).abs() < 0.045 + 1.0 / fps);
            assert!(near(0.0) || near(beat / 2.0), "{bpm} at {fps}: {:?}", beats.iter().map(|&b| off(b, 0.0)).collect::<Vec<_>>());
            // The phase runs from the beat: small just after one.
            let after = heard.iter().zip(heard.iter().skip(1)).find(|(a, _)| a.1.beat && a.0 > 10.0).unwrap().1;
            assert!(after.1.phase < 0.25, "{bpm} at {fps}: {}", after.1.phase);
        }
    }

    #[test]
    fn kicks_off_the_beat_do_not_move_it() {
        // A hip-hop bar: kicks on the one and half a beat after the three.
        let heard = listen(&track(88.0, 18.0, &[0.0, 2.5]), 60.0);
        let told = 60.0 / heard.last().unwrap().1.period;
        assert!((told - 88.0).abs() < 2.0 || (told - 176.0).abs() < 4.0, "{told}");
        let beat = 60.0 / told;
        for (t, _) in heard.iter().filter(|h| h.1.beat && h.0 > 12.0) {
            let off = (t + beat / 2.0) % beat - beat / 2.0;
            assert!(off.abs() < 0.06, "a beat {off} s off the bar's grid, at {t}");
        }
    }

    #[test]
    fn no_beat_is_told_where_there_is_none() {
        // A pad and a held bass note; noise; silence.
        let n = RATE as usize * 12;
        let pad: Vec<f32> = (0..n).map(|i| i as f32 / RATE as f32).map(|t| 0.2 * (std::f32::consts::TAU * 220.0 * t).sin() * (1.0 + 0.3 * (0.7 * t).sin()) + 0.15 * (std::f32::consts::TAU * 55.0 * t).sin()).collect();
        let mut seed = 0x2468_ace1u32;
        let hiss: Vec<f32> = (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                0.2 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0)
            })
            .collect();
        for (name, sound) in [("a pad", pad), ("noise", hiss), ("silence", vec![0.0; n])] {
            let heard = listen(&sound, 60.0);
            let beats = heard.iter().filter(|h| h.1.beat).count();
            assert!(beats == 0 && heard.last().unwrap().1.period == 0.0, "{name}: {beats} beats, period {}", heard.last().unwrap().1.period);
        }
    }

    #[test]
    fn a_kick_is_a_low_onset_and_a_stall_is_not_one() {
        let sound = track(120.0, 8.0, &[0.0, 1.0, 2.0, 3.0]);
        let heard = listen(&sound, 60.0);
        // In the frames just after each kick the low notes' onset is several times the usual;
        // between kicks it is not.
        let (mut on, mut between) = (f32::MAX, 0.0f32);
        for beat in 6..14 {
            let t = beat as f32 * 0.5;
            let peak = |a: f32, b: f32| heard.iter().filter(|h| h.0 >= t + a && h.0 < t + b).map(|h| h.1.low).fold(0.0, f32::max);
            on = on.min(peak(0.0, 0.06));
            between = between.max(peak(0.12, 0.2));
        }
        assert!(on > 3.0 && between < on / 2.0, "at the kicks {on}, between them {between}");
        // A frame a quarter of a second long: nothing is made of the gap.
        let mut m = Meter::default();
        let mut worst = 0.0f32;
        for i in 1..200usize {
            let dt = if i == 100 { 0.25 } else { 1.0 / 60.0 };
            let end = (((i as f32 / 60.0) + if i >= 100 { 0.23 } else { 0.0 }) * RATE as f32) as usize;
            let got = m.update(&sound[end.saturating_sub(8192)..end], RATE, dt);
            if (100..103).contains(&i) {
                worst = worst.max(got.low);
            }
        }
        assert!(worst < 30.0, "{worst}");
    }
}
