//! Folding for search, so the same words match however they were typed or tagged:
//! - katakana and hiragana compare equal (folded to hiragana),
//! - half-width katakana compare equal to full-width (with their voicing marks),
//! - full-width Latin letters, digits and punctuation compare equal to normal width,
//! - case is ignored (Unicode lowercase),
//! - Latin letters lose their diacritics ("Beyoncé" is "beyonce", "Dvořák" is "dvorak",
//!   "Straße" is "strasse"), precomposed or written as a letter plus combining marks.
//!   Only Latin: the kana voicing marks stay, so が is not か.
//!
//! Romaji to kana is not attempted: it needs a transliteration table and guesses.
//! Registered as the SQLite function `fold(text)` on every connection (`db.rs`).

/// Half-width katakana U+FF66..=U+FF9D, in order, as full-width.
const HALF_KANA: &str = "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";

fn voiced(c: char) -> Option<char> {
    // Dakuten: か -> が etc. (Unicode places the voiced form one after.)
    match c {
        'カ'..='ト' | 'ハ'..='ホ' => {
            let next = char::from_u32(c as u32 + 1)?;
            // Only the kana with a voiced form directly after (not ッ etc.).
            matches!(next, 'ガ' | 'ギ' | 'グ' | 'ゲ' | 'ゴ' | 'ザ' | 'ジ' | 'ズ' | 'ゼ' | 'ゾ' | 'ダ' | 'ヂ' | 'ヅ' | 'デ' | 'ド' | 'バ' | 'ビ' | 'ブ' | 'ベ' | 'ボ')
                .then_some(next)
        }
        'ウ' => Some('ヴ'),
        _ => None,
    }
}

fn semi_voiced(c: char) -> Option<char> {
    match c {
        'ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ' => char::from_u32(c as u32 + 2),
        _ => None,
    }
}

pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last: Option<char> = None;
    for c in s.chars() {
        let mapped = match c {
            // Full-width ASCII.
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            // Half-width katakana.
            '\u{FF66}'..='\u{FF9D}' => HALF_KANA.chars().nth((c as u32 - 0xFF66) as usize).unwrap_or(c),
            // Half-width voicing marks combine with the kana before them.
            '\u{FF9E}' | '\u{FF9F}' => {
                let base = last.and_then(|l| if c == '\u{FF9E}' { voiced(l) } else { semi_voiced(l) });
                if let Some(v) = base {
                    out.pop();
                    last = Some(v);
                    push_folded(&mut out, v);
                }
                continue;
            }
            _ => c,
        };
        last = Some(mapped);
        push_folded(&mut out, mapped);
    }
    out
}

/// Push one character: katakana as hiragana, Latin without diacritics, everything
/// lowercased.
fn push_folded(out: &mut String, c: char) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    match c {
        // Katakana with a hiragana counterpart (ァ..ヶ), not ・ー etc.
        '\u{30A1}'..='\u{30F6}' => out.push(char::from_u32(c as u32 - 0x60).unwrap_or(c)),
        // Combining diacritics (a letter written as "e" + U+0301): dropped after a Latin
        // letter. (The kana voicing marks are U+3099/U+309A, not in this block.)
        '\u{0300}'..='\u{036F}' if out.chars().next_back().is_some_and(|l| l.is_ascii_alphabetic()) => {}
        _ => match latin_base(c) {
            Some(base) => out.push_str(base),
            None => out.extend(c.to_lowercase()),
        },
    }
}

/// Latin-1 Supplement letters U+00C0..=U+00FF without their diacritics ("" for × and ÷,
/// which are not letters and stay as they are).
const LATIN1: [&str; 64] = [
    "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i", //
    "d", "n", "o", "o", "o", "o", "o", "", "o", "u", "u", "u", "u", "y", "th", "ss", //
    "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i", //
    "d", "n", "o", "o", "o", "o", "o", "", "o", "u", "u", "u", "u", "y", "th", "y",
];

/// Latin Extended-A U+0100..=U+017F, as runs of one base: (how many, base). 128 in all.
const EXTENDED_A: [(u8, &str); 22] = [
    (6, "a"),
    (8, "c"),
    (4, "d"),
    (10, "e"),
    (8, "g"),
    (4, "h"),
    (10, "i"),
    (2, "ij"),
    (2, "j"),
    (3, "k"),
    (10, "l"),
    (9, "n"),
    (6, "o"),
    (2, "oe"),
    (6, "r"),
    (8, "s"),
    (6, "t"),
    (12, "u"),
    (2, "w"),
    (3, "y"),
    (6, "z"),
    (1, "s"),
];

/// The plain lowercase letters of an accented Latin letter, or `None` for anything else
/// (which then only lowercases).
fn latin_base(c: char) -> Option<&'static str> {
    let u = c as u32;
    match u {
        0xC0..=0xFF => Some(LATIN1[(u - 0xC0) as usize]).filter(|b| !b.is_empty()),
        0x100..=0x17F => {
            let mut i = u - 0x100;
            EXTENDED_A.iter().find_map(|&(n, base)| {
                if i < u32::from(n) {
                    return Some(base);
                }
                i -= u32::from(n);
                None
            })
        }
        // Latin Extended-B: the letters common in names (Vietnamese ơ ư, the Croatian
        // and Serbian digraphs, pinyin tones, Romanian ș ț).
        0x1A0 | 0x1A1 => Some("o"),
        0x1AF | 0x1B0 => Some("u"),
        0x1C4..=0x1C6 => Some("dz"),
        0x1C7..=0x1C9 => Some("lj"),
        0x1CA..=0x1CC => Some("nj"),
        0x1CD | 0x1CE => Some("a"),
        0x1CF | 0x1D0 => Some("i"),
        0x1D1 | 0x1D2 => Some("o"),
        0x1D3..=0x1DC => Some("u"),
        0x218 | 0x219 => Some("s"),
        0x21A | 0x21B => Some("t"),
        0x1E00..=0x1EFF => extended_additional(u),
        _ => None,
    }
}

/// Latin Extended Additional: Vietnamese (U+1EA0..=U+1EF9) and the dotted and
/// underdotted consonants, by runs of one base letter.
fn extended_additional(u: u32) -> Option<&'static str> {
    Some(match u {
        0x1EA0..=0x1EB7 => "a",
        0x1EB8..=0x1EC7 => "e",
        0x1EC8..=0x1ECB => "i",
        0x1ECC..=0x1EE3 => "o",
        0x1EE4..=0x1EF1 => "u",
        0x1EF2..=0x1EF9 => "y",
        0x1E0C..=0x1E13 => "d",
        0x1E24..=0x1E2B => "h",
        0x1E36..=0x1E3D => "l",
        0x1E42..=0x1E4B => "n",
        0x1E5A..=0x1E5F => "r",
        0x1E62..=0x1E69 => "s",
        0x1E6C..=0x1E71 => "t",
        0x1E92..=0x1E95 => "z",
        _ => return None,
    })
}

/// Register `fold(text)`, and `kfold(text)` (folded, then long vowels folded away for
/// romaji matching, `romaji::loose`) on a connection.
pub fn register(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("fold", 1, flags, |ctx| {
        let s: Option<String> = ctx.get(0)?;
        Ok(s.map(|s| fold(&s)))
    })?;
    conn.create_scalar_function("kfold", 1, flags, |ctx| {
        let s: Option<String> = ctx.get(0)?;
        Ok(s.map(|s| crate::romaji::loose(&fold(&s))))
    })?;
    // seeded_rand(seed, id): a shuffle that stays the same for one seed, so a random
    // shelf and its full page agree, and paging does not reshuffle (Navidrome's
    // SEEDEDRAND does the same, persistence/sql_base_repository.go).
    conn.create_scalar_function("seeded_rand", 2, flags, |ctx| {
        let seed: i64 = ctx.get(0)?;
        let id: i64 = ctx.get(1)?;
        Ok(seeded_rand(seed, id))
    })?;
    // COLLATE NAMES: names as people sort them (`natural_cmp`), case ignored: "Artist 2" before
    // "Artist 10", "abba" beside "ABBA". The Albums page and the playlist list sort with it, as
    // the Artists, Genres and Labels indexes do in Rust.
    conn.create_collation("NAMES", |a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()).then_with(|| a.cmp(b)))
}

/// `seeded_rand(seed, id)` as SQL has it: a row's place in the shuffle of one seed.
pub fn seeded_rand(seed: i64, id: i64) -> i64 {
    splitmix64((seed as u64) ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) as i64
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Natural order for names, as people sort them: runs of digits compare by their value
/// ("Genre 2" before "Genre 10"), everything else character by character. Give it
/// folded text (`fold`) to ignore case, accents and width. Equal values with different
/// zero padding ("7", "007") put the shorter first, so the order is still total.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn digits(s: &str, at: usize) -> &str {
        let end = s[at..].find(|c: char| !c.is_ascii_digit()).map_or(s.len(), |n| at + n);
        &s[at..end]
    }
    let (mut i, mut j) = (0, 0);
    // Zero padding decides only when nothing else does.
    let mut padding = Ordering::Equal;
    loop {
        let (c, d) = (a[i..].chars().next(), b[j..].chars().next());
        match (c, d) {
            (None, None) => return padding,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (x, y) = (digits(a, i), digits(b, j));
                let (xv, yv) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let o = xv.len().cmp(&yv.len()).then_with(|| xv.cmp(yv));
                if o != Ordering::Equal {
                    return o;
                }
                padding = padding.then(x.len().cmp(&y.len()));
                i += x.len();
                j += y.len();
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                i += c.len_utf8();
                j += d.len_utf8();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fold, natural_cmp};

    #[test]
    fn natural_order() {
        let mut v = vec!["Genre 10", "genre 2", "Genre 1", "Genre 2b", "Genre 02", "Genre", "Genre 100", "Genre 9", "Ärger 3", "argus"];
        v.sort_by(|a, b| natural_cmp(&fold(a), &fold(b)).then_with(|| a.cmp(b)));
        assert_eq!(v, vec!["Ärger 3", "argus", "Genre", "Genre 1", "genre 2", "Genre 02", "Genre 2b", "Genre 9", "Genre 10", "Genre 100"]);
        assert_eq!(natural_cmp("a9", "a10"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("x 18446744073709551616", "x 2"), std::cmp::Ordering::Greater, "longer than any integer");
        assert_eq!(natural_cmp("", ""), std::cmp::Ordering::Equal);
        assert_eq!(natural_cmp("abc", "abc"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn kana_width_and_case_fold_together() {
        assert_eq!(fold("ヨルシカ"), fold("よるしか"));
        assert_eq!(fold("ﾖﾙｼｶ"), fold("ヨルシカ"));
        assert_eq!(fold("ｶﾞﾝﾀﾞﾑ"), fold("ガンダム"));
        assert_eq!(fold("ﾊﾟﾗﾊﾟﾗ"), fold("パラパラ"));
        assert_eq!(fold("ＢＲＡＩＮＳＴＯＲＭ"), "brainstorm");
        assert_eq!(fold("Ｓｐａｒｋｌｅ　Ｓｈｏｗｅｒ"), "sparkle shower");
        assert_eq!(fold("ÉTÉ"), "ete");
        // Kanji and the long vowel mark are left as they are.
        assert_eq!(fold("盗作"), "盗作");
        assert_eq!(fold("ラーメン"), "らーめん");
    }

    #[test]
    fn latin_diacritics_fold_away() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("Sigur Rós"), "sigur ros");
        assert_eq!(fold("Mötley Crüe"), "motley crue");
        assert_eq!(fold("Antonín Dvořák"), "antonin dvorak");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Łona"), "lona");
        assert_eq!(fold("Røyksopp"), "royksopp");
        assert_eq!(fold("Ólafur Arnalds"), "olafur arnalds");
        assert_eq!(fold("Œuvre"), "oeuvre");
        assert_eq!(fold("Ștefan"), "stefan");
        assert_eq!(fold("Mỹ Tâm"), "my tam");
        // Written as letter + combining mark (decomposed, as some taggers save it).
        assert_eq!(fold("Beyonce\u{301}"), "beyonce");
        assert_eq!(fold("Cafe\u{0301} Noir"), "cafe noir");
        // Not letters: kept.
        assert_eq!(fold("2×3÷1"), "2×3÷1");
    }

    #[test]
    fn kana_voicing_is_not_a_diacritic() {
        assert_ne!(fold("が"), fold("か"));
        assert_ne!(fold("パ"), fold("ハ"));
        assert_eq!(fold("ガ"), "が");
        // A decomposed kana (か + U+3099) keeps its mark: it is not after a Latin letter.
        assert_eq!(fold("か\u{3099}"), "か\u{3099}");
        // A combining accent after kana is left alone too.
        assert_eq!(fold("か\u{301}"), "か\u{301}");
        assert_eq!(fold("ÇA VA ヨルシカ"), "ca va よるしか");
    }
}
