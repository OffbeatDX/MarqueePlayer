//! The app icon. The artwork lives in `tray/icon_art.rs`, shared with `build.rs`, which
//! embeds it in the exe as icon resource 1. At runtime:
//!
//! - the tray gets the glyph alone (no tile) as a Slint image, and the Slint window the
//!   tile;
//! - on Windows the window gets the exe's icon resource through `WM_SETICON`
//!   (`platform::window`); on Linux the desktop shows the icon its desktop file names
//!   (`packaging/linux`, made from the same artwork), and X11 window managers the
//!   window's own (`_NET_WM_ICON`, set from this artwork).

#[path = "icon_art.rs"]
mod icon_art;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub fn app_icon(size: u32) -> Image {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&icon_art::rgba(size), size, size);
    Image::from_rgba8(buf)
}

/// The tray icon's pixels: the glyph without its tile, so it reads at 16 px.
#[cfg_attr(not(test), allow(dead_code))]
pub fn tray_rgba(size: u32) -> Vec<u8> {
    icon_art::glyph_rgba(size)
}

/// The tray glyph with its bars at these heights (`REST_HEIGHTS` is the static glyph),
/// as straight RGBA: the tray animation's frames.
pub fn tray_frame(size: u32, heights: &[f32; 5]) -> Vec<u8> {
    icon_art::glyph_on(size, heights, light_taskbar())
}

#[cfg(any(test, feature = "bench"))]
/// The same frame on a given ground (`light`: the deeper gradient), for the bench's sheets and the tests.
pub fn tray_frame_on(size: u32, heights: &[f32; 5], light: bool) -> Vec<u8> {
    icon_art::glyph_on(size, heights, light)
}

/// Does the taskbar use the light theme (Windows' "default app mode" for the system,
/// `SystemUsesLightTheme`; on Linux the settings portal's colour scheme)? Read when the tray
/// icon is drawn at rest or starts moving.
pub fn light_taskbar() -> bool {
    thread_local! {
        static CACHE: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) };
    }
    // Every frame asks; the system is asked at most every few seconds.
    if let Some((at, v)) = CACHE.with(std::cell::Cell::get)
        && at.elapsed() < std::time::Duration::from_secs(5)
    {
        return v;
    }
    let v = read_light_taskbar();
    CACHE.with(|c| c.set(Some((std::time::Instant::now(), v))));
    v
}

fn read_light_taskbar() -> bool {
    crate::platform::system::light_panel()
}

/// The tray glyph's bar heights at rest.
pub const REST_HEIGHTS: [f32; 5] = icon_art::REST;

pub fn image_from_rgba(rgba: &[u8], size: u32) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, size, size))
}
