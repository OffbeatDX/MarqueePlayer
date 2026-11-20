//! Output thread: pulls chunks, applies gain and volume, writes to the device on time.
//!
//! The device always runs at the stream's own sample rate (WASAPI shared mode with
//! the system converter enabled; PipeWire and Core Audio convert likewise). Consecutive tracks at the same rate are therefore
//! sample-continuous. A rate change between tracks drains the device and reopens it at
//! the new rate, which is the only place a gap can occur. The one exception to tracks
//! playing at their own rate is a crossfade across rates: the incoming track then comes
//! converted to the rate playing, by the decode thread (`engine.rs`, `resample.rs`), and
//! its chunks say so (`Chunk::converted_from`).
//!
//! Idle costs nothing: paused, the thread blocks until it is resumed (`Shared::wake`);
//! with nothing to play, until a chunk arrives or it is woken. It never polls.
//!
//! Fades (real devices only, `Device::fades`): pause, resume, stop, seek and play-now
//! would otherwise cut the signal mid-waveform, which clicks. A pause writes a short
//! fade-out after what is already queued, lets it play out, then stops the device; the
//! resume fades back in. A seek, play-now or stop must drop what is queued, so the
//! device is stopped and emptied, and in its place the first `FADE_MS` of exactly what
//! it had not played yet (kept in `history`) is written back, faded out; the new audio
//! then follows, faded in when it starts mid-track. Track boundaries (gapless) are never
//! touched, and neither is the start of a track played from its beginning.
//!
//! The processing after decoding happens here, per write, in this order: each track's
//! gain (ReplayGain and the Opus header gain; a crossfade's mix comes with its gains in
//! it), the equalizer (`eq.rs`), the peak limiter for what the decode thread has not
//! limited (`Chunk::limited`: with the equalizer on, and for a manual skip's mix), the
//! visualizer's tap, the volume, the fades. The equalizer is here and not on the decode
//! thread, which runs a second ahead, so a slider acts within the device buffer (~40 ms);
//! it costs a bounded amount per frame and allocates nothing. With the equalizer off and
//! no mix, nothing of it runs and the samples are exactly as before.
//!
//! A manual skip with crossfade (`Player::play_crossfade`) is mixed here too: instead of
//! the cut, what was playing carries on from the device buffer's end out of the chunks
//! already decoded (`outgoing`), and fades out under the new track's fade-in. A new track at
//! another rate comes converted to the rate playing (the decode thread reads `soft_rate`),
//! so nothing here converts: this thread mixes, filters and writes, as for any other chunk.
//!
//! Converted tracks (a crossfade across rates, `engine.rs`): the info sent to the app says
//! so (`OutputInfo::converted_from`), and a pause in one asks the decode thread to play on
//! at the track's own rate (`Cmd::Native`): the resume reopens the device at it.
//!
//! Buffering (a file still being written, `engine.rs`): when the decode thread is starved and
//! everything it sent has been written, this thread says `Event::Buffering` and the device runs
//! on silence as after any underrun (an offline capture gets nothing), the position held. With
//! fades, the last frames written before the wait fade out, and the first after it fade in, as
//! around a pause, so the wait is silence rather than a cut.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Select, Sender, TryRecvError};

use crate::engine::{Chunk, Cmd, Event, Msg, OutputConfig, OutputInfo, PlayState, Shared};
use crate::eq::Equalizer;
use crate::limiter::Limiter;

// Exclusive mode's format negotiation and sample conversion: platform-neutral and
// unit-tested everywhere; only the WASAPI device uses it.
pub mod exclusive;

// The platform's own output behind `Device`: one module per system, chosen here and
// nowhere else (`system_device`, `devices`).
#[cfg(windows)]
pub mod wasapi;
#[cfg(target_os = "linux")]
pub mod alsa;
#[cfg(target_os = "linux")]
pub mod pipewire;
#[cfg(target_os = "macos")]
pub mod coreaudio;

/// A render device, for the app's device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// The system default right now.
    pub default: bool,
}

/// The output the engine plays on: WASAPI on Windows; on Linux PipeWire, or ALSA's
/// default device where there is no PipeWire to play through (`MP_AUDIO=alsa` asks for it);
/// Core Audio on macOS.
pub fn system_device() -> Box<dyn Device> {
    #[cfg(windows)]
    {
        Box::new(wasapi::WasapiDevice::default())
    }
    #[cfg(target_os = "linux")]
    {
        if pipewire::usable() {
            Box::new(pipewire::PipeWireDevice::new())
        } else {
            log::warn!("output: PipeWire is not available for playback; ALSA's default device instead");
            Box::new(alsa::AlsaDevice::default())
        }
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(coreaudio::CoreAudioDevice::default())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Box::new(NoDevice)
    }
}

/// What the app calls itself to the sound server, and its id (desktop file and icon name):
/// on Linux the desktop's mixer shows them beside the stream. Call before the engine starts.
pub fn set_identity(name: &'static str, id: &'static str) {
    #[cfg(target_os = "linux")]
    pipewire::set_identity(name, id);
    #[cfg(not(target_os = "linux"))]
    let _ = (name, id);
}

/// `f` is called whenever a device comes or goes or the system default changes, on a thread
/// of the sound system's (it must only hand the news on): the app lists the devices again.
/// Linux (PipeWire) and macOS (Core Audio's property listeners); on Windows the app lists
/// them when Settings opens and at start.
pub fn on_devices_changed(f: fn()) {
    #[cfg(target_os = "linux")]
    pipewire::on_devices_changed(f);
    #[cfg(target_os = "macos")]
    coreaudio::on_devices_changed(f);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = f;
}

/// The render devices, by name, for Settings. Opens nothing; callable from any thread.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    #[cfg(windows)]
    {
        wasapi::devices()
    }
    #[cfg(target_os = "linux")]
    {
        if pipewire::usable() { pipewire::devices() } else { Ok(alsa::devices()) }
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio::devices()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err("no audio output on this system".into())
    }
}

/// A system without an output of its own here: every open fails, so nothing plays.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
struct NoDevice;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
impl Device for NoDevice {
    fn open(&mut self, _rate: u32) -> Result<(), String> {
        Err("no audio output on this system".into())
    }
    fn wait(&mut self, timeout: Duration) {
        std::thread::sleep(timeout);
    }
    fn available(&mut self) -> Result<usize, String> {
        Err("not open".into())
    }
    fn queued(&mut self) -> usize {
        0
    }
    fn write(&mut self, _samples: &[f32]) -> Result<(), String> {
        Err("not open".into())
    }
    fn start(&mut self) -> Result<(), String> {
        Err("not open".into())
    }
    fn stop(&mut self) {}
    fn flush(&mut self) {}
}

/// Length of the fades around pause, resume, stop and seek.
pub const FADE_MS: u32 = 5;
/// After a seek or stop has cut the device, how long it keeps running (playing the
/// faded tail, then silence) while waiting for the new audio, before it is stopped.
const CUT_HOLD: Duration = Duration::from_millis(100);
/// Audio kept of what was last written, to fade out on a cut. More than the device
/// buffer holds (40 ms requested; shared mode may round up).
const HISTORY_SECS: f64 = 0.25;
/// A device held exclusively is let go after a pause this long, so other applications
/// get their sound back; the resume reopens it.
pub const RELEASE_AFTER: Duration = Duration::from_secs(3);

/// What the output loop needs from an audio device. Interleaved stereo f32.
pub trait Device {
    /// Called once, on the output thread, before anything else. A real device raises
    /// the thread's scheduling priority here.
    fn thread_started(&mut self) {}
    /// (Re)open at this sample rate. Called before the first write and on rate changes.
    fn open(&mut self, rate: u32) -> Result<(), String>;
    /// Block until the device wants more data, or `timeout` passes.
    fn wait(&mut self, timeout: Duration);
    /// Frames the device can accept right now.
    fn available(&mut self) -> Result<usize, String>;
    /// Frames queued in the device and not yet audible.
    fn queued(&mut self) -> usize;
    fn write(&mut self, samples: &[f32]) -> Result<(), String>;
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self);
    /// Drop everything queued in the device (after a seek). Leaves it stopped.
    fn flush(&mut self);
    /// A cut (seek, play-now, stop) done by the device itself: fade out over `fade` frames
    /// from the next one to be heard, drop the rest, keep running (silence until new audio
    /// comes). For a device whose next frame to be heard only it knows exactly (PipeWire's
    /// stream is pulled by the graph's clock). False: not supported, and the output thread
    /// stops, flushes and writes the faded tail back itself (`Output::cut`).
    fn cut(&mut self, _fade: usize) -> bool {
        false
    }
    /// The device's own latency: how long written audio takes to be heard.
    fn latency(&mut self) -> Duration {
        Duration::ZERO
    }
    /// True when the device this should be open on is no longer the one it is open on:
    /// the system default changed while following it, the chosen device went away or came
    /// back. Cheap enough to call every loop.
    fn route_changed(&mut self) -> bool {
        false
    }
    /// The user's choice of device and mode (`Player::set_output`). Applies from the next
    /// `open`.
    fn configure(&mut self, _cfg: &OutputConfig) {}
    /// What the last `open` opened, for the app to show.
    fn info(&mut self) -> Option<OutputInfo> {
        None
    }
    /// What `info` says has changed since it was last read without a reopen (PipeWire
    /// reports the rate its graph runs at once the stream plays, and follows the default
    /// device itself). Cheap enough to call every loop.
    fn info_changed(&mut self) -> bool {
        false
    }
    /// Let go of the device if it is held exclusively (paused a while, or stopped).
    /// True when it was: it must be opened again before the next write.
    fn release(&mut self) -> bool {
        false
    }
    /// The device is held exclusively (no other application can play): it is let go
    /// after `RELEASE_AFTER` paused. Shared streams are not, so a pause costs no wakeups.
    fn holds_device(&self) -> bool {
        false
    }
    /// A real device must be fed silence on underrun. An offline capture device (tests)
    /// waits for data instead, so the capture is exactly the decoded stream.
    fn realtime(&self) -> bool {
        true
    }
    /// Short fades on pause, resume, stop and seek (see the module doc). On for real
    /// devices; an offline capture records the decoded stream exactly unless it asks.
    fn fades(&self) -> bool {
        self.realtime()
    }
}

struct Cursor {
    chunk: Chunk,
    /// Frames of `chunk` already written.
    offset: usize,
}

/// Chunks a manual skip's outgoing audio may hold: what the decode thread had queued.
const OUTGOING_CHUNKS: usize = 16;

/// What was playing when a manual skip asked for a crossfade (`out_chunks` holds its
/// audio). It plays on until the new track's first audio arrives, then the two mix.
#[derive(Clone, Copy, Debug)]
struct Outgoing {
    generation: u64,
    token: u64,
    rate: u32,
    /// The crossfade asked for, in frames.
    want: usize,
    /// Mixing: (frames done, length).
    mix: Option<(usize, usize)>,
    /// Fading out alone (it runs out before the new audio came, or that is at another
    /// sample rate): (frames done, length).
    ending: Option<(usize, usize)>,
}

/// A fade in progress: `pos` of `len` frames done.
#[derive(Clone, Copy, Debug)]
struct Fade {
    pos: usize,
    len: usize,
    out: bool,
}

/// Raised cosine, 0 at `t = 0` and 1 at `t = 1`.
fn curve(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

fn fade_frames(rate: u32) -> usize {
    (rate as usize * FADE_MS as usize / 1000).max(1)
}

struct Output {
    dev: Box<dyn Device>,
    rx: Receiver<Msg>,
    /// Woken by pause/resume, play and shutdown (`Shared::wake`).
    wake: Receiver<()>,
    shared: Arc<Shared>,
    events: Sender<Event>,
    rate: Option<u32>,
    running: bool,
    seen_gen: u64,
    cur: Option<Cursor>,
    last_token: Option<u64>,
    /// EndOfQueue received (of this generation); report Ended once the device has
    /// drained.
    ending: Option<u64>,
    /// Volume applied at the end of the previous write, for ramping.
    last_volume: f32,
    scratch: Vec<f32>,
    fades: bool,
    fade: Option<Fade>,
    /// The next write starts with a fade-in (a resume, or a seek's first audio).
    fade_in_next: bool,
    /// The generation changed: its first chunk decides whether it fades in.
    new_generation: bool,
    /// After a cut (seek, play-now, stop), the device runs on with the faded tail until
    /// the new generation's audio arrives, or until this deadline.
    awaiting: Option<Instant>,
    /// The new audio after a cut goes in at once, not a device period later.
    write_now: bool,
    /// The last samples written (as written), for the faded tail of a cut.
    history: VecDeque<f32>,
    /// The output configuration last given to the device (`Shared::output`'s sequence).
    output_seq: u64,
    /// `Shared::devices` when the device was last asked where it plays.
    devices_seen: u64,
    /// What the device last reported having opened (sent to the app when it changes).
    last_info: Option<OutputInfo>,
    /// The device's latency in frames, taken when it opens (for the visualizer's tap).
    latency_frames: u64,
    eq: Equalizer,
    /// `Shared::eq`'s sequence number last taken.
    eq_seq: u64,
    /// Limits what the decode thread could not (after the equalizer or a mix).
    limiter: Limiter,
    outgoing: Option<Outgoing>,
    out_chunks: VecDeque<Cursor>,
    /// The chunk last written out to its end: (generation, token, rate). A skip that comes
    /// between two chunks crossfades from what follows it.
    last_done: Option<(u64, u64, u32)>,
    /// The device could not be opened (or written to) since this moment: said to the app once
    /// (`Event::NoOutput`), and that it is back once it opens again.
    failing: Option<Instant>,
    /// To the decode thread (`Cmd::Native` after a pause in a converted track).
    cmd: Sender<Cmd>,
    /// The token said buffering (`Event::Buffering` on), until it is said off.
    buffering: Option<u64>,
}

pub(crate) fn run(
    dev: Box<dyn Device>,
    chunks: Receiver<Msg>,
    wake: Receiver<()>,
    shared: Arc<Shared>,
    events: Sender<Event>,
    cmd: Sender<Cmd>,
) {
    let fades = dev.fades();
    let mut o = Output {
        dev,
        rx: chunks,
        wake,
        last_volume: shared.volume(),
        shared,
        events,
        rate: None,
        running: false,
        seen_gen: 0,
        cur: None,
        last_token: None,
        ending: None,
        scratch: Vec::new(),
        fades,
        fade: None,
        fade_in_next: false,
        new_generation: false,
        awaiting: None,
        write_now: false,
        history: VecDeque::new(),
        output_seq: 0,
        devices_seen: 0,
        last_info: None,
        latency_frames: 0,
        eq: Equalizer::new(48_000),
        eq_seq: 0,
        // More than any device buffer holds (40 ms at 384 kHz is 15,360 frames).
        limiter: Limiter::with_capacity(32_768),
        outgoing: None,
        out_chunks: VecDeque::with_capacity(OUTGOING_CHUNKS),
        last_done: None,
        failing: None,
        cmd,
        buffering: None,
    };
    o.run();
}

impl Output {
    fn run(&mut self) {
        self.dev.thread_started();
        loop {
            if self.shared.shutdown.load(Ordering::Relaxed) {
                self.dev.stop();
                return;
            }
            self.sync_generation();
            self.sync_output_config();
            self.sync_devices();
            self.publish_soft_rate();

            if self.shared.paused.load(Ordering::Relaxed) {
                if self.running {
                    self.pause_out();
                    self.ask_native();
                }
                self.set_state(PlayState::Paused);
                // A stop or seek while paused ends a wait.
                self.note_buffering();
                // Block until resumed or shut down; both wake this thread. A wake that
                // raced ahead of this point is still in the channel, so none is lost. A
                // device held exclusively is let go after a while (other applications get
                // their sound back); the resume opens it again.
                if self.shared.paused.load(Ordering::Relaxed) && !self.shared.shutdown.load(Ordering::Relaxed) {
                    if self.rate.is_some() && self.dev.holds_device() {
                        if self.wake.recv_timeout(RELEASE_AFTER).is_err() && self.dev.release() {
                            log::info!("output: paused a while; device released");
                            self.rate = None;
                        }
                    } else {
                        let _ = self.wake.recv();
                    }
                }
                continue;
            }

            // Before blocking for a chunk: a resume while waiting is said at once.
            self.note_buffering();
            let alone = self.outgoing_alone();
            if self.cur.is_none() && !self.next_chunk((!self.running || self.awaiting.is_some()) && !alone) {
                return;
            }
            // The chunk just taken may be the first of a play-now or seek that this loop
            // had not seen yet: switch before writing any of it.
            self.sync_generation();
            self.first_of_generation();
            self.note_buffering();

            // A manual skip's outgoing audio plays on (at its own rate) until it mixes.
            let rate = match self.outgoing {
                Some(o) if self.outgoing_alone() => Some(o.rate),
                _ => self.cur.as_ref().map(|c| c.chunk.rate),
            };
            let Some(rate) = rate else {
                // Nothing to play: queue exhausted, not started, or waiting after a cut.
                self.idle();
                continue;
            };

            if self.rate.is_some() && self.dev.route_changed() {
                // Another device (the default changed, the chosen one came or went):
                // what is queued plays out, then the same stream continues there.
                log::info!("output: device changed; reopening");
                self.drain();
                self.rate = None;
            }
            if self.rate != Some(rate) {
                self.drain();
                if let Err(e) = self.dev.open(rate) {
                    log::error!("output open failed: {e}");
                    self.no_output(&e);
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                self.rate = Some(rate);
                self.running = false;
                self.history.clear();
                // A reopen follows a drain (between tracks): the filters start afresh.
                self.eq.set_rate(rate);
                self.limiter.reset();
                self.latency_frames = (self.dev.latency().as_secs_f64() * f64::from(rate)) as u64;
                self.report_output();
                if self.failing.take().is_some() {
                    let _ = self.events.send(Event::NoOutput(None));
                }
            }

            if self.running && !std::mem::take(&mut self.write_now) {
                self.dev.wait(Duration::from_millis(200));
            }
            if let Err(e) = self.fill() {
                log::warn!("output write failed ({e}); reopening device");
                self.rate = None;
                self.running = false;
                self.awaiting = None;
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if !self.running {
                if let Err(e) = self.dev.start() {
                    log::warn!("output start failed: {e}");
                    self.no_output(&e);
                    std::thread::sleep(Duration::from_millis(500));
                    self.rate = None;
                    continue;
                }
                self.running = true;
            }
            self.publish_position();
            self.publish_soft_rate();
        }
    }

    /// The decode thread is starved in the generation playing: the token it waits for.
    fn starved(&self) -> Option<u64> {
        self.shared.starved.lock().unwrap().filter(|&(g, _)| g == self.seen_gen).map(|(_, t)| t)
    }

    /// Say when playback waits for a growing file's frames: nothing left to write and the decode
    /// thread starved (`Event::Buffering`, in pairs).
    fn note_buffering(&mut self) {
        let waiting = if self.cur.is_none() && self.outgoing.is_none() && self.ending.is_none() { self.starved() } else { None };
        if waiting.is_some() && !self.shared.paused.load(Ordering::Relaxed) {
            // Resumed while waiting: playing, though nothing is written (the state is said
            // playing at the first write otherwise).
            let mut st = self.shared.status.lock().unwrap();
            if st.state == PlayState::Paused {
                st.state = PlayState::Playing;
            }
        }
        if waiting == self.buffering {
            return;
        }
        if let Some(token) = self.buffering.take() {
            let _ = self.events.send(Event::Buffering { token, on: false });
        }
        if let Some(token) = waiting {
            log::debug!("output: waiting for the frames of {token}");
            let _ = self.events.send(Event::Buffering { token, on: true });
        }
        self.buffering = waiting;
        self.shared.status.lock().unwrap().buffering = waiting.is_some();
    }

    /// The rate a manual skip's crossfade would mix at, for the decode thread to convert a
    /// new track at another rate to (`Shared::soft_rate`); 0 when it would not crossfade
    /// (`soft_skip`'s conditions, as far as they are known before the skip).
    fn publish_soft_rate(&self) {
        let soft = self.running
            && self.outgoing.is_none()
            && self.fade.is_none()
            && self.awaiting.is_none()
            && (self.cur.is_some() || self.last_done.is_some())
            && !self.shared.paused.load(Ordering::Relaxed);
        let rate = if soft { self.rate.unwrap_or(0) } else { 0 };
        self.shared.soft_rate.store(rate, Ordering::Relaxed);
    }

    /// Paused in a converted track (not in a crossfade's mix): ask the decode thread for the
    /// frame after the pause again, at the track's own rate, so what plays after the resume
    /// is untouched (the resume reopens the device at that rate, fading in as any resume).
    fn ask_native(&self) {
        let Some(c) = self.cur.as_ref() else { return };
        if c.chunk.converted_from.is_none() || c.chunk.mix || self.outgoing.is_some() || c.chunk.generation != self.seen_gen {
            return;
        }
        let frame = c.chunk.start + c.offset as u64;
        let _ = self.cmd.send(Cmd::Native { generation: c.chunk.generation, token: c.chunk.token, frame, rate: c.chunk.rate });
    }

    /// The chunk starting to be written is converted, or no longer is: tell the app (the
    /// info's `converted_from`). Sends nothing while it stays as it was.
    fn note_conversion(&mut self, from: Option<u32>) {
        let Some(info) = self.last_info.as_mut() else { return };
        if info.converted_from == from {
            return;
        }
        info.converted_from = from;
        let info = info.clone();
        *self.shared.output_info.lock().unwrap() = Some(info.clone());
        let _ = self.events.send(Event::Output(info));
    }

    /// The system's devices changed: where the open stream plays may have too (PipeWire
    /// moves a stream that follows the default, also while it is paused or stopped). While
    /// playing this is looked at anyway (`publish_position`).
    fn sync_devices(&mut self) {
        let seq = self.shared.devices.load(Ordering::Acquire);
        if seq != self.devices_seen {
            self.devices_seen = seq;
            if self.rate.is_some() && self.dev.info_changed() {
                self.report_output();
            }
        }
    }

    /// A new output choice from the app: hand it to the device, and reopen (after what is
    /// queued has played) if one is open. Playback goes on from the same frame.
    fn sync_output_config(&mut self) {
        let (seq, cfg) = {
            let o = self.shared.output.lock().unwrap();
            (o.0, o.1.clone())
        };
        if seq == self.output_seq {
            return;
        }
        self.output_seq = seq;
        self.dev.configure(&cfg);
        if self.rate.is_some() {
            self.drain();
            self.dev.release();
            self.rate = None;
        }
    }

    /// The device cannot be opened or started: the first time in a row, the app is told (it
    /// would otherwise show a track playing in silence). Opening is tried again meanwhile.
    fn no_output(&mut self, why: &str) {
        if self.failing.is_none() {
            self.failing = Some(Instant::now());
            let _ = self.events.send(Event::NoOutput(Some(why.to_owned())));
        }
    }

    fn report_output(&mut self) {
        let Some(mut info) = self.dev.info() else { return };
        // The device knows its format; whether the chunk about to play is converted is known
        // here.
        info.converted_from = self.cur.as_ref().and_then(|c| c.chunk.converted_from);
        if self.last_info.as_ref() != Some(&info) {
            self.last_info = Some(info.clone());
            *self.shared.output_info.lock().unwrap() = Some(info.clone());
            let _ = self.events.send(Event::Output(info));
        }
    }

    /// Seek or play-now: catch up with the newest generation, which is the shared one
    /// or, if the decode thread got there first, that of the chunk already taken. The
    /// device is flushed of the old position and a chunk older than the new generation
    /// is dropped; a chunk *of* the new generation is kept whole. (Before, a chunk taken
    /// before its generation was noticed lost all but its first write: ~85 ms skipped
    /// right after a play or seek, intermittently.)
    fn sync_generation(&mut self) {
        let shared = self.shared.generation.load(Ordering::Acquire);
        let generation = self.cur.as_ref().map_or(shared, |c| c.chunk.generation.max(shared));
        if generation == self.seen_gen {
            return;
        }
        let soft = self.soft_skip(generation);
        self.seen_gen = generation;
        if soft {
            // A manual skip with crossfade: no cut. What was playing carries on from the
            // chunk being written, and fades out once the new audio is here.
            // From the chunk being written, or, when the last one was just written out (the
            // skip came at a chunk boundary), from the chunks of that track still queued
            // (`next_chunk` keeps them).
            self.out_chunks.clear();
            let (generation_was, token, rate) = match self.cur.take() {
                Some(c) => {
                    let k = (c.chunk.generation, c.chunk.token, c.chunk.rate);
                    self.out_chunks.push_back(c);
                    k
                }
                None => self.last_done.expect("soft_skip checked it"),
            };
            let secs = f32::from_bits(self.shared.soft_secs.load(Ordering::Relaxed));
            self.outgoing = Some(Outgoing {
                generation: generation_was,
                token,
                rate,
                want: (f64::from(secs) * f64::from(rate)) as usize,
                mix: None,
                ending: None,
            });
            if self.ending.is_some_and(|g| g < generation) {
                self.ending = None;
            }
            self.fade = None;
            self.fade_in_next = false;
            self.new_generation = true;
            return;
        }
        self.outgoing = None;
        self.out_chunks.clear();
        self.last_done = None;
        self.eq.reset();
        self.limiter.reset();
        if self.cur.as_ref().is_some_and(|c| c.chunk.generation < generation) {
            self.cur = None;
        }
        // An end of queue from an older generation is void; one of this generation stays.
        // A play that fails to open sends its EndOfQueue right after bumping the
        // generation, and it can be taken before this loop sees the bump: clearing it
        // here dropped that Ended, and the app never moved past the failed track.
        if self.ending.is_some_and(|g| g < generation) {
            self.ending = None;
        }
        // A fade in progress belongs to the old position.
        self.fade = None;
        self.fade_in_next = false;
        self.new_generation = true;
        if self.running {
            if self.fades {
                self.cut();
            } else {
                self.dev.flush();
                self.running = false;
            }
        }
    }

    /// The first chunk of a new generation: one that starts mid-track (a seek, a resumed
    /// session) fades in; a track played from its start does not.
    fn first_of_generation(&mut self) {
        if !self.new_generation {
            return;
        }
        let Some(c) = self.cur.as_ref().filter(|c| c.chunk.generation == self.seen_gen) else { return };
        self.new_generation = false;
        self.write_now = self.awaiting.take().is_some();
        let (new_rate, mid_track) = (c.chunk.rate, c.offset == 0 && c.chunk.start > 0);
        let left = self.outgoing_frames();
        if let Some(o) = self.outgoing.as_mut()
            && o.ending.is_none()
        {
            if new_rate == o.rate {
                // The new track fades in over what is left of the old one, or what was
                // asked for, whichever is shorter.
                let len = o.want.min(left);
                o.mix = Some((0, len));
                if len == 0 {
                    self.outgoing = None;
                    self.out_chunks.clear();
                } else {
                    return;
                }
            } else {
                // Another sample rate: the old audio fades out (FADE_MS), the device
                // reopens, the new track starts as usual.
                o.ending = Some((0, fade_frames(o.rate).min(left)));
            }
        }
        if self.fades && mid_track {
            self.fade_in_next = true;
        }
    }

    /// Whether the generation just bumped asks for a crossfade from what plays (a manual
    /// skip), and there is something playing to crossfade from.
    fn soft_skip(&self, generation: u64) -> bool {
        self.running
            && self.rate.is_some()
            && self.outgoing.is_none()
            && self.fade.is_none()
            && self.awaiting.is_none()
            && self.shared.soft_gen.load(Ordering::Relaxed) == generation
            && !self.shared.paused.load(Ordering::Relaxed)
            && match self.cur.as_ref() {
                Some(c) => c.chunk.generation < generation && c.offset < c.chunk.samples.len() / 2,
                // Between two chunks of what plays: it carries on from the next one queued.
                None => self.last_done.is_some_and(|(g, _, _)| g < generation),
            }
    }

    /// The outgoing audio plays without the new (before the mix, or fading out alone).
    fn outgoing_alone(&self) -> bool {
        self.outgoing.is_some_and(|o| o.mix.is_none())
    }

    /// Frames of the outgoing audio not written yet.
    fn outgoing_frames(&self) -> usize {
        self.out_chunks.iter().map(|c| c.chunk.samples.len() / 2 - c.offset).sum()
    }

    /// The next frame of the outgoing audio, times its gain, and whether it is to be
    /// limited and was.
    fn pop_outgoing(&mut self) -> Option<([f32; 2], bool, bool)> {
        loop {
            let c = self.out_chunks.front_mut()?;
            if c.offset < c.chunk.samples.len() / 2 {
                let g = c.chunk.gain;
                let s = &c.chunk.samples[c.offset * 2..c.offset * 2 + 2];
                c.offset += 1;
                return Some(([s[0] * g, s[1] * g], c.chunk.limit, c.chunk.limited));
            }
            self.out_chunks.pop_front();
        }
    }

    /// The outgoing audio alone, into `scratch`: as it is while the new audio is not here
    /// yet, faded out over its last FADE_MS when it is about to run out (or the new audio
    /// is at another rate). Returns the frames written and whether any is to be limited by
    /// this thread.
    fn write_outgoing_alone(&mut self, want: usize) -> (usize, bool) {
        let mut n = 0;
        let mut unlimited = false;
        while n < want {
            let left = self.outgoing_frames();
            let Some(o) = self.outgoing.as_mut() else { break };
            if o.ending.is_none() && left <= fade_frames(o.rate) {
                o.ending = Some((0, left));
            }
            let g = match o.ending.as_mut() {
                Some((pos, len)) if *pos >= *len => break,
                Some((pos, len)) => {
                    *pos += 1;
                    curve(1.0 - *pos as f32 / *len as f32)
                }
                None => 1.0,
            };
            let Some((f, limit, limited)) = self.pop_outgoing() else { break };
            unlimited |= limit && !limited;
            self.scratch.push(f[0] * g);
            self.scratch.push(f[1] * g);
            n += 1;
        }
        if self.outgoing.is_some_and(|o| o.ending.is_some_and(|(p, l)| p >= l)) || self.out_chunks.is_empty() {
            // Done with it. Still no new audio: the device runs on into silence, as after a
            // cut, until it comes.
            self.outgoing = None;
            self.out_chunks.clear();
            if self.cur.is_none() {
                self.awaiting = Some(Instant::now() + CUT_HOLD);
            }
        }
        (n, unlimited)
    }

    /// Take a new equalizer setting, if there is one and the handle is not holding it.
    fn poll_eq(&mut self) {
        let seq = self.shared.eq.seq.load(Ordering::Acquire);
        if seq != self.eq_seq
            && let Ok(p) = self.shared.eq.params.try_lock()
        {
            self.eq.set_params(&p);
            self.eq_seq = seq;
        }
    }

    /// Drop what the device has queued (a seek, play-now or stop) without a click: stop
    /// it, empty it, and write back the first few milliseconds of what it had not played
    /// yet, faded out, then keep it running until the new audio arrives (`awaiting`).
    fn cut(&mut self) {
        if self.dev.cut(fade_frames(self.rate.unwrap_or(48_000))) {
            self.history.clear();
            self.awaiting = Some(Instant::now() + CUT_HOLD);
            return;
        }
        self.dev.stop();
        let held = self.history.len() / 2;
        let queued = self.dev.queued().min(held);
        self.dev.flush();
        let n = queued.min(fade_frames(self.rate.unwrap_or(48_000)));
        if n == 0 {
            self.running = false;
            return;
        }
        let from = (held - queued) * 2;
        self.scratch.clear();
        for i in 0..n {
            let g = curve(1.0 - (i + 1) as f32 / n as f32);
            self.scratch.push(self.history[from + i * 2] * g);
            self.scratch.push(self.history[from + i * 2 + 1] * g);
        }
        self.history.clear();
        let tail = std::mem::take(&mut self.scratch);
        let ok = self.dev.write(&tail).is_ok() && self.dev.start().is_ok();
        self.remember(&tail);
        self.scratch = tail;
        if ok {
            self.awaiting = Some(Instant::now() + CUT_HOLD);
        } else {
            self.dev.flush();
            self.running = false;
        }
    }

    /// Pause: with fades, write a short fade-out after what is queued and let it all
    /// play out (the pause is heard after the device buffer, ~40 ms), then stop. The
    /// resume fades back in from the frame after the fade.
    fn pause_out(&mut self) {
        if self.fades && (self.cur.is_some() || self.outgoing.is_some()) {
            let len = fade_frames(self.rate.unwrap_or(48_000));
            self.fade = Some(Fade { pos: 0, len, out: true });
            self.fade_in_next = false;
            let mut left = len;
            let deadline = Instant::now() + Duration::from_millis(300);
            while left > 0 && Instant::now() < deadline {
                let room = self.dev.available().unwrap_or(0);
                if room == 0 {
                    self.dev.wait(Duration::from_millis(20));
                    continue;
                }
                match self.write_frames(room.min(left)) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => left -= n,
                }
            }
            self.fade = None;
            while self.dev.queued() > 0 && Instant::now() < deadline {
                self.dev.wait(Duration::from_millis(20));
            }
            self.publish_position();
            self.fade_in_next = true;
        }
        self.dev.stop();
        self.running = false;
        self.awaiting = None;
    }

    /// Move the next live chunk into `cur`. Blocking (`block`) waits for a chunk or a
    /// wake, and after a cut no longer than its hold. Returns false if the decode side
    /// is gone.
    fn next_chunk(&mut self, block: bool) -> bool {
        loop {
            let msg = if block {
                let mut sel = Select::new();
                let chunks = sel.recv(&self.rx);
                let wake = sel.recv(&self.wake);
                let op = match self.awaiting {
                    Some(deadline) => match sel.select_deadline(deadline) {
                        Ok(op) => op,
                        Err(_) => return true,
                    },
                    None => sel.select(),
                };
                if op.index() == wake {
                    let _ = op.recv(&self.wake);
                    return true;
                }
                debug_assert_eq!(op.index(), chunks);
                match op.recv(&self.rx) {
                    Ok(m) => m,
                    Err(_) => return false,
                }
            } else {
                match self.rx.try_recv() {
                    Ok(m) => m,
                    Err(TryRecvError::Empty) => return true,
                    Err(TryRecvError::Disconnected) => return false,
                }
            };
            let generation = self.shared.generation.load(Ordering::Acquire);
            match msg {
                Msg::Audio(c) if c.generation < generation => {
                    // What a manual skip crossfades from: the rest of the track playing, as
                    // far as it was decoded.
                    if let Some(o) = self.outgoing
                        && o.mix.is_none()
                        && o.ending.is_none()
                        && c.generation == o.generation
                        && c.token == o.token
                        && self.out_chunks.len() < OUTGOING_CHUNKS
                    {
                        self.out_chunks.push_back(Cursor { chunk: c, offset: 0 });
                    }
                    continue;
                }
                Msg::Audio(c) => {
                    self.ending = None;
                    self.cur = Some(Cursor { chunk: c, offset: 0 });
                    return true;
                }
                Msg::EndOfQueue { generation: g } if g < generation => continue,
                Msg::EndOfQueue { generation: g } => {
                    self.ending = Some(g);
                    return true;
                }
            }
        }
    }

    /// Write as much as the device takes.
    fn fill(&mut self) -> Result<(), String> {
        let want = self.dev.available()?;
        self.write_frames(want).map(|_| ())
    }

    /// Write up to `want` frames, crossing chunk (and track) boundaries, with gain,
    /// volume and any fade applied. Returns the frames written.
    fn write_frames(&mut self, mut want: usize) -> Result<usize, String> {
        if want == 0 {
            return Ok(0);
        }
        self.scratch.clear();
        if self.fade_in_next && (self.cur.is_some() || self.outgoing.is_some()) {
            self.fade_in_next = false;
            self.fade = Some(Fade { pos: 0, len: fade_frames(self.rate.unwrap_or(48_000)), out: false });
        }
        let target_vol = self.shared.volume();
        // Whether anything in this block is to be limited here: peaks to limit that the
        // decode thread has not, or a mix of two tracks.
        let mut limit_here = false;
        let mut mixed = false;
        if self.outgoing_alone() {
            let (_, unlimited) = self.write_outgoing_alone(want);
            limit_here |= unlimited;
            want = 0;
        }
        while want > 0 {
            if let Some(c) = self.cur.as_ref().filter(|c| c.offset == 0) {
                let (token, from) = (c.chunk.token, c.chunk.converted_from);
                if self.last_token != Some(token) {
                    self.last_token = Some(token);
                    let _ = self.events.send(Event::TrackStarted { token });
                }
                self.note_conversion(from);
            }
            let Some(cur) = self.cur.as_mut() else { break };
            let frames = cur.chunk.samples.len() / 2;
            let n = (frames - cur.offset).min(want);
            let g = cur.chunk.gain;
            limit_here |= cur.chunk.limit && !cur.chunk.limited;
            let src = &cur.chunk.samples[cur.offset * 2..(cur.offset + n) * 2];
            self.scratch.extend(src.iter().map(|s| s * g));
            cur.offset += n;
            want -= n;
            if cur.offset == frames {
                let rate = cur.chunk.rate;
                self.last_done = Some((cur.chunk.generation, cur.chunk.token, rate));
                self.cur = None;
                // A seek, play-now or skip asked for since this loop last looked: stop here,
                // before the old position's next chunks are taken (and dropped), so a skip's
                // crossfade can still carry on from them (`sync_generation`).
                if self.shared.generation.load(Ordering::Acquire) != self.seen_gen {
                    break;
                }
                // Peek at the next chunk; stop at a rate change so it gets a reopened device.
                if !self.take_ready() {
                    break;
                }
                if self.cur.as_ref().is_some_and(|c| c.chunk.rate != rate) {
                    break;
                }
                // The first chunk of a seek or play-now: the device must be flushed of
                // the old position before any of it is written (next turn of the loop).
                if self.cur.as_ref().is_some_and(|c| c.chunk.generation != self.seen_gen) {
                    break;
                }
            }
        }
        if self.scratch.is_empty() {
            return Ok(0);
        }
        let rate = self.rate.unwrap_or(48_000);
        // A manual skip's crossfade: the old audio, fading out, under the new one fading in.
        if let Some(o) = self.outgoing
            && let Some((mut pos, len)) = o.mix
        {
            let mut i = 0;
            while i * 2 < self.scratch.len() && pos < len {
                let Some((old, limit, _)) = self.pop_outgoing() else { break };
                let (wo, wi) = crate::crossfade::gains(pos as u64, len as u64);
                let f = &mut self.scratch[i * 2..i * 2 + 2];
                f[0] = f[0] * wi + old[0] * wo;
                f[1] = f[1] * wi + old[1] * wo;
                limit_here |= limit;
                pos += 1;
                i += 1;
            }
            mixed = true;
            if pos >= len || self.out_chunks.is_empty() {
                self.outgoing = None;
                self.out_chunks.clear();
            } else if let Some(o) = self.outgoing.as_mut() {
                o.mix = Some((pos, len));
            }
        }
        // The equalizer, then the limiter on what is to be limited here, so it sees the
        // equalized mix. Neither touches a sample while off and idle.
        self.poll_eq();
        let equalized = !self.eq.is_bypassed();
        if equalized {
            self.eq.process(&mut self.scratch);
        }
        let limit_on = self.cur.as_ref().is_none_or(|c| c.chunk.limit);
        if (limit_here || ((equalized || mixed) && limit_on)) || !self.limiter.is_idle() {
            self.limiter.process(&mut self.scratch, 1.0, rate);
        }
        // The visualizer sees the music after ReplayGain, the equalizer and the limiter, and
        // before volume and fades (one relaxed load when it is off; lock-free when on).
        self.shared.spectrum.push(&self.scratch, rate);
        // Volume ramp across this block avoids zipper noise on volume changes.
        let n = self.scratch.len() / 2;
        let from = self.last_volume;
        for (i, f) in self.scratch.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let v = from + (target_vol - from) * (i + 1) as f32 / n as f32;
            // Perceptual volume curve; the slider is linear in its own position.
            let v = v * v;
            f[0] *= v;
            f[1] *= v;
        }
        self.last_volume = target_vol;
        if let Some(mut fade) = self.fade {
            for f in self.scratch.as_chunks_mut::<2>().0 {
                let g = if fade.pos >= fade.len {
                    if fade.out { 0.0 } else { 1.0 }
                } else {
                    fade.pos += 1;
                    let t = fade.pos as f32 / fade.len as f32;
                    curve(if fade.out { 1.0 - t } else { t })
                };
                f[0] *= g;
                f[1] *= g;
            }
            // A finished fade-in is done with; a fade-out holds (silence) until cleared.
            self.fade = (fade.out || fade.pos < fade.len).then_some(fade);
        }
        // The last frames decoded before a wait for a growing file's bytes: faded out, and
        // what comes after the wait fades in.
        if self.fades && self.cur.is_none() && self.outgoing.is_none() && self.rx.is_empty() && self.starved().is_some() {
            let len = fade_frames(rate).min(n);
            for (i, f) in self.scratch.as_chunks_mut::<2>().0[n - len..].iter_mut().enumerate() {
                let g = curve(1.0 - (i + 1) as f32 / len as f32);
                f[0] *= g;
                f[1] *= g;
            }
            self.fade_in_next = true;
        }
        self.dev.write(&self.scratch)?;
        let written = std::mem::take(&mut self.scratch);
        self.remember(&written);
        self.scratch = written;
        self.trace_seek();
        Ok(n)
    }

    /// Keep the last samples written, for a cut's faded tail (fades only).
    fn remember(&mut self, samples: &[f32]) {
        if !self.fades {
            return;
        }
        self.history.extend(samples.iter().copied());
        let cap = (f64::from(self.rate.unwrap_or(48_000)) * HISTORY_SECS) as usize * 2;
        if self.history.len() > cap {
            let excess = self.history.len() - cap;
            self.history.drain(..excess);
        }
    }

    /// The first write of a seek's generation: record where the time went.
    fn trace_seek(&mut self) {
        let mut t = self.shared.seek_trace.lock().unwrap();
        if t.generation != self.seen_gen || t.requested.is_none() {
            return;
        }
        let at = t.requested.take().unwrap();
        let ms = |d: Option<Duration>| d.map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let timing = crate::engine::SeekTiming {
            positioned_ms: ms(t.positioned),
            decoded_ms: ms(t.decoded),
            written_ms: at.elapsed().as_secs_f64() * 1000.0,
            device_ms: self.dev.latency().as_secs_f64() * 1000.0,
        };
        drop(t);
        log::debug!("seek timing: {timing:?}");
        self.shared.status.lock().unwrap().last_seek = Some(timing);
    }

    /// Non-blocking: move a ready chunk into `cur`. False if none is ready.
    fn take_ready(&mut self) -> bool {
        self.next_chunk(false);
        self.cur.is_some()
    }

    fn drain(&mut self) {
        if !self.running {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.dev.queued() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.dev.stop();
        self.running = false;
        self.awaiting = None;
    }

    fn idle(&mut self) {
        if let Some(g) = self.ending {
            self.drain();
            self.ending = None;
            self.last_token = None;
            // A play, seek or stop came while what was queued played out: this end belongs to
            // what played before it. Saying it now made the app skip the track just started
            // (it moves on at `Ended`); the new generation's own end is said when it comes.
            if self.shared.generation.load(Ordering::Acquire) > g {
                return;
            }
            {
                let mut st = self.shared.status.lock().unwrap();
                st.state = PlayState::Stopped;
            }
            let _ = self.events.send(Event::Ended);
            if self.dev.release() {
                self.rate = None;
            }
        } else if let Some(deadline) = self.awaiting {
            // After a cut: the faded tail plays and the device underruns into silence
            // (nothing is written, so the new audio goes straight in when it comes).
            // Nothing came (a stop): stop the device.
            if Instant::now() >= deadline {
                self.dev.stop();
                self.running = false;
                self.awaiting = None;
                if self.dev.release() {
                    self.rate = None;
                }
            }

        } else if self.running && !self.dev.realtime() {
            std::thread::sleep(Duration::from_millis(1));
        } else if self.running {
            // Underrun: keep the device fed with silence rather than stopping, so a slow
            // disk doesn't turn into a device restart.
            if let Ok(n) = self.dev.available() {
                self.scratch.clear();
                self.scratch.resize(n * 2, 0.0);
                let silence = std::mem::take(&mut self.scratch);
                let _ = self.dev.write(&silence);
                self.remember(&silence);
                self.scratch = silence;
            }
            self.dev.wait(Duration::from_millis(50));
        }
    }

    fn set_state(&self, state: PlayState) {
        let mut st = self.shared.status.lock().unwrap();
        if !self.stopped() {
            st.state = state;
        }
    }

    /// A stop began the generation playing, or a newer one has begun that this loop has not
    /// caught up with: the decode thread has said the state (`Cmd::Stop`), and what is still
    /// written belongs to before it. Asked with the status locked, so a stop that came first
    /// is seen here.
    fn stopped(&self) -> bool {
        self.shared.generation.load(Ordering::Acquire) > self.seen_gen || self.shared.stopped.load(Ordering::Acquire) == self.seen_gen
    }

    fn publish_position(&mut self) {
        if self.dev.info_changed() {
            self.report_output();
        }
        let queued = self.dev.queued() as u64;
        self.shared.spectrum.publish(queued + self.latency_frames);
        let mut st = self.shared.status.lock().unwrap();
        if self.stopped() {
            return;
        }
        st.state = PlayState::Playing;
        if let Some(cur) = &self.cur {
            let rate = f64::from(cur.chunk.rate);
            let written = cur.chunk.start + cur.offset as u64;
            st.token = Some(cur.chunk.token);
            st.position = written.saturating_sub(queued) as f64 / rate;
            st.duration = cur.chunk.duration.map(|d| d as f64 / rate);
        }
    }
}
