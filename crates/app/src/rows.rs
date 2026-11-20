//! Track and queue models for the UI, with covers filled in lazily (see covers.rs).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use library::Track;
use library::audio::{self, Column, FormatKey};
use slint::{Image, Model, ModelNotify, ModelTracker};

use crate::covers::{self, CoverListener, Kind};
use crate::queue::Item;
use crate::selection::Selection;
use crate::{QueueRow, TrackRow};

pub fn format_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Rows waiting for covers, shared by the models below.
#[derive(Default)]
struct Waiting(RefCell<HashMap<String, Vec<usize>>>);

impl Waiting {
    fn cover(&self, stem: Option<&str>, row: usize) -> Image {
        let Some(stem) = stem else { return Image::default() };
        match covers::get(stem, Kind::Thumb) {
            Some(img) => img,
            None => {
                self.0.borrow_mut().entry(stem.to_owned()).or_default().push(row);
                Image::default()
            }
        }
    }

    fn take(&self, stem: &str) -> Vec<usize> {
        self.0.borrow_mut().remove(stem).unwrap_or_default()
    }

    fn clear(&self) {
        self.0.borrow_mut().clear();
    }
}

thread_local! {
    /// Every rated track's stars (UI thread): loaded at start, kept in step by `rate`.
    static RATINGS: RefCell<std::collections::HashMap<i64, u8>> = RefCell::default();
}

pub fn load_ratings(conn: &rusqlite::Connection) {
    let r = library::query::ratings(conn).unwrap_or_default();
    RATINGS.with(|m| *m.borrow_mut() = r);
}

pub fn rating_of(id: i64) -> u8 {
    RATINGS.with(|m| m.borrow().get(&id).copied().unwrap_or(0))
}

pub fn remember_rating(id: i64, stars: u8) {
    RATINGS.with(|m| {
        if stars == 0 {
            m.borrow_mut().remove(&id);
        } else {
            m.borrow_mut().insert(id, stars);
        }
    });
}

/// "★★★☆☆"; "" for none.
pub fn stars(n: u8) -> String {
    if n == 0 { String::new() } else { "★".repeat(n.min(5) as usize) + &"☆".repeat(5 - n.min(5) as usize) }
}

/// A list of tracks on screen (album, artist top tracks, search, playlist).
pub struct TrackModel {
    tracks: RefCell<Vec<Track>>,
    favourites: RefCell<HashSet<i64>>,
    playing: Cell<i64>,
    with_covers: bool,
    /// Album pages show "Disc N" headers when there is more than one disc.
    disc_headers: Cell<bool>,
    /// Which rows show their technical details (`metadata/details.rs`): all, or (an album with
    /// one main format) only those of another. Made only while the setting is on.
    details: Cell<Column>,
    waiting: Waiting,
    /// Which rows are chosen (`selection.rs`), each row known by a key of its own while the
    /// tracks stay the same.
    selection: RefCell<Selection>,
    notify: ModelNotify,
}

thread_local! {
    /// The next key for a track list's rows: never the same twice in a run.
    static NEXT_KEY: Cell<u64> = const { Cell::new(1) };
}

fn new_keys(n: usize) -> Vec<u64> {
    NEXT_KEY.with(|k| {
        let first = k.get();
        k.set(first + n as u64);
        (first..first + n as u64).collect()
    })
}

/// A list whose rows can be chosen (`selection.rs`): a track list or the queue.
pub trait Selectable {
    fn selection(&self) -> std::cell::Ref<'_, Selection>;
    /// Change the selection; the rows it says changed are drawn again.
    fn edit_selection(&self, f: &mut dyn FnMut(&mut Selection, &mut Vec<usize>));
}

fn edit_with(sel: &RefCell<Selection>, notify: &ModelNotify, f: &mut dyn FnMut(&mut Selection, &mut Vec<usize>)) {
    let mut changed = Vec::new();
    f(&mut sel.borrow_mut(), &mut changed);
    for row in changed {
        notify.row_changed(row);
    }
}

impl TrackModel {
    pub fn new(with_covers: bool) -> Rc<Self> {
        let m = Rc::new(Self {
            tracks: RefCell::default(),
            favourites: RefCell::default(),
            playing: Cell::new(-1),
            with_covers,
            disc_headers: Cell::new(false),
            details: Cell::new(Column::Shown),
            waiting: Waiting::default(),
            selection: RefCell::default(),
            notify: ModelNotify::default(),
        });
        let weak: Weak<dyn CoverListener> = Rc::downgrade(&(m.clone() as Rc<dyn CoverListener>));
        covers::register(weak);
        m
    }

    /// The list's tracks. The same tracks in the same order (a page refilled, a smart playlist
    /// worked out again to the same result) keep their selection; any other change clears it.
    pub fn set(&self, tracks: Vec<Track>, favourites: HashSet<i64>, disc_headers: bool) {
        let same = {
            let old = self.tracks.borrow();
            old.len() == tracks.len() && old.iter().zip(&tracks).all(|(a, b)| a.id == b.id)
        };
        if !same {
            self.selection.borrow_mut().reset(new_keys(tracks.len()));
        }
        *self.tracks.borrow_mut() = tracks;
        *self.favourites.borrow_mut() = favourites;
        self.disc_headers.set(disc_headers);
        self.waiting.clear();
        self.notify.reset();
    }

    /// Which rows show their details (album pages; other lists show every row's).
    pub fn set_details_column(&self, column: Column) {
        self.details.set(column);
    }

    /// The details setting changed: make the rows again.
    pub fn details_changed(&self) {
        self.notify.reset();
    }

    fn details_of(&self, t: &Track) -> String {
        if !crate::metadata::details::shown() {
            return String::new();
        }
        match self.details.get() {
            Column::Hidden => String::new(),
            Column::Except(key) if FormatKey::of(&t.audio) == key => String::new(),
            _ => audio::compact(&t.audio),
        }
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.tracks.borrow().clone()
    }

    pub fn get(&self, i: usize) -> Option<Track> {
        self.tracks.borrow().get(i).cloned()
    }

    /// The chosen tracks, in list order.
    pub fn selected_tracks(&self) -> Vec<Track> {
        let tracks = self.tracks.borrow();
        self.selection.borrow().rows().into_iter().filter_map(|i| tracks.get(i).cloned()).collect()
    }

    pub fn set_playing(&self, track_id: i64) {
        let old = self.playing.replace(track_id);
        if old == track_id {
            return;
        }
        let tracks = self.tracks.borrow();
        for (i, t) in tracks.iter().enumerate() {
            if t.id == old || t.id == track_id {
                self.notify.row_changed(i);
            }
        }
    }

    /// A track's rating changed: redraw its rows.
    pub fn rating_changed(&self, track_id: i64) {
        self.redraw(|id| id == track_id);
    }

    /// Several tracks' ratings changed (a selection rated): one pass over the list.
    pub fn ratings_changed(&self, ids: &HashSet<i64>) {
        self.redraw(|id| ids.contains(&id));
    }

    fn redraw(&self, of: impl Fn(i64) -> bool) {
        let tracks = self.tracks.borrow();
        for (i, t) in tracks.iter().enumerate() {
            if of(t.id) {
                self.notify.row_changed(i);
            }
        }
    }

    pub fn set_favourite(&self, track_id: i64, fav: bool) {
        self.set_favourites(&HashSet::from([track_id]), fav);
    }

    /// Several tracks favourited or not (a selection): one pass over the list.
    pub fn set_favourites(&self, ids: &HashSet<i64>, fav: bool) {
        {
            let mut f = self.favourites.borrow_mut();
            for id in ids {
                if fav {
                    f.insert(*id);
                } else {
                    f.remove(id);
                }
            }
        }
        self.redraw(|id| ids.contains(&id));
    }
}

impl Model for TrackModel {
    type Data = TrackRow;

    fn row_count(&self) -> usize {
        self.tracks.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<TrackRow> {
        let tracks = self.tracks.borrow();
        let t = tracks.get(row)?;
        // A track without a disc number is on disc 1, as the album's order has it: "DISC 1"
        // once, whether its tracks carry the number or not.
        let disc_header = if self.disc_headers.get()
            && (row == 0 || tracks[row - 1].disc_no.unwrap_or(1) != t.disc_no.unwrap_or(1))
        {
            format!("DISC {}", t.disc_no.unwrap_or(1))
        } else {
            String::new()
        };
        let number = match t.track_no {
            Some(n) => n.to_string(),
            None => (row + 1).to_string(),
        };
        Some(TrackRow {
            id: t.id as i32,
            album_id: t.album_id as i32,
            number: number.into(),
            title: t.title.as_str().into(),
            artist: t.artist.as_str().into(),
            album: t.album.as_str().into(),
            duration: format_time(t.duration).into(),
            playing: t.id == self.playing.get(),
            favourite: self.favourites.borrow().contains(&t.id),
            stars: stars(rating_of(t.id)).into(),
            cover: if self.with_covers { self.waiting.cover(t.art.as_deref(), row) } else { Image::default() },
            disc_header: disc_header.into(),
            details: self.details_of(t).into(),
            remote: t.is_remote(),
            selected: self.selection.borrow().is_selected(row),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

impl Selectable for TrackModel {
    fn selection(&self) -> std::cell::Ref<'_, Selection> {
        self.selection.borrow()
    }
    fn edit_selection(&self, f: &mut dyn FnMut(&mut Selection, &mut Vec<usize>)) {
        edit_with(&self.selection, &self.notify, f);
    }
}

impl CoverListener for TrackModel {
    fn cover_ready(&self, stem: &str, kind: Kind) {
        if kind != Kind::Thumb {
            return;
        }
        let n = self.tracks.borrow().len();
        for r in self.waiting.take(stem) {
            if r < n {
                self.notify.row_changed(r);
            }
        }
    }
    fn cover_dropped(&self, stem: &str) {
        self.cover_ready(stem, Kind::Thumb);
    }
}

/// The play queue as shown in the queue panel and on the now-playing page.
pub struct QueueModel {
    items: RefCell<Vec<Item>>,
    current: Cell<Option<usize>>,
    waiting: Waiting,
    /// Which rows are chosen, by item uid (they keep it through moves and additions).
    selection: RefCell<Selection>,
    notify: ModelNotify,
}

impl QueueModel {
    pub fn new() -> Rc<Self> {
        let m = Rc::new(Self {
            items: RefCell::default(),
            current: Cell::new(None),
            waiting: Waiting::default(),
            selection: RefCell::default(),
            notify: ModelNotify::default(),
        });
        let weak: Weak<dyn CoverListener> = Rc::downgrade(&(m.clone() as Rc<dyn CoverListener>));
        covers::register(weak);
        m
    }

    pub fn set(&self, items: &[Item], current: Option<usize>) {
        self.selection.borrow_mut().rekey(items.iter().map(|i| i.uid).collect());
        *self.items.borrow_mut() = items.to_vec();
        self.current.set(current);
        self.waiting.clear();
        self.notify.reset();
    }
}

impl Model for QueueModel {
    type Data = QueueRow;

    fn row_count(&self) -> usize {
        self.items.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<QueueRow> {
        let items = self.items.borrow();
        let t = &items.get(row)?.track;
        Some(QueueRow {
            title: t.title.as_str().into(),
            track_id: t.id.max(0) as i32,
            artist: t.artist.as_str().into(),
            album: t.album.as_str().into(),
            album_id: t.album_id as i32,
            has_cover: t.art.is_some(),
            tint: crate::covers::tint_for(&t.album),
            duration: format_time(t.duration).into(),
            current: self.current.get() == Some(row),
            cover: self.waiting.cover(t.art.as_deref(), row),
            remote: t.is_remote(),
            selected: self.selection.borrow().is_selected(row),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

impl Selectable for QueueModel {
    fn selection(&self) -> std::cell::Ref<'_, Selection> {
        self.selection.borrow()
    }
    fn edit_selection(&self, f: &mut dyn FnMut(&mut Selection, &mut Vec<usize>)) {
        edit_with(&self.selection, &self.notify, f);
    }
}

impl CoverListener for QueueModel {
    fn cover_ready(&self, stem: &str, kind: Kind) {
        if kind != Kind::Thumb {
            return;
        }
        let n = self.items.borrow().len();
        for r in self.waiting.take(stem) {
            if r < n {
                self.notify.row_changed(r);
            }
        }
    }
    fn cover_dropped(&self, stem: &str) {
        self.cover_ready(stem, Kind::Thumb);
    }
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(61.9), "1:01");
        assert_eq!(format_time(3725.0), "1:02:05");
    }
}
