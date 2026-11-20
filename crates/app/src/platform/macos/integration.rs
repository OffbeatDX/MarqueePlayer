//! macOS: the editions (`crate::edition`) and "Open at login". The app bundle keeps its
//! files in the user's Library (`system::app_dirs`), wherever the bundle is: every copy is the
//! standalone edition. (There is no portable edition on macOS; `MP_DATA_DIR` still puts
//! everything in one folder.)
//!
//! "Open at login" is a LaunchAgent of the user's own, `~/Library/LaunchAgents/<APP_ID>.plist`,
//! which launchd reads at login: it runs this executable (inside the bundle) with the flag
//! that starts the app in the menu bar. The document types the app opens are the bundle's
//! Info.plist's (the Finder reads them); nothing else is registered, and there is no default
//! apps page to open.

use std::path::{Path, PathBuf};

use crate::edition::Edition;

pub use crate::platform::unix::volume_root;

pub fn detect() -> Edition {
    Edition::Standalone
}

/// The LaunchAgent's file.
fn agent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())?;
    Some(home.join("Library/LaunchAgents").join(format!("{}.plist", crate::platform::APP_ID)))
}

fn this_exe() -> Option<PathBuf> {
    std::env::current_exe().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p))
}

/// Text as a property list's string holds it.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The LaunchAgent for `exe`: run it at login, in the menu bar.
pub fn agent_plist(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>{flag}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        label = crate::platform::APP_ID,
        exe = xml_escape(&exe.to_string_lossy()),
        flag = crate::edition::MINIMIZED_FLAG,
    )
}

/// Is "Open at login" on for this executable? (The agent names it; one left by a copy
/// elsewhere is not this one's.)
pub fn autostart(_edition: &Edition) -> Option<bool> {
    let (path, exe) = (agent_path()?, this_exe()?);
    let text = std::fs::read_to_string(path).ok();
    Some(text.is_some_and(|t| t.contains(&format!("<string>{}</string>", xml_escape(&exe.to_string_lossy())))))
}

/// Turn "Open at login" on (this executable, in the menu bar) or off.
pub fn set_autostart(_edition: &Edition, on: bool) -> Result<(), String> {
    let path = agent_path().ok_or("cannot tell where the home folder is")?;
    if !on {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {} ({e})", path.display())),
        };
    }
    let exe = this_exe().ok_or("cannot tell where the app is")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {} ({e})", dir.display()))?;
    }
    std::fs::write(&path, agent_plist(&exe)).map_err(|e| format!("cannot write {} ({e})", path.display()))
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
    fn the_agent_runs_this_exe_in_the_menu_bar() {
        let p = agent_plist(Path::new("/Applications/Marquee Player.app/Contents/MacOS/MarqueePlayer"));
        assert!(p.contains(&format!("<key>Label</key>\n\t<string>{}</string>", crate::platform::APP_ID)));
        assert!(p.contains("<string>/Applications/Marquee Player.app/Contents/MacOS/MarqueePlayer</string>"));
        assert!(p.contains(&format!("<string>{}</string>", crate::edition::MINIMIZED_FLAG)));
        assert!(agent_plist(Path::new("/a&b/<c>")).contains("/a&amp;b/&lt;c&gt;"));
    }
}
