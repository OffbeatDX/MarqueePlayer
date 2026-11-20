//! Technical details of an audio file: its format (codec, as people name it), bit depth,
//! sample rate, bitrate and channels. Read by the scan from lofty's file properties and
//! kept per track (`tracks.codec` and its neighbours, schema 7); the playing track's are
//! refined with what the decoder reports (the app's `metadata/details.rs`).
//!
//! What each format can say (lofty 0.25):
//! - FLAC, ALAC, WAV, AIFF (lossless): bit depth and sample rate from the stream header,
//!   bitrate as the audio data over the duration (the average; FLAC varies with the music).
//! - MP3, MP2: no bit depth (a lossy stream has none). The bitrate is the average from
//!   the Xing/Info header when there is one, else the first frame's (true for CBR). From
//!   the Info header of a CBR file lofty gets the bitrate back only roughly (frames over
//!   whole milliseconds: 321 for a short 320 file), so a value within 1% of a bitrate an
//!   encoder can be set to is read as that bitrate; a VBR average is an average anyway.
//! - Opus: always decodes at 48 kHz; the header's "input sample rate" is only what the
//!   encoder was given, so 48 kHz is stored. Bitrate is the stream's average.
//! - Vorbis: the stream's average bitrate, measured from the file (`tags.rs`): lofty gives
//!   the header's nominal one, which is only the encoder's target. AAC (in MP4): the `esds` average bitrate when
//!   the encoder wrote one, else the media data over the duration. No bit depth for either.
//!
//! All formatting is here (`full`, `compact`, `khz`, `kbps_text`, `album_summary`), so
//! every view says it the same way.

use std::collections::HashMap;

/// The format, as people name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Not known (not read yet, or the file did not say).
    #[default]
    Unknown,
    Flac,
    Alac,
    Wav,
    Aiff,
    Mp3,
    Mp2,
    Aac,
    Opus,
    Vorbis,
}

impl Codec {
    const ALL: [Codec; 9] =
        [Codec::Flac, Codec::Alac, Codec::Wav, Codec::Aiff, Codec::Mp3, Codec::Mp2, Codec::Aac, Codec::Opus, Codec::Vorbis];

    /// "FLAC", "MP3", "Opus"...; "" when unknown. Also what `tracks.codec` stores.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Unknown => "",
            Codec::Flac => "FLAC",
            Codec::Alac => "ALAC",
            Codec::Wav => "WAV",
            Codec::Aiff => "AIFF",
            Codec::Mp3 => "MP3",
            Codec::Mp2 => "MP2",
            Codec::Aac => "AAC",
            Codec::Opus => "Opus",
            Codec::Vorbis => "Vorbis",
        }
    }

    pub fn from_name(s: &str) -> Codec {
        Codec::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(s)).unwrap_or_default()
    }

    /// Lossless: bit depth and sample rate are what describe it.
    pub fn lossless(self) -> bool {
        matches!(self, Codec::Flac | Codec::Alac | Codec::Wav | Codec::Aiff)
    }
}

/// What is known about one file. Every field may be absent; absent is never guessed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AudioInfo {
    pub codec: Codec,
    /// Hz.
    pub sample_rate: Option<u32>,
    /// Lossless formats only.
    pub bit_depth: Option<u8>,
    /// kbps; for variable bitrates, the average.
    pub bitrate: Option<u32>,
    pub channels: Option<u8>,
}

/// MP3 / MP2 bitrates an encoder can be set to (MPEG-1 and 2, layers II and III).
const STANDARD_MPEG_KBPS: [u32; 18] = [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 192, 224, 256, 320];

impl AudioInfo {
    pub fn is_known(&self) -> bool {
        self.codec != Codec::Unknown
    }

    /// From lofty's properties of a file of this codec, by the rules in the module doc.
    pub fn from_properties(codec: Codec, p: &lofty::properties::FileProperties) -> AudioInfo {
        let positive = |v: Option<u32>| v.filter(|&v| v > 0);
        let mut bitrate = positive(p.audio_bitrate()).or_else(|| positive(p.overall_bitrate()));
        if matches!(codec, Codec::Mp3 | Codec::Mp2)
            && let Some(b) = bitrate
            && let Some(&s) = STANDARD_MPEG_KBPS.iter().find(|&&s| b.abs_diff(s) <= (s / 100).max(1))
        {
            bitrate = Some(s);
        }
        let sample_rate = if codec == Codec::Opus { Some(48_000) } else { positive(p.sample_rate()) };
        AudioInfo {
            codec,
            sample_rate,
            bit_depth: if codec.lossless() { p.bit_depth().filter(|&b| b > 0) } else { None },
            bitrate,
            channels: p.channels().filter(|&c| c > 0),
        }
    }
}

// ---- Formatting ----------------------------------------------------------------------

/// A sample rate in kHz, with one decimal only when it has one: 44.1, 48, 88.2, 96, 192.
pub fn khz(hz: u32) -> String {
    let tenths = (f64::from(hz) / 100.0).round() as u64;
    if tenths.is_multiple_of(10) { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) }
}

/// "2,915": thousands separated.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// An average bitrate to whole kbps (half up): 245.5 -> 246.
pub fn round_kbps(kbps: f64) -> u32 {
    if kbps.is_finite() && kbps > 0.0 { (kbps + 0.5).floor() as u32 } else { 0 }
}

/// "2,915 kbps"
pub fn kbps_text(kbps: u32) -> String {
    format!("{} kbps", thousands(u64::from(kbps)))
}

/// "mono", "5.1", "7.1", "3 channels"; `None` for stereo (the usual case says nothing).
pub fn channels_text(channels: u8) -> Option<String> {
    match channels {
        0 | 2 => None,
        1 => Some("mono".into()),
        6 => Some("5.1".into()),
        8 => Some("7.1".into()),
        n => Some(format!("{n} channels")),
    }
}

/// Why a file of this format cannot be played, when the format itself is the reason (not
/// the file): Symphonia's AAC decoder, the only one the app has, decodes AAC-LC in mono or
/// stereo and refuses anything else ("aac too complex"): 5.1 and other multichannel AAC,
/// and HE-AAC (SBR). `engine` is the engine's message, when there is one. `None` when it
/// is not that.
pub fn unplayable_format(info: &AudioInfo, engine: Option<&str>) -> Option<String> {
    let aac = info.codec == Codec::Aac;
    let refused = engine.is_some_and(|m| m.to_ascii_lowercase().contains("too complex"));
    match info.channels {
        Some(n) if aac && n > 2 => Some(format!(
            "AAC with {} channels{}: the app's AAC decoder plays only mono and stereo AAC. The file itself is fine",
            n,
            channels_text(n).filter(|t| !t.ends_with("channels")).map(|t| format!(" ({t})")).unwrap_or_default()
        )),
        _ if aac && refused => Some("this AAC profile (HE-AAC or another beyond AAC-LC): the app's AAC decoder plays only AAC-LC. The file itself is fine".into()),
        _ => None,
    }
}

/// "24-bit / 96 kHz", "16-bit / 44.1 kHz"; the rate alone without a depth.
pub fn depth_rate(info: &AudioInfo) -> Option<String> {
    let rate = info.sample_rate.map(khz);
    match (info.bit_depth, rate) {
        (Some(b), Some(r)) => Some(format!("{b}-bit / {r} kHz")),
        (None, Some(r)) => Some(format!("{r} kHz")),
        (Some(b), None) => Some(format!("{b}-bit")),
        (None, None) => None,
    }
}

/// The full line: "FLAC · 24-bit / 96 kHz · 2,915 kbps", "Opus · 128 kbps",
/// "AAC · 256 kbps · 5.1". Lossy formats leave out the sample rate (the bitrate is what
/// describes them). "" when the format is not known.
pub fn full(info: &AudioInfo) -> String {
    if !info.is_known() {
        return String::new();
    }
    let mut parts: Vec<String> = vec![info.codec.name().to_owned()];
    if info.codec.lossless()
        && let Some(dr) = depth_rate(info)
    {
        parts.push(dr);
    }
    if let Some(b) = info.bitrate {
        parts.push(kbps_text(b));
    }
    if let Some(c) = info.channels.and_then(channels_text) {
        parts.push(c);
    }
    parts.join(" · ")
}

/// The short form for a column: "FLAC 24/96", "MP3 320", "Opus 128". "" when unknown.
pub fn compact(info: &AudioInfo) -> String {
    if !info.is_known() {
        return String::new();
    }
    let name = info.codec.name();
    if info.codec.lossless() {
        match (info.bit_depth, info.sample_rate) {
            (Some(b), Some(r)) => format!("{name} {b}/{}", khz(r)),
            (None, Some(r)) => format!("{name} {}", khz(r)),
            _ => name.to_owned(),
        }
    } else {
        match info.bitrate {
            Some(b) => format!("{name} {b}"),
            None => name.to_owned(),
        }
    }
}

/// `compact`, with the channels when they are not stereo ("AAC 256 · 5.1"): the player bar.
pub fn compact_channels(info: &AudioInfo) -> String {
    let c = compact(info);
    match info.channels.and_then(channels_text) {
        Some(ch) if !c.is_empty() => format!("{c} · {ch}"),
        _ => c,
    }
}

/// What a screen reader says for `full`: "FLAC, 24-bit, 96 kHz, 2,915 kbps".
pub fn spoken(info: &AudioInfo) -> String {
    full(info).replace(" · ", ", ").replace(" / ", ", ")
}

// ---- Albums --------------------------------------------------------------------------

/// What an album's track list does with the details column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// Every track is alike: the summary line says it all.
    Hidden,
    /// Most tracks share this format: only the others show theirs.
    Except(FormatKey),
    /// Show it on every row.
    Shown,
}

/// What makes two tracks "the same format": the codec and channels, and for lossless
/// formats the bit depth and sample rate (a lossy track's bitrate is compared apart).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FormatKey {
    codec: Codec,
    bit_depth: Option<u8>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
}

impl FormatKey {
    pub fn of(a: &AudioInfo) -> FormatKey {
        let lossless = a.codec.lossless();
        FormatKey {
            codec: a.codec,
            bit_depth: if lossless { a.bit_depth } else { None },
            sample_rate: if lossless { a.sample_rate } else { None },
            channels: a.channels,
        }
    }
}

/// An album's details: one line for its info, and what its track list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumSummary {
    /// "FLAC · 16-bit / 44.1 kHz", "Opus · 128 kbps", "Mixed formats", or "" when nothing
    /// is known yet.
    pub line: String,
    pub column: Column,
}

/// Lossy tracks of one codec count as one format while their bitrates stay within this
/// ratio of each other (a VBR album's tracks differ by less); beyond it, the album's
/// tracks were encoded differently and each shows its own.
const BITRATE_SPREAD: f64 = 1.25;

/// The album's summary from its tracks' details and durations (seconds). Tracks not
/// read yet (unknown) are left out.
pub fn album_summary(tracks: &[(AudioInfo, f64)]) -> AlbumSummary {
    let known: Vec<&(AudioInfo, f64)> = tracks.iter().filter(|(a, _)| a.is_known()).collect();
    if known.is_empty() {
        return AlbumSummary { line: String::new(), column: Column::Hidden };
    }
    let mut groups: HashMap<FormatKey, Vec<&(AudioInfo, f64)>> = HashMap::new();
    for t in &known {
        groups.entry(FormatKey::of(&t.0)).or_default().push(t);
    }
    if groups.len() == 1 {
        return uniform(&known);
    }
    // The largest group; ties go to the format that sorts first, so the summary does not
    // change from one look to the next.
    let mut largest: Vec<(&FormatKey, &Vec<&(AudioInfo, f64)>)> = groups.iter().collect();
    largest.sort_by_key(|(k, m)| (std::cmp::Reverse(m.len()), format!("{k:?}")));
    let (key, members) = (*largest[0].0, largest[0].1);
    if members.len() * 2 > known.len() {
        let mostly = uniform(members);
        AlbumSummary { line: format!("Mixed formats, mostly {}", mostly.line), column: Column::Except(key) }
    } else {
        AlbumSummary { line: "Mixed formats".into(), column: Column::Shown }
    }
}

/// Tracks that share a format: the format, and for lossy ones their bitrate (the
/// duration-weighted average when it varies a little; the range, with every row showing
/// its own, when it varies a lot).
fn uniform(tracks: &[&(AudioInfo, f64)]) -> AlbumSummary {
    let first = tracks[0].0;
    if first.codec.lossless() {
        let mut parts = vec![first.codec.name().to_owned()];
        parts.extend(depth_rate(&first));
        parts.extend(first.channels.and_then(channels_text));
        return AlbumSummary { line: parts.join(" · "), column: Column::Hidden };
    }
    let rates: Vec<(u32, f64)> = tracks.iter().filter_map(|(a, d)| a.bitrate.map(|b| (b, d.max(0.0)))).collect();
    let mut parts = vec![first.codec.name().to_owned()];
    let mut column = Column::Hidden;
    if let (Some(min), Some(max)) = (rates.iter().map(|r| r.0).min(), rates.iter().map(|r| r.0).max()) {
        if f64::from(max) > f64::from(min) * BITRATE_SPREAD {
            parts.push(format!("{}–{} kbps", thousands(u64::from(min)), thousands(u64::from(max))));
            column = Column::Shown;
        } else {
            let weight: f64 = rates.iter().map(|r| r.1).sum();
            let avg = if weight > 0.0 {
                rates.iter().map(|r| f64::from(r.0) * r.1).sum::<f64>() / weight
            } else {
                rates.iter().map(|r| f64::from(r.0)).sum::<f64>() / rates.len() as f64
            };
            parts.push(kbps_text(round_kbps(avg)));
        }
    }
    parts.extend(first.channels.and_then(channels_text));
    AlbumSummary { line: parts.join(" · "), column }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lossless(codec: Codec, bits: u8, rate: u32, kbps: u32) -> AudioInfo {
        AudioInfo { codec, sample_rate: Some(rate), bit_depth: Some(bits), bitrate: Some(kbps), channels: Some(2) }
    }

    fn lossy(codec: Codec, kbps: u32) -> AudioInfo {
        AudioInfo { codec, sample_rate: Some(44_100), bit_depth: None, bitrate: Some(kbps), channels: Some(2) }
    }

    #[test]
    fn sample_rates_in_khz() {
        assert_eq!(khz(44_100), "44.1");
        assert_eq!(khz(48_000), "48");
        assert_eq!(khz(88_200), "88.2");
        assert_eq!(khz(96_000), "96");
        assert_eq!(khz(176_400), "176.4");
        assert_eq!(khz(192_000), "192");
        assert_eq!(khz(22_050), "22.1");
        assert_eq!(khz(8_000), "8");
        assert_eq!(khz(11_025), "11");
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(320), "320");
        assert_eq!(thousands(1_411), "1,411");
        assert_eq!(thousands(2_915), "2,915");
        assert_eq!(thousands(12_345), "12,345");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(kbps_text(2_915), "2,915 kbps");
    }

    #[test]
    fn averages_round_half_up() {
        assert_eq!(round_kbps(245.4), 245);
        assert_eq!(round_kbps(245.5), 246);
        assert_eq!(round_kbps(127.96), 128);
        assert_eq!(round_kbps(0.0), 0);
        assert_eq!(round_kbps(f64::NAN), 0);
    }

    #[test]
    fn the_full_line() {
        assert_eq!(full(&lossless(Codec::Flac, 24, 96_000, 2_915)), "FLAC · 24-bit / 96 kHz · 2,915 kbps");
        assert_eq!(full(&lossless(Codec::Alac, 16, 44_100, 850)), "ALAC · 16-bit / 44.1 kHz · 850 kbps");
        assert_eq!(full(&lossy(Codec::Opus, 128)), "Opus · 128 kbps");
        assert_eq!(full(&lossy(Codec::Mp3, 320)), "MP3 · 320 kbps");
        assert_eq!(full(&AudioInfo { channels: Some(6), ..lossy(Codec::Aac, 256) }), "AAC · 256 kbps · 5.1");
        assert_eq!(full(&AudioInfo { channels: Some(1), ..lossless(Codec::Flac, 16, 44_100, 500) }), "FLAC · 16-bit / 44.1 kHz · 500 kbps · mono");
        assert_eq!(full(&AudioInfo { bitrate: None, ..lossy(Codec::Vorbis, 0) }), "Vorbis");
        assert_eq!(full(&AudioInfo::default()), "");
        assert_eq!(spoken(&lossless(Codec::Flac, 24, 96_000, 2_915)), "FLAC, 24-bit, 96 kHz, 2,915 kbps");
    }

    #[test]
    fn the_compact_form() {
        assert_eq!(compact(&lossless(Codec::Flac, 24, 96_000, 2_915)), "FLAC 24/96");
        assert_eq!(compact(&lossless(Codec::Flac, 16, 44_100, 900)), "FLAC 16/44.1");
        assert_eq!(compact(&lossy(Codec::Mp3, 320)), "MP3 320");
        assert_eq!(compact(&lossy(Codec::Opus, 128)), "Opus 128");
        assert_eq!(compact(&AudioInfo { bit_depth: None, ..lossless(Codec::Wav, 0, 48_000, 0) }), "WAV 48");
        assert_eq!(compact(&AudioInfo::default()), "");
        assert_eq!(compact_channels(&AudioInfo { channels: Some(6), ..lossy(Codec::Aac, 384) }), "AAC 384 · 5.1");
        assert_eq!(compact_channels(&lossy(Codec::Aac, 256)), "AAC 256");
    }

    #[test]
    fn codec_names_round_trip() {
        for c in Codec::ALL {
            assert_eq!(Codec::from_name(c.name()), c);
        }
        assert_eq!(Codec::from_name(""), Codec::Unknown);
        assert_eq!(Codec::from_name("wavpack"), Codec::Unknown);
        assert_eq!(Codec::from_name("flac"), Codec::Flac);
    }

    #[test]
    fn mpeg_bitrates_near_a_standard_one_are_that_one() {
        use lofty::properties::FileProperties;
        use std::time::Duration;
        let p = |kbps| FileProperties::new(Duration::from_secs(10), Some(kbps), Some(kbps), Some(44_100), None, Some(2), None);
        assert_eq!(AudioInfo::from_properties(Codec::Mp3, &p(319)).bitrate, Some(320));
        assert_eq!(AudioInfo::from_properties(Codec::Mp3, &p(321)).bitrate, Some(320));
        assert_eq!(AudioInfo::from_properties(Codec::Mp3, &p(316)).bitrate, Some(316), "beyond 1%: as read");
        assert_eq!(AudioInfo::from_properties(Codec::Mp3, &p(129)).bitrate, Some(128));
        assert_eq!(AudioInfo::from_properties(Codec::Mp2, &p(255)).bitrate, Some(256));
        assert_eq!(AudioInfo::from_properties(Codec::Mp3, &p(245)).bitrate, Some(245), "a VBR average stays");
        assert_eq!(AudioInfo::from_properties(Codec::Opus, &p(127)).bitrate, Some(127), "only MPEG");
        assert_eq!(AudioInfo::from_properties(Codec::Opus, &p(127)).sample_rate, Some(48_000), "Opus decodes at 48 kHz");
        let with_depth = FileProperties::new(Duration::from_secs(10), Some(256), Some(256), Some(44_100), Some(16), Some(2), None);
        assert_eq!(AudioInfo::from_properties(Codec::Aac, &with_depth).bit_depth, None, "lossy: no bit depth");
        assert_eq!(AudioInfo::from_properties(Codec::Alac, &with_depth).bit_depth, Some(16));
    }

    #[test]
    fn a_uniform_album_has_one_line_and_no_column() {
        let t = |a| (a, 200.0);
        let s = album_summary(&[t(lossless(Codec::Flac, 16, 44_100, 900)), t(lossless(Codec::Flac, 16, 44_100, 1_000))]);
        assert_eq!(s, AlbumSummary { line: "FLAC · 16-bit / 44.1 kHz".into(), column: Column::Hidden });
        // A VBR album: one codec, bitrates close: their weighted average.
        let s = album_summary(&[(lossy(Codec::Mp3, 240), 100.0), (lossy(Codec::Mp3, 260), 300.0)]);
        assert_eq!(s, AlbumSummary { line: "MP3 · 255 kbps".into(), column: Column::Hidden });
        let s = album_summary(&[(lossy(Codec::Opus, 128), 100.0), (lossy(Codec::Opus, 128), 100.0)]);
        assert_eq!(s.line, "Opus · 128 kbps");
        // A track not read yet does not make it mixed.
        let s = album_summary(&[t(lossless(Codec::Flac, 24, 96_000, 2_900)), t(AudioInfo::default())]);
        assert_eq!(s.line, "FLAC · 24-bit / 96 kHz");
        assert_eq!(album_summary(&[t(AudioInfo::default())]), AlbumSummary { line: String::new(), column: Column::Hidden });
        let surround = AudioInfo { channels: Some(6), ..lossless(Codec::Flac, 16, 48_000, 2_000) };
        assert_eq!(album_summary(&[t(surround)]).line, "FLAC · 16-bit / 48 kHz · 5.1");
    }

    #[test]
    fn mixed_albums_say_so() {
        let t = |a| (a, 200.0);
        let flac = lossless(Codec::Flac, 16, 44_100, 900);
        // No format has most of the tracks: every row shows its own.
        let s = album_summary(&[t(flac), t(lossless(Codec::Flac, 24, 96_000, 2_900)), t(lossy(Codec::Mp3, 320)), t(lossy(Codec::Opus, 128))]);
        assert_eq!(s, AlbumSummary { line: "Mixed formats".into(), column: Column::Shown });
        // Most are one format: only the others show theirs.
        let s = album_summary(&[t(flac), t(flac), t(flac), t(lossy(Codec::Mp3, 192))]);
        assert_eq!(s.line, "Mixed formats, mostly FLAC · 16-bit / 44.1 kHz");
        assert_eq!(s.column, Column::Except(FormatKey::of(&flac)));
        // Half is not most.
        let s = album_summary(&[t(flac), t(lossy(Codec::Mp3, 192))]);
        assert_eq!(s.column, Column::Shown);
        // One codec at very different bitrates: the range, and each row its own.
        let s = album_summary(&[t(lossy(Codec::Mp3, 128)), t(lossy(Codec::Mp3, 320))]);
        assert_eq!(s, AlbumSummary { line: "MP3 · 128–320 kbps".into(), column: Column::Shown });
    }
}
