//! PipeWire output, natively (libpipewire through the `pipewire` crate), the Linux
//! counterpart of `wasapi.rs`.
//!
//! **Threads.** Every PipeWire object (the main loop, the connection, the registry
//! listener that keeps the list of sinks and the default sink, the stream) lives on one
//! thread of its own, `mp-pipewire`, which sleeps in the loop until PipeWire or the output
//! thread has something for it: nothing polls. The output thread (`output.rs`) talks to it
//! over a channel attached to that loop (open, start, stop, flush, close) and never blocks
//! on it except to wait for an open.
//!
//! **Audio.** The stream is driven by PipeWire (`RT_PROCESS`: the process callback runs on
//! PipeWire's real-time data thread once per graph cycle). The output thread writes into a
//! single-producer single-consumer ring (`Ring`, atomics only); the callback copies what
//! the graph asks for out of it, pads an underrun with silence, publishes how much is
//! still unheard (the graph's delay and the stream's converter, from `pw_stream_get_time_n`)
//! and unparks the output thread, whose `wait` parks until then. The callback does not
//! allocate, lock, log or make a call that can block. The ring holds what `available`
//! offers, 50 ms or three graph cycles, whichever is more, so the output thread's view is
//! the same as with WASAPI's 40 ms device buffer: `queued` is the ring plus what the graph
//! has not played yet, so positions, the visualizer's tap and lyrics follow what is heard.
//!
//! **Pause, seek, stop.** `stop` deactivates the stream (the graph stops calling it, and
//! the sink suspends when nothing else plays: paused costs nothing), `start` activates it.
//! A seek, play-now or stop is a `cut`: the callback fades out the very next frames it hands
//! the graph (5 ms), so the fade follows what is already in the graph, and drops the rest;
//! the output thread then writes the new audio. (WASAPI tells exactly what is unplayed, so
//! there the output thread writes a faded tail itself; PipeWire's graph holds what it took.)
//! `flush` drops what the ring holds and what PipeWire queued, on a stopped stream. The
//! output thread's fades and cuts then work as on Windows.
//!
//! **Rates.** The stream is opened at the track's own rate (F32 stereo), `node.rate` asking
//! the graph for that rate (PipeWire switches when its `default.clock.allowed-rates` include
//! it and nothing else plays at another rate; otherwise its converter resamples). The
//! bit-perfect setting (`OutputConfig::exclusive`, Windows' exclusive mode) adds
//! `node.force-rate`: the graph switches even while other streams play (they are resampled),
//! still only to an allowed rate. Same-rate tracks are gapless on one stream; a rate change
//! reopens it after the old one has played out, as on Windows. `OutputInfo` says the rate
//! the graph actually runs at, once the stream runs, so the details' "Output:" line says
//! whether PipeWire resamples.
//!
//! **Devices.** "System default" leaves the stream to the session manager (WirePlumber),
//! which links it to the default sink and moves it when the default changes, without a gap.
//! A chosen sink (by `node.name`) is the stream's `target.object`; if it goes, the session
//! manager moves the stream to the default and `route_changed` reopens it there with
//! `OutputInfo::missing`; when it comes back, the stream is reopened on it.
//!
//! **Volume.** The app applies its volume to the samples, as everywhere. The stream's own
//! volume stays the desktop mixer's (per-application volume, as Windows' volume mixer is):
//! the two are not tied together.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;

use super::{Device, DeviceInfo};
use crate::engine::{OutputConfig, OutputInfo};

/// Interleaved stereo f32.
const FRAME_BYTES: usize = 8;
/// The ring's capacity in frames (a power of two): 0.68 s at 48 kHz, 85 ms at 384 kHz,
/// always more than `available` offers.
const RING_FRAMES: usize = 1 << 15;
/// What the ring is kept filled to, at least.
const TARGET_MS: u32 = 50;
/// The graph cycle the stream asks for (`node.latency`): 20 ms.
const LATENCY_MS: u32 = 20;
const SERVER: &str = "PipeWire";

/// What the app calls itself to PipeWire (`output::set_identity`).
static IDENTITY: Mutex<(&str, &str)> = Mutex::new(("Marquee Player", "marquee-player"));

/// The application's name and id (its desktop file's name, also its icon's), for the
/// desktop's mixer. Call before the engine starts.
pub fn set_identity(name: &'static str, id: &'static str) {
    *IDENTITY.lock().unwrap() = (name, id);
}

// ---------------------------------------------------------------------------------------
// The ring between the output thread and the process callback.

/// Single producer (the output thread), single consumer (PipeWire's data thread).
struct Ring {
    /// Interleaved stereo f32 as bits: written relaxed, published by `write`'s release.
    buf: Box<[AtomicU32]>,
    /// Frames written in all (producer).
    write: AtomicU64,
    /// Frames taken in all (consumer).
    read: AtomicU64,
    /// A cut (`cut`, `flush`): what was written before it is dropped (after a fade-out of
    /// `cut_len` frames from the next one taken); `cut_seq` says there is a new one.
    skip: AtomicU64,
    cut_len: AtomicU32,
    cut_seq: AtomicU64,
    /// The consumer's own: the cut it last took, its fade (done, length) and its end.
    cut_seen: AtomicU64,
    fade_pos: AtomicU32,
    fade_len: AtomicU32,
    fade_end: AtomicU64,
    /// The stream's sample rate.
    rate: u32,
    /// Frames handed to PipeWire and not heard yet, as of `pipe_ns` (the consumer's view).
    pipe: AtomicU64,
    pipe_ns: AtomicU64,
    /// The rate the graph runs at (0 until the stream has run).
    graph_rate: AtomicU32,
    /// The frames the graph asked for last cycle.
    quantum: AtomicU32,
    /// The stream is gone (an error, or the server went away): nothing will drain it.
    dead: AtomicBool,
    /// The stream's node in the graph (`NO_NODE` until it is connected), to see which sink
    /// the session manager linked it to.
    node: AtomicU32,
    underruns: AtomicU64,
    base: Instant,
    /// The output thread, unparked every cycle (`Device::wait` parks).
    waiter: std::thread::Thread,
}

impl Ring {
    fn new(rate: u32) -> Self {
        Ring {
            buf: (0..RING_FRAMES * 2).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            skip: AtomicU64::new(0),
            cut_len: AtomicU32::new(0),
            cut_seq: AtomicU64::new(0),
            cut_seen: AtomicU64::new(0),
            fade_pos: AtomicU32::new(0),
            fade_len: AtomicU32::new(0),
            fade_end: AtomicU64::new(0),
            rate,
            pipe: AtomicU64::new(0),
            pipe_ns: AtomicU64::new(0),
            graph_rate: AtomicU32::new(0),
            quantum: AtomicU32::new(0),
            dead: AtomicBool::new(false),
            node: AtomicU32::new(NO_NODE),
            underruns: AtomicU64::new(0),
            base: Instant::now(),
            waiter: std::thread::current(),
        }
    }

    fn now_ns(&self) -> u64 {
        self.base.elapsed().as_nanos() as u64
    }

    /// Frames in the ring the consumer will still play.
    fn in_ring(&self) -> u64 {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire).max(self.skip.load(Ordering::Acquire));
        w.saturating_sub(r)
    }

    /// Frames handed to PipeWire and not heard yet, now.
    fn in_pipe(&self) -> u64 {
        let pipe = self.pipe.load(Ordering::Acquire);
        let at = self.pipe_ns.load(Ordering::Acquire);
        let played = self.now_ns().saturating_sub(at) as u128 * u128::from(self.rate) / 1_000_000_000;
        pipe.saturating_sub(played as u64)
    }

    /// Room the producer may use without overwriting what the consumer has not taken.
    fn room(&self) -> u64 {
        let w = self.write.load(Ordering::Relaxed);
        RING_FRAMES as u64 - w.saturating_sub(self.read.load(Ordering::Acquire))
    }

    /// Producer: append interleaved stereo samples.
    fn push(&self, samples: &[f32]) -> Result<(), String> {
        let frames = samples.len() / 2;
        if frames as u64 > self.room() {
            return Err("PipeWire buffer full".into());
        }
        let w = self.write.load(Ordering::Relaxed);
        let mask = RING_FRAMES as u64 - 1;
        for (i, f) in samples.as_chunks::<2>().0.iter().enumerate() {
            let at = (((w + i as u64) & mask) * 2) as usize;
            self.buf[at].store(f[0].to_bits(), Ordering::Relaxed);
            self.buf[at + 1].store(f[1].to_bits(), Ordering::Relaxed);
        }
        self.write.store(w + frames as u64, Ordering::Release);
        Ok(())
    }

    /// Producer: drop everything not taken yet, the first `fade` frames of it faded out
    /// (they follow what the graph has, so the cut is continuous).
    fn cut(&self, fade: usize) {
        self.cut_len.store(fade as u32, Ordering::Relaxed);
        self.skip.store(self.write.load(Ordering::Relaxed), Ordering::Release);
        self.cut_seq.fetch_add(1, Ordering::Release);
    }

    /// Producer: drop everything not taken yet, at once (the stream is stopped and PipeWire
    /// drops what it holds too).
    fn flush(&self) {
        self.cut(0);
        self.pipe.store(0, Ordering::Release);
    }

    /// Consumer (real-time): fill `out` (little-endian f32 frames) with up to `frames` of
    /// the ring, silence after it. Returns the frames of real audio.
    fn take(&self, out: &mut [u8], frames: usize) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let mut r = self.read.load(Ordering::Relaxed);
        // A new cut: its fade starts at the next frame taken, then the rest is dropped.
        let seq = self.cut_seq.load(Ordering::Acquire);
        if seq != self.cut_seen.load(Ordering::Relaxed) {
            self.cut_seen.store(seq, Ordering::Relaxed);
            self.fade_len.store(self.cut_len.load(Ordering::Relaxed), Ordering::Relaxed);
            self.fade_pos.store(0, Ordering::Relaxed);
            self.fade_end.store(self.skip.load(Ordering::Acquire), Ordering::Relaxed);
        }
        let (fade_len, end) = (self.fade_len.load(Ordering::Relaxed), self.fade_end.load(Ordering::Relaxed));
        let mut fade_pos = self.fade_pos.load(Ordering::Relaxed);
        let mask = RING_FRAMES as u64 - 1;
        let mut n = 0;
        for frame in out[..frames * FRAME_BYTES].as_chunks_mut::<FRAME_BYTES>().0 {
            let gain = if fade_pos < fade_len && r < end {
                fade_pos += 1;
                // Raised cosine, 1 to 0, as the output thread's own fades.
                0.5 + 0.5 * (std::f32::consts::PI * fade_pos as f32 / fade_len as f32).cos()
            } else {
                if r < end {
                    r = end;
                    fade_pos = fade_len;
                }
                if r >= w {
                    break;
                }
                1.0
            };
            let at = ((r & mask) * 2) as usize;
            frame[..4].copy_from_slice(&(f32::from_bits(self.buf[at].load(Ordering::Relaxed)) * gain).to_le_bytes());
            frame[4..].copy_from_slice(&(f32::from_bits(self.buf[at + 1].load(Ordering::Relaxed)) * gain).to_le_bytes());
            r += 1;
            n += 1;
        }
        if fade_pos >= fade_len && r < end {
            r = end;
        }
        self.fade_pos.store(fade_pos, Ordering::Relaxed);
        out[n * FRAME_BYTES..frames * FRAME_BYTES].fill(0);
        self.read.store(r, Ordering::Release);
        n
    }

    /// Consumer (real-time): after a cycle that handed `real` frames of audio (of `want`),
    /// with `delay` frames (stream rate) between the stream and what is heard.
    fn after_cycle(&self, real: usize, want: usize, delay: u64) {
        let now = self.now_ns();
        let left = self.in_pipe();
        let pipe = (left + real as u64).min(delay + want as u64);
        self.pipe.store(pipe, Ordering::Release);
        self.pipe_ns.store(now, Ordering::Release);
        self.quantum.store(want as u32, Ordering::Relaxed);
        if real < want && self.in_ring() == 0 && real > 0 {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        self.waiter.unpark();
    }
}

/// The process callback: one graph cycle.
fn process(stream: &pw::stream::Stream, ring: &Ring) {
    let Some(mut buffer) = stream.dequeue_buffer() else { return };
    let requested = buffer.requested() as usize;
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else { return };
    let (real, want) = match d.data() {
        Some(slice) => {
            let max = slice.len() / FRAME_BYTES;
            let want = if requested > 0 { requested.min(max) } else { max };
            (ring.take(slice, want), want)
        }
        None => (0, 0),
    };
    let chunk = d.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = FRAME_BYTES as i32;
    *chunk.size_mut() = (want * FRAME_BYTES) as u32;
    drop(buffer);
    let mut delay = 0u64;
    if let Ok(t) = stream.time() {
        let r = t.rate();
        if r.num == 1 && r.denom > 0 {
            ring.graph_rate.store(r.denom, Ordering::Relaxed);
            delay = (t.delay().max(0) as u64 * u64::from(ring.rate)) / u64::from(r.denom);
        }
        delay += t.buffered();
    }
    ring.after_cycle(real, want, delay);
}

// ---------------------------------------------------------------------------------------
// The PipeWire thread.

#[derive(Clone, Debug)]
struct Sink {
    id: u32,
    /// `node.name`: stable across restarts, the device's id here.
    name: String,
    description: String,
}

#[derive(Default)]
struct Sinks {
    list: Vec<Sink>,
    /// The default sink's `node.name`: the "default" metadata's `default.audio.sink`, the
    /// default in effect, which the session manager keeps (the user's own choice,
    /// `default.configured.audio.sink`, may name a device that is not there).
    default: Option<String>,
    /// The graph's links: (link id, output node, input node).
    links: Vec<(u32, u32, u32)>,
}

impl Sinks {
    /// The sink a node is linked to now, if it is (the latest link while the session
    /// manager moves it).
    fn linked(&self, node: u32) -> Option<&Sink> {
        self.links.iter().rev().filter(|l| l.1 == node).find_map(|l| self.list.iter().find(|k| k.id == l.2))
    }
}

/// A node the app can play to: a device's output, a virtual or filter sink (an equalizer's,
/// a combined output), or a pro-audio profile's duplex node.
fn is_sink(class: Option<&str>) -> bool {
    matches!(class, Some("Audio/Sink" | "Audio/Sink/Virtual" | "Audio/Duplex"))
}

/// Called when a sink comes or goes or the default changes (`on_devices_changed`).
static ON_DEVICES: Mutex<Option<fn()>> = Mutex::new(None);

/// `f` is called whenever the sinks or the default change, on the PipeWire thread (it must
/// only hand the news on).
pub fn on_devices_changed(f: fn()) {
    *ON_DEVICES.lock().unwrap() = Some(f);
}

fn devices_changed() {
    let f = *ON_DEVICES.lock().unwrap();
    if let Some(f) = f {
        f();
    }
}

/// No node yet (the stream is not connected).
const NO_NODE: u32 = u32::MAX;

enum Cmd {
    Open { rate: u32, target: Option<String>, force_rate: bool, ring: Arc<Ring>, reply: mpsc::Sender<Result<(), String>> },
    Active(bool),
    Flush,
    Close,
    Quit,
}

struct Backend {
    tx: Mutex<pw::channel::Sender<Cmd>>,
    sinks: Arc<Mutex<Sinks>>,
    /// Bumped whenever a sink comes or goes or the default changes.
    seq: Arc<AtomicU64>,
    /// The connection to PipeWire was lost: a new backend is started on next use.
    lost: Arc<AtomicBool>,
}

impl Backend {
    fn send(&self, cmd: Cmd) -> bool {
        self.tx.lock().unwrap().send(cmd).is_ok()
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.send(Cmd::Quit);
    }
}

static BACKEND: Mutex<(Option<Arc<Backend>>, Option<Instant>)> = Mutex::new((None, None));

/// The running backend, started (or restarted, after PipeWire went away) when needed.
/// `None` when PipeWire cannot be reached; tried again at most every few seconds.
fn backend() -> Option<Arc<Backend>> {
    let mut g = BACKEND.lock().unwrap();
    if g.0.as_ref().is_some_and(|b| b.lost.load(Ordering::Acquire)) {
        log::warn!("pipewire: the connection was lost; reconnecting");
        g.0 = None;
    }
    if g.0.is_none() && g.1.is_none_or(|at| at.elapsed() > Duration::from_secs(3)) {
        g.0 = start().map(Arc::new);
        if g.0.is_none() {
            g.1 = Some(Instant::now());
        }
    }
    g.0.clone()
}

/// PipeWire is there to play through: `MP_AUDIO=alsa` not set, the server reachable, and
/// an audio sink listed (one is waited for a moment, for a login where the session manager
/// is still setting up the devices; a PipeWire that only carries video, beside PulseAudio,
/// lists none).
pub fn usable() -> bool {
    if std::env::var("MP_AUDIO").is_ok_and(|v| v.eq_ignore_ascii_case("alsa")) {
        log::info!("MP_AUDIO=alsa: not using PipeWire");
        return false;
    }
    let Some(b) = backend() else { return false };
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        if !b.sinks.lock().unwrap().list.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            log::warn!("pipewire: connected, but it lists no audio sink");
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The audio sinks, by name, for Settings.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let b = backend().ok_or("PipeWire is not running")?;
    let s = b.sinks.lock().unwrap();
    let mut out: Vec<DeviceInfo> = s
        .list
        .iter()
        .map(|k| DeviceInfo { id: k.name.clone(), name: k.description.clone(), default: s.default.as_deref() == Some(k.name.as_str()) })
        .collect();
    out.sort_by_key(|d| d.name.to_lowercase());
    Ok(out)
}

fn start() -> Option<Backend> {
    let (tx, rx) = pw::channel::channel::<Cmd>();
    let sinks = Arc::new(Mutex::new(Sinks::default()));
    let seq = Arc::new(AtomicU64::new(0));
    let lost = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (s, q, l) = (sinks.clone(), seq.clone(), lost.clone());
    let spawned = std::thread::Builder::new().name("mp-pipewire".into()).spawn(move || {
        if let Err(e) = run(rx, s, q, l.clone(), &ready_tx) {
            l.store(true, Ordering::Release);
            let _ = ready_tx.send(Err(e));
        }
    });
    if let Err(e) = spawned {
        log::warn!("pipewire: no thread ({e})");
        return None;
    }
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => {
            let n = sinks.lock().unwrap().list.len();
            log::info!("pipewire: connected; {n} audio sink(s)");
            Some(Backend { tx: Mutex::new(tx), sinks, seq, lost })
        }
        Ok(Err(e)) => {
            log::warn!("pipewire: {e}");
            None
        }
        Err(_) => {
            log::warn!("pipewire: no answer from the server in 3 s");
            let _ = tx.send(Cmd::Quit);
            None
        }
    }
}

/// The stream and its listener, kept together.
struct Live {
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<()>,
}

fn props_of<'a>(global: &'a pw::registry::GlobalObject<&'a spa::utils::dict::DictRef>, key: &str) -> Option<&'a str> {
    global.props.and_then(|p| p.get(key))
}

/// The value of `default.audio.sink`: `{"name":"alsa_output…"}`.
fn metadata_name(value: &str) -> Option<String> {
    let v = value.split_once("\"name\"")?.1;
    let v = v.split_once(':')?.1.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

fn run(
    rx: pw::channel::Receiver<Cmd>,
    sinks: Arc<Mutex<Sinks>>,
    seq: Arc<AtomicU64>,
    lost: Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let (name, id) = *IDENTITY.lock().unwrap();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("main loop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("context: {e}"))?;
    let client = pw::properties::properties! {
        *pw::keys::APP_NAME => name,
        *pw::keys::APP_ID => id,
        *pw::keys::APP_ICON_NAME => id,
    };
    let core = context.connect_rc(Some(client)).map_err(|e| format!("cannot connect to the PipeWire server ({e})"))?;
    let registry = core.get_registry_rc().map_err(|e| format!("registry: {e}"))?;

    let metadata: Rc<RefCell<Option<(pw::metadata::Metadata, pw::metadata::MetadataListener)>>> = Rc::default();
    let bump = {
        let seq = seq.clone();
        move || {
            seq.fetch_add(1, Ordering::AcqRel);
        }
    };
    let _registry_listener = {
        let (sinks, registry_, metadata, bump) = (sinks.clone(), registry.clone(), metadata.clone(), bump.clone());
        let (sinks_rm, bump_rm) = (sinks.clone(), bump.clone());
        registry
            .add_listener_local()
            .global(move |global| match global.type_ {
                pw::types::ObjectType::Node if is_sink(props_of(global, "media.class")) => {
                    let Some(name) = props_of(global, "node.name") else { return };
                    let description =
                        props_of(global, "node.description").or_else(|| props_of(global, "node.nick")).unwrap_or(name).to_owned();
                    sinks.lock().unwrap().list.push(Sink { id: global.id, name: name.to_owned(), description });
                    bump();
                    devices_changed();
                }
                // Where streams go: which sink the session manager linked ours to.
                pw::types::ObjectType::Link => {
                    let node = |k| props_of(global, k).and_then(|v| v.parse::<u32>().ok());
                    if let (Some(out), Some(inp)) = (node("link.output.node"), node("link.input.node")) {
                        sinks.lock().unwrap().links.push((global.id, out, inp));
                        bump();
                    }
                }
                pw::types::ObjectType::Metadata if props_of(global, "metadata.name") == Some("default") => {
                    let Ok(m) = registry_.bind::<pw::metadata::Metadata, _>(global) else { return };
                    let (sinks, bump) = (sinks.clone(), bump.clone());
                    let listener = m
                        .add_listener_local()
                        .property(move |subject, key, _type, value| {
                            if subject == pw::core::PW_ID_CORE && (key == Some("default.audio.sink") || key.is_none()) {
                                let name = value.and_then(metadata_name);
                                let mut s = sinks.lock().unwrap();
                                if s.default != name {
                                    log::info!("pipewire: the default sink is {}", name.as_deref().unwrap_or("none"));
                                    s.default = name;
                                    drop(s);
                                    bump();
                                    devices_changed();
                                }
                            }
                            0
                        })
                        .register();
                    *metadata.borrow_mut() = Some((m, listener));
                }
                _ => {}
            })
            .global_remove(move |id| {
                let mut s = sinks_rm.lock().unwrap();
                let (sinks, links) = (s.list.len(), s.links.len());
                s.list.retain(|k| k.id != id);
                s.links.retain(|l| l.0 != id);
                let sink_gone = s.list.len() != sinks;
                if sink_gone || s.links.len() != links {
                    drop(s);
                    bump_rm();
                    if sink_gone {
                        devices_changed();
                    }
                }
            })
            .register()
    };

    // The registry's first answers (every sink, the default metadata) come before this sync's
    // done; the default itself comes once the metadata is bound, which a second round trip
    // waits for. Until then the default is not known, and a stream opened or a list made then
    // would take the first sink for it.
    let pending = core.sync(0).map_err(|e| format!("sync: {e}"))?;
    let second = Rc::new(std::cell::Cell::new(None::<pw::spa::utils::result::AsyncSeq>));
    let synced = Rc::new(std::cell::Cell::new(false));
    let _core_listener = {
        let (synced, ready, lost, ml, core_) = (synced.clone(), ready.clone(), lost.clone(), mainloop.clone(), core.clone());
        core.add_listener_local()
            .done(move |id, s| {
                if id != pw::core::PW_ID_CORE || synced.get() {
                    return;
                }
                if s == pending && second.get().is_none() {
                    match core_.sync(0) {
                        Ok(seq) => second.set(Some(seq)),
                        Err(_) => {
                            synced.set(true);
                            let _ = ready.send(Ok(()));
                        }
                    }
                } else if second.get() == Some(s) {
                    synced.set(true);
                    let _ = ready.send(Ok(()));
                }
            })
            .error(move |id, _seq, res, message| {
                log::warn!("pipewire: error on object {id}: {message} ({res})");
                // The core itself: the server went away (or refused us).
                if id == pw::core::PW_ID_CORE {
                    lost.store(true, Ordering::Release);
                    ml.quit();
                }
            })
            .register()
    };

    let live: Rc<RefCell<Option<Live>>> = Rc::default();
    let _commands = {
        let (live, core, ml) = (live.clone(), core.clone(), mainloop.clone());
        rx.attach(mainloop.loop_(), move |cmd| match cmd {
            Cmd::Open { rate, target, force_rate, ring, reply } => {
                live.borrow_mut().take();
                let r = open_stream(&core, rate, target.as_deref(), force_rate, ring);
                let _ = reply.send(r.map(|l| {
                    *live.borrow_mut() = Some(l);
                }));
            }
            Cmd::Active(on) => {
                if let Some(l) = live.borrow().as_ref()
                    && let Err(e) = l.stream.set_active(on)
                {
                    log::warn!("pipewire: set_active({on}): {e}");
                }
            }
            Cmd::Flush => {
                if let Some(l) = live.borrow().as_ref() {
                    let _ = l.stream.flush(false);
                }
            }
            Cmd::Close => {
                live.borrow_mut().take();
            }
            Cmd::Quit => {
                live.borrow_mut().take();
                ml.quit();
            }
        })
    };
    mainloop.run();
    live.borrow_mut().take();
    lost.store(true, Ordering::Release);
    Ok(())
}

fn open_stream(core: &pw::core::CoreRc, rate: u32, target: Option<&str>, force_rate: bool, ring: Arc<Ring>) -> Result<Live, String> {
    let (name, id) = *IDENTITY.lock().unwrap();
    let latency = (rate * LATENCY_MS / 1000).max(64);
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::APP_NAME => name,
        *pw::keys::APP_ID => id,
        *pw::keys::APP_ICON_NAME => id,
        *pw::keys::NODE_NAME => id,
        *pw::keys::NODE_DESCRIPTION => name,
        *pw::keys::NODE_LATENCY => format!("{latency}/{rate}"),
        *pw::keys::NODE_RATE => format!("1/{rate}"),
    };
    if force_rate {
        // 0: force the rate `node.rate` asks for, while the stream is active.
        props.insert("node.force-rate", "0");
    }
    if let Some(t) = target {
        props.insert(*pw::keys::TARGET_OBJECT, t);
    }
    let stream = pw::stream::StreamRc::new(core.clone(), name, props).map_err(|e| format!("stream: {e}"))?;
    let (r, dead) = (ring.clone(), ring.clone());
    let listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |stream, _, old, new| {
            log::info!("pipewire: stream {old:?} -> {new:?}");
            dead.node.store(stream.node_id(), Ordering::Release);
            if matches!(new, pw::stream::StreamState::Error(_) | pw::stream::StreamState::Unconnected) {
                dead.dead.store(true, Ordering::Release);
                dead.waiter.unpark();
            }
        })
        .process(move |stream, _| process(stream, &r))
        .register()
        .map_err(|e| format!("stream listener: {e}"))?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(2);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let bytes: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|e| format!("format: {e:?}"))?
    .0
    .into_inner();
    let pod = spa::pod::Pod::from_bytes(&bytes).ok_or("format pod")?;
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS
                | pw::stream::StreamFlags::INACTIVE,
            &mut [pod],
        )
        .map_err(|e| format!("connect: {e}"))?;
    Ok(Live { stream, _listener: listener })
}

// ---------------------------------------------------------------------------------------
// The device.

pub struct PipeWireDevice {
    cfg: OutputConfig,
    backend: Option<Arc<Backend>>,
    ring: Option<Arc<Ring>>,
    rate: u32,
    info: Option<OutputInfo>,
    /// `Backend::seq` when the device list was last looked at.
    seq: u64,
    /// The graph rate `info` last said.
    graph_rate: u32,
}

impl PipeWireDevice {
    pub fn new() -> Self {
        PipeWireDevice { cfg: OutputConfig::default(), backend: None, ring: None, rate: 0, info: None, seq: 0, graph_rate: 0 }
    }

    fn close(&mut self) {
        if self.ring.take().is_some()
            && let Some(b) = &self.backend
        {
            b.send(Cmd::Close);
        }
    }

    /// The sink to play on: the chosen one if listed, else the default (and whether the
    /// chosen one is missing). (Target, `node.name`, description, missing.) Once the stream
    /// is linked, the sink it is linked to is the one named: the session manager may have put
    /// it elsewhere than asked (WirePlumber restores the device the user once moved the app to
    /// in the desktop's mixer, and moves it again when the user does).
    fn resolve(&self, b: &Backend) -> (Option<String>, String, String, bool) {
        let s = b.sinks.lock().unwrap();
        let describe = |name: &str| s.list.iter().find(|k| k.name == name).map(|k| k.description.clone());
        let linked = self.ring.as_ref().map(|r| r.node.load(Ordering::Acquire)).filter(|&n| n != NO_NODE).and_then(|n| s.linked(n));
        if let Some(id) = &self.cfg.device
            && let Some(d) = describe(id)
        {
            let (now, d) = linked.map_or((id.clone(), d), |k| (k.name.clone(), k.description.clone()));
            return (Some(id.clone()), now, d, false);
        }
        if let Some(k) = linked {
            return (None, k.name.clone(), k.description.clone(), self.cfg.device.is_some());
        }
        let default = s.default.clone().or_else(|| s.list.first().map(|k| k.name.clone())).unwrap_or_default();
        let description = describe(&default).unwrap_or_else(|| "System default".to_owned());
        (None, default, description, self.cfg.device.is_some())
    }

    /// `info` with the graph rate the stream runs at now.
    fn describe(&self, device: String, id: String, missing: bool, graph: u32) -> OutputInfo {
        let rate = self.rate;
        let mut info = OutputInfo { device, id, missing, rate, server: Some(SERVER), ..OutputInfo::default() };
        if graph > 0 {
            info.mix_rate = Some(graph);
            if self.cfg.exclusive {
                if graph == rate {
                    info.exclusive = Some(format!("at the track's own {} kHz", khz(rate)));
                } else {
                    info.note = Some(format!(
                        "PipeWire runs at {} kHz, not the track's {} kHz (its allowed rates leave that out, or another program holds the rate); it resamples",
                        khz(graph),
                        khz(rate)
                    ));
                }
            }
        }
        info
    }
}

impl Default for PipeWireDevice {
    fn default() -> Self {
        Self::new()
    }
}

fn khz(rate: u32) -> String {
    let k = f64::from(rate) / 1000.0;
    if rate.is_multiple_of(1000) { format!("{k:.0}") } else { format!("{k:.1}") }
}

impl Drop for PipeWireDevice {
    fn drop(&mut self) {
        self.close();
    }
}

impl Device for PipeWireDevice {
    fn configure(&mut self, cfg: &OutputConfig) {
        self.cfg = cfg.clone();
    }

    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.close();
        let b = backend().ok_or("PipeWire is not running")?;
        self.seq = b.seq.load(Ordering::Acquire);
        let (target, id, device, missing) = self.resolve(&b);
        let ring = Arc::new(Ring::new(rate));
        let (reply, answer) = mpsc::channel();
        if !b.send(Cmd::Open { rate, target: target.clone(), force_rate: self.cfg.exclusive, ring: ring.clone(), reply }) {
            b.lost.store(true, Ordering::Release);
            return Err("the PipeWire thread is gone".into());
        }
        match answer.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("PipeWire did not open the stream in 3 s".into()),
        }
        self.rate = rate;
        self.graph_rate = 0;
        self.info = Some(self.describe(device.clone(), id, missing, 0));
        log::info!(
            "pipewire: opened at {rate} Hz on {device}{}{}",
            if self.cfg.exclusive { ", asking the graph for the track's rate" } else { "" },
            if missing { " (the chosen device is not connected: system default)" } else { "" }
        );
        self.ring = Some(ring);
        self.backend = Some(b);
        Ok(())
    }

    fn wait(&mut self, timeout: Duration) {
        match &self.ring {
            Some(r) if !r.dead.load(Ordering::Acquire) => std::thread::park_timeout(timeout),
            _ => std::thread::sleep(timeout.min(Duration::from_millis(20))),
        }
    }

    fn available(&mut self) -> Result<usize, String> {
        let r = self.ring.as_ref().ok_or("not open")?;
        if r.dead.load(Ordering::Acquire) {
            return Err("the PipeWire stream ended".into());
        }
        let quantum = u64::from(r.quantum.load(Ordering::Relaxed));
        let target = (u64::from(self.rate) * u64::from(TARGET_MS) / 1000).max(quantum * 3).min(RING_FRAMES as u64 / 2);
        let queued = r.in_ring() + r.in_pipe();
        Ok(target.saturating_sub(queued).min(r.room()) as usize)
    }

    fn queued(&mut self) -> usize {
        match &self.ring {
            Some(r) if !r.dead.load(Ordering::Acquire) => (r.in_ring() + r.in_pipe()) as usize,
            _ => 0,
        }
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let r = self.ring.as_ref().ok_or("not open")?;
        if r.dead.load(Ordering::Acquire) {
            return Err("the PipeWire stream ended".into());
        }
        r.push(samples)
    }

    fn start(&mut self) -> Result<(), String> {
        let b = self.backend.as_ref().ok_or("not open")?;
        if self.ring.is_none() || !b.send(Cmd::Active(true)) {
            return Err("not open".into());
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let (Some(b), Some(_)) = (&self.backend, &self.ring) {
            b.send(Cmd::Active(false));
        }
    }

    fn cut(&mut self, fade: usize) -> bool {
        match &self.ring {
            Some(r) if !r.dead.load(Ordering::Acquire) => {
                r.cut(fade);
                true
            }
            _ => false,
        }
    }

    fn flush(&mut self) {
        if let Some(r) = &self.ring {
            r.flush();
            if let Some(b) = &self.backend {
                b.send(Cmd::Flush);
            }
        }
    }

    fn route_changed(&mut self) -> bool {
        let (Some(b), Some(info)) = (&self.backend, &self.info) else { return false };
        if b.lost.load(Ordering::Acquire) {
            return true;
        }
        let seq = b.seq.load(Ordering::Acquire);
        if seq == self.seq {
            return false;
        }
        // Following the default, the session manager moves the stream itself (`info_changed`
        // says where). A chosen device that went or came back: reopen.
        let Some(id) = &self.cfg.device else { return false };
        self.seq = seq;
        let present = b.sinks.lock().unwrap().list.iter().any(|k| &k.name == id);
        present == info.missing
    }

    fn info_changed(&mut self) -> bool {
        let (Some(b), Some(r), Some(info)) = (&self.backend, &self.ring, &self.info) else { return false };
        let graph = r.graph_rate.load(Ordering::Relaxed);
        let mut changed = graph != self.graph_rate;
        let (mut device, mut id) = (info.device.clone(), info.id.clone());
        let (target, now_id, now_device, _) = self.resolve(b);
        // Where the stream plays now: the default it follows, or the sink it is linked to.
        // (A chosen device back while missing: `route_changed` reopens on it; the info waits
        // for that.)
        if !(info.missing && target.is_some()) && now_id != id {
            log::info!("pipewire: the stream plays on {now_device} now");
            (device, id) = (now_device, now_id);
            changed = true;
        }
        if changed {
            self.graph_rate = graph;
            if graph != 0 && graph != self.rate {
                log::info!("pipewire: the graph runs at {graph} Hz; the stream ({} Hz) is resampled", self.rate);
            }
            let missing = info.missing;
            self.info = Some(self.describe(device, id, missing, graph));
        }
        changed
    }

    fn info(&mut self) -> Option<OutputInfo> {
        self.info.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_hands_over_in_order_and_pads_with_silence() {
        let r = Ring::new(48_000);
        let samples: Vec<f32> = (0..20).map(|i| i as f32).collect();
        r.push(&samples).unwrap();
        assert_eq!(r.in_ring(), 10);
        let mut out = vec![0xffu8; 16 * FRAME_BYTES];
        assert_eq!(r.take(&mut out, 16), 10);
        let got: Vec<f32> = out.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        assert_eq!(&got[..20], &samples[..]);
        assert!(got[20..].iter().all(|&s| s == 0.0), "an underrun is silence");
        assert_eq!(r.in_ring(), 0);
    }

    #[test]
    fn a_flush_skips_what_was_not_taken_and_wraps() {
        let r = Ring::new(48_000);
        // Fill and drain past the end of the buffer, so indices wrap.
        let block = vec![0.5f32; 2 * 10_000];
        let mut out = vec![0u8; 10_000 * FRAME_BYTES];
        for _ in 0..5 {
            r.push(&block).unwrap();
            assert_eq!(r.take(&mut out, 10_000), 10_000);
        }
        r.push(&[1.0, 1.0, 2.0, 2.0]).unwrap();
        r.flush();
        assert_eq!(r.in_ring(), 0);
        r.push(&[3.0, -3.0]).unwrap();
        let mut one = vec![0u8; 4 * FRAME_BYTES];
        assert_eq!(r.take(&mut one, 4), 1, "only what came after the flush");
        assert_eq!(f32::from_le_bytes(one[..4].try_into().unwrap()), 3.0);
        assert_eq!(f32::from_le_bytes(one[4..8].try_into().unwrap()), -3.0);
    }

    #[test]
    fn a_cut_fades_out_from_the_next_frame_then_drops_the_rest() {
        let r = Ring::new(48_000);
        r.push(&vec![1.0f32; 2 * 100]).unwrap();
        let mut out = vec![0u8; 10 * FRAME_BYTES];
        assert_eq!(r.take(&mut out, 10), 10);
        // Cut with a fade of 4 frames: the next 4 fade out, the other 86 are dropped, and
        // what is written after the cut follows.
        r.cut(4);
        r.push(&[0.5, 0.5, 0.5, 0.5]).unwrap();
        let mut out = vec![0u8; 8 * FRAME_BYTES];
        assert_eq!(r.take(&mut out, 8), 6);
        let left: Vec<f32> = out.as_chunks::<8>().0.iter().map(|f| f32::from_le_bytes(f[..4].try_into().unwrap())).collect();
        assert!(left[0] < 1.0 && left[0] > left[1] && left[1] > left[2] && left[2] > left[3], "{left:?}");
        assert!(left[3].abs() < 1e-6, "faded to silence: {left:?}");
        assert_eq!(&left[4..], [0.5, 0.5, 0.0, 0.0], "then the new audio, then silence");
        assert_eq!(r.in_ring(), 0);
    }

    #[test]
    fn the_ring_refuses_more_than_it_holds() {
        let r = Ring::new(48_000);
        assert!(r.push(&vec![0.0; RING_FRAMES * 2]).is_ok());
        assert!(r.push(&[0.0, 0.0]).is_err());
    }

    #[test]
    fn what_was_handed_over_drains_as_time_passes() {
        let r = Ring::new(48_000);
        r.after_cycle(960, 960, 960);
        let pipe = r.in_pipe();
        assert!(pipe > 900 && pipe <= 960, "{pipe}");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(r.in_pipe(), 0, "20 ms of audio is heard within 30 ms");
        // Silence after real audio adds nothing.
        r.after_cycle(0, 960, 960);
        assert_eq!(r.in_pipe(), 0);
    }

    #[test]
    fn the_default_sink_is_read_from_its_metadata() {
        assert_eq!(metadata_name(r#"{"name":"alsa_output.usb-DAC.analog-stereo"}"#).as_deref(), Some("alsa_output.usb-DAC.analog-stereo"));
        assert_eq!(metadata_name(r#"{ "name": "a \"b\"" }"#).as_deref(), Some("a \"b\""));
        assert_eq!(metadata_name("{}"), None);
    }
}
