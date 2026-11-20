//! `MP_BENCH=home-shelves` (`bench-home-shelves.txt`, `hs-*.png`, the core set): Home's
//! hero and grid shelves, driven by the pointer and the keys through the window's own
//! dispatch (no OS input).
//!
//! - The hero: its big cover's play button not shown without the pointer, shown while the
//!   pointer is on the cover, not elsewhere on the card; the pointer resting on the button and
//!   crossing its edge to the cover and back (a quarter pixel at a time), drawn after each
//!   move: shown every time (no flicker); a click on it plays the album, a click elsewhere on
//!   the cover opens it. The strip: a single click features a cover and plays nothing; a
//!   double click (at once, and with 150 ms between the clicks) plays the cover clicked.
//! - The shelf editor's rows: "More rows" and "Fewer rows" clicked, read back from the
//!   settings and the editor, clamped at 6 and 1; a row shelf's albums untouched.
//! - Grid shelves: 1, 2 and 6 rows at several window sizes and interface sizes (70%, 100%,
//!   150%): the page's height (its furthest scroll) is that of the rows wanted, the first and
//!   the last row drawn full; a shelf of fewer albums than a row holds shows them in one row;
//!   End and the arrows in the grid reach the last album shown, never one hidden.

use std::cell::RefCell;
use std::rc::Rc;

use library::ShelfQuery;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

use crate::bench::{Step, press, run_steps, snapshot, verdict, write_report};
use crate::settings::{ShelfConfig, ShelfLayout};
use crate::state::State;
use crate::{AlbumPage, AppWindow, Home, Nav, Page, ShelfEditor, views};

#[derive(Default)]
struct Ctx {
    r: String,
    /// The monitor's scale factor at 100%.
    monitor: f32,
    /// The hero's strip: the y (logical) of its covers' middle, once found.
    strip_y: f32,
    /// The shelf editor's first row: its top (logical), once found.
    editor_top: f32,
    /// The grid now measured: columns, albums shown, rows.
    cols: usize,
    shown: usize,
    rows: usize,
    albums: usize,
    /// Tabs from the page's start to the grid (found once).
    tabs: Option<usize>,
}

type C = Rc<RefCell<Ctx>>;

fn line(c: &C, text: String) {
    log::info!("home-shelves bench: {text}");
    let mut c = c.borrow_mut();
    c.r.push_str(&text);
    c.r.push('\n');
}

fn check(c: &C, what: &str, ok: bool, detail: impl std::fmt::Display) {
    verdict(&mut c.borrow_mut().r, what, ok, detail);
}

fn moved(app: &AppWindow, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
}

/// The pointer on the page's top band, clear of the shelves and of the window's edge (which
/// opens the sidebar).
fn away(app: &AppWindow) {
    let (w, _) = logical(app);
    moved(app, w * 0.6, 30.0);
}

fn click(app: &AppWindow, x: f32, y: f32) {
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position: p });
    w.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
}

/// The window's logical size.
fn logical(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

/// One pixel of the window as drawn now, at a logical point.
fn pixel(app: &AppWindow, x: f32, y: f32) -> Option<[u8; 3]> {
    let buf = app.window().take_snapshot().ok()?;
    pixel_in(&buf, app.window().scale_factor(), x, y)
}

fn pixel_in(buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, scale: f32, x: f32, y: f32) -> Option<[u8; 3]> {
    let (px, py) = ((x * scale) as u32, (y * scale) as u32);
    (px < buf.width() && py < buf.height()).then(|| {
        let p = buf.as_slice()[(py * buf.width() + px) as usize];
        [p.r, p.g, p.b]
    })
}

fn diff(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter().zip(b).map(|(&x, y)| (x as i32 - y as i32).unsigned_abs()).sum()
}

fn accent(app: &AppWindow) -> [u8; 3] {
    let c = app.global::<crate::Theme>().get_accent();
    [c.red(), c.green(), c.blue()]
}

fn set_shelves(st: &Rc<State>, shelves: Vec<ShelfConfig>) {
    st.settings.borrow_mut().shelves = shelves;
    views::shelves_changed(st);
}

fn grid_shelf(query: ShelfQuery, rows: usize) -> ShelfConfig {
    ShelfConfig { rows, ..ShelfConfig::new(query, ShelfLayout::Grid, 18) }
}

fn playing_album(st: &State) -> Option<i64> {
    st.queue.borrow().current().map(|i| i.track.album_id)
}

/// The first shelf on Home: its albums' ids, and the album it features.
fn first_shelf(app: &AppWindow) -> (Vec<i64>, usize) {
    let shelves = app.global::<Home>().get_shelves();
    shelves.row_data(0).map_or((Vec::new(), 0), |v| ((0..v.albums.row_count()).filter_map(|i| v.albums.row_data(i)).map(|a| i64::from(a.id)).collect(), v.hero.max(0) as usize))
}

// The hero's geometry (home.slint, `HeroShelf`), logical px at the page's top: the card at the
// page's margin and top, its cover 240 px inside 30 px of padding, the play button 56 px,
// 14 px in from the cover's bottom right corner; the strip's covers 52 px, 8 px apart, from
// the text column's left (the cover's right plus 30 px).
const COVER_X: f32 = 40.0 + 30.0;
const COVER_Y: f32 = 72.0 + 30.0;
const COVER: f32 = 240.0;
const BUTTON_CX: f32 = COVER_X + COVER - 14.0 - 28.0;
const BUTTON_CY: f32 = COVER_Y + COVER - 14.0 - 28.0;
const STRIP_X: f32 = COVER_X + COVER + 30.0;
const THUMB_STEP: f32 = 60.0;

fn thumb_x(i: usize) -> f32 {
    STRIP_X + i as f32 * THUMB_STEP + 26.0
}

/// The play button drawn: a point of its disc clear of the icon, near the accent.
fn button_shown(app: &AppWindow) -> (bool, [u8; 3]) {
    // (The accent, or a little brighter under the pointer.)
    let px = pixel(app, BUTTON_CX, BUTTON_CY + 19.0).unwrap_or_default();
    (diff(px, accent(app)) < 90, px)
}

// A grid shelf's geometry (home.slint, lists.slint `CoverGrid`): its title band 48 px over the
// covers, which start at the page's top; a row is the cover, 50 px of text and a 24 px gap.
const GRID_TOP: f32 = 72.0 + 48.0;
const GAP: f32 = 24.0;

/// The covers drawn along one line of the page (logical y): how many of `cols` places hold one.
fn covers_on_line(app: &AppWindow, buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, y: f32, cols: usize) -> usize {
    let theme = app.global::<crate::Theme>();
    let (size, margin) = (theme.get_cover_size(), theme.get_page_margin());
    let (w, _) = logical(app);
    let step = (w - 2.0 * margin + GAP) / cols as f32;
    let scale = app.window().scale_factor();
    let Some(bg) = pixel_in(buf, scale, margin * 0.3, y) else { return 0 };
    (0..cols)
        .filter(|&k| {
            let x0 = margin + k as f32 * step;
            [0.25, 0.5, 0.75].iter().any(|f| pixel_in(buf, scale, x0 + size * f, y).is_some_and(|p| diff(p, bg) > 45))
        })
        .count()
}

pub fn run() {
    let c: C = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, |$st:ident, $app:ident, $c:ident| $body:block) => {{
            let $c = c.clone();
            steps.push(($ms, Box::new(move |$st: &Rc<State>, $app: &AppWindow| $body)));
        }};
    }

    // ---- the hero ----
    step!(300, |st, app, c| {
        c.borrow_mut().monitor = app.window().scale_factor();
        let s = app.window().size();
        line(&c, format!("window {}x{} physical, scale {}", s.width, s.height, app.window().scale_factor()));
        set_shelves(st, vec![ShelfConfig::new(ShelfQuery::RecentlyAdded, ShelfLayout::Hero, 12)]);
        let m = c.borrow().monitor;
        crate::platform::devwindow::resize_client(st.hwnd.get(), (1920.0 * m) as i32, (1080.0 * m) as i32);
        views::go(st, Page::Home);
    });
    step!(1500, |_st, app, c| {
        app.global::<Nav>().set_page_scroll(0.0);
        moved(app, 960.0, 20.0);
        let (albums, hero) = first_shelf(app);
        line(&c, format!("hero: {} albums, featuring #{hero}", albums.len()));
        check(&c, "the hero shelf has a strip of ten albums or more", albums.len() >= 10, albums.len());
    });
    step!(500, |_st, app, c| {
        snapshot(app, "hs-hero-away");
        let (shown, px) = button_shown(app);
        check(&c, "hero: no play button without the pointer", !shown, format!("{px:?}"));
        moved(app, COVER_X + 60.0, COVER_Y + 60.0);
    });
    step!(500, |_st, app, c| {
        snapshot(app, "hs-hero-hover");
        let (shown, px) = button_shown(app);
        check(&c, "hero: the play button shown with the pointer on the cover", shown, format!("{px:?}"));
        moved(app, 1500.0, COVER_Y + 120.0);
    });
    step!(500, |_st, app, c| {
        let (shown, px) = button_shown(app);
        check(&c, "hero: no play button with the pointer elsewhere on the card", !shown, format!("{px:?}"));
        moved(app, COVER_X + 60.0, COVER_Y + 60.0);
    });
    // Resting on the button, then across its left edge and back: drawn after each move.
    step!(500, |_st, app, c| {
        let mut seen = String::new();
        for i in 0..8 {
            moved(app, BUTTON_CX + (i % 2) as f32 * 0.25, BUTTON_CY);
            seen.push(if button_shown(app).0 { 'B' } else { '.' });
        }
        let edge = BUTTON_CX - 28.0;
        for i in 0..12 {
            // Outside the disc's left edge (on the cover), then inside it.
            let x = if i % 2 == 0 { edge - 2.0 - (i % 4) as f32 * 0.25 } else { edge + 3.0 + (i % 4) as f32 * 0.25 };
            moved(app, x, BUTTON_CY);
            seen.push(if button_shown(app).0 { 'B' } else { '.' });
        }
        check(&c, "hero: the play button drawn on every move on it and across its edge", seen.chars().all(|ch| ch == 'B'), &seen);
        moved(app, BUTTON_CX, BUTTON_CY);
    });
    step!(400, |st, app, _c| {
        snapshot(app, "hs-hero-button-hover");
        st.player.stop();
        click(app, BUTTON_CX, BUTTON_CY);
    });
    step!(700, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let want = albums.get(hero).copied();
        let playing = playing_album(st);
        check(&c, "hero: a click on the play button plays the featured album", want.is_some() && playing == want, format!("playing {playing:?}, featured {want:?}"));
        check(&c, "hero: the play button's click opens nothing", app.global::<Nav>().get_page() == Page::Home, format!("{:?}", app.global::<Nav>().get_page()));
        click(app, COVER_X + 30.0, COVER_Y + 30.0);
    });
    step!(800, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let page = app.global::<Nav>().get_page();
        let open = i64::from(app.global::<AlbumPage>().get_album().id);
        check(&c, "hero: a click elsewhere on the cover opens the album", page == Page::Album && albums.get(hero) == Some(&open), format!("{page:?}, album {open}, featured {:?}", albums.get(hero)));
        views::go(st, Page::Home);
    });
    // A tap on the button's place with no hover before it (a touch): the button is not shown
    // yet, so the cover's click (open), not Play.
    step!(1200, |st, app, c| {
        app.global::<Nav>().set_page_scroll(0.0);
        away(app);
        st.player.stop();
        // Another album featured than the one playing.
        let index = app.global::<Home>().get_shelves().row_data(0).map_or(0, |v| v.index);
        views::hero_select(index as usize, 1);
        line(&c, format!("tap: playing {:?} before", playing_album(st)));
    });
    step!(500, |_st, app, _c| {
        click(app, BUTTON_CX, BUTTON_CY);
    });
    step!(700, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let page = app.global::<Nav>().get_page();
        let open = i64::from(app.global::<AlbumPage>().get_album().id);
        let playing = playing_album(st);
        check(&c, "hero: a tap where the play button is not shown yet opens the album, plays nothing", hero == 1 && page == Page::Album && albums.get(hero) == Some(&open) && playing != albums.get(hero).copied(), format!("{page:?}, album {open}, featured #{hero}, playing {playing:?}"));
        let index = app.global::<Home>().get_shelves().row_data(0).map_or(0, |v| v.index);
        views::hero_select(index as usize, 0);
        views::go(st, Page::Home);
    });
    // The strip: found by single clicks up from the card's foot on the fifth cover (a click
    // there features it), then the featured album put back.
    step!(1200, |_st, app, c| {
        app.global::<Nav>().set_page_scroll(0.0);
        let mut y = 72.0 + 300.0 - 4.0;
        while y > COVER_Y + 120.0 {
            click(app, thumb_x(4), y);
            if first_shelf(app).1 == 4 {
                c.borrow_mut().strip_y = y - 20.0;
                break;
            }
            y -= 4.0;
        }
        let sy = c.borrow().strip_y;
        check(&c, "hero: the strip's covers found by a click", sy > 0.0, format!("its covers' middle near y {sy:.0}"));
        let index = app.global::<Home>().get_shelves().row_data(0).map_or(0, |v| v.index);
        views::hero_select(index as usize, 0);
        moved(app, 960.0, 20.0);
    });
    // A double click at once on the fourth cover: it plays.
    step!(600, |st, app, c| {
        st.player.stop();
        let y = c.borrow().strip_y;
        if y <= 0.0 {
            return;
        }
        click(app, thumb_x(3), y);
        click(app, thumb_x(3), y);
    });
    step!(800, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let playing = playing_album(st);
        check(&c, "hero strip: a double click plays the cover clicked", playing.is_some() && playing == albums.get(3).copied() && hero == 3, format!("playing {playing:?}, the cover's {:?}, featuring #{hero}", albums.get(3)));
        snapshot(app, "hs-hero-strip-double");
        let y = c.borrow().strip_y;
        if y > 0.0 {
            click(app, thumb_x(5), y);
        }
    });
    step!(800, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let playing = playing_album(st);
        check(&c, "hero strip: a single click features the cover and plays nothing new", hero == 5 && playing == albums.get(3).copied(), format!("featuring #{hero}, playing {playing:?}"));
        check(&c, "hero strip: a click stays on Home", app.global::<Nav>().get_page() == Page::Home, format!("{:?}", app.global::<Nav>().get_page()));
        let y = c.borrow().strip_y;
        if y > 0.0 {
            click(app, thumb_x(7), y);
        }
    });
    // A double click with 150 ms between its clicks (the first one has featured the cover).
    step!(150, |_st, app, c| {
        let y = c.borrow().strip_y;
        if y > 0.0 {
            click(app, thumb_x(7), y);
        }
    });
    step!(800, |st, app, c| {
        let (albums, hero) = first_shelf(app);
        let playing = playing_album(st);
        check(&c, "hero strip: a double click 150 ms apart plays the cover clicked", playing.is_some() && playing == albums.get(7).copied() && hero == 7, format!("playing {playing:?}, the cover's {:?}, featuring #{hero}", albums.get(7)));
        st.player.stop();
        moved(app, 960.0, 20.0);
    });

    // ---- the shelf editor's rows ----
    step!(300, |st, _app, _c| {
        set_shelves(st, vec![grid_shelf(ShelfQuery::RecentlyAdded, 2), ShelfConfig::new(ShelfQuery::Random, ShelfLayout::Row, 20)]);
        views::go(st, Page::Shelves);
    });
    step!(1200, |_st, app, c| {
        app.global::<Nav>().set_page_scroll(0.0);
        moved(app, 960.0, 20.0);
        snapshot(app, "hs-editor-start");
        let Ok(buf) = app.window().take_snapshot() else { return };
        let scale = app.window().scale_factor();
        let (w, h) = logical(app);
        // The rows' right padding (their surface colour), down the page: the first run of
        // about 60 px is the first shelf's row.
        let x = w - 40.0 - 4.0;
        let surface = app.global::<crate::Theme>().get_surface();
        let surface = [surface.red(), surface.green(), surface.blue()];
        let mut run_from = None;
        let mut y = 60.0;
        while y < h - 100.0 {
            let on = pixel_in(&buf, scale, x, y).is_some_and(|p| diff(p, surface) <= 6);
            match (on, run_from) {
                (true, None) => run_from = Some(y),
                (false, Some(top)) => {
                    if (y - top - 60.0).abs() <= 3.0 {
                        c.borrow_mut().editor_top = top;
                        break;
                    }
                    run_from = None;
                }
                _ => {}
            }
            y += 1.0;
        }
        let top = c.borrow().editor_top;
        check(&c, "editor: the first shelf's row found", top > 0.0, format!("at y {top:.0}"));
    });
    // The stepper: from 2, More five times (to 6 and held), then Fewer six times (to 1 and held).
    let presses: Vec<(bool, usize)> = vec![(true, 3), (true, 4), (true, 5), (true, 6), (true, 6), (false, 5), (false, 4), (false, 3), (false, 2), (false, 1), (false, 1)];
    for (k, (more, want)) in presses.into_iter().enumerate() {
        step!(150, |_st, app, c| {
            let top = c.borrow().editor_top;
            if top <= 0.0 {
                return;
            }
            let (w, _) = logical(app);
            // From the row's right: 10 px of padding, Remove, Show, then More, the count (56 px)
            // and Fewer, 36 px buttons 8 px apart.
            let x = if more { w - 40.0 - 10.0 - 2.0 * 44.0 - 18.0 } else { w - 40.0 - 10.0 - 3.0 * 44.0 - 64.0 - 18.0 };
            click(app, x, top + 30.0);
        });
        step!(400, |st, app, c| {
            if c.borrow().editor_top <= 0.0 {
                return;
            }
            let saved = st.settings.borrow().shelves.first().map(|s| s.rows);
            let shown = app.global::<ShelfEditor>().get_rows().row_data(0).map(|r| r.rows);
            check(&c, &format!("editor: {} rows → {want}", if more { "More" } else { "Fewer" }), saved == Some(want) && shown == Some(want as i32), format!("settings {saved:?}, editor {shown:?}"));
            if k == 3 {
                snapshot(app, "hs-editor-6");
            }
            if k == 10 {
                snapshot(app, "hs-editor-1");
                let row = st.settings.borrow().shelves.get(1).map(|s| s.limit);
                check(&c, "editor: the row shelf's albums untouched", row == Some(20), format!("{row:?}"));
            }
        });
    }

    // ---- grid shelves ----
    // (name, logical window size at 100%, interface size, rows, the shelf).
    type Config = (&'static str, (f32, f32), u32, usize, ShelfQuery);
    let configs: Vec<Config> = vec![
        ("w1920-z100-r2", (1920.0, 1080.0), 100, 2, ShelfQuery::RecentlyAdded),
        ("w1400-z100-r2", (1400.0, 900.0), 100, 2, ShelfQuery::RecentlyAdded),
        ("w1100-z100-r2", (1100.0, 700.0), 100, 2, ShelfQuery::RecentlyAdded),
        ("w960-z100-r2", (960.0, 620.0), 100, 2, ShelfQuery::RecentlyAdded),
        ("w1920-z70-r2", (1920.0, 1080.0), 70, 2, ShelfQuery::RecentlyAdded),
        ("w1920-z150-r2", (1920.0, 1080.0), 150, 2, ShelfQuery::RecentlyAdded),
        ("w1920-z100-r1", (1920.0, 1080.0), 100, 1, ShelfQuery::RecentlyAdded),
        ("w1920-z100-r6", (1920.0, 1080.0), 100, 6, ShelfQuery::RecentlyAdded),
        ("w1920-z70-r6", (1920.0, 1080.0), 70, 6, ShelfQuery::RecentlyAdded),
        ("w1400-z150-r6", (1400.0, 1000.0), 150, 6, ShelfQuery::RecentlyAdded),
        ("w1920-z100-few", (1920.0, 1080.0), 100, 2, ShelfQuery::Label { name: "Warp".into() }),
    ];
    for (name, (w, h), zoom, rows, query) in configs {
        step!(200, |st, _app, c| {
            // A new shelf only when it changes: otherwise the same shelf follows the window.
            let want = grid_shelf(query, rows);
            if st.settings.borrow().shelves != [want.clone()] {
                set_shelves(st, vec![want]);
            }
            views::go(st, Page::Home);
            crate::zoom::set(st, zoom, false);
            let m = c.borrow().monitor;
            crate::platform::devwindow::resize_client(st.hwnd.get(), (w * m) as i32, (h * m) as i32);
        });
        step!(1500, |_st, app, c| {
            app.global::<Nav>().set_page_scroll(0.0);
            away(app);
            let theme = app.global::<crate::Theme>();
            let (lw, lh) = logical(app);
            let (size, margin) = (theme.get_cover_size(), theme.get_page_margin());
            let cols = ((lw - 2.0 * margin + GAP) / (size + GAP)).floor().max(1.0) as usize;
            let albums = first_shelf(app).0.len();
            let shown = if albums <= cols { albums } else { rows.min(albums / cols) * cols };
            let drawn_rows = shown.div_ceil(cols);
            // The shelf's model holds what it shows and no more (no tile built to be hidden):
            // its rows of covers, or the two albums of "Warp".
            let want = if name.ends_with("few") { 2 } else { rows * cols };
            check(&c, &format!("grid {name}: the shelf holds exactly the albums it shows"), albums == want, format!("{albums} albums, {want} wanted ({rows} rows of {cols})"));
            {
                let mut cc = c.borrow_mut();
                (cc.cols, cc.shown, cc.rows, cc.albums) = (cols, shown, drawn_rows, albums);
            }
            line(&c, format!("grid {name}: window {lw:.0}x{lh:.0} logical (scale {:.2}), {cols} columns, {albums} albums, {shown} shown in {drawn_rows} rows", app.window().scale_factor()));
        });
        step!(300, |_st, app, c| {
            let Some(_) = snapshot(app, &format!("hs-grid-{name}-top")) else { return };
            let Ok(buf) = app.window().take_snapshot() else { return };
            let (cols, shown) = (c.borrow().cols, c.borrow().shown);
            let size = app.global::<crate::Theme>().get_cover_size();
            let n = covers_on_line(app, &buf, GRID_TOP + size / 2.0, cols);
            check(&c, &format!("grid {name}: the first row drawn with {}", cols.min(shown)), n == cols.min(shown), format!("{n} of {cols} places hold a cover"));
            // To the page's foot by the wheel: the furthest it scrolls says how tall the grid is.
            let (lw, lh) = logical(app);
            let p = LogicalPosition::new(lw * 0.5, lh * 0.5);
            app.window().dispatch_event(WindowEvent::PointerMoved { position: p });
            app.window().dispatch_event(WindowEvent::PointerScrolled { position: p, delta_x: 0.0, delta_y: -100_000.0 });
            away(app);
        });
        step!(900, |_st, app, c| {
            let theme = app.global::<crate::Theme>();
            let (_, lh) = logical(app);
            let size = theme.get_cover_size();
            let row_h = size + 50.0 + GAP;
            let (cols, shown, rows) = (c.borrow().cols, c.borrow().shown, c.borrow().rows);
            let content = GRID_TOP + rows as f32 * row_h + theme.get_bar_height() + 32.0;
            let want = (content - lh).max(0.0);
            let got = -app.global::<Nav>().get_page_scroll();
            check(&c, &format!("grid {name}: the page as tall as {rows} rows"), (got - want).abs() <= 2.0, format!("scrolls {got:.1}, {want:.1} for {rows} rows"));
            snapshot(app, &format!("hs-grid-{name}-bottom"));
            let Ok(buf) = app.window().take_snapshot() else { return };
            let last = GRID_TOP + (rows.max(1) - 1) as f32 * row_h + size / 2.0 - got;
            let in_last = shown - (rows.max(1) - 1) * cols;
            let n = covers_on_line(app, &buf, last, cols);
            check(&c, &format!("grid {name}: the last row drawn with {in_last}"), n == in_last && (rows <= 1 || in_last == cols), format!("{n} of {cols} places hold a cover"));
        });
    }
    // The keys in a grid of two rows: End, then Down and Right beyond it, stay on the last
    // album shown (Enter opens it), never on one hidden. The grid is found by Tab from the
    // page's start (each try from Home afresh, Esc after).
    step!(200, |st, _app, c| {
        set_shelves(st, vec![grid_shelf(ShelfQuery::RecentlyAdded, 2)]);
        crate::zoom::set(st, 100, false);
        let m = c.borrow().monitor;
        crate::platform::devwindow::resize_client(st.hwnd.get(), (1920.0 * m) as i32, (1080.0 * m) as i32);
        views::go(st, Page::Home);
    });
    for tabs in 1..=8usize {
        step!(1000, |st, app, c| {
            if c.borrow().tabs.is_some() {
                return;
            }
            views::go(st, Page::Home);
            app.global::<Nav>().set_page_scroll(0.0);
            crate::bench::tabs(app, tabs);
            press(app, Key::End, &[]);
            for _ in 0..3 {
                press(app, Key::DownArrow, &[]);
                press(app, Key::RightArrow, &[]);
            }
            press(app, Key::Return, &[]);
        });
        step!(600, |st, app, c| {
            if c.borrow().tabs.is_some() {
                return;
            }
            let page = app.global::<Nav>().get_page();
            if page == Page::Album {
                c.borrow_mut().tabs = Some(tabs);
                views::go(st, Page::Home);
                return;
            }
            line(&c, format!("  {tabs} tabs: {page:?}"));
            press(app, Key::Escape, &[]);
            if page != Page::Home {
                views::go(st, Page::Home);
            }
        });
    }
    step!(1200, |st, app, c| {
        let Some(tabs) = c.borrow().tabs else {
            check(&c, "grid keys: the grid reached by Tab", false, "no album opened by End and Enter after 1 to 8 tabs");
            return;
        };
        line(&c, format!("grid keys: the grid is {tabs} tabs from the page's start"));
        let theme = app.global::<crate::Theme>();
        let (lw, _) = logical(app);
        let cols = ((lw - 2.0 * theme.get_page_margin() + GAP) / (theme.get_cover_size() + GAP)).floor().max(1.0) as usize;
        let albums = first_shelf(app).0;
        let shown = 2.min(albums.len() / cols) * cols;
        c.borrow_mut().shown = shown;
        let _ = st;
        app.global::<Nav>().set_page_scroll(0.0);
        crate::bench::tabs(app, tabs);
        press(app, Key::End, &[]);
        for _ in 0..3 {
            press(app, Key::DownArrow, &[]);
            press(app, Key::RightArrow, &[]);
        }
        snapshot(app, "hs-grid-keys-end");
        press(app, Key::Return, &[]);
    });
    step!(800, |st, app, c| {
        if c.borrow().tabs.is_none() {
            return;
        }
        let shown = c.borrow().shown;
        let open = i64::from(app.global::<AlbumPage>().get_album().id);
        views::go(st, Page::Home);
        let albums = first_shelf(app).0;
        let at = albums.iter().position(|&a| a == open);
        check(&c, "grid keys: End and the arrows stop at the last album shown", shown > 0 && at == Some(shown - 1), format!("opened #{at:?} of {} albums, {shown} shown", albums.len()));
    });
    step!(300, |st, _app, c| {
        let _ = st;
        write_report("home-shelves", &c.borrow().r);
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
