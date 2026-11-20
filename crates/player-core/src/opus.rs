//! libopus decoder plugged into Symphonia's codec registry.
//!
//! Why not `symphonia-adapter-libopus`: that adapter trims the Ogg pre-skip from the
//! first decoded packet only. That is wrong when pre-skip is longer than one packet
//! (some encoders use 80 ms), and wrong after a seek back to the start, where the
//! pre-skip region is decoded again but no longer trimmed. Here the trim is derived
//! from the packet timestamp instead: Symphonia's Ogg demuxer timestamps Opus packets
//! in raw granule units, which *include* pre-skip, so any decoded frame whose
//! timestamp is below `pre_skip` is pre-skip and is dropped. That holds no matter
//! where decoding started.
//!
//! End trimming (the last page's granule position) arrives as `packet.trim_end`
//! from the demuxer and is applied as is.
//!
//! The OpusHead output gain is parsed here and exposed through [`OpusHead`], but not
//! applied: gain is applied once, downstream, together with ReplayGain.

use std::ffi::c_int;
use std::fmt;

use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef, layouts,
};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::errors::{Error, Result, unsupported_error};
use symphonia::core::packet::PacketRef;

/// Opus always decodes at 48 kHz here (libopus resamples internally if asked for
/// less, but there is no reason to).
pub const OPUS_RATE: u32 = 48_000;

/// 120 ms at 48 kHz: the longest a single Opus packet can decode to.
const MAX_FRAMES_PER_PACKET: usize = 5760;

/// The fields of the OpusHead identification header this player cares about.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OpusHead {
    pub channels: u8,
    /// Samples at 48 kHz to discard from the start of the stream.
    pub pre_skip: u16,
    /// Output gain in dB (stored as Q7.8 in the header). Must always be applied.
    pub output_gain_db: f32,
}

impl OpusHead {
    /// Parse the 19+ byte OpusHead packet (RFC 7845 section 5.1).
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 19 || &buf[..8] != b"OpusHead" {
            return None;
        }
        let pre_skip = u16::from_le_bytes([buf[10], buf[11]]);
        let gain_q78 = i16::from_le_bytes([buf[16], buf[17]]);
        Some(Self { channels: buf[9], pre_skip, output_gain_db: f32::from(gain_q78) / 256.0 })
    }
}

/// Owned libopus decoder state.
struct Libopus {
    ptr: *mut opusic_sys::OpusDecoder,
}

// The decoder state is only ever touched through `&mut self`.
unsafe impl Send for Libopus {}
unsafe impl Sync for Libopus {}

impl Libopus {
    fn new(channels: usize) -> Result<Self> {
        let mut err: c_int = 0;
        let ptr =
            unsafe { opusic_sys::opus_decoder_create(OPUS_RATE as i32, channels as c_int, &mut err) };
        if err != opusic_sys::OPUS_OK || ptr.is_null() {
            return Err(Error::DecodeError("opus: decoder create failed"));
        }
        Ok(Self { ptr })
    }

    /// Decode one packet into `out` (interleaved). Returns frames per channel.
    fn decode(&mut self, data: &[u8], out: &mut [f32], channels: usize) -> Result<usize> {
        let (ptr, len) = if data.is_empty() { (std::ptr::null(), 0) } else { (data.as_ptr(), data.len()) };
        let n = unsafe {
            opusic_sys::opus_decode_float(
                self.ptr,
                ptr,
                len as i32,
                out.as_mut_ptr(),
                (out.len() / channels) as c_int,
                0,
            )
        };
        if n < 0 {
            return Err(Error::DecodeError("opus: decode failed"));
        }
        Ok(n as usize)
    }

    fn reset(&mut self) {
        unsafe {
            opusic_sys::opus_decoder_ctl(self.ptr, opusic_sys::OPUS_RESET_STATE);
        }
    }
}

impl Drop for Libopus {
    fn drop(&mut self) {
        unsafe { opusic_sys::opus_decoder_destroy(self.ptr) }
    }
}

pub struct OpusDecoder {
    params: AudioCodecParameters,
    head: OpusHead,
    channels: usize,
    lib: Libopus,
    pcm: Vec<f32>,
    buf: AudioBuffer<f32>,
}

impl fmt::Debug for OpusDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpusDecoder").field("head", &self.head).finish_non_exhaustive()
    }
}

fn spec_for(channels: usize) -> AudioSpec {
    let layout = if channels == 1 { layouts::CHANNEL_LAYOUT_MONO } else { layouts::CHANNEL_LAYOUT_STEREO };
    AudioSpec::new(OPUS_RATE, layout)
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, _opts: &AudioDecoderOptions) -> Result<Self> {
        let head = params.extra_data.as_deref().and_then(OpusHead::parse).unwrap_or_default();
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(head.channels as usize);
        if !(1..=2).contains(&channels) {
            // Multichannel Opus needs the multistream API; not worth it for a music library.
            return unsupported_error("opus: only mono and stereo are supported");
        }
        Ok(Self {
            params: params.clone(),
            head,
            channels,
            lib: Libopus::new(channels)?,
            pcm: vec![0.0; MAX_FRAMES_PER_PACKET * channels],
            buf: AudioBuffer::new(spec_for(channels), MAX_FRAMES_PER_PACKET),
        })
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        self.lib.reset();
    }

    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs()[0].info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        self.buf.clear();
        let frames = self.lib.decode(packet.data, &mut self.pcm, self.channels)?;
        self.buf.render_uninit(Some(frames));
        let n = frames * self.channels;
        self.buf.copy_from_slice_interleaved(&&self.pcm[..n]);

        // Pre-skip by timestamp: frames before `pre_skip` in raw granule units.
        let pre_skip_trim = pre_skip_trim(self.head.pre_skip, packet.pts.get(), frames);
        let start = (packet.trim_start.get() as usize).max(pre_skip_trim);
        let end = packet.trim_end.get() as usize;
        self.buf.trim(start.min(frames), end.min(frames.saturating_sub(start)));
        Ok(self.buf.as_generic_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(OpusDecoder::try_new(params, opts)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        &[symphonia_core::support_audio_codec!(CODEC_ID_OPUS, "opus", "Opus (libopus)")]
    }
}

/// How many of a packet's `frames` decoded at granule `pts` fall before `pre_skip`.
/// Saturates on a timestamp from a hostile file.
fn pre_skip_trim(pre_skip: u16, pts: i64, frames: usize) -> usize {
    i64::from(pre_skip).saturating_sub(pts).clamp(0, frames as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_skip_trim_saturates_on_hostile_timestamps() {
        assert_eq!(pre_skip_trim(312, 0, 960), 312);
        assert_eq!(pre_skip_trim(312, 960, 960), 0);
        assert_eq!(pre_skip_trim(312, i64::MIN, 960), 960);
        assert_eq!(pre_skip_trim(312, i64::MAX, 960), 0);
    }

    #[test]
    fn parses_opus_head() {
        let mut h = b"OpusHead".to_vec();
        h.push(1); // version
        h.push(2); // channels
        h.extend_from_slice(&312u16.to_le_bytes());
        h.extend_from_slice(&48_000u32.to_le_bytes());
        h.extend_from_slice(&(-512i16).to_le_bytes()); // -2 dB
        h.push(0);
        let head = OpusHead::parse(&h).unwrap();
        assert_eq!(head, OpusHead { channels: 2, pre_skip: 312, output_gain_db: -2.0 });
        assert!(OpusHead::parse(b"OpusTags").is_none());
    }
}
