//! The metadata viewer's reader (`library::meta`) on real files: every tag block of each
//! format, multi-valued items, the field model, pictures, and files left byte for byte as
//! they were. Fixtures are made by ffmpeg and tagged by lofty (`support/meta_fixtures.rs`).
//! Skips without ffmpeg on PATH.

use std::path::{Path, PathBuf};

use library::meta::{self, BlockKind, Field, Group, ReadOptions};

#[path = "support/meta_fixtures.rs"]
mod fixtures;

fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn root() -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not on PATH: skipping");
        return None;
    }
    // One set for all the tests in this file (they run in parallel).
    static MADE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    Some(
        MADE.get_or_init(|| {
            let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("meta-fixtures");
            // Made anew: the folder is this test file's own, and a CI cache can bring it back
            // with its folders but none of its files, which `make` rightly refuses to fill.
            let _ = std::fs::remove_dir_all(&dir);
            fixtures::make(&dir);
            dir
        })
        .clone(),
    )
}

fn field<'a>(f: &'a [Field], label: &str) -> &'a Field {
    f.iter().find(|x| x.label() == label).unwrap_or_else(|| panic!("no {label} in {:?}", f.iter().map(Field::label).collect::<Vec<_>>()))
}

fn read(path: &Path) -> (meta::FileTags, Vec<Field>) {
    let t = meta::read(path, ReadOptions::ALL).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let f = meta::fields(&t);
    (t, f)
}

#[test]
fn a_picard_flac_has_everything_in_order() {
    let Some(root) = root() else { return };
    let (t, f) = read(&root.join("Picard Set/01 Northern Lights.flac"));
    assert_eq!(t.blocks, [BlockKind::Vorbis]);
    assert_eq!(field(&f, "Artist").values(), ["Aurora Vale", "Mika Rhee"]);
    assert_eq!(field(&f, "Genre").values(), ["Synthpop", "Dream Pop"]);
    assert_eq!(field(&f, "MusicBrainz artist").values().len(), 2);
    assert_eq!(field(&f, "Date").values(), ["2019.03.08"], "as tagged; the library shows it as ISO");
    // The groups come in their order, and the unknown item last under its raw key.
    let groups: Vec<Group> = f.iter().map(Field::group).collect();
    assert!(groups.windows(2).all(|w| w[0] <= w[1]), "{groups:?}");
    assert_eq!(f.last().map(Field::label).as_deref(), Some("DISCOGS_RELEASE_ID"));
    assert_eq!(field(&f, "Lyrics").group(), Group::Lyrics);
    // Two pictures: the cover and the artist, with sizes and thumbnails.
    assert_eq!(t.pictures.len(), 2);
    let front = &t.pictures[0];
    assert!(front.front && front.format == "PNG" && front.size == Some((1200, 1200)));
    assert_eq!(front.thumb.as_ref().map(|t| (t.width, t.height)), Some((128, 128)));
    assert_eq!(t.pictures[1].kind, "Artist");
    assert_eq!(t.pictures[1].description, "Aurora and Mika, live");
    // Tags only: no picture parsed.
    assert!(meta::read(&root.join("Picard Set/01 Northern Lights.flac"), ReadOptions::TAGS).unwrap().pictures.is_empty());
}

#[test]
fn id3_versions_and_a_second_block() {
    let Some(root) = root() else { return };
    let (t, f) = read(&root.join("ID3 Formats/01 Twenty Four.mp3"));
    assert_eq!(t.blocks, [BlockKind::Id3v2(4)]);
    assert_eq!(field(&f, "Artist").values(), ["Aurora Vale", "Mika Rhee"], "NUL-separated in ID3v2.4");
    assert_eq!(field(&f, "MusicBrainz recording").values(), ["b1b2b3b4-0000-4000-8000-000000000001"], "from UFID");
    assert_eq!(field(&f, "MusicBrainz release").sources[0].key, "TXXX:MusicBrainz Album Id");
    assert_eq!(field(&f, "Comment").values(), ["An ID3v2.4 comment."]);
    assert_eq!(field(&f, "COMM:iTunNORM").group(), Group::Other);
    assert_eq!(t.pictures.len(), 1);

    let (t, f) = read(&root.join("ID3 Formats/02 Twenty Three.mp3"));
    assert_eq!(t.blocks, [BlockKind::Id3v2(3), BlockKind::Id3v1]);
    let title = field(&f, "Title");
    assert_eq!(title.sources.len(), 2);
    assert!(title.differs(), "ID3v1 cuts the title at 30 characters");
    assert!(field(&f, "Genre").differs());
    assert!(!field(&f, "Artist").differs());
    assert!(!field(&f, "Date").differs(), "ID3v1's year is compared with ID3v2's date");
}

#[test]
fn opus_mp4_and_untagged_wav() {
    let Some(root) = root() else { return };
    let (_, f) = read(&root.join("Other Formats/01 Opus R128.opus"));
    assert_eq!(field(&f, "R128 track gain").values(), ["-1234"]);
    assert_eq!(field(&f, "R128 album gain").group(), Group::Loudness);
    let (t, f) = read(&root.join("Other Formats/02 AAC Atoms.m4a"));
    assert_eq!(t.blocks, [BlockKind::Mp4]);
    assert_eq!(field(&f, "Track").values(), ["2/2"]);
    assert_eq!(field(&f, "MusicBrainz release").sources[0].key, "----:com.apple.iTunes:MusicBrainz Album Id");
    assert_eq!(field(&f, "ReplayGain track gain").values(), ["-3.30 dB"]);
    assert_eq!(t.pictures.first().map(|p| (p.kind.as_str(), p.front)), Some(("Cover", true)));
    let (t, f) = read(&root.join("Untagged/No Tags.wav"));
    assert!(t.blocks.is_empty() && f.is_empty() && t.pictures.is_empty());
}

#[test]
fn awkward_files() {
    let Some(root) = root() else { return };
    let (t, f) = read(&root.join("Edge Cases/01 Long Comment.flac"));
    assert!(field(&f, "Comment").values()[0].chars().count() > 80_000);
    assert!(f.iter().filter(|x| x.group() == Group::Other).count() >= 3000);
    let p = &t.pictures[0];
    assert!(p.bytes > 2_000_000 && p.size == Some((2500, 2500)), "{} bytes, {:?}", p.bytes, p.size);
    assert!(p.thumb.as_ref().is_some_and(|t| t.width <= meta::THUMB_PX && t.rgba.len() == (t.width * t.height * 4) as usize));
    // Not audio, or not there: an error, never a panic.
    let junk = root.join("junk.flac");
    std::fs::write(&junk, b"not a flac file at all").unwrap();
    assert!(meta::read(&junk, ReadOptions::ALL).is_err());
    assert!(meta::read(&root.join("nothing here.flac"), ReadOptions::ALL).is_err());
}

#[test]
fn reading_changes_nothing() {
    let Some(root) = root() else { return };
    for rel in ["Picard Set/01 Northern Lights.flac", "ID3 Formats/02 Twenty Three.mp3", "Other Formats/02 AAC Atoms.m4a", "Other Formats/01 Opus R128.opus"] {
        let p = root.join(rel);
        let (before, modified) = (std::fs::read(&p).unwrap(), std::fs::metadata(&p).unwrap().modified().unwrap());
        let _ = meta::read(&p, ReadOptions::ALL).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), before, "{rel} unchanged");
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn the_brief_of_an_inconsistent_album() {
    let Some(root) = root() else { return };
    let b = |name: &str| meta::brief(&meta::fields(&meta::read(&root.join("Inconsistent Album").join(name), ReadOptions::TAGS).unwrap()));
    assert_eq!(b("03 Patch Three.flac").album.as_deref(), Some("Patchwork (Deluxe)"));
    assert_eq!(b("06 Patch Six.flac").album_artist, None);
    assert!(!b("06 Patch Six.flac").replaygain);
    assert!(b("07 Patch Seven.mp3").replaygain);
    assert_eq!(b("07 Patch Seven.mp3").mb_album.as_deref(), Some("33333333-4444-4555-8666-777777777777"));
}
