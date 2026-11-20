//! The tray icon on Windows: Slint's `SystemTrayIcon` (the `Tray` component in app.slint)
//! for the icon, its tooltip and its menu, plus what Slint does not hand on: which button,
//! the double click, and the wheel.
//!
//! Slint's icon sends its mouse messages (`WM_APP + 1`, the mouse message in the low word of
//! lParam, the default notify-icon version) to a hidden message-only window of its own
//! (class `SlintSystemTrayWindow`, one per icon, on the UI thread). Slint only turns a left
//! button release into `clicked` and a right one into its menu. This subclasses that window
//! (found once the event loop has made it): the left release, the double click and the
//! middle release become `TrayEvent`s and do not reach Slint; the right button and the rest
//! go on to Slint's procedure, so the menu is Slint's as before. Until the window is found
//! (or if it never is) Slint's `clicked` stands in for the left release.
//!
//! The wheel: `Shell_NotifyIcon` sends no wheel messages. While the pointer is over the
//! icon (the icon's `WM_MOUSEMOVE`, and the cursor really inside the rectangle
//! `Shell_NotifyIconGetRect` gives), a low-level mouse hook (`WH_MOUSE_LL`) is put in; it
//! comes out as soon as the pointer moves outside that rectangle, when a check four times a
//! second finds it outside, or after `IDLE` with neither movement nor wheel over it. The
//! hook lives on a thread of its own that does nothing else (it waits in `GetMessageW`), so
//! a busy UI thread never delays the system's mouse input; its callback only compares the
//! point with the rectangle, posts a message and returns, never taking the wheel from
//! whatever is under the pointer. Where the shell gives no rectangle (the overflow
//! flyout closed, an old shell) there is no hook, and so no wheel: nothing else changes.
//! Nothing runs, and no hook exists, while the pointer is anywhere else.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_APP, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL};

use crate::platform::TrayEvent;

/// Slint's notify-icon callback message and icon id (`i-slint-core`,
/// `items/system_tray/windows.rs`: `WM_TRAYICON`, `TRAY_UID`).
const WM_TRAYICON: u32 = WM_APP + 1;
const TRAY_UID: u32 = 1;
/// The hook thread's wheel notch(es) for the UI thread: wParam is the delta (i16, sign-extended).
const WM_MP_WHEEL: u32 = WM_APP + 0x2A0;
/// To the hook thread: put the hook in / take it out.
const WM_MP_HOOK_ON: u32 = WM_APP + 0x2A1;
const WM_MP_HOOK_OFF: u32 = WM_APP + 0x2A2;
const SUBCLASS_ID: usize = 0x4D50_5452; // "MPTR"
/// The hook comes out after this long with no movement and no wheel over the icon.
const IDLE: Duration = Duration::from_secs(5);
/// How often the hook thread checks the pointer is still over the icon.
const CHECK_MS: u32 = 250;
/// How often the icon's rectangle is asked for again while the pointer moves over it.
const RECT_EVERY: Duration = Duration::from_millis(500);

/// The tray icon: the Slint component, and the search for its window.
pub struct Tray {
    ui: crate::Tray,
}

thread_local! {
    static ON: Cell<Option<fn(TrayEvent)>> = const { Cell::new(None) };
    static FIND: RefCell<Option<Timer>> = const { RefCell::new(None) };
    /// When the icon's rectangle was last asked for (the hook armed then).
    static RECT_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Slint's tray window, once subclassed (0 before).
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

impl Tray {
    /// Show the icon with `icon` and `tip`; `on` gets what happens on it, on the UI thread.
    pub fn new(icon: slint::Image, tip: &str, on: fn(TrayEvent)) -> Option<Tray> {
        let ui = crate::Tray::new().ok()?;
        ui.set_tray_icon(icon);
        ui.set_tip(tip.into());
        ui.on_toggle_play(move || on(TrayEvent::TogglePlay));
        ui.on_next(move || on(TrayEvent::Next));
        ui.on_previous(move || on(TrayEvent::Previous));
        ui.on_show_window(move || on(TrayEvent::ShowWindow));
        ui.on_about(move || on(TrayEvent::About));
        ui.on_quit(move || on(TrayEvent::Quit));
        // Only until the window is subclassed: from then on the left button never reaches Slint.
        ui.on_activated(move || on(TrayEvent::Click));
        let _ = ui.show();
        ON.with(|o| o.set(Some(on)));
        // Slint makes the icon (and its window) once the event loop runs.
        let tries = Cell::new(0u32);
        let find = Timer::default();
        find.start(TimerMode::Repeated, Duration::from_millis(20), move || {
            tries.set(tries.get() + 1);
            let found = subclass();
            if found || tries.get() >= 250 {
                if found {
                    log::info!("tray: clicks, double clicks, the middle button and the wheel are the app's");
                } else {
                    log::warn!("tray: Slint's tray window not found; a click only (no double click, middle button or wheel)");
                }
                FIND.with(|f| f.borrow_mut().take());
            }
        });
        FIND.with(|f| *f.borrow_mut() = Some(find));
        Some(Tray { ui })
    }

    pub fn set_icon(&self, img: slint::Image) {
        self.ui.set_tray_icon(img);
    }

    pub fn set_tooltip(&self, tip: &str) {
        self.ui.set_tip(tip.into());
    }

    /// The menu's Play/Pause item.
    pub fn set_playing(&self, playing: bool) {
        self.ui.set_playing(playing);
    }

    /// The bench: hand the icon what the mouse would, as the shell sends it (to the
    /// subclassed window, inside the app; the real pointer and input are never touched).
    /// False when there is no subclassed window to send to.
    #[cfg(feature = "bench")]
    pub fn simulate(&self, what: crate::platform::TrayPointer) -> bool {
        use crate::platform::TrayPointer as P;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_LBUTTONDOWN, WM_MBUTTONDOWN};
        let hwnd = TRAY_HWND.load(Ordering::Acquire) as HWND;
        if hwnd.is_null() {
            return false;
        }
        let icon = |events: &[u32]| {
            for &e in events {
                // SAFETY: our own window on this thread; the message carries no pointers.
                unsafe { SendMessageW(hwnd, WM_TRAYICON, TRAY_UID as WPARAM, e as LPARAM) };
            }
        };
        match what {
            P::Click => icon(&[WM_LBUTTONDOWN, WM_LBUTTONUP]),
            // What the shell sends for a double click.
            P::DoubleClick => icon(&[WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP]),
            P::Middle => icon(&[WM_MBUTTONDOWN, WM_MBUTTONUP]),
            P::Hover => icon(&[WM_MOUSEMOVE]),
            // What the hook thread posts for the wheel.
            // SAFETY: as above.
            P::Wheel(d) => unsafe {
                SendMessageW(hwnd, WM_MP_WHEEL, d as isize as WPARAM, 0);
            },
        }
        true
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        release();
        FIND.with(|f| f.borrow_mut().take());
    }
}

/// The system's double-click time (Control Panel > Mouse).
pub fn double_click_time() -> Duration {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    // SAFETY: a plain query.
    let ms = unsafe { GetDoubleClickTime() };
    Duration::from_millis(u64::from(ms.clamp(100, 5000)))
}

/// Take the wheel hook out now, from any thread (the exit, the crash path). Cheap when
/// there is none.
pub fn release() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, UnhookWindowsHookEx};
    let h = HOOK.swap(0, Ordering::AcqRel);
    if h != 0 {
        // SAFETY: a hook this process put in, taken out once (the swap).
        unsafe { UnhookWindowsHookEx(h as _) };
        log::info!("tray: wheel hook out");
    }
    let t = THREAD.load(Ordering::Acquire);
    if t != 0 {
        // SAFETY: a thread message without pointers; its timer stops too.
        unsafe { PostThreadMessageW(t, WM_MP_HOOK_OFF, 0, 0) };
    }
    ARMED.store(false, Ordering::Release);
}

/// Is the wheel hook in (the bench)?
#[cfg(feature = "bench")]
pub fn wheel_hooked() -> bool {
    HOOK.load(Ordering::Acquire) != 0
}

/// Is the pointer over the icon now, by the shell's rectangle (`None`: no rectangle)? The bench.
#[cfg(feature = "bench")]
pub fn pointer_over_icon() -> Option<bool> {
    let hwnd = TRAY_HWND.load(Ordering::Acquire) as HWND;
    let rc = icon_rect(hwnd)?;
    let pt = cursor()?;
    Some(inside(rect4(&rc), pt.x, pt.y))
}

/// Find Slint's tray window (this thread's) and subclass it.
fn subclass() -> bool {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE};
    let class: Vec<u16> = "SlintSystemTrayWindow\0".encode_utf16().collect();
    // SAFETY: queries with a NUL-terminated class name; the subclass is on our own thread's
    // window, which the procedure below only forwards to `DefSubclassProc` or handles.
    unsafe {
        let me = GetCurrentThreadId();
        let mut h: HWND = std::ptr::null_mut();
        loop {
            h = FindWindowExW(HWND_MESSAGE, h, class.as_ptr(), std::ptr::null());
            if h.is_null() {
                return false;
            }
            if GetWindowThreadProcessId(h, std::ptr::null_mut()) == me {
                break;
            }
        }
        if SetWindowSubclass(h, Some(icon_proc), SUBCLASS_ID, 0) == 0 {
            return false;
        }
        TRAY_HWND.store(h as isize, Ordering::Release);
    }
    true
}

fn emit(e: TrayEvent) {
    if let Some(f) = ON.with(Cell::get) {
        // After the message: the app's state may be borrowed where a modal loop runs this.
        let _ = slint::invoke_from_event_loop(move || f(e));
    }
}

unsafe extern "system" fn icon_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;
    match msg {
        WM_TRAYICON => match (lparam as u32) & 0xffff {
            WM_LBUTTONUP => {
                emit(TrayEvent::Click);
                return 0;
            }
            WM_LBUTTONDBLCLK => {
                emit(TrayEvent::DoubleClick);
                return 0;
            }
            WM_MBUTTONUP => {
                emit(TrayEvent::Middle);
                return 0;
            }
            WM_MOUSEMOVE => hover(hwnd),
            _ => {}
        },
        WM_MP_WHEEL => {
            emit(TrayEvent::Wheel(wparam as isize as i32));
            return 0;
        }
        WM_NCDESTROY => {
            release();
            TRAY_HWND.store(0, Ordering::Release);
            // SAFETY: removing our own subclass from the window being destroyed.
            unsafe { RemoveWindowSubclass(hwnd, Some(icon_proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    // SAFETY: the message as received, to the next procedure (Slint's).
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

// ---- the wheel hook -------------------------------------------------------------------

/// The icon's rectangle (screen, physical pixels) the hook compares with.
static LEFT: AtomicI32 = AtomicI32::new(0);
static TOP: AtomicI32 = AtomicI32::new(0);
static RIGHT: AtomicI32 = AtomicI32::new(0);
static BOTTOM: AtomicI32 = AtomicI32::new(0);
/// Where the wheel goes (Slint's tray window).
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// The hook while it is in (0: none).
static HOOK: AtomicIsize = AtomicIsize::new(0);
/// The hook thread (0: not started).
static THREAD: AtomicU32 = AtomicU32::new(0);
/// The hook was asked for and not yet taken out.
static ARMED: AtomicBool = AtomicBool::new(false);
/// The hook asked to come out (the pointer left), so it asks once.
static LEAVING: AtomicBool = AtomicBool::new(false);
/// The last movement or wheel over the icon, in ms since `base()`.
static ACTIVE_MS: AtomicU64 = AtomicU64::new(0);

fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    base().elapsed().as_millis() as u64
}

/// Is (x, y) inside the rectangle (left, top, right, bottom; right and bottom outside)?
pub(crate) fn inside(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// The wheel's delta in a `WM_MOUSEWHEEL`'s `mouseData` (its high word, signed).
pub(crate) fn wheel_delta(mouse_data: u32) -> i32 {
    i32::from((mouse_data >> 16) as u16 as i16)
}

/// Should the hook come out: the pointer outside the icon, or nothing over it for `IDLE`?
pub(crate) fn hook_done(r: [i32; 4], pointer: Option<(i32, i32)>, idle: Duration) -> bool {
    pointer.is_none_or(|(x, y)| !inside(r, x, y)) || idle >= IDLE
}

fn rect4(rc: &RECT) -> [i32; 4] {
    [rc.left, rc.top, rc.right, rc.bottom]
}

fn stored_rect() -> [i32; 4] {
    [LEFT.load(Ordering::Relaxed), TOP.load(Ordering::Relaxed), RIGHT.load(Ordering::Relaxed), BOTTOM.load(Ordering::Relaxed)]
}

fn cursor() -> Option<POINT> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: `pt` is valid for writes.
    (unsafe { GetCursorPos(&mut pt) } != 0).then_some(pt)
}

/// The icon's rectangle, where the shell gives one.
fn icon_rect(hwnd: HWND) -> Option<RECT> {
    use windows_sys::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};
    if hwnd.is_null() {
        return None;
    }
    let id = NOTIFYICONIDENTIFIER { cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32, hWnd: hwnd, uID: TRAY_UID, ..Default::default() };
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: both pointers to locals.
    let hr = unsafe { Shell_NotifyIconGetRect(&id, &mut rc) };
    (hr >= 0 && rc.right > rc.left && rc.bottom > rc.top).then_some(rc)
}

/// The pointer moved over the icon (the shell says so): note it, and put the hook in if
/// the cursor really is inside the icon's rectangle.
fn hover(hwnd: HWND) {
    let armed = ARMED.load(Ordering::Acquire);
    if armed && RECT_AT.with(Cell::get).is_some_and(|at| at.elapsed() < RECT_EVERY) {
        ACTIVE_MS.store(now_ms(), Ordering::Relaxed);
        return;
    }
    // The message may be late: only where the cursor is now counts.
    let Some(rc) = icon_rect(hwnd) else { return };
    let Some(pt) = cursor() else { return };
    let r = rect4(&rc);
    if !inside(r, pt.x, pt.y) {
        return;
    }
    RECT_AT.with(|a| a.set(Some(Instant::now())));
    LEFT.store(r[0], Ordering::Relaxed);
    TOP.store(r[1], Ordering::Relaxed);
    RIGHT.store(r[2], Ordering::Relaxed);
    BOTTOM.store(r[3], Ordering::Relaxed);
    TARGET.store(hwnd as isize, Ordering::Release);
    ACTIVE_MS.store(now_ms(), Ordering::Relaxed);
    if armed {
        return;
    }
    let Some(thread) = hook_thread() else { return };
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    ARMED.store(true, Ordering::Release);
    LEAVING.store(false, Ordering::Release);
    // SAFETY: a thread message without pointers.
    if unsafe { PostThreadMessageW(thread, WM_MP_HOOK_ON, 0, 0) } == 0 {
        ARMED.store(false, Ordering::Release);
    }
}

/// The hook thread's id, started the first time the pointer comes over the icon, and only
/// once: one slow to start (a loaded machine) is waited for at the next hover, not started again.
fn hook_thread() -> Option<u32> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    let t = THREAD.load(Ordering::Acquire);
    if t != 0 || STARTED.swap(true, Ordering::AcqRel) {
        return (t != 0).then_some(t);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if std::thread::Builder::new().name("mp-tray-wheel".into()).spawn(move || hook_loop(&tx)).is_err() {
        STARTED.store(false, Ordering::Release);
        return None;
    }
    rx.recv_timeout(Duration::from_secs(1)).ok()
}

/// The hook thread: waits for messages; puts the hook in and takes it out, and checks
/// four times a second while it is in.
fn hook_loop(ready: &std::sync::mpsc::Sender<u32>) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, KillTimer, MSG, PM_NOREMOVE, PeekMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_TIMER,
    };
    // SAFETY (whole function): Win32 calls on this thread's own queue, hook and timer,
    // with pointers to locals.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // The queue exists before anyone posts to it.
        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        // Known from here on, also when the hover that started it stopped waiting.
        THREAD.store(GetCurrentThreadId(), Ordering::Release);
        let _ = ready.send(GetCurrentThreadId());
        let mut timer = 0usize;
        let take_out = |timer: &mut usize| {
            let h = HOOK.swap(0, Ordering::AcqRel);
            if h != 0 {
                UnhookWindowsHookEx(h as _);
            }
            if *timer != 0 {
                KillTimer(std::ptr::null_mut(), *timer);
                *timer = 0;
            }
            ARMED.store(false, Ordering::Release);
        };
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            match msg.message {
                WM_MP_HOOK_ON if ARMED.load(Ordering::Acquire) && HOOK.load(Ordering::Acquire) == 0 => {
                    let h = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), GetModuleHandleW(std::ptr::null()), 0);
                    if h.is_null() {
                        ARMED.store(false, Ordering::Release);
                        continue;
                    }
                    HOOK.store(h as isize, Ordering::Release);
                    if timer == 0 {
                        timer = SetTimer(std::ptr::null_mut(), 0, CHECK_MS, None);
                    }
                }
                WM_MP_HOOK_OFF => take_out(&mut timer),
                WM_TIMER if msg.hwnd.is_null() && msg.wParam == timer => {
                    let idle = Duration::from_millis(now_ms().saturating_sub(ACTIVE_MS.load(Ordering::Relaxed)));
                    if hook_done(stored_rect(), cursor().map(|p| (p.x, p.y)), idle) {
                        take_out(&mut timer);
                    }
                }
                _ => {}
            }
        }
        take_out(&mut timer);
    }
}

/// The low-level mouse hook: compare the point, maybe post, pass the input on. Nothing
/// else (it runs for every mouse event in the session while it is in).
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, HC_ACTION, MSLLHOOKSTRUCT, PostMessageW, PostThreadMessageW};
    if code == HC_ACTION as i32 && lparam != 0 {
        // SAFETY: for HC_ACTION, lParam points to the event's MSLLHOOKSTRUCT.
        let m = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let over = inside(stored_rect(), m.pt.x, m.pt.y);
        match wparam as u32 {
            WM_MOUSEWHEEL if over => {
                ACTIVE_MS.store(now_ms(), Ordering::Relaxed);
                let d = wheel_delta(m.mouseData);
                // SAFETY: a message without pointers to a window that may be gone (then it fails).
                unsafe { PostMessageW(TARGET.load(Ordering::Acquire) as HWND, WM_MP_WHEEL, d as isize as WPARAM, 0) };
            }
            WM_MOUSEMOVE if over => ACTIVE_MS.store(now_ms(), Ordering::Relaxed),
            WM_MOUSEMOVE if !LEAVING.swap(true, Ordering::AcqRel) => {
                // SAFETY: a thread message without pointers.
                unsafe { PostThreadMessageW(THREAD.load(Ordering::Acquire), WM_MP_HOOK_OFF, 0, 0) };
            }
            _ => {}
        }
    }
    // SAFETY: the event as received, on to the next hook.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_rectangle_holds_its_left_and_top_edges_only() {
        let r = [100, 200, 124, 224];
        assert!(inside(r, 100, 200));
        assert!(inside(r, 123, 223));
        assert!(!inside(r, 124, 210), "right edge is outside");
        assert!(!inside(r, 110, 224), "bottom edge is outside");
        assert!(!inside(r, 99, 210));
        assert!(!inside([0, 0, 0, 0], 0, 0), "an empty rectangle holds nothing");
    }

    #[test]
    fn the_wheel_delta_is_the_signed_high_word() {
        assert_eq!(wheel_delta(120 << 16), 120);
        assert_eq!(wheel_delta(((-120i16) as u16 as u32) << 16), -120);
        assert_eq!(wheel_delta((((-30i16) as u16 as u32) << 16) | 0xffff), -30, "the low word is not part of it");
    }

    #[test]
    fn the_hook_comes_out_when_the_pointer_leaves_or_rests() {
        let r = [100, 200, 124, 224];
        assert!(!hook_done(r, Some((110, 210)), Duration::from_secs(1)));
        assert!(hook_done(r, Some((130, 210)), Duration::ZERO), "outside");
        assert!(hook_done(r, None, Duration::ZERO), "no cursor position: out");
        assert!(hook_done(r, Some((110, 210)), IDLE), "still over it, but resting");
    }
}
