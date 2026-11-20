//! `MP_BENCH=cleanup`: the auto-hiding player bar, and with it the queue, sorting, the
//! pill and Settings > Keys, each part snapshotted and reported (`bench-cleanup.txt`):
//!
//! - the auto-hiding player bar, driven by pointer events through Slint's own dispatch
//!   (`Window::dispatch_event`; no OS input, the real pointer never moves): hidden and
//!   shown, the pointer on the bar's own buttons, a slider drag, the keyboard focus in
//!   the bar, a menu open over it, the pointer leaving the window, and clicks just above the
//!   bar and on the bar's empty parts (do they reach the page beneath?);
//! - removing the current queue item while paused;
//! - the order of artists, genres and labels;
//! - the navigation pill with a long page title;
//! - Settings > Keys, the shortcut editor: capture, a conflict and its reassign, clear,
//!   reset, a refused key, the new binding acting, Esc cancelling, and the bar's labels.
//!
//! Needs a music folder with at least two albums of three tracks or more (`MP_MUSIC`).

use std::cell::RefCell;
use std::rc::Rc;

use player_core::PlayState;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

use slint::Model;
use slint::platform::Key;

use crate::bench::{Step, press, run_steps, snapshot, type_text, write_report};
use crate::state::State;
use crate::{AppWindow, Grid, Nav, Page, playback, views};

type Report = Rc<RefCell<String>>;
/// A step of a run: what it does, given the report.
type StepFn = Box<dyn Fn(&Rc<State>, &AppWindow, &Report)>;

fn line(r: &Report, text: String) {
    log::info!("cleanup: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

/// The window's size in logical pixels.
fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

fn move_to(app: &AppWindow, x: f32, y: f32) {
    app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
}

/// Click each point in turn as a hand would (move, press, a moment, release), 200 ms
/// apart, and count the clicks that opened an album (going back after each).
fn clicks(app: &AppWindow, points: Vec<(f32, f32)>, done: impl FnOnce((usize, usize)) + 'static) {
    let weak = app.as_weak();
    let n = points.len();
    let mut points = points.into_iter();
    let mut pending: Option<(f32, f32)> = None;
    let mut hits = 0;
    let mut done = Some(done);
    let timer = Rc::new(slint::Timer::default());
    let t = timer.clone();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(250), move || {
        let Some(app) = weak.upgrade() else { return };
        let w = app.window();
        if let Some((x, y)) = pending.take() {
            w.dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
            return;
        }
        if app.global::<Nav>().get_page() == Page::Album {
            hits += 1;
            crate::state::with(views::back);
            return;
        }
        match points.next() {
            Some((x, y)) => {
                let position = LogicalPosition::new(x, y);
                w.dispatch_event(WindowEvent::PointerMoved { position });
                w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                pending = Some((x, y));
            }
            None => {
                t.stop();
                if let Some(d) = done.take() {
                    d((hits, n));
                }
            }
        }
    });
    std::mem::forget(timer);
}

/// As `clicks`, reporting each point's outcome.
fn sweep(app: &AppWindow, points: Vec<(f32, f32)>, done: impl FnOnce(Vec<bool>) + 'static) {
    let results: Rc<RefCell<Vec<bool>>> = Rc::default();
    let weak = app.as_weak();
    let mut points = points.into_iter();
    let mut phase = 0;
    let mut cur = (0.0, 0.0);
    let mut done = Some(done);
    let timer = Rc::new(slint::Timer::default());
    let t = timer.clone();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(250), move || {
        let Some(app) = weak.upgrade() else { return };
        let w = app.window();
        match phase {
            0 => match points.next() {
                Some(p) => {
                    cur = p;
                    let position = LogicalPosition::new(p.0, p.1);
                    w.dispatch_event(WindowEvent::PointerMoved { position });
                    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                    phase = 1;
                }
                None => {
                    t.stop();
                    if let Some(d) = done.take() {
                        d(results.take());
                    }
                }
            },
            1 => {
                w.dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(cur.0, cur.1), button: PointerEventButton::Left });
                phase = 2;
            }
            _ => {
                let hit = app.global::<Nav>().get_page() == Page::Album;
                results.borrow_mut().push(hit);
                if hit {
                    crate::state::with(views::back);
                }
                phase = 0;
            }
        }
    });
    std::mem::forget(timer);
}

fn bar(app: &AppWindow) -> bool {
    app.get_bar_visible()
}

fn state(st: &State) -> String {
    let s = st.player.status();
    let what = match s.state {
        PlayState::Stopped => "stopped",
        _ if st.player.is_paused() => "paused",
        _ => "playing",
    };
    let cur = st.queue.borrow().current().map(|i| i.track.title.clone()).unwrap_or_default();
    format!("{what} on {cur:?} at {:.1} s", s.position)
}

fn album_tracks(st: &State, n: usize) -> Vec<library::Track> {
    let albums = library::query::all_albums(&st.db, library::query::AlbumSort::Artist, false).unwrap_or_default();
    albums.get(n).map(|a| library::query::album_tracks(&st.db, a.id).unwrap_or_default()).unwrap_or_default()
}

/// Samples of whether the bar is up, every `every` ms for `n` samples, moving the pointer
/// by a pixel each time (a hand never holds perfectly still) around (x, y).
fn sample_bar(app: &AppWindow, x: f32, y: f32, n: usize, every: u64, done: impl FnOnce(Vec<bool>) + 'static) {
    let weak = app.as_weak();
    let samples: Rc<RefCell<Vec<bool>>> = Rc::default();
    let timer = Rc::new(slint::Timer::default());
    let t = timer.clone();
    let mut done = Some(done);
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(every), move || {
        let Some(app) = weak.upgrade() else { return };
        let i = samples.borrow().len();
        move_to(&app, x + (i % 2) as f32, y);
        samples.borrow_mut().push(bar(&app));
        if samples.borrow().len() >= n {
            t.stop();
            if let Some(d) = done.take() {
                d(samples.take());
            }
        }
    });
    std::mem::forget(timer);
}

fn theme_bar(app: &AppWindow) -> f32 {
    app.global::<crate::Theme>().get_bar_height()
}

fn summary(s: &[bool]) -> String {
    let flips = s.windows(2).filter(|w| w[0] != w[1]).count();
    let up = s.iter().filter(|b| **b).count();
    format!("up in {up} of {} samples, {flips} changes: {}", s.len(), s.iter().map(|b| if *b { '#' } else { '.' }).collect::<String>())
}

pub fn run() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    let mut step = |ms: u64, f: StepFn| {
        let r = r.clone();
        steps.push((ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
    };

    // ---- Auto-hide player bar ------------------------------------------------------
    step(500, Box::new(|st, app, r| {
        let (w, h) = size(app);
        line(r, format!("window {w}x{h} logical, scale {}", app.window().scale_factor()));
        line(r, format!("library: {:?} (albums, tracks)", library::query::counts(&st.db)));
        st.settings.borrow_mut().bar_autohide = true;
        views::refresh_player_settings(st);
        views::go(st, Page::Albums);
        playback::play_tracks(st, album_tracks(st, 0), 0);
        move_to(app, w / 2.0, h / 2.0);
    }));
    step(1500, Box::new(move |st, app, r| {
        line(r, format!("[bar] auto-hide on, {}; pointer mid-page: bar up {}", state(st), bar(app)));
        snapshot(app, "c-01-bar-hidden");
        // The bar's own background, above its controls (no control there).
        let (w, h) = size(app);
        move_to(app, w / 2.0 - 160.0, h - theme_bar(app) + 6.0);
    }));
    step(800, Box::new(|_st, app, r| {
        line(r, format!("[bar] pointer on the bar's empty top strip: bar up {}", bar(app)));
        snapshot(app, "c-02-bar-shown");
    }));
    step(100, Box::new(move |_st, app, r| {
        // Onto the play button (centre of the transport row).
        let (w, h) = size(app);
        let y = h - theme_bar(app) + 34.0;
        move_to(app, w / 2.0, y);
        let r = r.clone();
        sample_bar(app, w / 2.0, y, 30, 40, move |s| line(&r, format!("[bar] pointer held on the play button for 1.2 s: {}", summary(&s))));
    }));
    step(1500, Box::new(move |_st, app, r| {
        snapshot(app, "c-03-pointer-on-play");
        let (w, h) = size(app);
        // The favourite button, the now playing block's last.
        let y = h - theme_bar(app) / 2.0;
        let x = 18.0 + w * 0.3 - 32.0 - 16.0;
        let r = r.clone();
        sample_bar(app, x, y, 20, 40, move |s| line(&r, format!("[bar] pointer held on the favourite button: {}", summary(&s))));
    }));
    step(1200, Box::new(move |_st, app, r| {
        // Drag the seek slider: press on it, move up out of the bar, release.
        let (w, h) = size(app);
        let x = w / 2.0;
        let y = h - theme_bar(app) + 14.0 + 40.0 + 4.0 + 8.0;
        move_to(app, x, y);
        app.window().dispatch_event(WindowEvent::PointerPressed { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
        let mut seen = Vec::new();
        for i in 1..=10 {
            move_to(app, x + i as f32 * 8.0, y - i as f32 * 30.0);
            seen.push(bar(app));
        }
        line(r, format!("[bar] during a seek drag up out of the bar (immediately): {}", summary(&seen)));
    }));
    step(900, Box::new(move |_st, app, r| {
        let (w, h) = size(app);
        line(r, format!("[bar] still dragging, pointer 300 px above the bar, 0.9 s later: bar up {}", bar(app)));
        let (x, y) = (w / 2.0 + 80.0, h - theme_bar(app) - 300.0 + 66.0);
        app.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
        move_to(app, w / 2.0, h / 2.0);
    }));
    step(1200, Box::new(|_st, app, r| {
        line(r, format!("[bar] pointer mid-page again: bar up {}", bar(app)));
        // Tab into the bar.
        let mut n = 0;
        while app.global::<crate::Focus>().get_zone() != "bar" && n < 120 {
            press(app, slint::platform::Key::Tab, &[]);
            n += 1;
        }
        line(r, format!("[bar] {n} Tabs: focus zone {:?}", app.global::<crate::Focus>().get_zone().to_string()));
    }));
    step(1200, Box::new(|_st, app, r| {
        line(r, format!("[bar] keyboard focus in the bar, pointer mid-page, 1.2 s later: bar up {}", bar(app)));
        snapshot(app, "c-04-focus-in-bar");
        app.invoke_focus_shortcuts();
        let (w, h) = size(app);
        move_to(app, w / 2.0 - 160.0, h - theme_bar(app) + 6.0);
    }));
    step(800, Box::new(move |st, app, r| {
        line(r, format!("[bar] focus back on the page, pointer on the bar: bar up {}", bar(app)));
        let (w, h) = size(app);
        // A menu over the bar (the output devices'); move the pointer up onto it.
        let x = 18.0 + w * 0.3 - 32.0 - 16.0;
        let _ = st;
        crate::output::open_menu(x, h - theme_bar(app) - 120.0);
        move_to(app, x + 20.0, h - theme_bar(app) - 100.0);
    }));
    step(1200, Box::new(|_st, app, r| {
        line(r, format!("[bar] menu open {}, pointer on it, 1.2 s later: bar up {}", app.global::<crate::MenuStore>().get_open(), bar(app)));
        snapshot(app, "c-05-menu-over-bar");
        app.global::<crate::MenuStore>().set_open(false);
        let (w, h) = size(app);
        move_to(app, w / 2.0 - 160.0, h - theme_bar(app) + 6.0);
    }));
    step(800, Box::new(|_st, app, r| {
        line(r, format!("[bar] pointer on the bar: bar up {}", bar(app)));
        app.window().dispatch_event(WindowEvent::PointerExited);
        let weak = app.as_weak();
        let r = r.clone();
        let marks: Rc<RefCell<Vec<(u64, bool)>>> = Rc::default();
        for ms in [30u64, 120, 200, 400, 800] {
            let (weak, marks, r) = (weak.clone(), marks.clone(), r.clone());
            slint::Timer::single_shot(std::time::Duration::from_millis(ms), move || {
                if let Some(app) = weak.upgrade() {
                    marks.borrow_mut().push((ms, bar(&app)));
                    if ms == 800 {
                        line(&r, format!("[bar] the pointer left the window (through the bottom): bar up after {:?}", marks.borrow()));
                    }
                }
            });
        }
    }));
    step(1200, Box::new(|_st, app, r| {
        // The bar hidden: the pointer just reaches the bottom, as on a maximized window.
        let (w, h) = size(app);
        move_to(app, w / 2.0, h / 2.0);
        move_to(app, w / 2.0 + 200.0, h - 1.0);
        line(r, format!("[bar] pointer on the window's last pixel row: bar up {} (right away)", bar(app)));
    }));
    step(800, Box::new(|_st, app, r| {
        line(r, format!("[bar] ... 0.8 s later: bar up {}", bar(app)));
        // Room: the queue pinned, the bar hidden.
        app.global::<Nav>().set_queue_pinned(true);
        let (w, h) = size(app);
        move_to(app, w / 3.0, h / 2.0);
    }));
    step(1500, Box::new(|_st, app, r| {
        line(r, format!("[bar] queue pinned, pointer mid-page: bar up {}", bar(app)));
        snapshot(app, "c-06-bar-hidden-queue-pinned");
        let (w, h) = size(app);
        move_to(app, w / 3.0, h - 30.0);
    }));
    step(1000, Box::new(|_st, app, r| {
        line(r, format!("[bar] queue pinned, pointer at the bottom: bar up {}", bar(app)));
        snapshot(app, "c-07-bar-shown-queue-pinned");
        app.global::<Nav>().set_queue_pinned(false);
    }));
    // Clicks just above the bar, and on its empty parts: do they reach the album grid?
    // First a click on a cover mid-page, to show a click opens an album at all.
    step(800, Box::new(|st, app, r| {
        playback::set_paused(st, true);
        // Pinned, the queue narrows the grid to 7 columns: the 24 fixture albums then
        // fill 4 rows and the last row's covers run under the bar.
        app.global::<Nav>().set_queue_pinned(true);
        let r = r.clone();
        clicks(app, vec![(132.0, 224.0)], move |hits| line(&r, format!("[bar] sanity: a click on the first cover mid-page opened an album: {hits:?} (hits, tries)")));
    }));
    // Single clicks on covers: mid-page, then on the last row's covers just above the bar,
    // with auto-hide on and off, alternately (twice each), slowly.
    for (n, on) in [true, false, true, false].into_iter().enumerate() {
        step(if n == 0 { 1500 } else { 6000 }, Box::new(move |st, app, r| {
            st.settings.borrow_mut().bar_autohide = on;
            views::refresh_player_settings(st);
            let (_, h) = size(app);
            let bh = theme_bar(app);
            let r = r.clone();
            let pts = vec![(132.0, 224.0), (132.0, h - bh - 6.0), (350.0, h - bh - 12.0), (567.0, h - bh - 20.0), (132.0, 480.0)];
            clicks(app, pts, move |hits| line(&r, format!("[bar] auto-hide {}: clicks on a cover mid-page, three just above the bar, one mid-page: {hits:?}", if on { "on" } else { "off" })));
        }));
    }
    // Where do cover clicks stop working? Auto-hide off, x on the first column, from well
    // above the bar down to it, one at a time.
    step(6000, Box::new(|st, app, r| {
        st.settings.borrow_mut().bar_autohide = false;
        views::refresh_player_settings(st);
        let (_, h) = size(app);
        let ys: Vec<f32> = (0..16).map(|i| h - theme_bar(app) - 2.0 - i as f32 * 8.0).collect();
        let r = r.clone();
        let w = app.as_weak();
        let ys2 = ys.clone();
        sweep(app, ys.into_iter().map(|y| (132.0, y)).collect(), move |hits| {
            let _ = &w;
            line(&r, format!("[bar] auto-hide off, x 132: y -> opened an album: {:?}", ys2.iter().zip(hits).collect::<Vec<_>>()))
        });
    }));
    step(14_000, Box::new(|st, _app, _r| {
        st.settings.borrow_mut().bar_autohide = true;
        views::refresh_player_settings(st);
    }));
    let probe_points = |app: &AppWindow, ys: &[f32]| -> Vec<(f32, f32)> {
        let (w, _) = size(app);
        ys.iter().flat_map(|&y| (0..12).map(move |i| (40.0 + i as f32 * (w - 80.0) / 11.0, y))).collect()
    };
    let above = |app: &AppWindow| -> Vec<f32> {
        let (_, h) = size(app);
        (1..=6).map(|i| h - theme_bar(app) - i as f32 * 4.0).collect()
    };
    let on_bar = |app: &AppWindow| -> Vec<f32> {
        let (_, h) = size(app);
        vec![h - theme_bar(app) + 4.0, h - theme_bar(app) + 8.0]
    };
    step(1500, Box::new(move |_st, app, r| {
        let r = r.clone();
        snapshot(app, "c-07b-click-probe");
        clicks(app, probe_points(app, &above(app)), move |hits| {
            line(&r, format!("[bar] paused (bar up), auto-hide on: clicks in the 24 px above the bar that opened an album: {hits:?}"))
        });
    }));
    step(45_000, Box::new(move |st, app, r| {
        st.settings.borrow_mut().bar_autohide = false;
        views::refresh_player_settings(st);
        snapshot(app, "c-07c-click-probe-autohide-off");
        line(r, format!("[bar] before the auto-hide-off probe: page {:?}, scroll {}, bar up {}", app.global::<Nav>().get_page(), app.global::<Grid>().get_content_y(), bar(app)));
        let r = r.clone();
        clicks(app, probe_points(app, &above(app)), move |hits| line(&r, format!("[bar] auto-hide off: clicks in the 24 px above the bar that opened an album: {hits:?}")));
    }));
    step(45_000, Box::new(move |_st, app, r| {
        let r = r.clone();
        clicks(app, probe_points(app, &on_bar(app)), move |hits| {
            line(&r, format!("[bar] auto-hide off: clicks on the bar's empty top strip that opened an album under it: {hits:?}"))
        });
    }));
    step(15_000, Box::new(move |st, app, r| {
        st.settings.borrow_mut().bar_autohide = true;
        views::refresh_player_settings(st);
        let r = r.clone();
        clicks(app, probe_points(app, &on_bar(app)), move |hits| {
            line(&r, format!("[bar] auto-hide on (bar up, paused): clicks on the bar's empty top strip that opened an album under it: {hits:?}"))
        });
    }));
    step(15_000, Box::new(|st, app, _r| {
        app.global::<Nav>().set_queue_pinned(false);
        st.settings.borrow_mut().bar_autohide = false;
        views::refresh_player_settings(st);
    }));

    // ---- Remove the current queue item while paused ----------------------------------
    step(500, Box::new(|st, _app, r| {
        playback::play_tracks(st, album_tracks(st, 1), 0);
        let _ = r;
    }));
    step(1200, Box::new(|st, _app, r| {
        playback::set_paused(st, true);
        line(r, format!("[queue] before: {}", state(st)));
    }));
    step(600, Box::new(|st, _app, r| {
        let i = st.queue.borrow().current_index().unwrap_or(0);
        playback::queue_remove(st, i);
        line(r, format!("[queue] removed the current item ({i}) while paused"));
    }));
    step(1000, Box::new(|st, app, r| {
        line(r, format!("[queue] 1 s later: {}; bar shows {:?} playing {}", state(st), app.global::<crate::Player>().get_title().to_string(), app.global::<crate::Player>().get_playing()));
        snapshot(app, "c-08-removed-while-paused");
        // Now from a restored-looking state: stopped with a current item.
        st.player.stop();
    }));
    step(600, Box::new(|st, _app, r| {
        line(r, format!("[queue] engine stopped, queue kept: {}", state(st)));
        let i = st.queue.borrow().current_index().unwrap_or(0);
        playback::queue_remove(st, i);
    }));
    step(1000, Box::new(|st, _app, r| {
        line(r, format!("[queue] removed the current item while stopped, 1 s later: {}", state(st)));
        // Not the current one, while playing: playback goes on.
        playback::play_tracks(st, album_tracks(st, 1), 0);
    }));
    step(1000, Box::new(|st, _app, r| {
        playback::queue_remove(st, 2);
        line(r, format!("[queue] removed another item while playing: {}", state(st)));
        playback::set_paused(st, true);
    }));

    // ---- Sorting of artists, genres, labels ------------------------------------------
    step(500, Box::new(|st, _app, r| {
        let names = |g: Vec<library::query::Group>| g.into_iter().map(|g| g.name).collect::<Vec<_>>();
        line(r, format!("[sort] artists: {:?}", names(library::query::album_artists(&st.db).unwrap_or_default())));
        line(r, format!("[sort] genres: {:?}", names(library::query::album_groups(&st.db, false).unwrap_or_default())));
        line(r, format!("[sort] labels: {:?}", names(library::query::album_groups(&st.db, true).unwrap_or_default())));
        views::go(st, Page::Genres);
    }));
    step(1500, Box::new(|st, app, _r| {
        snapshot(app, "c-09-genres");
        views::go(st, Page::Artists);
    }));
    step(1500, Box::new(|st, app, _r| {
        snapshot(app, "c-10-artists");
        let long = st
            .db
            .query_row("SELECT id FROM albums ORDER BY length(title) DESC LIMIT 1", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0);
        views::open_album(st, long);
    }));

    // ---- The pill ---------------------------------------------------------------------
    step(1500, Box::new(|_st: &Rc<State>, app, r| {
        line(r, format!("[pill] title {:?}", app.global::<Nav>().get_title().to_string()));
        snapshot(app, "c-11-pill-long-title");
        let _ = app;
        views::flash(_st, "Can't play \"A Very Long Album Title That Keeps Going On And On Well Past Any Reasonable Width For A Pill - Track 2.flac\": unsupported");
    }));
    step(600, Box::new(|_st, app, _r| {
        snapshot(app, "c-12-pill-long-note");
    }));

    // ---- Settings > Keys ---------------------------------------------------------------
    // Settings > Keys: the rows sit at known places in the column (`row_offset`; a note
    // shown under a row pushes the rows after it down, so notes are cleared before a
    // click), and each click first scrolls its row into view (`row_y`); the keys end at
    // a known distance from the column's right edge (`key_x`).
    step(600, Box::new(|st, app, r| {
        line(r, format!("[keys] at start: {:?}", key_rows(app)));
        line(r, format!("[keys] settings.json shortcuts at start: {:?}", st.settings.borrow().shortcuts));
        st.settings.borrow_mut().muted = false;
        views::go(st, Page::Settings);
        views::settings_section(st, crate::settings::nav::KEYS);
    }));
    step(1200, Box::new(|_st, app, _r| {
        row_y(app, MUTE);
    }));
    step(800, Box::new(|_st, app, r| {
        line(r, format!("[keys] Settings > Keys scrolled to {}", app.global::<Nav>().get_page_scroll()));
        snapshot(app, "c-13-keys-editor");
        press_at(app, key_x(app, 0, 1), row_y(app, MUTE));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] clicked Mute's key: {}", capture(app)));
        snapshot(app, "c-14-keys-capturing");
        type_text(app, "k");
    }));
    step(300, Box::new(|st, app, r| {
        line(r, format!("[keys] typed K: {}; Mute row {:?}", capture(app), row(app, MUTE)));
        let before = st.settings.borrow().muted;
        type_text(app, "k");
        let after_k = st.settings.borrow().muted;
        type_text(app, "m");
        let after_m = st.settings.borrow().muted;
        line(r, format!("[keys] then K (focus still on the key button) mutes: {before} -> {after_k}; M after it: -> {after_m} (unchanged: M is free)"));
        type_text(app, "k");
        line(r, format!("[keys] labels: mute {:?}, play {:?}", app.global::<crate::Keys>().get_mute().to_string(), app.global::<crate::Keys>().get_play_pause().to_string()));
        press_at(app, key_x(app, 0, 1), row_y(app, SHUFFLE));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] clicked Shuffle's key: {}", capture(app)));
        chord(app, "f", &[Key::Control]);
    }));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] pressed Ctrl+F (Search's): {}", capture(app)));
        snapshot(app, "c-15-keys-conflict");
        press(app, Key::Escape, &[]);
    }));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] Esc: {}; Shuffle {:?}, Search {:?}", capture(app), row(app, SHUFFLE), row(app, SEARCH)));
        press_at(app, key_x(app, 0, 1), row_y(app, SHUFFLE));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, _r| chord(app, "f", &[Key::Control])));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] Shuffle, Ctrl+F again: {}", capture(app)));
        press(app, Key::Return, &[]);
    }));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] Enter (reassign): {}; Shuffle {:?}, Search {:?}", capture(app), row(app, SHUFFLE), row(app, SEARCH)));
        snapshot(app, "c-16-keys-reassigned");
        let s = app.global::<crate::Player>().get_shuffle();
        app.invoke_focus_shortcuts();
        chord(app, "f", &[Key::Control]);
        line(r, format!("[keys] Ctrl+F now toggles shuffle: {s} -> {}; page {:?}", app.global::<crate::Player>().get_shuffle(), app.global::<Nav>().get_page()));
        type_text(app, "/");
        line(r, format!("[keys] / still searches: page {:?}", app.global::<Nav>().get_page()));
        chord(app, "f", &[Key::Control]);
    }));
    step(600, Box::new(|st, app, _r| {
        views::go(st, Page::Settings);
        views::settings_section(st, crate::settings::nav::KEYS);
        let _ = app;
    }));
    step(1000, Box::new(|_st, _app, _r| {}));
    step(600, Box::new(|_st, app, _r| {
        press_at(app, key_x(app, 0, 1), row_y(app, REPEAT));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] clicked Repeat's key: {}", capture(app)));
        press(app, Key::Tab, &[]);
    }));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] pressed Tab (fixed): {}", capture(app)));
        snapshot(app, "c-17-keys-refused");
        press(app, Key::Delete, &[]);
    }));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] Delete: {}; Repeat {:?}", capture(app), row(app, REPEAT)));
        // Reset Search (its reset button): takes Ctrl+F back from Shuffle.
        press_at(app, reset_x(app), row_y(app, SEARCH));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] reset Search: {}; Search {:?}, Shuffle {:?}", capture(app), row(app, SEARCH), row(app, SHUFFLE)));
        snapshot(app, "c-18-keys-reset-row");
        press_at(app, key_x(app, 0, 1), row_y(app, PIN_QUEUE));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, _r| chord(app, "j", &[Key::Control])));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[keys] Pin queue, Ctrl+J: {}; row {:?}; tooltip key {:?}", capture(app), row(app, PIN_QUEUE), app.global::<crate::Keys>().get_pin_queue().to_string()));
        let nav = app.global::<Nav>();
        let before = nav.get_queue_pinned();
        app.invoke_focus_shortcuts();
        chord(app, "j", &[Key::Control]);
        let after_j = nav.get_queue_pinned();
        chord(app, "q", &[Key::Control]);
        line(r, format!("[keys] Ctrl+J pins the queue: {before} -> {after_j}; Ctrl+Q after it: -> {} (unchanged)", nav.get_queue_pinned()));
        chord(app, "j", &[Key::Control]);
    }));
    step(600, Box::new(|_st, app, _r| {
        // Esc cancels: Mute's key, then Esc.
        press_at(app, key_x(app, 0, 1), row_y(app, MUTE));
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, _r| press(app, Key::Escape, &[])));
    step(300, Box::new(|st, app, r| {
        line(r, format!("[keys] Mute's key, then Esc: {}; Mute {:?}", capture(app), row(app, MUTE)));
        st.flush_settings();
        let text = std::fs::read_to_string(st.paths.settings()).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        line(r, format!("[keys] settings.json now: shortcuts = {}", v.get("shortcuts").cloned().unwrap_or_default()));
        // A plain key never acts in a text field: K (mute) typed into search.
        crate::keys::shortcuts::run(st, crate::keys::shortcuts::Shortcut::Do(crate::keys::map::Action::Search));
    }));
    step(500, Box::new(|st, app, r| {
        let muted = st.settings.borrow().muted;
        type_text(app, "kk");
        line(r, format!("[keys] typed \"kk\" in the search field: muted {muted} -> {}; field {:?}", st.settings.borrow().muted, app.global::<crate::Search>().get_query().to_string()));
        app.invoke_focus_shortcuts();
        views::go(st, Page::Settings);
        views::settings_section(st, crate::settings::nav::KEYS);
    }));
    step(1000, Box::new(|_st, app, _r| {
        row_y(app, MUTE);
    }));
    step(600, Box::new(|_st, app, r| {
        snapshot(app, "c-19-keys-remapped");
        line(r, format!("[keys] before Reset all: {:?}", key_rows(app)));
        crate::state::with(crate::keys::edit::reset_all);
        line(r, format!("[keys] Reset all: all default {}; Mute {:?}, Search {:?}", app.global::<crate::Keys>().get_all_default(), row(app, MUTE), row(app, SEARCH)));
    }));

    // Leaving Settings while a key button waits: the capture ends with the page.
    step(800, Box::new(|_st, _app, _r| {}));
    step(800, Box::new(|_st, app, _r| press_at(app, key_x(app, 0, 1), row_y(app, MUTE))));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|st, app, r| {
        line(r, format!("[keys] clicked Mute's key: {}", capture(app)));
        views::go(st, Page::Home);
        line(r, format!("[keys] went Home while it waited: {}", capture(app)));
        app.invoke_focus_shortcuts();
        let before = st.settings.borrow().muted;
        type_text(app, "m");
        line(r, format!("[keys] M on Home afterwards mutes: {before} -> {}", st.settings.borrow().muted));
        type_text(app, "m");
        views::go(st, Page::Settings);
        views::settings_section(st, crate::settings::nav::KEYS);
    }));

    // Page controls just above the bar, and in the bar's place while it is hidden: the Pin
    // queue key button scrolled to 100 px (then 60 px) above the window's bottom.
    step(800, Box::new(|st, app, _r| {
        crate::state::with(crate::keys::edit::cancel);
        app.invoke_focus_shortcuts();
        st.settings.borrow_mut().bar_autohide = true;
        views::refresh_player_settings(st);
        playback::set_paused(st, false);
        let (w, h) = size(app);
        show_row(app, PIN_QUEUE, h - 100.0);
        move_to(app, w / 2.0, h / 2.0);
    }));
    step(1500, Box::new(|_st, app, r| {
        line(r, format!("[bar] playing, auto-hide on, pointer mid-page: bar up {}", bar(app)));
        snapshot(app, "c-20-control-above-hidden-bar");
        press_at(app, key_x(app, 0, 1), size(app).1 - 100.0);
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|_st, app, r| {
        line(r, format!("[bar] bar hidden: a click on a key button 100 px above the bottom (just above where the bar comes): {}", capture(app)));
        crate::state::with(crate::keys::edit::cancel);
        let (w, h) = size(app);
        show_row(app, PIN_QUEUE, h - 60.0);
        move_to(app, w / 2.0, h / 2.0);
    }));
    step(1500, Box::new(|_st, app, r| {
        line(r, format!("[bar] pointer mid-page: bar up {}", bar(app)));
        press_at(app, key_x(app, 0, 1), size(app).1 - 60.0);
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|st, app, r| {
        line(r, format!("[bar] bar hidden: a click on a key button 60 px above the bottom (in the bar's place): {}; bar up {}", capture(app), bar(app)));
        crate::state::with(crate::keys::edit::cancel);
        playback::set_paused(st, true);
        show_row(app, PIN_QUEUE, size(app).1 - 100.0);
    }));
    step(1200, Box::new(|_st, app, r| {
        line(r, format!("[bar] paused (bar up), auto-hide on: bar up {}", bar(app)));
        press_at(app, key_x(app, 0, 1), size(app).1 - 100.0);
    }));
    step(150, Box::new(|_st, app, _r| release(app)));
    step(300, Box::new(|st, app, r| {
        line(r, format!("[bar] bar up: a click on a key button 100 px above the bottom (12 px above the bar): {}", capture(app)));
        crate::state::with(crate::keys::edit::cancel);
        st.settings.borrow_mut().bar_autohide = false;
        views::refresh_player_settings(st);
    }));

    step(500, Box::new(|_st, _app, r| {
        write_report("cleanup", &r.borrow());
        let _ = slint::quit_event_loop();
    }));
    run_steps(steps);
}


// Settings > Keys rows (keys::map::Action::ALL order).
const MUTE: usize = 7;
const SHUFFLE: usize = 8;
const REPEAT: usize = 9;
const SEARCH: usize = 16;
const PIN_QUEUE: usize = 20;
/// The first row's centre, from the top of the column (Settings > Keys at the top, on a
/// window 1920 px wide: the text above the rows takes two lines), each row KEY_ROW_H
/// below the one before, and each group's heading (above the rows KEY_GROUPS start)
/// KEY_GROUP_H more.
const KEY_FIRST: f32 = 160.0;
const KEY_ROW_H: f32 = 46.0;
const KEY_GROUP_H: f32 = 47.0;
const KEY_GROUPS: [usize; 2] = [10, 11];

/// A row's centre in the column's content, at scroll 0.
fn row_offset(row: usize) -> f32 {
    KEY_FIRST + row as f32 * KEY_ROW_H + KEY_GROUPS.iter().filter(|g| **g <= row).count() as f32 * KEY_GROUP_H
}

/// Scroll Settings so that `row`'s centre is at window y `at`, and return `at`.
fn show_row(app: &AppWindow, row: usize, at: f32) -> f32 {
    let top = app.global::<crate::SettingsView>().get_column_top();
    app.global::<Nav>().set_page_scroll((at - top - row_offset(row)).min(0.0));
    at
}

/// A row's centre, scrolled into view mid-window.
fn row_y(app: &AppWindow, row: usize) -> f32 {
    show_row(app, row, 420.0)
}

/// The right edge of the Settings column (pages/settings/settings.slint, SettingsPage; no sidebar pinned).
fn column_right(app: &AppWindow) -> f32 {
    let (w, _) = size(app);
    let margin = if w >= 900.0 { 40.0 } else { 24.0 };
    let x = margin + 200.0 + 32.0;
    x + (w - x - margin).min(700.0)
}

/// The centre of a row's key `slot` (of `slots`), each 76 px wide, 8 apart, ending 58 px
/// from the column's right edge (the row's padding, the reset button and its spacing).
fn key_x(app: &AppWindow, slot: usize, slots: usize) -> f32 {
    column_right(app) - 58.0 - 38.0 - (slots - 1 - slot) as f32 * 84.0
}

fn reset_x(app: &AppWindow) -> f32 {
    column_right(app) - 14.0 - 16.0
}

fn press_at(app: &AppWindow, x: f32, y: f32) {
    // A note left under a row (a reassign's, a reset's) would move the rows after it.
    if app.global::<crate::Keys>().get_capture_row() < 0 {
        app.global::<crate::Keys>().set_note("".into());
    }
    let position = LogicalPosition::new(x, y);
    PRESSED.with(|p| p.set(Some((x, y))));
    app.window().dispatch_event(WindowEvent::PointerMoved { position });
    app.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
}

fn release(app: &AppWindow) {
    if let Some((x, y)) = PRESSED.with(|p| p.take()) {
        app.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button: PointerEventButton::Left });
    }
}

thread_local! {
    static PRESSED: std::cell::Cell<Option<(f32, f32)>> = const { std::cell::Cell::new(None) };
}

/// A key with modifiers held: "f" with Ctrl is Ctrl+F.
fn chord(app: &AppWindow, text: &str, mods: &[Key]) {
    let w = app.window();
    for m in mods {
        w.dispatch_event(WindowEvent::KeyPressed { text: (*m).into() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    for m in mods.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: (*m).into() });
    }
}

/// What the editor is doing: the slot waiting and the note.
fn capture(app: &AppWindow) -> String {
    let k = app.global::<crate::Keys>();
    format!("capture row {} slot {}, conflict {}, note {:?}", k.get_capture_row(), k.get_capture_slot(), k.get_conflict(), k.get_note().to_string())
}

fn row(app: &AppWindow, i: usize) -> String {
    let k = app.global::<crate::Keys>();
    k.get_rows().row_data(i).map(|r| format!("{} = [{}|{}]", r.what, r.key0, r.key1)).unwrap_or_default()
}

/// Rows of the Keys editor whose keys are not the defaults, for the report.
fn key_rows(app: &AppWindow) -> Vec<String> {
    let k = app.global::<crate::Keys>();
    (0..k.get_rows().row_count()).filter_map(|i| k.get_rows().row_data(i)).filter(|r| !r.is_default).map(|r| format!("{} = [{}|{}]", r.what, r.key0, r.key1)).collect()
}
