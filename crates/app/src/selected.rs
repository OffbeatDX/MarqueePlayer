//! The selection on screen (`selection.rs` is the model): the track lists' and the queue's
//! callbacks, what a drag or a menu of several tracks acts on, and those actions.
//!
//! One list has a selection at a time: choosing rows in one clears every other, and going to
//! another page clears it (`views::show`). A list whose tracks change clears its own
//! (`rows::TrackModel::set`); the queue keeps the items that stay (their uids).
//!
//! A menu for several tracks remembers which rows it was opened on (their keys) and acts only
//! if exactly those are still chosen: never on rows the user no longer sees chosen. Each action
//! acts once on all of them, in list order; what it writes is one transaction, and the pill says
//! what was done with how many ("Added 12 tracks to “Mix”"). Files from outside the library in
//! the queue have no place in a playlist or a favourite: they are left out and counted.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use library::{Track, query};
use slint::ComponentHandle;

use crate::dnd::Grabbed;
use crate::rows::Selectable;
use crate::selection::Selection;
use crate::state::State;
use crate::{playback, views};

/// The queue's name among the lists (the track lists' are their `ctx`).
pub const QUEUE: &str = "queue";

/// Every list that can have a selection.
const ALL: [&str; 8] = ["album", "artist", "search", "playlist", "tracks", "smart", "preview", QUEUE];

fn list<'a>(st: &'a State, ctx: &str) -> Option<&'a dyn Selectable> {
    if ctx == QUEUE { Some(&*st.models.queue) } else { st.lists.by_ctx(ctx).map(|l| &**l as &dyn Selectable) }
}

fn edit(st: &State, ctx: &str, mut f: impl FnMut(&mut Selection, &mut Vec<usize>)) {
    if let Some(l) = list(st, ctx) {
        l.edit_selection(&mut f);
    }
}

/// Choosing in `ctx`: every other list's selection goes.
fn others_clear(st: &State, ctx: &str) {
    for other in ALL.iter().filter(|c| **c != ctx) {
        if list(st, other).is_some_and(|l| !l.selection().is_empty()) {
            edit(st, other, |s, c| {
                s.clear(c);
            });
        }
    }
}

/// The left button down on a row (Ctrl, Shift as they were held).
pub fn press(st: &State, ctx: &str, row: usize, ctrl: bool, shift: bool) {
    others_clear(st, ctx);
    edit(st, ctx, |s, c| s.press(row, ctrl, shift, c));
}

/// The press on a row ended as a click (no drag).
pub fn release(st: &State, ctx: &str, row: usize) {
    edit(st, ctx, |s, c| s.release(row, c));
}

/// The list's keyboard cursor moved (with Shift: `extend`).
pub fn cursor(st: &State, ctx: &str, from: usize, to: usize, extend: bool) {
    others_clear(st, ctx);
    edit(st, ctx, |s, c| s.cursor(from, to, extend, c));
}

/// Ctrl+A.
pub fn all(st: &State, ctx: &str) {
    others_clear(st, ctx);
    edit(st, ctx, |s, c| s.all(c));
}

/// Esc: true when something was chosen (else Esc goes on: a sidebar closes, Back).
pub fn clear(st: &State, ctx: &str) -> bool {
    let mut had = false;
    edit(st, ctx, |s, c| had = s.clear(c));
    had
}

/// Every list's selection goes (another page; Esc wherever the keyboard is). True when there
/// was one.
pub fn clear_all(st: &State) -> bool {
    let mut had = false;
    for ctx in ALL {
        if list(st, ctx).is_some_and(|l| !l.selection().is_empty()) {
            had |= clear(st, ctx);
        }
    }
    had
}

/// A right click or the Menu key on `row`: how many tracks its menu is for (the selection it
/// is part of; a row outside the selection is chosen alone first). 0 when there is no such row.
pub fn context(st: &State, ctx: &str, row: usize) -> usize {
    others_clear(st, ctx);
    let mut inside = false;
    edit(st, ctx, |s, c| inside = s.context(row, c));
    if inside { list(st, ctx).map_or(0, |l| l.selection().len()) } else { 0 }
}

/// The chosen rows of `ctx`, in list order.
pub fn rows(st: &State, ctx: &str) -> Vec<usize> {
    list(st, ctx).map(|l| l.selection().rows()).unwrap_or_default()
}

fn keys(st: &State, ctx: &str) -> Vec<u64> {
    list(st, ctx).map(|l| l.selection().selected_keys()).unwrap_or_default()
}

/// The tracks of rows of `ctx`, in that order.
fn tracks_at(st: &State, ctx: &str, rows: &[usize]) -> Vec<Track> {
    if ctx == QUEUE {
        let q = st.queue.borrow();
        rows.iter().filter_map(|&i| q.items().get(i).map(|i| i.track.clone())).collect()
    } else {
        st.lists.by_ctx(ctx).map(|l| rows.iter().filter_map(|&i| l.get(i)).collect()).unwrap_or_default()
    }
}

/// What a drag from row `index` of `ctx` carries: the rows chosen when it is one of them,
/// else that row alone.
pub fn dragged_rows(st: &State, ctx: &str, index: usize) -> Vec<usize> {
    let chosen = list(st, ctx).is_some_and(|l| l.selection().is_selected(index));
    if chosen { rows(st, ctx) } else { vec![index] }
}

/// The tracks a drag from `row` of `ctx` carries, in list order; none when the track grabbed
/// has left the list. A list that changed under the drag has lost its selection with it
/// (`rows::TrackModel::set`), so then the drag carries the track grabbed alone.
pub fn dragged(st: &State, ctx: &str, row: impl Into<Grabbed>) -> Vec<Track> {
    let Some(index) = regrab(st, ctx, row.into()) else { return Vec::new() };
    tracks_at(st, ctx, &dragged_rows(st, ctx, index))
}

/// Where the row a drag took hold of is now: the row showing its track, the one nearest where
/// it was when the track is in the list more than once (a playlist). `None` when it is gone.
pub fn regrab(st: &State, ctx: &str, row: Grabbed) -> Option<usize> {
    let Some(track) = row.track else { return Some(row.index) };
    if ctx == QUEUE {
        let q = st.queue.borrow();
        return nearest(q.items().len(), row.index, |i| q.items()[i].track.id == track);
    }
    st.lists.by_ctx(ctx)?.find_near(row.index, track)
}

/// The first of `0..len` nearest `at` for which `is` holds: `at` itself, then one after, one
/// before, two after and so on.
pub fn nearest(len: usize, at: usize, is: impl Fn(usize) -> bool) -> Option<usize> {
    let at = at.min(len.saturating_sub(1));
    (0..len).flat_map(|d| [at.checked_add(d), if d == 0 { None } else { at.checked_sub(d) }]).flatten().filter(|&i| i < len).find(|&i| is(i))
}

/// "1 track", "12 tracks".
pub fn tracks_word(n: usize) -> String {
    if n == 1 { "1 track".to_owned() } else { format!("{n} tracks") }
}

/// Files from outside the library (the queue can hold them) have no id to keep a mark or a
/// playlist entry by: the library's tracks, and a note for the others ("2 skipped: not in the
/// library"; "" when none were).
fn in_library(tracks: Vec<Track>) -> (Vec<Track>, String) {
    let n = tracks.len();
    let kept: Vec<Track> = tracks.into_iter().filter(|t| t.id > 0).collect();
    let skipped = n - kept.len();
    let note = if skipped == 0 { String::new() } else { format!(", {skipped} skipped: not in the library") };
    (kept, note)
}

// ---- marks: one track or several ---------------------------------------------------

/// The user changed these tracks' favourites: noted for the server, one per track
/// (`server::write::changed`: sent at once when the server has the track and "Sync favourites"
/// is on). Several notes in one transaction.
fn tell_server(st: &Rc<State>, ids: &[i64]) {
    let tx = if ids.len() > 1 { library::db::write(&st.db).ok() } else { None };
    for &id in ids {
        crate::server::write::changed(st, id);
    }
    if let Some(tx) = tx
        && let Err(e) = tx.commit()
    {
        log::warn!("server: changes not noted for the server ({e})");
    }
}

/// Favourites were written (one track's, or a selection's in one transaction): the server told,
/// every list, the smart playlists and the player bar in step. Every change of a track's
/// favourite the user makes goes through here (not those the server's own sync takes).
pub fn favourites_written(st: &Rc<State>, ids: &[i64], fav: bool) {
    tell_server(st, ids);
    crate::connections::listens::love(st, ids, fav);
    crate::playlists::smart::changed(st);
    let set: HashSet<i64> = ids.iter().copied().collect();
    for l in st.lists.all() {
        l.set_favourites(&set, fav);
    }
    if ids.contains(&views::playing_track_id(st))
        && let Some(app) = st.app.upgrade()
    {
        app.global::<crate::Player>().set_favourite(fav);
    }
}

// ---- the menu of several tracks ---------------------------------------------------

/// What an entry of the menu of several tracks does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Many {
    Play,
    Enqueue { next: bool },
    Favourite(bool),
    RemoveFromPlaylist,
    QueueRemove,
}

thread_local! {
    /// The menu of several tracks that is open: its list and the rows' keys it was opened on.
    static OPENED: RefCell<Option<(String, Vec<u64>)>> = const { RefCell::new(None) };
}

/// A menu for the selection of `ctx` opened (after `menus::show`, which forgets the last).
pub fn menu_opened(st: &State, ctx: &str) {
    let k = keys(st, ctx);
    OPENED.with(|o| *o.borrow_mut() = Some((ctx.to_owned(), k)));
}

/// Another menu opened: the last one's rows are forgotten.
pub fn menu_closed() {
    OPENED.with(|o| o.borrow_mut().take());
}

/// The open menu is one for several tracks.
pub fn menu_is_open() -> bool {
    OPENED.with(|o| o.borrow().is_some())
}

/// What the open menu acts on: its list and the rows, if they are still the ones chosen.
/// Otherwise the pill says so and nothing is done.
fn target(st: &Rc<State>) -> Option<(String, Vec<usize>)> {
    let (ctx, want) = OPENED.with(|o| o.borrow().clone())?;
    if keys(st, &ctx) != want || want.is_empty() {
        views::flash(st, "Those tracks are no longer chosen: nothing was done");
        return None;
    }
    let rows = rows(st, &ctx);
    Some((ctx, rows))
}

/// Whether the tracks of the open menu are all favourites (for its entries).
pub fn all_favourites(st: &State, ctx: &str) -> bool {
    let tracks = tracks_at(st, ctx, &rows(st, ctx));
    let favs = views::favourite_set(st, tracks.iter().map(|t| t.id));
    !tracks.is_empty() && tracks.iter().all(|t| favs.contains(&t.id))
}

/// An entry of the menu of several tracks chosen.
pub fn act(st: &Rc<State>, what: Many) {
    let Some((ctx, rows)) = target(st) else { return };
    menu_closed();
    let tracks = tracks_at(st, &ctx, &rows);
    let n = tracks.len();
    match what {
        Many::Play if ctx == QUEUE => {
            if let Some(&first) = rows.first() {
                playback::queue_play(st, first);
            }
        }
        Many::Play => playback::play_tracks(st, tracks, 0),
        Many::Enqueue { next } => {
            playback::enqueue(st, tracks, next);
            views::flash(st, &if next { format!("{} to play next", tracks_word(n)) } else { format!("Added {} to the queue", tracks_word(n)) });
        }
        Many::Favourite(fav) => {
            let (kept, note) = in_library(tracks);
            let ids: Vec<i64> = kept.iter().map(|t| t.id).collect();
            if !ids.is_empty() {
                if let Err(e) = query::set_favourite_tracks(&st.db, &ids, fav) {
                    return views::not_written(st, "Could not change the favourites", &e);
                }
                favourites_written(st, &ids, fav);
            }
            let done = if fav { format!("Added {} to favourites", tracks_word(ids.len())) } else { format!("Removed {} from favourites", tracks_word(ids.len())) };
            views::flash(st, &format!("{done}{note}"));
        }
        Many::RemoveFromPlaylist => remove_from_playlist(st, &rows),
        Many::QueueRemove => {
            playback::queue_remove_many(st, &rows);
            views::flash(st, &format!("Removed {} from the queue", tracks_word(n)));
        }
    }
}

/// "Add to playlist" of the menu of several tracks: to the playlist `pid`, or (`None`) a new
/// one through the name prompt.
pub fn add_to_playlist(st: &Rc<State>, pid: Option<i64>) {
    let Some((ctx, rows)) = target(st) else { return };
    menu_closed();
    let tracks = tracks_at(st, &ctx, &rows);
    match pid {
        Some(pid) => add_tracks_to_playlist(st, tracks, pid),
        None => {
            let (kept, _) = in_library(tracks);
            if kept.is_empty() {
                return views::flash(st, "Not in the library");
            }
            crate::playlists::newplaylist::open_from_menu(st, crate::playlists::newplaylist::Target::Tracks(kept.iter().map(|t| t.id).collect()));
        }
    }
}

/// Tracks appended to an ordinary playlist (a drop on it, a menu): one transaction, the open
/// playlist shown again, and the pill says how many.
pub fn add_tracks_to_playlist(st: &Rc<State>, tracks: Vec<Track>, pid: i64) {
    if crate::playlists::smart::is_smart(st, pid) {
        crate::playlists::smart::hint(st);
        return;
    }
    let (kept, note) = in_library(tracks);
    if kept.is_empty() {
        if !note.is_empty() {
            views::flash(st, "Not in the library");
        }
        return;
    }
    let ids: Vec<i64> = kept.iter().map(|t| t.id).collect();
    if let Err(e) = query::add_to_playlist(&st.db, pid, &ids) {
        return views::not_written(st, "Could not add to the playlist", &e);
    }
    let name = query::playlist_name(&st.db, pid).ok().flatten().unwrap_or_default();
    views::playlists_changed(st);
    if st.open_playlist.get() == Some(pid) && st.history.borrow().current == crate::nav::Route::Playlist(pid) {
        views::reload_playlist(st, pid);
    }
    views::flash(st, &format!("{}{note}", crate::playlists::newplaylist::added_note(ids.len(), &name)));
}

/// The open playlist's rows `rows` removed, in one transaction, if it still holds what it shows
/// (rows by the tracks shown, never by stored positions, which a scan can leave with gaps).
pub fn remove_from_playlist(st: &Rc<State>, rows: &[usize]) {
    let Some(id) = st.open_playlist.get() else { return };
    let shown: Vec<i64> = st.lists.playlist.tracks().iter().map(|t| t.id).collect();
    let keep: Vec<i64> = shown.iter().enumerate().filter(|(i, _)| !rows.contains(i)).map(|(_, t)| *t).collect();
    match query::rewrite_playlist(&st.db, id, &shown, &keep) {
        Ok(true) => {
            let name = query::playlist_name(&st.db, id).ok().flatten().unwrap_or_default();
            views::flash(st, &format!("Removed {} from \u{201c}{name}\u{201d}", tracks_word(shown.len() - keep.len())));
        }
        Ok(false) => views::flash(st, "The playlist changed meanwhile: nothing was removed"),
        Err(e) => views::not_written(st, "Could not remove them from the playlist", &e),
    }
    views::playlists_changed(st);
    views::reload_playlist(st, id);
}

/// Delete in a playlist or the queue: the rows chosen, or the one at the cursor when none is.
/// Never anything in the library or on disk.
pub fn delete(st: &Rc<State>, ctx: &str, at: usize) {
    let chosen = rows(st, ctx);
    match ctx {
        QUEUE if chosen.len() > 1 => {
            playback::queue_remove_many(st, &chosen);
            views::flash(st, &format!("Removed {} from the queue", tracks_word(chosen.len())));
        }
        QUEUE => playback::queue_remove(st, chosen.first().copied().unwrap_or(at)),
        "playlist" if !chosen.is_empty() => remove_from_playlist(st, &chosen),
        "playlist" => remove_from_playlist(st, &[at]),
        _ => {}
    }
}

/// A row of the open playlist dragged to the line before `at`: with the rows chosen when it is
/// one of them, as one block in their order (one transaction), and they stay chosen there.
pub fn reorder_playlist(st: &Rc<State>, index: usize, at: usize) {
    let rows = dragged_rows(st, "playlist", index);
    if rows.len() <= 1 {
        return views::move_in_playlist(st, index, at);
    }
    let Some(id) = st.open_playlist.get() else { return };
    let shown: Vec<i64> = st.lists.playlist.tracks().iter().map(|t| t.id).collect();
    let (order, start) = crate::selection::move_block(shown.len(), &rows, at);
    let new: Vec<i64> = order.iter().map(|&j| shown[j]).collect();
    if new == shown {
        return;
    }
    match query::rewrite_playlist(&st.db, id, &shown, &new) {
        Ok(true) => {}
        Ok(false) => views::flash(st, "The playlist changed meanwhile: nothing was moved"),
        Err(e) => return views::not_written(st, "Could not reorder the playlist", &e),
    }
    views::reload_playlist(st, id);
    let k = rows.len();
    edit(st, "playlist", |s, c| s.select_rows(start..start + k, c));
}

/// A queue row dragged to the line before `at`: with the rows chosen when it is one of them,
/// as one block (they stay chosen: the queue keeps them by uid).
pub fn reorder_queue(st: &Rc<State>, from: usize, at: usize) {
    let rows = dragged_rows(st, QUEUE, from);
    if rows.len() <= 1 {
        return playback::queue_move(st, from, crate::dnd::move_target(from, at));
    }
    playback::queue_move_block(st, &rows, at);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_words() {
        assert_eq!(tracks_word(1), "1 track");
        assert_eq!(tracks_word(12), "12 tracks");
        let t = |id| Track { id, ..Track::default() };
        let (kept, note) = in_library(vec![t(3), t(-1), t(4), t(0)]);
        assert_eq!(kept.iter().map(|t| t.id).collect::<Vec<_>>(), [3, 4]);
        assert_eq!(note, ", 2 skipped: not in the library");
        assert_eq!(in_library(vec![t(1)]).1, "");
    }

    #[test]
    fn a_grabbed_row_is_found_again_nearest_where_it_was() {
        // The list a drag started on: track 7 grabbed at row 2.
        let rows = |ids: &'static [i64]| move |at: usize| nearest(ids.len(), at, |i| ids[i] == 7);
        // Unchanged.
        assert_eq!(rows(&[1, 2, 7, 4])(2), Some(2));
        // Re-sorted under the drag: wherever it went.
        assert_eq!(rows(&[7, 1, 2, 4])(2), Some(0));
        assert_eq!(rows(&[1, 2, 4, 5, 7])(2), Some(4));
        // In the list twice: the copy nearest the row grabbed.
        assert_eq!(rows(&[7, 1, 2, 7, 4])(2), Some(3));
        assert_eq!(rows(&[7, 1, 2, 4, 5, 7])(2), Some(0));
        // Gone, or the list shorter now.
        assert_eq!(rows(&[1, 2, 4])(2), None);
        assert_eq!(rows(&[7])(5), Some(0));
        assert_eq!(nearest(0, 3, |_| true), None);
    }
}
