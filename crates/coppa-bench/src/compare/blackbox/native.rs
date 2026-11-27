//! COP-221: a modem's declared native audio format, and the relay's streaming
//! conversion between it and the channel's f32le 48000 Hz mono.
//!
//! COP-214 measured ALSA `plug` rate conversion failing the integrity gate
//! (RMS ratio 0.74 to 0.77, correlation near 0), so the cable stays a bare
//! `file` PCM in the modem's own format and the relay converts instead
//! (COP-170 decision 15). Every supported rate divides 48000, so each
//! direction is an integer-factor linear-phase FIR interpolator or decimator:
//! one 20 ms tick is always `rate / 50` native samples and 960 channel
//! samples, and a round trip delays the signal by a whole number of native
//! samples.

use anyhow::{bail, Context, Result};

/// The channel simulator's sample rate.
pub const CHANNEL_RATE: u32 = 48_000;
/// The `audio.format` values a profile may declare.
pub const SUPPORTED_FORMATS: &str = "f32le, s16le";
/// The `audio.rate` values a profile may declare, in Hz.
pub const SUPPORTED_RATES: &str = "8000, 12000, 16000, 24000, 48000";
const RATES: [u32; 5] = [8000, 12_000, 16_000, 24_000, 48_000];
/// Native samples of delay each conversion filter adds.
const FILTER_DELAY: usize = 24;
/// Filter cutoff as a fraction of the native rate.
const CUTOFF: f32 = 0.44;
/// Upper edge of the band-limited probe, as a fraction of the native rate.
const PROBE_BAND: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32le,
    S16le,
}

impl SampleFormat {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "f32le" => Some(Self::F32le),
            "s16le" => Some(Self::S16le),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::F32le => "f32le",
            Self::S16le => "s16le",
        }
    }

    pub fn bytes(self) -> usize {
        match self {
            Self::F32le => 4,
            Self::S16le => 2,
        }
    }

    /// Appends every whole sample in `bytes` to `out`.
    pub fn decode(self, bytes: &[u8], out: &mut Vec<f32>) {
        match self {
            Self::F32le => out.extend(
                bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            ),
            Self::S16le => out.extend(
                bytes
                    .chunks_exact(2)
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0),
            ),
        }
    }

    /// Appends a block of samples. Refuses an s16le block that would clip,
    /// writing nothing, so the relay stops rather than record a channel the
    /// modem hears clipped. The cable has no capture gain.
    pub fn encode(self, samples: &[f32], out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::F32le => out.extend(samples.iter().flat_map(|x| x.to_le_bytes())),
            Self::S16le => {
                if let Some(sample) = samples
                    .iter()
                    .find(|&&x| !(-32768.0..=32767.0).contains(&(x * 32768.0).round()))
                {
                    bail!(
                        "s16le capture sample {sample} is outside the representable range; \
                         the trial is not recorded because s16le would clip it"
                    );
                }
                out.extend(samples.iter().flat_map(|&x| to_i16(x).to_le_bytes()));
            }
        }
        Ok(())
    }
}

/// Rounds to the nearest 16-bit code, saturating at full scale.
pub fn to_i16(x: f32) -> i16 {
    (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// The format and rate a modem opens its ALSA device in, mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFormat {
    pub format: SampleFormat,
    pub rate: u32,
}

impl NativeFormat {
    /// What coppad opens, and what the cable carried before COP-221.
    pub const CHANNEL: Self = Self {
        format: SampleFormat::F32le,
        rate: CHANNEL_RATE,
    };

    /// Validates a profile's `[audio]` declaration. Each error names the key
    /// and the values it accepts.
    pub fn declared(
        format: Option<&str>,
        rate: Option<u32>,
        channels: Option<u32>,
    ) -> Result<Self> {
        let format = match format {
            None => bail!("audio.format is undeclared; supported values: {SUPPORTED_FORMATS}"),
            Some(name) => SampleFormat::parse(name).with_context(|| {
                format!(
                    "audio.format {name:?} is unsupported; supported values: {SUPPORTED_FORMATS}"
                )
            })?,
        };
        let rate = match rate {
            None => bail!("audio.rate is undeclared; supported values: {SUPPORTED_RATES}"),
            Some(rate) if RATES.contains(&rate) => rate,
            Some(rate) => {
                bail!("audio.rate {rate} is unsupported; supported values: {SUPPORTED_RATES}")
            }
        };
        match channels {
            None => bail!("audio.channels is undeclared; supported values: 1"),
            Some(1) => {}
            Some(n) => bail!("audio.channels {n} is unsupported; supported values: 1"),
        }
        Ok(Self { format, rate })
    }

    /// Channel samples per native sample.
    pub fn factor(self) -> usize {
        (CHANNEL_RATE / self.rate) as usize
    }

    /// Native samples in one 20 ms relay tick.
    pub fn block(self) -> usize {
        self.rate as usize / 50
    }

    /// FIFO capacity in bytes that holds as much time as the default 64 KiB
    /// pipe does for f32le 48000 Hz, or `None` for that format itself, whose
    /// pipes keep the default COP-214 measured (about 340 ms of lead). A
    /// 64 KiB pipe would hold 2.7 s of s16le 12000 Hz, past the probe's
    /// 1 s alignment window; the kernel rounds the request up to a power of
    /// two pages, so the lead stays between 0.34 s and 0.52 s.
    pub fn pipe_bytes(self) -> Option<usize> {
        if self == Self::CHANNEL {
            return None;
        }
        let rate = self.rate as usize * self.format.bytes();
        Some(
            (65_536 * rate)
                .div_ceil(CHANNEL_RATE as usize * 4)
                .max(4096),
        )
    }

    /// The form the probe child receives in `COPPA_COMPARE_PROBE_FORMAT`.
    pub fn to_env(self) -> String {
        format!("{}:{}", self.format.name(), self.rate)
    }

    pub fn from_env(text: &str) -> Result<Self> {
        let (format, rate) = text
            .split_once(':')
            .with_context(|| format!("probe format {text:?} is not format:rate"))?;
        let rate = rate
            .parse()
            .with_context(|| format!("probe rate {rate:?} is not an integer"))?;
        Self::declared(Some(format), Some(rate), Some(1))
    }

    /// Ten seconds of seeded noise the declared format represents exactly.
    /// At 48000 Hz it is the white probe COP-214 shipped; at a lower rate it
    /// is limited to 0.35 of the rate, inside the conversion passband, so
    /// a correct converter returns it unchanged apart from a whole-sample
    /// delay and quantization.
    pub fn probe_signal(self) -> Vec<f32> {
        let rate = self.rate as usize;
        let mut state = 0x1700_2140u64;
        let mut noise = |count: usize| -> Vec<f32> {
            (0..count)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
                })
                .collect()
        };
        let signal = if self.rate == CHANNEL_RATE {
            noise(rate * 10)
        } else {
            let taps = lowpass(129, PROBE_BAND);
            let white = noise(rate * 10 + taps.len());
            (0..rate * 10)
                .map(|n| {
                    taps.iter()
                        .zip(&white[n..n + taps.len()])
                        .map(|(h, x)| h * x)
                        .sum::<f32>()
                        .clamp(-0.9, 0.9)
                })
                .collect()
        };
        if self.format == SampleFormat::F32le {
            // Already exact, and the f32 48 kHz probe builds this after its
            // streams start, so a no-op round trip would only add lead.
            return signal;
        }
        let mut bytes = Vec::with_capacity(signal.len() * self.format.bytes());
        self.format
            .encode(&signal, &mut bytes)
            .expect("the probe signal is within the s16le range");
        let mut exact = Vec::with_capacity(signal.len());
        self.format.decode(&bytes, &mut exact);
        exact
    }
}

/// An odd-length Blackman-windowed lowpass with unity DC gain, cutoff as a
/// fraction of the rate it runs at.
fn lowpass(taps: usize, cutoff: f32) -> Vec<f32> {
    let mut h = coppa_dsp::fir::design_bandpass(taps, 1.0, 0.0, cutoff);
    let sum: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= sum);
    h
}

/// One station's two conversions: its playback up to the channel rate and
/// the channel's output down to its capture rate. Both keep their filter
/// history across ticks, so a stream split into ticks converts exactly as
/// it would whole.
pub struct Converter {
    factor: usize,
    taps: Vec<f32>,
    /// Native playback history, newest last.
    up_history: Vec<f32>,
    /// Channel-rate output history, newest last.
    down_history: Vec<f32>,
}

impl Converter {
    pub fn new(native: NativeFormat) -> Self {
        let factor = native.factor();
        let taps = if factor == 1 {
            Vec::new()
        } else {
            // Delay (len - 1) / 2 = factor * FILTER_DELAY channel samples,
            // FILTER_DELAY native samples, so the round trip is whole.
            let len = 2 * factor * FILTER_DELAY + 1;
            lowpass(len, CUTOFF / factor as f32)
        };
        let up_len = taps.len().div_ceil(factor.max(1));
        let down_len = taps.len().saturating_sub(1);
        Self {
            factor,
            taps,
            up_history: vec![0.0; up_len],
            down_history: vec![0.0; down_len],
        }
    }

    /// Native playback to channel-rate samples, `factor` out per sample in.
    pub fn up(&mut self, native: &[f32]) -> Vec<f32> {
        if self.factor == 1 {
            return native.to_vec();
        }
        let l = self.factor;
        let keep = self.up_history.len();
        let mut x = std::mem::take(&mut self.up_history);
        x.extend_from_slice(native);
        let mut out = Vec::with_capacity(native.len() * l);
        for n in keep..x.len() {
            for phase in 0..l {
                // y[nL + p] = L * sum_i h[p + iL] x[n - i]
                let mut acc = 0.0f32;
                let mut k = phase;
                let mut i = 0;
                while k < self.taps.len() {
                    acc += self.taps[k] * x[n - i];
                    k += l;
                    i += 1;
                }
                out.push(acc * l as f32);
            }
        }
        self.up_history = x[x.len() - keep..].to_vec();
        out
    }

    /// Channel-rate samples to native capture, one out per `factor` in.
    /// `channel.len()` must be a multiple of `factor` (960 always is).
    pub fn down(&mut self, channel: &[f32]) -> Vec<f32> {
        if self.factor == 1 {
            return channel.to_vec();
        }
        let l = self.factor;
        let keep = self.down_history.len();
        let mut y = std::mem::take(&mut self.down_history);
        y.extend_from_slice(channel);
        let out = (0..channel.len() / l)
            .map(|j| {
                // The first sample of each group of `factor` sits on the
                // native grid `up` produced, so the delay stays whole.
                let m = keep + j * l;
                self.taps
                    .iter()
                    .enumerate()
                    .map(|(k, h)| h * y[m - k])
                    .sum()
            })
            .collect();
        self.down_history = y[y.len() - keep..].to_vec();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(native: NativeFormat, signal: &[f32]) -> Vec<f32> {
        let mut converter = Converter::new(native);
        let mut received = Vec::new();
        let mut bytes = Vec::new();
        for tick in signal.chunks(native.block()) {
            let mut block = tick.to_vec();
            block.resize(native.block(), 0.0);
            let channel = converter.up(&block);
            assert_eq!(channel.len(), 960);
            let back = converter.down(&channel);
            assert_eq!(back.len(), native.block());
            bytes.clear();
            native.format.encode(&back, &mut bytes).unwrap();
            native.format.decode(&bytes, &mut received);
        }
        // Capture runs past the end of the probe, as the live probe does.
        for _ in 0..50 {
            let channel = converter.up(&vec![0.0; native.block()]);
            bytes.clear();
            native
                .format
                .encode(&converter.down(&channel), &mut bytes)
                .unwrap();
            native.format.decode(&bytes, &mut received);
        }
        received
    }

    /// AC1 at the conversion level: s16le 12000 Hz through the relay's
    /// conversion and a noiseless channel passes the AC10 gate.
    #[test]
    fn s16le_12k_round_trip_passes_the_integrity_gate() {
        let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
        let sent = native.probe_signal();
        let received = round_trip(native, &sent);
        let metrics = super::super::integrity::analyse(&sent, &received, 12_000, 0, 0).unwrap();
        assert!(metrics.correlation >= 0.999, "{metrics:?}");
        assert!(metrics.level_db.abs() <= 0.1, "{metrics:?}");
        // Two filters of FILTER_DELAY native samples each.
        assert!((metrics.cable_latency_ms - 4.0).abs() < 1e-9, "{metrics:?}");
    }

    #[test]
    fn every_supported_rate_round_trips_within_the_gate() {
        for format in ["f32le", "s16le"] {
            for rate in RATES {
                let native = NativeFormat::declared(Some(format), Some(rate), Some(1)).unwrap();
                let sent = native.probe_signal();
                let received = round_trip(native, &sent);
                let metrics =
                    super::super::integrity::analyse(&sent, &received, rate as usize, 0, 0)
                        .unwrap_or_else(|e| panic!("{format} {rate}: {e:#}"));
                assert!(metrics.correlation >= 0.999, "{format} {rate}: {metrics:?}");
                assert!(
                    metrics.level_db.abs() <= 0.1,
                    "{format} {rate}: {metrics:?}"
                );
            }
        }
    }

    /// AC3: coppad's f32le 48000 Hz path is untouched; conversion is the
    /// identity and the probe is the white noise COP-214 shipped.
    #[test]
    fn coppad_format_converts_bit_exactly() {
        let mut converter = Converter::new(NativeFormat::CHANNEL);
        let block: Vec<f32> = (0..960).map(|i| (i as f32 / 960.0) - 0.5).collect();
        assert_eq!(converter.up(&block), block);
        assert_eq!(converter.down(&block), block);
        let probe = NativeFormat::CHANNEL.probe_signal();
        assert_eq!(probe.len(), 480_000);
        let mut state = 0x1700_2140u64;
        let first = {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
        };
        assert_eq!(probe[0].to_bits(), first.to_bits());
    }

    /// AC2: an undeclared or unsupported value names its key and the
    /// supported values.
    #[test]
    fn bad_declarations_name_the_key_and_the_supported_values() {
        let cases = [
            (
                None,
                Some(48_000),
                Some(1),
                "audio.format",
                SUPPORTED_FORMATS,
            ),
            (
                Some("s24le"),
                Some(48_000),
                Some(1),
                "audio.format",
                SUPPORTED_FORMATS,
            ),
            (Some("f32le"), None, Some(1), "audio.rate", SUPPORTED_RATES),
            (
                Some("f32le"),
                Some(44_100),
                Some(1),
                "audio.rate",
                SUPPORTED_RATES,
            ),
            (Some("f32le"), Some(48_000), None, "audio.channels", "1"),
            (Some("f32le"), Some(48_000), Some(2), "audio.channels", "1"),
        ];
        for (format, rate, channels, key, values) in cases {
            let error = NativeFormat::declared(format, rate, channels)
                .unwrap_err()
                .to_string();
            assert!(error.contains(key), "{error}");
            assert!(
                error.contains(&format!("supported values: {values}")),
                "{error}"
            );
        }
    }

    #[test]
    fn native_pipes_hold_the_coppad_lead() {
        assert_eq!(NativeFormat::CHANNEL.pipe_bytes(), None);
        let s16_12k = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
        assert_eq!(s16_12k.pipe_bytes(), Some(8192));
        for format in ["f32le", "s16le"] {
            for rate in RATES {
                let native = NativeFormat::declared(Some(format), Some(rate), Some(1)).unwrap();
                let Some(bytes) = native.pipe_bytes() else {
                    continue;
                };
                let rounded = bytes.next_power_of_two();
                let seconds = rounded as f64 / (rate as f64 * native.format.bytes() as f64);
                assert!(
                    (0.34..=0.52).contains(&seconds),
                    "{format} {rate}: {seconds}"
                );
            }
        }
    }

    #[test]
    fn probe_format_survives_the_environment() {
        let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
        assert_eq!(NativeFormat::from_env(&native.to_env()).unwrap(), native);
        assert!(NativeFormat::from_env("s16le").is_err());
    }

    #[test]
    fn s16_codes_round_trip_at_full_scale() {
        let mut bytes = Vec::new();
        let samples = [0.5, -1.0, 32767.0 / 32768.0];
        SampleFormat::S16le.encode(&samples, &mut bytes).unwrap();
        let mut back = Vec::new();
        SampleFormat::S16le.decode(&bytes, &mut back);
        assert_eq!(back, samples);
        // The probe's cpal output still saturates rather than wraps.
        assert_eq!((to_i16(2.0), to_i16(-2.0)), (i16::MAX, i16::MIN));
    }

    #[test]
    fn s16le_capture_refuses_clipping_at_zero_db() {
        use coppa_channel::persistent::{PersistentChannel, SnrProfile};

        let native = NativeFormat::declared(Some("s16le"), Some(CHANNEL_RATE), Some(1)).unwrap();
        let signal: Vec<_> = (0..native.block())
            .map(|i| 0.5 * (std::f32::consts::TAU * i as f32 / 48.0).sin())
            .collect();
        let mut channel = PersistentChannel::new(
            CHANNEL_RATE as f32,
            None,
            Some(SnrProfile::constant(0.0)),
            0.125,
            7,
            0,
        );
        let capture = Converter::new(native).down(&channel.process(&signal));
        let mut bytes = Vec::new();
        let error = native.format.encode(&capture, &mut bytes).unwrap_err();
        assert!(
            error.to_string().contains("s16le capture sample"),
            "{error}"
        );
        assert!(bytes.is_empty(), "a rejected capture block wrote bytes");
    }
}
