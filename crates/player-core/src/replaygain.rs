//! Loudness normalisation from tags.
//!
//! Two tag families, two references:
//!
//! - Opus: `R128_TRACK_GAIN` / `R128_ALBUM_GAIN`, Q7.8 fixed-point dB *relative to the
//!   OpusHead output gain*, normalising to -23 LUFS (RFC 7845 section 5.2.1). The header
//!   gain is mandatory and applies even with normalisation off.
//! - Everything else: `REPLAYGAIN_TRACK_GAIN` / `REPLAYGAIN_ALBUM_GAIN` ("-6.20 dB"),
//!   normalising to the ReplayGain 2.0 reference of -18 LUFS.
//!
//! R128 targets are 5 dB quieter than ReplayGain targets, so R128 gains get +5 dB to
//! make an Opus album and a FLAC album land at the same loudness.
//!
//! Peaks (`REPLAYGAIN_*_PEAK`, linear full-scale) cap the gain from the same source
//! (track peak with track gain, album peak with album gain) when clipping protection is on.
//! When the matching peak is absent, the other peak caps it; with no peak at all (as on Opus
//! files), the gain is not capped.
//! Files with no gain tags for the chosen mode play unadjusted (header gain only).

/// dB difference between the ReplayGain (-18 LUFS) and R128 (-23 LUFS) references.
pub const R128_TO_REPLAYGAIN_DB: f32 = 5.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainMode {
    Off,
    #[default]
    Track,
    Album,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainConfig {
    pub mode: GainMode,
    /// Added to the tag gain when a tag gain is used.
    pub preamp_db: f32,
    pub prevent_clipping: bool,
    /// Limit samples that would go above full scale (`limiter.rs`). Applies with
    /// ReplayGain off too.
    pub limit_peaks: bool,
}

impl Default for GainConfig {
    fn default() -> Self {
        Self { mode: GainMode::Track, preamp_db: 0.0, prevent_clipping: true, limit_peaks: true }
    }
}

/// Gain-related tags of one file, as raw tag values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GainTags {
    /// `Some` for Opus files: the OpusHead output gain in dB.
    pub opus_header_db: Option<f32>,
    pub r128_track: Option<String>,
    pub r128_album: Option<String>,
    pub rg_track_gain: Option<String>,
    pub rg_album_gain: Option<String>,
    pub rg_track_peak: Option<String>,
    pub rg_album_peak: Option<String>,
}

impl GainTags {
    /// Offer one tag (any case). Unknown keys are ignored.
    pub fn offer(&mut self, key: &str, value: &str) {
        let slot = match key.to_ascii_uppercase().as_str() {
            "R128_TRACK_GAIN" => &mut self.r128_track,
            "R128_ALBUM_GAIN" => &mut self.r128_album,
            "REPLAYGAIN_TRACK_GAIN" => &mut self.rg_track_gain,
            "REPLAYGAIN_ALBUM_GAIN" => &mut self.rg_album_gain,
            "REPLAYGAIN_TRACK_PEAK" => &mut self.rg_track_peak,
            "REPLAYGAIN_ALBUM_PEAK" => &mut self.rg_album_peak,
            _ => return,
        };
        if slot.is_none() {
            *slot = Some(value.trim().to_owned());
        }
    }
}

/// The tag gains real material measures, and some: from a track at +46 LUFS to one at
/// -42 LUFS (against the -18 LUFS reference). A value outside is a damaged or hostile tag,
/// not a measurement, and is taken as no tag (an infinite gain made the output NaN, a huge
/// one full-scale noise).
const PLAUSIBLE_DB: std::ops::RangeInclusive<f32> = -64.0..=24.0;

/// "-6.20 dB", "+1.5dB", "-3" -> dB.
fn parse_rg_db(s: &str) -> Option<f32> {
    let t = s.trim();
    let t = t.strip_suffix("dB").or_else(|| t.strip_suffix("db")).or_else(|| t.strip_suffix("DB")).unwrap_or(t);
    t.trim().parse::<f32>().ok().filter(|v| PLAUSIBLE_DB.contains(v))
}

/// Q7.8 integer string -> dB (against R128's reference: `PLAUSIBLE_DB` once moved to
/// ReplayGain's).
fn parse_r128_db(s: &str) -> Option<f32> {
    s.trim().parse::<i32>().ok().map(|q| q as f32 / 256.0).filter(|v| PLAUSIBLE_DB.contains(&(v + R128_TO_REPLAYGAIN_DB)))
}

fn parse_peak(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|p| p.is_finite() && *p > 0.0)
}

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The linear gain to apply to decoded samples for this file under `cfg`.
pub fn linear_gain(tags: &GainTags, cfg: &GainConfig) -> f32 {
    db_to_linear(gain_db(tags, cfg))
}

/// The total gain in dB (header gain + normalisation), after clipping protection.
pub fn gain_db(tags: &GainTags, cfg: &GainConfig) -> f32 {
    let header = tags.opus_header_db.unwrap_or(0.0);
    header + applied(tags, cfg).map_or(0.0, |a| a.db)
}

/// Which tag a ReplayGain adjustment came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainSource {
    Track,
    Album,
}

/// The loudness adjustment a file gets under a config: its tag gain plus the preamp,
/// after clipping protection, without the Opus header gain (part of the file, always
/// applied). `None` with ReplayGain off or no gain tags (the file plays unadjusted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied {
    pub db: f32,
    /// The tag used: the mode's own, or the other one when only that is tagged.
    pub source: GainSource,
    /// Clipping protection lowered it to the peak.
    pub capped: bool,
}

pub fn applied(tags: &GainTags, cfg: &GainConfig) -> Option<Applied> {
    if cfg.mode == GainMode::Off {
        return None;
    }
    let header = tags.opus_header_db.unwrap_or(0.0);
    let album = cfg.mode == GainMode::Album;
    // Preferred value first, the other one as fallback; with which one it was.
    let pick = |track: &Option<String>, album_v: &Option<String>, parse: fn(&str) -> Option<f32>| {
        let (a, b) = if album { (album_v, track) } else { (track, album_v) };
        let (first, second) = if album { (GainSource::Album, GainSource::Track) } else { (GainSource::Track, GainSource::Album) };
        a.as_deref().and_then(parse).map(|v| (v, first)).or_else(|| b.as_deref().and_then(parse).map(|v| (v, second)))
    };

    // Opus files: R128 first. An Opus file tagged only with REPLAYGAIN_* (some taggers do
    // this) is treated like any other file, on top of the header gain.
    let normalisation = if tags.opus_header_db.is_some() {
        pick(&tags.r128_track, &tags.r128_album, parse_r128_db)
            .map(|(g, s)| (g + R128_TO_REPLAYGAIN_DB, s))
            .or_else(|| pick(&tags.rg_track_gain, &tags.rg_album_gain, parse_rg_db))
    } else {
        pick(&tags.rg_track_gain, &tags.rg_album_gain, parse_rg_db)
    };
    let (norm, source) = normalisation?;
    let mut total = header + norm + cfg.preamp_db;
    let mut capped = false;
    // The peak of the gain's own source; without one, the other peak still protects.
    let (own, other) = match source {
        GainSource::Track => (&tags.rg_track_peak, &tags.rg_album_peak),
        GainSource::Album => (&tags.rg_album_peak, &tags.rg_track_peak),
    };
    if cfg.prevent_clipping
        && let Some(peak) = own.as_deref().and_then(parse_peak).or_else(|| other.as_deref().and_then(parse_peak))
    {
        // Largest gain that keeps the peak at or below full scale.
        let max_db = -20.0 * peak.log10();
        if max_db < total {
            total = max_db;
            capped = true;
        }
    }
    Some(Applied { db: total - header, source, capped })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus(track: &str, album: &str) -> GainTags {
        GainTags {
            opus_header_db: Some(0.0),
            r128_track: Some(track.into()),
            r128_album: Some(album.into()),
            ..Default::default()
        }
    }

    fn flac(track: &str, album: &str, peak: Option<&str>) -> GainTags {
        GainTags {
            rg_track_gain: Some(track.into()),
            rg_album_gain: Some(album.into()),
            rg_track_peak: peak.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn r128_is_q78_plus_five_db() {
        // -4554 / 256 = -17.789 dB, referenced to -23 LUFS -> -12.789 dB at -18 LUFS.
        let g = gain_db(&opus("-4554", "-4849"), &GainConfig::default());
        assert!((g - (-4554.0 / 256.0 + 5.0)).abs() < 1e-4, "{g}");
        let a = gain_db(&opus("-4554", "-4849"), &GainConfig { mode: GainMode::Album, ..Default::default() });
        assert!((a - (-4849.0 / 256.0 + 5.0)).abs() < 1e-4, "{a}");
    }

    #[test]
    fn same_loudness_material_matches_across_formats() {
        // A track measured at -10 LUFS: R128 says -13 dB to -23, RG says -8 dB to -18.
        let o = gain_db(&opus(&(-13 * 256).to_string(), "0"), &GainConfig::default());
        let f = gain_db(&flac("-8.00 dB", "0 dB", None), &GainConfig::default());
        assert!((o - f).abs() < 1e-4, "opus {o} flac {f}");
    }

    #[test]
    fn header_gain_always_applies() {
        let mut t = opus("-2560", "-2560");
        t.opus_header_db = Some(-3.0);
        let off = GainConfig { mode: GainMode::Off, ..Default::default() };
        assert_eq!(gain_db(&t, &off), -3.0);
        // R128 is relative to the header gain.
        assert!((gain_db(&t, &GainConfig::default()) - (-3.0 - 10.0 + 5.0)).abs() < 1e-4);
    }

    #[test]
    fn untagged_plays_unadjusted() {
        assert_eq!(gain_db(&GainTags::default(), &GainConfig::default()), 0.0);
        let bare_opus = GainTags { opus_header_db: Some(0.0), ..Default::default() };
        assert_eq!(gain_db(&bare_opus, &GainConfig { preamp_db: 6.0, ..Default::default() }), 0.0);
    }

    #[test]
    fn falls_back_to_the_other_gain() {
        let t = GainTags { rg_album_gain: Some("-4 dB".into()), ..Default::default() };
        assert_eq!(gain_db(&t, &GainConfig::default()), -4.0);
    }

    #[test]
    fn clipping_protection_caps_at_peak() {
        // +6 dB gain, peak 0.8 -> max gain is 20*log10(1/0.8) = 1.938 dB.
        let t = flac("+6.00 dB", "0 dB", Some("0.800000"));
        let g = gain_db(&t, &GainConfig::default());
        assert!((g - 1.9382).abs() < 1e-3, "{g}");
        let g = gain_db(&t, &GainConfig { prevent_clipping: false, ..Default::default() });
        assert_eq!(g, 6.0);
    }

    #[test]
    fn the_adjustment_is_reported_with_its_tag() {
        let t = flac("-6.20 dB", "-4 dB", None);
        let a = applied(&t, &GainConfig::default()).unwrap();
        assert!((a.db + 6.2).abs() < 1e-4 && a.source == GainSource::Track && !a.capped, "{a:?}");
        let a = applied(&t, &GainConfig { mode: GainMode::Album, preamp_db: 1.0, ..Default::default() }).unwrap();
        assert!((a.db + 3.0).abs() < 1e-4 && a.source == GainSource::Album, "{a:?}");
        // Album mode, only a track gain tagged: the track's, said so.
        let only_track = GainTags { rg_track_gain: Some("-2 dB".into()), ..Default::default() };
        assert_eq!(applied(&only_track, &GainConfig { mode: GainMode::Album, ..Default::default() }).unwrap().source, GainSource::Track);
        assert_eq!(applied(&t, &GainConfig { mode: GainMode::Off, ..Default::default() }), None);
        assert_eq!(applied(&GainTags::default(), &GainConfig::default()), None, "no tags: unadjusted");
        // The Opus header gain is the file's own, not part of the adjustment.
        let mut o = opus("-2560", "0");
        o.opus_header_db = Some(-3.0);
        let a = applied(&o, &GainConfig::default()).unwrap();
        assert!((a.db - (-10.0 + 5.0)).abs() < 1e-4, "{a:?}");
        assert!((gain_db(&o, &GainConfig::default()) - (-3.0 - 5.0)).abs() < 1e-4);
        // Capped by clipping protection.
        let a = applied(&flac("+6.00 dB", "0 dB", Some("0.800000")), &GainConfig::default()).unwrap();
        assert!(a.capped && (a.db - 1.9382).abs() < 1e-3, "{a:?}");
    }

    #[test]
    fn preamp_and_tag_parsing() {
        let mut t = GainTags::default();
        t.offer("replaygain_track_gain", " -6.20 dB ");
        t.offer("REPLAYGAIN_TRACK_GAIN", "+99 dB"); // first one wins
        let g = gain_db(&t, &GainConfig { preamp_db: 2.0, ..Default::default() });
        assert!((g - (-4.2)).abs() < 1e-4);
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_gain_no_measurement_gives_is_ignored() {
        // A damaged or hostile tag: an infinite gain (NaN at the output), or 60 dB louder.
        for bad in ["1e30 dB", "+60 dB", "-100 dB"] {
            let t = flac(bad, "-4 dB", None);
            assert_eq!(gain_db(&t, &GainConfig::default()), -4.0, "{bad}: the album gain instead");
            let only = GainTags { rg_track_gain: Some(bad.into()), ..Default::default() };
            assert_eq!(applied(&only, &GainConfig::default()), None, "{bad}: unadjusted");
        }
        let o = opus("99999999", "-2560");
        assert!((gain_db(&o, &GainConfig::default()) - (-10.0 + 5.0)).abs() < 1e-4);
        assert!(linear_gain(&opus("99999999", "99999999"), &GainConfig::default()) == 1.0);
        // The ends of what real material measures still apply.
        assert_eq!(gain_db(&flac("+20 dB", "0 dB", None), &GainConfig::default()), 20.0);
        assert_eq!(gain_db(&flac("-30 dB", "0 dB", None), &GainConfig::default()), -30.0);
    }

    #[test]
    fn peak_is_paired_with_the_gains_source() {
        let album_cfg = GainConfig { mode: GainMode::Album, ..Default::default() };
        let track_cfg = GainConfig { mode: GainMode::Track, ..Default::default() };
        // Peak 0.8 caps at +1.9382 dB; peak 0.9 caps at +0.9151 dB.
        let cap_08 = -20.0 * 0.8f32.log10();
        let cap_09 = -20.0 * 0.9f32.log10();

        // Album mode, album gain + both peaks: uses album peak.
        let t = GainTags {
            rg_track_gain: Some("+6.00 dB".into()),
            rg_album_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.9".into()),
            rg_album_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &album_cfg).unwrap();
        assert_eq!(a.source, GainSource::Album);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "album gain pairs with album peak: {a:?}");

        // Album mode, album gain + only track peak: no album peak, so the track peak caps it.
        let t = GainTags {
            rg_album_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &album_cfg).unwrap();
        assert_eq!(a.source, GainSource::Album);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "album gain falls back to the track peak: {a:?}");

        // Album mode, only track gain (fallback) + both peaks: uses track peak.
        let t = GainTags {
            rg_track_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.8".into()),
            rg_album_peak: Some("0.9".into()),
            ..Default::default()
        };
        let a = applied(&t, &album_cfg).unwrap();
        assert_eq!(a.source, GainSource::Track);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "track gain fallback pairs with track peak: {a:?}");

        // Album mode, only track gain (fallback) + only album peak: no track peak, so the album peak caps it.
        let t = GainTags {
            rg_track_gain: Some("+6.00 dB".into()),
            rg_album_peak: Some("0.9".into()),
            ..Default::default()
        };
        let a = applied(&t, &album_cfg).unwrap();
        assert_eq!(a.source, GainSource::Track);
        assert!(a.capped && (a.db - cap_09).abs() < 1e-3, "track gain fallback falls back to the album peak: {a:?}");

        // Track mode, track gain + both peaks: uses track peak.
        let t = GainTags {
            rg_track_gain: Some("+6.00 dB".into()),
            rg_album_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.9".into()),
            rg_album_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &track_cfg).unwrap();
        assert_eq!(a.source, GainSource::Track);
        assert!(a.capped && (a.db - cap_09).abs() < 1e-3, "track gain pairs with track peak: {a:?}");

        // Track mode, track gain + only album peak: no track peak, so the album peak caps it.
        let t = GainTags {
            rg_track_gain: Some("+6.00 dB".into()),
            rg_album_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &track_cfg).unwrap();
        assert_eq!(a.source, GainSource::Track);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "track gain falls back to the album peak: {a:?}");

        // Track mode, only album gain (fallback) + both peaks: uses album peak.
        let t = GainTags {
            rg_album_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.9".into()),
            rg_album_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &track_cfg).unwrap();
        assert_eq!(a.source, GainSource::Album);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "album gain fallback pairs with album peak: {a:?}");

        // Track mode, only album gain (fallback) + only track peak: no album peak, so the track peak caps it.
        let t = GainTags {
            rg_album_gain: Some("+6.00 dB".into()),
            rg_track_peak: Some("0.8".into()),
            ..Default::default()
        };
        let a = applied(&t, &track_cfg).unwrap();
        assert_eq!(a.source, GainSource::Album);
        assert!(a.capped && (a.db - cap_08).abs() < 1e-3, "album gain fallback falls back to the track peak: {a:?}");
    }
}
