//! Crossfades between tracks at different sample rates, through the real engine into a
//! capture device, against ffmpeg-made tones (a frequency per track). The device keeps the
//! rate playing and the incoming track is converted to it (`engine.rs`); the expected stream
//! is worked out sample by sample from the files' own decodes, the converter run over the
//! incoming track on its own (`resample::Resampler::convert`: its output does not depend on
//! how the input arrives) and the curve, and must match exactly. Then: both tones in the
//! overlap at their true pitch, no step at either edge of it or at a reopen, the current track
//! switching in the middle, the fifth-of-a-track rule, an album gapless through the converter,
//! a gapless track at a third rate, a seek, a pause in and after the overlap (after it the
//! track plays on at its own rate, bit for bit), a skip, a manual skip's crossfade both ways,
//! repeat one, a next track that fails, the limiter on intersample overs, and the device's
//! formats in exclusive mode and PipeWire's bit-perfect mode (fake devices). Needs ffmpeg on
//! PATH (skips without).

mod common;

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Capture, decode, dir, ffmpeg, have_ffmpeg, max_step, off, play};
use player_core::crossfade::{gains, overlap_frames};
use player_core::output::Device;
use player_core::replaygain::linear_gain;
use player_core::resample::{Filter, Resampler};
use player_core::source::Source;
use player_core::{Event, GainConfig, GainMode, OutputConfig, OutputInfo, Player, TrackRequest, Transition};

struct Fx {
    /// 440 Hz, 12 s at 48 kHz; the same at 44.1 and at 192 kHz.
    a48: PathBuf,
    a44: PathBuf,
    a192: PathBuf,
    /// 660 Hz, 12 s at 44.1 kHz, tagged -4 dB of track gain; untagged at 48 and 192 kHz.
    b44: PathBuf,
    b48: PathBuf,
    b192: PathBuf,
    /// 880 Hz, 12 s at 48 kHz; 550 Hz, 6 s at 96 kHz.
    c48: PathBuf,
    d96: PathBuf,
    /// 330 Hz at 44.1 kHz, 2 s and 3 s.
    short2: PathBuf,
    short3: PathBuf,
    /// One continuous 44.1 kHz signal in two pieces of 6 s and a little (an album).
    piece0: PathBuf,
    piece1: PathBuf,
}

fn fixtures() -> Option<Fx> {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = dir("crossfade-rates-fixtures");
    let tone = |name: &str, f: u32, secs: f64, rate: u32, meta: &[&str]| {
        let p = d.join(name);
        if !p.exists() {
            let src = format!("aevalsrc=0.5*sin(2*PI*{f}*t)|0.4*sin(2*PI*{f}*t+1):s={rate}:d={secs}");
            let mut args = vec!["-f", "lavfi", "-i", &src];
            args.extend_from_slice(meta);
            args.extend_from_slice(&["-c:a", "flac", "-sample_fmt", "s16"]);
            let out = p.to_string_lossy().into_owned();
            args.push(&out);
            ffmpeg(&args);
        }
        p
    };
    let rg = ["-metadata", "REPLAYGAIN_TRACK_GAIN=-4.00 dB"];
    let fx = Fx {
        a48: tone("a48.flac", 440, 12.0, 48_000, &[]),
        a44: tone("a44.flac", 440, 12.0, 44_100, &[]),
        a192: tone("a192.flac", 440, 12.0, 192_000, &[]),
        b44: tone("b44.flac", 660, 12.0, 44_100, &rg),
        b48: tone("b48.flac", 660, 12.0, 48_000, &[]),
        b192: tone("b192.flac", 660, 12.0, 192_000, &[]),
        c48: tone("c48.flac", 880, 12.0, 48_000, &[]),
        d96: tone("d96.flac", 550, 6.0, 96_000, &[]),
        short2: tone("short2.flac", 330, 2.0, 44_100, &[]),
        short3: tone("short3.flac", 330, 3.0, 44_100, &[]),
        piece0: d.join("piece0.flac"),
        piece1: d.join("piece1.flac"),
    };
    let whole = d.join("whole.wav");
    if !whole.exists() {
        ffmpeg(&["-f", "lavfi", "-i", "aevalsrc=0.3*sin(2*PI*220*t)+0.2*sin(2*PI*3000*t)|0.3*sin(2*PI*330*t):s=44100:d=13", "-c:a", "pcm_s16le", whole.to_str().unwrap()]);
    }
    for (p, filter) in [(&fx.piece0, "atrim=end_sample=270003"), (&fx.piece1, "atrim=start_sample=270003,asetpts=PTS-STARTPTS")] {
        if !p.exists() {
            ffmpeg(&["-i", whole.to_str().unwrap(), "-af", filter, "-c:a", "flac", p.to_str().unwrap()]);
        }
    }
    Some(fx)
}

fn convert(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    Resampler::convert(Arc::new(Filter::new(from, to).unwrap()), samples)
}

fn rate_of(p: &Path) -> u32 {
    Source::open(p).unwrap().sample_rate
}

fn track_gain() -> GainConfig {
    GainConfig { mode: GainMode::Track, ..Default::default() }
}

fn gain_of(p: &Path, cfg: &GainConfig) -> f32 {
    linear_gain(&Source::open(p).unwrap().gain_tags, cfg)
}

fn xf(secs: f32) -> impl Fn(usize) -> Transition {
    move |_| Transition::Crossfade { secs }
}

/// `out` crossfading into `inc` over `len` frames (both at one rate already).
fn mixed(out: &[f32], go: f32, inc: &[f32], gi: f32, len: usize) -> Vec<f32> {
    let start = out.len() / 2 - len;
    let mut v: Vec<f32> = out[..start * 2].iter().map(|s| s * go).collect();
    for i in 0..len {
        let (wo, wi) = gains(i as u64, len as u64);
        for c in 0..2 {
            v.push(out[(start + i) * 2 + c] * go * wo + inc[i * 2 + c] * gi * wi);
        }
    }
    v.extend(inc[len * 2..].iter().map(|s| s * gi));
    v
}

fn first_diff(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x.to_bits() != y.to_bits()).map(|i| i / 2)
}

/// Amplitude at `f` Hz of the left channel over `frames` frames from `at`.
fn level(v: &[f32], rate: u32, at: usize, frames: usize, f: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..frames {
        let x = f64::from(v[(at + i) * 2]);
        let w = 2.0 * PI * f * i as f64 / f64::from(rate);
        re += x * w.cos();
        im += x * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / frames as f64
}

/// The largest step of a steady tone of `amp` at `f` Hz, sampled at `rate`.
fn tone_step(amp: f64, f: f64, rate: u32) -> f32 {
    (amp * 2.0 * (PI * f / f64::from(rate)).sin()) as f32
}

/// A crossfade from `out` into `inc` at another rate, `secs` long, checked to the sample.
fn crossfade_into_another_rate(out: &Path, inc: &Path, secs: f32, cfg: GainConfig) {
    let (ro, ri) = (rate_of(out), rate_of(inc));
    let (got, opens) = play(Capture::default(), &[out.to_path_buf(), inc.to_path_buf()], cfg, |_| {}, xf(secs), |_, _, _| {});
    assert_eq!(opens, vec![ro], "{ri} Hz into {ro} Hz: the device never reopens");
    let (go, gi) = (gain_of(out, &cfg), gain_of(inc, &cfg));
    let a = decode(out);
    let b = convert(&decode(inc), ri, ro);
    let len = overlap_frames(f64::from(secs), ro, Some((a.len() / 2) as u64), (a.len() / 2) as u64, Some((b.len() / 2) as u64)).unwrap() as usize;
    let want = mixed(&a, go, &b, gi, len);
    assert_eq!(got.len(), want.len(), "the outgoing track, then the incoming one converted, overlapping by {len} frames");
    assert_eq!(first_diff(&got, &want), None, "every sample: the equal-power curve, each track at its own gain");
    let start = a.len() / 2 - len;
    // Both tones in the middle of the overlap, at their true pitch, each near its curve's
    // level; nothing at the pitch the incoming one would have if it were not converted.
    let w = ro as usize / 10;
    let mid = start + len / 2 - w / 2;
    let (l_out, l_in) = (level(&got, ro, mid, w, 440.0), level(&got, ro, mid, w, 660.0));
    // At the whole hertz nearest the unconverted pitch: the real tone (a whole number of cycles
    // in the window) leaves nothing there, the wrong one would be there nearly in full.
    let wrong = level(&got, ro, start + len + ro as usize, ro as usize, (660.0 * f64::from(ri) / f64::from(ro)).round());
    let after = level(&got, ro, start + len + ro as usize, ro as usize, 660.0);
    eprintln!(
        "{} -> {} Hz, {len} frames of overlap: in its middle 440 Hz at {l_out:.4}, 660 Hz at {l_in:.4}; after it 660 Hz at {after:.6} (want {:.6}), at the unconverted pitch {wrong:.2e}",
        ri, ro, 0.5 * f64::from(gi)
    );
    let half = std::f64::consts::FRAC_1_SQRT_2 * 0.5;
    assert!((l_out / (half * f64::from(go)) - 1.0).abs() < 0.02 && (l_in / (half * f64::from(gi)) - 1.0).abs() < 0.02, "both at once, each near -3 dB");
    assert!((after / (0.5 * f64::from(gi)) - 1.0).abs() < 1e-4, "the incoming tone at its own pitch and level");
    assert!(wrong < 1e-6, "nothing at the wrong pitch");
    // No step at either edge of the overlap larger than the tones' own.
    let steady = tone_step(0.5, 660.0, ro).max(tone_step(0.5, 440.0, ro));
    for edge in [start, start + len] {
        let s = max_step(&got, edge.saturating_sub(64)..edge + 64);
        assert!(s <= steady * 1.01, "{ri} -> {ro}: a step of {s} at {edge} (the tones' own: {steady})");
    }
}

#[test]
fn a_crossfade_into_a_44_1_khz_track_overlaps_at_48_khz() {
    let Some(fx) = fixtures() else { return };
    crossfade_into_another_rate(&fx.a48, &fx.b44, 2.0, track_gain());
}

#[test]
fn a_crossfade_into_a_48_khz_track_overlaps_at_44_1_khz() {
    let Some(fx) = fixtures() else { return };
    crossfade_into_another_rate(&fx.a44, &fx.b48, 2.0, off());
}

#[test]
fn high_rates_crossfade_both_ways() {
    let Some(fx) = fixtures() else { return };
    crossfade_into_another_rate(&fx.a192, &fx.b44, 2.0, track_gain());
    crossfade_into_another_rate(&fx.a44, &fx.b192, 2.0, off());
}

#[test]
fn the_current_track_changes_in_the_middle_and_positions_are_the_track_s_own() {
    let Some(fx) = fixtures() else { return };
    let cap = Capture::paced();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(off());
    player.play(TrackRequest::new(0, fx.a48.clone()));
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut last_a, mut seen, mut last_b) = (0.0, None, 0.0);
    loop {
        if let Ok(e) = events.recv_timeout(Duration::from_millis(2)) {
            match e {
                Event::TrackStarted { token: 0 } => {
                    player.set_next_with(Some(TrackRequest::new(1, fx.b44.clone())), Transition::Crossfade { secs: 2.0 });
                }
                Event::TrackStarted { token: 1 } => {
                    let st = player.status();
                    seen = Some((st.token, st.position, st.duration));
                }
                Event::Ended => break,
                _ => {}
            }
        }
        let st = player.status();
        match st.token {
            Some(0) => last_a = st.position,
            Some(1) => last_b = st.position,
            _ => {}
        }
        assert!(Instant::now() < deadline);
    }
    drop(player);
    let (token, pos_b, dur_b) = seen.unwrap();
    eprintln!("B (44.1 kHz, converted) became current at {pos_b:.3} s; A shown to {last_a:.3} s; B last shown at {last_b:.3} of {dur_b:?}");
    assert_eq!(token, Some(1));
    assert!((1.0..1.2).contains(&pos_b), "the middle of the 2 s overlap, in B's own seconds: {pos_b}");
    assert!(last_a > 10.8 && last_a <= 11.0, "{last_a}");
    assert_eq!(dur_b, Some(12.0), "B's length, in seconds, as it is");
    // Runs to its own 12 s (a wrong ratio would end near 11.03 or 13.06); the last one polled
    // can be a little before the end.
    assert!(last_b > 11.5 && last_b <= 12.0, "{last_b}");
}

#[test]
fn the_overlap_is_never_more_than_a_fifth_of_either_track() {
    let Some(fx) = fixtures() else { return };
    // Into a 3 s track at 44.1 kHz: a fifth of it, 0.6 s, at 48 kHz.
    let (got, opens) = play(Capture::default(), &[fx.a48.clone(), fx.short3.clone()], off(), |_| {}, xf(6.0), |_, _, _| {});
    assert_eq!(opens, vec![48_000]);
    let b = convert(&decode(&fx.short3), 44_100, 48_000);
    assert_eq!(b.len() / 2, 144_000);
    let want = mixed(&decode(&fx.a48), 1.0, &b, 1.0, 144_000 / 5);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None);
    // Into a 2 s one: 0.4 s, too short to crossfade: gapless, the device reopens for it, and
    // it plays as it is.
    let (got, opens) = play(Capture::default(), &[fx.a48.clone(), fx.short2.clone()], off(), |_| {}, xf(6.0), |_, _, _| {});
    assert_eq!(opens, vec![48_000, 44_100]);
    let mut want = decode(&fx.a48);
    want.extend(decode(&fx.short2));
    assert_eq!(first_diff(&got, &want), None);
    assert_eq!(got.len(), want.len());
}

#[test]
fn an_album_after_the_crossfade_stays_gapless_through_the_converter() {
    let Some(fx) = fixtures() else { return };
    // A (48) crossfades into an album at 44.1 (two pieces of one signal, gapless), which
    // crossfades into C (48).
    let how = |i: usize| if i == 1 { Transition::Gapless } else { Transition::Crossfade { secs: 1.0 } };
    let files = [fx.a48.clone(), fx.piece0.clone(), fx.piece1.clone(), fx.c48.clone()];
    let (got, opens) = play(Capture::default(), &files, off(), |_| {}, how, |_, _, _| {});
    assert_eq!(opens, vec![48_000], "no reopen anywhere");
    let mut album = decode(&fx.piece0);
    album.extend(decode(&fx.piece1));
    // The album converted as one signal: its two pieces meet as they do at 44.1 kHz.
    let album = convert(&album, 44_100, 48_000);
    let stream = mixed(&decode(&fx.a48), 1.0, &album, 1.0, 48_000);
    let want = mixed(&stream, 1.0, &decode(&fx.c48), 1.0, 48_000);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None, "the album bit-exact across its boundary, converted, then C as it is");
}

#[test]
fn a_gapless_track_at_a_third_rate_reopens_and_plays_as_it_is() {
    let Some(fx) = fixtures() else { return };
    let how = |i: usize| if i == 0 { Transition::Crossfade { secs: 2.0 } } else { Transition::Gapless };
    let cap = Capture::default();
    let (got, opens) = play(cap, &[fx.a48.clone(), fx.b44.clone(), fx.d96.clone()], off(), |_| {}, how, |_, _, _| {});
    assert_eq!(opens, vec![48_000, 96_000]);
    let b = convert(&decode(&fx.b44), 44_100, 48_000);
    let mut want = mixed(&decode(&fx.a48), 1.0, &b, 1.0, 96_000);
    let d = decode(&fx.d96);
    let at = want.len() / 2;
    want.extend(&d);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None, "D untouched at its own rate");
    // The converted track's end (its tail complete, ending where B ends) and D's start: on the
    // left channel both tones pass through zero there, so no step larger than the tones' own.
    // (The right channel's tones start at a phase of 1 radian: a step that is in the files.)
    let left_step = (at - 64..at + 64).map(|i| (got[i * 2] - got[(i - 1) * 2]).abs()).fold(0f32, f32::max);
    // No step larger than B's own at its own rate (its converted tail rings a little, as a
    // band-limited signal that stops does), the junction included.
    assert!(left_step <= tone_step(0.5, 660.0, 44_100), "{left_step}");
}

#[test]
fn repeat_one_of_a_converted_track_stays_gapless() {
    let Some(fx) = fixtures() else { return };
    let how = |i: usize| if i == 0 { Transition::Crossfade { secs: 2.0 } } else { Transition::Gapless };
    let (got, opens) = play(Capture::default(), &[fx.a48.clone(), fx.b44.clone(), fx.b44.clone()], off(), |_| {}, how, |_, _, _| {});
    assert_eq!(opens, vec![48_000]);
    let mut bb = decode(&fx.b44);
    bb.extend(decode(&fx.b44));
    let want = mixed(&decode(&fx.a48), 1.0, &convert(&bb, 44_100, 48_000), 1.0, 96_000);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None, "the loop through the converter as one signal");
}

#[test]
fn a_next_track_that_fails_after_a_converted_one_lets_it_play_out() {
    let Some(fx) = fixtures() else { return };
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_gain_config(off());
    player.play(TrackRequest::new(0, fx.a48.clone()));
    let bad = dir("crossfade-rates-fixtures").join("missing.flac");
    let mut errors = 0;
    loop {
        match events.recv_timeout(Duration::from_secs(30)).expect("ends") {
            Event::TrackStarted { token: 0 } => player.set_next_with(Some(TrackRequest::new(1, fx.b44.clone())), Transition::Crossfade { secs: 2.0 }),
            Event::TrackStarted { token: 1 } => player.set_next_with(Some(TrackRequest::new(2, bad.clone())), Transition::Crossfade { secs: 2.0 }),
            Event::Error { token: 2, .. } => errors += 1,
            Event::Ended => break,
            _ => {}
        }
    }
    drop(player);
    assert_eq!(errors, 1);
    let want = mixed(&decode(&fx.a48), 1.0, &convert(&decode(&fx.b44), 44_100, 48_000), 1.0, 96_000);
    assert!(cap.take() == want, "B converted to its very end, no fade");
}

#[test]
fn seeking_in_a_crossfade_across_rates() {
    let Some(fx) = fixtures() else { return };
    let (a, b44) = (decode(&fx.a48), decode(&fx.b44));
    let b = convert(&b44, 44_100, 48_000);
    // Before the middle: A is sought, B waits again and comes in converted, as before.
    let mut done = false;
    let (got, opens) = play(Capture::paced(), &[fx.a48.clone(), fx.b44.clone()], off(), |_| {}, xf(2.0), |p, cap, i| {
        if i == 0 && !done {
            done = true;
            while cap.frames() < 48_000 * 21 / 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            p.seek(1.0);
        }
    });
    assert_eq!(opens, vec![48_000]);
    let after = mixed(&a[48_000 * 2..], 1.0, &b, 1.0, 96_000);
    assert_eq!(first_diff(&got[got.len() - after.len()..], &after), None, "A from 1 s, then the same crossfade");
    // After the middle: B is current, and a seek plays it at its own rate (the device reopens).
    let (got, opens) = play(Capture::paced(), &[fx.a48.clone(), fx.b44.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 {
            p.seek(3.0);
        }
    });
    assert_eq!(opens, vec![48_000, 44_100]);
    let tail = &b44[44_100 * 3 * 2..];
    assert_eq!(first_diff(&got[got.len() - tail.len()..], tail), None, "B from 3 s, at 44.1 kHz, as it is");
}

#[test]
fn a_pause_in_the_overlap_loses_nothing_and_one_after_it_returns_to_the_track_s_own_rate() {
    let Some(fx) = fixtures() else { return };
    let b44 = decode(&fx.b44);
    let b = convert(&b44, 44_100, 48_000);
    let want = mixed(&decode(&fx.a48), 1.0, &b, 1.0, 96_000);
    let fade = |rate: usize| rate * player_core::output::FADE_MS as usize / 1000;
    // In the overlap (B becomes current in its middle): the mix goes on as it was.
    let (got, opens) = play(Capture::fading(), &[fx.a48.clone(), fx.b44.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 {
            p.set_paused(true);
            std::thread::sleep(Duration::from_millis(200));
            p.set_paused(false);
        }
    });
    assert_eq!(opens, vec![48_000]);
    assert_eq!(got.len(), want.len(), "nothing lost or repeated");
    let diff: Vec<usize> = (0..want.len() / 2).filter(|&i| got[i * 2] != want[i * 2] || got[i * 2 + 1] != want[i * 2 + 1]).collect();
    assert!(!diff.is_empty() && diff.last().unwrap() - diff[0] < 2 * fade(48_000), "only the pause's fades differ");
    // After the overlap, B alone and converted: the pause plays on at 44.1 kHz.
    let mut paused = false;
    let (got, opens) = play(Capture::fading(), &[fx.a48.clone(), fx.b44.clone()], off(), |_| {}, xf(2.0), |p, cap, i| {
        if i == 1 && !paused {
            paused = true;
            while cap.frames() < 48_000 * 15 {
                std::thread::sleep(Duration::from_millis(1));
            }
            p.set_paused(true);
            std::thread::sleep(Duration::from_millis(200));
            p.set_paused(false);
        }
    });
    assert_eq!(opens, vec![48_000, 44_100], "the resume reopens at B's own rate");
    let k = first_diff(&got, &want).expect("the pause");
    let b_start = 12 * 48_000 - 96_000;
    assert!(k > 48_000 * 15 - 4800, "{k}");
    // The pause's fade-out, then B from the same moment at its own rate, faded in, then as it is.
    let s = k + fade(48_000);
    let q = (s - b_start) as u64;
    let j = ((q * 44_100 + 24_000) / 48_000) as usize;
    let fi = fade(44_100);
    eprintln!("paused at {k} (B's converted frame {q}); resumed at B's own frame {j} of {}", b44.len() / 2);
    assert_eq!(got.len() / 2, s + b44.len() / 2 - j, "nothing lost or repeated");
    assert_eq!(first_diff(&got[(s + fi) * 2..], &b44[(j + fi) * 2..]), None, "after the resume: B untouched, bit for bit");
    // The fade-in really is B from that frame.
    for i in 0..fi * 2 {
        let (g, w) = (got[s * 2 + i], b44[j * 2 + i]);
        assert!(g.abs() <= w.abs() + 1e-7 && (w.abs() < 1e-3 || g.signum() == w.signum()), "sample {i} of the fade-in: {g} of {w}");
    }
    let step = max_step(&got, s - 64..s + fi + 64);
    assert!(step <= tone_step(0.5, 660.0, 44_100) * 1.01, "no click at the pause or the reopen: {step}");
}

#[test]
fn a_skip_in_the_overlap_starts_the_new_track_as_it_is() {
    let Some(fx) = fixtures() else { return };
    let mut skipped = false;
    let (got, opens) = play(Capture::paced(), &[fx.a48.clone(), fx.b44.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 && !skipped {
            skipped = true;
            p.play(TrackRequest::new(9, fx.c48.clone()));
        }
    });
    assert_eq!(opens, vec![48_000]);
    let c = decode(&fx.c48);
    assert_eq!(first_diff(&got[got.len() - c.len()..], &c), None, "C from its start, alone, as it is");
}

/// A manual skip from `from` into `to` (another rate) with a crossfade, checked to the sample.
fn manual_skip(from: &Path, to: &Path) {
    let (ro, ri) = (rate_of(from), rate_of(to));
    let mut skipped = false;
    let heard = Arc::new(Mutex::new(0));
    let h = heard.clone();
    let (got, opens) = play(Capture::paced(), &[from.to_path_buf()], off(), |_| {}, |_| Transition::Gapless, |p, cap, i| {
        if i == 0 && !skipped {
            skipped = true;
            while cap.frames() < ro as usize * 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            *h.lock().unwrap() = common::held(cap, || p.play_crossfade(TrackRequest::new(5, to.to_path_buf()), 0.5));
        }
    });
    assert_eq!(opens, vec![ro], "{ri} Hz skipped to at {ro} Hz: no reopen");
    let (a, b) = (decode(from), convert(&decode(to), ri, ro));
    let k = first_diff(&got, &a).expect("B comes in");
    let at = *heard.lock().unwrap();
    let len = ro as usize / 2;
    eprintln!("manual skip {ro} -> {ri} Hz at frame {at}: mixing from {k}, {len} frames");
    assert!((at..at + ro as usize / 4).contains(&k), "from what was being heard: {at} {k}");
    for i in 0..len {
        let (wo, wi) = gains(i as u64, len as u64);
        for c in 0..2 {
            assert_eq!(got[(k + i) * 2 + c], b[i * 2 + c] * wi + a[(k + i) * 2 + c] * wo, "frame {i} of the crossfade");
        }
    }
    assert_eq!(first_diff(&got[(k + len) * 2..], &b[len * 2..]), None, "then B alone, converted");
    assert_eq!(got.len() / 2, k + b.len() / 2);
    let w = ro as usize / 10;
    assert!(level(&got, ro, k + len / 2 - w / 2, w, 440.0) > 0.2 && level(&got, ro, k + len / 2 - w / 2, w, 660.0) > 0.2, "both tones in the crossfade");
}

#[test]
fn a_manual_skip_to_another_rate_crossfades_both_ways() {
    let Some(fx) = fixtures() else { return };
    manual_skip(&fx.a48, &fx.b44);
    manual_skip(&fx.a44, &fx.b48);
}

/// Intersample overs: full-scale samples at 44.1 kHz whose converted samples land up to 3 dB
/// over full scale, crossfading in from a loud 48 kHz tone. With "Limit loud peaks" on (the
/// default) nothing leaves the engine over the ceiling, with the equalizer on too.
#[test]
fn the_limiter_takes_the_intersample_overs_of_a_converted_track() {
    let d = dir("crossfade-rates-fixtures");
    let (x, y) = (d.join("loud48.wav"), d.join("overs44.wav"));
    let tone: Vec<f32> = (0..48_000 * 6).flat_map(|i| {
        let s = (0.95 * (2.0 * PI * 440.0 * i as f64 / 48_000.0).sin()) as f32;
        [s, s]
    }).collect();
    common::write_wav(&x, 48_000, &tone);
    let overs: Vec<f32> = (0..44_100 * 6).flat_map(|i| if (i / 2) % 2 == 0 { [1.0, 1.0] } else { [-1.0, -1.0] }).collect();
    common::write_wav(&y, 44_100, &overs);
    let files = [x, y];
    let raw = GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() };
    let (got, _) = play(Capture::paced(), &files, raw, |_| {}, xf(1.0), |_, _, _| {});
    let tail = &got[got.len() - 44_100 * 2..];
    assert!(common::peak(tail) > 1.4, "converted, the full-scale samples rise to +3 dB: {}", common::peak(tail));
    let (got, _) = play(Capture::paced(), &files, off(), |_| {}, xf(1.0), |_, _, _| {});
    assert!(common::peak(&got) <= player_core::limiter::CEILING, "{}", common::peak(&got));
    let eq = player_core::EqParams::new(true, &player_core::eq::presets::BUILT_IN[7].bands(), player_core::Preamp::Manual(0.0));
    let (got, _) = play(Capture::paced(), &files, off(), |p| p.set_eq(&eq), xf(1.0), |_, _, _| {});
    assert!(common::peak(&got) <= player_core::limiter::CEILING, "with the equalizer: {}", common::peak(&got));
}

/// A device in exclusive mode (Windows) or PipeWire's bit-perfect mode (Linux), as far as the
/// engine sees it: it opens at the rate asked, says so in its info, and records each open.
#[derive(Clone)]
struct Fake {
    cap: Capture,
    server: Option<&'static str>,
    cfg: Arc<Mutex<OutputConfig>>,
    opens: Arc<Mutex<Vec<(u32, bool)>>>,
    rate: u32,
}

impl Device for Fake {
    fn configure(&mut self, cfg: &OutputConfig) {
        *self.cfg.lock().unwrap() = cfg.clone();
    }
    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.rate = rate;
        self.opens.lock().unwrap().push((rate, self.cfg.lock().unwrap().exclusive));
        self.cap.open(rate)
    }
    fn info(&mut self) -> Option<OutputInfo> {
        let excl = self.cfg.lock().unwrap().exclusive;
        let khz = format!("{} kHz", f64::from(self.rate) / 1000.0);
        Some(match self.server {
            // The graph runs at the stream's rate when the bit-perfect setting asks for it.
            Some(s) => OutputInfo { device: "Fake sink".into(), rate: self.rate, mix_rate: Some(self.rate), server: Some(s), exclusive: excl.then_some(khz), ..Default::default() },
            None => OutputInfo { device: "Fake DAC".into(), rate: self.rate, exclusive: excl.then(|| format!("24-bit / {khz}")), format: excl.then(|| "24-bit".into()), ..Default::default() },
        })
    }
    fn wait(&mut self, t: Duration) {
        self.cap.wait(t)
    }
    fn available(&mut self) -> Result<usize, String> {
        self.cap.available()
    }
    fn queued(&mut self) -> usize {
        self.cap.queued()
    }
    fn write(&mut self, s: &[f32]) -> Result<(), String> {
        self.cap.write(s)
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self) {}
    fn flush(&mut self) {
        self.cap.flush()
    }
    fn realtime(&self) -> bool {
        false
    }
    fn fades(&self) -> bool {
        true
    }
    fn holds_device(&self) -> bool {
        self.server.is_none() && self.cfg.lock().unwrap().exclusive
    }
}

/// A 48 kHz track crossfading into a 44.1 kHz one, paused 3 s after the overlap: the formats
/// the device was opened at and what the engine said of the conversion.
fn formats_across_a_crossfade(server: Option<&'static str>) -> (Vec<(u32, bool)>, Vec<Option<u32>>) {
    let fx = fixtures().unwrap();
    // Configured exclusive from the start (as the app does before the first track).
    let cfg = Arc::new(Mutex::new(OutputConfig { device: None, exclusive: true }));
    let fake = Fake { cap: Capture::paced(), server, cfg, opens: Arc::default(), rate: 0 };
    let dev = fake.clone();
    let (player, events) = Player::with_device(move || Box::new(dev));
    player.set_output(OutputConfig { device: None, exclusive: true });
    player.set_gain_config(off());
    player.play(TrackRequest::new(0, fx.a48.clone()));
    let mut conversions = Vec::new();
    let mut paused = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match events.recv_timeout(Duration::from_millis(5)) {
            Ok(Event::TrackStarted { token: 0 }) => player.set_next_with(Some(TrackRequest::new(1, fx.b44.clone())), Transition::Crossfade { secs: 2.0 }),
            Ok(Event::Output(info)) => {
                assert!(info.exclusive.is_some(), "the exclusive or bit-perfect format throughout");
                if conversions.last() != Some(&info.converted_from) {
                    conversions.push(info.converted_from);
                }
            }
            Ok(Event::Ended) => break,
            _ => {}
        }
        if !paused && player.status().token == Some(1) && fake.cap.frames() > 48_000 * 15 {
            paused = true;
            player.set_paused(true);
            std::thread::sleep(Duration::from_millis(200));
            player.set_paused(false);
        }
        assert!(Instant::now() < deadline, "timed out");
    }
    drop(player);
    let opens = fake.opens.lock().unwrap().clone();
    (opens, conversions)
}

#[test]
fn exclusive_mode_keeps_its_format_through_the_crossfade_and_reopens_at_the_pause() {
    if fixtures().is_none() {
        return;
    }
    let (opens, conversions) = formats_across_a_crossfade(None);
    eprintln!("exclusive: opened {opens:?}; conversion reported {conversions:?}");
    assert_eq!(opens, vec![(48_000, true), (44_100, true)], "48 kHz through the crossfade, 44.1 kHz from the resume");
    assert_eq!(conversions, vec![None, Some(44_100), None], "untouched, converted from 44.1 kHz after the middle, untouched again");
}

#[test]
fn pipewire_bit_perfect_follows_the_same_formats() {
    if fixtures().is_none() {
        return;
    }
    let (opens, conversions) = formats_across_a_crossfade(Some("PipeWire"));
    eprintln!("PipeWire bit-perfect: opened {opens:?}; conversion reported {conversions:?}");
    assert_eq!(opens, vec![(48_000, true), (44_100, true)]);
    assert_eq!(conversions, vec![None, Some(44_100), None]);
}

#[test]
fn what_conversion_costs_the_decode_thread() {
    // The whole engine as fast as it goes (an unpaced capture: the decode thread is the bottleneck),
    // a 44.1 kHz track played alone and the same crossfaded into at 48 kHz.
    let Some(fx) = fixtures() else { return };
    let time = |files: &[PathBuf], how: fn(usize) -> Transition| {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = Instant::now();
            play(Capture::default(), files, off(), |_| {}, how, |_, _, _| {});
            best = best.min(t.elapsed().as_secs_f64());
        }
        best
    };
    let native = time(&[fx.a48.clone(), fx.b48.clone()], |_| Transition::Crossfade { secs: 2.0 });
    let converted = time(&[fx.a48.clone(), fx.b44.clone()], |_| Transition::Crossfade { secs: 2.0 });
    // About 22 s of audio each.
    let per_sec = (converted - native) * 1000.0 / 22.0;
    eprintln!("engine, 48 kHz into 48 kHz: {:.0} ms; into 44.1 kHz converted: {:.0} ms; {per_sec:.2} ms more per second of audio", native * 1000.0, converted * 1000.0);
    if !cfg!(debug_assertions) {
        assert!(per_sec < 20.0, "{per_sec}");
    }
}
