//! Which edition of Marquee Player is running, and what that changes.
//!
//! | Edition | How it is recognised | Where its files are |
//! | --- | --- | --- |
//! | Portable (Windows) | `portable.txt` beside the exe (the portable zip has it) | `Data\` beside the exe |
//! | Portable (Linux) | a `<name>.AppImage.home` folder beside the AppImage (the AppImage convention) | under that folder, as a home |
//! | Installed (Windows) | the installer's registration (`Applications\MarqueePlayer.exe`) names this exe | the user's profile |
//! | AppImage (Linux) | `$APPIMAGE` set by the AppImage runtime | the XDG folders |
//! | Standalone | none of these: a build, a copied exe | the user's profile (XDG folders) |
//!
//! `MP_DATA_DIR` still puts everything in one folder in every edition (`paths`). The
//! portable edition writes nothing outside its folder, unless the user turns on "Start with
//! Windows" (the startup list names its exe); a portable folder that cannot be written is
//! refused with a message at start (`check_writable`), never swapped for the profile.
//! Library folders on the drive the portable edition runs from follow it when the drive's
//! letter changes (`follow_drive`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::paths::{Dirs, Paths};
use crate::settings::Settings;

/// The marker that makes the Windows exe portable, beside it.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// The portable edition's folder for everything it keeps, beside the exe.
#[cfg_attr(not(windows), allow(dead_code))]
pub const PORTABLE_FOLDER: &str = "Data";

#[derive(Clone, Debug, PartialEq)]
pub enum Edition {
    /// Not on macOS, where every copy of the bundle is standalone.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Portable(Portable),
    /// Installed by the Windows installer, for the user (`all_users` false) or for everyone.
    #[cfg_attr(not(windows), allow(dead_code))]
    Installed { all_users: bool },
    /// The Linux AppImage, without a portable home beside it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    AppImage,
    /// A plain exe or binary: a build, or a copy of the exe on its own.
    Standalone,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Portable {
    /// Where its files go.
    pub dirs: Dirs,
    /// The folder that holds them all (About names it).
    pub folder: PathBuf,
    /// What sits on the drive: the exe's folder, or the AppImage.
    pub anchor: PathBuf,
}

static EDITION: OnceLock<Edition> = OnceLock::new();

/// This run's edition, recognised once. A build with the bench harness can be told to act
/// as the installed edition (`MP_BENCH_EDITION=installed` or `installed-all`), to test what
/// only that edition does (the installer's mutex and quit request, the Default apps button)
/// without registering anything.
pub fn get() -> &'static Edition {
    EDITION.get_or_init(|| {
        #[cfg(feature = "bench")]
        match std::env::var("MP_BENCH_EDITION").as_deref() {
            Ok("installed") => return Edition::Installed { all_users: false },
            Ok("installed-all") => return Edition::Installed { all_users: true },
            _ => {}
        }
        crate::platform::integration::detect()
    })
}

impl Edition {
    pub fn portable(&self) -> Option<&Portable> {
        match self {
            Edition::Portable(p) => Some(p),
            _ => None,
        }
    }

    /// For the bug report line: "portable", "installed", ...
    pub fn short(&self) -> &'static str {
        match self {
            Edition::Portable(_) => "portable",
            Edition::Installed { all_users: false } => "installed",
            Edition::Installed { all_users: true } => "installed for all users",
            Edition::AppImage => "AppImage",
            Edition::Standalone => "standalone",
        }
    }

    /// About's line for it.
    pub fn describe(&self) -> String {
        match self {
            Edition::Portable(p) => format!("Portable: everything it keeps is in {}, beside the app.", p.folder.display()),
            Edition::Installed { all_users: false } => "Installed for you. Uninstall it from Windows' Settings > Apps.".to_owned(),
            Edition::Installed { all_users: true } => "Installed for everyone on this computer. Uninstall it from Windows' Settings > Apps.".to_owned(),
            Edition::AppImage => "AppImage: your files are kept in your home folder.".to_owned(),
            Edition::Standalone => format!("Not installed: a copy of the app on its own, keeping its files in your {}.", crate::platform::system::words().profile),
        }
    }
}

/// The one-folder layout (`MP_DATA_DIR`, the Windows portable folder): everything in it, the
/// caches in `cache` below it.
pub fn one_folder(dir: &Path) -> Dirs {
    Dirs { config: dir.to_path_buf(), data: dir.to_path_buf(), state: dir.to_path_buf(), cache: dir.join("cache") }
}

/// The portable folders must be writable: a portable copy on a read-only medium, or in a
/// folder the user may not write to, says so instead of quietly keeping its library
/// somewhere else. Creates the folders.
pub fn check_writable(paths: &Paths) -> Result<(), String> {
    for dir in [&paths.config, &paths.data, &paths.state, &paths.cache] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let probe = dir.join(format!(".write-test-{}", std::process::id()));
        std::fs::write(&probe, b"x").map_err(|e| format!("{}: {e}", dir.display()))?;
        let _ = std::fs::remove_file(&probe);
    }
    Ok(())
}

/// The message for `check_writable`'s failure.
pub fn not_writable_message(p: &Portable, error: &str) -> String {
    format!(
        "Marquee Player is set up as portable ({} beside it), so it keeps everything in\n{}\nbut it cannot write there:\n{error}\n\n\
         Copy the whole Marquee Player folder somewhere you can write to (not a CD or a read-only drive), \
         or delete {} to have it keep its files in your user profile instead.",
        PORTABLE_MARKER,
        p.folder.display(),
        PORTABLE_MARKER
    )
}

/// The portable edition: library folders on the drive it runs from follow the drive to a new
/// letter (or mount point), with the database's paths rewritten in place so nothing is lost
/// or re-read (`library::reroot`). Only folders that are gone where they were and found at
/// the same place on the new drive move. Remembers the drive for next time. True when the
/// settings changed (and should be saved).
pub fn follow_drive(settings: &mut Settings, db: &mut rusqlite::Connection) -> bool {
    let Some(p) = get().portable() else { return false };
    let Some(now) = crate::platform::integration::volume_root(&p.anchor) else { return false };
    let now_text = now.to_string_lossy().into_owned();
    let mut changed = settings.portable_volume.as_deref() != Some(now_text.as_str());
    if let Some(last) = settings.portable_volume.clone().filter(|_| changed) {
        let moves = library::reroot::plan(&settings.folders, Path::new(&last), &now, &|f: &Path| f.is_dir());
        if moves.is_empty() {
            log::info!("portable: now on {now_text} (was {last}); no library folder moved with it");
        } else {
            match library::reroot::apply(db, &moves) {
                Ok(n) => log::info!("portable: now on {now_text} (was {last}); {} folder(s) followed, {} tracks and {} albums re-pointed", moves.len(), n.tracks, n.albums),
                Err(e) => {
                    // The folders stay where the database has its tracks, and the drive as it
                    // was: tried again at the next start (moved now, the next scan would read
                    // every file anew, without its plays and favourites).
                    log::warn!("portable: re-pointing the library from {last} to {now_text} failed ({e}); tried again at the next start");
                    return false;
                }
            }
            for f in settings.folders.iter_mut() {
                if let Some((_, to)) = moves.iter().find(|(from, _)| from == f) {
                    *f = to.clone();
                }
            }
        }
        changed = true;
    }
    settings.portable_volume = Some(now_text);
    changed
}

/// Started by the system at sign-in ("Start with Windows"): open in the tray, not on screen.
pub const MINIMIZED_FLAG: &str = "--minimized";

pub fn started_minimized() -> bool {
    std::env::args_os().skip(1).any(|a| a == MINIMIZED_FLAG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_folder_keeps_the_caches_below() {
        let d = one_folder(Path::new("X"));
        assert_eq!(d.config, Path::new("X"));
        assert_eq!(d.data, Path::new("X"));
        assert_eq!(d.state, Path::new("X"));
        assert_eq!(d.cache, Path::new("X").join("cache"));
    }

    #[test]
    fn a_read_only_folder_is_refused() {
        let dir = std::env::temp_dir().join(format!("mp-edition-{}", std::process::id()));
        let paths = Paths::from_dirs(one_folder(&dir.join("Data")));
        assert!(check_writable(&paths).is_ok());
        assert!(dir.join("Data").join("cache").is_dir());
        // A file where the folder should be: cannot be made.
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"").unwrap();
        let bad = Paths::from_dirs(one_folder(&blocked.join("Data")));
        let e = check_writable(&bad).unwrap_err();
        assert!(e.contains("blocked"), "{e}");
        let p = Portable { dirs: one_folder(&blocked), folder: blocked.clone(), anchor: dir.clone() };
        let m = not_writable_message(&p, &e);
        assert!(m.contains(PORTABLE_MARKER) && m.contains("cannot write"), "{m}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_edition_has_words() {
        let p = Portable { dirs: one_folder(Path::new("D")), folder: PathBuf::from("D"), anchor: PathBuf::from(".") };
        for e in [Edition::Portable(p), Edition::Installed { all_users: false }, Edition::Installed { all_users: true }, Edition::AppImage, Edition::Standalone] {
            assert!(!e.short().is_empty());
            assert!(e.describe().ends_with('.'), "{}", e.describe());
        }
    }
}
