//! Keeping the lyrics the Lyrics tab shows (its "⋯" menu, `menu_entries`): Copy lyrics, Save
//! as .lrc (or .txt for plain lyrics) beside the track, and, with tag editing on, Save into the
//! file's tags.
//!
//! The app otherwise only reads music folders (tag editing aside), so a save is the user's
//! own act, said in full where it went: the sidecar is `library::lyrics::sidecar` (the name
//! the app reads first next time), UTF-8, the timing offset in its time stamps (other players
//! then agree); it never replaces a file already there without asking (an inline question
//! under the lyrics, "Replace"), and a track that plays from the server, which has no folder,
//! is refused with a word. Into the tags goes the same text through the tag editor's checked
//! save (`metadata::tagedit::save`: the file checked before it is replaced, the journal, Edit history's
//! Undo, the rescan), never another writer. Afterwards the lyrics are read again: when what is
//! read is what was written, the tab shows it as the track's own and the offset goes back to
//! 0 (it is in the file now).
//!
//! What the tab says (`Lyrics.notice`, under the lyrics until the track changes or OK), and
//! what it asks (`Lyrics.ask`: "replace", "tags", or "" for a note).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use library::lyrics::{self, Line, SaveError};
use slint::ComponentHandle;

use crate::menus::{Act, Entry};
use crate::state::{self, State};
use crate::{Lyrics as LyricsStore, metadata, views};

/// What an inline question is about: the track's lyrics as they were when asked.
#[derive(Clone, Debug)]
enum Pending {
    Replace,
    Tags,
}

thread_local! {
    static PENDING: RefCell<Option<(u64, Pending)>> = const { RefCell::new(None) };
}

/// The tab's "⋯" menu: what can be done with the lyrics shown (none: an empty menu).
pub fn menu_entries(st: &State) -> Vec<Entry> {
    let Some((t, l, offset)) = crate::lyrics::shown() else { return Vec::new() };
    let item = |label: &str, act: Act| Entry::Item { label: label.into(), act };
    let mut v = vec![item("Copy lyrics", Act::LyricsCopy), item(if l.synced { "Save as .lrc" } else { "Save as .txt" }, Act::LyricsSaveFile)];
    if metadata::tagedit::enabled(st) && !t.is_remote() {
        v.push(item("Save into the file\u{2019}s tags\u{2026}", Act::LyricsSaveTags));
    }
    if l.synced && offset != 0 {
        v.push(Entry::Separator);
        v.push(item(&format!("Reset timing ({})", lyrics::offset_text(offset)), Act::LyricsResetOffset));
    }
    v
}

pub fn menu(st: &Rc<State>, x: f32, y: f32, from_keys: bool) {
    let e = menu_entries(st);
    if !e.is_empty() {
        crate::menus::show_plain(st, e, x, y, from_keys);
    }
}

/// What the lyrics come to as a file: the text, and the lines it reads back as.
fn as_file(t: &library::Track, l: &lyrics::Lyrics, offset: i64) -> (String, Vec<Line>) {
    let text = if l.synced { lyrics::to_lrc(&l.lines, offset, &[("ti", &t.title), ("ar", &t.artist), ("al", &t.album)]) } else { lyrics::to_text(&l.lines) };
    let back = lyrics::parse(&text, "").lines;
    (text, back)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Say something under the lyrics; `ask` a question ("replace", "tags") or "" for a note.
fn notice(st: &Rc<State>, text: &str, ask: &str, attention: bool) {
    let Some(app) = st.app.upgrade() else { return };
    let s = app.global::<LyricsStore>();
    s.set_notice(text.into());
    s.set_ask(ask.into());
    s.set_notice_attention(attention);
}

/// The track changed, or OK / Cancel: nothing said, nothing asked.
pub fn clear_notice(st: &Rc<State>) {
    PENDING.with(|p| p.borrow_mut().take());
    notice(st, "", "", false);
}

/// The answer to the question under the lyrics: "replace", "tags", or anything else to let
/// it be.
pub fn answer(st: &Rc<State>, a: &str) {
    let pending = PENDING.with(|p| p.borrow_mut().take());
    clear_notice(st);
    let Some((g, what)) = pending else { return };
    if g != crate::lyrics::generation() {
        return;
    }
    match (a, what) {
        ("replace", Pending::Replace) => save_file(st, true),
        ("tags", Pending::Tags) => save_tags(st),
        _ => {}
    }
}

/// "Copy lyrics": the words, one line each (no time stamps).
pub fn copy(st: &Rc<State>) {
    let Some((_, l, _)) = crate::lyrics::shown() else { return };
    if crate::explorer::copy_text(&lyrics::to_text(&l.lines)) {
        views::flash(st, "Copied the lyrics");
    } else {
        views::flash(st, "Could not use the clipboard");
    }
}

/// "Reset timing".
pub fn reset_offset(st: &Rc<State>) {
    crate::lyrics::set_offset(st, 0);
}

/// "Save as .lrc" / ".txt": beside the track; `replace` only once the user said so.
pub fn save_file(st: &Rc<State>, replace: bool) {
    let Some((t, l, offset)) = crate::lyrics::shown() else { return };
    if t.is_remote() {
        notice(st, "This track plays from the server: there is no folder beside it to save lyrics in.", "", true);
        return;
    }
    let track = PathBuf::from(&t.path);
    let path = lyrics::sidecar(&track, l.synced);
    let (text, back) = as_file(&t, &l, offset);
    let g = crate::lyrics::generation();
    let spawned = std::thread::Builder::new().name("mp-lyrics-save".into()).spawn(move || {
        let r = lyrics::write_sidecar(&path, &text, replace);
        // What the track reads now (the file just written, unless one is read before it).
        let now = r.as_ref().ok().and_then(|()| lyrics::load(&track));
        state::post(move |st| saved_file(st, g, &path, r, now, &back, offset));
    });
    if let Err(e) = spawned {
        notice(st, &format!("Could not save the lyrics: {e}."), "", true);
    }
}

fn saved_file(st: &Rc<State>, g: u64, path: &Path, r: Result<(), SaveError>, now: Option<lyrics::Lyrics>, back: &[Line], offset: i64) {
    let here = g == crate::lyrics::generation();
    let said = match r {
        Ok(()) => {
            log::info!("lyrics saved to {}", path.display());
            match now {
                Some(l) if l.lines == back => {
                    let moved = offset != 0 && l.synced;
                    crate::lyrics::reloaded(st, g, l, moved);
                    if moved {
                        format!("Saved {}, with the timing ({}) in its time stamps.", path.display(), lyrics::offset_text(offset))
                    } else {
                        format!("Saved {}.", path.display())
                    }
                }
                Some(l) => format!("Saved {}, but the track\u{2019}s .{} file is read before it.", path.display(), l.source),
                None => format!("Saved {}.", path.display()),
            }
        }
        Err(SaveError::Exists) if here => {
            PENDING.with(|p| *p.borrow_mut() = Some((g, Pending::Replace)));
            notice(st, &format!("{} is there already. Replace it with these lyrics?", path.display()), "replace", false);
            return;
        }
        Err(SaveError::Exists) => format!("{} is there already: not saved.", path.display()),
        Err(e) => {
            log::warn!("lyrics not saved to {}: {e}", path.display());
            let text = format!("Could not save {}: {e}.", path.display());
            if here {
                notice(st, &text, "", true);
            } else {
                views::flash_long(st, &text);
            }
            return;
        }
    };
    if here {
        notice(st, &said, "", false);
    } else {
        views::flash_long(st, &said);
    }
}

/// "Save into the file's tags…": asked first, under the lyrics.
pub fn ask_tags(st: &Rc<State>) {
    let Some((t, _, _)) = crate::lyrics::shown() else { return };
    if t.is_remote() {
        notice(st, "This track plays from the server: it has no file here whose tags could hold its lyrics.", "", true);
        return;
    }
    if !metadata::tagedit::enabled(st) {
        notice(st, "Saving into the tags needs Settings > Library > Edit tags.", "", true);
        return;
    }
    let track = PathBuf::from(&t.path);
    let first = [lyrics::sidecar(&track, true), lyrics::sidecar(&track, false)].into_iter().find(|p| p.is_file());
    let also = first.map(|p| format!(" {} beside it is still read first.", file_name(&p))).unwrap_or_default();
    PENDING.with(|p| *p.borrow_mut() = Some((crate::lyrics::generation(), Pending::Tags)));
    notice(
        st,
        &format!("Write these lyrics into the tags of {}? The tag editor checks the file before it replaces it, and View metadata\u{2019}s Edit history can undo it.{also}", file_name(&track)),
        "tags",
        false,
    );
}

/// Write the lyrics shown into the playing file's tags, through the tag editor's save.
fn save_tags(st: &Rc<State>) {
    let Some((t, l, offset)) = crate::lyrics::shown() else { return };
    if t.is_remote() || !metadata::tagedit::enabled(st) {
        return;
    }
    if metadata::tagedit::saving() {
        notice(st, "Another tag save is under way: try again when it is done.", "", true);
        return;
    }
    let track = PathBuf::from(&t.path);
    let (text, back) = as_file(&t, &l, offset);
    let g = crate::lyrics::generation();
    let label = format!("Track \u{201c}{}\u{201d}: Lyrics", t.title);
    let summary = format!("Lyrics: {} lines, {}", l.lines.len(), if l.synced { "synced (time stamps)" } else { "plain text" });
    notice(st, "Saving the lyrics into the file\u{2019}s tags\u{2026}", "", false);
    // What the file must still be when written: as read now (off this thread: a slow drive).
    let spawned = std::thread::Builder::new().name("mp-lyrics-tags".into()).spawn(move || {
        let expect = library::meta::read(&track, library::meta::ReadOptions::TAGS).ok().and_then(|tags| library::edit::expect_now(&track, &tags));
        state::post(move |st| {
            let Some(expect) = expect else {
                notice(st, &format!("Could not read the tags of {}.", file_name(&track)), "", true);
                return;
            };
            let edit = library::edit::FileEdit { path: track.clone(), fields: vec![("LYRICS".into(), vec![text])], raw: Vec::new(), pictures: None, expect, summary: vec![summary] };
            let started = metadata::tagedit::save(st, vec![edit], label, None, move |st, msg| {
                if let metadata::tagedit::Saving::Done(o) = msg {
                    saved_tags(st, g, &track, &o, back.clone(), offset);
                }
            });
            if !started {
                notice(st, "Saving into the tags is not possible now (another save is under way, or tag editing is off).", "", true);
            }
        });
    });
    if let Err(e) = spawned {
        notice(st, &format!("Could not save the lyrics: {e}."), "", true);
    }
}

fn saved_tags(st: &Rc<State>, g: u64, track: &Path, o: &metadata::tagedit::Outcome, back: Vec<Line>, offset: i64) {
    let (title, text, attention) = metadata::tagedit::outcome_text(o);
    log::info!("lyrics into the tags: {title} {text}");
    metadata::tagedit::sync_settings(st);
    let saved: Vec<PathBuf> = o.report.saved.iter().map(|s| s.path.clone()).collect();
    metadata::tagedit::rescan_saved(st, &saved);
    let said = format!("{title} {text}").trim().to_owned();
    if attention || g != crate::lyrics::generation() {
        if g == crate::lyrics::generation() {
            notice(st, &said, "", true);
        } else {
            views::flash_long(st, &said);
        }
        return;
    }
    // Read again: the tags' lyrics are what the track shows now, unless a sidecar is read
    // before them.
    let track = track.to_path_buf();
    let _ = std::thread::Builder::new().name("mp-lyrics-reread".into()).spawn(move || {
        let now = lyrics::load(&track);
        state::post(move |st| {
            let said = match now {
                Some(l) if l.source == "tags" && l.lines == back => {
                    let moved = offset != 0 && l.synced;
                    crate::lyrics::reloaded(st, g, l, moved);
                    if moved { format!("Saved the lyrics into the tags of {}, with the timing ({}) in their time stamps.", file_name(&track), lyrics::offset_text(offset)) } else { format!("Saved the lyrics into the tags of {}.", file_name(&track)) }
                }
                Some(l) if l.source != "tags" => format!("Saved the lyrics into the tags of {}; its .{} file is still read first.", file_name(&track), l.source),
                _ => format!("Saved the lyrics into the tags of {}, but they read back differently: another lyrics item in the file is read first.", file_name(&track)),
            };
            if g == crate::lyrics::generation() {
                notice(st, &said, "", false);
            }
        });
    });
}
