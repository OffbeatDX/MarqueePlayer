//! Windows: File Explorer, the default browser and the clipboard. Only ever reads: nothing is
//! opened, moved or changed.

use std::path::{Path, PathBuf};

/// The library stores paths as scanned; Explorer wants backslashes.
pub fn native_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', "\\"))
}

/// The raw command line after `explorer.exe`. The path is quoted as one, whatever it holds
/// (spaces, commas); Explorer reads `/select,` and then the quoted path. A path cannot
/// contain a quote on Windows, so it needs no escaping.
pub fn select_args(path: &Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// A new Explorer window with `target` selected in its folder (`select`), or the folder
/// `target` opened, as `explorer.exe /select,<path>` does.
pub fn show_item(target: &Path, select: bool) -> bool {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    if select {
        cmd.raw_arg(select_args(target));
    } else {
        cmd.raw_arg(format!("\"{}\"", target.display()));
    }
    match cmd.spawn() {
        Ok(_) => true,
        Err(e) => {
            log::warn!("Show in Explorer {}: {e}", target.display());
            false
        }
    }
}

/// A folder of the app's own (Settings > About) in a new Explorer window.
pub fn open_folder(target: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    cmd.raw_arg(format!("\"{}\"", target.display()));
    match cmd.spawn() {
        Ok(_) => true,
        Err(e) => {
            log::warn!("Open folder {}: {e}", target.display());
            false
        }
    }
}

/// What "Show in Explorer" would run, for the bench's report (nothing is run).
#[cfg(feature = "bench")]
pub fn describe_show(path: &str) -> String {
    format!("explorer.exe {}", select_args(&native_path(path)))
}

/// Open a web page in the default browser.
pub fn open_url(url: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, url) = (w("open"), w(url));
    // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    r as isize > 32
}

/// Put text on the clipboard (View metadata's Copy). False when it could not.
pub fn copy_text(text: &str) -> bool {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the clipboard is opened and closed here; the memory handed to it is
    // allocated movable, filled while locked, and owned by the clipboard once set (freed
    // here only when it was not taken).
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard() != 0 {
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if !mem.is_null() {
                let dst = GlobalLock(mem) as *mut u16;
                if !dst.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                    GlobalUnlock(mem);
                    ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
                }
                if !ok {
                    GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
        ok
    }
}

/// Text from the clipboard (the equalizer's Paste): `Ok(None)` when it holds no text, an
/// error when it is busy or holds more than `max_bytes` (UTF-16, so twice that in memory),
/// which is refused before it is copied out.
pub fn paste_text(max_bytes: usize) -> Result<Option<String>, String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the clipboard is opened and closed here; the handle it gives is only read,
    // while locked, within the size it reports, and never kept.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return Ok(None);
        }
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("The clipboard is in use by another program: try again".into());
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let out = if h.is_null() {
            Ok(None)
        } else {
            let size = GlobalSize(h);
            if size > max_bytes.saturating_mul(2).saturating_add(2) {
                Err(format!("The clipboard holds too much text to paste ({:.1} MB)", size as f64 / 2.0 / 1_048_576.0))
            } else {
                let p = GlobalLock(h) as *const u16;
                if p.is_null() {
                    Err("Could not read the clipboard".into())
                } else {
                    let all = std::slice::from_raw_parts(p, size / 2);
                    let end = all.iter().position(|&c| c == 0).unwrap_or(all.len());
                    let text = String::from_utf16_lossy(&all[..end]);
                    GlobalUnlock(h);
                    Ok(Some(text))
                }
            }
        };
        CloseClipboard();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_quotes_the_whole_path() {
        let p = native_path("M:/Music/Sigur Rós/Ágætis byrjun, 1999/01 Intro.flac");
        assert_eq!(p, PathBuf::from(r"M:\Music\Sigur Rós\Ágætis byrjun, 1999\01 Intro.flac"));
        assert_eq!(select_args(&p), r#"/select,"M:\Music\Sigur Rós\Ágætis byrjun, 1999\01 Intro.flac""#);
    }
}
