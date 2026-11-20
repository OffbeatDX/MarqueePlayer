//! The app's context menus (drawn by ui/menu.slint). Each menu is built here from what
//! was right-clicked (or reached with the Menu key), and an entry runs the same `Actions`
//! callback the old built-in menus called, so behaviour is unchanged. The Albums page's
//! sort menu is one too: a menu of choices, the current one checked.

use std::cell::RefCell;
use std::rc::Rc;

use library::query::{self, AlbumSort};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::state::State;
use crate::{Actions, AppWindow, MenuItemData, MenuStore, playback, views};

/// What an entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    PlayAlbum(i32),
    ShuffleAlbum(i32),
    EnqueueAlbum(i32, bool),
    OpenArtist(String),
    OpenAlbum(i32),
    PlayIn(String, i32),
    EnqueueIn(String, i32, bool),
    FavouriteIn(String, i32),
    /// A row of the open playlist, by its index there.
    RemoveFromPlaylist(i32),
    QueuePlay(i32),
    QueueRemove(i32),
    PlayPlaylist(i64),
    EnqueuePlaylist(i64),
    OpenPlaylist(i32),
    /// A file or folder, selected in a new Explorer window (`explorer.rs`).
    ShowInExplorer(String),
    SortAlbums(AlbumSort),
    /// Settings > Output: a device by id, or the system default.
    SetOutput(Option<String>),
    /// Save a playlist as M3U8; true: paths relative to the file.
    ExportPlaylist(i64, bool),
    /// The rating menu of a track (by id).
    RateMenu(i64),
    /// Rate a track (by id), 0 clearing it.
    Rate(i64, u8),
    /// A playlist (by id), ordinary or smart: play it shuffled. A smart one: edit its rules,
    /// and its page's menu
    /// (rename in place, duplicate into the editor, save what it holds as an ordinary one).
    ShufflePlaylist(i64),
    EditSmart(i64),
    RenameSmart,
    DuplicateSmart(i64),
    FreezeSmart(i64),
    /// "View metadata" (metadata/view.rs): of a list's row (ctx, index), of a queue row, of an album.
    Metadata(String, i32),
    QueueMetadata(i32),
    AlbumMetadata(i32),
    /// Settings > Equalizer's preset menu (eq.rs): a preset by name, and what can be done.
    EqPreset(String),
    EqSave,
    EqRename,
    EqDelete,
    EqImport,
    EqExport(crate::eq::ExportAs),
    /// "Copy as…" a format; a band's kind (band index, `Kind::ALL` index).
    EqCopyAs(crate::eq::CopyAs),
    EqBandKind(usize, usize),
    /// Now playing's visualizer menu (visualizer/mod.rs): on and off, a style, a preset, where
    /// it shows (each by its index), its settings.
    VizToggle,
    VizPreset(usize),
    VizPlacement(usize),
    VizSettings,
    /// Its opacity where it shows now, in percent.
    VizOpacity(u32),
    /// Settings > Visualizer's preset menu (visualizer/presets.rs).
    VizSave,
    VizRename,
    VizUpdate,
    VizDelete,
    VizCopy,
    VizPaste,
    VizExport,
    VizImport,
    /// The Lyrics tab's menu (lyrics/save.rs).
    LyricsCopy,
    LyricsSaveFile,
    LyricsSaveTags,
    LyricsResetOffset,
    /// The menu of several tracks (the selection: `selected.rs`).
    Many(crate::selected::Many),
}

/// What an entry of the "Add to playlist" submenu is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubChoice {
    /// "New playlist…"
    New,
    /// The playlist at this index of the list.
    Playlist(usize),
    /// The separator.
    None,
}

/// The submenu's entries: "New playlist…", then, after a separator, the playlists. With
/// no playlists yet it is "New playlist…" alone.
pub fn sub_labels(playlists: &[(String, i64)]) -> Vec<Option<String>> {
    let mut v = vec![Some("New playlist\u{2026}".to_owned())];
    if !playlists.is_empty() {
        v.push(None);
        v.extend(playlists.iter().map(|(n, _)| Some(n.clone())));
    }
    v
}

/// The submenu entry at `sub` (as `sub_labels` lays them out).
pub fn sub_choice(sub: i32, playlists: usize) -> SubChoice {
    match sub {
        0 => SubChoice::New,
        1 if playlists > 0 => SubChoice::None,
        s if s >= 2 && ((s - 2) as usize) < playlists => SubChoice::Playlist((s - 2) as usize),
        _ => SubChoice::None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item { label: String, act: Act },
    /// A choice in effect (the current sort): drawn with a check mark.
    Checked { label: String, act: Act },
    /// "Add to playlist": its children are the playlists.
    Playlists { label: String },
    /// A submenu of these entries (a menu may have several; each opens beside its row).
    Choices { label: String, children: Vec<Entry> },
    Separator,
    /// A small caption over the entries that follow ("Presets"): says what they are, and
    /// cannot be chosen.
    Heading { label: String },
    /// An entry shown dimmed that cannot be chosen (what the menu of one track has, which
    /// several cannot do).
    Disabled { label: String },
}

/// What a track menu needs to know about its track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub id: i64,
    pub rating: u8,
    pub album: i32,
    /// Its credited artists (key, name), or the credit's names for a file from outside.
    pub artists: Vec<(String, String)>,
    pub favourite: bool,
    pub path: String,
}

/// What an album menu needs to know about its album (filled in `request`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumInfo {
    /// Its album artists (key, name).
    pub artists: Vec<(String, String)>,
    pub folder: String,
}

struct Open {
    entries: Vec<Entry>,
    /// (name, id) of the playlists in the submenu.
    playlists: Vec<(String, i64)>,
}

impl Open {
    /// The children of the `Choices` entry `i`, if it is one.
    fn choices(&self, i: i32) -> Option<&[Entry]> {
        match self.entries.get(usize::try_from(i).ok()?)? {
            Entry::Choices { children, .. } => Some(children.as_slice()),
            _ => None,
        }
    }

    /// Which rows of entry `i`'s submenu can be highlighted.
    fn sub_selectable(&self, i: i32) -> Vec<bool> {
        match self.choices(i) {
            Some(c) => c.iter().map(selectable).collect(),
            None => sub_labels(&self.playlists).iter().map(Option::is_some).collect(),
        }
    }

    /// The rows of entry `i`'s submenu: its choices, or "Add to playlist"'s playlists.
    fn sub_rows(&self, i: i32) -> Vec<MenuItemData> {
        match self.choices(i) {
            Some(c) => c.iter().map(menu_row).collect(),
            None => sub_labels(&self.playlists)
                .into_iter()
                .map(|l| match l {
                    Some(label) => row(&label, false, false, true),
                    None => row("", true, false, false),
                })
                .collect(),
        }
    }
}

/// Open entry `i`'s submenu beside it (its row hovered, Right or Enter on it): its rows,
/// where it opens. Nothing for an entry without one.
pub fn open_sub(st: &Rc<State>, i: i32) {
    let Some(app) = st.app.upgrade() else { return };
    let Some((rows, y)) = OPEN.with(|o| {
        let o = o.borrow();
        let o = o.as_ref()?;
        let e = o.entries.get(usize::try_from(i).ok()?)?;
        has_sub(e).then(|| (o.sub_rows(i), row_y(&o.entries, i as usize)))
    }) else {
        return;
    };
    let m = app.global::<MenuStore>();
    m.set_sub(ModelRc::new(VecModel::from(rows)));
    m.set_sub_row_y(y);
    m.set_sub_of(i);
}

/// The entry can be highlighted and chosen (not a separator, not a heading).
fn selectable(e: &Entry) -> bool {
    !matches!(e, Entry::Separator | Entry::Heading { .. } | Entry::Disabled { .. })
}

fn has_sub(e: &Entry) -> bool {
    matches!(e, Entry::Playlists { .. } | Entry::Choices { .. })
}

fn menu_row(e: &Entry) -> MenuItemData {
    match e {
        Entry::Item { label, act } => MenuItemData {
            danger: matches!(act, Act::RemoveFromPlaylist(_) | Act::Many(crate::selected::Many::RemoveFromPlaylist)),
            ..row(label, false, false, true)
        },
        Entry::Disabled { label } => row(label, false, false, false),
        Entry::Checked { label, .. } => MenuItemData { checked: true, ..row(label, false, false, true) },
        Entry::Playlists { label } | Entry::Choices { label, .. } => row(label, false, true, true),
        Entry::Separator => row("", true, false, false),
        Entry::Heading { label } => MenuItemData { heading: true, ..row(label, false, false, false) },
    }
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// What the open track or queue menu was opened on: its row then, and the item itself.
    static ANCHOR: RefCell<Option<Anchor>> = const { RefCell::new(None) };
}

/// A menu's entries name rows by index (the list's actions take one). A list can change while
/// its menu is open (a smart playlist working itself out again, a scan, the queue moving on), so
/// the item is remembered too and found again when an entry is chosen (`reanchored`).
#[derive(Clone, Debug)]
enum Anchor {
    Row { ctx: String, index: i32, track: i64 },
    Queue { index: i32, uid: u64 },
}

/// `act` with its row index pointing at the item the menu was opened on, wherever that item
/// is now; `None` when it is no longer in its list.
fn reanchored(st: &State, act: Act) -> Option<Act> {
    let anchor = ANCHOR.with(|a| a.borrow().clone());
    let row = |ctx: &str, i: i32| -> Option<i32> {
        match &anchor {
            Some(Anchor::Row { ctx: c, index, track }) if c == ctx && *index == i => {
                let list = st.lists.by_ctx(ctx)?;
                if list.get(i.max(0) as usize).is_some_and(|t| t.id == *track) {
                    return Some(i);
                }
                (0..slint::Model::row_count(&**list)).find(|&j| list.get(j).is_some_and(|t| t.id == *track)).map(|j| j as i32)
            }
            _ => Some(i),
        }
    };
    let queued = |i: i32| -> Option<i32> {
        match &anchor {
            Some(Anchor::Queue { index, uid }) if *index == i => st.queue.borrow().items().iter().position(|q| q.uid == *uid).map(|j| j as i32),
            _ => Some(i),
        }
    };
    Some(match act {
        Act::PlayIn(c, i) => Act::PlayIn(c.clone(), row(&c, i)?),
        Act::EnqueueIn(c, i, next) => Act::EnqueueIn(c.clone(), row(&c, i)?, next),
        Act::FavouriteIn(c, i) => Act::FavouriteIn(c.clone(), row(&c, i)?),
        Act::Metadata(c, i) => Act::Metadata(c.clone(), row(&c, i)?),
        Act::RemoveFromPlaylist(i) => Act::RemoveFromPlaylist(row("playlist", i)?),
        Act::QueuePlay(i) => Act::QueuePlay(queued(i)?),
        Act::QueueRemove(i) => Act::QueueRemove(queued(i)?),
        Act::QueueMetadata(i) => Act::QueueMetadata(queued(i)?),
        other => other,
    })
}

fn item(label: &str, act: Act) -> Entry {
    Entry::Item { label: label.into(), act }
}

/// "Go to artist" for one credited artist; for several, "Go to <name>" for each (up to
/// four: a long credit list is the album's page's business).
fn go_to_artists(artists: &[(String, String)]) -> Vec<Entry> {
    match artists {
        [] => Vec::new(),
        [(key, _)] => vec![item("Go to artist", Act::OpenArtist(key.clone()))],
        many => many.iter().take(4).map(|(key, name)| item(&format!("Go to {name}"), Act::OpenArtist(key.clone()))).collect(),
    }
}

/// The entries of a menu.
pub fn build(kind: &str, ctx: &str, index: i32, id: i32, track: Option<TrackInfo>, album: Option<AlbumInfo>, queue: Option<(i32, Vec<(String, String)>)>) -> Vec<Entry> {
    match kind {
        "album" => {
            let a = album.unwrap_or_default();
            let mut v = vec![
                item("Play", Act::PlayAlbum(id)),
                item("Shuffle", Act::ShuffleAlbum(id)),
                item("Play next", Act::EnqueueAlbum(id, true)),
                item("Add to queue", Act::EnqueueAlbum(id, false)),
                Entry::Playlists { label: "Add to playlist".into() },
                Entry::Separator,
            ];
            v.extend(go_to_artists(&a.artists));
            // An album only the server has has no folder here to show.
            if !library::remote::is_remote(&a.folder) {
                v.push(item(crate::platform::system::words().show_in, Act::ShowInExplorer(a.folder)));
            }
            v.push(item("View metadata", Act::AlbumMetadata(id)));
            v
        }
        "track" => {
            let t = track.unwrap_or_default();
            let mut v = vec![
                item("Play", Act::PlayIn(ctx.into(), index)),
                item("Play next", Act::EnqueueIn(ctx.into(), index, true)),
                item("Add to queue", Act::EnqueueIn(ctx.into(), index, false)),
                Entry::Playlists { label: "Add to playlist".into() },
            ];
            if ctx == "playlist" {
                v.push(item("Remove from playlist", Act::RemoveFromPlaylist(index)));
            }
            v.extend([
                Entry::Separator,
                item(if t.favourite { "Remove from favourites" } else { "Add to favourites" }, Act::FavouriteIn(ctx.into(), index)),
                item(&format!("Rate\u{2026}{}", if t.rating > 0 { format!("  {}", crate::rows::stars(t.rating)) } else { String::new() }), Act::RateMenu(t.id)),
                item("Go to album", Act::OpenAlbum(t.album)),
            ]);
            v.extend(go_to_artists(&t.artists));
            // A track only the server has has no file here to show.
            if !library::remote::is_remote(&t.path) {
                v.push(item(crate::platform::system::words().show_in, Act::ShowInExplorer(t.path)));
            }
            v.push(item("View metadata", Act::Metadata(ctx.into(), index)));
            v
        }
        "queue" => {
            let (album, artists) = queue.unwrap_or_default();
            let mut v = vec![
                item("Play", Act::QueuePlay(index)),
                item("Remove from queue", Act::QueueRemove(index)),
                Entry::Separator,
                item("Go to album", Act::OpenAlbum(album)),
            ];
            v.extend(go_to_artists(&artists));
            v.push(item("View metadata", Act::QueueMetadata(index)));
            v
        }
        "playlist" if ctx == "smart" => {
            let mut v = vec![
                item("Play", Act::PlayPlaylist(i64::from(id))),
                item("Shuffle", Act::ShufflePlaylist(i64::from(id))),
                item("Add to queue", Act::EnqueuePlaylist(i64::from(id))),
                Entry::Separator,
                item("Open", Act::OpenPlaylist(id)),
                item("Edit rules\u{2026}", Act::EditSmart(i64::from(id))),
            ];
            v.extend(export_entries(i64::from(id)));
            v
        }
        "playlist" => {
            let mut v = vec![
                item("Play", Act::PlayPlaylist(i64::from(id))),
                item("Shuffle", Act::ShufflePlaylist(i64::from(id))),
                item("Add to queue", Act::EnqueuePlaylist(i64::from(id))),
                Entry::Separator,
                item("Open", Act::OpenPlaylist(id)),
            ];
            v.extend(export_entries(i64::from(id)));
            v
        }
        _ => Vec::new(),
    }
}

/// The menu of several tracks chosen in a list (`ctx`; the queue's is `selected::QUEUE`): what
/// the menu of one track has that makes sense for all of them at once. `all_favourite`: every
/// one is a favourite already. View metadata is for one track: shown, but not for several.
pub fn build_many(ctx: &str, n: usize, all_favourite: bool) -> Vec<Entry> {
    use crate::selected::{Many, QUEUE};
    let queue = ctx == QUEUE;
    let mut v = vec![Entry::Heading { label: crate::selected::tracks_word(n) }, item("Play", Act::Many(Many::Play))];
    if !queue {
        v.push(item("Play next", Act::Many(Many::Enqueue { next: true })));
        v.push(item("Add to queue", Act::Many(Many::Enqueue { next: false })));
    }
    v.push(Entry::Playlists { label: "Add to playlist".into() });
    if ctx == "playlist" {
        v.push(item("Remove from playlist", Act::Many(Many::RemoveFromPlaylist)));
    }
    if queue {
        v.push(item("Remove from queue", Act::Many(Many::QueueRemove)));
    }
    v.extend([
        Entry::Separator,
        item(if all_favourite { "Remove from favourites" } else { "Add to favourites" }, Act::Many(Many::Favourite(!all_favourite))),
        item("Rate\u{2026}", Act::Many(Many::RateMenu)),
        Entry::Disabled { label: "View metadata".into() },
    ]);
    v
}

/// The rating menu of several tracks: their rating checked when they all have it.
pub fn rating_entries_many(current: Option<u8>) -> Vec<Entry> {
    (0..=5u8)
        .map(|n| {
            let label = if n == 0 { "No rating".to_owned() } else { crate::rows::stars(n) };
            let act = Act::Many(crate::selected::Many::Rate(n));
            if current == Some(n) { Entry::Checked { label, act } } else { Entry::Item { label, act } }
        })
        .collect()
}

/// "Rate…" of the menu of several tracks: the rating menu where that menu was.
pub fn show_rating_many(st: &Rc<State>, current: Option<u8>) {
    let Some(app) = st.app.upgrade() else { return };
    let (x, y) = (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y());
    show(st, rating_entries_many(current), x, y, false);
}

/// A track's rating, as a menu of choices with the current one checked.
pub fn rating_entries(id: i64, current: u8) -> Vec<Entry> {
    (0..=5u8)
        .map(|n| {
            let label = if n == 0 { "No rating".to_owned() } else { crate::rows::stars(n) };
            let act = Act::Rate(id, n);
            if n == current { Entry::Checked { label, act } } else { Entry::Item { label, act } }
        })
        .collect()
}

/// The rating menu of the playing track (the player bar's star), opening upward from `y`.
pub fn request_rating(st: &Rc<State>, x: f32, y: f32) {
    let Some(id) = st.queue.borrow().current().map(|i| i.track.id) else { return };
    if id < 0 {
        views::flash(st, "Not in the library");
        return;
    }
    let e = rating_entries(id, crate::rows::rating_of(id));
    let h = row_y(&e, e.len()) + 6.0;
    show(st, e, x, (y - h).max(8.0), false);
}

/// Rate a track: saved, and every list and the player bar in step.
pub fn rate(st: &Rc<State>, id: i64, stars: u8) {
    if id < 0 {
        views::flash(st, "Not in the library");
        return;
    }
    if let Err(e) = query::set_rating(&st.db, id, stars) {
        log::warn!("rating: {e}");
        return;
    }
    // The lists, the player bar and the server (server/write.rs) in step.
    crate::selected::ratings_written(st, &[id], stars);
    views::flash(st, &if stars == 0 { "Rating cleared".to_owned() } else { format!("Rated {}", crate::rows::stars(stars)) });
}

/// Saving a playlist as a file: absolute paths, or relative to where it is saved.
pub fn export_entries(id: i64) -> Vec<Entry> {
    vec![
        Entry::Separator,
        item("Export as M3U8\u{2026}", Act::ExportPlaylist(id, false)),
        item("Export with relative paths\u{2026}", Act::ExportPlaylist(id, true)),
    ]
}

/// The playlist page's Export button: the two export entries, under it.
pub fn request_export(st: &Rc<State>, id: i64, x: f32, y: f32) {
    show(st, export_entries(id).into_iter().skip(1).collect(), x, y, false);
}

/// The smart playlist page's "⋯": rename, duplicate, save as an ordinary playlist, export.
pub fn smart_page_entries(id: i64) -> Vec<Entry> {
    let mut v = vec![
        item("Rename", Act::RenameSmart),
        item("Duplicate\u{2026}", Act::DuplicateSmart(id)),
        item("Save as ordinary playlist\u{2026}", Act::FreezeSmart(id)),
    ];
    v.extend(export_entries(id));
    v
}

pub fn request_smart_page(st: &Rc<State>, id: i64, x: f32, y: f32) {
    show(st, smart_page_entries(id), x, y, false);
}

/// The Albums page's sort menu, `current` checked.
pub fn sort_entries(current: AlbumSort) -> Vec<Entry> {
    AlbumSort::ALL
        .iter()
        .map(|&s| {
            let (label, act) = (s.label().to_owned(), Act::SortAlbums(s));
            if s == current { Entry::Checked { label, act } } else { Entry::Item { label, act } }
        })
        .collect()
}

fn row(label: &str, separator: bool, submenu: bool, enabled: bool) -> MenuItemData {
    MenuItemData { label: label.into(), separator, submenu, enabled, danger: false, checked: false, heading: false }
}

fn entry_height(e: &Entry) -> f32 {
    match e {
        Entry::Separator => 9.0,
        Entry::Heading { .. } => 26.0,
        _ => 34.0,
    }
}

/// Offset of entry `i` from the top of its panel (6 px padding, rows of 34, separators 9, headings 26).
pub fn row_y(entries: &[Entry], i: usize) -> f32 {
    6.0 + entries[..i].iter().map(entry_height).sum::<f32>()
}

/// A menu for what was right-clicked (`kind`: "album", "track", "queue", "playlist").
pub fn request(st: &Rc<State>, kind: &str, ctx: &str, index: i32, id: i32, x: f32, y: f32) {
    open_menu(st, kind, ctx, index, id, x, y, false);
}

/// The same, from a list's Menu key: the first entry highlighted, the list keeps the focus.
pub fn request_keys(st: &Rc<State>, kind: &str, ctx: &str, index: i32, id: i32, x: f32, y: f32) {
    open_menu(st, kind, ctx, index, id, x, y, true);
}

/// A track's credited artists (key, name); for a file from outside the library, its
/// credit's names (opened by name).
fn credited(st: &State, id: i64, shown: &str) -> Vec<(String, String)> {
    let refs = if id > 0 { query::track_artists(&st.db, id).unwrap_or_default() } else { Vec::new() };
    if refs.is_empty() {
        library::names::credits(shown).into_iter().map(|c| (c.name.clone(), c.name)).collect()
    } else {
        refs.into_iter().map(|a| (a.key, a.name)).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn open_menu(st: &Rc<State>, kind: &str, ctx: &str, index: i32, id: i32, x: f32, y: f32, from_keys: bool) {
    // A row of a selection of several: the menu is for all of them (a row outside it is chosen
    // alone first, and has its own menu).
    if kind == "track" || kind == "queue" {
        let list = if kind == "queue" { crate::selected::QUEUE } else { ctx };
        let n = crate::selected::context(st, list, index.max(0) as usize);
        if n > 1 {
            let (all_favourite, _) = crate::selected::marks_of(st, list);
            show(st, build_many(list, n, all_favourite), x, y, from_keys);
            crate::selected::menu_opened(st, list);
            return;
        }
    }
    let track = (kind == "track")
        .then(|| st.lists.by_ctx(ctx).and_then(|l| l.get(index.max(0) as usize)))
        .flatten()
        .map(|t| TrackInfo {
            id: t.id,
            rating: crate::rows::rating_of(t.id),
            album: t.album_id as i32,
            artists: credited(st, t.id, &t.artist),
            favourite: query::is_favourite_track(&st.db, t.id).unwrap_or(false),
            path: t.path.clone(),
        });
    let queue = (kind == "queue")
        .then(|| st.queue.borrow().items().get(index.max(0) as usize).map(|i| (i.track.album_id as i32, credited(st, i.track.id, &i.track.artist))))
        .flatten();
    if kind == "track" && track.is_none() || kind == "queue" && queue.is_none() {
        return;
    }
    // The album menu's "Go to artist" and "Show in Explorer" need the album's artist and folder.
    let album = (kind == "album").then(|| AlbumInfo {
        artists: query::album_artist_refs(&st.db, i64::from(id)).unwrap_or_default().into_iter().map(|a| (a.key, a.name)).collect(),
        folder: query::album_folder(&st.db, i64::from(id)).ok().flatten().unwrap_or_default(),
    });
    let ctx = if kind == "playlist" && crate::playlists::smart::is_smart(st, i64::from(id)) { "smart" } else { ctx };
    let anchor = match kind {
        "track" => track.as_ref().map(|t| Anchor::Row { ctx: ctx.to_owned(), index, track: t.id }),
        "queue" => st.queue.borrow().items().get(index.max(0) as usize).map(|q| Anchor::Queue { index, uid: q.uid }),
        _ => None,
    };
    let entries = build(kind, ctx, index, id, track, album, queue);
    show(st, entries, x, y, from_keys);
    ANCHOR.with(|a| *a.borrow_mut() = anchor);
}

/// The Albums page's sort menu, under its button.
pub fn request_sort(st: &Rc<State>, x: f32, y: f32) {
    let current = st.settings.borrow().albums_sort;
    show(st, sort_entries(current), x, y, false);
}

/// A menu of entries built elsewhere (Now playing's visualizer menu), at a window point;
/// from the keyboard, its first entry highlighted.
pub fn show_plain(st: &Rc<State>, entries: Vec<Entry>, x: f32, y: f32, from_keys: bool) {
    show(st, entries, x, y, from_keys);
}

/// A menu of entries built elsewhere (Settings > Output's devices), at a window point.
pub fn show_entries(st: &Rc<State>, entries: Vec<Entry>, x: f32, y: f32) {
    show(st, entries, x, y, false);
    if let Some(app) = st.app.upgrade() {
        app.global::<MenuStore>().set_wide(true);
    }
}

fn show(st: &Rc<State>, entries: Vec<Entry>, x: f32, y: f32, from_keys: bool) {
    let Some(app) = st.app.upgrade() else { return };
    ANCHOR.with(|a| a.borrow_mut().take());
    crate::selected::menu_closed();
    // Only ordinary playlists can be added to: a smart one's tracks come from its rules.
    let playlists: Vec<(String, i64)> = query::playlists(&st.db).unwrap_or_default().into_iter().filter(|p| !p.smart()).map(|p| (p.name, p.id)).collect();
    let rows: Vec<MenuItemData> = entries.iter().map(menu_row).collect();
    let first_sub = entries.iter().position(has_sub);
    let checks = entries.iter().any(|e| matches!(e, Entry::Checked { .. }) || matches!(e, Entry::Choices { children, .. } if children.iter().any(|c| matches!(c, Entry::Checked { .. }))));
    let open = Open { entries, playlists };
    // The first submenu's rows, until another is opened (`open_sub`).
    let sub = first_sub.map(|i| open.sub_rows(i as i32)).unwrap_or_default();
    let sub_y = first_sub.map_or(0.0, |i| row_y(&open.entries, i));
    let selectable: Vec<bool> = open.entries.iter().map(selectable).collect();
    let m = app.global::<MenuStore>();
    m.set_checks(checks);
    m.set_wide(false);
    m.set_items(ModelRc::new(VecModel::from(rows)));
    m.set_sub(ModelRc::new(VecModel::from(sub)));
    m.set_sub_row_y(sub_y);
    m.set_sub_of(-1);
    m.set_sub_highlight(-1);
    m.set_highlight(if from_keys { step(&selectable, -1, 1) } else { -1 });
    m.set_from_keys(from_keys);
    m.set_x(x);
    m.set_y(y);
    m.set_open(true);
    OPEN.with(|o| *o.borrow_mut() = Some(open));
}

fn close(app: &AppWindow) {
    app.global::<MenuStore>().set_open(false);
}

pub fn activate(st: &Rc<State>, i: i32, sub: i32) {
    let Some(app) = st.app.upgrade() else { return };
    let Some((entry, playlist)) = OPEN.with(|o| {
        let o = o.borrow();
        let o = o.as_ref()?;
        let e = o.entries.get(i.max(0) as usize)?.clone();
        // A choice of the submenu acts as an entry of its own.
        let e = match e {
            Entry::Choices { children, .. } => children.get(sub.max(0) as usize).cloned().filter(|_| sub >= 0)?,
            e => e,
        };
        let choice = if sub >= 0 { sub_choice(sub, o.playlists.len()) } else { SubChoice::None };
        let p = match choice {
            SubChoice::Playlist(j) => o.playlists.get(j).map(|p| p.1),
            _ => None,
        };
        Some((e, (choice, p)))
    }) else {
        return;
    };
    let act = match entry {
        Entry::Item { act, .. } | Entry::Checked { act, .. } => act,
        Entry::Playlists { .. } if crate::selected::menu_is_open() => {
            close(&app);
            match playlist {
                (SubChoice::Playlist(_), Some(pid)) => crate::selected::add_to_playlist(st, Some(pid)),
                (SubChoice::New, _) => crate::selected::add_to_playlist(st, None),
                _ => {}
            }
            return;
        }
        Entry::Playlists { .. } => {
            // Which list the menu was for decides what is added.
            let first = OPEN.with(|o| o.borrow().as_ref().and_then(|o| o.entries.first().cloned()));
            let first = match first {
                Some(Entry::Item { label, act }) => match reanchored(st, act) {
                    Some(act) => Some(Entry::Item { label, act }),
                    None => {
                        close(&app);
                        views::flash(st, "That track is no longer in this list");
                        return;
                    }
                },
                other => other,
            };
            let (choice, pid) = playlist;
            match (choice, pid, first) {
                (SubChoice::Playlist(_), Some(pid), Some(Entry::Item { act: Act::PlayAlbum(album), .. })) => {
                    close(&app);
                    app.global::<Actions>().invoke_add_album_to_playlist(album, pid as i32);
                }
                (SubChoice::Playlist(_), Some(pid), Some(Entry::Item { act: Act::PlayIn(ctx, index), .. })) => {
                    close(&app);
                    app.global::<Actions>().invoke_add_to_playlist(ctx.into(), index, pid as i32);
                }
                (SubChoice::New, _, Some(Entry::Item { act: Act::PlayAlbum(album), .. })) => {
                    close(&app);
                    crate::playlists::newplaylist::open_from_menu(st, crate::playlists::newplaylist::Target::Album(i64::from(album)));
                }
                (SubChoice::New, _, Some(Entry::Item { act: Act::PlayIn(ctx, index), .. })) => {
                    let track = st.lists.by_ctx(&ctx).and_then(|l| l.get(index.max(0) as usize));
                    close(&app);
                    if let Some(t) = track {
                        crate::playlists::newplaylist::open_from_menu(st, crate::playlists::newplaylist::Target::Tracks(vec![t.id]));
                    }
                }
                _ => {}
            }
            return;
        }
        Entry::Separator | Entry::Heading { .. } | Entry::Choices { .. } | Entry::Disabled { .. } => return,
    };
    close(&app);
    let Some(act) = reanchored(st, act) else {
        views::flash(st, "That track is no longer in this list");
        return;
    };
    let a = app.global::<Actions>();
    match act {
        Act::PlayAlbum(id) => a.invoke_play_album(id),
        Act::ShuffleAlbum(id) => a.invoke_shuffle_album(id),
        Act::EnqueueAlbum(id, next) => a.invoke_enqueue_album(id, next),
        Act::OpenArtist(name) => a.invoke_open_artist(name.into()),
        Act::OpenAlbum(id) => a.invoke_open_album(id),
        Act::PlayIn(ctx, i) => a.invoke_play_in(ctx.into(), i),
        Act::EnqueueIn(ctx, i, next) => a.invoke_enqueue_in(ctx.into(), i, next),
        Act::FavouriteIn(ctx, i) => a.invoke_favourite_in(ctx.into(), i),
        Act::RemoveFromPlaylist(i) => a.invoke_playlist_remove(i),
        Act::QueuePlay(i) => a.invoke_queue_play(i),
        Act::QueueRemove(i) => a.invoke_queue_remove(i),
        Act::PlayPlaylist(id) if crate::playlists::smart::is_smart(st, id) => crate::playlists::smart::act(st, id, crate::playlists::smart::Act::Play { shuffle: false }),
        Act::ShufflePlaylist(id) if crate::playlists::smart::is_smart(st, id) => crate::playlists::smart::act(st, id, crate::playlists::smart::Act::Play { shuffle: true }),
        Act::EnqueuePlaylist(id) if crate::playlists::smart::is_smart(st, id) => crate::playlists::smart::act(st, id, crate::playlists::smart::Act::Enqueue),
        Act::EditSmart(id) => crate::playlists::smart::open_edit(st, id),
        Act::RenameSmart => crate::playlists::smart::start_rename(st),
        Act::DuplicateSmart(id) => crate::playlists::smart::duplicate(st, Some(id)),
        Act::FreezeSmart(id) => crate::playlists::smart::freeze(st, id),
        Act::PlayPlaylist(id) => {
            let tracks = query::playlist_tracks(&st.db, id).unwrap_or_default();
            playback::play_tracks(st, tracks, 0);
        }
        Act::ShufflePlaylist(id) => {
            let tracks = query::playlist_tracks(&st.db, id).unwrap_or_default();
            playback::shuffle_tracks(st, tracks);
        }
        Act::EnqueuePlaylist(id) => {
            let tracks = query::playlist_tracks(&st.db, id).unwrap_or_default();
            playback::enqueue(st, tracks, false);
        }
        Act::OpenPlaylist(id) => views::open_playlist(st, i64::from(id)),
        Act::ShowInExplorer(path) => {
            if path.is_empty() || !crate::explorer::show(&path) {
                views::flash(st, "Not found on disk");
            }
        }
        Act::SortAlbums(sort) => views::sort_grid(st, sort),
        Act::SetOutput(id) => crate::output::choose(st, id),
        Act::ExportPlaylist(id, relative) => crate::playlists::file::export_dialog(st, id, relative),
        Act::RateMenu(id) => {
            let (x, y) = (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y());
            show(st, rating_entries(id, crate::rows::rating_of(id)), x, y, false);
        }
        Act::Rate(id, n) => rate(st, id, n),
        Act::Metadata(ctx, i) => {
            if let Some(t) = st.lists.by_ctx(&ctx).and_then(|l| l.get(i.max(0) as usize)) {
                crate::metadata::view::open_track(st, t);
            }
        }
        Act::QueueMetadata(i) => {
            let t = st.queue.borrow().items().get(i.max(0) as usize).map(|i| i.track.clone());
            if let Some(t) = t {
                crate::metadata::view::open_track(st, t);
            }
        }
        Act::AlbumMetadata(id) => crate::metadata::view::open_album(st, i64::from(id)),
        Act::EqPreset(name) => crate::eq::choose(st, &name),
        Act::EqSave => crate::eq::save_prompt(st),
        Act::EqRename => crate::eq::rename_prompt(st),
        Act::EqDelete => crate::eq::delete(st),
        Act::EqImport => crate::eq::import_dialog(st),
        Act::EqExport(as_) => crate::eq::export_dialog(st, as_),
        Act::EqCopyAs(as_) => crate::eq::copy(st, as_),
        Act::EqBandKind(i, k) => crate::eq::set_kind(st, i, k as i32),
        Act::VizToggle => {
            crate::toggle_visualizer(st);
            views::flash(st, if st.settings.borrow().visualizer { "Visualizer on" } else { "Visualizer off" });
        }
        Act::VizPreset(i) => {
            crate::visualizer::choose_preset(st, i);
            if !st.settings.borrow().visualizer {
                crate::toggle_visualizer(st);
            }
        }
        Act::VizPlacement(i) => {
            crate::visualizer::choose(st, "placement", i);
            if !st.settings.borrow().visualizer {
                crate::toggle_visualizer(st);
            }
        }
        Act::VizSettings => views::settings_section(st, crate::settings::nav::VISUALIZER),
        Act::VizOpacity(p) => {
            crate::visualizer::set_opacity(st, p);
            if !st.settings.borrow().visualizer {
                crate::toggle_visualizer(st);
            }
        }
        Act::VizSave => crate::visualizer::presets::save_prompt(st),
        Act::VizRename => crate::visualizer::presets::rename_prompt(st),
        Act::VizUpdate => crate::visualizer::presets::update(st),
        Act::VizDelete => crate::visualizer::presets::delete(st),
        Act::VizCopy => crate::visualizer::presets::copy(st),
        Act::VizPaste => crate::visualizer::presets::paste(st),
        Act::VizExport => crate::visualizer::presets::export_dialog(st),
        Act::VizImport => crate::visualizer::presets::import_dialog(st),
        Act::LyricsCopy => crate::lyrics::save::copy(st),
        Act::LyricsSaveFile => crate::lyrics::save::save_file(st, false),
        Act::LyricsSaveTags => crate::lyrics::save::ask_tags(st),
        Act::LyricsResetOffset => crate::lyrics::save::reset_offset(st),
        Act::Many(what) => crate::selected::act(st, what),
    }
}

/// The next selectable row from `from` in direction `dir` (+1/-1), wrapping; `selectable`
/// says which rows can be highlighted. -1 when none can.
pub fn step(selectable: &[bool], from: i32, dir: i32) -> i32 {
    let n = selectable.len() as i32;
    if n == 0 || !selectable.iter().any(|s| *s) {
        return -1;
    }
    let mut i = if from < 0 { if dir > 0 { -1 } else { n } } else { from };
    loop {
        i = (i + dir).rem_euclid(n);
        if selectable[i as usize] {
            return i;
        }
    }
}

pub fn key(st: &Rc<State>, key: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let m = app.global::<MenuStore>();
    let Some(main) = OPEN.with(|o| o.borrow().as_ref().map(|o| o.entries.iter().map(selectable).collect::<Vec<_>>())) else {
        return;
    };
    let sub_ok = |i: i32| OPEN.with(|o| o.borrow().as_ref().map(|o| o.sub_selectable(i)).unwrap_or_default());
    let in_sub = m.get_sub_of() >= 0;
    match key {
        "escape" if in_sub => m.set_sub_of(-1),
        "escape" => close(&app),
        "down" | "up" => {
            let dir = if key == "down" { 1 } else { -1 };
            if in_sub {
                m.set_sub_highlight(step(&sub_ok(m.get_sub_of()), m.get_sub_highlight(), dir));
            } else {
                m.set_highlight(step(&main, m.get_highlight(), dir));
            }
        }
        "right" if !in_sub => {
            let h = m.get_highlight();
            let is_sub = OPEN.with(|o| o.borrow().as_ref().and_then(|o| o.entries.get(h.max(0) as usize).cloned()))
                .is_some_and(|e| has_sub(&e));
            if h >= 0 && is_sub {
                open_sub(st, h);
                m.set_sub_highlight(step(&sub_ok(h), -1, 1));
            }
        }
        "left" if in_sub => m.set_sub_of(-1),
        "enter" => {
            if in_sub {
                let j = m.get_sub_highlight();
                if j >= 0 {
                    activate(st, m.get_sub_of(), j);
                }
            } else {
                let h = m.get_highlight();
                if h < 0 {
                    return;
                }
                let is_sub = OPEN.with(|o| o.borrow().as_ref().and_then(|o| o.entries.get(h as usize).cloned()))
                    .is_some_and(|e| has_sub(&e));
                if is_sub {
                    open_sub(st, h);
                    m.set_sub_highlight(step(&sub_ok(h), -1, 1));
                } else {
                    activate(st, h, -1);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Show in Explorer" on Windows (the platform's words).
    const SHOW: &str = crate::platform::system::WORDS.show_in;

    fn labels(v: Vec<Entry>) -> Vec<String> {
        v.iter()
            .map(|e| match e {
                Entry::Item { label, .. } | Entry::Playlists { label } | Entry::Choices { label, .. } => label.clone(),
                Entry::Checked { label, .. } => format!("✓{label}"),
                Entry::Separator => "-".into(),
                Entry::Heading { label } => format!("[{label}]"),
                Entry::Disabled { label } => format!("({label})"),
            })
            .collect()
    }

    fn track() -> Option<TrackInfo> {
        Some(TrackInfo { id: 1, rating: 0, album: 7, artists: vec![("n:a".into(), "A".into())], favourite: true, path: "M:/a/01.flac".into() })
    }

    #[test]
    fn menus_keep_the_old_entries_in_order() {
        assert_eq!(
            labels(build("album", "", 0, 7, None, Some(AlbumInfo { artists: vec![("n:b".into(), "B".into())], folder: "M:/b".into() }), None)),
            ["Play", "Shuffle", "Play next", "Add to queue", "Add to playlist", "-", "Go to artist", SHOW, "View metadata"]
        );
        assert_eq!(
            labels(build("track", "album", 2, 0, track(), None, None)),
            ["Play", "Play next", "Add to queue", "Add to playlist", "-", "Remove from favourites", "Rate\u{2026}", "Go to album", "Go to artist", SHOW, "View metadata"]
        );
        assert_eq!(labels(rating_entries(3, 2)), ["No rating", "★☆☆☆☆", "✓★★☆☆☆", "★★★☆☆", "★★★★☆", "★★★★★"]);
        assert_eq!(labels(build("queue", "", 3, 0, None, None, Some((7, vec![("n:a".into(), "A".into())])))), ["Play", "Remove from queue", "-", "Go to album", "Go to artist", "View metadata"]);
        assert_eq!(
            labels(build("playlist", "", 0, 5, None, None, None)),
            ["Play", "Shuffle", "Add to queue", "-", "Open", "-", "Export as M3U8\u{2026}", "Export with relative paths\u{2026}"]
        );
    }

    #[test]
    fn several_artists_each_have_their_entry() {
        let two = TrackInfo { artists: vec![("n:a".into(), "A".into()), ("mb:b".into(), "B".into())], ..track().unwrap() };
        let e = build("track", "album", 0, 0, Some(two), None, None);
        assert!(e.contains(&Entry::Item { label: "Go to A".into(), act: Act::OpenArtist("n:a".into()) }));
        assert!(e.contains(&Entry::Item { label: "Go to B".into(), act: Act::OpenArtist("mb:b".into()) }));
        assert!(!labels(e).iter().any(|l| l == "Go to artist"));
    }

    #[test]
    fn a_playlist_track_can_be_removed_from_it() {
        let e = build("track", "playlist", 4, 0, track(), None, None);
        assert_eq!(
            labels(e.clone()),
            ["Play", "Play next", "Add to queue", "Add to playlist", "Remove from playlist", "-", "Remove from favourites", "Rate\u{2026}", "Go to album", "Go to artist", SHOW, "View metadata"]
        );
        assert!(e.contains(&Entry::Item { label: "Remove from playlist".into(), act: Act::RemoveFromPlaylist(4) }));
        // Only there: a playlist row elsewhere (search results) has no such entry.
        assert!(!labels(build("track", "search", 4, 0, track(), None, None)).iter().any(|l| l == "Remove from playlist"));
    }

    #[test]
    fn a_server_only_track_or_album_has_nothing_to_show_in_explorer() {
        let t = build("track", "album", 0, 0, Some(TrackInfo { path: library::remote::song_path("s1"), ..Default::default() }), None, None);
        assert!(!labels(t.clone()).iter().any(|l| l == SHOW), "{:?}", labels(t.clone()));
        assert_eq!(t.last(), Some(&Entry::Item { label: "View metadata".into(), act: Act::Metadata("album".into(), 0) }));
        let a = build("album", "", 0, 3, None, Some(AlbumInfo { artists: vec![], folder: library::remote::album_folder("al1") }), None);
        assert!(!labels(a.clone()).iter().any(|l| l == SHOW));
        assert_eq!(a.last(), Some(&Entry::Item { label: "View metadata".into(), act: Act::AlbumMetadata(3) }));
    }

    #[test]
    fn show_in_explorer_names_the_file_or_the_album_folder() {
        let t = build("track", "album", 0, 0, track(), None, None);
        assert!(t.contains(&Entry::Item { label: SHOW.into(), act: Act::ShowInExplorer("M:/a/01.flac".into()) }));
        assert_eq!(t.last(), Some(&Entry::Item { label: "View metadata".into(), act: Act::Metadata("album".into(), 0) }));
        let a = build("album", "", 0, 3, None, Some(AlbumInfo { artists: vec![("n:b".into(), "B".into())], folder: "M:/b".into() }), None);
        assert!(a.contains(&Entry::Item { label: SHOW.into(), act: Act::ShowInExplorer("M:/b".into()) }));
        assert_eq!(a.last(), Some(&Entry::Item { label: "View metadata".into(), act: Act::AlbumMetadata(3) }));
        assert!(a.contains(&Entry::Item { label: "Go to artist".into(), act: Act::OpenArtist("n:b".into()) }));
    }

    #[test]
    fn the_playlist_submenu_starts_with_new_playlist() {
        let none: Vec<(String, i64)> = Vec::new();
        assert_eq!(sub_labels(&none), [Some("New playlist\u{2026}".to_owned())], "no playlists yet: still there, alone");
        assert_eq!(sub_choice(0, 0), SubChoice::New);
        assert_eq!(sub_choice(1, 0), SubChoice::None);
        let two = vec![("Mix".to_owned(), 5), ("Road".to_owned(), 9)];
        assert_eq!(sub_labels(&two), [Some("New playlist\u{2026}".to_owned()), None, Some("Mix".to_owned()), Some("Road".to_owned())]);
        assert_eq!(sub_choice(0, 2), SubChoice::New);
        assert_eq!(sub_choice(1, 2), SubChoice::None, "the separator");
        assert_eq!(sub_choice(2, 2), SubChoice::Playlist(0));
        assert_eq!(sub_choice(3, 2), SubChoice::Playlist(1));
        assert_eq!(sub_choice(4, 2), SubChoice::None);
        // The keys step over the separator.
        let sel: Vec<bool> = sub_labels(&two).iter().map(Option::is_some).collect();
        assert_eq!(step(&sel, 0, 1), 2);
        assert_eq!(step(&sel, 2, -1), 0);
    }

    #[test]
    fn the_sort_menu_checks_the_current_order() {
        assert_eq!(labels(sort_entries(AlbumSort::Added)), ["Artist", "Title", "Release date", "✓Date added", "Most played"]);
    }

    #[test]
    fn keyboard_skips_separators_and_wraps() {
        let sel = [true, true, false, true];
        assert_eq!(step(&sel, -1, 1), 0);
        assert_eq!(step(&sel, 1, 1), 3, "skips the separator");
        assert_eq!(step(&sel, 3, 1), 0, "wraps");
        assert_eq!(step(&sel, -1, -1), 3);
        assert_eq!(step(&[false, false], -1, 1), -1);
    }

    #[test]
    fn submenu_sits_beside_its_row() {
        let e = build("album", "", 0, 1, None, None, None);
        assert_eq!(row_y(&e, 4), 6.0 + 4.0 * 34.0);
        assert_eq!(row_y(&e, 6), 6.0 + 5.0 * 34.0 + 9.0);
    }

    #[test]
    fn several_tracks_have_a_menu_of_their_own() {
        assert_eq!(
            labels(build_many("album", 12, false)),
            ["[12 tracks]", "Play", "Play next", "Add to queue", "Add to playlist", "-", "Add to favourites", "Rate\u{2026}", "(View metadata)"]
        );
        assert_eq!(
            labels(build_many("playlist", 3, true)),
            ["[3 tracks]", "Play", "Play next", "Add to queue", "Add to playlist", "Remove from playlist", "-", "Remove from favourites", "Rate\u{2026}", "(View metadata)"]
        );
        assert_eq!(
            labels(build_many(crate::selected::QUEUE, 2, false)),
            ["[2 tracks]", "Play", "Add to playlist", "Remove from queue", "-", "Add to favourites", "Rate\u{2026}", "(View metadata)"]
        );
        let e = build_many("playlist", 3, false);
        let sel: Vec<bool> = e.iter().map(selectable).collect();
        assert_eq!(step(&sel, -1, 1), 1, "the keys start on Play, past the heading");
        assert!(!sel[e.len() - 1], "View metadata cannot be chosen for several");
        assert!(menu_row(&e[5]).danger);
        assert_eq!(labels(rating_entries_many(Some(3))), ["No rating", "★☆☆☆☆", "★★☆☆☆", "✓★★★☆☆", "★★★★☆", "★★★★★"]);
        assert!(!labels(rating_entries_many(None)).iter().any(|l| l.starts_with('✓')));
    }
}
