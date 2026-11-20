//! Core Audio output (macOS), the counterpart of `wasapi.rs` and `pipewire.rs`.
//!
//! **Audio.** An output audio unit (AUHAL, `kAudioUnitSubType_HALOutput`) on the chosen
//! device, or the system default, with a render callback that Core Audio calls on its own
//! real-time I/O thread. The stream format is the track's own rate, f32 interleaved stereo;
//! the unit's converter turns it into what the device runs at (its nominal rate, shown as
//! `OutputInfo::mix_rate`), as Windows' shared mode does, so the app never resamples. The
//! output thread (`output.rs`) writes into a single-producer single-consumer ring (`Ring`,
//! atomics only); the callback copies what the unit asks for out of it, pads an underrun with
//! silence, notes how much is still unheard (the device's latency, safety offset and buffer)
//! and unparks the output thread, whose `wait` parks until then. The callback does not
//! allocate, lock, log or make a call that can block. The ring is kept filled to 50 ms or
//! three callback periods, whichever is more, the same view the output thread has of WASAPI's
//! 40 ms device buffer: `queued` is the ring plus what the device has not played yet.
//!
//! **Pause, seek, stop.** `stop` stops the unit (`AudioOutputUnitStop`), `start` starts it.
//! A seek, play-now or stop is a `cut`: the callback fades out the very next frames it hands
//! the device (5 ms) and drops the rest, as with PipeWire, since what the unit already took
//! is gone from reach. `flush` drops what the ring holds and resets the unit.
//!
//! **Devices.** Listed with the AudioObject properties of the system object (only devices
//! with output channels), by their UID (stable across restarts and reconnections) and name.
//! A chosen device that is not there means the default meanwhile (`OutputInfo::missing`);
//! when it comes back, the stream returns to it. Property listeners on the system object
//! (the device list, the default output device) say when to look again, on a thread of
//! Core Audio's: nothing is polled. The unit is bound to one device, so following the
//! default is a reopen on the new one.
//!
//! **Exclusive mode** (`OutputConfig::exclusive`: hog mode, the device switched to the
//! track's rate) is not implemented here: the stream plays shared, and the info carries a
//! note saying so.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use objc2_audio_toolbox::{
    AURenderCallbackStruct, AudioComponentDescription, AudioComponentFindNext, AudioComponentInstanceDispose,
    AudioComponentInstanceNew, AudioOutputUnitStart, AudioOutputUnitStop, AudioUnit, AudioUnitGetProperty,
    AudioUnitInitialize, AudioUnitRenderActionFlags, AudioUnitReset, AudioUnitSetProperty, AudioUnitUninitialize,
    kAudioOutputUnitProperty_CurrentDevice, kAudioUnitManufacturer_Apple, kAudioUnitProperty_Latency,
    kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitProperty_SetRenderCallback, kAudioUnitProperty_StreamFormat,
    kAudioUnitScope_Global, kAudioUnitScope_Input, kAudioUnitSubType_HALOutput, kAudioUnitType_Output,
};
use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyDeviceIsAlive,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyLatency, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject, kAudioObjectUnknown, kAudioStreamPropertyLatency,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagsNativeFloatPacked,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFRetained, CFString};

use super::{Device, DeviceInfo};
use crate::engine::{OutputConfig, OutputInfo};

/// The ring's capacity in frames (a power of two): 0.68 s at 48 kHz, 85 ms at 384 kHz,
/// always more than `available` offers.
const RING_FRAMES: usize = 1 << 15;
/// What the ring is kept filled to, at least.
const TARGET_MS: u32 = 50;
/// The most frames the unit asks for in one callback (`kAudioUnitProperty_MaximumFramesPerSlice`):
/// a device buffer of 4096 at a high rate, converted up from a low one, stays within it.
const MAX_SLICE: u32 = 8192;
const SERVER: &str = "Core Audio";
const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

// ---------------------------------------------------------------------------------------
// The ring between the output thread and the render callback.

/// Single producer (the output thread), single consumer (Core Audio's I/O thread).
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
    /// Frames handed to the device and not heard yet, as of `pipe_ns` (the consumer's view).
    pipe: AtomicU64,
    pipe_ns: AtomicU64,
    /// Frames (stream rate) between the callback and what is heard: set at open.
    delay: AtomicU64,
    /// The frames the unit asked for last callback.
    quantum: AtomicU32,
    underruns: AtomicU64,
    base: Instant,
    /// The output thread, unparked every callback (`Device::wait` parks).
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
            delay: AtomicU64::new(0),
            quantum: AtomicU32::new(0),
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

    /// Frames handed to the device and not heard yet, now.
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
            return Err("Core Audio buffer full".into());
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
    /// (they follow what the device has, so the cut is continuous).
    fn cut(&self, fade: usize) {
        self.cut_len.store(fade as u32, Ordering::Relaxed);
        self.skip.store(self.write.load(Ordering::Relaxed), Ordering::Release);
        self.cut_seq.fetch_add(1, Ordering::Release);
    }

    /// Producer: drop everything not taken yet, at once (the unit is stopped and reset).
    fn flush(&self) {
        self.cut(0);
        self.pipe.store(0, Ordering::Release);
    }

    /// Consumer (real-time): fill `out` (interleaved stereo) with up to `out.len() / 2`
    /// frames of the ring, silence after it. Returns the frames of real audio.
    fn take(&self, out: &mut [f32]) -> usize {
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
        let frames = out.len() / 2;
        let mut n = 0;
        for frame in out.as_chunks_mut::<2>().0 {
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
            frame[0] = f32::from_bits(self.buf[at].load(Ordering::Relaxed)) * gain;
            frame[1] = f32::from_bits(self.buf[at + 1].load(Ordering::Relaxed)) * gain;
            r += 1;
            n += 1;
        }
        if fade_pos >= fade_len && r < end {
            r = end;
        }
        self.fade_pos.store(fade_pos, Ordering::Relaxed);
        out[n * 2..frames * 2].fill(0.0);
        self.read.store(r, Ordering::Release);
        n
    }

    /// Consumer (real-time): after a callback that handed `real` frames of audio (of `want`).
    fn after_cycle(&self, real: usize, want: usize) {
        let now = self.now_ns();
        let left = self.in_pipe();
        let delay = self.delay.load(Ordering::Relaxed);
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

/// The render callback: Core Audio's I/O thread asks for `frames` frames. `refcon` is the
/// `Ring` (kept alive by the device until the unit is disposed).
unsafe extern "C-unwind" fn render(
    refcon: NonNull<c_void>,
    _flags: NonNull<AudioUnitRenderActionFlags>,
    _time: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    // SAFETY: `refcon` is `Arc::as_ptr` of the ring the device holds while the unit lives.
    let ring = unsafe { refcon.cast::<Ring>().as_ref() };
    let Some(list) = (unsafe { data.as_mut() }) else { return 0 };
    if list.mNumberBuffers == 0 {
        return 0;
    }
    // One interleaved buffer: the stream format set at open.
    let buffer: &mut AudioBuffer = &mut list.mBuffers[0];
    if buffer.mData.is_null() {
        return 0;
    }
    let len = (buffer.mDataByteSize as usize / 4).min(frames as usize * 2);
    // SAFETY: the unit hands a buffer of `mDataByteSize` bytes of f32 samples.
    let out = unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), len) };
    let real = ring.take(out);
    ring.after_cycle(real, len / 2);
    0
}

// ---------------------------------------------------------------------------------------
// AudioObject properties.

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

fn status(what: &str, s: i32) -> Result<(), String> {
    if s == 0 { Ok(()) } else { Err(format!("{what} failed (OSStatus {s})")) }
}

/// A fixed-size property of an object.
fn get<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let addr = address(selector, scope);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: the address and the out pointer are valid, `size` is the out buffer's size.
    let s = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast()),
        )
    };
    // SAFETY: Core Audio filled it on success, with the size asked for.
    (s == 0 && size as usize == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

/// A variable-size property, as bytes (8-aligned, for the structures read out of it).
fn get_bytes(object: AudioObjectID, selector: u32, scope: u32) -> Option<Vec<u64>> {
    let addr = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: valid address and out pointer.
    let s = unsafe { AudioObjectGetPropertyDataSize(object, NonNull::from(&addr), 0, ptr::null(), NonNull::from(&mut size)) };
    if s != 0 {
        return None;
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: the buffer holds at least `size` bytes.
    let s = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(buf.as_mut_ptr().cast()),
        )
    };
    if s != 0 {
        return None;
    }
    buf.truncate((size as usize).div_ceil(8));
    Some(buf)
}

/// A string property (a CFString the caller owns).
fn get_string(object: AudioObjectID, selector: u32) -> Option<String> {
    let p: *const CFString = get(object, selector, kAudioObjectPropertyScopeGlobal)?;
    // SAFETY: a "copy" property: the CFString is retained for the caller, released on drop.
    let s = unsafe { CFRetained::from_raw(NonNull::new(p.cast_mut())?) };
    Some(s.to_string())
}

/// Every device the system knows.
fn all_devices() -> Vec<AudioObjectID> {
    let Some(buf) = get_bytes(SYSTEM, kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal) else { return Vec::new() };
    let bytes: &[u8] = as_bytes(&buf);
    bytes.as_chunks::<4>().0.iter().map(|b| AudioObjectID::from_ne_bytes(*b)).collect()
}

fn as_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: u64 to its bytes, the same memory, every bit pattern valid.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) }
}

/// The device's output channels, from its output stream configuration (an AudioBufferList).
fn output_channels(device: AudioObjectID) -> u32 {
    let Some(buf) = get_bytes(device, kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeOutput) else { return 0 };
    let bytes = as_bytes(&buf);
    if bytes.len() < 4 {
        return 0;
    }
    let count = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
    // The buffers follow the count, at the alignment of their pointer field.
    let first = std::mem::offset_of!(AudioBufferList, mBuffers);
    let stride = size_of::<AudioBuffer>();
    (0..count)
        .filter_map(|i| {
            let at = first + i * stride;
            bytes.get(at..at + 4).map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
        })
        .sum()
}

fn default_output() -> Option<AudioObjectID> {
    get::<AudioObjectID>(SYSTEM, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal)
        .filter(|&d| d != kAudioObjectUnknown)
}

fn is_alive(device: AudioObjectID) -> bool {
    get::<u32>(device, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal).is_some_and(|a| a != 0)
}

struct Found {
    object: AudioObjectID,
    uid: String,
    name: String,
}

fn describe(object: AudioObjectID) -> Option<Found> {
    let uid = get_string(object, kAudioDevicePropertyDeviceUID)?;
    let name = get_string(object, kAudioObjectPropertyName).unwrap_or_else(|| uid.clone());
    Some(Found { object, uid, name })
}

/// The devices that can play (output channels, alive).
fn output_devices() -> Vec<Found> {
    all_devices().into_iter().filter(|&d| output_channels(d) > 0 && is_alive(d)).filter_map(describe).collect()
}

/// The output devices, by name, for Settings. Opens nothing; callable from any thread.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let default = default_output();
    let mut out: Vec<DeviceInfo> = output_devices()
        .into_iter()
        .map(|f| DeviceInfo { default: Some(f.object) == default, id: f.uid, name: f.name })
        .collect();
    out.sort_by_key(|d| d.name.to_lowercase());
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Device notifications: listeners on the system object, registered once for the process.

/// Raised by every notification (`Device::route_changed` compares it).
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The app's hook (`output::on_devices_changed`).
static HOOK: Mutex<Option<fn()>> = Mutex::new(None);
static LISTEN: Once = Once::new();

unsafe extern "C-unwind" fn changed(_object: AudioObjectID, _count: u32, _addresses: NonNull<AudioObjectPropertyAddress>, _data: *mut c_void) -> i32 {
    SEQ.fetch_add(1, Ordering::AcqRel);
    if let Some(f) = *HOOK.lock().unwrap_or_else(|e| e.into_inner()) {
        f();
    }
    0
}

/// Listen to the device list and the default output device (once).
fn listen() {
    LISTEN.call_once(|| {
        for selector in [kAudioHardwarePropertyDevices, kAudioHardwarePropertyDefaultOutputDevice] {
            let addr = address(selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: a valid address and a listener that lives as long as the process.
            let s = unsafe { AudioObjectAddPropertyListener(SYSTEM, NonNull::from(&addr), Some(changed), ptr::null_mut()) };
            if s != 0 {
                log::warn!("coreaudio: no device notifications (OSStatus {s}); a change of output is not followed");
            }
        }
    });
}

/// `f` is called whenever a device comes or goes or the default output changes.
pub fn on_devices_changed(f: fn()) {
    *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    listen();
}

// ---------------------------------------------------------------------------------------
// The output unit.

/// An AUHAL instance, initialized, its render callback set; disposed on drop.
struct Unit(AudioUnit);

// The unit is only touched from the output thread; Core Audio calls the callback on its own.
unsafe impl Send for Unit {}

impl Unit {
    fn new() -> Result<Unit, String> {
        let desc = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_HALOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: a valid description; null asks for the first match.
        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), NonNull::from(&desc)) };
        if component.is_null() {
            return Err("no HAL output audio unit".into());
        }
        let mut unit: AudioUnit = ptr::null_mut();
        // SAFETY: a component found above and a valid out pointer.
        status("AudioComponentInstanceNew", unsafe { AudioComponentInstanceNew(component, NonNull::from(&mut unit)) })?;
        if unit.is_null() {
            return Err("AudioComponentInstanceNew returned nothing".into());
        }
        Ok(Unit(unit))
    }

    fn set<T>(&self, what: &str, property: u32, scope: u32, element: u32, value: &T) -> Result<(), String> {
        // SAFETY: a live unit, and a value of the size given.
        let s = unsafe { AudioUnitSetProperty(self.0, property, scope, element, (value as *const T).cast(), size_of::<T>() as u32) };
        status(what, s)
    }

    fn get_f64(&self, property: u32, scope: u32) -> Option<f64> {
        let mut v = 0f64;
        let mut size = size_of::<f64>() as u32;
        // SAFETY: a live unit, a valid out pointer and its size.
        let s = unsafe { AudioUnitGetProperty(self.0, property, scope, 0, NonNull::from(&mut v).cast(), NonNull::from(&mut size)) };
        (s == 0).then_some(v)
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        // SAFETY: a unit from AudioComponentInstanceNew, stopped and disposed once.
        unsafe {
            AudioOutputUnitStop(self.0);
            AudioUnitUninitialize(self.0);
            AudioComponentInstanceDispose(self.0);
        }
    }
}

struct Stream {
    // Dropped first: the callback stops before the ring goes.
    unit: Unit,
    ring: Arc<Ring>,
    running: bool,
}

#[derive(Default)]
pub struct CoreAudioDevice {
    cfg: OutputConfig,
    stream: Option<Stream>,
    /// The device the stream is open on.
    device: Option<AudioObjectID>,
    rate: u32,
    info: Option<OutputInfo>,
    /// `SEQ` when the devices were last looked at.
    seq: u64,
}

impl CoreAudioDevice {
    /// The device to play on: the chosen one if it is there, else the default (and whether
    /// the chosen one is missing).
    fn resolve(&self) -> Result<(Found, bool), String> {
        if let Some(uid) = &self.cfg.device
            && let Some(f) = output_devices().into_iter().find(|f| &f.uid == uid)
        {
            return Ok((f, false));
        }
        let default = default_output().ok_or("no audio output device")?;
        let found = describe(default).ok_or("the default output device has no UID")?;
        Ok((found, self.cfg.device.is_some()))
    }

    /// What is between the render callback and what is heard, in frames at the stream's
    /// rate: the device's latency, safety offset and buffer, its stream's latency (all in
    /// device frames at its nominal rate), and the unit's own (its converter).
    fn delay(unit: &Unit, device: AudioObjectID, rate: u32, nominal: f64) -> u64 {
        let out = kAudioObjectPropertyScopeOutput;
        let mut frames = 0u64;
        for selector in [kAudioDevicePropertyLatency, kAudioDevicePropertySafetyOffset, kAudioDevicePropertyBufferFrameSize] {
            frames += u64::from(get::<u32>(device, selector, out).unwrap_or(0));
        }
        if let Some(buf) = get_bytes(device, kAudioDevicePropertyStreams, out)
            && let Some(first) = as_bytes(&buf).as_chunks::<4>().0.first()
        {
            let stream = AudioObjectID::from_ne_bytes(*first);
            frames += u64::from(get::<u32>(stream, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal).unwrap_or(0));
        }
        let device_secs = if nominal > 0.0 { frames as f64 / nominal } else { 0.0 };
        let unit_secs = unit.get_f64(kAudioUnitProperty_Latency, kAudioUnitScope_Global).unwrap_or(0.0).max(0.0);
        ((device_secs + unit_secs) * f64::from(rate)).round() as u64
    }

    fn open_unit(device: AudioObjectID, rate: u32, ring: &Arc<Ring>) -> Result<Unit, String> {
        let unit = Unit::new()?;
        unit.set("setting the device", kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &device)?;
        let format = AudioStreamBasicDescription {
            mSampleRate: f64::from(rate),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
            mBytesPerPacket: 8,
            mFramesPerPacket: 1,
            mBytesPerFrame: 8,
            mChannelsPerFrame: 2,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        // The format the unit is fed in (its input scope, bus 0); it converts to the device.
        unit.set("setting the stream format", kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &format)?;
        unit.set("setting the slice size", kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &MAX_SLICE)?;
        let callback = AURenderCallbackStruct { inputProc: Some(render), inputProcRefCon: Arc::as_ptr(ring).cast_mut().cast() };
        unit.set("setting the render callback", kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback)?;
        // SAFETY: a live, configured unit.
        status("AudioUnitInitialize", unsafe { AudioUnitInitialize(unit.0) })?;
        Ok(unit)
    }
}

impl Device for CoreAudioDevice {
    fn thread_started(&mut self) {
        // The highest QoS class an application thread gets: a library scan or a busy UI
        // cannot starve the ring. The render callback itself runs on Core Audio's
        // real-time thread.
        // SAFETY: changes only the calling thread's scheduling.
        let r = unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
        if r == 0 {
            log::info!("output thread: QoS user-interactive");
        } else {
            log::warn!("output thread: QoS user-interactive unavailable ({r}); default priority");
        }
    }

    fn configure(&mut self, cfg: &OutputConfig) {
        self.cfg = cfg.clone();
    }

    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.stream = None;
        self.device = None;
        listen();
        // Anything reported before this open is already followed by it.
        self.seq = SEQ.load(Ordering::Acquire);
        let (found, missing) = self.resolve()?;
        let ring = Arc::new(Ring::new(rate));
        let unit = Self::open_unit(found.object, rate, &ring)?;
        let nominal = get::<f64>(found.object, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal).unwrap_or(0.0);
        ring.delay.store(Self::delay(&unit, found.object, rate, nominal), Ordering::Relaxed);
        let mix_rate = (nominal > 0.0).then(|| nominal.round() as u32);
        let mut info = OutputInfo { device: found.name.clone(), id: found.uid, missing, rate, mix_rate, server: Some(SERVER), ..OutputInfo::default() };
        if self.cfg.exclusive {
            info.note = Some("exclusive mode is not supported on macOS; playing shared".into());
        }
        log::info!(
            "coreaudio: opened {} at {rate} Hz, shared (device at {} Hz){}",
            found.name,
            mix_rate.unwrap_or(0),
            if missing { " (the chosen device is not connected: system default)" } else { "" }
        );
        self.stream = Some(Stream { unit, ring, running: false });
        self.device = Some(found.object);
        self.rate = rate;
        self.info = Some(info);
        Ok(())
    }

    fn wait(&mut self, timeout: Duration) {
        match &self.stream {
            Some(s) if s.running => std::thread::park_timeout(timeout),
            _ => std::thread::sleep(timeout.min(Duration::from_millis(20))),
        }
    }

    fn available(&mut self) -> Result<usize, String> {
        let s = self.stream.as_ref().ok_or("not open")?;
        let r = &s.ring;
        let quantum = u64::from(r.quantum.load(Ordering::Relaxed));
        let target = (u64::from(self.rate) * u64::from(TARGET_MS) / 1000).max(quantum * 3).min(RING_FRAMES as u64 / 2);
        let queued = r.in_ring() + r.in_pipe();
        Ok(target.saturating_sub(queued).min(r.room()) as usize)
    }

    fn queued(&mut self) -> usize {
        self.stream.as_ref().map_or(0, |s| (s.ring.in_ring() + s.ring.in_pipe()) as usize)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let s = self.stream.as_ref().ok_or("not open")?;
        if let Some(d) = self.device
            && !is_alive(d)
        {
            return Err("the output device is gone".into());
        }
        s.ring.push(samples)
    }

    fn start(&mut self) -> Result<(), String> {
        let s = self.stream.as_mut().ok_or("not open")?;
        if !s.running {
            // SAFETY: a live, initialized unit.
            status("AudioOutputUnitStart", unsafe { AudioOutputUnitStart(s.unit.0) })?;
            s.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(s) = &mut self.stream
            && s.running
        {
            // SAFETY: a live unit.
            unsafe { AudioOutputUnitStop(s.unit.0) };
            s.running = false;
        }
    }

    fn cut(&mut self, fade: usize) -> bool {
        match &self.stream {
            Some(s) => {
                s.ring.cut(fade);
                true
            }
            None => false,
        }
    }

    fn flush(&mut self) {
        self.stop();
        if let Some(s) = &self.stream {
            s.ring.flush();
            // SAFETY: a live, stopped unit.
            unsafe { AudioUnitReset(s.unit.0, kAudioUnitScope_Global, 0) };
        }
    }

    fn latency(&mut self) -> Duration {
        let Some(s) = &self.stream else { return Duration::ZERO };
        Duration::from_secs_f64(s.ring.delay.load(Ordering::Relaxed) as f64 / f64::from(self.rate.max(1)))
    }

    fn route_changed(&mut self) -> bool {
        if self.stream.is_none() {
            return false;
        }
        let seq = SEQ.load(Ordering::Acquire);
        if seq == self.seq {
            return false;
        }
        self.seq = seq;
        // No device at all: keep the stream (writes fail if it is gone, and the output
        // reopens then).
        let Ok((found, _)) = self.resolve() else { return false };
        self.device != Some(found.object)
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
        let mut out = vec![9.0f32; 32];
        assert_eq!(r.take(&mut out), 10);
        assert_eq!(&out[..20], &samples[..]);
        assert!(out[20..].iter().all(|&s| s == 0.0), "an underrun is silence");
        assert_eq!(r.in_ring(), 0);
    }

    #[test]
    fn a_cut_fades_out_from_the_next_frame_then_drops_the_rest() {
        let r = Ring::new(48_000);
        r.push(&vec![1.0f32; 2 * 100]).unwrap();
        let mut out = vec![0f32; 20];
        assert_eq!(r.take(&mut out), 10);
        r.cut(4);
        r.push(&[0.5, 0.5, 0.5, 0.5]).unwrap();
        let mut out = vec![0f32; 16];
        assert_eq!(r.take(&mut out), 6);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(left[0] < 1.0 && left[0] > left[1] && left[1] > left[2] && left[2] > left[3], "{left:?}");
        assert!(left[3].abs() < 1e-6, "faded to silence: {left:?}");
        assert_eq!(&left[4..], [0.5, 0.5, 0.0, 0.0], "then the new audio, then silence");
    }

    #[test]
    fn a_flush_skips_what_was_not_taken() {
        let r = Ring::new(48_000);
        r.push(&[1.0, 1.0, 2.0, 2.0]).unwrap();
        r.flush();
        assert_eq!(r.in_ring(), 0);
        r.push(&[3.0, -3.0]).unwrap();
        let mut out = vec![0f32; 8];
        assert_eq!(r.take(&mut out), 1);
        assert_eq!(&out[..2], [3.0, -3.0]);
    }

    #[test]
    fn what_was_handed_over_drains_as_time_passes() {
        let r = Ring::new(48_000);
        r.delay.store(960, Ordering::Relaxed);
        r.after_cycle(960, 960);
        let pipe = r.in_pipe();
        assert!(pipe > 900 && pipe <= 960, "{pipe}");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(r.in_pipe(), 0, "20 ms of audio is heard within 30 ms");
    }

    /// Lists the devices of the machine the tests run on; passes with none.
    #[test]
    fn devices_lists_outputs_with_one_default_at_most() {
        let list = devices().unwrap();
        assert!(list.iter().filter(|d| d.default).count() <= 1, "{list:?}");
        assert!(list.iter().all(|d| !d.id.is_empty()), "{list:?}");
    }
}
