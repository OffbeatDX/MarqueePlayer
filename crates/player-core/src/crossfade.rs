//! Crossfades: the curve, and how long an overlap can be.
//!
//! Equal power: the outgoing track at `cos(pi t / 2)` and the incoming one at `sin(pi t /
//! 2)` over the overlap (`t` from 0 to 1), so for unrelated material, whose sum adds in
//! power, the loudness stays level through the change (an equal-gain fade dips by 3 dB in
//! the middle). Each track keeps its own ReplayGain; the sum goes through the equalizer and
//! the limiter as one signal, so the limiter sees the mix.
//!
//! The rules for when tracks crossfade and when they stay gapless are the app's (it knows
//! the albums); the engine only does what it is told per transition
//! (`Player::set_next_with`) and keeps the overlap within each track (`overlap_frames`).

/// Gains of the outgoing and incoming track at frame `i` of an overlap of `len` frames.
pub fn gains(i: u64, len: u64) -> (f32, f32) {
    if len == 0 {
        return (0.0, 1.0);
    }
    let t = (i as f64 + 0.5) / len as f64;
    let a = std::f64::consts::FRAC_PI_2 * t.clamp(0.0, 1.0);
    (a.cos() as f32, a.sin() as f32)
}

/// The shortest overlap worth having: under this a transition is simply gapless.
pub const MIN_SECS: f64 = 0.5;
/// The most of the outgoing track an overlap takes: a fifth, so the track always plays far
/// past three quarters of its length before the incoming one takes over (the play count's
/// rule) and its fade is never most of it.
pub const OUT_SHARE: f64 = 0.2;
/// The most of the incoming track it takes.
pub const IN_SHARE: f64 = 0.2;

/// How many frames the overlap of a `secs` crossfade can be between a track of `out_total`
/// frames, `remaining` of them still to play, and one of `in_total` frames (if known), at
/// `rate`. `None`: too short to be worth it (or the outgoing length is unknown), so the
/// change is gapless.
pub fn overlap_frames(secs: f64, rate: u32, out_total: Option<u64>, remaining: u64, in_total: Option<u64>) -> Option<u64> {
    let out_total = out_total?;
    let rate_f = f64::from(rate);
    let mut len = (secs.max(0.0) * rate_f) as u64;
    len = len.min((out_total as f64 * OUT_SHARE) as u64).min(remaining);
    if let Some(t) = in_total {
        len = len.min((t as f64 * IN_SHARE) as u64);
    }
    (len as f64 >= MIN_SECS * rate_f).then_some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_keeps_the_sum_of_powers_at_one() {
        let len = 48_000;
        for i in [0, 1, 100, 24_000, 47_999] {
            let (o, n) = gains(i, len);
            assert!(((o * o + n * n) - 1.0).abs() < 1e-6);
        }
        let (o, n) = gains(len / 2, len);
        assert!((o - n).abs() < 1e-4, "crossing in the middle");
        assert!(gains(0, len).0 > 0.9999 && gains(len - 1, len).1 > 0.9999);
    }

    #[test]
    fn the_overlap_stays_within_both_tracks() {
        let r = 48_000;
        assert_eq!(overlap_frames(6.0, r, Some(240 * r as u64), 240 * r as u64, Some(200 * r as u64)), Some(6 * r as u64));
        // A 10 s track: at most 2 s of it.
        assert_eq!(overlap_frames(6.0, r, Some(10 * r as u64), 10 * r as u64, None), Some(2 * r as u64));
        // A 2 s track next: at most 0.4 s, too short: gapless.
        assert_eq!(overlap_frames(6.0, r, Some(240 * r as u64), 240 * r as u64, Some(2 * r as u64)), None);
        // Seeked to 3 s before the end: what is left.
        assert_eq!(overlap_frames(6.0, r, Some(240 * r as u64), 3 * r as u64, None), Some(3 * r as u64));
        assert_eq!(overlap_frames(6.0, r, None, 1000, None), None, "length unknown: gapless");
    }
}
