//! The equalizer: up to [`MAX_BANDS`] second-order bands (peaking, low and high shelves,
//! low-pass, high-pass, band-pass, notch, all-pass), an overall preamp, and the processor
//! the output thread runs them with.
//!
//! **Filters.** Each band is a biquad from Robert Bristow-Johnson's "Audio EQ Cookbook"
//! (the standard audio-EQ formulas, also what Equalizer APO, AutoEq and EasyEffects' "APO"
//! mode use): `A = 10^(gain/40)`, `w0 = 2 pi f0 / fs`, `alpha = sin(w0) / 2Q`; a shelf's
//! `f0` is its midpoint (half the gain in dB) and its `Q` the cookbook's shelf Q (0.707 is
//! the steepest slope without overshoot). A band's width is a Q, or (as Equalizer APO
//! writes them) a bandwidth in octaves (`alpha = sin(w0) sinh(ln 2 / 2 * BW * w0 / sin(w0))`,
//! the cookbook's digital bandwidth) or a shelf's slope `S` (`alpha = sin(w0) / 2 *
//! sqrt((A + 1/A)(1/S - 1) + 2)`; APO's "x dB per octave" is `S = x / 12`). The low-pass,
//! high-pass, band-pass (0 dB at its peak), notch and all-pass filters have no gain: their
//! level is fixed by their kind. The coefficients are worked out for the sample rate that
//! plays, whenever it changes, so a band is the same curve at 44.1, 48, 96 or 384 kHz (bar
//! the bilinear transform's squeeze right at Nyquist); a band whose frequency is above 45%
//! of the rate (a 22 kHz band at 44.1 kHz) is held there.
//!
//! **The curve** (`response_db`, what Settings draws and the auto preamp is checked
//! against) is worked out from each band's analog prototype at the bilinear transform's
//! warped frequency, `|H(j tan(w/2) / tan(w0/2))|`, which is exactly the digital filter's
//! response (the cookbook's filters are those prototypes through the bilinear transform
//! prewarped at `f0`; `tests` checks the two against each other) and, unlike evaluating the
//! coefficients, loses no precision where a high-pass's numerator vanishes near 0 Hz.
//!
//! **Numerics.** The filters run in f64, in direct form I with the feedforward and feedback
//! terms of each delay paired (`b1 x1 - a1 y1`). Direct form I keeps the signal itself as
//! its state, so a coefficient change acts on real history (no internal-state transient,
//! which direct form II has), and f64 keeps the poles exact where they crowd z = 1: a
//! 20 Hz band at 384 kHz has poles within 0.0003 of the unit circle, which f32 coefficients
//! would put measurably off. At 0 dB a peak or a shelf is exactly `b = a`, and the pairing
//! makes such a band pass its input through bit for bit (`b0 = 1`, each pair `0`).
//!
//! **Changes while playing.** A new setting is a target: the gain (dB), the frequency and
//! the width (on log scales) glide to it, recomputed every [`SUB`] frames with a one-pole
//! of [`GLIDE_SECS`], and the preamp is interpolated per sample, so a slider moving or a
//! preset changing never steps the signal. A filter without gain (a low-pass...) glides in
//! by its *mix*: `(1 - m) + m H(z)`, which is still a biquad with `H`'s poles (numerator
//! `(1 - m) a + m b`), the identity at `m = 0` exactly as a 0 dB peak is, and `H` itself at
//! `m = 1`. A band that changes kind (or how its width is given) glides to the identity
//! (0 dB, or mix 0), changes there and glides back; new bands start at the identity;
//! removed ones glide out. Turning the equalizer off glides everything to the identity and
//! then leaves the samples alone (`is_bypassed`): with it off the output is bit-identical
//! to no equalizer at all.

use std::f64::consts::{LN_2, PI};

pub mod apo;
pub mod fit;
pub mod formats;
pub mod presets;

/// Bands an equalizer can have. AutoEq's files have 10, Equalizer APO's headphone
/// corrections commonly 10 to 20, EasyEffects' equalizer up to 32. Measured on the output
/// thread (`tests/eq.rs`, `what_it_costs`): 32 bands at 192 kHz are about 2% of one core.
pub const MAX_BANDS: usize = 32;
pub const FREQ_MIN: f32 = 10.0;
pub const FREQ_MAX: f32 = 24_000.0;
pub const GAIN_MAX: f32 = 24.0;
pub const Q_MIN: f32 = 0.1;
/// Room EQ Wizard's Generic equaliser's largest Q (Equalizer APO's default notch is 30).
pub const Q_MAX: f32 = 50.0;
/// A bandwidth in octaves (Equalizer APO's `BW Oct`).
pub const OCT_MIN: f32 = 0.01;
pub const OCT_MAX: f32 = 8.0;
/// A shelf's slope `S` (Equalizer APO's dB per octave / 12): 1 is 12 dB/octave, the
/// steepest without overshoot.
pub const SLOPE_MIN: f32 = 0.05;
pub const SLOPE_MAX: f32 = 2.0;
pub const PREAMP_MIN: f32 = -30.0;
pub const PREAMP_MAX: f32 = 12.0;
/// Frames between coefficient updates while a band glides.
pub const SUB: usize = 32;
/// Time constant of the glide to a new setting.
pub const GLIDE_SECS: f64 = 0.02;
/// A band's frequency is held below this share of the sample rate.
const NYQUIST_SHARE: f64 = 0.45;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    /// Constant 0 dB peak gain (the cookbook's second band-pass), Equalizer APO's `BP`.
    BandPass,
    Notch,
    AllPass,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Peak, Kind::LowShelf, Kind::HighShelf, Kind::LowPass, Kind::HighPass, Kind::BandPass, Kind::Notch, Kind::AllPass];

    /// It has a gain (peak and shelves); the others' level is their kind's.
    pub fn has_gain(self) -> bool {
        matches!(self, Kind::Peak | Kind::LowShelf | Kind::HighShelf)
    }

    pub fn is_shelf(self) -> bool {
        matches!(self, Kind::LowShelf | Kind::HighShelf)
    }

    /// As Settings shows it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Peak => "Peak",
            Kind::LowShelf => "Low shelf",
            Kind::HighShelf => "High shelf",
            Kind::LowPass => "Low-pass",
            Kind::HighPass => "High-pass",
            Kind::BandPass => "Band-pass",
            Kind::Notch => "Notch",
            Kind::AllPass => "All-pass",
        }
    }

    pub fn index(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Kind {
        Kind::ALL.get(i).copied().unwrap_or_default()
    }
}

/// How a band's width (`Band::q`) is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Width {
    /// The cookbook's Q.
    #[default]
    Q,
    /// A bandwidth in octaves (not for shelves).
    Octaves,
    /// A shelf's slope S (shelves only).
    Slope,
}

impl Width {
    /// It can describe a band of `kind`.
    pub fn fits(self, kind: Kind) -> bool {
        match self {
            Width::Q => true,
            Width::Octaves => !kind.is_shelf(),
            Width::Slope => kind.is_shelf(),
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Width::Q => (Q_MIN, Q_MAX),
            Width::Octaves => (OCT_MIN, OCT_MAX),
            Width::Slope => (SLOPE_MIN, SLOPE_MAX),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub kind: Kind,
    /// Hz: a peak's centre, a shelf's midpoint, a pass filter's corner, a notch's centre.
    pub freq: f32,
    /// dB; always 0 for the kinds without a gain.
    pub gain_db: f32,
    /// The width, as `width` says: a Q, octaves, or a shelf's slope S.
    pub q: f32,
    pub width: Width,
}

impl Default for Band {
    fn default() -> Self {
        Self { kind: Kind::Peak, freq: 1000.0, gain_db: 0.0, q: std::f32::consts::SQRT_2, width: Width::Q }
    }
}

impl Band {
    /// A band whose width is a Q.
    pub fn new(kind: Kind, freq: f32, gain_db: f32, q: f32) -> Self {
        Self { kind, freq, gain_db, q, width: Width::Q }.clamped()
    }

    pub fn with_width(kind: Kind, freq: f32, gain_db: f32, value: f32, width: Width) -> Self {
        Self { kind, freq, gain_db, q: value, width }.clamped()
    }

    /// Within the ranges the equalizer takes (non-finite values become the defaults); no
    /// gain on a kind that has none; a width its kind cannot have becomes the equivalent Q.
    pub fn clamped(self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let freq = fin(self.freq, 1000.0).clamp(FREQ_MIN, FREQ_MAX);
        let gain_db = if self.kind.has_gain() { fin(self.gain_db, 0.0).clamp(-GAIN_MAX, GAIN_MAX) } else { 0.0 };
        let mut b = Self { kind: self.kind, freq, gain_db, q: self.q, width: self.width };
        if !b.width.fits(b.kind) {
            // As the band it was (a slope keeps its shape at this gain; a bandwidth is
            // taken at 48 kHz, where it is within a few hundredths of the Q at any rate
            // below 10 kHz).
            let q = Band { kind: if b.width == Width::Slope { Kind::LowShelf } else { Kind::Peak }, ..b }.q_equivalent(48_000);
            b.q = q as f32;
            b.width = Width::Q;
        }
        let (lo, hi) = b.width.range();
        b.q = fin(b.q, 1.0).clamp(lo, hi);
        b
    }

    /// The Q this band has at `rate`: `sin(w0) / 2 alpha`.
    pub fn q_equivalent(&self, rate: u32) -> f64 {
        let p = Proto::of(self.kind, f64::from(self.freq), f64::from(self.gain_db), f64::from(self.q), self.width, rate);
        p.q()
    }

    /// |H|^2 of this band at `freq` Hz, from its analog prototype at the warped frequency:
    /// the digital filter's exact response, without the cancellation of evaluating its
    /// coefficients far below a high-pass's corner.
    pub fn power_at(&self, rate: u32, freq: f64) -> f64 {
        Proto::of(self.kind, f64::from(self.freq), f64::from(self.gain_db), f64::from(self.q), self.width, rate).power_at(freq)
    }
}

/// How loud the equalizer's input is made before the bands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preamp {
    /// Down by the largest boost of the combined curve (`max_boost_db`), so no frequency
    /// comes out louder than it went in; 0 dB for a curve that only cuts.
    Auto,
    /// This many dB, whatever the curve.
    Manual(f32),
}

/// What the equalizer does: `Player::set_eq`. Fixed size, so the output thread copies it
/// without allocating.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqParams {
    pub enabled: bool,
    bands: [Band; MAX_BANDS],
    count: usize,
    pub preamp: Preamp,
}

impl Default for EqParams {
    fn default() -> Self {
        Self { enabled: false, bands: [Band::default(); MAX_BANDS], count: 0, preamp: Preamp::Auto }
    }
}

impl EqParams {
    /// The first `MAX_BANDS` of `bands`, clamped to the ranges. (Importers refuse more,
    /// with the count; nothing reaching here is cut short silently.)
    pub fn new(enabled: bool, bands: &[Band], preamp: Preamp) -> Self {
        let mut p = Self { enabled, preamp, ..Self::default() };
        for (slot, b) in p.bands.iter_mut().zip(bands) {
            *slot = b.clamped();
        }
        p.count = bands.len().min(MAX_BANDS);
        if let Preamp::Manual(db) = p.preamp {
            p.preamp = Preamp::Manual(if db.is_finite() { db.clamp(PREAMP_MIN, PREAMP_MAX) } else { 0.0 });
        }
        p
    }

    pub fn bands(&self) -> &[Band] {
        &self.bands[..self.count]
    }

    /// The preamp in dB at this rate (the auto one depends on the curve).
    pub fn preamp_db(&self, rate: u32) -> f64 {
        match self.preamp {
            Preamp::Auto => -max_boost_db(self.bands(), rate),
            Preamp::Manual(db) => f64::from(db),
        }
    }

    /// Anything to do: on, and a band that changes the signal (a peak or shelf away from
    /// 0 dB, any filter without a gain: even an all-pass changes the phase) or the preamp
    /// away from 0 dB.
    pub fn changes_sound(&self) -> bool {
        self.enabled
            && (self.bands().iter().any(|b| !b.kind.has_gain() || b.gain_db != 0.0) || matches!(self.preamp, Preamp::Manual(db) if db != 0.0))
    }
}

/// A band's design: its kind, `w0`, `alpha` and `A` at one rate (frequency held below
/// Nyquist), shared by the coefficients and the analytic response.
#[derive(Clone, Copy, Debug)]
struct Proto {
    kind: Kind,
    fs: f64,
    w0: f64,
    sin: f64,
    cos: f64,
    alpha: f64,
    a: f64,
}

impl Proto {
    fn of(kind: Kind, freq: f64, gain_db: f64, value: f64, width: Width, rate: u32) -> Proto {
        // The same operations in the same order as before the other kinds and widths
        // existed, so a peak or shelf with a Q has exactly the coefficients it had.
        let fs = f64::from(rate.max(1));
        let f0 = freq.clamp(f64::from(FREQ_MIN), fs * NYQUIST_SHARE);
        let gain_db = if kind.has_gain() { gain_db } else { 0.0 };
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = match width {
            Width::Q => sin / (2.0 * value.max(f64::from(Q_MIN))),
            Width::Octaves => sin * (LN_2 / 2.0 * value.max(f64::from(OCT_MIN)) * w0 / sin).sinh(),
            Width::Slope => {
                let s = value.max(f64::from(SLOPE_MIN));
                // Past the steepest slope this gain allows the root would go negative
                // (Equalizer APO's filter then fails); held where the shelf's Q is the
                // largest the equalizer takes, so the poles stay clear of the unit circle.
                let q_max = f64::from(Q_MAX);
                sin / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(1.0 / (q_max * q_max)).sqrt()
            }
        };
        Proto { kind, fs, w0, sin, cos, alpha, a }
    }

    fn q(&self) -> f64 {
        self.sin / (2.0 * self.alpha)
    }

    fn coefs(&self) -> Coefs {
        let Proto { sin: _, cos, alpha, a, .. } = *self;
        // 1 - cos(w0), without the cancellation of the subtraction at low frequencies.
        let omc = {
            let s = (self.w0 / 2.0).sin();
            2.0 * s * s
        };
        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            Kind::Peak => (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a),
            Kind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            Kind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
            Kind::LowPass => (omc / 2.0, omc, omc / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Kind::HighPass => {
                let opc = 2.0 - omc;
                (opc / 2.0, -opc, opc / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            Kind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Kind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Kind::AllPass => (1.0 - alpha, -2.0 * cos, 1.0 + alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        };
        Coefs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// |H|^2 at `freq` Hz: the analog prototype at `Omega = tan(w / 2) / tan(w0 / 2)`.
    fn power_at(&self, freq: f64) -> f64 {
        let k = (self.w0 / 2.0).tan();
        let om = (PI * freq / self.fs).tan() / k;
        let o2 = om * om;
        let q = self.q();
        let re = 1.0 - o2;
        let im = om / q;
        let den = re * re + im * im;
        let a = self.a;
        let p = match self.kind {
            Kind::LowPass => 1.0 / den,
            Kind::HighPass => o2 * o2 / den,
            Kind::BandPass => im * im / den,
            Kind::Notch => re * re / den,
            Kind::AllPass => 1.0,
            Kind::Peak => {
                let (n, d) = (im * a, im / a);
                (re * re + n * n) / (re * re + d * d)
            }
            Kind::LowShelf => {
                let x = om * a.sqrt() / q;
                a * a * ((a - o2).powi(2) + x * x) / ((1.0 - a * o2).powi(2) + x * x)
            }
            Kind::HighShelf => {
                let x = om * a.sqrt() / q;
                a * a * ((1.0 - a * o2).powi(2) + x * x) / ((a - o2).powi(2) + x * x)
            }
        };
        if p.is_finite() { p } else { 0.0 }
    }
}

/// One biquad, normalised (`a0 = 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// The cookbook's coefficients for a band whose width is a Q, at a sample rate.
    pub fn new(kind: Kind, freq: f64, gain_db: f64, q: f64, rate: u32) -> Coefs {
        Proto::of(kind, freq, gain_db, q, Width::Q, rate).coefs()
    }

    /// The cookbook's coefficients for a band whose width is given as `width` says.
    pub fn design(kind: Kind, freq: f64, gain_db: f64, value: f64, width: Width, rate: u32) -> Coefs {
        Proto::of(kind, freq, gain_db, value, width, rate).coefs()
    }

    pub fn for_band(b: &Band, rate: u32) -> Coefs {
        Coefs::design(b.kind, f64::from(b.freq), f64::from(b.gain_db), f64::from(b.q), b.width, rate)
    }

    /// `(1 - m) + m H`: the same poles, the numerator `(1 - m) a + m b`; the identity at
    /// `m = 0` (bit for bit, with the paired terms) and `self` at `m = 1`.
    pub fn mixed(self, m: f64) -> Coefs {
        if m == 1.0 {
            return self;
        }
        let d = 1.0 - m;
        Coefs { b0: d + m * self.b0, b1: d * self.a1 + m * self.b1, b2: d * self.a2 + m * self.b2, a1: self.a1, a2: self.a2 }
    }

    /// |H| squared at angular frequency `w`, from `phi = 4 sin^2(w / 2)`. In this form
    /// (the cookbook's) nothing large cancels at low frequencies for the filters with a
    /// gain, where both polynomials are near zero: a 20 Hz shelf at 384 kHz comes out
    /// exact to 1e-9 dB.
    fn power(&self, phi: f64) -> f64 {
        let Coefs { b0, b1, b2, a1, a2 } = *self;
        let (sb, sa) = (b0 + b1 + b2, 1.0 + a1 + a2);
        let num = sb * sb - phi * (b1 * (b0 + b2) + 4.0 * b0 * b2) + phi * phi * b0 * b2;
        let den = sa * sa - phi * (a1 * (1.0 + a2) + 4.0 * a2) + phi * phi * a2;
        num / den
    }

    /// The gain in dB at `freq` Hz, from the coefficients.
    pub fn gain_db_at(&self, freq: f64, rate: u32) -> f64 {
        let w = 2.0 * PI * freq / f64::from(rate.max(1));
        10.0 * self.power(phi(w)).max(1e-30).log10()
    }
}

fn phi(w: f64) -> f64 {
    let s = (w / 2.0).sin();
    4.0 * s * s
}

/// The combined curve of `bands` in dB at `freq` Hz (without the preamp).
pub fn response_db(bands: &[Band], rate: u32, freq: f64) -> f64 {
    bands.iter().map(|b| 10.0 * b.power_at(rate, freq).max(1e-30).log10()).sum()
}

/// The combined curve at many frequencies at once (`freqs` in Hz, results in dB).
pub fn response_curve(bands: &[Band], rate: u32, freqs: &[f64], out: &mut Vec<f64>) {
    let protos: Vec<Proto> = bands.iter().map(|b| Proto::of(b.kind, f64::from(b.freq), f64::from(b.gain_db), f64::from(b.q), b.width, rate)).collect();
    out.clear();
    out.extend(freqs.iter().map(|&f| protos.iter().map(|p| 10.0 * p.power_at(f).max(1e-30).log10()).sum::<f64>()));
}

/// Points per octave of the search for the curve's largest boost.
const SEARCH_PER_OCTAVE: f64 = 48.0;

/// The band can never make any frequency louder: a cut, a notch, the 0 dB band-pass, an
/// all-pass, a low- or high-pass without a resonance (Q at most 1/sqrt(2)).
fn never_boosts(b: &Band) -> bool {
    match b.kind {
        Kind::Peak | Kind::LowShelf | Kind::HighShelf => b.gain_db <= 0.0,
        Kind::BandPass | Kind::Notch | Kind::AllPass => true,
        Kind::LowPass | Kind::HighPass => b.width == Width::Q && b.q <= std::f32::consts::FRAC_1_SQRT_2,
    }
}

/// The largest boost of the combined curve between 10 Hz and the top of the audible band
/// (20 kHz, or just under Nyquist), in dB; 0 when it only cuts. Evaluated every 1/48
/// octave and at each band's own frequency, then refined around the highest point, so it is
/// within a few thousandths of a dB of the true maximum. Allocation-free (the output thread
/// works it out when a setting arrives). A resonant low- or high-pass's peak counts as a
/// boost like any other.
pub fn max_boost_db(bands: &[Band], rate: u32) -> f64 {
    if bands.iter().all(never_boosts) {
        return 0.0;
    }
    let mut coefs = [Coefs::IDENTITY; MAX_BANDS];
    let n = bands.len().min(MAX_BANDS);
    for (c, b) in coefs.iter_mut().zip(bands) {
        *c = Coefs::for_band(b, rate);
    }
    let fs = f64::from(rate.max(1));
    let at = |f: f64| -> f64 {
        let w = 2.0 * PI * f / fs;
        let p = phi(w);
        coefs[..n].iter().map(|c| c.power(p)).product::<f64>()
    };
    let top = (fs * 0.4999).min(20_000.0);
    let lo = f64::from(FREQ_MIN);
    let octaves = (top / lo).log2();
    let steps = (octaves * SEARCH_PER_OCTAVE).ceil() as usize;
    let mut best = (0.0f64, 1.0f64); // (frequency, power)
    let consider = |f: f64, best: &mut (f64, f64)| {
        let p = at(f);
        if p > best.1 {
            *best = (f, p);
        }
    };
    for i in 0..=steps {
        consider(lo * 2f64.powf(i as f64 / SEARCH_PER_OCTAVE), &mut best);
    }
    for b in &bands[..n] {
        let f = f64::from(b.freq);
        if f > lo && f < top {
            consider(f, &mut best);
        }
    }
    if best.0 > 0.0 {
        // Golden-section search within one grid step either side.
        let step = 2f64.powf(1.0 / SEARCH_PER_OCTAVE);
        let (mut a, mut b) = ((best.0 / step).max(lo).ln(), (best.0 * step).min(top).ln());
        let g = (5f64.sqrt() - 1.0) / 2.0;
        for _ in 0..40 {
            let c = b - g * (b - a);
            let d = a + g * (b - a);
            if at(c.exp()) > at(d.exp()) {
                b = d;
            } else {
                a = c;
            }
        }
        consider(((a + b) / 2.0).exp(), &mut best);
    }
    (10.0 * best.1.log10()).max(0.0)
}

/// One band's processing state: where it is now (it glides to the target), its
/// coefficients, and the last two inputs and outputs of each channel.
#[derive(Clone, Copy, Debug)]
struct Slot {
    active: bool,
    kind: Kind,
    width: Width,
    /// How far from the identity: the gain in dB for a peak or shelf, the mix (0 to 1)
    /// for a filter without a gain. 0 is the identity for every kind.
    level: f64,
    /// log2 of the frequency, ln of the width.
    lf: f64,
    lq: f64,
    coefs: Coefs,
    /// Per channel: x1, x2, y1, y2.
    st: [[f64; 4]; 2],
}

impl Default for Slot {
    fn default() -> Self {
        Self { active: false, kind: Kind::Peak, width: Width::Q, level: 0.0, lf: 10.0, lq: 0.0, coefs: Coefs::IDENTITY, st: [[0.0; 4]; 2] }
    }
}

impl Slot {
    /// At the identity with its outputs equal to its inputs: it passes samples through
    /// unchanged.
    fn transparent(&self) -> bool {
        self.level == 0.0 && self.st.iter().all(|s| s[0] == s[2] && s[1] == s[3])
    }
}

/// A slot's coefficients for where it is now.
fn slot_coefs(kind: Kind, width: Width, level: f64, lf: f64, lq: f64, rate: u32) -> Coefs {
    if kind.has_gain() {
        Coefs::design(kind, lf.exp2(), level, lq.exp(), width, rate)
    } else {
        Coefs::design(kind, lf.exp2(), 0.0, lq.exp(), width, rate).mixed(level)
    }
}

/// The equalizer as the output thread runs it. `process` works in place on interleaved
/// stereo; nothing allocates after `new`.
#[derive(Clone, Debug)]
pub struct Equalizer {
    rate: u32,
    target: EqParams,
    slots: [Slot; MAX_BANDS],
    /// The preamp (linear) now and where it glides.
    pre: f64,
    pre_target_db: f64,
    pre_db: f64,
    /// The cascade's last two input frames (after the preamp), per channel.
    hist: [[f64; 2]; 2],
    /// Something still moves toward the target.
    gliding: bool,
    /// Nothing has gone through since the filters were emptied: a new setting applies at
    /// once (there is no sound to glide under).
    fresh: bool,
    buf: [f64; SUB * 2],
}

impl Default for Equalizer {
    fn default() -> Self {
        Self::new(48_000)
    }
}

/// Where a slot glides: (kind, width, level, log2 f, ln width, wanted).
type SlotTarget = (Kind, Width, f64, f64, f64, bool);

impl Equalizer {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            target: EqParams::default(),
            slots: [Slot::default(); MAX_BANDS],
            pre: 1.0,
            pre_target_db: 0.0,
            pre_db: 0.0,
            hist: [[0.0; 2]; 2],
            gliding: false,
            fresh: true,
            buf: [0.0; SUB * 2],
        }
    }

    /// The equalizer at its target at once, no glide (tests, and a measurement's start).
    pub fn settled(params: &EqParams, rate: u32) -> Self {
        let mut e = Self::new(rate);
        e.set_params(params);
        e.jump();
        e
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn params(&self) -> &EqParams {
        &self.target
    }

    /// A new setting: everything glides there from where it is.
    pub fn set_params(&mut self, p: &EqParams) {
        self.target = *p;
        self.pre_target_db = if p.enabled { p.preamp_db(self.rate) } else { 0.0 };
        self.gliding = true;
        if self.fresh {
            self.jump();
        }
    }

    /// The device opened at another rate (between tracks, after a drain): the bands are
    /// worked out for it and the filters start empty.
    pub fn set_rate(&mut self, rate: u32) {
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        self.pre_target_db = if self.target.enabled { self.target.preamp_db(rate) } else { 0.0 };
        self.reset();
    }

    /// The stream restarts elsewhere (a seek, play-now): empty filters, so nothing of the
    /// old position rings into the new one.
    pub fn reset(&mut self) {
        for s in &mut self.slots {
            s.st = [[0.0; 4]; 2];
        }
        self.hist = [[0.0; 2]; 2];
        // Empty filters: the setting applies as it is, no glide.
        self.jump();
        self.fresh = true;
    }

    /// Off, and everything back at the identity: `process` leaves the samples alone.
    pub fn is_bypassed(&self) -> bool {
        !self.target.enabled && !self.gliding && self.pre == 1.0 && self.slots.iter().all(|s| !s.active)
    }

    /// The target the glide goes to for slot `i`.
    fn slot_target(&self, i: usize) -> SlotTarget {
        let t = &self.target;
        if i < t.count {
            let b = &t.bands[i];
            let level = match (t.enabled, b.kind.has_gain()) {
                (false, _) => 0.0,
                (true, true) => f64::from(b.gain_db),
                (true, false) => 1.0,
            };
            (b.kind, b.width, level, f64::from(b.freq).log2(), f64::from(b.q).ln(), t.enabled)
        } else {
            let s = &self.slots[i];
            (s.kind, s.width, 0.0, s.lf, s.lq, false)
        }
    }

    /// Everything at its target now (no glide).
    fn jump(&mut self) {
        for i in 0..MAX_BANDS {
            let (kind, width, level, lf, lq, wanted) = self.slot_target(i);
            let s = &mut self.slots[i];
            s.kind = kind;
            s.width = width;
            s.level = level;
            s.lf = lf;
            s.lq = lq;
            s.active = wanted;
            s.coefs = slot_coefs(kind, width, level, lf, lq, self.rate);
        }
        self.pre_db = self.pre_target_db;
        self.pre = 10f64.powf(self.pre_db / 20.0);
        self.gliding = false;
    }

    /// One glide step (every `SUB` frames while gliding).
    fn step(&mut self) {
        let k = 1.0 - (-(SUB as f64) / (GLIDE_SECS * f64::from(self.rate.max(1)))).exp();
        let mut moving = false;
        let mut prev_out: Option<[[f64; 2]; 2]> = None;
        for i in 0..MAX_BANDS {
            let (kind, width, tl, tlf, tlq, wanted) = self.slot_target(i);
            let hist = prev_out.unwrap_or(self.hist);
            let rate = self.rate;
            let s = &mut self.slots[i];
            if !s.active {
                if wanted {
                    // A new band: in at the identity, its state the input it would have
                    // had, so it starts as the identity it is there.
                    *s = Slot { active: true, kind, width, level: 0.0, lf: tlf, lq: tlq, coefs: Coefs::IDENTITY, st: [[0.0; 4]; 2] };
                    for (c, st) in s.st.iter_mut().enumerate() {
                        *st = [hist[c][0], hist[c][1], hist[c][0], hist[c][1]];
                    }
                    s.coefs = slot_coefs(kind, width, 0.0, tlf, tlq, rate);
                    moving = true;
                } else {
                    continue;
                }
            }
            // Changing kind (or how the width is given): to the identity first, where
            // every kind is the same.
            let tl_now = if s.kind != kind || s.width != width { 0.0 } else { tl };
            let before = (s.level, s.lf, s.lq, s.kind, s.width);
            if s.level != tl_now {
                s.level += (tl_now - s.level) * k;
                // Within 0.002 dB of a gain, or 0.0005 of a mix (the rest of the way is
                // 66 dB below the filter's own effect): there.
                let near = if s.kind.has_gain() { 0.002 } else { 0.0005 };
                if (tl_now - s.level).abs() < near {
                    s.level = tl_now;
                }
            }
            if s.level == 0.0 && (s.kind != kind || s.width != width || s.lf != tlf || s.lq != tlq) {
                // At the identity the band is the identity whatever its kind, frequency and
                // width.
                s.kind = kind;
                s.width = width;
                s.lf = tlf;
                s.lq = tlq;
            } else {
                if s.lf != tlf {
                    s.lf += (tlf - s.lf) * k;
                    if (tlf - s.lf).abs() < 0.0005 {
                        s.lf = tlf;
                    }
                }
                if s.lq != tlq {
                    s.lq += (tlq - s.lq) * k;
                    if (tlq - s.lq).abs() < 0.0005 {
                        s.lq = tlq;
                    }
                }
            }
            if (s.level, s.lf, s.lq, s.kind, s.width) != before {
                s.coefs = slot_coefs(s.kind, s.width, s.level, s.lf, s.lq, rate);
            }
            if s.level != tl || s.kind != kind || s.width != width || s.lf != tlf || s.lq != tlq {
                moving = true;
            }
            if s.level == 0.0 {
                // A band at the identity still rings out what it did before; once that is
                // far below anything audible, its outputs are its inputs again (and it can
                // go).
                let settled = s.st.iter().all(|c| (c[0] - c[2]).abs() < 1e-9 && (c[1] - c[3]).abs() < 1e-9);
                if settled {
                    for c in &mut s.st {
                        c[2] = c[0];
                        c[3] = c[1];
                    }
                    if !wanted {
                        s.active = false;
                    }
                } else {
                    moving = true;
                }
            }
            if s.active {
                prev_out = Some([[s.st[0][2], s.st[0][3]], [s.st[1][2], s.st[1][3]]]);
            }
        }
        if self.pre_db != self.pre_target_db {
            self.pre_db += (self.pre_target_db - self.pre_db) * k;
            if (self.pre_target_db - self.pre_db).abs() < 0.002 {
                self.pre_db = self.pre_target_db;
            }
            moving = true;
        }
        self.gliding = moving;
    }

    /// Equalize interleaved stereo in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.is_bypassed() {
            return;
        }
        self.fresh = false;
        for block in samples.chunks_mut(SUB * 2) {
            self.process_block(block);
        }
    }

    fn process_block(&mut self, block: &mut [f32]) {
        let frames = block.len() / 2;
        let from = self.pre;
        if self.gliding {
            self.step();
        }
        let to = if self.pre_db == 0.0 { 1.0 } else { 10f64.powf(self.pre_db / 20.0) };
        self.pre = to;
        let all_transparent = self.slots.iter().all(|s| !s.active || s.transparent());
        if all_transparent && from == 1.0 && to == 1.0 {
            // Nothing changes the samples: keep the history (so a band that moves next
            // starts from the real signal) and leave them as they are.
            if frames >= 2 {
                for c in 0..2 {
                    let h = [f64::from(block[(frames - 1) * 2 + c]), f64::from(block[(frames - 2) * 2 + c])];
                    self.hist[c] = h;
                    for s in self.slots.iter_mut().filter(|s| s.active) {
                        s.st[c] = [h[0], h[1], h[0], h[1]];
                    }
                }
            } else if frames == 1 {
                for (c, &x) in block.iter().enumerate().take(2) {
                    let x = f64::from(x);
                    self.hist[c] = [x, self.hist[c][0]];
                    for s in self.slots.iter_mut().filter(|s| s.active) {
                        s.st[c] = [x, s.st[c][0], x, s.st[c][0]];
                    }
                }
            }
            return;
        }
        let buf = &mut self.buf[..frames * 2];
        if from == to {
            for (b, &x) in buf.iter_mut().zip(block.iter()) {
                *b = f64::from(x) * to;
            }
        } else {
            for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let g = from + (to - from) * (i + 1) as f64 / frames as f64;
                f[0] = f64::from(block[i * 2]) * g;
                f[1] = f64::from(block[i * 2 + 1]) * g;
            }
        }
        if frames >= 2 {
            for c in 0..2 {
                self.hist[c] = [buf[(frames - 1) * 2 + c], buf[(frames - 2) * 2 + c]];
            }
        } else if frames == 1 {
            for (h, &x) in self.hist.iter_mut().zip(buf.iter()) {
                *h = [x, h[0]];
            }
        }
        for s in self.slots.iter_mut().filter(|s| s.active) {
            let Coefs { b0, b1, b2, a1, a2 } = s.coefs;
            for (c, st) in s.st.iter_mut().enumerate() {
                let [mut x1, mut x2, mut y1, mut y2] = *st;
                for i in (c..frames * 2).step_by(2) {
                    let x = buf[i];
                    // Each delay's feedforward and feedback paired: exactly x at 0 dB.
                    let y = b0 * x + (b1 * x1 - a1 * y1) + (b2 * x2 - a2 * y2);
                    x2 = x1;
                    x1 = x;
                    y2 = y1;
                    y1 = y;
                    buf[i] = y;
                }
                // No denormals in silence.
                for v in [&mut x1, &mut x2, &mut y1, &mut y2] {
                    if v.abs() < 1e-30 {
                        *v = 0.0;
                    }
                }
                *st = [x1, x2, y1, y2];
            }
        }
        for (o, &y) in block.iter_mut().zip(buf.iter()) {
            *o = y as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, frames: usize, amp: f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let s = (amp * (2.0 * PI * freq * n as f64 / f64::from(rate)).sin()) as f32;
                [s, s]
            })
            .collect()
    }

    fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / v.len() as f64).sqrt()
    }

    #[test]
    fn the_cookbook_curves_hit_their_gains() {
        for rate in [44_100, 48_000, 96_000, 192_000, 384_000] {
            let p = Coefs::new(Kind::Peak, 1000.0, 6.0, 1.0, rate);
            assert!((p.gain_db_at(1000.0, rate) - 6.0).abs() < 1e-9, "{rate}");
            let ls = Coefs::new(Kind::LowShelf, 100.0, -8.0, 0.707, rate);
            assert!((ls.gain_db_at(100.0, rate) + 4.0).abs() < 1e-6, "a shelf is half its gain at its midpoint");
            assert!((ls.gain_db_at(10.0, rate) + 8.0).abs() < 0.2);
            let hs = Coefs::new(Kind::HighShelf, 8000.0, 4.0, 0.707, rate);
            assert!((hs.gain_db_at(8000.0, rate) - 2.0).abs() < 1e-6);
            assert!(hs.gain_db_at(100.0, rate).abs() < 0.05);
            // A Butterworth low- and high-pass are 3 dB down at their corner, the band-pass
            // 0 dB at its centre, the notch nothing there, the all-pass 0 dB everywhere.
            let fc = 1000.0;
            let lp = Coefs::new(Kind::LowPass, fc, 0.0, std::f64::consts::FRAC_1_SQRT_2, rate);
            assert!((lp.gain_db_at(fc, rate) + 10.0 * 2f64.log10()).abs() < 1e-9, "{rate}");
            let hp = Coefs::new(Kind::HighPass, fc, 0.0, std::f64::consts::FRAC_1_SQRT_2, rate);
            assert!((hp.gain_db_at(fc, rate) + 10.0 * 2f64.log10()).abs() < 1e-9);
            assert!(Coefs::new(Kind::BandPass, fc, 0.0, 2.0, rate).gain_db_at(fc, rate).abs() < 1e-9);
            assert!(Coefs::new(Kind::Notch, fc, 0.0, 2.0, rate).gain_db_at(fc, rate) < -100.0);
            assert!(Band::new(Kind::Notch, fc as f32, 0.0, 2.0).power_at(rate, fc) < 1e-20);
            for f in [20.0, 300.0, 1000.0, 7000.0] {
                assert!(Coefs::new(Kind::AllPass, fc, 0.0, 0.7, rate).gain_db_at(f, rate).abs() < 1e-9);
            }
        }
    }

    /// Every kind and width: the coefficients' response and the analog prototype at the
    /// warped frequency are the same curve (two derivations of the cookbook agreeing).
    #[test]
    fn the_coefficients_are_the_prototypes_through_the_bilinear_transform() {
        for rate in [44_100u32, 48_000, 96_000, 192_000, 384_000] {
            for kind in Kind::ALL {
                for (width, value) in [(Width::Q, 0.6), (Width::Q, 3.0), (Width::Octaves, 1.5), (Width::Slope, 0.9)] {
                    if !width.fits(kind) {
                        continue;
                    }
                    for (freq, gain) in [(20.0, -9.0), (440.0, 6.0), (6000.0, 3.0), (18_000.0, -4.0)] {
                        let band = Band::with_width(kind, freq, gain, value, width);
                        let c = Coefs::for_band(&band, rate);
                        // Away from where the coefficient form itself loses precision (deep
                        // in a stopband).
                        for f in [50.0, 200.0, 1000.0, 4000.0, 12_000.0] {
                            let from_coefs = c.gain_db_at(f, rate);
                            if from_coefs < -80.0 {
                                continue;
                            }
                            let analytic = 10.0 * band.power_at(rate, f).log10();
                            assert!((from_coefs - analytic).abs() < 1e-6, "{rate} {kind:?} {width:?} {freq} Hz at {f} Hz: {from_coefs} vs {analytic}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn widths_are_equalizer_apo_s() {
        // One octave at 1 kHz, 48 kHz: the cookbook's digital bandwidth, a hair below
        // than the analog Q, sqrt(2).
        let b = Band::with_width(Kind::Peak, 1000.0, 6.0, 1.0, Width::Octaves);
        let q = b.q_equivalent(48_000);
        assert!((q - std::f64::consts::SQRT_2).abs() < 0.01 && q != std::f64::consts::SQRT_2, "{q}");
        // A shelf's slope: S = 1 is Q 0.7071 at any gain (1/Q^2 = (A + 1/A) * 0 + 2).
        let s = Band::with_width(Kind::LowShelf, 200.0, 6.0, 1.0, Width::Slope);
        assert!((s.q_equivalent(48_000) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        // A slope steeper than its gain allows is held at Q 50, not a filter on the edge of
        // stability.
        let steep = Band::with_width(Kind::HighShelf, 2000.0, 24.0, 2.0, Width::Slope);
        assert!((steep.q_equivalent(48_000) - f64::from(Q_MAX)).abs() < 1e-6);
        let mut e = Equalizer::settled(&EqParams::new(true, &[steep], Preamp::Manual(-30.0)), 48_000);
        let mut s = sine(3000.0, 48_000, 48_000, 0.1);
        e.process(&mut s);
        assert!(s.iter().all(|x| x.is_finite() && x.abs() < 4.0));
        // A width a kind cannot have becomes its Q.
        let x = Band::with_width(Kind::Peak, 200.0, 6.0, 0.9, Width::Slope);
        assert_eq!(x.width, Width::Q);
        assert!(x.q > 0.0);
        // No gain on a filter that has none.
        assert_eq!(Band::new(Kind::LowPass, 5000.0, 6.0, 0.7).gain_db, 0.0);
    }

    #[test]
    fn zero_db_bands_pass_samples_through_bit_for_bit() {
        for kind in [Kind::Peak, Kind::LowShelf, Kind::HighShelf] {
            let c = Coefs::new(kind, 333.0, 0.0, 0.9, 44_100);
            assert_eq!(c.b0, 1.0);
            assert_eq!((c.b1, c.b2), (c.a1, c.a2), "{kind:?}");
        }
        for kind in [Kind::LowPass, Kind::HighPass, Kind::BandPass, Kind::Notch, Kind::AllPass] {
            let c = Coefs::new(kind, 333.0, 0.0, 0.9, 44_100).mixed(0.0);
            assert_eq!(c.b0, 1.0);
            assert_eq!((c.b1, c.b2), (c.a1, c.a2), "{kind:?} at mix 0");
        }
        let flat: Vec<Band> = [31.0, 63.0, 125.0, 250.0, 500.0, 1e3, 2e3, 4e3, 8e3, 16e3].iter().map(|&f| Band::new(Kind::Peak, f, 0.0, 1.41)).collect();
        let mut eq = Equalizer::settled(&EqParams::new(true, &flat, Preamp::Auto), 44_100);
        let input = sine(997.0, 44_100, 10_000, 0.7);
        let mut out = input.clone();
        eq.process(&mut out);
        assert!(out == input, "on and flat: untouched");
    }

    #[test]
    fn auto_preamp_is_the_largest_boost() {
        let b = [Band::new(Kind::Peak, 1000.0, 6.0, 1.0)];
        assert!((max_boost_db(&b, 48_000) - 6.0).abs() < 1e-3);
        // Two overlapping boosts sum to more than either.
        let b = [Band::new(Kind::Peak, 1000.0, 6.0, 1.0), Band::new(Kind::Peak, 1300.0, 6.0, 1.0)];
        let m = max_boost_db(&b, 48_000);
        assert!(m > 9.0, "{m}");
        assert!((m - (0..2000).map(|i| response_db(&b, 48_000, 800.0 + i as f64 * 0.5)).fold(f64::MIN, f64::max)).abs() < 0.01);
        assert_eq!(max_boost_db(&[Band::new(Kind::LowShelf, 100.0, -6.0, 0.7)], 48_000), 0.0);
        // A resonant low-pass boosts at its corner (Q 4: +12 dB); a Butterworth one, a
        // notch, a band-pass and an all-pass never do.
        let lp = [Band::new(Kind::LowPass, 2000.0, 0.0, 4.0)];
        let m = max_boost_db(&lp, 48_000);
        assert!((m - (0..4000).map(|i| response_db(&lp, 48_000, 1000.0 + i as f64 * 0.5)).fold(f64::MIN, f64::max)).abs() < 0.01, "{m}");
        assert!(m > 11.9, "{m}");
        let quiet = [Band::new(Kind::LowPass, 2000.0, 0.0, 0.7), Band::new(Kind::Notch, 50.0, 0.0, 10.0), Band::new(Kind::BandPass, 900.0, 0.0, 3.0), Band::new(Kind::AllPass, 300.0, 0.0, 0.7)];
        assert_eq!(max_boost_db(&quiet, 48_000), 0.0);
    }

    #[test]
    fn a_sine_comes_out_at_the_curve_s_gain() {
        for rate in [44_100u32, 48_000, 96_000, 192_000] {
            let bands = [Band::new(Kind::LowShelf, 105.0, 6.0, 0.7), Band::new(Kind::Peak, 3000.0, -4.0, 2.0), Band::new(Kind::HighShelf, 10_000.0, 3.0, 0.7)];
            let params = EqParams::new(true, &bands, Preamp::Manual(0.0));
            for f in [40.0, 105.0, 1000.0, 3000.0, 12_000.0] {
                let mut eq = Equalizer::settled(&params, rate);
                let n = rate as usize;
                let input = sine(f, rate, n, 0.25);
                let mut out = input.clone();
                eq.process(&mut out);
                let tail = n / 2;
                let got = 20.0 * (rms(&out[tail * 2..]) / rms(&input[tail * 2..])).log10();
                let want = response_db(&bands, rate, f);
                assert!((got - want).abs() < 0.01, "{rate} Hz rate, {f} Hz: {got:.4} vs {want:.4}");
            }
        }
    }

    #[test]
    fn turned_off_it_glides_out_and_then_leaves_samples_alone() {
        for bands in [vec![Band::new(Kind::Peak, 100.0, 9.0, 1.0)], vec![Band::new(Kind::LowPass, 3000.0, 0.0, 0.7), Band::new(Kind::Notch, 200.0, 0.0, 4.0)]] {
            let mut eq = Equalizer::settled(&EqParams::new(true, &bands, Preamp::Auto), 48_000);
            let mut s = sine(100.0, 48_000, 4800, 0.5);
            eq.process(&mut s);
            eq.set_params(&EqParams::new(false, &bands, Preamp::Auto));
            for _ in 0..200 {
                let mut s = sine(100.0, 48_000, 480, 0.5);
                eq.process(&mut s);
                if eq.is_bypassed() {
                    break;
                }
            }
            assert!(eq.is_bypassed(), "bypassed within 2 s");
            let input = sine(440.0, 48_000, 4800, 0.5);
            let mut out = input.clone();
            eq.process(&mut out);
            assert!(out == input);
        }
    }

    /// A low-pass coming in, changing to a high-pass and going out again: no step in the
    /// signal larger than the steady signal's own.
    #[test]
    fn a_filter_without_gain_glides_in_and_out() {
        let rate = 48_000;
        let input = sine(1000.0, rate, rate as usize * 2, 0.5);
        let steady_step = input.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0.0f32, f32::max);
        let mut eq = Equalizer::new(rate);
        let mut out = input.clone();
        let quarter = rate as usize / 2;
        let (a, rest) = out.split_at_mut(quarter * 2);
        eq.set_params(&EqParams::new(true, &[Band::new(Kind::Peak, 1000.0, 0.0, 1.0)], Preamp::Manual(0.0)));
        eq.process(a);
        eq.set_params(&EqParams::new(true, &[Band::new(Kind::LowPass, 2000.0, 0.0, 2.0)], Preamp::Manual(0.0)));
        let (b, rest) = rest.split_at_mut(quarter * 2);
        eq.process(b);
        eq.set_params(&EqParams::new(true, &[Band::new(Kind::HighPass, 3000.0, 0.0, 0.7)], Preamp::Manual(0.0)));
        let (c, d) = rest.split_at_mut(quarter * 2);
        eq.process(c);
        eq.set_params(&EqParams::new(false, &[Band::new(Kind::HighPass, 3000.0, 0.0, 0.7)], Preamp::Manual(0.0)));
        eq.process(d);
        let worst = out.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0.0f32, f32::max);
        // The resonant low-pass lifts 1 kHz by 2 dB: no step beyond the louder steady level.
        let lifted = 10f64.powf(response_db(&[Band::new(Kind::LowPass, 2000.0, 0.0, 2.0)], rate, 1000.0) / 20.0) as f32;
        assert!(worst <= steady_step * lifted * 1.02, "{worst} vs {steady_step} x {lifted}");
        // At the end it is bypassed and the samples untouched.
        let tail = &out[out.len() - 2000..];
        assert!(tail == &input[input.len() - 2000..], "glided out and bypassed");
    }
}
