//! A library database for timing search without a music folder: `-- <out.db> [tracks]`
//! (default 4,000 tracks, 12 to an album). Names mix Latin with accents, kana, kanji and
//! featured artists, the way a real library does; the same names every run. Then time it
//! with `search_time`:
//!
//!     cargo run --release -p library --example search_fixture -- %TEMP%\fixture.db 4000
//!     cargo run --release -p library --example search_time -- %TEMP%\fixture.db beyonce kameria "sigur ros"
use rusqlite::params;

const ARTISTS: &[&str] = &[
    "Beyoncé", "Sigur Rós", "Björk", "Mötley Crüe", "Céline Dion", "Héroes del Silencio", "Zaz", "Stromae", "かめりあ", "電音部",
    "ヨルシカ", "YOASOBI", "Ado", "Kasane Teto", "Yuta Imai;Kasane Teto", "Daft Punk", "Røyksopp", "Ólafur Arnalds", "Jóhann Jóhannsson",
    "Dvořák", "Antonín Dvořák", "Łona", "Anaïs Mitchell", "Mañana", "Aphex Twin", "Boards of Canada", "Hanazawa Kana", "米津玄師",
    "Earth, Wind & Fire", "AC/DC",
];
const WORDS: &[&str] = &[
    "night", "été", "light", "fréquence", "ghost", "señal", "blue", "夜", "雨", "ひかり", "ラブソング", "tokyo", "déjà", "vu", "storm",
    "café", "brain", "wave", "ocean", "naïve", "über", "garçon", "dream", "heart", "fire", "ice", "city", "star", "ルーム", "サマー",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("out.db"));
    let tracks: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(4000);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
    let conn = library::db::open(&path).expect("open");
    let mut seed: u64 = 0x5eed;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let tx = conn.unchecked_transaction().unwrap();
    let albums = tracks.div_ceil(12);
    for a in 0..albums {
        let artist = ARTISTS[rnd(ARTISTS.len())];
        let title = format!("{} {}", WORDS[rnd(WORDS.len())], WORDS[rnd(WORDS.len())]);
        tx.execute(
            "INSERT INTO albums (id, key, title, album_artist, date, genre, label, folder, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![a as i64 + 1, format!("k{a}"), title, artist, format!("{}-01-01", 1990 + rnd(35)), ["Pop", "Electronic", "Rock", "Hardcore"][rnd(4)], "Label", format!("M:/m/{a}"), a as i64],
        )
        .unwrap();
        for t in 0..12 {
            let id = a * 12 + t;
            if id >= tracks {
                break;
            }
            let track_artist = if rnd(5) == 0 { format!("{artist} feat. {}", ARTISTS[rnd(ARTISTS.len())]) } else { artist.to_owned() };
            tx.execute(
                "INSERT INTO tracks (id, path, album_id, title, artist, disc_no, track_no, duration, mtime, size, added_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 200.0, 0, 0, 0)",
                params![id as i64 + 1, format!("M:/m/{a}/{t:02}.flac"), a as i64 + 1, format!("{} {}", WORDS[rnd(WORDS.len())], WORDS[rnd(WORDS.len())]), track_artist, t as i64 + 1],
            )
            .unwrap();
        }
    }
    // Artists and genres from the rows, as the migration to schema 8 makes them.
    library::relations::rebuild_from_strings(&tx).unwrap();
    tx.commit().unwrap();
    println!("{}: {albums} albums, {tracks} tracks", path.display());
}
