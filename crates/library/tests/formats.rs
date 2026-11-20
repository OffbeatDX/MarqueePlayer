//! The scanner finds and reads the less common formats: AIFF (.aif, .aiff, .aifc),
//! raw AAC (.aac, ADTS with an ID3v2 tag), audiobooks in MP4 (.m4b) and MP2 (.mp2, tagged
//! here with lofty since ffmpeg's MP2 muxer writes no tags). Fixtures from ffmpeg.

use std::path::PathBuf;
use std::process::Command;

use library::query;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().is_ok_and(|o| o.status.success())
}

#[test]
fn new_formats_are_scanned_with_their_tags() {
    if !ffmpeg(&["-version"]) && Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not on PATH: skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scan-formats");
    let _ = std::fs::remove_dir_all(&root);
    let album = root.join("music").join("Formats");
    std::fs::create_dir_all(&album).unwrap();
    let cases: [(&str, &[&str]); 6] = [
        ("01.aiff", &["-c:a", "pcm_s16be", "-write_id3v2", "1"]),
        ("02.aif", &["-c:a", "pcm_s16be", "-write_id3v2", "1"]),
        ("03.aifc", &["-f", "aiff", "-c:a", "pcm_s16le", "-write_id3v2", "1"]),
        ("04.aac", &["-c:a", "aac", "-f", "adts", "-write_id3v2", "1"]),
        ("05.m4b", &["-c:a", "aac", "-f", "ipod"]),
        ("06.mp2", &["-c:a", "mp2"]),
    ];
    for (i, (name, args)) in cases.iter().enumerate() {
        let out = album.join(name);
        let title = format!("title=Track {}", i + 1);
        let track = format!("track={}", i + 1);
        let mut v = vec!["-f", "lavfi", "-i", "aevalsrc=0.2*sin(2*PI*440*t)|0.2*sin(2*PI*550*t):s=44100:d=4"];
        v.extend(["-metadata", &title, "-metadata", "artist=Format Tester", "-metadata", "album=Formats", "-metadata", &track]);
        v.extend(args.iter());
        let path = out.to_str().unwrap().to_owned();
        v.push(&path);
        assert!(ffmpeg(&v), "ffmpeg made {name}");
    }
    // MP2: an ID3v2 tag written with lofty.
    {
        use lofty::config::WriteOptions;
        use lofty::tag::{Accessor, Tag, TagExt, TagType};
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Track 6".into());
        tag.set_artist("Format Tester".into());
        tag.set_album("Formats".into());
        tag.set_track(6);
        tag.save_to_path(album.join("06.mp2"), WriteOptions::default()).unwrap();
    }

    let art = library::ArtCache::new(root.join("art"));
    let mut conn = library::db::open_in_memory().unwrap();
    let r = library::scan::scan(&mut conn, &art, &[root.join("music")], &|_| {}).unwrap();
    assert!(r.errors.is_empty(), "unreadable: {:?}", r.errors);
    assert_eq!(r.files, 6, "every extension is picked up");
    let albums = query::all_albums(&conn, query::AlbumSort::Title, false).unwrap();
    assert_eq!(albums.len(), 1, "one album: {:?}", albums.iter().map(|a| &a.title).collect::<Vec<_>>());
    let tracks = query::album_tracks(&conn, albums[0].id).unwrap();
    let got: Vec<(Option<u32>, String, String)> = tracks.iter().map(|t| (t.track_no, t.title.clone(), t.artist.clone())).collect();
    let want: Vec<(Option<u32>, String, String)> = (1..=6).map(|n| (Some(n), format!("Track {n}"), "Format Tester".to_owned())).collect();
    assert_eq!(got, want);
    for t in &tracks {
        // ADTS carries no length: lofty estimates it from the bitrate.
        let slack = if t.path.ends_with(".aac") { 0.25 } else { 0.05 };
        assert!((t.duration - 4.0).abs() < slack, "{}: duration {}", t.path, t.duration);
    }
}
