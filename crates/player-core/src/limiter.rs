//! Peak limiter for "hot" masters: tracks whose decoded samples go above full scale
//! (a lossy encode of a loudly mastered track, or a positive ReplayGain or preamp), which
//! would otherwise clip at the output.
//!
//! It is transparent: while every sample stays under the ceiling the gain is exactly 1
//! and the samples are not touched (bit-identical output). Only around an over does the
//! gain dip, just enough to keep the peak under the ceiling: it eases in over 1 ms
//! (`attack_frames`) before the peak and recovers over ~50 ms after it, which sounds far cleaner
//! than clipping the waveform flat.
//!
//! It runs on frames already decoded, before they are written: on the decode thread, which
//! works ahead of playback, or on the output thread after the equalizer or a crossfade's mix
//! (`Chunk::limited`). Its look-ahead reads frames that are already there and adds no delay:
//! nothing shifts in time, seeking is as fast as before, and the gain state carries across
//! tracks (gapless). The look-ahead does not reach into the next chunk (or block); a peak in a chunk's first frames gets a shorter
//! ramp, still never above the ceiling.

/// Highest allowed sample value: just under full scale, so only real overs are touched.
pub const CEILING: f32 = 0.9999;
/// Frames over which the gain eases down before a peak at 48 kHz (1 ms).
const ATTACK: usize = 48;

/// Frames over which the gain eases down before a peak at `rate`: the same 1 ms at every
/// rate from 48 kHz up (96 at 96 kHz, 192 at 192 kHz; it was 48 frames everywhere, a
/// quarter of a millisecond at 192 kHz). Below 48 kHz it stays 48 frames (1.09 ms at
/// 44.1 kHz), so what 44.1 kHz tracks sound like does not change.
fn attack_frames(rate: u32) -> usize {
    ((rate as usize * ATTACK + 24_000) / 48_000).max(ATTACK)
}
/// Recovery time constant after a peak.
const RELEASE_SECS: f32 = 0.05;

#[derive(Debug, Clone)]
pub struct Limiter {
    /// Gain at the end of the previous chunk (1.0 when idle).
    gain: f32,
    scratch: Vec<f32>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self { gain: 1.0, scratch: Vec::new() }
    }
}

impl Limiter {
    /// Forget the gain state (after a seek: the stream restarts elsewhere).
    pub fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// Room for `frames` frames a call, so processing that much never allocates (the
    /// output thread's limiter).
    pub fn with_capacity(frames: usize) -> Self {
        Self { gain: 1.0, scratch: Vec::with_capacity(frames) }
    }

    /// Not recovering from a peak: with nothing over the ceiling, it changes nothing.
    pub fn is_idle(&self) -> bool {
        self.gain >= 1.0
    }

    /// Limit interleaved stereo `samples` in place, given the linear gain `pre` that the
    /// output will apply to them (ReplayGain and preamp). Returns true when any sample was
    /// changed.
    pub fn process(&mut self, samples: &mut [f32], pre: f32, rate: u32) -> bool {
        let frames = samples.len() / 2;
        if frames == 0 {
            return false;
        }
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs())) * pre.abs();
        if peak <= CEILING && self.gain >= 1.0 {
            // The common case: nothing near full scale, nothing recovering.
            return false;
        }
        let release = 1.0 - (-1.0 / (RELEASE_SECS * rate.max(1) as f32)).exp();
        // Gain each frame needs so that it stays under the ceiling.
        let g = &mut self.scratch;
        g.clear();
        g.extend(samples.as_chunks::<2>().0.iter().map(|f| {
            let p = f[0].abs().max(f[1].abs()) * pre.abs();
            // A hair under the ceiling, so float rounding of `x * g` cannot cross it.
            if p > CEILING { CEILING * 0.99999 / p } else { 1.0 }
        }));
        // Ease in before each peak: the gain may fall by at most 1/attack per frame,
        // walking backwards from the peak.
        let step = 1.0 / attack_frames(rate) as f32;
        for i in (0..frames.saturating_sub(1)).rev() {
            g[i] = g[i].min(g[i + 1] + step);
        }
        // Recover after each peak: from the previous level, rise toward 1 at the release
        // rate, never above what the frame needs.
        let mut level = self.gain;
        let mut changed = false;
        for (i, f) in samples.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            level = (level + (1.0 - level) * release).min(g[i]);
            // Within 0.009 dB of unity and this frame needs nothing: back to exactly 1
            // (bit-transparent again). (Before, a frame that needed less than 0.009 dB of
            // reduction got none, so overs up to 1.0009 went through.)
            if level >= 0.999 && g[i] >= 1.0 {
                level = 1.0;
                continue;
            }
            f[0] *= level;
            f[1] *= level;
            changed = true;
        }
        self.gain = level;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, amp: f32, freq: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let s = amp * (n as f32 * freq * std::f32::consts::TAU / 48_000.0).sin();
                [s, s * 0.8]
            })
            .collect()
    }

    #[test]
    fn transparent_under_the_ceiling() {
        let mut l = Limiter::default();
        let original = sine(48_000, 0.999, 440.0);
        let mut s = original.clone();
        assert!(!l.process(&mut s, 1.0, 48_000));
        assert_eq!(s, original, "bit-identical");
        // A gain that keeps it under the ceiling is also untouched.
        let mut s = sine(4096, 0.5, 1000.0);
        let before = s.clone();
        assert!(!l.process(&mut s, 1.9, 48_000));
        assert_eq!(s, before);
    }

    #[test]
    fn overs_are_held_under_the_ceiling() {
        let mut l = Limiter::default();
        // +2.8 dBFS, as measured on a real hot master.
        let mut s = sine(48_000, 1.38, 440.0);
        assert!(l.process(&mut s, 1.0, 48_000));
        let peak = s.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak <= CEILING, "peak {peak}");
        // Not crushed: the limiter only takes off what is needed (~2.8 dB here).
        let rms_in = (sine(48_000, 1.38, 440.0).iter().map(|x| x * x).sum::<f32>() / 96_000.0).sqrt();
        let rms_out = (s.iter().map(|x| x * x).sum::<f32>() / 96_000.0).sqrt();
        assert!(rms_out / rms_in > 0.68, "{}", rms_out / rms_in);
    }

    #[test]
    fn overs_from_replaygain_or_preamp_are_caught() {
        let mut l = Limiter::default();
        let mut s = sine(4096, 0.8, 1000.0);
        // +6 dB of gain would put it at 1.6.
        assert!(l.process(&mut s, 2.0, 48_000));
        assert!(s.iter().all(|x| (x * 2.0).abs() <= CEILING + 1e-6));
    }

    #[test]
    fn a_single_click_is_reduced_smoothly_and_recovers() {
        let mut l = Limiter::default();
        let mut s = vec![0.5f32; 9600 * 2];
        s[4800 * 2] = 2.0;
        s[4800 * 2 + 1] = 2.0;
        l.process(&mut s, 1.0, 48_000);
        assert!(s[4800 * 2] <= CEILING);
        // Eased in before the peak (no step), untouched well before it.
        assert_eq!(s[4700 * 2], 0.5);
        // Half-way down the 24-frame ramp to the needed 0.5: about 0.75 of 0.5.
        let before = s[(4800 - 12) * 2];
        assert!(before < 0.45 && before > 0.3, "{before}");
        assert_eq!(s[(4800 - 30) * 2], 0.5, "the ramp starts only just before the peak");
        // Back to unity long after it (release ~50 ms), and across the next chunk.
        let mut next = vec![0.5f32; 19200 * 2];
        l.process(&mut next, 1.0, 48_000);
        assert_eq!(*next.last().unwrap(), 0.5);
        assert!(next[0] < 0.5, "recovery continues into the next chunk");
    }

    /// The ease-in before a peak is the same time at every rate (at 192 kHz it was a
    /// quarter of it), and 44.1 and 48 kHz are as they were.
    #[test]
    fn the_attack_is_the_same_time_at_every_rate() {
        assert_eq!([44_100, 48_000, 88_200, 96_000, 192_000, 384_000].map(attack_frames), [48, 48, 88, 96, 192, 384]);
        for rate in [44_100u32, 48_000, 96_000, 192_000] {
            let mut l = Limiter::default();
            let n = rate as usize / 10;
            let mut s = vec![0.5f32; n * 2];
            s[n] = 2.0;
            s[n + 1] = 2.0;
            l.process(&mut s, 1.0, rate);
            let before = |frames: usize| s[(n / 2 - frames) * 2];
            let attack = attack_frames(rate);
            // The gain falls 1/attack a frame: to the 0.5 needed in attack/2 frames.
            let half_way = before(attack / 4);
            assert!(half_way < 0.45 && half_way > 0.3, "{rate}: half-way down the ramp: {half_way}");
            assert_eq!(before(attack / 2 + 2), 0.5, "{rate}: untouched before the ramp");
        }
    }
}
