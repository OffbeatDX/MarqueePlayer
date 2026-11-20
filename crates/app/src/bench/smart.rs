//! `MP_BENCH=smart`: smart playlists (`bench-smart.txt`, `s-*.png`). On the messy fixture
//! library (`cargo run -p library --example messy_library`), with favourites and a few plays
//! seeded in the bench's own database:
//!
//! - the editor for a new smart playlist, built by the keyboard: the name typed, Tab to the
//!   first rule, its field chosen from the list by typing "play" and Enter, its value as it
//!   starts; Tab to "Add rule" and Enter; the new rule's field picked by the pointer in its
//!   list, its value (a genre) picked by typing; a value typed wrong and explained, a save
//!   refused for it, then fixed; an "any of" group; the limit switched on and typed;
//! - the live count and preview as the rules change, and Save (the page with its rules in
//!   words, at 1920 x 1080 and at 960 x 620, the editor too);
//! - live updating: a "never played" list loses the track once its play counts (the queue
//!   keeps it), a "most played this year" list holds what its rules do, a favourites list
//!   follows `L`;
//! - a rule from a newer build (kept, shown, matching nothing), a playlist that cannot name
//!   itself, Reshuffle, Delete's hint, "Add to playlist" listing only ordinary playlists,
//!   a home shelf of a smart playlist, Save as ordinary playlist, Export, the sidebar and
//!   the Playlists page;
//! - search operators (`search_operators`): typed into the search field, the hint, the syntax
//!   help, a plain query still plain, both paths timed, and Save as smart playlist.
//!
//! Events go through Slint's own dispatch inside the app (keys, pointer); no OS input.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use library::query;
use library::smart::{self, Smart};
use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::nav::Route;
use crate::state::State;
use crate::{AppWindow, Dialog, Home, MenuStore, Nav, Page, Picker, Playlists, SmartEdit, SmartList, playback, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("smart bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
    /// Ids of the smart playlists made, by name.
    static MADE: RefCell<Vec<(String, i64)>> = const { RefCell::new(Vec::new()) };
    static SAVED: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
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

fn size_window(st: &State, w: i32, h: i32) {
    crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
}

fn picker(app: &AppWindow) -> String {
    let p = app.global::<Picker>();
    let items: Vec<String> = (0..p.get_items().row_count().min(6)).filter_map(|i| p.get_items().row_data(i)).map(|s| s.to_string()).collect();
    format!("open {}, title {:?}, {} items {:?}, highlight {}, filter {:?}", p.get_open(), p.get_title().to_string(), p.get_items().row_count(), items, p.get_highlight(), p.get_filter().to_string())
}

/// Where row `i` of the open list is (as `SmartPicker` lays it out: under its control).
fn picker_row(app: &AppWindow, i: usize) -> (f32, f32) {
    let p = app.global::<Picker>();
    let w = app.window().size().to_logical(app.window().scale_factor()).width;
    let width = 260f32.max(p.get_min_width());
    let x = p.get_x().min(w - width - 8.0).max(8.0);
    let head = if p.get_filterable() { 44.0 } else { 0.0 };
    (x + 60.0, p.get_y() + 6.0 + head + i as f32 * 34.0 + 17.0)
}

fn lines() -> String {
    crate::playlists::smart::editor_lines().join(" | ")
}

fn page(app: &AppWindow) -> String {
    let l = app.global::<SmartList>();
    format!("{:?}: {:?}; {:?}; {} rows; problems {}", l.get_name().to_string(), l.get_summary().to_string(), l.get_info().to_string(), l.get_tracks().row_count(), l.get_problems().row_count())
}

fn smart_ids(st: &State, id: i64) -> Vec<i64> {
    let s = Smart::from_json(&smart::playlist_rules(&st.db, id).ok().flatten().flatten().unwrap_or_default());
    smart::evaluate_ids(&st.db, &s, Some(id), crate::playlists::smart::ctx()).map(|r| r.0).unwrap_or_default()
}

fn shown_ids(st: &State) -> Vec<i64> {
    crate::playlists::smart::shown_tracks(st).iter().map(|t| t.id).collect()
}

fn id_named(st: &State, name: &str) -> i64 {
    query::playlists(&st.db).unwrap_or_default().into_iter().find(|p| p.name == name).map_or(-1, |p| p.id)
}

/// Make a smart playlist from a starting point, through the editor (its callbacks), saved.
fn from_template(st: &Rc<State>, app: &AppWindow, i: i32) {
    crate::playlists::smart::open_new(st);
    app.global::<SmartEdit>().invoke_template(i);
    app.global::<SmartEdit>().invoke_save();
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

    // ---- the library, and a little history in the bench's own database ------------------------
    step!(800, |st, _app, r| {
        size_window(st, 1920, 1080);
        // By path (either slash), not id: ids follow the scan's order, which differs between file
        // systems, and the rules below must find the same tracks on Windows and Linux.
        let ids: Vec<i64> = st.db.prepare("SELECT id FROM tracks ORDER BY replace(path, '\\', '/')").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for (i, id) in ids.iter().enumerate() {
            if i % 11 == 0 {
                let _ = st.db.execute("INSERT OR IGNORE INTO favourite_tracks VALUES (?1, 1)", [id]);
            }
            if i % 7 == 0 {
                let _ = query::record_play(&st.db, *id);
            }
        }
        crate::playlists::smart::changed(st);
        line(r, format!("library: {:?} (albums, tracks); favourites {}, played {}", query::counts(&st.db).unwrap_or_default(), ids.len().div_ceil(11), ids.len().div_ceil(7)));
        views::go(st, Page::Playlists);
    });
    step!(1200, |_st, app, _r| {
        snapshot(app, "s-00-playlists-none");
    });

    // ---- the editor, by the keyboard --------------------------------------------------------
    step!(300, |_st, app, r| {
        app.global::<crate::Actions>().invoke_new_smart_playlist();
        line(r, "[editor] New smart playlist…".into());
    });
    step!(900, |_st, app, r| {
        let e = app.global::<SmartEdit>();
        check(r, "the editor is open, new, on the name", app.global::<Nav>().get_page() == Page::SmartEdit && e.get_is_new(), format!("page {:?}, rows: {}", app.global::<Nav>().get_page(), lines()));
        snapshot(app, "s-01-editor-new");
        type_text(app, "Bench played");
    });
    step!(400, |_st, app, r| {
        let e = app.global::<SmartEdit>();
        check(r, "typed into the name", e.get_name() == "Bench played", format!("{:?}", e.get_name().to_string()));
        // Name, Cancel, Save, five starting points, "all": the first rule's field.
        tabs(app, 9);
        press(app, Key::Return, &[]);
    });
    step!(500, |_st, app, r| {
        check(r, "Tab to the first rule's field, Enter opens its list", app.global::<Picker>().get_title() == "Field", picker(app));
        snapshot(app, "s-02-field-list");
        type_text(app, "play");
    });
    step!(400, |_st, app, r| {
        check(r, "typing filters the list (folded)", app.global::<Picker>().get_items().row_data(0).is_some_and(|s| s == "Play count"), picker(app));
        snapshot(app, "s-03-field-filtered");
        press(app, Key::Return, &[]);
    });
    step!(500, |_st, app, r| {
        check(r, "Enter takes it: a play count rule, its value to type", lines().contains("Play count is at least 1"), lines());
        check(r, "the list closed", !app.global::<Picker>().get_open(), picker(app));
        // The field has the keyboard again: Tab, Tab to the value.
        tabs(app, 2);
    });
    step!(400, |_st, app, r| {
        check(r, "the value as it starts", lines().contains("Play count is at least 1"), lines());
        // The value, then the line's remove button (up and down are off for the only line),
        // then "Add rule".
        tabs(app, 2);
        press(app, Key::Return, &[]);
    });
    step!(500, |_st, app, r| {
        check(r, "Add rule by the keyboard: like the last one, its field has the keyboard", crate::playlists::smart::editor_rule_count() == 3, lines());
        press(app, Key::Return, &[]);
    });
    // ---- the pointer: the new rule's field, from its list ----------------------------------------
    step!(500, |_st, app, r| {
        line(r, format!("[pointer] {}", picker(app)));
        // "Genre" is the fifth field.
        let (x, y) = picker_row(app, 4);
        click(app, x, y);
    });
    step!(500, |_st, app, r| {
        check(r, "a click in the list chooses Genre", lines().contains("Genre is Choose…"), lines());
        // Its value: Tab, Tab, Enter; type part of a genre.
        tabs(app, 2);
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        let genres = app.global::<Picker>().get_items().row_count();
        let want: String = st.db.query_row("SELECT name FROM genres g WHERE EXISTS (SELECT 1 FROM track_genres x WHERE x.genre_id = g.id) ORDER BY (SELECT COUNT(*) FROM track_genres x WHERE x.genre_id = g.id) DESC LIMIT 1", [], |r| r.get(0)).unwrap_or_default();
        line(r, format!("  genre list: {genres} genres; picking {want:?}"));
        snapshot(app, "s-04-genre-list");
        type_text(app, &want);
        let w = want.clone();
        let app2 = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
            if let Some(app) = app2.upgrade() {
                let first = app.global::<Picker>().get_items().row_data(0).map(|s| s.to_string()).unwrap_or_default();
                log::info!("smart bench: filtered genres start with {first:?} (wanted {w:?})");
                press(&app, Key::Return, &[]);
            }
        });
    });
    step!(900, |_st, app, r| {
        check(r, "a genre picked by typing", lines().contains("Genre is ") && !lines().contains("Choose…"), lines());
        let e = app.global::<SmartEdit>();
        line(r, format!("  live count: {:?}; preview {} rows; {:?}", e.get_count().to_string(), e.get_preview().row_count(), e.get_preview_note().to_string()));
        check(r, "the live count follows the rules", e.get_count().starts_with("Matches"), e.get_count().to_string());
        snapshot(app, "s-05-two-rules");
    });
    // ---- a value typed wrong, a save refused, fixed -----------------------------------------------
    step!(300, |_st, app, _r| {
        app.global::<SmartEdit>().invoke_add_rule(-1);
    });
    step!(400, |_st, app, _r| {
        // The new line's field has the keyboard: Enter, "year", Enter; Tab, Tab to its value.
        press(app, Key::Return, &[]);
        type_text(app, "year");
    });
    step!(400, |_st, app, _r| {
        press(app, Key::Return, &[]);
    });
    step!(400, |_st, app, _r| {
        tabs(app, 2);
        type_text(app, "19x");
    });
    step!(600, |_st, app, r| {
        check(r, "a value that is not a year is explained under its line", lines().contains("is not a whole number"), lines());
        snapshot(app, "s-06-invalid");
        app.global::<SmartEdit>().invoke_save();
    });
    step!(500, |st, app, r| {
        let e = app.global::<SmartEdit>();
        check(r, "Save is refused, and says why", e.get_save_error().contains("not a whole number") && matches!(st.history.borrow().current, Route::SmartEdit(_)), e.get_save_error().to_string());
        // The field that is wrong has the keyboard: clear it, type a year; then "is at least".
        for _ in 0..3 {
            press(app, Key::Backspace, &[]);
        }
        type_text(app, "1990");
    });
    step!(400, |_st, app, _r| {
        press(app, Key::Tab, &[Key::Shift]);
        press(app, Key::Return, &[]);
    });
    step!(400, |_st, app, r| {
        line(r, format!("  operator list: {}", picker(app)));
        press(app, Key::DownArrow, &[]);
        press(app, Key::DownArrow, &[]);
        press(app, Key::Return, &[]);
    });
    step!(500, |_st, app, r| {
        check(r, "fixed: Year is at least 1990", lines().contains("Year is at least 1990") && !lines().contains("whole number"), lines());
        // A group: any of favourite, or played 3+ times.
        app.global::<SmartEdit>().invoke_add_group();
    });
    step!(500, |_st, app, r| {
        check(r, "a group of the other way (any)", lines().contains("Any track matching any of:") && lines().contains("  Favourite yes"), lines());
        // The limit: switched on, 20 tracks, chosen at random.
        app.global::<SmartEdit>().invoke_toggle(1001, crate::playlists::smartedit::Role::LimitOn.index());
    });
    step!(400, |_st, app, _r| {
        // The limit's number has the keyboard, selected: typing replaces it.
        type_text(app, "20");
    });
    step!(800, |_st, app, r| {
        check(r, "the limit reads as a sentence", lines().contains("[on] Limit to 20 tracks chosen by Random"), lines());
        let e = app.global::<SmartEdit>();
        line(r, format!("  live count: {:?}; {:?}", e.get_count().to_string(), e.get_preview_note().to_string()));
        app.global::<Nav>().set_page_scroll(0.0);
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "s-07-editor-1080p");
        app.global::<Nav>().set_page_scroll(-520.0);
    });
    step!(600, |st, app, _r| {
        snapshot(app, "s-08-editor-1080p-preview");
        app.global::<Nav>().set_page_scroll(0.0);
        size_window(st, 960, 620);
    });
    step!(1400, |_st, app, _r| {
        snapshot(app, "s-09-editor-min");
        app.global::<Nav>().set_page_scroll(-400.0);
    });
    step!(700, |st, app, _r| {
        snapshot(app, "s-10-editor-min-rules");
        app.global::<Nav>().set_page_scroll(0.0);
        size_window(st, 1920, 1080);
    });
    step!(1200, |st, app, r| {
        app.global::<SmartEdit>().invoke_save();
        let id = id_named(st, "Bench played");
        MADE.with(|m| m.borrow_mut().push(("Bench played".into(), id)));
        check(r, "saved: its page", id > 0 && st.history.borrow().current == Route::Playlist(id), format!("id {id}, route {:?}", st.history.borrow().current));
    });
    step!(1200, |st, app, r| {
        let id = id_named(st, "Bench played");
        line(r, format!("[page] {}", page(app)));
        let shown = shown_ids(st);
        let want = smart_ids(st, id);
        check(r, "the page holds what the rules do now (the 20 of the random limit)", shown == want && shown.len() <= 20 && !shown.is_empty(), format!("{} shown, {} by the rules", shown.len(), want.len()));
        check(r, "its rules in words", app.global::<SmartList>().get_summary().contains("1+ plays") && app.global::<SmartList>().get_summary().contains("20 tracks, random"), app.global::<SmartList>().get_summary().to_string());
        check(r, "Reshuffle is offered (a random limit)", app.global::<SmartList>().get_random(), String::new());
        snapshot(app, "s-11-page-1080p");
        size_window(st, 960, 620);
    });
    step!(1400, |_st, app, _r| {
        snapshot(app, "s-12-page-min");
    });
    // Reshuffle: a new order (the starting point "Two hours at random", every track here).
    step!(300, |st, app, _r| {
        size_window(st, 1920, 1080);
        from_template(st, app, 5);
    });
    step!(1500, |st, app, r| {
        let shown = shown_ids(st);
        check(r, "a random order, stable", shown == smart_ids(st, id_named(st, "Two hours at random")) && !shown.is_empty(), format!("{} tracks", shown.len()));
        SAVED.with(|s| *s.borrow_mut() = shown);
        app.global::<SmartList>().invoke_reshuffle();
    });
    step!(1200, |st, _app, r| {
        let before = SAVED.with(|s| s.borrow().clone());
        let now = shown_ids(st);
        let (mut a, mut b) = (before.clone(), now.clone());
        a.sort();
        b.sort();
        check(r, "Reshuffle: the same tracks in a new order", a == b && now != before, format!("{} tracks; first {:?} then {:?}", now.len(), before.first(), now.first()));
    });
    step!(300, |st, app, r| {
        size_window(st, 1920, 1080);
        // Delete on a row, and Alt+Up: why nothing happens.
        app.global::<SmartList>().invoke_hint();
        check(r, "Delete on a smart playlist's row says why", app.global::<Nav>().get_flash().contains("fills itself"), app.global::<Nav>().get_flash().to_string());
        // The track menu: no "Remove from playlist"; "Add to playlist" lists ordinary ones only.
        let id = id_named(st, "Bench played");
        let _ = query::create_playlist(&st.db, "Ordinary bench list");
        views::playlists_changed(st);
        crate::menus::request(st, "track", "smart", 0, id as i32, 600.0, 400.0);
        let labels = crate::bench::menu_labels(app);
        let m = app.global::<MenuStore>();
        let sub: Vec<String> = (0..m.get_sub().row_count()).filter_map(|i| m.get_sub().row_data(i)).map(|i| i.label.to_string()).collect();
        check(r, "a smart list's track menu has no Remove from playlist", !labels.iter().any(|l| l == "Remove from playlist"), format!("{labels:?}"));
        check(r, "Add to playlist lists only ordinary playlists", sub.contains(&"Ordinary bench list".to_owned()) && !sub.contains(&"Bench played".to_owned()), format!("{sub:?}"));
        m.set_open(false);
    });

    // ---- live: a play, a favourite ----------------------------------------------------
    step!(500, |st, app, _r| {
        from_template(st, app, 0); // Recently added and unplayed
    });
    step!(1500, |st, app, r| {
        let id = id_named(st, "Recently added and unplayed");
        line(r, format!("[live: plays] {}", page(app)));
        let shown = shown_ids(st);
        check(r, "unplayed: what the rules hold", shown == smart_ids(st, id) && !shown.is_empty(), format!("{} tracks", shown.len()));
        SAVED.with(|s| *s.borrow_mut() = shown);
        snapshot(app, "s-13-live-unplayed-before");
        app.global::<SmartList>().invoke_play(false);
    });
    step!(800, |st, app, r| {
        let t = st.queue.borrow().current().map(|i| i.track.clone());
        line(r, format!("  playing {:?} ({:.1} s); queue {}", t.as_ref().map(|t| t.title.clone()), t.as_ref().map_or(0.0, |t| t.duration), st.queue.borrow().items().len()));
        let _ = app;
    });
    step!(9000, |st, app, r| {
        let before = SAVED.with(|s| s.borrow().clone());
        let now = shown_ids(st);
        // What has a play now (a file that cannot be played is skipped, and the next one counts).
        let played: Vec<i64> = before.iter().copied().filter(|id| query::play_count(&st.db, *id).unwrap_or(0) > 0).collect();
        check(r, "a play counted", !played.is_empty(), format!("played {played:?}"));
        let expect: Vec<i64> = before.iter().copied().filter(|id| !played.contains(id)).collect();
        check(r, "the list lost what was played, and only that", now == expect, format!("{} → {} tracks", before.len(), now.len()));
        check(r, "the queue made from it is as it was", st.queue.borrow().items().len() == before.len(), format!("queue {}", st.queue.borrow().items().len()));
        line(r, format!("  {}", page(app)));
        snapshot(app, "s-14-live-unplayed-after");
        playback::toggle_play(st);
    });
    step!(500, |st, app, _r| {
        from_template(st, app, 2); // Most played this year
    });
    step!(1500, |st, app, r| {
        let shown = shown_ids(st);
        let id = id_named(st, "Most played this year");
        line(r, format!("[live: most played] {}", page(app)));
        check(r, "most played this year: what the rules hold", shown == smart_ids(st, id) && !shown.is_empty(), format!("{} tracks", shown.len()));
        snapshot(app, "s-15-live-most-played");
        // A list of favourites, through the editor's callbacks: favourite is yes.
        crate::playlists::smart::open_new(st);
        let e = app.global::<SmartEdit>();
        e.set_name("Bench favourites".into());
        e.invoke_name_edited("Bench favourites".into());
    });
    step!(500, |_st, app, _r| {
        let e = app.global::<SmartEdit>();
        // The first rule's field: Favourite (the eighth).
        e.invoke_open_choice(1, crate::playlists::smartedit::Role::Field.index(), 300.0, 300.0);
    });
    step!(400, |_st, app, _r| {
        app.global::<Picker>().invoke_pick(7);
    });
    step!(500, |st, app, r| {
        check(r, "Favourite yes", lines().contains("Favourite yes"), lines());
        app.global::<SmartEdit>().invoke_save();
        SAVED.with(|s| *s.borrow_mut() = shown_ids(st));
    });
    step!(1500, |st, app, r| {
        let playing = views::playing_track_id(st);
        let was = query::is_favourite_track(&st.db, playing).unwrap_or(false);
        line(r, format!("[live: favourite] {}; playing {playing} favourite {was}", page(app)));
        SAVED.with(|s| *s.borrow_mut() = shown_ids(st));
        press(app, Key::Tab, &[]);
        crate::bench::type_text(app, "l");
    });
    step!(1200, |st, app, r| {
        let playing = views::playing_track_id(st);
        let fav = query::is_favourite_track(&st.db, playing).unwrap_or(false);
        let now = shown_ids(st);
        check(r, "L toggled the playing track's favourite, and the favourites list followed", now.contains(&playing) == fav, format!("favourite {fav}; {} → {} tracks", SAVED.with(|s| s.borrow().len()), now.len()));
        let _ = app;
    });

    // ---- a newer build's rule, a playlist naming itself ------------------------------------------
    step!(300, |st, _app, _r| {
        let json = r#"{"v":3,"match":"all","rules":[{"field":"mood","op":"is","value":"calm"},{"field":"favourite","op":"yes"}],"sort":{"by":"energy","desc":true},"seed":1}"#;
        let id = query::create_smart_playlist(&st.db, "From a newer version", json).unwrap_or(-1);
        views::playlists_changed(st);
        views::open_playlist(st, id);
    });
    step!(1200, |st, app, r| {
        let l = app.global::<SmartList>();
        let problems: Vec<String> = (0..l.get_problems().row_count()).filter_map(|i| l.get_problems().row_data(i)).map(|s| s.to_string()).collect();
        check(r, "a rule from a newer version: kept, said, matches nothing", l.get_tracks().row_count() == 0 && problems.len() == 2, format!("{problems:?}"));
        snapshot(app, "s-16-newer-version");
        let id = id_named(st, "From a newer version");
        crate::playlists::smart::open_edit(st, id);
    });
    step!(1000, |st, app, r| {
        check(r, "the editor shows it as a line of its own", lines().contains("") && crate::playlists::smart::editor_rule_count() == 3, lines());
        snapshot(app, "s-17-newer-version-editor");
        app.global::<SmartEdit>().invoke_save();
        let id = id_named(st, "From a newer version");
        let stored = smart::playlist_rules(&st.db, id).ok().flatten().flatten().unwrap_or_default();
        check(r, "saved again unchanged, the unknown rule and order kept", stored.contains("\"mood\"") && stored.contains("\"energy\"") && stored.contains("\"v\":3"), stored);
    });
    step!(800, |st, app, r| {
        // Most played this year may not name itself, nor a list that names it.
        let top = id_named(st, "Most played this year");
        let _ = query::create_smart_playlist(&st.db, "Names most played", &format!(r#"{{"rules":[{{"field":"playlist","op":"in","value":{top}}}]}}"#));
        views::playlists_changed(st);
        crate::playlists::smart::open_edit(st, top);
        let _ = app;
        let _ = r;
    });
    step!(800, |_st, app, _r| {
        let e = app.global::<SmartEdit>();
        e.invoke_add_rule(-1);
    });
    step!(400, |_st, app, _r| {
        let e = app.global::<SmartEdit>();
        // The new line (key 2): its field, Playlist (the last).
        e.invoke_open_choice(2, crate::playlists::smartedit::Role::Field.index(), 300.0, 300.0);
    });
    step!(400, |_st, app, _r| {
        app.global::<Picker>().invoke_filter_edited("playlist".into());
    });
    step!(300, |_st, app, _r| {
        app.global::<Picker>().invoke_pick(0);
    });
    step!(400, |_st, app, _r| {
        app.global::<SmartEdit>().invoke_open_choice(2, crate::playlists::smartedit::Role::A.index(), 300.0, 300.0);
    });
    step!(400, |_st, app, r| {
        let p = app.global::<Picker>();
        let items: Vec<String> = (0..p.get_items().row_count()).filter_map(|i| p.get_items().row_data(i)).map(|s| s.to_string()).collect();
        check(r, "a playlist cannot name itself or one that names it", !items.iter().any(|i| i.starts_with("Most played this year") || i.starts_with("Names most played")) && items.iter().any(|i| i.starts_with("Ordinary bench list")), format!("{items:?}"));
        snapshot(app, "s-18-playlist-choice");
        p.invoke_close();
        app.global::<SmartEdit>().invoke_cancel();
    });

    // ---- elsewhere: home shelf, save as ordinary playlist, export, sidebar, Playlists page ----------
    step!(600, |st, app, _r| {
        let id = id_named(st, "Most played this year");
        app.global::<crate::Actions>().invoke_shelf_add("playlist".into(), id.to_string().into());
        views::go(st, Page::Home);
    });
    step!(1500, |_st, app, r| {
        let shelves = app.global::<Home>().get_shelves();
        let titles: Vec<String> = (0..shelves.row_count()).filter_map(|i| shelves.row_data(i)).map(|s| s.title.to_string()).collect();
        check(r, "a smart playlist as a home shelf, titled with its name", titles.iter().any(|t| t == "Most played this year"), format!("{titles:?}"));
        app.global::<Nav>().set_page_scroll(-3000.0);
    });
    step!(800, |_st, app, _r| {
        snapshot(app, "s-19-home-shelf");
        app.global::<Nav>().set_page_scroll(0.0);
    });
    step!(300, |st, _app, _r| {
        let id = id_named(st, "Most played this year");
        views::open_playlist(st, id);
    });
    step!(1200, |st, app, _r| {
        let id = id_named(st, "Most played this year");
        SAVED.with(|s| *s.borrow_mut() = shown_ids(st));
        app.global::<SmartList>().invoke_more(700.0, 300.0);
        let labels = crate::bench::menu_labels(app);
        log::info!("smart bench: page menu {labels:?}");
        app.global::<MenuStore>().set_open(false);
        crate::playlists::smart::freeze(st, id);
    });
    step!(500, |_st, app, r| {
        let d = app.global::<Dialog>();
        check(r, "Save as ordinary playlist asks for a name", d.get_prompt_open() && d.get_prompt_text() == "Most played this year (saved)", format!("{:?}", d.get_prompt_text().to_string()));
        snapshot(app, "s-20-save-as-playlist");
        press(app, Key::Return, &[]);
    });
    step!(600, |st, _app, r| {
        let id = id_named(st, "Most played this year (saved)");
        let tracks: Vec<i64> = query::playlist_tracks(&st.db, id).unwrap_or_default().iter().map(|t| t.id).collect();
        let shown = SAVED.with(|s| s.borrow().clone());
        check(r, "an ordinary playlist with what it held, in its order", id > 0 && tracks == shown, format!("{} tracks", tracks.len()));
        let path = crate::bench::out_dir().join("smart-export.m3u8");
        let _ = std::fs::remove_file(&path);
        crate::playlists::file::export_file(st, id_named(st, "Most played this year"), &path, false);
    });
    step!(1200, |st, app, r| {
        let path = crate::bench::out_dir().join("smart-export.m3u8");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let entries = text.lines().filter(|l| l.starts_with("#EXTINF")).count();
        let want = smart_ids(st, id_named(st, "Most played this year")).len();
        check(r, "M3U8 export writes what it holds now", entries == want && want > 0, format!("{entries} entries, {want} tracks"));
        app.global::<Nav>().set_sidebar_pinned(true);
        views::go(st, Page::Playlists);
    });
    step!(1500, |st, app, r| {
        let list = app.global::<Playlists>().get_list();
        let rows: Vec<String> = (0..list.row_count()).filter_map(|i| list.row_data(i)).map(|p| format!("{}{} [{}] {}", if p.smart { "smart " } else { "" }, p.name, p.info, p.summary)).collect();
        line(r, format!("[playlists] {}", rows.join(" | ")));
        check(r, "smart playlists listed with their rules in words and counts", rows.iter().filter(|r| r.starts_with("smart ")).all(|r| r.contains(" tracks]") || r.contains("[0 tracks]")), String::new());
        snapshot(app, "s-21-playlists-sidebar");
        size_window(st, 960, 620);
    });
    step!(1400, |st, app, _r| {
        snapshot(app, "s-22-playlists-min");
        app.global::<Nav>().set_sidebar_pinned(false);
        size_window(st, 1920, 1080);
    });
    search_operators(&mut steps, &r);
    step!(800, |_st, _app, r| {
        let fails = FAILS.with(|f| f.get());
        line(r, format!("\n{fails} failed"));
        write_report("smart", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

/// Search operators (`library::smart::search`): typed into the search field, the results by the
/// rules (tracks only for a play count), the hint for what was not understood, the syntax help,
/// a plain query still plain, timings of both paths, and "Save as smart playlist" making a
/// smart playlist that holds what the search found.
fn search_operators(steps: &mut Vec<Step>, r: &Report) {
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }
    const OPS: &str = "plays:>=1 -is:remote";
    step!(300, |st, app, r| {
        line(r, "[search] operators typed into the field".into());
        views::go(st, Page::Search);
        app.invoke_focus_search();
    });
    step!(400, |_st, app, _r| {
        type_text(app, OPS);
    });
    step!(1000, |st, app, r| {
        let s = app.global::<crate::Search>();
        let tracks = s.get_tracks();
        let ids: Vec<i64> = (0..tracks.row_count()).filter_map(|i| tracks.row_data(i)).map(|t| i64::from(t.id)).collect();
        let played = ids.iter().all(|id| query::play_count(&st.db, *id).unwrap_or(0) >= 1);
        check(r, "the query reads as operators", s.get_operators() && s.get_hint().is_empty(), format!("query {:?}, operators {}, hint {:?}", s.get_query().to_string(), s.get_operators(), s.get_hint().to_string()));
        check(r, "tracks by the rules: every one played", !ids.is_empty() && played, format!("{} tracks, more {}", ids.len(), s.get_more_tracks()));
        check(r, "a play count says nothing of albums or artists: those sections are left out", s.get_albums().row_count() == 0 && s.get_artists().row_count() == 0, format!("{} albums, {} artists", s.get_albums().row_count(), s.get_artists().row_count()));
        snapshot(app, "s-23-search-operators");
        app.global::<crate::Search>().set_query("re:stacks year:1900..2100".into());
        views::search(st, "re:stacks year:1900..2100");
    });
    step!(900, |_st, app, r| {
        let s = app.global::<crate::Search>();
        check(r, "what is not understood is searched as text, and said under the field", s.get_operators() && s.get_hint().contains("re:"), format!("hint {:?}", s.get_hint().to_string()));
        snapshot(app, "s-24-search-hint");
        s.set_help_open(true);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "s-25-search-help");
        app.global::<crate::Search>().set_help_open(false);
    });
    step!(300, |st, app, r| {
        app.global::<crate::Search>().set_query("year:1990..2030".into());
        views::search(st, "year:1990..2030");
        // Both paths, timed here on the UI thread (the app runs them on the search thread).
        let time = |q: &str| {
            let mut v: Vec<f64> = (0..9)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = query::search(&st.db, q, 24);
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            v.sort_by(f64::total_cmp);
            v[4]
        };
        for q in ["love", "the", "kana", "plays:>=1 -is:remote", "year:1990..2030", "genre:rock OR genre:jazz -plays:0", "added:<30d is:lossless", "artist:the"] {
            line(r, format!("  search {q:?}: {:.2} ms (median of 9)", time(q)));
        }
    });
    step!(900, |st, app, r| {
        let s = app.global::<crate::Search>();
        let want = query::search(&st.db, "year:1990..2030", 24).unwrap_or_default();
        check(r, "a year is an album's too: albums and tracks", s.get_albums().row_count() == want.albums.len() && s.get_tracks().row_count() == want.tracks.len(), format!("{} albums, {} tracks", s.get_albums().row_count(), s.get_tracks().row_count()));
        snapshot(app, "s-26-search-year");
        s.set_query("the".into());
        views::search(st, "the");
    });
    step!(900, |st, app, r| {
        let s = app.global::<crate::Search>();
        let want = query::search(&st.db, "the", 24).unwrap_or_default();
        check(r, "a plain query is plain search, as before", !s.get_operators() && !want.operators && !want.tracks.is_empty() && s.get_tracks().row_count() == want.tracks.len() && s.get_albums().row_count() == want.albums.len(), format!("{} albums, {} tracks", s.get_albums().row_count(), s.get_tracks().row_count()));
        s.set_query(OPS.into());
        views::search(st, OPS);
    });
    step!(900, |_st, app, r| {
        line(r, "[search] Save as smart playlist".into());
        app.global::<crate::Actions>().invoke_search_save_smart();
    });
    step!(900, |_st, app, r| {
        let e = app.global::<SmartEdit>();
        check(r, "the editor opens with the search's rules and the query as the name", app.global::<Nav>().get_page() == Page::SmartEdit && e.get_name() == OPS && lines().contains("Play count is at least") && lines().contains("From the server"), format!("name {:?}, rows: {}", e.get_name().to_string(), lines()));
        snapshot(app, "s-27-search-to-editor");
        e.invoke_save();
    });
    step!(1200, |st, _app, r| {
        let id = id_named(st, OPS);
        let mut held = smart_ids(st, id);
        let mut found: Vec<i64> = query::search_section(&st.db, OPS, query::SearchKind::Tracks, usize::MAX).unwrap_or_default().tracks.iter().map(|t| t.id).collect();
        held.sort_unstable();
        found.sort_unstable();
        check(r, "saved: the smart playlist holds what the search found", id > 0 && !held.is_empty() && held == found, format!("id {id}, {} tracks, search {}", held.len(), found.len()));
    });
    step!(1200, |_st, app, _r| {
        snapshot(app, "s-28-search-saved");
    });
}
