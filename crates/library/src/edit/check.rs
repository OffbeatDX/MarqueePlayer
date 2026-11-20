//! Checking a written file against the original before it may replace it:
//!
//! - it parses, and its tags read back (`meta::read`, raw) as the original's with exactly
//!   the planned changes: every other item of every block the same, values and binary data;
//! - its pictures are the original's, but for the front cover when that was changed;
//! - lofty dropped nothing it does not show: the file's own tag items (`raw::Inventory`:
//!   ID3v2 frames, Vorbis comments, MP4 atoms, RIFF and AIFF chunks, APE items, ID3v1)
//!   are the original's outside what was changed, the same bytes where they can be
//!   compared as bytes; ID3v2 keeps its version and padding, FLAC its padding blocks;
//! - the audio is the same: the bytes outside the tags (`raw::Layout`: MP3 frames, FLAC
//!   frames and STREAMINFO, the Ogg audio packets and their granules, MP4 `mdat` and the
//!   sample tables, WAV and AIFF chunks), the packets Symphonia demuxes from it (the
//!   player's own demuxer: every packet's track, time stamps, trimming and bytes, and the
//!   codec parameters), and the duration, sample rate, channels and bit depth lofty reads.

use std::collections::BTreeMap;
use std::path::Path;

use crate::hash::Hasher;
use crate::meta::{self, BlockKind, FileTags, RawItem, ReadOptions};

use super::raw::{self, RawView};
use super::{Change, Container, PictureEdit};

/// What a file was before a write: what the written file is compared with.
#[derive(Clone, Debug)]
pub(super) struct Before {
    pub path: std::path::PathBuf,
    pub tags: FileTags,
    pub raw: RawView,
    pub audio: Audio,
}

/// The audio as the player's demuxer and lofty see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Audio {
    pub packets: u64,
    pub digest: u128,
    pub props: String,
}

fn e<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub(super) fn snapshot(path: &Path, container: Container, tags: FileTags) -> Result<Before, String> {
    Ok(Before { path: path.to_path_buf(), raw: raw::view(path, container.shape())?, audio: audio(path, path)?, tags })
}

/// Every packet Symphonia demuxes, hashed in order, and lofty's properties.
/// `hint`: the original file's path (the copy is a `.tmp`), whose extension tells Symphonia the format.
pub(super) fn audio(path: &Path, hint_path: &Path) -> Result<Audio, String> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).map_err(e)?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = hint_path.extension().and_then(|x| x.to_str()) {
        hint.with_extension(ext);
    }
    let mut reader = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).map_err(|x| format!("the audio cannot be opened: {x}"))?;
    let mut h = Hasher::new();
    for t in reader.tracks() {
        h.number(u64::from(t.id));
        h.update(format!("{:?}|{:?}|{:?}|{:?}|{:?}", t.codec_params, t.time_base, t.num_frames, t.delay, t.padding).as_bytes());
    }
    let mut packets = 0u64;
    loop {
        match reader.next_packet() {
            Ok(Some(p)) => {
                packets += 1;
                h.number(u64::from(p.track_id));
                h.number(p.pts.get() as u64);
                h.number(p.dts.get() as u64);
                h.number(p.dur.get());
                h.number(p.trim_start.get());
                h.number(p.trim_end.get());
                h.number(p.data.len() as u64);
                h.update(&p.data);
            }
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(x)) if x.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // The same error in both files is the same audio (a damaged last frame).
            Err(x) => {
                h.update(format!("error: {x}").as_bytes());
                break;
            }
        }
    }
    Ok(Audio { packets, digest: h.finish128(), props: properties(path)? })
}

/// lofty's view of the audio: duration, sample rate, channels, bit depth.
fn properties(path: &Path) -> Result<String, String> {
    use lofty::file::AudioFile;
    let options = lofty::config::ParseOptions::new().read_tags(false).read_cover_art(false);
    // By the content, not the name: the copy is a `.tmp`.
    let f = lofty::probe::Probe::open(path).map_err(e)?.options(options).guess_file_type().map_err(e)?.read().map_err(e)?;
    let p = f.properties();
    Ok(format!("{} ms, {:?} Hz, {:?} channels, {:?} bit", p.duration().as_millis(), p.sample_rate(), p.channels(), p.bit_depth()))
}

/// An item as compared: its block's kind, key, values and data.
type ItemKey = (String, String, Vec<String>, Option<usize>, Option<u64>);

fn item_key(tags: &FileTags, i: &RawItem) -> ItemKey {
    (tags.blocks.get(i.block).map(|b| b.name()).unwrap_or_default(), i.key.clone(), i.values.clone(), i.binary, i.hash)
}

/// Pictures as compared: block kind, type, MIME type, description, data.
type PicKey = (String, u8, String, String, u64);

fn pic_key(tags: &FileTags, p: &meta::PictureMeta) -> PicKey {
    (tags.blocks.get(p.block).map(|b| b.name()).unwrap_or_default(), p.type_code, p.mime.to_ascii_lowercase().replace("image/jpg", "image/jpeg"), p.description.clone(), p.hash)
}

fn counts<K: Ord + Clone>(items: impl Iterator<Item = K>) -> BTreeMap<K, usize> {
    let mut m = BTreeMap::new();
    for k in items {
        *m.entry(k).or_default() += 1;
    }
    m
}

fn show_item(k: &ItemKey) -> String {
    match k.3 {
        Some(n) if k.2.is_empty() => format!("{} {} ({n} bytes of data)", k.0, k.1),
        _ => format!("{} {} \u{201c}{}\u{201d}", k.0, k.1, k.2.join("; ")),
    }
}

/// The tags the written file should have: the original's with the changes made.
pub(super) fn expected(before: &FileTags, changes: &[Change], created: Option<BlockKind>) -> (Vec<ItemKey>, Vec<String>) {
    let mut items: Vec<ItemKey> = before.items.iter().map(|i| item_key(before, i)).collect();
    for c in changes {
        let block = c.block.name();
        items.retain(|k| !(k.0 == block && k.1 == c.key));
        if !c.values.is_empty() {
            items.push((block, c.key.clone(), c.values.clone(), None, None));
        }
    }
    let mut blocks: Vec<String> = before.blocks.iter().map(|b| b.name()).collect();
    if let Some(b) = created {
        blocks.push(b.name());
    }
    // A block left with nothing in it is gone (lofty does not write an empty tag), except
    // Vorbis comments, whose vendor string always stays.
    blocks.retain(|b| items.iter().any(|k| &k.0 == b));
    (items, blocks)
}

/// The front cover, as `meta` tells it: an MP4 `covr` image (they carry no type), or a
/// picture typed as the front cover.
pub(super) fn is_front(tags: &FileTags, p: &meta::PictureMeta) -> bool {
    p.front || (tags.blocks.get(p.block) == Some(&BlockKind::Mp4))
}

/// Check the written file at `after` against the original.
pub(super) fn verify(before: &Before, after: &Path, container: Container, changes: &[Change], pictures: Option<&PictureEdit>, created: Option<BlockKind>) -> Result<(), String> {
    let now = meta::read(after, ReadOptions::RAW).map_err(|x| format!("the written file cannot be read back ({x})"))?;
    // Tags.
    let (mut want, mut want_blocks) = expected(&before.tags, changes, created);
    // FLAC: lofty writes no comment block without items, so one left with only its vendor
    // string (an encoder's empty block) goes, vendor and all.
    let vorbis = BlockKind::Vorbis.name();
    let empty_comments = container == Container::Flac && want.iter().filter(|k| k.0 == vorbis).all(|k| k.1 == meta::VENDOR_KEY) && want.iter().any(|k| k.0 == vorbis);
    if empty_comments {
        want.retain(|k| k.0 != vorbis);
        want_blocks.retain(|b| *b != vorbis);
    }
    let got: Vec<ItemKey> = now.items.iter().map(|i| item_key(&now, i)).collect();
    let (w, g) = (counts(want.iter().cloned()), counts(got.iter().cloned()));
    if let Some((k, _)) = w.iter().find(|(k, n)| g.get(*k) != Some(n)) {
        let same_key = got.iter().find(|x| x.0 == k.0 && x.1 == k.1);
        return Err(match same_key {
            Some(x) => format!("{} would read back as \u{201c}{}\u{201d}", show_item(k), x.2.join("; ")),
            None => format!("{} would be lost", show_item(k)),
        });
    }
    if let Some((k, _)) = g.iter().find(|(k, n)| w.get(*k) != Some(n)) {
        return Err(format!("{} would appear", show_item(k)));
    }
    let mut got_blocks: Vec<String> = now.blocks.iter().map(|b| b.name()).collect();
    got_blocks.retain(|b| now.items.iter().any(|i| now.blocks.get(i.block).map(|x| x.name()).as_ref() == Some(b)));
    let (mut a, mut b) = (want_blocks.clone(), got_blocks.clone());
    a.sort();
    b.sort();
    if a != b {
        return Err(format!("the tag blocks would be {} instead of {}", b.join(", "), a.join(", ")));
    }
    // Pictures.
    let mut want_pics: Vec<PicKey> = before.tags.pictures.iter().filter(|p| !(pictures.is_some_and(|pe| pe.remove_front) && is_front(&before.tags, p))).map(|p| pic_key(&before.tags, p)).collect();
    if let Some(pe) = pictures {
        let main = picture_block(container, &before.tags, created);
        for a in &pe.add {
            // MP4 pictures carry no type: they read back as "other".
            let ty = if main == BlockKind::Mp4 { 0 } else { a.type_code };
            want_pics.push((main.name(), ty, a.mime.to_ascii_lowercase(), if main == BlockKind::Mp4 { String::new() } else { a.description.clone() }, crate::hash::of(&a.data)));
        }
    }
    let got_pics: Vec<PicKey> = now.pictures.iter().map(|p| pic_key(&now, p)).collect();
    if counts(want_pics.iter().cloned()) != counts(got_pics.iter().cloned()) {
        return Err(format!("the pictures would not be as intended ({} instead of {})", got_pics.len(), want_pics.len()));
    }
    // The file's own tag items, apart from lofty's reader.
    let after_raw = raw::view(after, container.shape())?;
    let mut touched = touched(changes, pictures, container);
    if empty_comments {
        touched.push(("Vorbis", "(vendor)".into()));
    }
    let keep = |inv: &raw::Inventory| -> BTreeMap<raw::Entry, usize> { counts(inv.entries.iter().filter(|x| !touched.iter().any(|(b, id)| *b == x.block && *id == x.id)).cloned()) };
    let (x, y) = (keep(&before.raw.inventory), keep(&after_raw.inventory));
    if x != y {
        let lost = x.iter().find(|(k, n)| y.get(*k) != Some(n)).map(|(k, _)| format!("{} {} would be lost or changed", k.block, k.id));
        let extra = y.iter().find(|(k, n)| x.get(*k) != Some(n)).map(|(k, _)| format!("{} {} would appear or change", k.block, k.id));
        return Err(lost.or(extra).unwrap_or_else(|| "the tags would change beyond the edit".into()));
    }
    let created_id3 = matches!(created, Some(BlockKind::Id3v2(_)));
    match (before.raw.inventory.id3v2, after_raw.inventory.id3v2) {
        (Some((v, _)), Some((w, _))) if v != w => return Err(format!("the ID3v2 tag would change from version 2.{v} to 2.{w}")),
        (Some((_, p)), Some((_, q))) if p != q => return Err(format!("the ID3v2 tag's padding would change from {p} to {q} bytes")),
        (None, Some(_)) if !created_id3 => return Err("an ID3v2 tag would appear".into()),
        // An ID3v2 tag left empty is removed, as lofty writes no empty tag.
        (Some(_), None) if !want_blocks.iter().any(|b| b.starts_with("ID3v2")) && !want_pics.iter().any(|p| p.0.starts_with("ID3v2")) => {}
        (Some(_), None) => return Err("the ID3v2 tag would be lost".into()),
        _ => {}
    }
    if before.raw.inventory.flac_padding != after_raw.inventory.flac_padding {
        return Err(format!("FLAC padding would change from {:?} to {:?}", before.raw.inventory.flac_padding, after_raw.inventory.flac_padding));
    }
    // The audio.
    if before.raw.layout != after_raw.layout {
        let what = before.raw.layout.parts.iter().zip(&after_raw.layout.parts).find(|(a, b)| a != b).map(|(a, _)| a.0.clone()).unwrap_or_else(|| "its layout".into());
        return Err(format!("the audio data would change ({what})"));
    }
    let now_audio = audio(after, &before.path)?;
    if now_audio.props != before.audio.props {
        return Err(format!("the audio properties would change ({} instead of {})", now_audio.props, before.audio.props));
    }
    if now_audio != before.audio {
        return Err(format!("the audio packets would change ({} packets instead of {})", now_audio.packets, before.audio.packets));
    }
    Ok(())
}

/// The inventory entries a write may change: (block, id) in `raw`'s terms.
fn touched(changes: &[Change], pictures: Option<&PictureEdit>, container: Container) -> Vec<(&'static str, String)> {
    let mut t = Vec::new();
    for c in changes {
        let (b, id): (&'static str, String) = match c.block {
            BlockKind::Id3v2(_) => ("ID3v2", c.key.split(':').next().unwrap_or("").to_owned()),
            BlockKind::Vorbis => ("Vorbis", c.key.to_ascii_uppercase()),
            BlockKind::Mp4 => ("MP4", c.key.clone()),
            BlockKind::Ape => ("APE", c.key.to_ascii_uppercase()),
            BlockKind::RiffInfo => ("RIFF INFO", c.key.clone()),
            BlockKind::AiffText => ("AIFF text", c.key.clone()),
            BlockKind::Id3v1 => ("ID3v1", "ID3v1".into()),
        };
        t.push((b, id));
    }
    if pictures.is_some() {
        match container {
            Container::Flac => t.push(("Vorbis", "PICTURE".into())),
            Container::Opus | Container::Vorbis => t.push(("Vorbis", "METADATA_BLOCK_PICTURE".into())),
            Container::Mp4 => t.push(("MP4", "covr".into())),
            _ => t.push(("ID3v2", "APIC".into())),
        }
    }
    t
}

/// A hash of a file's tag items as `meta::read` gives them (any mode, the same mode both
/// times): whether the tags changed since a draft was made from them.
pub fn items_digest(tags: &FileTags) -> u64 {
    let mut h = Hasher::new();
    for i in &tags.items {
        h.update(tags.blocks.get(i.block).map(|b| b.name()).unwrap_or_default().as_bytes());
        h.number(0);
        h.update(i.key.as_bytes());
        for v in &i.values {
            h.number(v.len() as u64);
            h.update(v.as_bytes());
        }
        h.number(i.binary.unwrap_or(usize::MAX) as u64);
        h.number(i.hash.unwrap_or(0));
    }
    h.finish()
}

/// The block pictures are written to: ID3v2 (the file's, else the one being made), the
/// Vorbis comments (FLAC's PICTURE blocks count with them), MP4's `covr`.
pub(super) fn picture_block(container: Container, tags: &FileTags, created: Option<BlockKind>) -> BlockKind {
    match container {
        Container::Flac | Container::Opus | Container::Vorbis => BlockKind::Vorbis,
        Container::Mp4 => BlockKind::Mp4,
        _ => tags.blocks.iter().copied().find(|b| matches!(b, BlockKind::Id3v2(_))).or(created.filter(|b| matches!(b, BlockKind::Id3v2(_)))).unwrap_or(BlockKind::Id3v2(3)),
    }
}
