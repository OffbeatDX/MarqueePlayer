//! Surround to stereo, by channel layout (ITU-R BS.775 style): front left and right
//! straight through, the centre into both at -3 dB, surrounds and backs to their side at
//! -3 dB, a back centre into both at -6 dB, height channels like the ones below them a
//! further -3 dB down, and the LFE left out (as BS.775 does). The result is scaled so
//! that even every channel at full scale, in phase, cannot clip: by 1 / the larger of the
//! two sides' summed coefficients. (Before, only front left and right were kept, which
//! lost the centre: vocals and dialogue.)

use symphonia::core::audio::{Channels, Position};

const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// (left, right) weight of a speaker position.
fn weights(p: Position) -> (f32, f32) {
    match p {
        Position::FRONT_LEFT => (1.0, 0.0),
        Position::FRONT_RIGHT => (0.0, 1.0),
        Position::FRONT_CENTER => (H, H),
        Position::LFE1 | Position::LFE2 => (0.0, 0.0),
        Position::REAR_LEFT | Position::SIDE_LEFT | Position::FRONT_LEFT_WIDE => (H, 0.0),
        Position::REAR_RIGHT | Position::SIDE_RIGHT | Position::FRONT_RIGHT_WIDE => (0.0, H),
        // Between front left and centre: mostly left.
        Position::FRONT_LEFT_CENTER => (0.924, 0.383),
        Position::FRONT_RIGHT_CENTER => (0.383, 0.924),
        Position::REAR_CENTER => (0.5, 0.5),
        Position::TOP_FRONT_LEFT | Position::BOTTOM_FRONT_LEFT => (H, 0.0),
        Position::TOP_FRONT_RIGHT | Position::BOTTOM_FRONT_RIGHT => (0.0, H),
        Position::TOP_REAR_LEFT | Position::TOP_SIDE_LEFT => (0.5, 0.0),
        Position::TOP_REAR_RIGHT | Position::TOP_SIDE_RIGHT => (0.0, 0.5),
        Position::TOP_CENTER | Position::TOP_FRONT_CENTER | Position::BOTTOM_FRONT_CENTER => (0.5, 0.5),
        Position::TOP_REAR_CENTER => (0.354, 0.354),
        _ => (0.5, 0.5),
    }
}

/// The usual WAVE order for files that do not say (`Channels::Discrete`).
fn guess(n: usize) -> Option<Position> {
    use Position as P;
    Some(match n {
        3 => P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER,
        4 => P::FRONT_LEFT | P::FRONT_RIGHT | P::REAR_LEFT | P::REAR_RIGHT,
        5 => P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER | P::REAR_LEFT | P::REAR_RIGHT,
        6 => P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER | P::LFE1 | P::REAR_LEFT | P::REAR_RIGHT,
        7 => P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER | P::LFE1 | P::REAR_CENTER | P::SIDE_LEFT | P::SIDE_RIGHT,
        8 => P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER | P::LFE1 | P::REAR_LEFT | P::REAR_RIGHT | P::SIDE_LEFT | P::SIDE_RIGHT,
        _ => return None,
    })
}

/// Per input channel (in buffer order), its (left, right) gain.
#[derive(Clone, Debug, PartialEq)]
pub struct Downmix {
    pub gains: Vec<(f32, f32)>,
}

impl Downmix {
    pub fn new(channels: &Channels, count: usize) -> Self {
        let positions = match channels {
            Channels::Positioned(p) if p.bits().count_ones() as usize == count => Some(*p),
            _ => guess(count),
        };
        let mut gains: Vec<(f32, f32)> = match positions {
            // Buffer order is ascending bit order.
            Some(p) => (0..64).map(|b| 1u64 << b).filter(|b| p.bits() & b != 0).map(|b| weights(Position::from_bits_truncate(b))).collect(),
            // Unknown: alternate left and right.
            None => (0..count).map(|i| if i % 2 == 0 { (1.0, 0.0) } else { (0.0, 1.0) }).collect(),
        };
        let (l, r) = gains.iter().fold((0.0f32, 0.0f32), |(l, r), g| (l + g.0, r + g.1));
        let norm = 1.0 / l.max(r).max(1.0);
        for g in &mut gains {
            g.0 *= norm;
            g.1 *= norm;
        }
        Self { gains }
    }

    /// Interleaved frames of `gains.len()` channels, appended to `out` as stereo.
    pub fn apply(&self, src: &[f32], out: &mut Vec<f32>) {
        let n = self.gains.len();
        for frame in src.chunks_exact(n) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for (s, g) in frame.iter().zip(&self.gains) {
                l += s * g.0;
                r += s * g.1;
            }
            out.push(l);
            out.push(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Position as P;

    #[test]
    fn five_one_keeps_the_centre_and_drops_the_lfe() {
        let d = Downmix::new(&Channels::Positioned(P::FRONT_LEFT | P::FRONT_RIGHT | P::FRONT_CENTER | P::LFE1 | P::REAR_LEFT | P::REAR_RIGHT), 6);
        let k = 1.0 / (1.0 + 2.0 * H);
        let want = [(k, 0.0), (0.0, k), (H * k, H * k), (0.0, 0.0), (H * k, 0.0), (0.0, H * k)];
        for (g, w) in d.gains.iter().zip(want) {
            assert!((g.0 - w.0).abs() < 1e-6 && (g.1 - w.1).abs() < 1e-6, "{:?}", d.gains);
        }
    }

    #[test]
    fn full_scale_everywhere_cannot_clip() {
        for (ch, n) in [(Channels::Discrete(6), 6), (Channels::Discrete(8), 8), (Channels::Discrete(7), 7), (Channels::Discrete(3), 3), (Channels::Discrete(12), 12)] {
            let d = Downmix::new(&ch, n);
            let mut out = Vec::new();
            d.apply(&vec![1.0; n * 4], &mut out);
            assert!(out.iter().all(|&s| s <= 1.0 + 1e-6), "{n} channels: {out:?}");
            assert!(out.iter().any(|&s| s > 0.99), "{n} channels: and full scale stays full scale");
        }
    }

    #[test]
    fn unlabelled_layouts_follow_wave_order() {
        let d = Downmix::new(&Channels::Discrete(8), 8);
        assert_eq!(d.gains[3], (0.0, 0.0), "the fourth of eight is the LFE");
        assert!(d.gains[2].0 > 0.0 && d.gains[2].0 == d.gains[2].1, "the third is the centre");
        assert!(d.gains[6].0 > 0.0 && d.gains[6].1 == 0.0, "side left");
    }
}
