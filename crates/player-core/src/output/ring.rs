//! The ring between the output thread and a pull-driven device's real-time callback (Core
//! Audio, PipeWire), and its accounting. Platform-neutral, so it is tested everywhere.
//!
//! **The ring.** Single producer (the output thread), single consumer (the callback),
//! atomics only: the callback does not allocate, lock, log or block. A cut (seek, play-now,
//! stop) is done by the consumer: it fades out the very next frames it takes and drops the
//! rest.
//!
//! **Fill.** The ring is kept filled by its own depth (`target`), whatever the device holds
//! beyond it: the slack against a late output thread is the ring alone. (Counting what the
//! device holds into the target left a high-latency output, Bluetooth, AirPlay, HDMI, a ring
//! too thin for one callback once the estimate of it had grown to its true size: every
//! callback then came short, for as long as the stream lasted.)
//!
//! **Pipe.** What was handed to the device and is not heard yet (`in_pipe`), for `queued`
//! (positions, drains, the visualizer): after a callback that handed real audio, the
//! device's delay plus those frames, plus what an earlier pull of the same device cycle
//! still has ahead of them (a converter may pull twice in one cycle); it then runs down
//! with the clock. It is set afresh by every callback, never accumulated, so it cannot creep.
//!
//! **Underruns.** The callback counts every cycle that came short (`short`, `lost`); the
//! producer (`Underruns`) tells those that were meant, after a cut, a drain or a stop, from
//! those that were not.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Single producer (the output thread), single consumer (the device's real-time thread).
pub(crate) struct Ring {
    /// Capacity in frames, a power of two.
    frames: usize,
    /// Interleaved stereo f32 as bits: written relaxed, published by `write`'s release.
    buf: Box<[AtomicU32]>,
    /// Frames written in all (producer).
    write: AtomicU64,
    /// Frames taken in all (consumer).
    read: AtomicU64,
    /// A cut (`cut`, `flush`): what was written before it is dropped (after a fade-out of
    /// `cut_len` frames from the next one taken); `cut_seq` says there is a new one.
    skip: AtomicU64,
    cut_len: AtomicU32,
    cut_seq: AtomicU64,
    /// The consumer's own: the cut it last took, its fade (done, length) and its end.
    cut_seen: AtomicU64,
    fade_pos: AtomicU32,
    fade_len: AtomicU32,
    fade_end: AtomicU64,
    /// The stream's sample rate.
    pub rate: u32,
    /// Frames until the last real frame handed to the device is heard, as of `pipe_ns`.
    pipe: AtomicU64,
    pipe_ns: AtomicU64,
    /// The consumer's own: real frames of the last callback.
    last_real: AtomicU64,
    /// Frames (stream rate) between a callback and the first of its frames heard: set at
    /// open, and by every callback.
    pub delay: AtomicU64,
    /// The frames the device asked for last callback.
    pub quantum: AtomicU32,
    /// Callbacks that came short of real audio, and the frames padded with silence.
    short: AtomicU64,
    lost: AtomicU64,
    /// The fewest frames the ring held at the start of a callback since `take_low`.
    low: AtomicU64,
    base: Instant,
    /// The output thread, unparked every callback (`Device::wait` parks).
    waiter: std::thread::Thread,
}

impl Ring {
    /// A ring of `frames` (a power of two) at `rate`; the calling thread is the producer.
    pub fn new(rate: u32, frames: usize) -> Self {
        assert!(frames.is_power_of_two());
        Ring {
            frames,
            buf: (0..frames * 2).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            skip: AtomicU64::new(0),
            cut_len: AtomicU32::new(0),
            cut_seq: AtomicU64::new(0),
            cut_seen: AtomicU64::new(0),
            fade_pos: AtomicU32::new(0),
            fade_len: AtomicU32::new(0),
            fade_end: AtomicU64::new(0),
            rate,
            pipe: AtomicU64::new(0),
            pipe_ns: AtomicU64::new(0),
            last_real: AtomicU64::new(0),
            delay: AtomicU64::new(0),
            quantum: AtomicU32::new(0),
            short: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            low: AtomicU64::new(u64::MAX),
            base: Instant::now(),
            waiter: std::thread::current(),
        }
    }

    pub fn now_ns(&self) -> u64 {
        self.base.elapsed().as_nanos() as u64
    }

    /// Frames in the ring the consumer will still play.
    pub fn in_ring(&self) -> u64 {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire).max(self.skip.load(Ordering::Acquire));
        w.saturating_sub(r)
    }

    /// Frames until the last real frame handed to the device is heard, now.
    pub fn in_pipe(&self) -> u64 {
        self.in_pipe_at(self.now_ns())
    }

    fn in_pipe_at(&self, now: u64) -> u64 {
        let pipe = self.pipe.load(Ordering::Acquire);
        let at = self.pipe_ns.load(Ordering::Acquire);
        let played = now.saturating_sub(at) as u128 * u128::from(self.rate) / 1_000_000_000;
        pipe.saturating_sub(played as u64)
    }

    /// Room the producer may use without overwriting what the consumer has not taken.
    pub fn room(&self) -> u64 {
        let w = self.write.load(Ordering::Relaxed);
        self.frames as u64 - w.saturating_sub(self.read.load(Ordering::Acquire))
    }

    /// What the ring is kept filled to: `ms`, or `periods` callbacks, whichever is more,
    /// within half its capacity.
    pub fn target(&self, ms: u32, periods: u64) -> u64 {
        let quantum = u64::from(self.quantum.load(Ordering::Relaxed));
        (u64::from(self.rate) * u64::from(ms) / 1000).max(quantum * periods).min(self.frames as u64 / 2)
    }

    /// Frames the producer may write now to fill the ring to `target`.
    pub fn wanted(&self, target: u64) -> u64 {
        target.saturating_sub(self.in_ring()).min(self.room())
    }

    /// Producer: append interleaved stereo samples. False when they do not fit.
    pub fn push(&self, samples: &[f32]) -> bool {
        let frames = samples.len() / 2;
        if frames as u64 > self.room() {
            return false;
        }
        let w = self.write.load(Ordering::Relaxed);
        let mask = self.frames as u64 - 1;
        for (i, f) in samples.as_chunks::<2>().0.iter().enumerate() {
            let at = (((w + i as u64) & mask) * 2) as usize;
            self.buf[at].store(f[0].to_bits(), Ordering::Relaxed);
            self.buf[at + 1].store(f[1].to_bits(), Ordering::Relaxed);
        }
        self.write.store(w + frames as u64, Ordering::Release);
        true
    }

    /// Producer: drop everything not taken yet, the first `fade` frames of it faded out
    /// (they follow what the device has, so the cut is continuous).
    pub fn cut(&self, fade: usize) {
        self.cut_len.store(fade as u32, Ordering::Relaxed);
        self.skip.store(self.write.load(Ordering::Relaxed), Ordering::Release);
        self.cut_seq.fetch_add(1, Ordering::Release);
    }

    /// Producer: drop everything not taken yet, at once (the device is stopped and drops
    /// what it holds too).
    pub fn flush(&self) {
        self.cut(0);
        self.pipe.store(0, Ordering::Release);
    }

    /// Consumer (real-time): up to `frames` frames of the ring, each handed to `put` with its
    /// index. Returns how many; the caller pads the rest with silence.
    pub fn take(&self, frames: usize, mut put: impl FnMut(usize, f32, f32)) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let mut r = self.read.load(Ordering::Relaxed);
        let held = w.saturating_sub(r.max(self.skip.load(Ordering::Acquire)));
        if held < self.low.load(Ordering::Relaxed) {
            self.low.store(held, Ordering::Relaxed);
        }
        // A new cut: its fade starts at the next frame taken, then the rest is dropped.
        let seq = self.cut_seq.load(Ordering::Acquire);
        if seq != self.cut_seen.load(Ordering::Relaxed) {
            self.cut_seen.store(seq, Ordering::Relaxed);
            let end = self.skip.load(Ordering::Acquire);
            let len = u64::from(self.cut_len.load(Ordering::Relaxed)).min(end.saturating_sub(r)) as u32;
            self.fade_len.store(len, Ordering::Relaxed);
            self.fade_pos.store(0, Ordering::Relaxed);
            self.fade_end.store(end, Ordering::Relaxed);
        }
        let (fade_len, end) = (self.fade_len.load(Ordering::Relaxed), self.fade_end.load(Ordering::Relaxed));
        let mut fade_pos = self.fade_pos.load(Ordering::Relaxed);
        let mask = self.frames as u64 - 1;
        let mut n = 0;
        while n < frames {
            let gain = if fade_pos < fade_len && r < end {
                fade_pos += 1;
                // Raised cosine, 1 to 0, as the output thread's own fades.
                0.5 + 0.5 * (std::f32::consts::PI * fade_pos as f32 / fade_len as f32).cos()
            } else {
                if r < end {
                    r = end;
                    fade_pos = fade_len;
                }
                if r >= w {
                    break;
                }
                1.0
            };
            let at = ((r & mask) * 2) as usize;
            put(n, f32::from_bits(self.buf[at].load(Ordering::Relaxed)) * gain, f32::from_bits(self.buf[at + 1].load(Ordering::Relaxed)) * gain);
            r += 1;
            n += 1;
        }
        if fade_pos >= fade_len && r < end {
            r = end;
        }
        self.fade_pos.store(fade_pos, Ordering::Relaxed);
        self.read.store(r, Ordering::Release);
        n
    }

    /// Consumer (real-time): after a callback that handed `real` frames of audio (of
    /// `want`), with `delay` frames between the callback and the first of them heard.
    pub fn after_cycle(&self, real: usize, want: usize, delay: u64) {
        self.after_cycle_at(real, want, delay, self.now_ns());
        self.unpark();
    }

    fn after_cycle_at(&self, real: usize, want: usize, delay: u64, now: u64) {
        let (real, want) = (real as u64, want as u64);
        self.delay.store(delay, Ordering::Relaxed);
        if real > 0 {
            // An earlier pull of the same device cycle (less than half a pull ago) plays
            // first: what it has ahead of the delay, at most its own length. Otherwise the
            // pipe is set afresh from the delay, so jitter cannot add up.
            let last = self.last_real.load(Ordering::Relaxed);
            let since = now.saturating_sub(self.pipe_ns.load(Ordering::Relaxed)) as u128 * u128::from(self.rate) / 1_000_000_000;
            let ahead = if (since as u64) < last / 2 { self.in_pipe_at(now).saturating_sub(delay).min(last) } else { 0 };
            self.pipe.store(delay + ahead + real, Ordering::Release);
            self.pipe_ns.store(now, Ordering::Release);
        }
        self.last_real.store(real, Ordering::Relaxed);
        self.quantum.store(want as u32, Ordering::Relaxed);
        if real < want {
            self.short.fetch_add(1, Ordering::Relaxed);
            self.lost.fetch_add(want - real, Ordering::Relaxed);
        }
    }

    /// Wake the producer (it parks in `Device::wait`), as a callback does.
    pub fn unpark(&self) {
        self.waiter.unpark();
    }

    /// Producer: the fewest frames the ring held at a callback's start since the last call
    /// (`None`: no callback since).
    pub fn take_low(&self) -> Option<u64> {
        let low = self.low.swap(u64::MAX, Ordering::Relaxed);
        (low != u64::MAX).then_some(low)
    }
}

/// The producer's count of underruns: callbacks that came short while audio was due. Those
/// after a cut (until the new audio is written), a drain or a stop were meant and are not
/// counted. Kept across reopens by the device.
#[derive(Default)]
pub(crate) struct Underruns {
    /// The ring's `short` and `lost` as last looked at.
    seen: (u64, u64),
    /// Short callbacks are meant until the next write.
    excused: bool,
    pub count: u64,
    pub lost: u64,
}

impl Underruns {
    /// A new ring: nothing seen of it yet, and nothing due until the first write.
    pub fn opened(&mut self) {
        self.seen = (0, 0);
        self.excused = true;
    }

    /// Count what came short since the last look, unless it was meant.
    pub fn poll(&mut self, ring: &Ring) {
        let now = (ring.short.load(Ordering::Relaxed), ring.lost.load(Ordering::Relaxed));
        if !self.excused {
            self.count += now.0.saturating_sub(self.seen.0);
            self.lost += now.1.saturating_sub(self.seen.1);
        }
        self.seen = now;
    }

    /// A cut, stop, flush or drain: what comes short from now until the next write is meant.
    /// What came short before it was not.
    pub fn excuse(&mut self, ring: &Ring) {
        self.poll(ring);
        self.excused = true;
    }

    /// The device stopped (after a drain or a pause, or a cut's hold): what came short since
    /// the last look was the ring running out on purpose.
    pub fn stopped(&mut self, ring: &Ring) {
        self.excused = true;
        self.poll(ring);
    }

    /// Audio was written: from now on a short callback is an underrun.
    pub fn wrote(&mut self, ring: &Ring) {
        if self.excused {
            self.poll(ring);
            self.excused = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: usize = 1 << 15;

    fn take_vec(r: &Ring, frames: usize) -> (usize, Vec<f32>) {
        let mut out = vec![0f32; frames * 2];
        let n = r.take(frames, |i, a, b| {
            out[i * 2] = a;
            out[i * 2 + 1] = b;
        });
        (n, out)
    }

    #[test]
    fn ring_hands_over_in_order() {
        let r = Ring::new(48_000, FRAMES);
        let samples: Vec<f32> = (0..20).map(|i| i as f32).collect();
        assert!(r.push(&samples));
        assert_eq!(r.in_ring(), 10);
        let (n, out) = take_vec(&r, 16);
        assert_eq!(n, 10);
        assert_eq!(&out[..20], &samples[..]);
        assert_eq!(r.in_ring(), 0);
    }

    #[test]
    fn a_cut_fades_out_from_the_next_frame_then_drops_the_rest() {
        let r = Ring::new(48_000, FRAMES);
        assert!(r.push(&vec![1.0f32; 2 * 100]));
        assert_eq!(take_vec(&r, 10).0, 10);
        r.cut(4);
        assert!(r.push(&[0.5, 0.5, 0.5, 0.5]));
        let (n, out) = take_vec(&r, 8);
        assert_eq!(n, 6);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(left[0] < 1.0 && left[0] > left[1] && left[1] > left[2] && left[2] > left[3], "{left:?}");
        assert!(left[3].abs() < 1e-6, "faded to silence: {left:?}");
        assert_eq!(&left[4..6], [0.5, 0.5], "then the new audio");
    }

    #[test]
    fn a_cut_with_fewer_frames_left_scales_the_fade_to_zero() {
        let r = Ring::new(48_000, FRAMES);
        assert!(r.push(&vec![1.0f32; 2 * 10]));
        assert_eq!(take_vec(&r, 8).0, 8);
        // Only 2 frames remain, fewer than the requested 8-frame fade.
        r.cut(8);
        assert!(r.push(&[0.5, 0.5, 0.5, 0.5]));
        let (n, out) = take_vec(&r, 8);
        assert_eq!(n, 4);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!((left[0] - 0.5).abs() < 1e-6, "half-way through a 2-frame fade: {left:?}");
        assert!(left[1].abs() < 1e-6, "fade must reach zero before the new audio: {left:?}");
        assert_eq!(&left[2..4], [0.5, 0.5], "then the new audio");

        // One frame left: fades to zero on that frame.
        let r = Ring::new(48_000, FRAMES);
        assert!(r.push(&[1.0, 1.0]));
        r.cut(8);
        assert!(r.push(&[0.5, 0.5]));
        let (n, out) = take_vec(&r, 4);
        assert_eq!(n, 2);
        assert!(out[0].abs() < 1e-6 && out[1].abs() < 1e-6, "single frame reaches zero: {out:?}");
        assert_eq!(&out[2..4], [0.5, 0.5]);

        // Zero frames left: no fade frames, new audio plays straight away.
        let r = Ring::new(48_000, FRAMES);
        r.cut(8);
        assert!(r.push(&[0.5, 0.5]));
        let (n, out) = take_vec(&r, 4);
        assert_eq!(n, 1);
        assert_eq!(&out[..2], [0.5, 0.5]);
    }

    #[test]
    fn a_flush_skips_what_was_not_taken_and_wraps() {
        let r = Ring::new(48_000, FRAMES);
        let block = vec![0.5f32; 2 * 10_000];
        for _ in 0..5 {
            assert!(r.push(&block));
            assert_eq!(take_vec(&r, 10_000).0, 10_000);
        }
        assert!(r.push(&[1.0, 1.0, 2.0, 2.0]));
        r.flush();
        assert_eq!(r.in_ring(), 0);
        assert!(r.push(&[3.0, -3.0]));
        let (n, out) = take_vec(&r, 4);
        assert_eq!(n, 1);
        assert_eq!(&out[..2], [3.0, -3.0]);
    }

    #[test]
    fn the_ring_refuses_more_than_it_holds() {
        let r = Ring::new(48_000, FRAMES);
        assert!(r.push(&vec![0.0; FRAMES * 2]));
        assert!(!r.push(&[0.0, 0.0]));
    }

    #[test]
    fn what_was_handed_over_drains_as_time_passes() {
        let r = Ring::new(48_000, FRAMES);
        r.after_cycle_at(960, 960, 960, 0);
        assert_eq!(r.in_pipe_at(0), 1920, "the delay, then the frames handed");
        assert_eq!(r.in_pipe_at(20_000_000), 960);
        assert_eq!(r.in_pipe_at(45_000_000), 0);
        // Silence after real audio adds nothing.
        r.after_cycle_at(0, 960, 960, 45_000_000);
        assert_eq!(r.in_pipe_at(45_000_000), 0);
    }

    /// A tiny deterministic generator, for jitter.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// The device side of a replay: callbacks of `want` frames every period, early or late by
    /// up to `jitter` frames, now and then two pulls in one cycle (a converter); the output
    /// thread tops the ring up after each, except for `stall_ms` from second `track_start`
    /// (a burst of work as a track starts). Returns the underruns of each 10 s, and the pipe
    /// estimate's range after the first second.
    fn replay(rate: u32, want: usize, delay: u64, jitter: f64, stall_ms: u64, secs: u64, track_start: u64) -> (Vec<u64>, (u64, u64)) {
        replay_with(rate, want, delay, jitter, stall_ms, secs, track_start, false)
    }

    /// `count_pipe`: fill to the target less what the device holds as well (the old rule).
    #[allow(clippy::too_many_arguments)]
    fn replay_with(rate: u32, want: usize, delay: u64, jitter: f64, stall_ms: u64, secs: u64, track_start: u64, count_pipe: bool) -> (Vec<u64>, (u64, u64)) {
        let r = Ring::new(rate, FRAMES);
        let mut u = Underruns::default();
        u.opened();
        let mut rnd = Lcg(7);
        let block = vec![0.25f32; FRAMES];
        let write = |r: &Ring, u: &mut Underruns| {
            let target = r.target(100, 4);
            let n = if count_pipe { r.wanted(target.saturating_sub(r.in_pipe_at(r.pipe_ns.load(Ordering::Relaxed)))) } else { r.wanted(target) } as usize;
            if n > 0 {
                assert!(r.push(&block[..n * 2]));
                u.wrote(r);
            }
        };
        write(&r, &mut u);
        let period_ns = want as f64 * 1e9 / f64::from(rate);
        let cycles = (secs as f64 * 1e9 / period_ns) as u64;
        let stall = track_start * 1_000_000_000..(track_start * 1000 + stall_ms) * 1_000_000;
        let mut per_10s = Vec::new();
        let (mut lo, mut hi) = (u64::MAX, 0);
        let mut counted = 0;
        for k in 0..cycles {
            let t = (k as f64 * period_ns + (rnd.next() * 2.0 - 1.0) * jitter * 1e9 / f64::from(rate)).max(0.0) as u64;
            let pulls = if rnd.next() < 0.1 { 2 } else { 1 };
            for p in 0..pulls {
                let w = if pulls == 2 { want / 2 + p } else { want };
                let n = r.take(w, |_, _, _| {});
                r.after_cycle_at(n, w, delay, t);
            }
            if !stall.contains(&t) {
                write(&r, &mut u);
            }
            u.poll(&r);
            if t >= 1_000_000_000 {
                let p = r.in_pipe_at(t);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            if t / 10_000_000_000 > per_10s.len() as u64 {
                per_10s.push(u.count - counted);
                counted = u.count;
            }
        }
        (per_10s, (lo, hi))
    }

    /// Bluetooth-sized delays, a 44.1 kHz stream through a converter, a 60 ms stall of the
    /// output thread as a track starts: the ring is filled by its own depth, so no callback
    /// comes short, however long the stream runs, and the pipe estimate stays within one pull
    /// of the delay (it does not creep).
    #[test]
    fn high_latency_outputs_never_starve_and_the_estimate_does_not_creep() {
        for delay in [500, 1300, 4000, 9000] {
            let (per_10s, (lo, hi)) = replay(44_100, 470, delay, 40.0, 60, 120, 30);
            assert!(per_10s.len() >= 11 && per_10s.iter().all(|&n| n == 0), "delay {delay}: underruns each 10 s: {per_10s:?}");
            assert!(lo >= delay && hi <= delay + 2 * 470, "delay {delay}: the pipe estimate ran {lo}..{hi}");
        }
    }

    /// The old rule, the ring filled to its target less what the device holds: once the
    /// estimate of the latter is near its true size, a high-latency output gets a sliver per
    /// callback and comes short every few callbacks, for as long as the stream lasts.
    #[test]
    fn counting_the_device_into_the_fill_starves_for_good() {
        let (per_10s, _) = replay_with(44_100, 470, 4000, 40.0, 0, 60, 0, true);
        assert!(per_10s.iter().skip(1).all(|&n| n > 100), "{per_10s:?}");
    }

    /// A stall longer than the ring holds is an underrun, counted, and the ring recovers at
    /// once: the next callbacks are whole again.
    #[test]
    fn a_stall_past_the_ring_is_counted_and_recovered_from() {
        let (per_10s, _) = replay(44_100, 470, 4000, 40.0, 150, 60, 30);
        assert!(per_10s[3] > 0, "{per_10s:?}");
        assert!(per_10s.iter().enumerate().all(|(i, &n)| i == 3 || n == 0), "{per_10s:?}");
    }

    #[test]
    fn short_callbacks_are_underruns_only_while_audio_is_due() {
        let r = Ring::new(48_000, FRAMES);
        let mut u = Underruns::default();
        u.opened();
        // Before the first write nothing is due.
        r.after_cycle_at(0, 512, 0, 0);
        assert!(r.push(&[0.0; 2 * 600]));
        u.wrote(&r);
        u.poll(&r);
        assert_eq!(u.count, 0);
        // Playing: a short callback is an underrun, with its frames lost.
        let n = r.take(512, |_, _, _| {});
        r.after_cycle_at(n, 512, 0, 1);
        let n = r.take(512, |_, _, _| {});
        r.after_cycle_at(n, 512, 0, 2);
        u.poll(&r);
        assert_eq!((u.count, u.lost), (1, 424));
        // A cut: what comes short until the new audio is written is meant.
        r.cut(4);
        u.excuse(&r);
        r.after_cycle_at(0, 512, 0, 3);
        u.poll(&r);
        assert_eq!(u.count, 1);
        assert!(r.push(&[0.0; 2 * 512]));
        u.wrote(&r);
        let n = r.take(512, |_, _, _| {});
        r.after_cycle_at(n, 512, 0, 4);
        r.after_cycle_at(0, 512, 0, 5);
        // The end of the queue: the ring runs out on purpose, then the device is stopped.
        u.stopped(&r);
        assert_eq!(u.count, 1, "a drain's short callbacks are not underruns");
    }

    #[test]
    fn the_lowest_fill_is_kept_until_read() {
        let r = Ring::new(48_000, FRAMES);
        assert_eq!(r.take_low(), None);
        assert!(r.push(&[0.0; 2 * 100]));
        r.take(60, |_, _, _| {});
        r.take(60, |_, _, _| {});
        assert_eq!(r.take_low(), Some(40));
        assert_eq!(r.take_low(), None);
    }
}
