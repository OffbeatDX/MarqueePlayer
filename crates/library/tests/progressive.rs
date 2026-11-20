//! The scan written in batches, on the messy fixture library (`examples/messy_library.rs`, the
//! one the home and library benches use: mixed formats, compilations without an album artist,
//! untagged files, disc folders, embedded art and cover files): stopped after every batch and
//! started again until it finishes, it must end as the library one uninterrupted scan makes, row
//! for row in everything a user can see and in all of their data (`support/scan_dump.rs`), and
//! read no file twice. For a first scan, and for a rescan after a reorganisation (folders moved,
//! a file renamed, an album retagged, files and an album removed) of a library with plays,
//! favourites, a playlist and a queue on it.
//!
//! Needs ffmpeg on PATH (the audio templates); skips without it.

#[path = "../examples/messy_library.rs"]
#[allow(dead_code)]
mod messy;
#[path = "support/scan_dump.rs"]
mod scan_dump;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use library::scan::{Options, Progress, scan_with};
use lofty::prelude::*;
use rusqlite::Connection;

fn ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn audio(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            audio(&p, out);
        } else if library::scan::is_audio(&p) {
            out.push(p);
        }
    }
}

/// The folders that hold audio files, in order.
fn album_folders(music: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    audio(music, &mut files);
    let mut dirs: Vec<PathBuf> = files.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
    dirs.dedup();
    dirs
}

fn copy_db(from: &Path, to: &Path) {
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", to.display()));
    }
    let c = Connection::open(from).unwrap();
    c.execute_batch(&format!("VACUUM INTO '{}'", to.to_string_lossy().replace('\'', "''"))).unwrap();
}

/// Scan to the end from a copy of `base`: in one batch, or `batch` files at a time stopped
/// after every batch and started again. The dump, the files read in all, and the runs.
fn scan_to_end(base: &Path, db: &Path, art: &library::ArtCache, music: &Path, batch: usize, stop: bool) -> (Vec<(&'static str, String)>, usize, usize) {
    copy_db(base, db);
    let mut conn = library::db::open(db).unwrap();
    let ids_up_to: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tracks", [], |r| r.get(0)).unwrap();
    let since = library::db::now_ms();
    let (mut runs, mut read) = (0, 0);
    loop {
        runs += 1;
        assert!(runs < 2000, "the scan never finishes");
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let progress = move |p: Progress| {
            if stop && p.batches >= 1 {
                c.store(true, Ordering::Relaxed);
            }
        };
        let opts = Options { cancel: Some(cancel), batch_files: Some(batch), ..Default::default() };
        let r = scan_with(&mut conn, art, &[music.to_path_buf()], opts, &progress).unwrap();
        read += r.read;
        if !r.interrupted {
            break;
        }
    }
    (scan_dump::dump(&conn, since, ids_up_to), read, runs)
}

/// Every album of a folder retitled (a retag that changes its identity).
fn retitle(folder: &Path, title: &str) -> bool {
    let mut files = Vec::new();
    audio(folder, &mut files);
    for f in &files {
        let Ok(mut tagged) = lofty::read_from_path(f) else { return false };
        let Some(tag) = tagged.primary_tag_mut() else { return false };
        tag.set_album(title.to_owned());
        if tag.save_to_path(f, lofty::config::WriteOptions::default()).is_err() {
            return false;
        }
    }
    !files.is_empty()
}

#[test]
fn the_messy_library_stopped_after_every_batch_is_the_same_library() {
    if !ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("mp-progressive-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("music");
    let tracks = messy::make(&music, 16);
    let art = library::ArtCache::new(dir.join("art"));

    // A first scan.
    let empty = dir.join("empty.db");
    drop(library::db::open(&empty).unwrap());
    let (want, read, _) = scan_to_end(&empty, &dir.join("ref.db"), &art, &music, usize::MAX, false);
    assert_eq!(read, tracks);
    let (got, read, runs) = scan_to_end(&empty, &dir.join("stopped.db"), &art, &music, 40, true);
    scan_dump::assert_same(&want, &got, "first scan, stopped after every batch");
    assert_eq!(read, tracks, "no file read twice");
    assert!(runs > 5, "{runs} runs");

    // The user's data on it.
    let base = dir.join("base.db");
    copy_db(&dir.join("ref.db"), &base);
    {
        let c = library::db::open(&base).unwrap();
        let ids: Vec<i64> = c.prepare("SELECT id FROM tracks ORDER BY path").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for (i, id) in ids.iter().enumerate() {
            if i % 7 == 0 {
                library::query::record_play(&c, *id).unwrap();
            }
            if i % 11 == 0 {
                library::query::toggle_favourite_track(&c, *id).unwrap();
            }
        }
        let albums: Vec<i64> = c.prepare("SELECT id FROM albums ORDER BY key").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for a in albums.iter().step_by(3) {
            library::query::toggle_favourite_album(&c, *a).unwrap();
        }
        let pl = library::query::create_playlist(&c, "mix").unwrap();
        library::query::add_to_playlist(&c, pl, &ids.iter().step_by(5).copied().collect::<Vec<_>>()).unwrap();
        c.execute_batch(&format!("INSERT INTO queue_items (position, track_id) VALUES (0, {}), (1, {});", ids[3], ids[ids.len() / 2])).unwrap();
        c.execute("UPDATE albums SET added_at = 1000 + id, first_seen = 1000 + id, added_source = NULL", []).unwrap();
        c.execute("UPDATE tracks SET added_at = 100 + id", []).unwrap();
    }

    // A reorganisation: two album folders moved, a file renamed, an album retagged, a folder and
    // a file removed.
    let folders = album_folders(&music);
    assert!(folders.len() > 20, "{} folders", folders.len());
    let moved = dir.join("music").join("Moved Here");
    std::fs::create_dir_all(&moved).unwrap();
    for f in [&folders[2], &folders[9]] {
        std::fs::rename(f, moved.join(f.file_name().unwrap())).unwrap();
    }
    let mut files = Vec::new();
    audio(&folders[4], &mut files);
    let first = &files[0];
    std::fs::rename(first, first.with_file_name(format!("renamed {}", first.file_name().unwrap().to_string_lossy()))).unwrap();
    assert!(folders[6..].iter().take(6).any(|f| retitle(f, "A New Title")), "an album retitled");
    std::fs::remove_dir_all(&folders[12]).unwrap();
    let mut files = Vec::new();
    audio(&folders[14], &mut files);
    std::fs::remove_file(&files[files.len() - 1]).unwrap();

    let (want, _, _) = scan_to_end(&base, &dir.join("ref2.db"), &art, &music, usize::MAX, false);
    let kept = &want.iter().find(|(w, _)| *w == "track ids kept").unwrap().1;
    assert!(kept.contains("Moved Here") && kept.contains("renamed "), "the moves kept their rows");
    for batch in [40, 7] {
        let (got, _, runs) = scan_to_end(&base, &dir.join(format!("stopped{batch}.db")), &art, &music, batch, true);
        scan_dump::assert_same(&want, &got, &format!("rescan in batches of {batch}, stopped after every one"));
        assert!(runs >= 2, "{runs} runs: stopped at least once");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
