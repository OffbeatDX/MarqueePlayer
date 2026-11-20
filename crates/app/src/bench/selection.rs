//! The selection of track lists (`selected.rs`), the second half of `MP_BENCH=playlist`: on its
//! playlist of 3,000 tracks, the album page and the queue. Clicks go through Slint's own event
//! dispatch inside the app, Ctrl and Shift held by key events of their own before the click
//! (`Window::dispatch_event`: no OS input, the real pointer never moves). Rows are found by
//! right clicks down the page (a row's menu chooses it, so the selection says which row is
//! there), then clicked:
//!
//! - a click, Shift+click, Ctrl+click, Shift+click from a moved anchor; each row's `selected`;
//! - the menu of several tracks (its entries, a snapshot of it) and of a row outside them;
//! - Ctrl+A on 3,000 rows and Shift+click across all of them, timed; Esc; the arrows and Shift;
//! - Add to queue, favourites, a rating, Add to playlist, each once for all, the pill's words;
//! - a drag of two rows within the playlist (a real drag in the window): moved as a block, still
//!   chosen; Delete removes the selection from the playlist and nothing from the library;
//! - the selection gone when the list changes or the page does, and a menu opened before the
//!   change doing nothing;
//! - search results and their "See all" page;
//! - the album page's list, and the queue: Delete, a block moved, Esc.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use library::query;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

use crate::bench::{Step, menu_labels, run_steps, snapshot, verdict, write_report};
use crate::selected::{self, QUEUE};
use crate::state::State;
use crate::{Actions, AppWindow, MenuStore, Nav, Page, Playlists, views};

#[derive(Default)]
struct Ctx {
    r: String,
    /// The bench's playlist of 3,000, and one to add to.
    pid: i64,
    target: i64,
    /// Where rows are clicked: x, the first row's top, the rows' pitch (the list on screen).
    x: f32,
    top: f32,
    pitch: f32,
    /// The queue's rows (x: on their length, not a link).
    qx: f32,
    qtop: f32,
    /// Before an action: the queue's length, the playlist's tracks.
    queue_len: usize,
    before: Vec<i64>,
}

type C = Rc<RefCell<Ctx>>;

fn line(c: &C, text: String) {
    log::info!("selection bench: {text}");
    let mut c = c.borrow_mut();
    c.r.push_str(&text);
    c.r.push('\n');
}

fn check(c: &C, what: &str, ok: bool, detail: impl std::fmt::Display) {
    verdict(&mut c.borrow_mut().r, what, ok, detail);
}

fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

fn mods(app: &AppWindow, keys: &[Key], down: bool) {
    for k in keys {
        let text = (*k).into();
        app.window().dispatch_event(if down { WindowEvent::KeyPressed { text } } else { WindowEvent::KeyReleased { text } });
    }
}

/// A click with these keys held (Ctrl, Shift).
fn click(app: &AppWindow, x: f32, y: f32, held: &[Key]) {
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    mods(app, held, true);
    w.dispatch_event(WindowEvent::PointerMoved { position: p });
    w.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
    mods(app, held, false);
}

fn right_click(app: &AppWindow, x: f32, y: f32) {
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position: p });
    w.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
    w.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
}

fn key(app: &AppWindow, k: Key, held: &[Key]) {
    crate::bench::press(app, k, held);
}

fn ctrl_a(app: &AppWindow) {
    let w = app.window();
    w.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    w.dispatch_event(WindowEvent::KeyPressed { text: "a".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "a".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
}

/// Right clicks down the page at `x`, every 3 px from `from`: the first that opens a menu for
/// one row of `ctx`, that row and where it was hit (its top, within 3 px). The menu is closed
/// and the row left unchosen.
fn probe(st: &Rc<State>, app: &AppWindow, ctx: &str, x: f32, from: f32, to: f32) -> Option<(usize, f32)> {
    let mut y = from;
    while y < to {
        right_click(app, x, y);
        let open = app.global::<MenuStore>().get_open();
        let rows = selected::rows(st, ctx);
        if open {
            crate::menus::key(st, "escape");
        }
        if open && rows.len() == 1 {
            selected::clear(st, ctx);
            return Some((rows[0], y));
        }
        y += 3.0;
    }
    None
}

/// The window y to click row `i` of the list found by `probe` (its title's line).
fn row_y(c: &C, i: usize) -> f32 {
    let c = c.borrow();
    c.top + i as f32 * c.pitch + 14.0
}

fn rows(st: &State, ctx: &str) -> Vec<usize> {
    selected::rows(st, ctx)
}

/// Which rows the playlist page's model draws as chosen.
fn drawn(app: &AppWindow) -> Vec<usize> {
    let m = app.global::<Playlists>().get_tracks();
    (0..m.row_count()).filter(|&i| m.row_data(i).is_some_and(|r| r.selected)).collect()
}

fn activate(st: &Rc<State>, app: &AppWindow, label: &str) -> bool {
    match menu_labels(app).iter().position(|l| l == label) {
        Some(i) => {
            crate::menus::activate(st, i as i32, -1);
            true
        }
        None => false,
    }
}

fn flash(app: &AppWindow) -> String {
    app.global::<Nav>().get_flash().to_string()
}

fn playlist_ids(st: &State, id: i64) -> Vec<i64> {
    query::playlist_tracks(&st.db, id).unwrap_or_default().iter().map(|t| t.id).collect()
}

/// Runs after the playlist bench's own steps (its report so far in `report`); writes the
/// report and quits.
pub fn run(report: String, pid: i64) {
    let c: C = Rc::new(RefCell::new(Ctx { r: report, pid, ..Ctx::default() }));
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let c = c.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &C)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &c))));
        }};
    }

    // ---- one row removed and moved where the stored positions have a gap ---------------------
    step!(200, |st, app, c| {
        let ids: Vec<i64> = playlist_ids(st, c.borrow().pid).into_iter().take(5).collect();
        let gap = query::create_playlist(&st.db, "Bench gaps").unwrap();
        query::add_to_playlist(&st.db, gap, &ids).unwrap();
        // As a scan's delete of a track leaves it: positions 0, 2, 3, 4.
        st.db.execute("DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = 1", [gap]).unwrap();
        views::open_playlist(st, gap);
        views::move_in_playlist(st, 0, 4); // the first row shown to the end
        let after = playlist_ids(st, gap);
        check(c, "a drag of one row in a playlist with a gap moves that row", after == [ids[2], ids[3], ids[4], ids[0]], format!("{after:?} of {ids:?}"));
        st.db.execute("DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = 1", [gap]).unwrap();
        views::refresh_all(st); // as after a sync: the open page follows
        let shown = app.global::<Playlists>().get_tracks().row_count();
        check(c, "an open playlist page is read again after the library changed", shown == 3, shown);
        selected::delete(st, "playlist", 1); // the 2nd row shown: ids[4]
        let after = playlist_ids(st, gap);
        check(c, "Delete on one row of a playlist with a gap removes that row", after == [ids[2], ids[0]], format!("{after:?} of {ids:?}"));
        let _ = query::delete_playlist(&st.db, gap);
    });
    // ---- the playlist of 3,000: where its rows are -------------------------------------------
    step!(200, |st, app, c| {
        line(c, "\n[selection]".into());
        let target = query::create_playlist(&st.db, "Bench selection").unwrap();
        c.borrow_mut().target = target;
        views::refresh_playlists(st);
        views::open_playlist(st, c.borrow().pid);
        app.global::<Nav>().set_page_scroll(0.0);
    });
    step!(900, |st, app, c| {
        let (w, h) = size(app);
        let x = w * 0.5;
        match probe(st, app, "playlist", x, 60.0, h - 120.0) {
            Some((r, y)) => {
                let mut cc = c.borrow_mut();
                cc.x = x;
                cc.pitch = 72.0;
                cc.top = y - r as f32 * 72.0;
                drop(cc);
                line(c, format!("playlist rows: row {r} at y {y:.0} (window {w:.0} x {h:.0})"));
            }
            None => check(c, "the playlist's rows are found by right clicks", false, "no row answered"),
        }
    });
    // ---- clicks with Ctrl and Shift --------------------------------------------------------------
    step!(200, |st, app, c| {
        let x = c.borrow().x;
        click(app, x, row_y(c, 0), &[]);
        check(c, "a click chooses one row", rows(st, "playlist") == [0], format!("{:?}", rows(st, "playlist")));
        click(app, x, row_y(c, 4), &[Key::Shift]);
        check(c, "Shift+click takes the range from it", rows(st, "playlist") == [0, 1, 2, 3, 4], format!("{:?}", rows(st, "playlist")));
        click(app, x, row_y(c, 2), &[Key::Control]);
        check(c, "Ctrl+click takes a row away", rows(st, "playlist") == [0, 1, 3, 4], format!("{:?}", rows(st, "playlist")));
        click(app, x, row_y(c, 6), &[Key::Control]);
        click(app, x, row_y(c, 8), &[Key::Shift]);
        check(c, "Shift+click from the anchor Ctrl+click moved", rows(st, "playlist") == [6, 7, 8], format!("{:?}", rows(st, "playlist")));
        check(c, "the rows are drawn chosen (the model's flag)", drawn(app) == [6, 7, 8], format!("{:?}", drawn(app)));
        let playing_marked = app.global::<Playlists>().get_tracks().row_data(7).is_some_and(|r| r.selected && !r.playing);
        check(c, "chosen is apart from playing", playing_marked, "");
    });
    step!(300, |st, app, c| {
        snapshot(app, "select-playlist");
        let x = c.borrow().x;
        right_click(app, x + 40.0, row_y(c, 7));
        let labels = menu_labels(app);
        check(c, "a right click on a chosen row: the menu for all of them", labels.first().map(String::as_str) == Some("3 tracks") && labels.iter().any(|l| l == "Remove from playlist"), format!("{labels:?}"));
        check(c, "the selection stays", rows(st, "playlist") == [6, 7, 8], format!("{:?}", rows(st, "playlist")));
    });
    step!(400, |st, app, c| {
        snapshot(app, "select-playlist-menu");
        crate::menus::key(st, "escape");
        let x = c.borrow().x;
        right_click(app, x, row_y(c, 3));
        let labels = menu_labels(app);
        check(c, "a right click outside it chooses that row alone, its own menu", rows(st, "playlist") == [3] && labels.first().map(String::as_str) == Some("Play"), format!("{:?} {labels:?}", rows(st, "playlist")));
        crate::menus::key(st, "escape");
    });
    // ---- the keyboard, and the long list's cost ------------------------------------------------
    step!(200, |st, app, c| {
        let x = c.borrow().x;
        click(app, x, row_y(c, 1), &[]);
        let t = Instant::now();
        ctrl_a(app);
        let all_ms = t.elapsed().as_secs_f64() * 1000.0;
        let n = rows(st, "playlist").len();
        let len = slint::Model::row_count(&*st.lists.playlist);
        check(c, "Ctrl+A chooses every row of the list that has the keyboard", n == len && len > 2000, format!("{n} of {len}"));
        line(c, format!("  Ctrl+A on {len} rows: {all_ms:.2} ms in the handler"));
        key(app, Key::Escape, &[]);
        check(c, "Esc clears it", rows(st, "playlist").is_empty(), format!("{:?}", rows(st, "playlist").len()));
        check(c, "and stays on the page", app.global::<Nav>().get_page() == Page::Playlist, format!("{:?}", app.global::<Nav>().get_page()));
        click(app, x, row_y(c, 3), &[]);
        key(app, Key::DownArrow, &[]);
        check(c, "Down moves the selection", rows(st, "playlist") == [4], format!("{:?}", rows(st, "playlist")));
        key(app, Key::DownArrow, &[Key::Shift]);
        key(app, Key::DownArrow, &[Key::Shift]);
        key(app, Key::UpArrow, &[Key::Shift]);
        check(c, "Shift+Down / Up stretch it from the anchor", rows(st, "playlist") == [4, 5], format!("{:?}", rows(st, "playlist")));
        // Shift+End: every row from the anchor; timed with the draw that follows.
        let t = Instant::now();
        key(app, Key::End, &[Key::Shift]);
        let end_ms = t.elapsed().as_secs_f64() * 1000.0;
        check(c, "Shift+End takes the rest of the list", rows(st, "playlist").len() == len - 4, format!("{} of {len}", rows(st, "playlist").len()));
        line(c, format!("  Shift+End over {} rows: {end_ms:.2} ms in the handler", len - 4));
        check(c, "choosing thousands is quick", all_ms < 50.0 && end_ms < 50.0, format!("{all_ms:.1} / {end_ms:.1} ms"));
        app.global::<Nav>().set_page_scroll(0.0);
        key(app, Key::Home, &[]);
    });
    // ---- the menu's actions, each once for all ----------------------------------------------------
    step!(300, |st, app, c| {
        let x = c.borrow().x;
        click(app, x, row_y(c, 0), &[]);
        click(app, x, row_y(c, 2), &[Key::Shift]);
        c.borrow_mut().queue_len = st.queue.borrow().items().len();
        right_click(app, x, row_y(c, 1));
        let ok = activate(st, app, "Add to queue");
        let len = st.queue.borrow().items().len();
        let want = c.borrow().queue_len + 3;
        check(c, "Add to queue adds the three, once", ok && len == want, format!("{len} (want {want}); pill {:?}", flash(app)));
        check(c, "the pill says how many", flash(app) == "Added 3 tracks to the queue", flash(app));
        let ids: Vec<i64> = selected::rows(st, "playlist").iter().filter_map(|&i| st.lists.playlist.get(i)).map(|t| t.id).collect();
        right_click(app, x, row_y(c, 1));
        let ok = activate(st, app, "Add to favourites");
        let favs = views::favourite_set(st, ids.iter().copied());
        check(c, "Add to favourites marks all three (one transaction)", ok && favs.len() == 3, format!("{} of {ids:?}; pill {:?}", favs.len(), flash(app)));
        right_click(app, x, row_y(c, 1));
        check(c, "then the menu offers to remove them", menu_labels(app).iter().any(|l| l == "Remove from favourites"), format!("{:?}", menu_labels(app)));
        let ok = activate(st, app, "Rate\u{2026}");
        let labels = menu_labels(app);
        check(c, "Rate… opens the stars for all of them", ok && labels.len() == 6, format!("{labels:?}"));
        crate::menus::activate(st, 4, -1);
        let rated = ids.iter().all(|&id| crate::rows::rating_of(id) == 4 && query::rating(&st.db, id).unwrap_or(0) == 4);
        check(c, "four stars for each", rated, format!("pill {:?}", flash(app)));
        right_click(app, x, row_y(c, 1));
        let i = menu_labels(app).iter().position(|l| l == "Add to playlist").unwrap_or(0) as i32;
        let target = c.borrow().target;
        let ordinary: Vec<i64> = query::playlists(&st.db).unwrap_or_default().into_iter().filter(|p| !p.smart()).map(|p| p.id).collect();
        let j = ordinary.iter().position(|&p| p == target).unwrap_or(0) as i32;
        crate::menus::activate(st, i, 2 + j);
        let added = playlist_ids(st, target);
        check(c, "Add to playlist adds them in list order", added == ids, format!("{added:?} (want {ids:?}); pill {:?}", flash(app)));
        check(c, "with one note", flash(app) == "Added 3 tracks to \u{201c}Bench selection\u{201d}", flash(app));
        check(c, "the selection is still there after them", rows(st, "playlist") == [0, 1, 2], format!("{:?}", rows(st, "playlist")));
    });
    // ---- a drag of two chosen rows within the playlist ---------------------------------------
    step!(300, |st, app, c| {
        let x = c.borrow().x;
        click(app, x, row_y(c, 1), &[]);
        click(app, x, row_y(c, 3), &[Key::Control]);
        c.borrow_mut().before = st.lists.playlist.tracks().iter().map(|t| t.id).collect();
        // Pressed on a chosen row (which waits for the release), then away: a drag.
        let p = LogicalPosition::new(x, row_y(c, 3));
        app.window().dispatch_event(WindowEvent::PointerMoved { position: p });
        app.window().dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
    });
    // Held past the page's Flickable delay (a press that moves within 500 ms scrolls the page),
    // then moved.
    step!(800, |_st, app, c| {
        let x = c.borrow().x;
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, row_y(c, 3) + 20.0) });
    });
    step!(60, |_st, app, c| {
        // Onto row 7's upper part: the line before it.
        let (x, y) = (c.borrow().x, c.borrow().top + 7.0 * 72.0 + 10.0);
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
    });
    step!(60, |_st, app, c| {
        let (x, y) = (c.borrow().x, c.borrow().top + 7.0 * 72.0 + 12.0);
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
        snapshot(app, "select-playlist-drag");
    });
    step!(60, |_st, app, c| {
        let (x, y) = (c.borrow().x, c.borrow().top + 7.0 * 72.0 + 12.0);
        app.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
    });
    step!(400, |st, _app, c| {
        let before = c.borrow().before.clone();
        let (order, _) = crate::selection::move_block(before.len(), &[1, 3], 7);
        let want: Vec<i64> = order.iter().map(|&j| before[j]).collect();
        let now = playlist_ids(st, c.borrow().pid);
        check(c, "a drag of two chosen rows moves both, as a block in their order", now == want, format!("first rows {:?}", &now[..now.len().min(9)]));
        check(c, "and they stay chosen where they landed", rows(st, "playlist") == [5, 6], format!("{:?}", rows(st, "playlist")));
    });
    // ---- Delete -------------------------------------------------------------------------------------
    step!(200, |st, app, c| {
        let x = c.borrow().x;
        let tracks_before: i64 = st.db.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap_or(0);
        let before = playlist_ids(st, c.borrow().pid);
        click(app, x, row_y(c, 0), &[]);
        click(app, x, row_y(c, 1), &[Key::Shift]);
        key(app, Key::Delete, &[]);
        let after = playlist_ids(st, c.borrow().pid);
        let tracks_after: i64 = st.db.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap_or(0);
        check(c, "Delete removes the two chosen from the playlist", after.len() + 2 == before.len() && after[..] == before[2..], format!("{} -> {}; pill {:?}", before.len(), after.len(), flash(app)));
        check(c, "and nothing from the library", tracks_after == tracks_before, format!("{tracks_before} -> {tracks_after}"));
        check(c, "the pill says so", flash(app) == "Removed 2 tracks from \u{201c}Bench 3000\u{201d}", flash(app));
    });
    // ---- the selection follows the list ---------------------------------------------------------
    step!(200, |st, app, c| {
        let x = c.borrow().x;
        click(app, x, row_y(c, 0), &[]);
        click(app, x, row_y(c, 1), &[Key::Shift]);
        right_click(app, x, row_y(c, 0));
        c.borrow_mut().queue_len = st.queue.borrow().items().len();
        // The list changes under the open menu (an album dropped on it from the sidebar).
        if let Ok(a) = st.db.query_row("SELECT id FROM albums WHERE title = 'Lemonade'", [], |r| r.get::<_, i64>(0)) {
            views::drop_on_playlist(st, crate::dnd::Payload::Album(a), c.borrow().pid);
        }
        check(c, "the list's tracks changed: nothing chosen", rows(st, "playlist").is_empty(), format!("{:?}", rows(st, "playlist")));
        let ok = activate(st, app, "Add to queue");
        let len = st.queue.borrow().items().len();
        check(c, "the menu opened before does nothing", ok && len == c.borrow().queue_len, format!("{len}; pill {:?}", flash(app)));
        check(c, "and says why", flash(app) == "Those tracks are no longer chosen: nothing was done", flash(app));
        click(app, x, row_y(c, 2), &[]);
        views::go(st, Page::Home);
    });
    step!(500, |st, _app, c| {
        check(c, "another page: nothing chosen", ALL_EMPTY(st), "");
        views::back(st);
    });
    step!(600, |st, app, c| {
        check(c, "back on the playlist: still nothing chosen", rows(st, "playlist").is_empty() && drawn(app).is_empty(), format!("{:?}", rows(st, "playlist")));
        // A drag of a chosen row onto a playlist in the sidebar (its drop callback, as the
        // sidebar calls it): the rows chosen, in order.
        let x = c.borrow().x;
        click(app, x, row_y(c, 4), &[]);
        click(app, x, row_y(c, 6), &[Key::Shift]);
        let want: Vec<i64> = [4, 5, 6].iter().filter_map(|&i| st.lists.playlist.get(i)).map(|t| t.id).collect();
        let target = c.borrow().target;
        let before = playlist_ids(st, target).len();
        let payload = app.global::<Actions>().invoke_track_payload("playlist".into(), 5);
        app.global::<Actions>().invoke_playlist_drop(payload, target as i32);
        let now = playlist_ids(st, target);
        check(c, "a chosen row dropped on a playlist carries the selection", now.len() == before + 3 && now[before..] == want[..], format!("{:?}", &now[before..]));
    });
    // ---- search results and their "See all" ------------------------------------------------
    step!(200, |st, app, _c| {
        views::go(st, Page::Search);
        app.global::<crate::Search>().set_query("bulk".into());
        views::search(st, "bulk");
    });
    step!(1500, |st, app, c| {
        let n = app.global::<crate::Search>().get_tracks().row_count();
        let a = app.global::<Actions>();
        a.invoke_select_press("search".into(), 0, false, false);
        a.invoke_select_press("search".into(), 2, false, true);
        check(c, "search results: a range chosen", n >= 3 && rows(st, "search") == [0, 1, 2], format!("{:?} of {n}", rows(st, "search")));
        crate::menus::request(st, "track", "search", 1, 0, 600.0, 300.0);
        check(c, "search results: the menu for three", menu_labels(app).first().map(String::as_str) == Some("3 tracks"), format!("{:?}", menu_labels(app)));
        crate::menus::key(st, "escape");
        a.invoke_search_all("tracks".into());
    });
    step!(1200, |st, app, c| {
        check(c, "See all is another page: nothing chosen", ALL_EMPTY(st), format!("{:?}", app.global::<Nav>().get_page()));
        let n = slint::Model::row_count(&*st.lists.tracks);
        let a = app.global::<Actions>();
        a.invoke_select_press("tracks".into(), 1, false, false);
        a.invoke_select_press("tracks".into(), 3, true, false);
        a.invoke_select_press("tracks".into(), 5, true, false);
        check(c, "See all: Ctrl toggles three rows", rows(st, "tracks") == [1, 3, 5], format!("{:?} of {n}", rows(st, "tracks")));
        let len = st.queue.borrow().items().len();
        crate::menus::request(st, "track", "tracks", 3, 0, 600.0, 300.0);
        let ok = activate(st, app, "Add to queue");
        check(c, "See all: Add to queue adds the three", ok && st.queue.borrow().items().len() == len + 3, flash(app));
    });
    // ---- the album page -----------------------------------------------------------------------
    step!(200, |st, _app, _c| {
        if let Ok(a) = st.db.query_row("SELECT id FROM albums WHERE title = 'Lemonade'", [], |r| r.get::<_, i64>(0)) {
            views::open_album(st, a);
        }
    });
    step!(900, |st, app, c| {
        let (w, h) = size(app);
        match probe(st, app, "album", w * 0.5, 60.0, h - 100.0) {
            Some((r, y)) => {
                let mut cc = c.borrow_mut();
                cc.x = w * 0.5;
                cc.pitch = 52.0;
                cc.top = y - r as f32 * 52.0;
                drop(cc);
                line(c, format!("album rows: row {r} at y {y:.0}"));
            }
            None => check(c, "the album's rows are found", false, "no row answered"),
        }
        let x = c.borrow().x;
        click(app, x, row_y(c, 0), &[]);
        click(app, x, row_y(c, 2), &[Key::Shift]);
        check(c, "album page: Shift+click takes the range", rows(st, "album") == [0, 1, 2], format!("{:?}", rows(st, "album")));
        check(c, "only one list has a selection", rows(st, "playlist").is_empty(), format!("{:?}", rows(st, "playlist")));
        right_click(app, x, row_y(c, 1));
        check(c, "album page: the menu for the three", menu_labels(app).first().map(String::as_str) == Some("3 tracks"), format!("{:?}", menu_labels(app)));
    });
    step!(400, |st, app, c| {
        snapshot(app, "select-album-menu");
        let want: Vec<i64> = st.lists.album.selected_tracks().iter().map(|t| t.id).collect();
        let at = st.queue.borrow().current_index().map_or(0, |i| i + 1);
        let ok = activate(st, app, "Play next");
        let q: Vec<i64> = st.queue.borrow().items().iter().map(|i| i.track.id).collect();
        check(c, "Play next puts the three after the playing track, in order", ok && q.get(at..at + 3) == Some(&want[..]), format!("{:?} at {at}; pill {:?}", q.get(at..(at + 3).min(q.len())), flash(app)));
    });
    // ---- the queue ------------------------------------------------------------------------------
    step!(200, |st, app, _c| {
        let more: Vec<library::Track> = (10..18).filter_map(|i| st.lists.playlist.get(i)).collect();
        crate::playback::enqueue(st, more, false);
        app.global::<Nav>().set_queue_open(true);
    });
    step!(900, |st, app, c| {
        let (w, h) = size(app);
        let qx = w - 150.0;
        match probe(st, app, QUEUE, qx, 40.0, h - 100.0) {
            Some((r, y)) => {
                let mut cc = c.borrow_mut();
                // Clicked on its length (the title and artist are links in the queue).
                cc.qx = w - 72.0;
                cc.qtop = y - r as f32 * 52.0;
                drop(cc);
                line(c, format!("queue rows: row {r} at y {y:.0}"));
            }
            None => check(c, "the queue's rows are found", false, "no row answered"),
        }
        let (qx, qtop) = (c.borrow().qx, c.borrow().qtop);
        let qy = |i: usize| qtop + i as f32 * 52.0 + 14.0;
        let before: Vec<u64> = st.queue.borrow().items().iter().map(|i| i.uid).collect();
        click(app, qx, qy(0), &[]);
        click(app, qx, qy(2), &[Key::Shift]);
        check(c, "queue: Shift+click takes the range", rows(st, QUEUE) == [0, 1, 2], format!("{:?}", rows(st, QUEUE)));
        snapshot(app, "select-queue");
        key(app, Key::Delete, &[]);
        let after: Vec<u64> = st.queue.borrow().items().iter().map(|i| i.uid).collect();
        check(c, "Delete removes the three from the queue", after[..] == before[3..], format!("{} -> {}; pill {:?}", before.len(), after.len(), flash(app)));
    });
    // (After a frame: the queue's rows are laid out again.)
    step!(300, |st, app, c| {
        let (qx, qtop) = (c.borrow().qx, c.borrow().qtop);
        let qy = |i: usize| qtop + i as f32 * 52.0 + 14.0;
        let after: Vec<u64> = st.queue.borrow().items().iter().map(|i| i.uid).collect();
        snapshot(app, "select-queue-after-delete");
        click(app, qx, qy(0), &[]);
        line(c, format!("  queue after a click on row 0: {:?}", rows(st, QUEUE)));
        click(app, qx, qy(1), &[Key::Shift]);
        line(c, format!("  queue chosen before the drag: {:?} of {}", rows(st, QUEUE), after.len()));
        let before = after;
        let payload = app.global::<Actions>().invoke_queue_payload(1);
        app.global::<Actions>().invoke_queue_drop(payload, 4);
        let after: Vec<u64> = st.queue.borrow().items().iter().map(|i| i.uid).collect();
        let (order, _) = crate::selection::move_block(before.len(), &[0, 1], 4);
        let want: Vec<u64> = order.iter().map(|&j| before[j]).collect();
        check(c, "a chosen queue row dragged moves the two as a block", after == want, format!("{:?}", &after[..after.len().min(6)]));
        check(c, "and they stay chosen", rows(st, QUEUE) == [2, 3], format!("{:?}", rows(st, QUEUE)));
        right_click(app, qx, qy(2));
        check(c, "the queue's menu for two", menu_labels(app).iter().any(|l| l == "Remove from queue") && menu_labels(app).first().map(String::as_str) == Some("2 tracks"), format!("{:?}", menu_labels(app)));
        crate::menus::key(st, "escape");
        key(app, Key::Escape, &[]);
        check(c, "Esc clears the queue's", rows(st, QUEUE).is_empty(), format!("{:?}", rows(st, QUEUE)));
        app.global::<Nav>().set_queue_open(false);
    });
    step!(300, |_st, _app, c| {
        let r = c.borrow().r.clone();
        write_report("playlist", &r);
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

#[allow(non_snake_case)]
fn ALL_EMPTY(st: &State) -> bool {
    ["album", "artist", "search", "playlist", "tracks", "smart", "preview", QUEUE].iter().all(|c| selected::rows(st, c).is_empty())
}
