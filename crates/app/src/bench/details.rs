//! `MP_BENCH=details`: technical details and "New playlist…" (`bench-details.txt`).
//!
//! With the details setting on: every view that shows them, snapshotted and reported:
//! album pages (uniform, hi-res, VBR, mixed with and without a main format, surround),
//! now playing and the player bar for each format of the mixed album (shared mode only:
//! exclusive mode is never opened here), the artist's top tracks, search and its "See
//! all" page, a playlist, Settings > Output and > Window, "View metadata", and the
//! minimum window size with and without the sidebar pinned (where the column steps
//! aside). Then the name prompt of "New playlist…": with no playlists yet, from a track
//! and from an album by the pointer, from a track list by the keyboard (Menu, the arrows,
//! Right, Enter), cancelled with Esc, an empty name, a name in use (and adding to it),
//! typing that must not reach the shortcuts, where the focus goes after it, and a drop on
//! the sidebar's Playlists heading.
//!
//! `MP_BENCH_DETAILS=off`: the same views with the setting off (`off-*` snapshots), to
//! compare with a build without the feature. Needs `MP_MUSIC` with the format fixtures.

use std::cell::RefCell;
use std::rc::Rc;

use library::query;
use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, menu_labels, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::state::State;
use crate::{AlbumPage, AppWindow, Dialog, MenuStore, Nav, Output, Page, Player, playback, settings, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("details bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn album_id(st: &State, title: &str) -> Option<i64> {
    st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok()
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

/// The album list's rows as shown: title and details.
fn album_rows(st: &State) -> Vec<String> {
    let m = &st.lists.album;
    (0..m.row_count()).filter_map(|i| m.row_data(i)).map(|r| format!("{} [{}]", r.title, r.details)).collect()
}

fn playlist_names(st: &State) -> Vec<(String, i64)> {
    query::playlists(&st.db).unwrap_or_default().into_iter().map(|p| (p.name, p.tracks)).collect()
}

fn dialog_state(app: &AppWindow) -> String {
    let d = app.global::<Dialog>();
    format!(
        "prompt open {}, text {:?}, note {:?}, button {:?}",
        d.get_prompt_open(),
        d.get_prompt_text().to_string(),
        d.get_prompt_note().to_string(),
        d.get_prompt_confirm().to_string()
    )
}

/// The entry index of "Add to playlist" in the open menu.
fn add_to_playlist_index(app: &AppWindow) -> i32 {
    menu_labels(app).iter().position(|l| l == "Add to playlist").map_or(-1, |i| i as i32)
}

fn sub_labels(app: &AppWindow) -> Vec<String> {
    let m = app.global::<MenuStore>();
    (0..m.get_sub().row_count()).filter_map(|i| m.get_sub().row_data(i)).map(|i| if i.separator { "-".into() } else { i.label.to_string() }).collect()
}

const ALBUMS: [&str; 9] = ["Lossless 16", "Hi-Res", "MP3 CBR", "MP3 VBR", "Mixed Bag", "Mostly FLAC", "Channels", "PCM Album", "ミックス盤"];

pub fn run() {
    let on = std::env::var("MP_BENCH_DETAILS").as_deref() != Ok("off");
    let p: &'static str = if on { "d" } else { "off" };
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }

    step!(500, move |st, app, r| {
        if st.settings.borrow().show_details != on {
            crate::metadata::details::toggle(st);
        }
        line(r, format!("setting on: {}, Player.show-details {}", st.settings.borrow().show_details, app.global::<Player>().get_show_details()));
        line(r, format!("library: {:?}", query::counts(&st.db)));
        let missing: i64 = st.db.query_row("SELECT COUNT(*) FROM tracks WHERE codec IS NULL", [], |r| r.get(0)).unwrap_or(-1);
        line(r, format!("tracks without details: {missing}"));
        let s = app.window().size().to_logical(app.window().scale_factor());
        line(r, format!("window {}x{}", s.width, s.height));
    });

    // ---- the no-playlists submenu, before anything makes one ------------------------
    if on {
        step!(300, |st, app, r| {
            let a = album_id(st, "Lossless 16").unwrap_or(0);
            line(r, format!("playlists at start: {:?}", playlist_names(st)));
            crate::menus::request(st, "album", "", 0, a as i32, 500.0, 300.0);
            let i = add_to_playlist_index(app);
            let m = app.global::<MenuStore>();
            m.set_highlight(i);
            m.set_sub_of(i);
            line(r, format!("[no playlists] album menu {:?}; submenu {:?}", menu_labels(app), sub_labels(app)));
        });
        step!(500, move |_st, app, _r| {
            snapshot(app, "d-menu-00-no-playlists");
            app.global::<MenuStore>().set_open(false);
        });
    }

    // ---- albums -------------------------------------------------------------------------
    for title in ALBUMS {
        step!(300, move |st, _app, _r| {
            if let Some(id) = album_id(st, title) {
                views::open_album(st, id);
            }
        });
        step!(900, move |st, app, r| {
            let page = app.global::<AlbumPage>();
            line(r, format!("[album {title}] summary {:?}, column {}; rows {:?}", page.get_details().to_string(), page.get_details_column(), album_rows(st)));
            snapshot(app, &format!("{p}-album-{}", slug(title)));
        });
    }

    // ---- now playing, each format of the mixed album ------------------------------------
    for i in 0..6 {
        step!(300, move |st, _app, _r| {
            if let Some(id) = album_id(st, "Mixed Bag") {
                playback::play_tracks(st, query::album_tracks(&st.db, id).unwrap_or_default(), i);
            }
        });
        step!(1800, move |st, app, r| {
            views::go(st, Page::NowPlaying);
            let pl = app.global::<Player>();
            line(
                r,
                format!(
                    "[now playing {}] bar {:?} | full {:?} | spoken {:?} | output {:?} | Settings {:?}",
                    pl.get_title(),
                    pl.get_details_short().to_string(),
                    pl.get_details_full().to_string(),
                    pl.get_details_spoken().to_string(),
                    pl.get_details_output().to_string(),
                    app.global::<Output>().get_stream().to_string()
                ),
            );
        });
        step!(900, move |_st, app, _r| {
            snapshot(app, &format!("{p}-np-{i}"));
        });
    }
    // Surround, mono, and a file from outside the library (details read on the fly).
    for (album, i, name) in [("Channels", 1usize, "surround"), ("Channels", 0, "mono"), ("Hi-Res", 0, "hires")] {
        step!(300, move |st, _app, _r| {
            if let Some(id) = album_id(st, album) {
                playback::play_tracks(st, query::album_tracks(&st.db, id).unwrap_or_default(), i);
            }
        });
        step!(1800, move |_st, app, r| {
            let pl = app.global::<Player>();
            line(r, format!("[now playing {name}] bar {:?} | full {:?} | output {:?}", pl.get_details_short().to_string(), pl.get_details_full().to_string(), pl.get_details_output().to_string()));
            snapshot(app, &format!("{p}-np-{name}"));
        });
    }
    step!(300, |st, _app, r| {
        let path: Option<String> = st.db.query_row("SELECT path FROM tracks WHERE path LIKE '%Opus.opus' LIMIT 1", [], |r| r.get(0)).ok();
        if let Some(path) = path {
            let t = crate::opener::outside_track(std::path::Path::new(&path), -7);
            line(r, format!("[outside] read on the fly: {:?}", t.audio));
            playback::play_tracks(st, vec![t], 0);
        }
    });
    step!(1800, move |_st, app, r| {
        let pl = app.global::<Player>();
        line(r, format!("[now playing outside] bar {:?} | full {:?} | output {:?}", pl.get_details_short().to_string(), pl.get_details_full().to_string(), pl.get_details_output().to_string()));
        snapshot(app, &format!("{p}-np-outside"));
    });
    // ReplayGain off: the adjustment leaves the line.
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Mixed Bag") {
            playback::play_tracks(st, query::album_tracks(&st.db, id).unwrap_or_default(), 1);
        }
    });
    step!(1500, |st, app, r| {
        let before = app.global::<Player>().get_details_output().to_string();
        st.settings.borrow_mut().gain = crate::settings::GainSetting::Off;
        playback::apply_gain(st);
        let after = app.global::<Player>().get_details_output().to_string();
        st.settings.borrow_mut().gain = crate::settings::GainSetting::Track;
        playback::apply_gain(st);
        line(r, format!("[gain] ReplayGain track: {before:?}; off: {after:?}"));
        playback::set_paused(st, true);
    });

    // ---- other lists -------------------------------------------------------------------
    step!(300, |st, _app, _r| views::open_artist(st, "Format Tester"));
    step!(1000, move |_st, app, _r| {
        snapshot(app, &format!("{p}-artist"));
    });
    step!(300, |st, _app, _r| {
        views::go(st, Page::Search);
        views::search(st, "mix");
    });
    step!(1500, move |_st, app, _r| {
        snapshot(app, &format!("{p}-search"));
    });
    step!(300, |st, _app, _r| views::open_search_all(st, "tracks"));
    step!(1200, move |_st, app, _r| {
        snapshot(app, &format!("{p}-tracks-page"));
    });
    step!(300, move |st, _app, r| {
        let mut ids = Vec::new();
        for a in ["Mixed Bag", "Mostly FLAC", "Hi-Res"] {
            if let Some(id) = album_id(st, a) {
                ids.extend(query::album_tracks(&st.db, id).unwrap_or_default().iter().map(|t| t.id));
            }
        }
        let name = if on { "Bench mixed" } else { "Bench mixed (off)" };
        let pl = query::create_playlist_with(&st.db, name, &ids).unwrap();
        views::refresh_playlists(st);
        views::open_playlist(st, pl);
        line(r, format!("[playlist] {} tracks", ids.len()));
    });
    step!(1200, move |_st, app, _r| {
        snapshot(app, &format!("{p}-playlist"));
    });
    step!(300, |st, app, _r| {
        app.global::<Nav>().set_queue_open(true);
        let _ = st;
    });
    step!(900, move |_st, app, _r| {
        snapshot(app, &format!("{p}-queue"));
        app.global::<Nav>().set_queue_open(false);
    });
    step!(300, |st, _app, _r| {
        views::go(st, Page::Settings);
        views::settings_section(st, settings::nav::OUTPUT);
    });
    step!(900, move |_st, app, r| {
        line(r, format!("[settings] Output status {:?} / {:?}", app.global::<Output>().get_status().to_string(), app.global::<Output>().get_stream().to_string()));
        snapshot(app, &format!("{p}-settings-output"));
    });
    step!(300, |st, _app, _r| views::settings_section(st, settings::nav::WINDOW));
    step!(900, move |_st, app, _r| {
        snapshot(app, &format!("{p}-settings-window"));
    });

    // ---- View metadata's File rows (what Properties showed; whatever the setting) --------
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Mixed Bag") {
            views::open_album(st, id);
        }
    });
    step!(900, |st, app, r| {
        crate::menus::request(st, "track", "album", 1, 0, 700.0, 420.0);
        let labels = menu_labels(app);
        line(r, format!("[properties] track menu {labels:?}"));
        if let Some(i) = labels.iter().position(|l| l == "View metadata") {
            crate::menus::activate(st, i as i32, -1);
        }
    });
    step!(1000, move |_st, app, r| {
        let d = app.global::<Dialog>();
        let lines = crate::metadata::view::lines();
        let file = lines.iter().position(|l| l.kind == crate::metadata::view::Kind::Heading && l.label == "File").unwrap_or(lines.len());
        let rows: Vec<String> = lines[file..].iter().skip(1).take_while(|l| l.kind != crate::metadata::view::Kind::Heading).map(|l| format!("{}: {}", l.label, l.values.join(" / "))).collect();
        line(r, format!("[properties] open {}, {:?}, file rows {rows:?}", d.get_meta_open(), app.global::<crate::Meta>().get_title().to_string()));
        snapshot(app, &format!("{p}-properties"));
        press(app, Key::Tab, &[]);
        press(app, Key::Tab, &[]);
    });
    step!(400, |st, app, r| {
        let muted = app.global::<Player>().get_muted();
        type_text(app, "m");
        line(r, format!("[properties] m typed with the panel open: muted {} -> {} (the panel keeps the keys)", muted, app.global::<Player>().get_muted()));
        press(app, Key::Escape, &[]);
        line(r, format!("[properties] Esc: open {}", app.global::<Dialog>().get_meta_open()));
        let _ = st;
    });

    // ---- the minimum window size --------------------------------------------------------
    step!(300, |st, _app, _r| crate::platform::devwindow::resize_client(st.hwnd.get(), 960, 620));
    step!(1200, |st, app, r| {
        let s = app.window().size().to_logical(app.window().scale_factor());
        line(r, format!("[min] window {}x{}, page width {}", s.width, s.height, app.global::<Nav>().get_content_width()));
        if let Some(id) = album_id(st, "Mixed Bag") {
            views::open_album(st, id);
        }
    });
    step!(900, move |_st, app, _r| {
        snapshot(app, &format!("{p}-min-album-mixed"));
        app.global::<Nav>().set_sidebar_pinned(true);
    });
    step!(900, move |_st, app, r| {
        line(r, format!("[min] sidebar pinned: page width {} (the column steps aside under 800)", app.global::<Nav>().get_content_width()));
        snapshot(app, &format!("{p}-min-album-mixed-pinned"));
        app.global::<Nav>().set_sidebar_pinned(false);
    });
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Hi-Res") {
            views::open_album(st, id);
        }
    });
    step!(900, move |_st, app, _r| {
        snapshot(app, &format!("{p}-min-album-hires"));
    });
    step!(300, |st, _app, _r| {
        if let Some(id) = album_id(st, "Channels") {
            playback::play_tracks(st, query::album_tracks(&st.db, id).unwrap_or_default(), 1);
        }
        views::go(st, Page::NowPlaying);
    });
    step!(1800, move |st, app, _r| {
        snapshot(app, &format!("{p}-min-np"));
        playback::set_paused(st, true);
        let _ = st;
    });
    step!(300, |st, _app, _r| views::open_search_all(st, "tracks"));
    step!(900, move |_st, app, _r| {
        snapshot(app, &format!("{p}-min-tracks-page"));
    });
    step!(300, |st, _app, _r| crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void));
    step!(1200, |st, _app, _r| {
        if let Some(id) = album_id(st, "Lossless 16") {
            views::open_album(st, id);
        }
    });

    if on {
        prompt_steps(&mut steps, &r);
    }

    step!(500, move |_st, _app, r| {
        write_report(if on { "details" } else { "details-off" }, &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}

/// The name prompt of "New playlist…", every way in and out.
fn prompt_steps(steps: &mut Vec<Step>, r: &Report) {
    macro_rules! step {
        ($ms:expr, $f:expr) => {{
            let r = r.clone();
            let f: Box<dyn FnOnce(&Rc<State>, &AppWindow, &Report)> = Box::new($f);
            steps.push(($ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
        }};
    }
    // From a track, by the pointer: the menu, Add to playlist, New playlist…
    step!(600, |st, app, r| {
        crate::menus::request(st, "track", "album", 0, 0, 640.0, 430.0);
        let i = add_to_playlist_index(app);
        let m = app.global::<MenuStore>();
        m.set_highlight(i);
        m.set_sub_of(i);
        line(r, format!("[pointer, track] submenu {:?}", sub_labels(app)));
    });
    step!(500, |st, app, _r| {
        snapshot(app, "d-menu-01-submenu");
        let i = add_to_playlist_index(app);
        crate::menus::activate(st, i, 0);
    });
    step!(600, |_st, app, r| {
        line(r, format!("[pointer, track] New playlist…: {}", dialog_state(app)));
        snapshot(app, "d-prompt-01-track-empty");
        // Enter on the empty name: refused, in the prompt.
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[empty name] Enter: {}; playlists {:?}", dialog_state(app), playlist_names(st)));
        snapshot(app, "d-prompt-02-empty-refused");
        // Typing in the field: no shortcut acts (m mutes, space plays, n opens now playing).
        let (muted, page) = (app.global::<Player>().get_muted(), app.global::<Nav>().get_page());
        type_text(app, "Night drive mn");
        line(
            r,
            format!(
                "[typing] \"Night drive mn\": text {:?}; muted {muted} -> {}, page {page:?} -> {:?}",
                app.global::<Dialog>().get_prompt_text().to_string(),
                app.global::<Player>().get_muted(),
                app.global::<Nav>().get_page()
            ),
        );
        for _ in 0..3 {
            press(app, Key::Backspace, &[]);
        }
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[pointer, track] Enter: {}; flash {:?}; playlists {:?}", dialog_state(app), app.global::<Nav>().get_flash().to_string(), playlist_names(st)));
        let sidebar: Vec<String> = app.global::<crate::Playlists>().get_list().iter().map(|p| p.name.to_string()).collect();
        line(r, format!("[pointer, track] sidebar playlists {sidebar:?}"));
        app.global::<Nav>().set_sidebar_forced(true);
    });
    step!(500, |_st, app, _r| {
        snapshot(app, "d-prompt-03-created-sidebar");
        app.global::<Nav>().set_sidebar_forced(false);
    });
    // From an album, by the pointer: the album's title, selected; Esc cancels.
    step!(500, |st, app, r| {
        let a = album_id(st, "Mostly FLAC").unwrap_or(0);
        crate::menus::request(st, "album", "", 0, a as i32, 300.0, 260.0);
        let i = add_to_playlist_index(app);
        line(r, format!("[pointer, album] submenu {:?}", { let m = app.global::<MenuStore>(); m.set_sub_of(i); sub_labels(app) }));
        crate::menus::activate(st, i, 0);
    });
    step!(600, |st, app, r| {
        line(r, format!("[pointer, album] {}", dialog_state(app)));
        snapshot(app, "d-prompt-04-album-prefilled");
        press(app, Key::Escape, &[]);
        line(r, format!("[pointer, album] Esc: {}; playlists {:?}", dialog_state(app), playlist_names(st)));
    });
    step!(400, |_st, app, r| {
        // The window's shortcuts have the keys again: M mutes and unmutes.
        let a = app.global::<Player>().get_muted();
        type_text(app, "m");
        let b = app.global::<Player>().get_muted();
        type_text(app, "m");
        line(r, format!("[pointer, album] after Esc, M: muted {a} -> {b} (the shortcuts have the keys back)"));
    });
    // A name in use: offered to add to it; Enter again adds.
    step!(400, |st, app, r| {
        let _ = query::create_playlist(&st.db, "Road Trip");
        views::refresh_playlists(st);
        let a = album_id(st, "Hi-Res").unwrap_or(0);
        crate::menus::request(st, "album", "", 0, a as i32, 900.0, 300.0);
        let i = add_to_playlist_index(app);
        crate::menus::activate(st, i, 0);
        line(r, format!("[duplicate] playlists {:?}", playlist_names(st)));
    });
    step!(500, |_st, app, _r| {
        type_text(app, "road  TRIP");
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[duplicate] \"road  TRIP\", Enter: {}; playlists {:?}", dialog_state(app), playlist_names(st)));
        snapshot(app, "d-prompt-05-duplicate");
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[duplicate] Enter again: {}; flash {:?}; playlists {:?}", dialog_state(app), app.global::<Nav>().get_flash().to_string(), playlist_names(st)));
        // Editing the name after the offer goes back to creating.
        let a = album_id(st, "Hi-Res").unwrap_or(0);
        crate::menus::request(st, "album", "", 0, a as i32, 900.0, 300.0);
        let i = add_to_playlist_index(app);
        crate::menus::activate(st, i, 0);
    });
    step!(500, |_st, app, _r| {
        type_text(app, "Road Trip");
        press(app, Key::Return, &[]);
    });
    step!(400, |st, app, r| {
        let offered = dialog_state(app);
        type_text(app, " 2");
        line(r, format!("[duplicate, edited] offered: {offered}; typed \" 2\": {}", dialog_state(app)));
        press(app, Key::Return, &[]);
        line(r, format!("[duplicate, edited] Enter: {}; playlists {:?}", dialog_state(app), playlist_names(st)));
    });
    // By the keyboard, from the album's track list: Tab to it, Menu, Down to Add to
    // playlist, Right, Enter on New playlist…, a name, Enter; the list has the focus after.
    step!(500, |st, app, _r| {
        if let Some(id) = album_id(st, "Mixed Bag") {
            views::open_album(st, id);
        }
        let _ = app;
    });
    step!(900, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        // The album page: the artist link, five buttons, the heart, the album's menu, then the list.
        tabs(app, 8);
        press(app, Key::DownArrow, &[]);
        press(app, Key::Menu, &[]);
    });
    step!(500, |_st, app, r| {
        let m = app.global::<MenuStore>();
        line(r, format!("[keys] Menu: open {}, from keys {}, entries {:?}", m.get_open(), m.get_from_keys(), menu_labels(app)));
        for _ in 0..3 {
            press(app, Key::DownArrow, &[]);
        }
        press(app, Key::RightArrow, &[]);
        line(r, format!("[keys] Down x3, Right: highlight {}, submenu of {}, sub highlight {} ({:?})", m.get_highlight(), m.get_sub_of(), m.get_sub_highlight(), sub_labels(app).get(m.get_sub_highlight().max(0) as usize)));
    });
    step!(400, |_st, app, _r| {
        snapshot(app, "d-keys-01-submenu");
        press(app, Key::Return, &[]);
    });
    step!(600, |_st, app, r| {
        line(r, format!("[keys] Enter: {}", dialog_state(app)));
        snapshot(app, "d-keys-02-prompt");
        type_text(app, "Keyboard mix");
        // Tab goes round the prompt's own controls, never to the page behind.
        press(app, Key::Tab, &[]);
        press(app, Key::Tab, &[]);
        press(app, Key::Tab, &[]);
        press(app, Key::Return, &[]);
    });
    step!(600, |st, app, r| {
        line(r, format!("[keys] typed, Tab x3 (back to the field), Enter: {}; flash {:?}; playlists {:?}", dialog_state(app), app.global::<Nav>().get_flash().to_string(), playlist_names(st)));
        let tracks: Vec<String> = playlist_names(st).iter().filter(|(n, _)| n == "Keyboard mix").map(|(n, c)| format!("{n}: {c}")).collect();
        line(r, format!("[keys] {tracks:?}"));
        // The list has the focus again: the Menu key opens its menu.
        press(app, Key::Menu, &[]);
        let m = app.global::<MenuStore>();
        line(r, format!("[keys] after the prompt, Menu: open {}, from keys {} (the track list had the focus back)", m.get_open(), m.get_from_keys()));
        press(app, Key::Escape, &[]);
    });
    // By the keyboard again, cancelled with Esc.
    step!(400, |_st, app, _r| {
        press(app, Key::Menu, &[]);
        for _ in 0..3 {
            press(app, Key::DownArrow, &[]);
        }
        press(app, Key::RightArrow, &[]);
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[keys, cancel] {}", dialog_state(app)));
        type_text(app, "Never made");
        press(app, Key::Escape, &[]);
        line(r, format!("[keys, cancel] Esc: {}; playlists {:?}", dialog_state(app), playlist_names(st)));
        press(app, Key::Menu, &[]);
        let m = app.global::<MenuStore>();
        line(r, format!("[keys, cancel] Menu: open {}, from keys {}", m.get_open(), m.get_from_keys()));
        press(app, Key::Escape, &[]);
    });
    // A click outside cancels.
    step!(400, |st, app, _r| {
        crate::menus::request(st, "track", "album", 2, 0, 640.0, 430.0);
        let i = add_to_playlist_index(app);
        crate::menus::activate(st, i, 0);
    });
    step!(500, |st, app, r| {
        use slint::platform::{PointerEventButton, WindowEvent};
        let pos = slint::LogicalPosition::new(30.0, 900.0);
        app.window().dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Left });
        app.window().dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Left });
        line(r, format!("[click outside] {}; playlists {:?}", dialog_state(app), playlist_names(st)));
    });
    // A drop on the sidebar's Playlists heading (the call the drop makes).
    step!(400, |st, app, _r| {
        app.global::<Nav>().set_sidebar_forced(true);
        let a = album_id(st, "Channels").unwrap_or(0);
        crate::playlists::newplaylist::open_from_drop(st, crate::dnd::Payload::Album(a), 120.0, 470.0);
    });
    step!(600, |_st, app, r| {
        line(r, format!("[drop] {}", dialog_state(app)));
        snapshot(app, "d-prompt-06-drop");
        press(app, Key::Return, &[]);
    });
    step!(500, |st, app, r| {
        line(r, format!("[drop] Enter: flash {:?}; playlists {:?}", app.global::<Nav>().get_flash().to_string(), playlist_names(st)));
        snapshot(app, "d-prompt-07-drop-created");
        app.global::<Nav>().set_sidebar_forced(false);
    });
}
