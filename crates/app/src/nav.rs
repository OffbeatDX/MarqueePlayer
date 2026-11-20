//! Where the user is and has been. Pure logic, unit-tested; `views` fills pages.

use library::ShelfQuery;
use library::query::SearchKind;

use crate::Page;

/// A place in the app. Content pages carry what they show, so going back to an album
/// shows that album again, not whichever one was opened last.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Page(Page),
    Album(i64),
    Artist(String),
    Playlist(i64),
    /// The smart playlist editor: for the smart playlist of that id, or -1 for a new one.
    /// Its draft is kept while elsewhere (playlists/smart.rs), so Back and Forward return to it.
    SmartEdit(i64),
    /// A home shelf in full: its query, its title, and for a random shelf the shuffle.
    /// Also a genre's or label's albums (from the Genres page).
    Shelf { query: ShelfQuery, title: String, seed: i64 },
    /// One section of a search in full ("See all").
    SearchAll { kind: SearchKind, query: String },
    /// Settings, showing one of its sections (`settings::nav::SECTIONS`). Changing the
    /// section replaces the entry rather than adding one: Back leaves Settings, and
    /// returns to the section that was left.
    Settings(usize),
}

impl Default for Route {
    fn default() -> Self {
        Route::Page(Page::Home)
    }
}

impl Route {
    /// The page name shown in the navigation pill.
    pub fn title(&self) -> String {
        match self {
            Route::Shelf { title, .. } => title.clone(),
            Route::SearchAll { kind, query } => search_title(*kind, query),
            Route::SmartEdit(id) if *id < 0 => "New smart playlist".into(),
            Route::SmartEdit(_) => "Edit smart playlist".into(),
            other => other.page_name().into(),
        }
    }

    fn page_name(&self) -> &'static str {
        match self {
            Route::Album(_) => "Album",
            Route::Artist(_) => "Artist",
            Route::Playlist(_) => "Playlist",
            Route::SmartEdit(_) => "Smart playlist",
            Route::Shelf { .. } => "Shelf",
            Route::SearchAll { .. } => "Search",
            Route::Settings(_) => "Settings",
            Route::Page(p) => match p {
                Page::Home => "Home",
                Page::Albums => "Albums",
                Page::Album => "Album",
                Page::Artist => "Artist",
                Page::Search => "Search",
                Page::Playlists => "Playlists",
                Page::Playlist => "Playlist",
                Page::NowPlaying => "Now playing",
                Page::Settings => "Settings",
                Page::Shelves => "Home shelves",
                Page::Shelf => "Shelf",
                Page::Artists => "Artists",
                Page::Genres => "Genres",
                Page::Labels => "Labels",
                Page::Tracks => "Tracks",
                Page::Smart => "Smart playlist",
                Page::SmartEdit => "Smart playlist",
            },
        }
    }
}

/// A search section's full page title ("Albums for “night”"), on the page and in the pill.
pub fn search_title(kind: SearchKind, query: &str) -> String {
    let what = match kind {
        SearchKind::Albums => "Albums",
        SearchKind::Artists => "Artists",
        SearchKind::Tracks => "Tracks",
    };
    format!("{what} for “{}”", query.trim())
}

const LIMIT: usize = 100;

/// Browser-style history: a back stack, the current place, a forward stack. Each entry
/// remembers how far its page was scrolled when it was left, so Back returns there.
#[derive(Debug, Default)]
pub struct History {
    back: Vec<(Route, f32)>,
    pub current: Route,
    forward: Vec<(Route, f32)>,
}

impl History {
    /// Record that `to` is now shown, after `current`, which was scrolled to `scroll`.
    /// Clears forward.
    pub fn push(&mut self, to: Route, scroll: f32) {
        let from = std::mem::replace(&mut self.current, to);
        if from != self.current {
            self.back.push((from, scroll));
            if self.back.len() > LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// The place to show for Back and its saved scroll; `current` (scrolled to `scroll`)
    /// moves to the forward stack. A place that no longer exists is skipped by stepping
    /// again.
    pub fn back(&mut self, scroll: f32) -> Option<(Route, f32)> {
        let (to, saved) = self.back.pop()?;
        let from = std::mem::replace(&mut self.current, to.clone());
        self.forward.push((from, scroll));
        Some((to, saved))
    }

    pub fn forward(&mut self, scroll: f32) -> Option<(Route, f32)> {
        let (to, saved) = self.forward.pop()?;
        let from = std::mem::replace(&mut self.current, to.clone());
        self.back.push((from, scroll));
        Some((to, saved))
    }

    /// Show `to` in place of the current place (no new history entry): a Settings
    /// section chosen on the page.
    pub fn replace(&mut self, to: Route) {
        self.current = to;
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(h: Option<(Route, f32)>) -> Option<Route> {
        h.map(|(r, _)| r)
    }

    #[test]
    fn back_restores_the_album_that_was_open() {
        let mut h = History::default();
        h.push(Route::Album(1), 0.0);
        h.push(Route::Artist("A".into()), 0.0);
        h.push(Route::Album(2), 0.0);
        assert_eq!(r(h.back(0.0)), Some(Route::Artist("A".into())));
        assert_eq!(r(h.back(0.0)), Some(Route::Album(1)));
        assert_eq!(r(h.back(0.0)), Some(Route::Page(Page::Home)));
        assert_eq!(h.back(0.0), None);
        assert!(!h.can_back());
        assert_eq!(r(h.forward(0.0)), Some(Route::Album(1)));
        assert_eq!(r(h.forward(0.0)), Some(Route::Artist("A".into())));
        assert_eq!(h.current, Route::Artist("A".into()));
    }

    #[test]
    fn back_and_forward_return_to_where_each_page_was_scrolled() {
        let mut h = History::default();
        // Home scrolled to -800 when an album is opened; the album scrolled to -300.
        h.push(Route::Album(7), -800.0);
        assert_eq!(h.back(-300.0), Some((Route::Page(Page::Home), -800.0)));
        assert_eq!(h.forward(-50.0), Some((Route::Album(7), -300.0)));
        assert_eq!(h.back(0.0), Some((Route::Page(Page::Home), -50.0)));
    }

    #[test]
    fn going_somewhere_new_clears_forward() {
        let mut h = History::default();
        h.push(Route::Page(Page::Albums), 0.0);
        h.back(0.0);
        assert!(h.can_forward());
        h.push(Route::Page(Page::Search), 0.0);
        assert!(!h.can_forward());
        assert_eq!(r(h.back(0.0)), Some(Route::Page(Page::Home)));
    }

    #[test]
    fn a_shelf_page_is_a_place_with_its_title_and_shuffle() {
        let shelf = Route::Shelf { query: ShelfQuery::Random, title: "Explore".into(), seed: 42 };
        assert_eq!(shelf.title(), "Explore");
        let mut h = History::default();
        h.push(shelf.clone(), 0.0);
        h.push(Route::Album(7), -900.0);
        assert_eq!(h.back(0.0), Some((shelf, -900.0)), "the same shuffle, scrolled where it was");
    }

    #[test]
    fn a_search_section_is_a_place_with_its_query() {
        let all = Route::SearchAll { kind: SearchKind::Tracks, query: "night ".into() };
        assert_eq!(all.title(), "Tracks for “night”");
        let mut h = History::default();
        h.push(Route::Page(Page::Search), 0.0);
        h.push(all.clone(), -100.0);
        h.push(Route::Album(3), -2400.0);
        assert_eq!(h.back(0.0), Some((all, -2400.0)));
        assert_eq!(Route::Page(Page::Labels).title(), "Labels");
    }

    #[test]
    fn a_settings_section_is_not_an_entry_but_back_returns_to_it() {
        let mut h = History::default();
        h.push(Route::Settings(0), 0.0);
        // Choosing sections on the page: the entry changes, no entries are added.
        h.replace(Route::Settings(2));
        h.replace(Route::Settings(7));
        h.push(Route::Page(Page::Shelves), -420.0);
        assert_eq!(h.back(0.0), Some((Route::Settings(7), -420.0)), "Keys, scrolled where it was");
        assert_eq!(h.current.title(), "Settings");
        assert_eq!(r(h.back(0.0)), Some(Route::Page(Page::Home)), "Back from Settings leaves it");
    }

    #[test]
    fn same_place_twice_is_one_entry() {
        let mut h = History::default();
        h.push(Route::Page(Page::Albums), 0.0);
        h.push(Route::Page(Page::Albums), 0.0);
        assert_eq!(r(h.back(0.0)), Some(Route::Page(Page::Home)));
        assert!(!h.can_back());
    }
}
