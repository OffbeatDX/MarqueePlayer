//! The tag editor (`library::edit`) on real files of every container the scanner takes.
//!
//! Every file written here is a fixture made by `support/edit_fixtures.rs` under this
//! crate's test folder in the system's temporary folder, in a folder carrying the
//! fixtures' marker; the editor's write guard (installed first thing) refuses any other
//! path. Each test works on its own copy. Audio is compared by ffmpeg's decode (an MD5 of
//! the samples, apart from anything the editor itself computes). Skips without ffmpeg.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use library::edit::{self, Crash, Expect, Failure, FileEdit, Faults, Journal, Outcome, PictureData, PictureEdit};
use library::meta::{self, BlockKind, FileTags, ReadOptions};

#[path = "support/edit_fixtures.rs"]
mod fx;

fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn base() -> PathBuf {
    // Under the system's temporary folder, not the build folder: the editor's write guard
    // (`edit::atomic::under_marker`) only lets tests write there, wherever the build
    // folder is.
    std::env::temp_dir().join("mp-edit-test-fixtures")
}

/// The fixtures, made once for all tests; the write guard, installed before anything.
fn master() -> Option<PathBuf> {
    edit::atomic::set_guard(fx::guard);
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    static MADE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    Some(
        MADE.get_or_init(|| {
            let dir = base().join("master");
            fx::make(&dir);
            dir
        })
        .clone(),
    )
}

/// A test's own copy of some fixtures.
fn work(test: &str, names: &[&str]) -> Option<PathBuf> {
    let m = master()?;
    Some(fx::copy(&m, &base().join(test), names))
}

fn journal(dir: &Path) -> Journal {
    Journal::new(&dir.join("journal"))
}

/// ffmpeg's decode of the audio, as an MD5 of the samples.
fn md5(path: &Path) -> String {
    let out = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args(["-map", "0:a", "-f", "md5", "-"]).output().unwrap();
    assert!(out.status.success(), "ffmpeg could not decode {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn raw(path: &Path) -> FileTags {
    meta::read(path, ReadOptions::RAW).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Item = (String, String, Vec<String>, Option<usize>, Option<u64>);

fn items(t: &FileTags) -> BTreeMap<Item, usize> {
    let mut m = BTreeMap::new();
    for i in &t.items {
        *m.entry((t.blocks[i.block].name(), i.key.clone(), i.values.clone(), i.binary, i.hash)).or_default() += 1;
    }
    m
}

fn pictures(t: &FileTags) -> Vec<(String, u8, String, u64)> {
    let mut v: Vec<_> = t.pictures.iter().map(|p| (t.blocks[p.block].name(), p.type_code, p.description.clone(), p.hash)).collect();
    v.sort();
    v
}

fn field_key(t: &FileTags, i: &Item) -> Option<&'static str> {
    let kind = t.blocks.iter().copied().find(|b| b.name() == i.0)?;
    meta::field_of(kind, &i.1).map(|d| d.key)
}

fn edit_of(path: &Path, fields: &[(&str, &[&str])]) -> FileEdit {
    FileEdit {
        path: path.to_path_buf(),
        fields: fields.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect(),
        raw: Vec::new(),
        pictures: None,
        expect: edit::expect_now(path, &meta::read(path, ReadOptions::TAGS).unwrap()).unwrap(),
        summary: vec!["test".into()],
    }
}

fn save(j: &Journal, e: &FileEdit) -> Result<Outcome, Failure> {
    edit::save_file(e, j, None, Faults::default())
}

/// What is in a folder besides the marker, the journal and the given files.
fn leftovers(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walk(dir) {
        let n = e.to_string_lossy().into_owned();
        if n.contains(edit::atomic::TEMP_MARK) {
            out.push(n);
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}

fn file_hash(p: &Path) -> String {
    library::hash::file_hex(p).unwrap()
}

/// The names of all fixtures but the ones the editor refuses.
fn editable() -> Vec<&'static str> {
    fx::FILES.iter().map(|(n, _)| *n).filter(|n| *n != "adts").collect()
}

// ---- One field, every format ---------------------------------------------------------------

#[test]
fn every_format_changes_only_the_edited_field() {
    let names = editable();
    let Some(dir) = work("one-field", &names) else { return };
    let j = journal(&dir);
    for name in &names {
        let path = fx::file(&dir, name);
        let before = raw(&path);
        let (md5_before, created) = (md5(&path), std::fs::metadata(&path).unwrap().created().unwrap());
        #[cfg(unix)]
        let mode_before = {
            use std::os::unix::fs::PermissionsExt;
            // Not the default, so keeping it means something.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o100640)).unwrap();
            std::fs::metadata(&path).unwrap().permissions().mode()
        };
        let scan_before = library::tags::read(&path, false).unwrap().0;
        let pics_before = pictures(&before);
        match save(&j, &edit_of(&path, &[("TITLE", &["Edited Title"])])) {
            Ok(Outcome::Saved(_)) => {}
            other => panic!("{name}: {other:?}"),
        }
        let after = raw(&path);
        // Every item the file had is there as it was, but the title's.
        let (a, b) = (items(&before), items(&after));
        for (k, n) in &a {
            if field_key(&before, k) == Some("TITLE") {
                continue;
            }
            assert_eq!(b.get(k), Some(n), "{name}: {k:?} lost or changed");
        }
        // What is new: the title, or a block the file did not have (its main tag, made with
        // a copy of what its limited block held).
        for k in b.keys().filter(|k| !a.contains_key(*k)) {
            let new_block = !before.blocks.iter().any(|x| x.name() == k.0);
            assert!(field_key(&after, k) == Some("TITLE") || new_block, "{name}: {k:?} appeared");
            if field_key(&after, k) == Some("TITLE") && k.0 != "ID3v1" {
                assert_eq!(k.2, ["Edited Title"], "{name}");
            }
        }
        assert_eq!(pictures(&after), pics_before, "{name}: pictures");
        assert_eq!(md5(&path), md5_before, "{name}: the audio");
        // ReplaceFileW keeps the creation time. Linux has no call to set a file's birth time,
        // so there the new file is born now; its permission bits are the original's.
        #[cfg(windows)]
        assert_eq!(std::fs::metadata(&path).unwrap().created().unwrap(), created, "{name}: creation time");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = created;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode(), mode_before, "{name}: permission bits");
        }
        for blk in &before.blocks {
            assert!(after.blocks.contains(blk), "{name}: block {} kept", blk.name());
        }
        // The scanner sees the new title and nothing else changed.
        let scan_after = library::tags::read(&path, false).unwrap().0;
        assert_eq!(scan_after.title.as_deref(), Some("Edited Title"), "{name}");
        let mut x = scan_before.clone();
        x.title = scan_after.title.clone();
        x.from_name.title = false;
        x.artists = scan_after.artists.clone();
        x.audio.bitrate = scan_after.audio.bitrate;
        assert_eq!(x, scan_after, "{name}: what the library reads");
    }
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
    // Raw AAC is refused, with the reason.
    let m = master().unwrap();
    let why = edit::support(&fx::file(&m, "adts")).unwrap_err();
    assert!(why.contains("Raw AAC"), "{why}");
}

#[test]
fn id3_keeps_its_version_padding_and_other_blocks_in_step() {
    let Some(dir) = work("id3", &["mp3-v23-v1", "mp3-ape", "mp3-odd", "mp3-v24"]) else { return };
    let j = journal(&dir);
    // ID3v2.3 + ID3v1: the v1 block follows, within its limits.
    let p = fx::file(&dir, "mp3-v23-v1");
    let inv = edit::raw_view(&p).unwrap().inventory.id3v2;
    let Outcome::Saved(s) = save(&j, &edit_of(&p, &[("TITLE", &["A Title That Is Much Longer Than Thirty"]), ("GENRE", &["Funk"])])).unwrap() else { panic!() };
    assert!(s.notes.iter().any(|n| n.contains("30 characters")), "{:?}", s.notes);
    let t = raw(&p);
    assert_eq!(t.blocks, [BlockKind::Id3v2(3), BlockKind::Id3v1]);
    let get = |b: BlockKind, k: &str| t.items.iter().find(|i| t.blocks[i.block] == b && i.key == k).map(|i| i.values.clone());
    assert_eq!(get(BlockKind::Id3v1, "Title"), Some(vec!["A Title That Is Much Longer Th".into()]));
    assert_eq!(get(BlockKind::Id3v1, "Genre"), Some(vec!["Funk".into()]));
    assert_eq!(get(BlockKind::Id3v2(3), "TYER"), Some(vec!["2022".into()]), "the date frames untouched");
    assert_eq!(edit::raw_view(&p).unwrap().inventory.id3v2.map(|x| x.0), inv.map(|x| x.0), "still ID3v2.3");
    // A genre ID3v1 does not list: cleared there, said so.
    let Outcome::Saved(s) = save(&j, &edit_of(&p, &[("GENRE", &["Synthwave"])])).unwrap() else { panic!() };
    assert!(s.notes.iter().any(|n| n.contains("fixed list")), "{:?}", s.notes);
    let t = raw(&p);
    assert!(!t.items.iter().any(|i| t.blocks[i.block] == BlockKind::Id3v1 && i.key == "Genre"));
    // ID3v2.3 date: TYER and TDAT.
    save(&j, &edit_of(&p, &[("DATE", &["2023.04.05"])])).unwrap();
    let t = raw(&p);
    let get = |k: &str| t.items.iter().find(|i| t.blocks[i.block] == BlockKind::Id3v2(3) && i.key == k).map(|i| i.values.clone());
    assert_eq!((get("TYER"), get("TDAT")), (Some(vec!["2023".into()]), Some(vec!["0504".into()])));
    assert_eq!(library::tags::read(&p, false).unwrap().0.date.as_deref(), Some("2023-04-05"), "the scanner reads the full date");
    // Three blocks: the APE tag kept in step, its other items (binary too) untouched.
    let p = fx::file(&dir, "mp3-ape");
    let before = raw(&p);
    save(&j, &edit_of(&p, &[("TITLE", &["Three Changed"]), ("TRACKNUMBER", &["6"])])).unwrap();
    let after = raw(&p);
    assert_eq!(after.blocks, before.blocks);
    let ape = |t: &FileTags, k: &str| t.items.iter().find(|i| t.blocks[i.block] == BlockKind::Ape && i.key == k).cloned();
    assert_eq!(ape(&after, "Title").unwrap().values, ["Three Changed"]);
    assert_eq!(ape(&after, "Track").unwrap().values, ["6/12"], "the total kept");
    assert_eq!(ape(&after, "Binary Thing"), ape(&before, "Binary Thing"));
    assert_eq!(ape(&after, "Catalog"), ape(&before, "Catalog"));
    // Frames the editor never writes come through byte for byte; the padding stays 300.
    let p = fx::file(&dir, "mp3-odd");
    let inv_before = edit::raw_view(&p).unwrap().inventory;
    assert_eq!(inv_before.id3v2, Some((3, 300)));
    save(&j, &edit_of(&p, &[("ALBUM", &["Odd Album"])])).unwrap();
    let inv_after = edit::raw_view(&p).unwrap().inventory;
    assert_eq!(inv_after.id3v2, Some((3, 300)), "version and padding");
    for id in ["GEOB", "PCNT", "PRIV", "POPM", "WXXX", "TXXX"] {
        let pick = |inv: &edit::raw::Inventory| inv.entries.iter().filter(|e| e.id == id).cloned().collect::<Vec<_>>();
        assert_eq!(pick(&inv_after), pick(&inv_before), "{id}");
    }
    // ID3v2.4: multiple values with NUL.
    let p = fx::file(&dir, "mp3-v24");
    save(&j, &edit_of(&p, &[("ARTIST", &["One", "Two"])])).unwrap();
    let t = raw(&p);
    assert_eq!(t.items.iter().find(|i| i.key == "TPE1").unwrap().values, ["One", "Two"]);
    assert_eq!(edit::raw_view(&p).unwrap().inventory.id3v2.map(|x| x.0), Some(4));
}

#[test]
fn multi_values_take_each_formats_own_form() {
    let names = ["flac", "mp3-v24", "mp3-v23-v1", "m4a", "opus", "vorbis", "wav-id3-info", "aiff"];
    let Some(dir) = work("multi", &names) else { return };
    let j = journal(&dir);
    for name in names {
        let p = fx::file(&dir, name);
        save(&j, &edit_of(&p, &[("ARTIST", &["Harbour Lights", "Mara Lind"]), ("GENRE", &["Ambient", "Drone"])])).unwrap_or_else(|e| panic!("{name}: {e}"));
        let t = raw(&p);
        let main = t.items.iter().find(|i| meta::field_of(t.blocks[i.block], &i.key).is_some_and(|d| d.key == "ARTIST") && !matches!(t.blocks[i.block], BlockKind::Id3v1 | BlockKind::RiffInfo | BlockKind::AiffText)).unwrap();
        let want: &[&str] = if t.blocks[main.block] == BlockKind::Id3v2(3) { &["Harbour Lights; Mara Lind"] } else { &["Harbour Lights", "Mara Lind"] };
        assert_eq!(main.values, want, "{name}: {}", main.key);
        // The library credits both, whatever the form.
        let s = library::tags::read(&p, false).unwrap().0;
        let credits: Vec<&str> = s.artists.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(credits, ["Harbour Lights", "Mara Lind"], "{name}");
        assert_eq!(s.genres, ["Ambient", "Drone"], "{name}");
    }
}

/// The Lyrics tab's "Save into the file's tags": LRC text as the file's lyrics item, read back
/// by the app's own lyrics reader as the same synced lines, the audio untouched.
#[test]
fn lyrics_go_into_each_formats_lyrics_item_and_read_back_synced() {
    let names = ["flac", "mp3-v24", "mp3-v23-v1", "m4a", "opus", "vorbis", "wav-id3-info", "aiff"];
    let Some(dir) = work("lyrics", &names) else { return };
    let j = journal(&dir);
    let lines = [
        library::lyrics::Line { time_ms: Some(0), text: "夜の港".into() },
        library::lyrics::Line { time_ms: Some(1_500), text: "Second line".into() },
        library::lyrics::Line { time_ms: Some(3_000), text: String::new() },
    ];
    let lrc = library::lyrics::to_lrc(&lines, 0, &[("ti", "Song")]);
    for name in names {
        let p = fx::file(&dir, name);
        let before = md5(&p);
        save(&j, &edit_of(&p, &[("LYRICS", &[lrc.as_str()])])).unwrap_or_else(|e| panic!("{name}: {e}"));
        let l = library::lyrics::load(&p).unwrap_or_else(|| panic!("{name}: no lyrics read back"));
        assert_eq!((l.source, l.synced), ("tags", true), "{name}");
        assert_eq!(l.lines, lines, "{name}");
        assert_eq!(md5(&p), before, "{name}: the audio");
        // Written again (a change of timing): one lyrics item, the new one.
        let moved = library::lyrics::to_lrc(&lines, 200, &[]);
        save(&j, &edit_of(&p, &[("LYRICS", &[moved.as_str()])])).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(library::lyrics::load(&p).unwrap().lines[1].time_ms, Some(1_700), "{name}");
        let t = raw(&p);
        let n = t.items.iter().filter(|i| meta::field_of(t.blocks[i.block], &i.key).is_some_and(|d| d.key == "LYRICS")).count();
        assert_eq!(n, 1, "{name}: one lyrics item");
    }
}

#[test]
fn numbers_dates_and_the_compilation_flag() {
    let names = ["flac", "mp3-v24", "m4a", "opus", "alac", "wav-untagged"];
    let Some(dir) = work("numbers", &names) else { return };
    let j = journal(&dir);
    for name in names {
        let p = fx::file(&dir, name);
        save(&j, &edit_of(&p, &[("TRACKNUMBER", &["07"]), ("TRACKTOTAL", &["9"]), ("DISCNUMBER", &["2"]), ("DATE", &["2021/6/30"]), ("COMPILATION", &["yes"]), ("BPM", &["120"])]))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = library::tags::read(&p, false).unwrap().0;
        assert_eq!((s.track_no, s.disc_no, s.date.as_deref(), s.compilation), (Some(7), Some(2), Some("2021-06-30"), true), "{name}");
        let f = meta::fields(&raw(&p));
        let total = meta::first(&f, "TRACKTOTAL").or_else(|| meta::first(&f, "TRACKNUMBER").and_then(|v| v.split_once('/').map(|x| x.1.to_owned())));
        assert_eq!(total.as_deref(), Some("9"), "{name}");
        assert_eq!(meta::first(&f, "BPM").as_deref(), Some("120"), "{name}");
    }
    // Removing a number: gone, the total kept where they share an item.
    let p = fx::file(&dir, "mp3-v24");
    save(&j, &edit_of(&p, &[("DISCNUMBER", &[])])).unwrap();
    assert_eq!(library::tags::read(&p, false).unwrap().0.disc_no, None);
}

// ---- The front cover ------------------------------------------------------------------------

fn jpeg_cover(shade: u8) -> PictureData {
    PictureData { type_code: 3, mime: "image/jpeg".into(), description: String::new(), data: fx::cover_jpeg(shade) }
}

#[test]
fn the_front_cover_is_replaced_added_and_removed() {
    let names = ["flac", "mp3-v24", "m4a", "opus", "vorbis", "aiff", "mp3-untagged", "flac-untagged", "alac"];
    let Some(dir) = work("cover", &names) else { return };
    let j = journal(&dir);
    let new = jpeg_cover(200);
    let new_hash = library::hash::of(&new.data);
    for name in names {
        let p = fx::file(&dir, name);
        let before = raw(&p);
        let others: Vec<_> = pictures(&before).into_iter().filter(|x| x.1 != 3 && !matches!(x.0.as_str(), "MP4 metadata")).collect();
        let audio = md5(&p);
        let mut e = edit_of(&p, &[]);
        e.pictures = Some(PictureEdit { remove_front: true, add: vec![new.clone()] });
        save(&j, &e).unwrap_or_else(|x| panic!("{name}: {x}"));
        let t = raw(&p);
        let fronts: Vec<_> = t.pictures.iter().filter(|x| x.front || t.blocks[x.block] == BlockKind::Mp4).collect();
        assert_eq!(fronts.len(), 1, "{name}: one front cover");
        assert_eq!(fronts[0].hash, new_hash, "{name}");
        let still: Vec<_> = pictures(&t).into_iter().filter(|x| x.1 != 3 && !matches!(x.0.as_str(), "MP4 metadata")).collect();
        assert_eq!(still, others, "{name}: other pictures kept");
        assert_eq!(md5(&p), audio, "{name}: the audio");
        // The scanner's art is the new cover.
        assert_eq!(library::tags::read(&p, true).unwrap().1.map(|d| library::hash::of(&d)), Some(new_hash), "{name}");
        // Removed.
        let mut e = edit_of(&p, &[]);
        e.pictures = Some(PictureEdit { remove_front: true, add: vec![] });
        save(&j, &e).unwrap_or_else(|x| panic!("{name}: {x}"));
        let t = raw(&p);
        assert!(!t.pictures.iter().any(|x| x.front || t.blocks[x.block] == BlockKind::Mp4), "{name}: no front cover");
        assert_eq!(md5(&p), audio, "{name}: the audio");
    }
}

// ---- Failures -----------------------------------------------------------------------------

#[test]
fn every_failure_leaves_the_file_as_it_was() {
    let Some(dir) = work("failures", &["flac", "mp3-v24", "opus", "m4a"]) else { return };
    let j = journal(&dir);
    for name in ["flac", "mp3-v24", "opus", "m4a"] {
        let p = fx::file(&dir, name);
        let hash = file_hash(&p);
        let modified = std::fs::metadata(&p).unwrap().modified().unwrap();
        let e = edit_of(&p, &[("TITLE", &["Never Written"])]);
        // The written copy damaged before its check: refused.
        let r = edit::save_file(&e, &j, None, Faults { damage_copy: true, ..Default::default() });
        assert!(matches!(r, Err(Failure::Check(_))), "{name}: {r:?}");
        // Held open by a program that does not share it: the replace fails.
        let r = edit::save_file(&e, &j, None, Faults { hold_original: true, ..Default::default() });
        assert_eq!(r, Err(Failure::InUse), "{name}");
        assert_eq!(file_hash(&p), hash, "{name}: untouched");
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), modified, "{name}: not even its time");
        assert!(leftovers(&dir).is_empty(), "{name}: {:?}", leftovers(&dir));
    }
    // Read-only.
    let p = fx::file(&dir, "flac");
    let hash = file_hash(&p);
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm.clone()).unwrap();
    assert_eq!(save(&j, &edit_of(&p, &[("TITLE", &["X"])])), Err(Failure::ReadOnly));
    assert_eq!(file_hash(&p), hash);
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
    // Changed on disk after the draft was made: by another program (even one that puts
    // the modification time back).
    let e = edit_of(&p, &[("TITLE", &["Draft"])]);
    let then = std::fs::metadata(&p).unwrap().modified().unwrap();
    {
        use lofty::config::WriteOptions;
        use lofty::tag::TagExt;
        let mut f = std::fs::File::open(&p).unwrap();
        let flac = <lofty::flac::FlacFile as lofty::file::AudioFile>::read_from(&mut f, lofty::config::ParseOptions::new()).unwrap();
        drop(f);
        let mut vc = flac.vorbis_comments().cloned().unwrap();
        vc.insert("COMMENT".into(), "changed elsewhere".into());
        vc.save_to_path(&p, WriteOptions::default()).unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(then).unwrap();
    }
    let changed = file_hash(&p);
    assert!(matches!(save(&j, &e), Err(Failure::Changed(_))));
    assert_eq!(file_hash(&p), changed, "not written over");
    // Gone.
    let gone = dir.join("nothing here.flac");
    let mut e2 = e.clone();
    e2.path = gone;
    assert_eq!(save(&j, &e2), Err(Failure::Missing));
    // A path outside the fixtures: the guard refuses it before anything is opened.
    let mut e3 = e;
    e3.path = std::env::temp_dir().join("not-a-fixture.flac");
    assert!(matches!(save(&j, &e3), Err(Failure::Other(x)) if x.contains("guard")));
}

#[test]
fn a_crash_at_any_point_leaves_one_whole_file_and_is_cleaned_up() {
    for (n, crash) in [Crash::BeforeReplace, Crash::AfterBackup, Crash::AfterReplace].into_iter().enumerate() {
        let Some(dir) = work(&format!("crash-{n}"), &["flac", "mp3-v24"]) else { return };
        let j = journal(&dir);
        for name in ["flac", "mp3-v24"] {
            let p = fx::file(&dir, name);
            let (hash, audio) = (file_hash(&p), md5(&p));
            let r = edit::save_file(&edit_of(&p, &[("TITLE", &["After the Crash"])]), &j, None, Faults { crash: Some(crash), ..Default::default() });
            assert!(r.is_err(), "{crash:?}");
            match crash {
                Crash::BeforeReplace => assert_eq!(file_hash(&p), hash, "untouched"),
                Crash::AfterBackup => assert!(!p.exists(), "the name is empty for a moment"),
                Crash::AfterReplace => assert_ne!(file_hash(&p), hash, "the new file in place"),
            }
            // After the replace, ReplaceFileW has left the old file under its backup name;
            // rename(2) (Linux) left nothing beside the file, only the record.
            let after_rename = cfg!(unix) && crash == Crash::AfterReplace;
            assert!(after_rename || !leftovers(&dir).is_empty(), "the crash left its files");
            assert!(std::fs::read_dir(j.inflight()).map(|d| d.count()).unwrap_or(0) > 0, "and its record");
            // The next start.
            let done = edit::atomic::recover(&j.inflight());
            assert!(after_rename || !done.is_empty());
            assert!(leftovers(&dir).is_empty(), "{crash:?}: {:?}", leftovers(&dir));
            assert!(std::fs::read_dir(j.inflight()).map(|d| d.count()).unwrap_or(0) == 0, "records gone");
            assert!(p.exists());
            if crash == Crash::AfterReplace {
                assert_eq!(library::tags::read(&p, false).unwrap().0.title.as_deref(), Some("After the Crash"));
            } else {
                assert_eq!(file_hash(&p), hash, "{crash:?}: the original back");
            }
            assert_eq!(md5(&p), audio, "{crash:?}: a whole file");
        }
    }
}

// ---- A file being played ------------------------------------------------------------------

fn decode_all(src: &mut player_core::source::Source, out: &mut Vec<f32>) {
    while src.next_chunk(out).unwrap() {}
}

#[test]
fn a_file_that_is_playing_is_saved_without_a_glitch() {
    let names = ["flac", "mp3-v24", "opus", "m4a"];
    let Some(dir) = work("playing", &names) else { return };
    let j = journal(&dir);
    for name in names {
        let p = fx::file(&dir, name);
        let reference = {
            let mut s = player_core::source::Source::open(&p).unwrap();
            let mut v = Vec::new();
            decode_all(&mut s, &mut v);
            v
        };
        // Playing: the engine has the file open and has decoded part of it; the next
        // track (the same file, as gapless pre-opens it) is open too.
        let mut playing = player_core::source::Source::open(&p).unwrap();
        let mut heard = Vec::new();
        while heard.len() < reference.len() / 3 {
            assert!(playing.next_chunk(&mut heard).unwrap());
        }
        let mut next = player_core::source::Source::open(&p).unwrap();
        // Saved meanwhile: a tag that grows the file, and a new cover.
        let long = "A much longer title than before, so the tag cannot stay the same size";
        let mut e = edit_of(&p, &[("TITLE", &[long]), ("COMMENT", &["x".repeat(5000).as_str()])]);
        e.pictures = Some(PictureEdit { remove_front: true, add: vec![jpeg_cover(90)] });
        save(&j, &e).unwrap_or_else(|x| panic!("{name}: saving a playing file: {x}"));
        // The playback goes on to the end without a break, and so does the next track.
        decode_all(&mut playing, &mut heard);
        assert!(heard == reference, "{name}: the playing track decodes as it would have");
        let mut v = Vec::new();
        decode_all(&mut next, &mut v);
        assert!(v == reference, "{name}: the pre-opened next track too");
        // Played again: the new file, the same audio.
        let mut again = player_core::source::Source::open(&p).unwrap();
        let mut v = Vec::new();
        decode_all(&mut again, &mut v);
        assert!(v == reference, "{name}: the new file plays the same");
        assert_eq!(library::tags::read(&p, false).unwrap().0.title.as_deref(), Some(long));
        drop((playing, next, again));
        assert!(leftovers(&dir).is_empty(), "{name}: the old file is gone once the player lets go");
    }
}

// ---- The journal: undo ------------------------------------------------------------------

#[test]
fn undo_puts_back_what_a_save_changed_and_refuses_a_file_changed_since() {
    let names = ["flac", "mp3-v23-v1", "m4a", "wav-info", "mp3-untagged"];
    let Some(dir) = work("undo", &names) else { return };
    let j = journal(&dir);
    let originals: Vec<(FileTags, String)> = names.iter().map(|n| fx::file(&dir, n)).map(|p| (raw(&p), md5(&p))).collect();
    let edits: Vec<FileEdit> = names
        .iter()
        .map(|n| {
            let p = fx::file(&dir, n);
            let mut e = edit_of(&p, &[("ALBUM", &["Undo Me"]), ("ALBUMARTIST", &["Someone"]), ("DATE", &["1999-01-02"])]);
            e.pictures = Some(PictureEdit { remove_front: true, add: vec![jpeg_cover(120)] });
            e
        })
        .collect();
    let cancel = AtomicBool::new(false);
    let r = edit::save_all(&edits, &j, "Undo test", None, &cancel, &|_| {});
    assert_eq!((r.saved.len(), r.failed.clone(), r.not_tried.len()), (names.len(), None, 0), "{r:?}");
    let id = r.id.clone().unwrap();
    let saves = j.list();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].files.len(), names.len());
    // The old covers are kept once each, whatever number of files had them.
    let (bytes, n) = j.size();
    assert!(bytes > 0 && n == 1);
    // Undo.
    let undo = j.undo_edits(&saves[0]).unwrap();
    let r = edit::save_all(&undo, &j, "Undo: Undo test", Some(id.clone()), &cancel, &|_| {});
    assert_eq!((r.saved.len(), r.failed.clone()), (names.len(), None), "{r:?}");
    assert!(j.undone(&id));
    for (i, n) in names.iter().enumerate() {
        let p = fx::file(&dir, n);
        let now = raw(&p);
        let (was, audio) = &originals[i];
        assert_eq!(items(&now), items(was), "{n}: the tags as they were");
        assert_eq!(pictures(&now), pictures(was), "{n}: the pictures as they were");
        assert_eq!(&md5(&p), audio, "{n}");
    }
    // A save, then the file changes: its undo is refused; the other files' go through.
    let e1 = edit_of(&fx::file(&dir, "flac"), &[("TITLE", &["Second"])]);
    let e2 = edit_of(&fx::file(&dir, "m4a"), &[("TITLE", &["Second"])]);
    let r = edit::save_all(&[e1, e2], &j, "Second", None, &cancel, &|_| {});
    assert_eq!(r.saved.len(), 2);
    let second = j.get(r.id.as_deref().unwrap()).unwrap();
    save(&j, &edit_of(&fx::file(&dir, "m4a"), &[("TITLE", &["Third, elsewhere"])])).unwrap();
    let undo = j.undo_edits(&second).unwrap();
    let r = edit::save_all(&undo, &j, "Undo: Second", Some(second.header.id.clone()), &cancel, &|_| {});
    assert_eq!(r.saved.len(), 1, "the FLAC is put back");
    assert!(matches!(&r.failed, Some((p, Failure::Changed(_))) if p.ends_with("02 AAC Atoms.m4a")), "{r:?}");
    assert_eq!(library::tags::read(&fx::file(&dir, "m4a"), false).unwrap().0.title.as_deref(), Some("Third, elsewhere"), "not overwritten");
}

// ---- An album -----------------------------------------------------------------------------

fn album_names() -> Vec<&'static str> {
    vec!["flac-plain", "mp3-v24", "m4a", "opus", "vorbis"]
}

#[test]
fn an_album_save_stops_at_a_failure_and_says_what_was_written() {
    let names = album_names();
    let Some(dir) = work("album-failure", &names) else { return };
    let j = journal(&dir);
    let hashes: Vec<String> = names.iter().map(|n| file_hash(&fx::file(&dir, n))).collect();
    let edits: Vec<FileEdit> = names.iter().map(|n| edit_of(&fx::file(&dir, n), &[("ALBUMARTIST", &["The New Menders"])])).collect();
    // File 3 of 5 fails its check.
    let cancel = AtomicBool::new(false);
    let r = edit::save_all_with(&edits, &j, "Album", None, &cancel, &|_| {}, |i| Faults { damage_copy: i == 2, ..Default::default() });
    assert_eq!(r.saved.iter().map(|s| s.path.clone()).collect::<Vec<_>>(), [fx::file(&dir, names[0]), fx::file(&dir, names[1])]);
    assert!(matches!(&r.failed, Some((p, Failure::Check(_))) if *p == fx::file(&dir, names[2])), "{r:?}");
    assert_eq!(r.not_tried, [fx::file(&dir, names[3]), fx::file(&dir, names[4])]);
    for (i, n) in names.iter().enumerate() {
        assert_eq!(file_hash(&fx::file(&dir, n)) == hashes[i], i >= 2, "{n}: written only if reported so");
    }
    // The two written are in the journal and can be put back.
    let save = j.get(r.id.as_deref().unwrap()).unwrap();
    assert_eq!(save.files.len(), 2);
    let undo = j.undo_edits(&save).unwrap();
    let r = edit::save_all(&undo, &j, "Undo", Some(save.header.id.clone()), &cancel, &|_| {});
    assert_eq!(r.saved.len(), 2);
    // Cancelled after the first file: the rest not tried.
    let edits: Vec<FileEdit> = names.iter().map(|n| edit_of(&fx::file(&dir, n), &[("LABEL", &["Lantern"])])).collect();
    let r = edit::save_all(&edits, &j, "Label", None, &cancel, &|p| {
        if p.done == 1 {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    // (Cancel came while the second file was being saved: that one is finished.)
    assert!(r.cancelled);
    assert_eq!(r.saved.len(), 2);
    assert_eq!(r.not_tried.len(), 3);
    // Read-only in the middle: per file, said so.
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    let ro = fx::file(&dir, names[1]);
    let mut perm = std::fs::metadata(&ro).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm.clone()).unwrap();
    let edits: Vec<FileEdit> = names.iter().map(|n| edit_of(&fx::file(&dir, n), &[("LABEL", &["Lantern 2"])])).collect();
    let r = edit::save_all(&edits, &j, "Label 2", None, &cancel, &|_| {});
    assert_eq!((r.saved.len(), r.failed.as_ref().map(|f| f.1.clone())), (1, Some(Failure::ReadOnly)));
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&ro, perm).unwrap();
    // A file already as asked: unchanged, not rewritten.
    let p = fx::file(&dir, names[0]);
    let h = file_hash(&p);
    assert_eq!(save_one(&j, &p, &[("LABEL", &["Lantern 2"])]), Outcome::Unchanged);
    assert_eq!(file_hash(&p), h);
}

fn save_one(j: &Journal, p: &Path, f: &[(&str, &[&str])]) -> Outcome {
    save(j, &edit_of(p, f)).unwrap()
}

#[test]
fn the_journal_keeps_pictures_once_and_prunes_old_saves() {
    let names = album_names();
    let Some(dir) = work("journal", &names) else { return };
    let j = journal(&dir);
    let cancel = AtomicBool::new(false);
    for round in 0..3u8 {
        let edits: Vec<FileEdit> = names
            .iter()
            .map(|n| {
                let mut e = edit_of(&fx::file(&dir, n), &[("COMMENT", &[format!("round {round}").as_str()])]);
                e.pictures = Some(PictureEdit { remove_front: true, add: vec![jpeg_cover(round)] });
                e
            })
            .collect();
        let r = edit::save_all(&edits, &j, &format!("Round {round}"), None, &cancel, &|_| {});
        assert_eq!(r.saved.len(), names.len(), "{r:?}");
    }
    let blobs = || std::fs::read_dir(j.dir().join("pictures")).map(|d| d.count()).unwrap_or(0);
    // Each round replaced one cover shared by every file: kept once. (Round 0 replaced the
    // fixtures' own covers, several different ones.)
    assert!(blobs() <= 2 + names.len(), "{}", blobs());
    j.prune(1, u64::MAX);
    assert_eq!(j.list().len(), 1, "the newest save kept");
    assert_eq!(blobs(), 1, "only the pictures it needs");
    j.clear();
    assert_eq!(j.size(), (0, 0));
}

/// The viewer's guarantee holds: reading never writes (the editor's own reads included).
#[test]
fn reading_for_the_editor_writes_nothing() {
    let Some(dir) = work("read-only", &editable()) else { return };
    for n in editable() {
        let p = fx::file(&dir, n);
        let (h, m) = (file_hash(&p), std::fs::metadata(&p).unwrap().modified().unwrap());
        let _ = raw(&p);
        let _ = edit::raw_view(&p).unwrap();
        let _ = edit::audio_print(&p).unwrap();
        let _ = edit::support(&p).unwrap();
        assert_eq!((file_hash(&p), std::fs::metadata(&p).unwrap().modified().unwrap()), (h, m), "{n}");
    }
    let _ = Expect::Any;
}

#[test]
fn a_writer_that_changes_more_than_asked_is_caught() {
    let names = ["flac", "mp3-v24", "m4a", "opus", "wav-id3-info"];
    let Some(dir) = work("extra-item", &names) else { return };
    let j = journal(&dir);
    for n in names {
        let p = fx::file(&dir, n);
        let h = file_hash(&p);
        let r = edit::save_file(&edit_of(&p, &[("TITLE", &["Only This"])]), &j, None, Faults { extra_item: true, ..Default::default() });
        // (lofty's generic writer, used to sneak the item in, also drops items it does not
        // map, ASIN for one: the check names whichever difference it meets first.)
        assert!(matches!(&r, Err(Failure::Check(_))), "{n}: {r:?}");
        assert_eq!(file_hash(&p), h, "{n}");
    }
    assert!(leftovers(&dir).is_empty());
}

/// Another tagger saving the file while it is written and checked: its save is not written
/// over with a copy of what the file held before it.
#[test]
fn a_file_another_program_saves_meanwhile_is_not_written_over() {
    let names = ["flac", "mp3-v24"];
    let Some(dir) = work("changed-meanwhile", &names) else { return };
    let j = journal(&dir);
    for n in names {
        let p = fx::file(&dir, n);
        let h = file_hash(&p);
        let r = edit::save_file(&edit_of(&p, &[("TITLE", &["Mine"])]), &j, None, Faults { changed_meanwhile: true, ..Default::default() });
        assert!(matches!(&r, Err(Failure::Changed(_))), "{n}: {r:?}");
        let before = file_hash(&p);
        assert_ne!(before, h, "{n}: the other program's save is there");
        let now = lofty::read_from_path(&p).unwrap();
        let tag = lofty::file::TaggedFileExt::primary_tag(&now).unwrap();
        assert_ne!(tag.get_string(lofty::tag::ItemKey::TrackTitle), Some("Mine"), "{n}: and not written over");
    }
    assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
}

#[test]
fn a_file_that_could_not_be_replaced_leaves_nothing_to_undo() {
    let names = ["flac-plain", "mp3-v24"];
    let Some(dir) = work("withdrawn", &names) else { return };
    let j = journal(&dir);
    let edits: Vec<FileEdit> = names.iter().map(|n| edit_of(&fx::file(&dir, n), &[("COMMENT", &["held"])])).collect();
    let cancel = AtomicBool::new(false);
    // The second file is held open by another program when it is replaced.
    let r = edit::save_all_with(&edits, &j, "Held", None, &cancel, &|_| {}, |i| Faults { hold_original: i == 1, ..Default::default() });
    assert_eq!((r.saved.len(), r.failed.as_ref().map(|f| f.1.clone())), (1, Some(Failure::InUse)));
    let save = j.get(r.id.as_deref().unwrap()).unwrap();
    assert_eq!(save.files.len(), 1, "only the file written is in the journal");
    assert!(save.files[0].path.ends_with("01 Patch One.flac"));
}
