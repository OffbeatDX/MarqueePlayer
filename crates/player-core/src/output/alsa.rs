//! ALSA's default device: the fallback on a Linux system without PipeWire to play through
//! (an older one with PulseAudio, whose ALSA plugin `default` then is, or bare ALSA).
//!
//! ALSA's API is the one `Device` describes: a buffer the output thread writes into
//! (`snd_pcm_writei` of what `snd_pcm_avail` offers), what is queued (`snd_pcm_delay`), a
//! wait for room (`snd_pcm_wait`), drop and prepare for a flush. The device is `default`,
//! opened at the track's own rate (the `plug` layer, or the sound server behind it,
//! converts when the hardware cannot), 80 ms of buffer in 10 ms periods. No device list:
//! `default` is the sound server's or the system's choice. Stopped, the PCM is dropped and
//! nothing runs.

use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM, State};
use alsa::{Direction, ValueOr};

use super::{Device, DeviceInfo};
use crate::engine::{OutputConfig, OutputInfo};

const DEVICE: &str = "default";
const BUFFER_US: u32 = 80_000;
const PERIOD_US: u32 = 10_000;

/// Only the default: ALSA's own list names hardware the sound server may be holding.
pub fn devices() -> Vec<DeviceInfo> {
    Vec::new()
}

#[derive(Default)]
pub struct AlsaDevice {
    pcm: Option<PCM>,
    rate: u32,
    info: Option<OutputInfo>,
    /// What was queued when `stop` dropped the PCM (`cut` asks after stopping).
    stopped_queued: Option<usize>,
    _cfg: OutputConfig,
}

fn err(e: alsa::Error) -> String {
    e.to_string()
}

impl AlsaDevice {
    fn pcm(&self) -> Result<&PCM, String> {
        self.pcm.as_ref().ok_or_else(|| "not open".to_owned())
    }

    /// After an underrun (or a suspend), make the PCM usable again.
    fn recover(&self, e: alsa::Error) -> Result<(), String> {
        let pcm = self.pcm()?;
        pcm.try_recover(e, true).map_err(err)
    }
}

impl Device for AlsaDevice {
    fn configure(&mut self, cfg: &OutputConfig) {
        self._cfg = cfg.clone();
    }

    fn open(&mut self, rate: u32) -> Result<(), String> {
        self.pcm = None;
        let pcm = PCM::new(DEVICE, Direction::Playback, true).map_err(|e| format!("ALSA {DEVICE}: {e}"))?;
        {
            let hw = HwParams::any(&pcm).map_err(err)?;
            hw.set_access(Access::RWInterleaved).map_err(err)?;
            hw.set_format(Format::float()).map_err(err)?;
            hw.set_channels(2).map_err(err)?;
            hw.set_rate_resample(true).map_err(err)?;
            hw.set_rate(rate, ValueOr::Nearest).map_err(|e| format!("ALSA cannot play at {rate} Hz ({e})"))?;
            hw.set_buffer_time_near(BUFFER_US, ValueOr::Nearest).map_err(err)?;
            hw.set_period_time_near(PERIOD_US, ValueOr::Nearest).map_err(err)?;
            pcm.hw_params(&hw).map_err(err)?;
        }
        let got = pcm.hw_params_current().and_then(|h| h.get_rate()).map_err(err)?;
        if got != rate {
            return Err(format!("ALSA opened at {got} Hz, not {rate} Hz"));
        }
        {
            let (buffer, period) = pcm.get_params().map_err(err)?;
            log::info!("alsa: buffer {buffer} frames, period {period} frames");
            let sw = pcm.sw_params_current().map_err(err)?;
            // Started by `start`, not by the first write.
            sw.set_start_threshold(buffer as i64).map_err(err)?;
            sw.set_avail_min(period as i64).map_err(err)?;
            pcm.sw_params(&sw).map_err(err)?;
        }
        log::info!("alsa: opened {DEVICE} at {rate} Hz");
        self.info = Some(OutputInfo { device: "Default (ALSA)".into(), id: DEVICE.into(), rate, server: Some("ALSA"), ..OutputInfo::default() });
        self.rate = rate;
        self.stopped_queued = None;
        self.pcm = Some(pcm);
        Ok(())
    }

    fn wait(&mut self, timeout: Duration) {
        match &self.pcm {
            Some(p) if p.state() == State::Running => {
                let _ = p.wait(Some(timeout.as_millis() as u32));
            }
            _ => std::thread::sleep(timeout.min(Duration::from_millis(10))),
        }
    }

    fn available(&mut self) -> Result<usize, String> {
        match self.pcm()?.avail_update() {
            Ok(n) => Ok(n.max(0) as usize),
            Err(e) => {
                self.recover(e)?;
                Ok(self.pcm()?.avail_update().map_err(err)?.max(0) as usize)
            }
        }
    }

    fn queued(&mut self) -> usize {
        if let Some(q) = self.stopped_queued {
            return q;
        }
        match &self.pcm {
            Some(p) if matches!(p.state(), State::Running | State::Prepared) => p.delay().map_or(0, |d| d.max(0) as usize),
            _ => 0,
        }
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.stopped_queued = None;
        let pcm = self.pcm()?;
        let io = pcm.io_f32().map_err(err)?;
        write_all(
            samples.len() / 2,
            WRITE_TIMEOUT,
            |done| match io.writei(&samples[done * 2..]) {
                Ok(frames) => Ok(Some(frames)),
                Err(e) if e.errno() == libc_eagain() => Ok(None),
                Err(e) => pcm.try_recover(e, true).map(|()| Some(0)).map_err(err),
            },
            |left| {
                // A PCM that is not running yet (its start waits for `start`) frees no room.
                if pcm.state() != State::Running {
                    return Err("ALSA buffer full before the device started".to_owned());
                }
                let _ = pcm.wait(Some(left.as_millis().clamp(1, 100) as u32));
                Ok(())
            },
        )
    }

    fn start(&mut self) -> Result<(), String> {
        self.stopped_queued = None;
        let pcm = self.pcm()?;
        if pcm.state() == State::Prepared {
            pcm.start().map_err(err)?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        let queued = self.queued();
        if let Some(p) = &self.pcm {
            let _ = p.drop();
            let _ = p.prepare();
        }
        self.stopped_queued = Some(queued);
    }

    fn flush(&mut self) {
        if let Some(p) = &self.pcm {
            let _ = p.drop();
            let _ = p.prepare();
        }
        self.stopped_queued = None;
    }

    fn info(&mut self) -> Option<OutputInfo> {
        self.info.clone()
    }
}

/// How long one `write` may wait for room before it gives up: far longer than the 80 ms
/// buffer takes to drain, so only a stalled device reaches it.
const WRITE_TIMEOUT: Duration = Duration::from_millis(1_000);

/// Write `frames` frames through `write` (given the frames already written; `None` when
/// the buffer is full, else the frames it took, 0 after a recovered error), waiting with
/// `wait` (given the time left) whenever the buffer has no room. The caller has already
/// taken these frames from the ring, so the rest is never dropped: every frame is written,
/// or the call fails once `timeout` has passed with frames still left.
fn write_all(
    frames: usize,
    timeout: Duration,
    mut write: impl FnMut(usize) -> Result<Option<usize>, String>,
    mut wait: impl FnMut(Duration) -> Result<(), String>,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut done = 0;
    let mut stuck = 0u32;
    while done < frames {
        match write(done)? {
            Some(n) if n > 0 => {
                done += n.min(frames - done);
                stuck = 0;
                continue;
            }
            // `Some(0)` is a recovered error: write again straight away, once.
            Some(_) if stuck == 0 => {
                stuck += 1;
                continue;
            }
            _ => {}
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(format!("ALSA took no audio for {} ms ({} of {frames} frames written)", timeout.as_millis(), done));
        }
        wait(deadline - now)?;
    }
    Ok(())
}

/// EAGAIN: a non-blocking write found the buffer full.
fn libc_eagain() -> i32 {
    11
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_buffer_is_waited_out_and_every_frame_written() {
        // Takes 3 frames, then is full twice, then takes the rest.
        let mut calls = Vec::new();
        let mut script = vec![Some(3), None, Some(0), Some(7)].into_iter();
        let mut waits = 0;
        let r = write_all(
            10,
            Duration::from_secs(5),
            |done| {
                calls.push(done);
                Ok(script.next().unwrap())
            },
            |_| {
                waits += 1;
                Ok(())
            },
        );
        assert_eq!(r, Ok(()));
        assert_eq!(calls, [0, 3, 3, 3]);
        assert!(waits >= 1);
    }

    #[test]
    fn a_device_that_never_takes_audio_fails_after_the_timeout() {
        let t = std::time::Instant::now();
        let r = write_all(
            10,
            Duration::from_millis(30),
            |_| Ok(Some(0)),
            |left| {
                std::thread::sleep(left.min(Duration::from_millis(5)));
                Ok(())
            },
        );
        assert!(r.is_err(), "{r:?}");
        assert!(t.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn a_wait_that_cannot_help_ends_the_write() {
        let r = write_all(4, Duration::from_secs(5), |_| Ok(None), |_| Err("not started".into()));
        assert_eq!(r, Err("not started".to_owned()));
    }
}
