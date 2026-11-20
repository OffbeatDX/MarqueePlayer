//! A playlist name that another playlist has already: never settled silently. The window's
//! clash prompt (dialogs.slint, `ClashPrompt`; `Dialog.clash-*`) asks, saying how many tracks
//! each has: "Replace" (only where something can be replaced: an imported file over an
//! ordinary playlist), "Keep both" (the newcomer takes "Name (2)", the next free number) or
//! "Cancel". A batch (several playlist files) offers "do the same for the rest" while more
//! clashes follow. Used by playlist import (`playlistfile`), renaming a playlist (its page,
//! ordinary or smart) and the Playlists page's Create.
//!
//! Names compare as the name prompt compares them (`playlists::newplaylist::same_name`): letter case
//! and runs of spaces aside.

use std::cell::RefCell;
use std::rc::Rc;

use library::query::{self, Playlist};
use slint::ComponentHandle;

use crate::Dialog;
use crate::state::State;
use crate::views;

/// The answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Cancel,
    Replace,
    KeepBoth,
}

impl Choice {
    /// The prompt's buttons: 0 Cancel, 1 Replace, 2 Keep both.
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Choice::Replace,
            2 => Choice::KeepBoth,
            _ => Choice::Cancel,
        }
    }
}

/// What the prompt says.
pub struct Ask {
    pub title: String,
    /// How many tracks each has.
    pub text: String,
    /// What the choices would do.
    pub note: String,
    pub can_replace: bool,
    /// Clashes still to come in the same batch: "do the same for the rest" is offered when
    /// there are any.
    pub rest: usize,
}

/// Called with the answer and whether it stands for the rest of the batch.
pub type Then = Box<dyn FnOnce(&Rc<State>, Choice, bool)>;

thread_local! {
    static PENDING: RefCell<Option<Then>> = RefCell::new(None);
}

/// "1 track", "12 tracks".
pub fn tracks(n: usize) -> String {
    if n == 1 { "1 track".to_owned() } else { format!("{n} tracks") }
}

/// “Name”, in the app's quotes.
pub fn q(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// The playlist (other than `except`) called `name`; an ordinary one first when several
/// share it (only an ordinary one can be replaced).
pub fn find<'a>(name: &str, all: &'a [Playlist], except: Option<i64>) -> Option<&'a Playlist> {
    let mut same = all.iter().filter(|p| Some(p.id) != except && crate::playlists::newplaylist::same_name(&p.name, name));
    let first = same.clone().next()?;
    Some(same.find(|p| !p.smart()).unwrap_or(first))
}

/// "Name (2)", or the next number no playlist in `taken` has ("Name (3)"...).
pub fn numbered<S: AsRef<str>>(name: &str, taken: &[S]) -> String {
    let name = name.trim();
    let free = |c: &str| !taken.iter().any(|t| crate::playlists::newplaylist::same_name(t.as_ref(), c));
    (2..).map(|n| format!("{name} ({n})")).find(|c| free(c)).unwrap_or_else(|| name.to_owned())
}

/// How many of `later` (names still to come in a batch) will meet a name already taken: by
/// a playlist in `names`, by the one being asked about (`current`), or by an earlier one of
/// them.
pub fn later_clashes<S: AsRef<str>>(current: &str, later: &[String], names: &[S]) -> usize {
    let mut seen: Vec<&str> = vec![current];
    let mut n = 0;
    for l in later {
        if seen.iter().any(|s| crate::playlists::newplaylist::same_name(s, l)) || names.iter().any(|s| crate::playlists::newplaylist::same_name(s.as_ref(), l)) {
            n += 1;
        }
        seen.push(l);
    }
    n
}

/// What playlist `p` holds, for the prompt: "“Mix” has 24 tracks", or for a smart one what
/// it held when last counted.
pub fn holds(p: &Playlist) -> String {
    if p.smart() {
        match crate::playlists::smart::known_count(p.id) {
            Some(n) => format!("{} is a smart playlist ({} now)", q(&p.name), tracks(n)),
            None => format!("{} is a smart playlist", q(&p.name)),
        }
    } else {
        format!("{} has {}", q(&p.name), tracks(p.tracks.max(0) as usize))
    }
}

/// Ask; `then` gets the answer. One prompt at a time: one asked while another waits answers
/// the waiting one with Cancel first (and any it asks then).
pub fn ask(st: &Rc<State>, a: Ask, then: Then) {
    let Some(app) = st.app.upgrade() else {
        then(st, Choice::Cancel, false);
        return;
    };
    // Until none waits: a Cancel may ask again (a batch's next clash), and that one is
    // answered too rather than dropped unanswered below.
    while let Some(old) = PENDING.with(|p| p.borrow_mut().take()) {
        old(st, Choice::Cancel, false);
    }
    PENDING.with(|p| *p.borrow_mut() = Some(then));
    let d = app.global::<Dialog>();
    d.set_clash_title(a.title.into());
    d.set_clash_text(a.text.into());
    d.set_clash_note(a.note.into());
    d.set_clash_can_replace(a.can_replace);
    d.set_clash_rest(a.rest as i32);
    d.set_clash_same(false);
    if !d.get_clash_open() {
        d.set_returns(false);
    }
    d.set_clash_shown(d.get_clash_shown() + 1);
    d.set_clash_open(true);
    log::info!("name clash asked (replace offered: {}, {} more to come)", a.can_replace, a.rest);
}

/// A button, Esc or a click outside: the waiting question is answered.
pub fn choose(st: &Rc<State>, choice: Choice, for_rest: bool) {
    let then = PENDING.with(|p| p.borrow_mut().take());
    // Closed first: what follows may ask again (the next clash of a batch).
    if let Some(app) = st.app.upgrade() {
        app.global::<Dialog>().set_clash_open(false);
    }
    log::info!("name clash answered: {choice:?}{}", if for_rest { ", for the rest too" } else { "" });
    if let Some(then) = then {
        then(st, choice, for_rest);
    }
}

/// Whether a question is waiting (the bench checks it).
#[cfg(feature = "bench")]
pub fn waiting() -> bool {
    PENDING.with(|p| p.borrow().is_some())
}

/// Rename playlist `id` (ordinary or smart) to `name`: an empty name leaves it as it is;
/// another playlist's name asks (Keep both names this one "Name (2)").
pub fn rename(st: &Rc<State>, id: i64, name: &str) {
    let name = name.trim().to_owned();
    if name.is_empty() {
        views::reload_playlist(st, id);
        return;
    }
    let all = query::playlists(&st.db).unwrap_or_default();
    let Some(other) = find(&name, &all, Some(id)) else {
        return renamed(st, id, &name);
    };
    let taken: Vec<&str> = all.iter().filter(|p| p.id != id).map(|p| p.name.as_str()).collect();
    let alt = numbered(&name, &taken);
    let this = all.iter().find(|p| p.id == id);
    let text = match this {
        Some(p) => format!("{}. This one ({}) {}.", holds(other), q(&p.name), if p.smart() { "is a smart playlist".to_owned() } else { format!("has {}", tracks(p.tracks.max(0) as usize)) }),
        None => format!("{}.", holds(other)),
    };
    let a = Ask {
        title: format!("Another playlist is called {}", q(&other.name)),
        text,
        note: format!("Keep both names this one {}. Cancel leaves its name as it was.", q(&alt)),
        can_replace: false,
        rest: 0,
    };
    ask(
        st,
        a,
        Box::new(move |st, choice, _| match choice {
            Choice::KeepBoth => renamed(st, id, &alt),
            _ => views::reload_playlist(st, id),
        }),
    );
}

fn renamed(st: &Rc<State>, id: i64, name: &str) {
    // Not renamed: the page is read again, with the name it has.
    if let Err(e) = query::rename_playlist(&st.db, id, name) {
        views::not_written(st, "Could not rename the playlist", &e);
    }
    views::playlists_changed(st);
    views::reload_playlist(st, id);
}

/// The Playlists page's Create: a new, empty playlist; another playlist's name asks (Keep
/// both makes "Name (2)").
pub fn create(st: &Rc<State>, name: &str) {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return;
    }
    let all = query::playlists(&st.db).unwrap_or_default();
    let Some(other) = find(&name, &all, None) else {
        return created(st, &name);
    };
    let alt = numbered(&name, &all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
    let a = Ask {
        title: format!("There is already a playlist called {}", q(&other.name)),
        text: format!("{}. The new one would start empty.", holds(other)),
        note: format!("Keep both makes the new one {}.", q(&alt)),
        can_replace: false,
        rest: 0,
    };
    ask(
        st,
        a,
        Box::new(move |st, choice, _| {
            if choice == Choice::KeepBoth {
                created(st, &alt);
            }
        }),
    );
}

fn created(st: &Rc<State>, name: &str) {
    if let Err(e) = query::create_playlist(&st.db, name) {
        return views::not_written(st, "Could not create the playlist", &e);
    }
    views::playlists_changed(st);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(id: i64, name: &str, smart: bool) -> Playlist {
        Playlist { id, name: name.into(), tracks: 3, rules: smart.then(|| "{}".into()) }
    }

    #[test]
    fn keep_both_takes_the_next_free_number() {
        assert_eq!(numbered("Mix", &["Mix"]), "Mix (2)");
        assert_eq!(numbered("Mix", &["Mix", "mix (2)", "Mix  (3)"]), "Mix (4)", "case and spaces aside, as names compare");
        assert_eq!(numbered("Mix", &["Mix", "Mix (3)"]), "Mix (2)", "the first free one, not after the highest");
        assert_eq!(numbered("  Mix ", &["Mix"]), "Mix (2)", "trimmed");
        assert_eq!(numbered("Mix (2)", &["Mix (2)"]), "Mix (2) (2)", "a number in the name is the name's");
        assert_eq!(numbered("Live (1999)", &["live (1999)"]), "Live (1999) (2)");
        let none: [&str; 0] = [];
        assert_eq!(numbered("Mix", &none), "Mix (2)", "only asked for when the name is taken");
    }

    #[test]
    fn a_clash_is_found_whatever_the_case_and_spacing_and_an_ordinary_one_first() {
        let all = [pl(1, "Road Trip", true), pl(2, "road  trip", false), pl(3, "Other", false)];
        assert_eq!(find("ROAD TRIP", &all, None).map(|p| p.id), Some(2), "the ordinary one can be replaced");
        assert_eq!(find("road trip", &all, Some(2)).map(|p| p.id), Some(1), "itself aside (a rename)");
        assert_eq!(find("Road Trip 2", &all, None).map(|p| p.id), None);
        assert_eq!(find("Other", &all, Some(3)).map(|p| p.id), None, "renaming to its own name is no clash");
        assert_eq!(find("Café", &[pl(1, "Cafe", false)], None).map(|p| p.id), None, "accents make another name");
    }

    #[test]
    fn the_rest_counts_the_clashes_still_to_come() {
        let names = ["Mix", "Chill"];
        let later: Vec<String> = ["chill", "New", "new", "Mix", "Fresh"].iter().map(|s| (*s).to_owned()).collect();
        // "chill" (a playlist), the second "new" (the first of the batch), "Mix" (a playlist and the one asked about).
        assert_eq!(later_clashes("Mix", &later, &names), 3);
        assert_eq!(later_clashes("Mix", &[], &names), 0);
        assert_eq!(later_clashes("Solo", &["solo".to_owned()], &names), 1, "the one being asked about counts");
    }

    #[test]
    fn the_buttons_map_to_the_choices() {
        assert_eq!(Choice::from_index(0), Choice::Cancel);
        assert_eq!(Choice::from_index(1), Choice::Replace);
        assert_eq!(Choice::from_index(2), Choice::KeepBoth);
        assert_eq!(Choice::from_index(-1), Choice::Cancel, "anything else cancels");
    }

    #[test]
    fn what_a_playlist_holds_is_said() {
        assert_eq!(holds(&Playlist { id: 1, name: "Mix".into(), tracks: 1, rules: None }), "\u{201c}Mix\u{201d} has 1 track");
        assert_eq!(holds(&Playlist { id: 1, name: "Mix".into(), tracks: 24, rules: None }), "\u{201c}Mix\u{201d} has 24 tracks");
    }
}
