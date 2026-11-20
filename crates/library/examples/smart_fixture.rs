//! A library database for timing smart playlists without a music folder: `-- <out.db>
//! [tracks]` (default 20,000 tracks, 10 to an album). A believable spread of what rules look
//! at: several artists and genres per track, years and labels (some missing), mixed formats,
//! favourites, and a play history over three years in which
//! a few tracks are played a lot, most a little and many never, more of it recent. Its
//! playlists: three ordinary ones, the five smart starting points (`smart::templates`), and
//! one smart playlist that names another. The same database every run. Never point it at a
//! music folder or the app's own data: it replaces the file it is given.
//!
//!     cargo run --release -p library --example smart_fixture -- %TEMP%\mp-smart\library.db 20000
//!     cargo run --release -p library --example smart_time -- %TEMP%\mp-smart\library.db
use rusqlite::params;

const ARTISTS: &[&str] = &[
    "Beyoncé", "Sigur Rós", "Björk", "Mötley Crüe", "Céline Dion", "Zaz", "Stromae", "かめりあ", "ヨルシカ", "YOASOBI", "Ado", "Kasane Teto",
    "Daft Punk", "Røyksopp", "Ólafur Arnalds", "Antonín Dvořák", "Anaïs Mitchell", "Aphex Twin", "Boards of Canada", "米津玄師",
    "Earth, Wind & Fire", "AC/DC", "Miles Davis", "John Coltrane", "Bill Evans", "Nina Simone", "Radiohead", "Portishead", "Massive Attack",
    "Burial", "Four Tet", "Floating Points", "Kendrick Lamar", "MF DOOM", "Erykah Badu", "Fela Kuti", "Khruangbin", "Bonobo", "Nils Frahm",
];
const WORDS: &[&str] = &[
    "night", "été", "light", "fréquence", "ghost", "señal", "blue", "夜", "雨", "ひかり", "tokyo", "déjà", "vu", "storm", "café", "wave",
    "ocean", "naïve", "über", "dream", "heart", "fire", "ice", "city", "star", "love", "song", "drive", "river", "glass", "morning",
];
const GENRES: &[&str] = &["Jazz", "Rock", "Electronic", "Hip-Hop", "Ambient", "Classical", "Soul", "Pop", "Afrobeat", "Trip-Hop", "IDM", "Vocaloid"];
const LABELS: &[Option<&str>] = &[None, Some("Blue Note"), Some("ECM"), Some("Warp"), Some("Ninja Tune"), Some("XL"), Some("Impulse!"), Some("[no label]")];
const FORMATS: &[(&str, Option<i64>, i64, i64)] = &[
    ("FLAC", Some(16), 44_100, 900),
    ("FLAC", Some(24), 96_000, 2900),
    ("MP3", None, 44_100, 320),
    ("MP3", None, 44_100, 192),
    ("Opus", None, 48_000, 128),
    ("AAC", None, 44_100, 256),
    ("ALAC", Some(16), 44_100, 850),
    ("Vorbis", None, 44_100, 192),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("out.db"));
    let tracks: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(20_000);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
    let conn = library::db::open(&path).expect("open");
    let mut seed: u64 = 0x5eed_2025;
    let mut rnd = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let now = library::db::now();
    let tx = conn.unchecked_transaction().unwrap();
    let albums = tracks.div_ceil(10);
    let mut plays = 0usize;
    for a in 0..albums {
        let artist = ARTISTS[rnd(ARTISTS.len())];
        let album_artist = if rnd(20) == 0 { format!("{artist}; {}", ARTISTS[rnd(ARTISTS.len())]) } else { artist.to_owned() };
        let title = format!("{} {}", WORDS[rnd(WORDS.len())], WORDS[rnd(WORDS.len())]);
        let date = match rnd(12) {
            0 => None,
            1 => Some(format!("{}", 1960 + rnd(66))),
            _ => Some(format!("{}-{:02}-{:02}", 1960 + rnd(66), 1 + rnd(12), 1 + rnd(28))),
        };
        let genre = if rnd(4) == 0 { format!("{}; {}", GENRES[rnd(GENRES.len())], GENRES[rnd(GENRES.len())]) } else { GENRES[rnd(GENRES.len())].to_owned() };
        let label = LABELS[rnd(LABELS.len())];
        // Added over five years, the newest albums most often.
        let age = (rnd(1000) as f64 / 1000.0).powi(2) * 5.0 * 365.0 * 86_400.0;
        let added_ms = (now - age as i64) * 1000;
        tx.execute(
            "INSERT INTO albums (id, key, title, album_artist, date, genre, label, label_key, folder, added_at, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, lower(?7), ?8, ?9, ?9)",
            params![a as i64 + 1, format!("k{a}"), title, album_artist, date, genre, label, format!("M:\\m\\{a}"), added_ms],
        )
        .unwrap();
        if rnd(20) == 0 {
            tx.execute("INSERT INTO favourite_albums VALUES (?1, ?2)", params![a as i64 + 1, now]).unwrap();
        }
        let (codec, depth, rate, kbps) = FORMATS[rnd(FORMATS.len())];
        // How much this album is listened to: a few a lot, many never.
        let love = match rnd(10) {
            0..=3 => 0,
            4..=7 => 1 + rnd(3),
            8 => 4 + rnd(10),
            _ => 15 + rnd(40),
        };
        for t in 0..10 {
            let id = a * 10 + t;
            if id >= tracks {
                break;
            }
            let track_artist = if rnd(6) == 0 { format!("{artist} feat. {}", ARTISTS[rnd(ARTISTS.len())]) } else { artist.to_owned() };
            let duration = 90.0 + rnd(420) as f64 + rnd(1000) as f64 / 1000.0;
            let tid = id as i64 + 1;
            tx.execute(
                "INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at, codec, sample_rate, bit_depth, bitrate, channels, tags_read)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, 0, 0, 0, ?9, ?10, ?11, ?12, 2, 8)",
                params![tid, format!("M:\\m\\{a}\\{t:02}.{}", codec.to_lowercase()), a as i64 + 1, format!("{} {}", WORDS[rnd(WORDS.len())], WORDS[rnd(WORDS.len())]), track_artist, t as i64 + 1, duration, genre, codec, rate, depth, kbps],
            )
            .unwrap();
            let n = if love == 0 { 0 } else { rnd(love * 2 + 1) };
            for _ in 0..n {
                // More of it recent.
                let back = (rnd(1000) as f64 / 1000.0).powi(3) * 3.0 * 365.0 * 86_400.0;
                tx.execute("INSERT INTO plays VALUES (?1, ?2)", params![tid, now - back as i64]).unwrap();
                plays += 1;
            }
            if rnd(12) == 0 {
                tx.execute("INSERT INTO favourite_tracks VALUES (?1, ?2)", params![tid, now]).unwrap();
            }
        }
    }
    // Artists and genres from the rows, as the migration to schema 8 makes them.
    library::relations::rebuild_from_strings(&tx).unwrap();
    // Ordinary playlists: 50, 500 and 3,000 tracks.
    for (i, n) in [50usize, 500, 3000].iter().enumerate() {
        tx.execute("INSERT INTO playlists (id, name, created_at) VALUES (?1, ?2, ?3)", params![i as i64 + 1, format!("Mix of {n}"), now]).unwrap();
        for pos in 0..*n {
            tx.execute("INSERT INTO playlist_items VALUES (?1, ?2, ?3)", params![i as i64 + 1, pos as i64, 1 + rnd(tracks) as i64]).unwrap();
        }
    }
    let mut top = 0;
    for (name, smart) in library::smart::templates() {
        let id = library::query::create_smart_playlist(&tx, name, &smart.to_json()).unwrap();
        if name == "Favourites not heard in 6 months" {
            top = id;
        }
    }
    let nested = format!(r#"{{"v":1,"match":"all","rules":[{{"field":"playlist","op":"in","value":{top}}},{{"field":"lossless","op":"yes"}},{{"group":"any","rules":[{{"field":"genre","op":"is","value":"jazz"}},{{"field":"genre","op":"is","value":"soul"}}]}}],"sort":{{"by":"artist_album","desc":false}},"seed":1}}"#);
    library::query::create_smart_playlist(&tx, "Unheard favourites lossless jazz or soul", &nested).unwrap();
    tx.commit().unwrap();
    let (a, t) = library::query::counts(&conn).unwrap();
    println!("{a} albums, {t} tracks, {plays} plays in {}", path.display());
}
