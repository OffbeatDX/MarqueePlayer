//! `MP_BENCH=playlist`, after the big playlist's timings (`bench/mod.rs`, `big_playlist`):
//!
//! - Shuffle for an ordinary playlist: the page's Shuffle (`Actions.shuffle-in`), then the
//!   sidebar menu's Shuffle (the playlist's context menu, its entry activated as a click
//!   does), then the sidebar's Shuffle and Play buttons (`Actions.play-playlist`): the
//!   queue holds the playlist's tracks, shuffle is on, the order is not the playlist's
//!   (Play: it is, from the first track).
//! - Name clashes, every answer given through the prompt's own callback (`Actions.clash-
//!   choose`, what its buttons call) or its keys: four playlist files imported at once
//!   (Keep both; a file without a clash comes in unasked; Replace for the rest), one again
//!   (Cancel), one named like a smart playlist (no Replace offered; Keep both), one dropped
//!   on the window as Explorer drops it (asks too, plays nothing; Esc cancels), a rename onto
//!   another playlist's name (Keep both; a smart playlist's too: Cancel), Create with a name in
//!   use (Cancel, then Keep both).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::query;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

use crate::bench::{Step, menu_labels, run_steps, snapshot, verdict};
use crate::state::State;
use crate::{Actions, AppWindow, Dialog, Player, views};

type Report = Rc<RefCell<String>>;
/// A step of the run, given the report.
type StepFn = Box<dyn Fn(&Rc<State>, &AppWindow, &Report)>;

thread_local! {
    static IDS: RefCell<Ids> = RefCell::new(Ids::default());
}

#[derive(Default)]
struct Ids {
    big: i64,
    small: i64,
    smart: i64,
    long_pl: i64,
    marquee_start: Vec<[u8; 4]>,
    queue_before: Vec<i64>,
}

fn by_name(st: &State, name: &str) -> Option<query::Playlist> {
    query::playlists(&st.db).unwrap_or_default().into_iter().find(|p| p.name == name)
}

fn track_ids(st: &State, id: i64) -> Vec<i64> {
    query::playlist_tracks(&st.db, id).unwrap_or_default().iter().map(|t| t.id).collect()
}

fn queue_ids(st: &State) -> Vec<i64> {
    st.queue.borrow().items().iter().map(|i| i.track.id).collect()
}

fn sorted(mut v: Vec<i64>) -> Vec<i64> {
    v.sort_unstable();
    v
}

/// Shuffled playback of playlist `id`: the same tracks, shuffle on, another order.
fn check_shuffled(st: &State, app: &AppWindow, r: &mut String, how: &str, id: i64) {
    let want = track_ids(st, id);
    let got = queue_ids(st);
    let current = st.queue.borrow().current().map(|i| i.track.id);
    verdict(r, &format!("{how}: the queue holds the playlist's tracks"), sorted(got.clone()) == sorted(want.clone()), format!("{} queued, {} in the playlist", got.len(), want.len()));
    verdict(r, &format!("{how}: shuffle is on"), st.queue.borrow().shuffled() && app.global::<Player>().get_shuffle(), st.queue.borrow().shuffled());
    verdict(r, &format!("{how}: not in the playlist's order"), got != want, format!("first queued {:?}, first listed {:?}", got.first(), want.first()));
    verdict(r, &format!("{how}: the current track is the queue's first"), current.is_some() && current == got.first().copied(), format!("{current:?}"));
}

/// The middle of the Shuffle button of the sidebar's first playlist (chrome.slint: the
/// panel's horizontal padding (8 px), its head, eight items, the PLAYLISTS heading; 232 px wide).
fn sidebar_shuffle_at() -> (f32, f32) {
    (232.0 - 8.0 - 31.0 + 14.0, 478.0)
}

/// Pixels inside a logical rectangle of the window (for checking whether a scrolling
/// label moved).
fn region_pixels(app: &AppWindow, x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[u8; 4]> {
    let Ok(buf) = app.window().take_snapshot() else { return Vec::new() };
    let s = app.window().scale_factor();
    let (w, h) = (buf.width(), buf.height());
    let (px0, py0) = ((x0 * s).max(0.0) as u32, (y0 * s).max(0.0) as u32);
    let (px1, py1) = (((x1 * s) as u32).min(w), ((y1 * s) as u32).min(h));
    let slice = buf.as_slice();
    let mut out = Vec::new();
    for py in (py0..py1).step_by(2) {
        for px in (px0..px1).step_by(2) {
            let p = slice[(py * w + px) as usize];
            out.push([p.r, p.g, p.b, p.a]);
        }
    }
    out
}

fn write_list(path: &Path, paths: &[String]) {
    let mut text = String::from("#EXTM3U\n");
    for p in paths {
        text.push_str(p);
        text.push('\n');
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    std::fs::write(path, text).unwrap();
}

fn paths_of(st: &State, id: i64, skip: usize, n: usize) -> Vec<String> {
    query::playlist_tracks(&st.db, id).unwrap_or_default().into_iter().skip(skip).take(n).map(|t| t.path).collect()
}

fn dialog(app: &AppWindow) -> String {
    let d = app.global::<Dialog>();
    format!("open {}, {:?} / {:?} / {:?}, replace offered {}, rest {}", d.get_clash_open(), d.get_clash_title().as_str(), d.get_clash_text().as_str(), d.get_clash_note().as_str(), d.get_clash_can_replace(), d.get_clash_rest())
}

fn choose(app: &AppWindow, choice: i32, for_rest: bool) {
    // The prompt's switch, then its button, as a click does.
    app.global::<Dialog>().set_clash_same(for_rest);
    app.global::<Actions>().invoke_clash_choose(choice, for_rest);
}

fn files() -> PathBuf {
    crate::bench::out_dir().join("clash-lists")
}

pub fn run(report: String, big: i64) {
    let r: Report = Rc::new(RefCell::new(report));
    let mut steps: Vec<Step> = Vec::new();
    let step = |steps: &mut Vec<Step>, delay: u64, f: StepFn| {
        let r = r.clone();
        steps.push((delay, Box::new(move |st, app| f(st, app, &r))));
    };

    // ---- Shuffle --------------------------------------------------------------------------
    step(&mut steps, 300, Box::new(move |st, app, r| {
        IDS.with(|i| i.borrow_mut().big = big);
        crate::playback::set_shuffle(st, false);
        views::open_playlist(st, big);
        // The page's head in view (Play, Shuffle) for the snapshot.
        app.global::<crate::Nav>().set_page_scroll(0.0);
        app.global::<Actions>().invoke_shuffle_in("playlist".into());
        check_shuffled(st, app, &mut r.borrow_mut(), "the page's Shuffle", big);
        IDS.with(|i| i.borrow_mut().queue_before = queue_ids(st));
    }));
    step(&mut steps, 600, Box::new(|st, app, r| {
        snapshot(app, "playlist-shuffled");
        let big = IDS.with(|i| i.borrow().big);
        crate::playback::set_shuffle(st, false);
        crate::menus::request(st, "playlist", "", 0, big as i32, 120.0, 300.0);
        let labels = menu_labels(app);
        let at = labels.iter().position(|l| l == "Shuffle");
        verdict(&mut r.borrow_mut(), "the sidebar's playlist menu offers Shuffle beside Play", labels.get(1).map(String::as_str) == Some("Shuffle"), format!("{labels:?}"));
        if let Some(at) = at {
            crate::menus::activate(st, at as i32, -1);
        }
        let mut r = r.borrow_mut();
        check_shuffled(st, app, &mut r, "the menu's Shuffle", big);
        let before = IDS.with(|i| i.borrow().queue_before.clone());
        verdict(&mut r, "a second shuffle is another order", queue_ids(st) != before, "");
        verdict(&mut r, "the menu closed", !app.global::<crate::MenuStore>().get_open(), "");
        // The sidebar's buttons on the playlist (shown on hover): Shuffle, then Play.
        crate::playback::set_shuffle(st, false);
        app.global::<Actions>().invoke_play_playlist(big as i32, true);
        check_shuffled(st, app, &mut r, "the sidebar's Shuffle button", big);
        crate::playback::set_shuffle(st, false);
        app.global::<Actions>().invoke_play_playlist(big as i32, false);
        let (want, got) = (track_ids(st, big), queue_ids(st));
        let current = st.queue.borrow().current().map(|i| i.track.id);
        verdict(&mut r, "the sidebar's Play button: the playlist in its order, from its first track", got == want && current == want.first().copied(), format!("{} queued, current {current:?}", got.len()));
        if !st.player.is_paused() {
            crate::playback::toggle_play(st);
        }
    }));

    // ---- The sidebar's buttons, by the pointer ------------------------------------------
    // The first playlist of the sidebar (pinned open), from the Home page: the pointer on
    // its Shuffle button, a snapshot, then a click there. (In-app pointer events.)
    step(&mut steps, 300, Box::new(|st, app, _| {
        crate::playback::set_shuffle(st, false);
        let nav = app.global::<crate::Nav>();
        nav.set_sidebar_pinned(true);
        nav.invoke_go(crate::Page::Home);
    }));
    // Once the sidebar has slid in.
    step(&mut steps, 500, Box::new(|_, app, _| {
        let (x, y) = sidebar_shuffle_at();
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
    }));
    step(&mut steps, 500, Box::new(|st, app, r| {
        snapshot(app, "sidebar-playlist-hover");
        let first = app.global::<crate::Playlists>().get_list().row_data(0);
        let (x, y) = sidebar_shuffle_at();
        let position = LogicalPosition::new(x, y);
        app.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        app.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        let mut r = r.borrow_mut();
        match first {
            Some(p) if !p.smart => {
                check_shuffled(st, app, &mut r, &format!("a click on the Shuffle button of {:?} in the sidebar", p.name.as_str()), i64::from(p.id));
                let page = app.global::<crate::Nav>().get_page();
                verdict(&mut r, "the click did not open the playlist", page == crate::Page::Home, format!("{page:?}"));
            }
            other => verdict(&mut r, "the sidebar's first playlist is an ordinary one", false, format!("{:?}", other.map(|p| p.name))),
        }
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(700.0, 300.0) });
        crate::playback::set_shuffle(st, false);
        if !st.player.is_paused() {
            crate::playback::toggle_play(st);
        }
        // Long-named playlist in the sidebar: hovering scrolls the overflowing text.
        let long_pl = query::create_playlist_with(&st.db, "Future Funk & City Pop Classics", &[]).unwrap();
        IDS.with(|i| i.borrow_mut().long_pl = long_pl);
        views::playlists_changed(st);
    }));
    // Hover the long playlist (row 1: 42 px below row 0); wait 150 ms (still in dwell).
    step(&mut steps, 200, Box::new(|_, app, _| {
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(90.0, 520.0) });
    }));
    step(&mut steps, 150, Box::new(|_, app, _| {
        snapshot(app, "sidebar-marquee-0-start");
        let px = region_pixels(app, 48.0, 506.0, 156.0, 534.0);
        IDS.with(|i| i.borrow_mut().marquee_start = px);
    }));
    // Past the 450 ms dwell and well into the scroll: pixels in the text slot must differ.
    step(&mut steps, 1300, Box::new(|_, app, r| {
        snapshot(app, "sidebar-marquee-1-mid");
        let start = IDS.with(|i| i.borrow().marquee_start.clone());
        let mid = region_pixels(app, 48.0, 506.0, 156.0, 534.0);
        let diff = start.iter().zip(mid.iter()).filter(|(a, b)| a != b).count();
        verdict(&mut r.borrow_mut(), "hovering a truncated playlist name scrolls it", !start.is_empty() && diff > 20, format!("{diff} pixels moved of {}", start.len()));
    }));
    // End pause, then move pointer away and clean up the temporary playlist.
    step(&mut steps, 2200, Box::new(|_, app, _| {
        snapshot(app, "sidebar-marquee-2-end");
        app.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(700.0, 300.0) });
    }));
    step(&mut steps, 250, Box::new(|st, app, _| {
        snapshot(app, "sidebar-marquee-3-reset");
        let long_pl = IDS.with(|i| i.borrow().long_pl);
        if long_pl > 0 {
            let _ = query::delete_playlist(&st.db, long_pl);
            views::playlists_changed(st);
        }
        app.global::<crate::Nav>().set_sidebar_pinned(false);
    }));

    // ---- Import: four files at once ------------------------------------------------------
    step(&mut steps, 400, Box::new(|st, app, r| {
        let big = IDS.with(|i| i.borrow().big);
        let small = query::create_playlist_with(&st.db, "Bench small", &track_ids(st, big)[..2]).unwrap();
        let smart = query::create_smart_playlist(&st.db, "Bench smart", r#"{"rules":[{"field":"favourite","op":"yes"}]}"#).unwrap();
        IDS.with(|i| {
            let mut i = i.borrow_mut();
            i.small = small;
            i.smart = smart;
        });
        views::playlists_changed(st);
        let d = files();
        let big_name = query::playlist_name(&st.db, big).ok().flatten().unwrap_or_default();
        write_list(&d.join(format!("{big_name}.m3u8")), &paths_of(st, big, 0, 10));
        write_list(&d.join("Fresh list.m3u8"), &paths_of(st, big, 10, 3));
        let mut five = paths_of(st, big, 20, 5);
        five.push("D:\\Nowhere\\missing.flac".into());
        write_list(&d.join("Bench small.m3u8"), &five);
        write_list(&d.join("again").join("bench SMALL.m3u8"), &paths_of(st, big, 30, 4));
        write_list(&d.join("Bench smart.m3u8"), &paths_of(st, big, 40, 2));
        crate::playlists::file::import_files(
            st,
            vec![d.join(format!("{big_name}.m3u8")), d.join("Fresh list.m3u8"), d.join("Bench small.m3u8"), d.join("again").join("bench SMALL.m3u8")],
        );
        let mut r = r.borrow_mut();
        let dl = app.global::<Dialog>();
        r.push_str(&format!("[clash] four files: {}\n", dialog(app)));
        verdict(&mut r, "a file named like a playlist asks before anything changes", dl.get_clash_open() && crate::playlists::nameclash::waiting(), dialog(app));
        verdict(&mut r, "nothing was imported yet", by_name(st, "Fresh list").is_none() && track_ids(st, big).len() > 10, "");
        verdict(&mut r, "it offers Replace over an ordinary playlist", dl.get_clash_can_replace(), "");
        let text = dl.get_clash_text().to_string();
        let n = track_ids(st, big).len();
        verdict(&mut r, "it says how many tracks each has", text.contains(&format!("has {n} tracks")) && text.contains("brings 10 tracks"), &text);
        verdict(&mut r, "Keep both says the name it would take", dl.get_clash_note().contains(&format!("\u{201c}{big_name} (2)\u{201d}")), dl.get_clash_note());
        verdict(&mut r, "the clashes still to come are counted (two: both small lists)", dl.get_clash_rest() == 2, dl.get_clash_rest());
    }));
    step(&mut steps, 500, Box::new(|st, app, r| {
        snapshot(app, "playlist-clash");
        let big = IDS.with(|i| i.borrow().big);
        let big_name = query::playlist_name(&st.db, big).ok().flatten().unwrap_or_default();
        let n = track_ids(st, big).len();
        choose(app, 2, false);
        let mut r = r.borrow_mut();
        let kept = by_name(st, &format!("{big_name} (2)"));
        verdict(&mut r, "Keep both: the file comes in as \u{201c}Name (2)\u{201d}", kept.as_ref().is_some_and(|p| p.tracks == 10), format!("{kept:?}"));
        verdict(&mut r, "Keep both: the playlist is left as it was", track_ids(st, big).len() == n, track_ids(st, big).len());
        let fresh = by_name(st, "Fresh list");
        verdict(&mut r, "a file with a name of its own comes in without a question", fresh.as_ref().is_some_and(|p| p.tracks == 3), format!("{fresh:?}"));
        let dl = app.global::<Dialog>();
        r.push_str(&format!("[clash] next: {}\n", dialog(app)));
        verdict(&mut r, "the next clash asks in turn", dl.get_clash_open() && dl.get_clash_title().contains("Bench small"), dl.get_clash_title());
        verdict(&mut r, "it says the file's missing entry", dl.get_clash_text().contains("1 of its 6 entries is not in the library"), dl.get_clash_text());
        verdict(&mut r, "one more clash to come", dl.get_clash_rest() == 1, dl.get_clash_rest());
    }));
    step(&mut steps, 400, Box::new(|st, app, r| {
        snapshot(app, "playlist-clash-rest");
        let small = IDS.with(|i| i.borrow().small);
        choose(app, 1, true);
        let mut r = r.borrow_mut();
        let d = files();
        let want: Vec<String> = std::fs::read_to_string(d.join("again").join("bench SMALL.m3u8")).unwrap_or_default().lines().skip(1).map(str::to_owned).collect();
        let got: Vec<String> = query::playlist_tracks(&st.db, small).unwrap_or_default().into_iter().map(|t| t.path).collect();
        verdict(&mut r, "Replace for the rest: the last file's tracks, in its order, in the same playlist", got == want, format!("{} tracks", got.len()));
        let same = by_name(st, "Bench small");
        verdict(&mut r, "Replace keeps the playlist's id and name", same.as_ref().is_some_and(|p| p.id == small), format!("{same:?}"));
        verdict(&mut r, "the prompt asked no more", !app.global::<Dialog>().get_clash_open() && !crate::playlists::nameclash::waiting(), dialog(app));
        let summary = app.global::<crate::Playlists>().get_import_summary().to_string();
        r.push_str(&format!("[clash] summary: {summary:?}\n"));
        verdict(&mut r, "the summary has a line for each file", summary.lines().count() == 4 && summary.contains("Replaced \u{201c}Bench small\u{201d}"), summary.lines().count());
        let names: Vec<String> = query::playlists(&st.db).unwrap_or_default().into_iter().map(|p| p.name).collect();
        verdict(&mut r, "no playlist was made twice", names.iter().filter(|n| n.eq_ignore_ascii_case("bench small")).count() == 1 && names.iter().filter(|n| n.as_str() == "Fresh list").count() == 1, format!("{names:?}"));
    }));

    // ---- Import: Cancel, and a smart playlist's name ----------------------------------------
    step(&mut steps, 300, Box::new(|st, app, r| {
        crate::playlists::file::import_files(st, vec![files().join("Fresh list.m3u8")]);
        let mut r = r.borrow_mut();
        let dl = app.global::<Dialog>();
        verdict(&mut r, "one file: the same question, no \u{201c}same for the rest\u{201d}", dl.get_clash_open() && dl.get_clash_rest() == 0, dialog(app));
        choose(app, 0, false);
        let fresh = by_name(st, "Fresh list");
        verdict(&mut r, "Cancel: nothing changes", fresh.as_ref().is_some_and(|p| p.tracks == 3) && by_name(st, "Fresh list (2)").is_none(), format!("{fresh:?}"));
        let summary = app.global::<crate::Playlists>().get_import_summary().to_string();
        verdict(&mut r, "Cancel: the page says the file was not imported", summary.contains("was not imported"), &summary);
    }));
    step(&mut steps, 300, Box::new(|st, app, r| {
        crate::playlists::file::import_files(st, vec![files().join("Bench smart.m3u8")]);
        let mut r = r.borrow_mut();
        let dl = app.global::<Dialog>();
        r.push_str(&format!("[clash] smart: {}\n", dialog(app)));
        verdict(&mut r, "a smart playlist's name asks without Replace", dl.get_clash_open() && !dl.get_clash_can_replace() && dl.get_clash_text().contains("is a smart playlist"), dialog(app));
    }));
    step(&mut steps, 400, Box::new(|st, app, r| {
        snapshot(app, "playlist-clash-smart");
        let smart = IDS.with(|i| i.borrow().smart);
        // Replace is not offered; answered anyway, it does nothing.
        choose(app, 1, false);
        let mut r = r.borrow_mut();
        verdict(&mut r, "Replace on a smart playlist changes nothing", crate::playlists::smart::is_smart(st, smart) && by_name(st, "Bench smart (2)").is_none(), "");
        crate::playlists::file::import_files(st, vec![files().join("Bench smart.m3u8")]);
        choose(app, 2, false);
        let kept = by_name(st, "Bench smart (2)");
        verdict(&mut r, "Keep both: an ordinary \u{201c}Bench smart (2)\u{201d} beside the smart one", kept.as_ref().is_some_and(|p| !p.smart() && p.tracks == 2) && crate::playlists::smart::is_smart(st, smart), format!("{kept:?}"));
    }));

    // ---- A drop on the window ---------------------------------------------------------------
    step(&mut steps, 300, Box::new(|st, app, r| {
        IDS.with(|i| i.borrow_mut().queue_before = queue_ids(st));
        let size = app.window().size();
        let n = crate::platform::window::simulate_drop(st.hwnd.get(), &[files().join("Fresh list.m3u8")], size.width as i32 / 3, size.height as i32 / 2);
        r.borrow_mut().push_str(&format!("[clash] dropped {n} file on the window\n"));
    }));
    step(&mut steps, 800, Box::new(|st, app, r| {
        let mut r = r.borrow_mut();
        let before = IDS.with(|i| i.borrow().queue_before.clone());
        verdict(&mut r, "a dropped playlist file asks too", app.global::<Dialog>().get_clash_open(), dialog(app));
        verdict(&mut r, "a dropped playlist file is not played", queue_ids(st) == before, format!("{} queued", queue_ids(st).len()));
        crate::bench::press(app, slint::platform::Key::Escape, &[]);
        verdict(&mut r, "Esc cancels it", !app.global::<Dialog>().get_clash_open() && by_name(st, "Fresh list (2)").is_none(), dialog(app));
    }));

    // ---- Rename and Create ------------------------------------------------------------------
    step(&mut steps, 300, Box::new(|st, app, r| {
        let Some(fresh) = by_name(st, "Fresh list") else { return };
        views::open_playlist(st, fresh.id);
        app.global::<Actions>().invoke_rename_playlist(fresh.id as i32, "  bench SMALL ".into());
        let mut r = r.borrow_mut();
        let dl = app.global::<Dialog>();
        r.push_str(&format!("[clash] rename: {}\n", dialog(app)));
        verdict(&mut r, "renaming onto another playlist's name asks (no Replace)", dl.get_clash_open() && !dl.get_clash_can_replace() && dl.get_clash_note().contains("\u{201c}bench SMALL (2)\u{201d}"), dialog(app));
        verdict(&mut r, "not renamed yet", query::playlist_name(&st.db, fresh.id).ok().flatten().as_deref() == Some("Fresh list"), "");
    }));
    step(&mut steps, 400, Box::new(|st, app, r| {
        snapshot(app, "playlist-clash-rename");
        let Some(fresh) = by_name(st, "Fresh list") else { return };
        choose(app, 2, false);
        let mut r = r.borrow_mut();
        let name = query::playlist_name(&st.db, fresh.id).ok().flatten();
        verdict(&mut r, "Keep both renames it \u{201c}Name (2)\u{201d}", name.as_deref() == Some("bench SMALL (2)"), format!("{name:?}"));
        verdict(&mut r, "the page shows the new name", app.global::<crate::Playlists>().get_current_name() == "bench SMALL (2)", app.global::<crate::Playlists>().get_current_name());
        app.global::<Actions>().invoke_rename_playlist(fresh.id as i32, "Bench SMALL (2)".into());
        let name = query::playlist_name(&st.db, fresh.id).ok().flatten();
        verdict(&mut r, "renaming to its own name in other letters asks nothing", !app.global::<Dialog>().get_clash_open() && name.as_deref() == Some("Bench SMALL (2)"), format!("{name:?}"));
        app.global::<Actions>().invoke_rename_playlist(fresh.id as i32, "   ".into());
        let name = query::playlist_name(&st.db, fresh.id).ok().flatten();
        verdict(&mut r, "an empty name leaves it as it is", name.as_deref() == Some("Bench SMALL (2)"), format!("{name:?}"));
    }));
    step(&mut steps, 300, Box::new(|st, app, r| {
        // A smart playlist's page renames through the same question.
        let smart = IDS.with(|i| i.borrow().smart);
        views::open_playlist(st, smart);
        app.global::<crate::SmartList>().invoke_rename("BENCH 3000".into());
        let mut r = r.borrow_mut();
        verdict(&mut r, "a smart playlist renamed onto another's name asks too", app.global::<Dialog>().get_clash_open() && app.global::<Dialog>().get_clash_note().contains("\u{201c}BENCH 3000 (3)\u{201d}"), dialog(app));
        choose(app, 0, false);
        let name = query::playlist_name(&st.db, smart).ok().flatten();
        verdict(&mut r, "Cancel leaves its name as it was", name.as_deref() == Some("Bench smart"), format!("{name:?}"));
    }));
    step(&mut steps, 300, Box::new(|st, app, r| {
        let before = query::playlists(&st.db).unwrap_or_default().len();
        app.global::<Actions>().invoke_create_playlist("bench small".into());
        let mut r = r.borrow_mut();
        verdict(&mut r, "Create with a name in use asks", app.global::<Dialog>().get_clash_open() && !app.global::<Dialog>().get_clash_can_replace(), dialog(app));
        choose(app, 0, false);
        verdict(&mut r, "Cancel makes nothing", query::playlists(&st.db).unwrap_or_default().len() == before, "");
        app.global::<Actions>().invoke_create_playlist("bench small".into());
        choose(app, 2, false);
        // "(2)" is the renamed list's ("Bench SMALL (2)"; names compare without letter case).
        verdict(&mut r, "Keep both makes the next free number, \u{201c}bench small (3)\u{201d}", by_name(st, "bench small (3)").is_some_and(|p| p.tracks == 0) && by_name(st, "bench small (2)").is_none(), "");
        let listed = app.global::<crate::Playlists>().get_list();
        verdict(&mut r, "the sidebar's list has them all", listed.row_count() == query::playlists(&st.db).unwrap_or_default().len(), listed.row_count());
    }));
    // Then the selection of track lists (`bench/selection.rs`), which writes the report and quits.
    step(&mut steps, 300, Box::new(move |_st, _app, r| {
        crate::bench::selection::run(std::mem::take(&mut *r.borrow_mut()), big);
    }));
    run_steps(steps);
}
