//! Surround files through `Source`: 5.1 and 7.1, FLAC and WAV, from ffmpeg, with a tone
//! of its own in every channel. Each tone must come out where the layout puts it, at the
//! level the downmix gives it: the centre in both sides at -3 dB of a front channel, the
//! LFE gone, surrounds and backs on their own side at -3 dB. And a file with every
//! channel at full scale in phase must not clip.

use std::path::{Path, PathBuf};
use std::process::Command;

use player_core::source::Source;

const RATE: f64 = 48_000.0;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().is_ok_and(|o| o.status.success())
}

fn decode(p: &Path) -> Vec<f32> {
    let mut s = Source::open(p).unwrap();
    let mut v = Vec::new();
    while s.next_chunk(&mut v).unwrap() {}
    v
}

/// Amplitude of `freq` in one channel of interleaved stereo (`ch` 0 or 1).
fn level(v: &[f32], ch: usize, freq: f64) -> f64 {
    let n = v.len() / 2;
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..n {
        let t = 2.0 * std::f64::consts::PI * freq * i as f64 / RATE;
        let x = f64::from(v[i * 2 + ch]);
        re += x * t.cos();
        im += x * t.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n as f64
}

#[test]
fn surround_is_mixed_down_by_layout() {
    if !ffmpeg(&["-version"]) {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("surround");
    std::fs::create_dir_all(&dir).unwrap();
    // ffmpeg's orders: 5.1 = FL FR FC LFE BL BR; 7.1 = FL FR FC LFE BL BR SL SR.
    let freqs = [300.0, 500.0, 700.0, 60.0, 1100.0, 1300.0, 1700.0, 1900.0];
    let h = std::f64::consts::FRAC_1_SQRT_2;
    for (layout, n) in [("5.1", 6), ("7.1", 8)] {
        let expr: Vec<String> = freqs[..n].iter().map(|f| format!("0.5*sin(2*PI*{f}*t)")).collect();
        let src = format!("aevalsrc={}:c={layout}:s=48000:d=1", expr.join("|"));
        for ext in ["flac", "wav"] {
            let f = dir.join(format!("tones-{layout}.{ext}"));
            assert!(ffmpeg(&["-f", "lavfi", "-i", &src, f.to_str().unwrap()]), "{layout} {ext}");
            let v = decode(&f);
            let l = |i: usize| level(&v, 0, freqs[i]);
            let r = |i: usize| level(&v, 1, freqs[i]);
            let front = l(0);
            eprintln!(
                "{layout} {ext}: FL {:.3}/{:.3}  FR {:.3}/{:.3}  FC {:.3}/{:.3}  LFE {:.4}/{:.4}  BL {:.3}/{:.3}  BR {:.3}/{:.3}",
                l(0), r(0), l(1), r(1), l(2), r(2), l(3), r(3), l(4), r(4), l(5), r(5)
            );
            assert!(front > 0.05, "front left is there");
            assert!(r(0) < front * 0.01 && l(1) < front * 0.01, "fronts stay on their side");
            assert!((r(1) / front - 1.0).abs() < 0.02, "front right as loud as front left");
            assert!((l(2) / front - h).abs() < 0.02 && (r(2) / front - h).abs() < 0.02, "centre in both sides at -3 dB");
            assert!(l(3) < front * 0.01 && r(3) < front * 0.01, "no LFE");
            assert!((l(4) / front - h).abs() < 0.02 && r(4) < front * 0.01, "back left on the left at -3 dB");
            assert!((r(5) / front - h).abs() < 0.02 && l(5) < front * 0.01, "back right on the right at -3 dB");
            if n == 8 {
                assert!((l(6) / front - h).abs() < 0.02 && r(6) < front * 0.01, "side left");
                assert!((r(7) / front - h).abs() < 0.02 && l(7) < front * 0.01, "side right");
            }
        }
        // Every channel at full scale, in phase: still no sample past full scale.
        let full: Vec<String> = (0..n).map(|_| "0.999*sin(2*PI*200*t)".to_owned()).collect();
        let f = dir.join(format!("full-{layout}.wav"));
        assert!(ffmpeg(&["-f", "lavfi", "-i", &format!("aevalsrc={}:c={layout}:s=48000:d=1", full.join("|")), "-c:a", "pcm_f32le", f.to_str().unwrap()]));
        let v = decode(&f);
        let peak = v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        eprintln!("{layout}: every channel at full scale: peak {peak:.3}");
        assert!(peak <= 1.0, "{layout} clips: {peak}");
    }
}
