//! A fixture library the way friends' libraries look: a few thousand short tracks in
//! mixed formats, most tagged properly, many not (no album artist, compilations without
//! one, untagged "01 - Title.mp3" files, case and whitespace variants, disc numbers on
//! some tracks only, "1/12", "01" and "A1" numbers, years beside full dates, "CD1"/"CD2"
//! folders, two spellings of one title, loose singles, missing titles, art per track,
//! featured artists in titles and artist fields, multi-valued artists and genres in every
//! format's own way, 5.1 AAC). `-- <empty folder> [albums]` (default 240 regular albums).
//! Writes only inside the folder it is given, and refuses a folder that holds anything.
//!
//!     cargo run --release -p library --example messy_library -- %TEMP%\mp-messy\music
//!
//! Needs ffmpeg on PATH (the audio templates).
use std::path::{Path, PathBuf};
use std::process::Command;

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};

#[derive(Clone, Copy, PartialEq)]
enum Fmt {
    Flac,
    Mp3v24,
    Mp3v23,
    Opus,
    Vorbis,
    M4a,
    M4a51,
    /// Untagged WAV.
    Wav,
}

impl Fmt {
    fn ext(self) -> &'static str {
        match self {
            Fmt::Flac => "flac",
            Fmt::Mp3v24 | Fmt::Mp3v23 => "mp3",
            Fmt::Opus => "opus",
            Fmt::Vorbis => "ogg",
            Fmt::M4a | Fmt::M4a51 => "m4a",
            Fmt::Wav => "wav",
        }
    }
    fn tag_type(self) -> TagType {
        match self {
            Fmt::Flac | Fmt::Opus | Fmt::Vorbis => TagType::VorbisComments,
            Fmt::Mp3v24 | Fmt::Mp3v23 => TagType::Id3v2,
            Fmt::M4a | Fmt::M4a51 => TagType::Mp4Ilst,
            Fmt::Wav => TagType::Id3v2,
        }
    }
}

struct Track {
    rel: String,
    fmt: Fmt,
    /// Pushed in order; a key given twice is a multi-valued item.
    items: Vec<(ItemKey, String)>,
    art: Option<Vec<u8>>,
    /// No tags at all (the file as ripped).
    untagged: bool,
    secs: usize,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.next(100) < pct
    }
}

const FIRST: &[&str] = &[
    "Velvet", "Paper", "Glass", "Northern", "Quiet", "Silver", "Hollow", "Neon", "Wild", "Electric", "Static", "Golden", "Lunar", "Copper",
    "Midnight", "Crystal", "Distant", "Broken", "Little", "Atomic", "Saint", "Echo", "Ivory", "Scarlet", "Arctic", "Desert", "Solar", "Rust",
];
const SECOND: &[&str] = &[
    "Harbour", "Orchids", "Engines", "Lanterns", "Wolves", "Rivers", "Parade", "Machines", "Gardens", "Pilots", "Comets", "Satellites",
    "Ghosts", "Tides", "Choir", "Signals", "Foxes", "Monks", "Mirrors", "Owls", "Hearts", "Sparrows", "Strangers", "Waves",
];
const PEOPLE: &[&str] = &[
    "Mara Lind", "Theo Vance", "Ines Okafor", "Kenji Arai", "Lucía Ferrer", "Oskar Brandt", "Noor Haddad", "Ada Kowalski", "Ruth Ellery",
    "Sami Virtanen", "Beyoncé", "Sigur Rós", "かめりあ", "Ólafur Arnalds", "Dvořák", "Zoë Marchetti", "Björn Halvorsen", "Céline Moreau",
];
const WORDS: &[&str] = &[
    "Night", "Light", "Ghost", "Blue", "Storm", "Café", "Wave", "Ocean", "Dream", "Heart", "Fire", "Ice", "City", "Star", "Rain", "Summer",
    "Winter", "Gold", "Echo", "Signal", "Mirror", "River", "Garden", "Satellite", "Lantern", "Paper", "Glass", "Static", "Comet", "Harbour",
];
const GENRES: &[&str] = &["Rock", "Pop", "Electronic", "Jazz", "Hip-Hop", "Folk", "Ambient", "Metal", "Soul", "Classical", "Indie", "Drum & Bass"];
const LABELS: &[&str] = &["Northern Lights Records", "Paper Moon", "Static Sound", "Harbour Music", "Glasshouse", "Lantern Audio", "[no label]"];

fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).status().is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg {args:?}");
}

/// A short tone in each format and length; the tracks are copies of these.
fn templates(dir: &Path) -> impl Fn(Fmt, usize) -> PathBuf + use<> {
    std::fs::create_dir_all(dir).unwrap();
    for secs in 1..=3 {
        for (fmt, codec, layout) in [
            (Fmt::Flac, vec!["-c:a", "flac"], "stereo"),
            (Fmt::Mp3v24, vec!["-c:a", "libmp3lame", "-b:a", "128k"], "stereo"),
            (Fmt::Opus, vec!["-c:a", "libopus", "-b:a", "64k"], "stereo"),
            (Fmt::Vorbis, vec!["-c:a", "libvorbis", "-q:a", "2"], "stereo"),
            (Fmt::M4a, vec!["-c:a", "aac", "-b:a", "96k"], "stereo"),
            (Fmt::M4a51, vec!["-c:a", "aac", "-b:a", "256k"], "5.1"),
            (Fmt::Wav, vec!["-c:a", "pcm_s16le"], "stereo"),
        ] {
            let out = dir.join(format!("t{secs}-{}.{}", layout.replace('.', ""), fmt.ext()));
            if out.exists() {
                continue;
            }
            let src = format!("sine=frequency={}:sample_rate=44100:duration={secs}", 220 + secs * 110);
            let o = out.to_string_lossy().into_owned();
            let mut a = vec!["-f", "lavfi", "-i", &src, "-af", if layout == "5.1" { "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0" } else { "pan=stereo|c0=c0|c1=c0" }, "-map_metadata", "-1"];
            a.extend(codec);
            a.push(&o);
            ffmpeg(&a);
        }
    }
    let dir = dir.to_path_buf();
    move |fmt, secs| {
        let layout = if fmt == Fmt::M4a51 { "51" } else { "stereo" };
        let ext = match fmt {
            Fmt::Mp3v23 => "mp3",
            f => f.ext(),
        };
        dir.join(format!("t{}-{layout}.{ext}", secs.clamp(1, 3)))
    }
}

/// A cover: a gradient with a few bands, different for each seed.
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

struct Lib {
    tracks: Vec<Track>,
    /// (folder, cover seed) for cover.jpg files.
    covers: Vec<(String, usize)>,
}

impl Lib {
    fn add(&mut self, rel: String, fmt: Fmt, items: Vec<(ItemKey, String)>) -> &mut Track {
        let secs = 1 + self.tracks.len() % 3;
        self.tracks.push(Track { rel, fmt, items, art: None, untagged: false, secs });
        self.tracks.last_mut().unwrap()
    }
}

fn kv(items: &[(ItemKey, &str)]) -> Vec<(ItemKey, String)> {
    items.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
}

/// The regular part: albums by bands and people, most tagged in full, some without an
/// album artist, a few with MusicBrainz ids, multi-valued artists and genres.
fn regular(lib: &mut Lib, rng: &mut Rng, albums: usize) {
    let fmts = [Fmt::Flac, Fmt::Flac, Fmt::Mp3v24, Fmt::Mp3v23, Fmt::Opus, Fmt::M4a, Fmt::Vorbis];
    let mut artists: Vec<String> = Vec::new();
    for i in 0..60 {
        artists.push(if i % 4 == 3 { PEOPLE[i % PEOPLE.len()].to_owned() } else { format!("{} {}", FIRST[i % FIRST.len()], SECOND[(i * 7) % SECOND.len()]) });
    }
    for a in 0..albums {
        let artist = artists[rng.next(artists.len())].clone();
        let title = format!("{} {}", WORDS[rng.next(WORDS.len())], WORDS[rng.next(WORDS.len())]);
        let fmt = fmts[rng.next(fmts.len())];
        let n = 6 + rng.next(10);
        let year = 1968 + rng.next(57);
        let full_date = rng.chance(40);
        let genre = GENRES[rng.next(GENRES.len())];
        let second_genre = rng.chance(15).then(|| GENRES[rng.next(GENRES.len())]).filter(|g| *g != genre);
        let label = LABELS[rng.next(LABELS.len())];
        let no_album_artist = rng.chance(30);
        let mbid = rng.chance(12).then(|| format!("{:08x}-0000-4000-8000-{:012x}", a * 7919, a));
        let discs = if rng.chance(6) { 2 } else { 1 };
        let folder = format!("{}/{} ({year})", artist.replace(['/', ':', '?'], "_"), title);
        let embedded = rng.chance(25);
        if !embedded {
            lib.covers.push((folder.clone(), a));
        }
        for d in 1..=discs {
            for t in 1..=n {
                let mut items: Vec<(ItemKey, String)> = Vec::new();
                let featured = rng.chance(10).then(|| PEOPLE[rng.next(PEOPLE.len())]);
                let track_artist = match featured {
                    Some(f) => format!("{artist} feat. {f}"),
                    None => artist.clone(),
                };
                items.push((ItemKey::TrackArtist, track_artist.clone()));
                if let Some(f) = featured
                    && mbid.is_some()
                {
                    // Picard: ARTIST is the credit as written, ARTISTS each artist.
                    items.push((ItemKey::TrackArtists, artist.clone()));
                    items.push((ItemKey::TrackArtists, f.to_owned()));
                }
                if !no_album_artist {
                    items.push((ItemKey::AlbumArtist, artist.clone()));
                }
                items.push((ItemKey::AlbumTitle, title.clone()));
                items.push((ItemKey::TrackTitle, format!("{} {} {}", WORDS[rng.next(WORDS.len())], WORDS[(a + t) % WORDS.len()], t)));
                items.push((ItemKey::TrackNumber, if rng.chance(50) { format!("{t}/{n}") } else { format!("{t:02}") }));
                if discs > 1 {
                    items.push((ItemKey::DiscNumber, format!("{d}/{discs}")));
                }
                items.push((ItemKey::RecordingDate, if full_date { format!("{year}-{:02}-{:02}", 1 + a % 12, 1 + a % 28) } else { year.to_string() }));
                items.push((ItemKey::Genre, genre.to_owned()));
                if let Some(g) = second_genre {
                    items.push((ItemKey::Genre, g.to_owned()));
                }
                items.push((ItemKey::Label, label.to_owned()));
                if let Some(mb) = &mbid {
                    items.push((ItemKey::MusicBrainzReleaseId, mb.clone()));
                    items.push((ItemKey::MusicBrainzReleaseArtistId, format!("aaaa{:04x}-0000-4000-8000-000000000000", artists.iter().position(|x| *x == artist).unwrap())));
                }
                let disc_dir = if discs > 1 { format!("/CD{d}") } else { String::new() };
                let tr = lib.add(format!("{folder}{disc_dir}/{t:02} - Track {t}.{}", fmt.ext()), fmt, items);
                if embedded {
                    tr.art = Some(cover(a, 300));
                }
            }
        }
    }
}

/// The cases the scanner has to make sense of. Folders under "Messy/".
fn messy(lib: &mut Lib) {
    use ItemKey as K;
    // No album artist; two tracks credit a featured singer in the artist field.
    for t in 1..=10 {
        let artist = if t == 4 || t == 7 { "Harbour Lights feat. Mara Lind" } else { "Harbour Lights" };
        lib.add(
            format!("Messy/Harbour Lights - Low Tide/{t:02} Low Tide {t}.flac"),
            Fmt::Flac,
            kv(&[(K::TrackArtist, artist), (K::AlbumTitle, "Low Tide"), (K::TrackTitle, &format!("Low Tide {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "2011"), (K::Genre, "Indie")]),
        );
    }
    lib.covers.push(("Messy/Harbour Lights - Low Tide".into(), 900));
    // A compilation without an album artist or a flag: every track its own artist.
    let comp = ["Mara Lind", "Theo Vance", "Ines Okafor", "Kenji Arai", "Lucía Ferrer", "Oskar Brandt", "Noor Haddad", "Ada Kowalski", "Ruth Ellery", "Sami Virtanen", "Glass Orchids", "Neon Pilots"];
    for (i, a) in comp.iter().enumerate() {
        let t = i + 1;
        lib.add(
            format!("Messy/Compilations/Summer Hits 2003/{t:02}. {a} - Hit {t}.mp3"),
            Fmt::Mp3v23,
            kv(&[(K::TrackArtist, a), (K::AlbumTitle, "Summer Hits 2003"), (K::TrackTitle, &format!("Summer Hit {t}")), (K::TrackNumber, &format!("{t}/12")), (K::RecordingDate, "2003"), (K::Genre, "Pop")]),
        );
    }
    lib.covers.push(("Messy/Compilations/Summer Hits 2003".into(), 901));
    // A compilation flagged as one (iTunes' "Part of a compilation"), no album artist.
    for (i, a) in comp.iter().take(6).enumerate() {
        let t = i + 1;
        lib.add(
            format!("Messy/Compilations/Night Drive/{t:02} {a}.m4a"),
            Fmt::M4a,
            kv(&[(K::TrackArtist, a), (K::AlbumTitle, "Night Drive"), (K::TrackTitle, &format!("Drive {t}")), (K::TrackNumber, &t.to_string()), (K::FlagCompilation, "1"), (K::RecordingDate, "2019-06-21"), (K::Genre, "Electronic")]),
        );
    }
    lib.covers.push(("Messy/Compilations/Night Drive".into(), 902));
    // Two different albums sharing a title in one folder: their numbers collide, so they
    // are two albums, not a compilation.
    for (artist, seed) in [("Copper Monks", 903), ("Lunar Sparrows", 904)] {
        for t in 1..=5 {
            lib.add(
                format!("Messy/Greatest Hits/{artist} - {t:02}.flac"),
                Fmt::Flac,
                kv(&[(K::TrackArtist, artist), (K::AlbumTitle, "Greatest Hits"), (K::TrackTitle, &format!("{artist} hit {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "1999")]),
            );
        }
        let _ = seed;
    }
    // Ripped without tags: the file names are all there is.
    for (name, fmt) in [("01 - First Light", Fmt::Wav), ("02. Second Wind", Fmt::Wav), ("1-03 Third Rail", Fmt::Wav), ("04_Fourth Wall", Fmt::Wav), ("Untitled", Fmt::Wav), ("99 Balloons", Fmt::Wav)] {
        let t = lib.add(format!("Messy/Untagged Rip/{name}.{}", fmt.ext()), fmt, Vec::new());
        t.untagged = true;
    }
    // Case, whitespace and Unicode normalisation: one artist, one genre.
    for (i, (artist, genre)) in [("The Paper Harbour", "rock"), ("the paper harbour", "Rock"), (" The  Paper Harbour ", "ROCK"), ("The Paper Harbour", "Rock")].iter().enumerate() {
        for t in 1..=3 {
            lib.add(
                format!("Messy/Paper Harbour/Album {}/{t:02}.opus", i + 1),
                Fmt::Opus,
                kv(&[(K::TrackArtist, artist), (K::AlbumArtist, artist), (K::AlbumTitle, &format!("Paper Album {}", i + 1)), (K::TrackTitle, &format!("Paper {t}")), (K::TrackNumber, &t.to_string()), (K::Genre, genre), (K::RecordingDate, &(2001 + i).to_string())]),
            );
        }
        lib.covers.push((format!("Messy/Paper Harbour/Album {}", i + 1), 910 + i));
    }
    for (i, artist) in ["Zoë Marchetti", "Zoe\u{0308} Marchetti"].iter().enumerate() {
        lib.add(
            format!("Messy/Zoe/Single {}/01.flac", i + 1),
            Fmt::Flac,
            kv(&[(K::TrackArtist, artist), (K::AlbumArtist, artist), (K::AlbumTitle, &format!("Zoë Single {}", i + 1)), (K::TrackTitle, "Zoë"), (K::TrackNumber, "1")]),
        );
    }
    // Disc numbers on some tracks only.
    for t in 1..=8 {
        let mut items = kv(&[(K::TrackArtist, "Silver Comets"), (K::AlbumArtist, "Silver Comets"), (K::AlbumTitle, "Half Numbered"), (K::TrackTitle, &format!("Half {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "2015")]);
        if t > 4 {
            items.push((K::DiscNumber, "1".into()));
        }
        lib.add(format!("Messy/Silver Comets/Half Numbered/{t:02}.flac"), Fmt::Flac, items);
    }
    lib.covers.push(("Messy/Silver Comets/Half Numbered".into(), 920));
    for (d, t) in (1..=2).flat_map(|d| (1..=4).map(move |t| (d, t))) {
        let mut items = kv(&[(K::TrackArtist, "Silver Comets"), (K::AlbumArtist, "Silver Comets"), (K::AlbumTitle, "Two Sides"), (K::TrackTitle, &format!("Side {d} song {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "2017")]);
        if d == 2 {
            items.push((K::DiscNumber, "2".into()));
        }
        lib.add(format!("Messy/Silver Comets/Two Sides/{d}{t:02}.flac"), Fmt::Flac, items);
    }
    lib.covers.push(("Messy/Silver Comets/Two Sides".into(), 921));
    // Vinyl positions as track numbers.
    for (i, pos) in ["A1", "A2", "A3", "B1", "B2", "B3"].iter().enumerate() {
        lib.add(
            format!("Messy/Vinyl/{pos} - Groove {}.flac", i + 1),
            Fmt::Flac,
            kv(&[(K::TrackArtist, "Desert Owls"), (K::AlbumArtist, "Desert Owls"), (K::AlbumTitle, "Groove Sessions"), (K::TrackTitle, &format!("Groove {}", i + 1)), (K::TrackNumber, pos), (K::RecordingDate, "1977")]),
        );
    }
    lib.covers.push(("Messy/Vinyl".into(), 922));
    // A year on some tracks, the full date on others; one track's genre differs.
    for t in 1..=10 {
        let date = if t <= 6 { "2004" } else { "2004-03-15" };
        let genre = if t == 3 { "Electronic" } else { "Pop" };
        lib.add(
            format!("Messy/Golden Engines/Dates/{t:02}.mp3"),
            Fmt::Mp3v24,
            kv(&[(K::TrackArtist, "Golden Engines"), (K::AlbumArtist, "Golden Engines"), (K::AlbumTitle, "Dates"), (K::TrackTitle, &format!("Date {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, date), (K::Genre, genre), (K::Label, if t == 9 { "Static Sound" } else { "Paper Moon" })]),
        );
    }
    lib.covers.push(("Messy/Golden Engines/Dates".into(), 923));
    // One album in "CD1"/"CD2" subfolders, another as sibling folders.
    for d in 1..=2 {
        for t in 1..=4 {
            lib.add(
                format!("Messy/Atomic Tides/Double/Disc {d}/{t:02}.flac"),
                Fmt::Flac,
                kv(&[(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Double"), (K::TrackTitle, &format!("Double {d}-{t}")), (K::TrackNumber, &t.to_string()), (K::DiscNumber, &d.to_string()), (K::RecordingDate, "2009")]),
            );
            lib.add(
                format!("Messy/Atomic Tides/Live CD{d}/{t:02}.flac"),
                Fmt::Flac,
                kv(&[(K::TrackArtist, "Atomic Tides"), (K::AlbumArtist, "Atomic Tides"), (K::AlbumTitle, "Live"), (K::TrackTitle, &format!("Live {d}-{t}")), (K::TrackNumber, &t.to_string()), (K::DiscNumber, &d.to_string()), (K::RecordingDate, "2010")]),
            );
        }
        lib.covers.push((format!("Messy/Atomic Tides/Live CD{d}"), 924));
    }
    lib.covers.push(("Messy/Atomic Tides/Double".into(), 925));
    // Two spellings of one title in one folder.
    for t in 1..=9 {
        let title = match t {
            8 => "patchwork ",
            9 => "Patchwork (Deluxe)",
            _ => "Patchwork",
        };
        lib.add(
            format!("Messy/Scarlet Foxes/Patchwork/{t:02}.ogg"),
            Fmt::Vorbis,
            kv(&[(K::TrackArtist, "Scarlet Foxes"), (K::AlbumArtist, "Scarlet Foxes"), (K::AlbumTitle, title), (K::TrackTitle, &format!("Patch {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "2013")]),
        );
    }
    lib.covers.push(("Messy/Scarlet Foxes/Patchwork".into(), 926));
    // A folder of loose singles: some with an album tag, some without.
    for (i, a) in ["Mara Lind", "Theo Vance", "Kenji Arai", "Noor Haddad", "Ruth Ellery"].iter().enumerate() {
        let mut items = kv(&[(K::TrackArtist, a), (K::TrackTitle, &format!("Single by {a}")), (K::RecordingDate, &(2018 + i).to_string())]);
        if i < 3 {
            items.push((K::AlbumTitle, format!("{a} - Single")));
        }
        lib.add(format!("Messy/Singles/{a} - Single.mp3"), Fmt::Mp3v24, items);
    }
    // Tracks without a title tag.
    for t in 1..=4 {
        lib.add(
            format!("Messy/Ivory Choir/Hymns/{t:02} - Hymn number {t}.flac"),
            Fmt::Flac,
            kv(&[(K::TrackArtist, "Ivory Choir"), (K::AlbumArtist, "Ivory Choir"), (K::AlbumTitle, "Hymns"), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "1998")]),
        );
    }
    // Embedded art that differs per track (the album takes its first track's).
    for t in 1..=4 {
        let tr = lib.add(
            format!("Messy/Neon Wolves/Every Cover/{t:02}.m4a"),
            Fmt::M4a,
            kv(&[(K::TrackArtist, "Neon Wolves"), (K::AlbumArtist, "Neon Wolves"), (K::AlbumTitle, "Every Cover"), (K::TrackTitle, &format!("Cover {t}")), (K::TrackNumber, &t.to_string()), (K::RecordingDate, "2021")]),
        );
        tr.art = Some(cover(930 + t, 300));
    }
    // Featured artists in the title.
    for (t, title) in ["Daybreak (feat. Ines Okafor)", "Evening [ft. Theo Vance]", "Noon", "Dusk feat. Kenji Arai"].iter().enumerate() {
        lib.add(
            format!("Messy/Saint Pilots/Hours/{:02}.flac", t + 1),
            Fmt::Flac,
            kv(&[(K::TrackArtist, "Saint Pilots"), (K::AlbumArtist, "Saint Pilots"), (K::AlbumTitle, "Hours"), (K::TrackTitle, title), (K::TrackNumber, &(t + 1).to_string()), (K::RecordingDate, "2020")]),
        );
    }
    lib.covers.push(("Messy/Saint Pilots/Hours".into(), 940));
    // Multi-valued artists and genres, in each format's own way.
    let multi = [
        // Vorbis: the key repeated.
        (Fmt::Flac, "flac", vec![(K::TrackArtist, "Mara Lind"), (K::TrackArtist, "Theo Vance"), (K::Genre, "Folk"), (K::Genre, "Ambient"), (K::Genre, "Jazz")]),
        // ID3v2.4: values separated by NUL (lofty writes repeated items so).
        (Fmt::Mp3v24, "mp3", vec![(K::TrackArtist, "Mara Lind"), (K::TrackArtist, "Kenji Arai"), (K::Genre, "Folk"), (K::Genre, "Soul")]),
        // ID3v2.3: one string, joined by the conventional " / ".
        (Fmt::Mp3v23, "mp3", vec![(K::TrackArtist, "Mara Lind / Noor Haddad"), (K::Genre, "Folk; Pop")]),
        // MP4: several values in one atom.
        (Fmt::M4a, "m4a", vec![(K::TrackArtist, "Mara Lind"), (K::TrackArtist, "Ruth Ellery"), (K::Genre, "Folk"), (K::Genre, "Indie")]),
        // Picard: ARTIST the credit as written, ARTISTS each artist, with ids.
        (Fmt::Opus, "opus", vec![(K::TrackArtist, "Mara Lind & Sami Virtanen"), (K::TrackArtists, "Mara Lind"), (K::TrackArtists, "Sami Virtanen"), (K::MusicBrainzArtistId, "11111111-0000-4000-8000-000000000001"), (K::MusicBrainzArtistId, "11111111-0000-4000-8000-000000000002"), (K::Genre, "Folk")]),
    ];
    for (t, (fmt, ext, items)) in multi.into_iter().enumerate() {
        let mut all = kv(&[(K::AlbumArtist, "Mara Lind"), (K::AlbumTitle, "Duets"), (K::TrackTitle, &format!("Duet {}", t + 1)), (K::TrackNumber, &(t + 1).to_string()), (K::RecordingDate, "2016")]);
        all.extend(items.into_iter().map(|(k, v)| (k, v.to_owned())));
        lib.add(format!("Messy/Mara Lind/Duets/{:02} Duet.{ext}", t + 1), fmt, all);
    }
    lib.covers.push(("Messy/Mara Lind/Duets".into(), 941));
    // Earth, Wind & Fire stays one artist; AC/DC too.
    for (t, a) in ["Earth, Wind & Fire", "AC/DC"].iter().enumerate() {
        lib.add(
            format!("Messy/Whole Names/{:02}.flac", t + 1),
            Fmt::Flac,
            kv(&[(K::TrackArtist, a), (K::AlbumArtist, "Whole Names"), (K::AlbumTitle, "Whole Names"), (K::TrackTitle, &format!("Whole {}", t + 1)), (K::TrackNumber, &(t + 1).to_string())]),
        );
    }
    // 5.1 AAC, which the decoder cannot play.
    lib.add(
        "Messy/Surround/01 Surround.m4a".into(),
        Fmt::M4a51,
        kv(&[(K::TrackArtist, "Arctic Satellites"), (K::AlbumArtist, "Arctic Satellites"), (K::AlbumTitle, "Surround"), (K::TrackTitle, "Six Channels"), (K::TrackNumber, "1"), (K::RecordingDate, "2022")]),
    );
}

fn write(root: &Path, tpl: &dyn Fn(Fmt, usize) -> PathBuf, t: &Track) {
    let path = root.join(&t.rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(tpl(t.fmt, t.secs), &path).unwrap();
    if t.untagged {
        return;
    }
    let mut tag = Tag::new(t.fmt.tag_type());
    for (k, v) in &t.items {
        // "3/12": lofty keeps a number and its total as two items (and drops "3/12" as one).
        if let (ItemKey::TrackNumber | ItemKey::DiscNumber, Some((n, total))) = (k, v.split_once('/')) {
            let total_key = if *k == ItemKey::TrackNumber { ItemKey::TrackTotal } else { ItemKey::DiscTotal };
            tag.push(TagItem::new(*k, ItemValue::Text(n.to_owned())));
            tag.push(TagItem::new(total_key, ItemValue::Text(total.to_owned())));
            continue;
        }
        tag.push(TagItem::new(*k, ItemValue::Text(v.clone())));
    }
    if let Some(data) = &t.art {
        tag.push_picture(Picture::unchecked(data.clone()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
    }
    let opts = WriteOptions::default().use_id3v23(t.fmt == Fmt::Mp3v23);
    if let Err(e) = tag.save_to_path(&path, opts) {
        panic!("{}: {e}", path.display());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("an empty folder to write into"));
    let albums: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(240);
    if root.exists() && std::fs::read_dir(&root).map(|mut d| d.next().is_some()).unwrap_or(true) {
        panic!("{} is not empty; give an empty or new folder", root.display());
    }
    let n = make(&root, albums);
    println!("{}: {n} tracks", root.display());
}

/// Write the library into `root` (an empty or new folder): `albums` regular albums and the
/// messy part, its audio templates in a folder beside it. Returns how many tracks. Also used by
/// `tests/progressive.rs`, which includes this file.
pub fn make(root: &Path, albums: usize) -> usize {
    let tpl = templates(&root.with_file_name(format!("{}-templates", root.file_name().unwrap().to_string_lossy())));
    let mut lib = Lib { tracks: Vec::new(), covers: Vec::new() };
    let mut rng = Rng(0x5eed_cafe);
    regular(&mut lib, &mut rng, albums);
    messy(&mut lib);
    for t in &lib.tracks {
        write(root, &tpl, t);
    }
    for (folder, seed) in &lib.covers {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cover.jpg"), cover(*seed, 500)).unwrap();
    }
    lib.tracks.len()
}
