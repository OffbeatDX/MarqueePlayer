//! Where the app keeps its files. Never inside the music folders.
//!
//! | What | Windows | Linux (XDG base directories) | macOS |
//! | --- | --- | --- | --- |
//! | Settings, theme (`config`) | `%APPDATA%\MarqueePlayer` | `$XDG_CONFIG_HOME/marquee-player` (`~/.config`) | `~/Library/Application Support/MarqueePlayer` |
//! | Library database, tag editor's journal, Last.fm (`data`) | `%APPDATA%\MarqueePlayer` | `$XDG_DATA_HOME/marquee-player` (`~/.local/share`) | `~/Library/Application Support/MarqueePlayer` |
//! | Art, lyrics and stream caches, safe to delete (`cache`) | `%LOCALAPPDATA%\MarqueePlayer` | `$XDG_CACHE_HOME/marquee-player` (`~/.cache`) | `~/Library/Caches/MarqueePlayer` |
//! | Log, rewritten on every start (`state`) | `%APPDATA%\MarqueePlayer` | `$XDG_STATE_HOME/marquee-player` (`~/.local/state`) | `~/Library/Application Support/MarqueePlayer` |
//!
//! Each system's roots come from `platform::system::app_dirs`. The portable edition keeps
//! everything in its own folder instead (`edition`: `Data\` beside the exe on Windows, the
//! AppImage's `.home` folder on Linux; macOS has none). `MP_DATA_DIR` overrides them all (one folder for
//! everything, the caches in `cache` below it), for testing.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    /// Settings and the theme; also the key of the single-instance guard.
    pub config: PathBuf,
    /// What the app keeps: the library database, the tag editor's journal, Last.fm.
    pub data: PathBuf,
    /// What can be made again: art, lyrics, the server's files.
    pub cache: PathBuf,
    /// The log.
    pub state: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        if let Some(dir) = std::env::var_os("MP_DATA_DIR") {
            return Self::from_dirs(crate::edition::one_folder(&PathBuf::from(dir)));
        }
        if let Some(p) = crate::edition::get().portable() {
            return Self::from_dirs(p.dirs.clone());
        }
        Self::from_dirs(crate::platform::system::app_dirs())
    }

    pub fn from_dirs(d: Dirs) -> Self {
        Self { config: d.config, data: d.data, cache: d.cache, state: d.state }
    }

    /// Kept in a folder of its own by `MP_DATA_DIR` (a test or bench run).
    pub fn overridden() -> bool {
        std::env::var_os("MP_DATA_DIR").is_some()
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn theme(&self) -> PathBuf {
        self.config.join("theme.toml")
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("library.db")
    }

    pub fn art(&self) -> PathBuf {
        self.cache.join("art")
    }

    pub fn lyrics(&self) -> PathBuf {
        self.cache.join("lyrics")
    }

    /// The files of server tracks, played as they download (`streaming`).
    pub fn stream(&self) -> PathBuf {
        self.cache.join("stream")
    }

    pub fn log(&self) -> PathBuf {
        self.state.join("marquee-player.log")
    }

    /// The tag editor's undo journal and the records of writes under way.
    pub fn edits(&self) -> PathBuf {
        self.data.join("edits")
    }

    pub fn ensure(&self) {
        for d in [&self.config, &self.data, &self.state] {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::create_dir_all(self.art());
    }
}

/// The roots a system gives the app (`platform::system::app_dirs`), or the portable
/// edition's (`edition`).
#[derive(Clone, Debug, PartialEq)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}
