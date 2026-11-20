//! `MP_BENCH=phase5`: output devices, and with them Last.fm, formats in the running app,
//! playlist files and lyrics, that no rating is left (and, with `MP_BENCH_PART=open`, files from outside),
//! each snapshotted, and a report of what the app did (`bench-phase5.txt`; the mode's name
//! is kept for the bench runner and its reports). Output devices are exercised without touching
//! any device but the default: the "chosen" device is the default named by its id, and
//! a missing one is a made-up id. Nothing is audible (`MP_VOLUME=0`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use crate::bench::{Step, menu_labels, run_steps, snapshot, write_report};
use crate::state::State;
use crate::{AppWindow, Nav, Page, views};

type Report = Rc<RefCell<String>>;
/// A step of a run: what it does, given the report.
type StepFn = Box<dyn Fn(&Rc<State>, &AppWindow, &Report)>;


/// `MP_BENCH_PART=open`: files from outside. The run script starts the app with files on
/// its command line, and a few seconds in launches it again with another file (handed
/// over to this instance); this posts real WM_DROPFILES messages to the window (a drop on
/// the queue, then on the page). No OS input is sent.
pub fn run_open() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    let mut step = |ms: u64, f: StepFn| {
        let r = r.clone();
        steps.push((ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
    };
    fn queue(st: &State) -> Vec<String> {
        st.queue.borrow().items().iter().map(|i| format!("{} [{}{}]", i.track.title, if i.track.id < 0 { "outside " } else { "library " }, i.track.id)).collect()
    }
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[open] command line: queue {:?}, playing {:?}, pill {:?}", queue(st), st.player.status().state, app.global::<Nav>().get_flash().to_string()));
        views::go(st, Page::NowPlaying);
    }));
    step(1500, Box::new(|_st, app, _r| {
        snapshot(app, "p5-14-opened-from-command-line");
    }));
    step(7000, Box::new(|st, app, r| {
        line(r, format!("[open] after a second launch with a file: queue {:?}, pill {:?}", queue(st), app.global::<Nav>().get_flash().to_string()));
        app.global::<Nav>().set_queue_open(true);
    }));
    step(800, Box::new(|st, app, r| {
        // Drop on the queue sidebar (right edge): added to the end.
        let size = app.window().size();
        let files = std::env::var("MP_BENCH_DROP").unwrap_or_default();
        let n = crate::platform::window::simulate_drop(st.hwnd.get(), &files.split(';').map(PathBuf::from).collect::<Vec<_>>(), size.width as i32 - 100, size.height as i32 / 2);
        line(r, format!("[open] posted WM_DROPFILES with {n} path(s) on the queue"));
    }));
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[open] after the drop on the queue: {:?}, pill {:?}", queue(st), app.global::<Nav>().get_flash().to_string()));
        snapshot(app, "p5-15-dropped-on-queue");
        app.global::<Nav>().set_queue_open(false);
        views::go(st, Page::Home);
    }));
    step(800, Box::new(|st, app, r| {
        let size = app.window().size();
        let files = std::env::var("MP_BENCH_DROP").unwrap_or_default();
        let first = files.split(';').next().map(PathBuf::from).into_iter().collect::<Vec<_>>();
        crate::platform::window::simulate_drop(st.hwnd.get(), &first, size.width as i32 / 3, size.height as i32 / 2);
        line(r, "[open] posted WM_DROPFILES with one file, on the page".into());
    }));
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[open] after the drop on the page: {:?}, state {:?}, pill {:?}", queue(st), st.player.status().state, app.global::<Nav>().get_flash().to_string()));
        playback_favourite_check(st, r);
    }));
    step(800, Box::new(|_st, _app, r| {
        write_report("phase5-open", &r.borrow());
        let _ = slint::quit_event_loop();
    }));
    drop(step);
    run_steps(steps);
}

fn playback_favourite_check(st: &Rc<State>, r: &Report) {
    // L on a file from outside: not in the library, nothing recorded.
    crate::playback::toggle_favourite_current(st);
    let favs: i64 = st.db.query_row("SELECT COUNT(*) FROM favourite_tracks", [], |r| r.get(0)).unwrap_or(-1);
    let plays: i64 = st.db.query_row("SELECT COUNT(*) FROM plays WHERE track_id < 0", [], |r| r.get(0)).unwrap_or(-1);
    line(r, format!("[open] favourite on an outside track: pill {:?}; favourite rows {favs}, plays with negative ids {plays}", st.app.upgrade().map(|a| a.global::<Nav>().get_flash().to_string()).unwrap_or_default()));
}

fn album_tracks(st: &State, title: &str) -> Vec<library::Track> {
    let id: Option<i64> = st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok();
    id.map(|id| library::query::album_tracks(&st.db, id).unwrap_or_default()).unwrap_or_default()
}

fn album_id_of(st: &State, title: &str) -> Option<i64> {
    st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok()
}

fn current_text(app: &AppWindow) -> String {
    let l = app.global::<crate::Lyrics>();
    usize::try_from(l.get_current()).ok().and_then(|i| l.get_lines().row_data(i)).map(|x| x.text.to_string()).unwrap_or_default()
}

fn playlist_titles(st: &State, id: i64) -> Vec<String> {
    library::query::playlist_tracks(&st.db, id).unwrap_or_default().into_iter().map(|t| t.title).collect()
}

fn app_summary(st: &State) -> String {
    st.app.upgrade().map(|a| a.global::<crate::Playlists>().get_import_summary().to_string()).unwrap_or_default()
}

thread_local! {
    static PL: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
    static LF_SAVED: RefCell<Option<(slint::SharedString, bool, slint::SharedString)>> = const { RefCell::new(None) };
}

fn line(r: &Report, text: String) {
    log::info!("phase5: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn output_line(app: &AppWindow) -> String {
    let o = app.global::<crate::Output>();
    format!("button {:?}; status {:?}", o.get_device_label().to_string(), o.get_status().to_string())
}

pub fn run() {
    let r: Report = Rc::default();
    let mut steps: Vec<Step> = Vec::new();
    let mut step = |ms: u64, f: StepFn| {
        let r = r.clone();
        steps.push((ms, Box::new(move |st: &Rc<State>, app: &AppWindow| f(st, app, &r))));
    };

    // ---- 1. Output device ----------------------------------------------------------
    step(500, Box::new(|st, app, r| {
        let size = app.window().size();
        line(r, format!("window {}x{} physical, scale {}", size.width, size.height, app.window().scale_factor()));
        line(r, format!("library: {:?} (albums, tracks)", library::query::counts(&st.db)));
        views::go(st, Page::Settings);
        views::settings_section(st, crate::settings::nav::OUTPUT);
    }));
    step(1500, Box::new(|_st, app, r| {
        line(r, format!("[output] settings: {}", output_line(app)));
        let devices = crate::output::devices().unwrap_or_default();
        line(r, format!("[output] {} devices listed, default: {:?}", devices.len(), devices.iter().find(|d| d.default).map(|d| &d.name)));
        snapshot(app, "p5-01-settings-output");
        crate::output::open_menu(900.0, 300.0);
    }));
    step(1000, Box::new(|_st, app, r| {
        line(r, format!("[output] menu: {:?}", menu_labels(app)));
        snapshot(app, "p5-02-output-menu");
        app.global::<crate::MenuStore>().set_open(false);
    }));
    step(200, Box::new(|st, _app, _r| {
        let first = library::query::all_albums(&st.db, library::query::AlbumSort::Artist, false).unwrap_or_default();
        let tracks = first.first().map(|a| library::query::album_tracks(&st.db, a.id).unwrap_or_default()).unwrap_or_default();
        crate::playback::play_tracks(st, tracks, 0);
    }));
    step(2000, Box::new(|st, app, r| {
        line(r, format!("[output] playing at {:.2} s on {:?}", st.player.status().position, st.player.output_info()));
        line(r, format!("[output] {}", output_line(app)));
        // Choose the default device by its id: the "specific device" path, on the device
        // that is playing anyway.
        let id = crate::output::devices().unwrap_or_default().into_iter().find(|d| d.default).map(|d| d.id);
        line(r, format!("[output] choosing the default device by id; position before {:.2} s", st.player.status().position));
        crate::output::choose(st, id);
    }));
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[output] after choosing: position {:.2} s, {:?}", st.player.status().position, st.player.output_info()));
        line(r, format!("[output] {}", output_line(app)));
        line(r, format!("[output] settings.output_device saved: {}", st.settings.borrow().output_device.is_some()));
        st.settings.borrow_mut().output_device_name = Some("Test DAC".into());
        crate::output::choose(st, Some("{0.0.0.00000000}.{00000000-dead-beef-0000-000000000000}".into()));
    }));
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[output] a missing device: position {:.2} s, {:?}", st.player.status().position, st.player.output_info()));
        line(r, format!("[output] {}; pill {:?}", output_line(app), app.global::<Nav>().get_flash().to_string()));
        snapshot(app, "p5-03-output-missing");
        crate::output::choose(st, None);
    }));
    step(1500, Box::new(|st, app, r| {
        line(r, format!("[output] back to the system default: position {:.2} s, {:?}", st.player.status().position, st.player.output_info()));
        line(r, format!("[output] {}", output_line(app)));
    }));

    // ---- 2. Last.fm --------------------------------------------------------------------
    // With MP_LASTFM_API at a local mock and a lastfm.json in the data folder, the app
    // starts connected: the playing track was sent as now playing and is scrobbled once
    // half of it has played (the first fixture track is 40 s: at 20 s).
    step(500, Box::new(|_st, app, r| {
        let l = app.global::<crate::LastFm>();
        line(r, format!("[lastfm] status {:?}, connected {}, user {:?}", l.get_status().to_string(), l.get_connected(), l.get_user().to_string()));
        crate::state::with(|st| views::settings_section(st, crate::settings::nav::CONNECTIONS));
    }));
    step(800, Box::new(|_st, app, _r| {
        snapshot(app, "p5-04-lastfm-connected");
    }));
    step(100, Box::new(|st, app, r| {
        // The other states as Settings shows them (the browser is never opened here).
        let l = app.global::<crate::LastFm>();
        let saved = (l.get_status(), l.get_connected(), l.get_user());
        crate::connections::scrobble::show(st, &lastfm::Status { state: lastfm::State::NoCredentials, queued: 0, note: String::new(), accepting: false });
        l.set_api_key("".into());
        l.set_api_secret("".into());
        line(r, format!("[lastfm] (shown) no credentials: {:?}", l.get_status().to_string()));
        snapshot(app, "p5-05-lastfm-empty");
        l.set_api_key("0123456789abcdef0123456789abcdef".into());
        l.set_api_secret("fedcba9876543210".into());
        crate::connections::scrobble::show(st, &lastfm::Status { state: lastfm::State::Authorising { url: String::new() }, queued: 2, note: String::new(), accepting: false });
        line(r, format!("[lastfm] (shown) authorising: {:?}", l.get_status().to_string()));
        LF_SAVED.with(|s| *s.borrow_mut() = Some(saved));
    }));
    step(600, Box::new(|_st, app, _r| {
        snapshot(app, "p5-06-lastfm-authorising");
        let l = app.global::<crate::LastFm>();
        if let Some((status, connected, user)) = LF_SAVED.with(|s| s.borrow_mut().take()) {
            l.set_status(status);
            l.set_connected(connected);
            l.set_authorising(false);
            l.set_user(user);
        }
    }));
    step(100, Box::new(|st, _app, r| {
        line(r, format!("[lastfm] waiting for the scrobble point; position {:.1} s", st.player.status().position));
    }));
    step(17_000, Box::new(|st, app, r| {
        line(r, format!("[lastfm] position {:.1} s; {:?}", st.player.status().position, app.global::<crate::LastFm>().get_status().to_string()));
        crate::playback::toggle_play(st);
    }));

    // ---- 4. Playlist files --------------------------------------------------------------
    step(500, Box::new(|st, _app, r| {
        use library::query;
        let id = query::create_playlist(&st.db, "Bench mix").unwrap();
        let mut ids: Vec<i64> = Vec::new();
        for a in query::all_albums(&st.db, query::AlbumSort::Artist, false).unwrap_or_default() {
            ids.extend(query::album_tracks(&st.db, a.id).unwrap_or_default().iter().map(|t| t.id));
        }
        query::add_to_playlist(&st.db, id, &ids).unwrap();
        let out = crate::bench::out_dir();
        let abs = out.join("bench-mix.m3u8");
        let rel = out.join("lists").join("bench-mix-relative.m3u8");
        let _ = std::fs::create_dir_all(rel.parent().unwrap());
        crate::playlists::file::export_file(st, id, &abs, false);
        crate::playlists::file::export_file(st, id, &rel, true);
        let text = std::fs::read_to_string(&abs).unwrap_or_default();
        line(r, format!("[m3u] exported {} tracks; absolute file:\n{}", ids.len(), text.trim_end()));
        let text = std::fs::read_to_string(&rel).unwrap_or_default();
        line(r, format!("[m3u] relative file (in a subfolder of the bench output):\n{}", text.trim_end()));
        // A playlist as other players write them: forward slashes, other case, a file URL,
        // a missing track, Windows-1252 text.
        let music = st.settings.borrow().folders.first().cloned().unwrap_or_default();
        let m = music.to_string_lossy().replace('\\', "/");
        let mut hand: Vec<u8> = format!(
            "#EXTM3U\r\n#EXTINF:40,Aurora Lines - Lantern\r\n{}/AURORA LINES - NIGHT HARBOUR/01 lantern.flac\r\nKestrel - Paper Moons/02 Ash and Clover.flac\r\nfile:///{}/Kestrel%20-%20Paper%20Moons/01%20Paper%20Moons.flac\r\n#EXTINF:200,",
            m.to_uppercase(),
            m
        )
        .into_bytes();
        hand.extend_from_slice(b"Beyonc\xe9 - Gone\r\nD:\\Elsewhere\\gone.flac\r\n");
        let hp = out.join("Other player.m3u");
        std::fs::write(&hp, hand).unwrap();
        for p in [rel, hp] {
            if let Some((pid, m)) = crate::playlists::file::import_file(st, &p) {
                line(r, format!("[m3u] imported {:?}: {} matched {:?}, missing {:?}", p.file_name().unwrap(), m.ids.len(), playlist_titles(st, pid), m.missing));
            }
        }
        line(r, format!("[m3u] summary on the Playlists page: {:?}", app_summary(st)));
        views::go(st, Page::Playlists);
        PL.with(|p| p.set(id));
    }));
    step(1200, Box::new(|st, app, _r| {
        snapshot(app, "p5-07-playlist-import");
        views::open_playlist(st, PL.with(|p| p.get()));
    }));
    step(1200, Box::new(|st, app, r| {
        let size = app.window().size().to_logical(app.window().scale_factor());
        let _ = size;
        crate::menus::request_export(st, PL.with(|p| p.get()), 360.0, 330.0);
        line(r, format!("[m3u] export menu: {:?}", menu_labels(app)));
        let id = PL.with(|p| p.get());
        crate::menus::request(st, "playlist", "", 0, id as i32, 300.0, 300.0);
        line(r, format!("[m3u] playlist context menu: {:?}", menu_labels(app)));
        crate::menus::request_export(st, PL.with(|p| p.get()), 360.0, 330.0);
    }));
    step(800, Box::new(|_st, app, _r| {
        snapshot(app, "p5-08-playlist-export-menu");
        app.global::<crate::MenuStore>().set_open(false);
    }));

    // ---- 3. Formats, in the running app --------------------------------------------------
    step(200, Box::new(|st, _app, r| {
        let tracks = album_tracks(st, "Sampler");
        line(r, format!("[formats] the Sampler album: {:?}", tracks.iter().map(|t| t.path.rsplit(['/', '\\']).next().unwrap_or("").to_owned()).collect::<Vec<_>>()));
        crate::playback::play_tracks(st, tracks, 0);
    }));
    for k in 0..4 {
        step(1500, Box::new(move |st, _app, r| {
            let s = st.player.status();
            let cur = st.queue.borrow().current().map(|i| i.track.title.clone()).unwrap_or_default();
            line(r, format!("[formats] playing {cur:?} at {:.2} s of {:?} s, state {:?}", s.position, s.duration.map(|d| (d * 100.0).round() / 100.0), s.state));
            // Seek near the end so the next one follows gaplessly.
            crate::playback::seek(st, s.duration.unwrap_or(20.0) - 0.6);
            let _ = k;
        }));
    }
    step(1500, Box::new(|st, _app, r| {
        let s = st.player.status();
        line(r, format!("[formats] after the last: state {:?}", s.state));
    }));

    // ---- 5. Lyrics ------------------------------------------------------------------------
    step(200, Box::new(|st, app, _r| {
        let tracks = album_tracks(st, "Night Harbour");
        crate::playback::play_tracks(st, tracks, 0);
        app.global::<crate::Lyrics>().set_shown(true);
        views::go(st, Page::NowPlaying);
    }));
    step(9000, Box::new(|st, app, r| {
        let l = app.global::<crate::Lyrics>();
        line(r, format!("[lyrics] {} lines, synced {}, at {:.1} s the current line is {} {:?}", l.get_lines().row_count(), l.get_synced(), st.player.status().position, l.get_current(), current_text(app)));
        snapshot(app, "p5-09-lyrics-synced");
        crate::lyrics::seek(st, 7);
    }));
    step(1200, Box::new(|st, app, r| {
        let l = app.global::<crate::Lyrics>();
        line(r, format!("[lyrics] clicked line 7: now at {:.1} s, current {} {:?}", st.player.status().position, l.get_current(), current_text(app)));
        snapshot(app, "p5-10-lyrics-after-seek");
        crate::bench::press(app, slint::platform::Key::Escape, &[]);
    }));
    step(600, Box::new(|st, app, r| {
        // Y: lyrics from anywhere; on now playing it flips back to Up next.
        views::go(st, Page::Home);
        app.global::<crate::Lyrics>().set_shown(false);
        crate::bench::type_text(app, "y");
        line(r, format!("[lyrics] Y from Home: page {:?}, lyrics shown {}", app.global::<Nav>().get_page(), app.global::<crate::Lyrics>().get_shown()));
    }));
    step(800, Box::new(|_st, app, r| {
        crate::bench::type_text(app, "y");
        line(r, format!("[lyrics] Y again: lyrics shown {}", app.global::<crate::Lyrics>().get_shown()));
        snapshot(app, "p5-11-up-next-tab");
        crate::bench::type_text(app, "y");
    }));
    step(400, Box::new(|st, _app, _r| {
        let tracks = album_tracks(st, "Paper Moons");
        crate::playback::play_tracks(st, tracks, 0);
    }));
    step(1500, Box::new(|_st, app, r| {
        let l = app.global::<crate::Lyrics>();
        line(r, format!("[lyrics] Paper Moons (tags): {} lines, synced {}", l.get_lines().row_count(), l.get_synced()));
        snapshot(app, "p5-12-lyrics-unsynced");
    }));
    step(200, Box::new(|st, _app, _r| crate::playback::next(st)));
    step(1500, Box::new(|_st, app, r| {
        let l = app.global::<crate::Lyrics>();
        line(r, format!("[lyrics] Ash and Clover (none): {} lines, status {:?}", l.get_lines().row_count(), l.get_status().to_string()));
        snapshot(app, "p5-13-lyrics-none");
    }));

    // ---- 9. No ratings ---------------------------------------------------------------------
    // Ratings are gone: 4 does nothing, no track menu offers them, the rows have no stars.
    step(200, Box::new(|st, app, r| {
        let tracks = album_tracks(st, "Night Harbour");
        crate::playback::play_tracks(st, tracks, 0);
        crate::bench::press(app, slint::platform::Key::Escape, &[]);
        // 4, as the keyboard delivers it.
        use slint::platform::WindowEvent;
        let w = app.window();
        w.dispatch_event(WindowEvent::KeyPressed { text: "4".into() });
        w.dispatch_event(WindowEvent::KeyReleased { text: "4".into() });
        line(r, format!("[ratings] 4 on the playing track: pill {:?} (none wanted)", app.global::<Nav>().get_flash().to_string()));
        if let Some(a) = album_id_of(st, "Night Harbour") {
            views::open_album(st, a);
        }
    }));
    step(1500, Box::new(|st, app, r| {
        snapshot(app, "p5-17-album-rows");
        crate::menus::request(st, "track", "album", 1, 0, 700.0, 420.0);
        let labels = menu_labels(app);
        line(r, format!("[ratings] track menu: {labels:?}; offers a rating: {}", labels.iter().any(|l| l.starts_with("Rate"))));
    }));
    step(800, Box::new(|st, app, _r| {
        snapshot(app, "p5-16-track-menu");
        app.global::<crate::MenuStore>().set_open(false);
        crate::playback::toggle_play(st);
    }));

    step(800, Box::new(|_st, _app, r| {
        write_report("phase5", &r.borrow());
        let _ = slint::quit_event_loop();
    }));
    drop(step);
    run_steps(steps);
}
