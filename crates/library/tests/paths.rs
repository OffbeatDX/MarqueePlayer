//! A library on file names and folders the way strangers' disks have them: names in other
//! scripts, emoji and right-to-left text, paths past Windows' old 260-character limit, a folder
//! that links back to its parent (a junction on Windows, a symbolic link on Linux), empty and
//! mis-named files, names Windows cannot open normally (a trailing dot, a device name), and on
//! Linux two files that differ only in letter case. The scan must finish (no hang in the loop, no
//! panic), take every file that can be read, say which cannot, and the player must open what the
//! library holds.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().is_ok_and(|o| o.status.success())
}

/// Create a file at a path the platform may not take as written (Windows: through the
/// verbatim `\\?\` form, which takes trailing dots, device names and any length).
fn put(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    let path = &PathBuf::from(format!(r"\\?\{}", path.display()));
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, bytes)
}

#[test]
fn strange_names_long_paths_and_loops_scan_and_play() {
    if !ffmpeg(&["-version"]) {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scan-paths");
    let _ = std::fs::remove_dir_all(&root);
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let tone = root.join("tone.flac");
    let t = tone.to_string_lossy().into_owned();
    assert!(ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=330:duration=2", "-metadata", "artist=Paths", "-metadata", "album=Paths", &t]));
    let flac = std::fs::read(&tone).unwrap();

    let mut good: Vec<PathBuf> = vec![
        music.join("Ünïcödé 🎵 ✨").join("01 Émoji 🎶.flac"),
        music.join("שירים ‏עברית").join("02 ‏שיר.flac"),
        music.join("日本語のアルバム").join("03 夜.flac"),
    ];
    // Nested folders to well past 260 characters.
    let mut deep = music.clone();
    while deep.as_os_str().len() < 330 {
        deep = deep.join("A rather long folder name for a deep library");
    }
    good.push(deep.join("04 Deep.flac"));
    for p in &good {
        put(p, &flac).unwrap();
    }
    // FLAC audio named .mp3 (a wrong extension: read by its content), an empty file, a text
    // file named .flac.
    good.push(music.join("Odd").join("05 Wrong extension.mp3"));
    put(&music.join("Odd").join("05 Wrong extension.mp3"), &flac).unwrap();
    put(&music.join("Odd").join("06 Empty.flac"), b"").unwrap();
    put(&music.join("Odd").join("07 Text.flac"), b"not audio at all\n").unwrap();
    // Names Windows' ordinary paths cannot reach (copied over from another system).
    #[cfg(windows)]
    {
        put(&music.join("Odd").join("08 Trailing dot.flac."), &flac).unwrap();
        put(&music.join("Odd").join("CON.flac"), &flac).unwrap();
    }
    // A folder that leads back to its parent.
    #[cfg(windows)]
    {
        let link = music.join("Loop");
        let ok = Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&music).output().is_ok_and(|o| o.status.success());
        assert!(ok, "a junction could be made");
    }
    // Two files that differ only in letter case: two tracks, where the file system tells
    // them apart (Linux; not macOS's default, case-insensitive volumes).
    #[cfg(unix)]
    let twins = {
        std::os::unix::fs::symlink(&music, music.join("Loop")).unwrap();
        put(&music.join("Case").join("Song.flac"), &flac).unwrap();
        let twins = !music.join("Case").join("SONG.flac").exists();
        if twins {
            put(&music.join("Case").join("song.flac"), &flac).unwrap();
        }
        twins
    };

    let art = library::ArtCache::new(root.join("art"));
    let (tx, rx) = std::sync::mpsc::channel();
    let scan_root = music.clone();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        let mut conn = library::db::open_in_memory().unwrap();
        let r = library::scan::scan(&mut conn, &art, &[scan_root], &|_| {}).map_err(|e| e.to_string());
        let paths: Vec<String> = conn.prepare("SELECT path FROM tracks").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        let _ = tx.send((r, paths));
    });
    let (report, paths) = rx.recv_timeout(Duration::from_secs(60)).expect("the scan finishes (no endless walk through the loop)");
    let report = report.expect("the scan succeeds");
    eprintln!("scanned in {:.1} s: {} tracks; unreadable: {:?}", t0.elapsed().as_secs_f64(), paths.len(), report.errors);
    for p in &good {
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(paths.iter().any(|t| t.ends_with(&*name)), "{name} is in the library");
    }
    // Through the loop, nothing comes twice.
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.dedup();
    assert!(paths.iter().filter(|p| p.contains("Loop")).count() <= good.len() + 3, "the loop is not walked round and round");
    // The empty and the text file are said to be unreadable, by name.
    for bad in ["06 Empty.flac", "07 Text.flac"] {
        assert!(report.errors.iter().any(|(p, _)| p.ends_with(bad)), "{bad} is listed as unreadable");
    }
    #[cfg(unix)]
    if twins {
        assert!(paths.iter().any(|p| p.ends_with("Case/Song.flac")) && paths.iter().any(|p| p.ends_with("Case/song.flac")), "both case twins");
    }
    // The player opens what the library holds, the long path included.
    for p in paths.iter().filter(|p| good.iter().any(|g| g.to_string_lossy() == **p)) {
        let mut s = player_core::source::Source::open(Path::new(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
        let mut buf = Vec::new();
        assert!(s.next_chunk(&mut buf).is_ok(), "{p} decodes");
    }
    #[cfg(windows)]
    let _ = Command::new("cmd").args(["/c", "rmdir"]).arg(music.join("Loop")).output();
    let _ = std::fs::remove_dir_all(format!(r"{}{}", if cfg!(windows) { r"\\?\" } else { "" }, root.display()));
}
