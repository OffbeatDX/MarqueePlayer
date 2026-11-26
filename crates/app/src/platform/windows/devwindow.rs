//! `MP_WINDOW=secondary`, for development and test runs: the window opens maximized on a
//! monitor other than the primary one and never takes the foreground or the keyboard
//! focus, so a test run does not disturb someone working on the main monitor.
//!
//! How:
//! - The winit window attributes (set before the window exists) place it on the first
//!   non-primary monitor and create it inactive (`with_active(false)`), so winit shows it
//!   with `SW_SHOWNOACTIVATE`, the first time and every later time (the tray's Show).
//! - `ShowWindow(SW_MAXIMIZE)`, which winit uses to maximize (and again on every show of
//!   a maximized window), activates. Refusing that activation once begun is too late:
//!   Windows has already taken the foreground from the window the user is in, and leaves
//!   none (seen in testing). So a thread-local CBT hook on the UI thread cancels every
//!   maximize request of this thread's windows (`HCBT_MINMAX`) before it starts, and
//!   maximizes instead by hand: the `WS_MAXIMIZE` style and a non-activating
//!   `SetWindowPos` to the monitor's maximized rectangle (`maximize_quietly`). Windows
//!   and winit then see an ordinary maximized window (`IsZoomed`, `SIZE_MAXIMIZED`).
//! - The same hook refuses any activation that is not a mouse click into the window
//!   (`HCBT_ACTIVATE`), as a backstop. Clicking into the window still works; Alt+Tab
//!   and the taskbar do not activate it.
//!
//! With only one monitor, placement is left to Windows (logged), and activation is
//! still refused.

use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True when `MP_WINDOW=secondary` (read once, at start).
pub fn enabled() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Read the variable and, when set, install the hook on this (the UI) thread. Call
/// before the window is created.
pub fn init() {
    let on = std::env::var("MP_WINDOW").is_ok_and(|v| v.eq_ignore_ascii_case("secondary"));
    ACTIVE.store(on, Ordering::Relaxed);
    if on {
        log::info!("MP_WINDOW=secondary: secondary monitor, maximized, never activated");
        install_hook();
    }
}

/// The work area (left, top, right, bottom; physical pixels, virtual-screen
/// coordinates) of the first monitor that is not the primary one.
pub fn secondary_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    const MONITORINFOF_PRIMARY: u32 = 1;
    unsafe extern "system" fn each(mon: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> i32 {
        // SAFETY: `data` is the Vec passed below, alive for the enumeration; `mi` is local.
        unsafe {
            let out = &mut *(data as *mut Vec<(bool, RECT)>);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                out.push((mi.dwFlags & MONITORINFOF_PRIMARY != 0, mi.rcWork));
            }
        }
        1
    }
    let mut monitors: Vec<(bool, RECT)> = Vec::new();
    // SAFETY: plain enumeration with a callback that only writes into `monitors`.
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut monitors as *mut _ as LPARAM);
    }
    log::info!("MP_WINDOW: {} monitor(s)", monitors.len());
    monitors.into_iter().find(|(primary, _)| !primary).map(|(_, r)| (r.left, r.top, r.right, r.bottom))
}

/// The backend for a test window: as it is (only macOS has to stop the application being
/// activated at launch; here an inactive window is enough).
pub fn backend(s: slint::BackendSelector) -> slint::BackendSelector {
    s
}

/// The window attributes for `MP_WINDOW=secondary` (the winit attributes hook; runs
/// when the window is created, after the event loop made the process DPI aware, so the
/// monitor rectangles are physical pixels).
pub fn attributes(
    attrs: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    let attrs = super::window::attributes(attrs).with_active(false);
    match secondary_work_area() {
        Some((left, top, _, _)) => {
            log::info!("MP_WINDOW: placing the window on the monitor at ({left}, {top})");
            attrs.with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(left + 40, top + 40))
        }
        None => {
            log::info!("MP_WINDOW: only one monitor; normal placement (still not activated)");
            attrs
        }
    }
}

/// Maximize the app's window where it is (the secondary monitor), once it exists.
pub fn maximize(hwnd: *mut std::ffi::c_void) {
    if enabled() && secondary_work_area().is_some() {
        log::info!("MP_WINDOW: maximizing");
        maximize_quietly(hwnd as isize);
    }
}

/// Maximize without `ShowWindow`, so without any activation: remember the restored
/// placement, set `WS_MAXIMIZE`, and size the window to its monitor's work area plus
/// the frame (where Windows puts a maximized window; frame.rs insets the client).
fn maximize_quietly(hwnd: isize) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowPlacement, IsWindowVisible, IsZoomed, SM_CXFRAME, SM_CXPADDEDBORDER,
        SM_CYFRAME, SW_HIDE, SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowLongPtrW,
        SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT, WS_MAXIMIZE,
    };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: our own window, on its UI thread; pointers into locals.
    unsafe {
        if IsZoomed(hwnd) != 0 {
            return;
        }
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return;
        }
        // The restored placement, kept for Restore (in its current visibility).
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut wp) != 0 {
            wp.flags = 0;
            wp.showCmd = if IsWindowVisible(hwnd) != 0 { SW_SHOWNA } else { SW_HIDE } as u32;
            SetWindowPlacement(hwnd, &wp);
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_MAXIMIZE as isize);
        let dpi = GetDpiForWindow(hwnd);
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad;
        let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad;
        let w = mi.rcWork;
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            w.left - fx,
            w.top - fy,
            w.right - w.left + 2 * fx,
            w.bottom - w.top + 2 * fy,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
        );
    }
}

#[cfg(feature = "bench")]
/// Bench only (`MP_BENCH=settings`): un-maximize and size the window to `w` x `h`
/// physical pixels at the top left of its monitor's work area, without `ShowWindow`, so
/// without any activation (as `maximize_quietly`). `maximize` puts it back.
pub fn resize_quietly(hwnd: isize, w: i32, h: i32) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
        WS_MAXIMIZE,
    };
    if !enabled() {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: our own window, on its UI thread; pointers into locals.
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_MAXIMIZE as isize));
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            mi.rcWork.left,
            mi.rcWork.top,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
        );
    }
}

fn install_hook() {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_CBT};
    // SAFETY: a hook on our own thread only (no module handle needed); the procedure is
    // a plain fn that lives as long as the process. Never unhooked: it is for the
    // whole run.
    let hook = unsafe { SetWindowsHookExW(WH_CBT, Some(cbt), std::ptr::null_mut(), GetCurrentThreadId()) };
    if hook.is_null() {
        log::warn!("MP_WINDOW: SetWindowsHookExW failed; the window may take the focus");
    }
}

unsafe extern "system" fn cbt(code: i32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CBTACTIVATESTRUCT, CallNextHookEx, HCBT_ACTIVATE, HCBT_MINMAX, SW_MAXIMIZE, SW_SHOWMAXIMIZED,
    };
    if code == HCBT_MINMAX as i32 {
        let cmd = (lparam & 0xffff) as i32;
        if cmd == SW_MAXIMIZE || cmd == SW_SHOWMAXIMIZED {
            // Cancelled before it starts (nothing is activated); done quietly instead,
            // out of the hook.
            let hwnd = wparam as isize;
            let _ = slint::invoke_from_event_loop(move || maximize_quietly(hwnd));
            return 1;
        }
    }
    if code == HCBT_ACTIVATE as i32 && lparam != 0 {
        // SAFETY: for HCBT_ACTIVATE, lParam points to a CBTACTIVATESTRUCT.
        let mouse = unsafe { (*(lparam as *const CBTACTIVATESTRUCT)).fMouse } != 0;
        if !mouse {
            log::info!("MP_WINDOW: refused activation of window {wparam:#x}");
            return 1;
        }
    }
    // SAFETY: passing the hook's own arguments on.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(feature = "bench")]
/// Bench only: the smallest client a drag of the window's border leaves, physical pixels, as
/// the window answers `WM_GETMINMAXINFO` (winit's minimum, frame.rs's caption taken off), the
/// frame's 16 x 8 taken off as `resize_client` adds it. A message to our own window, no input.
pub fn min_client(hwnd: isize) -> Option<(i32, i32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, SendMessageW, WM_GETMINMAXINFO};
    if hwnd == 0 {
        return None;
    }
    // SAFETY: our own window, on its UI thread; the procedure fills the struct in place.
    unsafe {
        let mut mmi: MINMAXINFO = std::mem::zeroed();
        SendMessageW(hwnd as _, WM_GETMINMAXINFO, 0, &mut mmi as *mut MINMAXINFO as isize);
        Some((mmi.ptMinTrackSize.x - 16, mmi.ptMinTrackSize.y - 8))
    }
}

#[cfg(feature = "bench")]
/// Bench only: the client area to `w` x `h` physical pixels (the frame's 16 x 8 added),
/// without activation (`resize_quietly`). `maximize` puts it back.
pub fn resize_client(hwnd: isize, w: i32, h: i32) {
    resize_quietly(hwnd, w + 16, h + 8);
}
