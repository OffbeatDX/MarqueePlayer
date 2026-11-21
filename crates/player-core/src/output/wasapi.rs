//! WASAPI, event-driven output on the chosen render device (or the system default).
//!
//! Shared mode (the default): the stream is opened at the track's own sample rate with
//! Windows' converter enabled (AUTOCONVERTPCM), so the app never resamples.
//!
//! Exclusive mode (`OutputConfig::exclusive`): the device is opened at the track's own
//! rate in the best sample format it accepts (`exclusive::negotiate`: 32-bit float, then
//! 32, 24-in-32, 24 and 16-bit integer), with no system mixer, polled (not event-driven:
//! the standard USB audio driver stutters in exclusive event mode). When it accepts
//! nothing, or another application holds it, the stream opens shared and the info
//! carries a note. The output loop lets go of it after a pause (`RELEASE_AFTER`) and when
//! playback stops.
//!
//! The device: a chosen one by id, or the default. A chosen device that is not there
//! (unplugged, disabled) means the default meanwhile (`OutputInfo::missing`); when it
//! comes back, the stream returns to it. Endpoint notifications (`IMMNotificationClient`,
//! on a thread of the audio system) say when to look again: nothing is polled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wasapi::{
    AudioClient, AudioClock, AudioRenderClient, Device as MmDevice, DeviceEnumerator, DeviceEventCallbacks, DeviceEventRegistration,
    DeviceState, Direction, Handle, SampleType, ShareMode, StreamMode, WasapiError, WaveFormat,
};

use super::Device;
use super::exclusive::{self, Converter, FormatProbe, Negotiated, Probe, SampleFormat};
use crate::engine::{OutputConfig, OutputInfo};

/// Requested device buffer: 40 ms. Shared mode refills every engine period (~10 ms),
/// so this is several periods of slack against scheduling hiccups.
const BUFFER_HNS: i64 = 400_000;

const AUDCLNT_E_UNSUPPORTED_FORMAT: u32 = 0x8889_0008;
const AUDCLNT_E_DEVICE_IN_USE: u32 = 0x8889_000A;
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: u32 = 0x8889_000E;
const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: u32 = 0x8889_0019;

pub use super::DeviceInfo;

/// The active render devices, by name. Enumerating opens nothing; callable from any
/// thread (it joins the COM multithreaded apartment).
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let _ = wasapi::initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(err)?;
    let default = enumerator.get_default_device(Direction::Render).and_then(|d| d.get_id()).ok();
    let all = enumerator.get_device_collection(Direction::Render).map_err(err)?;
    let mut out = Vec::new();
    for i in 0..all.get_nbr_devices().map_err(err)? {
        let Ok(d) = all.get_device_at_index(i) else { continue };
        let Ok(id) = d.get_id() else { continue };
        let name = d.get_friendlyname().unwrap_or_else(|_| id.clone());
        out.push(DeviceInfo { default: default.as_deref() == Some(id.as_str()), id, name });
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    Ok(out)
}

struct Stream {
    client: AudioClient,
    render: AudioRenderClient,
    /// Shared mode is event-driven; exclusive mode is polled every `poll`.
    event: Option<Handle>,
    poll: Duration,
    /// Frames between two refills: the engine's period (shared), the stream's (exclusive).
    period: usize,
    /// The stream's clock (what it has played), for the start-up line.
    clock: Option<AudioClock>,
}

/// How long after a stream first starts its health is said (`StartWatch`).
const WATCH_FOR: Duration = Duration::from_secs(3);

/// The first seconds of a newly opened stream, said once in the log: whether it played in
/// real time (its clock against the wall clock), how often the output thread found it
/// empty at a refill, and the longest time between two refills. A stream that stutters
/// right after opening (shared mode coming back after exclusive) shows here.
#[derive(Debug)]
struct StartWatch {
    started: Instant,
    refills: u32,
    dry: u32,
    last: Option<Instant>,
    longest: Duration,
}

impl StartWatch {
    fn new(started: Instant) -> StartWatch {
        StartWatch { started, refills: 0, dry: 0, last: None, longest: Duration::ZERO }
    }

    /// A refill at `now`, with `queued` frames still in the stream. The first one after the
    /// start is not counted dry (nothing has been written since).
    fn refill(&mut self, now: Instant, queued: u32) {
        if let Some(last) = self.last {
            self.longest = self.longest.max(now.saturating_duration_since(last));
            if queued == 0 {
                self.dry += 1;
            }
        }
        self.refills += 1;
        self.last = Some(now);
    }

    fn due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= WATCH_FOR
    }

    /// The line, with `played` seconds by the stream's clock when it is known.
    fn line(&self, now: Instant, mode: &str, played: Option<f64>) -> String {
        let wall = now.saturating_duration_since(self.started).as_secs_f64();
        let clock = played.map_or("its clock unknown".to_owned(), |p| format!("{p:.2} s played by its clock"));
        format!(
            "wasapi: {mode} stream, first {wall:.2} s: {clock}, empty at {} of {} refills, longest between two refills {:.0} ms",
            self.dry,
            self.refills,
            self.longest.as_secs_f64() * 1000.0
        )
    }
}

#[derive(Default)]
pub struct WasapiDevice {
    com_ready: bool,
    stream: Option<Stream>,
    /// Id of the device the stream is open on.
    device_id: Option<String>,
    /// Set by any endpoint notification (default changed, a device added, removed or
    /// changing state): the output thread looks again (`route_changed`).
    changed: Arc<AtomicBool>,
    /// Keeps the notifications registered (unregistered on drop).
    registration: Option<DeviceEventRegistration>,
    /// `write`'s byte buffer, reused (the device takes bytes, the engine has f32).
    bytes: Vec<u8>,
    /// The MMCSS registration of the output thread (see `thread_started`).
    mmcss: Option<mmcss::Registration>,
    cfg: OutputConfig,
    info: Option<OutputInfo>,
    /// Exclusive mode: the device's sample format. `None`: shared, f32.
    converter: Option<Converter>,
    /// Set by `open`: the next start begins a `StartWatch`.
    fresh: bool,
    watch: Option<StartWatch>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn hresult(e: &WasapiError) -> Option<u32> {
    match e {
        WasapiError::Windows(w) => Some(w.code().0 as u32),
        _ => None,
    }
}

fn wave_format(fmt: SampleFormat, rate: u32) -> WaveFormat {
    let (store, valid, float) = fmt.bits();
    WaveFormat::new(store, valid, if float { SampleType::Float } else { SampleType::Int }, rate as usize, 2, None)
}

/// `exclusive::FormatProbe` on a real audio client: `IsFormatSupported` in exclusive
/// mode, with the WAVEFORMATEX and channel-mask variants some drivers need. Keeps the
/// exact structure the device accepted, to initialise with.
struct ClientProbe<'a> {
    client: &'a AudioClient,
    accepted: Option<WaveFormat>,
}

impl FormatProbe for ClientProbe<'_> {
    fn probe(&mut self, rate: u32, fmt: SampleFormat) -> Probe {
        let wave = wave_format(fmt, rate);
        match self.client.is_supported(&wave, ShareMode::Exclusive) {
            Ok(_) => {
                self.accepted = Some(wave);
                return Probe::Accepted;
            }
            Err(e) => match hresult(&e) {
                Some(AUDCLNT_E_DEVICE_IN_USE) => return Probe::Unavailable("the device is in use by another application".into()),
                Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => {
                    return Probe::Unavailable("the device does not allow it: Sound settings, device properties, Advanced".into());
                }
                _ => {}
            },
        }
        match self.client.is_supported_exclusive_with_quirks(&wave) {
            Ok(w) => {
                self.accepted = Some(w);
                Probe::Accepted
            }
            Err(_) => Probe::Rejected,
        }
    }
}

impl WasapiDevice {
    fn ensure_com(&mut self) {
        if !self.com_ready {
            let _ = wasapi::initialize_mta();
            self.com_ready = true;
        }
    }

    /// Ask the audio system to say when the default device changes or a device comes,
    /// goes, or changes state.
    fn register_watch(&mut self, enumerator: &DeviceEnumerator) {
        if self.registration.is_some() {
            return;
        }
        let mut callbacks = DeviceEventCallbacks::new();
        let c = self.changed.clone();
        callbacks.set_default_device_callback(move |direction, _role, _id| {
            if direction == Direction::Render {
                c.store(true, Ordering::Release);
            }
        });
        let c = self.changed.clone();
        callbacks.set_device_added_callback(move |_| c.store(true, Ordering::Release));
        let c = self.changed.clone();
        callbacks.set_device_removed_callback(move |_| c.store(true, Ordering::Release));
        let c = self.changed.clone();
        callbacks.set_device_state_callback(move |_, _| c.store(true, Ordering::Release));
        match enumerator.register_notification_callback(callbacks) {
            Ok(r) => self.registration = Some(r),
            Err(e) => log::warn!("wasapi: no device notifications ({e}); a change of output is not followed"),
        }
    }

    /// The device to play on: the chosen one if it is there, else the default (and
    /// whether the chosen one is missing).
    fn resolve(&self, enumerator: &DeviceEnumerator) -> Result<(MmDevice, bool), String> {
        if let Some(id) = &self.cfg.device {
            if let Ok(d) = enumerator.get_device(id)
                && matches!(d.get_state(), Ok(DeviceState::Active))
            {
                return Ok((d, false));
            }
            return enumerator.get_default_device(Direction::Render).map(|d| (d, true)).map_err(err);
        }
        enumerator.get_default_device(Direction::Render).map(|d| (d, false)).map_err(err)
    }

    /// Shared mode at `rate`; also the device's mix rate (what Windows converts to).
    fn open_shared(device: &MmDevice, rate: u32) -> Result<(Stream, Option<u32>), String> {
        let mut client = device.get_iaudioclient().map_err(err)?;
        // The engine's format: what every shared stream on the device is mixed into.
        let mix_rate = client.get_mixformat().ok().map(|f| f.get_samplespersec()).filter(|&r| r > 0);
        let format = WaveFormat::new(32, 32, SampleType::Float, rate as usize, 2, None);
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: BUFFER_HNS };
        client.initialize_client(&format, Direction::Render, mode, None).map_err(err)?;
        let event = client.set_get_eventhandle().map_err(err)?;
        let render = client.get_audiorenderclient().map_err(err)?;
        let period = client.get_device_period().map_or(0, |(def, _)| frames(def, rate));
        let clock = client.get_audioclock().ok();
        Ok((Stream { client, render, event: Some(event), poll: Duration::ZERO, period, clock }, mix_rate))
    }

    /// Exclusive mode at `rate`, in the best format the device takes. `Err` carries the
    /// note shown to the user; the caller then opens shared.
    fn open_exclusive(device: &MmDevice, rate: u32) -> Result<(Stream, SampleFormat), String> {
        let client = device.get_iaudioclient().map_err(err)?;
        let mut probe = ClientProbe { client: &client, accepted: None };
        let fmt = match exclusive::negotiate(&mut probe, rate) {
            Negotiated::Exclusive(fmt) => fmt,
            Negotiated::Shared { note } => return Err(note),
        };
        let wave = probe.accepted.take().unwrap_or_else(|| wave_format(fmt, rate));
        let (default_period, _) = client.get_device_period().map_err(err)?;
        // Intel HDA devices want buffers in multiples of 128 bytes.
        let period = client.calculate_aligned_period_near(default_period, Some(128), &wave).map_err(err)?;
        let mut client = client;
        let attempt = |client: &mut AudioClient, (buffer_duration_hns, period_hns): (i64, i64)| {
            let mode = StreamMode::PollingExclusive { buffer_duration_hns, period_hns };
            client.initialize_client(&wave, Direction::Render, mode, None)
        };
        let mut used = exclusive_request(period);
        if let Err(e) = attempt(&mut client, used) {
            match hresult(&e) {
                Some(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED) => {
                    // The documented retry: exactly the size the device would take, on a new client.
                    let frames = client.get_buffer_size().map_err(err)?;
                    used = aligned_retry(frames, rate, period);
                    log::info!("wasapi: exclusive buffer not aligned; retrying with {frames} frames");
                    client = device.get_iaudioclient().map_err(err)?;
                    attempt(&mut client, used).map_err(|e| format!("Exclusive mode failed ({e}); playing shared"))?;
                }
                Some(AUDCLNT_E_DEVICE_IN_USE) => return Err("The device is in use by another application; playing shared".into()),
                Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => return Err("The device does not allow exclusive mode; playing shared".into()),
                Some(AUDCLNT_E_UNSUPPORTED_FORMAT) => return Err(format!("The device refused {}; playing shared", exclusive::describe(fmt, rate))),
                _ => return Err(format!("Exclusive mode failed ({e}); playing shared")),
            }
        }
        let render = client.get_audiorenderclient().map_err(err)?;
        let clock = client.get_audioclock().ok();
        Ok((Stream { client, render, event: None, poll: poll_interval(used.1), period: frames(used.1, rate), clock }, fmt))
    }
}

/// The buffer and period (100 ns units) first asked of an exclusive stream whose device
/// period is `period`: at least `BUFFER_HNS` of buffer, and four periods.
fn exclusive_request(period: i64) -> (i64, i64) {
    (BUFFER_HNS.max(period * 4), period)
}

/// The buffer and period for the retry after `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`: exactly
/// the `frames` the device reported it would take (at `rate`) as the buffer, and the
/// device period, never longer than that buffer.
fn aligned_retry(frames: u32, rate: u32, period: i64) -> (i64, i64) {
    let buffer = wasapi::calculate_period_100ns(i64::from(frames), i64::from(rate.max(1)));
    (buffer, period.min(buffer).max(1))
}

/// A duration in 100 ns units as frames at `rate`.
fn frames(hns: i64, rate: u32) -> usize {
    (hns.max(0) as u128 * u128::from(rate) / 10_000_000) as usize
}

/// How often a polled exclusive stream is topped up: every half period (at least 1 ms).
fn poll_interval(period_hns: i64) -> Duration {
    Duration::from_nanos((period_hns.max(10_000) as u64) * 100 / 2)
}

impl Device for WasapiDevice {
    fn thread_started(&mut self) {
        self.mmcss = mmcss::register();
    }

    fn configure(&mut self, cfg: &OutputConfig) {
        self.cfg = cfg.clone();
    }

    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.ensure_com();
        self.stream = None;
        self.converter = None;
        self.watch = None;
        let enumerator = DeviceEnumerator::new().map_err(err)?;
        self.register_watch(&enumerator);
        // Anything reported before this open is already followed by it.
        self.changed.store(false, Ordering::Release);
        let (device, missing) = self.resolve(&enumerator)?;
        let id = device.get_id().map_err(err)?;
        let name = device.get_friendlyname().unwrap_or_else(|_| id.clone());
        let mut info = OutputInfo { device: name.clone(), id: id.clone(), missing, rate, ..OutputInfo::default() };
        if self.cfg.exclusive {
            match Self::open_exclusive(&device, rate) {
                Ok((stream, fmt)) => {
                    info.exclusive = Some(exclusive::describe(fmt, rate));
                    info.format = Some(fmt.describe().to_owned());
                    self.converter = Some(Converter::new(fmt));
                    self.stream = Some(stream);
                }
                Err(note) => {
                    log::warn!("wasapi: {note}");
                    info.note = Some(note);
                }
            }
        }
        if self.stream.is_none() {
            let (stream, mix_rate) = Self::open_shared(&device, rate)?;
            self.stream = Some(stream);
            info.mix_rate = mix_rate;
        }
        self.device_id = Some(id);
        self.fresh = true;
        log::info!(
            "wasapi: opened {name} at {rate} Hz, {}{}",
            info.exclusive.as_deref().map_or("shared".to_owned(), |f| format!("exclusive {f}")),
            if missing { " (the chosen device is not connected: system default)" } else { "" }
        );
        self.info = Some(info);
        Ok(())
    }

    fn wait(&mut self, timeout: Duration) {
        match &self.stream {
            Some(Stream { event: Some(event), .. }) => {
                let _ = event.wait_for_event(timeout.as_millis() as u32);
            }
            Some(s) => std::thread::sleep(timeout.min(s.poll)),
            None => std::thread::sleep(timeout),
        }
    }

    fn available(&mut self) -> Result<usize, String> {
        let s = self.stream.as_ref().ok_or("not open")?;
        let Some(w) = self.watch.as_mut() else {
            return Ok(s.client.get_available_space_in_frames().map_err(err)? as usize);
        };
        let queued = s.client.get_current_padding().map_err(err)?;
        let size = s.client.get_buffer_size().map_err(err)?;
        let now = Instant::now();
        w.refill(now, queued);
        if w.due(now) {
            let mode = if self.converter.is_some() { "exclusive" } else { "shared" };
            let played = s.clock.as_ref().and_then(|c| {
                let f = c.get_frequency().ok().filter(|&f| f > 0)?;
                c.get_position().ok().map(|(p, _)| p as f64 / f as f64)
            });
            log::info!("{}", w.line(now, mode, played));
            self.watch = None;
        }
        Ok(size.saturating_sub(queued) as usize)
    }

    fn queued(&mut self) -> usize {
        self.stream.as_ref().and_then(|s| s.client.get_current_padding().ok()).unwrap_or(0) as usize
    }

    /// Shared and exclusive alike: both are fed by the output thread, and neither may run
    /// dry (shared glitches; an exclusive stream may replay stale audio from its buffer).
    fn period(&mut self) -> Option<usize> {
        self.stream.as_ref().map(|s| s.period).filter(|&p| p > 0)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let s = self.stream.as_ref().ok_or("not open")?;
        self.bytes.clear();
        match &mut self.converter {
            Some(c) => c.convert(samples, &mut self.bytes),
            None => {
                self.bytes.reserve(samples.len() * 4);
                for v in samples {
                    self.bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        s.render.write_to_device(&self.bytes, None).map_err(err)
    }

    fn start(&mut self) -> Result<(), String> {
        self.stream.as_ref().ok_or("not open")?.client.start_stream().map_err(err)?;
        if std::mem::take(&mut self.fresh) {
            self.watch = Some(StartWatch::new(Instant::now()));
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.client.stop_stream();
        }
        // A stop (a pause, a drain, a cut) ends the first seconds early: not said.
        self.watch = None;
    }

    fn route_changed(&mut self) -> bool {
        if self.stream.is_none() || !self.changed.swap(false, Ordering::AcqRel) {
            return false;
        }
        let Ok(enumerator) = DeviceEnumerator::new() else { return false };
        // No device at all: keep the stream (writes fail if it is gone, and the output
        // reopens then).
        let Ok((device, _)) = self.resolve(&enumerator) else { return false };
        device.get_id().ok().is_some_and(|id| self.device_id.as_ref() != Some(&id))
    }

    fn info(&mut self) -> Option<OutputInfo> {
        self.info.clone()
    }

    fn release(&mut self) -> bool {
        if self.converter.is_none() || self.stream.is_none() {
            return false;
        }
        self.stop();
        self.stream = None;
        self.converter = None;
        log::info!("wasapi: exclusive device released");
        true
    }

    fn holds_device(&self) -> bool {
        self.converter.is_some() && self.stream.is_some()
    }

    /// Exclusive: run dry, the hardware plays its buffer again.
    fn replays_when_dry(&self) -> bool {
        self.holds_device()
    }

    fn latency(&mut self) -> Duration {
        // Stream latency plus one device period: written audio is played no sooner.
        let Some(s) = &self.stream else { return Duration::ZERO };
        let stream = s.client.get_stream_latency().unwrap_or(0).max(0) as u64;
        let period = s.client.get_device_period().map(|(def, _)| def).unwrap_or(0).max(0) as u64;
        Duration::from_nanos((stream + period) * 100)
    }

    fn flush(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.client.stop_stream();
            let _ = s.client.reset_stream();
        }
    }
}

/// The output thread under MMCSS ("Pro Audio"): the multimedia class scheduler runs it
/// at a raised priority, so a library scan (up to 16 tag readers, image resizing) or a
/// busy UI cannot starve the 40 ms device buffer.
mod mmcss {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW};

    pub struct Registration(HANDLE);

    // The handle is only used (reverted) by the thread that registered it.
    unsafe impl Send for Registration {}

    pub fn register() -> Option<Registration> {
        let task: Vec<u16> = "Pro Audio".encode_utf16().chain(Some(0)).collect();
        let mut index = 0u32;
        // SAFETY: a NUL-terminated UTF-16 task name and a valid out pointer.
        let h = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
        if h.is_null() {
            log::warn!("output thread: MMCSS (Pro Audio) unavailable; normal priority");
            None
        } else {
            log::info!("output thread: MMCSS Pro Audio");
            Some(Registration(h))
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this thread.
            unsafe { AvRevertMmThreadCharacteristics(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_request_is_at_least_the_buffer_and_four_periods() {
        assert_eq!(exclusive_request(30_000), (BUFFER_HNS, 30_000));
        assert_eq!(exclusive_request(200_000), (800_000, 200_000));
    }

    #[test]
    fn the_aligned_retry_asks_for_exactly_the_reported_buffer() {
        // 44.1 kHz, 3 ms period: the device rounds the 40 ms buffer to 1792 frames.
        let (buffer, period) = aligned_retry(1792, 44_100, 30_000);
        assert_eq!(buffer, wasapi::calculate_period_100ns(1792, 44_100));
        assert_eq!(buffer, 406_349);
        assert_eq!(period, 30_000);
        assert_ne!(buffer, BUFFER_HNS.max(buffer * 4));
        // A reported buffer shorter than the period: the period shrinks to it.
        assert_eq!(aligned_retry(64, 48_000, 30_000), (13_333, 13_333));
    }

    #[test]
    fn the_start_up_line_says_empty_refills_and_the_longest_gap() {
        let t = Instant::now();
        let mut w = StartWatch::new(t);
        // The first refill after the start finds it empty (nothing written yet): not dry.
        w.refill(t, 0);
        w.refill(t + Duration::from_millis(10), 900);
        w.refill(t + Duration::from_millis(210), 0);
        w.refill(t + Duration::from_millis(220), 480);
        assert!(!w.due(t + Duration::from_millis(2999)));
        assert!(w.due(t + WATCH_FOR));
        assert_eq!(
            w.line(t + WATCH_FOR, "shared", Some(1.234)),
            "wasapi: shared stream, first 3.00 s: 1.23 s played by its clock, empty at 1 of 4 refills, longest between two refills 200 ms"
        );
        assert!(w.line(t + WATCH_FOR, "exclusive", None).contains("its clock unknown"));
    }

    #[test]
    fn polling_follows_the_period_used() {
        assert_eq!(poll_interval(30_000), Duration::from_micros(1_500));
        assert_eq!(poll_interval(1), Duration::from_micros(500));
    }
}
