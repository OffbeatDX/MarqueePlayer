//! Tag editing: changing a file's tags and front cover, safely.
//!
//! Off unless the user turns it on (the app's Settings). Nothing here runs otherwise, and
//! `meta` (the viewer's reader) stays read-only.
//!
//! A save of one file (`save_file`):
//!
//! 1. The file must be there, writable, and as it was when the draft was made (its size,
//!    modification time and tags; for an undo, its whole content as the save left it).
//! 2. Its tags are read as stored (`meta::ReadOptions::RAW`) and the field edits planned as
//!    changes to items of its blocks (`fields::plan`): the block the library reads, and
//!    every other block holding the field.
//! 3. It is copied beside itself (`atomic::stage`), the copy is changed with lofty
//!    (`write`), and checked (`check::verify`): its tags are the original's with exactly
//!    these changes, nothing lofty does not show was lost, and the audio is the same.
//! 4. What undoes it goes in the journal (`journal`), then the copy replaces the file
//!    (`atomic::commit`), keeping its creation time and attributes.
//!
//! Any failure leaves the file as it was, and says why. A save of several files
//! (`save_all`) stops at the first that fails and says which were written and which not;
//! it can be cancelled between files, never in the middle of one.

pub mod atomic;
mod check;
pub mod cover;
mod fields;
pub mod journal;
pub mod raw;
mod write;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use atomic::{Crash, Failure};
pub use check::items_digest;
pub use fields::{ALBUM_FIELDS, COMMON, Change, Editability, Kind, Plan, check, check_value, editability, iso_date, kind, main_block, multi_valued};
pub use journal::Journal;

use crate::meta::{self, BlockKind, FileTags, ReadOptions};

/// The containers the editor writes, as lofty tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Flac,
    /// MP3 and MP2.
    Mpeg,
    /// Raw ADTS AAC (`.aac`).
    Aac,
    /// M4A, MP4, M4B (AAC or ALAC).
    Mp4,
    Wav,
    Aiff,
    Opus,
    Vorbis,
}

impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Self::Flac => "FLAC",
            Self::Mpeg => "MPEG audio",
            Self::Aac => "AAC (ADTS)",
            Self::Mp4 => "MP4",
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Opus => "Opus",
            Self::Vorbis => "Ogg Vorbis",
        }
    }

    pub(crate) fn shape(self) -> raw::Shape {
        match self {
            Self::Flac => raw::Shape::Flac,
            Self::Mpeg | Self::Aac => raw::Shape::Frames,
            Self::Mp4 => raw::Shape::Mp4,
            Self::Wav => raw::Shape::Riff,
            Self::Aiff => raw::Shape::Aiff,
            Self::Opus | Self::Vorbis => raw::Shape::Ogg,
        }
    }

    /// The tag the library reads first in this format; a file without it gets it (ID3v2
    /// as version 2.3, which every player reads).
    pub fn primary(self) -> BlockKind {
        match self {
            Self::Flac | Self::Opus | Self::Vorbis => BlockKind::Vorbis,
            Self::Mp4 => BlockKind::Mp4,
            Self::Mpeg | Self::Aac | Self::Wav | Self::Aiff => BlockKind::Id3v2(3),
        }
    }
}

/// Whether a file can be edited: its container, or why not.
pub fn support(path: &Path) -> Result<Container, String> {
    use lofty::file::FileType;
    crate::remote::refuse(path)?;
    if !path.is_file() {
        return Err("The file is not there".into());
    }
    let probe = lofty::probe::Probe::open(path).map_err(|e| e.to_string())?;
    let probe = if probe.file_type().is_none() { probe.guess_file_type().map_err(|e| e.to_string())? } else { probe };
    let c = match probe.file_type() {
        Some(FileType::Flac) => Container::Flac,
        Some(FileType::Mpeg) => Container::Mpeg,
        // Measured: a tag in front of ADTS audio decodes the same, but lofty estimates a raw
        // AAC file's length from its size, so the check that nothing but the tags changed
        // cannot pass. Not attempted.
        Some(FileType::Aac) => return Err("Raw AAC files (.aac) are not edited: they have no tag format of their own, and their length is estimated from their size, so the check that only the tags changed cannot be made".into()),
        Some(FileType::Mp4) => Container::Mp4,
        Some(FileType::Wav) => Container::Wav,
        Some(FileType::Aiff) => Container::Aiff,
        Some(FileType::Opus) => Container::Opus,
        Some(FileType::Vorbis) => Container::Vorbis,
        Some(FileType::Speex) => return Err("Speex files are not edited (the app does not play them)".into()),
        Some(other) => return Err(format!("{other:?} files are not edited")),
        None => return Err("Not a format whose tags can be changed".into()),
    };
    Ok(c)
}

/// Why a file's tags as they are keep it from being edited (None: they do not).
pub fn blocked(container: Container, tags: &FileTags) -> Option<String> {
    if tags.blocks.contains(&BlockKind::Id3v2(2)) {
        return Some("Its tag is ID3v2.2, an old version the editor does not write (it would have to convert every frame)".into());
    }
    if tags.dropped > 0 {
        return Some(format!("It has more tag items than the editor reads ({} more)", tags.dropped));
    }
    if let Some(main) = main_block(container, &tags.blocks)
        && fields::same_kind(main, container.primary())
        && !fields::writable(container, main)
    {
        return Some(format!("Its {} tag cannot be written", main.name()));
    }
    None
}

/// An embedded picture to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureData {
    /// ID3v2's picture type (3: the front cover).
    pub type_code: u8,
    pub mime: String,
    pub description: String,
    pub data: Vec<u8>,
}

/// A change to the front cover: take it out (every front cover the file has; in MP4 every
/// `covr` image, which carry no type), and put these in.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PictureEdit {
    pub remove_front: bool,
    pub add: Vec<PictureData>,
}

/// What the file must still be for the edit to be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// As read for the draft: size, modification time (ns since 1970), and
    /// `items_digest` of its tags read with `ReadOptions::TAGS`.
    Read { size: u64, modified: i64, items: u64 },
    /// As a save left it (an undo): its size and content hash.
    Saved { size: u64, hash: String },
    /// Anything (tests).
    Any,
}

/// A file's modification time as `Expect::Read` keeps it.
pub fn modified_ns(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// What the draft of a file was made from, read now.
pub fn expect_now(path: &Path, tags: &FileTags) -> Option<Expect> {
    let m = std::fs::metadata(path).ok()?;
    Some(Expect::Read { size: m.len(), modified: modified_ns(&m), items: items_digest(tags) })
}

/// An edit of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    /// Field edits: canonical key, new values (none removes the field). Planned when saved.
    pub fields: Vec<(String, Vec<String>)>,
    /// Or changes to items as stored (an undo), written as they are.
    pub raw: Vec<Change>,
    pub pictures: Option<PictureEdit>,
    pub expect: Expect,
    /// What changes, as a person reads it (kept in the journal).
    pub summary: Vec<String>,
}

/// One file written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    pub size: u64,
    pub hash: String,
    /// What the format made of the edit ("ID3v1 holds 30 characters: its title is cut").
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Saved(Saved),
    /// Nothing to change: the file already holds these values.
    Unchanged,
}

/// The largest file the editor writes: its check reads the file whole (`raw::view`), once
/// for the original and once for the copy.
pub const MAX_FILE_BYTES: u64 = 2 << 30;

/// Faults to inject, for tests: each leaves the file as it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// Stop committing at this point, as a crash would.
    pub crash: Option<Crash>,
    /// Damage the written copy before it is checked.
    pub damage_copy: bool,
    /// Make the replacement fail (the file is held open without sharing).
    pub hold_original: bool,
    /// Add a tag item the edit did not ask for to the written copy, before it is checked
    /// (a writer that changes more than it should).
    pub extra_item: bool,
    /// Another program saves the original while the copy is written and checked.
    pub changed_meanwhile: bool,
}

/// Save one file. `journal`: where writes under way leave their records, and where the
/// undo goes when `save_id` (the save's entry there) is given.
pub fn save_file(edit: &FileEdit, journal: &Journal, save_id: Option<&str>, faults: Faults) -> Result<Outcome, Failure> {
    // A panic while writing or checking is this file's failure (`guard`): the original is untouched
    // until the checked copy replaces it, and a copy left half made is the record's to clear.
    crate::guard::catching(|| save_file_unguarded(edit, journal, save_id, faults)).unwrap_or_else(|what| Err(Failure::Other(format!("an internal error while writing it ({what})"))))
}

fn save_file_unguarded(edit: &FileEdit, journal: &Journal, save_id: Option<&str>, faults: Faults) -> Result<Outcome, Failure> {
    let path = edit.path.as_path();
    crate::remote::refuse(path).map_err(Failure::Other)?;
    let m = atomic::check_target(path)?;
    if m.len() > MAX_FILE_BYTES {
        return Err(Failure::Other(format!("the file is larger than the editor checks ({} GB)", MAX_FILE_BYTES >> 30)));
    }
    match &edit.expect {
        Expect::Read { size, modified, items } => {
            let now = meta::read(path, ReadOptions::TAGS).map_err(|e| Failure::Other(format!("the file cannot be read ({e})")))?;
            if m.len() != *size || modified_ns(&m) != *modified || items_digest(&now) != *items {
                return Err(Failure::Changed("the file changed on disk after you started editing it; open it again to see what it holds now".into()));
            }
        }
        Expect::Saved { size, hash } => {
            let now = crate::hash::file_hex(path).map_err(|e| atomic::failure(&e))?;
            if m.len() != *size || &now != hash {
                return Err(Failure::Changed("the file changed after that save (another program, or a later save: undo that one first)".into()));
            }
        }
        Expect::Any => {}
    }
    let container = support(path).map_err(Failure::Other)?;
    let tags = meta::read(path, ReadOptions::RAW).map_err(|e| Failure::Other(format!("its tags cannot be read ({e})")))?;
    if let Some(why) = blocked(container, &tags) {
        return Err(Failure::Other(why));
    }
    let (mut changes, mut created, notes) = if edit.fields.is_empty() {
        let created = edit.raw.iter().map(|c| c.block).find(|b| !tags.blocks.contains(b));
        (edit.raw.clone(), created, Vec::new())
    } else {
        let p = fields::plan(container, &tags, &edit.fields).map_err(Failure::Other)?;
        (p.changes, p.creates, p.notes)
    };
    // Changes that change nothing are left out.
    changes.retain(|c| {
        let block = tags.blocks.iter().position(|b| *b == c.block);
        let now: Vec<&meta::RawItem> = tags.items.iter().filter(|i| Some(i.block) == block && i.key == c.key).collect();
        match now.as_slice() {
            [] => !c.values.is_empty(),
            [one] => one.values != c.values || one.binary.is_some(),
            _ => true,
        }
    });
    let pictures = edit.pictures.as_ref().filter(|p| p.remove_front || !p.add.is_empty());
    if changes.is_empty() && pictures.is_none() {
        return Ok(Outcome::Unchanged);
    }
    if pictures.is_some_and(|p| !p.add.is_empty()) {
        let pb = check::picture_block(container, &tags, created);
        if !tags.blocks.contains(&pb) && created.is_none() && pb != BlockKind::Vorbis {
            created = Some(pb);
        }
    }
    if let Some(b) = created
        && !fields::writable(container, b)
    {
        return Err(Failure::Other(format!("a {} tag cannot be added to this file", b.name())));
    }
    let before = check::snapshot(path, container, tags.clone()).map_err(|e| Failure::Other(format!("the file cannot be read ({e})")))?;
    // The undo: previous values of what changes, the front covers it had.
    let undo: Vec<Change> = changes
        .iter()
        .map(|c| {
            let block = tags.blocks.iter().position(|b| *b == c.block);
            let values = tags.items.iter().filter(|i| Some(i.block) == block && i.key == c.key).flat_map(|i| i.values.clone()).collect();
            Change { block: c.block, key: c.key.clone(), values }
        })
        .collect();
    let undo_front = if pictures.is_some() { front_pictures(path, container).map_err(Failure::Other)? } else { Vec::new() };
    let keep = write::Keep { id3v2_padding: before.raw.inventory.id3v2.map(|(_, p)| p), flac_padding: !before.raw.inventory.flac_padding.is_empty() };
    let extra = pictures.map_or(0, |p| p.add.iter().map(|a| a.data.len() as u64).sum::<u64>()) + (1 << 16);
    let inflight = journal.inflight();
    let staged = atomic::stage(path, &inflight, extra)?;
    let result = (|| -> Result<Saved, Failure> {
        write::apply(&staged.temp, container, &changes, pictures, keep).map_err(|e| Failure::Check(format!("lofty could not write it ({e})")))?;
        if faults.damage_copy {
            damage(&staged.temp);
        }
        if faults.extra_item {
            sneak_in(&staged.temp);
        }
        check::verify(&before, &staged.temp, container, &changes, pictures, created).map_err(Failure::Check)?;
        // The scanner tells a changed file by its size and its modification time in whole
        // seconds: the new file's time is now, and at least a second past the old one's.
        later_than(&staged.temp, &m).map_err(|e| atomic::failure(&e))?;
        let hash = crate::hash::file_hex(&staged.temp).map_err(|e| atomic::failure(&e))?;
        let size = std::fs::metadata(&staged.temp).map_err(|e| atomic::failure(&e))?.len();
        if let Some(id) = save_id {
            let j = journal;
            let undo_pictures = match pictures {
                Some(_) => {
                    let mut restore = Vec::new();
                    for p in &undo_front {
                        restore.push(j.keep_picture(p).map_err(|e| Failure::Other(format!("the undo journal cannot be written ({e})")))?);
                    }
                    Some(journal::UndoPictures { remove_front: true, restore })
                }
                None => None,
            };
            let entry = journal::FileEntry { path: path.to_string_lossy().into_owned(), size, hash: hash.clone(), undo: undo.clone(), undo_pictures, summary: edit.summary.clone() };
            j.add(id, &entry).map_err(|e| Failure::Other(format!("the undo journal cannot be written ({e})")))?;
        }
        if faults.changed_meanwhile {
            sneak_in(path);
        }
        // Once more, just before the copy takes its place: another program's save meanwhile (the
        // check above reads the whole copy: seconds for a long file) is not written over.
        let unchanged = std::fs::metadata(path).map_err(|e| atomic::failure(&e)).and_then(|now| {
            if now.len() == m.len() && modified_ns(&now) == modified_ns(&m) {
                Ok(())
            } else {
                Err(Failure::Changed("another program changed the file while it was being saved; nothing was written: open it again to see what it holds now".into()))
            }
        });
        let _hold = if faults.hold_original { hold(path) } else { None };
        if let Err(e) = unchanged.and_then(|()| atomic::commit_with(&staged, faults.crash)) {
            // Not written (a crash a test simulates aside): its undo goes too.
            if let (Some(id), None) = (save_id, faults.crash) {
                let _ = journal.withdraw(id, &path.to_string_lossy());
            }
            return Err(e);
        }
        Ok(Saved { path: path.to_path_buf(), size, hash, notes: notes.clone() })
    })();
    match result {
        Ok(s) => Ok(Outcome::Saved(s)),
        Err(e) => {
            if faults.crash.is_none() {
                atomic::abandon(&staged);
            }
            Err(e)
        }
    }
}

/// Flip some bytes in the middle of a file (a test of the check).
fn damage(path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path)
        && let Ok(len) = f.seek(SeekFrom::End(0))
    {
        let _ = f.seek(SeekFrom::Start(len * 2 / 3));
        let _ = f.write_all(&[0x55; 16]);
    }
}

/// Add an item to a file's main tag (a test of the check).
fn sneak_in(path: &Path) {
    use lofty::file::TaggedFileExt;
    use lofty::tag::{ItemKey, TagExt};
    if let Ok(probe) = lofty::probe::Probe::open(path).map_err(|_| ()).and_then(|p| p.guess_file_type().map_err(|_| ()))
        && let Ok(mut f) = probe.read()
        && let Some(t) = f.primary_tag_mut()
    {
        t.insert_text(ItemKey::Label, "sneaked in".into());
        let _ = t.save_to_path(path, lofty::config::WriteOptions::default());
    }
}

/// Open a file without letting anyone delete or rename it (a test of a failed replace).
#[cfg(windows)]
fn hold(path: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).share_mode(1 /* FILE_SHARE_READ */).open(path).ok()
}

/// Unix has no open that stops a rename: the replace is refused instead, as the system
/// refuses one in a folder that does not allow it (a test of a failed replace).
#[cfg(unix)]
fn hold(_path: &Path) -> Option<Refused> {
    atomic::REFUSE_REPLACE.with(|r| r.set(true));
    Some(Refused)
}

#[cfg(unix)]
struct Refused;

#[cfg(unix)]
impl Drop for Refused {
    fn drop(&mut self) {
        atomic::REFUSE_REPLACE.with(|r| r.set(false));
    }
}

#[cfg(not(any(windows, unix)))]
fn hold(_path: &Path) -> Option<std::fs::File> {
    None
}

/// The file's front covers, with their data (what an undo puts back).
pub fn front_pictures(path: &Path, container: Container) -> Result<Vec<PictureData>, String> {
    use lofty::file::AudioFile;
    use lofty::ogg::OggPictureStorage;
    use lofty::picture::{Picture, PictureType};
    crate::remote::refuse(path)?;
    let po = write::parse_options();
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let e = |x: lofty::error::FileParseError| x.to_string();
    let data = |p: &Picture| PictureData { type_code: p.pic_type().as_u8(), mime: p.mime_type().map(|m| m.as_str().to_owned()).unwrap_or_default(), description: p.description().unwrap_or("").to_owned(), data: p.data().to_vec() };
    let front = |p: &&Picture| p.pic_type() == PictureType::CoverFront;
    let id3 = |t: Option<&lofty::id3::v2::Id3v2Tag>| -> Vec<PictureData> {
        t.map(|t| t.iter().filter_map(|fr| if let lofty::id3::v2::Frame::Picture(p) = fr { Some(&p.picture) } else { None }).filter(|p| p.pic_type() == PictureType::CoverFront).map(|p| data(p)).collect()).unwrap_or_default()
    };
    Ok(match container {
        Container::Flac => {
            let x = lofty::flac::FlacFile::read_from(&mut f, po).map_err(e)?;
            let mut v: Vec<PictureData> = x.pictures().iter().map(|(p, _)| p).filter(front).map(data).collect();
            if let Some(vc) = x.vorbis_comments() {
                v.extend(vc.pictures().iter().map(|(p, _)| p).filter(front).map(data));
            }
            v
        }
        Container::Opus => lofty::ogg::OpusFile::read_from(&mut f, po).map_err(e)?.vorbis_comments().pictures().iter().map(|(p, _)| p).filter(front).map(data).collect(),
        Container::Vorbis => lofty::ogg::VorbisFile::read_from(&mut f, po).map_err(e)?.vorbis_comments().pictures().iter().map(|(p, _)| p).filter(front).map(data).collect(),
        Container::Mp4 => lofty::mp4::Mp4File::read_from(&mut f, po).map_err(e)?.ilst().and_then(|i| i.pictures().map(|p| p.map(data).collect())).unwrap_or_default(),
        Container::Mpeg => id3(lofty::mpeg::MpegFile::read_from(&mut f, po).map_err(e)?.id3v2()),
        Container::Aac => id3(lofty::aac::AacFile::read_from(&mut f, po).map_err(e)?.id3v2()),
        Container::Wav => id3(lofty::iff::wav::WavFile::read_from(&mut f, po).map_err(e)?.id3v2()),
        Container::Aiff => id3(lofty::iff::aiff::AiffFile::read_from(&mut f, po).map_err(e)?.id3v2()),
    })
}

/// Where a save of several files is.
#[derive(Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: PathBuf,
}

/// How a save of several files went.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Its entry in the journal (None when nothing was written).
    pub id: Option<String>,
    pub saved: Vec<Saved>,
    pub unchanged: Vec<PathBuf>,
    /// The file it stopped at, and why.
    pub failed: Option<(PathBuf, Failure)>,
    /// Files not tried: after the one that failed, or after Cancel.
    pub not_tried: Vec<PathBuf>,
    pub cancelled: bool,
}

/// Save files one after the other, recording each in the journal before it is replaced.
/// Stops at the first file that fails, and between files when `cancel` is set.
pub fn save_all(edits: &[FileEdit], journal: &Journal, label: &str, undoes: Option<String>, cancel: &AtomicBool, progress: &dyn Fn(Progress)) -> Report {
    save_all_with(edits, journal, label, undoes, cancel, progress, |_| Faults::default())
}

/// `save_all`, with faults injected per file (tests).
pub fn save_all_with(edits: &[FileEdit], journal: &Journal, label: &str, undoes: Option<String>, cancel: &AtomicBool, progress: &dyn Fn(Progress), faults: impl Fn(usize) -> Faults) -> Report {
    let mut r = Report::default();
    for line in atomic::recover(&journal.inflight()) {
        log::warn!("edit: {line}");
    }
    let id = match journal.begin(label, undoes) {
        Ok(id) => id,
        Err(e) => {
            r.failed = edits.first().map(|f| (f.path.clone(), Failure::Other(format!("the undo journal cannot be written ({e})"))));
            r.not_tried = edits.iter().skip(1).map(|f| f.path.clone()).collect();
            return r;
        }
    };
    for (i, edit) in edits.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            r.cancelled = true;
            r.not_tried = edits[i..].iter().map(|f| f.path.clone()).collect();
            break;
        }
        progress(Progress { done: i, total: edits.len(), current: edit.path.clone() });
        match save_file(edit, journal, Some(&id), faults(i)) {
            Ok(Outcome::Saved(s)) => r.saved.push(s),
            Ok(Outcome::Unchanged) => r.unchanged.push(edit.path.clone()),
            Err(e) => {
                log::warn!("edit: {}: {e}", edit.path.display());
                r.failed = Some((edit.path.clone(), e));
                r.not_tried = edits[i + 1..].iter().map(|f| f.path.clone()).collect();
                break;
            }
        }
    }
    progress(Progress { done: edits.len() - r.not_tried.len(), total: edits.len(), current: PathBuf::new() });
    if r.saved.is_empty() {
        // Nothing written: no entry to keep.
        let _ = std::fs::remove_file(journal.dir().join("saves").join(format!("{id}.jsonl")));
    } else {
        r.id = Some(id);
        journal.prune(journal::MAX_SAVES, journal::MAX_BYTES);
    }
    r
}

/// A file's own tag items and its other parts, as the check sees them (tests, tools).
pub fn raw_view(path: &Path) -> Result<raw::RawView, String> {
    raw::view(path, support(path)?.shape())
}

/// The audio as the check compares it: the demuxed packets, codec parameters and
/// properties, as one line (tests, tools).
pub fn audio_print(path: &Path) -> Result<String, String> {
    check::audio(path, path).map(|a| format!("{} packets, {:032x}, {}", a.packets, a.digest, a.props))
}

/// Give `path` a modification time of now, or one second past the old file's (`was`)
/// when now is within the same second.
fn later_than(path: &Path, was: &std::fs::Metadata) -> std::io::Result<()> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let old = was.modified().unwrap_or(UNIX_EPOCH);
    let now = SystemTime::now();
    let t = if secs(now) > secs(old) { now } else { UNIX_EPOCH + Duration::from_secs(secs(old) + 1) };
    std::fs::OpenOptions::new().write(true).open(path)?.set_modified(t)
}
