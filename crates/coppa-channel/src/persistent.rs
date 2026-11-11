//! Persistent (session-length) channel for COP-169: a stateful Watterson HF
//! channel plus AWGN whose state carries across calls, so a session that
//! transmits many frames, pauses and turns the link around sees ONE continuous
//! fading process instead of a fresh draw per buffer.
//!
//! [`crate::watterson::watterson`] redraws its fading process for every buffer it
//! is handed, which is right for an i.i.d. per-frame sweep but wrong for a
//! connected session riding through a 20 → −6 → 20 dB ramp: a deep fade must
//! persist across a frame boundary and a turnaround the way the ionosphere
//! does. This module keeps the same channel mathematics (equal-power tapped
//! delay line, Gaussian Doppler power spectrum with sigma = spread / 2,
//! ensemble-unit-power taps, noise referenced to a fixed CLEAN signal power in a
//! 3 kHz bandwidth) but computes every stage as a causal filter with carried
//! state:
//!
//! - **Fading.** Each tap's gain is a complex white Gaussian sequence at a low
//!   rate ([`fading_rate_hz`]) passed through a Gaussian FIR whose amplitude
//!   response is `exp(-f²/(4σ²))`, so the power spectrum is `exp(-f²/(2σ²))`.
//!   The FIR is normalized to unit energy, so `E|g|² = 1` in the ensemble while
//!   each realization keeps its Rayleigh fades. The low-rate sequence is
//!   linearly interpolated to the audio rate.
//! - **Analytic signal.** A streaming Hilbert FIR
//!   ([`coppa_dsp::fir::design_hilbert`], [`HILBERT_TAPS`] taps) supplies the
//!   quadrature part; the in-phase part is delayed by the same group delay. The
//!   channel therefore has a fixed latency of [`PersistentChannel::latency_samples`]
//!   on faded paths (zero on AWGN-only paths).
//! - **Delay line.** The analytic history needed for the longest tap delay is
//!   carried across calls.
//! - **Noise.** Generated sample by sample from its own seeded stream, with a
//!   power set by an [`SnrProfile`] evaluated at the absolute sample time, never
//!   from the faded output or from silence.
//!
//! Every stage consumes its random streams strictly in sample order, so pushing
//! a signal through in arbitrary chunks gives the same output as one call (the
//! `chunk_partition_*` tests pin this).
//!
//! # Seed derivation
//!
//! A run has one base seed. Each (direction, stream) pair gets its own 64-bit
//! seed from [`derive_seed`], a SplitMix64 finalizer over
//! `base ^ (direction << 40) ^ (stream << 32)`. Direction 0 is A→B and 1 is
//! B→A; stream 0 is fading ([`STREAM_FADING`]) and stream 1 is noise
//! ([`STREAM_NOISE`]). Changing the SNR therefore never changes the fading draw,
//! and the two directions fade independently.

use crate::watterson::WattersonConfig;
use crate::HF_NOISE_BW_HZ;
use coppa_dsp::fir::{design_hilbert, StreamingFir};
use num_complex::Complex32;
use rand::rngs::StdRng;
use rand::{Rng, RngExt, SeedableRng};
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

/// Hilbert FIR length for the streaming analytic signal. 1023 taps (Blackman)
/// put the transition band well below the 300 Hz lower edge of the SSB passband
/// at 48 kHz, so the quadrature part is accurate across the modem's band.
pub const HILBERT_TAPS: usize = 1023;

/// Seed stream index for the fading process (see [`derive_seed`]).
pub const STREAM_FADING: u8 = 0;
/// Seed stream index for the additive noise (see [`derive_seed`]).
pub const STREAM_NOISE: u8 = 1;

/// Longest Gaussian Doppler FIR half-length, in low-rate samples. Bounds the
/// work for a tiny configured spread; the response is then truncated, which only
/// narrows an already very slow process.
const MAX_DOPPLER_HALF_TAPS: usize = 4096;

/// Derive the seed of one random stream of one link direction from a run's
/// base seed. See the module docs for the layout.
#[must_use]
pub fn derive_seed(base: u64, direction: u8, stream: u8) -> u64 {
    let mut z = base ^ (u64::from(direction) << 40) ^ (u64::from(stream) << 32);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Low rate at which the fading process is generated before interpolation:
/// 100 Hz, or 20x the spread for a faster configured channel.
#[must_use]
pub fn fading_rate_hz(doppler_spread_hz: f32) -> f32 {
    (20.0 * doppler_spread_hz).max(100.0)
}

/// Piecewise-linear SNR schedule over absolute channel time, in dB (3 kHz
/// reference). Before the first point and after the last the end values hold.
#[derive(Debug, Clone, PartialEq)]
pub struct SnrProfile {
    /// `(time_s, snr_db)` breakpoints, strictly increasing in time.
    points: Vec<(f64, f32)>,
}

impl SnrProfile {
    /// A constant SNR.
    #[must_use]
    pub fn constant(snr_db: f32) -> Self {
        Self {
            points: vec![(0.0, snr_db)],
        }
    }

    /// A schedule from `(time_s, snr_db)` breakpoints. Returns `None` when the
    /// list is empty, a value is not finite, or time does not strictly increase.
    #[must_use]
    pub fn from_points(points: Vec<(f64, f32)>) -> Option<Self> {
        if points.is_empty()
            || points
                .iter()
                .any(|&(t, s)| !t.is_finite() || !s.is_finite())
            || points.windows(2).any(|w| w[1].0 <= w[0].0)
        {
            return None;
        }
        Some(Self { points })
    }

    /// The COP-169 fade ramp, started at session establishment: hold 20 dB for
    /// 20 s, descend linearly to −6 dB over 30 s, hold −6 dB for 30 s, ascend
    /// linearly to 20 dB over 30 s, then hold 20 dB for 60 s (170 s in all).
    #[must_use]
    pub fn cop169_ramp() -> Self {
        Self {
            points: vec![
                (0.0, 20.0),
                (20.0, 20.0),
                (50.0, -6.0),
                (80.0, -6.0),
                (110.0, 20.0),
                (170.0, 20.0),
            ],
        }
    }

    /// Total length of the schedule in seconds (time of its last breakpoint).
    #[must_use]
    pub fn duration_s(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.0)
    }

    /// SNR in dB at absolute time `t_s`.
    #[must_use]
    pub fn at(&self, t_s: f64) -> f32 {
        let first = self.points[0];
        if t_s <= first.0 {
            return first.1;
        }
        for w in self.points.windows(2) {
            let (t0, s0) = w[0];
            let (t1, s1) = w[1];
            if t_s <= t1 {
                let frac = ((t_s - t0) / (t1 - t0)) as f32;
                return s0 + (s1 - s0) * frac;
            }
        }
        self.points[self.points.len() - 1].1
    }
}

/// Complex N(0, 1) sample (E|w|² = 1) by Box-Muller.
fn complex_gaussian(rng: &mut impl Rng) -> Complex32 {
    let u1: f32 = (1.0 - rng.random::<f32>()).max(1e-12);
    let u2: f32 = rng.random::<f32>();
    let r = (-u1.ln()).sqrt(); // sqrt(-2 ln u1) / sqrt(2)
    Complex32::new(r * (TAU * u2).cos(), r * (TAU * u2).sin())
}

/// One tap's low-rate Gaussian-Doppler fading generator: white complex
/// Gaussian input through a unit-energy Gaussian FIR, with carried history.
#[derive(Debug)]
struct DopplerProcess {
    rng: StdRng,
    taps: Vec<f32>,
    history: VecDeque<Complex32>,
}

impl DopplerProcess {
    fn new(doppler_sigma_hz: f32, rate_hz: f32, rng: StdRng) -> Self {
        let sigma = doppler_sigma_hz.max(1e-3);
        // h(t) = exp(-4π²σ²t²) has amplitude response exp(-f²/(4σ²)); truncate
        // where the exponent reaches 9 (h < 1.3e-4).
        let half_s = 3.0 / (2.0 * PI * sigma);
        let half = ((half_s * rate_hz).ceil() as usize).clamp(1, MAX_DOPPLER_HALF_TAPS);
        let mut taps: Vec<f32> = (0..=2 * half)
            .map(|j| {
                let t = (j as f32 - half as f32) / rate_hz;
                (-4.0 * PI * PI * sigma * sigma * t * t).exp()
            })
            .collect();
        let energy: f32 = taps.iter().map(|h| h * h).sum();
        let norm = energy.sqrt();
        for h in &mut taps {
            *h /= norm;
        }
        let mut p = Self {
            rng,
            history: VecDeque::with_capacity(taps.len()),
            taps,
        };
        // Pre-fill so the process is stationary from its first output.
        for _ in 0..p.taps.len() {
            let w = complex_gaussian(&mut p.rng);
            p.history.push_back(w);
        }
        p
    }

    fn next(&mut self) -> Complex32 {
        self.history.pop_front();
        let w = complex_gaussian(&mut self.rng);
        self.history.push_back(w);
        self.history
            .iter()
            .zip(self.taps.iter().rev())
            .map(|(&x, &h)| x * h)
            .sum()
    }
}

/// One faded path: delay, amplitude, and its interpolated fading gain.
#[derive(Debug)]
struct FadedTap {
    delay: usize,
    amp: f32,
    doppler: DopplerProcess,
    g0: Complex32,
    g1: Complex32,
}

/// The fading part of a persistent channel (absent on AWGN-only links).
#[derive(Debug)]
struct FadingState {
    hilbert: StreamingFir,
    /// In-phase delay matching the Hilbert group delay.
    inphase: VecDeque<f32>,
    /// Analytic-signal history, newest last, long enough for the longest tap.
    analytic: VecDeque<Complex32>,
    taps: Vec<FadedTap>,
    /// Audio samples per low-rate fading step.
    step: usize,
    /// Position inside the current low-rate interval, `0..step`.
    phase: usize,
}

/// One direction of a session link: persistent Watterson fading (optional) and
/// additive white Gaussian noise referred to a fixed clean signal power.
#[derive(Debug)]
pub struct PersistentChannel {
    sample_rate: f32,
    fading: Option<FadingState>,
    noise: Option<(StdRng, SnrProfile)>,
    ref_signal_power: f32,
    /// Samples processed so far (the channel clock).
    clock: u64,
}

impl PersistentChannel {
    /// Build one link direction.
    ///
    /// - `fading`: `None` for AWGN only, or a Watterson configuration.
    /// - `snr`: `None` for a noiseless channel, or the SNR schedule (3 kHz
    ///   reference) over channel time.
    /// - `ref_signal_power`: the CLEAN active-signal mean power that SNR refers
    ///   to. Measure it once on the transmitted waveform; it is never
    ///   re-estimated from the faded output or from silence.
    /// - `seed_base` / `direction`: see [`derive_seed`].
    #[must_use]
    pub fn new(
        sample_rate: f32,
        fading: Option<&WattersonConfig>,
        snr: Option<SnrProfile>,
        ref_signal_power: f32,
        seed_base: u64,
        direction: u8,
    ) -> Self {
        let fading = fading.map(|cfg| {
            let rate = fading_rate_hz(cfg.doppler_spread_hz);
            let step = ((sample_rate / rate).round() as usize).max(1);
            let low_rate = sample_rate / step as f32;
            let mut rng = StdRng::seed_from_u64(derive_seed(seed_base, direction, STREAM_FADING));
            let taps: Vec<FadedTap> = cfg
                .taps
                .iter()
                .map(|tap| {
                    let tap_seed = rng.random::<u64>();
                    let mut doppler = DopplerProcess::new(
                        cfg.doppler_sigma_hz(),
                        low_rate,
                        StdRng::seed_from_u64(tap_seed),
                    );
                    let g0 = doppler.next();
                    let g1 = doppler.next();
                    FadedTap {
                        delay: (tap.delay_s * sample_rate).round().max(0.0) as usize,
                        amp: tap.power.max(0.0).sqrt(),
                        doppler,
                        g0,
                        g1,
                    }
                })
                .collect();
            let max_delay = taps.iter().map(|t| t.delay).max().unwrap_or(0);
            let group_delay = (HILBERT_TAPS - 1) / 2;
            FadingState {
                hilbert: StreamingFir::new(design_hilbert(HILBERT_TAPS)),
                inphase: std::iter::repeat_n(0.0, group_delay).collect(),
                analytic: std::iter::repeat_n(Complex32::new(0.0, 0.0), max_delay + 1).collect(),
                taps,
                step,
                phase: 0,
            }
        });
        let noise = snr.map(|profile| {
            (
                StdRng::seed_from_u64(derive_seed(seed_base, direction, STREAM_NOISE)),
                profile,
            )
        });
        Self {
            sample_rate,
            fading,
            noise,
            ref_signal_power,
            clock: 0,
        }
    }

    /// Fixed latency, in samples, between a sample entering and its faded image
    /// leaving (the Hilbert group delay); zero without fading.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        if self.fading.is_some() {
            (HILBERT_TAPS - 1) / 2
        } else {
            0
        }
    }

    /// Channel time in seconds: samples processed so far / sample rate.
    #[must_use]
    pub fn now_s(&self) -> f64 {
        self.clock as f64 / f64::from(self.sample_rate)
    }

    /// Samples processed so far.
    #[must_use]
    pub fn clock_samples(&self) -> u64 {
        self.clock
    }

    /// The noise standard deviation at absolute channel time `t_s`, or 0 for a
    /// noiseless channel.
    #[must_use]
    pub fn noise_std_at(&self, t_s: f64) -> f32 {
        match &self.noise {
            None => 0.0,
            Some((_, profile)) => {
                let snr_lin = 10.0f32.powf(profile.at(t_s) / 10.0);
                (self.ref_signal_power / snr_lin * (self.sample_rate / 2.0) / HF_NOISE_BW_HZ).sqrt()
            }
        }
    }

    /// Pass `x` through the channel, returning exactly `x.len()` samples.
    /// State (fading, delay line, Hilbert history, noise stream, clock) carries
    /// to the next call.
    pub fn process(&mut self, x: &[f32]) -> Vec<f32> {
        let mut out = match &mut self.fading {
            None => x.to_vec(),
            Some(f) => {
                let mut quad = Vec::with_capacity(x.len());
                f.hilbert.process(x, &mut quad);
                let mut y = Vec::with_capacity(x.len());
                for (&xi, &qi) in x.iter().zip(&quad) {
                    f.inphase.push_back(xi);
                    let re = f.inphase.pop_front().unwrap_or(0.0);
                    f.analytic.pop_front();
                    f.analytic.push_back(Complex32::new(re, qi));
                    let newest = f.analytic.len() - 1;
                    let frac = f.phase as f32 / f.step as f32;
                    let mut acc = Complex32::new(0.0, 0.0);
                    for tap in &f.taps {
                        let g = tap.g0 + (tap.g1 - tap.g0) * frac;
                        acc += f.analytic[newest - tap.delay] * g * tap.amp;
                    }
                    y.push(acc.re);
                    f.phase += 1;
                    if f.phase == f.step {
                        f.phase = 0;
                        for tap in &mut f.taps {
                            tap.g0 = tap.g1;
                            tap.g1 = tap.doppler.next();
                        }
                    }
                }
                y
            }
        };
        if self.noise.is_some() {
            let sr = f64::from(self.sample_rate);
            for (i, s) in out.iter_mut().enumerate() {
                let t = (self.clock + i as u64) as f64 / sr;
                let std = self.noise_std_at(t);
                let Some((rng, _)) = self.noise.as_mut() else {
                    break;
                };
                let u1: f32 = (1.0 - rng.random::<f32>()).max(1e-10);
                let theta: f32 = rng.random_range(0.0f32..TAU);
                *s += std * (-2.0 * u1.ln()).sqrt() * theta.cos();
            }
        }
        self.clock += x.len() as u64;
        out
    }
}

/// Which way a transmission crosses the [`VirtualCable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Station A transmits, station B receives.
    AToB,
    /// Station B transmits, station A receives.
    BToA,
}

/// A half-duplex two-station audio cable over persistent channels. Both
/// directions share one clock: while one station transmits, the other
/// direction's channel advances over silence, so both fading processes and the
/// SNR schedule stay aligned with real elapsed time.
#[derive(Debug)]
pub struct VirtualCable {
    a_to_b: PersistentChannel,
    b_to_a: PersistentChannel,
}

/// What each station hears over one stretch of cable time.
#[derive(Debug, Clone, PartialEq)]
pub struct CableOutput {
    /// Audio arriving at station B (from A's transmitter and the A→B noise).
    pub at_b: Vec<f32>,
    /// Audio arriving at station A (from B's transmitter and the B→A noise).
    pub at_a: Vec<f32>,
}

impl VirtualCable {
    /// Build both directions from one base seed (direction 0 is A→B, 1 is
    /// B→A; see [`derive_seed`]).
    #[must_use]
    pub fn new(
        sample_rate: f32,
        fading: Option<&WattersonConfig>,
        snr: Option<SnrProfile>,
        ref_signal_power: f32,
        seed_base: u64,
    ) -> Self {
        Self {
            a_to_b: PersistentChannel::new(
                sample_rate,
                fading,
                snr.clone(),
                ref_signal_power,
                seed_base,
                0,
            ),
            b_to_a: PersistentChannel::new(
                sample_rate,
                fading,
                snr,
                ref_signal_power,
                seed_base,
                1,
            ),
        }
    }

    /// Cable time in seconds.
    #[must_use]
    pub fn now_s(&self) -> f64 {
        self.a_to_b.now_s()
    }

    /// One station transmits `samples`; the other direction carries silence
    /// for the same playout time. Returns what each station hears.
    pub fn transmit(&mut self, direction: Direction, samples: &[f32]) -> CableOutput {
        let silence = vec![0.0f32; samples.len()];
        let (ab_in, ba_in) = match direction {
            Direction::AToB => (samples, silence.as_slice()),
            Direction::BToA => (silence.as_slice(), samples),
        };
        CableOutput {
            at_b: self.a_to_b.process(ab_in),
            at_a: self.b_to_a.process(ba_in),
        }
    }

    /// Neither station transmits for `seconds` (turnaround, PTT tail, think
    /// time). Returns the noise and channel tails each station hears.
    pub fn idle(&mut self, seconds: f64) -> CableOutput {
        let n = (seconds.max(0.0) * f64::from(self.a_to_b.sample_rate)).round() as usize;
        let silence = vec![0.0f32; n];
        CableOutput {
            at_b: self.a_to_b.process(&silence),
            at_a: self.b_to_a.process(&silence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watterson::WattersonPreset;

    const FS: f32 = 48_000.0;

    fn tone(n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * f * i as f32 / FS).sin() * 0.5)
            .collect()
    }

    fn poor_channel(seed: u64, snr: Option<SnrProfile>) -> PersistentChannel {
        PersistentChannel::new(
            FS,
            Some(&WattersonPreset::Poor.config()),
            snr,
            0.125,
            seed,
            0,
        )
    }

    fn run_chunked(ch: &mut PersistentChannel, x: &[f32], sizes: &[usize]) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        let mut pos = 0;
        let mut k = 0;
        while pos < x.len() {
            let len = sizes[k % sizes.len()].min(x.len() - pos);
            out.extend(ch.process(&x[pos..pos + len]));
            pos += len;
            k += 1;
        }
        out
    }

    #[test]
    fn chunk_partition_matches_single_call() {
        let x = tone(30_000, 1_500.0);
        let whole = poor_channel(7, Some(SnrProfile::constant(3.0))).process(&x);
        let split = run_chunked(
            &mut poor_channel(7, Some(SnrProfile::constant(3.0))),
            &x,
            &[1, 17, 480, 4_097, 333],
        );
        assert_eq!(whole.len(), split.len());
        for (a, b) in whole.iter().zip(&split) {
            assert!((a - b).abs() <= 1e-6, "split run diverged: {a} vs {b}");
        }
    }

    #[test]
    fn chunk_partition_matches_on_awgn_with_a_ramp() {
        let x = tone(20_000, 900.0);
        let ramp = SnrProfile::from_points(vec![(0.0, 10.0), (0.4, -6.0)]).unwrap();
        let mk = || PersistentChannel::new(FS, None, Some(ramp.clone()), 0.125, 11, 1);
        let whole = mk().process(&x);
        let split = run_chunked(&mut mk(), &x, &[999, 1]);
        assert_eq!(whole, split);
    }

    #[test]
    fn delay_line_carries_across_chunk_boundary() {
        // A burst that ends exactly at a chunk boundary: its 2 ms echo (96
        // samples) must still appear in the next chunk, identical to one call.
        let mut x = tone(4_800, 1_200.0);
        x.extend(std::iter::repeat_n(0.0, 4_800));
        let whole = poor_channel(3, None).process(&x);
        let mut ch = poor_channel(3, None);
        let mut split = ch.process(&x[..4_800]);
        let tail = ch.process(&x[4_800..]);
        // The echo window: after the Hilbert latency, the second path still
        // carries the burst for 96 samples past its end.
        let lat = ch.latency_samples();
        let echo = &tail[lat..lat + 96];
        assert!(echo.iter().map(|v| v * v).sum::<f32>() > 1e-6);
        split.extend(tail);
        for (a, b) in whole.iter().zip(&split) {
            assert!((a - b).abs() <= 1e-6);
        }
    }

    #[test]
    fn same_seed_reproduces_and_other_seed_differs() {
        let x = tone(9_600, 700.0);
        let a = poor_channel(42, Some(SnrProfile::constant(0.0))).process(&x);
        let b = poor_channel(42, Some(SnrProfile::constant(0.0))).process(&x);
        let c = poor_channel(43, Some(SnrProfile::constant(0.0))).process(&x);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn directions_and_streams_get_independent_seeds() {
        let s = [
            derive_seed(1, 0, STREAM_FADING),
            derive_seed(1, 0, STREAM_NOISE),
            derive_seed(1, 1, STREAM_FADING),
            derive_seed(1, 1, STREAM_NOISE),
            derive_seed(2, 0, STREAM_FADING),
        ];
        for i in 0..s.len() {
            for j in i + 1..s.len() {
                assert_ne!(s[i], s[j]);
            }
        }
    }

    fn low_rate_run(sigma_hz: f32, rate: f32, n: usize, seed: u64) -> Vec<Complex32> {
        let mut p = DopplerProcess::new(sigma_hz, rate, StdRng::seed_from_u64(seed));
        (0..n).map(|_| p.next()).collect()
    }

    #[test]
    fn fading_process_has_unit_ensemble_power_and_rayleigh_fades() {
        // Poor: spread 1 Hz -> sigma 0.5 Hz; 4000 s at 100 Hz.
        let g = low_rate_run(0.5, 100.0, 400_000, 5);
        let p: Vec<f32> = g.iter().map(|c| c.norm_sqr()).collect();
        let mean = p.iter().sum::<f32>() / p.len() as f32;
        assert!((mean - 1.0).abs() < 0.1, "E|g|^2 = {mean}");
        // Rayleigh: P(|g|^2 < 0.1) = 1 - exp(-0.1) = 0.095. A renormalized
        // (constant-power) process would show no such fades.
        let deep = p.iter().filter(|&&v| v < 0.1).count() as f32 / p.len() as f32;
        assert!((deep - 0.095).abs() < 0.03, "deep-fade fraction {deep}");
    }

    #[test]
    fn fading_autocorrelation_matches_gaussian_doppler() {
        // Gaussian power PSD with sigma s has R(tau) = exp(-2 pi^2 s^2 tau^2).
        let sigma = 0.5f32;
        let rate = 100.0f32;
        let g = low_rate_run(sigma, rate, 400_000, 9);
        let r0: f32 = g.iter().map(|c| c.norm_sqr()).sum::<f32>() / g.len() as f32;
        for &tau in &[0.3f32, 0.6] {
            let lag = (tau * rate) as usize;
            let r: Complex32 = g
                .iter()
                .zip(&g[lag..])
                .map(|(a, b)| a * b.conj())
                .sum::<Complex32>()
                / (g.len() - lag) as f32;
            let rho = r.norm() / r0;
            let want = (-2.0 * PI * PI * sigma * sigma * tau * tau).exp();
            assert!((rho - want).abs() < 0.08, "tau {tau}: {rho} vs {want}");
        }
    }

    #[test]
    fn audio_path_preserves_ensemble_power() {
        // A fast 20 Hz spread gives ~hundreds of independent fades in 4 s, so
        // the time average approaches the ensemble mean: output power = input.
        let cfg = WattersonConfig {
            taps: WattersonPreset::Poor.config().taps,
            doppler_spread_hz: 20.0,
        };
        let x = tone(192_000, 1_500.0);
        let p_in = crate::mean_power(&x);
        let mut ch = PersistentChannel::new(FS, Some(&cfg), None, p_in, 21, 0);
        let y = ch.process(&x);
        let p_out = crate::mean_power(&y[ch.latency_samples() + 1_000..]);
        assert!(
            (p_out / p_in - 1.0).abs() < 0.2,
            "power ratio {}",
            p_out / p_in
        );
    }

    #[test]
    fn noise_is_referred_to_fixed_clean_power() {
        // Noiseless input at 0 dB in 3 kHz: noise power = ref * (fs/2) / 3000.
        let mut ch = PersistentChannel::new(FS, None, Some(SnrProfile::constant(0.0)), 0.5, 1, 0);
        let y = ch.process(&vec![0.0; 96_000]);
        let want = 0.5 * (FS / 2.0) / HF_NOISE_BW_HZ;
        let got = crate::mean_power(&y);
        assert!((got / want - 1.0).abs() < 0.03, "{got} vs {want}");
    }

    #[test]
    fn cop169_ramp_follows_its_schedule() {
        let r = SnrProfile::cop169_ramp();
        assert_eq!(r.duration_s(), 170.0);
        assert_eq!(r.at(0.0), 20.0);
        assert_eq!(r.at(20.0), 20.0);
        assert!((r.at(35.0) - 7.0).abs() < 1e-4);
        assert_eq!(r.at(65.0), -6.0);
        assert!((r.at(95.0) - 7.0).abs() < 1e-4);
        assert_eq!(r.at(200.0), 20.0);
        assert!(SnrProfile::from_points(vec![]).is_none());
        assert!(SnrProfile::from_points(vec![(1.0, 0.0), (1.0, 2.0)]).is_none());
    }

    #[test]
    fn cable_keeps_both_directions_on_one_clock() {
        let mut cable = VirtualCable::new(
            FS,
            Some(&WattersonPreset::Poor.config()),
            Some(SnrProfile::constant(10.0)),
            0.125,
            0x1691_0000,
        );
        let frame = tone(4_800, 1_000.0);
        let out = cable.transmit(Direction::AToB, &frame);
        assert_eq!(out.at_b.len(), 4_800);
        assert_eq!(out.at_a.len(), 4_800);
        let gap = cable.idle(0.25);
        assert_eq!(gap.at_a.len(), 12_000);
        let back = cable.transmit(Direction::BToA, &frame);
        // B's transmission arrives at A; B hears only noise.
        assert!(crate::mean_power(&back.at_a) > crate::mean_power(&back.at_b));
        assert!((cable.now_s() - 0.45).abs() < 1e-9);
    }
}
