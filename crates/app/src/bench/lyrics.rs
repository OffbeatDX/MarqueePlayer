//! `MP_BENCH=lyrics`: the Lyrics tab's timing offset and its Copy and Save (`bench-lyrics.txt`),
//! on the core fixtures' copy: Night Harbour's first track (a synced `.lrc`, a line every 1.5 s
//! from 0.5 s), Paper Moons (plain lyrics in its tags).
//!
//! - the offset: later by the buttons' callback and earlier by G (the window's own key
//!   dispatch), the line lit and a click on a line following at once, the timing row's text,
//!   kept for the track across a change of track and in `lyrics-offsets.json`;
//! - the menu's entries, with tag editing off and on;
//! - Copy: into the bench's stand-in clipboard (`explorer::copy_text`), never the real one;
//! - Save as .lrc over the `.lrc` already there: asked first and left as it was, then replaced
//!   on "Replace" with the offset in its time stamps, the offset back to 0 and the line lit the
//!   same; Save as .txt for plain lyrics (none there: written at once, read next);
//! - Save into the tags (tag editing on): asked, written through the tag editor's checked save
//!   (one save in the journal), the `.lrc` still read first and said so;
//! - a server's track: refused with a word.
//!
//! Every write lands in the run's copy of the fixtures: this marks that copy as the editor's
//! fixtures (`.mp-edit-fixtures`) only when it is inside the system's temporary folder, and the
//! bench build's write guard refuses every other path, sidecars included. No OS input; nothing
//! is audible (`MP_VOLUME=0`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::lyrics as lib;
use slint::{ComponentHandle, Model};

use crate::bench::{Step, menu_labels, run_steps, snapshot, type_text, write_report};
use crate::state::State;
use crate::{Actions, AppWindow, Lyrics, Nav, Page, views};

type Report = Rc<RefCell<String>>;

fn line(r: &Report, text: String) {
    log::info!("lyrics bench: {text}");
    r.borrow_mut().push_str(&text);
    r.borrow_mut().push('\n');
}

fn check(r: &Report, what: &str, ok: bool, detail: impl std::fmt::Display) {
    crate::bench::verdict(&mut r.borrow_mut(), what, ok, detail);
}

thread_local! {
    static ROOT: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    /// The line lit before a save: the same after it.
    static LIT: std::cell::Cell<i32> = const { std::cell::Cell::new(-1) };
}

fn root() -> PathBuf {
    ROOT.with(|r| r.borrow().clone())
}

fn harbour() -> PathBuf {
    root().join("050 Mara Lind").join("Night Harbour").join("01 Track 1.flac")
}

fn moons() -> PathBuf {
    root().join("051 Mara Lind").join("Paper Moons").join("01 Paper Moons.flac")
}

fn album_tracks(st: &State, title: &str) -> Vec<library::Track> {
    let id: Option<i64> = st.db.query_row("SELECT id FROM albums WHERE title = ?1", [title], |r| r.get(0)).ok();
    id.map(|id| library::query::album_tracks(&st.db, id).unwrap_or_default()).unwrap_or_default()
}

fn state(app: &AppWindow) -> String {
    let l = app.global::<Lyrics>();
    format!(
        "{} lines, synced {}, current {}, offset {} ({:?}), notice {:?}, ask {:?}",
        l.get_lines().row_count(),
        l.get_synced(),
        l.get_current(),
        l.get_offset(),
        l.get_offset_text().to_string(),
        l.get_notice().to_string(),
        l.get_ask().to_string()
    )
}

/// The line that should be lit now: the `.lrc` as read, the offset applied.
fn expected_current(st: &State, offset: i64) -> i32 {
    let pos = (st.player.status().position.max(0.0) * 1000.0) as u64;
    lib::load(&harbour()).and_then(|l| lib::current_shifted(&l.lines, pos, offset)).map_or(-1, |i| i as i32)
}

/// Paused, so the line lit stays put while it is checked.
fn pause(st: &Rc<State>) {
    if !st.player.is_paused() {
        crate::playback::toggle_play(st);
    }
}

fn times(p: &Path) -> Vec<Option<u64>> {
    lib::load(p).map(|l| l.lines.into_iter().map(|x| x.time_ms).collect()).unwrap_or_default()
}

/// Everything the run's copy holds that the app left behind (temporary files).
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".tmp") || n.contains(".mpedit-")).collect()).unwrap_or_default()
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
    // Saved .lrc of Night Harbour as the fixtures made it, and its stamps.
    let original: Rc<RefCell<(String, Vec<Option<u64>>)>> = Rc::default();

    let o = original.clone();
    step!(300, move |st, _app, r| {
        let music = st.settings.borrow().folders.first().cloned().unwrap_or_default();
        ROOT.with(|x| *x.borrow_mut() = music.clone());
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
        let inside = std::fs::canonicalize(&music).is_ok_and(|m| m.starts_with(&temp));
        check(r, "the library is a copy inside the temporary folder", inside, music.display());
        if inside && !music.join(".mp-edit-fixtures").is_file() {
            let _ = std::fs::write(music.join(".mp-edit-fixtures"), b"lyrics bench: the run's copy of the core fixtures");
        }
        let lrc = lib::sidecar(&harbour(), true);
        *o.borrow_mut() = (std::fs::read_to_string(&lrc).unwrap_or_default(), times(&harbour()));
        check(r, "Night Harbour has its .lrc", !o.borrow().1.is_empty(), lrc.display());
        check(r, "Paper Moons has no .txt yet", !lib::sidecar(&moons(), false).exists(), "");
        let tracks = album_tracks(st, "Night Harbour");
        crate::playback::play_tracks(st, tracks, 0);
        crate::lyrics::set_offset(st, 0);
    });
    step!(300, |st, app, _r| {
        app.global::<Lyrics>().set_shown(true);
        views::go(st, Page::NowPlaying);
    });

    // ---- the offset ----------------------------------------------------------------------
    step!(4500, |st, app, r| {
        line(r, format!("[start] at {:.2} s: {}", st.player.status().position, state(app)));
        check(r, "synced, no offset, the line lit as the .lrc says", app.global::<Lyrics>().get_synced() && app.global::<Lyrics>().get_offset() == 0 && app.global::<Lyrics>().get_current() == expected_current(st, 0), state(app));
        snapshot(app, "ly-01-timing-row");
        // Later by 0.8 s through the buttons' callback (the pause keeps the line still).
        pause(st);
        for _ in 0..8 {
            app.global::<Actions>().invoke_lyrics_nudge(1);
        }
    });
    step!(400, |st, app, r| {
        let l = app.global::<Lyrics>();
        check(r, "later by 8 steps: +0.8 s", l.get_offset() == 800 && l.get_offset_text() == "+0.8 s", state(app));
        check(r, "the line lit follows at once", l.get_current() == expected_current(st, 800), format!("{} want {} at {:.2} s", l.get_current(), expected_current(st, 800), st.player.status().position));
        // G, three times, through the window's key dispatch: earlier by 0.3 s.
        type_text(app, "ggg");
        line(r, format!("[keys] after G x3: {}; pill {:?}", state(app), app.global::<Nav>().get_flash().to_string()));
        check(r, "G: earlier, said in the pill", l.get_offset() == 500 && app.global::<Nav>().get_flash().contains("+0.5 s"), state(app));
        check(r, "the line lit with +0.5 s", l.get_current() == expected_current(st, 500), format!("{} want {}", l.get_current(), expected_current(st, 500)));
        snapshot(app, "ly-02-offset-plus-half");
        // A click on line 4 plays from where it shows: its stamp + 0.5 s.
        // Playing again (a seek while paused moves the engine only once it plays).
        crate::playback::toggle_play(st);
        crate::lyrics::seek(st, 3);
    });
    let o = original.clone();
    step!(900, move |st, app, r| {
        let want = o.borrow().1.get(3).copied().flatten().map_or(0.0, |t| (t + 500) as f64 / 1000.0);
        let at = st.player.status().position;
        check(r, "a click on a line seeks to it with the offset", at >= want - 0.05 && at < want + 1.2, format!("at {at:.2} s (playing since), want {want:.2} s"));
        check(r, "and lights it", app.global::<Lyrics>().get_current() == 3, state(app));
        let file = st.paths.data.join("lyrics-offsets.json");
        let kept = std::fs::read_to_string(&file).unwrap_or_default();
        let key = harbour().to_string_lossy().replace('\\', "\\\\");
        check(r, "kept in lyrics-offsets.json by the track's path", kept.contains(&key) && kept.contains("500"), kept.trim());
        // Another track, and back: the offset comes back with it.
        crate::playback::next(st);
    });
    step!(1200, |st, app, r| {
        check(r, "the next track has none of it", app.global::<Lyrics>().get_offset() == 0, state(app));
        crate::playback::previous(st);
        crate::playback::previous(st);
    });
    step!(1500, |st, app, r| {
        let cur = st.queue.borrow().current().map(|i| i.track.title.clone()).unwrap_or_default();
        check(r, "back on track 1: +0.5 s again", app.global::<Lyrics>().get_offset() == 500 && app.global::<Lyrics>().get_offset_text() == "+0.5 s", format!("{cur}: {}", state(app)));
        crate::lyrics::tick(st, st.player.status().position);
        check(r, "and the line lit by it", app.global::<Lyrics>().get_current() == expected_current(st, 500), format!("at {:.2} s: {} want {}", st.player.status().position, app.global::<Lyrics>().get_current(), expected_current(st, 500)));
        pause(st);
        // The menu, tag editing off.
        app.global::<Actions>().invoke_lyrics_menu(600.0, 200.0, false);
        let labels = menu_labels(app);
        line(r, format!("[menu] {labels:?}"));
        check(r, "menu: Copy, Save as .lrc, Reset timing; no tags (tag editing off)", labels.iter().any(|l| l == "Copy lyrics") && labels.iter().any(|l| l == "Save as .lrc") && labels.iter().any(|l| l == "Reset timing (+0.5 s)") && !labels.iter().any(|l| l.contains("tags")), format!("{labels:?}"));
        snapshot(app, "ly-03-menu");
        app.global::<crate::MenuStore>().set_open(false);
    });

    // ---- Copy ------------------------------------------------------------------------------
    step!(300, |st, app, r| {
        crate::explorer::bench_set_clipboard(None);
        crate::lyrics::save::copy(st);
        let clip = crate::explorer::bench_clipboard().unwrap_or_default();
        let want = lib::load(&harbour()).map(|l| lib::to_text(&l.lines)).unwrap_or_default();
        check(r, "Copy: the words, one a line, on the bench's stand-in clipboard", !want.is_empty() && clip == want, format!("{} lines; pill {:?}", clip.lines().count(), app.global::<Nav>().get_flash().to_string()));
    });

    // ---- Save as .lrc: a file is there --------------------------------------------------
    step!(300, |st, _app, _r| crate::lyrics::save::save_file(st, false));
    let o = original.clone();
    step!(800, move |_st, app, r| {
        let lrc = lib::sidecar(&harbour(), true);
        let l = app.global::<Lyrics>();
        check(r, "the .lrc there: asked, Replace", l.get_ask() == "replace" && l.get_notice().contains(&*lrc.to_string_lossy()), state(app));
        check(r, "and left as it was", std::fs::read_to_string(&lrc).unwrap_or_default() == o.borrow().0, "");
        LIT.with(|c| c.set(l.get_current()));
        snapshot(app, "ly-04-replace-asked");
        app.global::<Actions>().invoke_lyrics_answer("replace".into());
    });
    let o = original.clone();
    step!(1000, move |st, app, r| {
        let l = app.global::<Lyrics>();
        let now = times(&harbour());
        let want: Vec<Option<u64>> = o.borrow().1.iter().map(|t| t.map(|t| t + 500)).collect();
        check(r, "replaced: the .lrc's stamps 0.5 s later", now == want, format!("first {:?} -> {:?}", o.borrow().1.first(), now.first()));
        check(r, "the offset back to 0, said, the line lit the same", l.get_offset() == 0 && l.get_notice().contains("Saved ") && l.get_current() == LIT.with(std::cell::Cell::get), format!("lit before {}; {}", LIT.with(std::cell::Cell::get), state(app)));
        let text = std::fs::read_to_string(lib::sidecar(&harbour(), true)).unwrap_or_default();
        check(r, "LRC with the track's tags, no [offset:]", text.starts_with("[ti:") && !text.contains("offset"), text.lines().take(4).collect::<Vec<_>>().join(" | "));
        check(r, "no temporary file left", leftovers(&root().join("050 Mara Lind").join("Night Harbour")).is_empty(), format!("{:?}", leftovers(&root().join("050 Mara Lind").join("Night Harbour"))));
        line(r, format!("[saved] {}", state(app)));
        snapshot(app, "ly-05-saved-lrc");
        let _ = st;
        app.global::<Actions>().invoke_lyrics_answer("ok".into());
    });

    // ---- plain lyrics: no timing, Save as .txt --------------------------------------------
    step!(300, |st, _app, _r| {
        let tracks = album_tracks(st, "Paper Moons");
        crate::playback::play_tracks(st, tracks, 0);
    });
    step!(1500, |st, app, r| {
        pause(st);
        let l = app.global::<Lyrics>();
        check(r, "Paper Moons: plain lyrics from its tags", !l.get_synced() && l.get_lines().row_count() == 4, state(app));
        type_text(app, "h");
        check(r, "H on plain lyrics: nothing moves, said", l.get_offset() == 0 && app.global::<Nav>().get_flash().contains("no time stamps"), app.global::<Nav>().get_flash().to_string());
        app.global::<Actions>().invoke_lyrics_menu(600.0, 200.0, false);
        let labels = menu_labels(app);
        check(r, "menu: Save as .txt, no timing", labels.iter().any(|l| l == "Save as .txt") && !labels.iter().any(|l| l.starts_with("Reset")), format!("{labels:?}"));
        app.global::<crate::MenuStore>().set_open(false);
        snapshot(app, "ly-06-plain");
        crate::lyrics::save::save_file(st, false);
    });
    step!(1000, |_st, app, r| {
        let txt = lib::sidecar(&moons(), false);
        let read = lib::load(&moons());
        check(r, "written at once, said with its path", app.global::<Lyrics>().get_notice().contains(&*txt.to_string_lossy()), state(app));
        check(r, "the .txt is what the track reads now", read.as_ref().is_some_and(|l| l.source == "txt" && l.lines.len() == 4), format!("{:?}", read.map(|l| (l.source, l.lines.len()))));
        check(r, "UTF-8 text, a line each", std::fs::read_to_string(&txt).is_ok_and(|t| t.starts_with("Paper moons over the water\n")), "");
        app.global::<Actions>().invoke_lyrics_answer("ok".into());
    });

    // ---- Save into the tags (tag editing on) ----------------------------------------------
    step!(300, |st, _app, _r| {
        let tracks = album_tracks(st, "Night Harbour");
        crate::playback::play_tracks(st, tracks, 0);
    });
    step!(1500, |st, app, r| {
        pause(st);
        if !crate::metadata::tagedit::enabled(st) {
            crate::metadata::tagedit::toggle(st);
        }
        crate::lyrics::set_offset(st, 300);
        app.global::<Actions>().invoke_lyrics_menu(600.0, 200.0, false);
        let labels = menu_labels(app);
        check(r, "tag editing on: Save into the file's tags", labels.iter().any(|l| l == "Save into the file\u{2019}s tags\u{2026}"), format!("{labels:?}"));
        app.global::<crate::MenuStore>().set_open(false);
        crate::lyrics::save::ask_tags(st);
        let l = app.global::<Lyrics>();
        check(r, "asked first, the .lrc still read first said", l.get_ask() == "tags" && l.get_notice().contains("01 Track 1.lrc beside it is still read first"), state(app));
        snapshot(app, "ly-07-tags-asked");
        app.global::<Actions>().invoke_lyrics_answer("tags".into());
    });
    step!(5000, |st, app, r| {
        let raw = library::meta::read(&harbour(), library::meta::ReadOptions::RAW).ok();
        let lyr: Vec<String> = raw.map(|t| t.items.into_iter().filter(|i| library::meta::field_of(t.blocks[i.block], &i.key).is_some_and(|d| d.key == "LYRICS")).flat_map(|i| i.values).collect()).unwrap_or_default();
        let parsed = lyr.first().map(|t| lib::parse(t, "tags").lines.into_iter().map(|l| l.time_ms).collect::<Vec<_>>()).unwrap_or_default();
        let want: Vec<Option<u64>> = times(&harbour()).into_iter().map(|t| t.map(|t| t + 300)).collect();
        check(r, "the file's LYRICS: the .lrc's lines 0.3 s later", parsed == want && !want.is_empty(), format!("{} items, first {:?} want {:?}", lyr.len(), parsed.first(), want.first()));
        let saves = crate::metadata::tagedit::journal(st).list();
        check(r, "one save in the tag editor's journal (Undo can take it back)", saves.len() == 1 && saves[0].header.label.contains("Lyrics"), format!("{:?}", saves.iter().map(|s| s.header.label.clone()).collect::<Vec<_>>()));
        let l = app.global::<Lyrics>();
        check(r, "said; the .lrc still read first, so the offset stays", l.get_notice().contains(".lrc file is still read first") && l.get_offset() == 300, state(app));
        check(r, "no temporary file left", leftovers(&root().join("050 Mara Lind").join("Night Harbour")).is_empty(), "");
        snapshot(app, "ly-08-tags-saved");
        crate::lyrics::set_offset(st, 0);
        crate::metadata::tagedit::toggle(st);
        app.global::<Actions>().invoke_lyrics_answer("ok".into());
    });

    // ---- a server's track: refused --------------------------------------------------------
    step!(300, |st, app, r| {
        let remote = library::Track { id: -9, path: format!("{}bench/s-1", library::remote::SCHEME), title: "Remote".into(), remote_id: Some("s-1".into()), ..Default::default() };
        crate::lyrics::bench_pretend(st, remote, lib::parse("[00:01.00]From the server\n", "server"));
        crate::lyrics::save::save_file(st, false);
        let l = app.global::<Lyrics>();
        check(r, "a server's track: Save refused with a word", l.get_notice().contains("plays from the server") && l.get_notice_attention(), state(app));
        snapshot(app, "ly-09-server-refused");
    });
    step!(300, |_st, _app, r| {
        write_report("lyrics", &r.borrow());
        let _ = slint::quit_event_loop();
    });
    run_steps(steps);
}
