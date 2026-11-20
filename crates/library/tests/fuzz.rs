//! Damaged files through everything that reads them outside playback: the scanner's tag reader
//! (`tags::read`, with its art), the metadata viewer's reader (`meta::read`, every block and
//! picture) and the art cache (`ArtCache::store`). Truncated, bytes changed in the header region
//! and through the file, inserted and deleted runs. None may panic, hang or take long; a panic
//! the scanner would catch still counts here, as it costs the file. And covers that are
//! enormous or damaged: refused quickly, without gigabytes of memory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().is_ok_and(|o| o.status.success())
}

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

fn cover() -> Vec<u8> {
    let img = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 90]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

fn fixture(dir: &Path, ext: &str, codec: &[&str], tag: TagType) -> PathBuf {
    let out = dir.join(format!("source.{ext}"));
    let o = out.to_string_lossy().into_owned();
    let mut a = vec!["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=2", "-ac", "2"];
    a.extend(codec);
    a.push(&o);
    assert!(ffmpeg(&a), "ffmpeg {ext}");
    let mut t = Tag::new(tag);
    for (k, v) in [(ItemKey::TrackTitle, "Title"), (ItemKey::TrackArtist, "Artist; Other"), (ItemKey::AlbumTitle, "Album"), (ItemKey::TrackNumber, "3"), (ItemKey::Genre, "Rock"), (ItemKey::Lyrics, "la la la")] {
        t.push(TagItem::new(k, ItemValue::Text(v.into())));
    }
    t.push_picture(Picture::unchecked(cover()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
    t.save_to_path(&out, WriteOptions::default()).unwrap();
    out
}

fn mutants(bytes: &[u8], seed: u64) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    let n = bytes.len();
    for k in [0, 1, 3, 10, 64, 128, 300, 1000, n / 3, n / 2, n - 1] {
        if k < n {
            out.push((format!("truncated at {k}"), bytes[..k].to_vec()));
        }
    }
    let head = n.min(8192);
    for i in 0..60 {
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
    for i in 0..10 {
        let mut b = bytes.to_vec();
        for _ in 0..(n / 100).max(1) {
            let at = rng.below(n);
            b[at] = rng.next() as u8;
        }
        out.push((format!("1% random #{i}"), b));
    }
    for i in 0..10 {
        let mut b = bytes.to_vec();
        let at = rng.below(head);
        if i % 2 == 0 {
            let len = 1 + rng.below(32);
            let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            b.splice(at..at, junk);
            out.push((format!("{len} inserted at {at}"), b));
        } else {
            let len = (1 + rng.below(512)).min(n - at);
            b.drain(at..at + len);
            out.push((format!("{len} deleted at {at}"), b));
        }
    }
    for i in 0..8 {
        let mut b = bytes.to_vec();
        let at = rng.below(head.saturating_sub(4));
        // A size field set huge.
        b[at..at + 4].copy_from_slice(&[0x7F, 0xFF, 0xFF, if i % 2 == 0 { 0xFF } else { 0xF0 }]);
        out.push((format!("huge size at {at}"), b));
    }
    out
}

fn timed<T: Send + 'static>(what: String, problems: &mut Vec<String>, f: impl FnOnce() -> T + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(r.is_ok());
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(true) => {}
        Ok(false) => problems.push(format!("{what}: PANIC")),
        Err(_) => problems.push(format!("{what}: HANG")),
    }
    if t0.elapsed() > Duration::from_secs(3) {
        problems.push(format!("{what}: slow ({:.1} s)", t0.elapsed().as_secs_f64()));
    }
}

#[test]
fn damaged_files_never_panic_or_hang_the_readers() {
    if !ffmpeg(&["-version"]) {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("library-fuzz");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let formats: [(&str, &[&str], TagType); 9] = [
        ("flac", &["-c:a", "flac"], TagType::VorbisComments),
        ("mp3", &["-c:a", "libmp3lame", "-b:a", "128k"], TagType::Id3v2),
        ("opus", &["-c:a", "libopus"], TagType::VorbisComments),
        ("ogg", &["-c:a", "libvorbis"], TagType::VorbisComments),
        ("m4a", &["-c:a", "aac"], TagType::Mp4Ilst),
        ("wav", &["-c:a", "pcm_s16le"], TagType::Id3v2),
        ("aiff", &["-c:a", "pcm_s16be"], TagType::Id3v2),
        ("mp2", &["-c:a", "mp2"], TagType::Id3v2),
        ("aac", &["-c:a", "aac", "-f", "adts"], TagType::Id3v2),
    ];
    let mut problems = Vec::new();
    let mut cases = 0;
    let t0 = Instant::now();
    for (i, (ext, codec, tag)) in formats.iter().enumerate() {
        let sub = dir.join(ext);
        std::fs::create_dir_all(&sub).unwrap();
        let src = fixture(&sub, ext, codec, *tag);
        let bytes = std::fs::read(&src).unwrap();
        for (j, (what, data)) in (0..3u64).flat_map(|s| mutants(&bytes, 0x51_7cc1 + i as u64 * 977 + s * 13)).enumerate() {
            let path = sub.join(format!("m{j}.{ext}"));
            std::fs::write(&path, &data).unwrap();
            cases += 1;
            let p = path.clone();
            timed(format!("{ext} {what} (tags)"), &mut problems, move || {
                let _ = library::tags::read(&p, true);
            });
            let p = path.clone();
            timed(format!("{ext} {what} (meta)"), &mut problems, move || {
                let _ = library::meta::read(&p, library::meta::ReadOptions::ALL);
                let _ = library::meta::read(&p, library::meta::ReadOptions::RAW);
            });
            let _ = std::fs::remove_file(&path);
        }
    }
    eprintln!("{cases} damaged files read in {:.1} s, {} problems", t0.elapsed().as_secs_f64(), problems.len());
    for p in &problems {
        eprintln!("  {p}");
    }
    assert!(problems.is_empty(), "{} of {cases} damaged files misbehaved:\n{}", problems.len(), problems.join("\n"));
}

/// A PNG of `w` x `h` whose pixel data is a few bytes of a compressed run: a tiny file that
/// decodes to gigabytes if taken at its word.
fn png_bomb(w: u32, h: u32) -> Vec<u8> {
    let img = image::GrayImage::new(w.min(4000), h.min(4000));
    let mut out = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    // Rewrite the IHDR's width and height (bytes 16..24) and its CRC.
    out[16..20].copy_from_slice(&w.to_be_bytes());
    out[20..24].copy_from_slice(&h.to_be_bytes());
    let crc = crc32(&out[12..29]);
    out[29..33].copy_from_slice(&crc.to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

#[test]
fn enormous_and_damaged_covers_are_refused_quickly() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("art-fuzz");
    let _ = std::fs::remove_dir_all(&dir);
    let art = library::ArtCache::new(dir.join("art"));
    let good = cover();
    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("PNG claiming 60000 x 60000".into(), png_bomb(60_000, 60_000)),
        ("PNG claiming 20000 x 20000".into(), png_bomb(20_000, 20_000)),
        ("PNG claiming 1 x 1000000".into(), png_bomb(1, 1_000_000)),
        ("empty".into(), Vec::new()),
        ("text".into(), b"not an image".to_vec()),
    ];
    let mut rng = Rng(77);
    for k in [2, 10, 100, good.len() / 2] {
        cases.push((format!("JPEG truncated at {k}"), good[..k].to_vec()));
    }
    for i in 0..40 {
        let mut b = good.clone();
        let at = rng.below(b.len().min(600));
        b[at] = rng.next() as u8;
        cases.push((format!("JPEG byte {at} changed #{i}"), b));
    }
    let mut problems = Vec::new();
    for (what, bytes) in cases {
        let a = art.clone();
        timed(format!("cover {what}"), &mut problems, move || {
            let _ = a.store(&bytes);
        });
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
