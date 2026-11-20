//! Playlist files (`library::m3u`): import M3U / M3U8 files as playlists named after the
//! files, export one as M3U8 with absolute paths or paths relative to the saved file.
//!
//! A file named like a playlist there is already is never imported over it silently: the
//! clash prompt (`nameclash`) asks, saying how many tracks each has: Replace (the playlist
//! keeps its id, so home shelves and smart playlist rules naming it keep doing so, and gets
//! the file's tracks), Keep both (the file comes in as "Name (2)", the next free number) or
//! Cancel. Several files (the picker takes more than one, and playlist files dropped on the
//! window come here too) ask once per clash, or once with "do the same for the rest". A smart
//! playlist cannot be replaced by a file: only Keep both and Cancel. A file in which nothing
//! matched leaves a playlist of its name as it is, without asking.
//!
//! Import only reads the chosen files; export writes only where the user saves.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::query::Playlist;
use library::{m3u, query};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::playlists::nameclash::{self, Choice, q};
use crate::state::State;
use crate::{Nav, Page, Playlists, views};

/// Whether `path` names a playlist file import reads (.m3u, .m3u8).
pub fn is_playlist_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
}

pub fn import_dialog(st: &Rc<State>) {
    if !crate::dialog_allowed("import playlist") {
        return;
    }
    let picked = rfd::FileDialog::new().set_title("Import playlists").add_filter("Playlist", &["m3u8", "m3u"]).pick_files();
    if let Some(paths) = picked {
        import_files(st, paths);
    }
}

/// The name a file comes in under: its name without the extension.
pub fn name_of(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Imported playlist".into())
}

/// A file read and matched to the library.
struct Read {
    /// The file's name, as the prompt says it.
    file: String,
    /// The playlist name it asks for.
    name: String,
    entries: usize,
    m: m3u::Matched,
}

fn read(st: &Rc<State>, path: &Path) -> Result<Read, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let entries = m3u::parse(&m3u::decode(&bytes));
    let library: Vec<(String, i64)> = query::track_paths(&st.db).unwrap_or_default();
    let roots = st.settings.borrow().folders.clone();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let m = m3u::match_entries(&entries, &library, &dir, &roots);
    let file = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Read { file, name: name_of(path), entries: entries.len(), m })
}

/// What importing a file asking for the name `wanted` does, given the playlists there are
/// and the answer to a clash (no clash: a new playlist of that name, whatever the answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// A new playlist of this name.
    Create(String),
    /// This playlist's tracks become the file's; it keeps its id and its name.
    Replace(i64, String),
    /// Nothing changes.
    Skip,
}

pub fn plan(wanted: &str, all: &[Playlist], choice: Choice) -> Plan {
    let Some(p) = nameclash::find(wanted, all, None) else {
        return Plan::Create(wanted.trim().to_owned());
    };
    match choice {
        Choice::Replace if !p.smart() => Plan::Replace(p.id, p.name.clone()),
        // A smart playlist is not replaced by a file (the prompt does not offer it).
        Choice::Replace | Choice::Cancel => Plan::Skip,
        Choice::KeepBoth => Plan::Create(nameclash::numbered(wanted, &all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>())),
    }
}

/// Carry `plan` out with the file's tracks: the playlist's id (None: skipped). All or
/// nothing (one transaction).
pub fn carry_out(conn: &rusqlite::Connection, plan: &Plan, ids: &[i64]) -> rusqlite::Result<Option<i64>> {
    match plan {
        Plan::Create(name) => query::create_playlist_with(conn, name, ids).map(Some),
        Plan::Replace(id, _) => query::set_playlist_tracks(conn, *id, ids).map(|()| Some(*id)),
        Plan::Skip => Ok(None),
    }
}

/// The files of one import, taken in turn; a clash waits for its answer.
#[derive(Default)]
struct Batch {
    pending: VecDeque<PathBuf>,
    /// "Do the same for the rest": the answer for the clashes still to come.
    rule: Option<Choice>,
    /// The file the prompt is asking about.
    waiting: Option<Read>,
    /// More than one file: a missing entry says which file it is from.
    several: bool,
    lines: Vec<String>,
    missing: Vec<String>,
    /// The last file imported: its playlist and what matched (the bench's report).
    last: Option<(i64, m3u::Matched)>,
}

thread_local! {
    static BATCH: RefCell<Option<Batch>> = const { RefCell::new(None) };
    static LAST: RefCell<Option<(i64, m3u::Matched)>> = const { RefCell::new(None) };
}

/// Import `paths` (the picker, a drop), one after another; the Playlists page says how it
/// went. Asked for while an earlier import waits for an answer, they join it.
pub fn import_files(st: &Rc<State>, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let waiting = BATCH.with(|b| {
        let mut b = b.borrow_mut();
        let b = b.get_or_insert_with(Batch::default);
        b.several |= paths.len() > 1 || b.waiting.is_some() || !b.pending.is_empty() || !b.lines.is_empty();
        b.pending.extend(paths);
        b.waiting.is_some()
    });
    if !waiting {
        pump(st);
    }
}

/// One file, for the bench: its playlist and what matched, when it was imported without a
/// question.
#[cfg(feature = "bench")]
pub fn import_file(st: &Rc<State>, path: &Path) -> Option<(i64, m3u::Matched)> {
    LAST.with(|l| l.borrow_mut().take());
    import_files(st, vec![path.to_path_buf()]);
    LAST.with(|l| l.borrow_mut().take())
}

fn with_batch<R>(f: impl FnOnce(&mut Batch) -> R) -> Option<R> {
    BATCH.with(|b| b.borrow_mut().as_mut().map(f))
}

/// Take the files in turn until one needs an answer, or none is left.
fn pump(st: &Rc<State>) {
    loop {
        let Some(path) = with_batch(|b| b.pending.pop_front()).flatten() else {
            return finish(st);
        };
        let r = match read(st, &path) {
            Ok(r) => r,
            Err(line) => {
                with_batch(|b| b.lines.push(line));
                continue;
            }
        };
        let all = query::playlists(&st.db).unwrap_or_default();
        let Some(existing) = nameclash::find(&r.name, &all, None) else {
            settle(st, r, Choice::KeepBoth);
            continue;
        };
        if r.m.ids.is_empty() {
            let line = format!("{} was left as it is: none of the {} entries of {} are in the library.", q(&existing.name), r.entries, q(&r.file));
            with_batch(|b| {
                b.lines.push(line);
                b.missing.extend(r.m.missing.iter().map(|m| format!("{}: {m}", r.file)));
            });
            continue;
        }
        match with_batch(|b| b.rule).flatten() {
            // Replace for the rest does not reach a smart playlist: that one asks.
            Some(c) if !(c == Choice::Replace && existing.smart()) => {
                settle(st, r, c);
                continue;
            }
            _ => {}
        }
        let existing = existing.clone();
        return ask_about(st, r, &existing, &all);
    }
}

fn ask_about(st: &Rc<State>, r: Read, existing: &Playlist, all: &[Playlist]) {
    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
    let later: Vec<String> = with_batch(|b| b.pending.iter().map(|p| name_of(p)).collect()).unwrap_or_default();
    let rest = nameclash::later_clashes(&r.name, &later, &names);
    let alt = nameclash::numbered(&r.name, &names);
    let file = match r.m.missing.len() {
        0 => format!("The file {} brings {}.", q(&r.file), nameclash::tracks(r.m.ids.len())),
        n => format!("The file {} brings {} ({n} of its {} entries {} not in the library).", q(&r.file), nameclash::tracks(r.m.ids.len()), r.entries, if n == 1 { "is" } else { "are" }),
    };
    let a = nameclash::Ask {
        title: format!("A playlist called {} already exists", q(&existing.name)),
        text: format!("{}. {file}", nameclash::holds(existing)),
        note: if existing.smart() {
            format!("A smart playlist fills itself from its rules, so a file cannot replace it. Keep both imports the file as {}.", q(&alt))
        } else {
            format!("Replace gives {} the file\u{2019}s tracks instead of its own. Keep both imports the file as {}.", q(&existing.name), q(&alt))
        },
        can_replace: !existing.smart(),
        rest,
    };
    with_batch(|b| b.waiting = Some(r));
    nameclash::ask(st, a, Box::new(resume));
}

/// The prompt's answer about the waiting file; then on with the rest.
fn resume(st: &Rc<State>, choice: Choice, for_rest: bool) {
    let Some(r) = with_batch(|b| {
        if for_rest {
            b.rule = Some(choice);
        }
        b.waiting.take()
    })
    .flatten() else {
        return;
    };
    settle(st, r, choice);
    pump(st);
}

/// Import one file as `choice` says (no clash: as a new playlist).
fn settle(st: &Rc<State>, r: Read, choice: Choice) {
    let all = query::playlists(&st.db).unwrap_or_default();
    let plan = plan(&r.name, &all, choice);
    let line = match carry_out(&st.db, &plan, &r.m.ids) {
        Err(e) => format!("Could not import {}: {e}", q(&r.file)),
        Ok(None) => {
            let kept = nameclash::find(&r.name, &all, None).map_or(r.name.as_str(), |p| p.name.as_str());
            format!("{} was left as it is: {} was not imported.", q(kept), q(&r.file))
        }
        Ok(Some(id)) => {
            let (verb, name) = match &plan {
                Plan::Replace(_, name) => ("Replaced", name.as_str()),
                Plan::Create(name) => ("Imported", name.as_str()),
                Plan::Skip => ("Imported", r.name.as_str()),
            };
            log::info!("playlist import ({verb}): {} of {} entries matched", r.m.ids.len(), r.entries);
            views::playlists_changed(st);
            if matches!(plan, Plan::Replace(..)) && st.open_playlist.get() == Some(id) {
                views::reload_playlist(st, id);
            }
            with_batch(|b| b.last = Some((id, r.m.clone())));
            match (r.entries, r.m.missing.len()) {
                (0, _) => format!("{verb} {}: the file lists no tracks.", q(name)),
                (n, 0) => format!("{verb} {}: all {n} entries matched.", q(name)),
                (n, miss) => format!("{verb} {}: {} of {n} entries matched; {miss} not in the library:", q(name), n - miss),
            }
        }
    };
    with_batch(|b| {
        b.lines.push(line);
        let several = b.several;
        b.missing.extend(r.m.missing.iter().map(|m| if several { format!("{}: {m}", r.file) } else { m.clone() }));
    });
}

/// Every file taken: the Playlists page says how it went (and the pill, elsewhere).
fn finish(st: &Rc<State>) {
    let Some(b) = BATCH.with(|b| b.borrow_mut().take()) else { return };
    LAST.with(|l| *l.borrow_mut() = b.last);
    if b.lines.is_empty() {
        return;
    }
    let on_page = st.app.upgrade().is_some_and(|app| app.global::<Nav>().get_page() == Page::Playlists);
    if !on_page {
        let text = match b.lines.as_slice() {
            [one] => one.trim_end_matches(':').to_owned(),
            many => format!("{} playlist files: the Playlists page says how each went", many.len()),
        };
        views::flash_long(st, &text);
    }
    show(st, b.lines.join("\n"), b.missing);
}

fn show(st: &Rc<State>, summary: String, missing: Vec<String>) {
    let Some(app) = st.app.upgrade() else { return };
    let p = app.global::<Playlists>();
    p.set_import_summary(summary.into());
    let rows: Vec<slint::SharedString> = missing.into_iter().map(Into::into).collect();
    p.set_import_missing(ModelRc::new(VecModel::from(rows)));
}

pub fn dismiss(st: &Rc<State>) {
    show(st, String::new(), Vec::new());
}

pub fn export_dialog(st: &Rc<State>, id: i64, relative: bool) {
    if !crate::dialog_allowed("export playlist") {
        return;
    }
    let name = query::playlist_name(&st.db, id).ok().flatten().unwrap_or_else(|| "Playlist".into());
    let file: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let picked = rfd::FileDialog::new()
        .set_title(if relative { "Export playlist (paths relative to the file)" } else { "Export playlist" })
        .set_file_name(format!("{file}.m3u8"))
        .add_filter("M3U8 playlist", &["m3u8"])
        .save_file();
    if let Some(path) = picked {
        export_file(st, id, &path, relative);
    }
}

/// Write playlist `id` to `path` as M3U8. A smart playlist: what it holds now (worked out
/// first, off the UI thread; written when it is).
pub fn export_file(st: &Rc<State>, id: i64, path: &Path, relative: bool) -> bool {
    if crate::playlists::smart::is_smart(st, id) {
        crate::playlists::smart::act(st, id, crate::playlists::smart::Act::Export { path: path.to_path_buf(), relative });
        return true;
    }
    let tracks = query::playlist_tracks(&st.db, id).unwrap_or_default();
    write_tracks(st, &tracks, path, relative)
}

/// What an export says: how many it wrote, and how many it left out (tracks only the server has
/// have no file for a playlist file to name).
pub fn exported(total: usize, skipped: usize) -> String {
    let written = total - skipped.min(total);
    let n = |k: usize| if k == 1 { "1 track".to_owned() } else { format!("{k} tracks") };
    match skipped {
        0 => format!("Exported {}", n(written)),
        s => format!("Exported {}; left out {} only on the server (no file to name)", n(written), n(s)),
    }
}

/// Write `tracks` to `path` as M3U8, and say how it went.
pub fn write_tracks(st: &Rc<State>, tracks: &[library::Track], path: &Path, relative: bool) -> bool {
    let path: PathBuf = if path.extension().is_none() { path.with_extension("m3u8") } else { path.to_path_buf() };
    match std::fs::write(&path, m3u::export(tracks, &path, relative)) {
        Ok(()) => {
            let skipped = m3u::skipped(tracks);
            let text = exported(tracks.len(), skipped);
            if skipped == 0 { views::flash(st, &text) } else { views::flash_long(st, &text) }
            true
        }
        Err(e) => {
            views::flash_long(st, &format!("Could not export: {e}"));
            false
        }
    }
}

#[cfg(test)]
mod export_tests {
    #[test]
    fn an_export_says_what_it_left_out() {
        assert_eq!(super::exported(12, 0), "Exported 12 tracks");
        assert_eq!(super::exported(1, 0), "Exported 1 track");
        assert_eq!(super::exported(12, 3), "Exported 9 tracks; left out 3 tracks only on the server (no file to name)");
        assert_eq!(super::exported(2, 1), "Exported 1 track; left out 1 track only on the server (no file to name)");
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;

    /// A library of four tracks and the playlists "Road Trip" (tracks 1, 2) and the smart
    /// "Jazz".
    fn library() -> rusqlite::Connection {
        let conn = library::db::open_in_memory().unwrap();
        conn.execute_batch(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES (1, 'k', 'A', 'B', 'M:/a', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
                 (1, 'M:/a/1', 1, 'One', 'B', 1, 0, 0, 0), (2, 'M:/a/2', 1, 'Two', 'B', 1, 0, 0, 0),
                 (3, 'M:/a/3', 1, 'Three', 'B', 1, 0, 0, 0), (4, 'M:/a/4', 1, 'Four', 'B', 1, 0, 0, 0);",
        )
        .unwrap();
        query::create_playlist_with(&conn, "Road Trip", &[1, 2]).unwrap();
        query::create_smart_playlist(&conn, "Jazz", r#"{"rules":[{"field":"genre","op":"is","value":"Jazz"}]}"#).unwrap();
        conn
    }

    fn ids(conn: &rusqlite::Connection, id: i64) -> Vec<i64> {
        query::playlist_tracks(conn, id).unwrap().iter().map(|t| t.id).collect()
    }

    fn names(conn: &rusqlite::Connection) -> Vec<String> {
        query::playlists(conn).unwrap().into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn a_new_name_is_imported_whatever_the_answer() {
        let conn = library();
        let all = query::playlists(&conn).unwrap();
        for c in [Choice::Cancel, Choice::Replace, Choice::KeepBoth] {
            assert_eq!(plan(" Night Drive ", &all, c), Plan::Create("Night Drive".into()), "{c:?}: no clash, nothing to ask");
        }
        let id = carry_out(&conn, &plan("Night Drive", &all, Choice::Cancel), &[4, 3]).unwrap().unwrap();
        assert_eq!(ids(&conn, id), [4, 3]);
    }

    #[test]
    fn replace_gives_the_playlist_the_files_tracks_and_keeps_its_id_and_name() {
        let conn = library();
        let all = query::playlists(&conn).unwrap();
        let road = all.iter().find(|p| p.name == "Road Trip").unwrap().id;
        let p = plan("road  trip", &all, Choice::Replace);
        assert_eq!(p, Plan::Replace(road, "Road Trip".into()));
        assert_eq!(carry_out(&conn, &p, &[3, 4, 3]).unwrap(), Some(road));
        assert_eq!(ids(&conn, road), [3, 4, 3]);
        assert_eq!(names(&conn), ["Jazz", "Road Trip"], "no second playlist");
    }

    #[test]
    fn keep_both_imports_under_the_next_free_number_and_leaves_the_playlist_alone() {
        let conn = library();
        let all = query::playlists(&conn).unwrap();
        let road = all.iter().find(|p| p.name == "Road Trip").unwrap().id;
        let p = plan("Road Trip", &all, Choice::KeepBoth);
        assert_eq!(p, Plan::Create("Road Trip (2)".into()));
        let new = carry_out(&conn, &p, &[4]).unwrap().unwrap();
        assert_ne!(new, road);
        assert_eq!((ids(&conn, road), ids(&conn, new)), (vec![1, 2], vec![4]));
        // Again: "(2)" is taken now.
        let all = query::playlists(&conn).unwrap();
        assert_eq!(plan("ROAD TRIP", &all, Choice::KeepBoth), Plan::Create("ROAD TRIP (3)".into()));
    }

    #[test]
    fn cancel_changes_nothing() {
        let conn = library();
        let all = query::playlists(&conn).unwrap();
        let road = all.iter().find(|p| p.name == "Road Trip").unwrap().id;
        let p = plan("Road Trip", &all, Choice::Cancel);
        assert_eq!(p, Plan::Skip);
        assert_eq!(carry_out(&conn, &p, &[3, 4]).unwrap(), None);
        assert_eq!(ids(&conn, road), [1, 2]);
        assert_eq!(names(&conn), ["Jazz", "Road Trip"]);
    }

    #[test]
    fn a_smart_playlist_is_never_replaced_by_a_file() {
        let conn = library();
        let all = query::playlists(&conn).unwrap();
        assert_eq!(plan("jazz", &all, Choice::Replace), Plan::Skip);
        assert_eq!(plan("jazz", &all, Choice::KeepBoth), Plan::Create("jazz (2)".into()));
        let jazz = all.iter().find(|p| p.name == "Jazz").unwrap();
        assert!(query::set_playlist_tracks(&conn, jazz.id, &[1]).is_err(), "the library refuses it too");
    }

    #[test]
    fn a_file_is_named_after_itself() {
        assert_eq!(name_of(Path::new("C:/lists/Road Trip.m3u8")), "Road Trip");
        assert_eq!(name_of(Path::new("C:/lists/ Spaced .m3u")), "Spaced");
        assert_eq!(name_of(Path::new("C:/lists/.m3u")), ".m3u", "a dot file's stem is its name");
        assert!(is_playlist_file(Path::new("a/B.M3U8")) && is_playlist_file(Path::new("a/b.m3u")));
        assert!(!is_playlist_file(Path::new("a/b.flac")) && !is_playlist_file(Path::new("a/m3u")));
    }
}
