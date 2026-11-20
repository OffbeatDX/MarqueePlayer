//! Applying changes to a file with lofty (the version the reader uses). Only ever called
//! on the temporary copy `atomic` makes, never on a music file itself.
//!
//! Each block is changed item by item, everything else in it left as lofty read it, and
//! only the blocks that change are written (an MP3's APE tag stays as it was when only its
//! ID3v2 changes). The file is read with lofty's implicit conversions off, so an ID3v2.3
//! tag keeps its TYER and TDAT frames and MP4 its `gnre`. ID3v2.3 stays ID3v2.3 and keeps
//! its padding; FLAC gets no padding block it did not have.

use std::borrow::Cow;
use std::fs::File;
use std::path::Path;

use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{CommentFrame, ExtendedTextFrame, Frame, FrameId, Id3v2Tag, TextInformationFrame, TimestampFrame, UniqueFileIdentifierFrame, UnsynchronizedTextFrame};
use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::TagExt;

use super::{Change, Container, PictureData, PictureEdit};
use crate::meta::BlockKind;

pub(super) fn parse_options() -> ParseOptions {
    ParseOptions::new().implicit_conversions(false).read_cover_art(true).read_properties(false)
}

fn e<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn open_rw(path: &Path) -> Result<File, String> {
    std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(e)
}

fn open_r(path: &Path) -> Result<File, String> {
    File::open(path).map_err(e)
}

/// What the writes need to know of the file as it was (`raw::Inventory`).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Keep {
    /// The ID3v2 tag's padding (None: there was no ID3v2 tag).
    pub id3v2_padding: Option<u32>,
    /// FLAC: the file has a PADDING block.
    pub flac_padding: bool,
}

pub(super) fn picture(p: &PictureData) -> Picture {
    let b = Picture::unchecked(p.data.clone()).pic_type(PictureType::from_u8(p.type_code)).mime_type(MimeType::from_str(&p.mime));
    if p.description.is_empty() { b.build() } else { b.description(p.description.clone()).build() }
}

/// Apply `changes` and `pictures` to the file at `path` (the temporary copy).
pub(super) fn apply(path: &Path, container: Container, changes: &[Change], pictures: Option<&PictureEdit>, keep: Keep) -> Result<(), String> {
    let po = parse_options();
    let of = |k: fn(BlockKind) -> bool| changes.iter().filter(move |c| k(c.block));
    match container {
        Container::Flac => {
            let f = lofty::flac::FlacFile::read_from(&mut open_r(path)?, po).map_err(e)?;
            let mut vc = f.vorbis_comments().cloned().unwrap_or_default();
            vorbis(&mut vc, of(|b| b == BlockKind::Vorbis))?;
            // FLAC's PICTURE blocks are the file's, not the comments': lofty writes the
            // comments' pictures as PICTURE blocks, so they go in with them, in order.
            let mut pics: Vec<_> = f.pictures().to_vec();
            if let Some(pe) = pictures {
                if pe.remove_front {
                    pics.retain(|(p, _)| p.pic_type() != PictureType::CoverFront);
                    vc.remove_picture_type(PictureType::CoverFront);
                }
                for a in &pe.add {
                    let p = picture(a);
                    let info = lofty::picture::PictureInformation::from_picture(&p).unwrap_or_default();
                    pics.push((p, info));
                }
            }
            for (p, info) in pics {
                vc.insert_picture(p, Some(info)).map_err(e)?;
            }
            let wo = WriteOptions::new().parse_options(po).preferred_padding(if keep.flac_padding { WriteOptions::DEFAULT_PREFERRED_PADDING } else { 0 });
            vc.save_to(&mut open_rw(path)?, wo).map_err(e)?;
        }
        Container::Opus | Container::Vorbis => {
            let mut vc = if container == Container::Opus {
                lofty::ogg::OpusFile::read_from(&mut open_r(path)?, po).map_err(e)?.vorbis_comments().clone()
            } else {
                lofty::ogg::VorbisFile::read_from(&mut open_r(path)?, po).map_err(e)?.vorbis_comments().clone()
            };
            vorbis(&mut vc, of(|b| b == BlockKind::Vorbis))?;
            if let Some(pe) = pictures {
                if pe.remove_front {
                    vc.remove_picture_type(PictureType::CoverFront);
                }
                for a in &pe.add {
                    vc.insert_picture(picture(a), None).map_err(e)?;
                }
            }
            vc.save_to(&mut open_rw(path)?, WriteOptions::new().parse_options(po)).map_err(e)?;
        }
        Container::Mp4 => {
            let f = lofty::mp4::Mp4File::read_from(&mut open_r(path)?, po).map_err(e)?;
            let mut ilst = f.ilst().cloned().unwrap_or_default();
            for c in of(|b| b == BlockKind::Mp4) {
                mp4(&mut ilst, c)?;
            }
            if let Some(pe) = pictures {
                if pe.remove_front {
                    ilst.remove_pictures();
                }
                for a in &pe.add {
                    ilst.insert_picture(picture(a));
                }
            }
            ilst.save_to(&mut open_rw(path)?, WriteOptions::new().parse_options(po)).map_err(e)?;
        }
        Container::Mpeg | Container::Aac | Container::Wav | Container::Aiff => {
            let (mut id3, v1, ape, riff, aiff) = match container {
                Container::Mpeg => {
                    let f = lofty::mpeg::MpegFile::read_from(&mut open_r(path)?, po).map_err(e)?;
                    (f.id3v2().cloned(), f.id3v1().cloned(), f.ape().cloned(), None, None)
                }
                Container::Aac => {
                    let f = lofty::aac::AacFile::read_from(&mut open_r(path)?, po).map_err(e)?;
                    (f.id3v2().cloned(), f.id3v1().cloned(), None, None, None)
                }
                Container::Wav => {
                    let f = lofty::iff::wav::WavFile::read_from(&mut open_r(path)?, po).map_err(e)?;
                    (f.id3v2().cloned(), None, None, f.riff_info().cloned(), None)
                }
                _ => {
                    let f = lofty::iff::aiff::AiffFile::read_from(&mut open_r(path)?, po).map_err(e)?;
                    (f.id3v2().cloned(), None, None, None, f.text_chunks().cloned())
                }
            };
            let wo = WriteOptions::new().parse_options(po);
            // The trailing and side blocks first, then ID3v2.
            if let Some(mut t) = ape
                && changes.iter().any(|c| c.block == BlockKind::Ape)
            {
                for c in of(|b| b == BlockKind::Ape) {
                    ape_item(&mut t, c)?;
                }
                t.save_to(&mut open_rw(path)?, wo).map_err(e)?;
            }
            if changes.iter().any(|c| c.block == BlockKind::Id3v1) {
                let mut t = v1.unwrap_or_default();
                for c in of(|b| b == BlockKind::Id3v1) {
                    id3v1(&mut t, c)?;
                }
                t.save_to(&mut open_rw(path)?, wo).map_err(e)?;
            }
            if changes.iter().any(|c| c.block == BlockKind::RiffInfo) {
                let mut t = riff.unwrap_or_default();
                for c in of(|b| b == BlockKind::RiffInfo) {
                    match c.values.first() {
                        Some(v) => t.insert(c.key.clone(), v.clone()),
                        None => {
                            t.remove(&c.key);
                        }
                    }
                }
                t.save_to(&mut open_rw(path)?, wo).map_err(e)?;
            }
            if changes.iter().any(|c| c.block == BlockKind::AiffText) {
                let mut t = aiff.unwrap_or_default();
                for c in of(|b| b == BlockKind::AiffText) {
                    let one = c.values.first().cloned();
                    match c.key.as_str() {
                        "NAME" => t.name = one,
                        "AUTH" => t.author = one,
                        "(c) " => t.copyright = one,
                        "ANNO" => t.annotations = (!c.values.is_empty()).then(|| c.values.clone()),
                        other => return Err(format!("AIFF text chunk {other} is not written")),
                    }
                }
                t.save_to(&mut open_rw(path)?, wo).map_err(e)?;
            }
            let id3_changes: Vec<&Change> = changes.iter().filter(|c| matches!(c.block, BlockKind::Id3v2(_))).collect();
            if !id3_changes.is_empty() || pictures.is_some() {
                let version = id3_changes.first().map(|c| c.block).and_then(|b| if let BlockKind::Id3v2(v) = b { Some(v) } else { None });
                let created = id3.is_none();
                let t = id3.get_or_insert_with(Id3v2Tag::new);
                let v3 = if created { version != Some(4) } else { t.original_version() == lofty::id3::v2::Id3v2Version::V3 };
                for c in &id3_changes {
                    id3v2(t, c, v3)?;
                }
                if let Some(pe) = pictures {
                    if pe.remove_front {
                        t.remove_picture_type(PictureType::CoverFront);
                    }
                    for a in &pe.add {
                        let _ = t.insert_picture(picture(a));
                    }
                }
                // The padding the tag had (none: none); a new tag gets lofty's default.
                let padding = keep.id3v2_padding.unwrap_or(WriteOptions::DEFAULT_PREFERRED_PADDING);
                let mut wo = wo;
                let wo = wo.use_id3v23(v3).preferred_padding(padding);
                t.save_to(&mut open_rw(path)?, wo).map_err(e)?;
            }
        }
    }
    Ok(())
}

/// Vorbis comments: each changed key's values where its first item was, its other items
/// gone; new keys at the end. Every other item stays where it is, in its own spelling.
fn vorbis<'a>(vc: &mut VorbisComments, changes: impl Iterator<Item = &'a Change>) -> Result<(), String> {
    let changes: Vec<&Change> = changes.collect();
    if changes.is_empty() {
        return Ok(());
    }
    let items: Vec<(String, String)> = vc.take_items().collect();
    let mut done = vec![false; changes.len()];
    for (k, v) in items {
        match changes.iter().position(|c| c.key.eq_ignore_ascii_case(&k)) {
            Some(i) => {
                if !done[i] {
                    done[i] = true;
                    for nv in &changes[i].values {
                        vc.push(changes[i].key.clone(), nv.clone());
                    }
                }
            }
            None => vc.push(k, v),
        }
    }
    for (i, c) in changes.iter().enumerate() {
        if !done[i] {
            for nv in &c.values {
                vc.push(c.key.clone(), nv.clone());
            }
        }
    }
    // `push` silently drops a key Vorbis does not allow: say so instead.
    for c in &changes {
        if !c.values.is_empty() && vc.get(&c.key).is_none() {
            return Err(format!("\u{201c}{}\u{201d} is not a valid Vorbis comment name", c.key));
        }
    }
    Ok(())
}

const TIMESTAMP_FRAMES: &[&str] = &["TDRC", "TDOR", "TDRL", "TDEN", "TDTG"];

fn id3v2(t: &mut Id3v2Tag, c: &Change, v3: bool) -> Result<(), String> {
    let enc = if v3 { TextEncoding::UTF16 } else { TextEncoding::UTF8 };
    let key = c.key.as_str();
    // Which frames this change replaces.
    let (id, desc): (&str, Option<&str>) = match key.split_once(':') {
        Some((id, d)) => (id, Some(d)),
        None => (key, None),
    };
    let matches = |f: &Frame<'_>| -> bool {
        if f.id_str() != id {
            return false;
        }
        match (f, desc) {
            (Frame::UserText(x), Some(d)) => x.description == d,
            (Frame::UserText(x), None) => x.description.is_empty(),
            (Frame::Comment(x), Some(d)) => x.description == d,
            (Frame::Comment(x), None) => x.description.is_empty(),
            (Frame::UnsynchronizedText(x), Some(d)) => x.description == d,
            (Frame::UnsynchronizedText(x), None) => x.description.is_empty(),
            (Frame::UniqueFileIdentifier(x), Some(d)) => x.owner == d,
            (Frame::Private(x), Some(d)) => x.owner == d,
            (Frame::UserUrl(x), Some(d)) => x.description == d,
            (Frame::Popularimeter(x), Some(d)) => x.email == d,
            (_, None) => true,
            _ => false,
        }
    };
    // A comment keeps the language it had.
    let lang = t.iter().find_map(|f| if let Frame::Comment(x) = f { matches(f).then_some(x.language) } else { None }).unwrap_or(*b"eng");
    // Lyrics too; new ones say "XXX" (the language not known: they can be in any).
    let lyrics_lang = t.iter().find_map(|f| if let Frame::UnsynchronizedText(x) = f { matches(f).then_some(x.language) } else { None }).unwrap_or(*b"XXX");
    let new: Option<Frame<'static>> = if c.values.is_empty() {
        None
    } else {
        let value = c.values.join("\0");
        let fid = || FrameId::new(id.to_owned()).map_err(e);
        Some(match id {
            "TXXX" => Frame::UserText(ExtendedTextFrame::new(enc, desc.unwrap_or("").to_owned(), value)),
            "COMM" => Frame::Comment(CommentFrame::new(enc, lang, desc.unwrap_or("").to_owned(), c.values.join("\n"))),
            "USLT" => Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(enc, lyrics_lang, desc.unwrap_or("").to_owned(), c.values.join("\n"))),
            "UFID" => Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(desc.unwrap_or("").to_owned(), c.values[0].as_bytes().to_vec())),
            _ if TIMESTAMP_FRAMES.contains(&id) => {
                let ts: lofty::tag::items::Timestamp = c.values[0].parse().map_err(|_| format!("\u{201c}{}\u{201d} is not a date ID3v2 can hold", c.values[0]))?;
                Frame::Timestamp(TimestampFrame::new(fid()?, enc, ts))
            }
            _ if id.starts_with('T') && id.len() == 4 => Frame::Text(TextInformationFrame::new(fid()?, enc, value)),
            _ => return Err(format!("the ID3v2 frame {key} is not written by the editor")),
        })
    };
    // In place of the first frame it replaces; the others it replaces go.
    let mut new = new;
    let mut placed = false;
    t.retain_mut(|f| {
        if !matches(f) {
            return true;
        }
        if !placed && let Some(n) = new.take() {
            *f = n;
            placed = true;
            return true;
        }
        false
    });
    if let Some(n) = new {
        let _ = t.insert(n);
    }
    Ok(())
}

fn id3v1(t: &mut lofty::id3::v1::Id3v1Tag, c: &Change) -> Result<(), String> {
    let v = c.values.first().cloned();
    match c.key.as_str() {
        "Title" => t.title = v,
        "Artist" => t.artist = v,
        "Album" => t.album = v,
        "Comment" => t.comment = v,
        "Year" => t.year = v.and_then(|y| y.parse().ok()),
        "Track" => t.track_number = v.and_then(|n| n.parse().ok()),
        "Genre" => t.genre = v.and_then(|g| lofty::id3::v1::GENRES.iter().position(|x| x.eq_ignore_ascii_case(&g))).map(|i| i as u8),
        other => return Err(format!("ID3v1 has no {other}")),
    }
    Ok(())
}

fn ape_item(t: &mut lofty::ape::ApeTag, c: &Change) -> Result<(), String> {
    t.remove(&c.key);
    if !c.values.is_empty() {
        let item = lofty::ape::ApeItem::new(c.key.clone(), lofty::tag::ItemValue::Text(c.values.join("\0"))).map_err(e)?;
        t.insert(item);
    }
    Ok(())
}

/// An MP4 atom's name as `meta` writes it ("©nam", "----:com.apple.iTunes:LABEL").
fn mp4_ident(key: &str) -> Result<lofty::mp4::AtomIdent<'static>, String> {
    use lofty::mp4::AtomIdent;
    if let Some(rest) = key.strip_prefix("----:") {
        let (mean, name) = rest.split_once(':').ok_or_else(|| format!("bad freeform name {key}"))?;
        return Ok(AtomIdent::Freeform { mean: Cow::Owned(mean.to_owned()), name: Cow::Owned(name.to_owned()) });
    }
    let bytes: Vec<u8> = key.chars().map(|c| u8::try_from(c as u32).unwrap_or(b'?')).collect();
    let code: [u8; 4] = bytes.try_into().map_err(|_| format!("bad atom name {key}"))?;
    Ok(AtomIdent::Fourcc(code))
}

fn mp4(ilst: &mut lofty::mp4::Ilst, c: &Change) -> Result<(), String> {
    use lofty::mp4::{Atom, AtomData, DataType};
    let ident = mp4_ident(&c.key)?;
    let _ = ilst.remove(&ident).count();
    if c.values.is_empty() {
        return Ok(());
    }
    let number = |s: &str| s.trim().parse::<u16>().map_err(|_| format!("\u{201c}{s}\u{201d} is not a number MP4 can hold"));
    let data: Vec<AtomData> = match c.key.as_str() {
        "trkn" | "disk" => {
            let v = &c.values[0];
            let (n, t) = v.split_once('/').unwrap_or((v, "0"));
            let (n, t) = (number(n)?, number(t)?);
            let mut d = vec![0, 0];
            d.extend_from_slice(&n.to_be_bytes());
            d.extend_from_slice(&t.to_be_bytes());
            if c.key == "trkn" {
                d.extend_from_slice(&[0, 0]);
            }
            vec![AtomData::Unknown { code: DataType::Reserved, data: d }]
        }
        "cpil" => vec![AtomData::Bool(c.values[0] == "Yes")],
        // A 16-bit integer, as iTunes writes it.
        "tmpo" => vec![AtomData::Unknown { code: DataType::BeSignedInteger, data: number(&c.values[0])?.to_be_bytes().to_vec() }],
        _ => c.values.iter().map(|v| AtomData::UTF8(v.clone())).collect(),
    };
    let atom = Atom::from_collection(ident, data).ok_or("no values")?;
    ilst.insert(atom);
    Ok(())
}
