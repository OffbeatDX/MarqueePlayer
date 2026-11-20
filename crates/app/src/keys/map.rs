//! Remappable shortcuts as data: each action has default keys, the user's changes are
//! kept in settings.json (`shortcuts`, only the actions that differ from the defaults,
//! so a later change of a default still reaches everyone who never remapped it), and
//! Settings > Keys edits them. Pure and unit-tested; `keys/shortcuts.rs` matches key presses
//! against a `Keymap` and runs what they say, and `keys/edit.rs` is the editor.
//!
//! A binding is one key with modifiers, written as Settings and settings.json show it:
//! "M", "Space", "Ctrl+Left", "Ctrl+Shift+F5", "/". A plain character key ignores Shift
//! (the character says it: "/" is Shift+7 on a German layout, and letters match either
//! case, so Caps Lock does not matter). Keys that structure the app (Tab, Enter, Esc,
//! the arrows and paging keys in lists, Menu, Delete, the text-editing chords) are fixed:
//! `fixed_reason` refuses them, whatever the action.

use std::collections::{BTreeMap, HashSet};

use slint::platform::Key;

/// Everything a key can be given to, in the order Settings > Keys lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    PlayPause,
    SeekBack,
    SeekForward,
    Previous,
    Next,
    VolumeUp,
    VolumeDown,
    Mute,
    Shuffle,
    Repeat,
    Favourite,
    /// Rate the playing track; 0 clears its rating.
    Rate(u8),
    NowPlaying,
    Lyrics,
    Visualizer,
    /// The visualizer's next preset (its look).
    VisualizerLook,
    /// Where the visualizer shows: behind the page, in the panel, full page, in turn.
    VisualizerPlace,
    Search,
    Back,
    Forward,
    PinSidebar,
    PinQueue,
    /// View metadata, with tag editing on: review and save the draft.
    SaveTags,
    /// View metadata, with tag editing on: edit the field (or track row) the cursor is on.
    EditField,
    /// Settings > Equalizer: copy the setting as Equalizer APO text.
    EqCopy,
    /// Settings > Equalizer: paste a setting from the clipboard (shown first, then applied).
    EqPaste,
    /// The playing track's synced lyrics 0.1 s earlier, or later (kept for the track).
    LyricsEarlier,
    LyricsLater,
    /// The interface size a step larger, or smaller (`zoom.rs`).
    ZoomIn,
    ZoomOut,
    /// The interface size back to 100%.
    ZoomReset,
}

impl Action {
    // New actions go at the end: Settings > Keys lists them in this order and the bench
    // finds rows by their place.
    pub const ALL: [Action; 36] = [
        Action::PlayPause,
        Action::SeekBack,
        Action::SeekForward,
        Action::Previous,
        Action::Next,
        Action::VolumeUp,
        Action::VolumeDown,
        Action::Mute,
        Action::Shuffle,
        Action::Repeat,
        Action::Favourite,
        Action::Rate(1),
        Action::Rate(2),
        Action::Rate(3),
        Action::Rate(4),
        Action::Rate(5),
        Action::Rate(0),
        Action::NowPlaying,
        Action::Lyrics,
        Action::Visualizer,
        Action::VisualizerLook,
        Action::VisualizerPlace,
        Action::Search,
        Action::Back,
        Action::Forward,
        Action::PinSidebar,
        Action::PinQueue,
        Action::SaveTags,
        Action::EditField,
        Action::EqCopy,
        Action::EqPaste,
        Action::LyricsEarlier,
        Action::LyricsLater,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::ZoomReset,
    ];

    /// Its name in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            Action::PlayPause => "play_pause",
            Action::SeekBack => "seek_back",
            Action::SeekForward => "seek_forward",
            Action::Previous => "previous",
            Action::Next => "next",
            Action::VolumeUp => "volume_up",
            Action::VolumeDown => "volume_down",
            Action::Mute => "mute",
            Action::Shuffle => "shuffle",
            Action::Repeat => "repeat",
            Action::Favourite => "favourite",
            Action::Rate(0) => "rate_0",
            Action::Rate(1) => "rate_1",
            Action::Rate(2) => "rate_2",
            Action::Rate(3) => "rate_3",
            Action::Rate(4) => "rate_4",
            Action::Rate(_) => "rate_5",
            Action::NowPlaying => "now_playing",
            Action::Lyrics => "lyrics",
            Action::Visualizer => "visualizer",
            Action::VisualizerLook => "visualizer_look",
            Action::VisualizerPlace => "visualizer_placement",
            Action::Search => "search",
            Action::Back => "back",
            Action::Forward => "forward",
            Action::PinSidebar => "pin_sidebar",
            Action::PinQueue => "pin_queue",
            Action::SaveTags => "save_tags",
            Action::EditField => "edit_field",
            Action::EqCopy => "eq_copy",
            Action::EqPaste => "eq_paste",
            Action::LyricsEarlier => "lyrics_earlier",
            Action::LyricsLater => "lyrics_later",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::ZoomReset => "zoom_reset",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The group Settings > Keys lists it under.
    pub fn group(self) -> &'static str {
        match self {
            Action::PlayPause
            | Action::SeekBack
            | Action::SeekForward
            | Action::Previous
            | Action::Next
            | Action::VolumeUp
            | Action::VolumeDown
            | Action::Mute
            | Action::Shuffle
            | Action::Repeat => "Playback",
            Action::Favourite | Action::Rate(_) => "Favourites and ratings",
            Action::NowPlaying
            | Action::Lyrics
            | Action::Visualizer
            | Action::VisualizerLook
            | Action::VisualizerPlace
            | Action::Search
            | Action::Back
            | Action::Forward
            | Action::PinSidebar
            | Action::PinQueue => {
                "Pages and panels"
            }
            Action::SaveTags | Action::EditField => "Tag editing",
            Action::EqCopy | Action::EqPaste => "Equalizer",
            Action::LyricsEarlier | Action::LyricsLater => "Lyrics",
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => "Interface size",
        }
    }

    /// What it does, as Settings > Keys says it.
    pub fn describe(self) -> &'static str {
        match self {
            Action::PlayPause => "Play / pause",
            Action::SeekBack => "Seek 10 s back",
            Action::SeekForward => "Seek 10 s forward",
            Action::Previous => "Previous track",
            Action::Next => "Next track",
            Action::VolumeUp => "Volume up 5%",
            Action::VolumeDown => "Volume down 5%",
            Action::Mute => "Mute / unmute",
            Action::Shuffle => "Shuffle on / off",
            Action::Repeat => "Repeat: off, all, one",
            Action::Favourite => "Add the playing track to favourites, or remove it",
            Action::Rate(0) => "Clear the playing track's rating",
            Action::Rate(1) => "Rate the playing track 1 star",
            Action::Rate(2) => "Rate the playing track 2 stars",
            Action::Rate(3) => "Rate the playing track 3 stars",
            Action::Rate(4) => "Rate the playing track 4 stars",
            Action::Rate(_) => "Rate the playing track 5 stars",
            Action::NowPlaying => "Now playing (again: back to where you were)",
            Action::Lyrics => "Lyrics on now playing (again: Up next)",
            Action::Visualizer => "Visualizer on now playing on / off",
            Action::VisualizerLook => "Next visualizer look (its presets in turn, then yours)",
            Action::VisualizerPlace => "Where the visualizer shows: behind the page, in the panel, full page",
            Action::Search => "Search",
            Action::Back => "Back (also the mouse side button nearer you)",
            Action::Forward => "Forward (also the other side button)",
            Action::PinSidebar => "Pin or unpin the sidebar (works in the search field too)",
            Action::PinQueue => "Pin or unpin the queue (works in the search field too)",
            Action::SaveTags => "Review and save tag edits (View metadata, with tag editing on)",
            Action::EditField => "Edit the field or track the cursor is on (View metadata)",
            Action::EqCopy => "Copy the equalizer as Equalizer APO text (Settings > Equalizer), or the visualizer's look (Settings > Visualizer)",
            Action::EqPaste => "Paste equalizer settings, shown before they apply (Settings > Equalizer), or a visualizer preset (Settings > Visualizer)",
            Action::LyricsEarlier => "Synced lyrics of the playing track 0.1 s earlier (hold for more; kept for the track)",
            Action::LyricsLater => "Synced lyrics of the playing track 0.1 s later (hold for more; kept for the track)",
            Action::ZoomIn => "Interface size larger (zoom in; also Ctrl and the wheel, or a pinch)",
            Action::ZoomOut => "Interface size smaller (zoom out)",
            Action::ZoomReset => "Interface size back to 100%",
        }
    }

    /// Its default keys: one per slot. Search has two (`/` and Ctrl+F), zooming in two
    /// (Ctrl+= and Ctrl++); every other action one.
    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::PlayPause => &["Space"],
            Action::SeekBack => &["Left"],
            Action::SeekForward => &["Right"],
            Action::Previous => &["Ctrl+Left"],
            Action::Next => &["Ctrl+Right"],
            Action::VolumeUp => &["Ctrl+Up"],
            Action::VolumeDown => &["Ctrl+Down"],
            Action::Mute => &["M"],
            Action::Shuffle => &["S"],
            Action::Repeat => &["R"],
            Action::Favourite => &["L"],
            Action::Rate(0) => &["0"],
            Action::Rate(1) => &["1"],
            Action::Rate(2) => &["2"],
            Action::Rate(3) => &["3"],
            Action::Rate(4) => &["4"],
            Action::Rate(_) => &["5"],
            Action::NowPlaying => &["N"],
            Action::Lyrics => &["Y"],
            Action::Visualizer => &["V"],
            Action::VisualizerLook => &["C"],
            // Shift and the visualizer's own key: V turns it on and off (Ctrl+V pastes).
            Action::VisualizerPlace => &["Shift+V"],
            Action::Search => &["/", "Ctrl+F"],
            Action::Back => &["Alt+Left"],
            Action::Forward => &["Alt+Right"],
            Action::PinSidebar => &["Ctrl+B"],
            // On macOS Command stands for Ctrl, and Cmd+Q quits (the app menu's Quit takes it
            // before the window sees it): the queue's pin is Cmd+J there.
            Action::PinQueue => {
                if cfg!(target_os = "macos") {
                    &["Ctrl+J"]
                } else {
                    &["Ctrl+Q"]
                }
            }
            Action::SaveTags => &["Ctrl+S"],
            Action::EditField => &["F2"],
            // Copy and paste where nothing is a text to copy: Settings > Equalizer outside its
            // fields (a field keeps its own Ctrl+C and Ctrl+V).
            Action::EqCopy => &["Ctrl+C"],
            Action::EqPaste => &["Ctrl+V"],
            Action::LyricsEarlier => &["G"],
            Action::LyricsLater => &["H"],
            // As in browsers: Ctrl and the = key (+ with Shift on many layouts, which
            // `action_for` lets through), and the numeric keypad's + (or a layout's own +).
            // The keypad's - is the same "-".
            Action::ZoomIn => &["Ctrl+=", "Ctrl++"],
            Action::ZoomOut => &["Ctrl+-"],
            Action::ZoomReset => &["Ctrl+0"],
        }
    }

    pub fn slots(self) -> usize {
        self.defaults().len()
    }

    fn index(self) -> usize {
        Action::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }
}

/// Named keys: (name in settings.json and labels, the key, how Settings shows it).
fn named() -> Vec<(String, char, String)> {
    let mut v: Vec<(String, char, String)> = [
        ("Space", Key::Space, "Space"),
        ("Left", Key::LeftArrow, "←"),
        ("Right", Key::RightArrow, "→"),
        ("Up", Key::UpArrow, "↑"),
        ("Down", Key::DownArrow, "↓"),
        ("Enter", Key::Return, "Enter"),
        ("Tab", Key::Tab, "Tab"),
        ("Backtab", Key::Backtab, "Shift+Tab"),
        ("Esc", Key::Escape, "Esc"),
        ("Backspace", Key::Backspace, "Backspace"),
        ("Delete", Key::Delete, "Delete"),
        ("Insert", Key::Insert, "Insert"),
        ("Home", Key::Home, "Home"),
        ("End", Key::End, "End"),
        ("PageUp", Key::PageUp, "Page Up"),
        ("PageDown", Key::PageDown, "Page Down"),
        ("Menu", Key::Menu, "Menu"),
        ("Pause", Key::Pause, "Pause"),
        ("ScrollLock", Key::ScrollLock, "Scroll Lock"),
        ("PrintScreen", Key::SysReq, "Print Screen"),
    ]
    .into_iter()
    .map(|(n, k, d)| (n.to_string(), char::from(k), d.to_string()))
    .collect();
    let f1 = char::from(Key::F1) as u32;
    for i in 0..24 {
        if let Some(c) = char::from_u32(f1 + i) {
            v.push((format!("F{}", i + 1), c, format!("F{}", i + 1)));
        }
    }
    v
}

/// Modifier keys on their own: never a binding (a capture waits for the key after).
fn is_modifier(c: char) -> bool {
    [Key::Shift, Key::ShiftR, Key::Control, Key::ControlR, Key::Alt, Key::AltGr, Key::Meta, Key::MetaR, Key::CapsLock]
        .into_iter()
        .any(|k| char::from(k) == c)
}

/// One key and its modifiers. `key` is the key as Slint reports it (`KeyEvent.text`),
/// letters in lower case.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub key: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Binding {
    /// A key press as a binding, or `None` for a lone modifier, nothing or several
    /// characters (an input method's text).
    pub fn from_press(text: &str, ctrl: bool, shift: bool, alt: bool) -> Option<Binding> {
        let mut chars = text.chars();
        let c = chars.next()?;
        if chars.next().is_some() || is_modifier(c) {
            return None;
        }
        let printable = Self::printable_char(c);
        let key = if c.is_alphabetic() { c.to_lowercase().collect() } else { c.to_string() };
        // A typed character carries Shift itself ("/" is Shift+7 on some layouts); with Ctrl
        // or Alt, Shift is its own, and on a letter too (Shift+V is not V: a key that nothing
        // has with Shift still acts as the letter, `Keymap::action_for`).
        let shift = shift && !(printable && !ctrl && !alt && !c.is_alphabetic());
        Some(Binding { key, ctrl, shift, alt })
    }

    fn printable_char(c: char) -> bool {
        !c.is_control() && !c.is_whitespace() && !('\u{F700}'..='\u{F8FF}').contains(&c)
    }

    /// Read "Ctrl+Shift+F", "Space", "Alt+Left", "/", "Ctrl++" (case does not matter).
    pub fn parse(s: &str) -> Option<Binding> {
        let mut rest = s.trim();
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        loop {
            let lower = rest.to_ascii_lowercase();
            let take = |p: &str| lower.starts_with(p) && rest.len() > p.len();
            if take("ctrl+") {
                ctrl = true;
                rest = &rest[5..];
            } else if take("shift+") {
                shift = true;
                rest = &rest[6..];
            } else if take("alt+") {
                alt = true;
                rest = &rest[4..];
            } else if let Some(r) = rest.strip_prefix('⌘').filter(|r| !r.is_empty()) {
                // macOS shows the modifiers by their symbols (`display`).
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⇧').filter(|r| !r.is_empty()) {
                shift = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⌥').filter(|r| !r.is_empty()) {
                alt = true;
                rest = r;
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return None;
        }
        let key = named()
            .into_iter()
            .find(|(n, _, d)| n.eq_ignore_ascii_case(rest) || (d.chars().count() == 1 && d == rest))
            .map(|(_, c, _)| c.to_string())
            .or_else(|| {
                let mut chars = rest.chars();
                let c = chars.next()?;
                (chars.next().is_none() && Self::printable_char(c)).then(|| c.to_string())
            })?;
        Binding::from_press(&key, ctrl, shift, alt)
    }

    fn mods(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s
    }

    /// As tooltips and screen readers say it: "Ctrl+Left"; on macOS, where Command stands for
    /// Ctrl and Option is Alt, "Cmd+Left", "Option+Left".
    pub fn spoken(&self) -> String {
        let config = self.config();
        if !cfg!(target_os = "macos") {
            return config;
        }
        let mut rest = config.as_str();
        let mut s = String::new();
        for (from, to) in [("Ctrl+", "Cmd+"), ("Shift+", "Shift+"), ("Alt+", "Option+")] {
            if let Some(r) = rest.strip_prefix(from) {
                s.push_str(to);
                rest = r;
            }
        }
        s + rest
    }

    /// As settings.json keeps it: "Ctrl+Left" (on every system, so a settings file moves
    /// between them).
    pub fn config(&self) -> String {
        let c = self.key.chars().next().unwrap_or(' ');
        let name = named().into_iter().find(|(_, k, _)| *k == c).map(|(n, _, _)| n);
        format!("{}{}", self.mods(), name.unwrap_or_else(|| self.key.to_uppercase()))
    }

    /// As Settings > Keys shows it: "Ctrl+←"; on macOS as its menus do, Option, Shift and
    /// Command by their symbols in Apple's order: "⇧⌘F", "⌥←".
    pub fn display(&self) -> String {
        let c = self.key.chars().next().unwrap_or(' ');
        let name = named().into_iter().find(|(_, k, _)| *k == c).map(|(_, _, d)| d);
        let mods = if cfg!(target_os = "macos") {
            [(self.alt, "⌥"), (self.shift, "⇧"), (self.ctrl, "⌘")].iter().filter(|m| m.0).map(|m| m.1).collect()
        } else {
            self.mods()
        };
        format!("{mods}{}", name.unwrap_or_else(|| self.key.to_uppercase()))
    }

    fn is(&self, k: Key) -> bool {
        self.key == char::from(k).to_string()
    }

    fn plain(&self) -> bool {
        !self.ctrl && !self.shift && !self.alt
    }

    /// The Win32 virtual-key code, for the keys the window can catch before the focused
    /// control (letters, digits, F-keys and named keys; not punctuation, which differs
    /// by layout).
    pub fn virtual_key(&self) -> Option<u16> {
        let c = self.key.chars().next()?;
        if self.key.chars().count() != 1 {
            return None;
        }
        if c.is_ascii_lowercase() {
            return Some(0x41 + (c as u16 - 'a' as u16));
        }
        if c.is_ascii_digit() {
            return Some(0x30 + (c as u16 - '0' as u16));
        }
        let f1 = char::from(Key::F1) as u32;
        if (f1..f1 + 24).contains(&(c as u32)) {
            return Some(0x70 + (c as u32 - f1) as u16);
        }
        let vk = [
            (Key::Space, 0x20),
            (Key::PageUp, 0x21),
            (Key::PageDown, 0x22),
            (Key::End, 0x23),
            (Key::Home, 0x24),
            (Key::LeftArrow, 0x25),
            (Key::UpArrow, 0x26),
            (Key::RightArrow, 0x27),
            (Key::DownArrow, 0x28),
            (Key::Insert, 0x2D),
            (Key::Delete, 0x2E),
            (Key::Backspace, 0x08),
            (Key::Pause, 0x13),
        ];
        vk.into_iter().find(|(k, _)| char::from(*k) == c).map(|(_, v)| v)
    }
}

/// Why a key cannot be given to `action`, or `None` when it can. The keys that hold
/// the app together stay where they are.
pub fn fixed_reason(b: &Binding, action: Action) -> Option<&'static str> {
    let arrows_vertical = b.is(Key::UpArrow) || b.is(Key::DownArrow);
    let arrows_horizontal = b.is(Key::LeftArrow) || b.is(Key::RightArrow);
    let paging = b.is(Key::Home) || b.is(Key::End) || b.is(Key::PageUp) || b.is(Key::PageDown);
    Some(match () {
        _ if b.is(Key::Tab) || b.is(Key::Backtab) => "Tab moves the keyboard focus",
        _ if b.is(Key::Return) => "Enter presses buttons and opens what a list's cursor is on",
        _ if b.is(Key::Escape) => "Esc cancels, closes a sidebar and goes back",
        _ if b.is(Key::Menu) || (b.is(Key::F10) && b.shift && !b.ctrl && !b.alt) => "it opens the menu of an album or track",
        _ if b.plain() && arrows_vertical => "the arrows move in lists and change sliders",
        _ if b.plain() && arrows_horizontal && !matches!(action, Action::SeekBack | Action::SeekForward) => {
            "← and → move in rows and grids of covers; only seeking can share them"
        }
        _ if b.plain() && paging => "it moves in lists",
        _ if b.alt && !b.ctrl && !b.shift && arrows_vertical => on_mac("Alt+↑ and Alt+↓ move a track in a playlist", "Option+↑ and Option+↓ move a track in a playlist"),
        _ if b.plain() && b.is(Key::Delete) => "Delete removes a track from the queue or a playlist",
        _ if b.ctrl && b.alt => on_mac("Ctrl+Alt is AltGr on many keyboards, which types characters", "Option+Cmd is kept free, as Ctrl+Alt (AltGr, which types characters) is on other systems"),
        // Ctrl+C and Ctrl+V can be the equalizer's copy and paste: they act only outside text
        // fields, where they edit nothing.
        _ if b.ctrl && !b.alt && !b.shift && ["a", "c", "v", "x", "z", "y"].contains(&b.key.as_str()) && !(matches!(action, Action::EqCopy | Action::EqPaste) && ["c", "v"].contains(&b.key.as_str())) => {
            "it edits text"
        }
        _ if b.alt && !b.ctrl && (b.is(Key::F4) || b.is(Key::Space)) => "it belongs to the window (close, window menu)",
        _ if b.ctrl && b.shift && !b.alt && b.key == "d" => on_mac("Ctrl+Shift+D shows the side-button diagnostics", "Cmd+Shift+D shows the side-button diagnostics"),
        // The app menu's key equivalents (Slint's default menu bar), taken before the window.
        _ if cfg!(target_os = "macos") && b.ctrl && !b.alt && !b.shift && b.key == "q" => "Cmd+Q quits the app",
        _ if cfg!(target_os = "macos") && b.ctrl && !b.shift && b.key == "h" => "Cmd+H and Option+Cmd+H hide apps",
        _ => return None,
    })
}

/// The keys that are not remappable, for Settings > Keys: (keys, what they do).
pub fn fixed_help() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Tab / Shift+Tab", "Move between buttons, fields and lists"),
        ("Enter", "Press the focused button; in a list, open the album, artist or genre, or play the track"),
        ("Esc", "Close a sidebar, else back; cancel"),
        ("↑ ↓ ← →", "In a list or a grid of covers: move (← → seek in track lists); on a slider, change it"),
        ("Home / End, Page Up / Down", "Move in a list or a grid of covers"),
        ("Menu  or  Shift+F10", "The menu of the focused album or track"),
        ("Delete", "Remove the focused track from the queue, or from the open playlist"),
        (on_mac("Alt+↑ / Alt+↓", "Option+↑ / Option+↓"), "Move the focused track in the open playlist"),
        (
            on_mac("Ctrl+A, C, V, X, Z", "Cmd+A, C, V, X, Z"),
            on_mac(
                "Edit text in a text field (outside one, Ctrl+C and Ctrl+V copy and paste the equalizer)",
                "Edit text in a text field (outside one, Cmd+C and Cmd+V copy and paste the equalizer)",
            ),
        ),
        (on_mac("Ctrl+Shift+D", "Cmd+Shift+D"), "Show the last side-button input (diagnostics)"),
    ]
}

/// A fixed text as the system names its keys: on macOS Command stands for Ctrl and Option is
/// Alt, as `Binding::spoken` says them.
fn on_mac(other: &'static str, mac: &'static str) -> &'static str {
    if cfg!(target_os = "macos") { mac } else { other }
}

/// What `Keymap::assign` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Assign {
    /// The key is now the action's.
    Done,
    /// The slot already had it.
    Unchanged,
    /// Another action has it (slot given): nothing changed; `reassign` takes it over.
    Conflict(Action, usize),
    /// A fixed key (why).
    Fixed(&'static str),
}

/// Every action's keys, one `Option<Binding>` per slot (`None`: no key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    slots: Vec<Vec<Option<Binding>>>,
}

fn defaults_of(a: Action) -> Vec<Option<Binding>> {
    a.defaults().iter().map(|s| Binding::parse(s)).collect()
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap { slots: Action::ALL.iter().map(|a| defaults_of(*a)).collect() }
    }
}

impl Keymap {
    /// The defaults with the user's changes (`shortcuts` in settings.json) on top, and
    /// what was wrong with them (each logged by the caller, the rest still applied): an
    /// unknown action, a key that does not parse or is fixed, a key two changed actions
    /// both claim. A changed action's key always wins over a default (a default added
    /// later never takes a key the user chose); the action that had it by default goes
    /// without.
    pub fn from_overrides(map: &BTreeMap<String, Vec<String>>) -> (Keymap, Vec<String>) {
        let mut km = Keymap::default();
        let mut problems = Vec::new();
        let mut changed: Vec<Action> = Vec::new();
        for (id, keys) in map {
            let Some(a) = Action::from_id(id) else {
                problems.push(format!("unknown action {id:?}"));
                continue;
            };
            let mut slots: Vec<Option<Binding>> = Vec::new();
            let mut ok = true;
            for s in keys.iter().take(a.slots()) {
                if s.trim().is_empty() {
                    slots.push(None);
                    continue;
                }
                match Binding::parse(s) {
                    Some(b) if fixed_reason(&b, a).is_some() => {
                        problems.push(format!("{id}: {s:?} is a fixed key"));
                        ok = false;
                    }
                    Some(b) => slots.push(Some(b)),
                    None => {
                        problems.push(format!("{id}: {s:?} is not a key"));
                        ok = false;
                    }
                }
            }
            if keys.len() > a.slots() {
                problems.push(format!("{id}: only {} key(s); the rest is ignored", a.slots()));
            }
            if !ok {
                continue;
            }
            slots.resize(a.slots(), None);
            km.slots[a.index()] = slots;
            changed.push(a);
        }
        // One key, one action: the user's choices first (in list order), then defaults.
        let mut taken: HashSet<Binding> = HashSet::new();
        let order: Vec<Action> = Action::ALL.iter().copied().filter(|a| changed.contains(a)).chain(Action::ALL.iter().copied().filter(|a| !changed.contains(a))).collect();
        for a in order {
            for slot in km.slots[a.index()].iter_mut() {
                if let Some(b) = slot
                    && !taken.insert(b.clone())
                {
                    if changed.contains(&a) {
                        problems.push(format!("{}: {} is already another action's", a.id(), b.config()));
                    }
                    *slot = None;
                }
            }
        }
        (km, problems)
    }

    /// The actions whose keys differ from the defaults, for settings.json ("" for a
    /// slot without a key).
    pub fn overrides(&self) -> BTreeMap<String, Vec<String>> {
        Action::ALL
            .iter()
            .filter(|a| !self.is_default(**a))
            .map(|a| (a.id().to_string(), self.slots[a.index()].iter().map(|b| b.as_ref().map(Binding::config).unwrap_or_default()).collect()))
            .collect()
    }

    pub fn bindings(&self, a: Action) -> &[Option<Binding>] {
        &self.slots[a.index()]
    }

    pub fn is_default(&self, a: Action) -> bool {
        self.slots[a.index()] == defaults_of(a)
    }

    /// The action a key press is bound to. Shift and a letter that no action has with Shift
    /// is the letter's (as before Shift could make a letter a key of its own).
    pub fn action_for(&self, text: &str, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
        let b = Binding::from_press(text, ctrl, shift, alt)?;
        if let Some((a, _)) = self.holder(&b) {
            return Some(a);
        }
        let letter = b.key.chars().next().is_some_and(char::is_alphabetic);
        if letter && b.shift && !b.ctrl && !b.alt {
            return self.holder(&Binding { shift: false, ..b }).map(|(a, _)| a);
        }
        // Ctrl, Shift and a character that needs Shift on this layout ("+" on a US keyboard is
        // Shift and =): the character's, with Ctrl, when nothing has it with Shift too.
        if !letter && b.shift && b.ctrl && !b.alt {
            return self.holder(&Binding { shift: false, ..b }).map(|(a, _)| a);
        }
        None
    }

    /// Which action (and slot) has this key.
    pub fn holder(&self, b: &Binding) -> Option<(Action, usize)> {
        Action::ALL.iter().find_map(|a| self.slots[a.index()].iter().position(|s| s.as_ref() == Some(b)).map(|i| (*a, i)))
    }

    /// Give `b` to `a`'s `slot`, unless it is fixed or another action's.
    pub fn assign(&mut self, a: Action, slot: usize, b: Binding) -> Assign {
        if slot >= a.slots() {
            return Assign::Unchanged;
        }
        if let Some(why) = fixed_reason(&b, a) {
            return Assign::Fixed(why);
        }
        match self.holder(&b) {
            Some((h, s)) if h == a && s == slot => Assign::Unchanged,
            Some((h, s)) if h != a || s != slot => {
                if h == a {
                    // The action's other slot: move it here.
                    self.slots[a.index()][s] = None;
                    self.slots[a.index()][slot] = Some(b);
                    Assign::Done
                } else {
                    Assign::Conflict(h, s)
                }
            }
            _ => {
                self.slots[a.index()][slot] = Some(b);
                Assign::Done
            }
        }
    }

    /// Give `b` to `a`'s `slot`, taking it from whichever action had it (that one is left
    /// without a key there). Fixed keys are still refused.
    pub fn reassign(&mut self, a: Action, slot: usize, b: Binding) -> Assign {
        if let Some(why) = fixed_reason(&b, a) {
            return Assign::Fixed(why);
        }
        if let Some((h, s)) = self.holder(&b) {
            self.slots[h.index()][s] = None;
        }
        if slot < a.slots() {
            self.slots[a.index()][slot] = Some(b);
        }
        Assign::Done
    }

    /// Leave `a`'s `slot` without a key.
    pub fn clear(&mut self, a: Action, slot: usize) {
        if let Some(s) = self.slots[a.index()].get_mut(slot) {
            *s = None;
        }
    }

    /// Back to `a`'s default keys. Another action holding one of them gives it up, as a
    /// reassign does. Returns the actions that lost a key that way, and the key.
    pub fn reset(&mut self, a: Action) -> Vec<(Action, Binding)> {
        let mut lost = Vec::new();
        for (slot, b) in defaults_of(a).into_iter().enumerate() {
            self.slots[a.index()][slot] = None;
            if let Some(b) = b {
                if let Some((h, s)) = self.holder(&b) {
                    self.slots[h.index()][s] = None;
                    if h != a {
                        lost.push((h, b.clone()));
                    }
                }
                self.slots[a.index()][slot] = Some(b);
            }
        }
        lost
    }

    pub fn reset_all(&mut self) {
        *self = Keymap::default();
    }

    /// The action's keys for a label or tooltip: "Ctrl+F" or "/ or Ctrl+F"; "" for none.
    pub fn label(&self, a: Action) -> String {
        self.slots[a.index()].iter().flatten().map(Binding::spoken).collect::<Vec<_>>().join(" or ")
    }

    /// The rating keys in a few words, for the player bar's star: "0 to 5" while they are
    /// the defaults, else each one ("1: A, 2: S, …, clear: Q").
    pub fn rating_label(&self) -> String {
        if (0..=5).all(|n| self.is_default(Action::Rate(n))) {
            return "0 to 5".into();
        }
        let mut parts: Vec<String> = (1..=5u8).filter_map(|n| self.first(Action::Rate(n)).map(|b| format!("{n}: {}", b.spoken()))).collect();
        if let Some(b) = self.first(Action::Rate(0)) {
            parts.push(format!("clear: {}", b.spoken()));
        }
        parts.join(", ")
    }

    fn first(&self, a: Action) -> Option<&Binding> {
        self.slots[a.index()].iter().flatten().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Binding {
        Binding::parse(s).unwrap_or_else(|| panic!("{s} parses"))
    }

    fn k(k: Key) -> String {
        char::from(k).to_string()
    }

    #[test]
    fn parse_and_format() {
        assert_eq!(b("M"), Binding { key: "m".into(), ctrl: false, shift: false, alt: false });
        assert_eq!(b("ctrl+shift+f5").config(), "Ctrl+Shift+F5");
        assert_eq!(b("Ctrl+Left").display(), on_mac("Ctrl+←", "⌘←"));
        assert_eq!(b("⌥⇧⌘F"), b("Ctrl+Shift+Alt+F"), "macOS' symbols read on every system");
        assert_eq!(b("Ctrl+←"), b("Ctrl+Left"), "the arrow glyph reads too");
        assert_eq!(b("Alt+Right").config(), "Alt+Right");
        assert_eq!(b("Space").display(), "Space");
        assert_eq!(b("/").config(), "/");
        assert_eq!(b("Shift+/"), b("/"), "a character carries its Shift");
        assert_eq!(b("m"), b("M"), "letters ignore case");
        assert_ne!(b("Shift+M"), b("m"), "Shift and a letter is a key of its own");
        assert_eq!(b("Shift+V").config(), "Shift+V");
        assert_eq!(b("Ctrl++").key, "+");
        assert_eq!(b("Ctrl+Shift+D").config(), "Ctrl+Shift+D", "Shift counts with Ctrl");
        assert_eq!(b("PageDown").display(), "Page Down");
        assert_eq!(b("F12").config(), "F12");
        for bad in ["", "Ctrl+", "Hyper+X", "Ctrl+Fn", "MM", "Shift"] {
            assert_eq!(Binding::parse(bad), None, "{bad:?}");
        }
        // Every default round-trips through both forms.
        for a in Action::ALL {
            for d in a.defaults() {
                let x = b(d);
                assert_eq!(&x.config(), d);
                assert_eq!(Binding::parse(&x.config()), Some(x.clone()));
                assert_eq!(Binding::parse(&x.display()), Some(x));
            }
        }
    }

    #[test]
    fn presses() {
        assert_eq!(Binding::from_press(&k(Key::Shift), false, true, false), None, "a lone modifier waits for the key");
        assert_eq!(Binding::from_press(&k(Key::Control), true, false, false), None);
        assert_eq!(Binding::from_press("", false, false, false), None);
        assert_eq!(Binding::from_press("ab", false, false, false), None, "input-method text");
        assert_eq!(Binding::from_press("M", false, true, false), Some(b("Shift+M")), "Shift on a letter counts");
        assert_eq!(Binding::from_press("M", false, false, false), Some(b("M")), "Caps Lock: the letter");
        assert_eq!(Binding::from_press("?", false, true, false), Some(b("?")), "a typed character carries its Shift");
        assert_eq!(Binding::from_press(" ", false, true, false), Some(b("Shift+Space")), "Space is a named key: Shift counts");
        assert_eq!(Binding::from_press(&k(Key::F5), false, true, false).unwrap().config(), "Shift+F5");
    }

    #[test]
    fn defaults_match_the_old_table() {
        let km = Keymap::default();
        let act = |t: &str, c, s, a| km.action_for(t, c, s, a);
        assert_eq!(act(" ", false, false, false), Some(Action::PlayPause));
        assert_eq!(act(" ", false, true, false), None, "Shift+Space is not play");
        assert_eq!(act("/", false, true, false), Some(Action::Search), "Shift+7 on a German layout");
        assert_eq!(act("f", true, false, false), Some(Action::Search));
        assert_eq!(act("F", true, false, false), Some(Action::Search));
        assert_eq!(act("f", true, true, false), None);
        assert_eq!(act(&k(Key::LeftArrow), false, false, false), Some(Action::SeekBack));
        assert_eq!(act(&k(Key::LeftArrow), true, false, false), Some(Action::Previous));
        assert_eq!(act(&k(Key::LeftArrow), false, false, true), Some(Action::Back));
        assert_eq!(act(&k(Key::LeftArrow), false, true, false), None);
        assert_eq!(act(&k(Key::UpArrow), true, false, false), Some(Action::VolumeUp));
        assert_eq!(act(&k(Key::UpArrow), false, false, false), None, "plain Up belongs to lists");
        for (c, a) in [("m", Action::Mute), ("s", Action::Shuffle), ("r", Action::Repeat), ("l", Action::Favourite), ("n", Action::NowPlaying), ("y", Action::Lyrics)] {
            assert_eq!(act(c, false, false, false), Some(a));
            assert_eq!(act(&c.to_uppercase(), false, false, false), Some(a), "Caps Lock");
            assert_eq!(act(&c.to_uppercase(), false, true, false), Some(a), "Shift: no action has Shift with it, so the letter's");
            let with_ctrl = if c == "s" { Some(Action::SaveTags) } else { None };
            assert_eq!(act(c, true, false, false), with_ctrl, "not with Ctrl (Ctrl+S: save tag edits, in View metadata)");
            assert_eq!(act(c, false, false, true), None, "not with Alt");
        }
        // Shift+V: where the visualizer shows; V alone turns it on and off.
        assert_eq!(act("V", false, true, false), Some(Action::VisualizerPlace));
        assert_eq!(act("v", false, false, false), Some(Action::Visualizer));
        assert_eq!(act("V", false, false, false), Some(Action::Visualizer), "Caps Lock");
        assert_eq!(act("v", true, false, false), Some(Action::EqPaste), "Ctrl+V pastes");
        assert_eq!(act("4", false, false, false), Some(Action::Rate(4)));
        assert_eq!(act("0", false, false, false), Some(Action::Rate(0)));
        assert_eq!(act("6", false, false, false), None);
        assert_eq!(act("4", true, false, false), None);
        assert_eq!(act("b", true, false, false), Some(Action::PinSidebar));
        assert_eq!(act(on_mac("q", "j"), true, false, false), Some(Action::PinQueue), "Cmd+Q quits on macOS");
        assert_eq!(act("b", false, false, false), None, "plain B types");
        assert_eq!(act(&k(Key::Escape), false, false, false), None, "Esc is fixed, not in the map");
        assert_eq!(act("c", true, false, false), Some(Action::EqCopy), "Ctrl+C: the equalizer's copy, outside text fields");
        assert_eq!(act("V", true, false, false), Some(Action::EqPaste));
        assert_eq!(fixed_reason(&b("Ctrl+V"), Action::EqCopy), None, "copy and paste can swap");
        assert!(fixed_reason(&b("Ctrl+X"), Action::EqCopy).is_some(), "the other text chords stay fixed");
        // No default is fixed, and no key is two actions' defaults.
        let mut seen = HashSet::new();
        for a in Action::ALL {
            assert_eq!(a.slots(), km.bindings(a).len());
            for x in km.bindings(a).iter().flatten() {
                assert_eq!(fixed_reason(x, a), None, "{} for {a:?}", x.config());
                assert!(seen.insert(x.clone()), "{} twice", x.config());
            }
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert!(km.overrides().is_empty(), "nothing to save");
    }

    #[test]
    fn fixed_keys_are_refused() {
        for (s, a) in [("Tab", Action::Mute), ("Shift+Tab", Action::Mute), ("Enter", Action::Mute), ("Esc", Action::Back), ("Up", Action::VolumeUp), ("Left", Action::Mute), ("Home", Action::Previous), ("PageDown", Action::Next), ("Alt+Up", Action::Mute), ("Delete", Action::Mute), ("Ctrl+C", Action::PinQueue), ("Ctrl+Alt+Q", Action::PinQueue), ("Menu", Action::Mute), ("Shift+F10", Action::Mute), ("Alt+F4", Action::Mute), ("Ctrl+Shift+D", Action::Mute)] {
            assert!(fixed_reason(&b(s), a).is_some(), "{s} for {a:?}");
            let mut km = Keymap::default();
            assert!(matches!(km.assign(a, 0, b(s)), Assign::Fixed(_)), "{s}");
            assert!(matches!(km.reassign(a, 0, b(s)), Assign::Fixed(_)), "{s}");
            assert_eq!(km, Keymap::default(), "nothing changed for {s}");
        }
        assert_eq!(fixed_reason(&b("Left"), Action::SeekForward), None, "seeking can have either arrow");
        assert_eq!(fixed_reason(&b("Shift+Up"), Action::VolumeUp), None);
        assert_eq!(fixed_reason(&b("Ctrl+Home"), Action::Previous), None);
        assert_eq!(fixed_reason(&b("F10"), Action::Mute), None);
    }

    #[test]
    fn overrides_load_partial_and_invalid() {
        let map = |pairs: &[(&str, &[&str])]| pairs.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect::<BTreeMap<_, _>>();
        // Partial: only mute changed; everything else default.
        let (km, problems) = Keymap::from_overrides(&map(&[("mute", &["K"])]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(km.action_for("k", false, false, false), Some(Action::Mute));
        assert_eq!(km.action_for("m", false, false, false), None, "M is free now");
        assert_eq!(km.action_for(" ", false, false, false), Some(Action::PlayPause));
        assert_eq!(km.overrides(), map(&[("mute", &["K"])]), "only the change is saved");
        // Invalid entries: each ignored, its action keeps its default; the rest applies.
        let (km, problems) = Keymap::from_overrides(&map(&[("nope", &["X"]), ("shuffle", &["Hyper+Q"]), ("repeat", &["Tab"]), ("lyrics", &["U"])]));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(km.action_for("s", false, false, false), Some(Action::Shuffle));
        assert_eq!(km.action_for("r", false, false, false), Some(Action::Repeat));
        assert_eq!(km.action_for("u", false, false, false), Some(Action::Lyrics));
        // A slot left empty, a short and a long list.
        let (km, problems) = Keymap::from_overrides(&map(&[("search", &["", "Ctrl+F"]), ("favourite", &[""]), ("next", &["Ctrl+Right", "F9"])]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(km.action_for("/", false, false, false), None);
        assert_eq!(km.action_for("f", true, false, false), Some(Action::Search));
        assert_eq!(km.action_for("l", false, false, false), None, "favourite has no key");
        let (km, _) = Keymap::from_overrides(&map(&[("search", &["Ctrl+K"])]));
        assert_eq!(km.bindings(Action::Search), &[Some(b("Ctrl+K")), None], "a short list leaves the other slot empty");
        // A changed action's key beats another's default: here M, mute's default.
        let (km, problems) = Keymap::from_overrides(&map(&[("shuffle", &["M"])]));
        assert!(problems.is_empty());
        assert_eq!(km.action_for("m", false, false, false), Some(Action::Shuffle));
        assert_eq!(km.bindings(Action::Mute), &[None]);
        // Two changed actions claiming one key: the first in list order keeps it.
        let (km, problems) = Keymap::from_overrides(&map(&[("repeat", &["J"]), ("mute", &["J"])]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(km.action_for("j", false, false, false), Some(Action::Mute));
        assert_eq!(km.bindings(Action::Repeat), &[None]);
    }

    #[test]
    fn conflicts_reassign_clear_reset() {
        let mut km = Keymap::default();
        // A free key.
        assert_eq!(km.assign(Action::Mute, 0, b("K")), Assign::Done);
        assert_eq!(km.assign(Action::Mute, 0, b("K")), Assign::Unchanged);
        // Taken: nothing changes until reassigned.
        assert_eq!(km.assign(Action::Mute, 0, b("Ctrl+F")), Assign::Conflict(Action::Search, 1));
        assert_eq!(km.action_for("k", false, false, false), Some(Action::Mute));
        assert_eq!(km.action_for("f", true, false, false), Some(Action::Search));
        assert_eq!(km.reassign(Action::Mute, 0, b("Ctrl+F")), Assign::Done);
        assert_eq!(km.action_for("f", true, false, false), Some(Action::Mute));
        assert_eq!(km.bindings(Action::Search), &[Some(b("/")), None], "search keeps /");
        assert_eq!(km.label(Action::Search), "/");
        // Search's own other slot: the key moves within the action, no conflict.
        assert_eq!(km.assign(Action::Search, 1, b("/")), Assign::Done);
        assert_eq!(km.bindings(Action::Search), &[None, Some(b("/"))]);
        // Clear.
        km.clear(Action::Repeat, 0);
        assert_eq!(km.action_for("r", false, false, false), None);
        assert_eq!(km.label(Action::Repeat), "");
        // Reset one: search takes Ctrl+F back from mute (mute is left without).
        assert_eq!(km.reset(Action::Search), vec![(Action::Mute, b("Ctrl+F"))]);
        assert_eq!(km.label(Action::Search), on_mac("/ or Ctrl+F", "/ or Cmd+F"));
        assert_eq!(km.bindings(Action::Mute), &[None]);
        assert!(km.is_default(Action::Search) && !km.is_default(Action::Mute));
        let saved = km.overrides();
        assert_eq!(saved.keys().collect::<Vec<_>>(), vec!["mute", "repeat"]);
        assert_eq!(saved["repeat"], vec![String::new()]);
        // What was saved loads back the same.
        assert_eq!(Keymap::from_overrides(&saved).0, km);
        km.reset_all();
        assert_eq!(km, Keymap::default());
    }

    #[test]
    fn labels() {
        let mut km = Keymap::default();
        assert_eq!(km.label(Action::Previous), on_mac("Ctrl+Left", "Cmd+Left"));
        assert_eq!(km.rating_label(), "0 to 5");
        km.assign(Action::Rate(1), 0, b("A"));
        km.clear(Action::Rate(0), 0);
        assert_eq!(km.rating_label(), "1: A, 2: 2, 3: 3, 4: 4, 5: 5");
    }

    #[test]
    fn virtual_keys() {
        assert_eq!(b("Ctrl+B").virtual_key(), Some(0x42));
        assert_eq!(b("Ctrl+Q").virtual_key(), Some(0x51));
        assert_eq!(b("Alt+7").virtual_key(), Some(0x37));
        assert_eq!(b("Ctrl+F12").virtual_key(), Some(0x7B));
        assert_eq!(b("Ctrl+Left").virtual_key(), Some(0x25));
        assert_eq!(b("Ctrl+/").virtual_key(), None, "punctuation differs by layout");
    }

    #[test]
    fn the_fixed_keys_are_named_as_the_system_names_them() {
        let texts: Vec<&str> = fixed_help().into_iter().flat_map(|(keys, what)| [keys, what]).chain([fixed_reason(&b("Ctrl+Shift+D"), Action::Mute).unwrap(), fixed_reason(&b("Alt+Up"), Action::Mute).unwrap()]).collect();
        for t in texts {
            if cfg!(target_os = "macos") {
                assert!(!t.contains("Ctrl") && !t.contains("Alt+"), "{t}");
            } else {
                assert!(!t.contains("Cmd") && !t.contains("Option"), "{t}");
            }
        }
        assert!(fixed_help().iter().any(|(keys, _)| keys.starts_with(if cfg!(target_os = "macos") { "Cmd+A" } else { "Ctrl+A" })));
    }

    #[test]
    fn shift_letters_are_keys_of_their_own_and_remap() {
        let mut km = Keymap::default();
        // Given to an action, Shift+M is that action's; M stays mute's.
        assert_eq!(km.assign(Action::Shuffle, 0, b("Shift+M")), Assign::Done);
        assert_eq!(km.action_for("M", false, true, false), Some(Action::Shuffle));
        assert_eq!(km.action_for("m", false, false, false), Some(Action::Mute));
        // Saved and loaded back as it was.
        let saved = km.overrides();
        assert_eq!(saved["shuffle"], vec!["Shift+M".to_string()]);
        assert_eq!(Keymap::from_overrides(&saved).0, km);
        // The placement key moved: Shift+V is free again, and acts as V.
        assert_eq!(km.assign(Action::VisualizerPlace, 0, b("Shift+P")), Assign::Done);
        assert_eq!(km.action_for("V", false, true, false), Some(Action::Visualizer));
        assert_eq!(km.action_for("P", false, true, false), Some(Action::VisualizerPlace));
    }

    #[test]
    fn the_zoom_keys_are_a_browsers_and_remap() {
        let mut km = Keymap::default();
        // Ctrl and =, Ctrl and + (the keypad's, or a layout's own key), Ctrl, Shift and = (a
        // US keyboard's "+"), Ctrl and - (both minus keys), Ctrl and 0.
        assert_eq!(km.action_for("=", true, false, false), Some(Action::ZoomIn));
        assert_eq!(km.action_for("+", true, false, false), Some(Action::ZoomIn));
        assert_eq!(km.action_for("+", true, true, false), Some(Action::ZoomIn));
        assert_eq!(km.action_for("-", true, false, false), Some(Action::ZoomOut));
        assert_eq!(km.action_for("0", true, false, false), Some(Action::ZoomReset));
        // Without Ctrl they are what they were: 0 clears the rating, + and - nothing.
        assert_eq!(km.action_for("0", false, false, false), Some(Action::Rate(0)));
        assert_eq!(km.action_for("+", false, false, false), None);
        assert_eq!(km.action_for("-", true, false, true), None);
        // Shown and saved as written.
        assert_eq!(km.bindings(Action::ZoomIn).iter().flatten().map(Binding::display).collect::<Vec<_>>(), [on_mac("Ctrl+=", "⌘="), on_mac("Ctrl++", "⌘+")]);
        assert_eq!(b("Ctrl++").config(), "Ctrl++");
        // None is fixed, and none is another action's.
        for a in [Action::ZoomIn, Action::ZoomOut, Action::ZoomReset] {
            for k in km.bindings(a).iter().flatten() {
                assert_eq!(fixed_reason(k, a), None, "{}", k.config());
                assert_eq!(km.holder(k).map(|h| h.0), Some(a));
            }
        }
        // Remapped like any other, and back.
        assert_eq!(km.assign(Action::ZoomOut, 0, b("Ctrl+9")), Assign::Done);
        assert_eq!(km.action_for("9", true, false, false), Some(Action::ZoomOut));
        assert_eq!(km.action_for("-", true, false, false), None);
        let saved = km.overrides();
        assert_eq!(saved["zoom_out"], vec!["Ctrl+9".to_string()]);
        assert_eq!(Keymap::from_overrides(&saved).0, km);
        // Ctrl, Shift and a character goes to the character with Ctrl only when nothing has
        // it with Shift too.
        assert_eq!(km.assign(Action::ZoomReset, 0, b("Ctrl+Shift+=")), Assign::Done);
        assert_eq!(km.action_for("=", true, true, false), Some(Action::ZoomReset));
        assert_eq!(km.action_for("=", true, false, false), Some(Action::ZoomIn));
        // The new actions come last, after the lyrics keys (Settings > Keys' rows keep their
        // places).
        let n = Action::ALL.len();
        assert_eq!(&Action::ALL[n - 5..], &[Action::LyricsEarlier, Action::LyricsLater, Action::ZoomIn, Action::ZoomOut, Action::ZoomReset]);
    }
}
