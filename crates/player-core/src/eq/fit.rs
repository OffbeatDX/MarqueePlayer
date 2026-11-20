//! GraphicEQ curves (Equalizer APO's `GraphicEQ:`, AutoEq's `GraphicEQ.txt`, Wavelet's
//! presets) and how they are played here: fitted to bands.
//!
//! A GraphicEQ line is a list of (frequency, gain) points; Equalizer APO joins them
//! linearly over log frequency, holds the ends flat beyond them (`GainIterator.cpp`) and
//! plays the curve through a minimum-phase FIR filter of 16,384 taps
//! (`GraphicEQFilter.cpp`). This equalizer has no FIR filter, so the curve is fitted with
//! its own bands: peaks and shelves placed one at a time where the curve is furthest from
//! what is there (a shelf where that is at an end), each refined with the others by
//! Levenberg-Marquardt least squares on the dB curve every 1/24 octave from 20 Hz to
//! 20 kHz, until the curve is within 0.15 dB or the bands run out; a constant offset goes
//! to the preamp. Bands are minimum-phase too, so a close magnitude is a close phase.
//!
//! The result is measured, not assumed: the bands' real response (`response_db`, at 44.1,
//! 48 and 96 kHz, the worst of the three) against the curve every 1/96 octave from 20 Hz
//! to 20 kHz, its largest and RMS deviation. Within [`GOOD_DB`] it is applied as any
//! import; up to [`POOR_DB`] the preview warns that it is rough; beyond, it is refused.

use super::{Band, FREQ_MIN, GAIN_MAX, Kind, response_db};

/// The most points a GraphicEQ line may have (AutoEq writes 127).
pub const MAX_POINTS: usize = 10_000;
/// Largest deviation (dB, 20 Hz to 20 kHz) of a fit taken without a warning.
pub const GOOD_DB: f64 = 1.0;
/// Largest deviation beyond which a fit is refused.
pub const POOR_DB: f64 = 3.0;
/// The rates the fit is made at together (and its deviation measured at).
const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
/// Close enough to stop adding bands.
const DONE_DB: f64 = 0.15;
/// The ridge on the gains (per dB, against the dB residuals).
const PENALTY: f64 = 0.05;
/// Bands whose gain ends up smaller than this (dB) are dropped.
const PRUNE_DB: f64 = 0.15;

/// A GraphicEQ curve's gain at `f` Hz, as Equalizer APO plays it: linear over log
/// frequency between points, flat beyond the ends. `points` sorted by frequency.
pub fn curve_at(points: &[(f64, f64)], f: f64) -> f64 {
    let Some(&(f0, g0)) = points.first() else { return 0.0 };
    if f <= f0 {
        return g0;
    }
    let (fl, gl) = *points.last().unwrap();
    if f >= fl {
        return gl;
    }
    let i = points.partition_point(|p| p.0 <= f);
    let ((fa, ga), (fb, gb)) = (points[i - 1], points[i]);
    if ga == gb || fb <= fa {
        return ga;
    }
    ga + (f.ln() - fa.ln()) / (fb.ln() - fa.ln()) * (gb - ga)
}

/// Several GraphicEQ lines played one after the other: their gains add.
pub fn curves_at(curves: &[Vec<(f64, f64)>], f: f64) -> f64 {
    curves.iter().map(|c| curve_at(c, f)).sum()
}

/// How close a fit comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Within `GOOD_DB` everywhere.
    Close,
    /// Within `POOR_DB`: applied only with the warning in front of the user.
    Rough,
    /// Further: refused.
    Poor,
}

/// A curve fitted to bands.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub bands: Vec<Band>,
    /// dB to add to the bands (it goes into the preamp).
    pub offset_db: f64,
    /// The bands' largest and RMS deviation from the curve, 20 Hz to 20 kHz (dB), and
    /// where the largest is.
    pub max_err_db: f64,
    pub rms_err_db: f64,
    pub worst_hz: f64,
}

impl Fit {
    pub fn quality(&self) -> Quality {
        if self.max_err_db <= GOOD_DB {
            Quality::Close
        } else if self.max_err_db <= POOR_DB {
            Quality::Rough
        } else {
            Quality::Poor
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct P {
    kind: Kind,
    lf: f64,
    g: f64,
    lq: f64,
}

impl P {
    fn clamp(&mut self) {
        self.lf = self.lf.clamp(12f64.ln(), 21_000f64.ln());
        self.g = self.g.clamp(-f64::from(GAIN_MAX), f64::from(GAIN_MAX));
        let (lo, hi) = if self.kind == Kind::Peak { (0.25, 15.0) } else { (0.35, 1.5) };
        self.lq = self.lq.clamp(f64::ln(lo), f64::ln(hi));
    }

    fn band(&self) -> Band {
        Band::new(self.kind, self.lf.exp() as f32, self.g as f32, self.lq.exp() as f32)
    }
}

/// The fitting grid: the frequencies at each of `RATES` one after the other, their warped
/// values `tan(pi f / fs)`, the target there. Fitting all three rates at once gives bands
/// that follow the curve at any of them: near Nyquist the bilinear transform bends a band
/// differently at 44.1 kHz than at 48 or 96, and a fit made at one rate alone can miss by
/// several dB above 18 kHz at another.
struct Grid {
    tw: Vec<f64>,
    target: Vec<f64>,
    /// Points per rate.
    per: usize,
}

impl Grid {
    /// One band's dB curve on the grid (the cookbook prototype at the warped frequency, as
    /// `Proto::power_at`, with the warping done once for the grid).
    fn eval(&self, p: &P, out: &mut [f64]) {
        let a = 10f64.powf(p.g / 40.0);
        let q = p.lq.exp();
        let sa = a.sqrt();
        let ks = RATES.map(|rate| (std::f64::consts::PI * p.lf.exp().clamp(f64::from(FREQ_MIN), rate * 0.45) / rate).tan());
        for (i, (o, &t)) in out.iter_mut().zip(&self.tw).enumerate() {
            let om = t / ks[i / self.per];
            let o2 = om * om;
            let pw = match p.kind {
                Kind::LowShelf => {
                    let x = om * sa / q;
                    a * a * ((a - o2).powi(2) + x * x) / ((1.0 - a * o2).powi(2) + x * x)
                }
                Kind::HighShelf => {
                    let x = om * sa / q;
                    a * a * ((1.0 - a * o2).powi(2) + x * x) / ((a - o2).powi(2) + x * x)
                }
                _ => {
                    let re = 1.0 - o2;
                    let im = om / q;
                    (re * re + im * im * a * a) / (re * re + im * im / (a * a))
                }
            };
            *o = 10.0 * pw.log10();
        }
    }
}

/// Levenberg-Marquardt over the parameters of `which` bands (and the offset): `resp`
/// holds each band's curve, kept current.
fn refine(grid: &Grid, ps: &mut [P], resp: &mut [Vec<f64>], offset: &mut f64, which: &[usize], iters: usize) {
    let n = grid.tw.len();
    let m = which.len() * 3 + 1;
    let model = |resp: &[Vec<f64>], offset: f64| -> Vec<f64> {
        let mut v = vec![offset; n];
        for r in resp {
            for (x, y) in v.iter_mut().zip(r) {
                *x += y;
            }
        }
        v
    };
    let cost = |model: &[f64]| -> f64 { model.iter().zip(&grid.target).map(|(m, t)| (t - m) * (t - m)).sum() };
    // A little of every gain counts against the fit (ridge): bands do not fight each
    // other with large opposite gains for a hundredth of a dB.
    let penalty = |ps: &[P]| -> f64 { ps.iter().map(|p| (PENALTY * p.g).powi(2)).sum() };
    let mut cur = model(resp, *offset);
    let mut c0 = cost(&cur) + penalty(ps);
    let mut lambda = 1e-3;
    let mut col = vec![0.0; n];
    let mut jac = vec![0.0; m * n];
    for _ in 0..iters {
        // Jacobian, by differences (each column changes one band's curve only).
        for (wi, &bi) in which.iter().enumerate() {
            for k in 0..3 {
                let mut p = ps[bi];
                let h = 1e-4;
                match k {
                    0 => p.lf += h,
                    1 => p.g += h,
                    _ => p.lq += h,
                }
                grid.eval(&p, &mut col);
                let base = &resp[bi];
                let dst = &mut jac[(wi * 3 + k) * n..(wi * 3 + k + 1) * n];
                for ((d, a), b) in dst.iter_mut().zip(&col).zip(base) {
                    *d = (a - b) / h;
                }
            }
        }
        jac[(m - 1) * n..].fill(1.0);
        let r: Vec<f64> = grid.target.iter().zip(&cur).map(|(t, c)| t - c).collect();
        let mut jtj = vec![0.0; m * m];
        let mut jtr = vec![0.0; m];
        for a in 0..m {
            let ca = &jac[a * n..(a + 1) * n];
            jtr[a] = ca.iter().zip(&r).map(|(x, y)| x * y).sum();
            for b in a..m {
                let cb = &jac[b * n..(b + 1) * n];
                let v: f64 = ca.iter().zip(cb).map(|(x, y)| x * y).sum();
                jtj[a * m + b] = v;
                jtj[b * m + a] = v;
            }
        }
        for (wi, &bi) in which.iter().enumerate() {
            let k = wi * 3 + 1;
            jtj[k * m + k] += PENALTY * PENALTY;
            jtr[k] -= PENALTY * PENALTY * ps[bi].g;
        }
        let mut improved = false;
        for _ in 0..6 {
            let mut a = jtj.clone();
            for d in 0..m {
                a[d * m + d] += lambda * (jtj[d * m + d] + 1e-9);
            }
            let Some(delta) = solve(&mut a, &mut jtr.clone(), m) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial: Vec<P> = which.iter().map(|&bi| ps[bi]).collect();
            for (t, d) in trial.iter_mut().zip(delta.chunks(3)) {
                t.lf += d[0];
                t.g += d[1];
                t.lq += d[2];
                t.clamp();
            }
            let mut trial_resp: Vec<Vec<f64>> = Vec::with_capacity(which.len());
            for t in &trial {
                let mut v = vec![0.0; n];
                grid.eval(t, &mut v);
                trial_resp.push(v);
            }
            let trial_offset = *offset + delta[m - 1];
            let mut model_t = cur.clone();
            for (wi, &bi) in which.iter().enumerate() {
                for ((x, new), old) in model_t.iter_mut().zip(&trial_resp[wi]).zip(&resp[bi]) {
                    *x += new - old;
                }
            }
            for x in &mut model_t {
                *x += trial_offset - *offset;
            }
            let mut all = ps.to_vec();
            for (wi, &bi) in which.iter().enumerate() {
                all[bi] = trial[wi];
            }
            let c1 = cost(&model_t) + penalty(&all);
            if c1 < c0 {
                for (wi, &bi) in which.iter().enumerate() {
                    ps[bi] = trial[wi];
                    resp[bi] = std::mem::take(&mut trial_resp[wi]);
                }
                *offset = trial_offset;
                cur = model_t;
                let gain = (c0 - c1) / c0.max(1e-30);
                c0 = c1;
                lambda = (lambda / 3.0).max(1e-9);
                improved = gain > 1e-7;
                break;
            }
            lambda *= 5.0;
        }
        if !improved {
            break;
        }
    }
}

/// Solve `a x = b` (m x m, row-major) by Gaussian elimination with partial pivoting.
fn solve(a: &mut [f64], b: &mut [f64], m: usize) -> Option<Vec<f64>> {
    for c in 0..m {
        let p = (c..m).max_by(|&x, &y| a[x * m + c].abs().total_cmp(&a[y * m + c].abs()))?;
        if a[p * m + c].abs() < 1e-300 {
            return None;
        }
        if p != c {
            for k in 0..m {
                a.swap(p * m + k, c * m + k);
            }
            b.swap(p, c);
        }
        for r in c + 1..m {
            let f = a[r * m + c] / a[c * m + c];
            if f != 0.0 {
                for k in c..m {
                    a[r * m + k] -= f * a[c * m + k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    let mut x = vec![0.0; m];
    for c in (0..m).rev() {
        let s: f64 = (c + 1..m).map(|k| a[c * m + k] * x[k]).sum();
        x[c] = (b[c] - s) / a[c * m + c];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The deviation of `bands` + `offset` from `target`, 20 Hz to 20 kHz every 1/96 octave,
/// the worst of 44.1, 48 and 96 kHz: (largest, RMS, where the largest is).
pub fn deviation(bands: &[Band], offset: f64, target: &dyn Fn(f64) -> f64) -> (f64, f64, f64) {
    let n = (1000f64.log2() * 96.0).round() as usize;
    let mut worst = (0.0f64, 0.0f64, 20.0f64);
    for rate in [44_100u32, 48_000, 96_000] {
        let (mut max, mut sum, mut at) = (0.0f64, 0.0f64, 20.0f64);
        for i in 0..=n {
            let f = 20.0 * 1000f64.powf(i as f64 / n as f64);
            let e = (response_db(bands, rate, f) + offset - target(f)).abs();
            sum += e * e;
            if e > max {
                max = e;
                at = f;
            }
        }
        let rms = (sum / (n + 1) as f64).sqrt();
        if max > worst.0 {
            worst = (max, rms.max(worst.1), at);
        } else {
            worst.1 = worst.1.max(rms);
        }
    }
    worst
}

/// Fit `target` (dB by frequency) with at most `budget` bands and an offset.
pub fn fit(target: &dyn Fn(f64) -> f64, budget: usize) -> Fit {
    let n = (1000f64.log2() * 24.0).round() as usize;
    let freqs: Vec<f64> = (0..=n).map(|i| 20.0 * 1000f64.powf(i as f64 / n as f64)).collect();
    let len = freqs.len();
    let want: Vec<f64> = freqs.iter().map(|&f| target(f)).collect();
    let grid = Grid {
        tw: RATES.iter().flat_map(|&rate| freqs.iter().map(move |f| (std::f64::consts::PI * f / rate).tan())).collect(),
        target: RATES.iter().flat_map(|_| want.iter().copied()).collect(),
        per: len,
    };
    let total = grid.tw.len();
    let mut offset = want.iter().sum::<f64>() / len as f64;
    let mut ps: Vec<P> = Vec::new();
    let mut resp: Vec<Vec<f64>> = Vec::new();
    // The largest residual after each band added: when the last few gained next to
    // nothing (the top octave at 44.1 kHz no band can follow further), adding stops.
    let mut worst: Vec<f64> = Vec::new();
    while ps.len() < budget {
        let mut model = vec![offset; total];
        for r in &resp {
            for (x, y) in model.iter_mut().zip(r) {
                *x += y;
            }
        }
        // The residual at each frequency, averaged over the rates.
        let r: Vec<f64> = (0..len).map(|i| (0..RATES.len()).map(|s| grid.target[s * len + i] - model[s * len + i]).sum::<f64>() / RATES.len() as f64).collect();
        let (j, peak) = r.iter().enumerate().fold((0, 0.0f64), |b, (i, &v)| if v.abs() > b.1.abs() { (i, v) } else { b });
        worst.push(peak.abs());
        if peak.abs() < DONE_DB || (worst.len() > 4 && worst[worst.len() - 5] - peak.abs() < 0.03) {
            break;
        }
        // The stretch around it where the residual is at least half as large, same sign.
        let half = |i: usize| r[i].signum() == peak.signum() && r[i].abs() >= peak.abs() / 2.0;
        let mut lo = j;
        while lo > 0 && half(lo - 1) {
            lo -= 1;
        }
        let mut hi = j;
        while hi + 1 < len && half(hi + 1) {
            hi += 1;
        }
        let p = if lo == 0 && j < len / 8 {
            P { kind: Kind::LowShelf, lf: freqs[hi].ln(), g: peak, lq: 0.7f64.ln() }
        } else if hi == len - 1 && j > len * 7 / 8 {
            P { kind: Kind::HighShelf, lf: freqs[lo].ln(), g: peak, lq: 0.7f64.ln() }
        } else {
            let bw = (freqs[hi] / freqs[lo]).log2().max(1.0 / 12.0);
            let x = 2f64.powf(bw);
            P { kind: Kind::Peak, lf: freqs[j].ln(), g: peak, lq: (x.sqrt() / (x - 1.0)).ln() }
        };
        let mut p = p;
        p.clamp();
        let mut v = vec![0.0; total];
        grid.eval(&p, &mut v);
        ps.push(p);
        resp.push(v);
        let last = ps.len() - 1;
        refine(&grid, &mut ps, &mut resp, &mut offset, &[last], 10);
        if ps.len().is_multiple_of(4) || ps.len() == budget {
            let all: Vec<usize> = (0..ps.len()).collect();
            refine(&grid, &mut ps, &mut resp, &mut offset, &all, 4);
        }
    }
    let all: Vec<usize> = (0..ps.len()).collect();
    refine(&grid, &mut ps, &mut resp, &mut offset, &all, 40);
    // Bands that ended up doing next to nothing go, and the rest settle without them.
    if ps.iter().any(|p| p.g.abs() < PRUNE_DB) {
        let keep: Vec<usize> = (0..ps.len()).filter(|&i| ps[i].g.abs() >= PRUNE_DB).collect();
        ps = keep.iter().map(|&i| ps[i]).collect();
        resp = keep.iter().map(|&i| std::mem::take(&mut resp[i])).collect();
        let all: Vec<usize> = (0..ps.len()).collect();
        refine(&grid, &mut ps, &mut resp, &mut offset, &all, 20);
    }
    let mut bands: Vec<Band> = ps.iter().map(P::band).collect();
    bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
    let (max_err_db, rms_err_db, worst_hz) = deviation(&bands, offset, target);
    Fit { bands, offset_db: offset, max_err_db, rms_err_db, worst_hz }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_equalizer_apo_s() {
        let pts = vec![(100.0, -6.0), (1000.0, 0.0), (10_000.0, 6.0)];
        assert_eq!(curve_at(&pts, 20.0), -6.0, "flat below the first point");
        assert_eq!(curve_at(&pts, 20_000.0), 6.0, "flat above the last");
        assert!((curve_at(&pts, 316.227_766) + 3.0).abs() < 1e-9, "linear over log frequency");
        assert_eq!(curve_at(&[], 1000.0), 0.0);
    }

    #[test]
    fn a_smooth_curve_fits_closely_and_cheaply() {
        // A tilt with a bass shelf and a presence dip: what headphone corrections look like.
        let pts: Vec<(f64, f64)> = (0..127).map(|i| {
            let f = 20.0 * 1.0563f64.powi(i);
            let g = -4.0 + 4.0 / (1.0 + (f / 120.0).powi(2)) - 3.0 * (-((f / 4000.0).log2() * 2.0).powi(2)).exp() - 0.8 * (f / 1000.0).log2().max(0.0);
            (f.round(), (g * 10.0).round() / 10.0)
        }).collect();
        let t = |f: f64| curve_at(&pts, f);
        let start = std::time::Instant::now();
        let fit = fit(&t, 32);
        let took = start.elapsed();
        eprintln!("{} bands, offset {:.2} dB, max {:.3} dB at {:.0} Hz, RMS {:.3} dB, {took:?}", fit.bands.len(), fit.offset_db, fit.max_err_db, fit.worst_hz, fit.rms_err_db);
        assert_eq!(fit.quality(), Quality::Close, "{fit:?}");
        assert!(fit.max_err_db < 0.5);
        assert!(fit.bands.len() <= 32);
    }
}
