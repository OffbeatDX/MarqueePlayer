//! Damaged files of every format the player opens, through `Source` as the engine uses it
//! (open, decode to the end, seek, decode on): truncated anywhere, bytes changed in the header
//! region and through the file, runs of zeros and of 0xFF. No mutant may panic out of
//! `Source` (the engine's threads would die and the app sit "playing" in silence), take longer
//! than a few seconds (a decoder looping on a hostile header), or decode more audio than the
//! file could hold (a size field taken at its word). Deterministic: the same mutants every run.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use player_core::source::Source;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// (extension, ffmpeg codec arguments)
const FORMATS: &[(&str, &[&str])] = &[
    ("flac", &["-c:a", "flac"]),
    ("mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]),
    ("mp2", &["-c:a", "mp2", "-b:a", "192k"]),
    ("opus", &["-c:a", "libopus", "-b:a", "64k"]),
    ("ogg", &["-c:a", "libvorbis", "-q:a", "2"]),
    ("m4a", &["-c:a", "aac", "-b:a", "128k"]),
    ("alac.m4a", &["-c:a", "alac"]),
    ("wav", &["-c:a", "pcm_s16le"]),
    ("aiff", &["-c:a", "pcm_s16be"]),
    ("aac", &["-c:a", "aac", "-b:a", "128k", "-f", "adts"]),
];

fn fixture(dir: &Path, ext: &str, codec: &[&str]) -> PathBuf {
    let out = dir.join(format!("source.{ext}"));
    if !out.exists() {
        let o = out.to_string_lossy().into_owned();
        let mut a = vec!["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=3", "-ac", "2"];
        a.extend(codec);
        a.push(&o);
        common::ffmpeg(&a);
    }
    out
}

fn mutants(bytes: &[u8], seed: u64) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    let n = bytes.len();
    for k in [0, 1, 4, 12, 44, 100, 512, 2048, n / 4, n / 2, n - 1] {
        if k < n {
            out.push((format!("truncated at {k}"), bytes[..k].to_vec()));
        }
    }
    let head = n.min(4096);
    for i in 0..48 {
        let mut b = bytes.to_vec();
        let at = rng.below(head);
        b[at] = match i % 4 {
            0 => 0xFF,
            1 => 0x00,
            2 => b[at] ^ (1 << rng.below(8)),
            _ => rng.next() as u8,
        };
        out.push((format!("header byte {at}"), b));
    }
    for i in 0..16 {
        let mut b = bytes.to_vec();
        for _ in 0..(n / 100).max(1) {
            let at = rng.below(n);
            b[at] = rng.next() as u8;
        }
        out.push((format!("1% random bytes #{i}"), b));
    }
    for i in 0..12 {
        let mut b = bytes.to_vec();
        let at = rng.below(n);
        if i % 2 == 0 {
            let len = 1 + rng.below(64);
            let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            b.splice(at..at, junk);
            out.push((format!("{len} bytes inserted at {at}"), b));
        } else {
            let len = (1 + rng.below(4096)).min(n - at);
            b.drain(at..at + len);
            out.push((format!("{len} bytes deleted at {at}"), b));
        }
    }
    for (label, fill) in [("zeros", 0x00u8), ("0xFF", 0xFF)] {
        for _ in 0..6 {
            let mut b = bytes.to_vec();
            let at = rng.below(head);
            let len = 4 + rng.below(64);
            for x in b.iter_mut().skip(at).take(len) {
                *x = fill;
            }
            out.push((format!("{len} {label} at {at}"), b));
        }
    }
    out
}

/// Open, decode to the end, seek to the middle and decode on. The frames decoded.
fn exercise(path: &Path) -> Result<usize, String> {
    let mut src = Source::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut frames = 0;
    for _ in 0..20_000 {
        buf.clear();
        match src.next_chunk(&mut buf) {
            Ok(true) => frames += buf.len() / 2,
            Ok(false) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = src.seek(44_100);
    for _ in 0..2_000 {
        buf.clear();
        match src.next_chunk(&mut buf) {
            Ok(true) => frames += buf.len() / 2,
            _ => break,
        }
    }
    Ok(frames)
}

#[test]
fn damaged_files_never_panic_hang_or_balloon() {
    if !common::have_ffmpeg() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = common::dir("fuzz");
    let mut problems = Vec::new();
    let mut cases = 0;
    let mut errors = 0;
    let t0 = Instant::now();
    for (i, (ext, codec)) in FORMATS.iter().enumerate() {
        let src = fixture(&dir, ext, codec);
        let bytes = std::fs::read(&src).unwrap();
        for (j, (what, data)) in (0..4u64).flat_map(|s| mutants(&bytes, 0x9e37_79b9 + i as u64 * 31 + s * 7919)).enumerate() {
            let path = dir.join(format!("m{i}-{j}.{ext}"));
            std::fs::write(&path, &data).unwrap();
            cases += 1;
            let (tx, rx) = mpsc::channel();
            let p = path.clone();
            let started = Instant::now();
            std::thread::spawn(move || {
                let r = std::panic::catch_unwind(|| exercise(&p));
                let _ = tx.send(r);
            });
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok(Ok(frames))) => {
                    // 3 s at 44.1 kHz, plus the replay after the seek: never more than ~5 s of audio.
                    if frames > 44_100 * 6 {
                        problems.push(format!("{ext} {what}: decoded {frames} frames from a 3 s file"));
                    }
                }
                Ok(Ok(Err(_))) => errors += 1,
                Ok(Err(_)) => problems.push(format!("{ext} {what}: PANIC escaped Source")),
                Err(_) => problems.push(format!("{ext} {what}: HANG (still decoding after 10 s)")),
            }
            if started.elapsed() > Duration::from_secs(3) {
                problems.push(format!("{ext} {what}: slow ({:.1} s)", started.elapsed().as_secs_f64()));
            }
            let _ = std::fs::remove_file(&path);
        }
    }
    eprintln!("{cases} damaged files in {:.1} s: {errors} refused with an error, {} problems", t0.elapsed().as_secs_f64(), problems.len());
    for p in &problems {
        eprintln!("  {p}");
    }
    assert!(problems.is_empty(), "{} of {cases} damaged files misbehaved:\n{}", problems.len(), problems.join("\n"));
}
