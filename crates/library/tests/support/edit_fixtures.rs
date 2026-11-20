//! Fixtures for the tag editor's tests (`tests/edit.rs`) and the bench (`MP_BENCH=editing`):
//! the metadata viewer's set (`meta_fixtures.rs`) plus every other container the scanner
//! takes, each with awkward tags. Made by ffmpeg and tagged by lofty, only inside the
//! folder given, which gets the marker file `MARKER`: the editor's write guard lets tests
//! and the bench write only under a folder that has it.
#![allow(dead_code)]

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use lofty::TextEncoding;
use lofty::config::WriteOptions;
use lofty::id3::v2::{BinaryFrame, ExtendedTextFrame, ExtendedUrlFrame, Frame, FrameId, Id3v2Tag, PopularimeterFrame, PrivateFrame, TextInformationFrame};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::TagExt;

#[path = "meta_fixtures.rs"]
mod meta_fixtures;

pub use meta_fixtures::ffmpeg;

/// The marker file of a folder the editor may write in during tests and the bench.
pub const MARKER: &str = ".mp-edit-fixtures";

/// The write guard tests and the bench install.
pub fn guard(p: &Path) -> bool {
    library::edit::atomic::under_marker(p, MARKER)
}

fn tone(out: &Path, secs: u32, codec: &[&str]) {
    let d = secs.to_string();
    let src = format!("sine=frequency=440:sample_rate=44100:duration={d}");
    let mut a = vec!["-f", "lavfi", "-i", &src, "-ac", "2", "-map_metadata", "-1", "-fflags", "+bitexact"];
    a.extend(codec);
    let o = out.to_str().unwrap().to_owned();
    a.push(&o);
    ffmpeg(&a);
}

fn text(id: &'static str, value: &str) -> Frame<'static> {
    Frame::Text(TextInformationFrame::new(FrameId::Valid(Cow::Borrowed(id)), TextEncoding::UTF16, value.to_owned()))
}

fn jpeg(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x / 3) as u8 ^ shade, shade, (y / 2) as u8]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

pub fn cover_jpeg(shade: u8) -> Vec<u8> {
    jpeg(300, 300, shade)
}

fn pic(t: PictureType, data: Vec<u8>, desc: &str) -> Picture {
    let b = Picture::unchecked(data).pic_type(t).mime_type(MimeType::Jpeg);
    if desc.is_empty() { b.build() } else { b.description(desc.to_owned()).build() }
}

/// Every fixture, by a short name, relative to the root.
pub const FILES: &[(&str, &str)] = &[
    ("flac", "Picard Set/01 Northern Lights.flac"),
    ("flac-plain", "Inconsistent Album/01 Patch One.flac"),
    ("mp3-v24", "ID3 Formats/01 Twenty Four.mp3"),
    ("mp3-v23-v1", "ID3 Formats/02 Twenty Three.mp3"),
    ("opus", "Other Formats/01 Opus R128.opus"),
    ("m4a", "Other Formats/02 AAC Atoms.m4a"),
    ("wav-untagged", "Untagged/No Tags.wav"),
    ("vorbis", "More Formats/01 Vorbis.ogg"),
    ("alac", "More Formats/02 Alac.m4a"),
    ("m4b", "More Formats/03 Book.m4b"),
    ("mp2", "More Formats/04 Layer Two.mp2"),
    ("mp3-ape", "More Formats/05 Three Blocks.mp3"),
    ("mp3-odd", "More Formats/06 Odd Frames.mp3"),
    ("wav-id3-info", "More Formats/07 Both Blocks.wav"),
    ("wav-info", "More Formats/08 Info Only.wav"),
    ("aiff", "More Formats/09 Aiff.aiff"),
    ("aiff-text", "More Formats/10 Text Chunks.aif"),
    ("mp3-untagged", "More Formats/11 Bare.mp3"),
    ("flac-untagged", "More Formats/12 Bare.flac"),
    ("adts", "More Formats/13 Raw.aac"),
    ("long", "More Formats/14 Long Play.flac"),
];

pub fn file(root: &Path, name: &str) -> PathBuf {
    root.join(FILES.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no fixture {name}")).1)
}

/// Make every fixture under `dir` (emptied first when it holds our marker; refused when it
/// exists without it).
pub fn make(dir: &Path) {
    if dir.exists() {
        assert!(dir.join(MARKER).exists(), "{} exists and was not made by this tool", dir.display());
        std::fs::remove_dir_all(dir).unwrap();
    }
    meta_fixtures::make(dir);
    std::fs::write(dir.join(MARKER), b"made by the tag editor's fixtures: may be written to").unwrap();
    let a = dir.join("More Formats");
    std::fs::create_dir_all(&a).unwrap();
    let f = |n: &str| file(dir, n);

    // Ogg Vorbis with a picture in METADATA_BLOCK_PICTURE and a lower-case key.
    tone(&f("vorbis"), 3, &["-c:a", "libvorbis", "-q:a", "2"]);
    {
        use lofty::ogg::OggPictureStorage;
        use lofty::ogg::tag::VorbisComments;
        let mut vc = VorbisComments::new();
        vc.set_vendor("Marquee fixtures".into());
        for (k, v) in [("TITLE", "Vorbis One"), ("ARTIST", "Format Lab"), ("ALBUM", "More Formats"), ("tracknumber", "1"), ("GENRE", "Ambient"), ("GENRE", "Drone"), ("REPLAYGAIN_TRACK_GAIN", "-2.00 dB")] {
            vc.push(k.into(), v.into());
        }
        vc.insert_picture(pic(PictureType::CoverFront, cover_jpeg(10), ""), None).unwrap();
        vc.save_to_path(f("vorbis"), WriteOptions::default()).unwrap();
    }
    // ALAC and an audiobook in MP4.
    for (name, codec) in [("alac", &["-c:a", "alac"][..]), ("m4b", &["-c:a", "aac", "-b:a", "96k", "-f", "mp4"][..])] {
        tone(&f(name), 3, codec);
        use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
        let mut ilst = Ilst::new();
        for (c, v) in [(*b"\xa9nam", "MP4 Title"), (*b"\xa9ART", "Format Lab"), (*b"\xa9alb", "More Formats"), (*b"\xa9day", "2020")] {
            ilst.insert(Atom::new(AtomIdent::Fourcc(c), AtomData::UTF8(v.into())));
        }
        ilst.insert(Atom::new(AtomIdent::Freeform { mean: Cow::Borrowed("com.apple.iTunes"), name: Cow::Borrowed("CUSTOM NOTE") }, AtomData::UTF8("kept".into())));
        ilst.insert(Atom::new(AtomIdent::Fourcc(*b"gnre"), AtomData::Unknown { code: lofty::mp4::DataType::Reserved, data: vec![0, 18] }));
        ilst.insert_picture(pic(PictureType::Other, cover_jpeg(20), ""));
        ilst.save_to_path(f(name), WriteOptions::default()).unwrap();
    }
    // MP2 with ID3v2.4.
    tone(&f("mp2"), 3, &["-c:a", "mp2", "-b:a", "192k"]);
    {
        let mut t = Id3v2Tag::new();
        for fr in [text("TIT2", "Layer Two"), text("TPE1", "Format Lab"), text("TALB", "More Formats"), text("TRCK", "4")] {
            t.insert(fr);
        }
        t.save_to_path(f("mp2"), WriteOptions::default()).unwrap();
    }
    // MP3 with ID3v2.3, APEv2 and ID3v1, all three.
    let mp3: &[&str] = &["-c:a", "libmp3lame", "-b:a", "128k", "-id3v2_version", "0", "-write_id3v1", "0"];
    tone(&f("mp3-ape"), 3, mp3);
    {
        let mut t = Id3v2Tag::new();
        for fr in [text("TIT2", "Three Blocks"), text("TPE1", "Format Lab"), text("TALB", "More Formats"), text("TRCK", "5/12"), text("TYER", "2019"), text("TDAT", "0803")] {
            t.insert(fr);
        }
        t.insert_picture(pic(PictureType::CoverFront, cover_jpeg(30), "front"));
        t.save_to_path(f("mp3-ape"), WriteOptions::default().use_id3v23(true)).unwrap();
        let mut ape = lofty::ape::ApeTag::new();
        for (k, v) in [("Title", "Three Blocks"), ("Artist", "Format Lab"), ("Album", "More Formats"), ("Track", "5/12"), ("Catalog", "APE-1")] {
            ape.insert(lofty::ape::ApeItem::new(k.into(), lofty::tag::ItemValue::Text(v.into())).unwrap());
        }
        ape.insert(lofty::ape::ApeItem::new("Binary Thing".into(), lofty::tag::ItemValue::Binary(vec![1, 2, 3, 4, 5])).unwrap());
        ape.save_to_path(f("mp3-ape"), WriteOptions::default()).unwrap();
        let mut v1 = lofty::id3::v1::Id3v1Tag::new();
        v1.title = Some("Three Blocks".into());
        v1.artist = Some("Format Lab".into());
        v1.album = Some("More Formats".into());
        v1.year = Some(2019);
        v1.track_number = Some(5);
        v1.genre = Some(12);
        v1.save_to_path(f("mp3-ape"), WriteOptions::default()).unwrap();
    }
    // MP3, ID3v2.3, frames the editor never writes: they must come through untouched.
    tone(&f("mp3-odd"), 3, mp3);
    {
        let mut t = Id3v2Tag::new();
        for fr in [
            text("TIT2", "Odd Frames"),
            text("TPE1", "Format Lab"),
            text("TALB", "More Formats"),
            text("TYER", "2018"),
            Frame::UserText(ExtendedTextFrame::new(TextEncoding::UTF16, "MusicBrainz Album Id".to_owned(), "55555555-6666-4777-8888-999999999999".to_owned())),
            Frame::UserUrl(ExtendedUrlFrame::new(TextEncoding::Latin1, "Shop".to_owned(), "https://example.com/shop".to_owned())),
            Frame::Popularimeter(PopularimeterFrame::new("someone@example.com".to_owned(), 196, 42)),
            Frame::Private(PrivateFrame::new("com.example.private".to_owned(), vec![9u8, 8, 7, 6, 5, 4, 3, 2, 1])),
            Frame::Binary(BinaryFrame::new(FrameId::Valid(Cow::Borrowed("GEOB")), vec![0, b'a', b'/', b'b', 0, b'f', 0, b'd', 0, 1, 2, 3])),
            Frame::Binary(BinaryFrame::new(FrameId::Valid(Cow::Borrowed("PCNT")), vec![0, 0, 0, 77])),
        ] {
            t.insert(fr);
        }
        t.save_to_path(f("mp3-odd"), WriteOptions::default().use_id3v23(true).preferred_padding(300)).unwrap();
    }
    // WAV with RIFF INFO (ffmpeg writes it) and an ID3v2.4 chunk; WAV with INFO only.
    for (name, id3) in [("wav-id3-info", true), ("wav-info", false)] {
        tone(&f(name), 2, &["-c:a", "pcm_s16le", "-metadata", "title=INFO Title", "-metadata", "artist=Info Artist", "-metadata", "album=More Formats", "-metadata", "date=2017", "-metadata", "comment=from ffmpeg"]);
        if id3 {
            let mut t = Id3v2Tag::new();
            for fr in [text("TIT2", "ID3 Title"), text("TPE1", "Format Lab"), text("TALB", "More Formats"), text("TPE2", "Format Lab")] {
                t.insert(fr);
            }
            t.save_to_path(f(name), WriteOptions::default()).unwrap();
        }
    }
    // AIFF with ID3v2 and text chunks; AIFF with text chunks only.
    for (name, id3) in [("aiff", true), ("aiff-text", false)] {
        tone(&f(name), 2, &["-c:a", "pcm_s16be", "-f", "aiff", "-write_id3v2", "0"]);
        let mut tc = lofty::iff::aiff::AiffTextChunks::new();
        tc.name = Some("Text Name".into());
        tc.author = Some("Text Author".into());
        tc.annotations = Some(vec!["An annotation".into()]);
        tc.save_to_path(f(name), WriteOptions::default()).unwrap();
        if id3 {
            let mut t = Id3v2Tag::new();
            for fr in [text("TIT2", "Aiff ID3"), text("TPE1", "Format Lab"), text("TALB", "More Formats")] {
                t.insert(fr);
            }
            t.insert_picture(pic(PictureType::CoverFront, cover_jpeg(40), ""));
            t.save_to_path(f(name), WriteOptions::default()).unwrap();
        }
    }
    // No tags at all; raw ADTS AAC.
    tone(&f("mp3-untagged"), 2, mp3);
    tone(&f("flac-untagged"), 2, &["-c:a", "flac"]);
    tone(&f("adts"), 2, &["-c:a", "aac", "-b:a", "96k", "-f", "adts"]);
    // Long enough to be playing while it is saved (the bench).
    tone(&f("long"), 20, &["-c:a", "flac"]);
    {
        use lofty::ogg::tag::VorbisComments;
        let mut vc = VorbisComments::new();
        for (k, v) in [("TITLE", "Long Play"), ("ARTIST", "Format Lab"), ("ALBUM", "More Formats"), ("TRACKNUMBER", "14")] {
            vc.push(k.into(), v.into());
        }
        vc.save_to_path(f("long"), WriteOptions::default()).unwrap();
    }
}

/// Copy fixtures (by name) into a fresh folder of their own, keeping their relative paths,
/// with the marker. Returns the folder.
pub fn copy(master: &Path, to: &Path, names: &[&str]) -> PathBuf {
    if to.exists() {
        assert!(to.join(MARKER).exists(), "{} exists and was not made by these tests", to.display());
        std::fs::remove_dir_all(to).unwrap();
    }
    std::fs::create_dir_all(to).unwrap();
    std::fs::write(to.join(MARKER), b"a copy of the tag editor's fixtures").unwrap();
    for n in names {
        let src = file(master, n);
        let dst = file(to, n);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(&src, &dst).unwrap();
    }
    to.to_path_buf()
}
