//! Moving library folders to another root without losing anything: the portable edition on
//! a USB stick that is E: on one computer and F: on the next (Linux: mounted under another
//! user's `/media/<user>/`). The files are the same files, only the start of every path
//! changed, so the database's paths are rewritten in place (one transaction) instead of
//! letting a scan see every track gone and every file new: plays, favourites,
//! playlists and the queue belong to rows whose ids never change, and nothing is re-read.
//!
//! What holds a path: `tracks.path`, `albums.folder`, `track_tags.group_folder` (lower
//! case, `scan::group_folder`) and the folder part of an untagged album's key (`t:...|<folder>`,
//! lower case). Art is cached by content and lyrics by track, so neither needs to change.
//!
//! The app decides which folders moved (`plan`, with the file system's answers); `apply`
//! only rewrites.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, TransactionBehavior, params};

/// Paths compare without regard to letter case on Windows, and either slash is a
/// separator there.
const FOLD_CASE: bool = cfg!(windows);

fn norm(s: &str) -> String {
    if FOLD_CASE { s.replace('/', "\\").to_lowercase() } else { s.to_owned() }
}

fn is_sep(c: char) -> bool {
    c == '/' || (FOLD_CASE && c == '\\')
}

/// Remove `old` from the start of `s`, matching whole path components (ignoring letter case
/// and slash kind on Windows) and returning the remainder of `s` untouched, byte for byte.
fn strip_root<'a>(s: &'a str, old: &str) -> Option<&'a str> {
    if old.is_empty() {
        return None;
    }
    let old = old.trim_end_matches(is_sep);
    if old.is_empty() {
        return (s.is_empty() || s.starts_with(is_sep)).then_some(s);
    }
    let mut pos = 0;
    let mut parts = old.split(is_sep).peekable();
    while let Some(want) = parts.next() {
        let rem = &s[pos..];
        let len = rem.find(is_sep).unwrap_or(rem.len());
        let got = &rem[..len];
        let same = if FOLD_CASE { got.to_lowercase() == want.to_lowercase() } else { got == want };
        if !same {
            return None;
        }
        pos += len;
        if parts.peek().is_some() {
            let sep = s[pos..].chars().next().filter(|&c| is_sep(c))?;
            pos += sep.len_utf8();
        }
    }
    let rest = &s[pos..];
    (rest.is_empty() || rest.starts_with(is_sep)).then_some(rest)
}

/// `s` with `old` at its start replaced by `new`, when `old` is the whole of `s` or a
/// folder above it (not just a string prefix: `E:\Music` is not above `E:\Music2`).
/// `lower`: `s` is one of the lower-case columns (on every system), so `old` is matched in
/// lower case and the new start is written in lower case.
pub fn replace_root(s: &str, old: &str, new: &str, lower: bool) -> Option<String> {
    let old = if lower { old.to_lowercase() } else { old.to_owned() };
    let rest = strip_root(s, &old)?;
    let new = new.trim_end_matches(is_sep);
    let new = if lower { new.to_lowercase() } else { new.to_owned() };
    Some(format!("{new}{rest}"))
}

/// Rewrite the folder part of a tag-keyed album (`t:<artist>|<album>|<folder>`).
/// Any of the three parts may contain `|` (on Unix a folder name can too), so when the
/// album's `group_folder` is known it gives the exact tail; otherwise find the `|` after
/// the first two fields whose tail starts with a moved root.
fn reroot_album_key(key: &str, group_folder: Option<&str>, first: &impl Fn(&str, bool) -> Option<String>) -> Option<String> {
    let body = key.strip_prefix("t:")?;
    if let Some(gf) = group_folder
        && let Some(head) = key.strip_suffix(gf)
        && head.ends_with('|')
    {
        return Some(format!("{head}{}", first(gf, true)?));
    }
    let first_bar = body.find('|')?;
    for (off, _) in body[first_bar + 1..].match_indices('|') {
        let split = 2 + first_bar + 1 + off;
        if let Some(moved) = first(&key[split + 1..], true) {
            return Some(format!("{}|{moved}", &key[..split]));
        }
    }
    None
}

/// Which of the library `folders` moved from under `old_root` to the same place under
/// `new_root`: those that are gone where they were and are there under the new root (and
/// are not already library folders themselves). A folder still where it was, or not under
/// the new root either, is left alone: it was on another drive, or it is simply not
/// connected now.
pub fn plan(folders: &[PathBuf], old_root: &Path, new_root: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<(PathBuf, PathBuf)> {
    let (old, new) = (old_root.to_string_lossy(), new_root.to_string_lossy());
    if norm(&old).trim_end_matches(is_sep) == norm(&new).trim_end_matches(is_sep) {
        return Vec::new();
    }
    let listed: Vec<String> = folders.iter().map(|f| norm(&f.to_string_lossy()).trim_end_matches(is_sep).to_owned()).collect();
    folders
        .iter()
        .filter_map(|f| {
            let to = PathBuf::from(replace_root(&f.to_string_lossy(), &old, &new, false)?);
            let to_norm = norm(&to.to_string_lossy()).trim_end_matches(is_sep).to_owned();
            (!exists(f) && exists(&to) && !listed.contains(&to_norm)).then(|| (f.clone(), to))
        })
        .collect()
}

/// How many rows `apply` changed, by what they are.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rerooted {
    pub tracks: usize,
    pub albums: usize,
}

/// Rewrite every path under each `(old, new)` folder, in one transaction: all or nothing.
pub fn apply(conn: &mut Connection, moves: &[(PathBuf, PathBuf)]) -> rusqlite::Result<Rerooted> {
    let moves: Vec<(String, String)> = moves.iter().map(|(a, b)| (a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned())).collect();
    let first = |s: &str, lower: bool| moves.iter().find_map(|(old, new)| replace_root(s, old, new, lower));
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut out = Rerooted::default();
    {
        let rows: Vec<(i64, String)> = tx.prepare("SELECT id, path FROM tracks")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut set = tx.prepare("UPDATE tracks SET path = ?2 WHERE id = ?1")?;
        for (id, path) in rows {
            // A server's own track has no path under any folder (`remote`).
            if crate::remote::is_remote(&path) {
                continue;
            }
            if let Some(p) = first(&path, false) {
                set.execute(params![id, p])?;
                out.tracks += 1;
            }
        }
        // Read each album's pre-move `group_folder` before rewriting `track_tags`, so `t:`
        // album keys can strip the exact folder tail even if any part contains `|`.
        let group_folders: HashMap<i64, String> = tx
            .prepare("SELECT DISTINCT t.album_id, g.group_folder FROM tracks t JOIN track_tags g ON g.track_id = t.id WHERE g.group_folder IS NOT NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let rows: Vec<(i64, String)> =
            tx.prepare("SELECT track_id, group_folder FROM track_tags WHERE group_folder IS NOT NULL")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut set = tx.prepare("UPDATE track_tags SET group_folder = ?2 WHERE track_id = ?1")?;
        for (id, folder) in rows {
            if let Some(f) = first(&folder, true) {
                set.execute(params![id, f])?;
            }
        }
        let rows: Vec<(i64, String, String)> =
            tx.prepare("SELECT id, folder, key FROM albums")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut set = tx.prepare("UPDATE albums SET folder = ?2, key = ?3 WHERE id = ?1")?;
        for (id, folder, key) in rows {
            let new_folder = first(&folder, false);
            let new_key = reroot_album_key(&key, group_folders.get(&id).map(String::as_str), &first);
            if new_folder.is_some() || new_key.is_some() {
                set.execute(params![id, new_folder.unwrap_or(folder), new_key.unwrap_or(key)])?;
                out.albums += 1;
            }
        }
    }
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const E: &str = r"E:\";
    #[cfg(windows)]
    const F: &str = r"F:\";
    #[cfg(not(windows))]
    const E: &str = "/media/alice/STICK";
    #[cfg(not(windows))]
    const F: &str = "/run/media/bob/STICK";

    fn p(root: &str, rest: &str) -> String {
        let sep = std::path::MAIN_SEPARATOR;
        format!("{}{sep}{}", root.trim_end_matches(sep), rest.replace('/', &sep.to_string()))
    }

    #[test]
    fn only_whole_folders_are_replaced() {
        assert_eq!(replace_root(&p(E, "Music/a.flac"), &p(E, "Music"), &p(F, "Music"), false), Some(p(F, "Music/a.flac")));
        assert_eq!(replace_root(&p(E, "Music"), &p(E, "Music"), &p(F, "Music"), false), Some(p(F, "Music")));
        assert_eq!(replace_root(&p(E, "Music2/a.flac"), &p(E, "Music"), &p(F, "Music"), false), None, "a string prefix is not a folder");
        assert_eq!(replace_root(&p(E, "Other/a.flac"), &p(E, "Music"), &p(F, "Music"), false), None);
        // A drive root as the folder.
        assert_eq!(replace_root(&p(E, "a.flac"), E, F, false), Some(p(F, "a.flac")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_case_and_slashes() {
        assert_eq!(replace_root(r"e:\music\A.flac", r"E:\Music", r"F:\Music", false).as_deref(), Some(r"F:\Music\A.flac"));
        assert_eq!(replace_root("E:/Music/a.flac", r"E:\Music\", r"F:\Music", false).as_deref(), Some(r"F:\Music/a.flac"));
        assert_eq!(replace_root(r"e:\music\x", r"E:\Music", r"F:\Music", true).as_deref(), Some(r"f:\music\x"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_keeps_suffix_byte_for_byte_with_expanding_case_mappings() {
        // U+0130 ('İ') lowercases to 'i' + U+0307, changing UTF-8 byte length.
        assert_eq!(
            replace_root("E:\\Music\\\u{0130}stanbul\\01.flac", r"E:\Music", r"F:\Music", false).as_deref(),
            Some("F:\\Music\\\u{0130}stanbul\\01.flac")
        );
        assert_eq!(
            replace_root("e:\\music\\\u{0130}stanbul\\01.flac", r"E:\Music", r"F:\Music", false).as_deref(),
            Some("F:\\Music\\\u{0130}stanbul\\01.flac")
        );
        // Expanding character in the old root itself.
        assert_eq!(
            replace_root("E:\\\u{0130}stanbul\\Album\\01.flac", "e:\\i\u{0307}stanbul", r"F:\Music", false).as_deref(),
            Some(r"F:\Music\Album\01.flac")
        );
        assert_eq!(
            replace_root("e:\\i\u{0307}stanbul\\album", "E:\\\u{0130}stanbul", r"F:\Music", true).as_deref(),
            Some(r"f:\music\album")
        );
        assert_eq!(replace_root("E:\\\u{0130}\\01.flac", r"E:\i", r"F:\i", false), None);
    }

    #[test]
    fn plan_takes_only_folders_gone_from_the_old_root_and_found_under_the_new() {
        let music = PathBuf::from(p(E, "Music"));
        let elsewhere = PathBuf::from(if cfg!(windows) { r"D:\Albums".to_owned() } else { "/home/alice/Albums".to_owned() });
        let still = PathBuf::from(p(E, "Still here"));
        let unplugged = PathBuf::from(p(E, "Not on this stick"));
        let folders = vec![music.clone(), elsewhere.clone(), still.clone(), unplugged.clone()];
        let present = [PathBuf::from(p(F, "Music")), still.clone(), PathBuf::from(p(F, "Still here"))];
        let exists = |q: &Path| present.iter().any(|x| x == q);
        let got = plan(&folders, Path::new(E), Path::new(F), &exists);
        assert_eq!(got, vec![(music, PathBuf::from(p(F, "Music")))]);
        assert!(plan(&folders, Path::new(E), Path::new(E), &exists).is_empty(), "same root: nothing");
        // Already a library folder under the new root: not moved onto it.
        let both = vec![PathBuf::from(p(E, "Music")), PathBuf::from(p(F, "Music"))];
        assert!(plan(&both, Path::new(E), Path::new(F), &exists).is_empty());
    }

    #[test]
    fn apply_rewrites_every_path_and_keeps_every_row() {
        let mut conn = crate::db::open_in_memory().unwrap();
        let folder = p(E, "Music/Artist/Album");
        let lower = folder.to_lowercase();
        let other = p(E, "Other/X");
        conn.execute_batch(&format!(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES
               (1, 't:artist|album|{lower}', 'Album', 'Artist', '{folder}', 0),
               (2, 'mb:abc', 'Tagged', 'Artist', '{folder}', 0),
               (3, 't:x|x|{olower}', 'X', 'X', '{other}', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
               (10, '{folder}{sep}01.flac', 1, 'One', 'Artist', 1.0, 0, 0, 0),
               (11, '{other}{sep}01.flac', 3, 'X', 'X', 1.0, 0, 0, 0);
             INSERT INTO track_tags (track_id, group_folder) VALUES (10, '{lower}'), (11, '{olower}');
             INSERT INTO plays (track_id, played_at) VALUES (10, 5);",
            olower = other.to_lowercase(),
            sep = std::path::MAIN_SEPARATOR,
        ))
        .unwrap();
        let moves = vec![(PathBuf::from(p(E, "Music")), PathBuf::from(p(F, "Music")))];
        assert_eq!(apply(&mut conn, &moves).unwrap(), Rerooted { tracks: 1, albums: 2 });
        let s = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        let nfolder = p(F, "Music/Artist/Album");
        assert_eq!(s("SELECT path FROM tracks WHERE id = 10"), format!("{nfolder}{}01.flac", std::path::MAIN_SEPARATOR));
        assert_eq!(s("SELECT folder FROM albums WHERE id = 1"), nfolder);
        assert_eq!(s("SELECT key FROM albums WHERE id = 1"), format!("t:artist|album|{}", nfolder.to_lowercase()));
        assert_eq!(s("SELECT key FROM albums WHERE id = 2"), "mb:abc");
        assert_eq!(s("SELECT folder FROM albums WHERE id = 2"), nfolder);
        assert_eq!(s("SELECT group_folder FROM track_tags WHERE track_id = 10"), nfolder.to_lowercase());
        // Outside the moved folder: untouched.
        assert_eq!(s("SELECT path FROM tracks WHERE id = 11"), format!("{other}{}01.flac", std::path::MAIN_SEPARATOR));
        assert_eq!(s("SELECT folder FROM albums WHERE id = 3"), other);
        // The play still belongs to the track.
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM plays WHERE track_id = 10", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn apply_rewrites_keys_containing_vertical_bars() {
        let mut conn = crate::db::open_in_memory().unwrap();
        let old = "/media/alice/STICK/Music";
        let new = "/run/media/bob/STICK/Music";
        conn.execute_batch(&format!(
            "INSERT INTO albums (id, key, title, album_artist, folder, added_at) VALUES
               (1, 't:artist|album|/media/alice/stick/music/a | b/c | d', 'Album', 'Artist', '{old}/A | B/C | D', 0),
               (2, 't:band | one|side a | side b|/media/alice/stick/music/band | one/side a | side b', 'Side A | Side B', 'Band | One', '{old}/Band | One/Side A | Side B', 0),
               (3, 't:x | y|z | w|/media/alice/stick/music/dir | 1', 'Z | W', 'X | Y', '{old}/Dir | 1', 0);
             INSERT INTO tracks (id, path, album_id, title, artist, duration, mtime, size, added_at) VALUES
               (10, '{old}/A | B/C | D/01.flac', 1, 'One', 'Artist', 1.0, 0, 0, 0),
               (11, '{old}/Band | One/Side A | Side B/01.flac', 2, 'Two', 'Band | One', 1.0, 0, 0, 0);
             INSERT INTO track_tags (track_id, group_folder) VALUES
               (10, '/media/alice/stick/music/a | b/c | d'),
               (11, '/media/alice/stick/music/band | one/side a | side b');"
        ))
        .unwrap();
        let moves = vec![(PathBuf::from(old), PathBuf::from(new))];
        assert_eq!(apply(&mut conn, &moves).unwrap(), Rerooted { tracks: 2, albums: 3 });
        let s = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        assert_eq!(s("SELECT key FROM albums WHERE id = 1"), "t:artist|album|/run/media/bob/stick/music/a | b/c | d");
        assert_eq!(s("SELECT key FROM albums WHERE id = 2"), "t:band | one|side a | side b|/run/media/bob/stick/music/band | one/side a | side b");
        assert_eq!(s("SELECT key FROM albums WHERE id = 3"), "t:x | y|z | w|/run/media/bob/stick/music/dir | 1");
        assert_eq!(s("SELECT group_folder FROM track_tags WHERE track_id = 10"), "/run/media/bob/stick/music/a | b/c | d");
        assert_eq!(s("SELECT group_folder FROM track_tags WHERE track_id = 11"), "/run/media/bob/stick/music/band | one/side a | side b");
    }
}
