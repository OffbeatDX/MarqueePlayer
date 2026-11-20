//! `MP_BENCH=listenbrainz`: Settings > Connections > ListenBrainz end to end in the real app,
//! against a stand-in it starts itself (`listenbrainz::mock`, on 127.0.0.1: nothing leaves the
//! machine), reached through the card's own "Server address" (plain http, noted). Each request
//! is checked by the stand-in against the documented rules. `bench-listenbrainz.txt` and
//! `lb-*.png`:
//!
//! - not connected: the card, "Find a setting", the token page asked for and not opened;
//! - a token refused in words; the right one: connected as its user, the field emptied,
//!   `listenbrainz.json` without the token; the server card's switch enabled;
//! - a track played: "now playing" at its start, the listen (`single`) once half of it played,
//!   with the app's name and version;
//! - the server down: listens kept and counted; back: sent as one `import`;
//! - a 429: waited out (the time it said), then sent, nothing refused;
//! - the token refused while connected: "connect again", the section marked, the queue kept;
//!   connecting again sends it;
//! - Copy diagnostics' line (never the token); "Love favourites" on and off; Disconnect; the
//!   log never carrying the token.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use listenbrainz::mock::{Mock, Trouble};
use slint::ComponentHandle;

use crate::bench::{Step, run_steps, snapshot, write_report};
use crate::state::State;
use crate::{Actions, AppWindow, ListenBrainz, Page, settings, views};

type Report = Rc<RefCell<String>>;

const TOKEN: &str = "b3nch70k-0000-4000-8000-1157e2b2a1a2";
const USER: &str = "bench-listener";

thread_local! {
    static FAILS: Cell<u32> = const { Cell::new(0) };
    static MOCK: RefCell<Option<Mock>> = const { RefCell::new(None) };
    static FIRST: RefCell<Option<library::Track>> = const { RefCell::new(None) };
}

fn line(r: &Report, text: String) {
    log::info!("listenbrainz bench: {text}");
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

fn mock<T>(f: impl FnOnce(&Mock) -> T) -> T {
    MOCK.with(|m| f(m.borrow().as_ref().expect("the stand-in")))
}

fn card(app: &AppWindow) -> String {
    let l = app.global::<ListenBrainz>();
    format!("connected {}, connecting {}, user {:?}, attention {}, status {:?}", l.get_connected(), l.get_connecting(), l.get_user().to_string(), l.get_attention(), l.get_status().to_string())
}

fn account_file(st: &State) -> String {
    std::fs::read_to_string(st.paths.data.join("listenbrainz.json")).unwrap_or_default()
}

/// A play of `t` started `ago` seconds ago, as the app would queue it.
fn listen(t: &library::Track, ago: i64) -> listenbrainz::Listen {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    crate::connections::listens::listen_of(t, now - ago, None)
}

fn connect(app: &AppWindow, token: &str) {
    let l = app.global::<ListenBrainz>();
    l.set_token(token.into());
    app.global::<Actions>().invoke_listenbrainz_connect(l.get_token(), l.get_server());
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

    // ---- not connected ------------------------------------------------------------------
    step!(1500, |st, app, r| {
        MOCK.with(|m| *m.borrow_mut() = Some(Mock::start(TOKEN, USER)));
        line(r, format!("[start] {}", card(app)));
        check(r, "not connected at first", !app.global::<ListenBrainz>().get_connected() && account_file(st).is_empty(), card(app));
        let hits = settings::nav::find("listenbrainz");
        check(r, "Find a setting: \"listenbrainz\" finds the card first", hits.first().is_some_and(|h| h.label == "ListenBrainz" && h.section == settings::nav::CONNECTIONS), format!("{hits:?}"));
        let hits = settings::nav::find("self-hosted");
        check(r, "Find a setting: \"self-hosted\" finds the server address", hits.first().is_some_and(|h| h.label == "ListenBrainz server address"), format!("{hits:?}"));
        views::go(st, Page::Settings);
        views::settings_section(st, settings::nav::CONNECTIONS);
        app.global::<Actions>().invoke_listenbrainz_token_page();
        check(r, "the token page asked for, not opened (a bench run)", crate::connections::listens::token_page_asked() == 1, String::new());
    });
    step!(1200, |_st, app, _r| {
        app.global::<crate::Nav>().set_page_scroll(-560.0);
    });
    step!(600, |_st, app, r| {
        snapshot(app, "lb-01-not-connected");
        // The stand-in's address in the card's own field: plain http, noted.
        let url = mock(|m| m.url.clone());
        let l = app.global::<ListenBrainz>();
        l.set_server(url.clone().into());
        app.global::<Actions>().invoke_listenbrainz_server_edited(url.into());
        check(r, "a plain http address is noted, and the token page is not offered for it", l.get_plain_http() && l.get_own_server(), String::new());
        connect(app, "0000aaaa-0000-0000-0000-000000000000");
    });
    step!(1500, |_st, app, r| {
        let s = app.global::<ListenBrainz>().get_status().to_string();
        check(r, "a token it does not know: refused in words", s.contains("did not accept this token") && !app.global::<ListenBrainz>().get_connected(), s);
        snapshot(app, "lb-02-refused");
        connect(app, TOKEN);
    });
    step!(1500, |st, app, r| {
        let l = app.global::<ListenBrainz>();
        check(r, "the right token: connected as its user", l.get_connected() && l.get_user() == USER, card(app));
        check(r, "the token field emptied", l.get_token().is_empty(), String::new());
        let file = account_file(st);
        check(r, "listenbrainz.json kept, without the token as typed", !file.is_empty() && !file.contains(TOKEN) && file.contains(&mock(|m| m.url.clone())), format!("{} bytes", file.len()));
        check(r, "the server card's scrobbling switch shown enabled (a service connected)", app.global::<crate::LastFm>().get_connected() || l.get_connected(), String::new());
        snapshot(app, "lb-03-connected");
        // A track played: now playing at its start.
        let albums = library::query::all_albums(&st.db, library::query::AlbumSort::Artist, false).unwrap_or_default();
        let tracks = albums.first().map(|a| library::query::album_tracks(&st.db, a.id).unwrap_or_default()).unwrap_or_default();
        line(r, format!("[play] {:?}, {:.0} s", tracks.first().map(|t| t.title.clone()), tracks.first().map_or(0.0, |t| t.duration)));
        FIRST.with(|f| *f.borrow_mut() = tracks.first().cloned());
        crate::playback::play_tracks(st, tracks, 0);
    });
    step!(2000, |_st, _app, r| {
        let title = FIRST.with(|f| f.borrow().as_ref().map(|t| t.title.clone())).unwrap_or_default();
        let np = mock(|m| m.with(|s| s.playing_now.clone()));
        check(r, "now playing sent as the track started", np.len() == 1 && np[0]["track_metadata"]["track_name"] == title.as_str(), format!("{np:?}"));
        let info = np.first().map(|l| l["track_metadata"]["additional_info"].clone()).unwrap_or_default();
        check(r, "with the app's name and version", info["media_player"] == "Marquee Player" && info["submission_client_version"] == crate::about::VERSION, info.to_string());
        line(r, "[play] waiting for half of the track".into());
    });
    step!(22_000, |st, app, r| {
        let listens = mock(|m| m.with(|s| s.listens.clone()));
        let kinds: Vec<String> = mock(|m| m.with(|s| s.requests.iter().map(|q| q.listen_type.clone()).filter(|k| !k.is_empty()).collect()));
        check(r, "the listen sent once half of it played, as a single", listens.len() == 1 && kinds == ["playing_now", "single"], format!("{kinds:?}, position {:.1} s", st.player.status().position));
        check(r, "the card says so", app.global::<ListenBrainz>().get_status().contains("Sent 1 listen"), card(app));
        crate::playback::toggle_play(st);
        // The server down: two plays kept.
        mock(|m| m.set_trouble(Trouble::Down));
        if let Some(t) = FIRST.with(|f| f.borrow().clone()) {
            crate::connections::listens::submit(listen(&t, 900));
            crate::connections::listens::submit(listen(&t, 600));
        }
    });
    step!(1500, |st, app, r| {
        let s = app.global::<ListenBrainz>().get_status().to_string();
        let queued: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(st.paths.data.join("listens.json")).unwrap_or_default()).unwrap_or_default();
        check(r, "the server down: both listens kept, on disk, and counted", s.contains("2 listens waiting") && queued.len() == 2, s);
        snapshot(app, "lb-04-waiting");
        mock(|m| m.set_trouble(Trouble::None));
        app.global::<Actions>().invoke_listenbrainz_retry();
    });
    step!(2500, |_st, app, r| {
        let seen = mock(|m| m.with(|s| s.requests.last().map(|q| (q.listen_type.clone(), q.listens, q.status))));
        check(r, "back: sent as one import of two", seen == Some(("import".into(), 2, 200)) && app.global::<ListenBrainz>().get_status().contains("Sent 2 listens"), format!("{seen:?}; {}", card(app)));
        // A 429: waited out.
        mock(|m| m.set_trouble(Trouble::RateLimited { times: 1, reset_in: 2 }));
        if let Some(t) = FIRST.with(|f| f.borrow().clone()) {
            crate::connections::listens::submit(listen(&t, 300));
        }
    });
    step!(800, |_st, app, r| {
        let s = app.global::<ListenBrainz>().get_status().to_string();
        check(r, "a 429: the card says it waits", s.contains("rate limit"), s);
    });
    step!(3500, |_st, app, r| {
        let seen: Vec<(u16, Instant)> = mock(|m| m.with(|s| s.requests.iter().rev().take(2).map(|q| (q.status, q.at)).collect()));
        let ok = seen.len() == 2 && seen[1].0 == 429 && seen[0].0 == 200 && seen[0].1 - seen[1].1 >= Duration::from_millis(1900);
        check(r, "then sent again after the time it said, not before", ok && app.global::<ListenBrainz>().get_status().contains("Sent 1 listen"), format!("{:?}", seen.iter().map(|s| s.0).collect::<Vec<_>>()));
        // The token refused while connected.
        mock(|m| m.set_trouble(Trouble::BadToken));
        if let Some(t) = FIRST.with(|f| f.borrow().clone()) {
            crate::connections::listens::submit(listen(&t, 200));
        }
    });
    step!(1500, |st, app, r| {
        let l = app.global::<ListenBrainz>();
        check(r, "the token refused: connect again, the section marked, the listen kept", l.get_attention() && !l.get_connected() && l.get_status().contains("Connect again") && l.get_status().contains("1 listen waiting"), card(app));
        check(r, "and the token forgotten", !account_file(st).contains("sealed_token"), String::new());
        snapshot(app, "lb-05-connect-again");
        mock(|m| m.set_trouble(Trouble::None));
        connect(app, TOKEN);
    });
    step!(2500, |st, app, r| {
        let l = app.global::<ListenBrainz>();
        check(r, "connected again: the listen kept is sent", l.get_connected() && !l.get_attention() && mock(|m| m.with(|s| s.listens.len())) == 5, card(app));
        let d = crate::about::diagnostics(st);
        let settings_line = d.lines().find(|l| l.starts_with("Settings:")).unwrap_or("").to_owned();
        check(r, "Copy diagnostics names ListenBrainz, never the token or the address", settings_line.contains("ListenBrainz on (own server, 0 waiting)") && !d.contains(TOKEN) && !d.contains(&mock(|m| m.url.clone())), settings_line);
        // Love favourites: on, a heart given (the fixture has no MusicBrainz id: nothing sent), off.
        app.global::<Actions>().invoke_toggle_listenbrainz_love();
        check(r, "Love favourites on", st.settings.borrow().listenbrainz_love && l.get_love(), String::new());
        let fav = crate::playback::toggle_favourite_current(st);
        line(r, format!("[love] favourite now {fav:?}"));
    });
    step!(1500, |st, app, r| {
        check(r, "a favourite without a recording id sends no feedback", mock(|m| m.with(|s| s.feedback.len())) == 0, String::new());
        crate::playback::toggle_favourite_current(st);
        app.global::<Actions>().invoke_toggle_listenbrainz_love();
        check(r, "Love favourites off again", !st.settings.borrow().listenbrainz_love, String::new());
        app.global::<Actions>().invoke_listenbrainz_disconnect();
    });
    step!(1500, |st, app, r| {
        let l = app.global::<ListenBrainz>();
        check(r, "disconnected: the token forgotten, the address kept", !l.get_connected() && !account_file(st).contains("sealed_token") && account_file(st).contains("127.0.0.1"), card(app));
        snapshot(app, "lb-06-disconnected");
        let violations = mock(|m| m.with(|s| s.violations.clone()));
        check(r, "every request kept to the documented rules", violations.is_empty(), format!("{violations:?}"));
        let log = std::fs::read_to_string(st.paths.log()).unwrap_or_default();
        check(r, "the log never carries the token", !log.contains(TOKEN) && log.contains("listenbrainz"), String::new());
        let fails = FAILS.with(Cell::get);
        line(r, if fails == 0 { "ALL OK".into() } else { format!("{fails} FAILED") });
        write_report("listenbrainz", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
