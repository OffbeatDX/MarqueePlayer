//! Linux: the editions (`crate::edition`). The AppImage runtime sets `$APPIMAGE` to the
//! AppImage's own path. A folder `<name>.AppImage.home` beside it is the AppImage
//! convention for a portable home (the runtime makes it `$HOME`, and `--appimage-portable-home`
//! creates it): the app then keeps everything there, in the XDG layout under it, whatever the
//! session's `XDG_*_HOME` say, so the folder carries the library. A `<name>.AppImage.config`
//! folder (the runtime's `XDG_CONFIG_HOME`) holds the settings when it is there.
//!
//! Nothing is registered with the desktop by the app: the desktop file in the AppImage says
//! which formats it opens, and integrating it (a menu entry, Open With) is the user's choice,
//! made with the desktop's own tools. There is no "start with the system" setting (the
//! desktop's Autostart settings start any app) and no default apps page to open.

use std::path::{Path, PathBuf};

use crate::edition::{Edition, Portable};
use crate::paths::Dirs;

/// The portable home's layout: the XDG defaults under it.
pub fn portable_dirs(home: &Path, config: Option<&Path>) -> Dirs {
    let app = "marquee-player";
    Dirs {
        config: config.map_or_else(|| home.join(".config"), Path::to_path_buf).join(app),
        data: home.join(".local/share").join(app),
        cache: home.join(".cache").join(app),
        state: home.join(".local/state").join(app),
    }
}

pub fn detect() -> Edition {
    let Some(image) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_absolute()) else { return Edition::Standalone };
    let beside = |suffix: &str| {
        let mut p = image.clone().into_os_string();
        p.push(suffix);
        Some(PathBuf::from(p)).filter(|p| p.is_dir())
    };
    match beside(".home") {
        Some(home) => {
            let config = beside(".config");
            Edition::Portable(Portable { dirs: portable_dirs(&home, config.as_deref()), folder: home, anchor: image })
        }
        None => Edition::AppImage,
    }
}

pub use crate::platform::unix::volume_root;

pub fn autostart(_edition: &Edition) -> Option<bool> {
    None
}

pub fn set_autostart(_edition: &Edition, _on: bool) -> Result<(), String> {
    Err("the desktop's Autostart settings start apps on Linux".into())
}

pub fn default_apps_available(_edition: &Edition) -> bool {
    false
}

pub fn open_default_apps(_edition: &Edition) -> bool {
    false
}

/// Nothing installs over a running copy here.
pub fn installed_running(_edition: &Edition, _quit: fn()) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portable_home_has_the_xdg_layout() {
        let d = portable_dirs(Path::new("/m/App.AppImage.home"), None);
        assert_eq!(d.config, Path::new("/m/App.AppImage.home/.config/marquee-player"));
        assert_eq!(d.data, Path::new("/m/App.AppImage.home/.local/share/marquee-player"));
        assert_eq!(d.cache, Path::new("/m/App.AppImage.home/.cache/marquee-player"));
        assert_eq!(d.state, Path::new("/m/App.AppImage.home/.local/state/marquee-player"));
        let d = portable_dirs(Path::new("/m/h"), Some(Path::new("/m/App.AppImage.config")));
        assert_eq!(d.config, Path::new("/m/App.AppImage.config/marquee-player"));
    }

    #[test]
    fn a_mount_point_is_above_every_path() {
        let exe = std::env::current_exe().unwrap();
        let root = volume_root(&exe).unwrap();
        assert!(std::fs::canonicalize(&exe).unwrap().starts_with(&root));
    }
}
