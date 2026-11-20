//! Audio files from outside the library: on the command line, handed over by a second
//! launch (`platform::instance`), or dropped on the window from the file manager
//! (`platform::window`: WM_DROPFILES on Windows, winit's file drops on Linux).
//!
//! Files in the library play as its tracks (history, favourites and all). Others play as
//! plain files: their tags are read on the fly and they are not added to the library
//! (a negative track id marks them; nothing is recorded for them). Folders are opened as
//! the audio files under them, in path order. A drop on the queue (the queue sidebar, or
//! Up next on now playing) adds to the end of the queue; anywhere else plays now. Playlist
//! files (.m3u, .m3u8), dropped or on the command line, are imported as playlists
//! (`playlists::file`).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::query::Track;
use library::{m3u, query, scan};
use slint::ComponentHandle;

use crate::state::{self, State};
use crate::{Nav, Page, playback, views};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Replace the queue and play.
    Play,
    /// Add to the end of the queue.
    Append,
}

/// Audio files, playlist files and folders named on the command line, made absolute (a
/// second launch may run in another folder).
pub fn args() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        // Options (`--minimized`) are not files.
        .filter(|a| !a.to_string_lossy().starts_with("--"))
        .map(PathBuf::from)
        .filter_map(|p| std::path::absolute(&p).ok())
        .filter(|p| openable(p))
        .collect()
}

/// A folder, an audio file or a playlist file: what "Open with" and a drop can open.
fn openable(p: &Path) -> bool {
    p.is_dir() || (p.is_file() && (scan::is_audio(p) || crate::playlists::file::is_playlist_file(p)))
}

/// Files under `paths` (folders walked, in path order), audio only, in the order given. A
/// folder is walked once, however it is reached (a link or junction to a folder above it
/// would otherwise be walked again and again).
pub fn collect(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, seen: &mut std::collections::HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
        if !std::fs::canonicalize(dir).is_ok_and(|c| seen.insert(c)) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        for p in entries {
            if p.is_dir() {
                walk(&p, seen, out);
            } else if scan::is_audio(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut seen, &mut out);
        } else if scan::is_audio(p) && p.is_file() {
            out.push(p.clone());
        }
    }
    out
}

/// A track for a file outside the library, from its tags (or its name).
pub fn outside_track(path: &Path, id: i64) -> Track {
    let tags = library::tags::read(path, false).map(|(t, _)| t).unwrap_or_default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Track {
        id,
        album_id: -1,
        path: path.to_string_lossy().into_owned(),
        title: tags.title.unwrap_or(stem),
        artist: tags.artist.clone().unwrap_or_default(),
        album: tags.album.unwrap_or_default(),
        album_artist: tags.album_artist.or(tags.artist).unwrap_or_default(),
        disc_no: tags.disc_no,
        track_no: tags.track_no,
        duration: tags.duration,
        art: None,
        // Read with the tags, on the fly: such a file is not in the library.
        audio: tags.audio,
        remote_id: None,
    }
}

thread_local! {
    static NEXT_ID: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) };
}

/// Open `paths`: walked, looked up in the library (on a connection of its own) and read off
/// the UI thread, then played or queued.
pub fn open(st: &Rc<State>, paths: Vec<PathBuf>, mode: Mode) {
    if paths.is_empty() {
        return;
    }
    let db = st.paths.database();
    std::thread::Builder::new()
        .name("mp-open-files".into())
        .spawn(move || {
            library::background_thread();
            let files = collect(&paths);
            let conn = library::db::open(&db).inspect_err(|e| log::warn!("opening files: the library cannot be read ({e}); they play as files")).ok();
            let read = resolve(conn.as_ref(), &files);
            state::post(move |st| finish(st, read, mode));
        })
        .ok();
}

/// The library's track for each file it has, in the order given; for the others a track from
/// the file's own tags, with id 0 (`finish` numbers them).
fn resolve(conn: Option<&rusqlite::Connection>, files: &[PathBuf]) -> Vec<Track> {
    let ids: Vec<Option<i64>> = match conn {
        Some(c) => {
            let library: std::collections::HashMap<String, i64> = query::track_paths(c).unwrap_or_default().into_iter().map(|(p, id)| (m3u::key(Path::new(&p)), id)).collect();
            files.iter().map(|f| library.get(&m3u::key(f)).copied()).collect()
        }
        None => vec![None; files.len()],
    };
    let wanted: Vec<i64> = ids.iter().flatten().copied().collect();
    let tracks = conn.and_then(|c| query::tracks_by_id(c, &wanted).ok()).unwrap_or_default();
    // Tags are read only for the files the library does not have.
    files.iter().zip(ids).map(|(f, id)| id.and_then(|id| tracks.get(&id).cloned()).unwrap_or_else(|| outside_track(f, 0))).collect()
}

fn finish(st: &Rc<State>, read: Vec<Track>, mode: Mode) {
    if read.is_empty() {
        views::flash(st, "No audio files there");
        return;
    }
    let mut outside = 0;
    let tracks: Vec<Track> = read
        .into_iter()
        .map(|t| {
            if t.id > 0 {
                return t;
            }
            outside += 1;
            let id = NEXT_ID.with(|n| {
                let id = n.get();
                n.set(id - 1);
                id
            });
            Track { id, ..t }
        })
        .collect();
    let n = tracks.len();
    log::info!("opened {n} file(s) from outside ({outside} not in the library), {mode:?}");
    match mode {
        Mode::Play => playback::play_tracks(st, tracks, 0),
        Mode::Append => playback::enqueue(st, tracks, false),
    }
    let what = if n == 1 { "1 track".to_owned() } else { format!("{n} tracks") };
    views::flash(st, &match mode {
        Mode::Play => format!("Playing {what}"),
        Mode::Append => format!("Added {what} to the queue"),
    });
}

/// From a second launch (`at` None) or a drop (`at` the point, physical pixels).
pub fn from_outside(paths: Vec<PathBuf>, at: Option<(i32, i32)>) {
    state::with(|st| {
        let mode = match (at, st.app.upgrade()) {
            (Some((x, y)), Some(app)) => {
                if on_queue(&app, x as f32, y as f32) {
                    Mode::Append
                } else {
                    Mode::Play
                }
            }
            _ => Mode::Play,
        };
        open_given(st, paths, mode);
    });
}

/// What the command line, a second launch or a drop gave: playlist files are imported, asking
/// before any name clash; the rest plays (or is queued).
pub fn open_given(st: &Rc<State>, paths: Vec<PathBuf>, mode: Mode) {
    let (lists, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| p.is_file() && crate::playlists::file::is_playlist_file(p));
    crate::playlists::file::import_files(st, lists);
    open(st, paths, mode);
}

/// Whether a window point (physical pixels) is over the queue: the queue sidebar while it
/// shows, or the Up next panel on now playing.
pub fn on_queue(app: &crate::AppWindow, x: f32, y: f32) -> bool {
    let scale = app.window().scale_factor();
    let size = app.window().size().to_logical(scale);
    let (x, _y) = (x / scale, y / scale);
    let theme = app.global::<crate::Theme>();
    let nav = app.global::<Nav>();
    let queue_shown = nav.get_queue_open() || nav.get_queue_pinned() || nav.get_queue_hover_open();
    if queue_shown && x >= size.width - theme.get_queue_width() {
        return true;
    }
    // Now playing: the panel beside the art (pages/nowplaying.slint, NowPlayingPage).
    nav.get_page() == Page::NowPlaying
        && !app.global::<crate::Lyrics>().get_shown()
        && x >= size.width - theme.get_page_margin() - (420.0f32).min(size.width * 0.34)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_open_as_their_audio_files_in_order() {
        let d = std::env::temp_dir().join(format!("mp-opener-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("B").join("CD2")).unwrap();
        for f in ["B/02.flac", "B/01.flac", "B/cover.jpg", "B/CD2/01.opus", "single.mp3", "notes.txt"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let got = collect(&[d.join("single.mp3"), d.join("B"), d.join("notes.txt")]);
        let names: Vec<String> = got.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["single.mp3", "B/01.flac", "B/02.flac", "B/CD2/01.opus"]);
        let t = outside_track(&d.join("single.mp3"), -3);
        assert_eq!((t.id, t.title.as_str(), t.album_id), (-3, "single", -1), "untagged: named after the file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn files_the_library_has_open_as_its_tracks() {
        let d = std::env::temp_dir().join(format!("mp-opener-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (known, other) = (d.join("known.flac"), d.join("other.flac"));
        for f in [&known, &other] {
            std::fs::write(f, b"x").unwrap();
        }
        let conn = library::db::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES (1, 'k', 'A', 'B', ?1, 0)",
            [d.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES (5, ?1, 1, 'Known', 'B', 1, 0, 0, 0)",
            [known.to_string_lossy()],
        )
        .unwrap();
        let files = [other.clone(), known.clone(), known.clone()];
        let got: Vec<(i64, String)> = resolve(Some(&conn), &files).into_iter().map(|t| (t.id, t.title)).collect();
        // Without the library: every one a file from outside.
        let without: Vec<i64> = resolve(None, &files).iter().map(|t| t.id).collect();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(got, [(0, "other".to_owned()), (5, "Known".to_owned()), (5, "Known".to_owned())]);
        assert_eq!(without, [0, 0, 0]);
    }

    #[test]
    fn playlist_files_can_be_opened_from_outside() {
        let d = std::env::temp_dir().join(format!("mp-opener-args-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in ["a.flac", "mix.m3u", "MIX.M3U8", "notes.txt"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let kept: Vec<bool> = ["a.flac", "mix.m3u", "MIX.M3U8", "notes.txt", "gone.m3u"].iter().map(|f| openable(&d.join(f))).collect();
        assert!(openable(&d));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(kept, [true, true, true, false, false]);
    }

    /// A link (a junction on Windows) inside a folder back to that folder: its files once.
    #[test]
    fn a_folder_linked_inside_itself_is_walked_once() {
        let d = std::env::temp_dir().join(format!("mp-opener-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("A")).unwrap();
        std::fs::write(d.join("A").join("01.flac"), b"x").unwrap();
        let link = d.join("A").join("again");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(d.join("A"), &link).is_ok();
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(d.join("A")).output().is_ok_and(|o| o.status.success());
        assert!(made, "the link could not be made");
        let got = collect(&[d.join("A")]);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(got.len(), 1, "{got:?}");
    }
}
