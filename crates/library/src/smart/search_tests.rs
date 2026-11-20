//! Search operators: the parser (quotes, ranges, comparisons, lengths, relative dates,
//! negation, OR, garbage, thousands of random strings), and the rules it makes, evaluated on
//! the smart playlists' fixture library against the in-memory reference and against answers
//! worked out by hand from the tracks.

use std::collections::HashSet;

use super::super::reference::Lib;
use super::super::tests::{Rng, ctx, fixture};
use super::*;
use crate::smart::{Limiting, evaluate_ids};

fn p(q: &str) -> Parsed {
    parse(q, &|name| (name.eq_ignore_ascii_case("road trip")).then_some(7))
}

fn cond(f: Field, op: Op, v: Value) -> Atom {
    Atom::Rule(Node::Cond(Cond::new(f, op, v)))
}

fn num(v: f64) -> Value {
    Value::Num(v)
}

fn date(s: &str) -> DateSpec {
    DateSpec::parse(s).unwrap()
}

/// The single term of a one-term query.
fn one(q: &str) -> Atom {
    let parsed = p(q);
    assert_eq!(parsed.clauses.len(), 1, "{q}: {parsed:?}");
    assert_eq!(parsed.clauses[0].len(), 1, "{q}: {parsed:?}");
    parsed.clauses[0][0].clone()
}

fn text(neg: bool, t: &str) -> Atom {
    Atom::Text { neg, text: t.into() }
}

#[test]
fn text_fields_with_quotes_and_bare_words() {
    let parsed = p(r#"artist:"nina simone" live"#);
    assert_eq!(parsed.clauses, vec![vec![cond(Field::Artist, Op::Contains, Value::Text("nina simone".into()))], vec![text(false, "live")]]);
    assert!(parsed.has_operators() && parsed.notes.is_empty());
    // Case-insensitive names, aliases, typographic quotes, an unclosed quote, a space after the colon.
    assert_eq!(one("ARTIST:Björk"), cond(Field::Artist, Op::Contains, Value::Text("Björk".into())));
    assert_eq!(one("AlbumArtist:\u{201c}The Band\u{201d}"), cond(Field::AlbumArtist, Op::Contains, Value::Text("The Band".into())));
    assert_eq!(one("album_artist:x"), cond(Field::AlbumArtist, Op::Contains, Value::Text("x".into())));
    assert_eq!(one(r#"title:"love song"#), cond(Field::Title, Op::Contains, Value::Text("love song".into())));
    assert_eq!(one("genre: jazz"), cond(Field::Genre, Op::Contains, Value::Text("jazz".into())));
    assert_eq!(one("label:=ECM"), cond(Field::Label, Op::Is, Value::Text("ECM".into())));
    assert_eq!(one(r#"album:="Kind of Blue""#), cond(Field::Album, Op::Is, Value::Text("Kind of Blue".into())));
    assert_eq!(one("format:flac"), cond(Field::Format, Op::Contains, Value::Text("flac".into())));
    // A quoted phrase is one text term; words are each their own.
    let parsed = p(r#""night drive" café is:lossless"#);
    assert_eq!(parsed.clauses[0], vec![text(false, "night drive")]);
    assert_eq!(parsed.clauses[1], vec![text(false, "café")]);
    // A value with colons keeps them.
    assert_eq!(one("title:re:zero"), cond(Field::Title, Op::Contains, Value::Text("re:zero".into())));
}

#[test]
fn numbers_compare_and_range() {
    assert_eq!(one("year:1994"), cond(Field::Year, Op::Is, num(1994.0)));
    assert_eq!(one("year:1990..1999"), cond(Field::Year, Op::Between, Value::Range(1990.0, 1999.0)));
    assert_eq!(one("year:1999..1990"), cond(Field::Year, Op::Between, Value::Range(1990.0, 1999.0)), "either way round");
    assert_eq!(one("year:1990-1999"), cond(Field::Year, Op::Between, Value::Range(1990.0, 1999.0)));
    assert_eq!(one("year:1990s"), cond(Field::Year, Op::Between, Value::Range(1990.0, 1999.0)));
    assert_eq!(one("year:>=2010"), cond(Field::Year, Op::AtLeast, num(2010.0)));
    assert_eq!(one("year:2010.."), cond(Field::Year, Op::AtLeast, num(2010.0)));
    assert_eq!(one("year:..1980"), cond(Field::Year, Op::AtMost, num(1980.0)));
    assert_eq!(one("rating:>=4"), cond(Field::Rating, Op::AtLeast, num(4.0)));
    assert_eq!(one("rating:4+"), cond(Field::Rating, Op::AtLeast, num(4.0)));
    assert_eq!(one("rating:<3"), cond(Field::Rating, Op::AtMost, num(2.0)));
    assert_eq!(one("plays:0"), cond(Field::Plays, Op::Is, num(0.0)));
    assert_eq!(one("plays:>10"), cond(Field::Plays, Op::AtLeast, num(11.0)));
    assert_eq!(one("plays:<=3"), cond(Field::Plays, Op::AtMost, num(3.0)));
    assert_eq!(one("bitrate:>=320"), cond(Field::Bitrate, Op::AtLeast, num(320.0)));
    assert_eq!(one("bitrate:320kbps"), cond(Field::Bitrate, Op::Is, num(320.0)));
    assert_eq!(one("samplerate:>44.1"), cond(Field::SampleRate, Op::AtLeast, num(44_101.0)));
    assert_eq!(one("samplerate:96k"), cond(Field::SampleRate, Op::Is, num(96_000.0)));
    assert_eq!(one("samplerate:48000"), cond(Field::SampleRate, Op::Is, num(48_000.0)));
    assert_eq!(one("bitdepth:24bit"), cond(Field::BitDepth, Op::Is, num(24.0)));
    assert_eq!(one("disc:2"), cond(Field::Disc, Op::Is, num(2.0)));
    assert_eq!(one("track:1..3"), cond(Field::TrackNo, Op::Between, Value::Range(1.0, 3.0)));
}

#[test]
fn lengths_read_as_the_editor_reads_them_and_more() {
    assert_eq!(one("length:<3:00"), cond(Field::Duration, Op::AtMost, num(179.0)));
    assert_eq!(one("length:>=1:02:03"), cond(Field::Duration, Op::AtLeast, num(3723.0)));
    assert_eq!(one("length:>10"), cond(Field::Duration, Op::AtLeast, num(601.0)), "plain numbers are minutes");
    assert_eq!(one("duration:90s"), cond(Field::Duration, Op::Is, num(90.0)));
    assert_eq!(one("length:5m..8m"), cond(Field::Duration, Op::Between, Value::Range(300.0, 480.0)));
    assert_eq!(one("time:1h+"), cond(Field::Duration, Op::AtLeast, num(3600.0)));
    for bad in ["length:3:75", "length:abc", "length:-3", "length:1:2:3:4"] {
        let parsed = p(bad);
        assert!(!parsed.has_operators(), "{bad}: {parsed:?}");
        assert_eq!(parsed.notes.len(), 1, "{bad}");
    }
}

#[test]
fn dates_relative_and_absolute() {
    assert_eq!(one("added:<30d"), cond(Field::Added, Op::InLast, Value::Ago(30, Unit::Days)));
    assert_eq!(one("added:30d"), cond(Field::Added, Op::InLast, Value::Ago(30, Unit::Days)));
    assert_eq!(one("added:<2w"), cond(Field::Added, Op::InLast, Value::Ago(2, Unit::Weeks)));
    assert_eq!(one("added:>6months"), cond(Field::Added, Op::NotInLast, Value::Ago(6, Unit::Months)));
    assert_eq!(one("played:>1y"), cond(Field::LastPlayed, Op::NotInLast, Value::Ago(1, Unit::Years)));
    assert_eq!(one("played:<7d"), cond(Field::LastPlayed, Op::InLast, Value::Ago(7, Unit::Days)));
    assert_eq!(one("played:never"), cond(Field::Played, Op::No, Value::None));
    assert_eq!(one("added:2024"), cond(Field::Added, Op::Between, Value::Dates(date("2024"), date("2024"))));
    assert_eq!(one("added:2024-03"), cond(Field::Added, Op::Between, Value::Dates(date("2024-03"), date("2024-03"))));
    assert_eq!(one("added:>2024-03"), cond(Field::Added, Op::After, Value::Date(date("2024-03"))));
    assert_eq!(one("added:>=2024-03"), cond(Field::Added, Op::After, Value::Date(date("2024-02"))));
    assert_eq!(one("added:>=2024-01"), cond(Field::Added, Op::After, Value::Date(date("2023-12"))));
    assert_eq!(one("added:<2024"), cond(Field::Added, Op::Before, Value::Date(date("2024"))));
    assert_eq!(one("added:<=2024-12-31"), cond(Field::Added, Op::Before, Value::Date(date("2025-01-01"))));
    assert_eq!(one("added:2023..2024-06"), cond(Field::Added, Op::Between, Value::Dates(date("2023"), date("2024-06"))));
    assert_eq!(one("added:2023.."), cond(Field::Added, Op::After, Value::Date(date("2022"))));
    assert_eq!(one("played:..2020"), cond(Field::LastPlayed, Op::Before, Value::Date(date("2021"))));
    for bad in ["added:yesterday", "added:0d", "added:30x", "added:2024-13", "played:>", "added:..", "added:<=9999"] {
        let parsed = p(bad);
        assert!(!parsed.has_operators(), "{bad}: {parsed:?}");
        assert!(!parsed.notes.is_empty(), "{bad}");
    }
}

#[test]
fn flags_and_playlists() {
    for (q, f, op) in [
        ("is:favourite", Field::Favourite, Op::Yes),
        ("is:favorite", Field::Favourite, Op::Yes),
        ("IS:Fav", Field::Favourite, Op::Yes),
        ("is:favalbum", Field::AlbumFavourite, Op::Yes),
        ("is:local", Field::Remote, Op::No),
        ("is:remote", Field::Remote, Op::Yes),
        ("is:lossless", Field::Lossless, Op::Yes),
        ("is:lossy", Field::Lossless, Op::No),
        ("is:played", Field::Played, Op::Yes),
        ("is:unplayed", Field::Played, Op::No),
    ] {
        assert_eq!(one(q), cond(f, op, Value::None), "{q}");
    }
    assert_eq!(one("is:unrated"), cond(Field::Rating, Op::Is, num(0.0)));
    assert_eq!(one("has:rating"), cond(Field::Rating, Op::IsNot, num(0.0)));
    assert_eq!(one(r#"playlist:"Road Trip""#), cond(Field::Playlist, Op::In, Value::Id(7)));
    assert_eq!(one("-in:\"road trip\""), cond(Field::Playlist, Op::NotIn, Value::Id(7)));
    let parsed = p("playlist:nope");
    assert_eq!(parsed.clauses, vec![vec![text(false, "playlist:nope")]]);
    assert_eq!(parsed.notes, vec!["\u{201c}playlist:nope\u{201d}: no playlist is called \u{201c}nope\u{201d}".to_owned()]);
    let parsed = p("has:lyrics");
    assert!(!parsed.has_operators() && parsed.notes[0].contains("lyrics"), "{parsed:?}");
}

#[test]
fn negation_flips_to_the_rules_own_opposites() {
    assert_eq!(one("-genre:live"), cond(Field::Genre, Op::NotContains, Value::Text("live".into())));
    assert_eq!(one("-is:remote"), cond(Field::Remote, Op::No, Value::None));
    assert_eq!(one("-label:=ECM"), cond(Field::Label, Op::IsNot, Value::Text("ECM".into())));
    assert_eq!(one("-rating:>=4"), cond(Field::Rating, Op::AtMost, num(3.0)));
    assert_eq!(one("-plays:0"), cond(Field::Plays, Op::IsNot, num(0.0)));
    assert_eq!(one("-added:<30d"), cond(Field::Added, Op::NotInLast, Value::Ago(30, Unit::Days)));
    let any = |nodes: Vec<Node>| Atom::Rule(Node::Group(Group { all: false, nodes }));
    let c = |f: Field, op: Op, v: Value| Node::Cond(Cond::new(f, op, v));
    assert_eq!(one("-year:1990..1999"), any(vec![c(Field::Year, Op::AtMost, num(1989.0)), c(Field::Year, Op::AtLeast, num(2000.0))]));
    // Nothing is below 0 plays: only the upper side is left.
    assert_eq!(one("-plays:0..3"), cond(Field::Plays, Op::AtLeast, num(4.0)));
    // Never played is not last played before 2024.
    assert_eq!(one("-played:<2024"), any(vec![c(Field::LastPlayed, Op::After, Value::Date(date("2023"))), c(Field::Played, Op::No, Value::None)]));
    assert_eq!(
        one("-added:2024-02-29"),
        any(vec![c(Field::Added, Op::Before, Value::Date(date("2024-02-29"))), c(Field::Added, Op::After, Value::Date(date("2024-02-29")))])
    );
    // A negated word, with an operator in the query: none of the text fields has it.
    let parsed = p("-live is:lossless");
    assert_eq!(parsed.clauses[0], vec![text(true, "live")]);
    let s = parsed.rules(Level::Tracks).unwrap();
    assert_eq!(s.root.nodes.len(), 5, "the word's four 'does not contain' and the flag, flat: {}", s.to_json());
    // What nothing could match the opposite of is text, and said.
    let parsed = p("-plays:>=0");
    assert!(!parsed.has_operators() && parsed.notes.len() == 1, "{parsed:?}");
    // A dash inside a word, or alone, is not a negation.
    assert_eq!(p("a-ha is:local").clauses[0], vec![text(false, "a-ha")]);
}

#[test]
fn or_joins_its_neighbours_into_one_term() {
    let parsed = p("genre:jazz OR genre:soul year:>1990");
    assert_eq!(parsed.clauses.len(), 2);
    assert_eq!(parsed.clauses[0].len(), 2);
    let s = parsed.rules(Level::Tracks).unwrap();
    assert_eq!(
        s.to_json(),
        Smart {
            root: Group {
                all: true,
                nodes: vec![
                    Node::Group(Group {
                        all: false,
                        nodes: vec![
                            Node::Cond(Cond::new(Field::Genre, Op::Contains, Value::Text("jazz".into()))),
                            Node::Cond(Cond::new(Field::Genre, Op::Contains, Value::Text("soul".into())))
                        ]
                    }),
                    Node::Cond(Cond::new(Field::Year, Op::AtLeast, num(1991.0)))
                ]
            },
            sort: Sorting::Known(Sort::new(SortKey::Year)),
            ..Smart::default()
        }
        .to_json()
    );
    // Three in a row, words among them; lower-case "or" is a word; a dangling OR is dropped.
    let parsed = p("a OR b OR is:fav");
    assert_eq!(parsed.clauses.len(), 1);
    assert_eq!(parsed.clauses[0].len(), 3);
    assert!(parsed.has_operators());
    assert_eq!(p("now or never").clauses.len(), 3);
    assert!(!p("now or never").has_operators());
    assert_eq!(p("OR rock OR").clauses, vec![vec![text(false, "rock")]]);
    // OR alone between words is an operator.
    assert!(p("love OR hate").has_operators());
}

#[test]
fn what_is_not_understood_is_text_and_said() {
    let parsed = p("re:stacks year:1994");
    assert_eq!(parsed.clauses[0], vec![text(false, "re:stacks")]);
    assert_eq!(parsed.notes, vec!["\u{201c}re:\u{201d} is not a field".to_owned()]);
    // A time, a colon at the end, a URL-ish word: text, nothing said.
    for q in ["4:33", "Mission: Impossible", "12:00 am", ":)", "http://x"] {
        let parsed = p(q);
        assert!(!parsed.has_operators(), "{q}: {parsed:?}");
    }
    assert!(p("4:33").notes.is_empty() && p(":)").notes.is_empty() && p("Mission: Impossible").notes.is_empty());
    for q in ["rating:9", "rating:great", "year:85", "plays:-1", "bitrate:fast", "artist:", "composer:bach", "is:shiny", "has:art", "length:"] {
        let parsed = p(q);
        assert!(!parsed.has_operators(), "{q}: {parsed:?}");
        assert_eq!(parsed.notes.len(), 1, "{q}: {parsed:?}");
    }
    assert!(p("rating:9").notes[0].contains("0 to 5"), "{:?}", p("rating:9").notes);
    // A plain query reads as plain: one text term per word, no operators.
    let parsed = p("nina simone");
    assert!(!parsed.has_operators() && parsed.notes.is_empty());
}

/// Random strings from the pieces queries are made of: none panics, every rule set made is one
/// the editor would save (complete, and the same after its JSON form), and a sample of them
/// runs as SQL on the fixture.
#[test]
fn thousands_of_random_strings_never_panic_and_make_valid_rules() {
    const PIECES: [&str; 48] = [
        "artist:", "year:", "rating:", "plays:", "length:", "added:", "played:", "is:", "has:", "-", "\"", "\u{201c}", " ", " ", " ", "OR", "|", "..", ">", ">=",
        "<", "<=", "=", "+", ":", "1994", "0", "4", "3:00", "30d", "1y", "2024-03", "favourite", "remote", "lyrics", "nina simone", "é", "ヨルシカ", "%", "_", "\\",
        "'", ";", "\0", "-genre:", "samplerate:", "1e999", "99999999999999999999",
    ];
    let conn = fixture();
    let mut rng = Rng::new(42);
    let mut ran = 0;
    for i in 0..5000 {
        let n = 1 + rng.below(8);
        let q: String = (0..n).map(|_| *rng.pick(&PIECES)).collect();
        let parsed = p(&q);
        for level in [Level::Tracks, Level::Albums] {
            if let Some(s) = parsed.rules(level) {
                assert!(s.first_invalid().is_none(), "{q:?}: {:?} in {}", s.first_invalid(), s.to_json());
                assert!(s.problems().is_empty(), "{q:?}");
                assert_eq!(Smart::from_json(&s.to_json()).to_json(), s.to_json(), "{q:?}");
                if i % 10 == 0 {
                    evaluate_ids(&conn, &s, None, ctx()).unwrap_or_else(|e| panic!("{q:?}: {e}"));
                    ran += 1;
                }
            }
        }
    }
    assert!(ran > 500, "{ran}");
    // Pathological lengths and characters.
    for q in ["\"".repeat(10_000), "-".repeat(10_000), "a:".repeat(5_000), "year:".to_owned() + &"9".repeat(5_000), "\u{0}\u{FEFF}\u{202E}is:fav".into()] {
        let _ = p(&q).rules(Level::Tracks);
    }
}

// ---- the rules on the fixture library ---------------------------------------------------

fn ids(conn: &rusqlite::Connection, q: &str) -> Vec<i64> {
    let s = p(q).rules(Level::Tracks).unwrap_or_else(|| panic!("{q}"));
    evaluate_ids(conn, &s, None, ctx()).unwrap().0
}

fn set(v: Vec<i64>) -> HashSet<i64> {
    v.into_iter().collect()
}

fn has(hay: &str, needle: &str) -> bool {
    crate::fold::fold(hay).contains(&crate::fold::fold(needle))
}

/// Some tracks play from the server (their paths are the server's).
fn library() -> rusqlite::Connection {
    let conn = fixture();
    conn.execute("UPDATE tracks SET path = 'navidrome:song/' || id WHERE id % 7 = 0", []).unwrap();
    conn
}

#[test]
fn queries_select_what_they_say_on_the_fixture() {
    let conn = library();
    let lib = Lib::load(&conn);
    let all: HashSet<i64> = lib.tracks.iter().map(|t| t.id).collect();
    let year = |t: &super::super::reference::RTrack| t.date.as_deref().filter(|d| d.len() >= 4 && d[..4].bytes().all(|b| b.is_ascii_digit())).map(|d| d[..4].parse::<i64>().unwrap());
    let want = |f: &dyn Fn(&super::super::reference::RTrack) -> bool| -> HashSet<i64> { lib.tracks.iter().filter(|t| f(t)).map(|t| t.id).collect() };
    let words = |t: &super::super::reference::RTrack, w: &str| has(&t.title, w) || t.credited.iter().any(|a| has(a, w)) || has(&t.album, w) || t.album_artists.iter().any(|a| has(a, w));

    assert_eq!(set(ids(&conn, "rating:>=4")), want(&|t| t.rating >= 4));
    assert_eq!(set(ids(&conn, "-rating:>=4")), want(&|t| t.rating < 4));
    assert_eq!(set(ids(&conn, "plays:0")), want(&|t| t.plays.is_empty()));
    assert_eq!(set(ids(&conn, "plays:>2")), want(&|t| t.plays.len() > 2));
    assert_eq!(set(ids(&conn, "length:<3:00")), want(&|t| (t.duration as i64) < 180));
    assert_eq!(set(ids(&conn, "year:2000..2015")), want(&|t| year(t).is_some_and(|y| (2000..=2015).contains(&y))));
    assert_eq!(set(ids(&conn, "-year:2000..2015")), want(&|t| year(t).is_some_and(|y| !(2000..=2015).contains(&y))), "no year matches neither");
    assert_eq!(set(ids(&conn, "is:remote")), want(&|t| t.path.starts_with("navidrome:")));
    assert_eq!(set(ids(&conn, "is:local")), want(&|t| !t.path.starts_with("navidrome:")));
    assert_eq!(set(ids(&conn, "-is:remote")), set(ids(&conn, "is:local")));
    assert_eq!(set(ids(&conn, "is:favourite -is:remote")), want(&|t| t.favourite && !t.path.starts_with("navidrome:")));
    assert_eq!(set(ids(&conn, "genre:jazz")), want(&|t| t.genres.iter().any(|g| has(g, "jazz"))));
    assert_eq!(set(ids(&conn, "-genre:jazz")), want(&|t| !t.genres.iter().any(|g| has(g, "jazz"))));
    assert_eq!(set(ids(&conn, "genre:rock OR genre:ambient")), want(&|t| t.genres.iter().any(|g| has(g, "rock") || has(g, "ambient"))));
    assert_eq!(set(ids(&conn, "artist:beyonce")), want(&|t| t.credited.iter().any(|a| has(a, "beyonce"))));
    assert_eq!(set(ids(&conn, "love is:lossless")), want(&|t| words(t, "love") && matches!(t.codec.as_deref(), Some("FLAC" | "ALAC" | "WAV" | "AIFF"))));
    assert_eq!(set(ids(&conn, "-love rating:5")), want(&|t| !words(t, "love") && t.rating == 5));
    assert_eq!(set(ids(&conn, "band OR café plays:0")), want(&|t| (words(t, "band") || words(t, "café")) && t.plays.is_empty()));
    let ago = |n: i64| ctx().now - n * 86_400;
    assert_eq!(set(ids(&conn, "played:<30d")), want(&|t| t.plays.iter().any(|p| *p >= ago(30))));
    assert_eq!(set(ids(&conn, "played:>1y")), want(&|t| t.plays.iter().all(|p| *p < super::super::ago(ctx(), 1, Unit::Years))));
    assert_eq!(set(ids(&conn, "added:<90d")), want(&|t| t.added_ms >= ago(90) * 1000));
    // A term and its negation never overlap; for a value every track has, they cover the library.
    for q in ["rating:>=3", "plays:1..3", "length:>5", "is:favourite", "is:remote", "title:love", "played:<6m", "added:2024", "played:2024", "is:played", "label:=ECM"] {
        let a = set(ids(&conn, q));
        let b = set(ids(&conn, &format!("-{q}")));
        assert!(a.is_disjoint(&b), "{q}");
        assert_eq!(&a | &b, all, "{q} and its opposite cover the library");
    }
    // Every query's SQL agrees with the reference evaluation.
    for q in ["year:1990s OR is:favalbum rating:2..4", "-played:2024-06 artist:=Guest", "added:>=2024 -is:lossy", "bitrate:>=320 samplerate:>48 bitdepth:24", "disc:2 track:<=3"] {
        let s = p(q).rules(Level::Tracks).unwrap();
        assert_eq!(evaluate_ids(&conn, &s, None, ctx()).unwrap().0, lib.eval(&s, None, ctx()), "{q}");
    }
}

#[test]
fn track_only_terms_leave_albums_and_artists_out() {
    for q in ["rating:4", "love plays:0", "length:<3:00", "is:lossless", "title:x"] {
        assert!(p(q).rules(Level::Albums).is_none(), "{q}");
        assert!(p(q).artist_terms().is_none(), "{q}");
    }
    for q in ["year:1999", "artist:band", "genre:jazz", "label:ecm -is:remote", "is:favalbum added:<30d", "albumartist:band", "night"] {
        assert!(p(q).rules(Level::Albums).is_some(), "{q}");
    }
    assert_eq!(p("artist:nina -albumartist:simone").artist_terms(), Some(vec![(false, Op::Contains, "nina".into()), (true, Op::NotContains, "simone".into())]));
    assert!(p("artist:a OR artist:b").artist_terms().is_none());
    assert!(p("artist:a year:1999").artist_terms().is_none());
}

#[test]
fn search_takes_the_rules_path_only_with_operators() {
    let conn = library();
    let plain = crate::query::search(&conn, "love", 1000).unwrap();
    assert!(!plain.operators && plain.notes.is_empty() && !plain.tracks.is_empty());
    // "re:stacks" alone is plain search for the text, with the hint.
    let r = crate::query::search(&conn, "re:stacks", 10).unwrap();
    assert!(!r.operators && r.notes.len() == 1);
    // Tracks by the rules, albums with them, artists by name.
    let r = crate::query::search(&conn, "is:remote", 1000).unwrap();
    assert!(r.operators);
    assert!(r.tracks.iter().all(|t| t.path.starts_with("navidrome:")) && !r.tracks.is_empty());
    let want: HashSet<i64> = r.tracks.iter().map(|t| t.album_id).collect();
    assert_eq!(r.albums.iter().map(|a| a.id).collect::<HashSet<_>>(), want);
    assert!(r.artists.is_empty());
    let r = crate::query::search(&conn, "rating:5", 1000).unwrap();
    assert!(r.albums.is_empty() && r.artists.is_empty() && r.tracks.iter().all(|t| !t.title.is_empty()));
    let n = r.tracks.len();
    assert_eq!(n, Lib::load(&conn).tracks.iter().filter(|t| t.rating == 5).count());
    // The limit and "more".
    let r = crate::query::search(&conn, "rating:5", 3).unwrap();
    assert!(r.tracks.len() == 3 && r.more_tracks && !r.more_albums);
    let all = crate::query::search_section(&conn, "rating:5", crate::query::SearchKind::Tracks, usize::MAX).unwrap();
    assert_eq!(all.tracks.len(), n);
    // Artists by name, with a negation and album artists only.
    let r = crate::query::search(&conn, "artist:band -artist:abc", 100).unwrap();
    assert!(r.artists.iter().any(|a| a.name == "The Band") && !r.artists.iter().any(|a| a.name.contains("ＡＢＣ")), "{:?}", r.artists);
    let r = crate::query::search(&conn, "albumartist:=guest", 100).unwrap();
    let album_artists: HashSet<String> = conn.prepare("SELECT name FROM album_artists").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(r.artists.iter().map(|a| a.name.clone()).collect::<Vec<_>>(), if album_artists.contains("Guest") { vec!["Guest".to_owned()] } else { vec![] });
    // Hostile text stays a value.
    let r = crate::query::search(&conn, "title:\"'); DROP TABLE tracks;--\" is:local", 100).unwrap();
    assert!(r.operators);
    assert!(conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0)).unwrap() > 0);
}

#[test]
fn a_saved_query_is_an_ordinary_rule_set() {
    let s = p("artist:\"nina simone\" -genre:live year:1960..1969 plays:>3").rules(Level::Tracks).unwrap();
    assert!(s.limit.is_none() && s.first_invalid().is_none());
    assert_eq!(s.order(), Sort::new(SortKey::Year));
    let json = s.to_json();
    assert_eq!(Smart::from_json(&json), s);
    assert!(!matches!(s.limit, Some(Limiting::Unknown(_))));
    assert_eq!(
        super::super::summary(&s, &|_| None),
        super::super::summary(&Smart::from_json(&json), &|_| None),
    );
}
