//! "Show in Explorer" (Windows) or the file manager (Linux): a new window with the file
//! (or the album's folder) selected. Only ever reads: nothing is opened, moved or changed.
//! How each system does it is `platform::shell`; what to show is decided here.

use std::path::{Path, PathBuf};

/// What to show for `path`: the item itself when it exists (selected in its folder), else
/// the nearest folder above it that does (opened), else nothing.
pub fn target(path: &Path) -> Option<(PathBuf, bool)> {
    if path.exists() {
        return Some((path.to_path_buf(), true));
    }
    path.ancestors().skip(1).find(|p| !p.as_os_str().is_empty() && p.is_dir()).map(|p| (p.to_path_buf(), false))
}

/// Show `path` (a file or a folder from the library) in a new file manager window. False
/// when neither it nor any folder above it exists.
pub fn show(path: &str) -> bool {
    let path = crate::platform::shell::native_path(path);
    let Some((target, select)) = target(&path) else { return false };
    crate::platform::shell::show_item(&target, select)
}

/// Open a folder of the app's own (Settings > About) in a new file manager window, or the
/// nearest folder above it that exists. False when there is none.
pub fn open_folder(dir: &Path) -> bool {
    let Some(target) = dir.ancestors().find(|p| !p.as_os_str().is_empty() && p.is_dir()) else { return false };
    crate::platform::shell::open_folder(target)
}

/// Put text on the clipboard (View metadata's and the equalizer's Copy). False when it could not. A bench run
/// never touches the real clipboard (the person at this PC may be using it): the text is kept
/// for the bench to read (`bench_clipboard`).
pub fn copy_text(text: &str) -> bool {
    #[cfg(feature = "bench")]
    if crate::bench_run() {
        *BENCH_CLIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_owned());
        return true;
    }
    crate::platform::shell::copy_text(text)
}

/// Text from the clipboard (the equalizer's and the visualizer's Paste): `Ok(None)` when it has
/// no text, an error to show when it cannot be read or holds far more than any setting (the
/// equalizer's limit, the larger of the two). A
/// bench run reads the stand-in (`bench_set_clipboard`), never the real clipboard.
pub fn paste_text() -> Result<Option<String>, String> {
    #[cfg(feature = "bench")]
    if crate::bench_run() {
        return Ok(BENCH_CLIP.lock().unwrap_or_else(|e| e.into_inner()).clone());
    }
    crate::platform::shell::paste_text(player_core::eq::formats::MAX_BYTES)
}

#[cfg(feature = "bench")]
static BENCH_CLIP: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What a bench run last "copied".
#[cfg(feature = "bench")]
pub fn bench_clipboard() -> Option<String> {
    BENCH_CLIP.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What a bench run's Paste reads.
#[cfg(feature = "bench")]
pub fn bench_set_clipboard(text: Option<&str>) {
    *BENCH_CLIP.lock().unwrap_or_else(|e| e.into_inner()) = text.map(str::to_owned);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_falls_back_to_its_folder() {
        let dir = std::env::temp_dir().join(format!("mp-explorer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("track.flac");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(target(&file), Some((file.clone(), true)));
        let gone = dir.join("gone").join("track.flac");
        assert_eq!(target(&gone), Some((dir.clone(), false)), "the nearest folder that exists, opened");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(target(Path::new("")), None);
    }
}
