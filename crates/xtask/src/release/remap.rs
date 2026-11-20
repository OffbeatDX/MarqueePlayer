//! Keeping this machine's paths out of the release binary.
//!
//! Panic messages and `file!()` embed the source path of the code they come from. For the
//! workspace's own crates that path is relative, but dependencies from the registry and
//! generated code (Slint's, under the target folder) are absolute, and both may sit under the
//! home folder, so its path (with the user name in it) would ship. `--remap-path-prefix`
//! rewrites those prefixes at compile time; the C and C++ that build scripts compile (SQLite,
//! libopus, whose assertions name their source files) get the same through CFLAGS. The
//! prefixes depend on the machine, so they are written at release time into a cargo config
//! file in the target folder (never tracked), which the build loads with `--config`.
//! [`super::paths`] then searches the binary, so a path that slips through stops the release.

use std::path::{Path, PathBuf};

/// The prefixes to rewrite and what they become, in the order rustc must get them: when
/// several match, it applies the last one given, so the home folder comes first and the more
/// specific homes and the checkout (which may be under the home folder) after it.
pub fn prefixes(home: &Path, rustup_home: Option<PathBuf>, cargo_home: Option<PathBuf>, checkout: &Path) -> Vec<(String, &'static str)> {
    let rustup = rustup_home.unwrap_or_else(|| home.join(".rustup"));
    let cargo = cargo_home.unwrap_or_else(|| home.join(".cargo"));
    [(home.to_path_buf(), "home"), (rustup, "rustup"), (cargo, "cargo"), (checkout.to_path_buf(), "src")]
        .into_iter()
        .filter_map(|(p, to)| clean(&p).map(|p| (p, to)))
        .collect()
}

/// A path as rustc sees it: absolute, without a trailing separator (except a root's, `C:\`
/// or `/`). None for an empty one.
fn clean(p: &Path) -> Option<String> {
    if p.as_os_str().is_empty() {
        return None;
    }
    let full = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = full.to_string_lossy().into_owned();
    let trimmed = s.trim_end_matches(['\\', '/']);
    let is_root = trimmed.is_empty() || (trimmed.len() == 2 && trimmed.ends_with(':'));
    Some(if is_root { s } else { trimmed.to_owned() })
}

/// The `--remap-path-prefix` flags for rustc.
pub fn rustflags(prefixes: &[(String, &str)]) -> Vec<String> {
    prefixes.iter().map(|(from, to)| format!("--remap-path-prefix={from}={to}")).collect()
}

/// The rustflags this environment would give the build (`CARGO_ENCODED_RUSTFLAGS`, else
/// `RUSTFLAGS`), which the config file's flags would not survive: cargo takes a RUSTFLAGS
/// variable instead of the config's, so they are carried into the file and the variables
/// cleared for the build.
pub fn inherited_rustflags(encoded: Option<String>, plain: Option<String>) -> Vec<String> {
    match (encoded.filter(|e| !e.is_empty()), plain) {
        (Some(e), _) => e.split('\u{1f}').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
        (None, Some(p)) => p.split_whitespace().map(str::to_owned).collect(),
        (None, None) => Vec::new(),
    }
}

/// The C and C++ compilers' share of the remapping, added to CFLAGS and CXXFLAGS. MSVC
/// (`windows`) only trims a prefix (`/d1trimfile`), so only the home folder's, which holds the
/// user name; GCC and Clang map all of them as rustc does. The `cc` crate splits these
/// variables on whitespace, so a path with a space in it is left out (the path check says if
/// anything got through).
pub fn c_flags(windows: bool, prefixes: &[(String, &str)]) -> Option<String> {
    let usable = |p: &str| !p.chars().any(char::is_whitespace);
    let flags: Vec<String> = if windows {
        prefixes.iter().filter(|(p, to)| *to == "home" && usable(p)).map(|(p, _)| format!("/d1trimfile:{}\\", p.trim_end_matches('\\'))).collect()
    } else {
        prefixes.iter().filter(|(p, _)| usable(p)).map(|(p, to)| format!("-ffile-prefix-map={p}={to}")).collect()
    };
    (!flags.is_empty()).then(|| flags.join(" "))
}

/// The cargo config file: the rustflags, and CFLAGS / CXXFLAGS (what the environment has,
/// then `c_flags`) for the build scripts.
pub fn config_toml(rustflags: &[String], c_flags: Option<&str>, cflags: Option<&str>, cxxflags: Option<&str>) -> String {
    let mut s = String::from("# Written by `cargo xtask release` for this machine. Not tracked: it holds local paths.\n# Written again on every release.\n[build]\n");
    s.push_str(&format!("rustflags = [{}]\n", rustflags.iter().map(|f| toml_string(f)).collect::<Vec<_>>().join(", ")));
    if let Some(c) = c_flags {
        s.push_str("\n[env]\n");
        for (var, existing) in [("CFLAGS", cflags), ("CXXFLAGS", cxxflags)] {
            let value = [existing.filter(|e| !e.trim().is_empty()), Some(c)].into_iter().flatten().collect::<Vec<_>>().join(" ");
            s.push_str(&format!("{var} = {{ value = {}, force = true }}\n", toml_string(&value)));
        }
    }
    s
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefixes_in_rustc_order() {
        let got = prefixes(&p(r"C:\Users\Ann\"), None, Some(p(r"D:\cargo")), &p(r"C:\Users\Ann\src\mp"));
        assert_eq!(
            got,
            [(r"C:\Users\Ann".to_owned(), "home"), (r"C:\Users\Ann\.rustup".to_owned(), "rustup"), (r"D:\cargo".to_owned(), "cargo"), (r"C:\Users\Ann\src\mp".to_owned(), "src")]
        );
        assert_eq!(rustflags(&got)[0], r"--remap-path-prefix=C:\Users\Ann=home");
        assert_eq!(c_flags(true, &got).as_deref(), Some(r"/d1trimfile:C:\Users\Ann\"));
        // A drive root keeps its separator.
        assert_eq!(clean(&p(r"D:\")).as_deref(), Some(r"D:\"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_prefixes_in_rustc_order() {
        let got = prefixes(&p("/home/ann/"), Some(p("/opt/rustup")), None, &p("/mnt/c/Users/Ann/mp"));
        assert_eq!(
            got,
            [("/home/ann".to_owned(), "home"), ("/opt/rustup".to_owned(), "rustup"), ("/home/ann/.cargo".to_owned(), "cargo"), ("/mnt/c/Users/Ann/mp".to_owned(), "src")]
        );
        assert_eq!(clean(&p("/")).as_deref(), Some("/"));
    }

    #[test]
    fn gcc_maps_every_prefix_but_one_with_a_space() {
        let pre = [("/home/ann".to_owned(), "home"), ("/home/ann/my src".to_owned(), "src")];
        assert_eq!(c_flags(false, &pre).as_deref(), Some("-ffile-prefix-map=/home/ann=home"));
        assert_eq!(c_flags(true, &[(r"C:\Users\A B".to_owned(), "home")]), None);
    }

    #[test]
    fn inherited_flags_prefer_the_encoded_variable() {
        assert_eq!(inherited_rustflags(Some("-C\u{1f}opt-level=2".into()), Some("-g".into())), ["-C", "opt-level=2"]);
        assert_eq!(inherited_rustflags(Some(String::new()), Some(" -g  -C x ".into())), ["-g", "-C", "x"]);
        assert!(inherited_rustflags(None, None).is_empty());
    }

    #[test]
    fn config_file_quotes_paths_and_keeps_existing_c_flags() {
        let toml = config_toml(&[r#"--remap-path-prefix=C:\a "b"=home"#.to_owned()], Some("/d1trimfile:C:\\a\\"), Some("-O1"), None);
        assert!(toml.contains(r#"rustflags = ["--remap-path-prefix=C:\\a \"b\"=home"]"#), "{toml}");
        assert!(toml.contains(r#"CFLAGS = { value = "-O1 /d1trimfile:C:\\a\\", force = true }"#), "{toml}");
        assert!(toml.contains(r#"CXXFLAGS = { value = "/d1trimfile:C:\\a\\", force = true }"#), "{toml}");
        assert!(!config_toml(&[], None, None, None).contains("[env]"));
    }
}
