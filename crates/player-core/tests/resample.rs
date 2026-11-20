//! The sample-rate converter measured (`player_core::resample`), for every pair of the rates
//! the engine meets: 44.1, 48, 88.2, 96, 176.4 and 192 kHz both ways (and 384 and 768 kHz
//! for the length, alignment and cost checks). Each check asserts its number and prints it
//! (`cargo test -p player-core --release --test resample -- --nocapture`).
//!
//! The signals are generated in f64 and rounded to f32, as decoded audio is; levels are read
//! with a DFT at exact frequencies over one second (integer hertz: a whole number of cycles,
//! so the bins do not leak into one another), which puts the measurement's own floor below
//! -190 dB.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use player_core::resample::{BLOCK, Filter, Resampler};

/// Counts this thread's allocations, to show the converter makes none once running.
struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        unsafe { System.realloc(ptr, layout, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// One test at a time, so the cost measured is the converter's alone.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

const RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

fn pairs() -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for &a in &RATES {
        for &b in &RATES {
            if a != b {
                v.push((a, b));
            }
        }
    }
    v
}

fn khz(r: u32) -> String {
    let k = f64::from(r) / 1000.0;
    if k.fract() == 0.0 { format!("{k:.0}") } else { format!("{k:.1}") }
}

fn filter(from: u32, to: u32) -> Arc<Filter> {
    Arc::new(Filter::new(from, to).expect("a filter for every usual pair"))
}

/// Stereo, both channels the same: the sum of `tones` (hertz, amplitude), `secs` long.
fn signal(rate: u32, tones: &[(f64, f64)], secs: f64) -> Vec<f32> {
    let n = (secs * f64::from(rate)) as usize;
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let s: f64 = tones.iter().map(|&(f, a)| a * (2.0 * PI * f * t).sin()).sum();
        v.push(s as f32);
        v.push(s as f32);
    }
    v
}

/// The left channel over one second, from `skip` frames in.
fn window(v: &[f32], rate: u32, skip: usize) -> Vec<f64> {
    v.as_chunks::<2>().0.iter().skip(skip).take(rate as usize).map(|f| f64::from(f[0])).collect()
}

/// Amplitude and phase at `f` hertz (integer) of one second `x` at `rate`.
fn dft(x: &[f64], rate: u32, f: f64) -> (f64, f64) {
    let w = 2.0 * PI * f / f64::from(rate);
    // Recurrence for cos/sin of w*i, re-anchored now and then so it stays exact to ~1e-16.
    let (mut re, mut im) = (0.0, 0.0);
    let (cw, sw) = (w.cos(), w.sin());
    let (mut c, mut s) = (1.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        if i % 1024 == 0 {
            c = (w * i as f64).cos();
            s = (w * i as f64).sin();
        }
        re += v * c;
        im += v * s;
        (c, s) = (c * cw - s * sw, s * cw + c * sw);
    }
    let n = x.len() as f64;
    let (a, b) = (2.0 * re / n, 2.0 * im / n);
    ((a * a + b * b).sqrt(), b.atan2(a))
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Where an input tone at `f` can leave something in the output besides itself: its images
/// (k fin +- f) and aliases, folded into the output's band. Integer hertz.
fn spurs(fin: u32, fout: u32, f: f64, l: usize) -> Vec<f64> {
    let (fin, fout) = (f64::from(fin), f64::from(fout));
    let fold = |g: f64| {
        let g = g.rem_euclid(fout);
        if g > fout / 2.0 { fout - g } else { g }
    };
    let mut v = Vec::new();
    for k in 0..=(l / 2 + 2) {
        for g in [k as f64 * fin + f, k as f64 * fin - f] {
            let g = fold(g);
            let wanted = f < fout / 2.0 && (g - f).abs() < 0.5;
            if !wanted && g > 0.5 && g < fout / 2.0 - 0.5 && !v.iter().any(|&x: &f64| (x - g).abs() < 0.5) {
                v.push(g);
            }
        }
    }
    v
}

#[test]
fn the_passband_is_flat_and_nothing_else_comes_out() {
    let _serial = serial();
    let mut worst_ripple: f64 = 0.0;
    let mut worst_spur: f64 = -400.0;
    eprintln!("pair              taps  passband ripple (dB)  largest image/alias (dB)");
    for (from, to) in pairs() {
        let f = filter(from, to);
        let (edge, _) = f.edges();
        // Eleven tones across the passband, from 0.1% of its edge to 99.9%, 0.05 each.
        let tones: Vec<(f64, f64)> = [0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99, 0.999]
            .iter()
            .map(|q| ((edge * q).floor().max(20.0), 0.05))
            .collect();
        let out = Resampler::convert(f.clone(), &signal(from, &tones, 1.5));
        let w = window(&out, to, (to / 4) as usize);
        let mut ripple: f64 = 0.0;
        let mut spur: f64 = -400.0;
        for &(t, a) in &tones {
            ripple = ripple.max(db(dft(&w, to, t).0 / a).abs());
            // An image landing on another of the tones cannot be told from it: left out.
            for g in spurs(from, to, t, f.ratio().0.min(96)).into_iter().filter(|g| tones.iter().all(|&(x, _)| (x - g).abs() > 0.5)) {
                spur = spur.max(db(dft(&w, to, g).0 / a));
            }
        }
        let shown = if spur < -390.0 { "none".to_owned() } else { format!("{spur:.1}") };
        eprintln!("{:>5} -> {:<5} kHz  {:>4}  {ripple:>20.8}  {shown:>24}", khz(from), khz(to), f.taps());
        worst_ripple = worst_ripple.max(ripple);
        worst_spur = worst_spur.max(spur);
    }
    eprintln!("worst: ripple {worst_ripple:.8} dB, image or alias of a passband tone {worst_spur:.1} dB");
    assert!(worst_ripple < 0.00001, "passband flat within 0.00001 dB: {worst_ripple}");
    assert!(worst_spur < -150.0, "every image and alias at least 150 dB down: {worst_spur}");
}

#[test]
fn what_is_above_the_lower_nyquist_does_not_alias() {
    let _serial = serial();
    // Down-conversions: tones from just above the output's Nyquist frequency up to the
    // input's, each of which would fold back into the band.
    let mut worst: f64 = -400.0;
    eprintln!("pair              alias of tones above the output's Nyquist (dB)");
    for (from, to) in pairs().into_iter().filter(|(a, b)| a > b) {
        let f = filter(from, to);
        let nyq = f64::from(to) / 2.0;
        let top = f64::from(from) / 2.0;
        let tones: Vec<f64> = [0.0005, 0.01, 0.05, 0.2, 0.5, 0.8, 0.99].iter().map(|q| (nyq + (top - nyq) * q).floor()).collect();
        let mut level: f64 = -400.0;
        for &t in &tones {
            let out = Resampler::convert(f.clone(), &signal(from, &[(t, 0.5)], 1.5));
            let w = window(&out, to, (to / 4) as usize);
            for g in spurs(from, to, t, f.ratio().0.min(96)) {
                level = level.max(db(dft(&w, to, g).0 / 0.5));
            }
        }
        eprintln!("{:>5} -> {:<5} kHz  {level:>8.1}", khz(from), khz(to));
        worst = worst.max(level);
    }
    eprintln!("worst alias: {worst:.1} dB");
    assert!(worst < -150.0, "rejected by at least 150 dB: {worst}");
}

/// Everything in `w` that is not the tone at `f`, relative to it (THD+N).
fn thd_n(w: &[f64], rate: u32, f: f64) -> f64 {
    let (a, ph) = dft(w, rate, f);
    let mut err = 0.0;
    for (i, &v) in w.iter().enumerate() {
        let ideal = a * (2.0 * PI * f * i as f64 / f64::from(rate) + PI / 2.0 - ph).sin();
        // dft's phase is atan2(sin part, cos part): v = a cos(wt - ph) = a sin(wt + pi/2 - ph).
        err += (v - ideal) * (v - ideal);
    }
    let rms = (err / w.len() as f64).sqrt();
    db(rms / (a / 2f64.sqrt()))
}

#[test]
fn a_full_scale_tone_comes_out_with_thd_n_near_the_24_bit_floor() {
    let _serial = serial();
    let mut worst: f64 = -400.0;
    eprintln!("pair              THD+N of a full-scale 1 kHz tone (dB)   (the tone in f32, before: below)");
    let mut input_floor: f64 = -400.0;
    for &r in &RATES {
        let x = signal(r, &[(1000.0, 1.0)], 1.0);
        input_floor = input_floor.max(thd_n(&window(&x, r, 0), r, 1000.0));
    }
    for (from, to) in pairs() {
        let out = Resampler::convert(filter(from, to), &signal(from, &[(1000.0, 1.0)], 1.5));
        let t = thd_n(&window(&out, to, (to / 4) as usize), to, 1000.0);
        eprintln!("{:>5} -> {:<5} kHz  {t:>8.1}", khz(from), khz(to));
        worst = worst.max(t);
    }
    eprintln!("worst THD+N {worst:.1} dB; the f32 input tone itself measures {input_floor:.1} dB; 24-bit's floor is -146.3 dB");
    assert!(worst < -140.0, "far below 16-bit's floor (-98 dB), near 24-bit's (-146 dB): {worst}");
}

#[test]
fn the_impulse_response_is_symmetric_and_aligned() {
    let _serial = serial();
    eprintln!("pair              peak at  pre-ringing  post-ringing  (to -120 dB, ms)");
    for (from, to) in pairs() {
        let f = filter(from, to);
        let (l, m) = f.ratio();
        // An impulse at an input frame that falls exactly on an output frame.
        // Far enough in that the ringing before it is not cut off by the start.
        let k = (2 * f.taps()).div_ceil(m);
        let at = k * m;
        let mut x = vec![0f32; (at + 4 * f.taps() + m) * 2];
        x[at * 2] = 1.0;
        x[at * 2 + 1] = 1.0;
        let y: Vec<f64> = Resampler::convert(f.clone(), &x).as_chunks::<2>().0.iter().map(|v| f64::from(v[0])).collect();
        let n0 = k * l;
        let peak = (0..y.len()).max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs())).unwrap();
        assert_eq!(peak, n0, "{from} -> {to}: the peak lands on the impulse's own time");
        let tol = 1e-7 * y[n0].abs().max(1e-3);
        for j in 1..n0.min(y.len() - n0) {
            assert!((y[n0 + j] - y[n0 - j]).abs() <= tol, "{from} -> {to}: symmetric (linear phase), frame {j}");
        }
        let floor = y[n0].abs() * 1e-6;
        let pre = (0..n0).find(|&i| y[i].abs() > floor).map_or(0, |i| n0 - i);
        let post = (n0..y.len()).rev().find(|&i| y[i].abs() > floor).map_or(0, |i| i - n0);
        let ms = |n: usize| n as f64 * 1000.0 / f64::from(to);
        eprintln!("{:>5} -> {:<5} kHz  {n0:>7}  {:>11.3}  {:>12.3}", khz(from), khz(to), ms(pre), ms(post));
        assert!(pre.abs_diff(post) <= 1, "{pre} {post}");
        assert!(ms(pre) < 3.0, "the ringing is short: {} ms", ms(pre));
    }
}

#[test]
fn n_frames_in_give_exactly_the_frames_their_duration_holds_aligned_with_the_first() {
    let _serial = serial();
    let mut all = pairs();
    all.extend([(44_100, 384_000), (384_000, 44_100), (44_100, 768_000), (48_000, 768_000), (22_050, 48_000), (32_000, 44_100)]);
    for (from, to) in all {
        let f = filter(from, to);
        let (l, m) = f.ratio();
        for n in [0usize, 1, 2, 3, m - 1, m, m + 1, l, 1000, from as usize, from as usize + 1] {
            let out = Resampler::convert(f.clone(), &vec![0.25f32; n * 2]);
            let want = (n as u128 * l as u128).div_ceil(m as u128) as usize;
            assert_eq!(out.len() / 2, want, "{from} -> {to}: {n} frames");
        }
        // An impulse at the very first frame: the first output frame is its peak (the
        // converter is primed with silence, so frame 0 is computed like any other).
        let mut x = vec![0f32; (2 * f.taps()) * 2];
        x[0] = 1.0;
        x[1] = 1.0;
        let y = Resampler::convert(f.clone(), &x);
        let peak = y.as_chunks::<2>().0.iter().map(|v| v[0].abs()).fold(0f32, f32::max);
        assert_eq!(y[0], peak, "{from} -> {to}: aligned with the first frame");
        // A slow tone keeps its phase: no delay in the output.
        let tone = signal(from, &[(100.0, 0.5)], 0.2);
        let y = Resampler::convert(f, &tone);
        let mut err: f64 = 0.0;
        for n in (to as usize / 50)..(to as usize / 10) {
            let ideal = 0.5 * (2.0 * PI * 100.0 * n as f64 / f64::from(to)).sin();
            err = err.max((f64::from(y[n * 2]) - ideal).abs());
        }
        assert!(err < 1e-6, "{from} -> {to}: in phase with the source ({err})");
    }
}

#[test]
fn the_same_rate_is_a_bit_exact_bypass() {
    let _serial = serial();
    let f = filter(48_000, 48_000);
    assert!(f.is_bypass());
    let mut x: Vec<f32> = (0..50_000).map(|i| ((i as f32 * 0.31).sin() * 1.7) - 0.2).collect();
    x[10] = -0.0;
    x[11] = f32::MIN_POSITIVE / 4.0;
    let y = Resampler::convert(f, &x);
    assert_eq!(y.len(), x.len());
    assert!(x.iter().zip(&y).all(|(a, b)| a.to_bits() == b.to_bits()), "every bit, signed zero and denormals too");
}

#[test]
fn up_and_down_again_returns_the_signal() {
    let _serial = serial();
    eprintln!("round trip                     error, rms and largest (dB re the signal's peak)");
    let mut worst: f64 = -400.0;
    for (a, b) in [(44_100, 48_000), (48_000, 44_100), (44_100, 192_000), (48_000, 96_000), (96_000, 44_100), (44_100, 88_200), (176_400, 48_000)] {
        // Content within both passbands (that of the lower rate).
        let edge = 0.4535 * f64::from(a.min(b)) * 0.98;
        let tones: Vec<(f64, f64)> = [0.003, 0.05, 0.2, 0.45, 0.7, 0.9, 1.0].iter().map(|q| ((edge * q).floor(), 0.1)).collect();
        let x = signal(a, &tones, 1.5);
        let y = Resampler::convert(filter(b, a), &Resampler::convert(filter(a, b), &x));
        assert_eq!(y.len(), x.len(), "the same length back");
        let (s, e) = (a as usize / 4, a as usize * 5 / 4);
        let (mut sq, mut big) = (0f64, 0f64);
        for i in s..e {
            let d = f64::from(y[i * 2]) - f64::from(x[i * 2]);
            sq += d * d;
            big = big.max(d.abs());
        }
        let peak = 0.7;
        let (rms, big) = (db((sq / (e - s) as f64).sqrt() / peak), db(big / peak));
        eprintln!("{:>5} -> {:<5} -> {:<5} kHz  {rms:>8.1} {big:>8.1}", khz(a), khz(b), khz(a));
        worst = worst.max(big);
    }
    assert!(worst < -120.0, "back within -120 dB of the original, sample for sample: {worst}");
}

#[test]
fn intersample_peaks_rise_above_full_scale() {
    let _serial = serial();
    // Full-scale samples (+1 +1 -1 -1 ...) of a tone at a quarter of the rate, sampled 45
    // degrees off its peaks: its true peak is 3 dB over full scale, which a converter's
    // output samples land on. The engine's limiter takes it (crossfade_rates.rs).
    for (from, to) in [(44_100, 48_000), (48_000, 44_100), (44_100, 192_000)] {
        let x: Vec<f32> = (0..from as usize).flat_map(|i| if (i / 2) % 2 == 0 { [1.0, 1.0] } else { [-1.0, -1.0] }).collect();
        let y = Resampler::convert(filter(from, to), &x);
        let peak = y.iter().skip(to as usize / 10).take(to as usize).fold(0f32, |p, s| p.max(s.abs()));
        eprintln!("{} -> {} kHz: full-scale samples come out up to {peak:.4} ({:+.2} dB)", khz(from), khz(to), db(f64::from(peak)));
        assert!(peak > 1.3 && peak < 1.42, "{peak}");
    }
}

#[test]
fn streaming_allocates_nothing_once_running() {
    let _serial = serial();
    let f = filter(44_100, 192_000);
    let x = signal(44_100, &[(1000.0, 0.5)], 3.0);
    let mut r = Resampler::new(f);
    let mut out = Vec::with_capacity(8192 * 2);
    let mut blocks = x.chunks(BLOCK * 2);
    // Warm up: the first blocks.
    for b in blocks.by_ref().take(4) {
        r.push(b);
        out.clear();
        r.produce(&mut out, usize::MAX);
    }
    let before = ALLOCS.with(Cell::get);
    let mut made = 0;
    for b in blocks {
        r.push(b);
        while out.capacity() - out.len() >= 2 {
            let room = (out.capacity() - out.len()) / 2;
            let n = r.produce(&mut out, room);
            made += n;
            if n == 0 {
                break;
            }
            out.clear();
        }
    }
    r.finish();
    out.clear();
    r.produce(&mut out, 4096);
    let after = ALLOCS.with(Cell::get);
    assert!(made > 300_000);
    assert_eq!(after - before, 0, "no allocation while converting");
}

#[test]
fn what_it_costs() {
    let _serial = serial();
    // As the decode thread does it: input in blocks of BLOCK frames, output in chunks of 4096.
    eprintln!("pair              ms per second of audio   worst chunk of 4096 (ms)   table (ms)");
    let mut all = pairs();
    all.extend([(44_100, 384_000), (44_100, 768_000), (768_000, 44_100)]);
    let mut worst_share: f64 = 0.0;
    // The slowest chunk: within a quarter of the time it plays for (on a CI runner, which
    // measured a third once, `common::allowance`).
    let share_limit = 0.25 * common::allowance();
    for (from, to) in all {
        let t0 = Instant::now();
        let f = filter(from, to);
        let table_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let secs = 4.0;
        let x = signal(from, &[(997.0, 0.5), (5000.0, 0.2)], secs);
        let chunk_ms = 4096.0 * 1000.0 / f64::from(to);
        // The best of three: the slowest chunk is one measurement, and a core taken away for
        // a millisecond (a machine shared with other work) shows in it in full.
        let (mut per_sec, mut worst_chunk) = (f64::INFINITY, f64::INFINITY);
        for _ in 0..3 {
            let mut r = Resampler::new(f.clone());
            let mut out = Vec::with_capacity(4096 * 2);
            let mut fed = 0;
            let mut worst: f64 = 0.0;
            let start = Instant::now();
            let mut made = 0u64;
            loop {
                out.clear();
                let t = Instant::now();
                while out.len() < 4096 * 2 {
                    let room = 4096 - out.len() / 2;
                    let n = r.produce(&mut out, room);
                    if n == 0 {
                        if fed < x.len() {
                            let end = (fed + BLOCK * 2).min(x.len());
                            r.push(&x[fed..end]);
                            fed = end;
                        } else if !r.is_finished() {
                            r.finish();
                        } else {
                            break;
                        }
                    }
                }
                worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
                made += (out.len() / 2) as u64;
                if out.len() < 4096 * 2 {
                    break;
                }
            }
            let total = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(made, f.output_frames((x.len() / 2) as u64));
            per_sec = per_sec.min(total / secs);
            worst_chunk = worst_chunk.min(worst);
            if per_sec < 100.0 && worst_chunk / chunk_ms < share_limit {
                break;
            }
        }
        eprintln!("{:>5} -> {:<5} kHz  {per_sec:>22.2}   {worst_chunk:>24.3}   {table_ms:>10.2}", khz(from), khz(to));
        worst_share = worst_share.max(worst_chunk / chunk_ms);
        if !cfg!(debug_assertions) {
            assert!(per_sec < 100.0, "{from} -> {to}: under a tenth of a core ({per_sec} ms a second)");
        }
    }
    eprintln!("worst chunk: {:.1}% of the time it plays for", worst_share * 100.0);
    if !cfg!(debug_assertions) {
        assert!(worst_share < share_limit, "a chunk takes a small part of its own playing time");
    }
}
