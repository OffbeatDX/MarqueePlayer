//! THIRD-PARTY-NOTICES.txt, shipped in every package: every package linked into the binary,
//! with its licence and the licence files it ships, the C and C++ libraries bundled inside some
//! of them (Skia, libopus, SQLite), and what the package carries besides the binary (the
//! Visual C++ runtime on Windows, the AppImage's libraries and font on Linux).
//!
//! MIT, BSD, Apache-2.0, ISC, Zlib and the like ask that their copyright and licence notices go
//! with every binary copy; MPL-2.0 (Symphonia) asks that the recipient be told where its source
//! is. This file is how the packages do both.
//!
//! The packages are the ones `cargo metadata` resolves for the target, followed from
//! marquee-player through normal dependencies only (build dependencies and procedural macros
//! run at compile time and are not in the binary), less the workspace's own packages.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::cmd;
use crate::report::Fail;

const RULE: &str = "==============================================================================";

const HEADER: &str = "\
Marquee Player is built on the open-source components listed below. Each is
used under its own licence, given with it; where a component offers a choice of
licences, any of them may be chosen. Source code for every component is at the
repository named with it (crates.io has each exact version). Symphonia (MPL-2.0)
is used unmodified; its source is at https://github.com/pdeljanov/Symphonia.

Slint offers three licences; Marquee Player, itself under the GNU General Public
License version 3 or later (LICENSE.txt), uses Slint under GPL-3.0 (its text is
under \"slint\" below). Settings > About credits it.
";

const SQLITE: &str = "\
The author disclaims copyright to this source code. In place of a legal notice,
here is a blessing:

    May you do good and not evil.
    May you find forgiveness for yourself and forgive others.
    May you share freely, never taking more than you give.
";

/// A package linked into the binary.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub repository: Option<String>,
    /// The folder of its Cargo.toml (its source).
    pub dir: PathBuf,
    pub authors: Vec<String>,
}

/// Something the package carries besides the binary's own packages.
pub struct Extra {
    pub title: String,
    pub licence: String,
    pub source: String,
    pub body: Body,
}

pub enum Body {
    /// Its licence file, which must be there.
    File(PathBuf),
    /// Text written here.
    Text(String),
}

pub struct Notices {
    pub text: String,
    pub packages: usize,
    /// Packages for which no licence text at all was found (said, not fatal: their licence is
    /// named).
    pub bare: Vec<String>,
    /// Licence texts that must be there and were not found: the notices are incomplete.
    pub missing: Vec<String>,
}

/// The notices for `version`, from `cargo metadata` in `root` for `triple`. `profile_dir` is
/// the build's folder (Skia's licence comes with the prebuilt library it downloaded there).
pub fn generate(root: &Path, triple: &str, version: &str, profile_dir: &Path, extras: &[Extra]) -> Result<Notices, Fail> {
    let out = cmd::output(Command::new(cmd::cargo_program()).current_dir(root).args(["metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", triple]))?;
    if !out.status.success() {
        return Err(Fail::msg(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| Fail::msg(format!("cargo metadata: {e}")))?;
    let list = third_party(&meta, "marquee-player").map_err(|e| Fail::msg(format!("cargo metadata: {e}")))?;
    Ok(render(&list, version, profile_dir, extras))
}

/// The third-party packages linked into `root`: followed through normal dependencies, not
/// into procedural macros, less every workspace member; sorted by name and version.
pub fn third_party(meta: &Value, root: &str) -> Result<Vec<Package>, String> {
    let packages: HashMap<&str, &Value> = meta["packages"].as_array().ok_or("no packages")?.iter().filter_map(|p| Some((p["id"].as_str()?, p))).collect();
    let nodes: HashMap<&str, &Value> = meta["resolve"]["nodes"].as_array().ok_or("no resolve graph")?.iter().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    let ours: HashSet<&str> = meta["workspace_members"].as_array().ok_or("no workspace members")?.iter().filter_map(Value::as_str).collect();
    let start = packages.values().find(|p| p["name"] == root && ours.contains(p["id"].as_str().unwrap_or(""))).and_then(|p| p["id"].as_str()).ok_or(format!("no package {root}"))?;

    let proc_macro = |p: &Value| p["targets"].as_array().is_some_and(|t| t.iter().any(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "proc-macro"))));
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for d in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            let Some(pkg) = d["pkg"].as_str() else { continue };
            let normal = d["dep_kinds"].as_array().is_some_and(|k| k.iter().any(|k| k["kind"].is_null()));
            if normal && packages.get(pkg).is_some_and(|p| !proc_macro(p)) {
                stack.push(pkg);
            }
        }
    }
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let mut list: Vec<Package> = seen
        .into_iter()
        .filter(|id| !ours.contains(id))
        .filter_map(|id| packages.get(id))
        .map(|p| Package {
            name: text(&p["name"]).unwrap_or_default(),
            version: text(&p["version"]).unwrap_or_default(),
            license: text(&p["license"]),
            repository: text(&p["repository"]),
            dir: p["manifest_path"].as_str().map(Path::new).and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default(),
            authors: p["authors"].as_array().map(|a| a.iter().filter_map(text).collect()).unwrap_or_default(),
        })
        .collect();
    list.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(list)
}

/// The SPDX ids a licence expression names, in order, once each.
pub fn licence_ids(expr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in expr.replace(['(', ')', '/'], " ").split_whitespace() {
        if !matches!(t, "OR" | "AND" | "WITH") && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    }
    out
}

/// Whether a licence text names no copyright holder (a template: `Copyright (c) <year>
/// <copyright holders>`, or no copyright line at all), so it may stand as the standard text
/// for a package that ships none. A package's own licence file usually begins with its
/// authors' copyright line; given as another package's text, it would credit them with that
/// package.
pub fn names_no_holder(text: &str) -> bool {
    !text.lines().any(|line| {
        let l = line.trim().to_ascii_lowercase();
        let notice = l.starts_with("copyright") && (l.contains("(c)") || l.contains('\u{a9}') || l.bytes().any(|b| b.is_ascii_digit()));
        let placeholder = ["<year>", "[yyyy]", "[year]", "<yyyy>"].iter().any(|p| l.contains(p));
        notice && !placeholder
    })
}

/// Whether a file name is a licence file's: LICENSE, LICENCE, COPYING, NOTICE, UNLICENSE,
/// COPYRIGHT, alone or followed by `-`, `.` or `_` and anything (case ignored).
pub fn is_licence_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["license", "licence", "copying", "notice", "unlicense", "copyright"]
        .iter()
        .any(|p| n == *p || n.strip_prefix(p).is_some_and(|rest| rest.starts_with(['-', '.', '_'])))
}

/// Each licence text in full the first time, and referred to after: the same text (Apache-2.0
/// above all) ships in hundreds of packages.
#[derive(Default)]
pub struct Texts {
    seen: HashMap<String, String>,
}

impl Texts {
    pub fn append(&mut self, w: &mut Vec<String>, owner: &str, name: &str, text: &str) {
        let norm = text.replace("\r\n", "\n").trim().to_owned();
        if let Some(first) = self.seen.get(&norm) {
            w.push(format!("--- {name}: the same text as {first} above."));
            return;
        }
        w.push(format!("--- {name}"));
        w.push(norm.clone());
        self.seen.insert(norm, format!("{owner}'s {name}"));
    }
}

fn render(list: &[Package], version: &str, profile_dir: &Path, extras: &[Extra]) -> Notices {
    let mut w: Vec<String> = Vec::new();
    w.push(format!("Marquee Player{}: third-party notices", if version.is_empty() { String::new() } else { format!(" {version}") }));
    w.push(String::new());
    w.extend(HEADER.lines().map(str::to_owned));
    w.push(String::new());

    let standard = standard_texts(list);
    let mut texts = Texts::default();
    let mut bare = Vec::new();
    for p in list {
        let licence = p.license.as_deref().unwrap_or("not given (see its licence file)");
        w.push(RULE.into());
        w.push(format!("{} {}", p.name, p.version));
        w.push(format!("Licence: {licence}"));
        if let Some(r) = &p.repository {
            w.push(format!("Source: {r}"));
        }
        w.push(String::new());
        let files = licence_files(&p.dir);
        if files.is_empty() {
            // The standard text of each licence the expression names, as far as known, with
            // the package's own authors as its copyright holders.
            let ids: Vec<String> = licence_ids(p.license.as_deref().unwrap_or("")).into_iter().filter(|id| standard.contains_key(id)).collect();
            if ids.is_empty() {
                w.push(format!("(The package ships no licence file: its licence is {licence}, as above.)"));
                bare.push(format!("{} {} ({licence})", p.name, p.version));
            } else {
                w.push(if p.authors.is_empty() {
                    format!("(The package ships no licence file. Its copyright holders are the authors of {} at its source above.)", p.name)
                } else {
                    format!("(The package ships no licence file. Its copyright holders, as its Cargo.toml names them: {}.)", p.authors.join(", "))
                });
                for id in ids {
                    w.push(format!("(The standard {id} text follows.)"));
                    texts.append(&mut w, "the standard", &format!("{id} text"), &standard[&id]);
                }
            }
        }
        for f in &files {
            texts.append(&mut w, &p.name, &file_name(f), &read_text(f));
        }
        w.push(String::new());
    }

    // ---- C and C++ code bundled inside packages ----
    let mut missing = Vec::new();
    let section = |w: &mut Vec<String>, title: &str, licence: &str, source: &str| {
        w.push(RULE.into());
        w.push(title.into());
        w.push(format!("Licence: {licence}"));
        if !source.is_empty() {
            w.push(format!("Source: {source}"));
        }
        w.push(String::new());
    };
    let dir_of = |name: &str| list.iter().find(|p| p.name == name).map(|p| p.dir.clone());

    // Skia: the prebuilt library rust-skia downloads; its licence comes with it.
    section(&mut w, "Skia (the prebuilt library inside skia-bindings)", "BSD-3-Clause", "https://skia.org/");
    match skia_licence(profile_dir) {
        Some(f) => texts.append(&mut w, "Skia", "LICENSE", &read_text(&f)),
        None => missing.push(format!("Skia (LICENSE_SKIA not found under {})", profile_dir.join("build").display())),
    }
    w.push(String::new());

    section(&mut w, "libopus (compiled from the source bundled in opusic-sys)", "BSD-3-Clause", "https://opus-codec.org/");
    match dir_of("opusic-sys").map(|d| d.join("opus").join("COPYING")).filter(|f| f.is_file()) {
        Some(f) => texts.append(&mut w, "libopus", "COPYING", &read_text(&f)),
        None => missing.push("libopus (opus/COPYING not found in opusic-sys)".into()),
    }
    w.push(String::new());

    section(&mut w, "SQLite (compiled from the amalgamation bundled in libsqlite3-sys)", "Public domain", "https://sqlite.org/");
    w.extend(SQLITE.lines().map(str::to_owned));
    w.push(String::new());
    if dir_of("libsqlite3-sys").is_none() {
        missing.push("SQLite (libsqlite3-sys not found)".into());
    }

    for e in extras {
        section(&mut w, &e.title, &e.licence, &e.source);
        match &e.body {
            Body::Text(t) => w.extend(t.lines().map(str::to_owned)),
            Body::File(f) if f.is_file() => texts.append(&mut w, &e.title, &file_name(f), &read_text(f)),
            Body::File(f) => missing.push(format!("{} ({} not found)", e.title, f.display())),
        }
        w.push(String::new());
    }

    let mut text = w.join("\n");
    text.push('\n');
    Notices { text, packages: list.len(), bare, missing }
}

/// Standard licence texts by SPDX id, for the packages that ship no licence file: first the
/// reference texts in Slint's checkout (`LICENSES/MIT.txt` and the rest, REUSE templates), then,
/// for the ids still missing, the one licence file of a package under that licence alone, but
/// only a text that names no copyright holder ([`names_no_holder`]).
fn standard_texts(list: &[Package]) -> HashMap<String, String> {
    let mut standard = HashMap::new();
    if let Some(slint) = list.iter().find(|p| p.name == "slint") {
        let lic = slint.dir.ancestors().take(7).map(|d| d.join("LICENSES")).find(|d| d.join("MIT.txt").is_file());
        for f in lic.map(|d| sorted_files(&d)).unwrap_or_default() {
            if let (Some(id), Some("txt")) = (f.file_stem().and_then(|s| s.to_str()), f.extension().and_then(|s| s.to_str())) {
                let t = read_text(&f);
                if names_no_holder(&t) {
                    standard.entry(id.to_owned()).or_insert(t);
                }
            }
        }
    }
    for p in list {
        let Some(id) = p.license.as_deref().filter(|l| !l.contains(char::is_whitespace) && !l.contains('/')) else { continue };
        if standard.contains_key(id) {
            continue;
        }
        if let [only] = &licence_files(&p.dir)[..] {
            let t = read_text(only);
            if names_no_holder(&t) {
                standard.insert(id.to_owned(), t);
            }
        }
    }
    standard
}

/// A package's licence files: those in its folder, then everything in its LICENSES folder
/// (Slint's crates keep their licences there).
fn licence_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = sorted_files(dir).into_iter().filter(|f| is_licence_file(&file_name(f))).collect();
    files.extend(sorted_files(&dir.join("LICENSES")));
    files
}

/// The files in a folder (not its folders), by name, case ignored.
fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort_by_key(|f| (file_name(f).to_ascii_lowercase(), file_name(f)));
    files
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A text file, a UTF-8 BOM dropped. A file git checked out as a symbolic link may be a
/// text file holding the link's target: then the target's text.
fn read_text(path: &Path) -> String {
    read_text_at(path, 4)
}

fn read_text_at(path: &Path, depth: u32) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes).into_owned();
    let t = text.trim();
    if depth > 0 && t.len() < 300 && !t.contains('\n') && t.contains(['/', '\\']) {
        let target = path.parent().unwrap_or(Path::new(".")).join(t);
        if target.is_file() {
            return read_text_at(&target, depth - 1);
        }
    }
    text
}

/// LICENSE_SKIA, which skia-bindings' build script downloads with the library into its
/// output folder: the newest, if the build folder has more than one.
fn skia_licence(profile_dir: &Path) -> Option<PathBuf> {
    let build = profile_dir.join("build");
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut stack: Vec<PathBuf> = std::fs::read_dir(&build).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).filter(|p| file_name(p).starts_with("skia-bindings-")).collect();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().filter_map(Result::ok) {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() && e.file_name() == "LICENSE_SKIA" => found.push((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)),
                _ => {}
            }
        }
    }
    found.into_iter().max_by_key(|(t, _)| *t).map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(id: &str, name: &str, deps: &[(&str, &str)]) -> (Value, Value) {
        let kind = |k: &str| if k == "normal" { Value::Null } else { Value::String(k.into()) };
        let targets = if name.ends_with("-derive") { r#"[{"kind":["proc-macro"]}]"# } else { r#"[{"kind":["lib"]}]"# };
        let p = serde_json::json!({ "id": id, "name": name, "version": "1.0.0", "license": "MIT", "manifest_path": format!("/r/{name}/Cargo.toml"), "authors": [], "targets": serde_json::from_str::<Value>(targets).unwrap() });
        let n = serde_json::json!({ "id": id, "deps": deps.iter().map(|(d, k)| serde_json::json!({ "pkg": d, "dep_kinds": [{ "kind": kind(k) }] })).collect::<Vec<_>>() });
        (p, n)
    }

    #[test]
    fn third_party_follows_normal_dependencies_and_leaves_out_the_workspace() {
        let all = [
            pkg("app", "marquee-player", &[("subsonic", "normal"), ("zeta", "normal"), ("cc", "build"), ("serde-derive", "normal"), ("mockito", "dev")]),
            // A workspace crate that once was taken for a third-party one: still followed.
            pkg("subsonic", "subsonic", &[("md5", "normal"), ("alpha", "normal")]),
            pkg("md5", "md5", &[]),
            pkg("alpha", "alpha", &[]),
            pkg("zeta", "zeta", &[("alpha", "normal")]),
            pkg("cc", "cc", &[]),
            pkg("serde-derive", "serde-derive", &[("beta", "normal")]),
            pkg("beta", "beta", &[]),
            pkg("mockito", "mockito", &[]),
        ];
        let meta = serde_json::json!({
            "packages": all.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            "resolve": { "nodes": all.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>() },
            "workspace_members": ["app", "subsonic", "md5"],
        });
        let names: Vec<String> = third_party(&meta, "marquee-player").unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert!(third_party(&meta, "nothing").is_err());
    }

    #[test]
    fn licence_expressions() {
        assert_eq!(licence_ids("MIT OR Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(licence_ids("(MIT OR Apache-2.0) AND Unicode-3.0"), ["MIT", "Apache-2.0", "Unicode-3.0"]);
        assert_eq!(licence_ids("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(licence_ids("Apache-2.0 WITH LLVM-exception OR MIT OR Apache-2.0"), ["Apache-2.0", "LLVM-exception", "MIT"]);
        assert!(licence_ids("").is_empty());
    }

    #[test]
    fn a_text_with_a_holder_is_no_standard_text() {
        assert!(!names_no_holder("MIT License\n\nCopyright (c) 2020 Some One <a@b.c>\n\nPermission is hereby granted"));
        assert!(!names_no_holder("Copyright 2016 Foo Developers\n\nLicensed under the Apache License"));
        assert!(!names_no_holder("   Copyright \u{a9} Bar Inc.\n"));
        // Templates and texts without a copyright line.
        assert!(names_no_holder("MIT License\n\nCopyright (c) <year> <copyright holders>\n\nPermission is hereby granted"));
        assert!(names_no_holder("   Copyright [yyyy] [name of copyright owner]\n"));
        assert!(names_no_holder("copyright owner or by an individual or Legal Entity authorized to submit\n"));
        assert!(names_no_holder("Boost Software License - Version 1.0 - August 17th, 2003\n"));
    }

    #[test]
    fn licence_file_names() {
        for yes in ["LICENSE", "license-mit", "LICENSE-APACHE", "LICENCE.txt", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT_notice", "License.md"] {
            assert!(is_licence_file(yes), "{yes}");
        }
        for no in ["licensee", "README.md", "Cargo.toml", "LICENSES", "copyrights"] {
            assert!(!is_licence_file(no), "{no}");
        }
    }

    #[test]
    fn the_same_text_is_given_once_and_referred_to_after() {
        let mut t = Texts::default();
        let mut w = Vec::new();
        t.append(&mut w, "a", "LICENSE-MIT", "Copyright A\r\n\r\nPermission...\r\n");
        t.append(&mut w, "b", "LICENSE", "  Copyright A\n\nPermission...");
        t.append(&mut w, "c", "LICENSE", "Copyright C\n\nPermission...");
        assert_eq!(w, ["--- LICENSE-MIT", "Copyright A\n\nPermission...", "--- LICENSE: the same text as a's LICENSE-MIT above.", "--- LICENSE", "Copyright C\n\nPermission..."]);
    }
}
