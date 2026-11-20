//! Playlist files: M3U8 out (UTF-8, `#EXTM3U` / `#EXTINF`, absolute paths or paths
//! relative to the file), M3U and M3U8 in, matched to library tracks by path.
//!
//! Matching tolerates what playlists written elsewhere do: forward or back slashes, any
//! letter case on Windows (where paths ignore it; elsewhere case is part of a name),
//! `file:///` URLs, `.` and `..`, and relative entries, tried against the playlist's own
//! folder and then against each library root. An `.m3u` that is not valid UTF-8 is read
//! as Windows-1252 (what older Windows players wrote). Only reads the playlist file;
//! writing happens only where the user saves. Files are written in the system's own way:
//! backslashes and CRLF on Windows, forward slashes and LF elsewhere.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::query::Track;

/// Paths as this system writes them: Windows' backslashes and letter case that does not
/// matter; macOS's forward slashes, letter case that does not matter (its default file system
/// ignores it) and names compared in one Unicode form (its file systems may hand them back
/// decomposed, NFD, where a playlist made elsewhere has them composed); elsewhere forward
/// slashes and letter case that does. Entries are read with either slash everywhere (a
/// playlist made on Windows works on Linux and the other way).
#[cfg(windows)]
mod native {
    pub const SEP: &str = "\\";
    pub const OTHER: char = '/';
    pub const NEWLINE: &str = "\r\n";
    pub fn same(a: &str, b: &str) -> bool {
        a.eq_ignore_ascii_case(b)
    }
    pub fn fold(s: String) -> String {
        s.to_lowercase()
    }
}

#[cfg(target_os = "macos")]
mod native {
    pub const SEP: &str = "/";
    pub const OTHER: char = '\\';
    pub const NEWLINE: &str = "\n";
    pub fn same(a: &str, b: &str) -> bool {
        a == b || fold(a.to_owned()) == fold(b.to_owned())
    }
    pub fn fold(s: String) -> String {
        crate::names::nfc(&s).to_lowercase()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod native {
    pub const SEP: &str = "/";
    pub const OTHER: char = '\\';
    pub const NEWLINE: &str = "\n";
    pub fn same(a: &str, b: &str) -> bool {
        a == b
    }
    pub fn fold(s: String) -> String {
        s
    }
}

/// A path with this system's slashes.
fn with_native_slashes(p: &str) -> String {
    p.replace(native::OTHER, native::SEP)
}

/// One entry of a playlist file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// As written.
    pub location: String,
    /// The `#EXTINF` title ("Artist - Title"), if any.
    pub title: Option<String>,
}

/// A playlist as M3U8 text. `relative`: paths relative to `dest`'s folder where they can
/// be (same drive), absolute otherwise. A server's own tracks (`Track::is_remote`) have no file
/// to name and are left out (`skipped` counts them, for the app to say so).
pub fn export(tracks: &[Track], dest: &Path, relative: bool) -> String {
    let base = dest.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out = format!("#EXTM3U{}", native::NEWLINE);
    for t in tracks.iter().filter(|t| !t.is_remote()) {
        let secs = if t.duration > 0.0 { t.duration.round() as i64 } else { -1 };
        let name = if t.artist.is_empty() { t.title.clone() } else { format!("{} - {}", t.artist, t.title) };
        out.push_str(&format!("#EXTINF:{secs},{}{}", name.replace(['\r', '\n'], " "), native::NEWLINE));
        let path = with_native_slashes(&t.path);
        let line = if relative { relative_to(&path, &base).unwrap_or(path) } else { path };
        out.push_str(&line);
        out.push_str(native::NEWLINE);
    }
    out
}

/// How many of `tracks` an export leaves out: a server's own tracks, which have no file.
pub fn skipped(tracks: &[Track]) -> usize {
    tracks.iter().filter(|t| t.is_remote()).count()
}

/// `path` relative to the folder `base`, when both are on the same drive.
pub fn relative_to(path: &str, base: &Path) -> Option<String> {
    let p = normalise(Path::new(path));
    let b = normalise(base);
    let pc: Vec<Component> = p.components().collect();
    let bc: Vec<Component> = b.components().collect();
    let same = |a: &Component, b: &Component| native::same(&a.as_os_str().to_string_lossy(), &b.as_os_str().to_string_lossy());
    // Different drives (or one is not absolute): no relative path.
    if pc.first().zip(bc.first()).is_none_or(|(a, b)| !same(a, b)) {
        return None;
    }
    let common = pc.iter().zip(&bc).take_while(|(a, b)| same(a, b)).count();
    let mut parts: Vec<String> = vec!["..".into(); bc.len() - common];
    parts.extend(pc[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    Some(parts.join(native::SEP))
}

/// Resolve `.` and `..` without touching the disk; slashes made native.
fn normalise(p: &Path) -> PathBuf {
    let p = PathBuf::from(with_native_slashes(&p.to_string_lossy()));
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The comparison key of a path: native slashes, `.`/`..` resolved, lower case on Windows
/// and macOS (and in one Unicode form on macOS).
pub fn key(p: &Path) -> String {
    native::fold(normalise(p).to_string_lossy().into_owned())
}

/// Text of a playlist file: UTF-8 (with or without a BOM), else Windows-1252.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›',
        'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        b => b as char,
    }
}

/// The entries of an M3U / M3U8 text, in order.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut title = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            title = info.split_once(',').map(|(_, t)| t.trim().to_owned()).filter(|t| !t.is_empty());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        out.push(Entry { location: line.to_owned(), title: title.take() });
    }
    out
}

/// `file:///C:/a%20b.flac` -> `C:/a b.flac` (elsewhere `file:///home/a%20b.flac` ->
/// `/home/a b.flac`); on Windows `file://nas/music/a.flac` -> `//nas/music/a.flac`, a share;
/// anything else unchanged.
fn from_url(s: &str) -> Option<String> {
    #[cfg(windows)]
    let unc;
    #[cfg(windows)]
    let rest = match s.strip_prefix("file://")? {
        r if r.starts_with('/') => &r[1..],
        r => match r.split_once('/') {
            Some((host, path)) if host.eq_ignore_ascii_case("localhost") => path,
            // A host, not a drive (`file://C:/a.flac`, as some programs write it).
            Some((host, _)) if !host.is_empty() && !host.ends_with(':') => {
                unc = format!("//{r}");
                &unc
            }
            _ => r,
        },
    };
    #[cfg(not(windows))]
    let rest = s.strip_prefix("file://").map(|r| r.strip_prefix("localhost").unwrap_or(r))?;
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// What an import found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matched {
    /// Library track ids, in playlist order (a track listed twice is added twice).
    pub ids: Vec<i64>,
    /// Entries with no library track, as written (with their title when the file gave one).
    pub missing: Vec<String>,
}

/// Match entries to tracks. `library`: every track's path and id; `dir`: the playlist
/// file's folder; `roots`: the library folders.
pub fn match_entries(entries: &[Entry], library: &[(String, i64)], dir: &Path, roots: &[PathBuf]) -> Matched {
    let index: HashMap<String, i64> = library.iter().map(|(p, id)| (key(Path::new(p)), *id)).collect();
    let mut m = Matched::default();
    for e in entries {
        let loc = from_url(&e.location).unwrap_or_else(|| e.location.clone());
        let p = PathBuf::from(with_native_slashes(&loc));
        // A drive letter (Windows): absolute, whatever follows it. (Elsewhere a colon is
        // just part of a name.)
        let drive = cfg!(windows) && loc.contains(':');
        let candidates: Vec<PathBuf> = if p.has_root() && p.is_absolute() || drive {
            vec![p]
        } else if p.has_root() {
            // "\Music\a.flac": rooted on the playlist's drive.
            dir.components().next().map(|d| PathBuf::from(d.as_os_str()).join(&p)).into_iter().collect()
        } else {
            std::iter::once(dir.join(&p)).chain(roots.iter().map(|r| r.join(&p))).collect()
        };
        match candidates.iter().find_map(|c| index.get(&key(c))) {
            Some(&id) => m.ids.push(id),
            None => m.missing.push(match &e.title {
                Some(t) => format!("{} ({t})", e.location),
                None => e.location.clone(),
            }),
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(path: &str, artist: &str, title: &str, d: f64) -> Track {
        Track { path: path.into(), artist: artist.into(), title: title.into(), duration: d, ..Track::default() }
    }

    #[cfg(windows)]
    #[test]
    fn exports_extended_m3u8_absolute_or_relative() {
        let tracks = [t("M:/Music/Kestrel/01 Paper Moons.flac", "Kestrel", "Paper Moons", 245.4), t("M:/Music/Ä/02.flac", "", "Zwei", 0.0)];
        let abs = export(&tracks, Path::new("D:/Lists/mine.m3u8"), false);
        assert_eq!(abs, "#EXTM3U\r\n#EXTINF:245,Kestrel - Paper Moons\r\nM:\\Music\\Kestrel\\01 Paper Moons.flac\r\n#EXTINF:-1,Zwei\r\nM:\\Music\\Ä\\02.flac\r\n");
        let rel = export(&tracks, Path::new("M:/Music/Lists/mine.m3u8"), true);
        assert!(rel.contains("\r\n..\\Kestrel\\01 Paper Moons.flac\r\n"), "{rel}");
        assert!(rel.contains("\r\n..\\Ä\\02.flac\r\n"));
        // Another drive: absolute, even when relative was asked for.
        assert!(export(&tracks, Path::new("D:/Lists/mine.m3u8"), true).contains("\r\nM:\\Music\\Kestrel\\01 Paper Moons.flac\r\n"));
        // Same folder.
        assert_eq!(relative_to("M:/Music/Kestrel/a.flac", Path::new("m:\\music\\kestrel")).as_deref(), Some("a.flac"));
    }

    #[test]
    fn parses_m3u_and_its_titles() {
        let e = parse("\u{feff}#EXTM3U\n#EXTINF:120,A - B\nC:\\x\\b.flac\n\n# a comment\nrel/c.mp3\r\n");
        assert_eq!(
            e,
            [Entry { location: "C:\\x\\b.flac".into(), title: Some("A - B".into()) }, Entry { location: "rel/c.mp3".into(), title: None }]
        );
        assert_eq!(decode(b"\xEF\xBB\xBFa\xc3\xa9"), "aé");
        assert_eq!(decode(b"Beyonc\xe9 \x96 Halo"), "Beyoncé – Halo", "Windows-1252 when not UTF-8");
    }

    #[cfg(windows)]
    #[test]
    fn matches_by_path_whatever_the_slashes_case_or_base() {
        let library = vec![
            ("M:/Music/Kestrel/01 Paper Moons.flac".to_owned(), 1),
            ("M:/Music/Kestrel/02 Ash and Clover.flac".to_owned(), 2),
            ("M:/Music/Aurora Lines/01 Lantern.flac".to_owned(), 3),
            ("M:/Music/Ünïcödé/a b.flac".to_owned(), 4),
        ];
        let roots = [PathBuf::from("M:/Music")];
        let text = "#EXTM3U\n\
            m:\\music\\kestrel\\01 PAPER MOONS.flac\n\
            ../Kestrel/02 Ash and Clover.flac\n\
            Aurora Lines/01 Lantern.flac\n\
            file:///M:/Music/%C3%9Cn%C3%AFc%C3%B6d%C3%A9/a%20b.flac\n\
            #EXTINF:200,Gone - Missing\n\
            M:/Music/Gone/x.flac\n\
            \\Music\\Kestrel\\01 Paper Moons.flac\n\
            http://example.com/stream.mp3\n";
        let m = match_entries(&parse(text), &library, Path::new("M:/Music/Lists"), &roots);
        assert_eq!(m.ids, [1, 2, 3, 4, 1], "absolute, relative to the file, relative to a root, a file URL, drive-rooted");
        assert_eq!(m.missing, ["M:/Music/Gone/x.flac (Gone - Missing)", "http://example.com/stream.mp3"]);
    }

    /// A file URL with a host is a file on a share (`file://nas/music/a.flac`), not a path
    /// relative to the playlist's folder.
    #[cfg(windows)]
    #[test]
    fn a_file_url_with_a_host_is_a_share() {
        let library = vec![(r"\\nas\music\a b.flac".to_owned(), 1), ("M:/Music/c.flac".to_owned(), 2), ("M:/Lists/nas/music/a b.flac".to_owned(), 3)];
        let text = "file://nas/music/a%20b.flac\nfile://localhost/M:/Music/c.flac\nfile://M:/Music/c.flac\n";
        let m = match_entries(&parse(text), &library, Path::new("M:/Lists"), &[]);
        assert_eq!(m.ids, [1, 2, 2]);
        assert!(m.missing.is_empty(), "{:?}", m.missing);
    }

    #[cfg(windows)]
    #[test]
    fn round_trips() {
        let tracks = [t("M:/Music/K/01.flac", "K", "One", 60.0), t("M:/Music/K/sub/02.flac", "K", "Two", 61.0)];
        let library: Vec<(String, i64)> = tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i as i64 + 10)).collect();
        for relative in [false, true] {
            let dest = Path::new("M:/Music/Lists/k.m3u8");
            let m = match_entries(&parse(&export(&tracks, dest, relative)), &library, dest.parent().unwrap(), &[]);
            assert_eq!(m.ids, [10, 11], "relative {relative}");
            assert!(m.missing.is_empty());
        }
    }

    /// Linux (any system with forward slashes and case in names): the same three cases.
    #[cfg(not(windows))]
    #[test]
    fn exports_extended_m3u8_absolute_or_relative() {
        let tracks = [t("/home/me/Music/Kestrel/01 Paper Moons.flac", "Kestrel", "Paper Moons", 245.4), t("/home/me/Music/Ä/02.flac", "", "Zwei", 0.0)];
        let abs = export(&tracks, Path::new("/home/me/Lists/mine.m3u8"), false);
        assert_eq!(abs, "#EXTM3U\n#EXTINF:245,Kestrel - Paper Moons\n/home/me/Music/Kestrel/01 Paper Moons.flac\n#EXTINF:-1,Zwei\n/home/me/Music/Ä/02.flac\n");
        let rel = export(&tracks, Path::new("/home/me/Music/Lists/mine.m3u8"), true);
        assert!(rel.contains("\n../Kestrel/01 Paper Moons.flac\n"), "{rel}");
        assert!(rel.contains("\n../Ä/02.flac\n"));
        assert_eq!(relative_to("/home/me/Music/Kestrel/a.flac", Path::new("/home/me/Music/Kestrel")).as_deref(), Some("a.flac"));
        // Case is part of a name on Linux: another folder. Not on macOS.
        #[cfg(not(target_os = "macos"))]
        assert_eq!(relative_to("/home/me/Music/Kestrel/a.flac", Path::new("/home/me/music/kestrel")).as_deref(), Some("../../Music/Kestrel/a.flac"));
        #[cfg(target_os = "macos")]
        assert_eq!(relative_to("/home/me/Music/Kestrel/a.flac", Path::new("/home/me/music/kestrel")).as_deref(), Some("a.flac"));
    }

    #[cfg(not(windows))]
    #[test]
    fn matches_by_path_whatever_the_slashes_or_base() {
        let library = vec![
            ("/home/me/Music/Kestrel/01 Paper Moons.flac".to_owned(), 1),
            ("/home/me/Music/Kestrel/02 Ash and Clover.flac".to_owned(), 2),
            ("/home/me/Music/Aurora Lines/01 Lantern.flac".to_owned(), 3),
            ("/home/me/Music/Ünïcödé/a b.flac".to_owned(), 4),
            ("/home/me/Music/Live: 1999/01.flac".to_owned(), 5),
        ];
        let roots = [PathBuf::from("/home/me/Music")];
        let text = "#EXTM3U\n\
            /home/me/Music/Kestrel/01 Paper Moons.flac\n\
            ..\\Kestrel\\02 Ash and Clover.flac\n\
            Aurora Lines/01 Lantern.flac\n\
            file:///home/me/Music/%C3%9Cn%C3%AFc%C3%B6d%C3%A9/a%20b.flac\n\
            Live: 1999/01.flac\n\
            #EXTINF:200,Gone - Missing\n\
            /home/me/Music/Gone/x.flac\n\
            /home/me/music/kestrel/01 PAPER MOONS.flac\n\
            http://example.com/stream.mp3\n";
        let m = match_entries(&parse(text), &library, Path::new("/home/me/Music/Lists"), &roots);
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(m.ids, [1, 2, 3, 4, 5], "absolute, relative with Windows slashes, relative to a root, a file URL, a colon in a name");
            assert_eq!(
                m.missing,
                ["/home/me/Music/Gone/x.flac (Gone - Missing)", "/home/me/music/kestrel/01 PAPER MOONS.flac", "http://example.com/stream.mp3"],
                "another letter case is another file here"
            );
        }
        // macOS: another letter case is the same file.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(&m.ids[..5], [1, 2, 3, 4, 5], "absolute, relative with Windows slashes, relative to a root, a file URL, a colon in a name");
            assert_eq!(m.missing, ["/home/me/Music/Gone/x.flac (Gone - Missing)", "http://example.com/stream.mp3"]);
        }
    }

    /// macOS: a name the file system hands back decomposed (NFD) matches the playlist's
    /// composed one.
    #[cfg(target_os = "macos")]
    #[test]
    fn decomposed_names_match_composed_ones() {
        let library = vec![("/Users/me/Music/Bjo\u{308}rk/01.flac".to_owned(), 1)];
        let m = match_entries(&parse("/Users/me/Music/Bj\u{f6}rk/01.flac\n"), &library, Path::new("/Users/me"), &[]);
        assert_eq!(m.ids, [1]);
    }

    #[cfg(not(windows))]
    #[test]
    fn round_trips() {
        let tracks = [t("/home/me/Music/K/01.flac", "K", "One", 60.0), t("/home/me/Music/K/sub/02.flac", "K", "Two", 61.0)];
        let library: Vec<(String, i64)> = tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i as i64 + 10)).collect();
        for relative in [false, true] {
            let dest = Path::new("/home/me/Music/Lists/k.m3u8");
            let m = match_entries(&parse(&export(&tracks, dest, relative)), &library, dest.parent().unwrap(), &[]);
            assert_eq!(m.ids, [10, 11], "relative {relative}");
            assert!(m.missing.is_empty());
        }
    }
}
