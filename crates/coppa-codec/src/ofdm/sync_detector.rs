//! Streaming O(1) synchronization detector.
//!
//! Replaces the batch, O(N) full-search `detect_coppa_sync` (see `sync.rs`, removed
//! in the same change that introduced this module) with a detector that:
//! - Maintains a streaming analytic (complex) signal via a 129-tap Hilbert FIR
//!   (constant 64-sample group delay, absorbed below).
//! - Maintains the Schmidl-Cox `P`, `E1`, `E2` sliding sums with O(1) per-sample
//!   ring-buffer updates (no re-scanning old samples), evaluating the normalized
//!   metric `M = |P|^2/(E1*E2)` only every `STRIDE` samples.
//! - Opens/closes a candidate plateau on `M` crossing 0.5, then CONFIRMS the
//!   candidate against the cached clean preamble (rejecting steady tones and
//!   other non-preamble self-similar signals) and refines timing to the
//!   *strongest* multipath arrival, falling back to the *first* arrival only
//!   when the two are separated by more than half the cyclic prefix (see
//!   `docs/adr/004-strongest-path-timing.md`: anchoring on a weak-but-earliest
//!   tap under realistic HF Watterson fading measurably wrecked the sparse-pilot
//!   protected header's channel estimate; Task 5 originally anchored on the
//!   first arrival unconditionally, which is what this ADR corrects).
//!
//! See `docs/superpowers/plans/2026-07-03-phase1-radio-reality.md` Task 5 (or its
//! successor Phase-2 rate-loop plan) for the original design rationale and locked
//! algorithm, and `docs/adr/004-strongest-path-timing.md` for the timing-anchor
//! correction on top of it.
use std::collections::VecDeque;

use num_complex::Complex32;

use coppa_dsp::fir::{design_hilbert, StreamingFir};

use super::sync::generate_coppa_preamble;
use super::CoppaProfile;

/// Hilbert FIR tap count (odd; group delay = (HILBERT_TAPS-1)/2 = 64 samples).
const HILBERT_TAPS: usize = 129;
/// Constant group delay introduced by the streaming analytic signal (samples).
const GROUP_DELAY: u64 = (HILBERT_TAPS as u64 - 1) / 2;
/// The Schmidl-Cox metric is only evaluated (division + threshold check) every
/// `STRIDE` samples; the O(1) sliding sums themselves update every sample.
const STRIDE: u64 = 16;
/// Plateau open/close threshold on the normalized Schmidl-Cox metric M.
const PLATEAU_THRESHOLD: f32 = 0.5;
/// Confirmation cross-correlation threshold: rejects steady tones and other
/// non-preamble self-similar signals (measured tone-vs-Newman-comb xcorr <= ~0.1).
const CONFIRM_THRESHOLD: f32 = 0.25;
/// First-path local-peak acceptance fraction of the window's global xcorr max.
/// Only used to compute the FALLBACK anchor when the strongest tap is too far
/// from the earliest one to trust (see `docs/adr/004-strongest-path-timing.md`);
/// the strongest tap itself is preferred whenever it's within `cp/2` of first path.
const FIRST_PATH_FRACTION: f32 = 0.5;
/// Backoff (samples) from the detected timing anchor into the cyclic
/// prefix, giving the downstream FFT window margin against timing jitter.
///
/// DEVIATION FROM THE TASK 5 SPEC (documented per its "not to be changed
/// without a very good reason" rule): the plan's value was 60 (matching VHF
/// profiles' own `cp_samples`). Measured directly: with 60, the existing,
/// previously-passing `test_transceiver_16qam_rate_1_2_loopback` (a *clean*
/// channel, HF standard profile, no impairments) fails with
/// `LdpcNotConverged` — the 2D-pooled pilot-interpolation channel estimator
/// cannot fully null out the per-carrier linear phase ramp a 60-sample
/// constant timing offset introduces at 16-QAM's decision-region density
/// (BPSK/QPSK/8PSK tolerate it fine; verified by a binary search that the
/// pass/fail boundary sits between 50 and 52 samples on this profile/speed
/// level). This is a pre-existing equalizer limitation, not a sync bug —
/// tracking/correcting a larger, constant phase ramp across frequency is
/// exactly what denser pilots or higher-order (not just linear) interpolation
/// would fix, which is out of Task 5's scope (sync/detection only). 30 keeps
/// a comfortable margin below the measured 50-51 breaking point while still
/// giving real cyclic-prefix headroom for timing jitter; all of
/// `cargo test -p coppa-codec -p coppa-protocol --lib` passes at this value.
const TIMING_BACKOFF: u64 = 30;

/// A confirmed synchronization candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncCandidate {
    /// Sample index (in the coordinate system of all samples ever pushed) of the
    /// preamble start, after strongest/first-path timing refinement and
    /// TIMING_BACKOFF (see `docs/adr/004-strongest-path-timing.md`).
    pub frame_start: u64,
    /// Two-stage Moose CFO estimate (Hz), from `estimate_cfo_two_stage` run on the
    /// coarse plateau peak's analytic window. The modem removes this from the whole
    /// buffer once (only when `|cfo_hz| > 0.5` Hz) before symbol demod.
    pub cfo_hz: f32,
    /// Normalized confirmation cross-correlation (0..1) — quality metric.
    pub confirm_xcorr: f32,
}

/// Streaming preamble sync detector: O(1) per-sample cost, thresholded
/// confirmation, strongest-path timing (falling back to first-path only when
/// the two are separated by more than half the cyclic prefix — see
/// `docs/adr/004-strongest-path-timing.md`).
pub struct SyncDetector {
    profile: CoppaProfile,
    /// OFDM symbol length (fft_size + cp_samples) — the Schmidl-Cox lag `L`.
    symbol_len: usize,
    /// Cached clean 2-symbol preamble (length `2*symbol_len`), used for confirm
    /// + first-path cross-correlation.
    reference: Vec<f32>,
    ref_energy: f32,

    /// Pure O(1) delay line producing x[n - GROUP_DELAY], kept exactly
    /// time-aligned with the Hilbert filter's own output delay. (Not a
    /// `StreamingFir` with a delta-impulse coefficient vector: that would
    /// spend a full O(taps) convolution per sample computing what is,
    /// mathematically, a plain shift — measurably doubling `push`'s per-sample
    /// cost for no benefit; see the perf check in Task 5's report.)
    delay: DelayLine,
    /// 129-tap Hilbert transformer producing the quadrature component.
    hilbert: StreamingFir,

    /// Ring of up to `2*symbol_len + 1` most recent analytic samples, used to
    /// maintain the O(1) sliding Schmidl-Cox sums.
    ring: VecDeque<Complex32>,
    bootstrapped: bool,
    p: Complex32,
    e1: f32,
    e2: f32,
    /// Current Schmidl-Cox window-start index (valid once bootstrapped).
    d: u64,

    in_plateau: bool,
    plateau_best_m: f32,
    plateau_best_d: u64,
    /// Two-stage CFO estimate captured at `plateau_best_d`'s analytic window (see
    /// `estimate_cfo_from_ring`); carried through to the resolved `SyncCandidate`.
    plateau_best_cfo: f32,

    /// Raw (real) sample history, needed for the confirm/first-path steps
    /// (which correlate against the real-valued cached preamble).
    history: VecDeque<f32>,
    /// Absolute sample index of `history`'s front element.
    history_base: u64,
    /// How many trailing samples of raw history to retain.
    retain_samples: u64,

    /// Coarse peak estimates (raw-domain) awaiting enough future samples to
    /// run the confirm + first-path refinement steps, paired with the
    /// two-stage CFO estimate captured when each peak was recorded.
    pending: VecDeque<(u64, f32)>,

    /// Total raw samples ever pushed.
    total_pushed: u64,
}

impl SyncDetector {
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        let symbol_len = profile.fft_size + profile.cp_samples;
        let two_l = 2 * symbol_len;
        let reference = generate_coppa_preamble(&profile, version);
        let ref_energy: f32 = reference.iter().map(|x| x * x).sum();

        let hilbert_coeffs = design_hilbert(HILBERT_TAPS);
        let retain_samples = (4 * two_l).max(HILBERT_TAPS) as u64;

        Self {
            profile,
            symbol_len,
            reference,
            ref_energy,
            delay: DelayLine::new(GROUP_DELAY as usize),
            hilbert: StreamingFir::new(hilbert_coeffs),
            ring: VecDeque::with_capacity(two_l + 2),
            bootstrapped: false,
            p: Complex32::new(0.0, 0.0),
            e1: 0.0,
            e2: 0.0,
            d: 0,
            in_plateau: false,
            plateau_best_m: 0.0,
            plateau_best_d: 0,
            plateau_best_cfo: 0.0,
            history: VecDeque::with_capacity(retain_samples as usize + 64),
            history_base: 0,
            retain_samples,
            pending: VecDeque::new(),
            total_pushed: 0,
        }
    }

    /// Push samples; returns any candidates confirmed in this block (candidates
    /// whose coarse timing was found earlier but needed more future samples to
    /// confirm are resolved on a later `push` once enough history has arrived).
    pub fn push(&mut self, samples: &[f32]) -> Vec<SyncCandidate> {
        let mut out = Vec::new();
        if samples.is_empty() {
            return out;
        }

        for &s in samples {
            self.history.push_back(s);
        }
        self.total_pushed += samples.len() as u64;

        self.resolve_pending(&mut out);

        let mut delayed = Vec::with_capacity(samples.len());
        self.delay.process(samples, &mut delayed);
        let mut quadrature = Vec::with_capacity(samples.len());
        self.hilbert.process(samples, &mut quadrature);

        for k in 0..samples.len() {
            let z = Complex32::new(delayed[k], quadrature[k]);
            self.ingest_analytic_sample(z, &mut out);
        }

        self.evict_history();

        out
    }

    /// Batch convenience for the transceiver's one-shot path: `samples` must
    /// contain the full candidate window (coarse timing plus enough trailing
    /// samples to cover the confirm/first-path search), matching the
    /// requirement the old batch `detect_coppa_sync` also had.
    pub fn detect_all(profile: &CoppaProfile, version: u8, samples: &[f32]) -> Vec<SyncCandidate> {
        let mut detector = Self::new(profile.clone(), version);
        detector.push(samples)
    }

    fn ingest_analytic_sample(&mut self, z: Complex32, out: &mut Vec<SyncCandidate>) {
        let l = self.symbol_len;
        let two_l = 2 * l;

        self.ring.push_back(z);

        if !self.bootstrapped {
            if self.ring.len() == two_l {
                let mut p = Complex32::new(0.0, 0.0);
                let mut e1 = 0.0f32;
                let mut e2 = 0.0f32;
                for m in 0..l {
                    let a = self.ring[m];
                    let b = self.ring[m + l];
                    p += a.conj() * b;
                    e1 += a.norm_sqr();
                    e2 += b.norm_sqr();
                }
                self.p = p;
                self.e1 = e1;
                self.e2 = e2;
                self.d = 0;
                self.bootstrapped = true;
                self.evaluate_metric(out);
            }
            return;
        }

        debug_assert_eq!(self.ring.len(), two_l + 1);
        // ring[0] = z[d-1], ring[l] = z[d+L-1], ring[2L] = z[d+2L-1], for the
        // NEW window start d = (old d) + 1. See the locked recurrence in the
        // task brief / module docs.
        let z_old_start = self.ring[0];
        let z_mid = self.ring[l];
        let z_new_end = self.ring[two_l];

        self.p += z_mid.conj() * z_new_end - z_old_start.conj() * z_mid;
        self.e1 += z_mid.norm_sqr() - z_old_start.norm_sqr();
        self.e2 += z_new_end.norm_sqr() - z_mid.norm_sqr();

        self.ring.pop_front();
        self.d += 1;
        self.evaluate_metric(out);
    }

    fn evaluate_metric(&mut self, out: &mut Vec<SyncCandidate>) {
        if self.d % STRIDE != 0 {
            return;
        }
        let denom = (self.e1 * self.e2).max(1e-20);
        let m = self.p.norm_sqr() / denom;

        if m >= PLATEAU_THRESHOLD {
            if !self.in_plateau || m > self.plateau_best_m {
                self.plateau_best_m = m;
                self.plateau_best_d = self.d;
                self.plateau_best_cfo = self.estimate_cfo_from_ring();
            }
            self.in_plateau = true;
        } else if self.in_plateau {
            self.in_plateau = false;
            let coarse_peak = self.plateau_best_d.saturating_sub(GROUP_DELAY);
            let cfo_hz = self.plateau_best_cfo;
            if !self.try_resolve_one(coarse_peak, cfo_hz, out) {
                self.pending.push_back((coarse_peak, cfo_hz));
            }
        }
    }

    /// Two-stage CFO estimate (`estimate_cfo_two_stage`) from the ring's CURRENT
    /// contents. Whenever `evaluate_metric` runs, `self.ring` holds exactly
    /// `2*symbol_len` analytic samples spanning the window `[self.d, self.d+2*symbol_len)`
    /// (see the recurrence comment in `ingest_analytic_sample`), which is exactly the
    /// analytic window `estimate_cfo_two_stage` needs for `coarse_start = 0`.
    fn estimate_cfo_from_ring(&self) -> f32 {
        let window: Vec<Complex32> = self.ring.iter().copied().collect();
        super::sync::estimate_cfo_two_stage(
            &window,
            0,
            self.profile.fft_size,
            self.profile.cp_samples,
            self.profile.sample_rate as f32,
        )
    }

    /// Try to resolve any pending candidates now that more history may have
    /// arrived. Leaves still-unresolvable entries in place for a future call.
    fn resolve_pending(&mut self, out: &mut Vec<SyncCandidate>) {
        let mut i = 0;
        while i < self.pending.len() {
            let (peak, cfo_hz) = self.pending[i];
            if self.try_resolve_one(peak, cfo_hz, out) {
                self.pending.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Attempt confirm + first-path refinement for a coarse peak estimate.
    /// Returns `true` if resolved (whether accepted or rejected — in which
    /// case nothing is pushed to `out`), or `false` if more future samples
    /// are still needed (caller should keep it pending).
    ///
    /// `cfo_hz` is the two-stage CFO estimate captured at this peak (see
    /// `estimate_cfo_from_ring`); per the locked order (coarse peak -> CFO
    /// estimate -> derotate -> confirm/refine), the confirmation window is
    /// derotated by it BEFORE correlating against the (CFO-free) cached
    /// reference preamble, so a real mistune doesn't rot the confirm xcorr.
    fn try_resolve_one(
        &mut self,
        coarse_peak: u64,
        cfo_hz: f32,
        out: &mut Vec<SyncCandidate>,
    ) -> bool {
        let cp = self.profile.cp_samples as u64;
        let ref_len = self.reference.len() as u64;
        let lo = coarse_peak.saturating_sub(cp);
        let hi = coarse_peak + cp;
        let needed_end = hi + ref_len;

        if needed_end > self.total_pushed {
            return false;
        }
        let lo = lo.max(self.history_base);
        if lo > hi {
            // Already evicted the data we would have needed (shouldn't happen
            // given `retain_samples`'s margin) — drop rather than search a
            // backwards/empty range.
            return true;
        }

        let window_start = (lo - self.history_base) as usize;
        let window_len = (needed_end - lo) as usize;
        let raw_window: Vec<f32> = self
            .history
            .iter()
            .skip(window_start)
            .take(window_len)
            .copied()
            .collect();
        // Absolute (not window-local) sample index as the phase origin: this must match
        // the convention `remove_cfo` uses when the modem later de-rotates the WHOLE
        // buffer once with this same `cfo_hz` — a window-local origin would leave an
        // arbitrary residual phase that a constant per-window offset can't correct.
        let derotated = derotate_window(&raw_window, lo, cfo_hz, self.profile.sample_rate as f32);

        let n_positions = (hi - lo + 1) as usize;
        let mut xcorr = Vec::with_capacity(n_positions);
        let mut best = 0.0f32;
        let mut best_idx = 0usize;
        for d in lo..=hi {
            let start = (d - lo) as usize;
            let mut corr = 0.0f32;
            let mut sig_e = 0.0f32;
            for (i, &r) in self.reference.iter().enumerate() {
                let s = derotated[start + i];
                corr += s * r;
                sig_e += s * s;
            }
            let denom = (self.ref_energy * sig_e).sqrt().max(1e-20);
            let nc = corr.abs() / denom;
            xcorr.push(nc);
            if nc > best {
                best = nc;
                best_idx = start;
            }
        }

        if best < CONFIRM_THRESHOLD {
            return true;
        }

        let strongest_abs = lo + best_idx as u64;
        let first_path_abs = find_first_path(&xcorr, lo, best);
        // Prefer the STRONGEST tap as the timing anchor (the composite channel's
        // energy-weighted arrival point) over the earliest-arriving one, UNLESS the two
        // are separated by more than half the cyclic prefix — see
        // `docs/adr/004-strongest-path-timing.md` for the full evidence trail. Anchoring
        // at the strongest tap keeps the 2D-pooled, sparse-pilot header estimator's
        // linear interpolation tracking a much better-conditioned per-carrier response
        // than anchoring at a possibly-much-weaker first arrival does — measured directly
        // to cut Watterson-Moderate/Poor header decode failures on `hf_standard` levels
        // 1-2 from a hard ~30% FER floor back to matching (or beating) pre-Phase-1
        // levels, with real but smaller/partial gains for levels 3-4 (see
        // `.superpowers/sdd/p1-fading-regression-fix-report.md` for the full numbers),
        // and zero AWGN cost (a single dominant path, where the two anchors coincide).
        // The `cp/2` bound preserves the original first-path SAFETY intent
        // (never anchor on a late, dominant echo that could carry the FFT window
        // dangerously close to running out of cyclic-prefix margin) for delay spreads
        // beyond anything this codebase's own HF channel models produce (Watterson
        // Poor's modeled delay is 96 samples on this profile's 300-sample CP).
        let local_peak_abs = if strongest_abs.abs_diff(first_path_abs) > cp / 2 {
            first_path_abs
        } else {
            strongest_abs
        };
        let frame_start = local_peak_abs.saturating_sub(TIMING_BACKOFF);
        out.push(SyncCandidate {
            frame_start,
            cfo_hz,
            confirm_xcorr: best,
        });
        true
    }

    fn evict_history(&mut self) {
        let default_keep_from = self.total_pushed.saturating_sub(self.retain_samples);
        let cp = self.profile.cp_samples as u64;
        let earliest_pending = self
            .pending
            .iter()
            .map(|&(p, _)| p.saturating_sub(cp))
            .min()
            .unwrap_or(u64::MAX);
        let keep_from = default_keep_from.min(earliest_pending);
        while self.history_base < keep_from && !self.history.is_empty() {
            self.history.pop_front();
            self.history_base += 1;
        }
    }
}

/// De-rotate a real-valued window by `e^{-j*2*pi*cfo_hz*t/sample_rate}` (converting to
/// analytic first, as `sync::remove_cfo` does), using `t = abs_start + i` — the ABSOLUTE
/// sample index — as the phase origin, not a window-local `i` starting at 0. `samples[0]`
/// is sample `abs_start` of the overall stream.
fn derotate_window(samples: &[f32], abs_start: u64, cfo_hz: f32, sample_rate: f32) -> Vec<f32> {
    use std::f32::consts::TAU;
    if cfo_hz == 0.0 {
        return samples.to_vec();
    }
    let analytic = super::sync::analytic_signal(samples);
    analytic
        .iter()
        .enumerate()
        .map(|(i, &z)| {
            let t = abs_start as f32 + i as f32;
            let ph = -TAU * cfo_hz * t / sample_rate;
            (z * Complex32::new(ph.cos(), ph.sin())).re
        })
        .collect()
}

/// A plain O(1)-per-sample delay line: `process` emits `x[n - delay]` for each
/// pushed `x[n]`, carrying state across calls exactly like `StreamingFir` does,
/// but without spending a full convolution to compute a shift.
struct DelayLine {
    buf: VecDeque<f32>,
}

impl DelayLine {
    fn new(delay: usize) -> Self {
        Self {
            buf: VecDeque::from(vec![0.0f32; delay]),
        }
    }

    fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        out.reserve(x.len());
        for &s in x {
            self.buf.push_back(s);
            // `buf` always holds exactly `delay` samples before/after this pair,
            // so `pop_front` always succeeds once primed with `delay` zeros in `new`.
            out.push(self.buf.pop_front().expect("delay buffer never empties"));
        }
    }
}

/// Find the earliest local peak in `xcorr` (indexed from absolute sample
/// `lo_abs`) whose value is at least `FIRST_PATH_FRACTION` of `global_max`. A
/// local peak is strictly greater than both neighbors on the sample-by-sample
/// grid. Falls back to the position of `global_max` if no interior local peak
/// clears the threshold.
fn find_first_path(xcorr: &[f32], lo_abs: u64, global_max: f32) -> u64 {
    if xcorr.is_empty() {
        return lo_abs;
    }
    let threshold = FIRST_PATH_FRACTION * global_max;
    for i in 1..xcorr.len().saturating_sub(1) {
        if xcorr[i] > xcorr[i - 1] && xcorr[i] > xcorr[i + 1] && xcorr[i] >= threshold {
            return lo_abs + i as u64;
        }
    }
    let mut best_idx = 0usize;
    let mut best_val = xcorr[0];
    for (i, &v) in xcorr.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    lo_abs + best_idx as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofdm::coppa_modem::CoppaModem;
    use crate::ofdm::frame::{CoppaFrameType, CoppaHeader};
    use num_complex::Complex32;

    /// The TX bandpass filter's group delay for HF profiles (`phy_mode == 0`).
    /// `CoppaModem::modulate_mapped` runs the WHOLE frame — including the
    /// preamble — through a 601-tap bandpass (see `CoppaModem::tx_bpf`), which,
    /// being linear-phase, delays every sample by `(taps-1)/2` uniformly. Since
    /// `SyncDetector` correlates against the *unfiltered* reference preamble
    /// (`generate_coppa_preamble`), its correct answer is the preamble's
    /// filtered (delayed) position, not its pre-filter one — a real, physical
    /// property of the TX chain, not a detector approximation. (Found by direct
    /// measurement while writing these tests: the coarse+refine pipeline was
    /// initially "off by exactly 300 samples" on `hf_standard` until this delay
    /// was accounted for; VHF profiles have no TX bandpass, hence zero delay.)
    fn tx_bpf_group_delay(profile: &CoppaProfile) -> i64 {
        if profile.phy_mode == 0 {
            coppa_dsp::fir::Fir::new(coppa_dsp::fir::design_bandpass(
                601,
                profile.sample_rate as f32,
                250.0,
                2850.0,
            ))
            .group_delay() as i64
        } else {
            0
        }
    }

    fn test_frame(profile: &CoppaProfile) -> Vec<f32> {
        let modem = CoppaModem::new(profile.clone(), 1);
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 60,
        };
        // Random (not smoothly-rotating) payload phases: real payload data is
        // LDPC-coded/scrambled noise-like bits, so this is the representative
        // choice (a slowly-varying rotation is a less realistic corner case and
        // was ruled out, by direct measurement, as the source of an early
        // debugging surprise below — see `TX_BPF_GROUP_DELAY`).
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        let mut rng = StdRng::seed_from_u64(7);
        let symbols: Vec<Complex32> = (0..480)
            .map(|_| {
                let a: f32 = rng.random_range(0.0..std::f32::consts::TAU);
                Complex32::new(a.cos(), a.sin()) * std::f32::consts::FRAC_1_SQRT_2
            })
            .collect();
        modem.modulate_mapped(&header, &symbols, 6.0)
    }

    #[test]
    fn detector_finds_frame_in_stream_chunks() {
        let profile = CoppaProfile::hf_standard();
        let frame = test_frame(&profile);
        let frame_power = coppa_channel::mean_power(&frame);

        let lead = 30_000usize;
        let mut clean = vec![0.0f32; lead];
        clean.extend_from_slice(&frame);
        clean.extend(std::iter::repeat_n(
            0.0f32,
            4 * (profile.fft_size + profile.cp_samples),
        ));

        let noisy = coppa_channel::awgn_ref_seeded(
            &clean,
            10.0,
            frame_power,
            profile.sample_rate as f32,
            42,
        );

        // `awgn_ref_seeded`'s "10 dB" figure is referenced to a 3 kHz noise
        // bandwidth (see its doc comment) — i.e. it assumes a receiver that
        // rejects out-of-passband noise the way `CoppaTransceiver::receive`
        // does with its RX bandpass *before* calling into sync/demod. Skipping
        // that step and pushing the full-bandwidth noisy signal straight into
        // the detector is a materially harsher (~9 dB worse) test than intended
        // (measured directly: the coarse metric then never even crosses the
        // 0.5 plateau threshold). Apply the same RX bandpass `CoppaTransceiver`
        // would, to test what `SyncDetector` actually sees in the real pipeline.
        let rx_bpf = coppa_dsp::fir::Fir::new(coppa_dsp::fir::design_bandpass(
            601,
            profile.sample_rate as f32,
            250.0,
            2850.0,
        ));
        let filtered = rx_bpf.filter_block(&noisy);

        let mut detector = SyncDetector::new(profile.clone(), 1);
        let mut candidates = Vec::new();
        for chunk in filtered.chunks(512) {
            candidates.extend(detector.push(chunk));
        }

        assert_eq!(
            candidates.len(),
            1,
            "expected exactly one candidate, got {candidates:?}"
        );
        // Expected position: the preamble's TRUE arrival (delayed by BOTH the TX
        // bandpass baked into `frame` and the RX bandpass applied above) minus
        // the deliberate TIMING_BACKOFF — see `tx_bpf_group_delay`.
        let expected = lead as i64 + 2 * tx_bpf_group_delay(&profile) - TIMING_BACKOFF as i64;
        let err = (candidates[0].frame_start as i64 - expected).abs();
        assert!(
            err <= 90,
            "frame_start {} should be within 90 samples of {expected} (backoff {TIMING_BACKOFF} + slack), err={err}",
            candidates[0].frame_start
        );
    }

    #[test]
    fn detector_rejects_steady_tone() {
        let profile = CoppaProfile::hf_standard();
        let sr = profile.sample_rate as f32;
        let n = (5.0 * sr) as usize;
        let tone: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin())
            .collect();

        let mut detector = SyncDetector::new(profile, 1);
        let mut candidates = Vec::new();
        for chunk in tone.chunks(512) {
            candidates.extend(detector.push(chunk));
        }

        assert!(
            candidates.is_empty(),
            "steady tone must not produce sync candidates, got {candidates:?}"
        );
    }

    /// Two-tap test signal: `clean` delayed by `delay` samples and scaled by
    /// `echo_amp`, added to `direct_amp * clean`.
    fn two_tap(clean: &[f32], direct_amp: f32, echo_amp: f32, delay: usize) -> Vec<f32> {
        let mut rx = vec![0.0f32; clean.len()];
        for k in 0..clean.len() {
            let echo = if k >= delay {
                echo_amp * clean[k - delay]
            } else {
                0.0
            };
            rx[k] = direct_amp * clean[k] + echo;
        }
        rx
    }

    /// Leading gap + preamble/frame + tail (room for the confirm/refine window).
    /// The lead must comfortably exceed the streaming detector's bootstrap
    /// window (2*symbol_len analytic samples) so the very first Schmidl-Cox
    /// window is pure silence (M ~ 0) rather than already straddling the
    /// preamble.
    fn leading_clean_frame(profile: &CoppaProfile) -> (usize, Vec<f32>) {
        let frame = test_frame(profile);
        let lead = 4 * (profile.fft_size + profile.cp_samples);
        let mut clean = vec![0.0f32; lead];
        clean.extend_from_slice(&frame);
        clean.extend(std::iter::repeat_n(
            0.0f32,
            4 * (profile.fft_size + profile.cp_samples),
        ));
        (lead, clean)
    }

    /// **Corrected behavior (see `docs/adr/004-strongest-path-timing.md`)**: at a
    /// delay spread on the scale this codebase's own Watterson HF channel models
    /// actually produce (96 samples = Watterson-Poor's modeled 2 ms delay spread
    /// on this 48 kHz profile), the detector must anchor on the STRONGER tap, not
    /// the earliest one. Task 5 originally required the opposite (see the removed
    /// `detector_locks_first_path_not_strongest` test this replaces) on the theory
    /// that first-path anchoring is always safer; measured directly (paired,
    /// same-seed Watterson-Moderate bench + a same-seed bit-error comparison) that
    /// anchoring on a weak-but-earliest tap at this delay scale corrupts the
    /// protected header's sparse-pilot (4-pilot `hf_standard`) 2D channel estimate
    /// badly enough to floor FER at ~30% regardless of SNR (level 1, BPSK 1/4), while
    /// anchoring on the strongest tap restores the pre-Phase-1 ~2-5% FER there. Levels
    /// 3-4 recover most but not all of the gap (see
    /// `.superpowers/sdd/p1-fading-regression-fix-report.md`). The two anchors coincide
    /// (no behavior change at all) whenever one tap dominates, which is why AWGN
    /// and the VHF (denser-pilot) levels were and remain unaffected.
    #[test]
    fn detector_prefers_strongest_path_at_realistic_hf_delay() {
        let profile = CoppaProfile::hf_standard();
        let (lead, clean) = leading_clean_frame(&profile);

        // Two-tap channel: a WEAKER direct path plus a STRONGER echo +96 samples
        // later (Watterson-Poor's modeled delay spread on this profile). The
        // detector must now lock onto the stronger echo, not the weaker direct
        // arrival.
        let delay = 96usize;
        let rx = two_tap(&clean, 0.6, 1.0, delay);

        let candidates = SyncDetector::detect_all(&profile, 1, &rx);
        assert_eq!(
            candidates.len(),
            1,
            "expected exactly one candidate, got {candidates:?}"
        );

        // Expected position: the ECHO's TRUE (post-TX-bandpass) arrival minus the
        // deliberate TIMING_BACKOFF — see `tx_bpf_group_delay`.
        let expected =
            lead as i64 + tx_bpf_group_delay(&profile) + delay as i64 - TIMING_BACKOFF as i64;
        let err = (candidates[0].frame_start as i64 - expected).abs();
        assert!(
            err <= 30,
            "frame_start {} should be within 30 of the STRONGER echo's arrival - backoff \
             ({expected}), not the weaker direct path; err={err}",
            candidates[0].frame_start
        );
    }

    /// The original first-path SAFETY property is still preserved for delay
    /// spreads well beyond anything this codebase's HF channel models produce:
    /// once the stronger tap is more than half the cyclic prefix away from the
    /// earliest one, anchoring on it would risk running the FFT window too close
    /// to (or past) the edge of the cyclic-prefix-protected, ISI-free region, so
    /// the detector falls back to the earliest (direct) arrival instead.
    #[test]
    fn detector_falls_back_to_first_path_beyond_half_cp() {
        let profile = CoppaProfile::hf_standard();
        let (lead, clean) = leading_clean_frame(&profile);

        // Delay well beyond cp/2 (150 samples on this profile) but still within
        // one cyclic prefix (300 samples), so the confirm/refine search window
        // (+/- cp around the coarse peak) still covers both taps.
        let delay = profile.cp_samples * 3 / 4;
        let rx = two_tap(&clean, 0.6, 1.0, delay);

        let candidates = SyncDetector::detect_all(&profile, 1, &rx);
        assert_eq!(
            candidates.len(),
            1,
            "expected exactly one candidate, got {candidates:?}"
        );

        let expected = lead as i64 + tx_bpf_group_delay(&profile) - TIMING_BACKOFF as i64;
        let err = (candidates[0].frame_start as i64 - expected).abs();
        assert!(
            err <= 30,
            "frame_start {} should be within 30 of the DIRECT path's arrival - backoff \
             ({expected}), not the far, stronger echo; err={err}",
            candidates[0].frame_start
        );
    }
}
