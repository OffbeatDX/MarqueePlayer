//! The leaked-path check: this machine's home folder, the checkout's path and the user name,
//! searched for in the binary that ships, in UTF-8 and UTF-16, ASCII case ignored, so a path
//! the remapping (`remap.rs`) missed stops the release instead of shipping.

use std::path::Path;

pub struct Needle {
    pub text: String,
    /// Found only as a whole word (the user name: not inside a longer name).
    word: bool,
}

/// What to look for. Windows: the home folder and the checkout, with either slash and with
/// doubled backslashes (as in a debug string). Linux: the home folder (as a folder, `/home/ann/`),
/// the checkout, and under WSL the Windows side's user folders (`/mnt/c/Users/`). Both: the
/// user name as a word, when it is not `root` and is long enough (4 or more characters) not to
/// match by chance.
pub fn needles(windows: bool, home: &Path, checkout: &Path, user: Option<&str>) -> Vec<Needle> {
    let mut out: Vec<Needle> = Vec::new();
    let mut add = |text: String, word: bool| {
        if text.len() > 1 && !out.iter().any(|n| n.text.eq_ignore_ascii_case(&text)) {
            out.push(Needle { text, word });
        }
    };
    for p in [home, checkout] {
        let s = p.to_string_lossy();
        let s = s.trim_end_matches(['\\', '/']);
        if s.is_empty() {
            continue;
        }
        if windows {
            add(s.to_owned(), false);
            add(s.replace('\\', "/"), false);
            add(s.replace('\\', "\\\\"), false);
        } else if p == home {
            add(format!("{s}/"), false);
        } else {
            add(s.to_owned(), false);
        }
    }
    if !windows {
        add("/mnt/c/Users/".to_owned(), false);
    }
    if let Some(u) = user.map(str::trim).filter(|u| u.len() >= 4 && !u.eq_ignore_ascii_case("root")) {
        add(u.to_owned(), true);
    }
    out
}

pub struct Find {
    pub needle: String,
    pub encoding: &'static str,
    pub count: usize,
    pub first: usize,
    /// Up to 160 bytes from 40 before the first find, unprintable bytes as dots.
    pub context: String,
}

pub fn scan(bytes: &[u8], needles: &[Needle]) -> Vec<Find> {
    let hay: Vec<u8> = bytes.to_ascii_lowercase();
    let mut finds = Vec::new();
    for n in needles {
        let narrow: Vec<u8> = n.text.to_ascii_lowercase().into_bytes();
        let wide: Vec<u8> = n.text.to_ascii_lowercase().encode_utf16().flat_map(u16::to_le_bytes).collect();
        for (encoding, pattern, unit) in [("UTF-8", narrow, 1), ("UTF-16", wide, 2)] {
            let at = positions(&hay, &pattern, n.word, unit);
            if let Some(&first) = at.first() {
                let start = first.saturating_sub(40);
                let end = (start + 160).min(bytes.len());
                let context = bytes[start..end].iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
                finds.push(Find { needle: n.text.clone(), encoding, count: at.len(), first, context });
            }
        }
    }
    finds
}

/// Where `pattern` starts in `hay` (both lowercased already). For a word, the characters
/// around it (`unit` bytes each) must not be letters, digits or `_`.
fn positions(hay: &[u8], pattern: &[u8], word: bool, unit: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(&first) = pattern.first() else { return out };
    if hay.len() < pattern.len() {
        return out;
    }
    let is_word = |i: usize| hay.get(i).is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_') && (unit == 1 || hay.get(i + 1) == Some(&0));
    let mut i = 0;
    while i + pattern.len() <= hay.len() {
        match hay[i..=hay.len() - pattern.len()].iter().position(|&b| b == first) {
            None => break,
            Some(k) => i += k,
        }
        if &hay[i..i + pattern.len()] == pattern && (!word || (!(i >= unit && is_word(i - unit)) && !is_word(i + pattern.len()))) {
            out.push(i);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn windows_needles_cover_both_slashes_and_the_user() {
        let n = needles(true, Path::new(r"C:\Users\Ann\"), Path::new(r"D:\src\mp"), Some("Ann"));
        let texts: Vec<&str> = n.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(texts, [r"C:\Users\Ann", "C:/Users/Ann", r"C:\\Users\\Ann", r"D:\src\mp", "D:/src/mp", r"D:\\src\\mp"]);
        // A user name long enough to look for, as a word.
        let n = needles(true, Path::new(r"C:\Users\Robin"), Path::new(r"C:\Users\Robin\mp"), Some("Robin"));
        assert!(n.iter().any(|n| n.text == "Robin" && n.word));
    }

    #[test]
    fn linux_needles_and_root() {
        let n = needles(false, Path::new("/root"), Path::new("/mnt/c/Users/Ann/mp"), Some("root"));
        let texts: Vec<&str> = n.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(texts, ["/root/", "/mnt/c/Users/Ann/mp", "/mnt/c/Users/"]);
    }

    #[test]
    fn finds_paths_in_either_encoding_case_ignored() {
        let mut exe = b"\x00\x01panicked at c:\\users\\ann\\.cargo\\registry\\x.rs\x00".to_vec();
        exe.extend(wide("D:/SRC/MP/target"));
        let n = needles(true, Path::new(r"C:\Users\Ann"), Path::new(r"D:\src\mp"), None);
        let finds = scan(&exe, &n);
        let got: Vec<(&str, &str, usize)> = finds.iter().map(|f| (f.needle.as_str(), f.encoding, f.count)).collect();
        assert_eq!(got, [(r"C:\Users\Ann", "UTF-8", 1), ("D:/src/mp", "UTF-16", 1)]);
        assert_eq!(finds[0].first, 14);
        assert!(finds[0].context.starts_with("..panicked at c:\\users"), "{}", finds[0].context);
    }

    #[test]
    fn the_user_name_only_as_a_word() {
        let n = needles(false, Path::new("/home/runner"), Path::new("/src"), Some("runner"));
        let user: Vec<Needle> = n.into_iter().filter(|n| n.word).collect();
        assert_eq!(user.len(), 1);
        let only = |b: &[u8]| scan(b, &user).len();
        assert_eq!(only(b"a TaskRunner and runners"), 0);
        assert_eq!(only(b"user=runner;"), 1);
        assert_eq!(only(b"Runner"), 1);
        assert_eq!(only(&wide("xrunner")), 0);
        assert_eq!(only(&wide("(runner)")), 1);
    }

    #[test]
    fn a_clean_binary_has_no_finds() {
        let n = needles(true, Path::new(r"C:\Users\Ann"), Path::new(r"C:\Users\Ann\mp"), Some("Ann"));
        assert!(scan(b"home\\.cargo\\registry\\src\\x.rs src\\main.rs", &n).is_empty());
        assert!(scan(b"", &n).is_empty());
    }
}
