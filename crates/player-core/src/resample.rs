//! Sample-rate conversion: what lets a crossfade overlap two tracks at different rates.
//!
//! The device runs at one rate. When the incoming track of a crossfade is at another, its
//! samples are converted to the rate playing (`engine.rs` says which side and for how long);
//! nothing else is ever converted, so every other sample reaches the device as decoded.
//!
//! **The design.** Exact rational polyphase, a Kaiser-windowed sinc: the ratio is reduced to
//! `L/M` (48 000 / 44 100 = 160 / 147) and output frame `n` is the band-limited value of the
//! input at time `n M / L`, from a table of `L` phases of `taps` coefficients each. Every
//! output uses exactly the coefficients of its own phase (no interpolation between table
//! entries, so the filter is the only source of error), the sum is taken in f64, and the
//! result is rounded once to f32. Arbitrary integer rates work; a pair whose table would be
//! over `MAX_COEFS` (none of the usual rates, 8 to 768 kHz, but say 44 100 and 44 101 Hz)
//! has no filter (`Filter::new` gives `None`) and the engine falls back to a fade-out and a
//! reopen.
//!
//! - Passband: up to 0.4535 of the lower rate (20.0 kHz at 44.1 kHz, 21.8 at 48), flat
//!   within ±0.00001 dB (measured: `tests/resample.rs`).
//! - Stopband: from half the lower rate (its Nyquist frequency) on, at least 150 dB down
//!   (designed for 155): nothing aliases, not even into the transition band, and no image
//!   of an upsampled signal is left above the source's band.
//! - Transition: 20.0 to 22.05 kHz at 44.1 kHz; the same fraction of any lower rate. Opus,
//!   the 48 kHz format most often met here, is itself band-limited to 20 kHz.
//! - Linear phase: the impulse response is symmetric, so every frequency is delayed alike
//!   and the delay is a whole, known number of input frames, taken out entirely: output
//!   frame 0 is aligned with input frame 0 and a crossfade's overlap stays sample-aligned
//!   with its envelope. Minimum phase would move the ringing after the impulse but delay
//!   each frequency differently, so no single shift could align the two tracks; the
//!   symmetric pre-ringing it avoids is at the transition band (above 20 kHz), excited only
//!   by content there.
//! - Edges: the converter starts primed with silence before the first frame (a track starts
//!   from silence) and, at the end, pads silence as its lookahead, so the first output is
//!   right and the tail complete: `N` input frames give exactly `ceil(N L / M)` output
//!   frames, the output's length in time being the input's.
//! - Streaming: input is pushed in blocks of any size, output pulled frame by frame; the
//!   output does not depend on how the input was split (bit for bit). Memory is the table
//!   (160 x 224 f64 for 44.1 to 48 kHz, 287 kB; at most `MAX_COEFS`, 8 MB) and a buffer of
//!   `taps` frames plus a block; nothing is allocated after the first block.
//! - The latency is `taps / 2` input frames (2.5 ms whatever the rates): the converter reads
//!   that far ahead before giving out a frame, which the decode thread, a second ahead of the
//!   device, absorbs. There is no delay in the output.
//!
//! Converting between equal rates is a bypass: every sample is copied as it is.

use std::sync::Arc;

/// Passband edge, as a fraction of the lower of the two rates.
pub const PASSBAND: f64 = 0.4535;
/// Stopband edge, as a fraction of the lower rate: its Nyquist frequency.
pub const STOPBAND: f64 = 0.5;
/// Stopband attenuation the filter is designed for (Kaiser's formula), dB.
pub const DESIGN_DB: f64 = 155.0;
/// The largest table built (`L x taps` coefficients, f64): 8 MB.
pub const MAX_COEFS: usize = 1 << 20;

/// The polyphase table for one pair of rates. Built once per pair (a few milliseconds),
/// shared by every converter of that pair.
pub struct Filter {
    from: u32,
    to: u32,
    /// Output frames per `m` input frames: `to / gcd`, `from / gcd`.
    l: usize,
    m: usize,
    /// Coefficients per output frame (input frames it reads), a multiple of 4.
    taps: usize,
    /// `l` phases of `taps` coefficients; phase `p` is the output at `p / l` of an input
    /// frame past an input frame.
    coefs: Vec<f64>,
    bypass: bool,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The zeroth-order modified Bessel function of the first kind (Kaiser's window).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    loop {
        term *= q / (k * k);
        sum += term;
        if term < sum * 1e-21 {
            return sum;
        }
        k += 1.0;
    }
}

impl Filter {
    /// The filter converting `from` Hz to `to` Hz: a bypass when they are equal, `None` for a
    /// rate of 0 or a pair whose table would be over `MAX_COEFS`.
    pub fn new(from: u32, to: u32) -> Option<Self> {
        if from == 0 || to == 0 {
            return None;
        }
        if from == to {
            return Some(Filter { from, to, l: 1, m: 1, taps: 4, coefs: Vec::new(), bypass: true });
        }
        let g = gcd(u64::from(from), u64::from(to));
        let (l, m) = ((u64::from(to) / g) as usize, (u64::from(from) / g) as usize);
        let lower = f64::from(from.min(to));
        let fin = f64::from(from);
        // Kaiser: N = (A - 7.95) / (2.285 dw), in input frames (dw the transition's width in
        // radians per input frame), and beta for A.
        let transition = (STOPBAND - PASSBAND) * lower / fin;
        let n = (DESIGN_DB - 7.95) / (2.285 * 2.0 * std::f64::consts::PI * transition);
        let taps = (n.ceil() as usize).div_ceil(4) * 4;
        if l.checked_mul(taps).is_none_or(|c| c > MAX_COEFS) {
            return None;
        }
        let beta = 0.1102 * (DESIGN_DB - 8.7);
        let i0_beta = bessel_i0(beta);
        // The ideal low-pass, cut in the middle of the transition, with a gain of 1.
        let cut = (PASSBAND + STOPBAND) / 2.0 * lower / fin * 2.0; // 2 fc / fin
        let half = (taps / 2) as f64;
        let mut coefs = vec![0.0; l * taps];
        for p in 0..l {
            let row = &mut coefs[p * taps..(p + 1) * taps];
            for (j, c) in row.iter_mut().enumerate() {
                // Input frame j of the window lies `tau` input frames before the output time.
                let tau = p as f64 / l as f64 + half - 1.0 - j as f64;
                let x = tau / half;
                if x.abs() >= 1.0 {
                    continue;
                }
                let arg = std::f64::consts::PI * cut * tau;
                let sinc = if arg == 0.0 { 1.0 } else { arg.sin() / arg };
                *c = cut * sinc * bessel_i0(beta * (1.0 - x * x).sqrt()) / i0_beta;
            }
            // Each phase passes DC at exactly unity, so no phase is a hair louder than the
            // next (which would modulate the signal at the phase pattern's rate).
            let sum: f64 = row.iter().sum();
            row.iter_mut().for_each(|c| *c /= sum);
        }
        Some(Filter { from, to, l, m, taps, coefs, bypass: false })
    }

    pub fn from_rate(&self) -> u32 {
        self.from
    }

    pub fn to_rate(&self) -> u32 {
        self.to
    }

    /// The reduced ratio `(L, M)`: L output frames per M input frames.
    pub fn ratio(&self) -> (usize, usize) {
        (self.l, self.m)
    }

    /// Input frames each output frame is computed from.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Input frames read ahead before an output frame can be given out.
    pub fn lookahead(&self) -> usize {
        self.taps / 2
    }

    pub fn is_bypass(&self) -> bool {
        self.bypass
    }

    /// Output frames that `input` input frames make: `ceil(input L / M)`.
    pub fn output_frames(&self, input: u64) -> u64 {
        (u128::from(input) * self.l as u128).div_ceil(self.m as u128) as u64
    }

    /// Input frames needed for `output` output frames (and the lookahead after them).
    pub fn input_frames(&self, output: u64) -> u64 {
        (u128::from(output) * self.m as u128).div_ceil(self.l as u128) as u64 + self.taps as u64
    }

    /// Passband and stopband edges, Hz.
    pub fn edges(&self) -> (f64, f64) {
        let lower = f64::from(self.from.min(self.to));
        (PASSBAND * lower, STOPBAND * lower)
    }
}

/// `sum c[j] x[j]` for both channels of interleaved stereo `x`, in f64, four partial sums
/// a channel (so the loop vectorizes and pipelines; the order is fixed, so the result is
/// the same however the input arrived).
#[inline(always)]
fn dot(c: &[f64], x: &[f64]) -> [f64; 2] {
    debug_assert_eq!(x.len(), c.len() * 2);
    let mut a = [[0f64; 2]; 4];
    for (cc, xx) in c.as_chunks::<4>().0.iter().zip(x.as_chunks::<8>().0) {
        for u in 0..4 {
            a[u][0] += cc[u] * xx[2 * u];
            a[u][1] += cc[u] * xx[2 * u + 1];
        }
    }
    [(a[0][0] + a[1][0]) + (a[2][0] + a[3][0]), (a[0][1] + a[1][1]) + (a[2][1] + a[3][1])]
}

/// Input frames a push is expected to bring at most; the buffer is sized for it (larger
/// pushes work, growing it once).
pub const BLOCK: usize = 1024;

/// A streaming converter of interleaved stereo, one track (or a run of gapless tracks) long.
pub struct Resampler {
    f: Arc<Filter>,
    /// Interleaved stereo input, in f64: frames `start..start + buf.len() / 2`.
    buf: Vec<f64>,
    /// Absolute input frame of `buf[0]` (negative while the priming silence is in it).
    start: i64,
    /// The next output's time: input frame `i` plus `p / L`.
    i: i64,
    p: usize,
    /// Output frames given out.
    n: u64,
    /// Input frames pushed.
    pushed: u64,
    /// No more input: `pushed` is the end; the output ends at `output_frames(pushed)`.
    finished: bool,
}

impl Resampler {
    pub fn new(f: Arc<Filter>) -> Self {
        let h = f.lookahead();
        let mut buf = Vec::with_capacity((f.taps + BLOCK + h) * 2);
        // Silence before the first frame: the window of output 0 reaches h - 1 frames back.
        let prime = if f.bypass { 0 } else { h - 1 };
        buf.resize(prime * 2, 0.0);
        Resampler { f, buf, start: -(prime as i64), i: 0, p: 0, n: 0, pushed: 0, finished: false }
    }

    pub fn filter(&self) -> &Arc<Filter> {
        &self.f
    }

    /// Output frames given out so far.
    pub fn produced(&self) -> u64 {
        self.n
    }

    /// Input frames pushed so far.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The output ends here (once `finish` was called).
    pub fn output_end(&self) -> Option<u64> {
        self.finished.then(|| self.f.output_frames(self.pushed))
    }

    /// Every output frame has been given out.
    pub fn is_done(&self) -> bool {
        self.output_end().is_some_and(|e| self.n >= e)
    }

    /// The first input frame the next output reads.
    fn first_needed(&self) -> i64 {
        if self.f.bypass { self.i } else { self.i - self.f.lookahead() as i64 + 1 }
    }

    /// Append interleaved stereo input frames.
    pub fn push(&mut self, frames: &[f32]) {
        debug_assert!(!self.finished, "push after finish");
        // Drop what no output will read again, then append (a move within the buffer: no
        // allocation while pushes stay within `BLOCK`).
        let drop = (self.first_needed() - self.start).clamp(0, (self.buf.len() / 2) as i64) as usize;
        if drop > 0 && self.buf.len() + frames.len() > self.buf.capacity() {
            self.buf.drain(..drop * 2);
            self.start += drop as i64;
        }
        self.buf.extend(frames.iter().map(|&s| f64::from(s)));
        self.pushed += (frames.len() / 2) as u64;
    }

    /// No more input: what follows the last frame is silence.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        // The lookahead of the last outputs: `h` frames of silence after the end.
        let pad = if self.f.bypass { 0 } else { self.f.lookahead() };
        let drop = (self.first_needed() - self.start).clamp(0, (self.buf.len() / 2) as i64) as usize;
        if drop > 0 && self.buf.len() + pad * 2 > self.buf.capacity() {
            self.buf.drain(..drop * 2);
            self.start += drop as i64;
        }
        self.buf.resize(self.buf.len() + pad * 2, 0.0);
    }

    /// The next output needs input not pushed yet.
    pub fn needs_input(&self) -> bool {
        if self.finished {
            return false;
        }
        let last = if self.f.bypass { self.i } else { self.i + self.f.lookahead() as i64 };
        last >= self.start + (self.buf.len() / 2) as i64
    }

    /// Append up to `max` output frames to `out` (interleaved stereo f32), as many as the
    /// input pushed allows. Returns how many.
    pub fn produce(&mut self, out: &mut Vec<f32>, max: usize) -> usize {
        let end = self.output_end().unwrap_or(u64::MAX);
        let mut made = 0;
        while made < max && self.n < end && !self.needs_input() {
            let at = (self.first_needed() - self.start) as usize;
            if self.f.bypass {
                // f32 -> f64 -> f32 is exact (signed zeros included).
                out.push(self.buf[at * 2] as f32);
                out.push(self.buf[at * 2 + 1] as f32);
                self.i += 1;
            } else {
                let t = self.f.taps;
                let c = &self.f.coefs[self.p * t..(self.p + 1) * t];
                let [l, r] = dot(c, &self.buf[at * 2..(at + t) * 2]);
                out.push(l as f32);
                out.push(r as f32);
                self.p += self.f.m;
                self.i += (self.p / self.f.l) as i64;
                self.p %= self.f.l;
            }
            self.n += 1;
            made += 1;
        }
        made
    }

    /// Convert a whole signal at once (tests and tools).
    pub fn convert(f: Arc<Filter>, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(f);
        let mut out = Vec::with_capacity((r.f.output_frames((input.len() / 2) as u64) * 2) as usize);
        for block in input.chunks(BLOCK * 2) {
            r.push(block);
            r.produce(&mut out, usize::MAX);
        }
        r.finish();
        r.produce(&mut out, usize::MAX);
        out
    }
}

/// The filters built lately, by pair, so a crossfade between the same two rates does not
/// build its table again.
#[derive(Default)]
pub struct Filters {
    kept: Vec<Arc<Filter>>,
}

impl Filters {
    /// The filter for `from` to `to`, built if need be; `None` when there can be none.
    pub fn get(&mut self, from: u32, to: u32) -> Option<Arc<Filter>> {
        if let Some(f) = self.kept.iter().find(|f| f.from == from && f.to == to) {
            return Some(f.clone());
        }
        let f = Arc::new(Filter::new(from, to)?);
        if self.kept.len() >= 4 {
            self.kept.remove(0);
        }
        self.kept.push(f.clone());
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_lengths_and_tables() {
        let f = Filter::new(44_100, 48_000).unwrap();
        assert_eq!(f.ratio(), (160, 147));
        assert_eq!(f.taps() % 4, 0);
        assert_eq!(f.output_frames(44_100), 48_000);
        assert_eq!(f.output_frames(1), 2, "ceil(160 / 147)");
        assert_eq!(Filter::new(48_000, 44_100).unwrap().output_frames(48_000), 44_100);
        assert_eq!(Filter::new(44_100, 192_000).unwrap().ratio(), (640, 147));
        assert!(Filter::new(44_100, 44_101).is_none(), "a 44 101-phase table is not built");
        assert!(Filter::new(44_100, 768_000).is_some(), "44.1 to 768 kHz is");
        assert!(Filter::new(0, 48_000).is_none());
        assert!(Filter::new(48_000, 48_000).unwrap().is_bypass());
        let (pass, stop) = f.edges();
        assert!((pass - 19_999.35).abs() < 1.0 && (stop - 22_050.0).abs() < 1e-9);
        // Every phase passes DC at exactly unity.
        for p in 0..160 {
            let s: f64 = f.coefs[p * f.taps..(p + 1) * f.taps].iter().sum();
            assert!((s - 1.0).abs() < 1e-13);
        }
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_input_was_split() {
        let f = Arc::new(Filter::new(44_100, 48_000).unwrap());
        let input: Vec<f32> = (0..20_000).map(|i| ((i as f32) * 0.0137).sin() * 0.7).collect();
        let whole = Resampler::convert(f.clone(), &input);
        let mut r = Resampler::new(f);
        let mut out = Vec::new();
        let mut at = 0;
        let mut k = 1;
        while at < input.len() {
            let n = ((k * 37) % 900 + 1) * 2;
            let end = (at + n).min(input.len());
            r.push(&input[at..end]);
            at = end;
            k += 1;
            r.produce(&mut out, (k * 13) % 500);
        }
        r.finish();
        while !r.is_done() {
            r.produce(&mut out, 7);
        }
        assert_eq!(out.len(), whole.len());
        assert!(out.iter().zip(&whole).all(|(a, b)| a.to_bits() == b.to_bits()));
    }
}
