//! Exclusive (bit-perfect) mode, the parts that do not need a device: which sample
//! format to ask for, what to do when none is accepted, and turning the output's f32
//! samples into the bytes of that format.
//!
//! The WASAPI device (`wasapi.rs`) asks a [`FormatProbe`] (the real one wraps
//! `IAudioClient::IsFormatSupported`); tests use a fake device. Volume and ReplayGain
//! are applied before this, in f32, by the output loop; reducing to 16-bit adds TPDF
//! dither so the quantisation error is noise, not distortion. A block that is already
//! 16-bit (a 16-bit track nothing has changed: volume at 100%, no ReplayGain, equalizer,
//! limiting or fade) is written exactly as it is, bit-perfect. 24- and 32-bit are not
//! dithered (their quantisation is far below any DAC's noise floor); a 16- or 24-bit
//! track reaches them exactly.

/// A PCM sample format, in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// 32-bit IEEE float.
    F32,
    /// 32-bit integer, all 32 bits valid.
    I32,
    /// 24 valid bits in a 32-bit container (left-justified).
    I24In32,
    /// Packed 24-bit (3 bytes).
    I24,
    I16,
}

impl SampleFormat {
    /// Tried in this order: the track's rate at 32-bit float, then 32, 24-in-32, 24 and
    /// 16-bit integer.
    pub const PREFERENCE: [SampleFormat; 5] = [Self::F32, Self::I32, Self::I24In32, Self::I24, Self::I16];

    /// Bytes per sample (container size).
    pub fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::I24In32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }

    /// (container bits, valid bits, float)
    pub fn bits(self) -> (usize, usize, bool) {
        match self {
            Self::F32 => (32, 32, true),
            Self::I32 => (32, 32, false),
            Self::I24In32 => (32, 24, false),
            Self::I24 => (24, 24, false),
            Self::I16 => (16, 16, false),
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::F32 => "32-bit float",
            Self::I32 => "32-bit",
            Self::I24In32 | Self::I24 => "24-bit",
            Self::I16 => "16-bit",
        }
    }
}

/// "24-bit / 96 kHz"
pub fn describe(fmt: SampleFormat, rate: u32) -> String {
    let khz = f64::from(rate) / 1000.0;
    let khz = if khz.fract() == 0.0 { format!("{khz:.0}") } else { format!("{khz:.1}") };
    format!("{} / {khz} kHz", fmt.describe())
}

/// What a device says about one format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Accepted,
    Rejected,
    /// Another application holds the device exclusively, or the device does not allow
    /// exclusive mode: no format will do.
    Unavailable(String),
}

/// Asks a device whether it takes a format in exclusive mode (stereo, at `rate`).
pub trait FormatProbe {
    fn probe(&mut self, rate: u32, fmt: SampleFormat) -> Probe;
}

/// The outcome of negotiating exclusive mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Negotiated {
    Exclusive(SampleFormat),
    /// Play in shared mode instead; the note says why (shown to the user).
    Shared { note: String },
}

/// The first format of [`SampleFormat::PREFERENCE`] the device accepts at `rate`.
pub fn negotiate(probe: &mut dyn FormatProbe, rate: u32) -> Negotiated {
    for fmt in SampleFormat::PREFERENCE {
        match probe.probe(rate, fmt) {
            Probe::Accepted => return Negotiated::Exclusive(fmt),
            Probe::Rejected => {}
            Probe::Unavailable(why) => return Negotiated::Shared { note: format!("Exclusive mode unavailable ({why}); playing shared") },
        }
    }
    let khz = describe(SampleFormat::F32, rate);
    let khz = khz.split(" / ").nth(1).unwrap_or_default().to_owned();
    Negotiated::Shared { note: format!("The device does not take {khz} in exclusive mode; playing shared") }
}

/// f32 interleaved samples to the bytes of a PCM format, little-endian.
pub struct Converter {
    pub format: SampleFormat,
    /// xorshift32 state, for the dither.
    rng: u32,
}

impl Converter {
    pub fn new(format: SampleFormat) -> Self {
        Self { format, rng: 0x9e37_79b9 }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        // Uniform in [0, 1).
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Append `samples`, converted, to `out`.
    pub fn convert(&mut self, samples: &[f32], out: &mut Vec<u8>) {
        out.reserve(samples.len() * self.format.bytes());
        match self.format {
            SampleFormat::F32 => {
                for s in samples {
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
            SampleFormat::I32 => {
                for &s in samples {
                    out.extend_from_slice(&quantise(f64::from(s), 31).to_le_bytes());
                }
            }
            SampleFormat::I24In32 => {
                for &s in samples {
                    out.extend_from_slice(&(quantise(f64::from(s), 23) << 8).to_le_bytes());
                }
            }
            SampleFormat::I24 => {
                for &s in samples {
                    let v = quantise(f64::from(s), 23).to_le_bytes();
                    out.extend_from_slice(&v[..3]);
                }
            }
            SampleFormat::I16 => {
                // Every sample already on the 16-bit grid: nothing to quantise, so no dither
                // (it would only add noise to a bit-perfect stream). Decided for the block,
                // not per sample: processed audio stays dithered throughout instead of the
                // dither going off wherever a sample happens to land on the grid (silence,
                // every other code under a gain of exactly a half).
                let exact = samples.iter().all(|&s| on_grid(s, 32768.0));
                for &s in samples {
                    // TPDF: the sum of two uniform variables, ±1 LSB peak.
                    let d = if exact { 0.0 } else { self.rand() - self.rand() };
                    let v = (f64::from(s) * 32768.0 + f64::from(d)).round().clamp(-32768.0, 32767.0) as i16;
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
}

/// `s` is exactly a code of a signed integer format of full scale `scale` (in range).
fn on_grid(s: f32, scale: f64) -> bool {
    let x = f64::from(s) * scale;
    x == x.round() && (-scale..scale).contains(&x)
}

/// A sample in [-1, 1] to a signed integer of `bits` + 1 bits, clamped (full scale
/// positive is one step short of 1.0).
fn quantise(s: f64, bits: u32) -> i32 {
    let scale = f64::from(1u32 << bits);
    (s * scale).round().clamp(-scale, scale - 1.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake device: takes the formats it is given, at the rates it is given.
    struct Fake {
        rates: Vec<u32>,
        formats: Vec<SampleFormat>,
        busy: bool,
        asked: Vec<SampleFormat>,
    }

    impl FormatProbe for Fake {
        fn probe(&mut self, rate: u32, fmt: SampleFormat) -> Probe {
            self.asked.push(fmt);
            if self.busy {
                return Probe::Unavailable("the device is in use by another application".into());
            }
            if self.rates.contains(&rate) && self.formats.contains(&fmt) { Probe::Accepted } else { Probe::Rejected }
        }
    }

    fn fake(rates: &[u32], formats: &[SampleFormat]) -> Fake {
        Fake { rates: rates.to_vec(), formats: formats.to_vec(), busy: false, asked: Vec::new() }
    }

    #[test]
    fn the_best_accepted_format_wins_in_order() {
        let mut d = fake(&[44_100, 96_000], &[SampleFormat::I16, SampleFormat::I24In32, SampleFormat::I24]);
        assert_eq!(negotiate(&mut d, 96_000), Negotiated::Exclusive(SampleFormat::I24In32));
        assert_eq!(d.asked, [SampleFormat::F32, SampleFormat::I32, SampleFormat::I24In32], "stops at the first accepted");
        let mut d = fake(&[44_100], &SampleFormat::PREFERENCE);
        assert_eq!(negotiate(&mut d, 44_100), Negotiated::Exclusive(SampleFormat::F32));
        let mut d = fake(&[48_000], &[SampleFormat::I16]);
        assert_eq!(negotiate(&mut d, 48_000), Negotiated::Exclusive(SampleFormat::I16));
    }

    #[test]
    fn nothing_accepted_falls_back_to_shared_with_a_note() {
        let mut d = fake(&[48_000], &SampleFormat::PREFERENCE);
        let Negotiated::Shared { note } = negotiate(&mut d, 192_000) else { panic!("should fall back") };
        assert!(note.contains("192 kHz"), "{note}");
        assert_eq!(d.asked.len(), 5, "every format was tried");
        let Negotiated::Shared { note } = negotiate(&mut fake(&[44_100], &SampleFormat::PREFERENCE), 88_200) else { panic!() };
        assert!(note.contains("88.2 kHz"), "{note}");
    }

    #[test]
    fn a_busy_device_falls_back_at_once() {
        let mut d = fake(&[48_000], &SampleFormat::PREFERENCE);
        d.busy = true;
        let Negotiated::Shared { note } = negotiate(&mut d, 48_000) else { panic!() };
        assert!(note.contains("in use"), "{note}");
        assert_eq!(d.asked.len(), 1);
    }

    fn ints(fmt: SampleFormat, samples: &[f32]) -> Vec<i32> {
        let mut out = Vec::new();
        Converter::new(fmt).convert(samples, &mut out);
        match fmt {
            SampleFormat::I16 => out.as_chunks::<2>().0.iter().map(|b| i32::from(i16::from_le_bytes([b[0], b[1]]))).collect(),
            SampleFormat::I24 => out.as_chunks::<3>().0.iter().map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8).collect(),
            _ => out.as_chunks::<4>().0.iter().map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(),
        }
    }

    #[test]
    fn integer_formats_are_exact_and_clamped() {
        let s = [0.0, 0.5, -0.5, 1.0, -1.0, 1.5, -1.5, 1.0 / 8_388_608.0];
        assert_eq!(ints(SampleFormat::I24, &s), [0, 4_194_304, -4_194_304, 8_388_607, -8_388_608, 8_388_607, -8_388_608, 1]);
        assert_eq!(
            ints(SampleFormat::I24In32, &s),
            [0, 4_194_304 << 8, -4_194_304 << 8, 8_388_607 << 8, -8_388_608 << 8, 8_388_607 << 8, -8_388_608 << 8, 1 << 8],
            "24 valid bits, left-justified"
        );
        assert_eq!(ints(SampleFormat::I32, &s[..7]), [0, 1 << 30, -(1 << 30), i32::MAX, i32::MIN, i32::MAX, i32::MIN]);
        let mut f = Vec::new();
        Converter::new(SampleFormat::F32).convert(&[0.25, -1.0], &mut f);
        assert_eq!(f, [0.25f32.to_le_bytes(), (-1.0f32).to_le_bytes()].concat(), "float passes through");
    }

    #[test]
    fn sixteen_bit_is_dithered_and_unbiased() {
        // A constant a third of an LSB above 1000 LSB: dither spreads it over neighbouring
        // codes so the average comes out right, which plain rounding cannot do.
        let v = (1000.0 + 1.0 / 3.0) / 32768.0;
        let out = ints(SampleFormat::I16, &vec![v; 200_000]);
        assert!(out.iter().all(|&x| (998..=1002).contains(&x)), "within the TPDF range");
        let mean = out.iter().map(|&x| f64::from(x)).sum::<f64>() / out.len() as f64;
        assert!((mean - (1000.0 + 1.0 / 3.0)).abs() < 0.01, "mean {mean}");
        assert!(out.iter().any(|&x| x != out[0]), "dithered, not constant");
        assert_eq!(ints(SampleFormat::I16, &[1.5, -1.5, 2.0])[..], [32767, -32768, 32767], "clamped");
    }

    /// A 16-bit track nothing has changed reaches a 16-bit device bit for bit; a 24-bit
    /// track, or a 16-bit one turned down, is dithered.
    #[test]
    fn sixteen_bit_content_is_written_exactly_and_anything_else_dithered() {
        let codes: Vec<i32> = (-32768..32768).step_by(7).chain([32767, -32768, 0, 1, -1]).collect();
        let ramp: Vec<f32> = codes.iter().map(|&c| c as f32 / 32768.0).collect();
        assert_eq!(ints(SampleFormat::I16, &ramp), codes, "bit-identical");
        for fmt in [SampleFormat::I24, SampleFormat::I24In32, SampleFormat::I32] {
            let shift = if fmt == SampleFormat::I24 { 8 } else { 16 };
            assert_eq!(ints(fmt, &ramp), codes.iter().map(|&c| c << shift).collect::<Vec<_>>(), "{fmt:?}: exact too");
        }
        // 24-bit content: a third of a 16-bit step above each code.
        let deep: Vec<f32> = (0..20_000).map(|i| ((i % 2000 - 1000) * 256 + 85) as f32 / 8_388_608.0).collect();
        let plain: Vec<i32> = deep.iter().map(|&s| (f64::from(s) * 32768.0).round() as i32).collect();
        let got = ints(SampleFormat::I16, &deep);
        assert!(got != plain, "dithered, not plainly rounded");
        assert!(got.iter().zip(&plain).all(|(a, b)| (a - b).abs() <= 1), "within the TPDF range");
        // The 16-bit ramp turned down: dithered too.
        let quieter: Vec<f32> = ramp.iter().map(|&s| s * 0.7).collect();
        let plain: Vec<i32> = quieter.iter().map(|&s| (f64::from(s) * 32768.0).round() as i32).collect();
        assert!(ints(SampleFormat::I16, &quieter) != plain, "attenuated: dithered");
    }

    #[test]
    fn describes_formats() {
        assert_eq!(describe(SampleFormat::I24In32, 96_000), "24-bit / 96 kHz");
        assert_eq!(describe(SampleFormat::I16, 44_100), "16-bit / 44.1 kHz");
    }
}
