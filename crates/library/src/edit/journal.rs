//! The edit journal: what each save changed, so it can be undone.
//!
//! In the app's data folder (`edits/`): one file per save (`saves/<id>.jsonl`), a header
//! line and then a line per file written, appended and flushed before that file is
//! replaced, so a save that stops half way (an error, a crash) still lists every file it
//! changed. Each file's line holds what undoes it (the previous values of the items that
//! changed, as `Change`s; for the front cover the previous pictures, kept once each in
//! `pictures/<hash>.bin` whatever number of files had it) and what the file was once
//! written (its size and content hash), so an undo refuses a file changed since.
//!
//! Kept: the last `MAX_SAVES` saves, within `MAX_BYTES` (pictures included); older saves
//! go first when a save is added. Settings shows the size and can clear it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Change, PictureData, PictureEdit};

pub const MAX_SAVES: usize = 100;
pub const MAX_BYTES: u64 = 256 << 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub id: String,
    /// Unix time in milliseconds.
    pub time: i64,
    /// What was saved: "“Patchwork”: album artist on 3 tracks".
    pub label: String,
    /// The save this one undid.
    #[serde(default)]
    pub undoes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPicture {
    pub type_code: u8,
    pub mime: String,
    pub description: String,
    /// The blob's name in `pictures/`.
    pub blob: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoPictures {
    pub remove_front: bool,
    pub restore: Vec<StoredPicture>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    /// The file as the save left it.
    pub size: u64,
    pub hash: String,
    pub undo: Vec<Change>,
    #[serde(default)]
    pub undo_pictures: Option<UndoPictures>,
    /// What changed, as a person reads it ("Album artist: “A” → “B”").
    #[serde(default)]
    pub summary: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save {
    pub header: Header,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug)]
pub struct Journal {
    dir: PathBuf,
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

impl Journal {
    /// The journal in `dir` (made when first written to).
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where writes under way leave their records (`atomic`).
    pub fn inflight(&self) -> PathBuf {
        self.dir.join("inflight")
    }

    fn saves(&self) -> PathBuf {
        self.dir.join("saves")
    }

    fn pictures(&self) -> PathBuf {
        self.dir.join("pictures")
    }

    /// Start a save's entry.
    pub fn begin(&self, label: &str, undoes: Option<String>) -> Result<String, String> {
        std::fs::create_dir_all(self.saves()).map_err(io)?;
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        // Ids sort in the order of the saves (`list`, and `prune` keeps the newest): by time, never
        // before the newest id (the clock may have been set back since); the counter keeps two
        // saves in one millisecond apart.
        let newest = std::fs::read_dir(self.saves())
            .map_err(io)?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.strip_suffix(".jsonl")?.to_owned();
                let (t, n) = name.split_once('-')?;
                Some((t.parse::<i64>().ok()?, n.parse::<u32>().ok()?))
            })
            .max();
        let (at, mut n) = match newest {
            Some((t, n)) if t >= time => (t, n + 1),
            _ => (time, 0),
        };
        let id = loop {
            let id = format!("{at:013}-{n:04}");
            if !self.saves().join(format!("{id}.jsonl")).exists() {
                break id;
            }
            n += 1;
        };
        let h = Header { id: id.clone(), time, label: label.to_owned(), undoes };
        self.append(&id, &serde_json::to_string(&h).map_err(|e| e.to_string())?)?;
        Ok(id)
    }

    fn append(&self, id: &str, line: &str) -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.saves().join(format!("{id}.jsonl"))).map_err(io)?;
        f.write_all(line.as_bytes()).map_err(io)?;
        f.write_all(b"\n").map_err(io)?;
        f.sync_all().map_err(io)
    }

    /// Keep a picture (once) and say where.
    pub fn keep_picture(&self, p: &PictureData) -> Result<StoredPicture, String> {
        std::fs::create_dir_all(self.pictures()).map_err(io)?;
        let blob = format!("{:016x}-{}.bin", crate::hash::of(&p.data), p.data.len());
        let path = self.pictures().join(&blob);
        if !path.exists() {
            let tmp = path.with_extension("part");
            let mut f = std::fs::File::create(&tmp).map_err(io)?;
            f.write_all(&p.data).map_err(io)?;
            f.sync_all().map_err(io)?;
            drop(f);
            std::fs::rename(&tmp, &path).map_err(io)?;
        }
        Ok(StoredPicture { type_code: p.type_code, mime: p.mime.clone(), description: p.description.clone(), blob, bytes: p.data.len() as u64 })
    }

    /// Record a file of a save, before it is replaced.
    pub fn add(&self, id: &str, file: &FileEntry) -> Result<(), String> {
        self.append(id, &serde_json::to_string(file).map_err(|e| e.to_string())?)
    }

    /// Take a file back out of a save (its replacement failed: it was not written).
    pub fn withdraw(&self, id: &str, path: &str) -> Result<(), String> {
        let file = self.saves().join(format!("{id}.jsonl"));
        let text = std::fs::read_to_string(&file).map_err(io)?;
        let mut keep = String::new();
        for (i, line) in text.lines().enumerate() {
            let ours = i > 0 && serde_json::from_str::<FileEntry>(line).is_ok_and(|f| f.path == path);
            if !ours {
                keep.push_str(line);
                keep.push('\n');
            }
        }
        let tmp = file.with_extension("part");
        std::fs::write(&tmp, keep).map_err(io)?;
        std::fs::rename(&tmp, &file).map_err(io)
    }

    /// Every save, oldest first. A line that cannot be read (a crash mid-write) is skipped.
    pub fn list(&self) -> Vec<Save> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir(self.saves()) else { return out };
        let mut names: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")).collect();
        names.sort();
        for p in names {
            let Ok(f) = std::fs::File::open(&p) else { continue };
            let mut lines = std::io::BufReader::new(f).lines().map_while(Result::ok);
            let Some(header) = lines.next().and_then(|l| serde_json::from_str::<Header>(&l).ok()) else { continue };
            let files = lines.filter_map(|l| serde_json::from_str::<FileEntry>(&l).ok()).collect();
            out.push(Save { header, files });
        }
        out
    }

    /// A save by its id.
    pub fn get(&self, id: &str) -> Option<Save> {
        self.list().into_iter().find(|s| s.header.id == id)
    }

    /// The edits that undo a save: per file, its previous values, written only if the file
    /// is still as the save left it.
    pub fn undo_edits(&self, save: &Save) -> Result<Vec<super::FileEdit>, String> {
        let mut out = Vec::new();
        for f in &save.files {
            let pictures = match &f.undo_pictures {
                Some(u) => {
                    let mut add = Vec::new();
                    for sp in &u.restore {
                        let data = std::fs::read(self.pictures().join(&sp.blob)).map_err(|e| format!("a picture this save replaced is no longer kept ({e})"))?;
                        add.push(PictureData { type_code: sp.type_code, mime: sp.mime.clone(), description: sp.description.clone(), data });
                    }
                    Some(PictureEdit { remove_front: u.remove_front, add })
                }
                None => None,
            };
            out.push(super::FileEdit {
                path: PathBuf::from(&f.path),
                fields: Vec::new(),
                raw: f.undo.clone(),
                pictures,
                expect: super::Expect::Saved { size: f.size, hash: f.hash.clone() },
                summary: f.summary.iter().map(|s| format!("Undo: {s}")).collect(),
            });
        }
        Ok(out)
    }

    /// Whether a later save undid this one.
    pub fn undone(&self, id: &str) -> bool {
        self.list().iter().any(|s| s.header.undoes.as_deref() == Some(id) && !s.files.is_empty())
    }

    /// Bytes on disk, and the number of saves.
    pub fn size(&self) -> (u64, usize) {
        let mut bytes = 0;
        let mut saves = 0;
        for (sub, count) in [(self.saves(), true), (self.pictures(), false)] {
            if let Ok(d) = std::fs::read_dir(sub) {
                for e in d.flatten() {
                    bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                    if count {
                        saves += 1;
                    }
                }
            }
        }
        (bytes, saves)
    }

    /// Drop the oldest saves beyond `max_saves` or `max_bytes`, and pictures no save needs.
    /// Never the newest save.
    pub fn prune(&self, max_saves: usize, max_bytes: u64) {
        let saves = self.list();
        let mut keep: Vec<&Save> = saves.iter().collect();
        let blob_size = |b: &str| std::fs::metadata(self.pictures().join(b)).map(|m| m.len()).unwrap_or(0);
        let total = |k: &[&Save]| -> u64 {
            let mut blobs: Vec<&str> = k.iter().flat_map(|s| s.files.iter()).flat_map(|f| f.undo_pictures.iter()).flat_map(|u| u.restore.iter()).map(|p| p.blob.as_str()).collect();
            blobs.sort_unstable();
            blobs.dedup();
            let lines: u64 = k.iter().map(|s| std::fs::metadata(self.saves().join(format!("{}.jsonl", s.header.id))).map(|m| m.len()).unwrap_or(0)).sum();
            lines + blobs.iter().map(|b| blob_size(b)).sum::<u64>()
        };
        while keep.len() > 1 && (keep.len() > max_saves || total(&keep) > max_bytes) {
            let old = keep.remove(0);
            let _ = std::fs::remove_file(self.saves().join(format!("{}.jsonl", old.header.id)));
        }
        let wanted: Vec<&str> = keep.iter().flat_map(|s| s.files.iter()).flat_map(|f| f.undo_pictures.iter()).flat_map(|u| u.restore.iter()).map(|p| p.blob.as_str()).collect();
        if let Ok(d) = std::fs::read_dir(self.pictures()) {
            for e in d.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !wanted.contains(&name.as_str()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    /// Forget every save (Settings' "Clear"). Writes under way keep their records.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(self.saves());
        let _ = std::fs::remove_dir_all(self.pictures());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the clock set back (a dead clock battery, a dual boot), a new save still comes last:
    /// listed as the newest, and never the one pruned. Many in one millisecond keep their order.
    #[test]
    fn saves_keep_their_order_when_the_clock_goes_back() {
        let dir = crate::testdir::TestDir::new("journal-order");
        let j = Journal::new(&dir);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        // A save made while the clock was a day ahead.
        let ahead = format!("{:013}-0", now + 86_400_000);
        std::fs::create_dir_all(j.saves()).unwrap();
        let h = Header { id: ahead.clone(), time: now + 86_400_000, label: "ahead".into(), undoes: None };
        j.append(&ahead, &serde_json::to_string(&h).unwrap()).unwrap();
        let ids: Vec<String> = (0..12).map(|i| j.begin(&format!("save {i}"), None).unwrap()).collect();
        let listed: Vec<String> = j.list().into_iter().map(|s| s.header.id).collect();
        assert_eq!(listed[0], ahead, "{listed:?}");
        assert_eq!(listed[1..], ids[..], "the new saves after it, in the order made");
        j.prune(1, u64::MAX);
        let left: Vec<String> = j.list().into_iter().map(|s| s.header.label).collect();
        assert_eq!(left, ["save 11"], "the newest save is kept");
        // And one more after the prune still comes after it.
        j.begin("save 12", None).unwrap();
        j.prune(1, u64::MAX);
        let left: Vec<String> = j.list().into_iter().map(|s| s.header.label).collect();
        assert_eq!(left, ["save 12"]);
    }
}
