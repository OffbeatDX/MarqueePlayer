//! Technical details per format (`library::audio`), read by the scan from ffmpeg-made
//! fixtures, and the fill of tracks scanned before details were kept (schema 7): it
//! writes only the details, re-reads no tags, re-makes no art and leaves dates, ids and
//! plays as they were. Skips without ffmpeg on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use library::audio::{AudioInfo, Codec, compact, full};
use library::query;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().is_ok_and(|o| o.status.success())
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// (file name, sample rate, channel layout, encoder args)
const CASES: &[(&str, u32, &str, &[&str])] = &[
    ("01 flac16.flac", 44_100, "stereo", &["-c:a", "flac", "-sample_fmt", "s16"]),
    ("02 flac24.flac", 96_000, "stereo", &["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"]),
    ("03 cbr.mp3", 44_100, "stereo", &["-c:a", "libmp3lame", "-b:a", "320k"]),
    ("04 vbr.mp3", 44_100, "stereo", &["-c:a", "libmp3lame", "-q:a", "2"]),
    ("05 opus.opus", 44_100, "stereo", &["-c:a", "libopus", "-b:a", "128k"]),
    ("06 vorbis.ogg", 44_100, "stereo", &["-c:a", "libvorbis", "-q:a", "5"]),
    ("07 aac.m4a", 44_100, "stereo", &["-c:a", "aac", "-b:a", "256k"]),
    ("08 alac.m4a", 48_000, "stereo", &["-c:a", "alac", "-sample_fmt", "s32p", "-bits_per_raw_sample", "24"]),
    ("09 wav.wav", 44_100, "stereo", &["-c:a", "pcm_s16le"]),
    ("10 aiff.aiff", 48_000, "stereo", &["-c:a", "pcm_s24be", "-write_id3v2", "1"]),
    ("11 mono.flac", 44_100, "mono", &["-c:a", "flac", "-sample_fmt", "s16"]),
    ("12 surround.m4a", 48_000, "5.1", &["-c:a", "aac", "-b:a", "384k"]),
    ("13 mp2.mp2", 48_000, "stereo", &["-c:a", "mp2", "-b:a", "256k"]),
];

fn make(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for (i, (name, rate, layout, args)) in CASES.iter().enumerate() {
        let out = dir.join(name);
        let input = format!("sine=frequency=440:sample_rate={rate}:duration=6");
        let noise = format!("anoisesrc=color=pink:sample_rate={rate}:amplitude=0.1:duration=6");
        let filter = format!("[0][1]amix=inputs=2:duration=shortest,aformat=channel_layouts={layout}");
        let title = format!("title=T{}", i + 1);
        let track = format!("track={}", i + 1);
        let mut v = vec!["-f", "lavfi", "-i", &input, "-f", "lavfi", "-i", &noise, "-filter_complex", &filter];
        v.extend(args.iter());
        v.extend(["-metadata", &title, "-metadata", "artist=Details", "-metadata", "album=Details", "-metadata", &track]);
        let path = out.to_str().unwrap().to_owned();
        v.push(&path);
        assert!(ffmpeg(&v), "ffmpeg made {name}");
    }
}

fn details_of(tracks: &[library::Track], name: &str) -> AudioInfo {
    tracks.iter().find(|t| t.path.ends_with(name)).unwrap_or_else(|| panic!("{name} scanned")).audio
}

#[test]
fn each_format_reports_what_it_can() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("details-formats");
    let _ = std::fs::remove_dir_all(&root);
    make(&root.join("music").join("Details"));
    let art = library::ArtCache::new(root.join("art"));
    let mut conn = library::db::open_in_memory().unwrap();
    let r = library::scan::scan(&mut conn, &art, &[root.join("music")], &|_| {}).unwrap();
    assert!(r.errors.is_empty(), "unreadable: {:?}", r.errors);
    assert_eq!(r.details, 0, "read with the tags; nothing left to fill");
    // Every track, whichever album it landed in (ffmpeg's MP2 muxer writes no tags).
    let tracks: Vec<library::Track> = query::all_albums(&conn, query::AlbumSort::Title, false)
        .unwrap()
        .iter()
        .flat_map(|a| query::album_tracks(&conn, a.id).unwrap())
        .collect();
    assert_eq!(tracks.len(), CASES.len());
    let mut table = String::new();
    for t in &tracks {
        table.push_str(&format!("{:<18} {:?}  {:<40} {}\n", Path::new(&t.path).file_name().unwrap().to_string_lossy(), t.audio, full(&t.audio), compact(&t.audio)));
    }
    eprintln!("{table}");

    let d = |n: &str| details_of(&tracks, n);
    // A lossy file's average bitrate, independently: its size over its duration (the
    // fixtures' tags are a few hundred bytes).
    let average = |n: &str| -> u32 {
        let t = tracks.iter().find(|t| t.path.ends_with(n)).unwrap();
        (std::fs::metadata(&t.path).unwrap().len() as f64 * 8.0 / t.duration / 1000.0).round() as u32
    };
    let near_average = |n: &str| {
        let (got, want) = (d(n).bitrate.unwrap_or(0), average(n));
        assert!(got.abs_diff(want) <= want / 8 + 3, "{n}: {got} kbps, the file averages {want}");
    };
    // Lossless: depth and rate exactly; bitrate the data over the duration.
    assert_eq!((d("flac16.flac").codec, d("flac16.flac").bit_depth, d("flac16.flac").sample_rate, d("flac16.flac").channels), (Codec::Flac, Some(16), Some(44_100), Some(2)));
    assert_eq!((d("flac24.flac").codec, d("flac24.flac").bit_depth, d("flac24.flac").sample_rate), (Codec::Flac, Some(24), Some(96_000)));
    assert!(d("flac24.flac").bitrate.is_some_and(|b| b > 1_000), "{:?}", d("flac24.flac"));
    assert_eq!((d("alac.m4a").codec, d("alac.m4a").bit_depth, d("alac.m4a").sample_rate), (Codec::Alac, Some(24), Some(48_000)));
    assert_eq!((d("wav.wav").codec, d("wav.wav").bit_depth, d("wav.wav").sample_rate, d("wav.wav").bitrate), (Codec::Wav, Some(16), Some(44_100), Some(1_411)));
    assert_eq!((d("aiff.aiff").codec, d("aiff.aiff").bit_depth, d("aiff.aiff").sample_rate, d("aiff.aiff").bitrate), (Codec::Aiff, Some(24), Some(48_000), Some(2_304)));
    // Lossy: no bit depth, ever; the bitrate is what describes them.
    for n in ["cbr.mp3", "vbr.mp3", "opus.opus", "vorbis.ogg", "aac.m4a", "surround.m4a", "mp2.mp2"] {
        assert_eq!(d(n).bit_depth, None, "{n}: a lossy stream has no bit depth");
        assert!(d(n).bitrate.is_some(), "{n}: a bitrate");
    }
    assert_eq!((d("cbr.mp3").codec, d("cbr.mp3").bitrate, d("cbr.mp3").sample_rate), (Codec::Mp3, Some(320), Some(44_100)), "CBR: exactly the bitrate set");
    assert_eq!(d("vbr.mp3").codec, Codec::Mp3);
    assert_eq!((d("opus.opus").codec, d("opus.opus").sample_rate), (Codec::Opus, Some(48_000)), "Opus decodes at 48 kHz whatever it was made from");
    assert_eq!(d("vorbis.ogg").codec, Codec::Vorbis);
    assert_eq!(d("aac.m4a").codec, Codec::Aac);
    assert_eq!((d("mp2.mp2").codec, d("mp2.mp2").bitrate), (Codec::Mp2, Some(256)));
    // VBR and ABR streams: the average, as the file has it.
    for n in ["vbr.mp3", "opus.opus", "vorbis.ogg", "aac.m4a", "surround.m4a"] {
        near_average(n);
    }
    // Channels.
    assert_eq!(d("mono.flac").channels, Some(1));
    assert_eq!(full(&d("mono.flac")).rsplit(" · ").next(), Some("mono"));
    assert_eq!((d("surround.m4a").codec, d("surround.m4a").channels), (Codec::Aac, Some(6)));
    assert!(full(&d("surround.m4a")).ends_with(" · 5.1"), "{}", full(&d("surround.m4a")));
    // The album is mixed, with no format holding most of it.
    let summary = library::audio::album_summary(&tracks.iter().map(|t| (t.audio, t.duration)).collect::<Vec<_>>());
    assert_eq!(summary.line, "Mixed formats");
    // The file report (Properties) agrees with the scan.
    let rep = library::tags::file_report(Path::new(&tracks[1].path)).unwrap();
    assert_eq!(rep.audio, tracks[1].audio);
}

/// A library scanned before schema 7: its tracks have no details. The next full scan
/// fills them, reading only the files' properties: no track is re-read as changed, no
/// album's art is looked for again, and ids, dates, mtimes, plays and favourites stay.
#[test]
fn tracks_from_before_get_their_details_and_nothing_else_changes() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("details-fill");
    let _ = std::fs::remove_dir_all(&root);
    let music = root.join("music");
    make(&music.join("A"));
    // A second album, as a copy under another name: more than one album's art.
    let b = music.join("B");
    std::fs::create_dir_all(&b).unwrap();
    for (name, ..) in &CASES[..4] {
        std::fs::copy(music.join("A").join(name), b.join(name)).unwrap();
    }
    let art = library::ArtCache::new(root.join("art"));
    let mut conn = library::db::open_in_memory().unwrap();
    library::scan::scan(&mut conn, &art, std::slice::from_ref(&music), &|_| {}).unwrap();

    // As a schema-6 library is after migrating: no details. Some history on it, and
    // "recently added" dates and art stamps that must survive.
    conn.execute_batch(
        "UPDATE tracks SET codec = NULL, sample_rate = NULL, bit_depth = NULL, bitrate = NULL, channels = NULL;
         UPDATE albums SET added_at = 1234, first_seen = 1000;
         INSERT INTO plays SELECT id, 5000 FROM tracks;
         INSERT INTO favourite_tracks SELECT id, 6000 FROM tracks LIMIT 2;",
    )
    .unwrap();
    let snapshot = |conn: &rusqlite::Connection| -> Vec<String> {
        let mut out = Vec::new();
        for sql in [
            "SELECT id, path, album_id, title, artist, disc_no, track_no, duration, genre, mtime, size, added_at FROM tracks ORDER BY id",
            "SELECT * FROM albums ORDER BY id",
            "SELECT * FROM plays ORDER BY rowid",
            "SELECT * FROM favourite_tracks ORDER BY track_id",
        ] {
            let mut st = conn.prepare(sql).unwrap();
            let n = st.column_count();
            let rows: Vec<String> = st
                .query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join(",")))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            out.push(rows.join("\n"));
        }
        out
    };
    let before = snapshot(&conn);
    let n_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();

    let phases = std::sync::Mutex::new(Vec::new());
    let r = library::scan::scan(&mut conn, &art, std::slice::from_ref(&music), &|p| phases.lock().unwrap().push(p.phase)).unwrap();
    assert_eq!(r.read, 0, "no file is re-read as changed");
    assert_eq!(r.albums_changed, 0, "no album's art is made again");
    assert_eq!(r.removed, 0);
    assert!(r.held.is_empty());
    assert_eq!(r.details as i64, n_tracks, "every track filled");
    assert!(phases.lock().unwrap().contains(&library::scan::Phase::Details));
    assert_eq!(snapshot(&conn), before, "everything but the details is as it was");
    let missing: i64 = conn.query_row("SELECT COUNT(*) FROM tracks WHERE codec IS NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(missing, 0);
    let flac: String = conn.query_row("SELECT codec FROM tracks WHERE path LIKE '%flac24.flac' LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(flac, "FLAC");

    // Done once: the next scan has nothing to fill.
    let r = library::scan::scan(&mut conn, &art, std::slice::from_ref(&music), &|_| {}).unwrap();
    assert_eq!((r.read, r.details, r.albums_changed), (0, 0, 0));

    // A file that cannot be read gets "" (known unknown) and is not tried again.
    std::fs::write(music.join("A").join("zz broken.flac"), b"not audio at all").unwrap();
    let id: i64 = conn.query_row("SELECT id FROM tracks LIMIT 1", [], |r| r.get(0)).unwrap();
    let path: String = conn.query_row("SELECT path FROM tracks WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    // Make one known row point at the broken file as if it were scanned before (same
    // mtime and size), with no details: the fill reads it and fails.
    let meta = std::fs::metadata(music.join("A").join("zz broken.flac")).unwrap();
    let mtime = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    conn.execute(
        "INSERT INTO tracks (path, album_id, title, artist, duration, mtime, size, added_at)
         SELECT ?1, album_id, 'Broken', artist, 1.0, ?2, ?3, 1 FROM tracks WHERE id = ?4",
        rusqlite::params![music.join("A").join("zz broken.flac").to_string_lossy(), mtime, meta.len() as i64, id],
    )
    .unwrap();
    let r = library::scan::scan(&mut conn, &art, std::slice::from_ref(&music), &|_| {}).unwrap();
    assert_eq!(r.details, 1);
    let codec: Option<String> = conn.query_row("SELECT codec FROM tracks WHERE title = 'Broken'", [], |r| r.get(0)).unwrap();
    assert_eq!(codec.as_deref(), Some(""));
    let r = library::scan::scan(&mut conn, &art, std::slice::from_ref(&music), &|_| {}).unwrap();
    assert_eq!(r.details, 0, "not tried again");
    assert!(!path.is_empty());
}

/// Properties lists the loudness tags as tagged: REPLAYGAIN_* (any format) and R128_*
/// (Opus, which lofty keeps outside its generic keys).
#[test]
fn the_file_report_lists_the_replaygain_tags() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("details-gain");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (flac, opus, bare) = (dir.join("g.flac"), dir.join("g.opus"), dir.join("bare.flac"));
    let src = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"];
    let mut v: Vec<&str> = src.to_vec();
    let f = flac.to_str().unwrap().to_owned();
    v.extend(["-metadata", "REPLAYGAIN_TRACK_GAIN=-6.20 dB", "-metadata", "REPLAYGAIN_TRACK_PEAK=0.988", "-metadata", "REPLAYGAIN_ALBUM_GAIN=-5.10 dB", &f]);
    assert!(ffmpeg(&v));
    let mut v: Vec<&str> = src.to_vec();
    let o = opus.to_str().unwrap().to_owned();
    v.extend(["-c:a", "libopus", "-metadata", "R128_TRACK_GAIN=-1234", &o]);
    assert!(ffmpeg(&v));
    let mut v: Vec<&str> = src.to_vec();
    let b = bare.to_str().unwrap().to_owned();
    v.push(&b);
    assert!(ffmpeg(&v));
    let r = library::tags::file_report(&flac).unwrap();
    assert_eq!(r.replaygain, [("Track gain", "-6.20 dB".to_owned()), ("Track peak", "0.988".to_owned()), ("Album gain", "-5.10 dB".to_owned())]);
    let r = library::tags::file_report(&opus).unwrap();
    assert_eq!(r.replaygain, [("R128 track gain", "-1234".to_owned())]);
    assert_eq!(r.audio.codec, Codec::Opus);
    assert!(library::tags::file_report(&bare).unwrap().replaygain.is_empty());
}
