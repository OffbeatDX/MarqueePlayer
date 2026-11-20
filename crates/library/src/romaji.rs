//! Romaji in the search box finds titles written in kana: `yorushika` finds ヨルシカ.
//!
//! `to_kana` turns a query that reads as romaji into hiragana (Hepburn and Kunrei
//! spellings alike: shi/si, tsu/tu, chi/ti, fu/hu, ji/zi, sha/sya, ja/zya/jya...; a
//! doubled consonant becomes っ; n, nn, n' and m before b/m/p become ん; macrons are
//! long vowels). `loose` folds kana so long vowels compare equal however they are
//! written: ー is dropped and a lengthening vowel (おう, おお, うう, ああ, いい, ええ)
//! collapses, on both the query and the titles. Kanji are out of reach (no reading).
//! The search runs the kana form only for queries that convert cleanly and are long
//! enough not to match everything; ordinary Latin results come first (query.rs).

/// Romaji syllables, longest first within each start letter is not needed: the
/// converter tries 3, then 2, then 1 letters.
const TABLE: &[(&str, &str)] = &[
    ("a", "あ"), ("i", "い"), ("u", "う"), ("e", "え"), ("o", "お"),
    ("ka", "か"), ("ki", "き"), ("ku", "く"), ("ke", "け"), ("ko", "こ"),
    ("kya", "きゃ"), ("kyu", "きゅ"), ("kyo", "きょ"),
    ("ga", "が"), ("gi", "ぎ"), ("gu", "ぐ"), ("ge", "げ"), ("go", "ご"),
    ("gya", "ぎゃ"), ("gyu", "ぎゅ"), ("gyo", "ぎょ"),
    ("sa", "さ"), ("shi", "し"), ("si", "し"), ("su", "す"), ("se", "せ"), ("so", "そ"),
    ("sha", "しゃ"), ("sya", "しゃ"), ("shu", "しゅ"), ("syu", "しゅ"), ("sho", "しょ"), ("syo", "しょ"), ("she", "しぇ"),
    ("za", "ざ"), ("ji", "じ"), ("zi", "じ"), ("zu", "ず"), ("ze", "ぜ"), ("zo", "ぞ"),
    ("ja", "じゃ"), ("zya", "じゃ"), ("jya", "じゃ"), ("ju", "じゅ"), ("zyu", "じゅ"), ("jyu", "じゅ"),
    ("jo", "じょ"), ("zyo", "じょ"), ("jyo", "じょ"), ("je", "じぇ"),
    ("ta", "た"), ("chi", "ち"), ("ti", "ち"), ("tsu", "つ"), ("tu", "つ"), ("te", "て"), ("to", "と"),
    ("cha", "ちゃ"), ("tya", "ちゃ"), ("cya", "ちゃ"), ("chu", "ちゅ"), ("tyu", "ちゅ"), ("cyu", "ちゅ"),
    ("cho", "ちょ"), ("tyo", "ちょ"), ("cyo", "ちょ"), ("che", "ちぇ"),
    ("da", "だ"), ("di", "ぢ"), ("du", "づ"), ("dzu", "づ"), ("de", "で"), ("do", "ど"),
    ("na", "な"), ("ni", "に"), ("nu", "ぬ"), ("ne", "ね"), ("no", "の"),
    ("nya", "にゃ"), ("nyu", "にゅ"), ("nyo", "にょ"),
    ("ha", "は"), ("hi", "ひ"), ("fu", "ふ"), ("hu", "ふ"), ("he", "へ"), ("ho", "ほ"),
    ("hya", "ひゃ"), ("hyu", "ひゅ"), ("hyo", "ひょ"),
    ("fa", "ふぁ"), ("fi", "ふぃ"), ("fe", "ふぇ"), ("fo", "ふぉ"),
    ("ba", "ば"), ("bi", "び"), ("bu", "ぶ"), ("be", "べ"), ("bo", "ぼ"),
    ("bya", "びゃ"), ("byu", "びゅ"), ("byo", "びょ"),
    ("pa", "ぱ"), ("pi", "ぴ"), ("pu", "ぷ"), ("pe", "ぺ"), ("po", "ぽ"),
    ("pya", "ぴゃ"), ("pyu", "ぴゅ"), ("pyo", "ぴょ"),
    ("ma", "ま"), ("mi", "み"), ("mu", "む"), ("me", "め"), ("mo", "も"),
    ("mya", "みゃ"), ("myu", "みゅ"), ("myo", "みょ"),
    ("ya", "や"), ("yu", "ゆ"), ("yo", "よ"),
    ("ra", "ら"), ("ri", "り"), ("ru", "る"), ("re", "れ"), ("ro", "ろ"),
    ("rya", "りゃ"), ("ryu", "りゅ"), ("ryo", "りょ"),
    ("wa", "わ"), ("wi", "うぃ"), ("we", "うぇ"), ("wo", "を"),
    ("va", "ゔぁ"), ("vi", "ゔぃ"), ("vu", "ゔ"), ("ve", "ゔぇ"), ("vo", "ゔぉ"),
];

fn lookup(s: &str) -> Option<&'static str> {
    TABLE.iter().find(|(r, _)| *r == s).map(|(_, k)| *k)
}

fn is_vowel(c: u8) -> bool {
    matches!(c, b'a' | b'i' | b'u' | b'e' | b'o')
}

/// Hiragana for a query that reads as romaji; `None` when it does not (a letter with no
/// place in romaji such as l, q, x, a digit, or a dangling consonant).
pub fn to_kana(query: &str) -> Option<String> {
    // Lowercase, macrons to doubled vowels, drop the separators romanised titles use.
    let mut s = String::new();
    for c in query.chars().flat_map(char::to_lowercase) {
        match c {
            'ā' => s.push_str("aa"),
            'ī' => s.push_str("ii"),
            'ū' => s.push_str("uu"),
            'ē' => s.push_str("ee"),
            'ō' | 'ô' => s.push_str("ou"),
            ' ' | '-' | '.' | '_' => {}
            '\'' => s.push('\''),
            c if c.is_ascii_lowercase() => s.push(c),
            _ => return None,
        }
    }
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // n' and nn: ん.
        if c == b'\'' {
            i += 1;
            continue;
        }
        if c == b'n' && (i + 1 == b.len() || b[i + 1] == b'\'' || (b[i + 1] == b'n' && b.get(i + 2).is_none_or(|&x| !is_vowel(x) && x != b'y'))) {
            out.push('ん');
            i += if i + 1 < b.len() && b[i + 1] == b'n' { 2 } else { 1 };
            continue;
        }
        // n before a consonant other than y; m before b, m, p (Hepburn): ん.
        if (c == b'n' && i + 1 < b.len() && !is_vowel(b[i + 1]) && b[i + 1] != b'y')
            || (c == b'm' && i + 1 < b.len() && matches!(b[i + 1], b'b' | b'm' | b'p'))
        {
            out.push('ん');
            i += 1;
            continue;
        }
        // A doubled consonant (kk, tt, ss; tch): small っ.
        if !is_vowel(c) && c != b'n' && i + 1 < b.len() && (b[i + 1] == c || (c == b't' && b[i + 1] == b'c')) {
            out.push('っ');
            i += 1;
            continue;
        }
        let mut matched = false;
        for len in [3, 2, 1] {
            if i + len <= b.len()
                && let Some(k) = lookup(&s[i..i + len])
            {
                out.push_str(k);
                i += len;
                matched = true;
                break;
            }
        }
        if !matched {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The vowel a hiragana syllable ends in, for collapsing long vowels.
fn vowel_of(c: char) -> Option<char> {
    const ROWS: [(&str, char); 5] = [
        ("あかさたなはまやらわがざだばぱぁゃゎ", 'a'),
        ("いきしちにひみりぎじぢびぴぃ", 'i'),
        ("うくすつぬふむゆるぐずづぶぷぅゅゔ", 'u'),
        ("えけせてねへめれげぜでべぺぇ", 'e'),
        ("おこそとのほもよろをごぞどぼぽぉょ", 'o'),
    ];
    ROWS.iter().find(|(set, _)| set.contains(c)).map(|(_, v)| *v)
}

/// Kana with long vowels folded away (input already folded to hiragana): ー dropped,
/// and う/お after an o-syllable, う after u, あ after a, い after i, え after e removed.
pub fn loose(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if c == 'ー' {
            continue;
        }
        let lengthens = matches!(
            (prev.and_then(vowel_of), c),
            (Some('o'), 'う' | 'お') | (Some('u'), 'う') | (Some('a'), 'あ') | (Some('i'), 'い') | (Some('e'), 'え')
        );
        if lengthens {
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::fold;

    /// The query, as kana, is found in the title (both folded the way search folds them).
    fn finds(query: &str, title: &str) -> bool {
        let q = to_kana(query).map(|k| loose(&fold(&k)));
        q.is_some_and(|q| loose(&fold(title)).contains(&q))
    }

    #[test]
    fn real_names_from_the_library() {
        assert!(finds("yorushika", "ヨルシカ"));
        assert!(finds("yorushika", "よるしか"));
        assert!(finds("haretokidoki", "ハレトキドキ"));
        assert!(finds("hare toki doki", "ハレトキドキ"), "spaces between words");
    }

    #[test]
    fn spelling_variants() {
        assert!(finds("shinsekai", "シンセカイ") && finds("sinsekai", "シンセカイ"));
        assert!(finds("tsukimi", "つきみ") && finds("tukimi", "つきみ"));
        assert!(finds("chikyuu", "ちきゅう") && finds("tikyu", "ちきゅう"));
        assert!(finds("fuyu", "ふゆ") && finds("huyu", "ふゆ"));
        assert!(finds("jikan", "じかん") && finds("zikan", "じかん"));
        // Long vowels: ou, oo, ō, a plain o, and ー in the title.
        for q in ["toukyou", "tookyoo", "tōkyō", "tokyo"] {
            assert!(finds(q, "とうきょう"), "{q}");
        }
        assert!(finds("ramen", "ラーメン") && finds("raamen", "ラーメン"));
        // Small っ from a doubled consonant; ん as n, nn, n', m before b/p.
        assert!(finds("kitto", "きっと"));
        assert!(finds("matcha", "まっちゃ"));
        assert!(finds("konnichiha", "こんにちは") && finds("konichiha", "こにちは"));
        assert!(finds("kin'en", "きんえん"));
        assert!(finds("shimbun", "しんぶん") && finds("shinbun", "しんぶん"));
    }

    #[test]
    fn non_romaji_is_left_alone() {
        assert_eq!(to_kana("kamelia"), None, "l");
        assert_eq!(to_kana("s3rl"), None, "digits");
        assert_eq!(to_kana("black"), None);
        assert_eq!(to_kana("xi"), None);
        assert_eq!(to_kana("nihon"), Some("にほん".into()));
    }
}
