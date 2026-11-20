//! `MP_BENCH=settings`: the Settings page, each part snapshotted and reported
//! (`bench-settings.txt`):
//!
//! - every section at the window's size (`set-NN-<section>`);
//! - the states the section list marks: missing tracks held back (Library), Last.fm needing
//!   a reconnect, Discord connected and refusing, the chosen output device away (shown through the same calls the app
//!   makes, nothing is removed or sent);
//! - the keyboard: Tab to the filter box, the section list and into a section; the
//!   arrows moving through the list; a switch and a segmented control from the keys;
//! - "Find a setting": a query, its results, Enter going to the first;
//! - history: a section left for another page comes back on Back, and opening Settings
//!   again shows the section last shown;
//! - the sidebar and the queue pinned, the player bar hidden, and other window sizes (the
//!   minimum, 960 x 620, with and without pins; a wide 2560 x 1032), the window resized
//!   without activation (`platform::devwindow::resize_client`, only under `MP_WINDOW=secondary`).
//!
//! Needs a music folder (`MP_MUSIC`), ideally with an unreadable file in it.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, press, run_steps, snapshot, tabs, type_text, write_report};
use crate::nav::Route;
use crate::settings::nav::{self, SECTIONS};
use crate::state::State;
use crate::{AppWindow, Nav, Page, SettingsView, playback, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("settings bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn size(app: &AppWindow) -> (f32, f32) {
    let s = app.window().size().to_logical(app.window().scale_factor());
    (s.width, s.height)
}

fn slug(i: usize) -> String {
    SECTIONS[i].to_lowercase().replace(" & ", "-").replace('.', "").replace(' ', "-")
}

fn where_(st: &Rc<State>, app: &AppWindow) -> String {
    let v = app.global::<SettingsView>();
    format!(
        "page {:?}, route {:?}, section {} ({}), scroll {}, filter {:?}",
        app.global::<Nav>().get_page(),
        st.history.borrow().current,
        v.get_section(),
        SECTIONS.get(v.get_section().max(0) as usize).unwrap_or(&"?"),
        app.global::<Nav>().get_page_scroll(),
        v.get_filter().to_string()
    )
}

/// Snapshot every section, prefixed `prefix`.
fn all_sections(steps: &mut Vec<Step>, r: &Report, prefix: &'static str) {
    for i in 0..SECTIONS.len() {
        steps.push((300, Box::new(move |st, _app| views::settings_section(st, i))));
        let r = r.clone();
        steps.push((500, Box::new(move |st, app| {
            let name = format!("{prefix}-{i:02}-{}", slug(i));
            line(&r, format!("[{prefix}] {name}: {}", where_(st, app)));
            snapshot(app, &name);
        })));
    }
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

    step!(800, |st, app, r| {
        let (w, h) = size(app);
        line(r, format!("window {w}x{h} logical, scale {}", app.window().scale_factor()));
        line(r, format!("library: {:?}", library::query::counts(&st.db)));
        views::go(st, Page::Settings);
    });
    step!(1200, |st, app, r| {
        line(r, format!("opened: {}", where_(st, app)));
    });
    all_sections(&mut steps, &r, "set");

    // ---- states the list marks -------------------------------------------------------
    step!(300, |st, app, r| {
        views::settings_section(st, nav::LIBRARY);
        app.global::<crate::Library>().set_held(
            "212 of 230 tracks in M:\\Music were not found. They stay in the library (with their plays, favourites and playlists) until they are back or you remove them."
                .into(),
        );
        crate::connections::scrobble::show(st, &lastfm::Status { state: lastfm::State::Reconnect, queued: 3, note: String::new(), accepting: true });
        crate::connections::listens::show(st, &listenbrainz::Status { state: listenbrainz::State::Reconnect, queued: 2, note: String::new(), accepting: true, refused: String::new() });
        let o = app.global::<crate::Output>();
        o.set_attention(true);
        o.set_status("USB DAC is not connected: playing on the system default (Speakers) until it is back.".into());
        line(r, format!("[marks] held, Last.fm reconnect, device away: Last.fm attention {}, ListenBrainz attention {}", app.global::<crate::LastFm>().get_attention(), app.global::<crate::ListenBrainz>().get_attention()));
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-01-library-held");
    });
    step!(300, |st, _app, _r| views::settings_section(st, nav::CONNECTIONS));
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-02-lastfm-reconnect");
    });
    step!(300, |st, _app, _r| views::settings_section(st, nav::OUTPUT));
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-03-output-away");
    });
    step!(300, |st, _app, _r| {
        crate::connections::scrobble::show(st, &lastfm::Status { state: lastfm::State::Connected { user: "alice".into() }, queued: 2, note: "Sent 3 scrobbles".into(), accepting: true });
        crate::connections::listens::show(st, &listenbrainz::Status { state: listenbrainz::State::Connected { user: "alice".into() }, queued: 0, note: "Sent 12 listens".into(), accepting: true, refused: String::new() });
        views::settings_section(st, nav::CONNECTIONS);
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-04-lastfm-connected");
    });
    // Discord, shown as on (the setting stays off: a bench run never opens its pipe).
    step!(300, |st, app, r| {
        let d = app.global::<crate::Discord>();
        d.set_enabled(true);
        d.set_app_id("1234567890123456789".into());
        d.set_applied_id("1234567890123456789".into());
        crate::connections::presence::show(st, &discord_ipc::State::Connected { user: "Alice".into() });
        line(r, format!("[marks] Discord connected: {:?}, attention {}", d.get_title().to_string(), d.get_attention()));
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-05-discord-connected");
    });
    step!(300, |st, app, r| {
        let d = app.global::<crate::Discord>();
        d.set_app_id("12345".into());
        crate::connections::presence::show(st, &discord_ipc::State::Refused("Discord does not know this application ID. Check it against the one on the application's page.".into()));
        line(r, format!("[marks] Discord refused: {:?}, attention {}", d.get_title().to_string(), d.get_attention()));
    });
    step!(600, |_st, app, _r| {
        snapshot(app, "mark-06-discord-refused");
    });
    step!(300, |st, app, r| {
        let d = app.global::<crate::Discord>();
        d.set_enabled(false);
        d.set_app_id("".into());
        d.set_applied_id("".into());
        crate::connections::presence::show(st, &discord_ipc::State::Off);
        app.global::<crate::Library>().set_held("".into());
        crate::connections::scrobble::show(st, &lastfm::Status { state: lastfm::State::NoCredentials, queued: 0, note: String::new(), accepting: false });
        crate::connections::listens::show(st, &listenbrainz::Status { state: listenbrainz::State::Disconnected, queued: 0, note: String::new(), accepting: false, refused: String::new() });
        crate::output::refresh(st);
        line(r, format!("[marks] cleared: Output attention {}, Last.fm attention {}", app.global::<crate::Output>().get_attention(), app.global::<crate::LastFm>().get_attention()));
        views::settings_section(st, nav::LIBRARY);
    });

    // ---- keyboard ----------------------------------------------------------------------
    step!(600, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        tabs(app, 1);
    });
    step!(400, |_st, app, r| {
        snapshot(app, "key-01-tab1-filter");
        type_text(app, "m");
        line(r, format!("[keys] Tab 1, typed m: filter {:?} (the field takes it), muted {}", app.global::<SettingsView>().get_filter().to_string(), app.global::<crate::Player>().get_muted()));
    });
    step!(400, |st, app, r| {
        press(app, Key::Escape, &[]);
        line(r, format!("[keys] Esc clears the filter: {}", where_(st, app)));
        tabs(app, 1);
    });
    step!(400, |st, app, r| {
        snapshot(app, "key-02-tab2-list");
        press(app, Key::DownArrow, &[]);
        press(app, Key::DownArrow, &[]);
        line(r, format!("[keys] Tab 2 (the list), Down Down: {}", where_(st, app)));
    });
    step!(500, |st, app, r| {
        snapshot(app, "key-03-list-down2");
        press(app, Key::End, &[]);
        let end = app.global::<SettingsView>().get_section();
        press(app, Key::Home, &[]);
        line(r, format!("[keys] End: section {end}; Home: {}", where_(st, app)));
        press(app, Key::DownArrow, &[]);
        // Into Playback: ReplayGain's "More", then its segmented control.
        tabs(app, 2);
    });
    step!(500, |st, app, r| {
        snapshot(app, "key-04-into-playback");
        let before = st.settings.borrow().gain;
        press(app, Key::RightArrow, &[]);
        let after = st.settings.borrow().gain;
        press(app, Key::LeftArrow, &[]);
        line(r, format!("[keys] ReplayGain from the keys: {before:?} -> Right -> {after:?} -> Left -> {:?}; player bar shows {:?}", st.settings.borrow().gain, app.global::<crate::Player>().get_gain_mode().to_string()));
        tabs(app, 1);
    });
    step!(400, |st, app, r| {
        let pre = st.settings.borrow().preamp_db;
        press(app, Key::UpArrow, &[]);
        let up = st.settings.borrow().preamp_db;
        press(app, Key::DownArrow, &[]);
        line(r, format!("[keys] Preamp stepper: {pre} -> Up -> {up} -> Down -> {}", st.settings.borrow().preamp_db));
        snapshot(app, "key-05-preamp-focused");
        tabs(app, 2);
    });
    step!(400, |st, app, r| {
        // Tab past the "More" link of Clipping protection to its switch.
        snapshot(app, "key-06-switch-focused");
        let before = st.settings.borrow().prevent_clipping;
        press(app, Key::Return, &[]);
        let after = st.settings.borrow().prevent_clipping;
        press(app, Key::Return, &[]);
        line(r, format!("[keys] Enter on the focused switch: clipping {before} -> {after} -> {}", st.settings.borrow().prevent_clipping));
    });
    // ---- filter ------------------------------------------------------------------------
    step!(300, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        tabs(app, 1);
        type_text(app, "exclusive");
    });
    step!(500, |st, app, r| {
        let hits: Vec<String> = app.global::<SettingsView>().get_hits().iter().map(|h| format!("{} ({})", h.label, h.section_name)).collect();
        line(r, format!("[filter] \"exclusive\": {hits:?}; {}", where_(st, app)));
        snapshot(app, "find-01-exclusive");
        press(app, Key::Return, &[]);
    });
    step!(300, |st, app, r| {
        line(r, format!("[filter] Enter: {}; highlight {:?}", where_(st, app), app.global::<SettingsView>().get_highlight().to_string()));
        snapshot(app, "find-02-jumped");
    });
    step!(300, |_st, app, _r| {
        app.invoke_focus_shortcuts();
        tabs(app, 1);
        type_text(app, "key");
    });
    step!(500, |_st, app, r| {
        line(r, format!("[filter] \"key\": {} results", app.global::<SettingsView>().get_hits().row_count()));
        snapshot(app, "find-03-key");
        type_text(app, "zzz");
    });
    step!(400, |_st, app, r| {
        line(r, format!("[filter] \"keyzzz\": {} results", app.global::<SettingsView>().get_hits().row_count()));
        snapshot(app, "find-04-nothing");
        press(app, Key::Escape, &[]);
    });
    step!(300, |_st, app, r| {
        line(r, format!("[filter] Esc: filter {:?}", app.global::<SettingsView>().get_filter().to_string()));
        app.invoke_focus_shortcuts();
        tabs(app, 1);
        type_text(app, "lyrics online");
        press(app, Key::Return, &[]);
    });
    step!(400, |st, app, r| {
        line(r, format!("[filter] \"lyrics online\", Enter: {}", where_(st, app)));
        snapshot(app, "find-05-lyrics-online");
    });

    // ---- history -----------------------------------------------------------------------
    step!(300, |st, app, _r| {
        app.invoke_focus_shortcuts();
        views::settings_section(st, nav::KEYS);
    });
    step!(500, |_st, app, _r| app.global::<Nav>().set_page_scroll(-400.0));
    step!(400, |st, app, r| {
        line(r, format!("[history] Keys, scrolled: {}", where_(st, app)));
        views::go(st, Page::Home);
    });
    step!(800, |st, app, r| {
        line(r, format!("[history] went Home: {}", where_(st, app)));
        views::back(st);
    });
    step!(800, |st, app, r| {
        line(r, format!("[history] Back: {} (expect Keys at -400)", where_(st, app)));
        snapshot(app, "hist-01-back-to-keys");
        views::back(st);
    });
    step!(800, |st, app, r| {
        line(r, format!("[history] Back again (leaves Settings): {}", where_(st, app)));
        views::go(st, Page::Albums);
    });
    step!(800, |st, app, r| {
        views::go(st, Page::Settings);
        line(r, format!("[history] Settings opened from the sidebar: {} (expect Keys, top)", where_(st, app)));
        let is = matches!(st.history.borrow().current, Route::Settings(7));
        line(r, format!("[history] route is Settings(Keys): {is}"));
    });
    step!(400, |st, app, r| {
        views::settings_section(st, nav::HOME);
        line(r, format!("[history] Home section; Edit shelves -> {}", where_(st, app)));
        views::go(st, Page::Shelves);
    });
    step!(800, |st, app, r| {
        views::back(st);
        line(r, format!("[history] Back from the shelf editor: {} (expect Home)", where_(st, app)));
    });

    // ---- pinned sidebars, hidden bar ----------------------------------------------------
    step!(500, |st, app, _r| {
        let nav = app.global::<Nav>();
        nav.set_sidebar_pinned(true);
        nav.set_queue_pinned(true);
        views::settings_section(st, nav::PLAYBACK);
    });
    step!(800, |_st, app, _r| {
        snapshot(app, "pin-01-both-playback");
    });
    step!(300, |st, _app, _r| views::settings_section(st, nav::KEYS));
    step!(600, |_st, app, _r| {
        snapshot(app, "pin-02-both-keys");
        // Near the end of Keys (the fixed keys), where the player bar overlaps the column.
        app.global::<Nav>().set_page_scroll(-1150.0);
    });
    step!(600, |_st, app, r| {
        line(r, format!("[pins] Keys scrolled to the end: scroll {}", app.global::<Nav>().get_page_scroll()));
        snapshot(app, "pin-03-both-keys-end");
        let nav = app.global::<Nav>();
        nav.set_sidebar_pinned(false);
        nav.set_queue_pinned(false);
    });
    step!(300, |st, _app, _r| {
        // Auto-hide while playing: the bar slides away, the column keeps its room.
        st.settings.borrow_mut().bar_autohide = true;
        views::refresh_player_settings(st);
        let album = library::query::all_albums(&st.db, library::query::AlbumSort::Artist, false).unwrap_or_default();
        if let Some(a) = album.first() {
            playback::play_tracks(st, library::query::album_tracks(&st.db, a.id).unwrap_or_default(), 0);
        }
    });
    step!(2500, |_st, app, r| {
        line(r, format!("[bar] playing, auto-hide on: bar up {}", app.get_bar_visible()));
        snapshot(app, "bar-01-hidden-keys-end");
    });
    step!(300, |st, _app, _r| {
        playback::set_paused(st, true);
        st.settings.borrow_mut().bar_autohide = false;
        views::refresh_player_settings(st);
    });

    // ---- other window sizes ------------------------------------------------------------
    let sizes: [(i32, i32, &'static str); 2] = [(960, 620, "min"), (2560, 1032, "wide")];
    for (w, h, name) in sizes {
        step!(400, move |st, _app, _r| {
            // The window's frame takes 8 px each side and below (frame.rs).
            crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
        });
        step!(1200, move |_st, app, r| {
            let (lw, lh) = size(app);
            line(r, format!("[size] asked {w}x{h}: window now {lw}x{lh} logical"));
        });
        all_sections(&mut steps, &r, name);
        if name == "min" {
            for (pins, tag) in [((true, false), "min-sidebar"), ((false, true), "min-queue"), ((true, true), "min-both")] {
                step!(300, move |st, app, _r| {
                    let nav = app.global::<Nav>();
                    nav.set_sidebar_pinned(pins.0);
                    nav.set_queue_pinned(pins.1);
                    views::settings_section(st, nav::PLAYBACK);
                });
                step!(700, move |_st, app, _r| {
                    snapshot(app, &format!("{tag}-playback"));
                });
                step!(300, move |st, _app, _r| views::settings_section(st, nav::KEYS));
                step!(600, move |_st, app, _r| {
                    snapshot(app, &format!("{tag}-keys"));
                });
                step!(300, move |st, _app, _r| views::settings_section(st, nav::CONNECTIONS));
                step!(600, move |_st, app, _r| {
                    snapshot(app, &format!("{tag}-lastfm"));
                });
            }
            step!(300, |_st, app, _r| {
                let nav = app.global::<Nav>();
                nav.set_sidebar_pinned(false);
                nav.set_queue_pinned(false);
            });
        }
    }
    step!(300, |st, _app, _r| {
        crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void);
    });
    step!(1200, |_st, app, r| {
        let (w, h) = size(app);
        line(r, format!("[size] maximized again: {w}x{h}"));
        write_report("settings", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
