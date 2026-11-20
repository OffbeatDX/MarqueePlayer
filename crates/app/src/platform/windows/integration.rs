//! Windows: the editions (`crate::edition`) and what the installer registers.
//!
//! The installer (`packaging/windows/MarqueePlayer.iss`) writes, per user or per machine
//! (HKCU or HKLM, the scope chosen in it), the documented registration of a media app:
//! a ProgID per format (`MarqueePlayer.<ext>`), `Applications\MarqueePlayer.exe` with its
//! `SupportedTypes`, each extension's `OpenWithProgids`, and `Capabilities` under
//! `RegisteredApplications` as `Marquee Player`, so the app is in "Open with" and in
//! Settings > Default apps without taking any association. The names here and there must
//! agree (a test checks the script).
//!
//! The installed edition is the one that registration names: its `shell\open\command` is
//! this exe. Only then does Settings show "Choose default apps…", which opens Windows'
//! Default apps page for the app (`ms-settings:defaultapps?registeredAppUser=` or
//! `registeredAppMachine=`, Windows 11 21H2 with the 2023-04 update and later; Windows 10
//! opens the Default apps page itself, where the app is listed).
//!
//! "Start with Windows" is the user's `Run` key (`HKCU\...\CurrentVersion\Run`), starting the
//! exe with `--minimized` (in the tray). Task Manager's Startup apps can turn such an entry
//! off (`Explorer\StartupApproved\Run`): the switch shows it off then, and turning it on
//! again clears that.
//!
//! For the installer, the installed edition holds a mutex (`INSTALLED_MUTEX`, in the session
//! and machine-wide) and waits on an event (`QUIT_EVENT`): setting it quits the app the
//! normal way (queue, position and settings saved), so setup and uninstall can close it.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};

use crate::edition::{Edition, PORTABLE_FOLDER, PORTABLE_MARKER, Portable};

/// The name under `RegisteredApplications`, and in the Default apps page's address.
pub const REGISTERED_NAME: &str = "Marquee Player";
/// `Software\Classes\Applications\<this>`: "Open with", and how the installed exe is known.
pub const APPLICATION_KEY: &str = r"Software\Classes\Applications\MarqueePlayer.exe";
/// Held by the installed edition while it runs (`Local\` and `Global\`).
pub const INSTALLED_MUTEX: &str = "MarqueePlayer.Installed";
/// Set by setup and uninstall to have the installed edition quit (`Local\`).
pub const QUIT_EVENT: &str = r"Local\MarqueePlayer.Installed.Quit";

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn read_string(root: HKEY, key: &str, name: Option<&str>) -> Option<String> {
    let (key, name) = (wide(key), name.map(wide));
    let mut len = 0u32;
    let name_ptr = name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    // SAFETY: a size query, then a read into a buffer of that size; strings NUL-terminated.
    unsafe {
        if RegGetValueW(root, key.as_ptr(), name_ptr, RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
        let mut len = (buf.len() * 2) as u32;
        if RegGetValueW(root, key.as_ptr(), name_ptr, RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').to_owned())
    }
}

fn read_binary(root: HKEY, key: &str, name: &str) -> Option<Vec<u8>> {
    let (key, name) = (wide(key), wide(name));
    let mut buf = [0u8; 64];
    let mut len = buf.len() as u32;
    // SAFETY: a read into a local buffer whose size is passed.
    let r = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    (r == ERROR_SUCCESS).then(|| buf[..(len as usize).min(buf.len())].to_vec())
}

/// The program a command line starts: its first argument, quoted or not.
pub fn command_program(command: &str) -> Option<PathBuf> {
    let c = command.trim_start();
    let program = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => c.split_whitespace().next()?,
    };
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// The same file, letter case and `..` aside.
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    canon(a) == canon(b)
}

fn this_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Is this exe the one the installer registered, and for whom: `Some(false)` for the user,
/// `Some(true)` for everyone. The user's registration wins, as it does in Windows' merged
/// view of the two.
pub fn registration(exe: &Path) -> Option<bool> {
    let command = format!(r"{APPLICATION_KEY}\shell\open\command");
    for (root, machine) in [(HKEY_CURRENT_USER, false), (HKEY_LOCAL_MACHINE, true)] {
        let registered = read_string(root, r"Software\RegisteredApplications", Some(REGISTERED_NAME)).is_some();
        if let Some(program) = read_string(root, &command, None).as_deref().and_then(command_program)
            && registered
            && same_file(&program, exe)
        {
            return Some(machine);
        }
    }
    None
}

/// This run's edition: portable beside its marker, installed where the registration names
/// it, else standalone.
pub fn detect() -> Edition {
    let Some(exe) = this_exe() else { return Edition::Standalone };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if dir.join(PORTABLE_MARKER).is_file() {
        let folder = dir.join(PORTABLE_FOLDER);
        return Edition::Portable(Portable { dirs: crate::edition::one_folder(&folder), folder, anchor: dir });
    }
    match registration(&exe) {
        Some(all_users) => Edition::Installed { all_users },
        None => Edition::Standalone,
    }
}

/// The root of the drive a path is on, as the path names it: `E:\`, `\\server\share\`. What
/// changes when Windows gives a USB stick another letter. (Not `GetVolumePathNameW`, which
/// answers a `subst` drive with a folder inside it, and a folder-mounted volume with that
/// folder: neither is a letter that changes.)
pub fn volume_root(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let path = std::path::absolute(path).ok()?;
    let Some(Component::Prefix(prefix)) = path.components().next() else { return None };
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(std::path::MAIN_SEPARATOR_STR);
    Some(root)
}

// ---- Start with Windows --------------------------------------------------------------------

/// The Run value's name: the portable edition's apart, so it never replaces (or is replaced
/// by) an installed copy's.
fn run_value_name(edition: &Edition) -> &'static str {
    if edition.portable().is_some() { "Marquee Player (portable)" } else { "Marquee Player" }
}

/// The command the Run value holds for `exe`.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {}", exe.display(), crate::edition::MINIMIZED_FLAG)
}

/// Task Manager's Startup apps keeps whether a Run entry is allowed: a first byte that is
/// odd (3) means turned off there.
pub fn approved(value: Option<&[u8]>) -> bool {
    value.and_then(|v| v.first()).is_none_or(|b| b & 1 == 0)
}

/// Is "Start with Windows" on for this exe? `None` where the system has no such setting.
pub fn autostart(edition: &Edition) -> Option<bool> {
    let name = run_value_name(edition);
    let exe = this_exe()?;
    let ours = read_string(HKEY_CURRENT_USER, RUN_KEY, Some(name)).as_deref().and_then(command_program).is_some_and(|p| same_file(&p, &exe));
    Some(ours && approved(read_binary(HKEY_CURRENT_USER, APPROVED_KEY, name).as_deref()))
}

/// Turn "Start with Windows" on (this exe, in the tray) or off.
pub fn set_autostart(edition: &Edition, on: bool) -> Result<(), String> {
    let name = run_value_name(edition);
    let exe = this_exe().ok_or("cannot tell where the app is")?;
    let open = |key: &str| -> Result<HKEY, String> {
        let mut h: HKEY = std::ptr::null_mut();
        let k = wide(key);
        // SAFETY: opens an existing key of the user's hive for setting values.
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_SET_VALUE, &mut h) };
        if r == ERROR_SUCCESS { Ok(h) } else { Err(format!("cannot open {key} (error {r})")) }
    };
    let n = wide(name);
    let run = open(RUN_KEY)?;
    let result = if on {
        let cmd = wide(&run_command(&exe));
        // SAFETY: a REG_SZ value with its terminating NUL; the size is in bytes.
        let r = unsafe { RegSetValueExW(run, n.as_ptr(), 0, REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32) };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("cannot write the startup entry (error {r})")) }
    } else {
        // SAFETY: deletes a value of our own; a missing one is not an error here.
        unsafe { RegDeleteValueW(run, n.as_ptr()) };
        Ok(())
    };
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(run) };
    // Turned off in Task Manager earlier: turning it on here means on.
    if let Ok(k) = open(APPROVED_KEY) {
        // SAFETY: as above; the value may not exist.
        unsafe {
            RegDeleteValueW(k, n.as_ptr());
            RegCloseKey(k);
        }
    }
    result
}

// ---- Default apps --------------------------------------------------------------------------

/// The Default apps page for the app, for how it was installed.
pub fn default_apps_uri(all_users: bool) -> String {
    let name: String = REGISTERED_NAME.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("ms-settings:defaultapps?{}={name}", if all_users { "registeredAppMachine" } else { "registeredAppUser" })
}

/// Can the app open Windows' Default apps page for itself (the installed edition only)?
pub fn default_apps_available(edition: &Edition) -> bool {
    matches!(edition, Edition::Installed { .. })
}

pub fn open_default_apps(edition: &Edition) -> bool {
    match edition {
        Edition::Installed { all_users } => super::shell::open_url(&default_apps_uri(*all_users)),
        _ => false,
    }
}

// ---- the installer ---------------------------------------------------------------------------

/// The installed edition: hold the mutex setup looks for, and quit when setup asks
/// (`QUIT_EVENT`). Nothing for the other editions, which setup never replaces.
pub fn installed_running(edition: &Edition, quit: fn()) {
    use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, INFINITE, WaitForSingleObject};
    if !matches!(edition, Edition::Installed { .. }) {
        return;
    }
    // SAFETY: named objects created with default security; the handles are kept for the
    // whole run (the system closes them at exit, which is what setup waits for).
    unsafe {
        for prefix in ["Local\\", "Global\\"] {
            let name = wide(&format!("{prefix}{INSTALLED_MUTEX}"));
            if CreateMutexW(std::ptr::null(), 0, name.as_ptr()).is_null() {
                log::warn!("installer mutex {prefix}{INSTALLED_MUTEX}: not created");
            }
        }
        let name = wide(QUIT_EVENT);
        // Manual reset: every instance of the installed exe in the session sees it.
        let event: HANDLE = CreateEventW(std::ptr::null(), 1, 0, name.as_ptr());
        if event.is_null() {
            log::warn!("installer event: not created");
            return;
        }
        let event = event as usize;
        let spawned = std::thread::Builder::new().name("mp-setup-watch".into()).spawn(move || {
            if WaitForSingleObject(event as HANDLE, INFINITE) == 0 {
                log::info!("setup asked the app to quit");
                let _ = slint::invoke_from_event_loop(quit);
            }
        });
        if spawned.is_err() {
            log::warn!("installer event: no thread to wait on it");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_name_their_program() {
        assert_eq!(command_program(r#""C:\Program Files\Marquee Player\MarqueePlayer.exe" "%1""#), Some(PathBuf::from(r"C:\Program Files\Marquee Player\MarqueePlayer.exe")));
        assert_eq!(command_program(r"C:\x\MarqueePlayer.exe --minimized"), Some(PathBuf::from(r"C:\x\MarqueePlayer.exe")));
        assert_eq!(command_program("  "), None);
        assert_eq!(command_program(r#""""#), None);
        let exe = Path::new(r"E:\Apps\Marquee Player\MarqueePlayer.exe");
        assert_eq!(command_program(&run_command(exe)).as_deref(), Some(exe));
        assert!(run_command(exe).ends_with(" --minimized"));
    }

    #[test]
    fn task_manager_can_turn_an_entry_off() {
        assert!(approved(None), "no record: allowed");
        assert!(approved(Some(&[2, 0, 0, 0])));
        assert!(approved(Some(&[6, 0])));
        assert!(!approved(Some(&[3, 0, 0, 0])));
        assert!(!approved(Some(&[7])));
    }

    #[test]
    fn the_default_apps_page_is_addressed_by_scope() {
        assert_eq!(default_apps_uri(false), "ms-settings:defaultapps?registeredAppUser=Marquee%20Player");
        assert_eq!(default_apps_uri(true), "ms-settings:defaultapps?registeredAppMachine=Marquee%20Player");
        assert!(!default_apps_available(&Edition::Standalone));
        assert!(default_apps_available(&Edition::Installed { all_users: true }));
    }

    #[test]
    fn a_volume_root_is_the_drive_or_the_share() {
        let exe = std::env::current_exe().unwrap();
        let root = volume_root(&exe).unwrap();
        assert!(exe.starts_with(&root), "{} under {}", exe.display(), root.display());
        assert_eq!(volume_root(Path::new(r"E:\Apps\Marquee Player")), Some(PathBuf::from(r"E:\")));
        assert_eq!(volume_root(Path::new(r"\\nas\music\Apps")), Some(PathBuf::from(r"\\nas\music\")));
    }

    /// The installer writes what the app looks for, by the same names: the registered
    /// name, the Applications key, the mutex and the event, the Run key it cleans, the
    /// marker the portable zip has, and a ProgID with OpenWithProgids for every format the
    /// scanner takes.
    #[test]
    fn the_installer_script_uses_the_same_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
        let iss = std::fs::read_to_string(root.join("windows/MarqueePlayer.iss")).unwrap();
        for name in [REGISTERED_NAME, APPLICATION_KEY, INSTALLED_MUTEX, QUIT_EVENT, RUN_KEY, "registeredAppUser", "registeredAppMachine"] {
            assert!(iss.contains(name), "MarqueePlayer.iss does not name {name}");
        }
        let line = iss.lines().find_map(|l| l.strip_prefix("#define Extensions \"")).expect("#define Extensions");
        let listed: Vec<&str> = line.trim_end_matches('"').split(';').collect();
        for ext in library::scan::AUDIO_EXTS {
            assert!(listed.contains(ext), "{ext} missing from the installer's Extensions");
        }
        assert_eq!(listed.len(), library::scan::AUDIO_EXTS.len(), "{listed:?}");
        assert!(std::fs::read_to_string(root.join("windows/portable.txt")).unwrap().contains(PORTABLE_FOLDER));
    }
}
