//! Everything a file's tags hold, for the metadata viewer (app `metadata/view.rs`): every item
//! of every tag block in the file (ID3v2 and ID3v1 and APE in one MP3, ID3v2 beside the
//! Vorbis comments of a FLAC...), its embedded pictures, and a field model over them.
//!
//! Read-only. Files are opened only by `std::fs::File::open` (read access, shared with
//! other programs) and parsed by lofty's readers; nothing here writes, renames or touches
//! a file's times. `tests::nothing_here_can_write` checks this file names no writing API.
//!
//! The field model (`fields`) is what an editor would later work on: a `Field` is one
//! thing a person would call by name ("Album artist"), and each of its `Source`s says which
//! tag block holds it, under which raw key, with which values. A field the app does not
//! know keeps its raw key as its name, so nothing in the file is hidden.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;

/// A value longer than this is kept cut to it (a tag can hold megabytes of text).
pub const MAX_VALUE_CHARS: usize = 1 << 20;
/// Items beyond this many are counted, not kept.
pub const MAX_ITEMS: usize = 20_000;
/// Pictures larger than this are listed without a thumbnail (not decoded).
pub const MAX_THUMB_SOURCE_BYTES: usize = 48 << 20;
/// The thumbnail's longest side, in pixels.
pub const THUMB_PX: u32 = 128;

/// A tag block in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BlockKind {
    /// ID3v2.2, .3 or .4 (the minor version).
    Id3v2(u8),
    Id3v1,
    Ape,
    Vorbis,
    Mp4,
    RiffInfo,
    AiffText,
}

impl BlockKind {
    pub fn name(self) -> String {
        match self {
            Self::Id3v2(v) => format!("ID3v2.{v}"),
            Self::Id3v1 => "ID3v1".into(),
            Self::Ape => "APEv2".into(),
            Self::Vorbis => "Vorbis comments".into(),
            Self::Mp4 => "MP4 metadata".into(),
            Self::RiffInfo => "RIFF INFO".into(),
            Self::AiffText => "AIFF text".into(),
        }
    }
}

/// One item as the file has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawItem {
    /// Index into `FileTags::blocks`.
    pub block: usize,
    /// The key in the block's own terms: "TPE1", "TXXX:MusicBrainz Album Id", "ARTIST",
    /// "©ART", "----:com.apple.iTunes:ISRC", "COMM:iTunNORM".
    pub key: String,
    /// Its values, one per value the file holds (multi-valued items have several).
    pub values: Vec<String>,
    /// Bytes of data that is not text (shown as its size).
    pub binary: Option<usize>,
    /// The hash of that data (`crate::hash::of`), to tell whether it changed.
    pub hash: Option<u64>,
}

/// A small RGBA image, decoded off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PictureMeta {
    pub block: usize,
    /// "Front cover", "Back cover", "Artist"...
    pub kind: String,
    pub front: bool,
    /// "JPEG", "PNG"... (from the data when the tag does not say).
    pub format: String,
    pub description: String,
    pub bytes: usize,
    /// Pixel size, when the image could be read.
    pub size: Option<(u32, u32)>,
    pub thumb: Option<Thumb>,
    /// The picture type's number (ID3v2's APIC types; 3 is the front cover).
    pub type_code: u8,
    /// The MIME type as the tag gives it ("" when it gives none).
    pub mime: String,
    /// The hash of the image data (`crate::hash::of`), to tell whether it changed.
    pub hash: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileTags {
    pub blocks: Vec<BlockKind>,
    pub items: Vec<RawItem>,
    pub pictures: Vec<PictureMeta>,
    /// Items left out past `MAX_ITEMS`.
    pub dropped: usize,
}

/// How to read.
#[derive(Clone, Copy, Debug)]
pub struct ReadOptions {
    /// List embedded pictures (and decode thumbnails of them).
    pub pictures: bool,
    pub thumbnails: bool,
    /// Items exactly as the file holds them: lofty's implicit conversions off (ID3v2.3's
    /// TYER, TDAT and TIME stay three frames instead of one TDRC; MP4's `gnre` stays
    /// `gnre`). What the tag editor plans and checks its writes against.
    pub raw: bool,
}

impl ReadOptions {
    pub const ALL: Self = Self { pictures: true, thumbnails: true, raw: false };
    /// Tags only: pictures are not parsed at all (an album's tracks, read for its table).
    pub const TAGS: Self = Self { pictures: false, thumbnails: false, raw: false };
    /// Every item and picture as stored, no thumbnails (the tag editor).
    pub const RAW: Self = Self { pictures: true, thumbnails: false, raw: true };
}

/// Read every tag block of a file.
pub fn read(path: &Path, opts: ReadOptions) -> Result<FileTags, String> {
    crate::remote::refuse(path)?;
    // A panic in a tag reader or a picture decoder on a damaged file is that file's error (`guard`).
    crate::guard::catching(|| read_unguarded(path, opts)).unwrap_or_else(|what| Err(format!("the file's tags could not be read ({what})")))
}

fn read_unguarded(path: &Path, opts: ReadOptions) -> Result<FileTags, String> {
    let options = ParseOptions::new().read_cover_art(opts.pictures).read_properties(false).implicit_conversions(!opts.raw);
    // `Probe::open` opens the file for reading only (`File::open`).
    let probe = Probe::open(path).map_err(|e| e.to_string())?;
    let probe = if probe.file_type().is_none() { probe.guess_file_type().map_err(|e| e.to_string())? } else { probe };
    let file_type = probe.file_type();
    let mut reader = probe.into_inner();
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut out = Reader { tags: FileTags::default(), opts };
    let e = |e: lofty::error::FileParseError| e.to_string();
    match file_type {
        Some(FileType::Flac) => {
            use lofty::ogg::OggPictureStorage;
            let f = lofty::flac::FlacFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
            if let Some(vc) = f.vorbis_comments() {
                out.vorbis(vc);
            }
            if !f.pictures().is_empty() {
                // FLAC's own PICTURE blocks: counted with the Vorbis comments' block.
                let b = out.block_of(BlockKind::Vorbis);
                for (p, _) in f.pictures() {
                    out.picture(b, p);
                }
            }
        }
        Some(FileType::Opus) => {
            let f = lofty::ogg::OpusFile::read_from(&mut reader, options).map_err(e)?;
            out.vorbis(f.vorbis_comments());
        }
        Some(FileType::Vorbis) => {
            let f = lofty::ogg::VorbisFile::read_from(&mut reader, options).map_err(e)?;
            out.vorbis(f.vorbis_comments());
        }
        Some(FileType::Speex) => {
            let f = lofty::ogg::SpeexFile::read_from(&mut reader, options).map_err(e)?;
            out.vorbis(f.vorbis_comments());
        }
        Some(FileType::Mpeg) => {
            let f = lofty::mpeg::MpegFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
            if let Some(t) = f.ape() {
                out.ape(t);
            }
            if let Some(t) = f.id3v1() {
                out.id3v1(t);
            }
        }
        Some(FileType::Aac) => {
            let f = lofty::aac::AacFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
            if let Some(t) = f.id3v1() {
                out.id3v1(t);
            }
        }
        Some(FileType::Mp4) => {
            let f = lofty::mp4::Mp4File::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.ilst() {
                out.mp4(t);
            }
        }
        Some(FileType::Wav) => {
            let f = lofty::iff::wav::WavFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.riff_info() {
                out.riff(t);
            }
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
        }
        Some(FileType::Aiff) => {
            let f = lofty::iff::aiff::AiffFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.text_chunks() {
                out.aiff(t);
            }
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
        }
        Some(FileType::Ape) => {
            let f = lofty::ape::ApeFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.ape() {
                out.ape(t);
            }
            if let Some(t) = f.id3v2() {
                out.id3v2(t);
            }
            if let Some(t) = f.id3v1() {
                out.id3v1(t);
            }
        }
        Some(FileType::WavPack) => {
            let f = lofty::wavpack::WavPackFile::read_from(&mut reader, options).map_err(e)?;
            if let Some(t) = f.ape() {
                out.ape(t);
            }
            if let Some(t) = f.id3v1() {
                out.id3v1(t);
            }
        }
        _ => return Err("not a format whose tags can be read".into()),
    }
    Ok(out.tags)
}

struct Reader {
    tags: FileTags,
    opts: ReadOptions,
}

/// A value as kept: at most `MAX_VALUE_CHARS` characters.
fn cap(s: &str) -> String {
    match s.char_indices().nth(MAX_VALUE_CHARS) {
        Some((at, _)) => s[..at].to_owned(),
        None => s.to_owned(),
    }
}

/// ID3v2.4 keeps several values in one frame, separated by NUL.
fn split_nul(s: &str) -> Vec<String> {
    s.split('\0').filter(|v| !v.is_empty()).map(cap).collect()
}

/// An MP4 four-character code as text: its bytes as Latin-1, so 0xA9 reads "©".
pub fn fourcc_text(code: &[u8; 4]) -> String {
    code.iter().map(|&b| b as char).collect()
}

impl Reader {
    fn block_of(&mut self, kind: BlockKind) -> usize {
        if let Some(i) = self.tags.blocks.iter().position(|b| *b == kind) {
            return i;
        }
        self.tags.blocks.push(kind);
        self.tags.blocks.len() - 1
    }

    fn push(&mut self, block: usize, key: String, values: Vec<String>, binary: Option<usize>) {
        self.push_data(block, key, values, binary.map(|n| (n, None)));
    }

    /// An item with binary data: its size and, when the data is at hand, its hash.
    fn push_data(&mut self, block: usize, key: String, values: Vec<String>, binary: Option<(usize, Option<u64>)>) {
        if values.is_empty() && binary.is_none() {
            return;
        }
        if self.tags.items.len() >= MAX_ITEMS {
            self.tags.dropped += 1;
            return;
        }
        let (binary, hash) = match binary {
            Some((n, h)) => (Some(n), h),
            None => (None, None),
        };
        self.tags.items.push(RawItem { block, key, values, binary, hash });
    }

    fn push_bytes(&mut self, block: usize, key: String, data: &[u8]) {
        self.push_data(block, key, Vec::new(), Some((data.len(), Some(crate::hash::of(data)))));
    }

    /// Several items of one key (Vorbis comments repeat a key per value) become one.
    fn push_value(&mut self, block: usize, key: &str, value: &str) {
        if let Some(it) = self.tags.items.iter_mut().rev().find(|i| i.block == block && i.key == key) {
            it.values.push(cap(value));
            return;
        }
        self.push(block, key.to_owned(), vec![cap(value)], None);
    }

    fn picture(&mut self, block: usize, p: &Picture) {
        if !self.opts.pictures {
            return;
        }
        let data = p.data();
        let format = p
            .mime_type()
            .map(|m| mime_name(m.as_str()))
            .filter(|f| !f.is_empty())
            .or_else(|| image::guess_format(data).ok().map(|f| format!("{f:?}").to_uppercase()))
            .unwrap_or_else(|| "Unknown".into());
        let size = image_size(data);
        let thumb = (self.opts.thumbnails && data.len() <= MAX_THUMB_SOURCE_BYTES).then(|| thumbnail(data)).flatten();
        // MP4 `covr` pictures carry no type: they are the cover.
        let cover = self.tags.blocks[block] == BlockKind::Mp4 && p.pic_type() == PictureType::Other;
        self.tags.pictures.push(PictureMeta {
            block,
            kind: if cover { "Cover".to_owned() } else { picture_kind(p.pic_type()) },
            front: cover || p.pic_type() == PictureType::CoverFront,
            format,
            description: p.description().unwrap_or("").trim().to_owned(),
            bytes: data.len(),
            size,
            thumb,
            type_code: p.pic_type().as_u8(),
            mime: p.mime_type().map(|m| m.as_str().to_owned()).unwrap_or_default(),
            hash: crate::hash::of(data),
        });
    }

    fn vorbis(&mut self, vc: &lofty::ogg::tag::VorbisComments) {
        use lofty::ogg::OggPictureStorage;
        let b = self.block_of(BlockKind::Vorbis);
        let vendor = vc.vendor().trim();
        if !vendor.is_empty() {
            self.push(b, VENDOR_KEY.to_owned(), vec![cap(vendor)], None);
        }
        for (k, v) in vc.items() {
            self.push_value(b, k, v);
        }
        for (p, _) in vc.pictures() {
            self.picture(b, p);
        }
    }

    fn id3v2(&mut self, tag: &lofty::id3::v2::Id3v2Tag) {
        use lofty::id3::v2::{Frame, Id3v2Version};
        let v = match tag.original_version() {
            Id3v2Version::V2 => 2,
            Id3v2Version::V3 => 3,
            Id3v2Version::V4 => 4,
        };
        let b = self.block_of(BlockKind::Id3v2(v));
        for frame in tag {
            let id = frame.id_str().to_owned();
            match frame {
                Frame::Text(f) => self.push(b, id, split_nul(&f.value), None),
                Frame::UserText(f) => self.push(b, format!("TXXX:{}", f.description), split_nul(&f.content), None),
                Frame::Comment(f) => {
                    let key = if f.description.is_empty() { "COMM".to_owned() } else { format!("COMM:{}", f.description) };
                    self.push(b, key, vec![cap(&f.content)], None);
                }
                Frame::UnsynchronizedText(f) => {
                    let key = if f.description.is_empty() { "USLT".to_owned() } else { format!("USLT:{}", f.description) };
                    self.push(b, key, vec![cap(&f.content)], None);
                }
                Frame::Url(f) => self.push(b, id, vec![cap(f.url())], None),
                Frame::UserUrl(f) => self.push(b, format!("WXXX:{}", f.description), vec![cap(&f.content)], None),
                Frame::Picture(f) => self.picture(b, &f.picture),
                Frame::Popularimeter(f) => {
                    let key = if f.email.is_empty() { "POPM".to_owned() } else { format!("POPM:{}", f.email) };
                    self.push(b, key, vec![format!("{} of 255, played {} times", f.rating, f.counter)], None);
                }
                Frame::KeyValue(f) => {
                    let values = f.key_value_pairs.iter().map(|(k, v)| if k.is_empty() { cap(v) } else { format!("{k}: {v}") }).collect();
                    self.push(b, id, values, None);
                }
                Frame::UniqueFileIdentifier(f) => {
                    let value = String::from_utf8_lossy(&f.identifier).trim_end_matches('\0').to_owned();
                    self.push(b, format!("UFID:{}", f.owner), vec![value], None);
                }
                Frame::Timestamp(f) => self.push(b, id, vec![f.timestamp.to_string()], None),
                Frame::Private(f) => self.push_bytes(b, format!("PRIV:{}", f.owner), &f.private_data),
                Frame::Binary(f) => self.push_bytes(b, id, &f.data),
                _ => self.push(b, id, Vec::new(), Some(0)),
            }
        }
    }

    fn id3v1(&mut self, tag: &lofty::id3::v1::Id3v1Tag) {
        let b = self.block_of(BlockKind::Id3v1);
        let text = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| vec![s.to_owned()]).unwrap_or_default();
        self.push(b, "Title".into(), text(&tag.title), None);
        self.push(b, "Artist".into(), text(&tag.artist), None);
        self.push(b, "Album".into(), text(&tag.album), None);
        self.push(b, "Year".into(), tag.year.map(|y| vec![y.to_string()]).unwrap_or_default(), None);
        self.push(b, "Comment".into(), text(&tag.comment), None);
        self.push(b, "Track".into(), tag.track_number.filter(|n| *n > 0).map(|n| vec![n.to_string()]).unwrap_or_default(), None);
        let genre = tag.genre.filter(|g| *g != 255).map(|g| lofty::id3::v1::GENRES.get(usize::from(g)).map_or_else(|| format!("Genre {g}"), |n| (*n).to_owned()));
        self.push(b, "Genre".into(), genre.map(|g| vec![g]).unwrap_or_default(), None);
    }

    fn ape(&mut self, tag: &lofty::ape::ApeTag) {
        use lofty::tag::ItemValue;
        let b = self.block_of(BlockKind::Ape);
        for item in tag {
            let key = item.key().to_owned();
            match item.value() {
                ItemValue::Text(s) | ItemValue::Locator(s) => self.push(b, key, split_nul(s), None),
                ItemValue::Binary(bytes) => {
                    if key.to_ascii_lowercase().starts_with("cover art") {
                        match Picture::from_ape_bytes(&key, bytes) {
                            Ok(p) => self.picture(b, &p),
                            Err(_) => self.push_bytes(b, key, bytes),
                        }
                    } else {
                        self.push_bytes(b, key, bytes);
                    }
                }
            }
        }
    }

    fn mp4(&mut self, ilst: &lofty::mp4::Ilst) {
        use lofty::mp4::{AtomData, AtomIdent};
        let b = self.block_of(BlockKind::Mp4);
        for atom in ilst {
            let key = match atom.ident() {
                AtomIdent::Fourcc(c) => fourcc_text(c),
                AtomIdent::Freeform { mean, name } => format!("----:{mean}:{name}"),
            };
            let mut values = Vec::new();
            let mut binary = None;
            let mut hasher = crate::hash::Hasher::new();
            for d in atom.data() {
                match d {
                    AtomData::UTF8(s) | AtomData::UTF16(s) => values.push(cap(s)),
                    AtomData::Picture(p) => self.picture(b, p),
                    AtomData::SignedInteger(n) => values.push(n.to_string()),
                    AtomData::UnsignedInteger(n) => values.push(n.to_string()),
                    AtomData::Bool(v) => values.push(if *v { "Yes" } else { "No" }.into()),
                    AtomData::Unknown { data, .. } => match (key.as_str(), data.len()) {
                        // Track and disc: two big-endian u16 after two reserved bytes.
                        ("trkn" | "disk", n) if n >= 6 => {
                            let no = u16::from_be_bytes([data[2], data[3]]);
                            let total = u16::from_be_bytes([data[4], data[5]]);
                            values.push(if total > 0 { format!("{no}/{total}") } else { no.to_string() });
                        }
                        // A genre by its ID3v1 number, plus one.
                        ("gnre", 2) => {
                            let g = u16::from_be_bytes([data[0], data[1]]);
                            let name = (g as usize).checked_sub(1).and_then(|i| lofty::id3::v1::GENRES.get(i));
                            values.push(name.map_or_else(|| format!("Genre {g}"), |n| (*n).to_owned()));
                        }
                        _ => {
                            binary = Some(binary.unwrap_or(0) + data.len());
                            hasher.update(data);
                        }
                    },
                }
            }
            if !values.is_empty() || binary.is_some() {
                self.push_data(b, key, values, binary.map(|n| (n, Some(hasher.finish()))));
            }
        }
    }

    fn riff(&mut self, tag: &lofty::iff::wav::RiffInfoList) {
        let b = self.block_of(BlockKind::RiffInfo);
        for (k, v) in tag {
            self.push_value(b, k, v.trim_end_matches('\0'));
        }
    }

    fn aiff(&mut self, tag: &lofty::iff::aiff::AiffTextChunks) {
        let b = self.block_of(BlockKind::AiffText);
        let one = |s: &Option<String>| s.as_deref().map(|s| vec![cap(s)]).unwrap_or_default();
        self.push(b, "NAME".into(), one(&tag.name), None);
        self.push(b, "AUTH".into(), one(&tag.author), None);
        self.push(b, "(c) ".into(), one(&tag.copyright), None);
        if let Some(a) = &tag.annotations {
            self.push(b, "ANNO".into(), a.iter().map(|s| cap(s)).collect(), None);
        }
        if let Some(c) = &tag.comments {
            self.push(b, "COMT".into(), c.iter().map(|c| cap(&c.text)).collect(), None);
        }
    }
}

/// "image/jpeg" -> "JPEG".
fn mime_name(mime: &str) -> String {
    let m = mime.trim().to_ascii_lowercase();
    match m.as_str() {
        "image/jpeg" | "image/jpg" => "JPEG".into(),
        "image/png" => "PNG".into(),
        "image/gif" => "GIF".into(),
        "image/bmp" => "BMP".into(),
        "image/webp" => "WebP".into(),
        "image/tiff" => "TIFF".into(),
        "" => String::new(),
        other => other.strip_prefix("image/").unwrap_or(other).to_uppercase(),
    }
}

/// ID3v2's picture types, as a person names them.
pub fn picture_kind(t: PictureType) -> String {
    match t {
        PictureType::Other => "Other picture",
        PictureType::Icon => "File icon",
        PictureType::OtherIcon => "Other icon",
        PictureType::CoverFront => "Front cover",
        PictureType::CoverBack => "Back cover",
        PictureType::Leaflet => "Leaflet page",
        PictureType::Media => "Media (the disc itself)",
        PictureType::LeadArtist => "Lead artist",
        PictureType::Artist => "Artist",
        PictureType::Conductor => "Conductor",
        PictureType::Band => "Band",
        PictureType::Composer => "Composer",
        PictureType::Lyricist => "Lyricist",
        PictureType::RecordingLocation => "Recording location",
        PictureType::DuringRecording => "During recording",
        PictureType::DuringPerformance => "During performance",
        PictureType::ScreenCapture => "Screen capture",
        PictureType::BrightFish => "A bright coloured fish",
        PictureType::Illustration => "Illustration",
        PictureType::BandLogo => "Band logo",
        PictureType::PublisherLogo => "Publisher logo",
        _ => "Picture",
    }
    .to_owned()
}

/// The pixel size of an encoded image, from its header (nothing decoded).
pub fn image_size(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// A thumbnail of an encoded image (`THUMB_PX` on its longest side), or None when it
/// cannot be decoded. The full-size image is dropped as soon as it is made.
pub fn thumbnail(data: &[u8]) -> Option<Thumb> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let t = img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
    Some(Thumb { width: t.width(), height: t.height(), rgba: t.into_raw() })
}

/// An image file (an album folder's cover): its pixel size, bytes and a thumbnail.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageFile {
    pub size: Option<(u32, u32)>,
    pub bytes: u64,
    pub thumb: Option<Thumb>,
}

pub fn image_file(path: &Path, want_thumb: bool) -> Result<ImageFile, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let size = image::image_dimensions(path).ok();
    let thumb = if want_thumb && len as usize <= MAX_THUMB_SOURCE_BYTES {
        let mut bytes = Vec::with_capacity(len as usize);
        std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
        thumbnail(&bytes)
    } else {
        None
    };
    Ok(ImageFile { size, bytes: len, thumb })
}

// ---- The field model ----------------------------------------------------------------

/// The pseudo-key of a Vorbis comment block's vendor string (not an item of its own).
pub const VENDOR_KEY: &str = "(vendor)";

/// Where a field is shown, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Main,
    Ids,
    Sort,
    Loudness,
    Encoding,
    Lyrics,
    Other,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Self::Main => "Tags",
            Self::Ids => "Identifiers",
            Self::Sort => "Sort names",
            Self::Loudness => "Loudness",
            Self::Encoding => "Encoding",
            Self::Lyrics => "Lyrics",
            Self::Other => "Other tags",
        }
    }
}

/// A field the app knows by name: its canonical key (Picard's Vorbis name), what a person
/// calls it, its group, and the other names it goes by in Vorbis comments, APE items,
/// ID3v2 TXXX descriptions and MP4 freeform names (compared without case, spaces or
/// punctuation, so "MusicBrainz Album Id" is MUSICBRAINZ_ALBUMID).
#[derive(Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub aliases: &'static [&'static str],
}

const fn def(key: &'static str, label: &'static str, group: Group, aliases: &'static [&'static str]) -> FieldDef {
    FieldDef { key, label, group, aliases }
}

use Group::{Encoding, Ids, Loudness, Lyrics, Main, Sort};

/// Every field the app names, in the order they are shown.
pub static FIELDS: &[FieldDef] = &[
    def("TITLE", "Title", Main, &[]),
    def("ARTIST", "Artist", Main, &[]),
    def("ARTISTS", "Artists", Main, &[]),
    def("ALBUM", "Album", Main, &[]),
    def("ALBUMARTIST", "Album artist", Main, &["ALBUM ARTIST"]),
    def("TRACKNUMBER", "Track", Main, &["TRACK"]),
    def("TRACKTOTAL", "Total tracks", Main, &["TOTALTRACKS"]),
    def("DISCNUMBER", "Disc", Main, &["DISC"]),
    def("DISCTOTAL", "Total discs", Main, &["TOTALDISCS"]),
    def("DISCSUBTITLE", "Disc subtitle", Main, &["SETSUBTITLE"]),
    def("DATE", "Date", Main, &[]),
    def("ORIGINALDATE", "Original date", Main, &[]),
    def("ORIGINALYEAR", "Original year", Main, &[]),
    def("RELEASEDATE", "Release date", Main, &[]),
    def("YEAR", "Year", Main, &[]),
    def("GENRE", "Genre", Main, &[]),
    def("COMPOSER", "Composer", Main, &[]),
    def("LYRICIST", "Lyricist", Main, &["WRITER"]),
    def("CONDUCTOR", "Conductor", Main, &[]),
    def("PERFORMER", "Performer", Main, &[]),
    def("REMIXER", "Remixer", Main, &["MIXARTIST"]),
    def("PRODUCER", "Producer", Main, &[]),
    def("ENGINEER", "Engineer", Main, &[]),
    def("MIXER", "Mixer", Main, &[]),
    def("ARRANGER", "Arranger", Main, &[]),
    def("LABEL", "Label", Main, &["PUBLISHER", "ORGANIZATION"]),
    def("CATALOGNUMBER", "Catalogue number", Main, &[]),
    def("RELEASETYPE", "Release type", Main, &["MUSICBRAINZ_ALBUMTYPE"]),
    def("RELEASESTATUS", "Release status", Main, &["MUSICBRAINZ_ALBUMSTATUS"]),
    def("RELEASECOUNTRY", "Release country", Main, &["MUSICBRAINZ_ALBUMRELEASECOUNTRY"]),
    def("MEDIA", "Media", Main, &[]),
    def("SCRIPT", "Script", Main, &[]),
    def("LANGUAGE", "Language", Main, &[]),
    def("GROUPING", "Grouping", Main, &[]),
    def("SUBTITLE", "Subtitle", Main, &[]),
    def("WORK", "Work", Main, &[]),
    def("COMPILATION", "Compilation", Main, &["ITUNESCOMPILATION"]),
    def("BPM", "BPM", Main, &["TEMPO"]),
    def("KEY", "Key", Main, &["INITIALKEY"]),
    def("MOOD", "Mood", Main, &[]),
    def("RATING", "Rating", Main, &[]),
    def("COPYRIGHT", "Copyright", Main, &[]),
    def("COMMENT", "Comment", Main, &["DESCRIPTION"]),
    def("ISRC", "ISRC", Ids, &[]),
    def("BARCODE", "Barcode", Ids, &["UPC", "EAN"]),
    def("ASIN", "ASIN", Ids, &[]),
    def("MUSICBRAINZ_TRACKID", "MusicBrainz recording", Ids, &["MUSICBRAINZ_RECORDINGID"]),
    def("MUSICBRAINZ_RELEASETRACKID", "MusicBrainz track", Ids, &[]),
    def("MUSICBRAINZ_ALBUMID", "MusicBrainz release", Ids, &["MUSICBRAINZ_RELEASEID"]),
    def("MUSICBRAINZ_RELEASEGROUPID", "MusicBrainz release group", Ids, &[]),
    def("MUSICBRAINZ_ARTISTID", "MusicBrainz artist", Ids, &[]),
    def("MUSICBRAINZ_ALBUMARTISTID", "MusicBrainz album artist", Ids, &[]),
    def("MUSICBRAINZ_WORKID", "MusicBrainz work", Ids, &[]),
    def("MUSICBRAINZ_DISCID", "MusicBrainz disc", Ids, &[]),
    def("ACOUSTID_ID", "AcoustID", Ids, &[]),
    def("ACOUSTID_FINGERPRINT", "AcoustID fingerprint", Ids, &[]),
    def("ARTISTSORT", "Artist sort", Sort, &[]),
    def("ALBUMARTISTSORT", "Album artist sort", Sort, &[]),
    def("ALBUMSORT", "Album sort", Sort, &[]),
    def("TITLESORT", "Title sort", Sort, &[]),
    def("COMPOSERSORT", "Composer sort", Sort, &[]),
    def("REPLAYGAIN_TRACK_GAIN", "ReplayGain track gain", Loudness, &[]),
    def("REPLAYGAIN_TRACK_PEAK", "ReplayGain track peak", Loudness, &[]),
    def("REPLAYGAIN_ALBUM_GAIN", "ReplayGain album gain", Loudness, &[]),
    def("REPLAYGAIN_ALBUM_PEAK", "ReplayGain album peak", Loudness, &[]),
    def("REPLAYGAIN_REFERENCE_LOUDNESS", "ReplayGain reference", Loudness, &[]),
    def("R128_TRACK_GAIN", "R128 track gain", Loudness, &[]),
    def("R128_ALBUM_GAIN", "R128 album gain", Loudness, &[]),
    def("ENCODER", "Encoder", Encoding, &[]),
    def("ENCODEDBY", "Encoded by", Encoding, &[]),
    def("ENCODERSETTINGS", "Encoder settings", Encoding, &["ENCODING"]),
    def("VENDOR", "Vendor string", Encoding, &[]),
    def("LYRICS", "Lyrics", Lyrics, &["UNSYNCEDLYRICS", "UNSYNCED LYRICS"]),
    def("SYNCEDLYRICS", "Synced lyrics", Lyrics, &[]),
];

/// A key compared without case, spaces or punctuation.
fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect()
}

/// The field a name stands for (Vorbis comments, APE, TXXX descriptions, MP4 freeform).
pub fn by_name(name: &str) -> Option<&'static FieldDef> {
    let n = norm(name);
    if n.is_empty() {
        return None;
    }
    FIELDS.iter().find(|f| norm(f.key) == n || f.aliases.iter().any(|a| norm(a) == n))
}

pub fn by_key(key: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.key == key)
}

/// ID3v2 frames the app names.
const ID3V2: &[(&str, &str)] = &[
    ("TIT2", "TITLE"),
    ("TPE1", "ARTIST"),
    ("TALB", "ALBUM"),
    ("TPE2", "ALBUMARTIST"),
    ("TRCK", "TRACKNUMBER"),
    ("TPOS", "DISCNUMBER"),
    ("TSST", "DISCSUBTITLE"),
    ("TDRC", "DATE"),
    ("TYER", "DATE"),
    ("TDOR", "ORIGINALDATE"),
    ("TORY", "ORIGINALYEAR"),
    ("TDRL", "RELEASEDATE"),
    ("TCON", "GENRE"),
    ("TCOM", "COMPOSER"),
    ("TEXT", "LYRICIST"),
    ("TPE3", "CONDUCTOR"),
    ("TPE4", "REMIXER"),
    ("TPUB", "LABEL"),
    ("TMED", "MEDIA"),
    ("TLAN", "LANGUAGE"),
    ("TIT1", "GROUPING"),
    ("GRP1", "GROUPING"),
    ("TIT3", "SUBTITLE"),
    ("TCMP", "COMPILATION"),
    ("TBPM", "BPM"),
    ("TKEY", "KEY"),
    ("TMOO", "MOOD"),
    ("TCOP", "COPYRIGHT"),
    ("COMM", "COMMENT"),
    ("TSRC", "ISRC"),
    ("UFID:http://musicbrainz.org", "MUSICBRAINZ_TRACKID"),
    ("TSOP", "ARTISTSORT"),
    ("TSO2", "ALBUMARTISTSORT"),
    ("TSOA", "ALBUMSORT"),
    ("TSOT", "TITLESORT"),
    ("TSOC", "COMPOSERSORT"),
    ("TSSE", "ENCODER"),
    ("TENC", "ENCODEDBY"),
    ("USLT", "LYRICS"),
    ("SYLT", "SYNCEDLYRICS"),
];

/// MP4 atoms the app names.
const MP4: &[(&str, &str)] = &[
    ("©nam", "TITLE"),
    ("©ART", "ARTIST"),
    ("©alb", "ALBUM"),
    ("aART", "ALBUMARTIST"),
    ("trkn", "TRACKNUMBER"),
    ("disk", "DISCNUMBER"),
    ("©day", "DATE"),
    ("©gen", "GENRE"),
    ("gnre", "GENRE"),
    ("©wrt", "COMPOSER"),
    ("©cmt", "COMMENT"),
    ("cprt", "COPYRIGHT"),
    ("tmpo", "BPM"),
    ("©grp", "GROUPING"),
    ("cpil", "COMPILATION"),
    ("soar", "ARTISTSORT"),
    ("soaa", "ALBUMARTISTSORT"),
    ("soal", "ALBUMSORT"),
    ("sonm", "TITLESORT"),
    ("soco", "COMPOSERSORT"),
    ("©too", "ENCODER"),
    ("©enc", "ENCODEDBY"),
    ("©lyr", "LYRICS"),
];

/// RIFF INFO chunks the app names.
const RIFF: &[(&str, &str)] = &[
    ("INAM", "TITLE"),
    ("IART", "ARTIST"),
    ("IPRD", "ALBUM"),
    ("ITRK", "TRACKNUMBER"),
    ("IPRT", "TRACKNUMBER"),
    ("ICRD", "DATE"),
    ("IGNR", "GENRE"),
    ("ICMT", "COMMENT"),
    ("ICOP", "COPYRIGHT"),
    ("ISFT", "ENCODER"),
    ("IENG", "ENCODEDBY"),
    ("IMUS", "COMPOSER"),
    ("IWRI", "LYRICIST"),
    ("ILNG", "LANGUAGE"),
    ("ISRC", "ISRC"),
];

/// The field a raw item is, in its block's terms.
pub fn field_of(block: BlockKind, key: &str) -> Option<&'static FieldDef> {
    let table = |t: &[(&str, &str)]| t.iter().find(|(k, _)| *k == key).and_then(|(_, f)| by_key(f));
    match block {
        BlockKind::Vorbis if key == VENDOR_KEY => by_key("VENDOR"),
        BlockKind::Vorbis | BlockKind::Ape => {
            // APE's "Year" holds the date.
            if block == BlockKind::Ape && key.eq_ignore_ascii_case("year") {
                return by_key("DATE");
            }
            by_name(key)
        }
        BlockKind::Id3v2(_) => {
            if let Some(desc) = key.strip_prefix("TXXX:") {
                return by_name(desc);
            }
            table(ID3V2)
        }
        BlockKind::Mp4 => {
            if let Some(rest) = key.strip_prefix("----:") {
                // ----:com.apple.iTunes:NAME
                let name = rest.split_once(':').map_or(rest, |(_, n)| n);
                return by_name(name);
            }
            table(MP4)
        }
        BlockKind::Id3v1 => match key {
            "Title" => by_key("TITLE"),
            "Artist" => by_key("ARTIST"),
            "Album" => by_key("ALBUM"),
            // ID3v1 has only the year: compared with the other blocks' date.
            "Year" => by_key("DATE"),
            "Comment" => by_key("COMMENT"),
            "Track" => by_key("TRACKNUMBER"),
            "Genre" => by_key("GENRE"),
            _ => None,
        },
        BlockKind::RiffInfo => table(RIFF),
        BlockKind::AiffText => match key {
            "NAME" => by_key("TITLE"),
            "AUTH" => by_key("ARTIST"),
            "(c) " => by_key("COPYRIGHT"),
            "ANNO" | "COMT" => by_key("COMMENT"),
            _ => None,
        },
    }
}

/// One block's say on a field.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub block: usize,
    pub key: String,
    pub values: Vec<String>,
    pub binary: Option<usize>,
}

/// One thing a person would call by name, with every block's say on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The field the app knows it as; None for an item shown under its raw key.
    pub def: Option<&'static FieldDef>,
    pub sources: Vec<Source>,
}

impl Field {
    /// What it is called: the friendly name, else the raw key.
    pub fn label(&self) -> String {
        match self.def {
            Some(d) => d.label.to_owned(),
            None => self.sources.first().map(|s| s.key.trim().to_owned()).unwrap_or_default(),
        }
    }

    /// A key that names the field whatever the format: the canonical key, else the raw one.
    pub fn key(&self) -> String {
        match self.def {
            Some(d) => d.key.to_owned(),
            None => self.sources.first().map(|s| s.key.clone()).unwrap_or_default(),
        }
    }

    pub fn group(&self) -> Group {
        self.def.map_or(Group::Other, |d| d.group)
    }

    /// The values of its first source (the block the app reads first).
    pub fn values(&self) -> &[String] {
        self.sources.first().map_or(&[], |s| s.values.as_slice())
    }

    /// The blocks disagree: they hold different values for it. A number without its total
    /// ("2" beside "2/12") and a year beside a full date of that year (ID3v1 has only the
    /// year) agree.
    pub fn differs(&self) -> bool {
        let key = self.def.map_or("", |d| d.key);
        let norm = |s: &Source| (s.values.iter().map(|v| v.trim().to_owned()).collect::<Vec<_>>(), s.binary);
        let same = |a: &Source, b: &Source| {
            let (va, vb) = (norm(a), norm(b));
            if va == vb {
                return true;
            }
            match (key, va.0.as_slice(), vb.0.as_slice()) {
                ("TRACKNUMBER" | "DISCNUMBER", [x], [y]) => {
                    let (nx, tx) = x.split_once('/').map_or((x.as_str(), None), |(n, t)| (n, Some(t)));
                    let (ny, ty) = y.split_once('/').map_or((y.as_str(), None), |(n, t)| (n, Some(t)));
                    nx.trim().trim_start_matches('0') == ny.trim().trim_start_matches('0') && (tx.is_none() || ty.is_none() || tx == ty)
                }
                ("DATE", [x], [y]) => (x.len() == 4 && y.starts_with(x.as_str())) || (y.len() == 4 && x.starts_with(y.as_str())),
                _ => false,
            }
        };
        self.sources.windows(2).any(|w| !same(&w[0], &w[1]))
    }
}

/// The fields of a file: known ones in their fixed order, then the rest by raw key in the
/// order the file has them. Items of one field from several blocks are one field with a
/// source per block.
pub fn fields(tags: &FileTags) -> Vec<Field> {
    let mut known: Vec<Field> = Vec::new();
    let mut other: Vec<Field> = Vec::new();
    for item in &tags.items {
        let kind = tags.blocks[item.block];
        let source = Source { block: item.block, key: item.key.clone(), values: item.values.clone(), binary: item.binary };
        match field_of(kind, &item.key) {
            Some(d) => match known.iter_mut().find(|f| f.def == Some(d)) {
                Some(f) => f.sources.push(source),
                None => known.push(Field { def: Some(d), sources: vec![source] }),
            },
            None => match other.iter_mut().find(|f| f.sources[0].key.eq_ignore_ascii_case(&item.key)) {
                Some(f) => f.sources.push(source),
                None => other.push(Field { def: None, sources: vec![source] }),
            },
        }
    }
    known.sort_by_key(|f| f.def.and_then(|d| FIELDS.iter().position(|x| std::ptr::eq(x, d))).unwrap_or(usize::MAX));
    known.extend(other);
    known
}

/// A value of a field as the app reads it: the first source's first value, trimmed.
pub fn first(fields: &[Field], key: &str) -> Option<String> {
    fields.iter().find(|f| f.def.is_some_and(|d| d.key == key)).and_then(|f| f.values().first()).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// What an album's track table needs of one file's tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brief {
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub mb_album: Option<String>,
    pub replaygain: bool,
}

pub fn brief(fields: &[Field]) -> Brief {
    let has = |k: &str| first(fields, k).is_some();
    Brief {
        album: first(fields, "ALBUM"),
        album_artist: first(fields, "ALBUMARTIST"),
        date: first(fields, "DATE").or_else(|| first(fields, "RELEASEDATE")).or_else(|| first(fields, "ORIGINALDATE")).or_else(|| first(fields, "YEAR")).map(|d| crate::tags::normalize_date(&d)),
        genre: first(fields, "GENRE"),
        mb_album: first(fields, "MUSICBRAINZ_ALBUMID").map(|s| s.to_ascii_lowercase()),
        replaygain: has("REPLAYGAIN_TRACK_GAIN") || has("REPLAYGAIN_ALBUM_GAIN") || has("R128_TRACK_GAIN") || has("R128_ALBUM_GAIN"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(block: usize, key: &str, values: &[&str]) -> RawItem {
        RawItem { block, key: key.into(), values: values.iter().map(|s| (*s).to_owned()).collect(), ..Default::default() }
    }

    #[test]
    fn names_in_every_format_reach_one_field() {
        assert_eq!(by_name("MusicBrainz Album Id").map(|f| f.key), Some("MUSICBRAINZ_ALBUMID"));
        assert_eq!(by_name("musicbrainz_albumid").map(|f| f.key), Some("MUSICBRAINZ_ALBUMID"));
        assert_eq!(by_name("Album Artist").map(|f| f.key), Some("ALBUMARTIST"));
        assert_eq!(by_name("Acoustid Id").map(|f| f.key), Some("ACOUSTID_ID"));
        assert_eq!(by_name("MusicBrainz Album Type").map(|f| f.key), Some("RELEASETYPE"));
        assert_eq!(by_name("ENCODED-BY").map(|f| f.key), Some("ENCODEDBY"));
        assert_eq!(by_name("UNSYNCEDLYRICS").map(|f| f.key), Some("LYRICS"));
        assert_eq!(by_name("WAVEFORMATEXTENSIBLE_CHANNEL_MASK"), None);
        assert_eq!(field_of(BlockKind::Id3v2(4), "TPE2").map(|f| f.key), Some("ALBUMARTIST"));
        assert_eq!(field_of(BlockKind::Id3v2(3), "TXXX:MusicBrainz Release Group Id").map(|f| f.key), Some("MUSICBRAINZ_RELEASEGROUPID"));
        assert_eq!(field_of(BlockKind::Id3v2(4), "UFID:http://musicbrainz.org").map(|f| f.key), Some("MUSICBRAINZ_TRACKID"));
        assert_eq!(field_of(BlockKind::Id3v2(4), "COMM:iTunNORM"), None, "a described comment is not the comment");
        assert_eq!(field_of(BlockKind::Mp4, "----:com.apple.iTunes:MusicBrainz Track Id").map(|f| f.key), Some("MUSICBRAINZ_TRACKID"));
        assert_eq!(field_of(BlockKind::Mp4, "©day").map(|f| f.key), Some("DATE"));
        assert_eq!(field_of(BlockKind::Ape, "Year").map(|f| f.key), Some("DATE"));
        assert_eq!(field_of(BlockKind::Vorbis, "YEAR").map(|f| f.key), Some("YEAR"));
        assert_eq!(field_of(BlockKind::Id3v1, "Genre").map(|f| f.key), Some("GENRE"));
        assert_eq!(field_of(BlockKind::Id3v1, "Year").map(|f| f.key), Some("DATE"));
        assert_eq!(field_of(BlockKind::RiffInfo, "IART").map(|f| f.key), Some("ARTIST"));
        assert_eq!(fourcc_text(&[0xA9, b'n', b'a', b'm']), "©nam");
    }

    #[test]
    fn fields_follow_the_fixed_order_and_keep_the_rest_by_raw_key() {
        let tags = FileTags {
            blocks: vec![BlockKind::Vorbis],
            items: vec![
                item(0, "MY_TAG", &["x"]),
                item(0, "GENRE", &["Rock", "Pop"]),
                item(0, "REPLAYGAIN_TRACK_GAIN", &["-6.20 dB"]),
                item(0, "TITLE", &["One"]),
                item(0, "ARTISTSORT", &["Band, The"]),
                item(0, "MUSICBRAINZ_ALBUMID", &["abc"]),
                item(0, "ALBUM", &["Al"]),
            ],
            ..Default::default()
        };
        let f = fields(&tags);
        let labels: Vec<String> = f.iter().map(Field::label).collect();
        assert_eq!(labels, ["Title", "Album", "Genre", "MusicBrainz release", "Artist sort", "ReplayGain track gain", "MY_TAG"]);
        assert_eq!(f[2].values(), ["Rock", "Pop"], "multi-valued: separate values");
        assert_eq!(f[6].group(), Group::Other);
        assert_eq!(f[6].key(), "MY_TAG");
    }

    #[test]
    fn blocks_that_disagree_are_one_field_with_a_source_each() {
        let tags = FileTags {
            blocks: vec![BlockKind::Id3v2(3), BlockKind::Id3v1],
            items: vec![item(0, "TIT2", &["A long title that ID3v1 cuts"]), item(0, "TPE1", &["Artist"]), item(1, "Title", &["A long title that ID3v1"]), item(1, "Artist", &["Artist"])],
            ..Default::default()
        };
        let f = fields(&tags);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].sources.len(), 2);
        assert!(f[0].differs());
        assert!(!f[1].differs(), "the same value in both blocks");
        let tags = FileTags {
            blocks: vec![BlockKind::Id3v2(3), BlockKind::Id3v1],
            items: vec![item(0, "TRCK", &["2/12"]), item(0, "TDRC", &["2022-05-06"]), item(1, "Track", &["2"]), item(1, "Year", &["2022"])],
            ..Default::default()
        };
        assert!(fields(&tags).iter().all(|f| !f.differs()), "ID3v1 has no totals and only years");
        let tags = FileTags { blocks: vec![BlockKind::Id3v2(3), BlockKind::Id3v1], items: vec![item(0, "TRCK", &["3/12"]), item(1, "Track", &["2"])], ..Default::default() };
        assert!(fields(&tags)[0].differs());
    }

    #[test]
    fn the_brief_reads_what_the_scanner_reads() {
        let tags = FileTags {
            blocks: vec![BlockKind::Vorbis],
            items: vec![item(0, "ALBUM", &[" Al "]), item(0, "YEAR", &["2019"]), item(0, "R128_TRACK_GAIN", &["-512"]), item(0, "MUSICBRAINZ_ALBUMID", &["ABC"])],
            ..Default::default()
        };
        let b = brief(&fields(&tags));
        assert_eq!(b, Brief { album: Some("Al".into()), date: Some("2019".into()), mb_album: Some("abc".into()), replaygain: true, ..Default::default() });
    }

    #[test]
    fn long_values_are_cut() {
        let long = "x".repeat(MAX_VALUE_CHARS + 10);
        assert_eq!(cap(&long).chars().count(), MAX_VALUE_CHARS);
        assert_eq!(split_nul("a\0b\0"), ["a", "b"]);
    }

    /// The viewer is read-only towards music files: this file names no API that writes.
    #[test]
    fn nothing_here_can_write() {
        let src = include_str!("meta.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        for word in ["OpenOptions", "File::create", "save_to", "write_to", "remove_file", "rename(", ".save(", "set_len", "fs::write", "write_all"] {
            assert!(!body.contains(word), "meta.rs must not use {word}");
        }
    }
}
