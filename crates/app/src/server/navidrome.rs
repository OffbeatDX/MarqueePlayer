//! The server sign-in kept between runs: `navidrome.json` in the data folder, beside
//! `lastfm.json` (docs/architecture.md, "Files"). It holds the address, the user, what the server said it is,
//! and the salt and token that sign each request, sealed (`platform::secret`: DPAPI for the
//! current Windows user; on Linux the file is the user's alone, mode 0600). The password is
//! never kept anywhere: sign-in makes the token from it and drops it.
//!
//! Read once at start. A file that does not read as one is kept as `navidrome.json.bad` and the
//! app starts signed out (as `lastfm.json`); a token that does not open here (the portable
//! edition's drive taken to another PC, another Windows account, a damaged seal) leaves the
//! address and user to sign in again with (`Loaded::Locked`). Written to a temporary file,
//! flushed and renamed, so a crash never leaves half a file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use subsonic::Credentials;

/// The file's name, in the data folder.
pub const FILE: &str = "navidrome.json";

pub fn path(data: &Path) -> PathBuf {
    data.join(FILE)
}

/// What the server said it is at sign-in (nothing secret).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct About {
    /// "navidrome"; empty for a server that does not say (plain Subsonic).
    #[serde(default)]
    pub kind: String,
    /// "0.53.3 (13af8ed4)".
    #[serde(default)]
    pub version: String,
    /// The Subsonic API version ("1.16.1").
    #[serde(default)]
    pub api: String,
}

impl About {
    pub fn of(info: &subsonic::ServerInfo) -> Self {
        About { kind: info.kind.clone(), version: info.server_version.clone(), api: info.version.clone() }
    }

    /// "Navidrome 0.53.3", "a Subsonic server (API 1.16.1)".
    pub fn describe(&self) -> String {
        let name = match self.kind.as_str() {
            "" => return if self.api.is_empty() { "a Subsonic server".into() } else { format!("a Subsonic server (API {})", self.api) },
            "navidrome" => "Navidrome".to_owned(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        };
        // Navidrome's version carries its commit: "0.53.3 (13af8ed4)" reads as "0.53.3".
        let version = self.version.split(" (").next().unwrap_or("").trim();
        if version.is_empty() { name } else { format!("{name} {version}") }
    }
}

/// As stored.
#[derive(Serialize, Deserialize)]
struct File {
    url: String,
    user: String,
    /// `platform::secret::SCHEME` of the sealing.
    sealed_with: String,
    /// The sealed "salt\ntoken", as hex.
    secret: String,
    #[serde(default)]
    server: About,
}

/// What the file gave at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// No file (never signed in, or signed out).
    None,
    SignedIn { credentials: Credentials, about: About },
    /// The address and user, but a token that cannot be opened here.
    Locked { url: String, user: String },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

pub fn load(path: &Path) -> Loaded {
    let Ok(text) = std::fs::read_to_string(path) else { return Loaded::None };
    let file: File = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            let mut bad = path.file_name().unwrap_or_default().to_owned();
            bad.push(".bad");
            // The error names a position, never the content.
            if std::fs::copy(path, path.with_file_name(bad)).is_ok() {
                let _ = std::fs::remove_file(path);
                log::error!("{} cannot be read ({e}); kept as .bad, signed out", FILE);
            } else {
                log::error!("{} cannot be read ({e}), nor copied aside; left as it is, signed out", FILE);
            }
            return Loaded::None;
        }
    };
    let opened = (file.sealed_with == crate::platform::secret::SCHEME)
        .then(|| unhex(&file.secret))
        .flatten()
        .and_then(|sealed| crate::platform::secret::open(&sealed))
        .and_then(|plain| String::from_utf8(plain).ok());
    match opened.as_deref().and_then(|p| p.split_once('\n')) {
        Some((salt, token)) if !salt.is_empty() && !token.is_empty() => {
            Loaded::SignedIn { credentials: Credentials { url: file.url, user: file.user, salt: salt.to_owned(), token: token.to_owned() }, about: file.server }
        }
        _ => {
            log::warn!("{FILE}: the saved sign-in cannot be opened here (made for another PC or user, or damaged); sign in again");
            Loaded::Locked { url: file.url, user: file.user }
        }
    }
}

/// Keep a sign-in. False when it cannot be written (read-only folder, full disk): the app is
/// signed in for this run only.
pub fn save(path: &Path, c: &Credentials, about: &About) -> bool {
    let Some(sealed) = crate::platform::secret::seal(format!("{}\n{}", c.salt, c.token).as_bytes()) else {
        log::warn!("{FILE}: the token could not be sealed; not saved");
        return false;
    };
    let file = File { url: c.url.clone(), user: c.user.clone(), sealed_with: crate::platform::secret::SCHEME.into(), secret: hex(&sealed), server: about.clone() };
    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    let written = crate::platform::secret::write_private(&tmp, text.as_bytes()).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("{FILE} cannot be saved: {e}");
        return false;
    }
    true
}

/// Forget the sign-in (signed out): the file, and the copies made beside it that may hold the
/// sealed token too (`navidrome.json.tmp`, a save cut short; `navidrome.json.bad`, a damaged file
/// kept aside by `load`).
pub fn remove(path: &Path) {
    let name = path.file_name().unwrap_or_default();
    for suffix in ["", ".tmp", ".bad"] {
        let mut n = name.to_owned();
        n.push(suffix);
        if let Err(e) = std::fs::remove_file(path.with_file_name(&n))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("{} cannot be removed: {e}", n.to_string_lossy());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mp-navidrome-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn creds() -> Credentials {
        Credentials { url: "https://music.example.org".into(), user: "ann".into(), salt: "0123456789abcdef".into(), token: "fedcba9876543210fedcba9876543210".into() }
    }

    #[test]
    fn a_sign_in_comes_back_and_its_token_is_not_in_the_file() {
        let d = dir("round");
        let p = path(&d);
        assert_eq!(load(&p), Loaded::None);
        let about = About { kind: "navidrome".into(), version: "0.53.3 (13af8ed4)".into(), api: "1.16.1".into() };
        assert!(save(&p, &creds(), &about));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("music.example.org") && text.contains("\"ann\""));
        #[cfg(windows)]
        assert!(!text.contains(&creds().token) && !text.contains(&creds().salt), "sealed: {text}");
        assert!(!text.to_lowercase().contains("password"));
        assert_eq!(load(&p), Loaded::SignedIn { credentials: creds(), about });
        assert!(!p.with_extension("json.tmp").exists());
        remove(&p);
        assert_eq!(load(&p), Loaded::None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Signed out: nothing of the sign-in is left in the folder, not the file, the copy a save
    /// cut short left, nor a damaged one kept aside; the address, user, salt, token and the
    /// sealed hex are in no file there.
    #[test]
    fn signing_out_leaves_nothing_of_the_sign_in() {
        let d = dir("forget");
        let p = path(&d);
        assert!(save(&p, &creds(), &About::default()));
        let sealed = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&p).unwrap()).unwrap()["secret"].as_str().unwrap().to_owned();
        std::fs::write(d.join("navidrome.json.tmp"), format!("{{\"secret\":\"{sealed}\"")).unwrap();
        std::fs::copy(&p, d.join("navidrome.json.bad")).unwrap();
        std::fs::write(d.join("other.json"), "{}").unwrap();
        remove(&p);
        let left: Vec<(String, String)> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).into_owned()))
            .collect();
        assert_eq!(left.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["other.json"], "only what is not the sign-in's");
        let c = creds();
        for (name, text) in &left {
            for needle in [c.url.as_str(), &c.user, &c.salt, &c.token, &sealed] {
                assert!(!text.contains(needle), "{name} still holds {needle}");
            }
        }
        assert_eq!(load(&p), Loaded::None, "a restart is signed out");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_file_is_kept_aside_and_a_foreign_seal_asks_to_sign_in_again() {
        let d = dir("bad");
        let p = path(&d);
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(load(&p), Loaded::None);
        assert!(d.join("navidrome.json.bad").exists() && !p.exists());
        // No copy can be kept aside (here a folder in its place): the file stays where it is.
        std::fs::write(&p, "{ not json").unwrap();
        std::fs::remove_file(d.join("navidrome.json.bad")).unwrap();
        std::fs::create_dir(d.join("navidrome.json.bad")).unwrap();
        assert_eq!(load(&p), Loaded::None);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json", "not removed without its copy");
        std::fs::remove_dir(d.join("navidrome.json.bad")).unwrap();
        // A seal from elsewhere (another PC's DPAPI blob, or a hand-edited secret).
        assert!(save(&p, &creds(), &About::default()));
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        v["secret"] = "00ff".into();
        std::fs::write(&p, v.to_string()).unwrap();
        #[cfg(windows)]
        assert_eq!(load(&p), Loaded::Locked { url: "https://music.example.org".into(), user: "ann".into() });
        v["sealed_with"] = "something-else".into();
        std::fs::write(&p, v.to_string()).unwrap();
        assert_eq!(load(&p), Loaded::Locked { url: "https://music.example.org".into(), user: "ann".into() });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn servers_describe_themselves() {
        let a = |k: &str, v: &str, api: &str| About { kind: k.into(), version: v.into(), api: api.into() }.describe();
        assert_eq!(a("navidrome", "0.53.3 (13af8ed4)", "1.16.1"), "Navidrome 0.53.3");
        assert_eq!(a("gonic", "0.16", "1.15.0"), "Gonic 0.16");
        assert_eq!(a("", "", "1.16.1"), "a Subsonic server (API 1.16.1)");
        assert_eq!(a("", "", ""), "a Subsonic server");
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(unhex("0g"), None);
    }
}
