//! Smart playlists in the app: working them out, keeping what is shown of them up to date,
//! their page, and the editor (`playlists/smartedit.rs` is its draft; `library::smart` the rules).
//!
//! **Off the UI thread.** Every evaluation runs on a thread of its own with its own read
//! connection (as search does): the page, the editor's preview, the Playlists page's counts,
//! a home shelf, and Play / Add to queue / Export from a menu. Requests carry a sequence or
//! the library's generation; only the newest of each kind waiting is worked out, and a result
//! that answers an older request than what is now shown is dropped.
//!
//! **Live.** `changed` is called when what rules look at changes: a scan that changed the
//! library, a favourite, a play counted, a playlist's tracks or name. It moves the
//! generation on and works out again only what is on screen: the open smart playlist, the
//! Playlists page's counts, the editor's preview. Anything else (a home shelf) is worked out
//! when it is next shown. The queue already made from a smart playlist is never changed by
//! this: playing from it copies its tracks, as from any list. Nothing runs on a timer: with
//! nothing changing, nothing is evaluated.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crossbeam_channel::{Sender, unbounded};
use library::smart::{self, Ctx, Outcome, Smart};
use library::{Album, Track, query};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::nav::Route;
use crate::playlists::smartedit::{self, At, Draft, Names, Role};
use crate::state::{self, State};
use crate::{AppWindow, MenuStore, Nav, Page, Picker, Playlists, SmartEdit, SmartList, SmartRow, SmartSlot, views};

/// The preview's rows (the editor shows the start of the list).
const PREVIEW_ROWS: usize = 100;

// ---- time ------------------------------------------------------------------------------

/// Now, in the local time zone (for dates typed in local time and counting back on the
/// calendar): the system's offset at each instant (`platform::system::offset_at`), so a day on
/// the other side of a daylight-saving change starts at its own midnight.
pub fn ctx() -> Ctx {
    let offset_at: fn(i64) -> i64 = crate::platform::system::offset_at;
    smart::set_local_zone(offset_at);
    Ctx::with_zone(library::db::now(), smart::Zone::Rules(offset_at))
}

// ---- the evaluation thread -----------------------------------------------------------------

/// What a menu asked of a smart playlist that is not on screen.
#[derive(Clone, Debug)]
pub enum Act {
    Play { shuffle: bool },
    Enqueue,
    Export { path: PathBuf, relative: bool },
}

enum Job {
    Page { id: i64, smart: Smart, generation: u64, ctx: Ctx },
    Preview { seq: u64, smart: Smart, me: Option<i64>, ctx: Ctx },
    Counts { generation: u64, list: Vec<(i64, Smart)>, ctx: Ctx },
    Shelf { id: i64, smart: Smart, generation: u64, limit: usize, ctx: Ctx },
    Act { id: i64, smart: Smart, act: Act, ctx: Ctx },
    Names,
}

/// The library's names the editor picks values from.
struct Values {
    artists: Vec<String>,
    album_artists: Vec<String>,
    genres: Vec<String>,
    labels: Vec<String>,
    formats: Vec<String>,
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by(|a, b| library::fold::natural_cmp(&library::fold::fold(a), &library::fold::fold(b)).then_with(|| a.cmp(b)));
    v.dedup();
    v
}

fn read_values(conn: &rusqlite::Connection) -> Values {
    let col = |sql: &str| -> Vec<String> {
        conn.prepare(sql).and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect())).unwrap_or_default()
    };
    Values {
        artists: sorted(col("SELECT name FROM artists WHERE id IN (SELECT artist_id FROM track_artists)")),
        album_artists: sorted(col("SELECT name FROM artists WHERE id IN (SELECT artist_id FROM album_artists)")),
        genres: sorted(col("SELECT name FROM genres WHERE id IN (SELECT genre_id FROM track_genres)")),
        labels: sorted(col("SELECT DISTINCT label FROM albums WHERE label IS NOT NULL AND trim(label) <> ''")),
        formats: sorted(col("SELECT DISTINCT codec FROM tracks WHERE codec IS NOT NULL AND codec <> ''")),
    }
}

fn spawn(db: PathBuf) -> Sender<Job> {
    let (tx, rx) = unbounded::<Job>();
    let spawned = std::thread::Builder::new().name("mp-smart".into()).spawn(move || {
        library::background_thread();
        let mut conn = None;
        while let Ok(first) = rx.recv() {
            // The newest of each kind waiting; every menu action, in order.
            let mut jobs = vec![first];
            while let Ok(j) = rx.try_recv() {
                jobs.push(j);
            }
            let (mut page, mut preview, mut counts, mut names) = (None, None, None, false);
            let mut shelves: Vec<Job> = Vec::new();
            let mut acts = Vec::new();
            for j in jobs {
                match j {
                    Job::Page { .. } => page = Some(j),
                    Job::Preview { .. } => preview = Some(j),
                    Job::Counts { .. } => counts = Some(j),
                    Job::Shelf { id, .. } => {
                        shelves.retain(|s| !matches!(s, Job::Shelf { id: other, .. } if *other == id));
                        shelves.push(j);
                    }
                    Job::Act { .. } => acts.push(j),
                    Job::Names => names = true,
                }
            }
            if conn.is_none() {
                match library::db::open(&db) {
                    Ok(c) => conn = Some(c),
                    Err(e) => {
                        log::error!("smart playlists: cannot open the library ({e})");
                        continue;
                    }
                }
            }
            let Some(c) = conn.as_ref() else { continue };
            for j in acts.into_iter().chain(page).chain(preview).chain(counts).chain(shelves) {
                run(c, j);
            }
            if names {
                let v = read_values(c);
                state::post(move |st| got_values(st, v));
            }
        }
    });
    if let Err(e) = spawned {
        log::error!("cannot start the smart playlist thread: {e}");
    }
    tx
}

fn run(conn: &rusqlite::Connection, job: Job) {
    let t = std::time::Instant::now();
    match job {
        Job::Page { id, smart, generation, ctx } => {
            let out = smart::evaluate(conn, &smart, Some(id), ctx).map_err(|e| e.to_string());
            log::debug!("smart playlist {id}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            state::post(move |st| got_page(st, id, generation, out));
        }
        Job::Preview { seq, smart, me, ctx } => {
            let out = smart::evaluate(conn, &smart, me, ctx).map_err(|e| e.to_string());
            log::debug!("smart preview {seq}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            state::post(move |st| got_preview(st, seq, out));
        }
        Job::Counts { generation, list, ctx } => {
            let counts: Vec<(i64, usize, f64)> = list.iter().filter_map(|(id, s)| smart::count(conn, s, Some(*id), ctx).ok().map(|(n, secs)| (*id, n, secs))).collect();
            log::debug!("smart counts of {}: {:.1} ms", list.len(), t.elapsed().as_secs_f64() * 1000.0);
            state::post(move |st| got_counts(st, generation, counts));
        }
        Job::Shelf { id, smart, generation, limit, ctx } => {
            let albums = smart::evaluate(conn, &smart, Some(id), ctx).and_then(|o| query::albums_of_tracks(conn, &o.tracks, limit, false)).unwrap_or_default();
            state::post(move |st| got_shelf(st, id, generation, limit, albums));
        }
        Job::Act { id, smart, act, ctx } => {
            let tracks = smart::evaluate(conn, &smart, Some(id), ctx).map(|o| o.tracks).unwrap_or_default();
            state::post(move |st| got_act(st, id, act, tracks));
        }
        Job::Names => {}
    }
}

// ---- what the UI thread keeps -----------------------------------------------------------------

/// A line of the editor and its controls' model, kept so that typing in a control updates
/// it in place (the control keeps the keyboard).
struct Lines {
    model: Rc<VecModel<SmartRow>>,
    slots: Vec<Rc<VecModel<SmartSlot>>>,
    at: Vec<At>,
    rows: Vec<smartedit::Row>,
}

impl Default for Lines {
    fn default() -> Self {
        Lines { model: Rc::new(VecModel::default()), slots: Vec::new(), at: Vec::new(), rows: Vec::new() }
    }
}

struct PickerState {
    key: i32,
    role: Role,
    at: At,
    labels: Vec<String>,
    /// The entries shown (the filter's), as indices into `labels`.
    shown: Vec<usize>,
    checked: Option<usize>,
    /// What `labels` were made from: the entry picked is this one's, even if the library's
    /// names or the playlists change while the list is open.
    names: Names,
}

#[derive(Default)]
struct Ui {
    tx: Option<Sender<Job>>,
    /// Moves on whenever what rules look at changes.
    generation: u64,
    /// The smart playlists last worked out for their page: (generation, tracks, problems),
    /// the few most recent, so Back to one shows it at once and where it was scrolled.
    pages: Vec<(i64, u64, Vec<Track>, Vec<String>)>,
    page_asked: Option<(i64, u64)>,
    counts: HashMap<i64, (u64, usize, f64)>,
    counts_asked: Option<u64>,
    shelves: HashMap<i64, (u64, usize, Vec<Album>)>,
    shelves_asked: HashSet<(i64, u64)>,
    preview_seq: u64,
    draft: Option<Draft>,
    values: Option<Values>,
    values_asked: bool,
    rules: Lines,
    tail: Lines,
    picker: Option<PickerState>,
    preview_timer: Option<slint::Timer>,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::default();
}

fn send(job: Job) {
    UI.with(|u| {
        if let Some(tx) = &u.borrow().tx {
            let _ = tx.send(job);
        }
    });
}

pub fn install(st: &Rc<State>) {
    let tx = spawn(st.paths.database());
    UI.with(|u| u.borrow_mut().tx = Some(tx));
}

/// The rules of playlist `id` when it is a smart one.
pub fn rules_of(st: &State, id: i64) -> Option<Smart> {
    match smart::playlist_rules(&st.db, id) {
        Ok(Some(Some(json))) => Some(Smart::from_json(&json)),
        _ => None,
    }
}

pub fn is_smart(st: &State, id: i64) -> bool {
    matches!(smart::playlist_rules(&st.db, id), Ok(Some(Some(_))))
}

/// What rules look at changed (see the module's notes): work out again what shows.
pub fn changed(st: &Rc<State>) {
    UI.with(|u| u.borrow_mut().generation += 1);
    let Some(app) = st.app.upgrade() else { return };
    let current = st.history.borrow().current.clone();
    match (app.global::<Nav>().get_page(), current) {
        (Page::Smart, Route::Playlist(id)) => ask_page(st, id),
        (Page::Playlists, _) => views::refresh_playlists(st),
        (Page::SmartEdit, _) => schedule_preview(st),
        _ => {}
    }
}

fn generation() -> u64 {
    UI.with(|u| u.borrow().generation)
}

// ---- the page ---------------------------------------------------------------------------

/// "142 tracks · 9 h 12 min".
fn info(tracks: &[Track]) -> String {
    let n = tracks.len();
    if n == 0 {
        return "No tracks match its rules now".into();
    }
    let total: f64 = tracks.iter().map(|t| t.duration).sum();
    format!("{n} {} \u{b7} {}", if n == 1 { "track" } else { "tracks" }, views::duration_text(total))
}

fn names_of_playlists(st: &State) -> impl Fn(i64) -> Option<String> + '_ {
    move |id| query::playlist_name(&st.db, id).ok().flatten()
}

/// Show smart playlist `id` on its page: what was last worked out for it at once (if
/// anything), and work it out again when the library has changed since.
pub fn fill_page(st: &Rc<State>, id: i64) -> bool {
    let Some(app) = st.app.upgrade() else { return false };
    let Ok(Some(name)) = query::playlist_name(&st.db, id) else { return false };
    let Some(rules) = rules_of(st, id) else { return false };
    let p = app.global::<SmartList>();
    p.set_id(id as i32);
    p.set_name(name.as_str().into());
    p.set_summary(smart::summary(&rules, &names_of_playlists(st)).into());
    p.set_random(rules.random());
    app.global::<Playlists>().set_current_id(id as i32);
    app.global::<Playlists>().set_current_name(name.into());
    app.global::<Playlists>().set_used_by(crate::views::used_by(st, id).into());
    st.open_playlist.set(Some(id));
    let cached = UI.with(|u| u.borrow().pages.iter().find(|c| c.0 == id).map(|c| (c.1, c.2.clone(), c.3.clone())));
    match cached {
        Some((generation_then, tracks, problems)) => {
            show_tracks(st, &tracks, &problems);
            if generation_then != generation() {
                ask_page(st, id);
            }
        }
        None => {
            show_tracks(st, &[], &rules.problems());
            p.set_info("Working it out\u{2026}".into());
            ask_page(st, id);
        }
    }
    true
}

fn show_tracks(st: &Rc<State>, tracks: &[Track], problems: &[String]) {
    let Some(app) = st.app.upgrade() else { return };
    let favs = views::favourite_set(st, tracks.iter().map(|t| t.id));
    st.lists.smart.set(tracks.to_vec(), favs, false);
    st.lists.smart.set_playing(views::playing_track_id(st));
    let p = app.global::<SmartList>();
    p.set_info(info(tracks).into());
    let rows: Vec<slint::SharedString> = problems.iter().map(|s| s.as_str().into()).collect();
    p.set_problems(ModelRc::new(VecModel::from(rows)));
    p.set_tracks(ModelRc::from(st.lists.smart.clone()));
}

fn ask_page(st: &Rc<State>, id: i64) {
    let Some(smart) = rules_of(st, id) else { return };
    let generation = generation();
    let asked = UI.with(|u| u.borrow().page_asked);
    if asked == Some((id, generation)) {
        return;
    }
    UI.with(|u| u.borrow_mut().page_asked = Some((id, generation)));
    send(Job::Page { id, smart, generation, ctx: ctx() });
}

fn got_page(st: &Rc<State>, id: i64, generation_then: u64, out: Result<Outcome, String>) {
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            log::warn!("smart playlist {id}: {e}");
            Outcome { tracks: Vec::new(), problems: vec![format!("It could not be worked out: {e}")] }
        }
    };
    let same = UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.page_asked == Some((id, generation_then)) {
            u.page_asked = None;
        }
        let same = u.pages.iter().find(|c| c.0 == id).is_some_and(|c| c.2 == out.tracks && c.3 == out.problems);
        u.pages.retain(|c| c.0 != id);
        u.pages.insert(0, (id, generation_then, out.tracks.clone(), out.problems.clone()));
        u.pages.truncate(4);
        same
    });
    let showing = st.app.upgrade().is_some_and(|app| app.global::<Nav>().get_page() == Page::Smart && app.global::<SmartList>().get_id() == id as i32)
        && st.history.borrow().current == Route::Playlist(id);
    // The same tracks as shown: nothing to redraw (the list keeps its rows, cursor and scroll).
    if showing && !(same && st.lists.smart.row_count() == out.tracks.len()) {
        show_tracks(st, &out.tracks, &out.problems);
    } else if showing && let Some(app) = st.app.upgrade() {
        app.global::<SmartList>().set_info(info(&out.tracks).into());
    }
}

/// What smart playlist `id` held when last counted (the Playlists page's count), if it
/// was counted (the name clash prompt says it).
pub fn known_count(id: i64) -> Option<usize> {
    UI.with(|u| u.borrow().counts.get(&id).map(|c| c.1))
}

/// The open smart playlist's tracks as shown (Play, Shuffle, Save as playlist).
pub fn shown_tracks(st: &State) -> Vec<Track> {
    st.lists.smart.tracks()
}

/// Delete on a row, or Alt+Up / Down: a smart playlist's order and contents are its rules'.
pub fn hint(st: &Rc<State>) {
    views::flash_long(st, "A smart playlist fills itself from its rules: edit them to change what is in it");
}

/// A new shuffle for the open smart playlist (its random order or selection).
pub fn reshuffle(st: &Rc<State>) {
    let Some(id) = st.open_playlist.get() else { return };
    let Some(mut s) = rules_of(st, id) else { return };
    s.seed = new_seed();
    if let Err(e) = query::set_playlist_rules(&st.db, id, &s.to_json()) {
        log::warn!("reshuffling {id}: {e}");
        return;
    }
    UI.with(|u| u.borrow_mut().pages.retain(|c| c.0 != id));
    changed(st);
}

pub fn new_seed() -> i64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    ((t as i64) ^ ((t >> 64) as i64)).wrapping_mul(0x5851_F42D_4C95_7F2D)
}

// ---- elsewhere: Playlists page counts, home shelves, menu actions ---------------------------------

/// The Playlists page's and the sidebar's row of a playlist: a smart one with its rules in
/// words and its count as last worked out; asks for the counts that are out of date (once
/// per generation: this runs when the playlists are shown or change, never on a timer).
pub fn playlist_rows(lists: &[query::Playlist]) -> Vec<crate::PlaylistRow> {
    let generation = generation();
    let names = |id: i64| lists.iter().find(|p| p.id == id).map(|p| p.name.clone());
    let mut stale = Vec::new();
    let rows = lists
        .iter()
        .map(|p| {
            let (summary, info) = match &p.rules {
                None => (String::new(), format!("{} {}", p.tracks, if p.tracks == 1 { "track" } else { "tracks" })),
                Some(json) => {
                    let s = Smart::from_json(json);
                    let count = UI.with(|u| u.borrow().counts.get(&p.id).copied());
                    if count.is_none_or(|c| c.0 != generation) {
                        stale.push((p.id, s.clone()));
                    }
                    let info = count.map_or(String::new(), |(_, n, _)| format!("{n} {}", if n == 1 { "track" } else { "tracks" }));
                    (smart::summary(&s, &names), info)
                }
            };
            crate::PlaylistRow { id: p.id as i32, name: p.name.as_str().into(), count: p.tracks as i32, smart: p.rules.is_some(), summary: summary.into(), info: info.into() }
        })
        .collect();
    if !stale.is_empty() && UI.with(|u| u.borrow().counts_asked) != Some(generation) {
        UI.with(|u| u.borrow_mut().counts_asked = Some(generation));
        send(Job::Counts { generation, list: stale, ctx: ctx() });
    }
    rows
}

fn got_counts(st: &Rc<State>, generation_then: u64, counts: Vec<(i64, usize, f64)>) {
    UI.with(|u| {
        let mut u = u.borrow_mut();
        for (id, n, secs) in counts {
            u.counts.insert(id, (generation_then, n, secs));
        }
        if u.counts_asked == Some(generation_then) {
            u.counts_asked = None;
        }
    });
    views::refresh_playlists(st);
}

/// A home shelf of smart playlist `id`: its albums as last worked out (None before the
/// first time), and asked for again when the library has changed since.
pub fn shelf_albums(st: &Rc<State>, id: i64, limit: usize) -> Option<Vec<Album>> {
    let generation = generation();
    let cached = UI.with(|u| u.borrow().shelves.get(&id).map(|c| (c.0, c.1, c.2.clone())));
    let fresh = cached.as_ref().is_some_and(|c| c.0 == generation && c.1 >= limit);
    if !fresh
        && UI.with(|u| !u.borrow().shelves_asked.contains(&(id, generation)))
        && let Some(smart) = rules_of(st, id)
    {
        UI.with(|u| u.borrow_mut().shelves_asked.insert((id, generation)));
        send(Job::Shelf { id, smart, generation, limit, ctx: ctx() });
    }
    cached.map(|c| c.2.into_iter().take(limit).collect())
}

fn got_shelf(st: &Rc<State>, id: i64, generation_then: u64, limit: usize, albums: Vec<Album>) {
    let changed = UI.with(|u| {
        let mut u = u.borrow_mut();
        u.shelves_asked.remove(&(id, generation_then));
        let changed = u.shelves.get(&id).is_none_or(|c| c.2 != albums);
        u.shelves.insert(id, (generation_then, limit, albums));
        changed
    });
    if changed && st.app.upgrade().is_some_and(|app| app.global::<Nav>().get_page() == Page::Home) {
        views::refresh_home(st);
    }
}

/// Play, queue or export smart playlist `id` from a menu (its tracks worked out first).
pub fn act(st: &Rc<State>, id: i64, act: Act) {
    // On its own page: the tracks shown, at once.
    if st.history.borrow().current == Route::Playlist(id) && st.app.upgrade().is_some_and(|a| a.global::<Nav>().get_page() == Page::Smart) {
        got_act(st, id, act, shown_tracks(st));
        return;
    }
    let Some(smart) = rules_of(st, id) else { return };
    send(Job::Act { id, smart, act, ctx: ctx() });
}

fn got_act(st: &Rc<State>, id: i64, act: Act, tracks: Vec<Track>) {
    match act {
        Act::Play { shuffle: false } => crate::playback::play_tracks(st, tracks, 0),
        Act::Play { shuffle: true } => crate::playback::shuffle_tracks(st, tracks),
        Act::Enqueue => {
            let n = tracks.len();
            crate::playback::enqueue(st, tracks, false);
            views::flash(st, &format!("Added {n} {} to the queue", if n == 1 { "track" } else { "tracks" }));
        }
        Act::Export { path, relative } => {
            crate::playlists::file::write_tracks(st, &tracks, &path, relative);
        }
    }
    log::debug!("smart playlist {id}: acted");
}

// ---- the editor -------------------------------------------------------------------------

/// "New smart playlist…": a fresh draft in the editor.
pub fn open_new(st: &Rc<State>) {
    let mut d = Draft::new();
    d.seed = new_seed();
    UI.with(|u| u.borrow_mut().draft = Some(d));
    views::navigate(st, Route::SmartEdit(-1));
}

/// "Save as smart playlist" on a search with operators: the editor with the query's rules
/// (`library::smart::search`) and the query as the name, not saved yet, so the rules can be
/// read and changed before Save makes the playlist.
pub fn open_from_search(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let q = app.global::<crate::Search>().get_query().trim().to_owned();
    let parsed = smart::search::parse_in(&st.db, &q);
    if !parsed.has_operators() {
        return;
    }
    let Some(mut s) = parsed.rules(smart::search::Level::Tracks) else { return };
    s.seed = new_seed();
    let taken: Vec<String> = query::playlists(&st.db).unwrap_or_default().into_iter().map(|p| p.name).collect();
    let base: String = if q.chars().count() > 60 { format!("{}\u{2026}", q.chars().take(59).collect::<String>().trim_end()) } else { q.clone() };
    let mut d = Draft::from_smart(None, &smartedit::unique_name(&base, &taken), &s);
    d.dirty = true;
    UI.with(|u| u.borrow_mut().draft = Some(d));
    views::navigate(st, Route::SmartEdit(-1));
    let e = app.global::<SmartEdit>();
    e.set_name_focus_seq(e.get_name_focus_seq() + 1);
    views::flash(st, "The search as rules, not saved yet: check them, name it, then Save");
}

/// Edit smart playlist `id`'s rules.
pub fn open_edit(st: &Rc<State>, id: i64) {
    UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.draft.as_ref().is_some_and(|d| d.id != Some(id)) {
            u.draft = None;
        }
    });
    views::navigate(st, Route::SmartEdit(id));
}

/// A copy of smart playlist `id` (or of the draft being edited), in the editor, not saved yet.
pub fn duplicate(st: &Rc<State>, from: Option<i64>) {
    let base = match from {
        Some(id) => rules_of(st, id).map(|s| (query::playlist_name(&st.db, id).ok().flatten().unwrap_or_default(), s)),
        None => UI.with(|u| u.borrow().draft.as_ref().and_then(|d| d.smart().ok().map(|s| (d.name.clone(), s)))),
    };
    let Some((name, s)) = base else {
        if from.is_none() {
            set_save_error(st, "Fix the rule marked below first: a copy is made of rules that work.");
        }
        return;
    };
    let taken: Vec<String> = query::playlists(&st.db).unwrap_or_default().into_iter().map(|p| p.name).collect();
    let mut d = Draft::from_smart(None, &smartedit::unique_name(&format!("{} copy", name.trim()), &taken), &s);
    d.dirty = true;
    UI.with(|u| u.borrow_mut().draft = Some(d));
    if from.is_none() && matches!(st.history.borrow().current, Route::SmartEdit(_)) {
        views::replace(st, Route::SmartEdit(-1));
    } else {
        views::navigate(st, Route::SmartEdit(-1));
    }
    if let Some(app) = st.app.upgrade() {
        let e = app.global::<SmartEdit>();
        e.set_name_focus_seq(e.get_name_focus_seq() + 1);
    }
    views::flash(st, "A copy, not saved yet: rename it, change it, then Save");
}

/// Show the editor for route `SmartEdit(id)`: the draft kept for it (Back and Forward), or
/// one made from the stored rules (or a new one).
pub fn fill_editor(st: &Rc<State>, id: i64) -> bool {
    let want = (id > 0).then_some(id);
    let keep = UI.with(|u| u.borrow().draft.as_ref().is_some_and(|d| d.id == want));
    if !keep {
        let d = match want {
            Some(id) => {
                let (Some(s), Ok(Some(name))) = (rules_of(st, id), query::playlist_name(&st.db, id)) else { return false };
                Draft::from_smart(Some(id), &name, &s)
            }
            None => {
                let mut d = Draft::new();
                d.seed = new_seed();
                d
            }
        };
        UI.with(|u| u.borrow_mut().draft = Some(d));
    }
    let Some(app) = st.app.upgrade() else { return false };
    let e = app.global::<SmartEdit>();
    let (name, new) = UI.with(|u| u.borrow().draft.as_ref().map(|d| (d.name.clone(), d.id.is_none()))).unwrap_or_default();
    e.set_is_new(new);
    e.set_can_duplicate(!new);
    e.set_name(name.as_str().into());
    e.set_name_error("".into());
    e.set_save_error("".into());
    let templates: Vec<slint::SharedString> = smart::templates().into_iter().map(|(n, _)| n.into()).collect();
    let (first, more) = templates.split_at(templates.len().div_ceil(2));
    e.set_templates(ModelRc::new(VecModel::from(first.to_vec())));
    e.set_templates_more(ModelRc::new(VecModel::from(more.to_vec())));
    app.global::<Playlists>().set_current_id(want.map_or(-1, |id| id as i32));
    if !UI.with(|u| u.borrow().values_asked) {
        UI.with(|u| u.borrow_mut().values_asked = true);
        send(Job::Names);
    }
    rebuild(st);
    schedule_preview(st);
    if new && name.is_empty() {
        e.set_name_focus_seq(e.get_name_focus_seq() + 1);
    }
    true
}

fn got_values(st: &Rc<State>, v: Values) {
    UI.with(|u| {
        let mut u = u.borrow_mut();
        u.values = Some(v);
        u.values_asked = false;
    });
    // A scan may change them: asked again the next time the editor opens.
    if st.app.upgrade().is_some_and(|app| app.global::<Nav>().get_page() == Page::SmartEdit) {
        refresh(st);
    }
}

/// What the editor picks from: the library's names (read on the evaluation thread when the
/// editor opens; here if a list is opened before they arrive), the playlists, and those a
/// rule here may not name.
fn names(st: &State) -> Names {
    if UI.with(|u| u.borrow().values.is_none()) {
        let v = read_values(&st.db);
        UI.with(|u| u.borrow_mut().values = Some(v));
    }
    let me = UI.with(|u| u.borrow().draft.as_ref().and_then(|d| d.id));
    let playlists: Vec<(i64, String, bool)> = query::playlists(&st.db).unwrap_or_default().into_iter().map(|p| (p.id, p.name, p.rules.is_some())).collect();
    let excluded: Vec<i64> = playlists
        .iter()
        .filter(|p| {
            Some(p.0) == me || (p.2 && me.is_some() && {
                let probe = Smart { root: smart::Group { all: true, nodes: vec![smart::Node::Cond(smart::Cond::new(smart::Field::Playlist, smart::Op::In, smart::Value::Id(p.0)))] }, ..Smart::default() };
                smart::would_cycle(&st.db, me, &probe).is_some()
            })
        })
        .map(|p| p.0)
        .collect();
    UI.with(|u| {
        let u = u.borrow();
        let v = u.values.as_ref();
        Names {
            artists: v.map(|v| v.artists.clone()).unwrap_or_default(),
            album_artists: v.map(|v| v.album_artists.clone()).unwrap_or_default(),
            genres: v.map(|v| v.genres.clone()).unwrap_or_default(),
            labels: v.map(|v| v.labels.clone()).unwrap_or_default(),
            formats: v.map(|v| v.formats.clone()).unwrap_or_default(),
            playlists,
            excluded,
        }
    })
}

fn to_slint_slot(s: &smartedit::Slot) -> SmartSlot {
    SmartSlot {
        kind: s.kind,
        role: s.role,
        text: s.text.as_str().into(),
        placeholder: s.placeholder.as_str().into(),
        label: s.label.as_str().into(),
        width: s.width,
        invalid: s.invalid,
        on: s.on,
    }
}

fn to_slint_row(r: &smartedit::Row, slots: &Rc<VecModel<SmartSlot>>) -> SmartRow {
    SmartRow {
        kind: r.kind,
        depth: r.depth,
        slots: ModelRc::from(slots.clone()),
        error: r.error.as_str().into(),
        note: r.note.as_str().into(),
        can_up: r.can_up,
        can_down: r.can_down,
        removable: r.removable,
        label: r.label.as_str().into(),
    }
}

fn lines(rows: Vec<(At, smartedit::Row)>) -> Lines {
    let mut l = Lines::default();
    for (at, row) in rows {
        let slots = Rc::new(VecModel::from(row.slots.iter().map(to_slint_slot).collect::<Vec<_>>()));
        l.model.push(to_slint_row(&row, &slots));
        l.slots.push(slots);
        l.at.push(at);
        l.rows.push(row);
    }
    l
}

/// Update `lines` to `rows` in place: each line and control whose content changed, keeping
/// the models (so a control being typed in is not made again). False when the lines are not
/// the same ones (count, controls), which needs `rebuild`.
fn update(lines: &mut Lines, rows: Vec<(At, smartedit::Row)>) -> bool {
    if rows.len() != lines.rows.len() || rows.iter().zip(&lines.rows).any(|((_, a), b)| a.slots.len() != b.slots.len() || a.slots.iter().zip(&b.slots).any(|(x, y)| x.kind != y.kind || x.role != y.role)) {
        return false;
    }
    for (i, (at, row)) in rows.into_iter().enumerate() {
        if row == lines.rows[i] {
            continue;
        }
        for (j, s) in row.slots.iter().enumerate() {
            if *s != lines.rows[i].slots[j] {
                lines.slots[i].set_row_data(j, to_slint_slot(s));
            }
        }
        lines.model.set_row_data(i, to_slint_row(&row, &lines.slots[i]));
        lines.at[i] = at;
        lines.rows[i] = row;
    }
    true
}

/// Make the editor's lines again from the draft (after a change of what they are).
fn rebuild(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let n = names(st);
    let Some((rules, tail)) = UI.with(|u| u.borrow().draft.as_ref().map(|d| (d.rule_rows(&n), d.tail_rows()))) else { return };
    let (rules, tail) = (lines(rules), lines(tail));
    let e = app.global::<SmartEdit>();
    e.set_rows(ModelRc::from(rules.model.clone()));
    e.set_tail(ModelRc::from(tail.model.clone()));
    UI.with(|u| {
        let mut u = u.borrow_mut();
        u.rules = rules;
        u.tail = tail;
    });
}

/// Bring the editor's lines up to date with the draft, in place where it can be.
fn refresh(st: &Rc<State>) {
    let n = names(st);
    let Some((rules, tail)) = UI.with(|u| u.borrow().draft.as_ref().map(|d| (d.rule_rows(&n), d.tail_rows()))) else { return };
    let ok = UI.with(|u| {
        let mut u = u.borrow_mut();
        let a = update(&mut u.rules, rules);
        let b = update(&mut u.tail, tail);
        a && b
    });
    if !ok {
        rebuild(st);
    }
}

/// Where the editor's line `key` is in the draft.
fn at_of(key: i32) -> Option<At> {
    UI.with(|u| {
        let u = u.borrow();
        if key >= 1000 { u.tail.at.get((key - 1000) as usize).cloned() } else { u.rules.at.get(usize::try_from(key).ok()?).cloned() }
    })
}

/// The key of the line at `at` now.
fn key_of(at: &At) -> Option<i32> {
    UI.with(|u| {
        let u = u.borrow();
        u.rules.at.iter().position(|a| a == at).map(|i| i as i32).or_else(|| u.tail.at.iter().position(|a| a == at).map(|i| 1000 + i as i32))
    })
}

/// Give a control the keyboard (now, and again once the lines just made exist).
fn focus(st: &Rc<State>, key: i32, role: Role) {
    let Some(app) = st.app.upgrade() else { return };
    let e = app.global::<SmartEdit>();
    e.set_focus_key(key);
    e.set_focus_role(role.index());
    e.set_focus_seq(e.get_focus_seq() + 1);
    let weak = st.app.clone();
    slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
        if let Some(app) = weak.upgrade() {
            let e = app.global::<SmartEdit>();
            e.set_focus_seq(e.get_focus_seq() + 1);
        }
    });
}

/// The first control of the line at `at` (its field, or a group's all / any).
fn focus_line(st: &Rc<State>, at: &At) {
    let Some(key) = key_of(at) else { return };
    let role = UI.with(|u| {
        let u = u.borrow();
        let lines = if key >= 1000 { &u.tail } else { &u.rules };
        let i = if key >= 1000 { key - 1000 } else { key } as usize;
        lines.rows.get(i).and_then(|r| r.slots.iter().find(|s| s.kind != smartedit::WORDS)).and_then(|s| Role::from_index(s.role))
    });
    if let Some(role) = role {
        focus(st, key, role);
    }
}

/// The control of the line at `at` whose value is wrong (else its first control).
fn focus_invalid(st: &Rc<State>, at: &At) {
    let Some(key) = key_of(at) else { return };
    let role = UI.with(|u| {
        let u = u.borrow();
        let lines = if key >= 1000 { &u.tail } else { &u.rules };
        let i = if key >= 1000 { key - 1000 } else { key } as usize;
        lines.rows.get(i).and_then(|r| r.slots.iter().find(|s| s.invalid)).and_then(|s| Role::from_index(s.role))
    });
    match role {
        Some(role) => focus(st, key, role),
        None => focus_line(st, at),
    }
}

fn with_draft<R>(f: impl FnOnce(&mut Draft) -> R) -> Option<R> {
    UI.with(|u| u.borrow_mut().draft.as_mut().map(f))
}

fn edited(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        app.global::<SmartEdit>().set_save_error("".into());
    }
    schedule_preview(st);
}

/// The preview a moment after the last change (typing does not ask for one per key).
fn schedule_preview(st: &Rc<State>) {
    let timer = slint::Timer::default();
    let weak = Rc::downgrade(st);
    timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(150), move || {
        if let Some(st) = weak.upgrade() {
            ask_preview(&st);
        }
    });
    UI.with(|u| u.borrow_mut().preview_timer = Some(timer));
}

fn ask_preview(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let Some(draft) = UI.with(|u| u.borrow().draft.clone()) else { return };
    let e = app.global::<SmartEdit>();
    match draft.smart() {
        Ok(s) => {
            let seq = UI.with(|u| {
                let mut u = u.borrow_mut();
                u.preview_seq += 1;
                u.preview_seq
            });
            if e.get_count_attention() || e.get_count().is_empty() {
                e.set_count("Working it out\u{2026}".into());
                e.set_count_attention(false);
            }
            send(Job::Preview { seq, smart: s, me: draft.id, ctx: ctx() });
        }
        Err((_, why)) => {
            UI.with(|u| u.borrow_mut().preview_seq += 1);
            e.set_count(format!("Nothing to show yet: {}", lower_first(&why)).into());
            e.set_count_attention(true);
            e.set_preview_note("".into());
            st.lists.preview.set(Vec::new(), HashSet::new(), false);
            e.set_preview(ModelRc::from(st.lists.preview.clone()));
        }
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn got_preview(st: &Rc<State>, seq: u64, out: Result<Outcome, String>) {
    if UI.with(|u| u.borrow().preview_seq) != seq {
        return;
    }
    let Some(app) = st.app.upgrade() else { return };
    let e = app.global::<SmartEdit>();
    let out = match out {
        Ok(o) => o,
        Err(e2) => {
            e.set_count(format!("It could not be worked out: {e2}").into());
            e.set_count_attention(true);
            return;
        }
    };
    let n = out.tracks.len();
    let total: f64 = out.tracks.iter().map(|t| t.duration).sum();
    e.set_count(if n == 0 { "Matches no tracks now".into() } else { format!("Matches {n} {} \u{b7} {}", if n == 1 { "track" } else { "tracks" }, views::duration_text(total)).into() });
    e.set_count_attention(false);
    let mut note = Vec::new();
    if n > PREVIEW_ROWS {
        note.push(format!("The first {PREVIEW_ROWS} of {n}, in the playlist\u{2019}s order."));
    }
    note.extend(out.problems.iter().cloned());
    e.set_preview_note(note.join(" ").into());
    let rows: Vec<Track> = out.tracks.into_iter().take(PREVIEW_ROWS).collect();
    let favs = views::favourite_set(st, rows.iter().map(|t| t.id));
    st.lists.preview.set(rows, favs, false);
    st.lists.preview.set_playing(views::playing_track_id(st));
    e.set_preview(ModelRc::from(st.lists.preview.clone()));
}

fn set_save_error(st: &Rc<State>, text: &str) {
    if let Some(app) = st.app.upgrade() {
        app.global::<SmartEdit>().set_save_error(text.into());
    }
}

/// Save: refused, with the reason where it is, while a rule says nothing yet, the name is
/// empty or another playlist's, or a rule would make the playlist include itself.
pub fn save(st: &Rc<State>) {
    let Some(app) = st.app.upgrade() else { return };
    let e = app.global::<SmartEdit>();
    let Some(draft) = UI.with(|u| u.borrow().draft.clone()) else { return };
    let name = draft.name.trim().to_owned();
    if name.is_empty() {
        e.set_name_error("Name the playlist first.".into());
        e.set_name_focus_seq(e.get_name_focus_seq() + 1);
        return;
    }
    if let Some(other) = query::playlists(&st.db).unwrap_or_default().into_iter().find(|p| Some(p.id) != draft.id && crate::playlists::newplaylist::same_name(&p.name, &name)) {
        e.set_name_error(format!("There is already a playlist called \u{201c}{}\u{201d}: give this one another name.", other.name).into());
        e.set_name_focus_seq(e.get_name_focus_seq() + 1);
        return;
    }
    let s = match draft.smart() {
        Ok(s) => s,
        Err((at, why)) => {
            // Every line's problem shows now, not only those already touched.
            with_draft(|d| touch_all(&mut d.items));
            refresh(st);
            e.set_save_error(format!("Not saved yet: {}", lower_first(&why)).into());
            focus_invalid(st, &at);
            return;
        }
    };
    if let Some(via) = smart::would_cycle(&st.db, draft.id, &s) {
        e.set_save_error(format!("Not saved: \u{201c}{via}\u{201d} would bring this playlist into itself. Choose another playlist in that rule.").into());
        return;
    }
    let json = s.to_json();
    let saved = match draft.id {
        Some(id) => query::set_playlist_rules(&st.db, id, &json).and_then(|()| query::rename_playlist(&st.db, id, &name)).map(|()| id),
        None => query::create_smart_playlist(&st.db, &name, &json),
    };
    let id = match saved {
        Ok(id) => id,
        Err(err) => {
            log::warn!("saving smart playlist {name:?}: {err}");
            e.set_save_error(format!("Could not save it: {err}").into());
            return;
        }
    };
    log::info!("smart playlist {name:?} ({id}) saved: {json}");
    UI.with(|u| {
        let mut u = u.borrow_mut();
        u.draft = None;
        u.pages.retain(|c| c.0 != id);
        u.shelves.remove(&id);
    });
    views::playlists_changed(st);
    views::replace(st, Route::Playlist(id));
    views::flash(st, &format!("Saved \u{201c}{name}\u{201d}"));
}

fn touch_all(items: &mut [smartedit::Item]) {
    for it in items {
        match it {
            smartedit::Item::Line(l) => l.touched = true,
            smartedit::Item::Group { items, .. } => touch_all(items),
            smartedit::Item::Unknown(_) => {}
        }
    }
}

/// Cancel: the draft goes, and so does the editor.
pub fn cancel(st: &Rc<State>) {
    UI.with(|u| u.borrow_mut().draft = None);
    if !views::back(st) {
        views::go(st, Page::Playlists);
    }
}

// ---- the editor's list (Picker) ------------------------------------------------------------

fn open_choice(st: &Rc<State>, key: i32, role: i32, x: f32, y: f32) {
    let (Some(at), Some(role)) = (at_of(key), Role::from_index(role)) else { return };
    let n = names(st);
    let Some(o) = UI.with(|u| u.borrow().draft.as_ref().map(|d| d.options(&at, role, &n))) else { return };
    if o.labels.is_empty() {
        let what = if role == Role::A { "There is nothing in the library to choose from yet" } else { "Nothing to choose" };
        views::flash(st, what);
        return;
    }
    let Some(app) = st.app.upgrade() else { return };
    let p = app.global::<Picker>();
    p.set_x(x);
    p.set_y(y);
    p.set_title(o.title.as_str().into());
    p.set_filterable(o.filterable && o.labels.len() > 8);
    p.set_filter("".into());
    set_items(&app, &o.labels, &(0..o.labels.len()).collect::<Vec<_>>(), o.checked);
    p.set_highlight(o.checked.map_or(0, |c| c as i32));
    p.set_open(true);
    UI.with(|u| u.borrow_mut().picker = Some(PickerState { key, role, at, shown: (0..o.labels.len()).collect(), labels: o.labels, checked: o.checked, names: n }));
}

fn set_items(app: &AppWindow, labels: &[String], shown: &[usize], checked: Option<usize>) {
    let p = app.global::<Picker>();
    let items: Vec<slint::SharedString> = shown.iter().map(|&i| labels[i].as_str().into()).collect();
    p.set_items(ModelRc::new(VecModel::from(items)));
    p.set_checked(checked.and_then(|c| shown.iter().position(|&i| i == c)).map_or(-1, |i| i as i32));
}

fn picker_filter(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let shown = UI.with(|u| {
        let mut u = u.borrow_mut();
        let p = u.picker.as_mut()?;
        p.shown = smartedit::filter(&p.labels, text);
        Some((p.labels.clone(), p.shown.clone(), p.checked))
    });
    if let Some((labels, shown, checked)) = shown {
        set_items(&app, &labels, &shown, checked);
        app.global::<Picker>().set_highlight(0);
    }
}

fn picker_pick(st: &Rc<State>, i: i32) {
    let Some(p) = UI.with(|u| u.borrow_mut().picker.take()) else { return };
    close_picker(st, None);
    let Some(&orig) = p.shown.get(usize::try_from(i).unwrap_or(usize::MAX)) else {
        focus(st, p.key, p.role);
        return;
    };
    with_draft(|d| d.choose(&p.at, p.role, orig, &p.names));
    rebuild(st);
    edited(st);
    // The same control has the keyboard again (its line may have new neighbours).
    match key_of(&p.at) {
        Some(key) => focus(st, key, p.role),
        None => focus(st, p.key, p.role),
    }
}

fn close_picker(st: &Rc<State>, back_to: Option<(i32, Role)>) {
    if let Some(app) = st.app.upgrade() {
        app.global::<Picker>().set_open(false);
    }
    if let Some((key, role)) = back_to {
        focus(st, key, role);
    }
}

// ---- wiring -------------------------------------------------------------------------------

pub fn wire(app: &AppWindow) {
    let e = app.global::<SmartEdit>();
    e.on_name_edited(|t| {
        with_draft(|d| {
            d.name = t.to_string();
            d.dirty = true;
        });
        state::with(|st| {
            if let Some(app) = st.app.upgrade() {
                app.global::<SmartEdit>().set_name_error("".into());
            }
        });
    });
    e.on_open_choice(|key, role, x, y| {
        state::with(|st| open_choice(st, key, role, x, y));
    });
    e.on_text_edited(|key, role, t| {
        let (Some(at), Some(role)) = (at_of(key), Role::from_index(role)) else { return };
        with_draft(|d| d.type_text(&at, role, &t));
        state::with(|st| {
            refresh(st);
            edited(st);
        });
    });
    e.on_toggle(|key, role| {
        let (Some(at), Some(Role::LimitOn)) = (at_of(key), Role::from_index(role)) else { return };
        with_draft(Draft::toggle_limit);
        state::with(|st| {
            rebuild(st);
            edited(st);
            if let Some(k) = key_of(&at) {
                // The limit's number is the next thing to set.
                let on = UI.with(|u| u.borrow().draft.as_ref().is_some_and(|d| d.limit_on));
                focus(st, k, if on { Role::LimitN } else { Role::LimitOn });
            }
        });
    });
    e.on_add_rule(|key| {
        let group = if key >= 0 {
            match at_of(key) {
                Some(At::Item(p)) => Some(p),
                _ => None,
            }
        } else {
            None
        };
        let Some(path) = with_draft(|d| d.add_rule(group.as_deref())) else { return };
        state::with(|st| {
            rebuild(st);
            edited(st);
            focus_line(st, &At::Item(path));
        });
    });
    e.on_add_group(|| {
        let Some(path) = with_draft(Draft::add_group) else { return };
        state::with(|st| {
            rebuild(st);
            edited(st);
            focus_line(st, &At::Item(path));
        });
    });
    e.on_remove(|key| {
        let Some(At::Item(path)) = at_of(key) else { return };
        with_draft(|d| d.remove(&path));
        state::with(|st| {
            rebuild(st);
            edited(st);
            // The line now in its place, else the one before it.
            let next = if key_of(&At::Item(path.clone())).is_some() {
                At::Item(path.clone())
            } else {
                let mut p = path.clone();
                match p.last_mut() {
                    Some(l) if *l > 0 => {
                        *l -= 1;
                        At::Item(p)
                    }
                    _ => {
                        p.pop();
                        if p.is_empty() { At::Join } else { At::Item(p) }
                    }
                }
            };
            focus_line(st, &next);
        });
    });
    e.on_move(|key, dir| {
        let Some(At::Item(path)) = at_of(key) else { return };
        let Some(to) = with_draft(|d| d.move_item(&path, dir)).flatten() else { return };
        state::with(|st| {
            rebuild(st);
            edited(st);
            focus_line(st, &At::Item(to));
        });
    });
    e.on_template(|i| {
        with_draft(|d| d.apply_template(i.max(0) as usize, new_seed()));
        state::with(|st| {
            if let Some(app) = st.app.upgrade() {
                let name = UI.with(|u| u.borrow().draft.as_ref().map(|d| d.name.clone())).unwrap_or_default();
                app.global::<SmartEdit>().set_name(name.into());
                app.global::<SmartEdit>().set_name_error("".into());
            }
            rebuild(st);
            edited(st);
            focus_line(st, &At::Join);
        });
    });
    e.on_save(|| {
        state::with(save);
    });
    e.on_cancel(|| {
        state::with(cancel);
    });
    e.on_duplicate(|| {
        state::with(|st| duplicate(st, None));
    });

    let p = app.global::<Picker>();
    p.on_filter_edited(|t| {
        state::with(|st| picker_filter(st, &t));
    });
    p.on_pick(|i| {
        state::with(|st| picker_pick(st, i));
    });
    p.on_close(|| {
        let back = UI.with(|u| u.borrow_mut().picker.take().map(|p| (p.key, p.role)));
        state::with(|st| close_picker(st, back));
    });

    let l = app.global::<SmartList>();
    l.on_play(|shuffle| {
        state::with(|st| {
            let tracks = shown_tracks(st);
            if shuffle {
                crate::playback::shuffle_tracks(st, tracks);
            } else {
                crate::playback::play_tracks(st, tracks, 0);
            }
        });
    });
    l.on_edit(|| {
        state::with(|st| {
            if let Some(id) = st.open_playlist.get() {
                open_edit(st, id);
            }
        });
    });
    l.on_reshuffle(|| {
        state::with(reshuffle);
    });
    l.on_more(|x, y| {
        state::with(|st| {
            if let Some(id) = st.open_playlist.get() {
                crate::menus::request_smart_page(st, id, x, y);
            }
        });
    });
    l.on_rename(|name| {
        state::with(|st| {
            let Some(id) = st.open_playlist.get() else { return };
            // Another playlist's name asks first; an empty one leaves it as it is.
            crate::playlists::nameclash::rename(st, id, &name);
        });
    });
    l.on_delete(|| {
        state::with(|st| {
            let Some(id) = st.open_playlist.get() else { return };
            // Not deleted: it stays open.
            if let Err(e) = query::delete_playlist(&st.db, id) {
                return views::not_written(st, "Could not delete the playlist", &e);
            }
            UI.with(|u| u.borrow_mut().pages.retain(|c| c.0 != id));
            views::playlists_changed(st);
            views::go(st, Page::Playlists);
        });
    });
    l.on_hint(|| {
        state::with(hint);
    });
}

/// "Save as ordinary playlist": the name prompt, with what the smart playlist holds now.
pub fn freeze(st: &Rc<State>, id: i64) {
    let ids: Vec<i64> = shown_tracks(st).iter().map(|t| t.id).collect();
    let name = query::playlist_name(&st.db, id).ok().flatten().unwrap_or_default();
    let (x, y) = st.app.upgrade().map_or((400.0, 300.0), |app| (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y()));
    crate::playlists::newplaylist::open(st, crate::playlists::newplaylist::Target::Tracks(ids), &format!("{name} (saved)"), x, y);
}

/// The menu's Rename on the smart playlist's page: rename in place.
pub fn start_rename(st: &Rc<State>) {
    if let Some(app) = st.app.upgrade() {
        let l = app.global::<SmartList>();
        l.set_rename_seq(l.get_rename_seq() + 1);
    }
}

/// The editor's lines as text, each control's text joined (for the bench's report).
#[cfg(feature = "bench")]
pub fn editor_lines() -> Vec<String> {
    UI.with(|u| {
        let u = u.borrow();
        u.rules.rows.iter().chain(&u.tail.rows).map(|r| {
            let mut s = r.slots.iter().map(|s| if s.kind == smartedit::SWITCH { if s.on { "[on]".to_owned() } else { "[off]".to_owned() } } else { s.text.clone() }).collect::<Vec<_>>().join(" ");
            if !r.error.is_empty() {
                s.push_str(&format!("  ! {}", r.error));
            }
            format!("{}{s}", "  ".repeat(r.depth as usize))
        }).collect()
    })
}

/// The line keys of the editor's rules (for the bench), with their texts.
#[cfg(feature = "bench")]
pub fn editor_rule_count() -> usize {
    UI.with(|u| u.borrow().rules.rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_update_in_place_when_only_values_change() {
        let mut d = Draft::new();
        let n = Names { genres: vec!["Jazz".into()], ..Names::default() };
        let mut l = lines(d.rule_rows(&n));
        let slots_before = l.slots[1].clone();
        d.choose(&At::Item(vec![0]), Role::Op, 2, &n); // contains: a typed field now
        assert!(!update(&mut l, d.rule_rows(&n)), "another kind of control: made again");
        let mut l = lines(d.rule_rows(&n));
        let slots = l.slots[1].clone();
        d.type_text(&At::Item(vec![0]), Role::A, "fus");
        assert!(update(&mut l, d.rule_rows(&n)), "typing: in place");
        assert!(Rc::ptr_eq(&slots, &l.slots[1]), "the same controls' model");
        assert_eq!(l.slots[1].row_data(2).unwrap().text, "fus");
        assert!(!Rc::ptr_eq(&slots_before, &slots));
    }

    #[test]
    fn the_page_says_how_much() {
        let t = |d: f64| Track { duration: d, ..Track::default() };
        assert_eq!(info(&[]), "No tracks match its rules now");
        assert_eq!(info(&[t(200.0)]), "1 track \u{b7} 3 min");
        assert_eq!(info(&[t(3000.0), t(1000.0)]), "2 tracks \u{b7} 1 h 6 min");
    }
}
