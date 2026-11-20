//! Audio engine for Marquee Player. No UI code lives here.
//!
//! - [`Player`]: the engine handle (play, set_next for gapless, seek, pause, volume).
//! - [`replaygain`]: gain rules for R128 and ReplayGain tags.
//! - [`source::Source`]: one decoded file, used directly by tests and tools.
//! - [`feed::Feed`]: a file still being written (a download), played as it grows.

pub mod crossfade;
pub mod downmix;
mod engine;
pub mod eq;
pub mod feed;
pub mod guard;
pub mod limiter;
mod mp4gapless;
pub mod opus;
pub mod output;
pub mod replaygain;
pub mod resample;
pub mod source;
pub mod spectrum;
mod stream;

pub use engine::{Event, OutputConfig, OutputInfo, PlayState, Player, SeekTiming, Status, StreamInfo, TrackRequest, Transition};
pub use feed::Feed;
pub use eq::{Band, EqParams, Kind as EqKind, Preamp};
pub use replaygain::{GainConfig, GainMode};
