//! Windows: the native window's hooks, installed once it exists: the taskbar icon from the exe's resource, the window procedure subclass
//! (`winmsg`: side buttons, the window-level pin keys, `WM_COPYDATA` from a second launch,
//! `WM_DROPFILES`) and the app-drawn caption (`frame`).

use slint::ComponentHandle;

use super::{frame, winmsg};
use crate::AppWindow;
use crate::platform::Hooks;

pub use super::frame::{close, minimize, system_menu, toggle_maximize};
pub use super::instance::bring_back;
pub use super::winmsg::{set_capturing, set_pin_keys};

/// The window attributes every window gets: winit's own OLE drop target off, so
/// Explorer drops arrive as WM_DROPFILES, which carry the drop point (`winmsg`). (Moved
/// here from `opener.rs`.)
pub fn attributes(attrs: slint::winit_030::winit::window::WindowAttributes) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    attrs.with_drag_and_drop(false)
}

/// Taskbar icon from the exe's icon resource, the window procedure hooks (`winmsg`) and
/// the app-drawn frame (`frame`). The window's handle, or `None` while it does not exist.
pub fn install(app: &AppWindow, hooks: Hooks) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = app.window().window_handle();
    let Ok(RawWindowHandle::Win32(h)) = handle.window_handle().map(|h| h.as_raw()) else { return None };
    let hwnd = h.hwnd.get() as *mut std::ffi::c_void;
    set_window_icon(hwnd);
    winmsg::on_pin_shortcut(hooks.pin);
    winmsg::install(hwnd, hooks.nav);
    frame::install(hwnd, hooks.frame);
    winmsg::on_show_request(hooks.show);
    winmsg::on_files(hooks.files);
    winmsg::on_end_session(hooks.end_session);
    Some(hwnd as isize)
}

/// Input taken before Slint sees it: on Windows the window procedure does that (`winmsg`,
/// `frame`), so nothing here.
pub fn winit_event(_app: &AppWindow, _window: &slint::Window, _event: &slint::winit_030::winit::event::WindowEvent) -> bool {
    false
}

/// Set the title bar and taskbar icon of a native window from the exe's icon resource.
/// This does not depend on the UI toolkit (Slint 1.18's winit
/// backend ignores a `Window.icon` made from a pixel buffer, slint-ui/slint#13615).
fn set_window_icon(hwnd: *mut std::ffi::c_void) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON, SendMessageW,
        WM_SETICON,
    };
    // SAFETY: plain Win32 calls on our own module and window; resource 1 is embedded
    // by build.rs. A failed load returns null, which we skip.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let px = GetSystemMetrics(metric);
            let icon = LoadImageW(module, 1 as _, IMAGE_ICON, px, px, LR_DEFAULTCOLOR);
            if icon.is_null() {
                log::warn!("app icon resource not found");
                return;
            }
            SendMessageW(hwnd as _, WM_SETICON, which as usize, icon as isize);
        }
    }
}

/// A WM_DROPFILES as Explorer sends it: an HGLOBAL DROPFILES block with wide paths (the
/// bench drops files on the window this way).
#[cfg(feature = "bench")]
pub fn simulate_drop(hwnd: isize, paths: &[std::path::PathBuf], x: i32, y: i32) -> usize {
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    let mut wide: Vec<u16> = Vec::new();
    for p in paths {
        wide.extend(p.to_string_lossy().encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize; // DROPFILES: pFiles, POINT, fNC, fWide
    // SAFETY: a fresh block written within its size, then handed to the window, which
    // releases it (DragFinish).
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, header + wide.len() * 2);
        let p = GlobalLock(h) as *mut u8;
        *(p as *mut u32) = header as u32;
        *(p.add(4) as *mut i32) = x;
        *(p.add(8) as *mut i32) = y;
        *(p.add(16) as *mut i32) = 1;
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, p.add(header), wide.len() * 2);
        GlobalUnlock(h);
        PostMessageW(hwnd as _, 0x0233, h as usize, 0);
    }
    paths.len()
}
