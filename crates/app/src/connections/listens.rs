//! Settings > Connections > ListenBrainz and the listens it is sent. The network, the queue and
//! the account are `listenbrainz::Submitter`'s, on its own thread; when a play counts, when
//! "now playing" goes and whether a track is told at all are `connections/scrobble.rs`'s, shared with
//! Last.fm. This builds a listen from a track (`listen_of`), keeps the Settings card in step and
//! sends "love" for favourites when that switch is on (`love`).
//!
//! The token is pasted once (from the user's ListenBrainz settings page), checked against the
//! server and kept sealed in `listenbrainz.json` as `navidrome.json`'s token is
//! (`platform::secret`); the field is emptied once connected, and the token is never shown,
//! logged or put in the diagnostics. "Server address" sends to a server of one's own that speaks
//! ListenBrainz's API instead (`listenbrainz::api::normalize_server`).
//!
//! `MP_LISTENBRAINZ_API=<url>` points every request at another API root (a local mock), in bench
//! and test builds only (`crate::test_override`): a release build sends the token only where
//! the user's own setting says.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use library::query::Track;
use listenbrainz::{Ids, Listen, State as LbState, Status, Submitter};
use slint::ComponentHandle;

use crate::ListenBrainz;
use crate::state::{self, State};

thread_local! {
    static SUBMITTER: RefCell<Option<Submitter>> = const { RefCell::new(None) };
    static STATUS: RefCell<Option<Status>> = const { RefCell::new(None) };
    /// The token page was asked for (a bench run counts it instead of opening the browser).
    static TOKEN_PAGE_ASKED: Cell<u32> = const { Cell::new(0) };
}

/// The token in `listenbrainz.json` is sealed as `navidrome.json`'s is (`platform::secret`).
fn sealer() -> listenbrainz::Sealer {
    use crate::platform::secret;
    listenbrainz::Sealer { scheme: secret::SCHEME, seal: secret::seal, open: secret::open, write: secret::write_private }
}

fn ids_of(m: library::tags::MusicBrainzIds) -> Ids {
    Ids { recording: m.recording, track: m.release_track, release: m.release, release_group: m.release_group, artists: m.artists }.tidy()
}

pub fn install(st: &Rc<State>) {
    let mut opts = listenbrainz::Options::new(&st.paths.data);
    opts.agent = crate::about::agent();
    opts.version = crate::about::VERSION.into();
    opts.sealer = sealer();
    opts.root = crate::test_override("MP_LISTENBRAINZ_API");
    // The server address shows in its field (never the token).
    let account = listenbrainz::Account::read(&st.paths.data.join(listenbrainz::submitter::ACCOUNT_FILE), &sealer()).0;
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<ListenBrainz>();
        l.set_server(account.server.clone().into());
        l.set_plain_http(listenbrainz::api::is_plain_http(&account.server));
        l.set_own_server(!account.server.is_empty());
        l.set_love(st.settings.borrow().listenbrainz_love);
    }
    start(opts);
}

/// Start the submitter (also the tests', against a stand-in).
fn start(opts: listenbrainz::Options) {
    let s = Submitter::start(
        opts,
        Box::new(|path| ids_of(library::tags::musicbrainz_ids(std::path::Path::new(path)))),
        Box::new(|status| state::post(move |st| on_status(st, status))),
    );
    SUBMITTER.with(|c| *c.borrow_mut() = Some(s));
}

fn with(f: impl FnOnce(&Submitter)) {
    SUBMITTER.with(|c| {
        if let Some(s) = c.borrow().as_ref() {
            f(s);
        }
    });
}

fn status_is(f: impl FnOnce(&Status) -> bool) -> bool {
    STATUS.with(|s| s.borrow().as_ref().is_some_and(f))
}

/// Listens are taken: connected, or waiting to connect again (they are sent then).
fn active() -> bool {
    status_is(|s| s.accepting)
}

/// Connected now: "now playing" and feedback go.
pub fn connected() -> bool {
    status_is(|s| matches!(s.state, LbState::Connected { .. }))
}

pub fn on_status(st: &Rc<State>, status: Status) {
    STATUS.with(|s| *s.borrow_mut() = Some(status.clone()));
    show(st, &status);
}

/// What Settings shows for a status (also driven directly by the bench).
pub fn show(st: &Rc<State>, status: &Status) {
    let Some(app) = st.app.upgrade() else { return };
    let l = app.global::<ListenBrainz>();
    let (text, connected, connecting, user) = describe(status, &l.get_server());
    if connected && !l.get_connected() {
        // Kept sealed now: never left in the field.
        l.set_token("".into());
    }
    l.set_status(text.into());
    l.set_connected(connected);
    l.set_connecting(connecting);
    l.set_user(user.into());
    l.set_attention(needs_attention(status));
}

/// Sending stopped until the user acts: the token is no longer accepted (also after an attempt
/// to connect again that did not work). Marked on Settings' section list.
pub fn needs_attention(s: &Status) -> bool {
    matches!(s.state, LbState::Reconnect) || (s.accepting && matches!(s.state, LbState::Error(_))) || !s.refused.is_empty()
}

/// (status line, connected, checking a token, user name); `server` the address set ("" for
/// ListenBrainz's own).
pub fn describe(s: &Status, server: &str) -> (String, bool, bool, String) {
    let queued = match s.queued {
        0 => String::new(),
        1 => " 1 listen waiting to be sent.".into(),
        n => format!(" {n} listens waiting to be sent."),
    };
    let note = if s.note.is_empty() { String::new() } else { format!(" {}.", s.note.trim_end_matches('.')) };
    let to = match server.split_once("://") {
        Some((_, host)) if !host.is_empty() => format!(" to {host}"),
        _ => String::new(),
    };
    match &s.state {
        LbState::Disconnected => (format!("Paste your user token, then Connect.{queued}"), false, false, String::new()),
        LbState::Connecting => ("Checking the token\u{2026}".into(), false, true, String::new()),
        // The token is fine, the account is not (no verified e-mail address): the server's words.
        LbState::Connected { user } if !s.refused.is_empty() => {
            (format!("Connected as {user}{to}, but ListenBrainz is refusing the listens: {} They are kept and sent once it accepts them.{queued}", s.refused.trim()), true, false, user.clone())
        }
        LbState::Connected { user } => (format!("Sending listens as {user}{to}.{note}{queued}"), true, false, user.clone()),
        LbState::Reconnect => (format!("ListenBrainz no longer accepts this token. Connect again to keep sending listens.{queued}"), false, false, String::new()),
        LbState::Error(e) => (format!("{e}{queued}"), false, false, String::new()),
    }
}

/// A play as ListenBrainz takes it: the artist credit as Last.fm gets it (`scrobble::scrobble_of`:
/// as tagged, the first of several joined), the title, the album as the release, the length and
/// track number, and the MusicBrainz ids: a file's are read from its tags on the submitter's
/// thread (from `path`), a track only the server has takes what the server said (`db`).
pub fn listen_of(t: &Track, started: i64, db: Option<&rusqlite::Connection>) -> Listen {
    let ids = match (t.is_remote(), t.remote_id.as_deref(), db) {
        (true, Some(song), Some(db)) => library::remote::musicbrainz_ids(db, song).map(ids_of).unwrap_or_default(),
        _ => Ids::default(),
    };
    Listen {
        artist: library::names::service_artist(&t.artist),
        track: t.title.clone(),
        release: t.album.clone(),
        duration_ms: (t.duration.max(0.0) * 1000.0).round() as u64,
        track_number: t.track_no,
        ids,
        listened_at: started,
        path: (!t.is_remote()).then(|| t.path.clone()),
    }
}

/// For Copy diagnostics: whether it is connected, to ListenBrainz's own server or another, and
/// the listens waiting. Never the token, the user or the address.
pub fn diagnostics(st: &State) -> String {
    let own = SUBMITTER.with(|c| c.borrow().is_some());
    let server = st.app.upgrade().map(|a| a.global::<ListenBrainz>().get_server().to_string()).unwrap_or_default();
    let place = if server.trim().is_empty() { "listenbrainz.org" } else { "own server" };
    STATUS.with(|s| match s.borrow().as_ref().map(|s| (&s.state, s.queued)) {
        _ if !own => "off".into(),
        Some((LbState::Connected { .. }, q)) => format!("on ({place}, {q} waiting)"),
        Some((LbState::Reconnect, q)) => format!("to connect again ({q} waiting)"),
        Some((LbState::Error(_), q)) if q > 0 => format!("not connected ({q} waiting)"),
        _ => "off".into(),
    })
}

/// "Now playing", while connected.
pub fn now_playing(l: &Listen) {
    if connected() {
        with(|s| s.now_playing(l.clone()));
    }
}

/// The play counted: queued (written to disk at once) and sent, while listens are taken.
pub fn submit(l: Listen) {
    if active() {
        with(|s| s.submit(l));
    }
}

pub fn connect(token: &str, server: &str) {
    with(|s| s.connect(token, server));
}

/// Disconnect: no listen is taken from now on, and the submitter forgets the token and the user
/// and drops the listens waiting (the server address stays in its field and its file).
pub fn disconnect() {
    STATUS.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.accepting = false;
        }
    });
    with(Submitter::disconnect);
}

pub fn retry() {
    with(Submitter::retry);
}

/// The address typed: the plain-http note, and whether ListenBrainz's own token page applies.
pub fn server_edited(st: &Rc<State>, text: &str) {
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<ListenBrainz>();
        l.set_plain_http(listenbrainz::api::is_plain_http(text));
        l.set_own_server(listenbrainz::api::normalize_server(text).map_or(true, |s| !s.is_empty()));
    }
}

/// "Open the token page": ListenBrainz's own settings page, through the app's way of opening a
/// link; never from a bench run.
pub fn open_token_page() {
    TOKEN_PAGE_ASKED.with(|c| c.set(c.get() + 1));
    if crate::bench_run() {
        log::info!("bench run: the ListenBrainz token page was asked for and not opened");
    } else if !crate::platform::shell::open_url(listenbrainz::TOKEN_PAGE) {
        log::warn!("could not open the browser for ListenBrainz's token page");
    }
}

/// How often the token page was asked for (the bench's check).
#[cfg(feature = "bench")]
pub fn token_page_asked() -> u32 {
    TOKEN_PAGE_ASKED.with(Cell::get)
}

pub fn toggle_love(st: &Rc<State>) {
    let on = {
        let mut s = st.settings.borrow_mut();
        s.listenbrainz_love = !s.listenbrainz_love;
        s.listenbrainz_love
    };
    st.save_settings_soon();
    log::info!("listenbrainz: love favourites {}", if on { "on" } else { "off" });
    if let Some(app) = st.app.upgrade() {
        app.global::<ListenBrainz>().set_love(on);
    }
}

/// Favourites were written by the user (`selected::favourites_written`): with "Love favourites
/// on ListenBrainz" on and connected, each track with a MusicBrainz recording id is loved (or,
/// its heart taken away, no longer loved). Best effort: sent while the app runs, not kept on disk.
pub fn love(st: &Rc<State>, ids: &[i64], fav: bool) {
    if !st.settings.borrow().listenbrainz_love || !connected() {
        return;
    }
    let score = i8::from(fav);
    for &id in ids {
        let Ok(Some(t)) = library::query::track(&st.db, id) else { continue };
        let (recording, path) = if t.is_remote() {
            let rec = t.remote_id.as_deref().and_then(|s| library::remote::musicbrainz_ids(&st.db, s).ok()).and_then(|m| m.recording);
            (rec, None)
        } else {
            // As the scan read it; else from the file, on the submitter's thread.
            let rec: Option<String> = st.db.query_row("SELECT mb_track_id FROM track_tags WHERE track_id = ?1", [id], |r| r.get(0)).ok().flatten();
            (rec, Some(t.path.clone()))
        };
        if recording.is_none() && path.is_none() {
            continue;
        }
        with(|s| s.feedback(recording, path, score));
    }
}

/// The tests' submitter (against a stand-in) and its statuses, which they hand to
/// `use_status` (no UI loop runs to post them).
#[cfg(test)]
pub(crate) fn start_for_test(opts: listenbrainz::Options) -> std::sync::mpsc::Receiver<Status> {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let s = Submitter::start(opts, Box::new(|_| Ids::default()), Box::new(move |st| drop(tx.lock().unwrap().send(st))));
    SUBMITTER.with(|c| *c.borrow_mut() = Some(s));
    rx
}

#[cfg(test)]
pub(crate) fn use_status(s: Option<Status>) {
    STATUS.with(|c| *c.borrow_mut() = s);
    if STATUS.with(|c| c.borrow().is_none()) {
        SUBMITTER.with(|c| c.borrow_mut().take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: LbState, queued: usize, note: &str, accepting: bool) -> Status {
        Status { state, queued, note: note.into(), accepting, refused: String::new() }
    }

    #[test]
    fn status_lines() {
        assert_eq!(describe(&status(LbState::Connected { user: "alice".into() }, 0, "Sent 3 listens", true), "").0, "Sending listens as alice. Sent 3 listens.");
        assert_eq!(describe(&status(LbState::Connected { user: "alice".into() }, 2, "", true), "http://192.168.1.10:8100/apis").0, "Sending listens as alice to 192.168.1.10:8100/apis. 2 listens waiting to be sent.");
        assert!(describe(&status(LbState::Connecting, 0, "", false), "").2);
        assert!(describe(&status(LbState::Reconnect, 1, "", true), "").0.contains("Connect again"));
        assert!(describe(&status(LbState::Reconnect, 1, "", true), "").0.ends_with("1 listen waiting to be sent."));
    }

    #[test]
    fn listens_refused_with_a_valid_token_say_why_and_need_attention() {
        let mut s = status(LbState::Connected { user: "alice".into() }, 2, "", true);
        s.refused = "The listens were rejected because your MetaBrainz account does not have a verified email address.".into();
        let (text, connected, ..) = describe(&s, "");
        assert!(connected && needs_attention(&s));
        assert!(text.starts_with("Connected as alice, but ListenBrainz is refusing the listens: The listens were rejected") && text.ends_with("2 listens waiting to be sent."), "{text}");
    }

    #[test]
    fn a_token_refused_needs_attention() {
        assert!(needs_attention(&status(LbState::Reconnect, 0, "", true)));
        assert!(needs_attention(&status(LbState::Error("no".into()), 0, "", true)), "connecting again did not work");
        assert!(!needs_attention(&status(LbState::Error("no".into()), 0, "", false)), "a first token that did not work: said in the card only");
        assert!(!needs_attention(&status(LbState::Connected { user: "a".into() }, 0, "", true)));
        assert!(!needs_attention(&status(LbState::Disconnected, 0, "", false)));
    }

    #[test]
    fn a_listen_carries_the_track() {
        let t = Track { title: "Lantern".into(), artist: "A feat. B".into(), album: "N".into(), album_artist: "A; C".into(), track_no: Some(3), duration: 245.4, path: "M:/a.flac".into(), ..Track::default() };
        let l = listen_of(&t, 1_760_000_000, None);
        assert_eq!(
            l,
            Listen { artist: "A feat. B".into(), track: "Lantern".into(), release: "N".into(), duration_ms: 245_400, track_number: Some(3), ids: Ids::default(), listened_at: 1_760_000_000, path: Some("M:/a.flac".into()) }
        );
        // Several artists joined: the first, as for Last.fm.
        let l = listen_of(&Track { artist: "A; C".into(), ..t.clone() }, 1, None);
        assert_eq!(l.artist, "A");
        // Only on the server: no file to read.
        let l = listen_of(&Track { path: library::remote::song_path("s1"), remote_id: Some("s1".into()), ..t }, 1, None);
        assert_eq!((l.path, l.track.as_str()), (None, "Lantern"));
    }
}
