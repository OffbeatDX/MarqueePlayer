//! Linux: the window, with client-side decorations that keep the app's look on Wayland
//! (xdg-shell) and X11 alike. The window is created without the system's decorations (no
//! title bar from the compositor: the page runs to the top edge as on Windows) and this
//! module gives it what a frame would:
//!
//! - **Moving**: the caption strip is Slint's `WindowMoveArea` (app.slint), which hands a
//!   drag past its threshold to the compositor (`xdg_toplevel.move`, `_NET_WM_MOVERESIZE`),
//!   so snapping and edge tiling are the desktop's own. A click or double click there
//!   stays the app's (double click maximizes; right click opens the window menu).
//! - **Resizing**: an invisible band along every edge (wider at the corners) shows the
//!   resize cursor and, pressed, hands the resize to the compositor
//!   (`drag_resize_window`). None while maximized.
//! - **The caption buttons**: drawn by the app (chrome.slint) and shown when the pointer
//!   nears the top right corner; the maximize button's hover and press come from here, as
//!   Windows' non-client messages give them there (`FrameEvent`).
//! - **The window menu**: the compositor's (`xdg_toplevel.show_window_menu`) on Wayland;
//!   X11 has no standard one (nothing happens there).
//!
//! Also here, as `winmsg.rs` does on Windows: the mouse side buttons (back, forward), files
//! dropped from a file manager (X11: winit's XDND; winit has no drag and drop on Wayland
//! yet, so there drops are not taken), and the pin shortcuts taken before a text field
//! sees them. Everything runs in winit's event hook, before Slint, on the UI thread.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use slint::ComponentHandle;
use slint::winit_030::winit;
use slint::winit_030::{WinitWindowAccessor, winit::window::ResizeDirection};
use winit::event::{ElementState, MouseButton, WindowEvent};

use crate::AppWindow;
use crate::platform::{BUTTON_H, BUTTON_W, FrameEvent, Hooks, NavInput, PinKey, PinSide, in_caption_zone};
pub use crate::platform::unix::virtual_key;

/// The resize band along each edge, and the corners' length along the edge (logical px).
const EDGE: f64 = 6.0;
const CORNER: f64 = 16.0;

thread_local! {
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    static HOOKS: Cell<Option<Hooks>> = const { Cell::new(None) };
    /// The pointer in the window (physical pixels), while it is there.
    static POINTER: Cell<Option<(f64, f64)>> = const { Cell::new(None) };
    static EDGE_DIR: Cell<Option<ResizeDirection>> = const { Cell::new(None) };
    /// Mouse buttons pressed in the page and not yet released (a slider or a row being
    /// dragged): the resize edges wait, so the drag keeps following the pointer.
    static HELD: Cell<u32> = const { Cell::new(0) };
    static MAX_HOVER: Cell<bool> = const { Cell::new(false) };
    static MAX_PRESSED: Cell<bool> = const { Cell::new(false) };
    static ZONE: Cell<bool> = const { Cell::new(false) };
    static MAXIMIZED: Cell<Option<bool>> = const { Cell::new(None) };
    static MODS: Cell<winit::keyboard::ModifiersState> = const { Cell::new(winit::keyboard::ModifiersState::empty()) };
    static PIN_KEYS: RefCell<Vec<PinKey>> =
        RefCell::new(vec![(0x42, true, false, false, PinSide::Left), (0x51, true, false, false, PinSide::Right)]);
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    /// Files dropped one event each (X11), gathered for one hand-over.
    static DROPPED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// The window attributes every window gets: no system decorations (the app draws its
/// caption), and the app id the desktop matches with the desktop file (Wayland `app_id`,
/// X11 `WM_CLASS`), so the taskbar groups the window with its launcher and shows its icon.
pub fn attributes(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;
    let id = crate::platform::APP_ID;
    let attrs = attrs.with_decorations(false);
    let attrs = WindowAttributesExtWayland::with_name(attrs, id, id);
    WindowAttributesExtX11::with_name(attrs, id, id)
}

fn with_window<R>(f: impl FnOnce(&AppWindow, &winit::window::Window) -> R) -> Option<R> {
    let app = APP.with(|a| a.borrow().as_ref().and_then(slint::Weak::upgrade))?;
    app.window().with_winit_window(|w| f(&app, w))
}

/// Once the winit window exists: keep the hooks, report the maximized state. The handle
/// is 0 (Linux code finds the window through the app).
pub fn install(app: &AppWindow, hooks: Hooks) -> Option<isize> {
    let maximized = app.window().with_winit_window(|w| w.is_maximized())?;
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    HOOKS.with(|h| h.set(Some(hooks)));
    MAXIMIZED.with(|m| m.set(Some(maximized)));
    (hooks.frame)(FrameEvent::Maximized(maximized));
    super::instance::window_ready(hooks);
    Some(0)
}

fn emit(e: FrameEvent) {
    if let Some(h) = HOOKS.with(Cell::get) {
        (h.frame)(e);
    }
}

/// Which edge or corner a point (physical) is on, for a window this size; `None` inside.
pub fn edge_at(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Option<ResizeDirection> {
    let (e, c) = (EDGE * scale, CORNER * scale);
    let (left, right, top, bottom) = (x < e, x >= width - e, y < e, y >= height - e);
    let (near_left, near_right, near_top, near_bottom) = (x < c, x >= width - c, y < c, y >= height - c);
    Some(match () {
        _ if (top && near_left) || (left && near_top) => ResizeDirection::NorthWest,
        _ if (top && near_right) || (right && near_top) => ResizeDirection::NorthEast,
        _ if (bottom && near_left) || (left && near_bottom) => ResizeDirection::SouthWest,
        _ if (bottom && near_right) || (right && near_bottom) => ResizeDirection::SouthEast,
        _ if top => ResizeDirection::North,
        _ if bottom => ResizeDirection::South,
        _ if left => ResizeDirection::West,
        _ if right => ResizeDirection::East,
        _ => return None,
    })
}

/// The app-drawn maximize button (the middle of the three at the top right).
pub fn on_max_button(x: f64, y: f64, width: f64, scale: f64) -> bool {
    let (bw, bh) = (f64::from(BUTTON_W) * scale, f64::from(BUTTON_H) * scale);
    y >= 0.0 && y < bh && x >= width - 2.0 * bw && x < width - bw
}

fn cursor_for(dir: ResizeDirection) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as C;
    match dir {
        ResizeDirection::North => C::NResize,
        ResizeDirection::South => C::SResize,
        ResizeDirection::East => C::EResize,
        ResizeDirection::West => C::WResize,
        ResizeDirection::NorthEast => C::NeResize,
        ResizeDirection::NorthWest => C::NwResize,
        ResizeDirection::SouthEast => C::SeResize,
        ResizeDirection::SouthWest => C::SwResize,
    }
}

fn set_max_hover(on: bool) {
    if MAX_HOVER.with(|h| h.replace(on)) != on {
        emit(FrameEvent::MaxHover(on));
    }
}

fn set_zone(on: bool) {
    if ZONE.with(|z| z.replace(on)) != on {
        emit(FrameEvent::CaptionZone(on));
    }
}

/// Input this module takes before Slint does. True when it was taken (Slint does not see
/// it).
pub fn winit_event(_app: &AppWindow, window: &slint::Window, event: &WindowEvent) -> bool {
    match event {
        WindowEvent::CursorMoved { position, .. } => pointer_moved(window, position.x, position.y),
        WindowEvent::CursorLeft { .. } => {
            POINTER.with(|p| p.set(None));
            set_max_hover(false);
            set_zone(false);
            if EDGE_DIR.with(|d| d.take()).is_some() {
                window.with_winit_window(|w| w.set_cursor(winit::window::CursorIcon::Default));
            }
            false
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let taken = mouse_input(window, *state, *button);
            let bit = 1u32 << (held_bit(*button) & 31);
            HELD.with(|h| match state {
                ElementState::Pressed if !taken => h.set(h.get() | bit),
                ElementState::Released => h.set(h.get() & !bit),
                _ => {}
            });
            taken
        }
        WindowEvent::Resized(_) => {
            if let Some(m) = window.with_winit_window(|w| w.is_maximized())
                && MAXIMIZED.with(|c| c.replace(Some(m))) != Some(m)
            {
                emit(FrameEvent::Maximized(m));
            }
            false
        }
        WindowEvent::ModifiersChanged(m) => {
            MODS.with(|c| c.set(m.state()));
            false
        }
        WindowEvent::KeyboardInput { event, .. } => pin_key(event),
        WindowEvent::DroppedFile(path) => {
            dropped(path.clone());
            true
        }
        WindowEvent::HoveredFile(_) | WindowEvent::HoveredFileCancelled => true,
        _ => false,
    }
}

fn pointer_moved(window: &slint::Window, x: f64, y: f64) -> bool {
    POINTER.with(|p| p.set(Some((x, y))));
    let size = window.size();
    let (w, h, scale) = (f64::from(size.width), f64::from(size.height), f64::from(window.scale_factor()));
    let maximized = MAXIMIZED.with(Cell::get).unwrap_or(false);
    set_zone(in_caption_zone(x as f32, y as f32, w as f32, scale as f32));
    set_max_hover(on_max_button(x, y, w, scale));
    // No edge while a drag goes on or autoscroll is on: the page and autoscroll see the move.
    let busy = HELD.with(Cell::get) != 0 || !matches!(crate::autoscroll::mode(), crate::autoscroll::Mode::Off);
    let dir = if maximized || busy { None } else { edge_at(x, y, w, h, scale) };
    let was = EDGE_DIR.with(|d| d.replace(dir));
    if dir != was {
        let icon = dir.map_or(winit::window::CursorIcon::Default, cursor_for);
        window.with_winit_window(|w| w.set_cursor(icon));
    }
    // On a resize edge the page does not see the pointer (it would set its own cursor).
    dir.is_some()
}

fn held_bit(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => 5 + u32::from(n),
    }
}

fn mouse_input(window: &slint::Window, state: ElementState, button: MouseButton) -> bool {
    let Some((x, y)) = POINTER.with(Cell::get) else { return false };
    let size = window.size();
    let scale = f64::from(window.scale_factor());
    match (button, state) {
        (MouseButton::Left, ElementState::Pressed) => {
            if let Some(dir) = EDGE_DIR.with(Cell::get) {
                window.with_winit_window(|w| {
                    if let Err(e) = w.drag_resize_window(dir) {
                        log::warn!("resize from the edge: {e}");
                    }
                });
                return true;
            }
            if on_max_button(x, y, f64::from(size.width), scale) {
                MAX_PRESSED.with(|p| p.set(true));
                emit(FrameEvent::MaxPressed(true));
                return true;
            }
            false
        }
        (MouseButton::Left, ElementState::Released) => {
            if MAX_PRESSED.with(|p| p.replace(false)) {
                emit(FrameEvent::MaxPressed(false));
                if on_max_button(x, y, f64::from(size.width), scale) {
                    toggle_maximize();
                }
                return true;
            }
            false
        }
        (MouseButton::Back, ElementState::Released) => nav("mouse button 4 (back) up", NavInput::Back),
        (MouseButton::Forward, ElementState::Released) => nav("mouse button 5 (forward) up", NavInput::Forward),
        (MouseButton::Back | MouseButton::Forward, ElementState::Pressed) => {
            if let Some(h) = HOOKS.with(Cell::get) {
                (h.nav)(format!("{button:?} button down (acts on release)"), None);
            }
            true
        }
        _ => false,
    }
}

fn nav(what: &str, to: NavInput) -> bool {
    if let Some(h) = HOOKS.with(Cell::get) {
        (h.nav)(what.to_owned(), Some(to));
    }
    true
}

fn pin_key(event: &winit::event::KeyEvent) -> bool {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    if event.state != ElementState::Pressed || event.repeat || CAPTURING.with(Cell::get) {
        return false;
    }
    let Some(vk) = virtual_key(&event.key_without_modifiers()) else { return false };
    let m = MODS.with(Cell::get);
    let side = PIN_KEYS.with(|k| {
        k.borrow().iter().find(|k| k.0 == vk && (k.1, k.2, k.3) == (m.control_key(), m.shift_key(), m.alt_key())).map(|k| k.4)
    });
    let Some(side) = side else { return false };
    if let Some(h) = HOOKS.with(Cell::get) {
        // Out of the event hook: pinning touches app state and the UI.
        let pin = h.pin;
        let _ = slint::invoke_from_event_loop(move || pin(side));
    }
    true
}

/// A file dropped (X11 sends one event per file): gathered for a moment, then handed over
/// together. The drop point is not known (winit does not say where), so a drop plays.
fn dropped(path: PathBuf) {
    let first = DROPPED.with(|d| {
        let mut d = d.borrow_mut();
        d.push(path);
        d.len() == 1
    });
    if first {
        slint::Timer::single_shot(std::time::Duration::from_millis(60), || {
            let paths = DROPPED.with(|d| std::mem::take(&mut *d.borrow_mut()));
            if let Some(h) = HOOKS.with(Cell::get) {
                log::info!("{} file(s) dropped on the window", paths.len());
                (h.files)(paths, None);
            }
        });
    }
}

/// The pin actions' keys the window takes before the focused control (`shortcuts`).
pub fn set_pin_keys(keys: Vec<PinKey>) {
    PIN_KEYS.with(|k| *k.borrow_mut() = keys);
}

/// Settings > Keys is waiting for a key: the window takes none of its own.
pub fn set_capturing(on: bool) {
    CAPTURING.with(|c| c.set(on));
}

pub fn minimize() {
    with_window(|app, _| app.window().set_minimized(true));
}

pub fn toggle_maximize() {
    with_window(|app, w| app.window().set_maximized(!w.is_maximized()));
}

/// The same path as Alt+F4 or the desktop's close: a close request, which hides to the
/// tray (or quits, by the setting, or where there is no tray).
pub fn close() {
    with_window(|app, _| app.window().dispatch_event(slint::platform::WindowEvent::CloseRequested));
}

/// The compositor's window menu at a window point (logical pixels), as a right click on a
/// title bar shows it (Wayland; X11 has none).
pub fn system_menu(x: f32, y: f32) {
    with_window(|_, w| w.show_window_menu(winit::dpi::LogicalPosition::new(f64::from(x), f64::from(y))));
}

/// Bring the window back for a second launch or the media controls' Raise: show it (from
/// the tray too), unminimize it, and ask for the focus (the compositor may only mark it as
/// wanting attention: focus stealing prevention).
pub fn bring_back(app: &AppWindow, _hwnd: isize) {
    let _ = app.show();
    let quiet = super::devwindow::enabled();
    app.window().with_winit_window(|w| {
        w.set_minimized(false);
        if !quiet {
            w.focus_window();
        }
    });
}

/// Bench: a drop as the window would get it (the files and a point, physical pixels).
#[cfg(feature = "bench")]
pub fn simulate_drop(_hwnd: isize, paths: &[PathBuf], x: i32, y: i32) -> usize {
    if let Some(h) = HOOKS.with(Cell::get) {
        let (paths, files) = (paths.to_vec(), h.files);
        let n = paths.len();
        let _ = slint::invoke_from_event_loop(move || files(paths, Some((x, y))));
        return n;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners_resize_the_rest_is_the_page() {
        let at = |x, y| edge_at(x, y, 1440.0, 900.0, 1.0);
        assert_eq!(at(700.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at(700.0, 897.0), Some(ResizeDirection::South));
        assert_eq!(at(2.0, 400.0), Some(ResizeDirection::West));
        assert_eq!(at(1437.0, 400.0), Some(ResizeDirection::East));
        assert_eq!(at(10.0, 2.0), Some(ResizeDirection::NorthWest), "a corner reaches along the edge");
        assert_eq!(at(1437.0, 890.0), Some(ResizeDirection::SouthEast));
        assert_eq!(at(700.0, 400.0), None);
        assert_eq!(at(700.0, 8.0), None, "the caption strip below the band moves the window");
        // At 150 %: the band scales.
        assert_eq!(edge_at(700.0, 8.0, 2160.0, 1350.0, 1.5), Some(ResizeDirection::North));
    }

    #[test]
    fn the_maximize_button_is_the_middle_one() {
        // Buttons from the right: close [1394,1440), maximize [1348,1394), minimize.
        assert!(on_max_button(1370.0, 16.0, 1440.0, 1.0));
        assert!(!on_max_button(1400.0, 16.0, 1440.0, 1.0), "close");
        assert!(!on_max_button(1330.0, 16.0, 1440.0, 1.0), "minimize");
        assert!(!on_max_button(1370.0, 40.0, 1440.0, 1.0), "below it");
        assert!(on_max_button(2160.0 - 1.5 * 70.0, 20.0, 2160.0, 1.5));
    }

    #[test]
    fn keys_are_named_as_the_keymap_names_them() {
        use winit::keyboard::{Key, NamedKey};
        assert_eq!(virtual_key(&Key::Character("b".into())), Some(0x42));
        assert_eq!(virtual_key(&Key::Character("Q".into())), Some(0x51));
        assert_eq!(virtual_key(&Key::Character("7".into())), Some(0x37));
        assert_eq!(virtual_key(&Key::Named(NamedKey::F12)), Some(0x7B));
        assert_eq!(virtual_key(&Key::Named(NamedKey::ArrowLeft)), Some(0x25));
        assert_eq!(virtual_key(&Key::Character("/".into())), None, "punctuation differs by layout");
        assert_eq!(virtual_key(&Key::Character("é".into())), None);
    }
}
