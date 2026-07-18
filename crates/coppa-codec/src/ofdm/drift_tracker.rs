//! Coarse-delay drift Kalman tracker.
//!
//! Targets the unresolved Phase 2 Task 1/7 regression
//! (`docs/adr/006-phase2-parametric-estimation-nr-bg2.md`, decisions 1 and
//! 3): `coppa_modem.rs`'s `coarse_delay` is fit once per frame from the
//! probe symbol and reused unchanged for every window, but a per-window
//! diagnostic showed the true bulk-delay reference drifting continuously
//! within a frame (`|Ĥ(24)|²` decaying 3.2→0.13, `noise_var` climbing
//! 44→360 across one frame — see `.superpowers/sdd/p2-task-1-report.md`).
//! Task 7's AR(1) tap-amplitude Kalman tracker (`kalman_tracker.rs`) did
//! not fix this — both task reports concluded the drift looks like a
//! continuously-accumulating reference error, not stationary Rayleigh
//! fading, which a mean-reverting AR(1) model actively fights instead of
//! tracking.
//!
//! # Model
//!
//! State `x = [τ, τ̇]` — coarse delay (same grid-unit basis as
//! `delay_domain::estimate_coarse_delay`/`tau_basis`) and its rate of
//! change. Transition is a discrete integrated random walk (nearly-
//! constant-velocity), **not** mean-reverting: `τ_t = τ_{t-1} + τ̇_{t-1}`,
//! `τ̇_t = τ̇_{t-1}`, plus process noise `(q_tau, q_dot)` on each state.
//! One step = one OFDM symbol (`Δt = 1`). This is deliberately different
//! from `kalman_tracker.rs`'s AR(1) model: a random walk has no
//! pull-back-to-zero term, so it can track a real sustained drift instead
//! of fighting it.
//!
//! # Robustness (why this differs from Task 1's rejected naive per-window
//! # re-derivation)
//!
//! Task 1's naive per-window hard override tracked the drift correctly but
//! had no robustness weighting, so individual noisy low-SNR windows
//! corrupted the LDPC-facing noise variance. This tracker's `advance`
//! weights each observation by its own `r` (Kalman gain naturally
//! down-weights a high-`r` observation) and additionally rejects any
//! observation whose innovation exceeds [`DRIFT_INNOVATION_CLAMP`]
//! regardless of its claimed `r` — a defensive guard mirroring
//! `coppa_modem.rs`'s `SCO_PER_SYMBOL_CLAMP` pattern for the same class of
//! problem.

use super::delay_domain::timing_offset_samples;
use num_complex::Complex32;

/// Reject any observation whose innovation (`z - predicted τ`) exceeds this
/// many grid units, regardless of its claimed `r` — see the module doc's
/// "Robustness" section. Starting point for the Task 5 gate sweep, not a
/// derived-from-first-principles constant.
const DRIFT_INNOVATION_CLAMP: f32 = 1.0;

/// Per-step posterior: `[τ, τ̇]` and their 2×2 covariance
/// `[[p_tt, p_td], [p_td, p_dd]]` (symmetric, stored as 3 floats).
#[derive(Clone, Copy)]
struct DriftState {
    tau: f32,
    tau_dot: f32,
    p_tt: f32,
    p_td: f32,
    p_dd: f32,
}

/// Stateful 2-state (delay, delay-rate) Kalman filter over a sequence of
/// per-symbol delay observations within one frame. See the module doc for
/// the full model.
pub struct DriftTracker {
    q_tau: f32,
    q_dot: f32,
    filt: Vec<DriftState>,
    init: DriftState,
}

impl DriftTracker {
    /// `tau0`: initial delay estimate (grid units), typically the frame's
    /// `CoppaModem::bounded_coarse_delay`. `q_tau`/`q_dot`: process-noise
    /// variances (per step) on delay and delay-rate respectively — tuning
    /// knobs, see the Task 5 gate sweep. Initial covariance is a fixed,
    /// moderately loose prior (`p_tt=0.1`, `p_dd=0.01`, `p_td=0`): we trust
    /// `tau0` reasonably well (it's already `bounded_coarse_delay`-derived)
    /// but have zero prior information on the rate.
    pub fn new(tau0: f32, q_tau: f32, q_dot: f32) -> Self {
        let init = DriftState {
            tau: tau0,
            tau_dot: 0.0,
            p_tt: 0.1,
            p_td: 0.0,
            p_dd: 0.01,
        };
        Self {
            q_tau,
            q_dot,
            filt: Vec::new(),
            init,
        }
    }

    /// Advance one step: predict, then update from `obs = Some((z, r))` if
    /// this step has a valid observation (grid-unit delay estimate `z`,
    /// observation-noise variance `r`), or predict-only if `None` (no
    /// usable pilots this symbol — see `observe_drift`).
    pub fn advance(&mut self, obs: Option<(f32, f32)>) {
        let prev = self.filt.last().copied().unwrap_or(self.init);
        // Predict: F = [[1,1],[0,1]] (Δt = one step).
        let tau_pred = prev.tau + prev.tau_dot;
        let tau_dot_pred = prev.tau_dot;
        let p_tt_pred = prev.p_tt + 2.0 * prev.p_td + prev.p_dd + self.q_tau;
        let p_td_pred = prev.p_td + prev.p_dd;
        let p_dd_pred = prev.p_dd + self.q_dot;

        let post = match obs {
            Some((z, r)) if (z - tau_pred).abs() <= DRIFT_INNOVATION_CLAMP => {
                // H = [1, 0] (observe τ only). S = H P Hᵀ + r = p_tt_pred + r.
                let s = p_tt_pred + r.max(1e-9);
                let k_tau = p_tt_pred / s;
                let k_dot = p_td_pred / s;
                let y = z - tau_pred;
                DriftState {
                    tau: tau_pred + k_tau * y,
                    tau_dot: tau_dot_pred + k_dot * y,
                    p_tt: (1.0 - k_tau) * p_tt_pred,
                    p_td: (1.0 - k_tau) * p_td_pred,
                    p_dd: p_dd_pred - k_dot * p_td_pred,
                }
            }
            _ => DriftState {
                tau: tau_pred,
                tau_dot: tau_dot_pred,
                p_tt: p_tt_pred,
                p_td: p_td_pred,
                p_dd: p_dd_pred,
            },
        };
        self.filt.push(post);
    }

    /// Filtered (causal) delay estimate at step `t` (grid units), 0-indexed
    /// in `advance` call order.
    pub fn tau(&self, t: usize) -> f32 {
        self.filt[t].tau
    }
}

/// Per-frame fixed noise-floor scaling for [`observe_drift`]'s returned `r`
/// — multiplies the frame's probe-derived `noise_var`
/// (`CoppaModem::probe_calibration`) before dividing by the pilot-pair
/// count, giving `r` roughly the same footing as `KalmanLagSmoother`'s
/// fixed-per-frame `sigma_v2` convention (`kalman_tracker.rs`). Starting
/// point for the Task 5 gate sweep.
const DRIFT_NOISE_SCALE: f32 = 1.0;

/// Derive one step's `(z, r)` observation for [`DriftTracker::advance`]
/// from a symbol's raw (non-derotated) pilot set: `z` is the absolute bulk
/// delay (grid units, [`super::delay_domain::tau_basis`]'s convention)
/// measured from this symbol's own pilot phase slope via
/// [`timing_offset_samples`]; `r` is a FIXED noise floor (the frame's
/// probe-derived `sigma_v2`) scaled down by the number of pilot pairs this
/// symbol contributed — more pilots, lower variance, WITHOUT re-deriving
/// the noise floor itself from this window's own (possibly noisy) data.
/// Deliberately reusing a fixed per-frame floor rather than a per-window
/// residual is the same lesson `KalmanLagSmoother::new`'s doc already
/// documents: re-deriving noise scale per window was exactly the mechanism
/// that corrupted Task 1's rejected per-window re-fit.
///
/// Returns `None` if `pilots` has too few entries for
/// [`timing_offset_samples`] to produce an estimate.
pub(crate) fn observe_drift(
    fft_size: usize,
    nc: usize,
    sigma_v2: f32,
    pilots: &[(usize, Complex32)],
) -> Option<(f32, f32)> {
    let tau_samples = timing_offset_samples(fft_size, pilots)?;
    let z = tau_samples * nc as f32 / fft_size as f32;
    let pairs = (pilots.len() as f32 - 1.0).max(1.0);
    let r = (DRIFT_NOISE_SCALE * sigma_v2.max(1e-6)) / pairs;
    Some((z, r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn converges_to_a_static_delay_from_noisy_observations() {
        let true_tau = 0.5f32;
        let mut rng = StdRng::seed_from_u64(1);
        let r = 0.05f32;
        let mut tracker = DriftTracker::new(0.0, 1e-6, 1e-6);
        for _ in 0..40 {
            let noise = (rng.random::<f32>() - 0.5) * 2.0 * r.sqrt();
            tracker.advance(Some((true_tau + noise, r)));
        }
        let tau = tracker.tau(39);
        assert!(
            (tau - true_tau).abs() < 0.1,
            "expected ~{true_tau}, got {tau}"
        );
    }

    #[test]
    fn tracks_a_linear_drift_better_than_a_frozen_estimate() {
        let true_tau_at = |t: usize| 0.1 + 0.01 * t as f32;
        let mut rng = StdRng::seed_from_u64(2);
        let r = 0.02f32;
        let mut tracker = DriftTracker::new(true_tau_at(0), 1e-4, 1e-5);
        let mut tracked_err = 0.0f32;
        let mut frozen_err = 0.0f32;
        let frozen = true_tau_at(0);
        for t in 0..30 {
            let noise = (rng.random::<f32>() - 0.5) * 2.0 * r.sqrt();
            tracker.advance(Some((true_tau_at(t) + noise, r)));
            tracked_err += (tracker.tau(t) - true_tau_at(t)).powi(2);
            frozen_err += (frozen - true_tau_at(t)).powi(2);
        }
        assert!(
            tracked_err < 0.3 * frozen_err,
            "tracker should follow the ramp far better than a frozen estimate: tracked={tracked_err} frozen={frozen_err}"
        );
    }

    #[test]
    fn a_high_noise_observation_is_down_weighted_not_trusted_fully() {
        let true_tau = 0.3f32;
        let mut tracker = DriftTracker::new(true_tau, 1e-6, 1e-6);
        for _ in 0..10 {
            tracker.advance(Some((true_tau, 0.01)));
        }
        let before = tracker.tau(9);
        // Correct-magnitude noise floor, but a badly wrong z -- the tracker
        // should barely move because r says "don't trust this much."
        tracker.advance(Some((true_tau + 0.5, 5.0)));
        let after = tracker.tau(10);
        assert!(
            (after - before).abs() < 0.05,
            "a high-r (low-confidence) observation should barely move the state: before={before} after={after}"
        );
    }

    #[test]
    fn an_outlier_observation_is_rejected_by_the_innovation_clamp() {
        let true_tau = 0.2f32;
        let mut tracker = DriftTracker::new(true_tau, 1e-6, 1e-6);
        for _ in 0..10 {
            tracker.advance(Some((true_tau, 0.01)));
        }
        let before = tracker.tau(9);
        // Wildly wrong AND claims to be confident (small r) -- the clamp
        // must reject it regardless of claimed r. Trusting a
        // claimed-confident but implausible jump is exactly the failure
        // mode Task 1's naive per-window re-derivation had.
        tracker.advance(Some((true_tau + 50.0, 0.001)));
        let after = tracker.tau(10);
        assert!(
            (after - before).abs() < 0.05,
            "an implausible jump must be rejected by the innovation clamp regardless of claimed r: before={before} after={after}"
        );
    }

    #[test]
    fn observe_drift_recovers_a_known_bulk_delay_in_grid_units() {
        let fft_size = 960;
        let nc = 48;
        let true_tau_samples = 6.0f32; // real ADC samples
        let pilots: Vec<(usize, Complex32)> = [0usize, 12, 24, 36]
            .iter()
            .map(|&k| {
                let ang = -std::f32::consts::TAU * (k as f32) * true_tau_samples / fft_size as f32;
                (k, Complex32::new(ang.cos(), ang.sin()))
            })
            .collect();
        let (z, r) = observe_drift(fft_size, nc, 0.1, &pilots).expect("should estimate");
        let expected_grid_units = true_tau_samples * nc as f32 / fft_size as f32; // 0.3
        assert!(
            (z - expected_grid_units).abs() < 0.01,
            "expected ~{expected_grid_units}, got {z}"
        );
        assert!(r > 0.0 && r.is_finite());
    }

    #[test]
    fn observe_drift_returns_none_with_too_few_pilots() {
        assert!(observe_drift(960, 48, 0.1, &[(0, Complex32::new(1.0, 0.0))]).is_none());
    }

    #[test]
    fn observe_drift_gives_denser_pilot_sets_lower_r() {
        let fft_size = 960;
        let nc = 48;
        let tau = 2.0f32;
        let h_at = |k: usize| {
            let ang = -std::f32::consts::TAU * (k as f32) * tau / fft_size as f32;
            Complex32::new(ang.cos(), ang.sin())
        };
        let sparse: Vec<(usize, Complex32)> = [0usize, 12].iter().map(|&k| (k, h_at(k))).collect();
        let dense: Vec<(usize, Complex32)> =
            [0usize, 12, 24, 36].iter().map(|&k| (k, h_at(k))).collect();
        let (_, r_sparse) = observe_drift(fft_size, nc, 0.1, &sparse).expect("sparse estimate");
        let (_, r_dense) = observe_drift(fft_size, nc, 0.1, &dense).expect("dense estimate");
        assert!(
            r_dense < r_sparse,
            "more pilot pairs should give a lower (more confident) r: dense={r_dense} sparse={r_sparse}"
        );
    }
}
