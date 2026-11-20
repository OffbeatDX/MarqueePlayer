//! `MP_BENCH=zoom` and `zoom-check`: the interface size (`zoom.rs`), on the core fixtures.
//!
//! `zoom` (`bench-zoom.txt`):
//! - the levels 70%, 100%, 150% and 200% set one after the other: the scale factor Slint has
//!   (the monitor's times the zoom), the layout in the window at each (the caption buttons at
//!   the top right, the player bar's bottom on the window's, the bar's minimum width within
//!   the window), the grids' covers decoded at the size drawn once that is larger than a
//!   thumbnail (on a 150% monitor, simulated), a snapshot each (`zoom-070.png`, `zoom-100.png`, `zoom-150.png`,
//!   `zoom-200.png`);
//! - the keys through the window's own key dispatch: Ctrl+=, Ctrl+Shift++ (a US keyboard's
//!   +), Ctrl++ (the keypad's), Ctrl+-, Ctrl+0, the pill after each; remapped in Settings >
//!   Keys' editor (its capture, as a key press reaches it) and reset;
//! - Settings > Window > Interface size (its callback, the text it shows) and "Find a
//!   setting" (the setting, and the keys' actions);
//! - Ctrl and the wheel (the handler the window hook calls);
//! - a pinch, its fingers handed to the handler winit's touch events go to: the zoom live,
//!   saved when the fingers lift, and the press the first finger made on the Back button
//!   undone (no click, the page unchanged), where a press and release without the pinch
//!   goes back;
//! - another monitor's scale factor (the handler of winit's ScaleFactorChanged): the zoom
//!   kept on top of it, and back;
//! - the minimum size at 200% and at 100%: the window no smaller, the layout inside it, at 200%
//!   still on the monitor (`zoom-200-min.png`);
//! - 150% left in settings.json for `zoom-check`.
//!
//! `zoom-check` (`bench-zoom-check.txt`), the next start on the same data folder: 150% from
//! settings.json, applied before anything is drawn for long, the layout inside the window,
//! `zoom-restored-150.png`; then 100% again.
//!
//! No OS input: keys go through Slint's dispatch, the wheel, the touches and the scale factor
//! straight to the handlers the window hook calls.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

use crate::bench::{Step, run_steps, snapshot, write_report};
use crate::keys::map::Action;
use crate::state::{self, State};
use crate::zoom::Touch;
use crate::{Actions, AppWindow, Chrome, Nav, Page, SettingsView, settings, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("zoom bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn check(r: &Report, what: &str, ok: bool, detail: impl std::fmt::Display) {
    crate::bench::verdict(&mut r.borrow_mut(), what, ok, detail);
}

thread_local! {
    /// The monitor's own scale factor, as found at the start (at 100%).
    static MONITOR: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

fn monitor() -> f32 {
    MONITOR.with(std::cell::Cell::get)
}

/// A key with Ctrl (and Shift) held, through the window's own dispatch.
fn chord(app: &AppWindow, text: &str, shift: bool) {
    let w = app.window();
    w.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    if shift {
        w.dispatch_event(WindowEvent::KeyPressed { text: Key::Shift.into() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    if shift {
        w.dispatch_event(WindowEvent::KeyReleased { text: Key::Shift.into() });
    }
    w.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
}

fn saved(st: &State) -> u32 {
    st.settings.borrow().zoom
}

/// The zoom everywhere it shows: the setting, Settings' text, the scale factor.
fn zoom_is(r: &Report, st: &State, app: &AppWindow, what: &str, pct: u32) {
    let sf = app.window().scale_factor();
    let want = monitor() * pct as f32 / 100.0;
    let c = app.global::<Chrome>();
    let ok = saved(st) == pct && c.get_zoom() == pct as i32 && c.get_zoom_text() == format!("{pct}%") && (sf - want).abs() < 1e-3 && (crate::zoom::factor() - pct as f32 / 100.0).abs() < 1e-4;
    check(r, what, ok, format!("setting {}%, shown {:?}, scale factor {sf:.3} (want {want:.3}), pill {:?}", saved(st), c.get_zoom_text().to_string(), app.global::<Nav>().get_flash().to_string()));
}

fn pill_says(r: &Report, app: &AppWindow, pct: u32) {
    let f = app.global::<Nav>().get_flash().to_string();
    check(r, &format!("the pill says {pct}%"), f == format!("Interface size {pct}%"), format!("{f:?}"));
}

/// The layout inside the window: the caption buttons at its top right, the player bar's bottom
/// on the window's bottom, the bar's minimum width within the window.
fn layout_fits(r: &Report, app: &AppWindow, what: &str) {
    let sf = app.window().scale_factor();
    let size = app.window().size().to_logical(sf);
    let (cap_left, cap_h) = (app.get_caption_left(), app.get_caption_height());
    let cap_w = size.width - cap_left;
    let caption_ok = cap_left > 0.0 && (cap_w - 3.0 * crate::platform::BUTTON_W).abs() < 1.0 && (cap_h - crate::platform::BUTTON_H).abs() < 1.0;
    let bar_bottom = app.get_bar_top() + app.get_bar_height();
    let bar_ok = app.get_bar_visible() && (bar_bottom - size.height).abs() < 1.0 && app.get_bar_top() > cap_h;
    let width_ok = app.get_bar_min_width() <= size.width + 0.5;
    check(
        r,
        &format!("{what}: the layout fits the window"),
        caption_ok && bar_ok && width_ok,
        format!(
            "window {}x{} px = {:.0}x{:.0} logical; caption buttons {cap_w:.0}x{cap_h:.0} at x {cap_left:.0}; bar {:.0}..{bar_bottom:.0} (min width {:.0})",
            app.window().size().width,
            app.window().size().height,
            size.width,
            size.height,
            app.get_bar_top(),
            app.get_bar_min_width()
        ),
    );
}

fn monitor_size(app: &AppWindow) -> Option<(u32, u32)> {
    use slint::winit_030::WinitWindowAccessor;
    app.window().with_winit_window(|w| w.current_monitor().map(|m| (m.size().width, m.size().height))).flatten()
}

/// A touch's physical position for a logical point.
fn phys(app: &AppWindow, x: f32, y: f32) -> (f32, f32) {
    let sf = app.window().scale_factor();
    (x * sf, y * sf)
}

fn press_at(app: &AppWindow, x: f32, y: f32) {
    let p = LogicalPosition::new(x, y);
    app.window().dispatch_event(WindowEvent::PointerMoved { position: p });
    app.window().dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
}

fn release_at(app: &AppWindow, x: f32, y: f32) {
    let p = LogicalPosition::new(x, y);
    app.window().dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
    app.window().dispatch_event(WindowEvent::PointerExited);
}

/// The pill's Back button (chrome.slint, NavPill: 12 px in, 14 down, its second 32 px button
/// after 4 px), with the sidebar not pinned.
const BACK: (f32, f32) = (12.0 + 4.0 + 32.0 + 16.0, 14.0 + 4.0 + 16.0);

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

    step!(1500, |st, app, r| {
        views::go(st, Page::Home);
        app.invoke_focus_shortcuts();
        let sf = app.window().scale_factor();
        MONITOR.with(|m| m.set(sf / crate::zoom::factor()));
        line(r, format!("monitor scale factor {:.3}, monitor {:?}, window {:?}", monitor(), monitor_size(app), app.window().size()));
        zoom_is(r, st, app, "100% at the start", 100);
        layout_fits(r, app, "100%");
    });

    // ---- the levels -----------------------------------------------------------------------
    for pct in [70u32, 100, 150, 200] {
        step!(300, move |st, _app, _r| {
            crate::zoom::set(st, pct, true);
        });
        step!(1200, move |st, app, r| {
            zoom_is(r, st, app, &format!("{pct}% set"), pct);
            pill_says(r, app, pct);
            layout_fits(r, app, &format!("{pct}%"));
            // The grids' covers: thumbnails while those are as large as drawn, else decoded at
            // the size drawn.
            let drawn = (app.global::<crate::Theme>().get_cover_size() * app.window().scale_factor()).ceil() as u32;
            let want_card = crate::covers::card_size(drawn) > 0;
            let kind = crate::covers::card_kind();
            check(
                r,
                &format!("{pct}%: the grid covers at the size drawn"),
                (kind == crate::covers::Kind::Card) == want_card,
                format!("covers drawn {drawn} px, thumbnails {} px, decoded as {kind:?} ({} px)", library::art::THUMB_PX, crate::covers::card_size(drawn)),
            );
            snapshot(app, &format!("zoom-{pct:03}"));
        });
    }
    // The covers drawn larger than a thumbnail: 200% on a 150% monitor (its scale factor
    // handed to the handler winit's ScaleFactorChanged goes to), on the Albums grid.
    step!(300, |st, app, _r| {
        crate::zoom::monitor_changed(app, 1.5);
        views::go(st, Page::Albums);
    });
    step!(2500, |st, app, r| {
        snapshot(app, "zoom-200-on-150-albums");
        let grid = st.models.grid.clone();
        let stems: Vec<String> = (0..12).filter_map(|i| grid.art_at(i).flatten()).collect();
        let kind = crate::covers::card_kind();
        let held = stems.iter().filter(|s| crate::covers::cached(s, kind)).count();
        // Only the rows on screen ask (one or two at a total scale of 3).
        check(r, "200%: the grid's covers on screen are in memory at that size", held >= 1, format!("{held} of the first {} as {kind:?}", stems.len()));
        if kind == crate::covers::Kind::Card {
            let px = crate::covers::card_size((app.global::<crate::Theme>().get_cover_size() * app.window().scale_factor()).ceil() as u32);
            let sizes: Vec<u32> = stems.iter().filter_map(|s| crate::covers::cached_size(s, kind)).map(|(w, h)| w.max(h)).collect();
            check(r, "200%: those covers are as large as drawn (or as their image)", !sizes.is_empty() && sizes.iter().all(|&w| w == px || w != library::art::THUMB_PX), format!("longest sides {sizes:?}, wanted {px}"));
        } else {
            check(r, "200% on a 150% monitor: the grid's covers decoded at the size drawn", false, format!("{kind:?}"));
        }
        crate::zoom::monitor_changed(app, monitor());
        check(r, "back on the monitor's own scale: thumbnails again", crate::covers::card_kind() == crate::covers::Kind::Thumb || crate::covers::card_size((app.global::<crate::Theme>().get_cover_size() * app.window().scale_factor()).ceil() as u32) > 0, format!("{:?}", crate::covers::card_kind()));
        crate::zoom::set(st, 100, true);
        views::go(st, Page::Home);
        app.invoke_focus_shortcuts();
    });

    // ---- the keys -------------------------------------------------------------------------
    for (text, shift, want, what) in [
        ("=", false, 110, "Ctrl+="),
        ("+", true, 125, "Ctrl+Shift++ (a US keyboard's +)"),
        ("+", false, 150, "Ctrl++ (the keypad's +)"),
        ("-", false, 125, "Ctrl+-"),
        ("-", false, 110, "Ctrl+- again"),
        ("0", false, 100, "Ctrl+0"),
    ] {
        step!(400, move |st, app, r| {
            chord(app, text, shift);
            zoom_is(r, st, app, &format!("{what}: {want}%"), want);
            pill_says(r, app, want);
        });
    }
    // At the limits it stays.
    step!(400, |st, app, r| {
        for _ in 0..12 {
            chord(app, "=", false);
        }
        zoom_is(r, st, app, "Ctrl+= past the top: 200%", 200);
        for _ in 0..12 {
            chord(app, "-", false);
        }
        zoom_is(r, st, app, "Ctrl+- past the bottom: 70%", 70);
        chord(app, "0", false);
        zoom_is(r, st, app, "and Ctrl+0: 100%", 100);
    });

    // ---- Settings > Keys: listed last, remapped and reset ------------------------------------
    step!(400, |st, app, r| {
        let rows = app.global::<crate::Keys>().get_rows();
        let n = rows.row_count();
        let last: Vec<String> = (n.saturating_sub(3)..n).filter_map(|i| rows.row_data(i)).map(|k| format!("{} [{}{}]", k.what, k.key0, if k.key1.is_empty() { String::new() } else { format!(" / {}", k.key1) })).collect();
        let heading = rows.row_data(n.saturating_sub(3)).map(|k| k.group.to_string()).unwrap_or_default();
        let ok = n == Action::ALL.len() && last.len() == 3 && last[0].starts_with("Interface size larger") && last[0].contains("Ctrl+=") && last[0].contains("Ctrl++") && last[1].contains("Ctrl+-") && last[2].contains("Ctrl+0") && heading == "Interface size";
        check(r, "Settings > Keys lists the three last, under their heading", ok, format!("{n} rows; heading {heading:?}; {last:?}"));
        // The lyrics keys keep their rows (the cleanup bench counts on them).
        let g = rows.row_data(Action::ALL.iter().position(|a| *a == Action::LyricsEarlier).unwrap_or(0)).map(|k| k.key0.to_string()).unwrap_or_default();
        check(r, "the lyrics keys' rows did not move", g == "G" && Action::ALL[n - 5] == Action::LyricsEarlier, format!("row {} is {g:?}", n - 5));
        // Ctrl+9 for smaller, by the editor's capture.
        let row = Action::ALL.iter().position(|a| *a == Action::ZoomOut).unwrap_or(0) as i32;
        crate::keys::edit::capture(st, row, 0);
        crate::keys::edit::captured(st, "9", true, false, false);
        let shown = rows.row_data(row as usize).map(|k| k.key0.to_string()).unwrap_or_default();
        let kept = st.settings.borrow().shortcuts.get("zoom_out").cloned().unwrap_or_default();
        check(r, "zoom out remapped to Ctrl+9", shown == "Ctrl+9" && kept == ["Ctrl+9"], format!("shown {shown:?}, settings.json {kept:?}"));
        app.invoke_focus_shortcuts();
        chord(app, "9", false);
        zoom_is(r, st, app, "Ctrl+9: 90%", 90);
        chord(app, "-", false);
        zoom_is(r, st, app, "Ctrl+- no longer zooms out", 90);
        crate::keys::edit::reset(st, row);
        let shown = rows.row_data(row as usize).map(|k| k.key0.to_string()).unwrap_or_default();
        check(r, "and reset to Ctrl+-", shown == "Ctrl+-" && !st.settings.borrow().shortcuts.contains_key("zoom_out"), format!("shown {shown:?}"));
        chord(app, "0", false);
    });

    // ---- Settings > Window and "Find a setting" ---------------------------------------------
    step!(400, |st, _app, _r| {
        views::go(st, Page::Settings);
        views::settings_section(st, settings::nav::WINDOW);
    });
    step!(800, |st, app, r| {
        app.global::<Actions>().invoke_zoom_step(1);
        zoom_is(r, st, app, "Settings' + button: 110%", 110);
        app.global::<Actions>().invoke_zoom_step(1);
        zoom_is(r, st, app, "again: 125%", 125);
        app.global::<Actions>().invoke_zoom_step(-1);
        zoom_is(r, st, app, "Settings' - button: 110%", 110);
    });
    step!(900, |_st, app, _r| {
        snapshot(app, "zoom-settings-110");
    });
    step!(200, |st, app, r| {
        app.global::<Actions>().invoke_zoom_step(0);
        zoom_is(r, st, app, "Settings' Reset: 100%", 100);
        for q in ["zoom", "interface size", "bigger text", "pinch"] {
            views::settings_filter(st, q);
            let hits: Vec<String> = app.global::<SettingsView>().get_hits().iter().map(|h| format!("{} ({})", h.label, h.section_name)).collect();
            let ok = hits.iter().any(|h| h == "Interface size (Window)");
            check(r, &format!("Find a setting \"{q}\" finds it"), ok, format!("{hits:?}"));
        }
        views::settings_filter(st, "interface size");
        let hits: Vec<String> = app.global::<SettingsView>().get_hits().iter().map(|h| h.label.to_string()).collect();
        let keys = hits.iter().filter(|h| h.starts_with("Interface size") && h.as_str() != "Interface size").count();
        check(r, "and its three keys' actions", keys == 3, format!("{hits:?}"));
        views::settings_filter(st, "");
    });

    // ---- Ctrl and the wheel ---------------------------------------------------------------
    step!(400, |st, app, r| {
        crate::zoom::wheel(st, 1.0);
        zoom_is(r, st, app, "Ctrl+wheel up a notch: 110%", 110);
        crate::zoom::wheel(st, 0.5);
        zoom_is(r, st, app, "half a notch more (a touchpad): still 110%", 110);
        crate::zoom::wheel(st, 0.5);
        zoom_is(r, st, app, "the other half: 125%", 125);
        crate::zoom::wheel(st, -2.0);
        zoom_is(r, st, app, "two notches down: 100%", 100);
    });

    // ---- a pinch ---------------------------------------------------------------------------
    // Back's place first, without a pinch: a press and release there goes back.
    step!(300, |st, _app, _r| {
        views::go(st, Page::Home);
        views::go(st, Page::Albums);
    });
    step!(900, |_st, app, r| {
        press_at(app, BACK.0, BACK.1);
        release_at(app, BACK.0, BACK.1);
        let page = app.global::<Nav>().get_page();
        check(r, "without a pinch, a press and release on Back goes back", page == Page::Home, format!("{page:?}"));
    });
    step!(300, |st, _app, _r| views::go(st, Page::Albums));
    step!(900, |st, app, r| {
        // Finger 1 on Back: the handler leaves it to Slint, which presses Back (done here
        // as Slint's touch handling would).
        let f1 = phys(app, BACK.0, BACK.1);
        let took = crate::zoom::touch(app, 1, Touch::Started, f1);
        press_at(app, BACK.0, BACK.1);
        check(r, "pinch: the first finger is the page's", !took, format!("taken {took}"));
        // Finger 2, 300 px to the right: the pinch is on, the press undone.
        let f2 = (f1.0 + 300.0, f1.1 + 100.0);
        let took = crate::zoom::touch(app, 2, Touch::Started, f2);
        check(r, "pinch: the second finger is the pinch's", took && crate::zoom::pinching(), format!("taken {took}, pinching {}", crate::zoom::pinching()));
        // Apart: past the threshold, then to 1.5 times the distance, in moves.
        let d0 = ((300.0f32).powi(2) + 100.0f32.powi(2)).sqrt();
        let along = |d: f32| (f1.0 + 300.0 * d / d0, f1.1 + 100.0 * d / d0);
        let mut seen = Vec::new();
        let start = d0 + 20.0;
        crate::zoom::touch(app, 2, Touch::Moved, along(start));
        for i in 1..=10 {
            let d = start * (1.0 + 0.05 * i as f32);
            let took = crate::zoom::touch(app, 2, Touch::Moved, along(d));
            seen.push(format!("{}{}", st.settings.borrow().zoom, if took { "" } else { "!" }));
        }
        let live = app.window().scale_factor();
        check(
            r,
            "pinch: the zoom follows the fingers, every move taken",
            saved(st) == 150 && (live - monitor() * 1.5).abs() < 1e-3 && !seen.iter().any(|s| s.ends_with('!')),
            format!("zoom along the way {seen:?}; scale factor {live:.3}"),
        );
        // Finger 1 lifts: Slint hears it (and releases where the press was): no click.
        let took = crate::zoom::touch(app, 1, Touch::Ended, f1);
        release_at(app, BACK.0, BACK.1);
        check(r, "pinch: the first finger's end is Slint's", !took, format!("taken {took}"));
        let took = crate::zoom::touch(app, 2, Touch::Moved, (f2.0 + 40.0, f2.1));
        check(r, "pinch: the last finger's moves are not", took, format!("taken {took}"));
        let took = crate::zoom::touch(app, 2, Touch::Ended, (f2.0 + 40.0, f2.1));
        check(r, "pinch: over when the fingers are up", took && !crate::zoom::pinching(), format!("taken {took}"));
    });
    step!(500, |st, app, r| {
        let page = app.global::<Nav>().get_page();
        check(r, "pinch: Back was not clicked (still on Albums)", page == Page::Albums, format!("{page:?}"));
        zoom_is(r, st, app, "pinch: 150% kept", 150);
        pill_says(r, app, 150);
        snapshot(app, "zoom-after-pinch");
        // Pinched in again, to 70% (clamped from 50%).
        let c = (800.0, 500.0);
        crate::zoom::touch(app, 7, Touch::Started, (c.0 - 200.0, c.1));
        crate::zoom::touch(app, 8, Touch::Started, (c.0 + 200.0, c.1));
        crate::zoom::touch(app, 8, Touch::Moved, (c.0 + 170.0, c.1));
        crate::zoom::touch(app, 8, Touch::Moved, (c.0 - 200.0 + 370.0 * 0.4, c.1));
        crate::zoom::touch(app, 7, Touch::Ended, (c.0 - 200.0, c.1));
        crate::zoom::touch(app, 8, Touch::Ended, (c.0 - 200.0 + 370.0 * 0.4, c.1));
        zoom_is(r, st, app, "pinched in: 70% (clamped)", 70);
    });
    // (The player bar slides to its new place: looked at once it is there.)
    step!(1200, |st, app, r| {
        layout_fits(r, app, "70% after a pinch");
        crate::zoom::set(st, 100, true);
    });

    // ---- another monitor ---------------------------------------------------------------------
    step!(400, |st, app, r| {
        crate::zoom::set(st, 150, true);
        let real = monitor();
        crate::zoom::monitor_changed(app, 1.25);
        let sf = app.window().scale_factor();
        check(r, "a 125% monitor: its factor times the zoom", (sf - 1.25 * 1.5).abs() < 1e-3 && saved(st) == 150, format!("scale factor {sf:.3}"));
        crate::zoom::monitor_changed(app, real);
        let sf = app.window().scale_factor();
        check(r, "and back: the monitor's own times the zoom", (sf - real * 1.5).abs() < 1e-3, format!("scale factor {sf:.3}"));
        crate::zoom::set(st, 200, true);
    });

    // ---- the minimum size ------------------------------------------------------------------
    for pct in [200u32, 100] {
        // The smallest a drag of the border leaves (what the window answers WM_GETMINMAXINFO
        // with: the bench's own resize is not held to it), then the window sized to that.
        step!(500, move |st, app, r| {
            crate::zoom::set(st, pct, true);
            let sf = app.window().scale_factor();
            let c = app.global::<Chrome>();
            let want = ((c.get_min_width() * sf).round() as i32, (c.get_min_height() * sf).round() as i32);
            let got = crate::platform::devwindow::min_client(st.hwnd.get());
            let mon = monitor_size(app);
            let ok = got.is_none_or(|(w, h)| (w - want.0).abs() <= 2 && (h - want.1).abs() <= 40 && mon.is_none_or(|(mw, mh)| w <= mw as i32 && h <= mh as i32));
            check(r, &format!("{pct}%: a drag stops at the minimum, which fits the monitor"), ok, format!("smallest client {got:?} px, minimum {want:?} px, monitor {mon:?}"));
            let (w, h) = got.unwrap_or(want);
            crate::platform::devwindow::resize_client(st.hwnd.get(), w.max(want.0), h.max(want.1));
        });
        step!(1500, move |_st, app, r| {
            let sf = app.window().scale_factor();
            let c = app.global::<Chrome>();
            let (mw, mh) = (c.get_min_width(), c.get_min_height());
            let phys = app.window().size();
            let logical = phys.to_logical(sf);
            let mon = monitor_size(app);
            let at_least = logical.width + 1.0 >= mw && logical.height + 1.0 >= mh;
            let on_screen = mon.is_none_or(|(w, h)| phys.width <= w && phys.height <= h);
            let design = (mw, mh) == crate::zoom::BASE_MIN;
            check(
                r,
                &format!("{pct}% at the minimum size: no smaller than the minimum, on the monitor"),
                at_least && on_screen && (pct != 100 || design),
                format!("window {}x{} px, {:.0}x{:.0} logical; minimum {mw:.0}x{mh:.0} logical; monitor {mon:?}", phys.width, phys.height, logical.width, logical.height),
            );
            layout_fits(r, app, &format!("{pct}% at the minimum size"));
            snapshot(app, &format!("zoom-{pct:03}-min"));
        });
    }
    step!(300, |st, _app, _r| {
        crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void);
        views::go(st, Page::Home);
    });

    // ---- left at 150% for the next start ---------------------------------------------------
    step!(1200, |st, _app, _r| crate::zoom::set(st, 150, true));
    step!(1200, |st, app, r| {
        st.flush_settings();
        let text = std::fs::read_to_string(st.paths.settings()).unwrap_or_default();
        let on_disk = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("zoom").and_then(|z| z.as_u64()));
        check(r, "150% in settings.json", on_disk == Some(150), format!("{on_disk:?}"));
        layout_fits(r, app, "150%, maximized");
        let fails = crate::bench::failed();
        line(r, format!("{fails} failed checks"));
        write_report("zoom", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

/// `zoom-check`: the next start, with 150% kept.
pub fn run_check() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }
    step!(1200, |st, app, r| {
        views::go(st, Page::Home);
        let sf = app.window().scale_factor();
        MONITOR.with(|m| m.set(sf / crate::zoom::factor()));
        line(r, format!("monitor scale factor {:.3}, window {:?}", monitor(), app.window().size()));
        zoom_is(r, st, app, "restored: 150%", 150);
        let started = STARTED.with(std::cell::Cell::get);
        check(r, "applied as the window came up", started.is_some_and(|d| d < Duration::from_secs(3)), format!("{started:?} after the harness started"));
        layout_fits(r, app, "150% restored");
    });
    step!(800, |st, app, r| {
        snapshot(app, "zoom-restored-150");
        crate::zoom::set(st, 100, true);
        zoom_is(r, st, app, "back to 100%", 100);
    });
    step!(1200, |_st, app, r| {
        layout_fits(r, app, "100% again");
        let fails = crate::bench::failed();
        line(r, format!("{fails} failed checks"));
        write_report("zoom-check", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

thread_local! {
    /// How long after the harness started the restored zoom was first seen applied.
    static STARTED: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// From the harness's start (before the scan): watch for the saved zoom to apply.
pub fn watch_start() {
    let t0 = std::time::Instant::now();
    let timer = Rc::new(slint::Timer::default());
    let t = timer.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(10), move || {
        let seen = state::with(|st| {
            let want = st.settings.borrow().zoom;
            want != 100 && crate::zoom::factor() == want as f32 / 100.0
        })
        .unwrap_or(false);
        if seen || t0.elapsed() > Duration::from_secs(10) {
            if seen {
                STARTED.with(|s| s.set(Some(t0.elapsed())));
            }
            t.stop();
        }
    });
    std::mem::forget(timer);
}
