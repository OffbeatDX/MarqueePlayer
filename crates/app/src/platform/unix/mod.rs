//! What Linux and macOS share behind the platform seam (`crate::platform`): one instance
//! per data folder through a Unix socket (`instance`), the server's secret in a file of mode
//! 0600 (`secret`), the window's saved size and state (`placement`), and the keys and file
//! systems as both see them. Each system's module re-exports these as its own.

pub mod instance;
pub mod placement;
pub mod secret;

use std::path::{Path, PathBuf};

/// The key's Win32 virtual-key code, as the keymap names keys (`Binding::virtual_key`).
pub fn virtual_key(key: &slint::winit_030::winit::keyboard::Key) -> Option<u16> {
    use slint::winit_030::winit::keyboard::{Key, NamedKey as N};
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let c = c.to_ascii_uppercase();
            (c.is_ascii_uppercase() || c.is_ascii_digit()).then_some(c as u16)
        }
        Key::Named(n) => Some(match n {
            N::Space => 0x20,
            N::PageUp => 0x21,
            N::PageDown => 0x22,
            N::End => 0x23,
            N::Home => 0x24,
            N::ArrowLeft => 0x25,
            N::ArrowUp => 0x26,
            N::ArrowRight => 0x27,
            N::ArrowDown => 0x28,
            N::Insert => 0x2D,
            N::Delete => 0x2E,
            N::Backspace => 0x08,
            N::Pause => 0x13,
            N::F1 => 0x70,
            N::F2 => 0x71,
            N::F3 => 0x72,
            N::F4 => 0x73,
            N::F5 => 0x74,
            N::F6 => 0x75,
            N::F7 => 0x76,
            N::F8 => 0x77,
            N::F9 => 0x78,
            N::F10 => 0x79,
            N::F11 => 0x7A,
            N::F12 => 0x7B,
            _ => return None,
        }),
        _ => None,
    }
}

/// The mount point of the file system a path is on: the highest folder above it on the same
/// device.
pub fn volume_root(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(path).ok()?;
    let dev = std::fs::metadata(&path).ok()?.dev();
    let mut root = path.clone();
    for p in path.ancestors().skip(1) {
        match std::fs::metadata(p) {
            Ok(m) if m.dev() == dev => root = p.to_path_buf(),
            _ => break,
        }
    }
    Some(root)
}
