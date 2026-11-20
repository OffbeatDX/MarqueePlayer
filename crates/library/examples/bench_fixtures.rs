//! Every fixture any bench mode needs, in one folder (`tools/bench-runner` copies a fresh set
//! for each mode, so the modes that change their files never see another run's changes):
//!
//!     cargo build --release -p library --examples
//!     cargo run --release -p library --example bench_fixtures -- %TEMP%\mp-bench-fixtures
//!
//! The folder must be inside the system's temporary folder and must not exist yet, or be one
//! this tool made (it holds `.mp-bench-fixtures`: it is emptied first). Never a music folder.
//! Needs ffmpeg on PATH, and the other fixture tools built beside this one (they are run for
//! their sets). What it makes:
//!
//! - `core/`: the library the first benches were written against (`grid`, `sidebar`, `tour`,
//!   `gapless`, `skip`, `theme`, `session-*`, `guard`, `idle-*`, `covers`, `playlist`,
//!   `phase4`, `phase5`, `cleanup`, `settings`, `autoscroll`, `tray`, `about`, `eq`): 320
//!   albums with covers for the grid, "Lemonade" (Opus, gapless, a featured credit only in a
//!   title), a Dvořák album, Warp's label, "Bulk" albums for "See all", a CJK album, the
//!   "Sampler" of every format, "Night Harbour" with a synced `.lrc`, "Paper Moons" with
//!   lyrics in its tags, "Groove" (long, with long synced lyrics) and "Long Album" (60 tracks),
//!   an album whose second track cannot be played (5.1 AAC) and one whose second track is
//!   damaged, and unreadable files.
//! - `formats/`: the technical details' albums (`details`): 16-bit and hi-res FLAC, MP3 CBR
//!   and VBR, a mixed album with and without a main format, mono and 5.1, PCM, a CJK title.
//! - `words/`: "Words", whose titles say what the runner's mock LRCLIB answers (`visualizer`).
//! - `viz/`, `messy/`, `meta/`, `edit/`: the sets of `viz_fixtures`, `messy_library` (240
//!   albums), `meta_fixtures` and `edit_fixtures` (marked `.mp-edit-fixtures`, so the editor's
//!   write guard lets the bench write there).
use std::path::{Path, PathBuf};
use std::process::Command;

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem, TagType};

const MARKER: &str = ".mp-bench-fixtures";

fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).status().is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg {args:?}");
}

/// An encoding: the extension, ffmpeg's codec arguments, and the tag the file gets.
#[derive(Clone, Copy, PartialEq)]
enum Enc {
    Flac,
    Flac24,
    Mp3,
    Mp3Cbr,
    Mp3Vbr,
    Mp2,
    Opus,
    Ogg,
    Aac,
    Alac,
    RawAac,
    Wav,
    Aiff,
}

impl Enc {
    fn ext(self) -> &'static str {
        match self {
            Enc::Flac | Enc::Flac24 => "flac",
            Enc::Mp3 | Enc::Mp3Cbr | Enc::Mp3Vbr => "mp3",
            Enc::Mp2 => "mp2",
            Enc::Opus => "opus",
            Enc::Ogg => "ogg",
            Enc::Aac | Enc::Alac => "m4a",
            Enc::RawAac => "aac",
            Enc::Wav => "wav",
            Enc::Aiff => "aiff",
        }
    }
    fn codec(self) -> &'static [&'static str] {
        match self {
            Enc::Flac => &["-c:a", "flac", "-sample_fmt", "s16"],
            Enc::Flac24 => &["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
            Enc::Mp3 => &["-c:a", "libmp3lame", "-b:a", "128k"],
            Enc::Mp3Cbr => &["-c:a", "libmp3lame", "-b:a", "320k"],
            Enc::Mp3Vbr => &["-c:a", "libmp3lame", "-q:a", "0"],
            Enc::Mp2 => &["-c:a", "mp2", "-b:a", "192k"],
            Enc::Opus => &["-c:a", "libopus", "-b:a", "48k"],
            Enc::Ogg => &["-c:a", "libvorbis", "-q:a", "2"],
            Enc::Aac | Enc::RawAac => &["-c:a", "aac", "-b:a", "128k"],
            Enc::Alac => &["-c:a", "alac"],
            Enc::Wav => &["-c:a", "pcm_s16le"],
            Enc::Aiff => &["-c:a", "pcm_s16be"],
        }
    }
    fn tag(self) -> TagType {
        match self {
            Enc::Flac | Enc::Flac24 | Enc::Opus | Enc::Ogg => TagType::VorbisComments,
            Enc::Aac | Enc::Alac => TagType::Mp4Ilst,
            _ => TagType::Id3v2,
        }
    }
}

/// A tone file: `secs` long at `rate` Hz with `layout` ("mono", "stereo", "5.1"), made once
/// and copied for every track that uses it.
struct Tones {
    dir: PathBuf,
}

impl Tones {
    fn get(&self, enc: Enc, secs: u32, rate: u32, layout: &str) -> PathBuf {
        let name = format!("{:?}-{secs}-{rate}-{}.{}", enc as u8, layout.replace('.', ""), enc.ext());
        let out = self.dir.join(name);
        if !out.exists() {
            let freq = 220 + (secs * 37 + rate / 1000) % 600;
            let src = format!("sine=frequency={freq}:sample_rate={rate}:duration={secs}");
            let pan = match layout {
                "mono" => "pan=mono|c0=c0",
                "5.1" => "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0",
                _ => "pan=stereo|c0=c0|c1=c0",
            };
            let o = out.to_string_lossy().into_owned();
            let mut a = vec!["-f", "lavfi", "-i", &src, "-af", pan, "-map_metadata", "-1"];
            if enc == Enc::RawAac {
                a.extend(["-f", "adts"]);
            }
            a.extend(enc.codec());
            a.push(&o);
            ffmpeg(&a);
        }
        out
    }
}

/// A cover: a gradient with bands, different for each seed.
fn cover(seed: usize, size: u32) -> Vec<u8> {
    let s = seed as u32;
    let (r0, g0, b0) = ((s * 67 % 200) as u8 + 30, (s * 131 % 200) as u8 + 30, (s * 29 % 200) as u8 + 30);
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let band = ((x + y * (s % 3 + 1)) / (size / 6).max(1)).is_multiple_of(2);
        let t = (x + y) as f32 / (2 * size) as f32;
        let f = |c: u8| ((c as f32) * (1.0 - t * 0.6) + if band { 25.0 } else { 0.0 }).min(255.0) as u8;
        image::Rgb([f(r0), f(g0), f(b0)])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

/// One track to write: where, from which tone, with which tags.
struct T {
    rel: String,
    enc: Enc,
    secs: u32,
    rate: u32,
    layout: &'static str,
    items: Vec<(ItemKey, String)>,
    art: Option<Vec<u8>>,
}

fn t(rel: impl Into<String>, enc: Enc, secs: u32, items: &[(ItemKey, &str)]) -> T {
    T { rel: rel.into(), enc, secs, rate: 44_100, layout: "stereo", items: items.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(), art: None }
}

fn put(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write(root: &Path, tones: &Tones, tr: &T) {
    let path = root.join(&tr.rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(tones.get(tr.enc, tr.secs, tr.rate, tr.layout), &path).unwrap();
    let mut tag = Tag::new(tr.enc.tag());
    for (k, v) in &tr.items {
        tag.push(TagItem::new(*k, ItemValue::Text(v.clone())));
    }
    if let Some(data) = &tr.art {
        tag.push_picture(Picture::unchecked(data.clone()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
    }
    use lofty::tag::TagExt;
    if let Err(e) = tag.save_to_path(&path, WriteOptions::default()) {
        panic!("{}: {e}", path.display());
    }
}

/// An album's tracks: `n` of them, `secs` each, numbered, by `artist`.
#[allow(clippy::too_many_arguments)]
fn album(out: &mut Vec<T>, folder: &str, artist: &str, title: &str, enc: Enc, n: usize, secs: u32, extra: &[(ItemKey, &str)]) {
    for i in 1..=n {
        let name = format!("Track {i}");
        let num = i.to_string();
        let mut items = vec![
            (ItemKey::TrackArtist, artist),
            (ItemKey::AlbumArtist, artist),
            (ItemKey::AlbumTitle, title),
            (ItemKey::TrackTitle, name.as_str()),
            (ItemKey::TrackNumber, num.as_str()),
        ];
        items.extend_from_slice(extra);
        out.push(t(format!("{folder}/{i:02} {name}.{}", enc.ext()), enc, secs, &items));
    }
}

fn lrc(lines: usize, every: f32, words: &str) -> String {
    (0..lines).map(|i| {
        let at = i as f32 * every + 0.5;
        format!("[{:02}:{:05.2}]{words} line {}\n", (at / 60.0) as u32, at % 60.0, i + 1)
    }).collect()
}

fn core(root: &Path, tones: &Tones) {
    use ItemKey as K;
    let mut out: Vec<T> = Vec::new();
    // First by folder, so first by id: an album of five whose second track cannot be played
    // (5.1 AAC, which the decoder refuses), for `skip`.
    for i in 1..=5 {
        let enc = if i == 2 { Enc::Aac } else { Enc::Flac };
        let mut tr = t(
            format!("000 Skip/0{i} Skip {i}.{}", enc.ext()),
            enc,
            6,
            &[(K::TrackArtist, "Fixture Skip"), (K::AlbumArtist, "Fixture Skip"), (K::AlbumTitle, "Skip Test"), (K::TrackTitle, &format!("Skip {i}")), (K::TrackNumber, &i.to_string())],
        );
        if i == 2 {
            tr.layout = "5.1";
        }
        out.push(tr);
    }
    // The first album by artist (phase5 plays it and scrobbles its 40 s first track at 20 s).
    album(&mut out, "001 Aardvark Quartet/First Light", "Aardvark Quartet", "First Light", Enc::Opus, 3, 40, &[(K::Genre, "Jazz"), (K::Label, "Paper Moon")]);
    // Lemonade: Opus, gapless (the first Opus album of three or more by id), a featured credit
    // only in a title ("kendrick" finds only that track's credit), Beyoncé's accent.
    let lemonade = [
        "Pray You Catch Me",
        "Hold Up",
        "Don't Hurt Yourself (feat. Jack White)",
        "Sorry",
        "Freedom (feat. Kendrick Lamar)",
        "All Night",
    ];
    for (i, title) in lemonade.iter().enumerate() {
        let n = (i + 1).to_string();
        let mut tr = t(
            format!("010 Beyoncé/Lemonade/{:02} {}.opus", i + 1, title.replace(['(', ')', '\''], "")),
            Enc::Opus,
            9,
            &[(K::TrackArtist, "Beyoncé"), (K::AlbumArtist, "Beyoncé"), (K::AlbumTitle, "Lemonade"), (K::TrackTitle, title), (K::TrackNumber, &n), (K::RecordingDate, "2016-04-23"), (K::Genre, "R&B"), (K::Label, "Parkwood")],
        );
        tr.art = Some(cover(7, 600));
        out.push(tr);
    }
    album(&mut out, "020 Dvořák/Symphony No. 9", "Antonín Dvořák", "Symphony No. 9 From the New World", Enc::Flac, 4, 3, &[(K::Genre, "Classical"), (K::RecordingDate, "1893")]);
    album(&mut out, "030 Aphex Fixture/Selected Ambient Works", "Aphex Fixture", "Selected Ambient Works", Enc::Flac, 3, 2, &[(K::Label, "Warp"), (K::Genre, "Electronic"), (K::RecordingDate, "1992")]);
    album(&mut out, "031 Boards of Fixtures/Music Has the Right", "Boards of Fixtures", "Music Has the Right", Enc::Mp3, 3, 2, &[(K::Label, "Warp"), (K::Genre, "Electronic"), (K::RecordingDate, "1998")]);
    album(&mut out, "040 かめりあ/ニューワールド", "かめりあ", "ニューワールド", Enc::Flac, 3, 2, &[(K::Genre, "Electronic"), (K::RecordingDate, "2020")]);
    put(&root.join("040 かめりあ/ニューワールド/cover.jpg"), &cover(40, 600));
    // Night Harbour: a synced .lrc beside its first track (a line every 1.5 s), 30 s tracks.
    album(&mut out, "050 Mara Lind/Night Harbour", "Mara Lind", "Night Harbour", Enc::Flac, 3, 30, &[(K::Genre, "Folk"), (K::RecordingDate, "2021-06-01")]);
    put(&root.join("050 Mara Lind/Night Harbour/cover.jpg"), &cover(50, 600));
    // Paper Moons: lyrics in the first track's tags (unsynced), none in the second.
    out.push(t(
        "051 Mara Lind/Paper Moons/01 Paper Moons.flac",
        Enc::Flac,
        8,
        &[(K::TrackArtist, "Mara Lind"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Paper Moons"), (K::TrackTitle, "Paper Moons"), (K::TrackNumber, "1"), (K::Lyrics, "Paper moons over the water\nfolded light on the tide\nwe kept the lanterns burning\nall through the night")],
    ));
    out.push(t(
        "051 Mara Lind/Paper Moons/02 Ash and Clover.flac",
        Enc::Flac,
        8,
        &[(K::TrackArtist, "Mara Lind"), (K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Paper Moons"), (K::TrackTitle, "Ash and Clover"), (K::TrackNumber, "2")],
    ));
    // Sampler: one track of every container the scanner takes.
    for (i, enc) in [Enc::Flac, Enc::Mp3, Enc::Opus, Enc::Ogg, Enc::Aac, Enc::Alac, Enc::Wav, Enc::Aiff, Enc::Mp2, Enc::RawAac].into_iter().enumerate() {
        let n = (i + 1).to_string();
        let title = format!("Sampler {}", enc.ext().to_uppercase());
        out.push(t(
            format!("060 Sampler/{:02} {title}.{}", i + 1, enc.ext()),
            enc,
            3,
            &[(K::TrackArtist, "Format Band"), (K::AlbumArtist, "Format Band"), (K::AlbumTitle, "Sampler"), (K::TrackTitle, &title), (K::TrackNumber, &n)],
        ));
    }
    // Groove: two long tracks (the tray bench plays a minute or more), the first with long
    // synced lyrics (autoscroll scrolls them).
    album(&mut out, "070 Fixture Band/Groove", "Fixture Band", "Groove", Enc::Opus, 2, 150, &[(K::Genre, "Funk")]);
    // Long Album: a queue long enough to scroll.
    album(&mut out, "071 Fixture Band/Long Album", "Fixture Band", "Long Album", Enc::Opus, 60, 2, &[(K::Genre, "Funk")]);
    // An album whose second track's audio is damaged (its tags read, the audio does not).
    album(&mut out, "080 Fixture Damage/Damaged", "Fixture Damage", "Damaged", Enc::Flac, 4, 4, &[]);
    // Greek letters: search for "alpha", then "beta" (session-check types both).
    out.push(t("091 Fixture Greek/Greek Letters/01 Alpha.flac", Enc::Flac, 2, &[(K::TrackArtist, "Fixture Greek"), (K::AlbumArtist, "Fixture Greek"), (K::AlbumTitle, "Greek Letters"), (K::TrackTitle, "Alpha"), (K::TrackNumber, "1")]));
    out.push(t("091 Fixture Greek/Greek Letters/02 Beta.flac", Enc::Flac, 2, &[(K::TrackArtist, "Fixture Greek"), (K::AlbumArtist, "Fixture Greek"), (K::AlbumTitle, "Greek Letters"), (K::TrackTitle, "Beta"), (K::TrackNumber, "2")]));
    // Bulk: more of everything than search shows ("See all").
    for i in 1..=40 {
        let (artist, title, track) = (format!("Bulk Artist {i}"), format!("Bulk Album {i}"), format!("Bulk Track {i}"));
        let mut tr = t(
            format!("090 Bulk/{artist}/{title}/01 {track}.flac"),
            Enc::Flac,
            1,
            &[(K::TrackArtist, &artist), (K::AlbumArtist, &artist), (K::AlbumTitle, &title), (K::TrackTitle, &track), (K::TrackNumber, "1"), (K::Genre, "Bulk")],
        );
        tr.art = Some(cover(900 + i, 300));
        out.push(tr);
    }
    // Filler for the grid: 300 albums with covers, eight short tracks each, many artists.
    const W: &[&str] = &["Night", "Light", "Glass", "Velvet", "Static", "Harbour", "Lantern", "Echo", "Signal", "River", "Comet", "Garden"];
    const G: &[&str] = &["Rock", "Pop", "Electronic", "Jazz", "Hip-Hop", "Folk", "Ambient", "Metal", "Soul", "Indie"];
    for i in 0..300 {
        let artist = format!("{} {}", W[i % W.len()], ["Pilots", "Owls", "Tides", "Foxes", "Choir", "Engines", "Wolves"][(i / 3) % 7]);
        let title = format!("{} {} {}", W[(i * 5 + 3) % W.len()], W[(i * 7 + 1) % W.len()], i + 1);
        let year = (1970 + i % 55).to_string();
        let genre = G[i % G.len()];
        let folder = format!("100 Grid/{artist}/{title}");
        album(&mut out, &folder, &artist, &title, if i % 2 == 0 { Enc::Flac } else { Enc::Mp3 }, 8, 1, &[(K::Genre, genre), (K::RecordingDate, &year)]);
        let dir = root.join(&folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cover.jpg"), cover(i, 500)).unwrap();
    }
    for tr in &out {
        write(root, tones, tr);
    }
    let grooves = root.join("070 Fixture Band/Groove");
    std::fs::write(grooves.join("01 Track 1.lrc"), lrc(90, 1.6, "Groove")).unwrap();
    std::fs::write(root.join("050 Mara Lind/Night Harbour/01 Track 1.lrc"), lrc(20, 1.5, "Harbour")).unwrap();
    // The damaged track: its tags are intact, its audio frames overwritten past the header.
    let damaged = root.join("080 Fixture Damage/Damaged/02 Track 2.flac");
    let mut bytes = std::fs::read(&damaged).unwrap();
    let from = bytes.len() / 3;
    for (i, b) in bytes[from..].iter_mut().enumerate() {
        *b = (i * 151 % 251) as u8;
    }
    std::fs::write(&damaged, bytes).unwrap();
    // Unreadable files: garbage with an audio extension, an empty file, a text file named .mp3.
    let broken = root.join("999 Broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("garbage.flac"), (0..4096u32).map(|i| (i * 31 % 256) as u8).collect::<Vec<_>>()).unwrap();
    std::fs::write(broken.join("empty.mp3"), b"").unwrap();
    std::fs::write(broken.join("notes.mp3"), b"This is not an MP3 file, it is a note.\n").unwrap();
    println!("core: {} tracks", out.len());
}

fn formats(root: &Path, tones: &Tones) {
    use ItemKey as K;
    let mut out: Vec<T> = Vec::new();
    let tag = |album: &str, title: &str, n: usize| -> Vec<(ItemKey, String)> {
        vec![
            (K::TrackArtist, "Format Tester".into()),
            (K::AlbumArtist, "Format Tester".into()),
            (K::AlbumTitle, album.into()),
            (K::TrackTitle, title.into()),
            (K::TrackNumber, n.to_string()),
        ]
    };
    let mut add = |folder: &str, album_title: &str, list: &[(&str, Enc, u32, &'static str)], gain: bool| {
        for (i, (title, enc, rate, layout)) in list.iter().enumerate() {
            let mut items = tag(album_title, title, i + 1);
            if gain && i % 2 == 0 {
                items.push((K::ReplayGainTrackGain, "-6.20 dB".into()));
                items.push((K::ReplayGainTrackPeak, "0.988".into()));
            }
            out.push(T { rel: format!("{folder}/{:02} {title}.{}", i + 1, enc.ext()), enc: *enc, secs: 6, rate: *rate, layout, items, art: None });
        }
    };
    add("Lossless 16", "Lossless 16", &[("One", Enc::Flac, 44_100, "stereo"), ("Two", Enc::Flac, 44_100, "stereo"), ("Three", Enc::Flac, 44_100, "stereo")], true);
    add("Hi-Res", "Hi-Res", &[("High One", Enc::Flac24, 96_000, "stereo"), ("High Two", Enc::Flac24, 96_000, "stereo")], false);
    add("MP3 CBR", "MP3 CBR", &[("Constant One", Enc::Mp3Cbr, 44_100, "stereo"), ("Constant Two", Enc::Mp3Cbr, 44_100, "stereo"), ("Constant Three", Enc::Mp3Cbr, 44_100, "stereo")], false);
    add("MP3 VBR", "MP3 VBR", &[("Variable One", Enc::Mp3Vbr, 44_100, "stereo"), ("Variable Two", Enc::Mp3Vbr, 44_100, "stereo"), ("Variable Three", Enc::Mp3Vbr, 44_100, "stereo")], false);
    add(
        "Mixed Bag",
        "Mixed Bag",
        &[("FLAC", Enc::Flac, 44_100, "stereo"), ("MP3", Enc::Mp3, 44_100, "stereo"), ("Opus", Enc::Opus, 48_000, "stereo"), ("AAC", Enc::Aac, 44_100, "stereo"), ("Vorbis", Enc::Ogg, 44_100, "stereo"), ("WAV", Enc::Wav, 48_000, "stereo")],
        true,
    );
    add(
        "Mostly FLAC",
        "Mostly FLAC",
        &[("Mostly One", Enc::Flac, 44_100, "stereo"), ("Mostly Two", Enc::Flac, 44_100, "stereo"), ("Mostly Three", Enc::Flac, 44_100, "stereo"), ("Mostly Four", Enc::Flac, 44_100, "stereo"), ("Odd One", Enc::Mp3, 44_100, "stereo")],
        false,
    );
    add("Channels", "Channels", &[("Mono", Enc::Flac, 44_100, "mono"), ("Surround", Enc::Flac, 48_000, "5.1")], false);
    add("PCM Album", "PCM Album", &[("Wave", Enc::Wav, 44_100, "stereo"), ("Interchange", Enc::Aiff, 44_100, "stereo")], false);
    add("ミックス盤", "ミックス盤", &[("夜の街", Enc::Flac, 44_100, "stereo"), ("朝の光", Enc::Mp3, 44_100, "stereo")], false);
    for tr in &out {
        write(root, tones, tr);
    }
    println!("formats: {} tracks", out.len());
}

/// What the runner's mock LRCLIB answers for each title is in `tools/bench-runner`.
pub const WORDS: [&str; 12] = [
    "Night Drive",
    "Paper Lantern",
    "Interlude",
    "Lost Signal",
    "Static",
    "Dropout",
    "With Its Own",
    "Skip One",
    "Skip Two",
    "Skip Three",
    "Skip Four",
    "Quiet Off",
];

fn words(root: &Path, tones: &Tones) {
    use ItemKey as K;
    for (i, title) in WORDS.iter().enumerate() {
        let n = (i + 1).to_string();
        let tr = t(
            format!("Words/{:02} {title}.flac", i + 1),
            Enc::Flac,
            8,
            &[(K::TrackArtist, "Fixture Voices"), (K::AlbumArtist, "Fixture Voices"), (K::AlbumTitle, "Words"), (K::TrackTitle, title), (K::TrackNumber, &n)],
        );
        write(root, tones, &tr);
    }
    std::fs::write(root.join("Words/07 With Its Own.lrc"), lrc(8, 1.0, "Local")).unwrap();
}

/// Run a sibling fixture tool (built with this one) into `dir`.
fn sibling(name: &str, args: &[&std::ffi::OsStr]) {
    let me = std::env::current_exe().unwrap();
    let exe = me.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.exists(), "{} is not built: cargo build --release -p library --examples", exe.display());
    let ok = Command::new(&exe).args(args).status().is_ok_and(|s| s.success());
    assert!(ok, "{name} failed");
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("usage: bench_fixtures <folder in the temporary folder>"));
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    let parent = root.ancestors().skip(1).find(|p| p.exists()).and_then(|p| std::fs::canonicalize(p).ok());
    assert!(parent.is_some_and(|p| p.starts_with(&temp)), "{} is not in the temporary folder ({}): refused", root.display(), temp.display());
    if root.exists() {
        assert!(root.join(MARKER).exists(), "{} exists and was not made by this tool: refused", root.display());
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(MARKER), "Fixtures made by bench_fixtures; deleted and made again by it.\n").unwrap();
    let t0 = std::time::Instant::now();
    let tones = Tones { dir: root.join("tones") };
    std::fs::create_dir_all(&tones.dir).unwrap();
    let dir = |n: &str| {
        let d = root.join(n);
        std::fs::create_dir_all(&d).unwrap();
        d
    };
    core(&dir("core"), &tones);
    formats(&dir("formats"), &tones);
    words(&dir("words"), &tones);
    sibling("viz_fixtures", &[root.join("viz").as_os_str()]);
    sibling("messy_library", &[root.join("messy").as_os_str()]);
    // The smart playlist bench's seeded favourites and plays are made for 120 albums.
    sibling("messy_library", &[root.join("messy120").as_os_str(), "120".as_ref()]);
    sibling("meta_fixtures", &[root.join("meta").as_os_str()]);
    sibling("edit_fixtures", &[root.join("edit").as_os_str()]);
    let _ = std::fs::remove_dir_all(&tones.dir);
    println!("fixtures in {} ({:.0} s)", root.display(), t0.elapsed().as_secs_f64());
}
