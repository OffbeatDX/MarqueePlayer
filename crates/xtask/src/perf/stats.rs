//! The figures of a comparison: a measure's rounds on each side, its difference, the spread
//! between rounds, and the verdict (docs/testing.md, "What a verdict means").

/// One measure of one scene, round by round on each side (the same count of rounds, in the
/// order they ran: base, change, base, change...). A round that had no figure is left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sides {
    pub base: Vec<f64>,
    pub change: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Lower on the change, by more than the spread and the floor.
    Cheaper,
    /// Higher on the change, by more than the spread and the floor.
    Dearer,
    /// The difference is no larger than the spread between rounds, or than the floor.
    Noise,
    /// Fewer than two rounds on a side: no spread, so no verdict.
    NoSpread,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Cheaper => "cheaper",
            Verdict::Dearer => "dearer",
            Verdict::Noise => "within the noise",
            Verdict::NoSpread => "no verdict (one round)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compared {
    pub base: f64,
    pub change: f64,
    /// change - base.
    pub diff: f64,
    /// The difference as a share of the base (0.05 = 5%); 0 when the base is 0.
    pub rel: f64,
    /// The spread between rounds: the larger of the two sides' ranges (max - min).
    pub spread: f64,
    /// The spread as a share of the base.
    pub spread_rel: f64,
    pub verdict: Verdict,
}

pub fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

pub fn range(v: &[f64]) -> f64 {
    let (lo, hi) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    if v.is_empty() { 0.0 } else { hi - lo }
}

/// The standard deviation of the samples (n - 1), 0 with fewer than two.
pub fn stdev(v: &[f64]) -> f64 {
    let Some(m) = mean(v) else { return 0.0 };
    if v.len() < 2 {
        return 0.0;
    }
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

/// Compares the sides of a measure where lower is cheaper. `floor` is the smallest share of
/// the base that counts as a difference at all, whatever the spread (two rounds can agree by
/// chance closer than the measure ever repeats: the floor is the tool's own noise, measured
/// with no change). The rule: a difference counts when it is larger than the spread between
/// rounds of either side and larger than the floor, and the rounds do not overlap (every
/// round of one side below every round of the other).
///
/// `floor_abs` is a floor in the measure's own unit, for what the whole machine moves (the
/// window server, the GPU: another app drawing moves them by points, whatever their level).
pub fn compare(s: &Sides, floor: f64, floor_abs: f64) -> Option<Compared> {
    let (base, change) = (mean(&s.base)?, mean(&s.change)?);
    let diff = change - base;
    let rel = if base.abs() > 1e-12 { diff / base } else { 0.0 };
    let spread = range(&s.base).max(range(&s.change));
    let spread_rel = if base.abs() > 1e-12 { spread / base.abs() } else { 0.0 };
    let verdict = if s.base.len() < 2 || s.change.len() < 2 {
        Verdict::NoSpread
    } else {
        let apart = if diff < 0.0 { max(&s.change) < min(&s.base) } else { min(&s.change) > max(&s.base) };
        let floor_abs = (floor * base.abs()).max(floor_abs);
        if diff.abs() > spread && diff.abs() > floor_abs && apart {
            if diff < 0.0 { Verdict::Cheaper } else { Verdict::Dearer }
        } else {
            Verdict::Noise
        }
    };
    Some(Compared { base, change, diff, rel, spread, spread_rel, verdict })
}

fn min(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::INFINITY, f64::min)
}

fn max(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// Two means of per-second samples (watts), each side one run: the difference counts when it
/// is larger than twice the standard error of the difference and than the floor.
pub fn compare_samples(base: &[f64], change: &[f64], floor: f64) -> Option<Compared> {
    let (b, c) = (mean(base)?, mean(change)?);
    let se = ((stdev(base).powi(2) / base.len() as f64) + (stdev(change).powi(2) / change.len() as f64)).sqrt();
    let diff = c - b;
    let rel = if b.abs() > 1e-12 { diff / b } else { 0.0 };
    let spread = 2.0 * se;
    let verdict = if base.len() < 2 || change.len() < 2 {
        Verdict::NoSpread
    } else if diff.abs() > spread && diff.abs() > floor * b.abs() {
        if diff < 0.0 { Verdict::Cheaper } else { Verdict::Dearer }
    } else {
        Verdict::Noise
    };
    Some(Compared { base: b, change: c, diff, rel, spread, spread_rel: if b.abs() > 1e-12 { spread / b.abs() } else { 0.0 }, verdict })
}

/// A scene's verdict from its measures, the app's CPU first: the app cheaper while the window
/// server or the GPU is dearer is work moved, not saved, and said so.
pub fn scene_verdict(app: Option<Compared>, window_server: Option<Compared>, gpu: Option<Compared>) -> String {
    let Some(app) = app else { return "no verdict: the app's CPU was not measured".into() };
    let dearer = |c: Option<Compared>| c.is_some_and(|c| c.verdict == Verdict::Dearer);
    let cheaper = |c: Option<Compared>| c.is_some_and(|c| c.verdict == Verdict::Cheaper);
    let mut elsewhere = Vec::new();
    if dearer(window_server) {
        elsewhere.push("the window server");
    }
    if dearer(gpu) {
        elsewhere.push("the GPU");
    }
    match app.verdict {
        Verdict::Cheaper if !elsewhere.is_empty() => format!("moved, not saved: the app is cheaper but {} is dearer", elsewhere.join(" and ")),
        Verdict::Cheaper => "cheaper".into(),
        Verdict::Dearer => "dearer".into(),
        Verdict::Noise if !elsewhere.is_empty() => format!("dearer elsewhere: the app within the noise, {} dearer", elsewhere.join(" and ")),
        Verdict::Noise if cheaper(window_server) || cheaper(gpu) => "within the noise for the app; cheaper in the window server or the GPU".into(),
        Verdict::Noise => "within the noise".into(),
        Verdict::NoSpread => Verdict::NoSpread.word().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides(b: &[f64], c: &[f64]) -> Sides {
        Sides { base: b.to_vec(), change: c.to_vec() }
    }

    #[test]
    fn a_difference_larger_than_the_spread_and_the_floor_is_a_verdict() {
        let c = compare(&sides(&[1.00, 1.02], &[0.80, 0.81]), 0.03, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Cheaper);
        assert!((c.diff - -0.205).abs() < 1e-9);
        assert!((c.spread - 0.02).abs() < 1e-9);
        let c = compare(&sides(&[1.00, 1.02], &[1.20, 1.25]), 0.03, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Dearer);
    }

    #[test]
    fn a_difference_inside_the_spread_is_noise() {
        // 5% apart on average, but the base's own rounds are 10% apart.
        let c = compare(&sides(&[1.00, 1.10], &[1.00, 1.01]), 0.01, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Noise);
    }

    #[test]
    fn rounds_that_overlap_are_noise_even_when_the_means_differ() {
        let c = compare(&sides(&[1.0, 1.3, 1.0], &[1.1, 1.1, 1.4]), 0.0, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Noise);
    }

    #[test]
    fn under_the_floor_is_noise_however_tight_the_rounds() {
        let c = compare(&sides(&[1.000, 1.000], &[0.990, 0.990]), 0.02, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Noise);
        let c = compare(&sides(&[1.000, 1.000], &[0.970, 0.970]), 0.02, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::Cheaper);
    }

    #[test]
    fn a_machine_wide_measure_needs_its_floor_in_points_too() {
        // The window server with no change, while another app drew (2026-11-26).
        let s = sides(&[8.25, 9.38], &[5.13, 6.25]);
        assert_eq!(compare(&s, 0.10, 0.0).unwrap().verdict, Verdict::Cheaper);
        assert_eq!(compare(&s, 0.10, 4.0).unwrap().verdict, Verdict::Noise);
    }

    #[test]
    fn one_round_gives_figures_but_no_verdict() {
        let c = compare(&sides(&[1.0], &[0.5]), 0.0, 0.0).unwrap();
        assert_eq!(c.verdict, Verdict::NoSpread);
        assert!(compare(&sides(&[], &[0.5]), 0.0, 0.0).is_none());
    }

    #[test]
    fn watts_from_samples_compare_by_their_standard_error() {
        let b = [5.0, 5.2, 4.8, 5.1, 4.9];
        let c = [4.0, 4.1, 3.9, 4.0, 4.0];
        assert_eq!(compare_samples(&b, &c, 0.02).unwrap().verdict, Verdict::Cheaper);
        let c = [5.1, 4.9, 5.0, 5.2, 4.8];
        assert_eq!(compare_samples(&b, &c, 0.02).unwrap().verdict, Verdict::Noise);
    }

    #[test]
    fn the_app_cheaper_with_the_window_server_dearer_is_work_moved() {
        let cmp = |v| Some(Compared { base: 1.0, change: 1.0, diff: 0.0, rel: 0.0, spread: 0.0, spread_rel: 0.0, verdict: v });
        assert!(scene_verdict(cmp(Verdict::Cheaper), cmp(Verdict::Dearer), cmp(Verdict::Noise)).starts_with("moved, not saved"));
        assert_eq!(scene_verdict(cmp(Verdict::Cheaper), cmp(Verdict::Noise), None), "cheaper");
        assert!(scene_verdict(cmp(Verdict::Noise), None, cmp(Verdict::Dearer)).starts_with("dearer elsewhere"));
        assert_eq!(scene_verdict(cmp(Verdict::Noise), None, None), "within the noise");
    }
}
