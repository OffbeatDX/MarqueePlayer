//! The window without the Windows title bar, keeping everything the native frame gives.
//!
//! The window stays a normal decorated window (WS_OVERLAPPEDWINDOW), so Windows keeps
//! its shadow, rounded corners (Windows 11), Aero snap, Win+arrows, Alt+Space, Alt+F4,
//! the taskbar entry and the side and bottom resize borders. Only the caption goes:
//! `WM_NCCALCSIZE` hands the top strip to the app, which draws its own surface there.
//! Then:
//! - `WM_NCHITTEST`: a thin band at the top is the top resize border (with corners),
//!   and the app's maximize button reports `HTMAXBUTTON`, which is what makes Windows 11
//!   show the snap layouts flyout on hover. Everything else is client: the drag area is
//!   a Slint `WindowMoveArea` under the app's controls, so it never takes their clicks.
//! - The maximize button's non-client mouse messages drive its hover and click (Slint
//!   does not see the pointer there).
//! - Maximized, the client is inset by the frame thickness for the window's DPI (so
//!   nothing is lost past the screen edge), and by 2 px on an edge with an auto-hiding
//!   taskbar so it can still be revealed.
//!
//! Caption buttons are drawn by Slint (chrome.slint) at the geometry below, in logical
//! pixels: physical ones are the window's DPI times the interface size (`ui_scale`). The
//! top resize band is the system's frame thickness for the DPI alone.

use std::cell::Cell;

pub use crate::platform::{BUTTON_H, BUTTON_W, FrameEvent, in_caption_zone};

/// Width of the top-left / top-right resize corners, logical pixels.
const CORNER: f32 = 16.0;

const HTCLIENT: isize = 1;
const HTMAXBUTTON: isize = 9;
const HTTOP: isize = 12;
const HTTOPLEFT: isize = 13;
const HTTOPRIGHT: isize = 14;

/// What a client point (physical pixels) is for, beyond plain client: the top resize
/// border or the maximize button. `scale` is physical pixels per logical one of the app's
/// drawing (the DPI's times the interface size); `band` the top border thickness (physical).
pub fn hit(x: f32, y: f32, width: f32, scale: f32, maximized: bool, band: f32) -> Option<isize> {
    if !maximized && y >= 0.0 && y < band {
        return Some(if x < CORNER * scale {
            HTTOPLEFT
        } else if x >= width - CORNER * scale {
            HTTOPRIGHT
        } else {
            HTTOP
        });
    }
    let (left, right) = (width - 2.0 * BUTTON_W * scale, width - BUTTON_W * scale);
    if y >= 0.0 && y < BUTTON_H * scale && x >= left && x < right {
        return Some(HTMAXBUTTON);
    }
    None
}

thread_local! {
    static HWND: Cell<isize> = const { Cell::new(0) };
    static EVENTS: Cell<Option<fn(FrameEvent)>> = const { Cell::new(None) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static PRESSED: Cell<bool> = const { Cell::new(false) };
    static ZONE: Cell<bool> = const { Cell::new(false) };
}

fn set_zone(on: bool) {
    if ZONE.with(|z| z.replace(on)) != on {
        emit(FrameEvent::CaptionZone(on));
    }
}

fn emit(e: FrameEvent) {
    if let Some(f) = EVENTS.with(Cell::get) {
        let _ = slint::invoke_from_event_loop(move || f(e));
    }
}

/// Take over the caption of `hwnd` (already subclassed by winmsg.rs, which forwards
/// messages to `handle`).
pub fn install(hwnd: *mut std::ffi::c_void, events: fn(FrameEvent)) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    };
    HWND.with(|h| h.set(hwnd as isize));
    EVENTS.with(|e| e.set(Some(events)));
    // SAFETY: our own window, on its UI thread. Recomputes the frame with our
    // WM_NCCALCSIZE (the caption is gone from here on).
    unsafe {
        SetWindowPos(hwnd as _, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        emit(FrameEvent::Maximized(IsZoomed(hwnd as _) != 0));
    }
}

fn hwnd() -> windows_sys::Win32::Foundation::HWND {
    HWND.with(Cell::get) as _
}

/// Physical pixels per logical one of the app's drawing: the window's DPI times the
/// interface size (`zoom.rs`).
fn ui_scale(hwnd: windows_sys::Win32::Foundation::HWND) -> f32 {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    // SAFETY: a query on our own window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    dpi as f32 / 96.0 * crate::zoom::factor()
}

/// Frame thickness (resize border plus padding) for a DPI, physical pixels.
unsafe fn frame_y(dpi: u32) -> i32 {
    use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SM_CXPADDEDBORDER, SM_CYFRAME};
    unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

/// Handle a frame message, or `None` to let it through.
///
/// # Safety
/// Called from the window procedure with that message's arguments.
pub unsafe fn handle(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ScreenToClient};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows_sys::Win32::UI::Shell::{ABM_GETAUTOHIDEBAREX, APPBARDATA, DefSubclassProc, SHAppBarMessage};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, IsZoomed, NCCALCSIZE_PARAMS};

    const WM_SIZE: u32 = 0x0005;
    const WM_GETMINMAXINFO: u32 = 0x0024;
    const WM_NCCALCSIZE: u32 = 0x0083;
    const WM_NCHITTEST: u32 = 0x0084;
    const WM_NCMOUSEMOVE: u32 = 0x00A0;
    const WM_NCLBUTTONDOWN: u32 = 0x00A1;
    const WM_NCLBUTTONUP: u32 = 0x00A2;
    const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
    const WM_NCMOUSELEAVE: u32 = 0x02A2;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    // Client point (physical) of a message's lParam, screen or client.
    // SAFETY: queries on our own window into a local.
    let zone_at = |x: i32, y: i32| unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        in_caption_zone(x as f32, y as f32, (rc.right - rc.left) as f32, ui_scale(hwnd))
    };
    let lp_xy = |lp: isize| ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    if HWND.with(Cell::get) == 0 {
        return None;
    }
    // SAFETY (whole match): Win32 calls on our own window with pointers into locals or
    // into the message's own NCCALCSIZE_PARAMS.
    unsafe {
        match msg {
            WM_NCCALCSIZE if wparam != 0 => {
                let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                let top = params.rgrc[0].top;
                let r = DefSubclassProc(hwnd, msg, wparam, lparam);
                if r != 0 {
                    return Some(r);
                }
                let maximized = IsZoomed(hwnd) != 0;
                let rc = &mut params.rgrc[0];
                rc.top = top + if maximized { frame_y(GetDpiForWindow(hwnd)) } else { 0 };
                if maximized {
                    // Leave a sliver on edges with an auto-hiding taskbar, or it could
                    // never be revealed over a window covering the whole monitor.
                    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut mi: MONITORINFO = std::mem::zeroed();
                    mi.cbSize = size_of::<MONITORINFO>() as u32;
                    if GetMonitorInfoW(mon, &mut mi) != 0 {
                        for edge in 0..4u32 {
                            let mut abd: APPBARDATA = std::mem::zeroed();
                            abd.cbSize = size_of::<APPBARDATA>() as u32;
                            abd.uEdge = edge;
                            abd.rc = mi.rcMonitor;
                            if SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut abd) != 0 {
                                match edge {
                                    0 => rc.left += 2,
                                    1 => rc.top += 2,
                                    2 => rc.right -= 2,
                                    _ => rc.bottom -= 2,
                                }
                            }
                        }
                    }
                }
                Some(0)
            }
            WM_NCHITTEST => {
                let r = DefSubclassProc(hwnd, msg, wparam, lparam);
                if r != HTCLIENT {
                    return Some(r);
                }
                let mut pt = POINT { x: (lparam & 0xffff) as i16 as i32, y: ((lparam >> 16) & 0xffff) as i16 as i32 };
                ScreenToClient(hwnd, &mut pt);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let band = frame_y(GetDpiForWindow(hwnd)) as f32;
                Some(hit(pt.x as f32, pt.y as f32, (rc.right - rc.left) as f32, ui_scale(hwnd), IsZoomed(hwnd) != 0, band).unwrap_or(r))
            }
            WM_MOUSEMOVE => {
                let (x, y) = lp_xy(lparam);
                set_zone(zone_at(x, y));
                None
            }
            WM_MOUSELEAVE => {
                // Into the maximize button (non-client) is still in the zone; the
                // WM_NCMOUSEMOVE that follows says so.
                set_zone(false);
                None
            }
            WM_NCMOUSEMOVE => {
                let (sx, sy) = lp_xy(lparam);
                let mut pt = POINT { x: sx, y: sy };
                ScreenToClient(hwnd, &mut pt);
                set_zone(zone_at(pt.x, pt.y));
                let on_max = wparam as isize == HTMAXBUTTON;
                emit(FrameEvent::MaxHover(on_max));
                if !TRACKING.with(Cell::get) {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut t);
                    TRACKING.with(|c| c.set(true));
                }
                if !on_max && PRESSED.with(Cell::get) {
                    PRESSED.with(|c| c.set(false));
                    emit(FrameEvent::MaxPressed(false));
                }
                None
            }
            WM_NCMOUSELEAVE => {
                TRACKING.with(|c| c.set(false));
                set_zone(false);
                PRESSED.with(|c| c.set(false));
                emit(FrameEvent::MaxHover(false));
                emit(FrameEvent::MaxPressed(false));
                None
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam as isize == HTMAXBUTTON => {
                // Ours: no classic button drawing, no default action.
                PRESSED.with(|c| c.set(true));
                emit(FrameEvent::MaxPressed(true));
                Some(0)
            }
            WM_NCLBUTTONUP if wparam as isize == HTMAXBUTTON => {
                if PRESSED.with(|c| c.replace(false)) {
                    emit(FrameEvent::MaxPressed(false));
                    toggle_maximize();
                }
                Some(0)
            }
            WM_GETMINMAXINFO => {
                // winit sizes the window for a client under a caption, which is gone:
                // without this the minimum client would be a caption taller than asked.
                use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, SM_CYCAPTION};
                let r = DefSubclassProc(hwnd, msg, wparam, lparam);
                let mmi = &mut *(lparam as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd);
                let extra = windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYCAPTION, dpi) + frame_y(dpi);
                if mmi.ptMinTrackSize.y > extra {
                    mmi.ptMinTrackSize.y -= extra;
                }
                Some(r)
            }
            WM_SIZE => {
                match wparam {
                    0 => emit(FrameEvent::Maximized(false)),
                    2 => emit(FrameEvent::Maximized(true)),
                    _ => {}
                }
                None
            }
            _ => None,
        }
    }
}

fn syscommand(cmd: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_SYSCOMMAND};
    // SAFETY: posting to our own window.
    unsafe {
        PostMessageW(hwnd(), WM_SYSCOMMAND, cmd as usize, 0);
    }
}

pub fn minimize() {
    syscommand(windows_sys::Win32::UI::WindowsAndMessaging::SC_MINIMIZE);
}

pub fn toggle_maximize() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsZoomed, SC_MAXIMIZE, SC_RESTORE};
    // SAFETY: a state query on our own window.
    let zoomed = unsafe { IsZoomed(hwnd()) } != 0;
    syscommand(if zoomed { SC_RESTORE } else { SC_MAXIMIZE });
}

/// The same path as Alt+F4 and the old close button: WM_CLOSE, which hides to the tray.
pub fn close() {
    syscommand(windows_sys::Win32::UI::WindowsAndMessaging::SC_CLOSE);
}

/// The window's system menu at a client point (logical pixels), as a right click on a
/// title bar shows it.
pub fn system_menu(x: f32, y: f32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnableMenuItem, GetSystemMenu, IsZoomed, MF_BYCOMMAND, MF_ENABLED, MF_GRAYED, PostMessageW, SC_MAXIMIZE, SC_MOVE,
        SC_RESTORE, SC_SIZE, SetMenuDefaultItem, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_SYSCOMMAND,
    };
    let h = hwnd();
    // SAFETY: our own window and its own system menu.
    unsafe {
        let scale = ui_scale(h);
        let mut pt = POINT { x: (x * scale) as i32, y: (y * scale) as i32 };
        ClientToScreen(h, &mut pt);
        let menu = GetSystemMenu(h, 0);
        if menu.is_null() {
            return;
        }
        let zoomed = IsZoomed(h) != 0;
        let state = |on: bool| MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED };
        EnableMenuItem(menu, SC_RESTORE, state(zoomed));
        EnableMenuItem(menu, SC_MAXIMIZE, state(!zoomed));
        EnableMenuItem(menu, SC_MOVE, state(!zoomed));
        EnableMenuItem(menu, SC_SIZE, state(!zoomed));
        SetMenuDefaultItem(menu, windows_sys::Win32::UI::WindowsAndMessaging::SC_CLOSE, 0);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, std::ptr::null());
        if cmd != 0 {
            PostMessageW(h, WM_SYSCOMMAND, cmd as usize, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_band_resizes_with_corners() {
        // 1440 px wide at 100 %, 8 px band.
        assert_eq!(hit(700.0, 3.0, 1440.0, 1.0, false, 8.0), Some(HTTOP));
        assert_eq!(hit(5.0, 3.0, 1440.0, 1.0, false, 8.0), Some(HTTOPLEFT));
        assert_eq!(hit(1435.0, 3.0, 1440.0, 1.0, false, 8.0), Some(HTTOPRIGHT));
        // No top border when maximized (the screen edge).
        assert_eq!(hit(700.0, 3.0, 1440.0, 1.0, true, 8.0), None);
        // Below the band: client, for the app's own drag area and controls.
        assert_eq!(hit(700.0, 20.0, 1440.0, 1.0, false, 8.0), None);
    }

    #[test]
    fn maximize_button_is_reported_for_snap_layouts() {
        // Buttons from the right: close [1394,1440), maximize [1348,1394), minimize.
        assert_eq!(hit(1370.0, 16.0, 1440.0, 1.0, false, 8.0), Some(HTMAXBUTTON));
        assert_eq!(hit(1400.0, 16.0, 1440.0, 1.0, false, 8.0), None, "close is drawn and handled by the app");
        assert_eq!(hit(1330.0, 16.0, 1440.0, 1.0, false, 8.0), None, "minimize too");
        assert_eq!(hit(1370.0, 40.0, 1440.0, 1.0, false, 8.0), None, "below the button");
        // At 150 %: everything scales.
        assert_eq!(hit(2160.0 - 1.5 * 70.0, 20.0, 2160.0, 1.5, false, 11.0), Some(HTMAXBUTTON));
        // 100 % display, interface size 200 %: the buttons drawn twice as large are hit
        // where they are drawn, the system's band stays its own.
        assert_eq!(hit(1440.0 - 2.0 * 70.0, 50.0, 1440.0, 2.0, false, 8.0), Some(HTMAXBUTTON));
        assert_eq!(hit(1440.0 - 70.0, 50.0, 1440.0, 1.0, false, 8.0), None, "below the unzoomed button");
        assert_eq!(hit(700.0, 7.0, 1440.0, 2.0, false, 8.0), Some(HTTOP));
        assert_eq!(hit(700.0, 9.0, 1440.0, 2.0, false, 8.0), None);
    }
}
