//! Reading the tags the library indexes. Read-only: nothing here writes to files.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::audio::{AudioInfo, Codec};
use crate::names::{self, Credit};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackTags {
    /// TITLE as tagged; without one, what the file's name says (see `from_name`).
    pub title: Option<String>,
    /// The artist credit as shown: the ARTIST value as written ("A feat. B"), or its values
    /// joined with "; " when the file holds several.
    pub artist: Option<String>,
    /// Each artist the track credits, in order (`names::credits`; ARTISTS when tagged, with
    /// MusicBrainz artist ids when they line up), and featured artists from the title.
    pub artists: Vec<Credit>,
    pub album: Option<String>,
    /// ALBUMARTIST as shown (several values joined with "; "), `None` when not tagged.
    pub album_artist: Option<String>,
    /// MUSICBRAINZ_ALBUMARTISTID values, when they line up with the album artist's names.
    pub album_artist_mbids: Vec<String>,
    pub album_artist_sort: Option<String>,
    /// ARTISTSORT.
    pub artist_sort: Option<String>,
    /// The track number: the tag's, else the file name's (`from_name`).
    pub track_no: Option<u32>,
    /// The disc number: the tag's, else the file name's ("1-03 Title").
    pub disc_no: Option<u32>,
    /// The track number tag as written when it is not a number ("A1", a vinyl side).
    pub track_raw: Option<String>,
    /// DATE, falling back to RELEASEDATE / ORIGINALDATE / YEAR.
    pub date: Option<String>,
    /// ORIGINALDATE (TDOR, ORIGINALYEAR): when the music first came out, for a reissue.
    pub original_date: Option<String>,
    /// RELEASEDATE (TDRL): this edition's release.
    pub release_date: Option<String>,
    /// GENRE as shown (several values joined with "; ").
    pub genre: Option<String>,
    /// Each genre (`names::split_genres` of each value).
    pub genres: Vec<String>,
    pub label: Option<String>,
    pub release_type: Option<String>,
    pub mb_album_id: Option<String>,
    /// MUSICBRAINZ_TRACKID (Picard's recording id): what a server's copy of the track is
    /// matched by first (`remote`).
    pub mb_recording_id: Option<String>,
    /// The compilation flag (TCMP, cpil, COMPILATION) is set.
    pub compilation: bool,
    /// What came from the file's name for want of a tag.
    pub from_name: NameUse,
    pub duration: f64,
    /// Format, bit depth, sample rate, bitrate, channels (`audio.rs`).
    pub audio: AudioInfo,
}

/// Which values the scanner took from a file's name (`names::from_file_name`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NameUse {
    pub title: bool,
    pub track: bool,
    pub disc: bool,
}

impl NameUse {
    /// As stored (`tracks.from_name`): 1 the title, 2 the track number, 4 the disc number.
    pub fn bits(self) -> i64 {
        i64::from(self.title) | (i64::from(self.track) << 1) | (i64::from(self.disc) << 2)
    }
    pub fn from_bits(b: i64) -> Self {
        NameUse { title: b & 1 != 0, track: b & 2 != 0, disc: b & 4 != 0 }
    }
}

/// Open a file's tags as its extension says it is, else (a FLAC named .mp3, a file renamed by
/// hand) as its content says: the player plays such files by their content too.
fn open(path: &Path, options: ParseOptions) -> Result<(TaggedFile, Codec), String> {
    let probe = Probe::open(path).map_err(|e| e.to_string())?.options(options);
    let named = probe.file_type();
    let first = match open_probe(probe, options) {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    let guessed = Probe::open(path).map_err(|e| e.to_string())?.guess_file_type().map_err(|e| e.to_string())?;
    match guessed.file_type() {
        Some(t) if Some(t) != named => {
            log::info!("{}: read as {t:?}, what its content is, not what its name says", path.display());
            open_probe(guessed.options(options), options).map_err(|_| first)
        }
        _ => Err(first),
    }
}

/// Open and parse a file, and say which codec it holds. MP4 and MPEG files are read
/// through their own types, whose properties tell AAC from ALAC and MP3 from MP2 (the
/// generic properties do not); the result is the same `TaggedFile` either way.
fn open_probe(probe: Probe<std::io::BufReader<std::fs::File>>, options: ParseOptions) -> Result<(TaggedFile, Codec), String> {
    let e = |e: lofty::error::FileParseError| e.to_string();
    match probe.file_type() {
        Some(FileType::Mp4) => {
            let mut reader = probe.into_inner();
            let mp4 = lofty::mp4::Mp4File::read_from(&mut reader, options).map_err(e)?;
            let codec = match mp4.properties().codec() {
                Some(lofty::mp4::Mp4Codec::AAC) => Codec::Aac,
                Some(lofty::mp4::Mp4Codec::ALAC) => Codec::Alac,
                Some(lofty::mp4::Mp4Codec::MP3) => Codec::Mp3,
                Some(lofty::mp4::Mp4Codec::FLAC) => Codec::Flac,
                _ => Codec::Unknown,
            };
            Ok((mp4.into(), codec))
        }
        Some(FileType::Mpeg) => {
            let mut reader = probe.into_inner();
            let mpeg = lofty::mpeg::MpegFile::read_from(&mut reader, options).map_err(e)?;
            let codec = match mpeg.properties().layer() {
                lofty::mpeg::Layer::Layer3 => Codec::Mp3,
                lofty::mpeg::Layer::Layer2 => Codec::Mp2,
                _ => Codec::Unknown,
            };
            Ok((mpeg.into(), codec))
        }
        ft => {
            let codec = match ft {
                Some(FileType::Flac) => Codec::Flac,
                Some(FileType::Opus) => Codec::Opus,
                Some(FileType::Vorbis) => Codec::Vorbis,
                Some(FileType::Wav) => Codec::Wav,
                Some(FileType::Aiff) => Codec::Aiff,
                Some(FileType::Aac) => Codec::Aac,
                _ => Codec::Unknown,
            };
            Ok((probe.read().map_err(e)?, codec))
        }
    }
}

/// The details of a parsed file. Vorbis is measured here: lofty gives the header's
/// nominal bitrate (the encoder's target, often far from what the file has), and its
/// overall bitrate counts the tags, cover art included.
fn audio_of(path: &Path, file: &TaggedFile, codec: Codec) -> AudioInfo {
    let mut a = AudioInfo::from_properties(codec, file.properties());
    if codec == Codec::Vorbis {
        let secs = file.properties().duration().as_secs_f64();
        a.bitrate = vorbis_average_kbps(path, secs).or(a.bitrate);
    }
    a
}

/// A Vorbis stream's average bitrate: the audio pages over the duration. Vorbis audio
/// starts on a fresh page after the three header packets (Vorbis I, section 4.2.1), so
/// only the first few page headers are read (bodies are skipped: the comment packet may
/// hold a large picture).
fn vorbis_average_kbps(path: &Path, secs: f64) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    if secs <= 0.0 {
        return None;
    }
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let len = f.get_ref().metadata().ok()?.len();
    let mut packets = 0;
    let mut pos = 0u64;
    // A header holding a picture spans many pages; give up well past any real one.
    for _ in 0..4096 {
        let mut head = [0u8; 27];
        f.read_exact(&mut head).ok()?;
        if &head[..4] != b"OggS" {
            return None;
        }
        let mut lacing = vec![0u8; usize::from(head[26])];
        f.read_exact(&mut lacing).ok()?;
        let body: u64 = lacing.iter().map(|&l| u64::from(l)).sum();
        packets += lacing.iter().filter(|&&l| l < 255).count();
        pos += 27 + lacing.len() as u64 + body;
        if packets >= 3 {
            let audio = len.checked_sub(pos)?;
            return Some(crate::audio::round_kbps(audio as f64 * 8.0 / secs / 1000.0)).filter(|&k| k > 0);
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
    }
    None
}

/// Only the file's technical details, tags not parsed (the scan's fill of tracks read
/// before schema 7, `scan::fill_details`).
pub fn read_audio(path: &Path) -> Result<AudioInfo, String> {
    crate::remote::refuse(path)?;
    let options = ParseOptions::new().read_tags(false).read_cover_art(false);
    let (file, codec) = open(path, options)?;
    Ok(audio_of(path, &file, codec))
}

/// Everything the Properties panel shows that the library does not keep: the details
/// as the file says them now, and the loudness tags present (as tagged).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileReport {
    pub audio: AudioInfo,
    pub duration: f64,
    /// ("Track gain", "-6.20 dB"), ... in a fixed order; only those present.
    pub replaygain: Vec<(&'static str, String)>,
}

pub fn file_report(path: &Path) -> Result<FileReport, String> {
    crate::remote::refuse(path)?;
    let options = ParseOptions::new().read_cover_art(false);
    let (file, codec) = open(path, options)?;
    let mut report = FileReport {
        audio: audio_of(path, &file, codec),
        duration: file.properties().duration().as_secs_f64(),
        replaygain: Vec::new(),
    };
    let keys: [(&'static str, ItemKey); 4] = [
        ("Track gain", ItemKey::ReplayGainTrackGain),
        ("Track peak", ItemKey::ReplayGainTrackPeak),
        ("Album gain", ItemKey::ReplayGainAlbumGain),
        ("Album peak", ItemKey::ReplayGainAlbumPeak),
    ];
    let r128: [(&'static str, &'static str); 2] = [("R128 track gain", "R128_TRACK_GAIN"), ("R128 album gain", "R128_ALBUM_GAIN")];
    for tag in file.tags() {
        for (label, key) in &keys {
            if let Some(v) = get(tag, *key)
                && !report.replaygain.iter().any(|(l, _)| l == label)
            {
                report.replaygain.push((label, v));
            }
        }
    }
    // R128 gains (Opus) are not among lofty's generic keys: from the Vorbis comments.
    if codec == Codec::Opus
        && let Ok(mut f) = std::fs::File::open(path)
        && let Ok(opus) = lofty::ogg::OpusFile::read_from(&mut f, options)
    {
        for (label, raw) in r128 {
            if let Some(v) = opus.vorbis_comments().get(raw).map(str::trim).filter(|v| !v.is_empty()) {
                report.replaygain.push((label, v.to_owned()));
            }
        }
    }
    let order = ["Track gain", "Track peak", "Album gain", "Album peak", "R128 track gain", "R128 album gain"];
    report.replaygain.sort_by_key(|(l, _)| order.iter().position(|o| o == l));
    Ok(report)
}

fn get(tag: &Tag, key: ItemKey) -> Option<String> {
    tag.get_string(key).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn clean(s: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Dates as ISO text, so they sort as text: "2026.07.12", "2025/8/17" and "20250801"
/// become "2026-07-12", "2025-08-17", "2025-08-01"; a timestamp keeps its date. Anything
/// else is left as tagged.
pub fn normalize_date(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| s.get(r.clone()).is_some_and(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()));
    if b.len() == 8 && digits(0..8) {
        return format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]);
    }
    if b.len() < 4 || !digits(0..4) {
        return s.to_owned();
    }
    if b.len() == 4 {
        return s.to_owned();
    }
    let sep = b[4] as char;
    if !matches!(sep, '-' | '.' | '/') {
        return s.to_owned();
    }
    let rest = &s[5..];
    let mut parts = rest.splitn(2, sep);
    let month = parts.next().unwrap_or("");
    let day_part = parts.next();
    let num = |x: &str| (!x.is_empty() && x.len() <= 2 && x.bytes().all(|c| c.is_ascii_digit())).then(|| x.parse::<u32>().ok()).flatten();
    let Some(m) = num(month).filter(|m| (1..=12).contains(m)) else { return s.to_owned() };
    match day_part {
        None => format!("{}-{m:02}", &s[0..4]),
        Some(d) => {
            // "2026-07-12T10:00:00" or "2026-07-12 extra": the date part.
            let d = d.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
            match num(d).filter(|d| (1..=31).contains(d)) {
                Some(d) => format!("{}-{m:02}-{d:02}", &s[0..4]),
                None => format!("{}-{m:02}", &s[0..4]),
            }
        }
    }
}

/// The album's three dates from what is tagged, as ISO text (`normalize_date`): `date` is
/// DATE (`recorded`), falling back to RELEASEDATE, ORIGINALDATE and YEAR. Taggers
/// long wrote an album's release date to DATE and left RELEASEDATE empty; with an ORIGINALDATE
/// at or before it, DATE is this edition's release (Navidrome's mapDates,
/// model/metadata/map_mediafile.go). A server's dates (`remote`) are mapped the same way.
pub fn map_dates(
    recorded: Option<String>,
    original: Option<String>,
    release: Option<String>,
    year: Option<String>,
) -> (Option<String>, Option<String>, Option<String>) {
    let clean = |d: Option<String>| d.map(|d| normalize_date(&d)).filter(|d| !d.is_empty());
    let (recorded, original, mut release) = (clean(recorded), clean(original), clean(release));
    let date = recorded.or_else(|| release.clone()).or_else(|| original.clone()).or_else(|| clean(year));
    if release.is_none()
        && let (Some(orig), Some(date)) = (&original, &date)
        && date >= orig
    {
        release = Some(date.clone());
    }
    (date, original, release)
}

/// A track's genres from its GENRE values: as shown (several joined with "; ") and each genre
/// (`names::split_genres` of each value, repeats by `norm` dropped).
pub fn genres_of(values: &[String]) -> (Option<String>, Vec<String>) {
    let mut genres: Vec<String> = Vec::new();
    for v in values {
        for g in names::split_genres(v) {
            if !genres.iter().any(|x| names::norm(x) == names::norm(g)) {
                genres.push(g.to_owned());
            }
        }
    }
    (shown(values), genres)
}

/// The MusicBrainz recording id (Picard's MUSICBRAINZ_TRACKID), read when a play is
/// scrobbled (the scan keeps it too: `TrackTags::mb_recording_id`).
pub fn musicbrainz_track_id(path: &Path) -> Option<String> {
    crate::remote::refuse(path).ok()?;
    let options = ParseOptions::new().read_cover_art(false);
    let file = Probe::open(path).and_then(|p| p.options(options).read()).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    get(tag, ItemKey::MusicBrainzRecordingId)
}

/// A track's MusicBrainz ids as tagged (Picard's names), each as written: what ListenBrainz
/// takes with a listen. Empty where the file has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MusicBrainzIds {
    /// MUSICBRAINZ_TRACKID.
    pub recording: Option<String>,
    /// MUSICBRAINZ_RELEASETRACKID.
    pub release_track: Option<String>,
    /// MUSICBRAINZ_ALBUMID.
    pub release: Option<String>,
    /// MUSICBRAINZ_RELEASEGROUPID.
    pub release_group: Option<String>,
    /// MUSICBRAINZ_ARTISTID, each value.
    pub artists: Vec<String>,
}

/// The file's MusicBrainz ids, read when a listen is sent (nothing for a track only the server
/// has: `remote::musicbrainz_ids`).
pub fn musicbrainz_ids(path: &Path) -> MusicBrainzIds {
    let read = || -> Option<MusicBrainzIds> {
        crate::remote::refuse(path).ok()?;
        let options = ParseOptions::new().read_cover_art(false);
        let file = Probe::open(path).and_then(|p| p.options(options).read()).ok()?;
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        Some(MusicBrainzIds {
            recording: get(tag, ItemKey::MusicBrainzRecordingId),
            release_track: get(tag, ItemKey::MusicBrainzTrackId),
            release: get(tag, ItemKey::MusicBrainzReleaseId),
            release_group: get(tag, ItemKey::MusicBrainzReleaseGroupId),
            artists: all(tag, ItemKey::MusicBrainzArtistId),
        })
    };
    read().unwrap_or_default()
}

/// Every value of an item, tidied (`names::tidy`), empties and repeats dropped. lofty
/// gives each value of a multi-valued item as its own item (Vorbis comments and APE repeat
/// the key, ID3v2.4 separates values with NUL, MP4 holds several data items); a NUL left
/// inside a value separates values too.
fn all(tag: &Tag, key: ItemKey) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in tag.get_strings(key).flat_map(|v| v.split('\0')) {
        let v = names::tidy(v);
        if !v.is_empty() && !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// A value as shown: the one value as written, or several joined with "; ".
fn shown(values: &[String]) -> Option<String> {
    if values.is_empty() { None } else { Some(values.join("; ")) }
}

/// The track's artists. ARTISTS (Picard's list of each artist) when tagged, else each
/// ARTIST value taken apart (`names::credits`). Featured artists are marked from the
/// credit as written. MusicBrainz artist ids are kept only when there are exactly as many
/// as names (Picard writes them in the same order). Then featured artists named in the
/// title.
fn track_credits(tag: &Tag, artist_values: &[String], title: Option<&str>) -> Vec<Credit> {
    credits_of(artist_values, &all(tag, ItemKey::TrackArtists), all(tag, ItemKey::MusicBrainzArtistId), title)
}

/// `track_credits` from the values themselves: the ARTIST values, the ARTISTS list, the
/// MusicBrainz artist ids and the title. A server's track (`remote`) is credited by the same
/// rules, from what the server lists.
pub fn credits_of(artist_values: &[String], listed: &[String], ids: Vec<String>, title: Option<&str>) -> Vec<Credit> {
    let from_credit: Vec<Credit> = artist_values.iter().flat_map(|v| names::credits(v)).collect();
    let mut out: Vec<Credit> = Vec::new();
    if listed.is_empty() {
        for c in from_credit {
            names::push_credit(&mut out, c);
        }
    } else {
        let featured: Vec<String> = from_credit.iter().filter(|c| c.featured).map(|c| names::norm(&c.name)).collect();
        for v in listed {
            for n in names::split_artists(v) {
                names::push_credit(&mut out, Credit::new(n, featured.contains(&names::norm(n))));
            }
        }
    }
    if !ids.is_empty() && ids.len() == out.len() {
        for (c, id) in out.iter_mut().zip(ids) {
            c.mbid = Some(id.to_ascii_lowercase());
        }
    }
    if let Some(t) = title {
        for n in names::title_featured(t) {
            names::push_credit(&mut out, Credit::new(&n, true));
        }
    }
    out
}

/// A flag item (TCMP, cpil, COMPILATION) that is set.
fn flag(tag: &Tag, key: ItemKey) -> bool {
    tag.get_string(key).is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// Fill what the tags lack from the file's name: the title, and the track (and disc)
/// number ("01 - Title", "1-03 Title"). Says what it used in `from_name`.
pub fn apply_file_name(t: &mut TrackTags, path: &Path) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let from = names::from_file_name(&stem);
    if t.title.is_none() {
        t.title = Some(from.title.clone()).filter(|s| !s.is_empty());
        t.from_name.title = t.title.is_some();
    }
    // A number from the name only when the file has no track number tag at all ("A1" is
    // there, even if it is not a number).
    if t.track_no.is_none() && t.track_raw.is_none() && from.track.is_some() {
        t.track_no = from.track;
        t.from_name.track = true;
        if t.disc_no.is_none() && from.disc.is_some() {
            t.disc_no = from.disc;
            t.from_name.disc = true;
        }
    }
}

/// Read tags, and the embedded picture (front cover preferred) when `want_art`.
///
/// Without `want_art`, lofty is told not to parse pictures at all (`read_cover_art`):
/// with its defaults it copies every embedded image out of the file, and base64-decodes
/// it for Ogg/Opus, only for the scan to drop it. Everything else (properties, every
/// tag field) is read the same either way. A missing title or track number is taken from
/// the file's name (`apply_file_name`).
/// A panic in the tag reader on a damaged file is that file's error (`guard`), whoever reads it.
pub fn read(path: &Path, want_art: bool) -> Result<(TrackTags, Option<Vec<u8>>), String> {
    crate::remote::refuse(path)?;
    crate::guard::catching(|| read_unguarded(path, want_art)).unwrap_or_else(|what| Err(format!("panicked: {what}")))
}

fn read_unguarded(path: &Path, want_art: bool) -> Result<(TrackTags, Option<Vec<u8>>), String> {
    let options = ParseOptions::new().read_cover_art(want_art);
    let (file, codec) = open(path, options)?;
    let duration = file.properties().duration().as_secs_f64();
    let mut t = TrackTags { duration, audio: audio_of(path, &file, codec), ..Default::default() };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        apply_file_name(&mut t, path);
        return Ok((t, None));
    };
    t.title = clean(tag.title()).map(|s| names::tidy(&s));
    let artist_values = all(tag, ItemKey::TrackArtist);
    t.artist = shown(&artist_values);
    t.artists = track_credits(tag, &artist_values, t.title.as_deref());
    t.album = clean(tag.album()).map(|s| names::tidy(&s));
    let album_artists = all(tag, ItemKey::AlbumArtist);
    t.album_artist = shown(&album_artists);
    let aa_ids = all(tag, ItemKey::MusicBrainzReleaseArtistId);
    let aa_names = album_artists.iter().flat_map(|v| names::split_artists(v)).count();
    if !aa_ids.is_empty() && aa_ids.len() == aa_names {
        t.album_artist_mbids = aa_ids.into_iter().map(|s| s.to_ascii_lowercase()).collect();
    }
    t.album_artist_sort = get(tag, ItemKey::AlbumArtistSortOrder);
    t.artist_sort = get(tag, ItemKey::TrackArtistSortOrder);
    let track = get(tag, ItemKey::TrackNumber);
    t.track_no = track.as_deref().and_then(names::leading_number);
    t.track_raw = track.filter(|_| t.track_no.is_none());
    t.disc_no = get(tag, ItemKey::DiscNumber).as_deref().and_then(names::leading_number);
    (t.date, t.original_date, t.release_date) =
        map_dates(get(tag, ItemKey::RecordingDate), get(tag, ItemKey::OriginalReleaseDate), get(tag, ItemKey::ReleaseDate), get(tag, ItemKey::Year));
    let genre_values = all(tag, ItemKey::Genre);
    (t.genre, t.genres) = genres_of(&genre_values);
    t.label = get(tag, ItemKey::Label).or_else(|| get(tag, ItemKey::Publisher)).map(|s| names::tidy(&s));
    t.release_type = get(tag, ItemKey::MusicBrainzReleaseType);
    t.mb_album_id = get(tag, ItemKey::MusicBrainzReleaseId);
    t.mb_recording_id = get(tag, ItemKey::MusicBrainzRecordingId).map(|s| s.to_ascii_lowercase());
    t.compilation = flag(tag, ItemKey::FlagCompilation);
    apply_file_name(&mut t, path);

    let art = if want_art {
        let pics = tag.pictures();
        pics.iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pics.first())
            .map(|p| p.data().to_vec())
    } else {
        None
    };
    Ok((t, art))
}

#[cfg(test)]
mod tests {
    use super::normalize_date;

    #[test]
    fn dates_become_iso() {
        assert_eq!(normalize_date("2026.07.12"), "2026-07-12");
        assert_eq!(normalize_date("2025/8/17"), "2025-08-17");
        assert_eq!(normalize_date("20250801"), "2025-08-01");
        assert_eq!(normalize_date("2024"), "2024");
        assert_eq!(normalize_date("2024-10"), "2024-10");
        assert_eq!(normalize_date("2024-10-27T09:00:00Z"), "2024-10-27");
        assert_eq!(normalize_date("Summer 1999"), "Summer 1999");
    }
}
