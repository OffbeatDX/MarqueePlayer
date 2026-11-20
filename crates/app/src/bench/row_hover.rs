//! `MP_BENCH=row-hover`: a track row's hover while the pointer rests on its play button (the
//! number's cell on an album page, where the button shows while the row is hovered). The
//! pointer moves go through the window's own dispatch (`Window::dispatch_event`: no OS input),
//! a fraction of a pixel back and forth on the button as a hand on a mouse moves it, and after
//! each the row is drawn (`take_snapshot`) to see whether it is drawn hovered. The row must
//! stay hovered, the button must stay, and a click there must play the row's track (not choose
//! the row). The failure it guards against: every move flipping the row's hover (the button,
//! once shown, taking the pointer from the row, which then hid it), so that the row blinks and
//! a click lands on the button or the row by turns. The runner runs it at scale 1, 1.25 and 1.5
//! (`SLINT_SCALE_FACTOR`) and at the interface sizes 70% and 200% (`MP_BENCH_ZOOM`), on X11
//! and Wayland alike. Last, where the window is frameless (Linux), the window is sized down
//! (not maximized) and its edge line must be drawn on the left, right and bottom edges.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

use crate::bench::{Step, run_steps, snapshot, verdict, write_report};
use crate::state::State;
use crate::{AppWindow, MenuStore, views};

#[derive(Default)]
struct Ctx {
    r: String,
    /// The list's left edge and its first row's top (logical px), and the album's tracks.
    left: f32,
    top: f32,
    tracks: Vec<i64>,
    /// The rows tested: from `first`, the first wholly on the page, `rows` of them wholly
    /// above the player bar (at most six).
    first: usize,
    rows: usize,
}

type C = Rc<RefCell<Ctx>>;
const PITCH: f32 = 52.0;
/// Moves the pointer rests through on each row (a quarter pixel apart).
const MOVES: usize = 8;

fn line(c: &C, text: String) {
    log::info!("row-hover bench: {text}");
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

fn right_click(app: &AppWindow, x: f32, y: f32) {
    let w = app.window();
    let p = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position: p });
    w.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Right });
    w.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Right });
}

/// A right click opens a menu for one row of the album's list at this point.
fn row_at(st: &Rc<State>, app: &AppWindow, x: f32, y: f32) -> Option<usize> {
    right_click(app, x, y);
    let open = app.global::<MenuStore>().get_open();
    let rows = crate::selected::rows(st, "album");
    if open {
        crate::menus::key(st, "escape");
    }
    crate::selected::clear(st, "album");
    (open && rows.len() == 1).then(|| rows[0])
}

/// The row's background as drawn, a few pixels below its top edge, at `x` (logical): its
/// colour says whether it is drawn hovered (no text there).
fn row_pixel(app: &AppWindow, x: f32, top: f32) -> Option<[u8; 4]> {
    let buf = app.window().take_snapshot().ok()?;
    let s = app.window().scale_factor();
    let (px, py) = ((x * s) as u32, ((top + 3.0) * s) as u32);
    (px < buf.width() && py < buf.height()).then(|| {
        let p = buf.as_slice()[(py * buf.width() + px) as usize];
        [p.r, p.g, p.b, p.a]
    })
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

    // `MP_BENCH_ZOOM`: the interface size to run at (row-hover-z70, row-hover-z200).
    step!(100, |st, _app, c| {
        if let Some(pct) = std::env::var("MP_BENCH_ZOOM").ok().and_then(|v| v.parse::<u32>().ok()) {
            crate::zoom::set(st, pct, false);
            line(&c, format!("interface size {pct}%"));
        }
    });
    step!(600, |st, app, c| {
        let size = app.window().size();
        line(&c, format!("window {}x{} physical, scale {}", size.width, size.height, app.window().scale_factor()));
        // An album with eight tracks or more, its page (the number's cell, no covers).
        let album: Option<i64> = st.db.query_row("SELECT album_id FROM tracks GROUP BY album_id HAVING COUNT(*) >= 8 ORDER BY album_id LIMIT 1", [], |r| r.get(0)).ok();
        match album {
            Some(a) => {
                c.borrow_mut().tracks = library::query::album_tracks(&st.db, a).unwrap_or_default().iter().map(|t| t.id).collect();
                views::open_album(st, a);
            }
            None => check(&c, "an album of eight tracks", false, "none in the library"),
        }
    });
    // A short window (200%: 524 logical pixels on a 1080p screen) shows the album's header
    // and no row: the page scrolled down (a wheel through the window's dispatch).
    step!(800, |_st, app, c| {
        let s = app.window().size().to_logical(app.window().scale_factor());
        if s.height < 800.0 {
            let p = LogicalPosition::new(s.width * 0.5, s.height * 0.5);
            app.window().dispatch_event(WindowEvent::PointerMoved { position: p });
            app.window().dispatch_event(WindowEvent::PointerScrolled { position: p, delta_x: 0.0, delta_y: -300.0 });
            line(&c, format!("a short window ({:.0} logical pixels): the page scrolled down", s.height));
        }
    });
    step!(800, |st, app, c| {
        let s = app.window().size().to_logical(app.window().scale_factor());
        // The rows: down the middle of the page, then along the first row to its left edge.
        let mut found = None;
        let mut y = 60.0;
        while y < s.height - 100.0 {
            if let Some(r) = row_at(st, app, s.width * 0.5, y) {
                found = Some((r, y));
                break;
            }
            y += 3.0;
        }
        let Some((r, y)) = found else {
            check(&c, "the album's rows are found", false, "no row answered a right click");
            return;
        };
        let top = y - r as f32 * PITCH;
        // The first row wholly below the top strip, and how many wholly above the player bar.
        let first = if top >= 60.0 { 0 } else { ((60.0 - top) / PITCH).ceil() as usize };
        let end = ((s.height - 110.0 - top) / PITCH).floor().max(0.0) as usize;
        let rows = end.saturating_sub(first).min(6).min(c.borrow().tracks.len().saturating_sub(first));
        let first_top = top + first as f32 * PITCH;
        // The row's left edge, by its hover alone (no click: one on the number's cell would play,
        // one beside it could open the sidebar): the pointer comes from outside to a point of
        // row 0's line, and the row is drawn lit or not; halved down to a pixel.
        // (Its top margin: above the title and the artist, whose links take the pointer.)
        let row0 = first_top + 5.0;
        let away = (s.width * 0.5, 20.0);
        moved(app, away.0, away.1);
        let unlit = row_pixel(app, s.width * 0.5 + 100.0, first_top);
        let lit_from = |x: f32| {
            moved(app, away.0, away.1);
            moved(app, x, row0);
            let lit = row_pixel(app, s.width * 0.5 + 100.0, first_top);
            lit.is_some() && lit != unlit
        };
        let (mut lo, mut hi) = (16.0f32, s.width * 0.5);
        if !lit_from(hi) {
            check(&c, "the first row is drawn lit under the pointer", false, "not at the middle of the page");
        }
        while hi - lo > 1.0 {
            let mid = (lo + hi) / 2.0;
            if lit_from(mid) { hi = mid } else { lo = mid }
        }
        let x = hi.ceil();
        c.borrow_mut().left = x;
        c.borrow_mut().top = top;
        c.borrow_mut().first = first;
        c.borrow_mut().rows = rows;
        if rows < 3 {
            check(&c, "three rows or more on the page", false, format!("{rows} from row {first}"));
        }
        line(&c, format!("rows from y {top:.0} (row {first} the first wholly shown, {rows} tested), the list's left edge at x {x:.0}"));
        moved(app, s.width * 0.5, 20.0);
    });
    // Each row: the pointer away, the row as drawn so; then the pointer resting on its play
    // button, the row drawn after each move; then a click there, its effect seen a moment later.
    for k in 0..6usize {
        step!(300, |_st, app, c| {
            let (left, top, tracks, first, rows) = { let c = c.borrow(); (c.left, c.top, c.tracks.len(), c.first, c.rows) };
            if tracks < 8 || left <= 0.0 || k >= rows {
                return;
            }
            let row = first + k;
            let s = app.window().size().to_logical(app.window().scale_factor());
            // The number's cell: 12 px in, 28 px wide; the play button's icon at its right.
            let bx = left + 12.0 + 20.0;
            let (probe_x, row_top, y) = (left + 300.0, top + row as f32 * PITCH, top + row as f32 * PITCH + 26.0);
            moved(app, s.width * 0.5, 20.0);
            let away = row_pixel(app, probe_x, row_top);
            let mut hovered = Vec::new();
            for i in 0..MOVES {
                moved(app, bx + (i % 2) as f32 * 0.25, y);
                let px = row_pixel(app, probe_x, row_top);
                hovered.push(px.is_some() && px != away);
            }
            let flips = hovered.windows(2).filter(|w| w[0] != w[1]).count();
            let drawn: String = hovered.iter().map(|&h| if h { 'H' } else { '.' }).collect();
            check(&c, &format!("row {row}: drawn hovered on every move while the pointer rests on its play button"), hovered.iter().all(|&h| h), format!("{drawn} ({flips} changes in {MOVES} moves)"));
            // The click where the pointer rests: the button's (plays the row), not the row's.
            let p = LogicalPosition::new(bx + 0.25 * (MOVES % 2) as f32, y);
            app.window().dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
            app.window().dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
            if k == 0 {
                snapshot(app, "row-hover");
            }
        });
        step!(500, |st, _app, c| {
            let (first, rows) = { let c = c.borrow(); (c.first, c.rows) };
            if k >= rows {
                return;
            }
            let row = first + k;
            let want = c.borrow().tracks.get(row).copied();
            let playing = st.queue.borrow().current().map(|i| i.track.id);
            let chosen = crate::selected::rows(st, "album");
            check(&c, &format!("row {row}: the click on its play button plays it"), want.is_some() && playing == want, format!("playing {playing:?}, the row's {want:?}; rows chosen {chosen:?}"));
            crate::selected::clear(st, "album");
        });
    }
    // Frameless (Linux): the hairline along the window's edges while it is not maximized, at
    // this scale too (0.7 px at 70%, 2 px at 200%).
    step!(300, |st, app, _c| {
        st.player.stop();
        if app.global::<crate::Chrome>().get_frameless() {
            let s = app.window().size();
            crate::platform::devwindow::resize_client(st.hwnd.get(), (s.width * 4 / 5) as i32, (s.height * 4 / 5) as i32);
        }
    });
    step!(900, |_st, app, c| {
        if !app.global::<crate::Chrome>().get_frameless() {
            return;
        }
        let maximized = app.global::<crate::Chrome>().get_maximized();
        let Some(img) = snapshot(app, "row-hover-edge") else { return };
        let (w, h) = (img.width(), img.height());
        let px = |x: u32, y: u32| img.get_pixel(x.min(w - 1), y.min(h - 1)).0;
        let inset = (6.0 * app.window().scale_factor()).ceil() as u32;
        // Down the left and right edges at mid height, along the bottom edge at mid width
        // (inside the player bar), each against the pixel a few in.
        let sides = [((0, h / 2), (inset, h / 2)), ((w - 1, h / 2), (w - 1 - inset, h / 2)), ((w / 2, h - 1), (w / 2, h - 1 - inset))];
        let drawn = sides.iter().filter(|(e, i)| px(e.0, e.1) != px(i.0, i.1)).count();
        check(&c, "frameless: the window's edge is drawn while not maximized", !maximized && drawn == 3, format!("maximized {maximized}; {drawn} of 3 edges differ from the pixel {inset} px in ({w}x{h})"));
    });
    step!(300, |_st, _app, c| {
        write_report("row-hover", &c.borrow().r);
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
