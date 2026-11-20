//! One opened audio file: demuxer + decoder, producing interleaved stereo f32 at the
//! file's own sample rate, with encoder delay and padding already trimmed.
//!
//! Positions are in *content frames*: frame 0 is the first audible sample, after
//! Opus pre-skip / MP3 encoder delay.

use std::fs::File;
use std::path::Path;
use std::sync::OnceLock;

use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ADPCM_G722, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_S32LE,
    CODEC_ID_VORBIS,
};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{MetadataOptions, RawValue};
use symphonia::core::units::Timestamp;

use crate::feed::{Canceller, Feed};
use crate::opus::{OpusDecoder, OpusHead};
use crate::replaygain::GainTags;

/// Frames decoded and thrown away before a seek target, so libopus has converged by
/// the time audible output starts. RFC 7845 says at least 80 ms, but measured on this
/// library 80 ms still leaves the first 20 ms after a seek at only ~20 dB SNR against
/// a straight decode (CELT energy prediction converges slowly). 1 s makes the output
/// identical to a straight decode and costs a few ms of decoding per seek.
const OPUS_SEEK_PREROLL: u64 = 48_000;

#[derive(Debug)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

impl From<SymError> for SourceError {
    fn from(e: SymError) -> Self {
        SourceError(e.to_string())
    }
}

impl From<std::io::Error> for SourceError {
    fn from(e: std::io::Error) -> Self {
        SourceError(e.to_string())
    }
}

fn codecs() -> &'static CodecRegistry {
    static REG: OnceLock<CodecRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let mut reg = CodecRegistry::new();
        reg.register_audio_decoder::<OpusDecoder>();
        symphonia::default::register_enabled_codecs(&mut reg);
        reg
    })
}

/// Undecodable packets in a row after which a file counts as damaged rather than having a
/// damaged stretch (about 10 s of MP3 or AAC, 2 s of 4096-frame FLAC blocks).
const BAD_RUN_LIMIT: u32 = 400;

pub struct Source {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub sample_rate: u32,
    /// Raw timestamp of content frame 0 (Opus pre-skip; 0 elsewhere, where the decoder
    /// trims delay itself and timestamps already start at the first audible frame).
    ts_offset: i64,
    /// Content frames in the file, when the container says.
    pub total_frames: Option<u64>,
    pub gain_tags: GainTags,
    /// Content frames before this one are dropped (seek target).
    discard_until: u64,
    /// Content frame of the next frame `next_chunk` will return.
    pub position: u64,
    scratch: Vec<f32>,
    is_opus: bool,
    /// AAC in MP4 with gapless information Symphonia does not apply (`mp4gapless`): the
    /// frames before `ts_offset` and after `total_frames` are dropped here.
    trim_here: bool,
    /// Surround to stereo, built for the layout of the last decoded packet.
    downmix: Option<(usize, crate::downmix::Downmix)>,
    /// What the decoder says about the stream (`StreamInfo`).
    pub codec_name: Option<&'static str>,
    pub bits_per_sample: Option<u32>,
    pub channels: Option<u32>,
    /// Packets the decoder refused (skipped): all of them, and in a row.
    skipped: u32,
    bad_run: u32,
}

/// A codec's name as people say it, for the ones played here; PCM (WAV, AIFF) is "PCM",
/// the container naming it better.
fn codec_name(id: AudioCodecId) -> Option<&'static str> {
    Some(match id {
        CODEC_ID_FLAC => "FLAC",
        CODEC_ID_ALAC => "ALAC",
        CODEC_ID_MP3 => "MP3",
        CODEC_ID_MP2 => "MP2",
        CODEC_ID_AAC => "AAC",
        CODEC_ID_OPUS => "Opus",
        CODEC_ID_VORBIS => "Vorbis",
        // The PCM ids run from PCM_S32LE up to the first ADPCM one (symphonia's well_known).
        id if (CODEC_ID_PCM_S32LE..CODEC_ID_ADPCM_G722).contains(&id) => "PCM",
        _ => return None,
    })
}

impl Source {
    /// Open a file for decoding. A panic in the demuxer or decoder (a damaged file meeting a bug)
    /// is this file's error, never the engine's: the same holds for `seek` and `next_chunk`.
    pub fn open(path: &Path) -> Result<Self, SourceError> {
        crate::guard::catching(|| Self::open_inner(path, None)).unwrap_or_else(|what| Err(SourceError(format!("the decoder failed on this file ({what})"))))
    }

    /// Open a file still being written, reading it only through `feed` (`feed.rs`): a read
    /// waits for its bytes, so this and every later call can wait, until `cancel` stops it.
    pub fn open_feed(path: &Path, feed: &Feed, cancel: &Canceller) -> Result<Self, SourceError> {
        crate::guard::catching(|| Self::open_inner(path, Some((feed, cancel)))).unwrap_or_else(|what| Err(SourceError(format!("the decoder failed on this file ({what})"))))
    }

    fn open_inner(path: &Path, feed: Option<(&Feed, &Canceller)>) -> Result<Self, SourceError> {
        let media: Box<dyn MediaSource> = match feed {
            None => Box::new(File::open(path)?),
            Some((f, c)) => Box::new(f.reader(path, c)),
        };
        let mss = MediaSourceStream::new(media, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let mut reader = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;

        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| SourceError("no audio track".into()))?
            .clone();
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            return Err(SourceError("no audio codec parameters".into()));
        };
        let is_opus = params.codec == CODEC_ID_OPUS;
        let opus_head = if is_opus { params.extra_data.as_deref().and_then(OpusHead::parse) } else { None };
        let sample_rate = params.sample_rate.ok_or_else(|| SourceError("unknown sample rate".into()))?;

        let decoder = codecs().make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(true))?;

        let mut ts_offset = opus_head.map(|h| i64::from(h.pre_skip)).unwrap_or(0);
        let mut total_frames = track.num_frames.map(|n| {
            let delay = u64::from(track.delay.unwrap_or(0));
            let padding = u64::from(track.padding.unwrap_or(0));
            n.saturating_sub(delay + padding)
        });
        let mut trim_here = false;
        if params.codec == CODEC_ID_AAC
            && track.delay.is_none()
            && reader.format_info().short_name == "isomp4"
            && let Some(t) = match feed {
                None => crate::mp4gapless::read(path),
                // Through the feed: the `moov` box was on disk for the probe, so this does not wait.
                Some((f, c)) => {
                    let r = f.reader(path, c);
                    let len = r.byte_len().unwrap_or(u64::MAX);
                    crate::mp4gapless::read_from(r, len)
                }
            }
            .filter(|t| t.scale == u64::from(sample_rate))
        {
            ts_offset = i64::try_from(t.delay).unwrap_or(i64::MAX);
            total_frames = Some(t.frames);
            trim_here = true;
        }

        let mut gain_tags = GainTags { opus_header_db: opus_head.map(|h| h.output_gain_db), ..Default::default() };
        read_gain_tags(reader.as_mut(), &mut gain_tags);
        let lossy = matches!(params.codec, CODEC_ID_MP3 | CODEC_ID_MP2 | CODEC_ID_AAC | CODEC_ID_OPUS | CODEC_ID_VORBIS);

        Ok(Self {
            reader,
            decoder,
            track_id: track.id,
            sample_rate,
            ts_offset,
            total_frames,
            gain_tags,
            discard_until: 0,
            position: 0,
            scratch: Vec::new(),
            is_opus,
            trim_here,
            downmix: None,
            skipped: 0,
            bad_run: 0,
            codec_name: codec_name(params.codec),
            // A lossy decoder's output depth is its own choice, not the file's.
            bits_per_sample: if lossy { None } else { params.bits_per_sample.filter(|&b| b > 0) },
            channels: params.channels.as_ref().map(|c| c.count() as u32).filter(|&c| c > 0),
        })
    }

    /// Seek to a content frame. Accurate: the next chunk starts exactly there.
    pub fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
        crate::guard::catching(|| self.seek_inner(frame)).unwrap_or_else(|what| Err(SourceError(format!("the decoder failed seeking in this file ({what})"))))
    }

    fn seek_inner(&mut self, frame: u64) -> Result<(), SourceError> {
        if i64::try_from(frame).is_err() {
            // Past any timestamp a stream can hold (a seek to infinity in a track of unknown
            // length): refused, and the track plays on from where it was.
            return Err(SourceError(format!("frame {frame} is past the end of any track")));
        }
        let preroll = if self.is_opus { OPUS_SEEK_PREROLL } else { 0 };
        let ts = seek_timestamp(frame, preroll, self.ts_offset);
        self.reader.seek(SeekMode::Accurate, SeekTo::Timestamp { ts: Timestamp::from(ts), track_id: self.track_id })?;
        self.decoder.reset();
        self.discard_until = frame;
        self.position = frame;
        Ok(())
    }

    /// Append the next decoded frames (interleaved stereo) to `out`. Returns `false` at
    /// end of stream. May append nothing and return `true` (trimmed or skipped packet).
    pub fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<bool, SourceError> {
        crate::guard::catching(|| self.next_chunk_inner(out)).unwrap_or_else(|what| Err(SourceError(format!("the decoder failed on this file ({what})"))))
    }

    fn next_chunk_inner(&mut self, out: &mut Vec<f32>) -> Result<bool, SourceError> {
        let packet = loop {
            match self.reader.next_packet() {
                Ok(Some(p)) if p.track_id == self.track_id => break p,
                Ok(Some(_)) => continue,
                Ok(None) => return Ok(false),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(e) => return Err(e.into()),
            }
        };
        let decoded = match self.decoder.decode(&packet) {
            Ok(d) => d,
            // A corrupt packet is skipped, not fatal.
            // Logged for the first few (a long damaged stretch would flood the log); a file
            // whose packets keep failing is an error, not a track that races by in silence.
            Err(SymError::DecodeError(msg)) => {
                self.skipped += 1;
                self.bad_run += 1;
                if self.skipped <= 3 || self.skipped.is_power_of_two() {
                    log::warn!("decode error, skipping packet ({} so far): {msg}", self.skipped);
                }
                if self.bad_run >= BAD_RUN_LIMIT {
                    return Err(SourceError(format!("its audio is damaged ({} packets in a row could not be decoded)", self.bad_run)));
                }
                return Ok(true);
            }
            Err(e) => return Err(e.into()),
        };
        self.bad_run = 0;
        let frames = decoded.frames();
        if frames == 0 {
            return Ok(true);
        }
        let channels = decoded.spec().channels().count();
        decoded.copy_to_vec_interleaved(&mut self.scratch);

        // Content frame of the first decoded frame. The decoder trimmed `trim_start` (and,
        // for Opus, everything before pre-skip), so start from whichever is later. MP4
        // priming is not trimmed by the decoder: those frames (`pre`) are dropped here.
        let (pre, first) = content_start(packet.pts.get(), packet.trim_start.get(), self.ts_offset, frames, self.trim_here);
        if self.trim_here && self.total_frames.is_some_and(|t| first >= t) {
            // Encoder padding: the content has ended.
            return Ok(false);
        }
        let skip = (pre + self.discard_until.saturating_sub(first) as usize).min(frames);
        if skip == frames {
            return Ok(true);
        }
        let start = first + (skip - pre) as u64;
        let mut kept = frames - skip;
        if self.trim_here
            && let Some(t) = self.total_frames
        {
            kept = kept.min(t.saturating_sub(start) as usize);
        }
        let float = matches!(decoded, GenericAudioBufferRef::F32(_) | GenericAudioBufferRef::F64(_));
        let src = &mut self.scratch[skip * channels..(skip + kept) * channels];
        if float {
            // A float file can hold a NaN or an infinity (or a 64-bit value beyond f32's
            // range): it would poison the equalizer's filters and the limiter, and reach
            // the device. Silence instead. Finite samples are left exactly as they are;
            // integer samples cannot be anything but finite, so they are not looked at.
            clean(src);
        }
        let src = &*src;
        out.reserve(kept * 2);
        match channels {
            1 => src.iter().for_each(|&s| out.extend_from_slice(&[s, s])),
            2 => out.extend_from_slice(src),
            // Surround: mixed down by its channel layout (downmix.rs).
            n => {
                if self.downmix.as_ref().is_none_or(|(c, _)| *c != n) {
                    let d = crate::downmix::Downmix::new(decoded.spec().channels(), n);
                    log::debug!("downmix of {n} channels ({}): {:?}", decoded.spec().channels(), d.gains);
                    self.downmix = Some((n, d));
                }
                self.downmix.as_ref().unwrap().1.apply(src, out);
            }
        }
        self.position = start + kept as u64;
        Ok(true)
    }
}

/// Non-finite samples (NaN, an infinity) to 0.0; every other sample untouched. A select,
/// not a branch, so it vectorises.
fn clean(samples: &mut [f32]) {
    for s in samples {
        *s = if s.is_finite() { *s } else { 0.0 };
    }
}

fn read_gain_tags(reader: &mut dyn FormatReader, tags: &mut GainTags) {
    let mut meta = reader.metadata();
    let Some(rev) = meta.skip_to_latest() else { return };
    let containers = std::iter::once(&rev.media).chain(rev.per_track.iter().map(|t| &t.metadata));
    for c in containers {
        for tag in &c.tags {
            let value = match &tag.raw.value {
                RawValue::String(s) => s.as_str().to_owned(),
                RawValue::StringList(l) => l.first().cloned().unwrap_or_default(),
                other => other.to_string(),
            };
            tags.offer(&tag.raw.key, &value);
        }
    }
}

/// The stream timestamp a seek to content `frame` asks the reader for: `preroll` frames
/// early (Opus), shifted by the stream's `ts_offset`. The start is 0 itself. Saturates
/// rather than wrapping on a frame or offset from a hostile file.
fn seek_timestamp(frame: u64, preroll: u64, ts_offset: i64) -> i64 {
    let frame = frame.saturating_sub(preroll);
    if frame == 0 { 0 } else { i64::try_from(frame).unwrap_or(i64::MAX).saturating_add(ts_offset) }
}

/// For a decoded packet at `pts` (stream timestamp) whose decoder trimmed `trim_start`
/// frames, with `frames` left: how many leading frames are priming to drop here (`pre`,
/// only when `trim_here`) and the content frame of the first one kept. Saturates rather
/// than wrapping on timestamps from a hostile file.
fn content_start(pts: i64, trim_start: u64, ts_offset: i64, frames: usize, trim_here: bool) -> (usize, u64) {
    let raw = pts.saturating_add(i64::try_from(trim_start).unwrap_or(i64::MAX));
    let pre = if trim_here { ts_offset.saturating_sub(raw).clamp(0, frames as i64) as usize } else { 0 };
    let first = raw.max(ts_offset).saturating_sub(ts_offset).max(0) as u64;
    (pre, first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_samples_are_silenced_and_the_rest_left_alone() {
        let mut v = [0.5, f32::NAN, -1.25, f32::INFINITY, f32::NEG_INFINITY, f32::MIN_POSITIVE / 2.0, -0.0, f32::MAX];
        clean(&mut v);
        let want = [0.5, 0.0, -1.25, 0.0, 0.0, f32::MIN_POSITIVE / 2.0, -0.0, f32::MAX];
        assert_eq!(v.map(f32::to_bits), want.map(f32::to_bits), "finite samples bit-identical");
    }

    #[test]
    fn seek_timestamps_saturate_on_hostile_values() {
        assert_eq!(seek_timestamp(0, 0, 1024), 0);
        assert_eq!(seek_timestamp(100, 3840, 312), 0);
        assert_eq!(seek_timestamp(10_000, 0, 2112), 12_112);
        assert_eq!(seek_timestamp(u64::MAX, 0, 2112), i64::MAX);
        assert_eq!(seek_timestamp(i64::MAX as u64, 0, i64::MAX), i64::MAX);
    }

    #[test]
    fn packet_starts_saturate_on_hostile_values() {
        // Ordinary: MP4 priming of 2112 frames, the first packet at 0.
        assert_eq!(content_start(0, 0, 2112, 1024, true), (1024, 0));
        assert_eq!(content_start(2048, 0, 2112, 1024, true), (64, 0));
        assert_eq!(content_start(4096, 0, 2112, 1024, true), (0, 1984));
        // Crafted: timestamps and delays at the ends of the range.
        assert_eq!(content_start(i64::MIN, 0, i64::MAX, 1024, true), (1024, 0));
        assert_eq!(content_start(i64::MAX, u64::MAX, 0, 1024, true), (0, i64::MAX as u64));
        assert_eq!(content_start(i64::MAX, 5, i64::MIN, 1024, false), (0, i64::MAX as u64));
    }
}
