//! Windows: the window's placement as `GetWindowPlacement` gives it, restored with
//! `SetWindowPlacement` once the window exists, moved onto a monitor that is there (`fit`).
//! (`placement.rs` keeps the polling and the setting.)

use crate::settings::WindowPlacement;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, IsIconic, IsWindowVisible, IsZoomed, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SetWindowPlacement, ShowWindow,
    WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
};

thread_local! {
    /// Started in the tray ("Start with Windows"): the window exists but is hidden, so the
    /// saved placement is applied without showing it, and whether it comes back maximized
    /// waits here for the first time it is shown (`show_pending`).
    static PENDING: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// At least this much of the window's top strip (where it is dragged from) must be on
/// a monitor for the saved position to be used as it is.
const VISIBLE_W: i32 = 120;
const VISIBLE_H: i32 = 24;
/// The smallest restored size accepted (a corrupt or hand-edited value).
const MIN_W: i32 = 400;
const MIN_H: i32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

// Saturating: a saved rectangle comes from a file that may have been edited by hand.
impl Rect {
    fn width(&self) -> i32 {
        self.right.saturating_sub(self.left)
    }
    fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
    fn overlap(&self, o: &Rect) -> (i32, i32) {
        let w = self.right.min(o.right) - self.left.max(o.left);
        let h = self.bottom.min(o.bottom) - self.top.max(o.top);
        (w.max(0), h.max(0))
    }
}

/// Where to put a saved rectangle, given the monitors' work areas (all in the same
/// coordinates, the first being the primary monitor's): as saved if its top strip is
/// on one of them, else centred on the primary one, shrunk to fit. `None` for a size
/// that makes no sense.
pub fn fit(saved: Rect, work_areas: &[Rect]) -> Option<Rect> {
    if saved.width() < MIN_W || saved.height() < MIN_H || saved.width() > 100_000 || saved.height() > 100_000 {
        return None;
    }
    let strip = Rect { bottom: saved.top.saturating_add(VISIBLE_H.max(1) * 2), ..saved };
    let visible = work_areas.iter().any(|w| {
        let (ow, oh) = strip.overlap(w);
        ow >= VISIBLE_W.min(saved.width()) && oh >= VISIBLE_H
    });
    if visible {
        return Some(saved);
    }
    let p = *work_areas.first()?;
    let w = saved.width().min(p.width());
    let h = saved.height().min(p.height());
    let left = p.left + (p.width() - w) / 2;
    let top = p.top + (p.height() - h) / 2;
    Some(Rect { left, top, right: left + w, bottom: top + h })
}

/// Each monitor's work area in workspace coordinates (what `rcNormalPosition` uses:
/// screen coordinates offset by the primary monitor's work area origin), primary
/// first.
fn work_areas() -> Vec<Rect> {
    const MONITORINFOF_PRIMARY: u32 = 1;
    unsafe extern "system" fn each(mon: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> i32 {
        // SAFETY: `data` is the Vec below, alive for the enumeration; `mi` is local.
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
    monitors.sort_by_key(|(primary, _)| !primary);
    let (dx, dy) = monitors.first().map_or((0, 0), |(_, r)| (r.left, r.top));
    monitors
        .into_iter()
        .map(|(_, r)| Rect { left: r.left - dx, top: r.top - dy, right: r.right - dx, bottom: r.bottom - dy })
        .collect()
}

pub fn read(hwnd: isize) -> Option<WindowPlacement> {
    let hwnd = hwnd as HWND;
    // SAFETY: our own window; `wp` is local.
    unsafe {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut wp) == 0 {
            return None;
        }
        // Hidden in the tray, or minimized: still the state it comes back in. Never shown
        // since a start in the tray: as it was saved.
        let maximized = PENDING.with(|p| p.get()).unwrap_or_else(|| IsZoomed(hwnd) != 0 || (IsIconic(hwnd) != 0 && wp.flags & WPF_RESTORETOMAXIMIZED != 0));
        let r = wp.rcNormalPosition;
        Some(WindowPlacement { left: r.left, top: r.top, right: r.right, bottom: r.bottom, maximized })
    }
}

pub fn apply(hwnd: isize, saved: &WindowPlacement) {
    let rect = Rect { left: saved.left, top: saved.top, right: saved.right, bottom: saved.bottom };
    let areas = work_areas();
    let Some(r) = fit(rect, &areas) else {
        log::warn!("window placement {saved:?} ignored (implausible size)");
        return;
    };
    if r != rect {
        log::info!("window placement {rect:?} is off every monitor; moved to {r:?}");
    }
    let hwnd = hwnd as HWND;
    // SAFETY: our own window, on its UI thread; `wp` is local.
    unsafe {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut wp) == 0 {
            return;
        }
        wp.flags = 0;
        let hidden = IsWindowVisible(hwnd) == 0;
        wp.showCmd = if hidden {
            PENDING.with(|p| p.set(Some(saved.maximized)));
            SW_HIDE
        } else if saved.maximized {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        } as u32;
        wp.rcNormalPosition = RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
        SetWindowPlacement(hwnd, &wp);
    }
    log::info!("window placement restored: {r:?}, maximized {}", saved.maximized);
}

/// The window is about to be shown for the first time after a start in the tray: show it
/// where and how it was saved (maximized or not), before the toolkit shows it as it is.
pub fn show_pending(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    if let Some(maximized) = PENDING.with(|p| p.take()) {
        // SAFETY: our own window, on its UI thread.
        unsafe { ShowWindow(hwnd as HWND, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL }) };
    }
}

/// Nothing to do before the window is shown: Windows restores the native window itself.
pub fn before_show(_app: &crate::AppWindow, _saved: Option<WindowPlacement>) {}

/// The window's placement now. `None` before the window exists.
pub fn now(_app: &crate::AppWindow, hwnd: isize) -> Option<WindowPlacement> {
    if hwnd != 0 { read(hwnd) } else { None }
}

/// Put the window where it was, once it exists.
pub fn restore(hwnd: isize, saved: WindowPlacement) {
    apply(hwnd, &saved);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Rect = Rect { left: 0, top: 0, right: 2560, bottom: 1400 };
    const LEFT: Rect = Rect { left: -1920, top: 0, right: 0, bottom: 1040 };

    #[test]
    fn on_a_monitor_it_stays() {
        let r = Rect { left: -1800, top: 100, right: -400, bottom: 900 };
        assert_eq!(fit(r, &[PRIMARY, LEFT]), Some(r));
        // Partly off the edge, but its top strip is on the primary: kept.
        let r = Rect { left: 2300, top: 50, right: 3500, bottom: 900 };
        assert_eq!(fit(r, &[PRIMARY]), Some(r));
    }

    #[test]
    fn off_every_monitor_it_moves_to_the_primary() {
        // Saved on the left monitor, which is gone.
        let r = Rect { left: -1800, top: 100, right: -400, bottom: 900 };
        let got = fit(r, &[PRIMARY]).unwrap();
        assert_eq!((got.width(), got.height()), (1400, 800), "same size");
        assert!(got.left >= 0 && got.right <= 2560 && got.top >= 0 && got.bottom <= 1400);
        // Larger than the primary: shrunk to it.
        let r = Rect { left: 5000, top: 0, right: 9000, bottom: 3000 };
        assert_eq!(fit(r, &[PRIMARY]), Some(PRIMARY));
        // Only a sliver of the top strip showing (title bar under the bottom edge): moved.
        let r = Rect { left: 100, top: 1390, right: 1500, bottom: 2200 };
        assert_ne!(fit(r, &[PRIMARY]), Some(r));
    }

    #[test]
    fn nonsense_is_ignored() {
        assert_eq!(fit(Rect { left: 0, top: 0, right: 10, bottom: 10 }, &[PRIMARY]), None);
        assert_eq!(fit(Rect { left: 0, top: 0, right: 900, bottom: 700 }, &[]), None);
        // A hand-edited settings file: numbers whose difference does not fit (a debug build
        // panicked on the overflow).
        assert_eq!(fit(Rect { left: i32::MIN, top: 0, right: 1000, bottom: 900 }, &[PRIMARY]), None);
        assert_eq!(fit(Rect { left: 0, top: i32::MIN, right: 1000, bottom: i32::MAX }, &[PRIMARY]), None);
        assert!(fit(Rect { left: 0, top: i32::MAX - 800, right: 1000, bottom: i32::MAX }, &[PRIMARY]).is_some_and(|r| r.top >= 0 && r.bottom <= 1400), "far below every monitor: moved");
    }
}
