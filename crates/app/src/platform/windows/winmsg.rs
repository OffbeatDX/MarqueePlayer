//! Back/forward from the mouse side buttons and browser keys, caught at the Win32 level.
//!
//! A side button reaches a window in one of three ways, depending on the mouse and its
//! driver software: `WM_XBUTTONDOWN/UP` (XBUTTON1 = back, XBUTTON2 = forward),
//! `WM_APPCOMMAND` with `APPCOMMAND_BROWSER_BACKWARD/FORWARD` (what vendor software often
//! sends instead, and what `DefWindowProc` makes of an unhandled X button), or a
//! `VK_BROWSER_BACK/FORWARD` key, which `DefWindowProc` also turns into an APPCOMMAND.
//! winit only turns the first into a mouse event, so this subclasses the window and
//! handles X buttons and APPCOMMANDs. Every one is logged, and the last one is
//! shown on screen with Ctrl+Shift+D, so a mouse that sends something else can be told
//! apart from one that sends nothing.

use std::cell::Cell;

pub use crate::platform::{NavInput, PinKey, PinSide};

const WM_APPCOMMAND: u32 = 0x0319;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_XBUTTONDBLCLK: u32 = 0x020D;

/// Decode a window message. Returns a description for the log and readout, and the action
/// (`None` for messages this module ignores).
/// Pure, so it can be tested without a window.
pub fn classify(msg: u32, wparam: usize, lparam: isize) -> Option<(String, Option<NavInput>)> {
    let hi = |v: usize| ((v >> 16) & 0xffff) as u16;
    match msg {
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => {
            let b = hi(wparam);
            Some((format!("XBUTTON{b} down (acts on release)"), None))
        }
        WM_XBUTTONUP => {
            let b = hi(wparam);
            let nav = match b {
                1 => Some(NavInput::Back),
                2 => Some(NavInput::Forward),
                _ => None,
            };
            Some((format!("XBUTTON{b} up"), nav))
        }
        WM_APPCOMMAND => {
            let cmd = hi(lparam as usize) & !0xF000;
            let device = match hi(lparam as usize) & 0xF000 {
                0x8000 => "mouse",
                0x0000 => "keyboard",
                _ => "other device",
            };
            let nav = match cmd {
                1 => Some(NavInput::Back),
                2 => Some(NavInput::Forward),
                _ => return None,
            };
            Some((format!("APPCOMMAND {cmd} from {device}"), nav))
        }
        // VK_BROWSER_BACK/FORWARD keys are left to DefWindowProc, which turns them into
        // the keyboard APPCOMMAND above; handling both would navigate twice.
        _ => None,
    }
}

/// The pin actions' keys (Ctrl+B / Ctrl+Q unless remapped), taken at the window before
/// any focused element (the search field included) sees them. Only Ctrl or Alt chords
/// are put here (`shortcuts::sync_window_keys`): they type nothing. Key repeat is
/// ignored, so holding the key does not flicker.
pub fn pin_shortcut(msg: u32, wparam: usize, lparam: isize, ctrl: bool, shift: bool, alt: bool, keys: &[PinKey]) -> Option<PinSide> {
    const WM_KEYDOWN: u32 = 0x0100;
    // Alt chords arrive as WM_SYSKEYDOWN.
    const WM_SYSKEYDOWN: u32 = 0x0104;
    let repeat = (lparam >> 30) & 1 == 1;
    if (msg != WM_KEYDOWN && msg != WM_SYSKEYDOWN) || repeat {
        return None;
    }
    keys.iter().find(|k| usize::from(k.0) == wparam && (k.1, k.2, k.3) == (ctrl, shift, alt)).map(|k| k.4)
}

/// Settings > Keys is waiting for a key: the window takes none of its own, so the pin
/// keys can be captured too.
pub fn set_capturing(on: bool) {
    CAPTURING.with(|c| c.set(on));
}

/// The pin keys the window takes (see `pin_shortcut`).
pub fn set_pin_keys(keys: Vec<PinKey>) {
    PIN_KEYS.with(|k| *k.borrow_mut() = keys);
}

/// `WM_COPYDATA`'s `dwData` for audio paths from a second launch (`instance`).
pub const COPYDATA_PATHS: usize = 0x4D50_4631;

/// Paths in a `WM_COPYDATA` payload: UTF-16, one per line.
pub fn encode_paths(paths: &[std::path::PathBuf]) -> Vec<u16> {
    paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n").encode_utf16().collect()
}

pub fn decode_paths(data: &[u16]) -> Vec<std::path::PathBuf> {
    String::from_utf16_lossy(data).split('\n').filter(|l| !l.trim().is_empty()).map(std::path::PathBuf::from).collect()
}

/// Paths opened from outside: a second launch's (`None`) or dropped on the window (with
/// the drop point, physical pixels in the window).
pub type FilesHandler = fn(Vec<std::path::PathBuf>, Option<(i32, i32)>);
/// Called with a key's text and modifiers, or a mouse side button (`NavInput`).
type NavHandler = fn(String, Option<NavInput>);

thread_local! {
    static FILES_HANDLER: Cell<Option<FilesHandler>> = const { Cell::new(None) };
    static HANDLER: Cell<Option<NavHandler>> = const { Cell::new(None) };
    static PIN_HANDLER: Cell<Option<fn(PinSide)>> = const { Cell::new(None) };
    static SHOW_HANDLER: Cell<Option<fn(isize)>> = const { Cell::new(None) };
    static END_HANDLER: Cell<Option<fn(bool)>> = const { Cell::new(None) };
    static PIN_KEYS: std::cell::RefCell<Vec<PinKey>> =
        std::cell::RefCell::new(vec![(0x42, true, false, false, PinSide::Left), (0x51, true, false, false, PinSide::Right)]);
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
}

/// Where a second launch's "show yourself" (`instance`) goes, with the window handle.
pub fn on_show_request(handler: fn(isize)) {
    SHOW_HANDLER.with(|h| h.set(Some(handler)));
}

/// What to do when the session ends or an installer closes the app (`WM_ENDSESSION`).
pub fn on_end_session(handler: fn(bool)) {
    END_HANDLER.with(|h| h.set(Some(handler)));
}

/// `WM_ENDSESSION`'s meaning: `None` when the session goes on after all (another program
/// refused), else whether only the app is asked to close (`ENDSESSION_CLOSEAPP`: Restart
/// Manager, as installers use it) rather than the whole session ending.
pub fn end_session(wparam: usize, lparam: isize) -> Option<bool> {
    const ENDSESSION_CLOSEAPP: isize = 0x1;
    (wparam != 0).then_some(lparam & ENDSESSION_CLOSEAPP != 0)
}

/// Where paths from outside go (set before `install`).
pub fn on_files(handler: FilesHandler) {
    FILES_HANDLER.with(|h| h.set(Some(handler)));
}

/// Where the pin shortcuts go (set before `install`).
pub fn on_pin_shortcut(handler: fn(PinSide)) {
    PIN_HANDLER.with(|h| h.set(Some(handler)));
}

/// Subclass the window. `handler` runs on the UI thread, deferred out of the window
/// procedure, with the description and the action.
pub fn install(hwnd: *mut std::ffi::c_void, handler: fn(String, Option<NavInput>)) {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    HANDLER.with(|h| h.set(Some(handler)));
    // SAFETY: our own top-level window, on its UI thread; the procedure is a plain fn.
    let ok = unsafe { SetWindowSubclass(hwnd as _, Some(subclass_proc), 0x4D50, 0) };
    if ok == 0 {
        log::warn!("side buttons: SetWindowSubclass failed");
    }
    // Files dropped from Explorer arrive as WM_DROPFILES (winit's OLE drop target is
    // turned off: it reports no drop point, and Slint does not pass its drops on).
    // SAFETY: our own window.
    unsafe { windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd as _, 1) };
}

unsafe extern "system" fn subclass_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    // SAFETY: the message's own arguments, from the window procedure.
    if let Some(r) = unsafe { super::frame::handle(hwnd, msg, wparam, lparam) } {
        return r;
    }
    let show = super::instance::show_message();
    if show != 0 && msg == show {
        if let Some(handler) = SHOW_HANDLER.with(Cell::get) {
            let hwnd = hwnd as isize;
            let _ = slint::invoke_from_event_loop(move || handler(hwnd));
        }
        return 0;
    }
    const WM_ENDSESSION: u32 = 0x0016;
    if msg == WM_ENDSESSION {
        // Synchronously: after this returns, an ending session may end the process.
        if let (Some(close_app), Some(handler)) = (end_session(wparam, lparam), END_HANDLER.with(Cell::get)) {
            handler(close_app);
        }
        return 0;
    }
    const WM_COPYDATA: u32 = 0x004A;
    const WM_DROPFILES: u32 = 0x0233;
    if msg == WM_COPYDATA && lparam != 0 {
        use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
        // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT valid for this call; its data
        // is copied out before returning.
        let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        if cds.dwData == COPYDATA_PATHS && !cds.lpData.is_null() {
            let data = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
            let paths = decode_paths(data);
            if let Some(handler) = FILES_HANDLER.with(Cell::get) {
                let _ = slint::invoke_from_event_loop(move || handler(paths, None));
            }
            return 1;
        }
    }
    if msg == WM_DROPFILES {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
        let drop = wparam as HDROP;
        let mut paths = Vec::new();
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: the HDROP of this message, read and then released once.
        unsafe {
            let n = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
                paths.push(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
            }
            DragQueryPoint(drop, &mut pt);
            DragFinish(drop);
        }
        if let Some(handler) = FILES_HANDLER.with(Cell::get) {
            let at = (pt.x, pt.y);
            let _ = slint::invoke_from_event_loop(move || handler(paths, Some(at)));
        }
        return 0;
    }
    if (msg == 0x0100 || msg == 0x0104) && !CAPTURING.with(Cell::get) {
        // SAFETY: plain key-state queries on the UI thread.
        let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
        let side = PIN_KEYS.with(|k| pin_shortcut(msg, wparam, lparam, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU), &k.borrow()));
        if let Some(side) = side {
            if let Some(handler) = PIN_HANDLER.with(Cell::get) {
                let _ = slint::invoke_from_event_loop(move || handler(side));
            }
            return 0;
        }
    }
    if let Some((what, nav)) = classify(msg, wparam, lparam) {
        if let Some(handler) = HANDLER.with(Cell::get) {
            // Out of the window procedure: navigation touches app state and the UI.
            let _ = slint::invoke_from_event_loop(move || handler(what, nav));
        }
        // Consumed. TRUE for X buttons (so DefWindowProc does not also send an
        // APPCOMMAND for the same press) and for APPCOMMANDs (handled).
        return 1;
    }
    // SAFETY: forwarding to the next procedure in the chain with the original arguments.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_ending_is_told_apart_from_an_app_close() {
        assert_eq!(end_session(0, 0), None, "the session goes on");
        assert_eq!(end_session(1, 0), Some(false), "shutdown or sign-out");
        assert_eq!(end_session(1, 0x1), Some(true), "Restart Manager closes the app");
        assert_eq!(end_session(1, 0x8000_0000u32 as i32 as isize), Some(false), "ENDSESSION_LOGOFF");
    }

    #[test]
    fn x_buttons_act_on_release() {
        assert_eq!(classify(WM_XBUTTONDOWN, 1 << 16, 0).unwrap().1, None);
        assert_eq!(classify(WM_XBUTTONUP, 1 << 16, 0).unwrap().1, Some(NavInput::Back));
        assert_eq!(classify(WM_XBUTTONUP, 2 << 16, 0).unwrap().1, Some(NavInput::Forward));
    }

    #[test]
    fn appcommands_from_mouse_software() {
        // HIWORD(lParam) = FAPPCOMMAND_MOUSE (0x8000) | APPCOMMAND_BROWSER_BACKWARD (1).
        let (what, nav) = classify(WM_APPCOMMAND, 0, (0x8001isize) << 16).unwrap();
        assert_eq!(nav, Some(NavInput::Back));
        assert!(what.contains("mouse"));
        assert_eq!(classify(WM_APPCOMMAND, 0, 2isize << 16).unwrap().1, Some(NavInput::Forward));
        // Other app commands (volume, media) are left alone.
        assert!(classify(WM_APPCOMMAND, 0, 8isize << 16).is_none());
    }

    #[test]
    fn pin_shortcuts() {
        let keys = PIN_KEYS.with(|k| k.borrow().clone());
        let pin = |msg, vk, lp, c, s, a| pin_shortcut(msg, vk, lp, c, s, a, &keys);
        assert_eq!(pin(0x0100, 0x42, 0, true, false, false), Some(PinSide::Left));
        assert_eq!(pin(0x0100, 0x51, 0, true, false, false), Some(PinSide::Right));
        assert_eq!(pin(0x0100, 0x42, 0, false, false, false), None, "plain B types");
        assert_eq!(pin(0x0100, 0x42, 0, true, true, false), None, "Ctrl+Shift+B is something else");
        assert_eq!(pin(0x0100, 0x42, 1 << 30, true, false, false), None, "auto-repeat");
        assert_eq!(pin(0x0101, 0x42, 0, true, false, false), None, "key up");
        // Remapped: Alt+P for the queue (a WM_SYSKEYDOWN), Ctrl+B no longer anything.
        let keys = vec![(0x50, false, false, true, PinSide::Right)];
        assert_eq!(pin_shortcut(0x0104, 0x50, 0, false, false, true, &keys), Some(PinSide::Right));
        assert_eq!(pin_shortcut(0x0100, 0x42, 0, true, false, false, &keys), None);
    }

    #[test]
    fn paths_survive_the_copydata_payload() {
        let p = vec![std::path::PathBuf::from(r"C:\Music\Ünï code\01 a.flac"), std::path::PathBuf::from(r"D:\x\b.opus")];
        assert_eq!(decode_paths(&encode_paths(&p)), p);
    }

    #[test]
    fn browser_keys_pass_through_to_become_appcommands() {
        // WM_KEYDOWN with VK_BROWSER_BACK: not handled here (no double navigation).
        assert!(classify(0x0100, 0xA6, 0).is_none());
    }
}
