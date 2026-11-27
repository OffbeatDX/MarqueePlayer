//! `MP_BENCH=metadata`: "View metadata" (`bench-metadata.txt`, `m-*.png` snapshots).
//!
//! Needs `MP_MUSIC` with the fixtures of `cargo run -p library --example meta_fixtures`.
//! For each fixture track the view is opened from its track list's menu by the pointer and
//! snapshotted (the top, and scrolled down where there is more); from the keyboard (Tab to
//! the list, Menu, Down to the entry, Enter; the cursor, expanding, Ctrl+C, Tab round the
//! panel, typing that must not reach the shortcuts, Esc and where the focus goes); albums
//! from a tile's menu and the album page's More button, an inconsistent album's checks and
//! table, a row to its track and Back to where the album was left; the queue's menu; a
//! file deleted and a file broken after the scan; middle-click scrolling inside the panel;
//! a click outside; the minimum window size. The clipboard is put back as it was.
//! Everything goes through Slint's own event dispatch inside the app; no OS input is sent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use library::query;
use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::autoscroll::{self, Button, Input};
use crate::bench::{Step, menu_labels, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::metadata::view::{self, Kind};
use crate::state::State;
use crate::{AppWindow, Dialog, Meta, MenuStore, Nav, Page, Player, playback, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("metadata bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
    static T0: Cell<Option<Instant>> = const { Cell::new(None) };
    static SAVED_CLIP: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
}

/// An album by its title, or (for the inconsistent one) by its folder's name.
fn album_id(st: &State, title: &str) -> Option<i64> {
    let folder = if title == "Patchwork" { "%Inconsistent Album" } else { "" };
    st.db
        .query_row("SELECT id FROM albums WHERE title = ?1 OR (?2 != '' AND folder LIKE ?2) ORDER BY id LIMIT 1", [title, folder], |r| r.get(0))
        .ok()
}

/// The index of a track (by file name) in the album page's list.
fn row_of(st: &State, file: &str) -> Option<i32> {
    let m = &st.lists.album;
    (0..m.row_count()).find(|&i| m.get(i).is_some_and(|t| t.path.ends_with(file))).map(|i| i as i32)
}

fn open_menu_entry(st: &Rc<State>, app: &AppWindow, label: &str) -> bool {
    match menu_labels(app).iter().position(|l| l == label) {
        Some(i) => {
            crate::menus::activate(st, i as i32, -1);
            true
        }
        None => false,
    }
}

/// The view's lines, short: "label = value" for fields, "# heading", "! note".
fn summary(limit: usize) -> Vec<String> {
    view::lines()
        .iter()
        .take(limit)
        .map(|l| {
            let v = l.values.join(" | ");
            let v = if v.chars().count() > 60 { format!("{}…", v.chars().take(60).collect::<String>()) } else { v };
            match l.kind {
                Kind::Heading => format!("# {}", l.label),
                Kind::Note => format!("! {v}"),
                Kind::Track => format!("{} {} {} [{}]", l.cells.get(1).cloned().unwrap_or_default(), l.label, l.cells.get(5).cloned().unwrap_or_default(), l.note),
                Kind::TableHead => "(table head)".into(),
                _ => format!("{} = {v}{}{}", l.label, if l.note.is_empty() { String::new() } else { format!("  ({})", l.note) }, if l.attention { " !" } else { "" }),
            }
        })
        .collect()
}

/// The first field (not heading) of that name.
fn find(label: &str) -> Option<view::Line> {
    view::lines().into_iter().find(|l| l.label == label && l.kind != Kind::Heading)
}

fn meta_state(app: &AppWindow) -> String {
    let m = app.global::<Meta>();
    format!(
        "open {}, {} {:?}, blocks {:?}, status {:?}, cursor {}, rows {}, can-back {}, scroll {:.0}",
        app.global::<Dialog>().get_meta_open(),
        m.get_eyebrow(),
        m.get_title().to_string(),
        m.get_blocks().to_string(),
        m.get_status().to_string(),
        m.get_cursor(),
        m.get_rows().row_count(),
        m.get_can_back(),
        m.get_scroll()
    )
}

/// Scroll the panel's list `px` down (keep it within the list's length).
fn scroll_to(app: &AppWindow, px: f32) {
    app.global::<Meta>().set_scroll(-px);
}

fn clipboard() -> Option<String> {
    crate::explorer::bench_clipboard()
}

/// The pointer, through the autoscroll filter and then Slint (as bench/autoscroll.rs does).
fn send(app: &AppWindow, ev: Input) -> bool {
    use slint::LogicalPosition;
    use slint::platform::{PointerEventButton, WindowEvent};
    if autoscroll::handle(app, ev) {
        return true;
    }
    let w = app.window();
    let b = |b: Button| match b {
        Button::Left => PointerEventButton::Left,
        Button::Right => PointerEventButton::Right,
        Button::Middle => PointerEventButton::Middle,
        Button::Other => PointerEventButton::Other,
    };
    match ev {
        Input::Move(x, y) => w.dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) }),
        Input::Press(k, x, y) => w.dispatch_event(WindowEvent::PointerPressed { position: LogicalPosition::new(x, y), button: b(k) }),
        Input::Release(k, x, y) => w.dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: b(k) }),
        _ => {}
    }
    false
}

fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

/// (file in the album, album, snapshot name, scroll down for a second look)
const TRACKS: [(&str, &str, &str, f32); 8] = [
    ("01 Northern Lights.flac", "Picard Set", "picard-flac", 1500.0),
    ("01 Twenty Four.mp3", "ID3 Formats", "id3v24", 900.0),
    ("02 Twenty Three.mp3", "ID3 Formats", "id3v23-v1", 0.0),
    ("01 Opus R128.opus", "Other Formats", "opus-r128", 0.0),
    ("02 AAC Atoms.m4a", "Other Formats", "m4a", 700.0),
    ("No Tags.wav", "Untagged", "wav-untagged", 0.0),
    ("01 Long Comment.flac", "Edge Cases", "long-comment", 600.0),
    ("06 Patch Six.flac", "Patchwork", "inconsistent-track", 0.0),
];

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
        SAVED_CLIP.with(|c| *c.borrow_mut() = clipboard());
        line(r, format!("library: {:?}", query::counts(&st.db)));
        let (w, h) = size(app);
        line(r, format!("window {w}x{h}"));
        let albums: Vec<String> = query::all_albums(&st.db, query::AlbumSort::Title, false).unwrap_or_default().into_iter().map(|a| a.title).collect();
        line(r, format!("albums: {albums:?}"));
    });

    // ---- each fixture, from its track list's menu, by the pointer -----------------------
    for (file, album, name, down) in TRACKS {
        step!(300, move |st, _app, _r| {
            if let Some(id) = album_id(st, album) {
                views::open_album(st, id);
            }
        });
        step!(700, move |st, app, r| {
            line(r, format!("[{name}] {album} / {file}"));
            let Some(i) = row_of(st, file) else {
                check(r, "track listed", false, format!("{file} not in {album}"));
                return;
            };
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            let labels = menu_labels(app);
            check(r, "the track menu has View metadata, last", labels.last().map(String::as_str) == Some("View metadata"), format!("{labels:?}"));
            let t0 = Instant::now();
            open_menu_entry(st, app, "View metadata");
            let shown = t0.elapsed();
            check(r, "opens at once with what the library knows", app.global::<Dialog>().get_meta_open() && !view::lines().is_empty(), format!("{:.1} ms; {}", shown.as_secs_f64() * 1000.0, meta_state(app)));
            // When the file's read arrives (polled here only; the app itself is told).
            let rr = r.clone();
            let timer = Rc::new(slint::Timer::default());
            let keep = timer.clone();
            timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(5), move || {
                let pending = view::lines().iter().any(|l| l.kind == Kind::Note && l.values.first().is_some_and(|v| v.starts_with("Reading the file")));
                if !pending || t0.elapsed().as_secs() > 5 {
                    line(&rr, format!("  file read and shown {:.0} ms after opening", t0.elapsed().as_secs_f64() * 1000.0));
                    keep.stop();
                }
            });
            std::mem::forget(timer);
        });
        step!(1300, move |_st, app, r| {
            line(r, format!("  {}", meta_state(app)));
            for l in summary(80) {
                line(r, format!("    {l}"));
            }
            snapshot(app, &format!("m-track-{name}"));
            if down > 0.0 {
                scroll_to(app, down);
            }
        });
        if down > 0.0 {
            step!(500, move |_st, app, _r| {
                snapshot(app, &format!("m-track-{name}-down"));
            });
        }
        step!(200, |st, app, r| {
            press(app, Key::Escape, &[]);
            check(r, "Esc closes", !app.global::<Dialog>().get_meta_open(), meta_state(app));
            let _ = st;
        });
    }

    // ---- what the views say, checked ----------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Picard Set") {
            views::open_album(st, id);
        }
    });
    step!(600, |st, app, _r| {
        if let Some(i) = row_of(st, "01 Northern Lights.flac") {
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1200, |_st, app, r| {
        line(r, "[picard checks]".into());
        let artist: Vec<String> = view::lines().iter().filter(|l| l.label == "Artist").map(|l| l.values.join(" | ")).collect();
        check(r, "two artists as two values (tags), the library's artist line too", artist.iter().any(|a| a == "Aurora Vale | Mika Rhee"), format!("{artist:?}"));
        let date = view::lines().into_iter().find(|l| l.label == "Date").map(|l| (l.values.join(""), l.note)).unwrap_or_default();
        check(r, "the library's date says it was normalised", date.1.contains("2019.03.08"), format!("{date:?}"));
        let pics: Vec<String> = view::lines().iter().filter(|l| l.kind == Kind::Picture).map(|l| format!("{}: {} ({}) thumb {}", l.label, l.values.join(""), l.note, l.thumb.is_some())).collect();
        check(r, "two pictures with sizes and thumbnails", pics.len() == 2 && pics.iter().all(|p| p.ends_with("thumb true")), format!("{pics:?}"));
        let lyr = find("Lyrics").map(|l| l.values.join("")).unwrap_or_default();
        check(r, "lyrics: present and how long, not the text", lyr.contains("characters") && lyr.contains("synced"), lyr);
        let rows = app.global::<Meta>().get_rows();
        let thumbs = (0..rows.row_count()).filter_map(|i| rows.row_data(i)).filter(|r| r.has_thumb).count();
        check(r, "thumbnails reached the panel", thumbs == 2, format!("{thumbs} rows with a thumbnail"));
        // Copy all: readable text, everything in it.
        let text = view::copy_all_text(&crate::state::with(Rc::clone).unwrap()).unwrap_or_default();
        check(r, "Copy all has the header, groups and values", text.starts_with("Track: Northern Lights") && text.contains("\nIdentifiers\n") && text.contains("  MusicBrainz release: 6f2e3c1a"), format!("{} characters; starts {:?}", text.len(), text.lines().take(4).collect::<Vec<_>>()));
        let _ = std::fs::write(crate::bench::out_dir().join("m-copy-all-picard.txt"), &text);
        press(app, Key::Escape, &[]);
    });
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "ID3 Formats") {
            views::open_album(st, id);
        }
    });
    step!(600, |st, app, _r| {
        if let Some(i) = row_of(st, "02 Twenty Three.mp3") {
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1000, |_st, app, r| {
        line(r, "[id3v2.3 + id3v1 checks]".into());
        check(r, "both blocks named", app.global::<Meta>().get_blocks() == "Tags: ID3v2.3, ID3v1", app.global::<Meta>().get_blocks().to_string());
        let titles: Vec<String> = view::lines().iter().filter(|l| l.label == "Title" && l.field.is_some()).map(|l| format!("{} ({}) {}", l.values.join(""), l.note, l.attention)).collect();
        check(r, "the title: one row per block, marked", titles.len() == 2 && titles.iter().all(|t| t.ends_with("true")), format!("{titles:?}"));
        let genre: Vec<String> = view::lines().iter().filter(|l| l.label == "Genre" && l.field.is_some()).map(|l| format!("{} ({})", l.values.join(""), l.note)).collect();
        check(r, "the genre differs too", genre.len() == 2, format!("{genre:?}"));
        press(app, Key::Escape, &[]);
    });

    // ---- by the keyboard: Tab to the list, Menu, Down to the entry, Enter ---------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Picard Set") {
            views::open_album(st, id);
        }
    });
    step!(900, |_st, app, r| {
        app.invoke_focus_shortcuts();
        // As bench/details.rs: eight Tabs reach the album's track list (the artist link, five
        // buttons, the heart, the album's menu).
        tabs(app, 8);
        press(app, Key::DownArrow, &[]);
        press(app, Key::Menu, &[]);
        let m = app.global::<MenuStore>();
        let labels = menu_labels(app);
        line(r, format!("[keys] Menu: open {}, from keys {}, {:?}", m.get_open(), m.get_from_keys(), labels));
        let at = labels.iter().position(|l| l == "View metadata").unwrap_or(0);
        // The first entry is highlighted; Down to "View metadata" (separators are skipped).
        let steps = labels.iter().take(at).filter(|l| !l.is_empty()).count();
        for _ in 0..steps {
            press(app, Key::DownArrow, &[]);
        }
        line(r, format!("[keys] Down x{steps}: highlight {} ({:?})", m.get_highlight(), labels.get(m.get_highlight().max(0) as usize)));
        press(app, Key::Return, &[]);
    });
    step!(1200, |_st, app, r| {
        check(r, "Enter opened the view", app.global::<Dialog>().get_meta_open(), meta_state(app));
        let m = app.global::<Meta>();
        let c0 = m.get_cursor();
        for _ in 0..3 {
            press(app, Key::DownArrow, &[]);
        }
        let c1 = m.get_cursor();
        check(r, "Down moves the cursor over the rows (headings skipped)", c1 > c0 && view::lines()[c1 as usize].selectable(), format!("{c0} -> {c1} ({:?})", view::lines().get(c1 as usize).map(|l| l.label.clone())));
        snapshot(app, "m-keys-01-cursor");
        // Ctrl+C on the row: the clipboard has its value.
        let want = view::lines()[c1 as usize].copy.clone();
        press(app, Key::Control, &[]);
        type_text(app, "c");
        press(app, Key::Control, &[]);
        let ctrl_c_ok = clipboard().as_deref() == Some(want.as_str());
        // Slint delivers Ctrl+C as the letter with the control modifier only when the key is
        // held: send the chord the way the keyboard does.
        {
            use slint::platform::WindowEvent;
            let w = app.window();
            w.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
            w.dispatch_event(WindowEvent::KeyPressed { text: "c".into() });
            w.dispatch_event(WindowEvent::KeyReleased { text: "c".into() });
            w.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        }
        let clip = clipboard();
        check(r, "Ctrl+C copies the row's value", ctrl_c_ok || clip.as_deref() == Some(want.as_str()), format!("want {want:?}, clipboard {clip:?}; flash {:?}", m.get_flash().to_string()));
        snapshot(app, "m-keys-02-copied");
    });
    step!(500, |_st, app, r| {
        // Typing reaches no shortcut: M would mute, N open now playing, / search.
        let (muted, page) = (app.global::<Player>().get_muted(), app.global::<Nav>().get_page());
        type_text(app, "mn/ ");
        check(r, "typing in the panel reaches no shortcut", app.global::<Player>().get_muted() == muted && app.global::<Nav>().get_page() == page && app.global::<Dialog>().get_meta_open(), format!("muted {muted} -> {}, page {page:?} -> {:?}", app.global::<Player>().get_muted(), app.global::<Nav>().get_page()));
        // End, then the long fingerprint: Right expands it.
        let m = app.global::<Meta>();
        let fp = view::lines().iter().position(|l| l.label == "AcoustID fingerprint").unwrap_or(0) as i32;
        m.set_cursor(fp);
        press(app, Key::RightArrow, &[]);
        let expanded = (0..m.get_rows().row_count()).filter_map(|i| m.get_rows().row_data(i)).nth(fp as usize).is_some_and(|r| r.expanded);
        check(r, "Right expands a long value", expanded, format!("row {fp}"));
    });
    step!(600, |_st, app, r| {
        snapshot(app, "m-keys-03-expanded");
        let m = app.global::<Meta>();
        press(app, Key::LeftArrow, &[]);
        let folded = (0..m.get_rows().row_count()).filter_map(|i| m.get_rows().row_data(i)).nth(m.get_cursor() as usize).is_some_and(|r| !r.expanded && r.expandable);
        check(r, "Left folds it", folded, String::new());
        press(app, Key::End, &[]);
        let last = m.get_cursor();
        check(r, "End reaches the last row", last == view::lines().iter().rposition(view::Line::selectable).map_or(-1, |i| i as i32), format!("cursor {last}"));
        // Tab goes round the panel: list -> Copy all -> Show in Explorer -> Close -> list.
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(app, Key::Tab, &[]);
            seen.push(app.global::<crate::Focus>().get_zone().to_string());
        }
        check(r, "Tab stays in the panel", app.global::<Dialog>().get_meta_open() && seen.iter().take(3).all(|z| z == "dialog"), format!("zones {seen:?}"));
    });
    step!(400, |_st, app, r| {
        snapshot(app, "m-keys-04-end");
        press(app, Key::Escape, &[]);
        check(r, "Esc closes", !app.global::<Dialog>().get_meta_open(), String::new());
        // The track list has the focus back: Menu opens its menu.
        press(app, Key::Menu, &[]);
        let m = app.global::<MenuStore>();
        check(r, "the focus went back to the track list", m.get_open() && m.get_from_keys(), format!("menu open {}, from keys {}", m.get_open(), m.get_from_keys()));
        press(app, Key::Escape, &[]);
    });

    // ---- albums -------------------------------------------------------------------------
    step!(300, |st, _app, _r| views::go(st, Page::Albums));
    step!(900, |st, app, r| {
        let Some(id) = album_id(st, "Patchwork") else {
            check(r, "the inconsistent album", false, "not found".into());
            return;
        };
        crate::menus::request(st, "album", "", 0, id as i32, 400.0, 300.0);
        let labels = menu_labels(app);
        check(r, "an album tile's menu has View metadata", labels.last().map(String::as_str) == Some("View metadata"), format!("{labels:?}"));
        open_menu_entry(st, app, "View metadata");
        line(r, format!("[album] at once: {}", meta_state(app)));
        snapshot(app, "m-album-00-at-once");
    });
    step!(1500, |_st, app, r| {
        line(r, format!("[album Patchwork] {}", meta_state(app)));
        for l in summary(120) {
            line(r, format!("    {l}"));
        }
        let lines = view::lines();
        let summary_rows: Vec<String> = {
            let at = lines.iter().position(|l| l.kind == Kind::Heading && l.label == "Consistency").unwrap_or(0);
            lines.iter().skip(at + 1).take_while(|l| l.kind != Kind::Heading).map(|l| format!("{}: {}", l.label, l.values.join(""))).collect()
        };
        let has = |s: &str| summary_rows.iter().any(|x| x.starts_with(s));
        check(r, "spots album, album artist, date, genre, numbers, ReplayGain and formats", has("Album:") && has("Album artist:") && has("Date:") && has("Genre:") && has("Track numbers:") && has("ReplayGain:") && has("Formats:"), format!("{summary_rows:?}"));
        let cover = find("Cover").map(|l| format!("{} ({})", l.values.join(""), l.note)).unwrap_or_default();
        check(r, "the cover's source: the folder's image", cover.contains("folder.jpg") && cover.contains("700 \u{d7} 700"), cover);
        snapshot(app, "m-album-01-top");
        let at = lines.iter().position(|l| l.kind == Kind::Heading && l.label == "Consistency").unwrap_or(0);
        app.global::<Meta>().set_cursor(at as i32 + 1);
        press(app, Key::DownArrow, &[]);
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "m-album-02-consistency");
        // End: the cursor to the last track, which scrolls the table into view.
        press(app, Key::End, &[]);
    });
    step!(600, |st, app, r| {
        snapshot(app, "m-album-03-table");
        // A row leads to its track.
        let lines = view::lines();
        let Some(i) = lines.iter().position(|l| l.kind == Kind::Track && l.label == "Patch Six") else {
            check(r, "the table lists Patch Six", false, String::new());
            return;
        };
        app.global::<Meta>().set_cursor(i as i32);
        let scroll = app.global::<Meta>().get_scroll();
        T0.with(|t| t.set(Some(Instant::now())));
        press(app, Key::Return, &[]);
        check(r, "Enter on a row opens its track, with Back", app.global::<Meta>().get_eyebrow() == "TRACK" && app.global::<Meta>().get_can_back(), meta_state(app));
        line(r, format!("  (album scroll was {scroll:.0}, cursor {i})"));
        let _ = st;
    });
    step!(1200, |_st, app, r| {
        for l in summary(30) {
            line(r, format!("    {l}"));
        }
        snapshot(app, "m-album-04-track");
        // The album's values are the most common among its tracks: this track has no album
        // artist tag, and its album's other tracks name one, which holds for it too; it says so.
        let aa = find("Album artist").map(|l| (l.values.join(""), l.note, l.attention)).unwrap_or_default();
        let ok = aa.0 == "The Menders" && aa.1.starts_with("No album artist tag on this file") && !aa.2;
        check(r, "the track explains its album artist (its own tag is missing)", ok, format!("{aa:?}"));
        press(app, Key::Backspace, &[]);
    });
    step!(600, |_st, app, r| {
        let m = app.global::<Meta>();
        let at = view::lines().get(m.get_cursor().max(0) as usize).map(|l| l.label.clone()).unwrap_or_default();
        check(r, "Backspace returns to the album, where it was", m.get_eyebrow() == "ALBUM" && !m.get_can_back() && at == "Patch Six", format!("{} (cursor on {at:?})", meta_state(app)));
        // The album's values are its tracks' most common, whichever track a scan read last:
        // none of the minority's values is the album's.
        let album_rows: Vec<(String, String, bool)> = view::lines().iter().take_while(|l| !(l.kind == Kind::Heading && l.label == "In the library")).filter(|l| l.kind == Kind::Field).map(|l| (l.values.join(""), l.note.clone(), l.attention)).collect();
        let minority = album_rows.iter().filter(|(v, _, _)| v == "Guest Singer" || v == "Patchwork (Deluxe)" || v == "2020" || v == "Pop").collect::<Vec<_>>();
        check(r, "the album's values are the most common among its tracks", minority.is_empty() && !album_rows.is_empty(), format!("{album_rows:?}"));
        snapshot(app, "m-album-05-back");
        press(app, Key::Escape, &[]);
    });
    // The album page's More button, and the album of a lossless set with an embedded cover.
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Picard Set") {
            views::open_album(st, id);
        }
    });
    step!(900, |st, app, r| {
        let id = album_id(st, "Picard Set").unwrap_or(0);
        crate::menus::request(st, "album", "", 0, id as i32, 700.0, 380.0);
        snapshot(app, "m-album-06-more-menu");
        open_menu_entry(st, app, "View metadata");
        check(r, "the album page's menu opens it", app.global::<Dialog>().get_meta_open(), meta_state(app));
    });
    step!(1300, |_st, app, r| {
        for l in summary(40) {
            line(r, format!("    {l}"));
        }
        let cover = find("Cover").map(|l| format!("{} ({})", l.values.join(""), l.note)).unwrap_or_default();
        check(r, "embedded cover: which file, its type, size", cover.contains("Front cover, embedded in") && cover.contains("1,200 \u{d7} 1,200"), cover);
        let grouped = find("Grouped by").map(|l| format!("{} ({})", l.values.join(""), l.note)).unwrap_or_default();
        check(r, "grouped by its MusicBrainz release", grouped.contains("6f2e3c1a"), grouped);
        snapshot(app, "m-album-07-picard");
        press(app, Key::Escape, &[]);
    });

    // ---- the queue --------------------------------------------------------------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Other Formats") {
            playback::play_tracks(st, query::album_tracks(&st.db, id).unwrap_or_default(), 0);
        }
    });
    step!(1500, |st, app, r| {
        playback::set_paused(st, true);
        app.global::<Nav>().set_queue_open(true);
        crate::menus::request(st, "queue", "", 0, 0, 1500.0, 300.0);
        let labels = menu_labels(app);
        let show = crate::platform::system::words().show_in;
        check(r, "the queue's menu has Show in Explorer", labels.iter().any(|l| l == show), format!("{labels:?}"));
        check(r, "the queue's menu has View metadata", labels.last().map(String::as_str) == Some("View metadata"), format!("{labels:?}"));
        open_menu_entry(st, app, "View metadata");
    });
    step!(1200, |_st, app, r| {
        let fmt = find("Format").map(|l| l.values.join("")).unwrap_or_default();
        check(r, "the playing track's file rows", fmt.starts_with("Opus"), fmt);
        let r128 = find("R128 track gain").map(|l| l.note).unwrap_or_default();
        check(r, "R128 in dB", r128.contains("4.82 dB"), r128);
        snapshot(app, "m-queue-opus");
        press(app, Key::Escape, &[]);
        app.global::<Nav>().set_queue_open(false);
    });

    // ---- a file deleted after the scan, and one broken ----------------------------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Edge Cases") {
            views::open_album(st, id);
        }
    });
    step!(800, |st, app, r| {
        // Only ever inside the fixture folder (it has the fixture tool's marker file).
        let music = std::env::var_os("MP_MUSIC").map(std::path::PathBuf::from).unwrap_or_default();
        if !music.join(".meta-fixtures").exists() {
            check(r, "MP_MUSIC is the fixture folder", false, music.display().to_string());
            return;
        }
        let gone = music.join("Edge Cases").join("02 Gone.flac");
        let broken = music.join("Edge Cases").join("03 Corrupt.flac");
        let _ = std::fs::remove_file(&gone);
        let _ = std::fs::write(&broken, b"this is not a FLAC file any more");
        line(r, format!("[gone] removed {}, broke {}", gone.display(), broken.display()));
        if let Some(i) = row_of(st, "02 Gone.flac") {
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1000, |_st, app, r| {
        check(r, "a missing file says so, and shows what the library knows", app.global::<Meta>().get_status().contains("missing") && find("Path").is_some() && find("Duration").is_some(), meta_state(app));
        snapshot(app, "m-gone");
        press(app, Key::Escape, &[]);
    });
    step!(300, |st, app, _r| {
        if let Some(i) = row_of(st, "03 Corrupt.flac") {
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1000, |st, app, r| {
        check(r, "an unreadable file says so", app.global::<Meta>().get_status().contains("could not be read"), meta_state(app));
        snapshot(app, "m-corrupt");
        press(app, Key::Escape, &[]);
        if let Some(id) = album_id(st, "Edge Cases") {
            crate::menus::request(st, "album", "", 0, id as i32, 400.0, 300.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1500, |_st, app, r| {
        let files = find("Files").map(|l| l.values.join("")).unwrap_or_default();
        check(r, "the album names its missing and unreadable files", files.contains("missing") && files.contains("not readable"), files);
        press(app, Key::End, &[]);
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "m-edge-album");
        press(app, Key::Escape, &[]);
    });

    // ---- middle-click scrolling in the panel; a click outside ------------------------------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Edge Cases") {
            views::open_album(st, id);
        }
    });
    step!(700, |st, app, _r| {
        if let Some(i) = row_of(st, "01 Long Comment.flac") {
            crate::menus::request(st, "track", "album", i, 0, 620.0, 420.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1500, |_st, app, r| {
        let (w, h) = size(app);
        let (x, y) = (w / 2.0, h / 2.0);
        send(app, Input::Move(x, y));
        let kept = (send(app, Input::Press(Button::Middle, x, y)), send(app, Input::Release(Button::Middle, x, y)));
        let page_before = app.global::<Nav>().get_page_scroll();
        let g = app.global::<crate::Autoscroll>();
        line(r, format!("[autoscroll] middle click at the panel: kept {kept:?}, on {}, axes y {}", g.get_on(), g.get_has_y()));
        send(app, Input::Move(x, y + 220.0));
        T0.with(|t| t.set(Some(Instant::now())));
        let s0 = app.global::<Meta>().get_scroll();
        let pb = page_before;
        let weak = app.as_weak();
        let rr = r.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(800), move || {
            let Some(app) = weak.upgrade() else { return };
            let s1 = app.global::<Meta>().get_scroll();
            check(&rr, "middle-click scrolling moves the panel's list, not the page", s1 < s0 - 50.0 && app.global::<Nav>().get_page_scroll() == pb, format!("panel {s0:.0} -> {s1:.0}, page {pb:.0} -> {:.0}", app.global::<Nav>().get_page_scroll()));
            snapshot(&app, "m-autoscroll");
            let (w, h) = size(&app);
            let _ = send(&app, Input::Press(Button::Left, w / 2.0, h / 2.0));
            let _ = send(&app, Input::Release(Button::Left, w / 2.0, h / 2.0));
        });
    });
    step!(1500, |_st, app, r| {
        let g = app.global::<crate::Autoscroll>();
        check(r, "a click ends it, the panel stays", !g.get_on() && app.global::<Dialog>().get_meta_open(), meta_state(app));
        // A click outside the panel closes it.
        let _ = send(app, Input::Press(Button::Left, 6.0, 300.0));
        let _ = send(app, Input::Release(Button::Left, 6.0, 300.0));
        check(r, "a click outside closes it", !app.global::<Dialog>().get_meta_open(), String::new());
    });

    // ---- an album of 400 tracks ---------------------------------------------------------
    step!(300, |st, app, r| {
        let Some(id) = album_id(st, "Big Album") else {
            check(r, "the big album", false, "not found".into());
            return;
        };
        let t0 = Instant::now();
        crate::menus::request(st, "album", "", 0, id as i32, 400.0, 300.0);
        open_menu_entry(st, app, "View metadata");
        line(r, format!("[big album] shown in {:.1} ms: {}", t0.elapsed().as_secs_f64() * 1000.0, meta_state(app)));
        let rr = r.clone();
        let timer = Rc::new(slint::Timer::default());
        let keep = timer.clone();
        timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(10), move || {
            let reading = view::lines().iter().any(|l| l.kind == Kind::Note && l.values.first().is_some_and(|v| v.starts_with("Reading the tags")));
            if !reading || t0.elapsed().as_secs() > 20 {
                line(&rr, format!("  all 400 files read and checked {:.0} ms after opening", t0.elapsed().as_secs_f64() * 1000.0));
                keep.stop();
            }
        });
        std::mem::forget(timer);
    });
    step!(150, |_st, app, _r| {
        snapshot(app, "m-big-00-reading");
    });
    step!(3000, |_st, app, r| {
        let tracks = view::lines().iter().filter(|l| l.kind == Kind::Track).count();
        let consistent = view::lines().iter().any(|l| l.kind == Kind::Note && l.values.first().is_some_and(|v| v.starts_with("The tracks agree")));
        check(r, "400 tracks: the first 300 listed, the checks over all", tracks == 300 && consistent, format!("{tracks} rows; agree {consistent}"));
        press(app, Key::End, &[]);
    });
    step!(500, |_st, app, r| {
        snapshot(app, "m-big-01-end");
        let last = view::lines().get(app.global::<Meta>().get_cursor().max(0) as usize).map(|l| l.values.join("")).unwrap_or_default();
        check(r, "End reaches \u{201c}Show more\u{201d}", last == "Show the other 100", last);
        press(app, Key::Return, &[]);
        let tracks = view::lines().iter().filter(|l| l.kind == Kind::Track).count();
        check(r, "Enter on it lists the rest", tracks == 400, format!("{tracks} rows"));
        press(app, Key::Escape, &[]);
    });
    // Closed while still reading: the reading stops (nothing lands later).
    step!(300, |st, app, r| {
        if let Some(id) = album_id(st, "Big Album") {
            crate::menus::request(st, "album", "", 0, id as i32, 400.0, 300.0);
            open_menu_entry(st, app, "View metadata");
            press(app, Key::Escape, &[]);
            check(r, "closed at once while reading", !app.global::<Dialog>().get_meta_open() && view::lines().is_empty(), String::new());
        }
    });
    step!(1500, |_st, app, r| {
        check(r, "nothing arrived for the closed view", !app.global::<Dialog>().get_meta_open() && view::lines().is_empty() && app.global::<Meta>().get_rows().row_count() == 0, String::new());
    });

    // ---- the minimum window size ----------------------------------------------------------
    step!(300, |st, _app, _r| crate::platform::devwindow::resize_client(st.hwnd.get(), 960, 620));
    step!(1200, |st, app, r| {
        let (w, h) = size(app);
        line(r, format!("[min] window {w}x{h}"));
        if let Some(id) = album_id(st, "Picard Set") {
            views::open_album(st, id);
        }
    });
    step!(800, |st, app, _r| {
        if let Some(i) = row_of(st, "01 Northern Lights.flac") {
            crate::menus::request(st, "track", "album", i, 0, 500.0, 300.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1300, |_st, app, _r| {
        snapshot(app, "m-min-track");
        scroll_to(app, 1200.0);
    });
    step!(500, |st, app, _r| {
        snapshot(app, "m-min-track-down");
        press(app, Key::Escape, &[]);
        if let Some(id) = album_id(st, "Patchwork") {
            crate::menus::request(st, "album", "", 0, id as i32, 400.0, 300.0);
            open_menu_entry(st, app, "View metadata");
        }
    });
    step!(1500, |_st, app, _r| {
        snapshot(app, "m-min-album");
        press(app, Key::End, &[]);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "m-min-album-table");
        press(app, Key::Escape, &[]);
    });
    step!(300, |st, _app, _r| crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void));

    step!(1200, |_st, _app, r| {
        if let Some(text) = SAVED_CLIP.with(|c| c.borrow_mut().take()) {
            crate::explorer::copy_text(&text);
            line(r, "clipboard put back as it was".into());
        }
        let fails = FAILS.with(Cell::get);
        line(r, format!("\n{fails} checks failed"));
        write_report("metadata", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
