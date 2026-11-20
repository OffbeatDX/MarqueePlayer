//! The metadata viewer's fixtures (see `examples/meta_fixtures.rs`), shared with
//! `tests/meta.rs`. Writes only inside the folder it is given.
#![allow(dead_code)]

use std::borrow::Cow;
use std::path::Path;
use std::process::Command;

use lofty::config::WriteOptions;
use lofty::id3::v2::{AttachedPictureFrame, CommentFrame, ExtendedTextFrame, Frame, FrameId, Id3v2Tag, TextInformationFrame, UniqueFileIdentifierFrame, UnsynchronizedTextFrame};
use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::TagExt;
use lofty::TextEncoding;

pub fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).status().is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg {args:?}");
}

/// A few seconds of a tone with some noise, encoded with `codec` args.
fn audio(out: &Path, secs: u32, codec: &[&str]) {
    let d = secs.to_string();
    let tone = format!("sine=frequency=330:sample_rate=44100:duration={d}");
    let noise = format!("anoisesrc=color=pink:sample_rate=44100:amplitude=0.05:duration={d}");
    let mut a = vec!["-f", "lavfi", "-i", &tone, "-f", "lavfi", "-i", &noise, "-filter_complex", "[0][1]amix=inputs=2:duration=shortest", "-map_metadata", "-1", "-fflags", "+bitexact"];
    a.extend(codec);
    let o = out.to_str().unwrap().to_owned();
    a.push(&o);
    ffmpeg(&a);
}

fn picture(t: PictureType, mime: MimeType, data: Vec<u8>, desc: &str) -> Picture {
    let b = Picture::unchecked(data).pic_type(t).mime_type(mime);
    if desc.is_empty() { b.build() } else { b.description(desc.to_owned()).build() }
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8 ^ seed, (y * 255 / h) as u8, 160u8.wrapping_add(seed)]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn jpeg(w: u32, h: u32, noisy: bool) -> Vec<u8> {
    let mut s: u32 = 12345;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if noisy {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgb([(s >> 16) as u8, (s >> 8) as u8, (x ^ y) as u8])
        } else {
            image::Rgb([(x / 4) as u8, 90, (y / 3) as u8])
        }
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

fn vorbis(path: &Path, items: &[(&str, &str)], pictures: Vec<Picture>) {
    let mut vc = VorbisComments::new();
    vc.set_vendor("Marquee fixtures (lofty)".into());
    for (k, v) in items {
        vc.push((*k).to_owned(), (*v).to_owned());
    }
    for p in pictures {
        vc.insert_picture(p, None).unwrap();
    }
    if let Err(e) = vc.save_to_path(path, WriteOptions::default()) {
        panic!("{}: {e}", path.display());
    }
}

fn text(id: &'static str, value: &str) -> Frame<'static> {
    Frame::Text(TextInformationFrame::new(FrameId::Valid(Cow::Borrowed(id)), TextEncoding::UTF8, value.to_owned()))
}

fn txxx(desc: &str, value: &str) -> Frame<'static> {
    Frame::UserText(ExtendedTextFrame::new(TextEncoding::UTF8, desc.to_owned(), value.to_owned()))
}

const MBID: &str = "6f2e3c1a-8b7d-4c55-9a1e-0d3b2f4a5c6e";
const LRC: &str = "[ar:Aurora Vale]\n[00:00.50]Northern lights above the bay\n[00:02.10]We count the seconds into day\n[00:03.40]And every light goes out";

/// Make every fixture under `dir` (emptied first when this made it; refused otherwise).
pub fn make(dir: &Path) {
    if dir.exists() {
        assert!(dir.join(".meta-fixtures").exists(), "{} exists and was not made by this tool", dir.display());
        std::fs::remove_dir_all(dir).unwrap();
    }
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(".meta-fixtures"), b"made by meta_fixtures").unwrap();
    let flac: &[&str] = &["-c:a", "flac", "-sample_fmt", "s16"];
    let mp3: &[&str] = &["-c:a", "libmp3lame", "-b:a", "192k", "-id3v2_version", "0", "-write_id3v1", "0"];

    // ---- Picard Set ------------------------------------------------------------------
    let a = dir.join("Picard Set");
    std::fs::create_dir_all(&a).unwrap();
    let fingerprint: String = (0..1400).map(|i| (b'A' + (i * 7 % 26) as u8) as char).collect();
    for (n, title, lyrics) in [(1, "Northern Lights", true), (2, "Second Wind", false)] {
        let p = a.join(format!("0{n} {title}.flac"));
        audio(&p, 4, flac);
        let no = n.to_string();
        let mut items: Vec<(&str, &str)> = vec![
            ("TITLE", title),
            ("ARTIST", "Aurora Vale"),
            ("ARTIST", "Mika Rhee"),
            ("ARTISTS", "Aurora Vale"),
            ("ARTISTS", "Mika Rhee"),
            ("ARTISTSORT", "Vale, Aurora; Rhee, Mika"),
            ("ALBUM", "Picard Set"),
            ("ALBUMARTIST", "Aurora Vale"),
            ("ALBUMARTISTSORT", "Vale, Aurora"),
            ("TRACKNUMBER", &no),
            ("TRACKTOTAL", "2"),
            ("DISCNUMBER", "1"),
            ("DISCTOTAL", "1"),
            ("DATE", "2019.03.08"),
            ("ORIGINALDATE", "2018-11-02"),
            ("ORIGINALYEAR", "2018"),
            ("GENRE", "Synthpop"),
            ("GENRE", "Dream Pop"),
            ("COMPOSER", "Aurora Vale"),
            ("LYRICIST", "Mika Rhee"),
            ("LABEL", "Lantern Records"),
            ("CATALOGNUMBER", "LAN-042"),
            ("BARCODE", "0123456789012"),
            ("ASIN", "B07XYZ1234"),
            ("ISRC", "GBXYZ1900042"),
            ("RELEASETYPE", "album"),
            ("RELEASESTATUS", "official"),
            ("RELEASECOUNTRY", "XE"),
            ("MEDIA", "Digital Media"),
            ("SCRIPT", "Latn"),
            ("LANGUAGE", "eng"),
            ("MUSICBRAINZ_ALBUMID", MBID),
            ("MUSICBRAINZ_TRACKID", "a1b2c3d4-0000-4000-8000-000000000001"),
            ("MUSICBRAINZ_RELEASETRACKID", "a1b2c3d4-0000-4000-8000-000000000101"),
            ("MUSICBRAINZ_RELEASEGROUPID", "a1b2c3d4-0000-4000-8000-000000000201"),
            ("MUSICBRAINZ_ARTISTID", "a1b2c3d4-0000-4000-8000-000000000301"),
            ("MUSICBRAINZ_ARTISTID", "a1b2c3d4-0000-4000-8000-000000000302"),
            ("MUSICBRAINZ_ALBUMARTISTID", "a1b2c3d4-0000-4000-8000-000000000301"),
            ("ACOUSTID_ID", "0f9e8d7c-6b5a-4938-2716-0a1b2c3d4e5f"),
            ("ACOUSTID_FINGERPRINT", &fingerprint),
            ("REPLAYGAIN_TRACK_GAIN", "-6.20 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.988525"),
            ("REPLAYGAIN_ALBUM_GAIN", "-6.05 dB"),
            ("REPLAYGAIN_ALBUM_PEAK", "0.991211"),
            ("REPLAYGAIN_REFERENCE_LOUDNESS", "89.0 dB"),
            ("COMMENT", "Mastered for the long nights."),
            ("DISCOGS_RELEASE_ID", "13579246"),
            ("ENCODER", "reference libFLAC 1.4.3"),
        ];
        if lyrics {
            items.push(("LYRICS", LRC));
        }
        let mut pics = vec![picture(PictureType::CoverFront, MimeType::Png, png(1200, 1200, 0), "")];
        if n == 1 {
            pics.push(picture(PictureType::Artist, MimeType::Jpeg, jpeg(800, 600, false), "Aurora and Mika, live"));
        }
        vorbis(&p, &items, pics);
    }

    // ---- ID3 Formats -----------------------------------------------------------------
    let a = dir.join("ID3 Formats");
    std::fs::create_dir_all(&a).unwrap();
    let p = a.join("01 Twenty Four.mp3");
    audio(&p, 4, mp3);
    let mut t = Id3v2Tag::new();
    for f in [
        text("TIT2", "Twenty Four"),
        text("TPE1", "Aurora Vale\0Mika Rhee"),
        text("TALB", "ID3 Formats"),
        text("TPE2", "Format Lab"),
        text("TRCK", "1/2"),
        text("TPOS", "1/1"),
        text("TDRC", "2022-05-06"),
        text("TCON", "Synthwave\0Electronic"),
        text("TSRC", "GBXYZ2200001"),
        text("TSOP", "Vale, Aurora"),
        text("TPUB", "Lantern Records"),
        txxx("MusicBrainz Album Id", "11111111-2222-4333-8444-555555555555"),
        txxx("CATALOGNUMBER", "LAN-100"),
        txxx("REPLAYGAIN_TRACK_GAIN", "-4.10 dB"),
        txxx("REPLAYGAIN_TRACK_PEAK", "0.912"),
        txxx("Producer Notes", "Recorded in one take."),
        Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new("http://musicbrainz.org", b"b1b2b3b4-0000-4000-8000-000000000001".to_vec())),
        Frame::Comment(CommentFrame::new(TextEncoding::UTF8, *b"eng", String::new(), "An ID3v2.4 comment.".to_owned())),
        Frame::Comment(CommentFrame::new(TextEncoding::UTF8, *b"eng", "iTunNORM".to_owned(), " 00000A2B 00000A2B 00003C1D".to_owned())),
        Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(TextEncoding::UTF8, *b"eng", String::new(), "Plain lyrics\nwithout time stamps".to_owned())),
        Frame::Picture(AttachedPictureFrame::new(TextEncoding::UTF8, picture(PictureType::CoverFront, MimeType::Jpeg, jpeg(600, 600, false), "front"))),
    ] {
        t.insert(f);
    }
    t.save_to_path(&p, WriteOptions::default()).unwrap();
    let p = a.join("02 Twenty Three.mp3");
    audio(&p, 4, mp3);
    let mut t = Id3v2Tag::new();
    let long_title = "Twenty Three, With a Title Longer Than Thirty";
    for f in [
        text("TIT2", long_title),
        text("TPE1", "Aurora Vale"),
        text("TALB", "ID3 Formats"),
        text("TPE2", "Format Lab"),
        text("TRCK", "2/2"),
        text("TYER", "2022"),
        text("TCON", "Synthwave"),
        txxx("MusicBrainz Album Id", "11111111-2222-4333-8444-555555555555"),
    ] {
        t.insert(f);
    }
    t.save_to_path(&p, WriteOptions::default().use_id3v23(true)).unwrap();
    let mut v1 = lofty::id3::v1::Id3v1Tag::new();
    v1.title = Some(long_title.chars().take(30).collect());
    v1.artist = Some("Aurora Vale".into());
    v1.album = Some("ID3 Formats".into());
    v1.year = Some(2022);
    v1.track_number = Some(2);
    v1.genre = Some(17); // Rock: not what ID3v2 says
    v1.save_to_path(&p, WriteOptions::default()).unwrap();

    // ---- Other Formats ---------------------------------------------------------------
    let a = dir.join("Other Formats");
    std::fs::create_dir_all(&a).unwrap();
    let p = a.join("01 Opus R128.opus");
    audio(&p, 4, &["-c:a", "libopus", "-b:a", "96k"]);
    vorbis(
        &p,
        &[
            ("TITLE", "Opus R128"),
            ("ARTIST", "Format Lab"),
            ("ALBUM", "Other Formats"),
            ("ALBUMARTIST", "Format Lab"),
            ("TRACKNUMBER", "1"),
            ("DATE", "2023"),
            ("GENRE", "Ambient"),
            ("MUSICBRAINZ_ALBUMID", "22222222-3333-4444-8555-666666666666"),
            ("R128_TRACK_GAIN", "-1234"),
            ("R128_ALBUM_GAIN", "-1100"),
        ],
        Vec::new(),
    );
    let m4a = a.join("02 AAC Atoms.m4a");
    audio(&m4a, 4, &["-c:a", "aac", "-b:a", "192k"]);
    {
        use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
        let mut ilst = Ilst::new();
        let fourcc = |c: &[u8; 4], v: AtomData| Atom::new(AtomIdent::Fourcc(*c), v);
        let free = |name: &str, v: &str| Atom::new(AtomIdent::Freeform { mean: Cow::Borrowed("com.apple.iTunes"), name: Cow::Owned(name.to_owned()) }, AtomData::UTF8(v.to_owned()));
        for atom in [
            fourcc(b"\xa9nam", AtomData::UTF8("AAC Atoms".into())),
            fourcc(b"\xa9ART", AtomData::UTF8("Format Lab".into())),
            fourcc(b"\xa9alb", AtomData::UTF8("Other Formats".into())),
            fourcc(b"aART", AtomData::UTF8("Format Lab".into())),
            fourcc(b"\xa9day", AtomData::UTF8("2023-01-15".into())),
            fourcc(b"\xa9gen", AtomData::UTF8("Ambient".into())),
            fourcc(b"\xa9wrt", AtomData::UTF8("Mika Rhee".into())),
            fourcc(b"\xa9cmt", AtomData::UTF8("Tagged as iTunes would.".into())),
            fourcc(b"\xa9too", AtomData::UTF8("iTunes 12.13".into())),
            fourcc(b"trkn", AtomData::Unknown { code: lofty::mp4::DataType::Reserved, data: vec![0, 0, 0, 2, 0, 2, 0, 0] }),
            fourcc(b"disk", AtomData::Unknown { code: lofty::mp4::DataType::Reserved, data: vec![0, 0, 0, 1, 0, 1] }),
            fourcc(b"cpil", AtomData::Bool(false)),
            free("MusicBrainz Album Id", "22222222-3333-4444-8555-666666666666"),
            free("MusicBrainz Track Id", "c1c2c3c4-0000-4000-8000-000000000001"),
            free("replaygain_track_gain", "-3.30 dB"),
            free("iTunSMPB", " 00000000 00000840 000001C0 0000000000029E00"),
        ] {
            ilst.insert(atom);
        }
        ilst.insert_picture(picture(PictureType::CoverFront, MimeType::Jpeg, jpeg(500, 500, false), ""));
        ilst.save_to_path(&m4a, WriteOptions::default()).unwrap();
    }
    let a = dir.join("Untagged");
    std::fs::create_dir_all(&a).unwrap();
    audio(&a.join("No Tags.wav"), 3, &["-c:a", "pcm_s16le"]);

    // ---- Inconsistent Album ----------------------------------------------------------
    let a = dir.join("Inconsistent Album");
    std::fs::create_dir_all(&a).unwrap();
    let mb = "33333333-4444-4555-8666-777777777777";
    // (file, title, track, album, album artist, date, genre, ReplayGain)
    type Row<'a> = (&'a str, &'a str, Option<&'a str>, &'a str, Option<&'a str>, &'a str, &'a str, bool);
    let rows: [Row; 6] = [
        ("01 Patch One.flac", "Patch One", Some("1"), "Patchwork", Some("The Menders"), "2021", "Indie", true),
        ("02 Patch Two.flac", "Patch Two", Some("2"), "Patchwork", Some("The Menders"), "2021", "Indie", true),
        ("03 Patch Three.flac", "Patch Three", Some("3"), "Patchwork (Deluxe)", Some("The Menders"), "2021", "Indie", true),
        ("05 Patch Five.flac", "Patch Five", Some("5"), "Patchwork", Some("The Menders"), "2020", "Indie", true),
        ("06 Patch Six.flac", "Patch Six", Some("6"), "Patchwork", None, "2021", "Pop", false),
        ("Bonus.flac", "Bonus Patch", None, "Patchwork", Some("The Menders"), "2021", "Indie", true),
    ];
    for (file, title, no, album, aa, date, genre, rg) in rows {
        let p = a.join(file);
        audio(&p, 3, flac);
        let artist = if aa.is_some() { "The Menders" } else { "Guest Singer" };
        let mut items = vec![("TITLE", title), ("ARTIST", artist), ("ALBUM", album), ("DATE", date), ("GENRE", genre), ("MUSICBRAINZ_ALBUMID", mb)];
        if let Some(n) = no {
            items.push(("TRACKNUMBER", n));
        }
        if let Some(aa) = aa {
            items.push(("ALBUMARTIST", aa));
        }
        if rg {
            items.push(("REPLAYGAIN_TRACK_GAIN", "-5.00 dB"));
            items.push(("REPLAYGAIN_ALBUM_GAIN", "-5.50 dB"));
        }
        vorbis(&p, &items, Vec::new());
    }
    let p = a.join("07 Patch Seven.mp3");
    audio(&p, 3, mp3);
    let mut t = Id3v2Tag::new();
    for f in [
        text("TIT2", "Patch Seven"),
        text("TPE1", "The Menders"),
        text("TALB", "Patchwork"),
        text("TPE2", "The Menders"),
        text("TRCK", "7"),
        text("TDRC", "2021"),
        text("TCON", "Indie"),
        txxx("MusicBrainz Album Id", mb),
        txxx("REPLAYGAIN_TRACK_GAIN", "-5.00 dB"),
    ] {
        t.insert(f);
    }
    t.save_to_path(&p, WriteOptions::default()).unwrap();
    std::fs::write(a.join("folder.jpg"), jpeg(700, 700, false)).unwrap();

    // ---- Edge Cases ------------------------------------------------------------------
    let a = dir.join("Edge Cases");
    std::fs::create_dir_all(&a).unwrap();
    let p = a.join("01 Long Comment.flac");
    audio(&p, 3, flac);
    let comment: String = (0..2400).map(|i| format!("Line {i:04} of a very long liner note. ")).collect::<String>();
    let extra: Vec<(String, String)> = (0..3000).map(|i| (format!("CUSTOM_{i:04}"), format!("value {i}"))).collect();
    let mut items: Vec<(&str, &str)> = vec![("TITLE", "Long Comment"), ("ARTIST", "Edge"), ("ALBUM", "Edge Cases"), ("TRACKNUMBER", "1"), ("COMMENT", &comment)];
    items.extend(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    vorbis(&p, &items, vec![picture(PictureType::CoverFront, MimeType::Jpeg, jpeg(2500, 2500, true), "")]);
    for (n, title) in [(2, "Gone"), (3, "Corrupt")] {
        let p = a.join(format!("0{n} {title}.flac"));
        audio(&p, 3, flac);
        vorbis(&p, &[("TITLE", title), ("ARTIST", "Edge"), ("ALBUM", "Edge Cases"), ("TRACKNUMBER", &n.to_string())], Vec::new());
    }

    // ---- Big Album: 400 tracks (one short file, copied and retagged) ------------------
    let a = dir.join("Big Album");
    std::fs::create_dir_all(&a).unwrap();
    let one = a.join("source.tmp.flac");
    audio(&one, 1, flac);
    for n in 1..=400u32 {
        let p = a.join(format!("{n:03} Part {n}.flac"));
        std::fs::copy(&one, &p).unwrap();
        let (no, title) = (n.to_string(), format!("Part {n}"));
        vorbis(&p, &[("TITLE", &title), ("ARTIST", "Long Ensemble"), ("ALBUM", "Big Album"), ("TRACKNUMBER", &no), ("MUSICBRAINZ_ALBUMID", "44444444-5555-4666-8777-888888888888")], Vec::new());
    }
    std::fs::remove_file(&one).unwrap();
}
