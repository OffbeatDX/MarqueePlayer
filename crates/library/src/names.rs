//! Names as tagged, made comparable and taken apart: the rules the scanner applies to
//! artist, album artist and genre values, and what it reads from a file name when a tag is
//! missing. Pure functions, unit-tested; nothing here touches files or the database.
//!
//! - **Identity** (`norm`): a name's Unicode normalisation form (NFC), surrounding and
//!   repeated whitespace and letter case do not make it another name. Accents do: "Sigur
//!   Ros" and "Sigur Rós" stay two (search finds both either way).
//! - **Splitting** (`split_artists`): a single string holding several artists is split on
//!   ";" and on " / " (a slash with spaces), never on ",", "&" or a bare "/": "Earth, Wind
//!   & Fire" and "AC/DC" are one artist each. A tag that holds several values of its own
//!   (Vorbis comments and APE repeat the key, ID3v2.4 separates values with NUL, MP4 holds
//!   several data items) needs no splitting: each value is taken, and then split the same
//!   way in case it joins several itself.
//! - **Featured artists** (`featured`, `credits`): "A feat. B", "A ft. B", "A featuring
//!   B", "A (feat. B)" credit A, and B as featured. In a title, only a bracketed credit
//!   counts ("Song (feat. B)", "Song [ft. B]"): a bare "feat." in a title is left alone.
//! - **File names** (`from_file_name`): "01 - Title", "01. Title", "01_Title", "01 Title"
//!   (with a leading zero), "1-03 Title" (disc 1, track 3) give a number and a title when
//!   the tags do not.

use std::borrow::Cow;

/// The identity of a name: NFC, trimmed, inner whitespace collapsed to one space, lower
/// case. Two names with the same `norm` are one artist, album title or genre.
pub fn norm(s: &str) -> String {
    let nfc = nfc(s);
    let mut out = String::with_capacity(nfc.len());
    for word in nfc.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// `s` in Unicode normalisation form C (precomposed), borrowed when it already is.
pub fn nfc(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return Cow::Borrowed(s);
    }
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(s)
}

/// A name as it is shown: NFC, trimmed, inner runs of whitespace as one space. Case is
/// kept as tagged.
pub fn tidy(s: &str) -> String {
    let nfc = nfc(s);
    let mut out = String::with_capacity(nfc.len());
    for word in nfc.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// The artists in a value that joins several: split on `;` and ` / `. Commas, `&` and a
/// bare `/` are left alone ("Earth, Wind & Fire", "AC/DC"). Trimmed, without empties or
/// repeats.
pub fn split_artists(s: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for part in s.split(';').flat_map(|p| p.split(" / ")) {
        let p = part.trim();
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Genres in a value that joins several: the same separators as artists (";" and " / ").
/// "Hip-Hop/Rap", "Drum & Bass" and "Folk, World, & Country" stay one genre each.
pub fn split_genres(s: &str) -> Vec<&str> {
    split_artists(s)
}

/// The names on either side of featuring markers in one artist credit: "A feat. B" and
/// "A (ft. B)" give "A", "B". Empty when there is no marker. A marker is a whole word
/// (after a space or "(" or "[", before a space), in any case.
pub fn featured(artist: &str) -> Vec<&str> {
    const MARKERS: [&[u8]; 3] = [b"feat.", b"ft.", b"featuring"];
    let b = artist.as_bytes();
    let mut parts = Vec::new();
    let (mut from, mut i) = (0, 1);
    while i < b.len() {
        let hit = matches!(b[i - 1], b' ' | b'(' | b'[')
            .then(|| MARKERS.iter().find(|m| b.get(i..i + m.len()).is_some_and(|w| w.eq_ignore_ascii_case(m)) && b.get(i + m.len()) == Some(&b' ')))
            .flatten();
        match hit {
            // ASCII bytes never fall inside a multi-byte character: `i` is a boundary.
            Some(m) => {
                parts.push(&artist[from..i]);
                i += m.len();
                from = i;
            }
            None => i += 1,
        }
    }
    if parts.is_empty() {
        return parts;
    }
    parts.push(&artist[from..]);
    parts.into_iter().map(|p| p.trim_matches(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']'))).filter(|p| !p.is_empty()).collect()
}

/// One credited artist, as the scanner keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub name: String,
    /// MusicBrainz artist id, when the file has one for this artist.
    pub mbid: Option<String>,
    /// Credited after a "feat." marker (in the artist value or the title).
    pub featured: bool,
}

impl Credit {
    pub fn new(name: &str, featured: bool) -> Self {
        Credit { name: tidy(name), mbid: None, featured }
    }
}

/// The artists one artist value credits, in order: each artist of a joined value
/// (`split_artists`), and for a "A feat. B" credit A and then B as featured.
pub fn credits(value: &str) -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for part in split_artists(value) {
        let names = featured(part);
        if names.is_empty() {
            push_credit(&mut out, Credit::new(part, false));
        } else {
            for (i, n) in names.into_iter().enumerate() {
                push_credit(&mut out, Credit::new(n, i > 0));
            }
        }
    }
    out
}

/// Add a credit unless the same artist (by `norm`) is there already.
pub fn push_credit(out: &mut Vec<Credit>, c: Credit) {
    let n = norm(&c.name);
    if !n.is_empty() && !out.iter().any(|o| norm(&o.name) == n) {
        out.push(c);
    }
}

/// Featured artists named in a title, only in brackets: "Song (feat. B)", "Song [ft. B &
/// C]" give B (and "B & C" as one name: an ampersand is never split). "Feat. of Clay" or
/// "Song feat. B" without brackets give nothing.
pub fn title_featured(title: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = title.to_ascii_lowercase();
    for (open, close) in [('(', ')'), ('[', ']')] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(open).map(|i| i + from) {
            let Some(j) = lower[i..].find(close).map(|j| j + i) else { break };
            let inner = &title[i + 1..j];
            let li = &lower[i + 1..j];
            for m in ["feat. ", "ft. ", "featuring "] {
                if let Some(rest) = li.strip_prefix(m) {
                    let name = &inner[inner.len() - rest.len()..];
                    for n in split_artists(name) {
                        out.push(tidy(n));
                    }
                    break;
                }
            }
            from = j + 1;
        }
    }
    out
}

/// Values that mean "this album is a compilation": "Various Artists" and its common short
/// forms, compared by `norm`.
pub fn is_various(name: &str) -> bool {
    matches!(norm(name).as_str(), "various artists" | "various" | "va" | "v.a." | "v/a" | "various artist")
}

/// The name a compilation is listed under.
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// What a file name says when the tags do not: (disc, track, title).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FromName {
    pub disc: Option<u32>,
    pub track: Option<u32>,
    pub title: String,
}

/// Read a number and a title from a file's stem: "01 - Title", "01. Title", "01) Title",
/// "01_Title", "1-03 Title" (disc and track), "01 Title" (with only a space after it, the
/// number must have a leading zero: "4 Minutes" and "99 Balloons" are titles). Anything else
/// is all title.
pub fn from_file_name(stem: &str) -> FromName {
    let s = stem.trim();
    let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits(s);
    let title_of = |rest: &str| {
        let t = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '.' | '_' | ')')).trim();
        if t.is_empty() { None } else { Some(t.to_owned()) }
    };
    if (1..=3).contains(&n) {
        let num: u32 = s[..n].parse().unwrap_or(0);
        let rest = &s[n..];
        // "1-03 Title": disc, hyphen, track, no spaces around the hyphen.
        if n <= 2
            && let Some(r) = rest.strip_prefix('-')
        {
            let m = digits(r);
            if (1..=3).contains(&m)
                && r[m..].starts_with([' ', '_', '.', '-'])
                && let Some(t) = title_of(&r[m..])
            {
                return FromName { disc: Some(num), track: r[..m].parse().ok(), title: t };
            }
        }
        // The first character after the number and any spaces.
        let spaced = rest.trim_start();
        let sep = spaced.chars().next();
        let after = spaced.chars().nth(1);
        let ok = match sep {
            // "01 - Title", "01_Title", "01) Title"
            Some('-' | '_' | ')') => true,
            // "01. Title", not "1.5 Hours"
            Some('.') => !after.is_some_and(|c| c.is_ascii_digit()),
            // "01 Title": only with a leading zero.
            Some(_) if spaced.len() < rest.len() => n >= 2 && s.starts_with('0'),
            _ => false,
        };
        if ok
            && num > 0
            && let Some(t) = title_of(rest)
        {
            return FromName { disc: None, track: Some(num), title: t };
        }
    }
    FromName { disc: None, track: None, title: s.to_owned() }
}

/// A track or disc number tag: "3", "03", "3/12" give 3. "A1" (a vinyl side and position)
/// and anything else unreadable give `None`.
pub fn leading_number(s: &str) -> Option<u32> {
    s.split('/').next()?.trim().parse().ok().filter(|&n| n > 0)
}

/// The most common value, ties going to the value that comes first in `values` (which the
/// caller gives in a fixed order: the album's tracks by disc, number and path). Values are
/// compared by `key`.
pub fn most_common<'a, T, K: PartialEq>(values: impl IntoIterator<Item = &'a T>, key: impl Fn(&T) -> K) -> Option<&'a T>
where
    T: 'a,
{
    let mut counts: Vec<(&'a T, K, usize)> = Vec::new();
    for v in values {
        let k = key(v);
        match counts.iter_mut().find(|(_, ck, _)| *ck == k) {
            Some(c) => c.2 += 1,
            None => counts.push((v, k, 1)),
        }
    }
    let best = counts.iter().map(|c| c.2).max()?;
    counts.into_iter().find(|c| c.2 == best).map(|c| c.0)
}

/// The artist string to send to an online service (Last.fm, LRCLIB): the credit as tagged
/// ("A feat. B" is how both list such tracks), unless it lists several artists with the
/// multi-value separators (";", " / "): a joined list is a name neither service knows, so
/// the first artist.
pub fn service_artist(display: &str) -> String {
    let parts = split_artists(display);
    match parts.as_slice() {
        [first, _, ..] => (*first).to_owned(),
        _ => display.trim().to_owned(),
    }
}

/// Where each credited artist's name stands in a credit as shown, so it can be a link and
/// the rest plain text ("A feat. B": A, " feat. ", B). Names are found in any order, case
/// ignored; overlapping finds keep the longer name. Returns the parts in order, each with
/// the index of its artist in `names` (`None`: plain text), or `None` when some name is not
/// in the credit at all (a group's name with its members credited: "The Carters" with
/// Beyoncé and JAY-Z), for the caller to list the names instead.
pub fn link_parts(shown: &str, names: &[&str]) -> Option<Vec<(String, Option<usize>)>> {
    // Longest names first, each at its first place not taken by a longer one ("Mara" after
    // "Mara Lind").
    let mut order: Vec<usize> = (0..names.len()).filter(|&i| !names[i].trim().is_empty()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(names[i].trim().len()));
    let mut kept: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, artist)
    for i in order {
        let n = names[i].trim();
        let mut from = 0;
        loop {
            let (s, e) = find_ci(&shown[from..], n).map(|(s, e)| (s + from, e + from))?;
            if !kept.iter().any(|k| s < k.1 && k.0 < e) {
                kept.push((s, e, i));
                break;
            }
            from = s + shown[s..].chars().next().map_or(1, char::len_utf8);
        }
    }
    kept.sort();
    let mut out = Vec::new();
    let mut at = 0;
    for (s, e, i) in kept {
        if s > at {
            out.push((shown[at..s].to_owned(), None));
        }
        out.push((shown[s..e].to_owned(), Some(i)));
        at = e;
    }
    if at < shown.len() {
        out.push((shown[at..].to_owned(), None));
    }
    Some(out)
}

/// The first place `needle` occurs in `hay`, letter case ignored: byte range in `hay`.
fn find_ci(hay: &str, needle: &str) -> Option<(usize, usize)> {
    if let Some(i) = hay.find(needle) {
        return Some((i, i + needle.len()));
    }
    let n: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    for (start, _) in hay.char_indices() {
        // Lower the text from here as far as the needle reaches, and compare.
        let mut got: Vec<char> = Vec::with_capacity(n.len());
        let mut end = start;
        for (off, c) in hay[start..].char_indices() {
            if got.len() >= n.len() {
                break;
            }
            got.extend(c.to_lowercase());
            end = start + off + c.len_utf8();
        }
        if got == n {
            return Some((start, end));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case_space_or_form() {
        assert_eq!(norm("  The  National "), "the national");
        assert_eq!(norm("The National"), norm("the national"));
        assert_eq!(norm("Zoe\u{0308} Marchetti"), norm("Zoë Marchetti"), "decomposed and precomposed");
        assert_ne!(norm("Sigur Ros"), norm("Sigur Rós"), "accents are part of a name");
        assert_eq!(norm("ÉCOLE"), "école");
        assert_eq!(tidy("  The  National "), "The National");
        assert_eq!(tidy("Zoe\u{0308}"), "Zoë");
    }

    #[test]
    fn multi_artist_values_split_into_artists() {
        assert_eq!(split_artists("Yuta Imai;Kasane Teto"), vec!["Yuta Imai", "Kasane Teto"]);
        assert_eq!(split_artists("A; B ;A"), vec!["A", "B"]);
        assert_eq!(split_artists("Kagamine Rin / Kagamine Len"), vec!["Kagamine Rin", "Kagamine Len"]);
        assert_eq!(split_artists("Earth, Wind & Fire"), vec!["Earth, Wind & Fire"]);
        assert_eq!(split_artists("AC/DC"), vec!["AC/DC"]);
        assert_eq!(split_artists(""), Vec::<&str>::new());
        assert_eq!(split_genres("Hip-Hop/Rap"), vec!["Hip-Hop/Rap"]);
        assert_eq!(split_genres("Folk, World, & Country"), vec!["Folk, World, & Country"]);
        assert_eq!(split_genres("Folk; Pop"), vec!["Folk", "Pop"]);
    }

    #[test]
    fn featured_credits_count() {
        let names = |v: &str| credits(v).into_iter().map(|c| (c.name, c.featured)).collect::<Vec<_>>();
        assert_eq!(names("Someone feat. かめりあ"), vec![("Someone".into(), false), ("かめりあ".into(), true)]);
        assert_eq!(names("Someone FT. X"), vec![("Someone".into(), false), ("X".into(), true)]);
        assert_eq!(names("Someone (featuring X)"), vec![("Someone".into(), false), ("X".into(), true)]);
        assert_eq!(names("A feat. B feat. C"), vec![("A".into(), false), ("B".into(), true), ("C".into(), true)]);
        assert_eq!(names("Defeat. X"), vec![("Defeat. X".into(), false)], "a marker is a whole word");
        assert_eq!(names("Earth, Wind & Fire"), vec![("Earth, Wind & Fire".into(), false)]);
        assert_eq!(names("A;B feat. C; a"), vec![("A".into(), false), ("B".into(), false), ("C".into(), true)], "repeats by norm dropped");
        assert_eq!(names("  "), vec![]);
    }

    #[test]
    fn titles_credit_only_bracketed_features() {
        assert_eq!(title_featured("Daybreak (feat. Ines Okafor)"), vec!["Ines Okafor"]);
        assert_eq!(title_featured("Evening [ft. Theo Vance]"), vec!["Theo Vance"]);
        assert_eq!(title_featured("Song (Remix) (featuring A; B)"), vec!["A", "B"]);
        assert_eq!(title_featured("Dusk feat. Kenji Arai"), Vec::<String>::new(), "not bracketed");
        assert_eq!(title_featured("Feat. of Clay"), Vec::<String>::new());
        assert_eq!(title_featured("(Featuring)"), Vec::<String>::new());
    }

    #[test]
    fn file_names_give_numbers_and_titles() {
        let f = |s| from_file_name(s);
        assert_eq!(f("01 - First Light"), FromName { disc: None, track: Some(1), title: "First Light".into() });
        assert_eq!(f("02. Second Wind"), FromName { disc: None, track: Some(2), title: "Second Wind".into() });
        assert_eq!(f("1-03 Third Rail"), FromName { disc: Some(1), track: Some(3), title: "Third Rail".into() });
        assert_eq!(f("04_Fourth Wall"), FromName { disc: None, track: Some(4), title: "Fourth Wall".into() });
        assert_eq!(f("05 Fifth"), FromName { disc: None, track: Some(5), title: "Fifth".into() });
        assert_eq!(f("12) Twelve"), FromName { disc: None, track: Some(12), title: "Twelve".into() });
        assert_eq!(f("Untitled").track, None);
        assert_eq!(f("Untitled").title, "Untitled");
        assert_eq!(f("4 Minutes").track, None, "one digit and a space: a title");
        assert_eq!(f("99 Balloons").track, None, "no leading zero and only a space: a title");
        assert_eq!(f("10 - Ten").track, Some(10));
        assert_eq!(f("1.5 Hours").track, None);
        assert_eq!(f("1979").track, None);
        assert_eq!(f("01").title, "01", "nothing after the number: all title");
        assert_eq!(f("01 - ").track, None);
        assert_eq!(f("2001 - A Space Odyssey").track, None, "four digits: a title");
    }

    #[test]
    fn numbers_read_from_tags() {
        assert_eq!(leading_number("3/12"), Some(3));
        assert_eq!(leading_number("03"), Some(3));
        assert_eq!(leading_number(" 7 "), Some(7));
        assert_eq!(leading_number("A1"), None);
        assert_eq!(leading_number("0"), None);
    }

    #[test]
    fn most_common_breaks_ties_by_order() {
        let v = ["b", "a", "a", "b", "c"];
        assert_eq!(most_common(v.iter(), |s| *s), Some(&"b"), "tie: the first given");
        let v = ["x", "y", "y"];
        assert_eq!(most_common(v.iter(), |s| *s), Some(&"y"));
        let v: [&str; 0] = [];
        assert_eq!(most_common(v.iter(), |s| *s), None);
        let v = ["Rock", "rock", "ROCK", "Pop"];
        assert_eq!(most_common(v.iter(), |s| norm(s)), Some(&"Rock"));
    }

    #[test]
    fn various_artists_spellings() {
        assert!(is_various("Various Artists"));
        assert!(is_various(" various  artists "));
        assert!(is_various("VA"));
        assert!(!is_various("Various Cruelties"));
    }

    #[test]
    fn credits_shown_as_links_and_text() {
        let parts = |shown: &str, names: &[&str]| link_parts(shown, names).map(|v| v.into_iter().map(|(t, i)| format!("{}{t}", if i.is_some() { "@" } else { "" })).collect::<Vec<_>>());
        assert_eq!(parts("A feat. B", &["A", "B"]), Some(vec!["@A".into(), " feat. ".into(), "@B".into()]));
        assert_eq!(parts("Mara Lind; Kenji Arai", &["Kenji Arai", "Mara Lind"]), Some(vec!["@Mara Lind".into(), "; ".into(), "@Kenji Arai".into()]), "any order");
        assert_eq!(parts("Kanye West & Jay-Z", &["Kanye West", "JAY-Z"]), Some(vec!["@Kanye West".into(), " & ".into(), "@Jay-Z".into()]), "case ignored, shown as written");
        assert_eq!(parts("Earth, Wind & Fire", &["Earth, Wind & Fire"]), Some(vec!["@Earth, Wind & Fire".into()]));
        assert_eq!(parts("The Carters", &["Beyoncé", "JAY-Z"]), None, "a name not in the credit");
        assert_eq!(parts("Mara Lind & Mara", &["Mara", "Mara Lind"]), Some(vec!["@Mara Lind".into(), " & ".into(), "@Mara".into()]), "the longer name wins its place");
        assert_eq!(parts("ÉCOLE", &["école"]), Some(vec!["@ÉCOLE".into()]));
    }

    #[test]
    fn services_get_the_credit_or_the_first_artist() {
        assert_eq!(service_artist("A feat. B"), "A feat. B");
        assert_eq!(service_artist("Earth, Wind & Fire"), "Earth, Wind & Fire");
        assert_eq!(service_artist("Mara Lind; Theo Vance"), "Mara Lind");
        assert_eq!(service_artist("Kagamine Rin / Kagamine Len"), "Kagamine Rin");
        assert_eq!(service_artist("AC/DC"), "AC/DC");
    }
}
