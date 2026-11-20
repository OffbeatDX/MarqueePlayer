//! `MP_BENCH=editing`: tag editing in View metadata (`bench-editing.txt`, `e-*.png`).
//!
//! Needs `MP_MUSIC` with the fixtures of `cargo run -p library --example edit_fixtures`
//! (a folder with the `.mp-edit-fixtures` marker): every bench build refuses to write any
//! other file (`bench::install` sets the editor's write guard). The run:
//!
//! - with tag editing off (the default): a track's view is the read-only viewer (no row
//!   editable, no draft bar; F2 and Ctrl+S do nothing);
//! - turns it on from Settings > Library (its row, by the pointer);
//! - a track, by the keyboard: F2 on the title, a new value typed, Enter; a second artist
//!   (Ctrl+Enter, typed, Alt+Up to put it first); a track number that is not one (the
//!   message, then Esc); Ctrl+S, the review, Enter to save, the result; the file checked
//!   on disk (its tags, its audio unchanged); the save undone from Edit history;
//! - by the pointer: a click on a value edits it; a click outside with a draft asks first;
//! - an album: its inconsistencies' fixes added to the draft, an album field for every
//!   track, a table row edited (Up and Down between rows), the front cover replaced from
//!   an enormous image (picked for the bench: no dialog), reviewed and saved; the library
//!   rescanned and the album checked;
//! - a save stopped by a read-only file: the result, and Undo of what was written;
//! - a playing file saved, playback going on;
//! - the minimum window size: editing, the review.
//!
//! Everything goes through Slint's own event dispatch inside the app; no OS input is sent,
//! nothing goes to the clipboard.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::query;
use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, run_steps, snapshot, type_text, write_report};
use crate::metadata::view::{self, Edit, Kind};
use crate::state::State;
use crate::{Actions, AppWindow, Dialog, Library, Meta, playback, metadata, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("editing bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
    static ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static AUDIO: RefCell<Vec<(PathBuf, String)>> = const { RefCell::new(Vec::new()) };
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
}

fn press(app: &AppWindow, key: Key, mods: &[Key]) {
    crate::bench::press(app, key, mods);
}

/// A key typed as a character with Ctrl (Ctrl+S, Ctrl+A).
fn ctrl(app: &AppWindow, c: &str) {
    use slint::platform::WindowEvent;
    let w = app.window();
    w.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    w.dispatch_event(WindowEvent::KeyPressed { text: c.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: c.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
}

fn click(app: &AppWindow, x: f32, y: f32) {
    use slint::LogicalPosition;
    use slint::platform::{PointerEventButton, WindowEvent};
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position: p });
    w.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
}

fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

/// The fixtures' folder (the library's only root), and a file in it.
fn fixture(rel: &str) -> PathBuf {
    ROOT.with(|r| r.borrow().clone()).unwrap_or_default().join(rel)
}

fn album_id(st: &State, folder_end: &str) -> Option<i64> {
    st.db.query_row("SELECT id FROM albums WHERE folder LIKE ?1 ORDER BY id LIMIT 1", [format!("%{folder_end}")], |r| r.get(0)).ok()
}

fn track_of(st: &State, file: &str) -> Option<library::Track> {
    let id: i64 = st.db.query_row("SELECT id FROM tracks WHERE path LIKE ?1", [format!("%{file}")], |r| r.get(0)).ok()?;
    query::track(&st.db, id).ok().flatten()
}

/// The first line that edits this way.
fn row(pred: impl Fn(&view::Line) -> bool) -> Option<i32> {
    view::lines().iter().position(pred).map(|i| i as i32)
}

fn field_row(key: &str) -> Option<i32> {
    row(|l| matches!(&l.edit, Some(Edit::Field(k) | Edit::Album(k) | Edit::Remove(k)) if k == key))
}

fn state(app: &AppWindow) -> String {
    let m = app.global::<Meta>();
    format!(
        "open {}, edit-on {}, stage {}, edit-row {}, values {:?}, error {:?}, draft {:?}, cursor {}",
        app.global::<Dialog>().get_meta_open(),
        m.get_edit_on(),
        m.get_stage(),
        m.get_edit_row(),
        (0..m.get_edit_values().row_count()).filter_map(|i| m.get_edit_values().row_data(i)).map(|s| s.to_string()).collect::<Vec<_>>(),
        m.get_edit_error().to_string(),
        m.get_draft_text().to_string(),
        m.get_cursor()
    )
}

fn tags_of(p: &Path) -> library::tags::TrackTags {
    library::tags::read(p, false).map(|t| t.0).unwrap_or_default()
}

/// Remember a file's audio (the demuxed packets), to compare after saves.
fn keep_audio(p: &Path) {
    let a = library::edit::audio_print(p).unwrap_or_default();
    AUDIO.with(|x| x.borrow_mut().push((p.to_path_buf(), a)));
}

fn audio_same(p: &Path) -> bool {
    let now = library::edit::audio_print(p).unwrap_or_default();
    AUDIO.with(|x| x.borrow().iter().any(|(q, a)| q == p && *a == now && !a.is_empty()))
}

/// Select all of the field with the keyboard (Ctrl+A) and type over it.
fn retype(app: &AppWindow, text: &str) {
    ctrl(app, "a");
    type_text(app, text);
}

pub fn run() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }

    step!(500, |st, app, r| {
        let root = st.settings.borrow().folders.first().cloned().unwrap_or_default();
        let marked = root.join(".mp-edit-fixtures").is_file();
        check(r, "the library is the editor's fixtures (marked)", marked, root.display().to_string());
        ROOT.with(|x| *x.borrow_mut() = Some(root));
        line(r, format!("library: {:?}; window {:?}", query::counts(&st.db), size(app)));
        check(r, "tag editing is off by default", !metadata::tagedit::enabled(st), String::new());
        for f in ["Inconsistent Album/01 Patch One.flac", "Inconsistent Album/02 Patch Two.flac", "Inconsistent Album/06 Patch Six.flac", "Inconsistent Album/07 Patch Seven.mp3", "Inconsistent Album/Bonus.flac", "ID3 Formats/02 Twenty Three.mp3", "More Formats/14 Long Play.flac"] {
            keep_audio(&fixture(f));
        }
    });

    // ---- off: the read-only viewer --------------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(t) = track_of(st, "01 Patch One.flac") {
            view::open_track(st, t);
        }
    });
    step!(900, |_st, app, r| {
        line(r, "[off]".into());
        let editable = view::lines().iter().filter(|l| l.edit.is_some()).count();
        check(r, "no row is editable, no draft bar", editable == 0 && !app.global::<Meta>().get_edit_on(), state(app));
        let rows = app.global::<Meta>().get_rows();
        let marked = (0..rows.row_count()).filter_map(|i| rows.row_data(i)).filter(|x| x.edit != 0 || x.changed || x.empty).count();
        check(r, "the panel's rows carry nothing of editing", marked == 0, format!("{marked} rows"));
        let before = view::lines().len();
        press(app, Key::F2, &[]);
        ctrl(app, "s");
        check(r, "F2 and Ctrl+S do nothing", app.global::<Meta>().get_edit_row() == -1 && app.global::<Meta>().get_stage() == 0 && view::lines().len() == before, state(app));
        snapshot(app, "e-off-track");
        press(app, Key::Escape, &[]);
    });

    // ---- on, from Settings > Library ------------------------------------------------------
    step!(300, |st, _app, _r| views::settings_section(st, crate::settings::nav::LIBRARY));
    step!(700, |st, app, r| {
        line(r, "[settings]".into());
        snapshot(app, "e-settings-off");
        app.global::<Actions>().invoke_toggle_tag_editing();
        check(r, "turned on", metadata::tagedit::enabled(st) && app.global::<Library>().get_tag_editing(), format!("history {:?}", app.global::<Library>().get_edit_history().to_string()));
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "e-settings-on");
    });

    // ---- a track, by the keyboard ---------------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(t) = track_of(st, "01 Patch One.flac") {
            view::open_track(st, t);
        }
    });
    step!(900, |_st, app, r| {
        line(r, "[track]".into());
        let editable: Vec<String> = view::lines().iter().filter(|l| l.edit.is_some()).map(|l| l.label.clone()).collect();
        check(r, "fields are editable, the common ones there even when not tagged", editable.iter().any(|l| l == "Album artist") && editable.iter().any(|l| l == "Composer"), format!("{editable:?}"));
        snapshot(app, "e-track");
        let Some(i) = field_row("TITLE") else {
            check(r, "a title row", false, String::new());
            return;
        };
        app.global::<Meta>().set_cursor(i);
        app.global::<Meta>().set_focus_seq(app.global::<Meta>().get_focus_seq() + 1);
    });
    step!(200, |_st, app, r| {
        press(app, Key::F2, &[]);
        check(r, "F2 edits the field at the cursor", app.global::<Meta>().get_edit_row() == app.global::<Meta>().get_cursor(), state(app));
    });
    step!(200, |_st, app, r| {
        retype(app, "Patch One, Edited");
        check(r, "the field has the keyboard", app.global::<Meta>().get_edit_row() >= 0 && app.global::<Meta>().get_edit_slot() == 0, state(app));
        snapshot(app, "e-track-editing");
        press(app, Key::Return, &[]);
    });
    step!(300, |_st, app, r| {
        let l = view::lines().into_iter().find(|l| matches!(&l.edit, Some(Edit::Field(k)) if k == "TITLE"));
        check(r, "Enter keeps it in the draft, marked, saying what it was", l.as_ref().is_some_and(|l| l.changed && l.values == ["Patch One, Edited"] && l.note.contains("Patch One")), format!("{:?}", l.map(|l| (l.values, l.note))));
        check(r, "the draft bar says so", app.global::<Meta>().get_draft_text() == "1 unsaved change", state(app));
        let lib = view::lines().into_iter().find(|l| l.kind == Kind::Field && l.label == "Title" && l.edit.is_none());
        check(r, "In the library shows what it will be", lib.as_ref().is_some_and(|l| l.changed && l.note.starts_with("Once saved")), format!("{:?}", lib.map(|l| (l.values, l.note))));
        snapshot(app, "e-track-draft");
        // Artists: a second value, put first.
        if let Some(i) = field_row("ARTIST") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::Return, &[]);
        }
    });
    step!(200, |_st, app, r| {
        check(r, "Enter edits too", app.global::<Meta>().get_edit_row() >= 0, state(app));
        press(app, Key::Return, &[Key::Control]);
    });
    step!(200, |_st, app, r| {
        check(r, "Ctrl+Enter adds a value", app.global::<Meta>().get_edit_values().row_count() == 2 && app.global::<Meta>().get_edit_slot() == 1, state(app));
        type_text(app, "Guest Singer");
        press(app, Key::UpArrow, &[Key::Alt]);
    });
    step!(200, |_st, app, r| {
        check(r, "Alt+Up moves it first", state(app).contains("[\"Guest Singer\", \"The Menders\"]"), state(app));
        snapshot(app, "e-track-multi");
        press(app, Key::Return, &[]);
    });
    step!(300, |_st, app, r| {
        let l = view::lines().into_iter().find(|l| matches!(&l.edit, Some(Edit::Field(k)) if k == "ARTIST"));
        check(r, "two artists in the draft, in that order", l.as_ref().is_some_and(|l| l.values == ["Guest Singer", "The Menders"]), format!("{:?}", l.map(|l| l.values)));
        // A track number that is not one.
        if let Some(i) = field_row("TRACKNUMBER") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, r| {
        retype(app, "A1");
        press(app, Key::Return, &[]);
        check(r, "an invalid number is refused there, with why", app.global::<Meta>().get_edit_row() >= 0 && app.global::<Meta>().get_edit_error().contains("digits"), state(app));
        snapshot(app, "e-track-invalid");
        press(app, Key::Escape, &[]);
    });
    step!(200, |_st, app, r| {
        check(r, "Esc cancels the field, the draft as it was", app.global::<Meta>().get_edit_row() == -1 && app.global::<Meta>().get_draft_text() == "2 unsaved changes", state(app));
        // "3/12" typed as a number: number and total.
        if let Some(i) = field_row("TRACKNUMBER") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| retype(app, "1/7"));
    step!(200, |_st, app, _r| press(app, Key::Return, &[]));
    step!(300, |_st, app, r| {
        // (Its number was 1 already: the total is the change.)
        check(r, "1/7 is the number and the total", app.global::<Meta>().get_draft_text() == "3 unsaved changes" && view::lines().iter().any(|l| matches!(&l.edit, Some(Edit::Field(k)) if k == "TRACKTOTAL") && l.changed && l.values == ["7"]), state(app));
        ctrl(app, "s");
    });
    step!(400, |_st, app, r| {
        line(r, "[review]".into());
        let rows = app.global::<Meta>().get_review_rows();
        let text: Vec<String> = (0..rows.row_count()).filter_map(|i| rows.row_data(i)).map(|x| if x.kind == 0 { format!("# {}", x.label) } else { x.values.row_data(0).map(|s| s.to_string()).unwrap_or_default() }).collect();
        check(r, "Ctrl+S reviews: the file, each change old to new", app.global::<Meta>().get_stage() == 1 && text.iter().any(|t| t.contains("Title: \u{201c}Patch One\u{201d} \u{2192} \u{201c}Patch One, Edited\u{201d}")), format!("{text:?}"));
        check(r, "the button says how many files", app.global::<Meta>().get_save_label() == "Save 1 file", app.global::<Meta>().get_save_label().to_string());
        snapshot(app, "e-review");
        press(app, Key::Return, &[]);
    });
    step!(1500, |_st, app, r| {
        line(r, "[saved]".into());
        check(r, "saved, said so", app.global::<Meta>().get_stage() == 3 && app.global::<Meta>().get_result_title() == "Saved 1 file.", format!("{:?} {:?}", app.global::<Meta>().get_result_title().to_string(), app.global::<Meta>().get_result_text().to_string()));
        snapshot(app, "e-result");
        let p = fixture("Inconsistent Album/01 Patch One.flac");
        let t = tags_of(&p);
        check(r, "the file holds it", t.title.as_deref() == Some("Patch One, Edited") && t.artist.as_deref() == Some("Guest Singer; The Menders") && t.track_no == Some(1), format!("{:?} / {:?} / {:?}", t.title, t.artist, t.track_no));
        check(r, "its audio is the same", audio_same(&p), String::new());
        check(r, "no draft left", app.global::<Meta>().get_draft_text().is_empty(), state(app));
        press(app, Key::Return, &[]);
    });
    step!(2500, |st, app, r| {
        // The rescan that follows the save has run (a partial scan of the folder).
        let t = track_of(st, "01 Patch One.flac");
        check(r, "the library took it (same track id)", t.as_ref().is_some_and(|t| t.title == "Patch One, Edited"), format!("{:?}", t.map(|t| (t.id, t.title))));
        let lib = view::lines().into_iter().find(|l| l.kind == Kind::Field && l.label == "Title" && l.edit.is_none());
        check(r, "the panel shows the saved state, not stale", lib.as_ref().is_some_and(|l| l.values == ["Patch One, Edited"] && !l.attention && !l.changed), format!("{:?}", lib.map(|l| (l.values, l.note))));
        let undo = row(|l| matches!(l.act, view::Act::Undo(_)));
        check(r, "Edit history lists the save", undo.is_some(), String::new());
        snapshot(app, "e-history");
        if let Some(i) = undo {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::Return, &[]);
        }
    });
    step!(400, |_st, app, r| {
        check(r, "Undo reviews first", app.global::<Meta>().get_stage() == 1 && app.global::<Meta>().get_review_title().starts_with("Undo this save?"), app.global::<Meta>().get_review_title().to_string());
        snapshot(app, "e-undo-review");
        press(app, Key::Return, &[]);
    });
    step!(1500, |_st, app, r| {
        let p = fixture("Inconsistent Album/01 Patch One.flac");
        let t = tags_of(&p);
        check(r, "undone: the file as it was", app.global::<Meta>().get_result_title() == "Undone 1 file." && t.title.as_deref() == Some("Patch One") && t.artist.as_deref() == Some("The Menders"), format!("{:?} {:?} {:?}", app.global::<Meta>().get_result_title().to_string(), t.title, t.artist));
        check(r, "its audio is the same", audio_same(&p), String::new());
        press(app, Key::Escape, &[]);
    });

    // ---- by the pointer -------------------------------------------------------------------
    step!(500, |_st, app, r| {
        line(r, "[pointer]".into());
        // Down the value column from the top of the list, until a click starts an edit.
        let (w, h) = size(app);
        let pw = (w - 48.0).min(960.0);
        let ph = (h - 40.0).min(980.0);
        let (px, py) = ((w - pw) / 2.0, (h - ph) / 2.0);
        app.global::<Meta>().set_scroll(0.0);
        let mut hit = None;
        let mut y = py + 200.0;
        while y < py + ph - 40.0 {
            click(app, px + 14.0 + 10.0 + 176.0 + 16.0 + 60.0, y);
            if app.global::<Meta>().get_edit_row() >= 0 {
                hit = Some(y);
                break;
            }
            y += 6.0;
        }
        let edited = view::lines().get(app.global::<Meta>().get_edit_row().max(0) as usize).map(|l| l.label.clone());
        check(r, "a click on a value edits it", hit.is_some(), format!("at y {hit:?}: {edited:?}; {}", state(app)));
        snapshot(app, "e-pointer-edit");
        retype(app, "Clicked In");
        press(app, Key::Return, &[]);
    });
    step!(300, |_st, app, r| {
        check(r, "a draft", !app.global::<Meta>().get_draft_text().is_empty(), state(app));
        click(app, 6.0, 6.0);
    });
    step!(300, |_st, app, r| {
        check(r, "a click outside with a draft asks first", app.global::<Dialog>().get_meta_open() && app.global::<Meta>().get_stage() == 4, state(app));
        snapshot(app, "e-confirm");
        press(app, Key::Escape, &[]);
    });
    step!(200, |_st, app, r| {
        check(r, "Esc: keep editing", app.global::<Meta>().get_stage() == 0 && !app.global::<Meta>().get_draft_text().is_empty(), state(app));
        // The tray's Quit with the draft unsaved: nothing quits, the panel asks.
        let quit = crate::request_quit();
        check(r, "Quit from the tray with unsaved edits does not quit, it asks", !quit && app.global::<Dialog>().get_meta_open() && app.global::<Meta>().get_stage() == 4, state(app));
        press(app, Key::Escape, &[]);
    });
    step!(200, |_st, _app, _r| {
        crate::bench::press(_app, Key::Escape, &[]);
    });
    step!(200, |_st, app, r| {
        check(r, "Esc with a draft asks again", app.global::<Meta>().get_stage() == 4, state(app));
        press(app, Key::Tab, &[]);
        press(app, Key::Return, &[]);
    });
    step!(300, |_st, app, r| {
        check(r, "Discard and close: closed, nothing written", !app.global::<Dialog>().get_meta_open() && tags_of(&fixture("Inconsistent Album/01 Patch One.flac")).title.as_deref() == Some("Patch One"), state(app));
    });

    // ---- an album -------------------------------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Inconsistent Album") {
            view::open_album(st, id);
        }
    });
    step!(1500, |_st, app, r| {
        line(r, "[album]".into());
        let fixes: Vec<String> = view::lines().iter().filter(|l| matches!(l.act, view::Act::Fix(_))).map(|l| l.values.join("")).collect();
        check(r, "each inconsistency offers its fix", fixes.iter().any(|f| f.contains("album artist")) && fixes.iter().any(|f| f.starts_with("Number the tracks")), format!("{fixes:?}"));
        snapshot(app, "e-album");
        if let Some(i) = row(|l| matches!(l.act, view::Act::Fix(_)) && l.values.join("").contains("album artist")) {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::Return, &[]);
        }
    });
    step!(300, |_st, app, r| {
        check(r, "the fix is a draft to review", app.global::<Meta>().get_draft_text() == "1 unsaved change", state(app));
        if let Some(i) = row(|l| matches!(l.act, view::Act::Fix(_)) && l.values.join("").starts_with("Number the tracks")) {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::Return, &[]);
        }
    });
    step!(300, |_st, app, r| {
        let nums: Vec<String> = view::lines().iter().filter(|l| l.kind == Kind::Track).map(|l| l.cells.get(1).cloned().unwrap_or_default()).collect();
        let mut sorted: Vec<u32> = nums.iter().filter_map(|n| n.parse().ok()).collect();
        sorted.sort_unstable();
        let bonus = view::lines().into_iter().find(|l| l.kind == Kind::Track && l.cells.get(2).is_some_and(|c| c == "Bonus Patch")).and_then(|l| l.cells.get(1).cloned());
        check(r, "numbered in file order, in the table", sorted == (1..=7).collect::<Vec<u32>>() && bonus.as_deref() == Some("7"), format!("{nums:?}, Bonus {bonus:?}"));
        snapshot(app, "e-album-fixes");
        // An album field, for every track: the label.
        if let Some(i) = field_row("LABEL") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| type_text(app, "Lantern Records"));
    step!(200, |_st, app, _r| press(app, Key::Return, &[]));
    step!(300, |_st, app, r| {
        let label = view::lines().into_iter().find(|l| matches!(&l.edit, Some(Edit::Album(k)) if k == "LABEL"));
        check(r, "the label on all 7 tracks", label.as_ref().is_some_and(|l| l.changed && l.note == "On 7 tracks when you save; they had none before"), format!("{:?}", label.map(|l| (l.values, l.note))));
        // A table row: F2, the title, Down to the next row, Esc.
        if let Some(i) = row(|l| l.kind == Kind::Track && l.cells.get(2).is_some_and(|c| c == "Patch Two")) {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| press(app, Key::Tab, &[]));
    step!(200, |_st, app, _r| retype(app, "Patch Two (Live)"));
    step!(300, |_st, app, r| {
        check(r, "the row's three cells, the title being typed", app.global::<Meta>().get_edit_kind() == 1 && app.global::<Meta>().get_edit_slot() == 1, state(app));
        snapshot(app, "e-album-row-editing");
        press(app, Key::DownArrow, &[]);
    });
    step!(300, |_st, app, r| {
        let two = view::lines().into_iter().find(|l| l.kind == Kind::Track && l.cells.get(2).is_some_and(|c| c == "Patch Two (Live)"));
        check(r, "Down keeps the row and edits the next, same cell", two.is_some_and(|l| l.cell_changed[1]) && app.global::<Meta>().get_edit_slot() == 1 && app.global::<Meta>().get_edit_row() >= 0, state(app));
    });
    step!(300, |_st, app, _r| press(app, Key::Escape, &[]));
    step!(300, |_st, app, r| {
        check(r, "Esc in a row: the row closes, the list has the keys", app.global::<Meta>().get_edit_row() == -1, state(app));
        // The front cover, from an enormous image (the bench's pick).
        let big = crate::bench::out_dir().join("e-picked-cover.png");
        let img = image::RgbImage::from_fn(4200, 4200, |x, y| image::Rgb([(x / 17) as u8, (y / 17) as u8, 120]));
        let _ = img.save(&big);
        metadata::tagedit::PICKED.with(|p| *p.borrow_mut() = Some(big));
        if let Some(i) = row(|l| matches!(l.act, view::Act::Cover(view::CoverAct::Pick))) {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::Return, &[]);
        }
    });
    step!(2500, |_st, app, r| {
        let new = view::lines().into_iter().find(|l| l.label == "New front cover");
        check(r, "the new cover, made smaller, on how many files", new.as_ref().is_some_and(|l| l.values.join("").contains("2,000 \u{d7} 2,000") && l.note.contains("4,200") && l.note.contains("7 tracks")), format!("{:?}", new.map(|l| (l.values, l.note))));
        let rows = app.global::<Meta>().get_rows();
        let thumb = (0..rows.row_count()).filter_map(|i| rows.row_data(i)).any(|x| x.label == "New front cover" && x.has_thumb);
        check(r, "with its thumbnail", thumb, String::new());
        snapshot(app, "e-album-cover");
        ctrl(app, "s");
    });
    step!(500, |_st, app, r| {
        check(r, "the review lists every file", app.global::<Meta>().get_stage() == 1 && app.global::<Meta>().get_save_label() == "Save 7 files", format!("{:?} {:?}", app.global::<Meta>().get_review_title().to_string(), app.global::<Meta>().get_save_label().to_string()));
        snapshot(app, "e-album-review");
        press(app, Key::Return, &[]);
    });
    step!(60, |_st, app, r| {
        line(r, format!("  while saving: {}; {:?}", state(app), app.global::<Meta>().get_progress_text().to_string()));
        snapshot(app, "e-album-saving");
    });
    step!(4000, |st, app, r| {
        check(r, "saved 7 files", app.global::<Meta>().get_result_title() == "Saved 7 files.", format!("{:?} {:?}", app.global::<Meta>().get_result_title().to_string(), app.global::<Meta>().get_result_text().to_string()));
        snapshot(app, "e-album-result");
        for f in ["01 Patch One.flac", "02 Patch Two.flac", "06 Patch Six.flac", "07 Patch Seven.mp3", "Bonus.flac"] {
            let p = fixture(&format!("Inconsistent Album/{f}"));
            let t = tags_of(&p);
            let art = library::tags::read(&p, true).ok().and_then(|x| x.1).and_then(|d| library::meta::image_size(&d));
            check(r, &format!("{f}: tags, cover, audio"), t.label.as_deref() == Some("Lantern Records") && t.album_artist.as_deref() == Some("The Menders") && art == Some((2000, 2000)) && audio_same(&p), format!("{:?} {:?} {:?} {art:?}", t.label, t.album_artist, t.track_no));
        }
        let six = tags_of(&fixture("Inconsistent Album/06 Patch Six.flac"));
        let bonus = tags_of(&fixture("Inconsistent Album/Bonus.flac"));
        check(r, "numbered", six.track_no == Some(5) && bonus.track_no == Some(7), format!("{:?} {:?}", six.track_no, bonus.track_no));
        let _ = st;
        press(app, Key::Return, &[]);
    });
    step!(3000, |st, app, r| {
        // The library, after the rescan the save asked for.
        let id = album_id(st, "Inconsistent Album");
        let facts = id.and_then(|id| query::album_facts(&st.db, id).ok().flatten());
        check(r, "the library's album: its album artist and label", facts.as_ref().is_some_and(|f| f.album_artist == "The Menders" && f.label.as_deref() == Some("Lantern Records")), format!("{:?}", facts.map(|f| (f.album_artist, f.label))));
        let nums: Vec<Option<u32>> = id.map(|id| query::album_tracks(&st.db, id).unwrap_or_default().iter().map(|t| t.track_no).collect()).unwrap_or_default();
        check(r, "the library's numbers", nums == (1..=7).map(Some).collect::<Vec<_>>(), format!("{nums:?}"));
        let n = st.db.query_row("SELECT COUNT(*) FROM tracks", [], |x| x.get::<_, i64>(0)).unwrap_or(0);
        line(r, format!("  tracks in the library: {n}; scanning {}", app.global::<Library>().get_scanning()));
        snapshot(app, "e-album-after-scan");
    });

    // ---- a save that stops half way ---------------------------------------------------------
    step!(300, |_st, app, r| {
        line(r, "[partial]".into());
        let p = fixture("Inconsistent Album/07 Patch Seven.mp3");
        if let Ok(m) = std::fs::metadata(&p) {
            let mut perm = m.permissions();
            perm.set_readonly(true);
            let _ = std::fs::set_permissions(&p, perm);
        }
        if let Some(i) = field_row("LABEL") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| retype(app, "Second Label"));
    step!(200, |_st, app, _r| press(app, Key::Return, &[]));
    step!(300, |_st, app, _r| ctrl(app, "s"));
    step!(400, |_st, app, _r| press(app, Key::Return, &[]));
    step!(3000, |_st, app, r| {
        let m = app.global::<Meta>();
        check(r, "stopped at the read-only file, said which were written and which not", m.get_result_title() == "Saved 5 of 7 files." && m.get_result_text().contains("07 Patch Seven.mp3 could not be saved: the file is read-only") && m.get_result_text().contains("Bonus.flac") && m.get_result_can_undo(), format!("{:?} {:?}", m.get_result_title().to_string(), m.get_result_text().to_string()));
        check(r, "what was not written is still a draft", m.get_draft_text() == "2 unsaved changes in 2 files", state(app));
        snapshot(app, "e-partial-result");
        press(app, Key::Tab, &[]);
        press(app, Key::Return, &[]);
    });
    step!(400, |_st, app, r| {
        check(r, "Undo what was saved: its review", app.global::<Meta>().get_stage() == 1 && app.global::<Meta>().get_save_label() == "Undo in 5 files", app.global::<Meta>().get_save_label().to_string());
        press(app, Key::Return, &[]);
    });
    step!(3000, |_st, app, r| {
        let one = tags_of(&fixture("Inconsistent Album/01 Patch One.flac"));
        check(r, "undone in the 5", app.global::<Meta>().get_result_title() == "Undone 5 files." && one.label.as_deref() == Some("Lantern Records"), format!("{:?} {:?}", app.global::<Meta>().get_result_title().to_string(), one.label));
        let p = fixture("Inconsistent Album/07 Patch Seven.mp3");
        if let Ok(m) = std::fs::metadata(&p) {
            let mut perm = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&p, perm);
        }
        press(app, Key::Return, &[]);
        app.global::<Actions>().invoke_meta_discard();
    });
    step!(300, |_st, app, _r| {
        press(app, Key::Escape, &[]);
    });
    step!(300, |_st, app, r| {
        check(r, "closed (nothing left to ask about)", !app.global::<Dialog>().get_meta_open(), state(app));
    });

    // ---- a file that is playing -------------------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(t) = track_of(st, "14 Long Play.flac") {
            playback::play_tracks(st, vec![t], 0);
        }
    });
    step!(1200, |st, _app, r| {
        line(r, "[playing]".into());
        let s = st.player.status();
        line(r, format!("  playing: {:?} at {:.2} s", s.state, s.position));
        r.borrow_mut().push_str("");
        if let Some(t) = track_of(st, "14 Long Play.flac") {
            view::open_track(st, t);
        }
        POS.with(|p| p.set(s.position));
    });
    step!(800, |_st, app, _r| {
        if let Some(i) = field_row("TITLE") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| retype(app, "Long Play, Saved While Playing"));
    step!(200, |_st, app, _r| press(app, Key::Return, &[]));
    step!(200, |_st, app, _r| ctrl(app, "s"));
    step!(300, |_st, app, _r| press(app, Key::Return, &[]));
    step!(1500, |st, app, r| {
        let s = st.player.status();
        let before = POS.with(Cell::get);
        check(r, "saved while playing", app.global::<Meta>().get_result_title() == "Saved 1 file.", app.global::<Meta>().get_result_text().to_string());
        check(r, "playback went on", s.position > before + 1.0 && st.play_failures.get() == 0, format!("{:?} at {:.2} s (was {before:.2} s), failures {}", s.state, s.position, st.play_failures.get()));
        let p = fixture("More Formats/14 Long Play.flac");
        check(r, "the file holds it; its audio the same", tags_of(&p).title.as_deref() == Some("Long Play, Saved While Playing") && audio_same(&p), String::new());
        press(app, Key::Return, &[]);
        playback::set_paused(st, true);
    });

    // ---- the minimum window size ------------------------------------------------------------
    step!(300, |st, _app, _r| crate::platform::devwindow::resize_client(st.hwnd.get(), 960, 620));
    step!(1000, |_st, app, r| {
        line(r, format!("[min] window {:?}", size(app)));
        if let Some(i) = field_row("ALBUMARTIST") {
            app.global::<Meta>().set_cursor(i);
            press(app, Key::F2, &[]);
        }
    });
    step!(200, |_st, app, _r| press(app, Key::Return, &[Key::Control]));
    step!(200, |_st, app, _r| type_text(app, "Second Artist"));
    step!(400, |_st, app, _r| {
        snapshot(app, "e-min-editing");
        press(app, Key::Return, &[]);
    });
    step!(200, |_st, app, _r| ctrl(app, "s"));
    step!(500, |_st, app, _r| {
        snapshot(app, "e-min-review");
        press(app, Key::Escape, &[]);
    });
    step!(300, |_st, app, _r| press(app, Key::Escape, &[]));
    step!(300, |_st, app, _r| {
        snapshot(app, "e-min-confirm");
        app.global::<Actions>().invoke_meta_discard_close();
    });
    step!(300, |st, _app, _r| crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void));

    step!(800, |st, _app, r| {
        let (bytes, saves) = metadata::tagedit::journal(st).size();
        line(r, format!("journal: {saves} saves, {bytes} bytes"));
        let fails = FAILS.with(Cell::get);
        line(r, format!("\n{fails} checks failed"));
        write_report("editing", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

thread_local! {
    static POS: Cell<f64> = const { Cell::new(0.0) };
}
