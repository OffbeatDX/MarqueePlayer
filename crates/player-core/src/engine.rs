//! The player: a decode thread that stays ~1 s ahead, an output thread that feeds the
//! device on time, and a handle the app drives.
//!
//! Gapless: the app sets the *next* track whenever a track starts. It is opened straight
//! away on a thread of its own (`mp-open`), so a slow or sleeping disk never stalls the
//! decoding of the current track; when the current file runs out, the decode thread
//! continues with it into the same chunk stream, so the device sees one continuous
//! signal.
//!
//! Seek / play-now: the decode thread bumps a generation counter; the output thread
//! drops every chunk from an older generation and flushes the device buffer.
//!
//! Crossfades between tracks at different sample rates: the device runs at one rate, so one
//! track of the overlap has to be converted (`resample.rs`). The device keeps the rate it
//! plays at and the incoming track is converted to it, here on the decode thread, from its
//! first frame; reopening the device at the incoming rate before the overlap (converting
//! the outgoing track's tail instead) would cut a gap into the outgoing track's music, as a
//! reopen in the middle of a track always does. The incoming track stays converted after
//! the overlap, since going to its own rate means a reopen too, until the stream is cut
//! anyway: a pause (the output asks for the audible frame again at the track's own rate,
//! `Cmd::Native`, and the resume reopens the device for it), a seek, a skip without
//! crossfade, a stop, the end of the queue. A track at the rate playing that follows
//! (crossfading or gapless) plays as it is, and a gapless track at a third rate reopens the
//! device as before; one at the converted track's own rate goes on through the same
//! converter, gapless (the album the crossfade went into). Nothing else is converted: with
//! crossfade off, and for every track not reached through a crossfade across rates, the
//! samples are exactly as before. The output reports the conversion
//! (`OutputInfo::converted_from`) so the app can say so.
//!
//! Files still being written (`TrackRequest::feed`, `feed.rs`): each is decoded on a thread of
//! its own (`stream.rs`), where its reads wait for their bytes; this thread only takes what is
//! decoded there and never waits on a file, so every command is served as promptly as for a
//! file on disk. When a growing file has nothing decoded yet (its open, a seek past what is on
//! disk, a download slower than playback, a `set_next` track not there when the one before it
//! ends), this thread is *starved*: it has sent all it has and waits for that file's thread or
//! a command, whichever comes first, and tells the output (`Shared::starved`), which says
//! `Event::Buffering` once what is queued has played and the position stops. It plays on by
//! itself when the frames come. A crossfade is planned only once the incoming track has frames
//! decoded: one that is late starts late (shorter), or does not happen and the tracks follow
//! gaplessly. A download that fails is the track's `Event::Error`, after the frames that were
//! on disk, and the engine moves on as for a file that cannot be decoded.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};

use crate::crossfade;
use crate::eq::EqParams;
use crate::feed::Feed;
use crate::limiter::Limiter;
use crate::output::{self, Device};
use crate::replaygain::{GainConfig, GainTags, linear_gain};
use crate::resample::{self, Filter, Filters, Resampler};
use crate::source::Source;
use crate::stream::{Opening, Pull, Stream};

/// Frames per chunk sent to the output thread (~85 ms at 48 kHz).
const CHUNK_FRAMES: usize = 4096;
/// Chunks buffered between decode and output (~1 s at 48 kHz).
const CHUNK_QUEUE: usize = 12;
/// How long a seek into the track just played waits for it to reopen when it is a file still
/// being written (all of it was read, so it is on disk and opens at once).
const REOPEN_WAIT: Duration = Duration::from_millis(500);

/// How the engine goes from one track to the `set_next` one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Transition {
    /// Straight on: the next file continues the same sample stream.
    #[default]
    Gapless,
    /// The two overlap for up to this many seconds, equal power (`crossfade.rs`): within
    /// a fifth of either track, and gapless when that is under half a second or the
    /// outgoing track's length is not known. Across a change of sample rate the incoming
    /// track is converted to the rate playing (see the module doc); for the rare pair of
    /// rates with no converter (`resample::Filter::new`) the outgoing track fades out over
    /// the same length, the device reopens, and the next one starts as it is.
    Crossfade { secs: f32 },
}

/// A track to play. `token` is the app's handle for it (a queue entry id, say);
/// the engine reports it back in events and status.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackRequest {
    pub token: u64,
    pub path: PathBuf,
    /// The file at `path` is still being written through this feed (`feed.rs`): the engine
    /// plays it as it grows, waiting (`Event::Buffering`) wherever its bytes are not on disk
    /// yet. `None`: a file on disk, read as it always was; so is a feed already complete.
    pub feed: Option<Feed>,
}

impl TrackRequest {
    /// A file on disk.
    pub fn new(token: u64, path: impl Into<PathBuf>) -> Self {
        TrackRequest { token, path: path.into(), feed: None }
    }

    /// A file being written at `path` through `feed`.
    pub fn growing(token: u64, path: impl Into<PathBuf>, feed: Feed) -> Self {
        TrackRequest { token, path: path.into(), feed: Some(feed) }
    }

    /// Its feed, while the file is not complete: it is decoded on a thread of its own.
    fn still_growing(&self) -> Option<&Feed> {
        self.feed.as_ref().filter(|f| !f.is_complete())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The first frame of this track reached the device.
    TrackStarted { token: u64 },
    /// The last track finished and no next track was set. Also follows a `play` whose
    /// track could not be opened (nothing is playing then), so moving on to the next
    /// queue item on `Ended` is the one place a failed track is skipped.
    Ended,
    /// A track could not be opened or decoded. Reported only: the engine itself moves on
    /// as if the track had ended (into the `set_next` track, or `Ended`). Callers must not
    /// skip on this as well, or the track after the bad one is skipped too. A `set_next`
    /// track that cannot be opened is reported once its open fails (moments after it is
    /// set, while the track before it still plays) and then left out; it is tried again
    /// (and fails again) only if the caller plays it.
    Error { token: u64, message: String },
    /// The output (re)opened: which device, in which mode. Sent when it differs from
    /// the last one reported.
    Output(OutputInfo),
    /// `Some(why)`: the device cannot be opened (no device, one that refuses the format, one
    /// removed with nothing to fall back to): nothing is heard, and the engine keeps trying.
    /// Sent once when it starts failing; `None` once a device opens again.
    NoOutput(Option<String>),
    /// Playback waits for the bytes of the track with this token, a file still being written
    /// (`TrackRequest::feed`): nothing is heard and the position holds (`on: true`) until they
    /// are on disk and it plays on by itself, fading in (`on: false`). Always in pairs: every
    /// `on: true` is followed by an `on: false` for the same token, also when a stop, a seek or
    /// another track ends the wait first. The token is the track waited for: the next one when
    /// it is not there as the one before it ends.
    Buffering { token: u64, on: bool },
}

/// Where to play: the app's choice (`Player::set_output`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputConfig {
    /// A device by its id (stable across restarts), or `None`: the system default, and
    /// whatever it becomes.
    pub device: Option<String>,
    /// Exclusive (bit-perfect) mode: the device at the track's own format, no system
    /// mixer. Falls back to shared mode when the device refuses.
    pub exclusive: bool,
}

/// What the output opened, as the device reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputInfo {
    /// The device's name, as the system shows it.
    pub device: String,
    /// Its id.
    pub id: String,
    /// The chosen device is not there: this is the system default meanwhile.
    pub missing: bool,
    /// Exclusive mode is in effect: the format ("24-bit / 96 kHz").
    pub exclusive: Option<String>,
    /// Exclusive mode was asked for but is not in effect, and why.
    pub note: Option<String>,
    /// The sample rate the stream was opened at (the track's own).
    pub rate: u32,
    /// Shared mode: the device's mix format rate (Windows' "Default Format"), which
    /// Windows converts the stream to when it differs from `rate`.
    pub mix_rate: Option<u32>,
    /// Exclusive mode: the sample format the device took ("24-bit", "32-bit float").
    pub format: Option<String>,
    /// The sound server that mixes and converts ("PipeWire", "ALSA"); `None` on Windows,
    /// whose own audio engine the wording there names. On Linux, `mix_rate` is the rate the
    /// server's graph runs at (once known) and `exclusive` says the graph runs at the
    /// track's own rate because the bit-perfect setting asked for it.
    pub server: Option<&'static str>,
    /// The track playing is converted by the engine from this rate to `rate`: it came in
    /// through a crossfade from a track at `rate` and keeps that rate until the next pause,
    /// seek or stop (the module doc of `engine.rs`). `None`: it plays at its own rate.
    /// Reported again whenever it changes.
    pub converted_from: Option<u32>,
}

/// What the decoder found in a track's file (`Player::stream_info`), and its loudness
/// tags, so the app can say what plays and how it is adjusted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    /// "FLAC", "MP3", "Opus"...; "PCM" for WAV and AIFF; `None` when not known.
    pub codec: Option<&'static str>,
    pub sample_rate: u32,
    /// Lossless streams only.
    pub bits_per_sample: Option<u32>,
    pub channels: Option<u32>,
    pub gain_tags: GainTags,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: PlayState,
    pub token: Option<u64>,
    /// Position of the audible frame, seconds.
    pub position: f64,
    pub duration: Option<f64>,
    /// Timing of the last seek, once its first audio reached the device.
    pub last_seek: Option<SeekTiming>,
    /// Waiting for a growing file's bytes (between `Event::Buffering` on and off).
    pub buffering: bool,
}

/// Where the time of a seek goes, in milliseconds from the `seek` call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeekTiming {
    /// The file is repositioned (demuxer seek, decoder reset).
    pub positioned_ms: f64,
    /// The first chunk from the new position is decoded and handed to the output
    /// (includes the Opus preroll decode).
    pub decoded_ms: f64,
    /// The first samples from the new position are written to the device.
    pub written_ms: f64,
    /// The device's own latency, added before they are heard.
    pub device_ms: f64,
}

impl SeekTiming {
    pub fn audible_ms(&self) -> f64 {
        self.written_ms + self.device_ms
    }
}

/// Seek timing in progress, filled in by the handle and both threads.
#[derive(Debug, Default)]
pub(crate) struct SeekTrace {
    pub requested: Option<Instant>,
    pub generation: u64,
    pub positioned: Option<Duration>,
    pub decoded: Option<Duration>,
}

/// Decoded audio on its way to the device.
pub(crate) struct Chunk {
    pub generation: u64,
    pub token: u64,
    pub rate: u32,
    /// Content frame of the first frame in `samples`.
    pub start: u64,
    pub duration: Option<u64>,
    /// Interleaved stereo.
    pub samples: Vec<f32>,
    /// Linear gain (header gain + ReplayGain) for this track; 1 for a crossfade's mix,
    /// whose tracks' gains are in the samples.
    pub gain: f32,
    /// Peaks above full scale are to be limited (`GainConfig::limit_peaks`).
    pub limit: bool,
    /// The decode thread limited them already (the equalizer was off): the output thread
    /// limits what it has not, after the equalizer and any mix, so the limiter always
    /// sees what plays.
    pub limited: bool,
    /// The track it is labelled with is converted from this rate to `rate`; `start` and
    /// `duration` count frames at `rate`.
    pub converted_from: Option<u32>,
    /// Two tracks mixed (a crossfade).
    pub mix: bool,
}

pub(crate) enum Msg {
    Audio(Chunk),
    /// Nothing follows in this generation.
    EndOfQueue { generation: u64 },
}

pub(crate) enum Cmd {
    /// Play from this many seconds in; with a crossfade of this many seconds from what is
    /// playing (a manual skip), done by the output thread.
    Play(TrackRequest, f64, Option<f32>),
    SetNext(Option<TrackRequest>, Transition),
    /// Seek within the track with this token (the audible one).
    Seek { token: Option<u64>, secs: f64 },
    /// From the output thread, paused in a converted track: play on from this frame (counted
    /// at `rate`, the converted stream's) at the track's own rate, if this generation is
    /// still the decoder's. The resume reopens the device at that rate.
    Native { generation: u64, token: u64, frame: u64, rate: u32 },
    Stop,
    Gain(GainConfig),
    Shutdown,
}

/// State shared by the handle and both threads.
pub(crate) struct Shared {
    /// Chunks from generations below this are stale.
    pub generation: AtomicU64,
    /// The generation a stop began (0 before anything played): nothing plays in it, and the
    /// output says neither playing nor paused for it. Written after the bump.
    pub stopped: AtomicU64,
    pub paused: AtomicBool,
    /// f32 bits.
    pub volume: AtomicU32,
    pub status: Mutex<Status>,
    pub shutdown: AtomicBool,
    pub seek_trace: Mutex<SeekTrace>,
    /// The output choice and its sequence number (bumped on every change).
    pub output: Mutex<(u64, OutputConfig)>,
    /// What the output last opened.
    pub output_info: Mutex<Option<OutputInfo>>,
    /// Wakes the output thread (it blocks while paused or idle). One pending wake is
    /// enough: the thread re-reads the shared state when it wakes.
    pub wake: Sender<()>,
    /// The visualizer's tap on what is written to the device (`spectrum.rs`).
    pub spectrum: Arc<crate::spectrum::Tap>,
    /// The tracks opened last (the current one and the next), by token.
    pub streams: Mutex<Vec<(u64, StreamInfo)>>,
    /// The equalizer's setting (`Player::set_eq`).
    pub eq: EqShared,
    /// A play-now that crossfades from what is playing: its generation, and the fade's
    /// length (f32 bits, seconds). Written before the generation is bumped.
    pub soft_gen: AtomicU64,
    pub soft_secs: AtomicU32,
    /// The rate a manual skip's crossfade would mix at now (the device's, while it plays and
    /// could crossfade from what it plays); 0: it would not crossfade. The decode thread
    /// converts the new track to it when the track is at another rate.
    pub soft_rate: AtomicU32,
    /// The decode thread has sent all it has and waits for a growing file's frames:
    /// (generation, token of the track waited for). The output says `Event::Buffering` once it
    /// has played what is queued.
    pub starved: Mutex<Option<(u64, u64)>>,
    /// Bumped when the system's devices or its default changed (`Player::devices_changed`):
    /// the output looks again at where it plays, also while paused or stopped.
    pub devices: AtomicU64,
}

/// The equalizer's setting, from the handle to the output thread, which takes it with
/// `try_lock` (never waiting: if the handle holds it, the next write takes it).
pub(crate) struct EqShared {
    /// Bumped on every change.
    pub seq: AtomicU64,
    pub params: Mutex<EqParams>,
    /// On: the decode thread leaves limiting to the output thread (after the equalizer).
    pub enabled: AtomicBool,
}

/// How many opened tracks `Shared::streams` remembers.
const STREAMS_KEPT: usize = 4;

impl Shared {
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn wake_output(&self) {
        let _ = self.wake.try_send(());
    }
}

pub struct Player {
    cmd: Sender<Cmd>,
    shared: Arc<Shared>,
    threads: Vec<thread::JoinHandle<()>>,
    /// `mp-open`: waited for a moment only, as an open can hang (a network share gone away).
    opener: Option<thread::JoinHandle<()>>,
}

impl Player {
    /// Start the engine on the system's output (`output::system_device`: WASAPI on
    /// Windows, PipeWire on Linux).
    pub fn new() -> (Self, Receiver<Event>) {
        Self::with_device(output::system_device)
    }

    /// Start the engine on any device. Tests use a capture device.
    pub fn with_device<F>(make_device: F) -> (Self, Receiver<Event>)
    where
        F: FnOnce() -> Box<dyn Device> + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = unbounded();
        let (chunk_tx, chunk_rx) = bounded(CHUNK_QUEUE);
        let (ev_tx, ev_rx) = unbounded();
        let (wake_tx, wake_rx) = bounded(1);
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            stopped: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            status: Mutex::new(Status::default()),
            shutdown: AtomicBool::new(false),
            seek_trace: Mutex::new(SeekTrace::default()),
            output: Mutex::new((0, OutputConfig::default())),
            output_info: Mutex::new(None),
            wake: wake_tx,
            spectrum: crate::spectrum::Tap::new(),
            streams: Mutex::new(Vec::new()),
            eq: EqShared { seq: AtomicU64::new(0), params: Mutex::new(EqParams::default()), enabled: AtomicBool::new(false) },
            soft_gen: AtomicU64::new(u64::MAX),
            soft_secs: AtomicU32::new(0),
            soft_rate: AtomicU32::new(0),
            starved: Mutex::new(None),
            devices: AtomicU64::new(0),
        });
        let (open_tx, open_rx) = unbounded::<(u64, TrackRequest)>();
        let (opened_tx, opened_rx) = unbounded::<Opened>();
        // Files still being written are opened and decoded on threads of their own, which
        // signal here when they have something new (one pending signal is enough).
        let feed_wake = bounded::<()>(1);
        let opener = thread::Builder::new()
            .name("mp-open".into())
            .spawn(move || {
                // Ends when the decode thread (the only sender) is gone.
                while let Ok(mut asked) = open_rx.recv() {
                    // Only the last request is waited for: those before it are not opened.
                    while let Ok(newer) = open_rx.try_recv() {
                        asked = newer;
                    }
                    let (id, req) = asked;
                    let result = Source::open(&req.path).map_err(|e| e.to_string());
                    if opened_tx.send(Opened { id, req, result }).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn open thread");

        let dec = {
            let shared = shared.clone();
            let ev = ev_tx.clone();
            thread::Builder::new()
                .name("mp-decode".into())
                .spawn(move || Decoder::new(shared, chunk_tx, ev, open_tx, opened_rx, feed_wake).run(cmd_rx))
                .expect("spawn decode thread")
        };
        let out = {
            let shared = shared.clone();
            let cmd = cmd_tx.clone();
            thread::Builder::new()
                .name("mp-output".into())
                .spawn(move || output::run(make_device(), chunk_rx, wake_rx, shared, ev_tx, cmd))
                .expect("spawn output thread")
        };
        (Self { cmd: cmd_tx, shared, threads: vec![dec, out], opener: Some(opener) }, ev_rx)
    }

    /// Play this track now, dropping whatever is buffered.
    pub fn play(&self, req: TrackRequest) {
        self.play_from(req, 0.0);
    }

    /// Play this track now from `seconds` in (a resumed session). The file is positioned
    /// before anything is decoded, so its start is never heard.
    pub fn play_from(&self, req: TrackRequest, seconds: f64) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let _ = self.cmd.send(Cmd::Play(req, seconds.max(0.0), None));
        self.shared.wake_output();
    }

    /// Play this track now, crossfading from what is playing over up to `secs` (a manual
    /// skip with crossfade on). What is playing carries on from the very sample being
    /// heard and fades out under the new track's fade-in, both equal power; the fade is no
    /// longer than what is already decoded of it (about a second at 44.1 and 48 kHz, a
    /// quarter at 192). A new track at another sample rate is converted to the rate playing,
    /// until the next pause, seek or stop (the module doc). Paused or stopped: as `play`.
    pub fn play_crossfade(&self, req: TrackRequest, secs: f32) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let fade = (secs.is_finite() && secs > 0.0).then_some(secs);
        let _ = self.cmd.send(Cmd::Play(req, 0.0, fade));
        self.shared.wake_output();
    }

    /// The track to continue with, gaplessly, when the current one ends.
    pub fn set_next(&self, req: Option<TrackRequest>) {
        self.set_next_with(req, Transition::Gapless);
    }

    /// The track to continue with when the current one ends, and how.
    pub fn set_next_with(&self, req: Option<TrackRequest>, how: Transition) {
        let _ = self.cmd.send(Cmd::SetNext(req, how));
    }

    /// The equalizer. Takes effect at once on the output thread (what is already in the
    /// device, about 40 ms, plays as it was), gliding to the new setting (`eq.rs`). Paused
    /// or stopped, nothing wakes for it: the next write takes it.
    pub fn set_eq(&self, params: &EqParams) {
        let e = &self.shared.eq;
        *e.params.lock().unwrap() = *params;
        e.enabled.store(params.enabled, Ordering::Relaxed);
        e.seq.fetch_add(1, Ordering::Release);
    }

    /// Seek within the audible track.
    pub fn seek(&self, seconds: f64) {
        let token = self.status().token;
        *self.shared.seek_trace.lock().unwrap() = SeekTrace { requested: Some(Instant::now()), ..SeekTrace::default() };
        let _ = self.cmd.send(Cmd::Seek { token, secs: seconds.max(0.0) });
    }

    pub fn stop(&self) {
        let _ = self.cmd.send(Cmd::Stop);
        self.shared.wake_output();
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        self.shared.wake_output();
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// Linear volume, 0..=1. Applied at the output, so it takes effect immediately.
    pub fn set_volume(&self, v: f32) {
        self.shared.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Applies to the current track from the next decoded chunk (~1 s of buffered
    /// audio still plays at the old gain).
    pub fn set_gain_config(&self, cfg: GainConfig) {
        let _ = self.cmd.send(Cmd::Gain(cfg));
    }

    /// The system's devices or its default changed (`output::on_devices_changed`): the
    /// output looks again at where its stream plays, and says so (`Event::Output`) if that
    /// changed, also while paused or stopped.
    pub fn devices_changed(&self) {
        self.shared.devices.fetch_add(1, Ordering::AcqRel);
        self.shared.wake_output();
    }

    /// Play through this device, in this mode. While playing, what is buffered in the
    /// device plays out and playback continues on the new one from the same frame.
    pub fn set_output(&self, cfg: OutputConfig) {
        {
            let mut o = self.shared.output.lock().unwrap();
            if o.1 == cfg && o.0 > 0 {
                return;
            }
            o.0 += 1;
            o.1 = cfg;
        }
        self.shared.wake_output();
    }

    /// What the decoder found in the track with this token, once it has been opened (it
    /// is when its `TrackStarted` arrives). `None` for a track not opened (lately).
    pub fn stream_info(&self, token: u64) -> Option<StreamInfo> {
        self.shared.streams.lock().unwrap().iter().find(|(t, _)| *t == token).map(|(_, s)| s.clone())
    }

    /// What the output last opened (`None` before anything played).
    pub fn output_info(&self) -> Option<OutputInfo> {
        self.shared.output_info.lock().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// The samples being played, for a visualizer: after ReplayGain, before volume. It
    /// costs the output thread nothing until `set_active(true)`.
    pub fn spectrum(&self) -> Arc<crate::spectrum::Tap> {
        self.shared.spectrum.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        let _ = self.cmd.send(Cmd::Shutdown);
        self.shared.wake_output();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        // It ends at its next request or open, the decode thread gone; one still opening is
        // let go after a while (it ends when the open does).
        if let Some(t) = self.opener.take() {
            let deadline = Instant::now() + Duration::from_secs(1);
            while !t.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

struct Playing {
    src: Stream,
    req: TrackRequest,
    token: u64,
    gain: f32,
    /// Decoded frames not handed out yet: a crossfade starts at an exact frame, so a chunk
    /// may end inside a packet.
    carry: Vec<f32>,
    /// The file has no more packets (`carry` may still hold some).
    done: bool,
    /// How it was handed over as the next track.
    via: Transition,
    /// Unique among every track opened (a converter carried on into the next track names it).
    id: u64,
    /// Nothing has been read yet.
    fresh: bool,
    /// Converted to another rate: the incoming track of a crossfade across rates, from its
    /// first frame (see the module doc).
    conv: Option<Conv>,
    /// Decoded frames on their way into `conv`.
    scratch: Vec<f32>,
    /// The last read ran dry: a growing file has not decoded that far yet. More comes later.
    starved: bool,
}

/// A track's conversion to the rate playing.
struct Conv {
    rs: Resampler,
    /// The converter's input frame at which this track starts: 0, or where the track before
    /// ended when the converter carries on across a gapless boundary (one signal through one
    /// converter, as one signal through the device). `out0` is the output frame there.
    in0: u64,
    out0: u64,
    /// The converter's input and output frame at which this track ends, once its input has
    /// all gone in.
    end: Option<(u64, u64)>,
    /// The track (`Playing::id`) whose first frames went in after this one's last, as their
    /// lookahead: it carries on through this converter.
    into: Option<u64>,
}

impl Conv {
    fn new(f: Arc<Filter>) -> Self {
        Conv { rs: Resampler::new(f), in0: 0, out0: 0, end: None, into: None }
    }
}

static TRACK_IDS: AtomicU64 = AtomicU64::new(1);

impl Playing {
    fn new(src: Stream, req: TrackRequest, gain: f32, via: Transition) -> Self {
        Playing {
            src,
            token: req.token,
            req,
            gain,
            carry: Vec::new(),
            done: false,
            via,
            id: TRACK_IDS.fetch_add(1, Ordering::Relaxed),
            fresh: true,
            conv: None,
            scratch: Vec::new(),
            starved: false,
        }
    }

    /// `frames` more can be handed out (at the rate it is handed out at) without running dry:
    /// always for a file on disk. A converted track's input is counted with the converter's
    /// lookahead and some slack.
    fn ready(&self, frames: usize) -> bool {
        if self.done {
            return true;
        }
        let need = match &self.conv {
            None => frames,
            Some(c) => c.rs.filter().input_frames(frames as u64) as usize + c.rs.filter().lookahead() + 2 * resample::BLOCK,
        };
        self.src.ready(need.saturating_sub(self.carry.len() / 2))
    }

    /// Hand out nothing more: the rest is silence (a fade-out is over).
    fn cut_short(&mut self) {
        self.done = true;
        self.carry.clear();
        if let Some(c) = self.conv.as_mut() {
            c.end = Some((c.rs.pushed(), c.rs.produced()));
        }
    }

    /// The content frame of the next frame `read` hands out.
    fn pos(&self) -> u64 {
        self.src.position() - (self.carry.len() / 2) as u64
    }

    /// Every decoded frame has been handed out.
    fn exhausted(&self) -> bool {
        self.done && self.carry.is_empty()
    }

    /// A seek plays the track at its own rate again.
    fn seek(&mut self, frame: u64) -> Result<(), crate::source::SourceError> {
        self.carry.clear();
        self.done = false;
        self.conv = None;
        self.fresh = frame == 0;
        self.src.seek(frame)
    }

    /// The rate it is handed out at: its own, or the one it is converted to.
    fn rate(&self) -> u32 {
        self.conv.as_ref().map_or(self.src.sample_rate(), |c| c.rs.filter().to_rate())
    }

    fn converted_from(&self) -> Option<u32> {
        self.conv.as_ref().map(|_| self.src.sample_rate())
    }

    /// Convert it to `f`'s rate from its first frame.
    fn convert(&mut self, f: Option<Arc<Filter>>) {
        debug_assert!(self.fresh || f.is_none());
        self.conv = f.map(Conv::new);
    }

    /// `pos`, at the rate it is handed out at.
    fn pos_out(&self) -> u64 {
        match &self.conv {
            Some(c) => c.rs.produced() - c.out0,
            None => self.pos(),
        }
    }

    /// Its length in frames at the rate it is handed out at, if known.
    fn total_out(&self) -> Option<u64> {
        match &self.conv {
            None => self.src.total_frames(),
            Some(c) => match c.end {
                Some((_, out)) => Some(out - c.out0),
                None => self.src.total_frames().map(|t| c.rs.filter().output_frames(c.in0 + t) - c.out0),
            },
        }
    }

    /// Converted, and within a chunk of the end of its input: the track after it should be
    /// open, in case it carries on through this converter.
    fn near_end(&self) -> bool {
        let Some(c) = &self.conv else { return false };
        if self.exhausted() {
            return true;
        }
        let need = c.rs.filter().input_frames(CHUNK_FRAMES as u64) + resample::BLOCK as u64;
        self.src.total_frames().is_some_and(|t| t.saturating_sub(self.pos()) <= need)
    }

    /// `next` follows this converted track gaplessly at its own rate, and has not been read:
    /// its first frames can be the lookahead of this one's last, and it carries on through
    /// the same converter.
    fn continues_into(&self, next: &Playing) -> bool {
        let Some(c) = &self.conv else { return false };
        c.into == Some(next.id) || (!c.rs.is_finished() && next.fresh && next.conv.is_none() && next.src.sample_rate() == self.src.sample_rate())
    }

    /// `read` at the rate it is handed out at: through the converter when it has one (then
    /// always exactly `frames`, fewer only at the end). `cont`: the track that carries on
    /// through the converter, if it does (`continues_into`).
    fn read_out(&mut self, out: &mut Vec<f32>, frames: usize, exact: bool, events: &Sender<Event>, cont: Option<&mut Playing>) -> bool {
        let Some(mut c) = self.conv.take() else { return self.read(out, frames, exact, events) };
        let eof = self.read_converted(&mut c, out, frames, events, cont);
        self.conv = Some(c);
        eof
    }

    fn read_converted(&mut self, c: &mut Conv, out: &mut Vec<f32>, frames: usize, events: &Sender<Event>, mut cont: Option<&mut Playing>) -> bool {
        let target = out.len() + frames * 2;
        loop {
            let end = c.end.map_or(u64::MAX, |(_, out)| out);
            if c.rs.produced() >= end {
                return true;
            }
            if out.len() >= target {
                return false;
            }
            let max = (((target - out.len()) / 2) as u64).min(end - c.rs.produced());
            if c.rs.produce(out, max as usize) > 0 {
                continue;
            }
            // The converter needs more input: this track's, then the lookahead past its end.
            if !self.exhausted() {
                let mut block = std::mem::take(&mut self.scratch);
                block.clear();
                let eof = self.read(&mut block, resample::BLOCK, true, events);
                c.rs.push(&block);
                self.scratch = block;
                if eof {
                    c.end = Some((c.rs.pushed(), c.rs.filter().output_frames(c.rs.pushed())));
                }
                if self.starved {
                    // A growing file not decoded further yet: fewer frames now.
                    return false;
                }
            } else if c.end.is_none() {
                c.end = Some((c.rs.pushed(), c.rs.filter().output_frames(c.rs.pushed())));
            } else if let Some(next) = cont.as_deref_mut().filter(|n| !c.rs.is_finished() && !n.exhausted()) {
                let mut block = std::mem::take(&mut next.scratch);
                block.clear();
                next.read(&mut block, resample::BLOCK, true, events);
                c.rs.push(&block);
                next.scratch = block;
                c.into = Some(next.id);
                if next.starved {
                    return false;
                }
            } else {
                // Silence after the end (the queue ends, or the next track is mixed in, or at
                // another rate).
                c.rs.finish();
            }
        }
    }

    /// Append decoded frames to `out`: at least `frames` (a packet's worth more, as the
    /// decoder hands them out) or, `exact`, exactly `frames`, keeping the rest for the next
    /// read. Fewer only at the end, or when a growing file runs dry (`starved`). Returns true
    /// at the end of the file.
    fn read(&mut self, out: &mut Vec<f32>, frames: usize, exact: bool, events: &Sender<Event>) -> bool {
        self.fresh = false;
        self.starved = false;
        let target = frames * 2;
        if !self.carry.is_empty() {
            let n = self.carry.len().min(target.saturating_sub(out.len()));
            out.extend(self.carry.drain(..n));
        }
        while out.len() < target && !self.done {
            match self.src.next_chunk(out) {
                Ok(Pull::More) => {}
                Ok(Pull::End) => self.done = true,
                Ok(Pull::Dry) => {
                    self.starved = true;
                    break;
                }
                Err(e) => {
                    let _ = events.send(Event::Error { token: self.token, message: e.to_string() });
                    self.done = true;
                }
            }
        }
        if exact && out.len() > target {
            // Only when the carry was used up first, so the order holds.
            debug_assert!(self.carry.is_empty());
            self.carry.extend(out.drain(target..));
        }
        self.done && self.carry.is_empty()
    }
}

/// Where the coming crossfade starts in the current track, how long it is, and whether it
/// is only a fade-out (the next track is at a rate there is no converter to this one for).
#[derive(Clone, Copy, Debug)]
struct Plan {
    start: u64,
    len: u64,
    fade_only: bool,
}

/// A crossfade under way: the current track (outgoing) is mixed with `incoming`.
struct Xfade {
    /// `None`: a fade-out only (a rate follows that there is no converter for).
    incoming: Option<Playing>,
    len: u64,
    pos: u64,
    /// Chunks before this frame of the overlap carry the outgoing track's token and
    /// position, from it the incoming one's: the "current" track changes in the middle,
    /// when the incoming one becomes the louder.
    mid: u64,
    out_done: bool,
    in_done: bool,
}

/// A `play` of a file still being written, opening on its own thread: where to start it,
/// and the rate a skip's crossfade would mix at (`Cmd::Play`).
struct Starting {
    opening: Opening,
    req: TrackRequest,
    from: f64,
    mix_rate: u32,
}

/// A `set_next` track, opened on the `mp-open` thread.
struct Opened {
    id: u64,
    req: TrackRequest,
    result: Result<Source, String>,
}

struct Decoder {
    shared: Arc<Shared>,
    out: Sender<Msg>,
    events: Sender<Event>,
    gain_cfg: GainConfig,
    generation: u64,
    current: Option<Playing>,
    next: Option<Playing>,
    /// Opens `set_next` tracks off this thread; results come back on `opened`.
    open: Sender<(u64, TrackRequest)>,
    opened: Receiver<Opened>,
    /// The open asked for last, not back yet. Results for any other id are stale.
    pending: Option<u64>,
    /// A `set_next` track still being written, opening on its own thread (in place of
    /// `pending`). Never waited for: `produce` is starved until it is open.
    opening: Option<(Opening, TrackRequest)>,
    /// A `play` of a track still being written, opening (`current` is empty meanwhile).
    starting: Option<Starting>,
    /// Signalled by the threads of growing files when they have something new.
    feed_wake: (Sender<()>, Receiver<()>),
    /// `produce` could not go on: the frames of the growing file of this token are not decoded.
    starved: Option<u64>,
    /// `produce` could not go on: the next track, a local file, is still opening on `mp-open`
    /// (waited for as `starved` is, but not said: a local open is not buffering).
    awaiting_open: bool,
    /// What `Shared::starved` says now.
    starved_said: Option<(u64, u64)>,
    open_seq: u64,
    /// How the next (or pending) track follows the current one.
    next_via: Transition,
    plan: Option<Plan>,
    xfade: Option<Xfade>,
    /// The incoming track's frames, while mixing.
    mixbuf: Vec<f32>,
    /// The track decoded before `current`. Near a boundary the decoder is already in
    /// the next file while this one is still audible; a seek may target it.
    prev: Option<TrackRequest>,
    /// Set once EndOfQueue went out for this generation.
    idle: bool,
    buf: Vec<f32>,
    limiter: Limiter,
    /// Converters' tables built lately (`resample.rs`).
    filters: Filters,
}

/// Where a seek goes.
#[derive(Clone, Copy, Debug)]
enum SeekTo {
    Secs(f64),
    /// A frame counted at this rate (a converted stream's): the same moment at the track's
    /// own rate.
    Frame { frame: u64, rate: u32 },
}

impl SeekTo {
    fn frame(self, rate: u32) -> u64 {
        match self {
            SeekTo::Secs(s) => (s * f64::from(rate)) as u64,
            SeekTo::Frame { frame, rate: at } => ((u128::from(frame) * u128::from(rate) + u128::from(at) / 2) / u128::from(at.max(1))) as u64,
        }
    }
}

impl Decoder {
    fn new(
        shared: Arc<Shared>,
        out: Sender<Msg>,
        events: Sender<Event>,
        open: Sender<(u64, TrackRequest)>,
        opened: Receiver<Opened>,
        feed_wake: (Sender<()>, Receiver<()>),
    ) -> Self {
        Self {
            shared,
            out,
            events,
            gain_cfg: GainConfig::default(),
            generation: 0,
            current: None,
            next: None,
            open,
            opened,
            pending: None,
            opening: None,
            starting: None,
            feed_wake,
            starved: None,
            awaiting_open: false,
            starved_said: None,
            open_seq: 0,
            next_via: Transition::Gapless,
            plan: None,
            xfade: None,
            mixbuf: Vec::new(),
            prev: None,
            idle: true,
            buf: Vec::with_capacity(CHUNK_FRAMES * 2 * 2),
            limiter: Limiter::default(),
            filters: Filters::default(),
        }
    }

    /// Remember what the decoder found in a track just opened (`Player::stream_info`).
    fn remember(&self, token: u64, src: &Stream) {
        let info = src.stream_info();
        let mut s = self.shared.streams.lock().unwrap();
        s.retain(|(t, _)| *t != token);
        if s.len() >= STREAMS_KEPT {
            s.remove(0);
        }
        s.push((token, info));
    }

    /// Open a track now, on this thread. One still being written opens on its own, waited for
    /// here only up to `REOPEN_WAIT` (this reopens the track just played, all of which is on
    /// disk; a `play` of one goes through `starting` instead).
    fn open(&self, req: TrackRequest) -> Option<Playing> {
        let result = match req.still_growing() {
            None => Source::open(&req.path).map(Stream::Local).map_err(|e| e.to_string()),
            Some(feed) => Opening::spawn(&req.path, feed, self.feed_wake.0.clone())
                .wait(REOPEN_WAIT)
                .unwrap_or_else(|| Err("its bytes are not on disk yet".into()))
                .map(Stream::Feed),
        };
        self.opened(req, result, Transition::Gapless)
    }

    /// A track opened, or not: remember what it is and make it playable, or say why not.
    fn opened(&self, req: TrackRequest, result: Result<Stream, String>, via: Transition) -> Option<Playing> {
        match result {
            Ok(src) => {
                self.remember(req.token, &src);
                let gain = linear_gain(src.gain_tags(), &self.gain_cfg);
                Some(Playing::new(src, req, gain, via))
            }
            Err(e) => {
                let _ = self.events.send(Event::Error { token: req.token, message: format!("{}: {e}", req.path.display()) });
                None
            }
        }
    }

    /// Ask for `req` to be opened as the next track, off this thread. Whatever was asked for
    /// before is let go (a growing file's thread is cancelled and joined).
    fn open_next(&mut self, req: Option<TrackRequest>, via: Transition) {
        self.next = None;
        self.pending = None;
        self.opening = None;
        self.plan = None;
        self.next_via = via;
        if let Some(req) = req {
            match req.still_growing().map(|f| Opening::spawn(&req.path, f, self.feed_wake.0.clone())) {
                Some(o) => self.opening = Some((o, req)),
                None => {
                    self.open_seq += 1;
                    self.pending = Some(self.open_seq);
                    let _ = self.open.send((self.open_seq, req));
                }
            }
        }
    }

    /// A next track that finished opening: keep it if it is the one asked for last.
    fn accept_opened(&mut self, o: Opened) {
        if self.pending != Some(o.id) {
            return;
        }
        self.pending = None;
        self.next = self.opened(o.req, o.result.map(Stream::Local), self.next_via);
    }

    /// Take next tracks that have finished opening, and a growing `play` that has, without
    /// waiting.
    fn poll_opened(&mut self) {
        while let Ok(o) = self.opened.try_recv() {
            self.accept_opened(o);
        }
        self.poll_opening();
        if let Some(s) = self.starting.as_mut()
            && let Some(result) = s.opening.poll()
        {
            let s = self.starting.take().unwrap();
            let cur = self.opened(s.req, result.map(Stream::Feed), Transition::Gapless);
            self.begin(cur, s.from, s.mix_rate);
        }
    }

    /// A growing next track that has finished opening becomes `next`.
    fn poll_opening(&mut self) {
        if let Some((o, _)) = self.opening.as_mut()
            && let Some(result) = o.poll()
        {
            let (_, req) = self.opening.take().unwrap();
            self.next = self.opened(req, result.map(Stream::Feed), self.next_via);
        }
    }

    /// The current track ran out, or nearly: whether its successor has finished opening,
    /// without waiting. False while it is still opening, on `mp-open` or on its own thread: the
    /// caller goes no further (the output plays the buffered audio meanwhile), and `run` waits
    /// for the open or a command (`awaiting_open`, or `starved` for a growing file).
    fn next_opened(&mut self) -> bool {
        while self.pending.is_some() {
            match self.opened.try_recv() {
                Ok(o) => self.accept_opened(o),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        self.poll_opening();
        self.pending.is_none() && self.opening.is_none()
    }

    /// The token of the growing next track still opening.
    fn opening_token(&self) -> Option<u64> {
        self.opening.as_ref().map(|(_, r)| r.token)
    }

    /// Tell the output whether this thread is starved, when that changes (it says
    /// `Event::Buffering` once what is queued has played).
    fn say_starved(&mut self) {
        let now = self.starved.map(|t| (self.generation, t));
        if now != self.starved_said {
            self.starved_said = now;
            *self.shared.starved.lock().unwrap() = now;
            self.shared.wake_output();
        }
    }

    /// Starved: wait for a growing file's thread to have something new, a command, or a
    /// local next track opened on `mp-open` (it is remembered for `stream_info` at once, as
    /// while playing). Returns false on shutdown.
    fn wait_feed(&mut self, cmds: &Receiver<Cmd>) -> bool {
        let (wake, opened) = (self.feed_wake.1.clone(), self.opened.clone());
        crossbeam_channel::select! {
            recv(cmds) -> cmd => match cmd {
                Ok(cmd) => self.handle(cmd),
                Err(_) => false,
            },
            recv(wake) -> _ => true,
            recv(opened) -> o => {
                if let Ok(o) = o {
                    self.accept_opened(o);
                }
                true
            }
        }
    }

    /// Start playing `cur`, just opened for a `play`: from `from` seconds in, converted to
    /// `mix_rate` for a skip's crossfade from a track at another rate (0: not). Nothing to
    /// play: the queue ends, so the app moves on.
    fn begin(&mut self, cur: Option<Playing>, from: f64, mix_rate: u32) {
        self.current = cur;
        if from > 0.0
            && let Some(cur) = self.current.as_mut()
        {
            let frame = (from * f64::from(cur.src.sample_rate())) as u64;
            let frame = cur.src.total_frames().map_or(frame, |t| frame.min(t));
            if let Err(e) = cur.seek(frame) {
                log::warn!("starting at {from:.1} s: {e}");
            }
        }
        // A skip's crossfade into a track at another rate: converted to the rate playing, so
        // the output can mix the two (no filter for the pair: the output fades out and
        // reopens, as before).
        let native = self.current.as_ref().map(|c| c.src.sample_rate());
        if mix_rate != 0
            && let Some(native) = native.filter(|&r| r != mix_rate)
        {
            let f = self.filters.get(native, mix_rate);
            if let Some(cur) = self.current.as_mut().filter(|c| c.fresh) {
                cur.convert(f);
            }
        }
        if self.current.is_none() {
            // No longer waiting for it (said before the end, or the output would see a wait
            // after it).
            self.starved = None;
            self.say_starved();
            // Let the app move on: same as a track that ended immediately.
            let _ = self.out.send(Msg::EndOfQueue { generation: self.generation });
            self.idle = true;
        }
    }

    fn new_generation(&mut self) {
        self.generation += 1;
        self.shared.generation.store(self.generation, Ordering::Release);
        self.buf.clear();
        self.idle = false;
        self.limiter.reset();
        self.plan = None;
        self.xfade = None;
        self.starved = None;
        self.say_starved();
    }

    /// Returns false on shutdown.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Play(req, from, fade) => {
                // The rate the output would crossfade at (0: it would not).
                let mut mix_rate = 0;
                if let Some(secs) = fade {
                    // The output thread crossfades into this generation (output.rs).
                    self.shared.soft_secs.store(secs.to_bits(), Ordering::Relaxed);
                    self.shared.soft_gen.store(self.generation + 1, Ordering::Relaxed);
                    mix_rate = self.shared.soft_rate.load(Ordering::Relaxed);
                }
                self.new_generation();
                self.current = None;
                self.starting = None;
                self.open_next(None, Transition::Gapless);
                self.prev = None;
                // A file still being written opens on its own thread; this one goes on taking
                // commands meanwhile (`run`), and begins it once it is open.
                match req.still_growing().map(|f| Opening::spawn(&req.path, f, self.feed_wake.0.clone())) {
                    Some(opening) => self.starting = Some(Starting { opening, req, from, mix_rate }),
                    None => {
                        let cur = self.open(req);
                        self.begin(cur, from, mix_rate);
                    }
                }
            }
            Cmd::SetNext(req, via) => self.open_next(req, via),
            Cmd::Seek { token, secs } => self.seek(token, SeekTo::Secs(secs)),
            Cmd::Native { generation, token, frame, rate } => {
                // Only for the track it names, wherever the decoder is with it (still current,
                // being mixed in, or already finished: `seek` handles each).
                let known = self.current.as_ref().is_some_and(|c| c.token == token)
                    || self.prev.as_ref().is_some_and(|r| r.token == token)
                    || self.xfade.as_ref().and_then(|x| x.incoming.as_ref()).is_some_and(|i| i.token == token);
                if generation == self.generation && known {
                    log::debug!("paused in a converted track: playing on at its own rate");
                    self.seek(Some(token), SeekTo::Frame { frame, rate });
                }
            }
            Cmd::Stop => {
                self.new_generation();
                self.shared.stopped.store(self.generation, Ordering::Release);
                self.current = None;
                self.starting = None;
                self.open_next(None, Transition::Gapless);
                self.idle = true;
                let mut st = self.shared.status.lock().unwrap();
                *st = Status::default();
            }
            Cmd::Gain(cfg) => {
                self.gain_cfg = cfg;
                let incoming = self.xfade.as_mut().and_then(|x| x.incoming.as_mut());
                for p in self.current.iter_mut().chain(self.next.iter_mut()).chain(incoming) {
                    p.gain = linear_gain(p.src.gain_tags(), &cfg);
                }
            }
            Cmd::Shutdown => return false,
        }
        true
    }

    /// A seek plays every track at its own rate again (`Playing::seek`).
    fn seek(&mut self, token: Option<u64>, to: SeekTo) {
        self.plan = None;
        if let Some(x) = self.xfade.take()
            && let Some(mut incoming) = x.incoming
        {
            if token == Some(incoming.token) {
                // Past the middle of a crossfade the incoming track is the audible one: it
                // goes on alone from where it is sought to.
                if let Some(out) = self.current.replace(incoming) {
                    self.prev = Some(out.req);
                }
            } else if incoming.seek(0).is_ok() {
                // Seeking in the outgoing one: the incoming track waits again, from its
                // start, and the crossfade comes round again at the end.
                self.next_via = incoming.via;
                self.pending = None;
                self.opening = None;
                self.next = Some(incoming);
            }
        }
        let cur_token = self.current.as_ref().map(|c| c.token);
        if token.is_some() && token != cur_token && token == self.prev.as_ref().map(|r| r.token) {
            // The audible track is the one the decoder already finished: reopen it and
            // queue the following track again from its start.
            let req = self.prev.take().unwrap();
            let Some(reopened) = self.open(req) else { return };
            if let Some(mut following) = self.current.take()
                && following.seek(0).is_ok()
            {
                self.open_next(None, following.via);
                self.next = Some(following);
            } else {
                self.open_next(None, Transition::Gapless);
            }
            self.current = Some(reopened);
        }
        let Some(cur) = self.current.as_mut() else { return };
        let frame = to.frame(cur.src.sample_rate());
        let frame = cur.src.total_frames().map_or(frame, |t| frame.min(t));
        if let Err(e) = cur.seek(frame) {
            log::warn!("seek failed: {e}");
        }
        // Its converter, gone with the seek, may have taken the next track's first frames as
        // lookahead: that track starts again from its start.
        if let Some(next) = self.next.as_mut().filter(|n| !n.fresh)
            && let Err(e) = next.seek(0)
        {
            log::warn!("rewinding the next track failed: {e}");
        }
        self.new_generation();
        let mut t = self.shared.seek_trace.lock().unwrap();
        if let Some(at) = t.requested {
            t.generation = self.generation;
            t.positioned = Some(at.elapsed());
        }
    }

    /// Limit peaks here, on the decode thread, unless the equalizer is on: then the output
    /// thread does, after it (`Chunk::limited`).
    fn limits_here(&self) -> bool {
        self.gain_cfg.limit_peaks && !self.shared.eq.enabled.load(Ordering::Relaxed)
    }

    /// How many frames of the current track to decode before its crossfade starts (`Some(0)`:
    /// now), once it is near; `None`: no crossfade coming, or not near yet. Near the start,
    /// it waits for the next track to finish opening (the output plays what is buffered).
    fn overlap_ahead(&mut self) -> Option<u64> {
        let Transition::Crossfade { secs } = self.next_via else { return None };
        if self.next.is_none() && self.pending.is_none() && self.opening.is_none() {
            return None;
        }
        let cur = self.current.as_ref()?;
        // Frames at the rate playing (the track's own, or the one it is converted to).
        let total = cur.total_out()?;
        let pos = cur.pos_out();
        if let Some(p) = &self.plan {
            return Some(p.start.saturating_sub(pos));
        }
        let rate = cur.rate();
        let remaining = total.saturating_sub(pos);
        let longest = crossfade::overlap_frames(f64::from(secs), rate, Some(total), remaining, None)?;
        // A chunk is at most a few packets past CHUNK_FRAMES: until within reach of the
        // longest overlap, decode as usual.
        if remaining > longest + 4 * CHUNK_FRAMES as u64 {
            return None;
        }
        // A growing next track not open yet, or with nothing decoded: no plan yet. The
        // crossfade starts later and shorter, or not at all (`overlap_frames` on what remains).
        // A local next track still opening: decoding stops here until it is open (`produce`).
        if !self.next_opened() {
            self.awaiting_open = self.pending.is_some();
            return None;
        }
        if !self.next.as_ref()?.ready(CHUNK_FRAMES) {
            return None;
        }
        let next_rate = self.next.as_ref()?.src.sample_rate();
        // At another rate the incoming track is converted to this one; with no converter for
        // the pair, the outgoing track only fades out (and the device reopens).
        let filter = (next_rate != rate).then(|| self.filters.get(next_rate, rate)).flatten();
        let fade_only = next_rate != rate && filter.is_none();
        let next = self.next.as_mut()?;
        let in_total = match &filter {
            _ if fade_only => None,
            Some(f) => next.src.total_frames().map(|t| f.output_frames(t)),
            None => next.src.total_frames(),
        };
        let len = crossfade::overlap_frames(f64::from(secs), rate, Some(total), remaining, in_total)?;
        let start = total - len;
        self.plan = Some(Plan { start, len, fade_only });
        if next.fresh {
            next.convert(filter);
        }
        Some(start.saturating_sub(pos))
    }

    fn start_crossfade(&mut self) {
        let Some(plan) = self.plan.take() else { return };
        let incoming = if plan.fade_only { None } else { self.next.take() };
        if !plan.fade_only && incoming.is_none() {
            return;
        }
        self.xfade = Some(Xfade { incoming, len: plan.len, pos: 0, mid: plan.len / 2, out_done: false, in_done: false });
    }

    /// The current track ran out (or its fade-out ended): on to the next, in the same
    /// stream.
    fn hand_over(&mut self) {
        if !self.next_opened() {
            // A successor still opening: this track stays, finished, until it is open
            // (`produce` waits meanwhile).
            return;
        }
        let old = self.current.take();
        self.current = self.next.take();
        self.plan = None;
        let Some(mut old) = old else {
            self.prev = None;
            return;
        };
        let (rate, own) = (old.rate(), old.src.sample_rate());
        let conv = old.conv.take();
        self.prev = Some(old.req);
        let Some(cur) = self.current.as_mut() else { return };
        // Gapless after a converted track: one at the rate playing plays as it is; one at the
        // converted track's own rate carries on through its converter (or a new one), so a
        // run of tracks at one rate stays gapless; one at a third rate reopens the device.
        match conv {
            Some(mut c) if c.into == Some(cur.id) => {
                let (in_end, out_end) = c.end.expect("the converted track ran out");
                c.in0 = in_end;
                c.out0 = out_end;
                c.into = None;
                c.end = cur.exhausted().then(|| (c.rs.pushed(), c.rs.filter().output_frames(c.rs.pushed())));
                cur.conv = Some(c);
            }
            Some(_) if cur.fresh && cur.src.sample_rate() == own => {
                let f = self.filters.get(own, rate);
                cur.convert(f);
            }
            _ if cur.fresh => cur.convert(None),
            _ => {}
        }
    }

    /// One chunk of a crossfade: the outgoing track (`current`) and the incoming one, each
    /// times its own gain and its curve, summed; or the outgoing alone, fading out.
    fn produce_mix(&mut self) -> Option<Msg> {
        let x = self.xfade.as_mut()?;
        let cur = self.current.as_mut()?;
        let left = x.len - x.pos;
        let mut n = CHUNK_FRAMES.min(left as usize);
        if x.incoming.is_some() && x.pos < x.mid {
            n = n.min((x.mid - x.pos) as usize);
        }
        // Both tracks' frames must be decoded: a growing one not that far yet is waited for.
        if !x.out_done && !cur.ready(n) {
            self.starved = Some(cur.token);
            return None;
        }
        if let Some(inc) = x.incoming.as_ref().filter(|i| !x.in_done && !i.ready(n)) {
            self.starved = Some(inc.token);
            return None;
        }
        // Both at the rate playing: the outgoing track's own (or its conversion), the
        // incoming one converted to it when it is at another.
        let out_start = cur.pos_out();
        self.buf.clear();
        if !x.out_done {
            x.out_done = cur.read_out(&mut self.buf, n, true, &self.events, None);
        }
        // An outgoing track shorter than its header said is silent for the rest.
        self.buf.resize(n * 2, 0.0);
        let go = cur.gain;
        let mut label = (cur.token, cur.rate(), out_start, cur.total_out(), cur.converted_from());
        if let Some(inc) = x.incoming.as_mut() {
            let in_start = inc.pos_out();
            self.mixbuf.clear();
            if !x.in_done {
                x.in_done = inc.read_out(&mut self.mixbuf, n, true, &self.events, None);
            }
            self.mixbuf.resize(n * 2, 0.0);
            let gi = inc.gain;
            for (i, (o, m)) in self.buf.as_chunks_mut::<2>().0.iter_mut().zip(self.mixbuf.as_chunks::<2>().0).enumerate() {
                let (wo, wi) = crossfade::gains(x.pos + i as u64, x.len);
                o[0] = o[0] * go * wo + m[0] * gi * wi;
                o[1] = o[1] * go * wo + m[1] * gi * wi;
            }
            if x.pos >= x.mid {
                label = (inc.token, inc.rate(), in_start, inc.total_out(), inc.converted_from());
            }
        } else {
            for (i, o) in self.buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let (wo, _) = crossfade::gains(x.pos + i as u64, x.len);
                o[0] *= go * wo;
                o[1] *= go * wo;
            }
        }
        x.pos += n as u64;
        let finished = x.pos >= x.len;
        let limited = self.limits_here();
        if limited {
            self.limiter.process(&mut self.buf, 1.0, label.1);
        }
        let chunk = Chunk {
            generation: self.generation,
            token: label.0,
            rate: label.1,
            start: label.2,
            duration: label.3,
            samples: std::mem::take(&mut self.buf),
            gain: 1.0,
            limit: self.gain_cfg.limit_peaks,
            limited,
            converted_from: label.4,
            mix: true,
        };
        if finished {
            let x = self.xfade.take().unwrap();
            match x.incoming {
                Some(inc) => {
                    self.prev = self.current.replace(inc).map(|p| p.req);
                    self.plan = None;
                }
                // The fade-out is over: whatever is left of the track is silent.
                None => {
                    if let Some(c) = self.current.as_mut() {
                        c.cut_short();
                    }
                    self.hand_over();
                }
            }
        }
        Some(Msg::Audio(chunk))
    }

    /// Decode until one chunk is ready. `None` when the queue ran dry, or when a growing file
    /// has nothing more decoded yet (`starved` says which).
    fn produce(&mut self) -> Option<Msg> {
        self.starved = None;
        self.awaiting_open = false;
        loop {
            if self.xfade.is_some() {
                return self.produce_mix();
            }
            self.current.as_ref()?;
            let ahead = self.overlap_ahead();
            if self.awaiting_open {
                return None;
            }
            if ahead == Some(0) {
                self.start_crossfade();
                if self.xfade.is_some() {
                    continue;
                }
            }
            let limited = self.limits_here();
            if self.current.as_ref().is_some_and(Playing::near_end) && !self.next_opened() {
                // A converted track's last frames may read ahead into the next one: it is
                // waited for until it is open.
                self.starved = self.opening_token();
                self.awaiting_open = self.pending.is_some();
                return None;
            }
            if self.current.as_ref().is_some_and(Playing::exhausted) && (self.opening.is_some() || self.pending.is_some()) {
                // Ended, and its successor is still being opened (`hand_over`).
                self.starved = self.opening_token();
                self.awaiting_open = self.pending.is_some();
                return None;
            }
            let cur = self.current.as_mut()?;
            let mut cont = self.next.as_mut().filter(|n| cur.continues_into(n));
            if let Some(n) = cont.as_mut() {
                n.starved = false;
            }
            let start = cur.pos_out();
            self.buf.clear();
            let eof = match ahead {
                Some(n) if n > 0 => cur.read_out(&mut self.buf, CHUNK_FRAMES.min(n as usize), true, &self.events, cont),
                _ => cur.read_out(&mut self.buf, CHUNK_FRAMES, false, &self.events, cont),
            };
            let starved = if cur.starved { Some(cur.token) } else { self.next.as_ref().filter(|n| n.starved).map(|n| n.token) };
            let rate = cur.rate();
            if limited {
                // Sees the samples as the output will play them (times the track gain).
                self.limiter.process(&mut self.buf, cur.gain, rate);
            }
            let chunk = (!self.buf.is_empty()).then(|| Chunk {
                generation: self.generation,
                token: cur.token,
                rate,
                start,
                duration: cur.total_out(),
                samples: std::mem::take(&mut self.buf),
                gain: cur.gain,
                limit: self.gain_cfg.limit_peaks,
                limited,
                converted_from: cur.converted_from(),
                mix: false,
            });
            if eof {
                // Gapless hand-over: the next file continues the same stream.
                self.hand_over();
            }
            if let Some(c) = chunk {
                return Some(Msg::Audio(c));
            }
            if starved.is_some() {
                self.starved = starved;
                return None;
            }
            self.current.as_ref()?;
        }
    }

    fn run(mut self, cmds: Receiver<Cmd>) {
        loop {
            if self.current.is_none() && self.starting.is_none() {
                self.starved = None;
                self.say_starved();
                if !self.idle {
                    self.idle = true;
                    if self.out.send(Msg::EndOfQueue { generation: self.generation }).is_err() {
                        return;
                    }
                }
                match cmds.recv() {
                    Ok(cmd) => {
                        if !self.handle(cmd) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
                continue;
            }
            // Drain commands without blocking before decoding more.
            while let Ok(cmd) = cmds.try_recv() {
                if !self.handle(cmd) {
                    return;
                }
            }
            self.poll_opened();
            let msg = match &self.starting {
                // A growing `play` still opening.
                Some(s) if self.current.is_none() => {
                    self.starved = Some(s.req.token);
                    None
                }
                _ if self.current.is_none() => continue,
                _ => self.produce(),
            };
            self.say_starved();
            let Some(msg) = msg else {
                if (self.starved.is_some() || self.awaiting_open) && !self.wait_feed(&cmds) {
                    return;
                }
                continue;
            };
            // Wait for room in the output queue or for a command, whichever comes first,
            // so a seek never waits for the output to drain a full queue. `Select` only
            // takes the chunk if the send is the operation chosen: a command that does not
            // flush (set next, gain) leaves it to be sent afterwards; one that flushes
            // (play, seek, stop) makes it stale, and it is dropped.
            let generation = match &msg {
                Msg::Audio(c) => c.generation,
                Msg::EndOfQueue { generation } => *generation,
            };
            let mut pending = Some(msg);
            while let Some(m) = pending.take() {
                let mut sel = crossbeam_channel::Select::new();
                let send_idx = sel.send(&self.out);
                let recv_idx = sel.recv(&cmds);
                let op = sel.select();
                if op.index() == send_idx {
                    if op.send(&self.out, m).is_err() {
                        return;
                    }
                    let mut t = self.shared.seek_trace.lock().unwrap();
                    if t.generation == generation && t.decoded.is_none() {
                        t.decoded = t.requested.map(|at| at.elapsed());
                    }
                } else {
                    debug_assert_eq!(op.index(), recv_idx);
                    let Ok(cmd) = op.recv(&cmds) else { return };
                    if !self.handle(cmd) {
                        return;
                    }
                    if self.generation == generation && !self.shared.shutdown.load(Ordering::Relaxed) {
                        pending = Some(m);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes nothing: the test drives a decoder of its own, and the player only lends it its
    /// shared state.
    struct Null;

    impl Device for Null {
        fn open(&mut self, _rate: u32) -> Result<(), String> {
            Ok(())
        }
        fn wait(&mut self, _timeout: Duration) {
            thread::sleep(Duration::from_millis(1));
        }
        fn available(&mut self) -> Result<usize, String> {
            Ok(0)
        }
        fn queued(&mut self) -> usize {
            0
        }
        fn write(&mut self, _samples: &[f32]) -> Result<(), String> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self) {}
        fn flush(&mut self) {}
    }

    /// A 32-bit float stereo WAV of `frames` frames of a ramp.
    fn wav(path: &std::path::Path, rate: u32, frames: usize) {
        let data_len = frames * 8;
        let mut b = Vec::with_capacity(44 + data_len);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 8).to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data_len as u32).to_le_bytes());
        for i in 0..frames {
            let x = (i % 1000) as f32 / 2000.0;
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&x.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// A seek in a converted track while its converter holds the first frames of the next
    /// track as lookahead (its last chunk not reached yet): the converter is gone, and the next
    /// track played from where the lookahead had read it to, its first ~1024 frames lost.
    #[test]
    fn a_seek_at_the_end_of_a_converted_track_keeps_the_next_tracks_start() {
        let (player, _events) = Player::with_device(|| Box::new(Null));
        let dir = std::env::temp_dir().join(format!("mp-engine-seek-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c_path = dir.join("c.wav");
        wav(&c_path, 44_100, 44_100);
        let mut hits = 0;
        for k in 0..40 {
            // B at another rate than the one playing (converted), C gapless after it at B's.
            let b_path = dir.join(format!("b{k}.wav"));
            wav(&b_path, 44_100, 22_050 + k * 97);
            let (out, _out_rx) = unbounded();
            let (ev, _ev_rx) = unbounded();
            let (open, _open_rx) = unbounded();
            let (_opened_tx, opened) = unbounded();
            let mut d = Decoder::new(player.shared.clone(), out, ev, open, opened, bounded(1));
            let mut b = d.open(TrackRequest::new(1, b_path.clone())).unwrap();
            b.convert(d.filters.get(44_100, 48_000));
            let c = d.open(TrackRequest::new(2, c_path.clone())).unwrap();
            let c_id = c.id;
            d.current = Some(b);
            d.next = Some(c);
            // Decode until the converter has taken C's first frames while B is still current.
            let window = loop {
                let Some(Msg::Audio(_)) = d.produce() else { break false };
                match d.current.as_ref() {
                    Some(cur) if cur.token == 1 => {
                        if cur.conv.as_ref().is_some_and(|c| c.into == Some(c_id)) {
                            break true;
                        }
                    }
                    _ => break false,
                }
            };
            if window {
                hits += 1;
                d.seek(Some(1), SeekTo::Secs(0.1));
                let start = loop {
                    match d.produce() {
                        Some(Msg::Audio(c)) if c.token == 2 => break c.start,
                        Some(_) => {}
                        None => panic!("C never played"),
                    }
                };
                assert_eq!(start, 0, "B {} frames: C plays from its start", 22_050 + k * 97);
            }
            drop(d);
            let _ = std::fs::remove_file(b_path);
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(hits > 0, "no length of B left C's lookahead taken before B's last chunk");
    }
}
