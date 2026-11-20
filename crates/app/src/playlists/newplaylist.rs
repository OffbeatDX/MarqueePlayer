//! "New playlist…": the first entry of every "Add to playlist" submenu (track and album
//! menus, `menus.rs`), and a drop on the sidebar's Playlists heading. It asks for a name in
//! a small prompt where the menu was (dialogs.slint, `Dialog`), pre-filled with the
//! album's title when it came from an album (empty, with a placeholder, from a track),
//! then creates the playlist with what was right-clicked in one step and says so in the
//! navigation pill. Enter confirms, Esc (or a click outside) cancels.
//!
//! An empty name is refused in the prompt. A name a playlist already has (letter case
//! and spaces aside) is not made twice: the prompt says so and offers to add to that one
//! instead (its button, or Enter again); editing the name goes back to creating.

use std::cell::RefCell;
use std::rc::Rc;

use library::query;
use slint::ComponentHandle;

use crate::state::State;
use crate::{Dialog, MenuStore, views};

/// What the new playlist gets.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// These tracks (by id).
    Tracks(Vec<i64>),
    /// An album's tracks.
    Album(i64),
}

/// A name, checked against the playlists there are.
#[derive(Clone, Debug, PartialEq)]
pub enum NameCheck {
    /// Nothing but spaces.
    Empty,
    /// A playlist has this name already: (its id, its name as it is).
    Exists(i64, String),
    /// A new name, trimmed.
    New(String),
}

/// Names compare without letter case and with runs of spaces as one ("My  mix" is "my mix").
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    norm(a) == norm(b)
}

pub fn check_name(input: &str, playlists: &[(i64, String)]) -> NameCheck {
    let name = input.trim();
    if name.is_empty() {
        return NameCheck::Empty;
    }
    match playlists.iter().find(|(_, n)| same_name(n, name)) {
        Some((id, n)) => NameCheck::Exists(*id, n.clone()),
        None => NameCheck::New(name.to_owned()),
    }
}

/// "Added 12 tracks to “Name”", the pill's note after adding to a playlist.
pub fn added_note(n: usize, name: &str) -> String {
    format!("Added {} to \u{201c}{name}\u{201d}", if n == 1 { "1 track".to_owned() } else { format!("{n} tracks") })
}

struct Pending {
    target: Target,
    /// The name matched a playlist that exists: adding to it is offered.
    existing: Option<(i64, String)>,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Ask for a name for a new playlist of `target`, at a window point.
pub fn open(st: &Rc<State>, target: Target, prefill: &str, x: f32, y: f32) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    if d.get_open() {
        return;
    }
    PENDING.with(|p| *p.borrow_mut() = Some(Pending { target, existing: None }));
    d.set_prompt_title("New playlist".into());
    d.set_prompt_text(prefill.into());
    d.set_prompt_note("".into());
    d.set_prompt_confirm("Create".into());
    d.set_x(x);
    d.set_y(y);
    d.set_returns(false);
    d.set_prompt_open(true);
}

/// From a menu's "New playlist…": where the menu was.
pub fn open_from_menu(st: &Rc<State>, target: Target) {
    let Some(app) = st.app.upgrade() else { return };
    let (x, y) = (app.global::<MenuStore>().get_x(), app.global::<MenuStore>().get_y());
    let prefill = match &target {
        Target::Album(id) => query::album(&st.db, *id).ok().flatten().map(|a| a.title).unwrap_or_default(),
        Target::Tracks(_) => String::new(),
    };
    open(st, target, &prefill, x, y);
}

/// Albums, tracks or a queue row dropped on the sidebar's Playlists heading.
pub fn open_from_drop(st: &Rc<State>, payload: crate::dnd::Payload, x: f32, y: f32) {
    use crate::dnd::Payload;
    match payload {
        Payload::Album(id) => {
            let prefill = query::album(&st.db, id).ok().flatten().map(|a| a.title).unwrap_or_default();
            open(st, Target::Album(id), &prefill, x, y);
        }
        // A dragged row carries the rows chosen with it (selected.rs).
        Payload::Track { ctx, index } => {
            let ids: Vec<i64> = crate::selected::dragged(st, &ctx, index).iter().map(|t| t.id).collect();
            if !ids.is_empty() {
                open(st, Target::Tracks(ids), "", x, y);
            }
        }
        Payload::QueueRow(i) => {
            let tracks = crate::selected::dragged(st, crate::selected::QUEUE, i);
            let ids: Vec<i64> = tracks.iter().map(|t| t.id).filter(|id| *id > 0).collect();
            match (tracks.is_empty(), ids.is_empty()) {
                (true, _) => {}
                (false, true) => views::flash(st, "Not in the library"),
                (false, false) => open(st, Target::Tracks(ids), "", x, y),
            }
        }
    }
}

/// The name was edited: a duplicate offer no longer applies, nor an "empty" note.
pub fn edited(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    let was_offer = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let Some(p) = p.as_mut() else { return false };
        match &p.existing {
            Some((_, name)) if !same_name(name, text) => {
                p.existing = None;
                true
            }
            _ => false,
        }
    });
    if was_offer || (!text.trim().is_empty() && d.get_prompt_note() != "") {
        d.set_prompt_note("".into());
        d.set_prompt_confirm("Create".into());
    }
}

/// Enter or the confirming button.
pub fn confirm(st: &Rc<State>, text: &str) {
    let Some(app) = st.app.upgrade() else { return };
    let d = app.global::<Dialog>();
    let Some((target, existing)) = PENDING.with(|p| p.borrow().as_ref().map(|p| (p.target.clone(), p.existing.clone()))) else {
        close(st);
        return;
    };
    let ids: Vec<i64> = match &target {
        Target::Tracks(ids) => ids.clone(),
        Target::Album(id) => query::album_tracks(&st.db, *id).unwrap_or_default().iter().map(|t| t.id).collect(),
    };
    // The offer to add to the playlist of that name stands while the name is unchanged.
    if let Some((id, name)) = existing.filter(|(_, n)| same_name(n, text)) {
        close(st);
        add_to_existing(st, id, &name, &ids);
        return;
    }
    let all = query::playlists(&st.db).unwrap_or_default();
    let playlists: Vec<(i64, String)> = all.iter().map(|p| (p.id, p.name.clone())).collect();
    match check_name(text, &playlists) {
        // A smart playlist of that name: it cannot be added to (its tracks come from its rules).
        NameCheck::Exists(id, name) if all.iter().any(|p| p.id == id && p.smart()) => {
            d.set_prompt_note(format!("A smart playlist is called \u{201c}{name}\u{201d}. Type another name.").into());
            d.set_prompt_confirm("Create".into());
        }
        NameCheck::Empty => {
            d.set_prompt_note("Type a name for the playlist.".into());
            d.set_prompt_confirm("Create".into());
        }
        NameCheck::Exists(id, name) => {
            PENDING.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.existing = Some((id, name.clone()));
                }
            });
            d.set_prompt_note(format!("There is already a playlist called \u{201c}{name}\u{201d}. Add to it, or type another name.").into());
            d.set_prompt_confirm("Add to existing".into());
        }
        NameCheck::New(name) => {
            close(st);
            match query::create_playlist_with(&st.db, &name, &ids) {
                Ok(id) => {
                    views::playlists_changed(st);
                    views::flash(st, &added_note(ids.len(), &name));
                    log::info!("new playlist {name:?} ({id}) with {} tracks", ids.len());
                }
                Err(e) => {
                    log::warn!("creating playlist {name:?}: {e}");
                    views::flash(st, "Could not create the playlist");
                }
            }
        }
    }
}

fn add_to_existing(st: &Rc<State>, id: i64, name: &str, ids: &[i64]) {
    if let Err(e) = query::add_to_playlist(&st.db, id, ids) {
        log::warn!("adding to playlist {id}: {e}");
        views::flash(st, "Could not add to the playlist");
        return;
    }
    views::playlists_changed(st);
    if st.open_playlist.get() == Some(id) && st.history.borrow().current == crate::nav::Route::Playlist(id) {
        views::reload_playlist(st, id);
    }
    views::flash(st, &added_note(ids.len(), name));
}

/// Esc, Cancel or a click outside: nothing is made.
pub fn cancel(st: &Rc<State>) {
    close(st);
}

fn close(st: &Rc<State>) {
    PENDING.with(|p| *p.borrow_mut() = None);
    if let Some(app) = st.app.upgrade() {
        app.global::<Dialog>().set_prompt_open(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lists() -> Vec<(i64, String)> {
        vec![(1, "Road Trip".into()), (2, "Chill  Mix".into())]
    }

    #[test]
    fn empty_names_are_refused() {
        assert_eq!(check_name("", &lists()), NameCheck::Empty);
        assert_eq!(check_name("   \t ", &lists()), NameCheck::Empty);
    }

    #[test]
    fn a_name_in_use_is_found_whatever_its_case_and_spacing() {
        assert_eq!(check_name("Road Trip", &lists()), NameCheck::Exists(1, "Road Trip".into()));
        assert_eq!(check_name("  road   trip ", &lists()), NameCheck::Exists(1, "Road Trip".into()));
        assert_eq!(check_name("chill mix", &lists()), NameCheck::Exists(2, "Chill  Mix".into()));
    }

    #[test]
    fn new_names_are_trimmed() {
        assert_eq!(check_name("  Night Drive ", &lists()), NameCheck::New("Night Drive".into()));
        assert_eq!(check_name("Road Trip 2", &lists()), NameCheck::New("Road Trip 2".into()));
        // Accents make another name.
        assert_eq!(check_name("Café", &[(1, "Cafe".into())]), NameCheck::New("Café".into()));
        assert_eq!(check_name("x", &[]), NameCheck::New("x".into()));
    }

    #[test]
    fn the_note_counts_tracks() {
        assert_eq!(added_note(1, "Mix"), "Added 1 track to \u{201c}Mix\u{201d}");
        assert_eq!(added_note(12, "Mix"), "Added 12 tracks to \u{201c}Mix\u{201d}");
    }
}
