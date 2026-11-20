//! Several artists and genres per track, forgiving tags, and the home page.
//!
//! - `MP_BENCH=home`: the home page at 1920 x 1080 and at the minimum size (960 x 620),
//!   from the top and scrolled (`home-<size>-<n>.png`), and a hero shelf featuring another
//!   of its albums (`home-hero-picked.png`).
//! - `MP_BENCH=library`: on the fixtures of `cargo run -p library --example messy_library`
//!   (`MP_MUSIC`): the compilation without an album artist (one album, "Various Artists"),
//!   an album whose featured credits used to split it, the artist pages of an artist with
//!   albums of their own, guest spots and a compilation track, the Artists index, the
//!   genres, search, the disc headers of albums with disc numbers on some tracks only, the
//!   album page's buttons at the minimum width with the sidebar pinned, "View metadata" of
//!   an untagged file, of a compilation track and of a 5.1 AAC file, and its "Can't play"
//!   note (`bench-library.txt`, `lib-*.png`).
//!
//! Everything goes through the app's own calls; no OS input is sent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use library::query;
use slint::{ComponentHandle, Model, Timer, TimerMode};

use crate::bench::{Step, run_steps, snapshot, write_report};
use crate::state::{self, State};
use crate::{AlbumPage, ArtistPage, Browse, Home, Nav, Page, Search, metadata, playback, views};

type Report = Rc<RefCell<String>>;

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
}

fn line(r: &Report, text: String) {
    log::info!("library bench: {text}");
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

/// Run `then` once the start-up scan is over (polled; at most two minutes).
fn after_scan(then: impl FnOnce() + 'static) {
    let then = Rc::new(RefCell::new(Some(then)));
    let timer = Rc::new(Timer::default());
    let t = timer.clone();
    let seen_scan = Rc::new(Cell::new(false));
    let started = std::time::Instant::now();
    timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
        let scanning = state::with(|st| st.scanning.get()).unwrap_or(false);
        seen_scan.set(seen_scan.get() || scanning);
        let waited = started.elapsed();
        if (!scanning && (seen_scan.get() || waited > Duration::from_secs(5))) || waited > Duration::from_secs(120) {
            t.stop();
            if let Some(f) = then.borrow_mut().take() {
                f();
            }
        }
    });
    // The timer lives as long as it runs.
    std::mem::forget(timer);
}

fn size_window(st: &State, w: i32, h: i32) {
    // The window's frame takes 8 px each side and below (frame.rs).
    crate::platform::devwindow::resize_client(st.hwnd.get(), w, h);
}

pub fn run() {
    let mut steps: Vec<Step> = vec![(1500, Box::new(|st, _app| views::go(st, Page::Home)))];
    for (w, h, name) in [(1920, 1080, "1080p"), (960, 620, "min")] {
        steps.push((300, Box::new(move |st, _app| size_window(st, w, h))));
        steps.push((1500, Box::new(move |_st, app| app.global::<Nav>().set_page_scroll(0.0))));
        for i in 0..4 {
            steps.push((900, Box::new(move |_st, app| {
                let s = app.window().size().to_logical(app.window().scale_factor());
                app.global::<Nav>().set_page_scroll(-(i as f32 * (s.height - 160.0)).max(0.0));
            })));
            steps.push((900, Box::new(move |_st, app| {
                snapshot(app, &format!("home-{name}-{i}"));
            })));
        }
    }
    // A hero shelf features another of its albums.
    steps.push((300, Box::new(|st, app| {
        size_window(st, 1920, 1080);
        app.global::<Nav>().set_page_scroll(0.0);
    })));
    steps.push((1200, Box::new(|_st, app| {
        let shelves = app.global::<Home>().get_shelves();
        if let Some(v) = (0..shelves.row_count()).filter_map(|r| shelves.row_data(r)).find(|v| v.layout == crate::ShelfLayout::Hero) {
            views::hero_select(v.index as usize, 3);
        }
    })));
    steps.push((1200, Box::new(|_st, app| {
        snapshot(app, "home-hero-picked");
        let shelves = app.global::<Home>().get_shelves();
        let titles: Vec<String> = (0..shelves.row_count()).filter_map(|r| shelves.row_data(r)).map(|v| format!("{} ({:?}, {} albums, featuring #{})", v.title, v.layout, v.albums.row_count(), v.hero)).collect();
        write_report("home", &format!("home shelves, in order:\n  {}\n", titles.join("\n  ")));
        let _ = slint::quit_event_loop();
    })));
    after_scan(move || run_steps(steps));
}

/// An album's id by its title (and, when several share it, the first).
fn album(st: &State, title: &str) -> Option<i64> {
    st.db.query_row("SELECT id FROM albums WHERE title = ?1 ORDER BY id LIMIT 1", [title], |r| r.get(0)).ok()
}

fn album_names(v: &[library::Album]) -> Vec<String> {
    v.iter().map(|a| format!("{} [{}]", a.title, a.artist)).collect()
}

pub fn run_library() {
    let r: Report = Rc::new(RefCell::new(String::new()));
    let mut steps: Vec<Step> = Vec::new();
    macro_rules! step {
        ($delay:expr, $body:expr) => {{
            let r = r.clone();
            let f: Box<dyn Fn(&Rc<State>, &crate::AppWindow, &Report)> = Box::new($body);
            steps.push(($delay, Box::new(move |st, app| f(st, app, &r))));
        }};
    }
    step!(200, |st, _app, r| {
        let (albums, tracks) = query::counts(&st.db).unwrap_or((0, 0));
        line(r, format!("library: {albums} albums, {tracks} tracks"));
        // The compilation without an album artist.
        let comp: Vec<(String, String, i64, bool)> = st
            .db
            .prepare("SELECT a.title, a.album_artist, (SELECT COUNT(*) FROM tracks t WHERE t.album_id = a.id), a.compilation FROM albums a WHERE a.title IN ('Summer Hits 2003', 'Night Drive', 'Greatest Hits', 'Low Tide', 'Live', 'Double', 'Patchwork', 'Patchwork (Deluxe)', 'Dates') ORDER BY a.title, a.album_artist")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
            .unwrap_or_default();
        for (t, a, n, c) in &comp {
            line(r, format!("  album {t:?} by {a:?}: {n} tracks{}", if *c { ", a compilation" } else { "" }));
        }
        let has = |t: &str, a: &str, n: i64| comp.iter().any(|c| c.0 == t && c.1 == a && c.2 == n);
        check(r, "a compilation without an album artist is one album", has("Summer Hits 2003", "Various Artists", 12), String::new());
        check(r, "a flagged compilation is one album", has("Night Drive", "Various Artists", 6), String::new());
        check(r, "two albums of one title in one folder stay apart", has("Greatest Hits", "Copper Monks", 5) && has("Greatest Hits", "Lunar Sparrows", 5), String::new());
        check(r, "featured credits do not split an album", has("Low Tide", "Harbour Lights", 10), String::new());
        check(r, "discs in sibling folders are one album", has("Live", "Atomic Tides", 8), String::new());
        check(r, "two spellings of a title: case and spaces merge, other words do not", has("Patchwork", "Scarlet Foxes", 8) && has("Patchwork (Deluxe)", "Scarlet Foxes", 1), String::new());
        let dates: Option<(String, String, String)> = st.db.query_row("SELECT date, genre, label FROM albums WHERE title = 'Dates'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok();
        check(r, "album values are the most common (fullest date of the year)", dates == Some(("2004-03-15".into(), "Pop".into(), "Paper Moon".into())), format!("{dates:?}"));
        let paper: Vec<(String, i64)> = query::album_artists(&st.db).unwrap_or_default().into_iter().filter(|g| library::names::norm(&g.name) == "the paper harbour").map(|g| (g.name, g.albums)).collect();
        check(r, "names in other case and spacing are one artist, in the most common spelling", paper == vec![("The Paper Harbour".to_owned(), 4)], format!("{paper:?}"));
        let zoe = query::album_artists(&st.db).unwrap_or_default().into_iter().filter(|g| g.name.starts_with("Zo")).count();
        check(r, "a name in decomposed Unicode is the same artist", zoe == 1, format!("{zoe} entries"));
        let genres: Vec<(String, i64)> = query::album_groups(&st.db, false).unwrap_or_default().into_iter().map(|g| (g.name, g.albums)).collect();
        line(r, format!("  genres: {genres:?}"));
        check(r, "rock, Rock and ROCK are one genre", genres.iter().filter(|g| library::names::norm(&g.0) == "rock").count() == 1, String::new());
    });
    // An artist with albums of their own, guest spots and a compilation track.
    step!(300, |st, _app, _r| views::open_artist(st, "Mara Lind"));
    step!(1500, |st, app, r| {
        snapshot(app, "lib-artist-mara-lind");
        let id = query::find_artist(&st.db, "Mara Lind").ok().flatten().map_or(-1, |a| a.id);
        let (own, appears) = query::artist_albums(&st.db, id).unwrap_or_default();
        line(r, format!("Mara Lind: own {:?}\n  appears on {:?}", album_names(&own), album_names(&appears)));
        check(r, "her own albums", own.iter().any(|a| a.title == "Duets"), String::new());
        check(r, "she appears on the compilation and as a featured artist", ["Summer Hits 2003", "Low Tide"].iter().all(|t| appears.iter().any(|a| a.title == *t)), String::new());
        let page = app.global::<ArtistPage>();
        check(r, "the page names her", page.get_name() == "Mara Lind", page.get_name().to_string());
    });
    step!(300, |st, _app, _r| views::open_artist(st, "Kenji Arai"));
    step!(1200, |st, app, r| {
        snapshot(app, "lib-artist-kenji-arai");
        let id = query::find_artist(&st.db, "Kenji Arai").ok().flatten().map_or(-1, |a| a.id);
        let (_, appears) = query::artist_albums(&st.db, id).unwrap_or_default();
        check(r, "an artist of a two-artist ID3v2.4 value appears on the album", appears.iter().any(|a| a.title == "Duets"), format!("{:?}", album_names(&appears)));
    });
    step!(300, |st, _app, _r| views::open_artist(st, "Ines Okafor"));
    step!(1200, |st, app, r| {
        snapshot(app, "lib-artist-ines-okafor");
        let id = query::find_artist(&st.db, "Ines Okafor").ok().flatten().map_or(-1, |a| a.id);
        let top: Vec<String> = query::artist_top_tracks(&st.db, id, 20).unwrap_or_default().into_iter().map(|t| t.title).collect();
        check(r, "a featured artist named in a title has the track", top.iter().any(|t| t.starts_with("Daybreak")), format!("{top:?}"));
    });
    step!(300, |st, _app, _r| views::go(st, Page::Artists));
    step!(1500, |_st, app, r| {
        snapshot(app, "lib-artists");
        line(r, format!("Artists index: {} artists", app.global::<Browse>().get_artists_count()));
    });
    step!(300, |st, app, _r| {
        views::go(st, Page::Search);
        app.global::<Search>().set_query("mara".into());
        views::search(st, "mara");
    });
    step!(1500, |_st, app, r| {
        snapshot(app, "lib-search-mara");
        let s = app.global::<Search>();
        let artists: Vec<String> = (0..s.get_artists().row_count()).filter_map(|i| s.get_artists().row_data(i)).map(|a| a.text.to_string()).collect();
        check(r, "search finds the artist", artists.first().map(String::as_str) == Some("Mara Lind"), format!("{artists:?}"));
    });
    // The compilation's page; disc headers.
    for (title, snap) in [("Summer Hits 2003", "lib-album-compilation"), ("Half Numbered", "lib-album-half-numbered"), ("Two Sides", "lib-album-two-sides"), ("Double", "lib-album-double"), ("Duets", "lib-album-duets")] {
        step!(300, move |st, _app, _r| {
            if let Some(id) = album(st, title) {
                views::open_album(st, id);
            }
        });
        step!(1300, move |st, app, r| {
            snapshot(app, snap);
            let headers: Vec<String> = (0..st.lists.album.row_count()).filter_map(|i| st.lists.album.row_data(i)).filter(|t| !t.disc_header.is_empty()).map(|t| t.disc_header.to_string()).collect();
            line(r, format!("{title}: {} by {}; disc headers {headers:?}", app.global::<AlbumPage>().get_album().title, app.global::<AlbumPage>().get_album().artist));
            match title {
                "Half Numbered" => check(r, "disc 1 on some tracks only: no header (one disc)", headers.is_empty(), format!("{headers:?}")),
                "Two Sides" => check(r, "disc 2 tagged, disc 1 not: each header once", headers == ["DISC 1", "DISC 2"], format!("{headers:?}")),
                "Double" => check(r, "disc subfolders: each header once", headers == ["DISC 1", "DISC 2"], format!("{headers:?}")),
                _ => {}
            }
        });
    }
    // The album page's buttons at the minimum size, with and without the sidebar pinned.
    step!(300, |st, _app, _r| size_window(st, 960, 620));
    for (pinned, snap) in [(false, "lib-album-min"), (true, "lib-album-min-pinned")] {
        step!(500, move |_st, app, _r| app.global::<Nav>().set_sidebar_pinned(pinned));
        step!(900, move |_st, app, _r| {
            snapshot(app, snap);
        });
    }
    step!(300, |st, app, _r| {
        app.global::<Nav>().set_sidebar_pinned(false);
        crate::platform::devwindow::maximize(st.hwnd.get() as *mut std::ffi::c_void);
    });
    // View metadata: an untagged file, a compilation track, a 5.1 AAC file.
    for (path_like, snap) in [("%Untagged Rip%1-03 Third Rail.wav", "lib-meta-untagged"), ("%Summer Hits 2003%03.%", "lib-meta-compilation"), ("%Low Tide%04 %", "lib-meta-featured"), ("%Surround%", "lib-meta-surround"), ("%Duets%05 Duet%", "lib-meta-picard")] {
        step!(600, move |st, _app, _r| {
            let id: Option<i64> = st.db.query_row("SELECT id FROM tracks WHERE path LIKE ?1 ORDER BY path LIMIT 1", [path_like], |r| r.get(0)).ok();
            if let Some(t) = id.and_then(|id| query::track(&st.db, id).ok().flatten()) {
                metadata::view::open_track(st, t);
            }
        });
        step!(1600, move |_st, app, r| {
            snapshot(app, snap);
            let lines: Vec<String> = metadata::view::lines().iter().take(14).map(|l| format!("{}: {}{}", l.label, l.values.join(" | "), if l.note.is_empty() { String::new() } else { format!("  [{}]", l.note) })).collect();
            line(r, format!("{snap}:\n    {}", lines.join("\n    ")));
        });
        step!(300, |st, _app, _r| metadata::view::close(st));
    }
    // The 5.1 AAC file: the note says it is the format.
    step!(300, |st, _app, _r| {
        if let Some(id) = album(st, "Surround") {
            let tracks = query::album_tracks(&st.db, id).unwrap_or_default();
            playback::play_tracks(st, tracks, 0);
        }
    });
    step!(1500, |_st, app, r| {
        let flash = app.global::<Nav>().get_flash().to_string();
        snapshot(app, "lib-cant-play-surround");
        check(r, "a 5.1 AAC file says the format is the reason", flash.contains("AAC with 6 channels"), flash);
    });
    step!(500, |st, _app, r| {
        playback::set_paused(st, true);
        let fails = FAILS.with(Cell::get);
        line(r, format!("\n{fails} failed"));
        write_report("library", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    after_scan(move || run_steps(steps));
}
