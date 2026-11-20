//! The graphic layout (ten bands an octave apart) and the built-in presets.
//!
//! **The graphic layout.** Peaking bands at the ISO octave centres from 31.5 Hz to 16 kHz,
//! each an octave wide (Q 1.41). It is how the equalizer looks to most people: ten sliders.
//! Neighbouring bands overlap, so two sliders up together lift the octave between them by a
//! little more than either; the curve drawn is the real combined one.
//!
//! **The presets** are the graphic layout, so they show as sliders and can be adjusted from
//! there. Each was made by fitting the ten bands to a target curve that says what it is for
//! (`fit_graphic`, least squares), not copied from another player. The test
//! `presets_follow_their_targets` checks they are still that fit and how close it comes
//! from 31.5 Hz to 16 kHz: within 1 dB, the treble shelves within 1.6 dB (octave bands
//! cannot follow a shelf's corner between 8 and 16 kHz exactly):
//!
//! - Bass boost / Bass reducer: a low shelf of +-6 dB with its midpoint at 105 Hz (Q 0.71),
//!   the bass shelf of the Harman headphone target: weight below 100 Hz, nothing above
//!   400 Hz touched, so voices keep their tone.
//! - Treble boost / Treble reducer: a high shelf of +-4 dB from 6 kHz (midpoint, Q 0.71):
//!   air and detail for dull recordings or speakers, or less glare and sibilance for bright
//!   ones. 4 dB, not 6: treble boosts tire quickly.
//! - Vocal: +3 dB of presence around 2.5 kHz (Q 1) where consonants and the voice's edge are,
//!   and 3 dB less below 150 Hz, so speech and singing stand forward of the band.
//! - Loudness: for listening quietly. The ear loses bass first as the level drops (the general
//!   shape of the equal-loudness contours): the curve rises smoothly below 1 kHz, to about
//!   +9 dB at 31 Hz, +5 at 125 Hz and +1 at 500 Hz, the treble left alone. These are rounded
//!   numbers chosen by ear and from the contours' general shape, not fitted to the ISO 226
//!   tables (the standard is not freely published, so they were not checked against it).
//! - Warm / Bright: a gentle tilt of about 0.45 dB an octave through 700 Hz, +-2 dB at the
//!   ends: a whole recording a little darker or lighter, nothing singled out.
//! - Spoken word: Vocal's presence, with 8 dB less below 120 Hz (room rumble, handling
//!   noise, the boom of a close microphone) and 2 dB less above 12 kHz (hiss): podcasts
//!   and audiobooks.
//!
//! Flat is all zero. With auto preamp (the default) a preset only ever lowers the level of
//! what it does not boost, so none of them can clip (`tests/eq.rs` plays each at full scale).

use super::{Band, Kind, Width, response_db};

/// The graphic layout's centres (ISO 266 octave bands).
pub const GRAPHIC_FREQS: [f32; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0];
/// One octave wide: Q = sqrt(2) / (2 - 1).
pub const GRAPHIC_Q: f32 = 1.41;
/// The sliders' range.
pub const GRAPHIC_MAX_DB: f32 = 12.0;

/// The slider labels.
pub const GRAPHIC_LABELS: [&str; 10] = ["31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];

/// Ten graphic sliders as bands.
pub fn graphic(gains: &[f32; 10]) -> Vec<Band> {
    GRAPHIC_FREQS.iter().zip(gains).map(|(&f, &g)| Band::new(Kind::Peak, f, g.clamp(-GRAPHIC_MAX_DB, GRAPHIC_MAX_DB), GRAPHIC_Q)).collect()
}

/// The sliders' gains, when `bands` are exactly the graphic layout.
pub fn as_graphic(bands: &[Band]) -> Option<[f32; 10]> {
    if bands.len() != 10 {
        return None;
    }
    let mut g = [0.0; 10];
    for (i, b) in bands.iter().enumerate() {
        let same = b.kind == Kind::Peak && b.width == Width::Q && (b.freq - GRAPHIC_FREQS[i]).abs() < 0.01 && (b.q - GRAPHIC_Q).abs() < 0.001;
        if !same || b.gain_db.abs() > GRAPHIC_MAX_DB {
            return None;
        }
        g[i] = b.gain_db;
    }
    Some(g)
}

/// The graphic sliders that come closest to a curve (dB by frequency): least squares over
/// every sixth of an octave from 31.5 Hz to 16 kHz (Gauss-Newton on the real combined
/// curve), held within the sliders' range and rounded to 0.1 dB. Also how a parametric
/// curve is shown as sliders.
pub fn fit_graphic(target: impl Fn(f64) -> f64) -> [f32; 10] {
    const RATE: u32 = 48_000;
    let lo = f64::from(GRAPHIC_FREQS[0]);
    let hi = f64::from(GRAPHIC_FREQS[9]);
    let n_pts = ((hi / lo).log2() * 6.0).round() as usize;
    let freqs: Vec<f64> = (0..=n_pts).map(|i| lo * (hi / lo).powf(i as f64 / n_pts as f64)).collect();
    let want: Vec<f64> = freqs.iter().map(|&f| target(f)).collect();
    let curve = |g: &[f64; 10]| -> Vec<f64> {
        let bands = graphic(&std::array::from_fn(|i| g[i] as f32));
        let mut out = Vec::new();
        super::response_curve(&bands, RATE, &freqs, &mut out);
        out
    };
    let max = f64::from(GRAPHIC_MAX_DB);
    let mut g: [f64; 10] = std::array::from_fn(|i| target(f64::from(GRAPHIC_FREQS[i])).clamp(-max, max));
    for _ in 0..12 {
        let base = curve(&g);
        let r: Vec<f64> = want.iter().zip(&base).map(|(w, b)| w - b).collect();
        // Jacobian by finite differences of 0.1 dB.
        let mut jac = vec![[0.0f64; 10]; freqs.len()];
        for j in 0..10 {
            let mut gj = g;
            gj[j] += 0.1;
            let c = curve(&gj);
            for k in 0..freqs.len() {
                jac[k][j] = (c[k] - base[k]) / 0.1;
            }
        }
        // (J^T J + small damping) d = J^T r, by Gaussian elimination.
        let mut a = [[0.0f64; 11]; 10];
        for i in 0..10 {
            for j in 0..10 {
                a[i][j] = (0..freqs.len()).map(|k| jac[k][i] * jac[k][j]).sum::<f64>();
            }
            a[i][i] += 1e-6;
            a[i][10] = (0..freqs.len()).map(|k| jac[k][i] * r[k]).sum::<f64>();
        }
        for c in 0..10 {
            let p = (c..10).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs())).unwrap();
            a.swap(c, p);
            for row in 0..10 {
                if row != c {
                    let f = a[row][c] / a[c][c];
                    let pivot = a[c];
                    for (x, p) in a[row].iter_mut().zip(pivot).skip(c) {
                        *x -= f * p;
                    }
                }
            }
        }
        let mut step = 0.0f64;
        for i in 0..10 {
            let d = a[i][10] / a[i][i];
            g[i] = (g[i] + d).clamp(-max, max);
            step = step.max(d.abs());
        }
        if step < 1e-4 {
            break;
        }
    }
    std::array::from_fn(|i| ((g[i] * 10.0).round() / 10.0) as f32 + 0.0)
}

/// The fit of `bands`' curve as sliders (a parametric curve shown in the graphic layout).
pub fn graphic_from(bands: &[Band]) -> [f32; 10] {
    fit_graphic(|f| response_db(bands, 48_000, f))
}

/// A built-in preset: its name and the ten sliders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub gains: [f32; 10],
}

impl Preset {
    pub fn bands(&self) -> Vec<Band> {
        graphic(&self.gains)
    }
}

/// The built-in presets, in the order the picker lists them. The gains are
/// `fit_graphic(target)` for each `target` below (the test checks they still are).
pub const BUILT_IN: [Preset; 10] = [
    Preset { name: "Flat", gains: [0.0; 10] },
    Preset { name: "Bass boost", gains: [5.4, 4.7, 1.1, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    Preset { name: "Bass reducer", gains: [-5.4, -4.7, -1.1, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    Preset { name: "Treble boost", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 3.6, 4.9] },
    Preset { name: "Treble reducer", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, -3.6, -4.9] },
    Preset { name: "Vocal", gains: [-2.6, -2.5, -1.7, 0.1, 0.1, -0.1, 2.6, 1.4, -0.1, 0.0] },
    Preset { name: "Loudness", gains: [8.4, 5.3, 3.7, 2.2, 0.5, -0.2, 0.0, 0.0, 0.0, 0.0] },
    Preset { name: "Warm", gains: [1.8, 1.2, 0.8, 0.5, 0.2, -0.2, -0.5, -0.9, -1.6, -2.4] },
    Preset { name: "Bright", gains: [-1.8, -1.2, -0.8, -0.5, -0.2, 0.2, 0.5, 0.9, 1.6, 2.4] },
    Preset { name: "Spoken word", gains: [-7.1, -6.5, -2.6, 0.5, 0.1, 0.0, 2.6, 1.5, -0.3, -2.1] },
];

pub fn built_in(name: &str) -> Option<&'static Preset> {
    BUILT_IN.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// What each built-in preset is fitted to (dB by frequency).
pub fn target(name: &str) -> Option<Box<dyn Fn(f64) -> f64>> {
    let shelf = |kind: Kind, f: f32, g: f32, q: f32| {
        let b = [Band::new(kind, f, g, q)];
        move |x: f64| response_db(&b, 48_000, x)
    };
    let tilt = |db_per_octave: f64| move |f: f64| (-db_per_octave * (f / 700.0).log2()).clamp(-2.0, 2.0);
    Some(match name {
        "Flat" => Box::new(|_| 0.0),
        "Bass boost" => Box::new(shelf(Kind::LowShelf, 105.0, 6.0, 0.71)),
        "Bass reducer" => Box::new(shelf(Kind::LowShelf, 105.0, -6.0, 0.71)),
        "Treble boost" => Box::new(shelf(Kind::HighShelf, 6000.0, 4.0, 0.71)),
        "Treble reducer" => Box::new(shelf(Kind::HighShelf, 6000.0, -4.0, 0.71)),
        "Vocal" => {
            let b = [Band::new(Kind::LowShelf, 150.0, -3.0, 0.71), Band::new(Kind::Peak, 2500.0, 3.0, 1.0)];
            Box::new(move |x| response_db(&b, 48_000, x))
        }
        "Loudness" => {
            // Points of the curve (Hz, dB), the rounded numbers above (not ISO 226 data),
            // joined smoothly on a log scale.
            const PTS: [(f64, f64); 7] = [(20.0, 10.0), (31.5, 9.0), (63.0, 7.0), (125.0, 5.0), (250.0, 3.0), (500.0, 1.0), (1000.0, 0.0)];
            Box::new(|f: f64| {
                if f <= PTS[0].0 {
                    return PTS[0].1;
                }
                for w in PTS.windows(2) {
                    let ((f0, g0), (f1, g1)) = (w[0], w[1]);
                    if f <= f1 {
                        let t = (f / f0).log2() / (f1 / f0).log2();
                        // Smoothstep between points: no corners.
                        let t = t * t * (3.0 - 2.0 * t);
                        return g0 + (g1 - g0) * t;
                    }
                }
                0.0
            })
        }
        "Warm" => Box::new(tilt(0.45)),
        "Bright" => Box::new(tilt(-0.45)),
        "Spoken word" => {
            // Vocal, with the low rumble of rooms and microphones taken out and less hiss.
            let b = [
                Band::new(Kind::LowShelf, 120.0, -8.0, 0.71),
                Band::new(Kind::Peak, 2500.0, 3.0, 1.0),
                Band::new(Kind::HighShelf, 12_000.0, -2.0, 0.71),
            ];
            Box::new(move |x| response_db(&b, 48_000, x))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_follow_their_targets() {
        for p in &BUILT_IN {
            let t = target(p.name).unwrap();
            let fitted = fit_graphic(&t);
            for (i, (&a, &b)) in fitted.iter().zip(&p.gains).enumerate() {
                assert!((a - b).abs() < 0.051, "{}: band {i} is {b}, the fit says {a}: {fitted:?}", p.name);
            }
            let bands = p.bands();
            let mut worst = 0.0f64;
            for i in 0..=200 {
                let f = 31.5 * (16_000.0f64 / 31.5).powf(f64::from(i) / 200.0);
                worst = worst.max((response_db(&bands, 48_000, f) - t(f)).abs());
            }
            let allowed = if p.name.starts_with("Treble") { 1.6 } else { 1.0 };
            assert!(worst < allowed, "{}: {worst:.2} dB off its target", p.name);
        }
    }

    #[test]
    fn graphic_round_trips() {
        let g = [1.0, -2.0, 3.0, 0.0, 0.5, -0.5, 12.0, -12.0, 0.0, 4.0];
        assert_eq!(as_graphic(&graphic(&g)), Some(g));
        assert_eq!(as_graphic(&[Band::new(Kind::Peak, 1000.0, 3.0, 1.41)]), None);
        // A graphic curve shown as sliders is itself.
        let back = graphic_from(&graphic(&g));
        for (a, b) in back.iter().zip(&g) {
            assert!((a - b).abs() < 0.11, "{back:?}");
        }
    }
}
