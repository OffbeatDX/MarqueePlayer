//! The equalizer through the real engine (decode thread, output thread, a capture device):
//! the frequency response measured at every sample rate against the filters' own curve,
//! off and flat as bit-identical as no equalizer, gapless boundaries untouched by it, a
//! slider move smooth and prompt, seeks and pauses without a thump, every built-in preset
//! unclipped at full scale, and what it costs. The fixtures are 32-bit float WAVs written
//! here, so the input is exactly known.

mod common;

use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Capture, decode, dir, max_step, off, peak, play, write_wav};
use player_core::eq::formats::{self, Format, NoteKind};
use player_core::eq::presets::{self, BUILT_IN};
use player_core::eq::{self, Band, EqParams, Equalizer, Kind, MAX_BANDS, Preamp, Width, fit};
use player_core::limiter::CEILING;
use player_core::{Event, Transition};

fn gapless(_: usize) -> Transition {
    Transition::Gapless
}

fn tones(rate: u32, secs: f64, parts: &[(f64, f64)]) -> Vec<f32> {
    let n = (secs * f64::from(rate)) as usize;
    (0..n)
        .flat_map(|i| {
            let t = i as f64 / f64::from(rate);
            let s: f64 = parts.iter().map(|&(f, a)| a * (2.0 * PI * f * t).sin()).sum();
            let r: f64 = parts.iter().map(|&(f, a)| a * (2.0 * PI * f * t + 0.3).sin()).sum();
            [s as f32, r as f32]
        })
        .collect()
}

/// The equalizer the measurement uses: shelves and peaks, cuts and boosts, narrow and wide.
fn test_bands() -> Vec<Band> {
    vec![
        Band::new(Kind::LowShelf, 40.0, 6.0, 0.7),
        Band::new(Kind::Peak, 200.0, -5.0, 2.0),
        Band::new(Kind::Peak, 2500.0, 4.0, 1.4),
        Band::new(Kind::HighShelf, 9000.0, -3.0, 0.7),
        Band::new(Kind::Peak, 15_000.0, 3.0, 3.0),
    ]
}

/// Amplitude of `freq` in channel 0 over `frames` (an exact number of its periods).
fn amplitude(v: &[f32], rate: u32, freq: f64, from: usize, frames: usize) -> f64 {
    let periods = (frames as f64 * freq / f64::from(rate)).floor().max(1.0);
    let n = (periods * f64::from(rate) / freq).round() as usize;
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..n {
        let w = 2.0 * PI * freq * (from + i) as f64 / f64::from(rate);
        let x = f64::from(v[(from + i) * 2]);
        re += x * w.cos();
        im += x * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n as f64
}

/// The amplitude of `freq` in the left channel over `frames` frames from `from`, through a Hann
/// window: another tone in the signal leaks into a plain sum by a small amount whose sign
/// depends on where the window starts (a 63 Hz tone under a 6 kHz one 20 dB down: up to
/// +-0.05 dB), and the window brings that down to nothing.
fn amplitude_hann(v: &[f32], rate: u32, freq: f64, from: usize, frames: usize) -> f64 {
    let (mut re, mut im, mut sum) = (0.0, 0.0, 0.0);
    for i in 0..frames {
        let h = 0.5 - 0.5 * (2.0 * PI * i as f64 / frames as f64).cos();
        let w = 2.0 * PI * freq * (from + i) as f64 / f64::from(rate);
        let x = f64::from(v[(from + i) * 2]) * h;
        re += x * w.cos();
        im += x * w.sin();
        sum += h;
    }
    2.0 * (re * re + im * im).sqrt() / sum
}

#[test]
fn off_or_flat_the_output_is_bit_identical() {
    let d = dir("eq-fixtures");
    let f = d.join("tones-44k.wav");
    let input = tones(44_100, 3.0, &[(440.0, 0.4), (5000.0, 0.3)]);
    write_wav(&f, 44_100, &input);
    let files = vec![f];
    // Off, with boosts set: nothing of it applies.
    let boosts = EqParams::new(false, &presets::BUILT_IN[6].bands(), Preamp::Auto);
    let (got, _) = play(Capture::default(), &files, off(), |p| p.set_eq(&boosts), gapless, |_, _, _| {});
    assert!(got == input, "off: untouched");
    // On, every band at 0 dB: untouched too (the bands pass samples through exactly).
    let flat = EqParams::new(true, &presets::BUILT_IN[0].bands(), Preamp::Auto);
    let (got, _) = play(Capture::default(), &files, off(), |p| p.set_eq(&flat), gapless, |_, _, _| {});
    assert!(got == input, "on and flat: untouched");
}

/// A float file holding a NaN and infinities: decoded as silence there, every other sample
/// bit-identical, and with the equalizer on the track plays on (it is not NaN, or silent,
/// until the next seek).
#[test]
fn a_float_file_with_non_finite_samples_plays_on_through_the_equalizer() {
    let d = dir("eq-fixtures");
    let f = d.join("non-finite.wav");
    let mut input = tones(48_000, 1.0, &[(440.0, 0.4)]);
    for (i, bad) in [(2000, f32::NAN), (2001, f32::INFINITY), (30_001, f32::NEG_INFINITY)] {
        input[i] = bad;
    }
    write_wav(&f, 48_000, &input);
    let want: Vec<f32> = input.iter().map(|&x| if x.is_finite() { x } else { 0.0 }).collect();
    assert!(decode(&f).iter().map(|x| x.to_bits()).eq(want.iter().map(|x| x.to_bits())), "decoded clean, the rest untouched");
    let boost = EqParams::new(true, &[Band::new(Kind::Peak, 440.0, 3.0, 1.0)], Preamp::Manual(-3.0));
    let (got, _) = play(Capture::default(), &[f], off(), |p| p.set_eq(&boost), gapless, |_, _, _| {});
    assert!(got.iter().all(|x| x.is_finite()), "nothing non-finite reaches the device");
    let tail = &got[got.len() - 9600..];
    assert!(peak(tail) > 0.3, "still playing at the end: {}", peak(tail));
}

#[test]
fn the_measured_response_is_the_filters_curve_at_every_rate() {
    let d = dir("eq-fixtures");
    let bands = test_bands();
    let params = EqParams::new(true, &bands, Preamp::Auto);
    for rate in [44_100u32, 48_000, 88_200, 96_000, 192_000, 384_000] {
        // Stepped tones, 20 Hz to 20 kHz, 0.5 s each: the filters settle, then each is
        // measured over its last quarter second.
        let freqs: Vec<f64> = (0..21).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 20.0)).filter(|&f| f < 0.45 * f64::from(rate)).collect();
        let step = rate as usize / 2;
        let mut input = Vec::new();
        for &f in &freqs {
            input.extend(tones(rate, 0.5, &[(f, 0.25)]));
        }
        let path = d.join(format!("steps-{rate}.wav"));
        write_wav(&path, rate, &input);
        let (got, _) = play(Capture::default(), &[path], off(), |p| p.set_eq(&params), gapless, |_, _, _| {});
        assert_eq!(got.len(), input.len());
        // The engine's output is exactly the equalizer run over the input.
        let mut offline = input.clone();
        Equalizer::settled(&params, rate).process(&mut offline);
        assert!(got == offline, "{rate}: the engine equalizes exactly as the filters do");
        let pre = params.preamp_db(rate);
        let mut worst = 0f64;
        for (k, &f) in freqs.iter().enumerate() {
            let from = k * step + step / 2;
            let measured = 20.0 * (amplitude(&got, rate, f, from, step / 2) / amplitude(&input, rate, f, from, step / 2)).log10();
            let want = eq::response_db(&bands, rate, f) + pre;
            worst = worst.max((measured - want).abs());
            assert!((measured - want).abs() < 0.02, "{rate} Hz rate, {f:.0} Hz: measured {measured:.4} dB, the filters say {want:.4} dB");
        }
        eprintln!("{rate} Hz: {} tones, worst deviation from the filters' curve {worst:.5} dB (preamp {pre:.2} dB)", freqs.len());
    }
}

#[test]
fn a_gapless_boundary_is_untouched_by_the_equalizer() {
    let d = dir("eq-fixtures");
    let rate = 48_000;
    let whole = tones(rate, 6.0, &[(55.0, 0.3), (440.0, 0.2), (7000.0, 0.1)]);
    let cut = 123_457;
    let (a, b) = (d.join("cont-a.wav"), d.join("cont-b.wav"));
    write_wav(&a, rate, &whole[..cut * 2]);
    write_wav(&b, rate, &whole[cut * 2..]);
    let params = EqParams::new(true, &presets::BUILT_IN[1].bands(), Preamp::Auto);
    let (got, opens) = play(Capture::default(), &[a, b], off(), |p| p.set_eq(&params), gapless, |_, _, _| {});
    let mut want = whole.clone();
    Equalizer::settled(&params, rate).process(&mut want);
    assert_eq!(opens, vec![rate]);
    assert!(got == want, "the two pieces equalize as one signal: the filters carry on across the boundary");
}

#[test]
fn a_slider_move_is_smooth_and_takes_effect_at_once() {
    let d = dir("eq-fixtures");
    let rate = 48_000;
    let input = tones(rate, 8.0, &[(63.0, 0.2), (1000.0, 0.1)]);
    let f = d.join("slider.wav");
    write_wav(&f, rate, &input);
    let mut gains = [0.0f32; 10];
    let flat = EqParams::new(true, &presets::graphic(&gains), Preamp::Manual(0.0));
    let marks = std::sync::Arc::new(std::sync::Mutex::new((0usize, 0usize)));
    let m = marks.clone();
    let (got, _) = play(
        Capture::paced(),
        &[f],
        off(),
        |p| p.set_eq(&flat),
        gapless,
        move |p, cap, _| {
            // A second in: the 63 Hz slider jumps to +12 dB.
            while cap.frames() < rate as usize {
                std::thread::sleep(Duration::from_millis(1));
            }
            gains[1] = 12.0;
            m.lock().unwrap().0 = cap.frames();
            p.set_eq(&EqParams::new(true, &presets::graphic(&gains), Preamp::Manual(0.0)));
            // Three seconds in: dragged back down to 0 in 0.5 dB steps every 10 ms.
            while cap.frames() < 3 * rate as usize {
                std::thread::sleep(Duration::from_millis(1));
            }
            m.lock().unwrap().1 = cap.frames();
            for k in (0..24).rev() {
                gains[1] = k as f32 * 0.5;
                p.set_eq(&EqParams::new(true, &presets::graphic(&gains), Preamp::Manual(0.0)));
                std::thread::sleep(Duration::from_millis(1));
            }
        },
    );
    let (jump_at, drag_at) = *marks.lock().unwrap();
    let r = rate as usize;
    // Promptly: within 150 ms of audio after the call, the 63 Hz tone is within 0.5 dB of
    // its new level (+12 dB of band gain where the slider is).
    let want = 10f64.powf(eq::response_db(&presets::graphic(&{
        let mut g = [0.0; 10];
        g[1] = 12.0;
        g
    }), rate, 63.0) / 20.0);
    let window = r / 63 * 3;
    let at = |frame: usize| amplitude(&got, rate, 63.0, frame, window) / amplitude(&input, rate, 63.0, frame, window);
    let reached = (jump_at..jump_at + r).step_by(240).find(|&i| (20.0 * (at(i) / want).log10()).abs() < 0.5).expect("reaches the new level");
    let latency = (reached - jump_at) as f64 / f64::from(rate);
    eprintln!("slider: within 0.5 dB {:.0} ms of audio after the change", latency * 1000.0);
    assert!(latency < 0.15, "{latency} s");
    // Smoothly: no sample-to-sample step during the changes larger than in the steady
    // signal at the higher of the two levels.
    let steady_hi = max_step(&got, jump_at + r / 2..jump_at + r);
    let during_jump = max_step(&got, jump_at.saturating_sub(r / 10)..jump_at + r / 2);
    let during_drag = max_step(&got, drag_at.saturating_sub(r / 10)..drag_at + r);
    eprintln!("slider: largest step {during_jump:.5} during the jump, {during_drag:.5} during the drag, {steady_hi:.5} steady at +12 dB");
    assert!(during_jump <= steady_hi * 1.02, "{during_jump} vs {steady_hi}");
    assert!(during_drag <= steady_hi * 1.02, "{during_drag} vs {steady_hi}");
}

#[test]
fn seeks_and_pauses_do_not_thump_with_a_bass_boost() {
    let d = dir("eq-fixtures");
    let rate = 48_000;
    let input = tones(rate, 6.0, &[(40.0, 0.25), (2000.0, 0.1)]);
    let f = d.join("thump.wav");
    write_wav(&f, rate, &input);
    let loud = EqParams::new(true, &presets::BUILT_IN[6].bands(), Preamp::Auto);
    let mut steady = input.clone();
    Equalizer::settled(&loud, rate).process(&mut steady);
    let steady_step = max_step(&steady, rate as usize..steady.len() / 2);
    let steady_peak = peak(&steady[rate as usize * 2..]);
    for case in ["seek", "pause"] {
        let (got, _) = play(
            Capture::fading(),
            std::slice::from_ref(&f),
            off(),
            |p| p.set_eq(&loud),
            gapless,
            |p, cap, _| {
                while cap.frames() < rate as usize * 3 / 2 {
                    std::thread::sleep(Duration::from_millis(1));
                }
                cap.queued.store(480, std::sync::atomic::Ordering::Relaxed);
                if case == "seek" {
                    p.seek(4.0);
                } else {
                    p.set_paused(true);
                    std::thread::sleep(Duration::from_millis(50));
                    p.set_paused(false);
                }
            },
        );
        let step = max_step(&got, 0..got.len() / 2);
        let pk = peak(&got[rate as usize * 2..]);
        eprintln!("{case}: largest step {step:.5} (steady {steady_step:.5}), peak {pk:.4} (steady {steady_peak:.4})");
        assert!(step <= steady_step * 1.05, "{case}: a step of {step} against {steady_step} in the steady signal");
        // A pause keeps the filters going: nothing above the steady level. A seek starts
        // them afresh on audio that starts mid-waveform, so a boosted bass note settles in
        // over a few tens of milliseconds (here +0.5 dB at most), no step and no thump.
        let allowed = if case == "seek" { 1.12 } else { 1.01 };
        assert!(pk <= steady_peak * allowed, "{case}: a bump above the steady level ({pk} vs {steady_peak})");
    }
}

#[test]
fn no_built_in_preset_clips_a_full_scale_signal() {
    let d = dir("eq-fixtures");
    let rate = 48_000u32;
    // A logarithmic sweep from 20 Hz to 20 kHz at full scale, then white noise at full scale.
    let secs = 6.0;
    let n = (secs * f64::from(rate)) as usize;
    let (f0, f1) = (20.0f64, 20_000.0f64);
    let k = (f1 / f0).ln();
    let mut input: Vec<f32> = (0..n)
        .flat_map(|i| {
            let t = i as f64 / f64::from(rate);
            let phase = 2.0 * PI * f0 * secs / k * ((t / secs * k).exp() - 1.0);
            let s = (0.999 * phase.sin()) as f32;
            [s, s]
        })
        .collect();
    let mut seed = 0x1234_5678u32;
    for _ in 0..rate * 2 {
        let mut r = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        input.push(r());
        input.push(r());
    }
    let path = d.join("full-scale.wav");
    write_wav(&path, rate, &input);
    for preset in &BUILT_IN {
        let params = EqParams::new(true, &preset.bands(), Preamp::Auto);
        let mut offline = input.clone();
        Equalizer::settled(&params, rate).process(&mut offline);
        let sweep_peak = peak(&offline[..n * 2]);
        let noise_peak = peak(&offline[n * 2..]);
        let (got, _) = play(Capture::default(), std::slice::from_ref(&path), off(), |p| p.set_eq(&params), gapless, |_, _, _| {});
        let out = peak(&got);
        eprintln!(
            "{:>14}: auto preamp {:+.2} dB; equalized peak {:.3} (sweep) {:.3} (noise); after the limiter {out:.4}",
            preset.name,
            params.preamp_db(rate),
            sweep_peak,
            noise_peak
        );
        assert!(out <= CEILING, "{}: {out}", preset.name);
        // The auto preamp alone keeps a full-scale tone at full scale, within the 1-2% a
        // sweep's transients add through a boosted resonance, anywhere in the band; the
        // limiter takes those and the noise's rarer summed peaks (a cut's phase shift can
        // raise a noise peak by 10% with no boost at all).
        assert!(sweep_peak <= 1.02, "{}: the sweep peaks at {sweep_peak}", preset.name);
    }
}

#[test]
fn what_it_costs() {
    // 32 bands at 192 kHz: within 3% of one core, whatever the kinds (on a CI runner's slower
    // core, `common::allowance`).
    let budget = 30.0 * common::allowance();
    for rate in [44_100u32, 96_000, 192_000] {
        for (what, bands) in [
            ("10-band graphic", presets::graphic(&[3.0, -2.0, 1.0, 4.0, -3.0, 2.0, -1.0, 3.0, -2.0, 1.0])),
            ("16-band parametric", (0..16).map(|i| Band::new(Kind::Peak, 30.0 * 1.5f32.powi(i), if i % 2 == 0 { 3.0 } else { -2.0 }, 2.0)).collect()),
            ("32-band parametric", (0..32).map(|i| Band::new(Kind::Peak, 25.0 * 1.23f32.powi(i), if i % 2 == 0 { 3.0 } else { -2.0 }, 2.0)).collect()),
            ("32 bands of every kind", (0..32).map(|i| Band::new(Kind::ALL[i as usize % 8], 25.0 * 1.23f32.powi(i), if i % 2 == 0 { 3.0 } else { -2.0 }, 0.9)).collect()),
        ] {
            let params = EqParams::new(true, &bands, Preamp::Auto);
            let secs = 10.0;
            // The best of three: the other tests run beside this one, and a machine shared
            // with other work (a CI runner) takes a core away now and then.
            let (mut per_sec, mut buf) = (f64::INFINITY, Vec::new());
            for _ in 0..3 {
                let mut e = Equalizer::settled(&params, rate);
                buf = tones(rate, secs, &[(440.0, 0.3)]);
                let start = Instant::now();
                for block in buf.chunks_mut(960) {
                    e.process(block);
                }
                per_sec = per_sec.min(start.elapsed().as_secs_f64() * 1000.0 / secs);
                if per_sec < budget {
                    break;
                }
            }
            eprintln!("{what} at {rate} Hz: {per_sec:.2} ms of CPU per second of audio ({:.2}% of one core)", per_sec / 10.0);
            if !cfg!(debug_assertions) {
                assert!(per_sec < budget, "{per_sec} ms per second");
            }
            assert!(buf.iter().all(|x| x.is_finite()));
        }
    }
    // A new setting on the output thread: the auto preamp's search (the costliest part of
    // taking one), 32 bands that boost, at 192 kHz.
    let bands: Vec<Band> = (0..32).map(|i| Band::new(Kind::ALL[i as usize % 8], 25.0 * 1.23f32.powi(i), 3.0, 2.0)).collect();
    let params = EqParams::new(true, &bands, Preamp::Auto);
    let mut e = Equalizer::new(192_000);
    let start = Instant::now();
    for _ in 0..100 {
        e.set_params(&params);
    }
    let per = start.elapsed().as_secs_f64() * 1000.0 / 100.0;
    eprintln!("a new 32-band setting taken at 192 kHz: {per:.3} ms (once per change)");
    if !cfg!(debug_assertions) {
        assert!(per < 5.0, "{per} ms");
    }
}

/// Changing the setting while paused touches nothing: the output thread stays asleep.
#[test]
fn a_setting_while_paused_wakes_nothing() {
    let d = dir("eq-fixtures");
    let f = d.join("pause.wav");
    write_wav(&f, 48_000, &tones(48_000, 3.0, &[(440.0, 0.3)]));
    let cap = Capture::paced();
    let dev = cap.clone();
    let (player, events) = player_core::Player::with_device(move || Box::new(dev));
    player.set_gain_config(off());
    player.play(player_core::TrackRequest::new(0, PathBuf::from(&f)));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(events.recv_timeout(Duration::from_millis(20)), Ok(Event::TrackStarted { .. })) {
        assert!(Instant::now() < deadline);
    }
    player.set_paused(true);
    std::thread::sleep(Duration::from_millis(100));
    let before = cap.frames();
    for g in 0..20 {
        player.set_eq(&EqParams::new(true, &presets::graphic(&[g as f32 * 0.5; 10]), Preamp::Auto));
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(cap.frames(), before, "nothing written while paused");
    drop(player);
    let _ = decode(&f);
}

/// The AutoEq files these tests read: AutoEq's results for the FiiO FT1 (MIT licence,
/// `tests/fixtures/autoeq/LICENSE-AutoEq.txt`).
fn autoeq(name: &str) -> (Vec<u8>, String) {
    let file = format!("FiiO FT1 {name}.txt");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("autoeq").join(&file);
    (std::fs::read(&path).unwrap(), file)
}

/// Gain (dB) and phase (degrees) of `freq` in channel 0, output against input, over
/// `frames` from `from` (an exact number of its periods).
fn transfer(out: &[f32], input: &[f32], rate: u32, freq: f64, from: usize, frames: usize) -> (f64, f64) {
    let periods = (frames as f64 * freq / f64::from(rate)).floor().max(1.0);
    let n = (periods * f64::from(rate) / freq).round() as usize;
    let dft = |v: &[f32]| {
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..n {
            let w = 2.0 * PI * freq * (from + i) as f64 / f64::from(rate);
            let x = f64::from(v[(from + i) * 2]);
            re += x * w.cos();
            im -= x * w.sin();
        }
        (re, im)
    };
    let (or, oi) = dft(out);
    let (ir, ii) = dft(input);
    let gain = 10.0 * ((or * or + oi * oi) / (ir * ir + ii * ii)).log10();
    let mut phase = (oi.atan2(or) - ii.atan2(ir)).to_degrees();
    while phase > 180.0 {
        phase -= 360.0;
    }
    while phase <= -180.0 {
        phase += 360.0;
    }
    (gain, phase)
}

/// Each new kind and width, through the engine at five rates, against its analytic curve
/// (`response_db`, the cookbook's prototype at the warped frequency), as the peaks and
/// shelves are above. Also bit-exactly the offline equalizer.
#[test]
fn every_filter_kind_measures_as_its_curve_at_every_rate() {
    let d = dir("eq-fixtures");
    let sets: Vec<(&str, Vec<Band>)> = vec![
        (
            "mixed",
            vec![
                Band::new(Kind::HighPass, 40.0, 0.0, 0.6),
                Band::with_width(Kind::LowShelf, 120.0, 5.0, 0.9, Width::Slope),
                Band::new(Kind::Notch, 250.0, 0.0, 4.0),
                Band::new(Kind::AllPass, 900.0, 0.0, 0.7),
                Band::with_width(Kind::Peak, 3000.0, -4.0, 0.5, Width::Octaves),
                Band::with_width(Kind::HighShelf, 7000.0, -3.0, 0.5, Width::Slope),
                Band::new(Kind::LowPass, 15_000.0, 0.0, 1.2),
            ],
        ),
        ("band-pass", vec![Band::with_width(Kind::BandPass, 1000.0, 0.0, 2.0, Width::Octaves)]),
        ("low-pass", vec![Band::with_width(Kind::LowPass, 2000.0, 0.0, 1.0, Width::Octaves), Band::new(Kind::HighPass, 150.0, 0.0, 2.5)]),
    ];
    for rate in [44_100u32, 48_000, 96_000, 192_000, 384_000] {
        let freqs: Vec<f64> = (0..21).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 20.0)).filter(|&f| f < 0.45 * f64::from(rate)).collect();
        let step = rate as usize / 2;
        let mut input = Vec::new();
        for &f in &freqs {
            input.extend(tones(rate, 0.5, &[(f, 0.25)]));
        }
        let path = d.join(format!("kinds-{rate}.wav"));
        write_wav(&path, rate, &input);
        for (what, bands) in &sets {
            let params = EqParams::new(true, bands, Preamp::Auto);
            let (got, _) = play(Capture::default(), std::slice::from_ref(&path), off(), |p| p.set_eq(&params), gapless, |_, _, _| {});
            let mut offline = input.clone();
            Equalizer::settled(&params, rate).process(&mut offline);
            assert!(got == offline, "{rate} {what}: the engine equalizes exactly as the filters do");
            let pre = params.preamp_db(rate);
            let mut worst = 0f64;
            let mut measured_n = 0;
            for (k, &f) in freqs.iter().enumerate() {
                let want = eq::response_db(bands, rate, f) + pre;
                if want < -60.0 {
                    continue;
                }
                let from = k * step + step / 2;
                let (gain, _) = transfer(&got, &input, rate, f, from, step / 2);
                worst = worst.max((gain - want).abs());
                measured_n += 1;
                assert!((gain - want).abs() < 0.02, "{rate} Hz rate, {what}, {f:.0} Hz: measured {gain:.4} dB, the curve says {want:.4} dB");
            }
            eprintln!("{rate} Hz, {what}: {measured_n} tones, worst deviation {worst:.5} dB (preamp {pre:.2} dB)");
        }
    }
}

/// An all-pass leaves every level as it is and turns the phase as its prototype does,
/// `-2 atan((Omega / Q) / (1 - Omega^2))`: -180 degrees at its centre.
#[test]
fn an_all_pass_turns_the_phase_and_leaves_the_level() {
    let rate = 48_000u32;
    let (f0, q) = (1000.0, 0.7);
    let params = EqParams::new(true, &[Band::new(Kind::AllPass, f0 as f32, 0.0, q as f32)], Preamp::Auto);
    assert_eq!(params.preamp_db(rate), 0.0, "an all-pass needs no headroom");
    for f in [100.0, 500.0, 1000.0, 2000.0, 8000.0] {
        let input = tones(rate, 1.0, &[(f, 0.5)]);
        let mut out = input.clone();
        Equalizer::settled(&params, rate).process(&mut out);
        let (gain, phase) = transfer(&out, &input, rate, f, rate as usize / 2, rate as usize / 2);
        let om = (PI * f / f64::from(rate)).tan() / (PI * f0 / f64::from(rate)).tan();
        let want = -2.0 * (om / q).atan2(1.0 - om * om).to_degrees();
        eprintln!("all-pass at {f} Hz: {gain:+.5} dB, {phase:.3} degrees (prototype {want:.3})");
        assert!(gain.abs() < 0.001, "{f}: {gain}");
        let diff = (phase - want + 540.0).rem_euclid(360.0) - 180.0;
        assert!(diff.abs() < 0.05, "{f}: phase {phase} vs {want}");
    }
}

/// A low-pass added while playing and then changed to a notch: it glides in by its mix, so
/// no sample steps further than the steady signal's.
#[test]
fn a_filter_without_gain_comes_and_goes_smoothly() {
    let d = dir("eq-fixtures");
    let rate = 48_000;
    let input = tones(rate, 6.0, &[(63.0, 0.2), (1000.0, 0.15), (6000.0, 0.1)]);
    let f = d.join("gainless.wav");
    write_wav(&f, rate, &input);
    let marks = std::sync::Arc::new(std::sync::Mutex::new((0usize, 0usize)));
    let m = marks.clone();
    let flat = EqParams::new(true, &[], Preamp::Manual(0.0));
    let (got, _) = play(
        Capture::paced(),
        &[f],
        off(),
        |p| p.set_eq(&flat),
        gapless,
        move |p, cap, _| {
            while cap.frames() < rate as usize {
                std::thread::sleep(Duration::from_millis(1));
            }
            m.lock().unwrap().0 = cap.frames();
            p.set_eq(&EqParams::new(true, &[Band::new(Kind::LowPass, 2000.0, 0.0, 0.707)], Preamp::Manual(0.0)));
            while cap.frames() < 3 * rate as usize {
                std::thread::sleep(Duration::from_millis(1));
            }
            m.lock().unwrap().1 = cap.frames();
            p.set_eq(&EqParams::new(true, &[Band::new(Kind::Notch, 1000.0, 0.0, 2.0)], Preamp::Manual(0.0)));
        },
    );
    let (lp_at, notch_at) = *marks.lock().unwrap();
    let r = rate as usize;
    let steady_before = max_step(&got, r / 2..lp_at.saturating_sub(r / 10));
    let during_lp = max_step(&got, lp_at.saturating_sub(r / 10)..lp_at + r / 2);
    let during_notch = max_step(&got, notch_at.saturating_sub(r / 10)..notch_at + r / 2);
    eprintln!("gainless: largest step {during_lp:.5} as the low-pass comes in, {during_notch:.5} as it becomes a notch, {steady_before:.5} steady before");
    assert!(during_lp <= steady_before * 1.02, "{during_lp} vs {steady_before}");
    assert!(during_notch <= steady_before * 1.02, "{during_notch} vs {steady_before}");
    // And it acted: the 6 kHz tone is down by the low-pass's curve a second later.
    let window = r / 4;
    let at = lp_at + r;
    let measured = 20.0 * (amplitude_hann(&got, rate, 6000.0, at, window) / amplitude_hann(&input, rate, 6000.0, at, window)).log10();
    let want = eq::response_db(&[Band::new(Kind::LowPass, 2000.0, 0.0, 0.707)], rate, 6000.0);
    eprintln!("gainless: 6 kHz down {measured:.4} dB, the curve says {want:.4}");
    assert!((measured - want).abs() < 0.05, "{measured} vs {want}");
}

/// A resonant low-pass and high-pass get the auto preamp's headroom (their peak counts as
/// a boost); a Butterworth one, a notch and an all-pass get none, and the limiter takes the
/// rest: nothing out of the engine above its ceiling.
#[test]
fn resonant_filters_and_all_passes_never_clip() {
    let d = dir("eq-fixtures");
    let rate = 48_000u32;
    let secs = 4.0;
    let n = (secs * f64::from(rate)) as usize;
    let (f0, f1) = (20.0f64, 20_000.0f64);
    let k = (f1 / f0).ln();
    let input: Vec<f32> = (0..n)
        .flat_map(|i| {
            let t = i as f64 / f64::from(rate);
            let s = (0.999 * (2.0 * PI * f0 * secs / k * ((t / secs * k).exp() - 1.0)).sin()) as f32;
            // A square wave on the right: its peaks rise through an all-pass.
            let sq = if (2.0 * PI * 110.0 * t).sin() >= 0.0 { 0.999 } else { -0.999 };
            [s, sq]
        })
        .collect();
    let path = d.join("resonant.wav");
    write_wav(&path, rate, &input);
    for (what, bands, boosts) in [
        ("low-pass Q 8 at 5 kHz", vec![Band::new(Kind::LowPass, 5000.0, 0.0, 8.0)], true),
        ("high-pass Q 10 at 100 Hz", vec![Band::new(Kind::HighPass, 100.0, 0.0, 10.0)], true),
        ("Butterworth low-pass", vec![Band::new(Kind::LowPass, 5000.0, 0.0, std::f32::consts::FRAC_1_SQRT_2)], false),
        ("notch and all-pass", vec![Band::new(Kind::Notch, 60.0, 0.0, 30.0), Band::new(Kind::AllPass, 300.0, 0.0, 2.0)], false),
    ] {
        let params = EqParams::new(true, &bands, Preamp::Auto);
        let pre = params.preamp_db(rate);
        assert_eq!(pre < -1.0, boosts, "{what}: auto preamp {pre}");
        let mut offline = input.clone();
        Equalizer::settled(&params, rate).process(&mut offline);
        let (got, _) = play(Capture::default(), std::slice::from_ref(&path), off(), |p| p.set_eq(&params), gapless, |_, _, _| {});
        let out = peak(&got);
        eprintln!("{what}: auto preamp {pre:+.2} dB; equalized peak {:.4}; after the limiter {out:.4}", peak(&offline));
        assert!(out <= CEILING, "{what}: {out}");
    }
}

/// One pair of headphones (FiiO FT1), as AutoEq publishes them: the ParametricEQ and FixedBandEQ
/// files play exactly as their filters say; the GraphicEQ curve is fitted within its bound.
#[test]
fn autoeq_s_files_play_as_written() {
    let (bytes, file) = autoeq("ParametricEQ");
    let p = formats::read_bytes(&bytes, Some(&file)).unwrap();
    assert_eq!(p.format, Format::AutoEqParametric);
    assert_eq!((p.name.as_deref(), p.bands.len(), p.preamp), (Some("FiiO FT1"), 10, Some(Preamp::Manual(-4.3))));
    assert_eq!(p.bands[0], Band::new(Kind::LowShelf, 105.0, -2.7, 0.7));
    assert_eq!(p.bands[3], Band::new(Kind::Peak, 3835.0, 4.6, 3.54));
    assert!(p.notes.is_empty(), "{:?}", p.notes);
    // Copied as Equalizer APO text and read back: the same filters.
    let text = eq::apo::write("FiiO FT1", &p.bands, p.preamp.unwrap(), 0.0, true);
    assert_eq!(formats::read(&text, None).unwrap().bands, p.bands);
    let (bytes, file) = autoeq("FixedBandEQ");
    let f = formats::read_bytes(&bytes, Some(&file)).unwrap();
    assert_eq!((f.format, f.bands.len()), (Format::AutoEqFixedBand, 10));
    assert!(presets::as_graphic(&f.bands).is_none(), "31 and 62 Hz are not the sliders' 31.5 and 63");
    let (bytes, file) = autoeq("GraphicEQ");
    let start = Instant::now();
    let g = formats::read_bytes(&bytes, Some(&file)).unwrap();
    let took = start.elapsed();
    let fit = g.fit.as_ref().unwrap();
    eprintln!(
        "FT1 GraphicEQ: {} points as {} bands, offset {:+.2} dB: max {:.3} dB near {:.0} Hz, RMS {:.3} dB (44.1, 48 and 96 kHz), fitted in {took:?}",
        g.graphic[0].len(),
        fit.bands.len(),
        fit.offset_db,
        fit.max_err_db,
        fit.worst_hz,
        fit.rms_err_db
    );
    assert_eq!(g.format, Format::GraphicEq);
    assert!(fit.bands.len() <= MAX_BANDS);
    for b in &fit.bands {
        eprintln!("  {:?} {:.1} Hz {:+.2} dB Q {:.2}", b.kind, b.freq, b.gain_db, b.q);
    }
    // No band idles, none fights another with a large opposite gain.
    assert!(fit.bands.iter().all(|b| (0.15..=12.0).contains(&b.gain_db.abs())), "{fit:?}");
    assert!(fit.max_err_db <= fit::GOOD_DB, "{fit:?}");
    assert!(fit.rms_err_db <= 0.25);
    assert_eq!(g.notes[0].kind, NoteKind::Approximated);
    // Through the equalizer at 48 kHz: within the fit's bound of the curve Equalizer APO
    // plays.
    let rate = 48_000u32;
    let params = EqParams::new(true, &g.bands, g.preamp.unwrap());
    for f in [30.0, 100.0, 440.0, 1000.0, 3842.0, 5337.0, 10_000.0, 16_000.0] {
        let input = tones(rate, 1.0, &[(f, 0.1)]);
        let mut out = input.clone();
        Equalizer::settled(&params, rate).process(&mut out);
        let (gain, _) = transfer(&out, &input, rate, f, rate as usize / 2, rate as usize / 2);
        let want = g.target_db(f).unwrap();
        assert!((gain - want).abs() <= fit.max_err_db + 0.01, "{f} Hz: {gain:.3} vs the curve's {want:.3}");
    }
}
