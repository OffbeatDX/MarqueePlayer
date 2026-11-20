//! Which album each track belongs to, and whose album it is: pure rules over what the
//! tracks' tags say, unit-tested. The scanner (`scan.rs`) applies them to every track of
//! the folders a scan touches, so the outcome never depends on the order files are read.
//!
//! An album is identified (`albums.key`) by its MusicBrainz release id when the tracks
//! have one ("mb:<id>"), else by "t:<album artist>|<album title>|<folder>", all three by
//! `names::norm` (case, whitespace and Unicode form ignored; the folder in lower case).
//! The folder is the track's own, or the album's when the track sits in a disc subfolder
//! ("CD1", "Disc 2") or a sibling folder named for its disc ("Live CD1", "Live CD2"): see
//! `scan::group_folder`. Tracks without an album tag are grouped under an empty title.
//!
//! The album artist part, for the tracks of one folder and title (`decide`):
//! 1. Album artist tags that all agree (by `norm`): that album artist, for every track of
//!    the group, also those without the tag (an album tagged on most tracks only).
//! 2. Album artist tags that disagree: one album per album artist, as tagged; unless a
//!    track is flagged as part of a compilation, which makes the group one compilation.
//! 3. No album artist tag at all:
//!    - every track's first credited artist is the same: theirs ("A" and "A feat. B" are
//!      one album by A, not two);
//!    - else a compilation ("Various Artists") when a track carries the compilation flag,
//!      or when every track has an album tag and no two tracks share a disc and track
//!      number (a compilation numbers its tracks once; two albums of the same title in one
//!      folder repeat their numbers);
//!    - else one album per first credited artist, as before.
//!
//! A track with no artist and no album artist at all has an empty artist part (shown as
//! "Unknown Artist"). An album artist tag that says "Various Artists" (or "VA", ...) is a
//! compilation's.

use std::collections::HashMap;

use crate::names;

/// What the grouping needs of one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Member {
    /// The folder it is grouped in (`scan::group_folder`), lower case.
    pub folder: String,
    /// ALBUM as tagged.
    pub album: Option<String>,
    /// ALBUMARTIST as tagged (shown form).
    pub album_artist: Option<String>,
    /// The first credited artist that is not a featured one.
    pub main_artist: Option<String>,
    pub compilation: bool,
    pub disc: Option<u32>,
    pub track: Option<u32>,
    pub mb_album: Option<String>,
}

impl Member {
    fn title_norm(&self) -> String {
        self.album.as_deref().map(names::norm).unwrap_or_default()
    }
    fn explicit(&self) -> Option<String> {
        self.album_artist.as_deref().map(|a| if names::is_various(a) { VARIOUS.to_owned() } else { names::norm(a) }).filter(|a| !a.is_empty())
    }
    fn main(&self) -> String {
        self.main_artist.as_deref().map(names::norm).unwrap_or_default()
    }
}

/// The album artist part of a key for a compilation.
const VARIOUS: &str = "various artists";

/// Whose album a track is in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum By {
    /// The album artist tag (its `norm`).
    Explicit(String),
    /// No album artist tag: the first credited artist (its `norm`; "" for none at all).
    Main(String),
    /// A compilation.
    Various,
}

impl By {
    fn key_part(&self) -> &str {
        match self {
            By::Explicit(a) | By::Main(a) => a,
            By::Various => VARIOUS,
        }
    }
}

/// The album key of every member, in order (see the module doc).
pub fn keys(members: &[Member]) -> Vec<String> {
    let mut out = vec![String::new(); members.len()];
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, m) in members.iter().enumerate() {
        match m.mb_album.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(mb) => out[i] = format!("mb:{}", mb.to_ascii_lowercase()),
            None => groups.entry((m.folder.clone(), m.title_norm())).or_default().push(i),
        }
    }
    for ((folder, title), idx) in groups {
        let group: Vec<&Member> = idx.iter().map(|&i| &members[i]).collect();
        for (i, by) in idx.iter().zip(decide(&group)) {
            out[*i] = format!("t:{}|{title}|{folder}", by.key_part());
        }
    }
    out
}

/// Whose album each member of one group (one folder, one album title) is in.
pub fn decide(group: &[&Member]) -> Vec<By> {
    let flagged = group.iter().any(|m| m.compilation);
    let mut explicit: Vec<String> = group.iter().filter_map(|m| m.explicit()).collect();
    explicit.sort();
    explicit.dedup();
    if let [only] = explicit.as_slice() {
        let by = if only == VARIOUS { By::Various } else { By::Explicit(only.clone()) };
        return vec![by; group.len()];
    }
    if explicit.len() > 1 {
        if flagged || explicit.iter().any(|e| e == VARIOUS) {
            return vec![By::Various; group.len()];
        }
        return group
            .iter()
            .map(|m| match m.explicit() {
                Some(e) => By::Explicit(e),
                None if explicit.contains(&m.main()) => By::Explicit(m.main()),
                None => By::Main(m.main()),
            })
            .collect();
    }
    // A track with no artist at all goes with the others.
    let mut mains: Vec<String> = group.iter().map(|m| m.main()).filter(|a| !a.is_empty()).collect();
    mains.sort();
    mains.dedup();
    match mains.as_slice() {
        [] => return vec![By::Main(String::new()); group.len()],
        [only] => return vec![By::Main(only.clone()); group.len()],
        _ => {}
    }
    if flagged || (group.iter().all(|m| m.album.is_some()) && !numbers_repeat(group)) {
        return vec![By::Various; group.len()];
    }
    group.iter().map(|m| By::Main(m.main())).collect()
}

/// Whose album an album is, as one: the rules of `decide` for its tracks, where album
/// artists that disagree go to the most common one (a MusicBrainz release, whose tracks
/// are one album whatever they say).
pub fn decide_one(members: &[&Member]) -> By {
    let each = decide(members);
    if let Some(first) = each.first()
        && each.iter().all(|b| b == first)
    {
        return first.clone();
    }
    names::most_common(each.iter(), Clone::clone).cloned().unwrap_or(By::Main(String::new()))
}

/// Two tracks share a disc and track number (no disc number is disc 1).
fn numbers_repeat(group: &[&Member]) -> bool {
    let mut seen = std::collections::HashSet::new();
    group.iter().filter_map(|m| m.track.map(|t| (m.disc.unwrap_or(1), t))).any(|n| !seen.insert(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(album: &str, aa: Option<&str>, artist: &str, track: u32) -> Member {
        Member {
            folder: "m:/x".into(),
            album: (!album.is_empty()).then(|| album.to_owned()),
            album_artist: aa.map(str::to_owned),
            main_artist: (!artist.is_empty()).then(|| artist.to_owned()),
            track: Some(track),
            ..Default::default()
        }
    }

    fn distinct(keys: &[String]) -> usize {
        let mut k = keys.to_vec();
        k.sort();
        k.dedup();
        k.len()
    }

    #[test]
    fn a_normal_album_keeps_its_key() {
        let ms = vec![m("Album", Some("Artist"), "Artist", 1), m("Album", Some("Artist"), "Artist", 2)];
        assert_eq!(keys(&ms), vec!["t:artist|album|m:/x"; 2], "the key earlier versions made");
    }

    #[test]
    fn a_featured_credit_does_not_split_an_album() {
        // No album artist; the first credited artist of "A feat. B" is A.
        let ms = vec![m("Low Tide", None, "Harbour Lights", 1), m("Low Tide", None, "Harbour Lights", 2), m("Low Tide", None, "Harbour Lights", 3)];
        assert_eq!(distinct(&keys(&ms)), 1);
        assert_eq!(keys(&ms)[0], "t:harbour lights|low tide|m:/x");
    }

    #[test]
    fn an_album_artist_on_some_tracks_holds_for_all() {
        let ms = vec![m("Album", Some("Band"), "Band", 1), m("Album", None, "Band feat. X", 2), m("Album", None, "Someone Else", 3)];
        let k = keys(&ms);
        assert_eq!(distinct(&k), 1);
        assert_eq!(k[0], "t:band|album|m:/x");
    }

    #[test]
    fn a_compilation_without_an_album_artist_is_one_album() {
        let ms: Vec<Member> = ["A", "B", "C", "D"].iter().enumerate().map(|(i, a)| m("Summer Hits", None, a, i as u32 + 1)).collect();
        let k = keys(&ms);
        assert_eq!(distinct(&k), 1);
        assert_eq!(k[0], "t:various artists|summer hits|m:/x");
    }

    #[test]
    fn two_albums_of_one_title_stay_apart() {
        // Same folder, same title, two artists, numbers repeated: two albums.
        let mut ms = Vec::new();
        for a in ["Copper Monks", "Lunar Sparrows"] {
            for t in 1..=3 {
                ms.push(m("Greatest Hits", None, a, t));
            }
        }
        let k = keys(&ms);
        assert_eq!(distinct(&k), 2);
        assert!(k.contains(&"t:copper monks|greatest hits|m:/x".to_owned()));
    }

    #[test]
    fn the_compilation_flag_makes_one_album() {
        let mut ms: Vec<Member> = ["A", "B"].iter().map(|a| m("Night Drive", None, a, 1)).collect();
        assert_eq!(distinct(&keys(&ms)), 2, "repeated numbers, no flag: apart");
        ms[1].compilation = true;
        assert_eq!(keys(&ms), vec!["t:various artists|night drive|m:/x"; 2]);
        // And with album artists that disagree.
        let mut ms = vec![m("Mix", Some("A"), "A", 1), m("Mix", Some("B"), "B", 2)];
        assert_eq!(distinct(&keys(&ms)), 2, "album artists as tagged");
        ms[0].compilation = true;
        assert_eq!(distinct(&keys(&ms)), 1);
    }

    #[test]
    fn loose_singles_without_an_album_tag_stay_apart() {
        // No album tag: no compilation is inferred.
        let ms = vec![m("", None, "A", 1), m("", None, "B", 2), m("", None, "C", 3)];
        assert_eq!(distinct(&keys(&ms)), 3);
        assert_eq!(keys(&ms)[0], "t:a||m:/x", "the key earlier versions made");
    }

    #[test]
    fn various_artists_tags_and_case() {
        let ms = vec![m("Comp", Some("VA"), "A", 1), m("Comp", Some("Various Artists"), "B", 2)];
        assert_eq!(keys(&ms), vec!["t:various artists|comp|m:/x"; 2]);
        let ms = vec![m("Album", Some("The  National"), "x", 1), m("ALBUM ", Some("the national"), "y", 2)];
        assert_eq!(distinct(&keys(&ms)), 1, "case and spaces ignored");
    }

    #[test]
    fn musicbrainz_ids_win() {
        let mut ms = vec![m("A", Some("X"), "X", 1), m("B", Some("Y"), "Y", 1)];
        for x in &mut ms {
            x.mb_album = Some("ABC".into());
        }
        assert_eq!(keys(&ms), vec!["mb:abc"; 2]);
        let refs: Vec<&Member> = ms.iter().collect();
        assert_eq!(decide_one(&refs), By::Explicit("x".into()), "the most common, ties to the first");
    }

    #[test]
    fn a_track_without_an_artist_goes_with_the_others() {
        let ms = vec![m("Album", None, "A", 1), m("Album", None, "", 2)];
        assert_eq!(keys(&ms), vec!["t:a|album|m:/x"; 2]);
    }

    #[test]
    fn no_artist_at_all() {
        let ms = vec![m("", None, "", 1), m("", None, "", 2)];
        assert_eq!(keys(&ms), vec!["t:||m:/x"; 2], "the key earlier versions made");
    }
}
