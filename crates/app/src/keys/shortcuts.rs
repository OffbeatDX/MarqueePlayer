//! Keyboard shortcuts: a key press nothing focused took (the window's key handler,
//! through the `Actions.shortcut` callback) is looked up in the keymap (`keys/map.rs`,
//! the user's keys over the defaults, edited in Settings > Keys) and run here. Esc and
//! Ctrl+Shift+D are fixed and matched first. Also where a list's cursor moves for the
//! arrow and paging keys (`list_key`). Pure apart from `run`, and unit-tested.
//!
//! Text fields keep their keys (`TypingScope` in widgets.slint), so a key bound to an
//! action never fires while typing. The pin actions' keys, when they are Ctrl or Alt
//! chords, are also taken at the window before focus (`platform::window`), so they work in the
//! search field too.

use std::rc::Rc;

use slint::ComponentHandle;
use slint::platform::Key;

use crate::keys::map::{Action, Keymap};
use crate::state::State;
use crate::{Focus, Nav, Page, playback, views};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// A remappable action.
    Do(Action),
    /// Esc: close a sidebar, else back.
    Escape,
    /// Ctrl+Shift+D: the side-button readout.
    Diagnostics,
}

fn key(k: Key) -> String {
    char::from(k).to_string()
}

/// Tell the window which pin keys to take before the focused control: the pin actions'
/// Ctrl or Alt chords (they type nothing, so taking them in a text field loses nothing).
/// A plain key bound to a pin is left to the shortcut handler, which never sees what a
/// text field types.
pub fn sync_window_keys(km: &Keymap) {
    use crate::platform::PinSide;
    let mut keys = Vec::new();
    for (a, side) in [(Action::PinSidebar, PinSide::Left), (Action::PinQueue, PinSide::Right)] {
        for b in km.bindings(a).iter().flatten() {
            if b.ctrl != b.alt
                && let Some(vk) = b.virtual_key()
            {
                keys.push((vk, b.ctrl, b.shift, b.alt, side));
            }
        }
    }
    crate::platform::window::set_pin_keys(keys);
}

/// The shortcut for a key press nothing on the page took, if it is one.
pub fn action(keymap: &Keymap, text: &str, ctrl: bool, shift: bool, alt: bool) -> Option<Shortcut> {
    if ctrl && shift && !alt && text.eq_ignore_ascii_case("d") {
        return Some(Shortcut::Diagnostics);
    }
    if !ctrl && !alt && text == key(Key::Escape) {
        return Some(Shortcut::Escape);
    }
    keymap.action_for(text, ctrl, shift, alt).map(Shortcut::Do)
}

/// How a list is laid out, for `list_key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Up and Down move (track lists, the queue). Left and Right are left to seeking.
    Vertical,
    /// Left and Right move (a home shelf's row of covers).
    Horizontal,
    /// All four arrows move, Up and Down by a row of `cols` (cover grids).
    Grid,
}

impl Layout {
    pub fn from_index(i: i32) -> Layout {
        match i {
            1 => Layout::Horizontal,
            2 => Layout::Grid,
            _ => Layout::Vertical,
        }
    }
}

/// Where a list's cursor goes for a key: the arrows, Home and End, Page Up and Down
/// (`page` rows a screen). `None` when the key does not move this list (it then goes on
/// to the shortcuts: Left and Right seek in a track list). At an end the cursor stays put
/// and the key is still the list's, so holding Down never starts seeking. Down from the
/// row above a short last row lands on the last item.
pub fn list_key(text: &str, index: i32, count: i32, layout: Layout, cols: i32, page: i32) -> Option<i32> {
    if count <= 0 {
        return None;
    }
    let last = count - 1;
    let i = index.clamp(0, last);
    let cols = if layout == Layout::Grid { cols.max(1) } else { 1 };
    let page = page.max(1) * cols;
    let is = |k: Key| text == key(k);
    let vertical = layout != Layout::Horizontal;
    let horizontal = layout != Layout::Vertical;
    let down = |by: i32| {
        if i + by <= last {
            i + by
        } else if (layout == Layout::Grid && by == cols && i / cols < last / cols) || by > cols {
            last
        } else {
            i
        }
    };
    let up = |by: i32| if i - by >= 0 { i - by } else if by > cols { 0 } else { i };
    Some(match () {
        _ if vertical && is(Key::UpArrow) => up(cols),
        _ if vertical && is(Key::DownArrow) => down(cols),
        _ if horizontal && is(Key::LeftArrow) => (i - 1).max(0),
        _ if horizontal && is(Key::RightArrow) => (i + 1).min(last),
        _ if is(Key::Home) => 0,
        _ if is(Key::End) => last,
        _ if is(Key::PageUp) => up(if vertical { page } else { page.max(2) }),
        _ if is(Key::PageDown) => down(if vertical { page } else { page.max(2) }),
        _ => return None,
    })
}

/// Do what a shortcut says.
pub fn run(st: &Rc<State>, s: Shortcut) {
    let Some(app) = st.app.upgrade() else { return };
    let nav = app.global::<Nav>();
    let a = match s {
        Shortcut::Do(a) => a,
        Shortcut::Escape => {
            // Rows chosen in a list: Esc clears them first (the list itself takes it when it has
            // the keyboard; this is for after a menu the pointer opened, which took it).
            if crate::selected::clear_all(st) {
                return;
            }
            // The keyboard was in a sidebar: close it and come back to the page.
            let zone = app.global::<Focus>().get_zone();
            if zone == "sidebar" || zone == "queue" {
                app.invoke_focus_shortcuts();
            }
            if nav.get_sidebar_forced() || nav.get_sidebar_hover_open() || zone == "sidebar" {
                nav.set_sidebar_forced(false);
                nav.set_sidebar_hover_open(false);
            } else if nav.get_queue_open() || nav.get_queue_hover_open() || zone == "queue" {
                nav.set_queue_open(false);
                nav.set_queue_hover_open(false);
            } else {
                views::back(st);
            }
            return;
        }
        Shortcut::Diagnostics => {
            nav.set_show_input_readout(!nav.get_show_input_readout());
            return;
        }
    };
    match a {
        Action::Search => {
            views::go(st, Page::Search);
            app.invoke_focus_search();
        }
        Action::PlayPause => playback::toggle_play(st),
        Action::SeekBack => playback::seek_by(st, -10.0),
        Action::SeekForward => playback::seek_by(st, 10.0),
        Action::Previous => playback::previous(st),
        Action::Next => playback::next(st),
        Action::VolumeUp | Action::VolumeDown => {
            let v = playback::nudge_volume(st, if a == Action::VolumeUp { 1 } else { -1 });
            views::flash(st, &format!("Volume {}%", (v * 100.0).round()));
        }
        Action::Mute => {
            playback::toggle_mute(st);
            views::flash(st, if st.settings.borrow().muted { "Muted" } else { "Sound on" });
        }
        Action::Shuffle => {
            playback::toggle_shuffle(st);
            views::flash(st, if st.queue.borrow().shuffled() { "Shuffle on" } else { "Shuffle off" });
        }
        Action::Repeat => {
            playback::cycle_repeat(st);
            let r = app.global::<crate::Player>().get_repeat();
            views::flash(st, ["Repeat off", "Repeat all", "Repeat one"][r.clamp(0, 2) as usize]);
        }
        Action::Favourite => {
            if views::playing_track_id(st) < 0 {
                views::flash(st, "Nothing is playing");
                return;
            }
            // Not changed: the pill already says why.
            if let Some(fav) = playback::toggle_favourite_current(st) {
                views::flash(st, if fav { "Added to favourites" } else { "Removed from favourites" });
            }
        }
        Action::NowPlaying => {
            if nav.get_page() == Page::NowPlaying {
                views::back(st);
            } else {
                views::go(st, Page::NowPlaying);
            }
        }
        Action::Lyrics => {
            let l = app.global::<crate::Lyrics>();
            // The panel's visualizer tab gives way to the lyrics.
            app.global::<crate::Visualizer>().set_tab(false);
            if nav.get_page() == Page::NowPlaying {
                l.set_shown(!l.get_shown());
            } else {
                l.set_shown(true);
                views::go(st, Page::NowPlaying);
            }
        }
        Action::Visualizer => {
            crate::toggle_visualizer(st);
            views::flash(st, if st.settings.borrow().visualizer { "Visualizer on" } else { "Visualizer off" });
        }
        Action::VisualizerLook => crate::visualizer::next_look(st),
        Action::VisualizerPlace => crate::visualizer::cycle_placement(st),
        Action::Back => {
            views::back(st);
        }
        Action::Forward => {
            views::forward(st);
        }
        Action::PinSidebar => crate::on_pin_shortcut(crate::platform::PinSide::Left),
        Action::PinQueue => crate::on_pin_shortcut(crate::platform::PinSide::Right),
        // They act only in View metadata, which takes its keys itself (metadata::view::shortcut).
        Action::SaveTags | Action::EditField => {}
        Action::LyricsEarlier => crate::lyrics::nudge(st, -1, true),
        Action::LyricsLater => crate::lyrics::nudge(st, 1, true),
        Action::ZoomIn => crate::zoom::step_by(st, 1),
        Action::ZoomOut => crate::zoom::step_by(st, -1),
        Action::ZoomReset => crate::zoom::step_by(st, 0),
        // Only on Settings > Equalizer, and on Settings > Visualizer for its look (elsewhere
        // Ctrl+C and Ctrl+V do nothing, as before).
        Action::EqCopy | Action::EqPaste => {
            if crate::eq::shown(st) {
                if a == Action::EqCopy {
                    crate::eq::copy(st, crate::eq::CopyAs::Apo);
                } else {
                    crate::eq::paste(st);
                }
            } else if crate::visualizer::shown(st) {
                if a == Action::EqCopy {
                    crate::visualizer::presets::copy(st);
                } else {
                    crate::visualizer::presets::paste(st);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(k: Key) -> String {
        key(k)
    }

    #[test]
    fn keys_map_to_shortcuts() {
        let km = Keymap::default();
        let act = |t: &str, c, s, a| action(&km, t, c, s, a);
        assert_eq!(act(" ", false, false, false), Some(Shortcut::Do(Action::PlayPause)));
        assert_eq!(act(&k(Key::Escape), false, false, false), Some(Shortcut::Escape));
        assert_eq!(act(&k(Key::Escape), false, true, false), Some(Shortcut::Escape), "Shift+Esc too, as before");
        assert_eq!(act("d", true, true, false), Some(Shortcut::Diagnostics));
        assert_eq!(act("D", true, true, false), Some(Shortcut::Diagnostics));
        assert_eq!(act("b", true, false, false), Some(Shortcut::Do(Action::PinSidebar)), "also matched here (the bench, a plain-key binding)");
        assert_eq!(act("x", false, false, false), None);
        assert_eq!(act(&k(Key::Delete), false, false, false), None, "Delete belongs to the queue and playlists");
    }

    #[test]
    fn remapped_keys_act_and_fixed_ones_stay() {
        let mut km = Keymap::default();
        let k_key = crate::keys::map::Binding::parse("K").unwrap();
        assert_eq!(km.assign(Action::Mute, 0, k_key), crate::keys::map::Assign::Done);
        assert_eq!(action(&km, "k", false, false, false), Some(Shortcut::Do(Action::Mute)));
        assert_eq!(action(&km, "m", false, false, false), None);
        // A remap can never take Esc or the diagnostics chord.
        let esc = crate::keys::map::Binding::from_press(&k(Key::Escape), false, false, false).unwrap();
        assert!(matches!(km.assign(Action::Mute, 0, esc), crate::keys::map::Assign::Fixed(_)));
        assert_eq!(action(&km, &k(Key::Escape), false, false, false), Some(Shortcut::Escape));
    }

    #[test]
    fn vertical_lists_move_up_and_down() {
        let v = |key: Key, i| list_key(&k(key), i, 10, Layout::Vertical, 1, 4);
        assert_eq!(v(Key::DownArrow, 0), Some(1));
        assert_eq!(v(Key::UpArrow, 5), Some(4));
        assert_eq!(v(Key::UpArrow, 0), Some(0), "stays at the top, still taken");
        assert_eq!(v(Key::DownArrow, 9), Some(9));
        assert_eq!(v(Key::Home, 6), Some(0));
        assert_eq!(v(Key::End, 2), Some(9));
        assert_eq!(v(Key::PageDown, 2), Some(6));
        assert_eq!(v(Key::PageDown, 8), Some(9), "a page past the end lands on the last");
        assert_eq!(v(Key::PageUp, 2), Some(0));
        assert_eq!(v(Key::LeftArrow, 3), None, "left to seeking");
        assert_eq!(v(Key::RightArrow, 3), None);
        assert_eq!(list_key(" ", 0, 10, Layout::Vertical, 1, 4), None);
        assert_eq!(list_key(&k(Key::DownArrow), 0, 0, Layout::Vertical, 1, 4), None, "empty");
        assert_eq!(list_key(&k(Key::DownArrow), 40, 10, Layout::Vertical, 1, 4), Some(9), "a cursor past a shrunk list");
    }

    #[test]
    fn grids_move_by_rows() {
        // 10 covers, 4 a row: rows 0-3, 4-7, 8-9.
        let g = |key: Key, i| list_key(&k(key), i, 10, Layout::Grid, 4, 2);
        assert_eq!(g(Key::RightArrow, 3), Some(4), "wraps to the next row");
        assert_eq!(g(Key::LeftArrow, 4), Some(3));
        assert_eq!(g(Key::LeftArrow, 0), Some(0));
        assert_eq!(g(Key::DownArrow, 1), Some(5));
        assert_eq!(g(Key::DownArrow, 6), Some(9), "into the short last row: the last cover");
        assert_eq!(g(Key::DownArrow, 9), Some(9), "already on the last row");
        assert_eq!(g(Key::DownArrow, 8), Some(8));
        assert_eq!(g(Key::UpArrow, 9), Some(5));
        assert_eq!(g(Key::UpArrow, 2), Some(2), "top row stays");
        assert_eq!(g(Key::PageDown, 1), Some(9), "two rows down from row 0 is row 2's end");
        assert_eq!(g(Key::PageUp, 9), Some(1));
    }

    #[test]
    fn rows_of_covers_move_sideways() {
        let h = |key: Key, i| list_key(&k(key), i, 5, Layout::Horizontal, 1, 3);
        assert_eq!(h(Key::RightArrow, 0), Some(1));
        assert_eq!(h(Key::RightArrow, 4), Some(4));
        assert_eq!(h(Key::LeftArrow, 2), Some(1));
        assert_eq!(h(Key::UpArrow, 2), None, "up and down leave the row to the page");
        assert_eq!(h(Key::DownArrow, 2), None);
        assert_eq!(h(Key::End, 0), Some(4));
        assert_eq!(h(Key::PageDown, 0), Some(3));
    }
}
