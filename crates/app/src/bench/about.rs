//! `MP_BENCH=about`: Settings > About, checked and snapshotted (`bench-about.txt`):
//!
//! - what it shows: the version (and that it is the build's own), the system line, every
//!   place with its path and measured size, the library's totals (against the database),
//!   the credits;
//! - the line Copy would put on the clipboard, built by the same code path, without
//!   touching the clipboard;
//! - measuring stops when the section is left before it is done, and runs again on return;
//! - the keyboard: Tab from the filter box through the list into the section; "Find a
//!   setting" finding About's rows;
//! - the tray's "About Marquee Player" (the same function the menu entry calls), and the
//!   tooltip with the version while nothing is loaded;
//! - snapshots at the window's size and at 960 x 620 (`platform::devwindow::resize_client`, only
//!   under `MP_WINDOW=secondary`).
//!
//! Needs a music folder (`MP_MUSIC`) for the totals to mean something.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::nav::Route;
use crate::settings::nav::{self, SECTIONS};
use crate::state::State;
use crate::{AboutView, AppWindow, Nav, Page, SettingsView, about, views};

type Report = Rc<RefCell<String>>;

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
}

fn line(r: &Report, text: String) {
    log::info!("about bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn check(r: &Report, what: &str, ok: bool, detail: String) {
    if !ok {
        FAILS.with(|f| f.set(f.get() + 1));
        crate::bench::count_fail();
    }
    line(r, format!("  [{}] {what}: {detail}", if ok { "ok" } else { "FAIL" }));
}

fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

fn on_about(st: &Rc<State>) -> bool {
    st.history.borrow().current == Route::Settings(nav::ABOUT)
}

/// Everything the section shows, as text.
fn report_view(r: &Report, app: &AppWindow) {
    let v = app.global::<AboutView>();
    line(r, format!("  name {:?}, version {:?} (dev {}), system {:?}", v.get_name().to_string(), v.get_version().to_string(), v.get_dev(), v.get_system().to_string()));
    for f in v.get_files().iter() {
        line(r, format!("  {}: {} | {}", f.what, f.path, f.detail));
    }
    line(r, format!("  library: {} albums, {} tracks, {}, {} (measuring {})", v.get_albums(), v.get_tracks(), v.get_length(), v.get_size(), v.get_measuring()));
    let credits: Vec<String> = v.get_credits().iter().map(|c| format!("{} ({})", c.name, c.licence)).collect();
    line(r, format!("  credits ({}): {}", credits.len(), credits.join("; ")));
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

    // ---- what it shows -------------------------------------------------------------------
    step!(1500, |st, app, r| {
        let (w, h) = size(app);
        line(r, format!("window {w}x{h} logical, scale {}", app.window().scale_factor()));
        check(r, "About is the last section", SECTIONS[nav::ABOUT] == "About" && nav::ABOUT == SECTIONS.len() - 1, format!("{SECTIONS:?}"));
        let tip = crate::tray_tooltip();
        check(r, "the tray tooltip names the version while nothing is loaded", tip.as_deref() == Some(&format!("Marquee Player {}", about::VERSION)), format!("{tip:?}"));
        views::go(st, Page::Settings);
        views::settings_section(st, nav::ABOUT);
        let v = app.global::<AboutView>();
        line(r, format!("[open] measuring at once: {}", v.get_measuring()));
    });
    step!(1500, |st, app, r| {
        let v = app.global::<AboutView>();
        line(r, "[shown]".into());
        report_view(r, app);
        check(r, "measured within 1.5 s", !v.get_measuring(), String::new());
        check(r, "the version is the build's", v.get_version() == about::VERSION && v.get_dev() == about::is_dev(), about::VERSION.into());
        let (albums, tracks) = library::query::counts(&st.db).unwrap_or_default();
        check(r, "albums and tracks as the database counts them", v.get_albums() == library::audio::thousands(albums as u64) && v.get_tracks() == library::audio::thousands(tracks as u64), format!("{albums} albums, {tracks} tracks"));
        let files: Vec<_> = v.get_files().iter().collect();
        check(r, "six places, each with a path and a size", files.len() == 6 && files.iter().all(|f| !f.path.is_empty() && !f.detail.contains("Measuring")), format!("{}", files.len()));
        // MP_DATA_DIR, or the portable edition's Data folder: one folder for everything.
        let data = st.paths.config.display().to_string();
        // Shown with a zero-width space after each separator (about::breakable_path).
        check(r, "every path is under the data folder", !data.is_empty() && files.iter().all(|f| f.path.replace('\u{200b}', "").starts_with(data.as_str())), data);
        let rep = about::this_report_line();
        line(r, format!("[copy] the line Copy puts on the clipboard (not copied here): {rep}"));
        check(r, "it names the version, the system and the renderer", rep.contains(about::VERSION) && rep.contains(about::os_version()) && rep.contains(about::renderer()), about::renderer().into());
        snapshot(app, "about-01-top");
        app.global::<Nav>().set_page_scroll(-560.0);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "about-02-library");
        app.global::<Nav>().set_page_scroll(-1150.0);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "about-03-built-with");
        // The end of the column at 1920 x 1032 (a scroll set from here is not clamped).
        app.global::<Nav>().set_page_scroll(-2100.0);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "about-04-end");
    });

    // ---- measuring stops when left -------------------------------------------------------
    step!(300, |st, app, r| {
        views::settings_section(st, nav::LIBRARY);
        views::settings_section(st, nav::ABOUT);
        // Left at once for another page: whatever was being measured is dropped.
        views::go(st, Page::Home);
        let v = app.global::<AboutView>();
        line(r, format!("[leave] About shown and left at once: measuring {}", v.get_measuring()));
    });
    step!(1200, |st, app, r| {
        let v = app.global::<AboutView>();
        check(r, "a measurement for About left before it finished is dropped", v.get_measuring(), format!("measuring {}", v.get_measuring()));
        views::back(st);
    });
    step!(1500, |st, app, r| {
        let v = app.global::<AboutView>();
        check(r, "Back to About measures again", on_about(st) && !v.get_measuring(), format!("route {:?}, measuring {}", st.history.borrow().current, v.get_measuring()));
    });

    // ---- the tray's About -------------------------------------------------------------------
    step!(300, |st, _app, _r| views::go(st, Page::Albums));
    step!(800, |st, app, r| {
        crate::show_about(app);
        check(r, "the tray's About opens Settings > About", on_about(st) && app.global::<Nav>().get_page() == Page::Settings, format!("{:?}", st.history.borrow().current));
    });

    // ---- keyboard -------------------------------------------------------------------------
    step!(1200, |_st, app, _r| {
        app.global::<Nav>().set_page_scroll(0.0);
        app.invoke_focus_shortcuts();
        // The filter box, the list, then the section: Version's "More", Copy, the first
        // "Open folder".
        tabs(app, 2);
    });
    step!(400, |_st, app, r| {
        snapshot(app, "about-key-01-list");
        line(r, "[keys] Tab 2: the section list".into());
        tabs(app, 1);
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "about-key-02-more");
        tabs(app, 1);
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "about-key-03-copy");
        tabs(app, 1);
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "about-key-04-open-settings");
        tabs(app, 5);
    });
    step!(500, |_st, app, r| {
        line(r, format!("[keys] Tab 5 more: the last Open folder (Log), scroll {}", app.global::<Nav>().get_page_scroll()));
        snapshot(app, "about-key-05-open-log");
        press(app, Key::Tab, &[Key::Shift]);
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "about-key-06-shift-tab");
    });
    step!(300, |st, app, r| {
        // Up from About in the list goes to Keys; End comes back.
        app.invoke_focus_shortcuts();
        tabs(app, 2);
        press(app, Key::UpArrow, &[]);
        let up = app.global::<SettingsView>().get_section();
        press(app, Key::End, &[]);
        check(r, "Up goes to Keys, End to About", up == nav::KEYS as i32 && on_about(st), format!("up {up}, then {:?}", st.history.borrow().current));
    });

    // ---- find a setting ------------------------------------------------------------------
    step!(400, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        tabs(app, 1);
        type_text(app, "version");
    });
    step!(400, |_st, app, r| {
        let hits: Vec<String> = app.global::<SettingsView>().get_hits().iter().map(|h| format!("{} ({})", h.label, h.section_name)).collect();
        check(r, "\"version\" finds About > Version first", hits.first().map(String::as_str) == Some("Version (About)"), format!("{hits:?}"));
        snapshot(app, "about-find-01-version");
        press(app, Key::Return, &[]);
    });
    step!(300, |st, app, r| {
        let h = app.global::<SettingsView>().get_highlight();
        check(r, "Enter goes to it, outlined", on_about(st) && h == "Version", format!("highlight {h:?}"));
        snapshot(app, "about-find-02-version-outlined");
    });
    step!(2600, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        tabs(app, 1);
        // "disk space" finds Undo history first since tag editing came (it shows and clears what
        // the journal takes): a word only the database row has.
        type_text(app, "sqlite");
    });
    step!(400, |_st, app, r| {
        let hits: Vec<String> = app.global::<SettingsView>().get_hits().iter().map(|h| h.label.to_string()).collect();
        line(r, format!("[find] \"sqlite\": {hits:?}"));
        snapshot(app, "about-find-03-sqlite");
        press(app, Key::Return, &[]);
    });
    step!(600, |st, app, r| {
        check(r, "Enter goes to the database row", on_about(st) && app.global::<SettingsView>().get_highlight() == "Library database", String::new());
        snapshot(app, "about-find-04-database-outlined");
    });

    // ---- 960 x 620 --------------------------------------------------------------------------
    step!(2600, |st, _app, _r| {
        // The window's frame takes 8 px each side and below (frame.rs).
        crate::platform::devwindow::resize_client(st.hwnd.get(), 960, 620);
    });
    step!(1200, |st, app, r| {
        let (w, h) = size(app);
        line(r, format!("[size] asked 960x620: window now {w}x{h} logical"));
        views::settings_section(st, nav::LIBRARY);
        views::settings_section(st, nav::ABOUT);
    });
    for (i, scroll) in [0.0f32, -450.0, -900.0, -1350.0, -1800.0, -2250.0, -2700.0].into_iter().enumerate() {
        step!(700, move |_st, app, _r| {
            app.global::<Nav>().set_page_scroll(scroll);
        });
        step!(400, move |_st, app, _r| {
            snapshot(app, &format!("about-min-{:02}", i + 1));
        });
    }
    step!(300, |st, app, _r| {
        let nav = app.global::<Nav>();
        nav.set_sidebar_pinned(true);
        nav.set_queue_pinned(true);
        views::settings_section(st, nav::ABOUT);
    });
    step!(900, |_st, app, _r| {
        snapshot(app, "about-min-pinned");
        app.global::<Nav>().set_page_scroll(-700.0);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "about-min-pinned-files");
        let nav = app.global::<Nav>();
        nav.set_sidebar_pinned(false);
        nav.set_queue_pinned(false);
    });
    step!(300, |st, _app, _r| {
        crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void);
    });
    step!(1200, |_st, app, r| {
        let (w, h) = size(app);
        line(r, format!("[size] maximized again: {w}x{h}"));
        let fails = FAILS.with(Cell::get);
        line(r, if fails == 0 { "ALL OK".into() } else { format!("{fails} FAILED") });
        write_report("about", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
