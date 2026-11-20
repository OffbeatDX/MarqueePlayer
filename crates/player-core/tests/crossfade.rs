//! Crossfades through the real engine with a capture device, against ffmpeg-made tones (a
//! different frequency per track, so each track's part of the mix can be told apart). The
//! expected stream is worked out sample by sample from the files' own decodes and the
//! curve, and must match exactly: the mix's envelope (each track at its own ReplayGain, on
//! its equal-power curve), gapless transitions untouched, a change to a rate with no
//! converter (fade-out and reopen), tracks too short for an overlap, the queue ending and a
//! next track that fails, seeks, a pause and a skip in the middle of a crossfade, and a
//! manual skip's crossfade from what is being heard. Crossfades between the usual rates,
//! converted, are `crossfade_rates.rs`. Needs ffmpeg on PATH (skips without).

mod common;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Capture, decode, dir, ffmpeg, have_ffmpeg, off, play};
use player_core::crossfade::gains;
use player_core::replaygain::linear_gain;
use player_core::source::Source;
use player_core::{GainConfig, GainMode, TrackRequest, Transition};

const RATE: usize = 48_000;

struct Fx {
    /// 440 Hz, 12 s; 660 Hz, 12 s, tagged -4 dB of track gain; 880 Hz, 12 s.
    a: PathBuf,
    b: PathBuf,
    c: PathBuf,
    /// B at 44 101 Hz: a rate with no converter from 48 kHz (`resample::Filter::new`), so the
    /// fallback (a fade-out and a reopen). Usual rates are converted: `crossfade_rates.rs`.
    b_odd: PathBuf,
    /// 2 s and 3 s of 330 Hz.
    short2: PathBuf,
    short3: PathBuf,
    /// One continuous signal in two pieces (an album's consecutive tracks).
    piece0: PathBuf,
    piece1: PathBuf,
}

fn fixtures() -> Option<Fx> {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let d = dir("crossfade-fixtures");
    let tone = |name: &str, f: u32, secs: f64, rate: usize, meta: &[&str]| {
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
    let a = tone("a.flac", 440, 12.0, RATE, &[]);
    let b = tone("b.flac", 660, 12.0, RATE, &["-metadata", "REPLAYGAIN_TRACK_GAIN=-4.00 dB"]);
    let c = tone("c.flac", 880, 12.0, RATE, &[]);
    let b_odd = tone("b-odd.flac", 660, 12.0, 44_101, &[]);
    let short2 = tone("short2.flac", 330, 2.0, RATE, &[]);
    let short3 = tone("short3.flac", 330, 3.0, RATE, &[]);
    let whole = d.join("whole.wav");
    if !whole.exists() {
        ffmpeg(&["-f", "lavfi", "-i", "aevalsrc=0.3*sin(2*PI*220*t)+0.2*sin(2*PI*3000*t)|0.3*sin(2*PI*330*t):s=48000:d=8", "-c:a", "pcm_s16le", whole.to_str().unwrap()]);
    }
    let piece = |i: usize, filter: &str| {
        let p = d.join(format!("piece{i}.flac"));
        if !p.exists() {
            ffmpeg(&["-i", whole.to_str().unwrap(), "-af", filter, "-c:a", "flac", p.to_str().unwrap()]);
        }
        p
    };
    let piece0 = piece(0, "atrim=end_sample=200003");
    let piece1 = piece(1, "atrim=start_sample=200003,asetpts=PTS-STARTPTS");
    Some(Fx { a, b, c, b_odd, short2, short3, piece0, piece1 })
}

fn track_gain() -> GainConfig {
    GainConfig { mode: GainMode::Track, ..Default::default() }
}

fn gain_of(p: &std::path::Path, cfg: &GainConfig) -> f32 {
    linear_gain(&Source::open(p).unwrap().gain_tags, cfg)
}

fn xf(secs: f32) -> impl Fn(usize) -> Transition {
    move |_| Transition::Crossfade { secs }
}

/// What the engine must play for `out` crossfading into `inc` over `len` frames.
fn mixed(out: &[f32], go: f32, inc: &[f32], gi: f32, len: usize) -> Vec<f32> {
    let n_out = out.len() / 2;
    let start = n_out - len;
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
    a.iter().zip(b).position(|(x, y)| x != y).map(|i| i / 2)
}

#[test]
fn the_mix_is_equal_power_with_each_track_at_its_own_gain() {
    let Some(fx) = fixtures() else { return };
    let cfg = track_gain();
    let (got, opens) = play(Capture::default(), &[fx.a.clone(), fx.b.clone()], cfg, |_| {}, xf(2.0), |_, _, _| {});
    let (go, gi) = (gain_of(&fx.a, &cfg), gain_of(&fx.b, &cfg));
    assert!((gi - 0.631).abs() < 0.001 && go == 1.0, "B plays at its own -4 dB: {gi}");
    let want = mixed(&decode(&fx.a), go, &decode(&fx.b), gi, 2 * RATE);
    assert_eq!(opens, vec![48_000]);
    assert_eq!(got.len(), want.len(), "12 s + 12 s - 2 s of overlap");
    assert_eq!(first_diff(&got, &want), None, "every sample of the mix as the curve says");
}

#[test]
fn the_current_track_changes_in_the_middle_of_the_overlap() {
    let Some(fx) = fixtures() else { return };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let cap = Capture::paced();
    let dev = cap.clone();
    let (player, events) = player_core::Player::with_device(move || Box::new(dev));
    player.set_gain_config(off());
    player.play(TrackRequest::new(0, fx.a.clone()));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut last_a = 0.0;
    loop {
        if let Ok(e) = events.recv_timeout(Duration::from_millis(2)) {
            match e {
                player_core::Event::TrackStarted { token: 0 } => {
                    player.set_next_with(Some(TrackRequest::new(1, fx.b.clone())), Transition::Crossfade { secs: 2.0 });
                }
                player_core::Event::TrackStarted { token: 1 } => {
                    let st = player.status();
                    s.lock().unwrap().push((st.token, st.position, last_a));
                }
                player_core::Event::Ended => break,
                _ => {}
            }
        }
        let st = player.status();
        if st.token == Some(0) {
            last_a = st.position;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    drop(player);
    let seen = seen.lock().unwrap();
    let (token, pos_b, last_a) = seen[0];
    eprintln!("B became current at {pos_b:.3} s into it; A was last shown at {last_a:.3} s");
    assert_eq!(token, Some(1));
    // The overlap is 2 s: A is current to 11 s (its last second plays under B), B from 1 s.
    assert!((1.0..1.2).contains(&pos_b), "B starts showing at the middle of the overlap: {pos_b}");
    assert!(last_a > 10.8 && last_a <= 11.0, "A shows until the middle: {last_a}");
}

#[test]
fn an_album_s_tracks_stay_gapless_and_the_next_album_crossfades() {
    let Some(fx) = fixtures() else { return };
    // The app's rule: piece0 -> piece1 is album order (gapless), piece1 -> C another album.
    let how = |i: usize| if i == 0 { Transition::Gapless } else { Transition::Crossfade { secs: 1.0 } };
    let files = [fx.piece0.clone(), fx.piece1.clone(), fx.c.clone()];
    let (got, _) = play(Capture::default(), &files, off(), |_| {}, how, |_, _, _| {});
    let mut album = decode(&fx.piece0);
    album.extend(decode(&fx.piece1));
    // The overlap is the second piece's fifth (0.77 s): no more of the album than that.
    let len = decode(&fx.piece1).len() / 2 / 5;
    let want = mixed(&album, 1.0, &decode(&fx.c), 1.0, len);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None, "the album is bit-exact across its boundary, then the crossfade");
}

#[test]
fn with_crossfade_off_the_stream_is_bit_identical() {
    let Some(fx) = fixtures() else { return };
    let files = [fx.a.clone(), fx.b.clone(), fx.c.clone()];
    let (got, _) = play(Capture::default(), &files, track_gain(), |_| {}, |_| Transition::Gapless, |_, _, _| {});
    let cfg = track_gain();
    let want: Vec<f32> = files
        .iter()
        .flat_map(|f| {
            let g = gain_of(f, &cfg);
            decode(f).into_iter().map(move |s| s * g)
        })
        .collect();
    assert_eq!(first_diff(&got, &want), None);
    assert_eq!(got.len(), want.len());
}

#[test]
fn tracks_too_short_for_an_overlap_stay_gapless_and_short_ones_overlap_less() {
    let Some(fx) = fixtures() else { return };
    // 2 s: a fifth of it is 0.4 s, under the half second worth crossfading: gapless.
    let (got, _) = play(Capture::default(), &[fx.short2.clone(), fx.a.clone()], off(), |_| {}, xf(6.0), |_, _, _| {});
    let mut want = decode(&fx.short2);
    want.extend(decode(&fx.a));
    assert_eq!(first_diff(&got, &want), None);
    assert_eq!(got.len(), want.len());
    // 3 s: the overlap is a fifth of it, 0.6 s, though 6 s were asked for.
    let (got, _) = play(Capture::default(), &[fx.short3.clone(), fx.a.clone()], off(), |_| {}, xf(6.0), |_, _, _| {});
    let want = mixed(&decode(&fx.short3), 1.0, &decode(&fx.a), 1.0, RATE * 6 / 10);
    assert_eq!(got.len(), want.len());
    assert_eq!(first_diff(&got, &want), None);
    // Into a short track: a fifth of the incoming one.
    let (got, _) = play(Capture::default(), &[fx.a.clone(), fx.short3.clone()], off(), |_| {}, xf(6.0), |_, _, _| {});
    let want = mixed(&decode(&fx.a), 1.0, &decode(&fx.short3), 1.0, RATE * 6 / 10);
    assert_eq!(first_diff(&got, &want), None);
}

#[test]
fn a_change_to_a_rate_with_no_converter_fades_out_then_reopens_for_the_next_track() {
    let Some(fx) = fixtures() else { return };
    let (got, opens) = play(Capture::default(), &[fx.a.clone(), fx.b_odd.clone()], off(), |_| {}, xf(1.5), |_, _, _| {});
    assert_eq!(opens, vec![48_000, 44_101], "no converter: the device reopens at the new rate");
    let a = decode(&fx.a);
    let len = RATE * 3 / 2;
    let start = a.len() / 2 - len;
    let mut want: Vec<f32> = a[..start * 2].to_vec();
    for i in 0..len {
        let (wo, _) = gains(i as u64, len as u64);
        want.push(a[(start + i) * 2] * 1.0 * wo);
        want.push(a[(start + i) * 2 + 1] * 1.0 * wo);
    }
    want.extend(decode(&fx.b_odd));
    assert_eq!(got.len(), want.len(), "no overlap: both tracks whole");
    assert_eq!(first_diff(&got, &want), None, "A's last 1.5 s fade out, B starts as it is");
}

#[test]
fn the_end_of_the_queue_or_a_next_track_that_fails_plays_the_last_track_out() {
    let Some(fx) = fixtures() else { return };
    // Nothing next: A plays to its end, untouched.
    let (got, _) = play(Capture::default(), std::slice::from_ref(&fx.a), off(), |_| {}, xf(2.0), |_, _, _| {});
    assert!(got == decode(&fx.a));
    // A next track that cannot be opened: reported, and A plays out untouched.
    let cap = Capture::default();
    let dev = cap.clone();
    let (player, events) = player_core::Player::with_device(move || Box::new(dev));
    player.set_gain_config(off());
    player.play(TrackRequest::new(0, fx.a.clone()));
    let bad = dir("crossfade-fixtures").join("missing.flac");
    let mut errors = 0;
    loop {
        match events.recv_timeout(Duration::from_secs(30)).expect("ends") {
            player_core::Event::TrackStarted { token: 0 } => {
                player.set_next_with(Some(TrackRequest::new(1, bad.clone())), Transition::Crossfade { secs: 2.0 });
            }
            player_core::Event::Error { token: 1, .. } => errors += 1,
            player_core::Event::Ended => break,
            _ => {}
        }
    }
    drop(player);
    assert_eq!(errors, 1);
    assert!(cap.take() == decode(&fx.a), "A whole, no fade");
}

#[test]
fn seeking_in_a_crossfade_seeks_the_track_shown() {
    let Some(fx) = fixtures() else { return };
    let (a, b) = (decode(&fx.a), decode(&fx.b));
    // Before the middle, A is current: seeking moves A, and the crossfade comes again.
    let mut done = false;
    let (got, _) = play(Capture::paced(), &[fx.a.clone(), fx.b.clone()], off(), |_| {}, xf(2.0), |p, cap, i| {
        if i == 0 && !done {
            done = true;
            while cap.frames() < RATE * 21 / 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            p.seek(1.0);
        }
    });
    let after = mixed(&a[RATE * 2..], 1.0, &b, 1.0, 2 * RATE);
    assert!(got.len() >= after.len());
    assert_eq!(first_diff(&got[got.len() - after.len()..], &after), None, "A from 1 s, then the same crossfade");
    // After the middle, B is current: B goes on alone from where it is sought to.
    let (got, _) = play(Capture::paced(), &[fx.a.clone(), fx.b.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 {
            p.seek(3.0);
        }
    });
    let tail = &b[RATE * 3 * 2..];
    assert_eq!(first_diff(&got[got.len() - tail.len()..], tail), None, "B from 3 s, nothing of A");
}

#[test]
fn a_pause_in_a_crossfade_loses_nothing() {
    let Some(fx) = fixtures() else { return };
    let (got, _) = play(Capture::fading(), &[fx.a.clone(), fx.b.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 {
            p.set_paused(true);
            std::thread::sleep(Duration::from_millis(60));
            p.set_paused(false);
        }
    });
    let want = mixed(&decode(&fx.a), 1.0, &decode(&fx.b), 1.0, 2 * RATE);
    assert_eq!(got.len(), want.len(), "nothing lost or repeated");
    let diff: Vec<usize> = (0..want.len() / 2).filter(|&i| got[i * 2] != want[i * 2] || got[i * 2 + 1] != want[i * 2 + 1]).collect();
    let fade = RATE * player_core::output::FADE_MS as usize / 1000;
    assert!(!diff.is_empty() && diff.last().unwrap() - diff[0] < 2 * fade, "only the pause's fades differ");
}

#[test]
fn a_skip_in_a_crossfade_starts_the_new_track_cleanly() {
    let Some(fx) = fixtures() else { return };
    let mut skipped = false;
    let (got, _) = play(Capture::paced(), &[fx.a.clone(), fx.b.clone()], off(), |_| {}, xf(2.0), |p, _, i| {
        if i == 1 && !skipped {
            skipped = true;
            p.play(TrackRequest::new(9, fx.c.clone()));
        }
    });
    let c = decode(&fx.c);
    assert_eq!(first_diff(&got[got.len() - c.len()..], &c), None, "C from its start, alone");
}

#[test]
fn a_manual_skip_crossfades_from_what_is_being_heard() {
    let Some(fx) = fixtures() else { return };
    let mut skipped = false;
    let heard = Arc::new(Mutex::new(0));
    let h = heard.clone();
    let (got, opens) = play(Capture::paced(), std::slice::from_ref(&fx.a), off(), |_| {}, |_| Transition::Gapless, |p, cap, i| {
        if i == 0 && !skipped {
            skipped = true;
            while cap.frames() < RATE * 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            // The device holds still while the skip is asked for and B decoded, so where it
            // comes in does not depend on how busy the machine is.
            *h.lock().unwrap() = common::held(cap, || p.play_crossfade(TrackRequest::new(5, fx.b.clone()), 0.5));
        }
    });
    assert_eq!(opens, vec![48_000]);
    let (a, b) = (decode(&fx.a), decode(&fx.b));
    let k = first_diff(&got, &a).expect("B comes in");
    let len = RATE / 2;
    let at = *heard.lock().unwrap();
    eprintln!("manual skip at {:.3} s: A carried on to {:.3} s, then {len} frames of crossfade", at as f64 / RATE as f64, k as f64 / RATE as f64);
    // Within a quarter second of it: B was decoded while the device held still, so all that
    // can delay the mix is the decode thread waking to hand it over.
    assert!(at >= RATE * 2 && (at..at + RATE / 4).contains(&k), "from what was being heard: {at} {k}");
    for i in 0..len {
        let (wo, wi) = gains(i as u64, len as u64);
        for c in 0..2 {
            let want = b[i * 2 + c] * 1.0 * wi + a[(k + i) * 2 + c] * 1.0 * wo;
            assert_eq!(got[(k + i) * 2 + c], want, "frame {i} of the manual crossfade");
        }
    }
    assert_eq!(first_diff(&got[(k + len) * 2..], &b[len * 2..]), None, "then B alone");
    assert_eq!(got.len() / 2, k + b.len() / 2);
}

#[test]
fn a_manual_skip_to_a_rate_with_no_converter_fades_out_and_reopens() {
    let Some(fx) = fixtures() else { return };
    let mut skipped = false;
    let cap = Capture::paced();
    cap.queued.store(0, Ordering::Relaxed);
    let (got, opens) = play(cap, std::slice::from_ref(&fx.a), off(), |_| {}, |_| Transition::Gapless, |p, cap, i| {
        if i == 0 && !skipped {
            skipped = true;
            while cap.frames() < RATE * 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            p.play_crossfade(TrackRequest::new(5, fx.b_odd.clone()), 0.5);
        }
    });
    assert_eq!(opens, vec![48_000, 44_101]);
    let (a, b) = (decode(&fx.a), decode(&fx.b_odd));
    let k = first_diff(&got, &a).expect("A fades");
    let fade = RATE * player_core::output::FADE_MS as usize / 1000;
    assert_eq!(first_diff(&got[(k + fade) * 2..], &b), None, "a 5 ms fade-out, then B as it is");
    assert_eq!(got.len() / 2, k + fade + b.len() / 2);
}

#[test]
fn repeat_one_and_the_same_track_again_stay_gapless() {
    let Some(fx) = fixtures() else { return };
    let (got, _) = play(Capture::default(), &[fx.a.clone(), fx.a.clone()], off(), |_| {}, |_| Transition::Gapless, |_, _, _| {});
    let mut want = decode(&fx.a);
    want.extend(decode(&fx.a));
    assert!(got == want);
}

/// Two loud tracks sum above full scale in the middle of an equal-power crossfade: the
/// limiter sees the mix (on the decode thread, or after the equalizer when it is on), so
/// nothing leaves the engine over the ceiling.
#[test]
fn the_limiter_sees_the_mix() {
    let d = dir("crossfade-fixtures");
    let tone = |f: f64| -> Vec<f32> {
        (0..RATE * 6)
            .flat_map(|i| {
                let s = (0.95 * (2.0 * std::f64::consts::PI * f * i as f64 / RATE as f64).sin()) as f32;
                [s, s]
            })
            .collect()
    };
    let (x, y) = (d.join("loud-x.wav"), d.join("loud-y.wav"));
    common::write_wav(&x, RATE as u32, &tone(440.0));
    common::write_wav(&y, RATE as u32, &tone(445.0));
    let files = [x, y];
    let raw = GainConfig { mode: GainMode::Off, limit_peaks: false, ..Default::default() };
    let (got, _) = play(Capture::paced(), &files, raw, |_| {}, xf(1.0), |_, _, _| {});
    assert!(common::peak(&got) > 1.2, "the fixture really sums over full scale: {}", common::peak(&got));
    let (got, _) = play(Capture::paced(), &files, off(), |_| {}, xf(1.0), |_, _, _| {});
    assert!(common::peak(&got) <= player_core::limiter::CEILING, "{}", common::peak(&got));
    let eq = player_core::EqParams::new(true, &player_core::eq::presets::BUILT_IN[7].bands(), player_core::Preamp::Manual(0.0));
    let (got, _) = play(Capture::paced(), &files, off(), |p| p.set_eq(&eq), xf(1.0), |_, _, _| {});
    assert!(common::peak(&got) <= player_core::limiter::CEILING, "with the equalizer: {}", common::peak(&got));
}
