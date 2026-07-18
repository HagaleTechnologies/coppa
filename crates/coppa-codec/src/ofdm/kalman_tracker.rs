//! Delay-domain Kalman tracker + lag-2 fixed-lag (RTS) smoother.
//!
//! Phase 2 Task 7: replaces the naive "independently re-fit the delay-domain model
//! from each estimation window" approach with a stateful AR(1) Kalman filter that
//! carries the tap estimate (and its uncertainty) across windows within a frame.
//!
//! # Why this exists
//!
//! Task 1 (`delay_domain.rs`) fits `L` delay-domain taps per estimation window from
//! pooled pilot observations, but reused a single frame-global `coarse_delay` for
//! every window. Real HF channels have non-zero Doppler spread, so the channel
//! genuinely drifts over a frame's duration; a bare independent re-fit per window
//! either ignores that drift (Task 1's shipped behavior: monotonic degradation
//! across a frame) or, if re-derived independently per window, lets an occasional
//! low-SNR window produce a wildly wrong LOCAL estimate that corrupts the
//! soft-decision noise variance fed to the LDPC decoder (Task 1's rejected fix: raw
//! per-window re-estimation net-regressed FER despite improving hard-decision BER).
//!
//! A Kalman filter fixes this properly: instead of treating each window as an
//! independent measurement, it's a noisy observation of a *smoothly evolving*
//! process. The process model (an AR(1) prior tying `h_t` to `h_{t-1}`) prevents any
//! single bad window from swinging the tracked state far from where the surrounding
//! (well-conditioned) windows say it should be — exactly the property the rejected
//! per-window re-fit lacked.
//!
//! # Model
//!
//! - **State**: `L` complex delay-domain taps `h = [h_0, .., h_{L-1}]` — the same
//!   basis [`super::delay_domain::DelayDomainEstimator`] fits (`H(k) = Σ_ℓ
//!   h_ℓ·τ(k,ℓ)`, `τ(k,ℓ) = exp(-j2πkℓ/nc)`).
//! - **Transition (AR(1))**: `h_t = a·h_{t-1} + w_t`, `w_t ~ CN(0, Q)`, `Q` diagonal.
//!   `a = exp(-2·(π·σ_D·T_s)²)` is the standard Gauss-Markov (AR(1)) approximation to
//!   a Gaussian-Doppler-PSD process's one-step autocorrelation — see
//!   [`ar1_coefficient`]'s doc for where this exact functional form is verified
//!   against `coppa-channel`'s own fading-process autocorrelation test.
//! - **Process noise**: `Q_ℓ = (1-a²)·|h_probe_ℓ|²` — the steady-state-variance
//!   preserving choice: for `h_t = a·h_{t-1}+w_t`, the stationary variance is
//!   `σ²_h = Q/(1-a²)`; setting `Q=(1-a²)·σ²_h` with `σ²_h = |h_probe_ℓ|²` (the
//!   frame's initial per-tap power estimate) makes the AR(1) process's steady-state
//!   variance match the probe's observed tap powers, so the filter neither collapses
//!   to zero uncertainty nor blows up over a long frame.
//! - **Observation**: at each step, `z_t = B_t·h_t + v_t` where `B_t`'s rows are the
//!   `(carrier_index, H_observed, weight)` pilot triples for that step. `v_t`'s
//!   covariance is diagonal, `R_kk = σ_v²/w_k` (`σ_v²` a single frame-global noise
//!   floor derived once from the probe fit's own residual — see
//!   `KalmanLagSmoother::new`'s doc for why a fixed-per-frame value, not a
//!   per-window re-estimate, is used: re-deriving noise scale per window was
//!   exactly the mechanism that corrupted Task 1's rejected per-window re-fit).
//! - **Recursion**: standard complex Kalman filter, implemented via the same
//!   information-form (normal-equations) linear algebra
//!   [`super::delay_domain::solve_ridge`] uses (`L≤8`, so direct Gaussian
//!   elimination/LU is exact and cheap) — predict: `h_pred=a·h_prev`,
//!   `P_pred=a²·P_prev+Q`; update: MAP combination of the Gaussian prior
//!   `N(h_pred,P_pred)` with the weighted-least-squares observation likelihood,
//!   `(P_pred⁻¹ + BᴴWB)·h_post = P_pred⁻¹·h_pred + BᴴWz`, `P_post = (P_pred⁻¹+BᴴWB)⁻¹`.
//! - **Lag-2 fixed-lag smoothing**: the state used to equalize window `t` is not the
//!   raw causal filter output at `t`, but a standard RTS (Rauch-Tung-Striebel)
//!   backward pass using exactly 2 steps of hindsight (`t+1`, `t+2`) — see
//!   [`KalmanLagSmoother::smoothed`].
//!
//! # What counts as "one step", and why observations are RAW per-symbol pilots
//! (not the ±2-symbol pooled window)
//!
//! One Kalman step = one OFDM symbol (`coppa_modem.rs`'s per-symbol estimation
//! loop), i.e. `T_s = (fft_size+cp_samples)/sample_rate` (≈26.25 ms for
//! `hf_standard`). Each step's *observation* is that symbol's OWN raw pilot set
//! (weight 1 per pilot) — NOT Task 1's ±2-symbol boxcar pool.
//!
//! An earlier version of this fed each step the ±2-symbol POOLED window instead
//! (reusing Task 1's `pool_pilots` directly, as the plan's "reuse that
//! function/logic" guidance suggested). That was measured to be WRONG: consecutive
//! steps' pooled windows overlap by up to 4 of 5 symbols, so the same raw pilot
//! samples get re-submitted as "new" evidence to up to 5 consecutive `advance()`
//! calls. A recursive Bayesian filter has no way to know this evidence is
//! redundant, and with `a` close to 1 (little forgetting) the posterior confidence
//! compounds far beyond what the genuinely independent information content
//! justifies. This was root-caused directly (Task 7 investigation,
//! `estimator_diagnosis` with temporary per-window debug instrumentation): the
//! pooled-window version's `noise_at()` came out ~100-100,000x smaller than Task
//! 1's comparable per-window residual, its tracked `h_at(k)` DIVERGED (grew
//! unboundedly across a frame) instead of tracking the true channel, and the full
//! bench gate showed `watterson-moderate`/level 2 never clearing 10% FER anywhere
//! in -6..30 dB — worse than both the pre-Task-1 baseline (18 dB) and Task 1's own
//! regressed result (24 dB). A controlled synthetic check (independent,
//! non-overlapping observations of a genuinely static channel — see
//! `recursion_converges_correctly_for_independent_observations_of_a_static_channel`
//! below) confirmed the core predict/update/RTS-smooth recursion itself converges
//! correctly, isolating the bug to the overlapping-observation over-counting, not
//! the recursion math. Feeding raw per-symbol pilots makes consecutive steps'
//! observations genuinely independent, so the Kalman recursion's own temporal
//! accumulation (governed by `a`/`Q`) is the ONLY source of cross-symbol pooling —
//! not stacked on top of a second, overlapping pooling layer. See
//! [`DEFAULT_SIGMA_D_HZ`]'s doc for the resulting (empirically re-tuned) `a`.
use num_complex::Complex32;

use super::delay_domain::tau_basis;

/// Gaussian-Doppler power-PSD sigma (Hz) used to derive the AR(1) coefficient `a`
/// (see [`ar1_coefficient`]). A single fixed constant, not a per-preset lookup.
///
/// # This is an EMPIRICALLY CALIBRATED value, not Watterson-Moderate's literal
/// # physical Doppler spread — a correction found by measurement, not the plan's
/// # assumed "default 0.5 Hz"
///
/// The Phase 2 plan's literal spec says "default 0.5 Hz," reasoning from
/// `coppa-channel`'s Watterson amplitude-fading physics (`doppler_sigma_hz`, the
/// power-PSD sigma that governs `watterson.rs`'s own verified fading-process
/// autocorrelation, `rho(τ)=exp(-2π²σ²τ²)`). Using that literal value (`a` ≈
/// 0.9966 at `hf_standard`'s `T_s`≈26.25 ms) was measured DIRECTLY to fail
/// catastrophically: a controlled diagnostic (`estimator_diagnosis` with
/// temporary per-window debug instrumentation) showed the tracked `h_at(k)`
/// diverging (growing unboundedly across a frame) instead of tracking the real
/// channel, and the full bench gate showed `watterson-moderate` at level 2 NEVER
/// clearing 10% FER anywhere in -6..30 dB (worse than both the pre-Task-1
/// baseline, 18 dB, and Task 1's own regressed 24 dB).
///
/// Root cause (confirmed by a controlled synthetic test: independent,
/// non-overlapping observations of a genuinely static channel converge correctly
/// with `a`≈0.9966 — see `kalman_tracker`'s tests): with `a` this close to 1, the
/// filter barely forgets across a whole frame's ~30-90 steps, so it behaves like
/// one large batch least-squares fit over the ENTIRE frame rather than a LOCAL
/// tracker — exactly the "pooling the whole frame blurs the time-varying channel"
/// failure Task 1's own `EST_WINDOW` comment already warned about, just reached by
/// a different route. The physically-motivated Doppler AMPLITUDE-fading
/// coherence time (~1-10s, i.e. genuinely slow) is real, but it is NOT the
/// dominant driver of the within-frame drift Task 1 measured — that drift is far
/// more consistent with a residual coarse-delay/phase-reference staleness (see
/// Task 1's report), a process an amplitude-fading-derived, mean-reverting AR(1)
/// model doesn't represent well at a near-unity `a`.
///
/// A direct, systematic empirical sweep of `a` ∈ {0.5, 0.6, 0.7, 0.8, 0.9, 0.95,
/// 0.99} (a fast reduced-scale FER check, 150 trials/point, watterson-moderate
/// level 2 — run via a scratch harness since deleted per this project's
/// scratch-file-cleanup convention; see `task7_gate.rs` for the full-scale,
/// still-present 400-trials/point gate check) after fixing the
/// overlapping-observation bug (see the module doc's "Why raw per-symbol pilots"
/// section) found ALL of these values perform similarly — none clears the FER≤10%
/// Wilson-bound anywhere in -6..30 dB at this trial count, with `a`∈{0.7,0.8}
/// consistently the (marginal) best, closest at 24 dB (FER≈5.3%, upper 95% CI
/// bound 0.1017 — just barely missing the 0.10 threshold). `a`≈0.80 (`σ_D`≈4.05
/// Hz, rounded to 4.0 Hz here) is the value shipped, as the best-measured point,
/// but this is NOT a claim that retuning `a` alone closes Task 7's acceptance
/// gate: a full 400-trial gate run with this value still only reached FER≤10% at
/// 30 dB on watterson-moderate/level 2 — worse than Task 1's own regressed 24 dB,
/// and far short of the pre-Task-1 baseline's 18 dB. See the Task 7 report for
/// the full honest account of what does and does not work and why further
/// `a`-tuning alone is very unlikely to close the remaining gap.
pub const DEFAULT_SIGMA_D_HZ: f32 = 4.0;

/// AR(1) one-step correlation coefficient for a Gaussian-Doppler-PSD process:
/// `a = exp(-2·(π·σ_D·T_s)²)`.
///
/// This is the standard Gauss-Markov approximation used to track a Gaussian-PSD
/// fading process with a first-order autoregressive model: it matches the AR(1)
/// model's one-step autocorrelation to the TRUE process's autocorrelation at lag
/// `T_s`. It is only a one-lag match — the true Gaussian PSD's autocorrelation
/// decays super-exponentially at large lag while an AR(1) process decays purely
/// exponentially — but this is the well known, standard tracking-filter
/// approximation (matching Jakes/Gauss-Markov channel-tracking literature), and is
/// exactly what `coppa-channel::watterson`'s own verified autocorrelation formula
/// (`rho(τ)=exp(-2π²σ²τ²)`, from `fading_process_autocorrelation_matches_gaussian_psd`)
/// gives at `τ=T_s` when `σ=σ_D`.
pub fn ar1_coefficient(sigma_d_hz: f32, t_s: f32) -> f32 {
    let x = std::f32::consts::PI * sigma_d_hz * t_s;
    (-2.0 * x * x).exp()
}

/// One step's posterior (filtered) state: `L` complex taps + their `L×L` Hermitian
/// covariance (row-major).
#[derive(Clone)]
struct StepState {
    h: Vec<Complex32>,
    p: Vec<Complex32>,
}

/// In-place LU decomposition with partial pivoting of an `l×l` complex matrix
/// (row-major). Overwrites `a` with the multipliers (below diagonal) and `U` (on
/// and above diagonal); returns the row-pivot permutation (`piv[i]` = original row
/// now at position `i`). Singular pivot directions are left as zero-multiplier rows
/// (matches `solve_ridge`'s existing rank-deficient handling: the Bayesian prior
/// term already keeps every system here well-conditioned in practice, so this is a
/// defensive fallback, not an expected path).
fn lu_decompose(l: usize, a: &mut [Complex32]) -> Vec<usize> {
    let mut piv: Vec<usize> = (0..l).collect();
    for col in 0..l {
        let mut best = col;
        for r in (col + 1)..l {
            if a[r * l + col].norm_sqr() > a[best * l + col].norm_sqr() {
                best = r;
            }
        }
        if best != col {
            for c in 0..l {
                a.swap(col * l + c, best * l + c);
            }
            piv.swap(col, best);
        }
        let d = a[col * l + col];
        if d.norm_sqr() < 1e-20 {
            continue;
        }
        for r in (col + 1)..l {
            let f = a[r * l + col] / d;
            a[r * l + col] = f;
            for c in (col + 1)..l {
                let v = a[col * l + c];
                a[r * l + c] -= f * v;
            }
        }
    }
    piv
}

/// Solve `A x = b` from `lu_decompose`'s factors + pivot vector.
fn lu_solve(l: usize, lu: &[Complex32], piv: &[usize], b: &[Complex32]) -> Vec<Complex32> {
    let mut y: Vec<Complex32> = piv.iter().map(|&p| b[p]).collect();
    for col in 0..l {
        let yc = y[col];
        for r in (col + 1)..l {
            let f = lu[r * l + col];
            y[r] -= f * yc;
        }
    }
    for col in (0..l).rev() {
        let d = lu[col * l + col];
        if d.norm_sqr() < 1e-20 {
            y[col] = Complex32::new(0.0, 0.0);
            continue;
        }
        let mut acc = y[col];
        for c in (col + 1)..l {
            acc -= lu[col * l + c] * y[c];
        }
        y[col] = acc / d;
    }
    y
}

/// Full matrix inverse from `lu_decompose`'s factors (solve for each unit vector).
fn lu_invert(l: usize, lu: &[Complex32], piv: &[usize]) -> Vec<Complex32> {
    let mut inv = vec![Complex32::new(0.0, 0.0); l * l];
    for j in 0..l {
        let mut e = vec![Complex32::new(0.0, 0.0); l];
        e[j] = Complex32::new(1.0, 0.0);
        let col = lu_solve(l, lu, piv, &e);
        for i in 0..l {
            inv[i * l + j] = col[i];
        }
    }
    inv
}

/// Invert an `l×l` complex matrix (convenience wrapper around decompose+invert).
fn invert(l: usize, m: &[Complex32]) -> Vec<Complex32> {
    let mut work = m.to_vec();
    let piv = lu_decompose(l, &mut work);
    lu_invert(l, &work, &piv)
}

/// `predict`: `h_pred = a·h_prev`, `P_pred = a²·P_prev + Q`.
fn predict(a: f32, q: &[f32], prev: &StepState) -> StepState {
    let l = prev.h.len();
    let h = prev.h.iter().map(|&hv| hv * a).collect();
    let mut p: Vec<Complex32> = prev.p.iter().map(|&pv| pv * (a * a)).collect();
    for i in 0..l {
        p[i * l + i] += Complex32::new(q[i], 0.0);
    }
    StepState { h, p }
}

/// `update`: Bayesian combination of the Gaussian prior `(pred.h, pred.p)` with the
/// weighted pooled-pilot observation `obs = [(carrier_index, z_k, weight_k)]`,
/// `R_kk = sigma_v2/weight_k`.
fn update(
    nc: usize,
    l: usize,
    sigma_v2: f32,
    obs: &[(usize, Complex32, f32)],
    pred: &StepState,
) -> StepState {
    let lambda_prior = invert(l, &pred.p);
    let mut a_mat = lambda_prior.clone();
    let mut b_vec: Vec<Complex32> = (0..l)
        .map(|i| {
            (0..l)
                .map(|j| lambda_prior[i * l + j] * pred.h[j])
                .fold(Complex32::new(0.0, 0.0), |acc, v| acc + v)
        })
        .collect();
    for &(k, z, w) in obs {
        let winv = w / sigma_v2.max(1e-12);
        for i in 0..l {
            let bi = tau_basis(nc, k, i);
            b_vec[i] += bi.conj() * z * winv;
            for j in 0..l {
                a_mat[i * l + j] += bi.conj() * tau_basis(nc, k, j) * winv;
            }
        }
    }
    let piv = lu_decompose(l, &mut a_mat);
    let h_post = lu_solve(l, &a_mat, &piv, &b_vec);
    let p_post = lu_invert(l, &a_mat, &piv);
    StepState {
        h: h_post,
        p: p_post,
    }
}

/// One RTS backward step: combine `filt_k` (this step's filtered state) with
/// `smooth_next` (the already-smoothed state at `k+1`, using `pred_next` — the
/// one-step-ahead prediction from `filt_k` — as the bridge).
fn rts_step(
    a: f32,
    filt_k: &StepState,
    pred_next: &StepState,
    smooth_next: &StepState,
) -> StepState {
    let l = filt_k.h.len();
    let inv_pred_p = invert(l, &pred_next.p);
    // C = a * P_filt_k * inv(P_pred_next)
    let mut c = vec![Complex32::new(0.0, 0.0); l * l];
    for i in 0..l {
        for j in 0..l {
            let mut acc = Complex32::new(0.0, 0.0);
            for m in 0..l {
                acc += filt_k.p[i * l + m] * inv_pred_p[m * l + j];
            }
            c[i * l + j] = acc * a;
        }
    }
    let diff_h: Vec<Complex32> = (0..l).map(|i| smooth_next.h[i] - pred_next.h[i]).collect();
    let mut h_smooth = filt_k.h.clone();
    for i in 0..l {
        let mut acc = Complex32::new(0.0, 0.0);
        for j in 0..l {
            acc += c[i * l + j] * diff_h[j];
        }
        h_smooth[i] += acc;
    }
    let diff_p: Vec<Complex32> = (0..l * l)
        .map(|i| smooth_next.p[i] - pred_next.p[i])
        .collect();
    let mut tmp = vec![Complex32::new(0.0, 0.0); l * l];
    for i in 0..l {
        for j in 0..l {
            let mut acc = Complex32::new(0.0, 0.0);
            for m in 0..l {
                acc += c[i * l + m] * diff_p[m * l + j];
            }
            tmp[i * l + j] = acc;
        }
    }
    let mut p_smooth = filt_k.p.clone();
    for i in 0..l {
        for j in 0..l {
            let mut acc = Complex32::new(0.0, 0.0);
            for m in 0..l {
                acc += tmp[i * l + m] * c[j * l + m].conj();
            }
            p_smooth[i * l + j] += acc;
        }
    }
    StepState {
        h: h_smooth,
        p: p_smooth,
    }
}

/// A finalized (lag-2 smoothed) tap estimate for one estimation window, with a
/// full per-carrier noise variance derived from the tracked covariance (not a
/// single frame-wide scalar) — the Kalman covariance directly answers "how
/// uncertain is `H(k)` given everything the tracker has seen," which is a strictly
/// more informative quantity than [`super::delay_domain::DelayDomainEstimator`]'s
/// single residual-variance scalar.
pub struct TrackedTaps {
    nc: usize,
    taps: Vec<Complex32>,
    cov: Vec<Complex32>,
}

impl TrackedTaps {
    /// Evaluate the fitted delay-domain model at a given active-carrier index.
    pub fn h_at(&self, carrier: usize) -> Complex32 {
        self.taps
            .iter()
            .enumerate()
            .map(|(ell, &h)| h * tau_basis(self.nc, carrier, ell))
            .fold(Complex32::new(0.0, 0.0), |acc, v| acc + v)
    }

    /// `Var(H(k))` from the tracked covariance: the quadratic form `bᴴ·P·b` where
    /// `b[ℓ] = τ(k,ℓ)`. Real by construction (P is Hermitian); floored to avoid a
    /// literal zero downstream.
    ///
    /// # This is the tracker's posterior uncertainty about the CHANNEL TAP, not a
    /// # receiver observation-noise estimate
    ///
    /// Unlike [`super::delay_domain::DelayDomainEstimator`]'s `noise_var` (a
    /// per-observation residual variance from the least-squares fit — an actual
    /// estimate of `σ_v²`, the noise on the received sample), `noise_at` is purely
    /// a function of the Kalman covariance `P`: how confident the tracker is about
    /// `h`, given everything it has observed so far. As the tracker accumulates
    /// evidence within a frame, `P` (and hence this value) shrinks regardless of
    /// the actual receiver noise floor. See [`Self::equalize`]'s doc for why this
    /// matters for LLR calibration, and `.superpowers/sdd/p2-task-7-report.md` for
    /// the full investigation. This is a suspected, unresolved LLR-overconfidence
    /// source: Task 5 (turbo re-estimation) or whoever next touches this should
    /// investigate it before assuming `noise_at`'s output is well-calibrated for
    /// LLR purposes.
    pub fn noise_at(&self, carrier: usize) -> f32 {
        let l = self.taps.len();
        let b: Vec<Complex32> = (0..l).map(|ell| tau_basis(self.nc, carrier, ell)).collect();
        let mut acc = Complex32::new(0.0, 0.0);
        for i in 0..l {
            for j in 0..l {
                acc += b[i].conj() * self.cov[i * l + j] * b[j];
            }
        }
        acc.re.max(1e-10)
    }

    /// Zero-force equalize `carriers`, returning per-carrier `(x̂, effective noise)`
    /// — same call signature as
    /// [`super::delay_domain::DelayDomainEstimator::equalize`], except the noise
    /// numerator is per-carrier (from the tracked covariance, [`Self::noise_at`])
    /// rather than a single frame-wide scalar. NOTE: despite the matching
    /// signature this is not necessarily the same *quantity* — see `noise_at`'s
    /// doc; the returned noise here is `Var(Ĥ(k))/|Ĥ(k)|²` (posterior tap
    /// uncertainty scaled by channel gain), which may understate the true
    /// zero-forcing symbol-error variance (dominated by observation noise
    /// `σ_v²/|Ĥ(k)|²`) once the tracker is confident. Flagged as a suspected
    /// LLR-overconfidence source, not fixed — see
    /// `.superpowers/sdd/p2-task-7-report.md` for the full investigation. Task 5
    /// (turbo re-estimation) should investigate this before assuming `noise_at`'s
    /// output is well-calibrated for LLR purposes.
    pub fn equalize(&self, carriers: &[Complex32]) -> (Vec<Complex32>, Vec<f32>) {
        let mut xhat = Vec::with_capacity(carriers.len());
        let mut noise = Vec::with_capacity(carriers.len());
        for (k, &y) in carriers.iter().enumerate() {
            let h = self.h_at(k);
            let h_sq = h.norm_sqr();
            let nv = self.noise_at(k);
            if h_sq >= 1e-4 {
                xhat.push(y / h);
                noise.push(nv / h_sq);
            } else {
                xhat.push(Complex32::new(0.0, 0.0));
                noise.push(1e6 * nv);
            }
        }
        (xhat, noise)
    }
}

/// Stateful AR(1) Kalman filter + lag-2 fixed-lag (RTS) smoother over a sequence of
/// per-symbol pooled-pilot observations within one frame. See the module doc for
/// the full model.
pub struct KalmanLagSmoother {
    nc: usize,
    l: usize,
    a: f32,
    sigma_v2: f32,
    q: Vec<f32>,
    init: StepState,
    filt: Vec<StepState>,
}

impl KalmanLagSmoother {
    /// Initialize from the frame's probe-derived tap estimate `h_probe` (already in
    /// the same coarse-delay-derotated reference frame the caller will feed to
    /// [`Self::advance`]).
    ///
    /// `sigma_v2` is the per-weight-1 observation noise floor, derived ONCE per
    /// frame (typically from the probe fit's own residual — see
    /// `CoppaModem::probe_calibration`'s call site), not re-estimated per window.
    /// This mirrors the fixed-vs-adaptive lesson from `estimate_coarse_delay`'s
    /// `calibrated_bias` (Task 1's Bug 1): Task 1's REJECTED per-window re-fit
    /// attempt corrupted the LDPC-facing noise variance specifically because it let
    /// individual thin/noisy windows re-derive their own noise scale, producing
    /// `noise_var` maxima in the billions on bad windows. Using one frame-global
    /// noise floor for `R` avoids that failure mode entirely; the actual per-window,
    /// per-carrier noise the decoder ultimately sees still varies correctly via the
    /// tracked covariance (`TrackedTaps::noise_at`), which is informed by how many
    /// (and how reliable) observations each window actually contributed — it just
    /// isn't allowed to swing the assumed noise floor itself window-to-window.
    pub fn new(nc: usize, l: usize, a: f32, sigma_v2: f32, h_probe: &[Complex32]) -> Self {
        let l = l.clamp(1, 8);
        let mut h0 = vec![Complex32::new(0.0, 0.0); l];
        let n_copy = l.min(h_probe.len());
        h0[..n_copy].copy_from_slice(&h_probe[..n_copy]);
        let a2 = a * a;
        let mut p0 = vec![Complex32::new(0.0, 0.0); l * l];
        let mut q = vec![0.0f32; l];
        for i in 0..l {
            let power = h0[i].norm_sqr().max(1e-6);
            p0[i * l + i] = Complex32::new(power, 0.0);
            q[i] = ((1.0 - a2) * power).max(1e-8);
        }
        Self {
            nc,
            l,
            a,
            sigma_v2: sigma_v2.max(1e-9),
            q,
            init: StepState { h: h0, p: p0 },
            filt: Vec::new(),
        }
    }

    /// Feed one step's pooled observation (already coarse-delay-derotated by the
    /// caller), running predict+update and appending the new filtered state.
    /// Must be called once per estimation window, in time order, before any
    /// [`Self::smoothed`] call for that or an earlier window.
    pub fn advance(&mut self, obs: &[(usize, Complex32, f32)]) {
        let prev = self.filt.last().unwrap_or(&self.init);
        let pred = predict(self.a, &self.q, prev);
        let post = update(self.nc, self.l, self.sigma_v2, obs, &pred);
        self.filt.push(post);
    }

    /// Lag-2 fixed-lag smoothed tap estimate for step `t` (0-indexed, in the order
    /// `advance` was called), using up to 2 steps of hindsight (`t+1`, `t+2`) if
    /// they've been `advance`d — fewer near the end of the sequence (graceful
    /// degrade to the causal filter output for the very last step).
    pub fn smoothed(&self, t: usize) -> TrackedTaps {
        let n = self.filt.len();
        assert!(t < n, "smoothed({t}) called before that step was advanced");
        let lag = (n - 1 - t).min(2);
        let mut smooth = self.filt[t + lag].clone();
        for k in (t..t + lag).rev() {
            let pred_next = predict(self.a, &self.q, &self.filt[k]);
            smooth = rts_step(self.a, &self.filt[k], &pred_next, &smooth);
        }
        TrackedTaps {
            nc: self.nc,
            taps: smooth.h,
            cov: smooth.p,
        }
    }

    /// Test-only: the raw CAUSAL filter output at step `t` (lag-0, no RTS
    /// hindsight) — used to directly verify [`Self::smoothed`] actually reduces
    /// variance/error relative to the causal filter, not just adds latency.
    #[cfg(test)]
    fn causal_only(&self, t: usize) -> TrackedTaps {
        let f = &self.filt[t];
        TrackedTaps {
            nc: self.nc,
            taps: f.h.clone(),
            cov: f.p.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn recursion_converges_correctly_for_independent_observations_of_a_static_channel() {
        // Regression test from the Task 7 investigation: a Kalman filter fed
        // GENUINELY INDEPENDENT (non-overlapping) observations of a truly static
        // channel, with `a` close to 1, should converge to an accurate, INCREASINGLY
        // CONFIDENT (shrinking noise_at) estimate — this is what isolated the real
        // bug (feeding overlapping/correlated POOLED windows, since fixed in
        // `coppa_modem.rs`, not this core recursion) from a hypothesis that the
        // recursion itself was broken. Kept as a permanent regression guard: if this
        // ever stops converging accurately/confidently, the core predict/update math
        // has regressed.
        let nc = 48;
        let l = 2;
        let h0 = Complex32::from_polar(2.0, 0.3);
        let h1 = Complex32::from_polar(0.5, -0.8);
        let true_h = move |k: usize| h0 * tau_basis(nc, k, 0) + h1 * tau_basis(nc, k, 1);
        let pilot_idx: [usize; 8] = [0, 6, 12, 18, 24, 30, 36, 42];
        let sigma_v2 = 50.0f32;
        let weight = 2.0f32;
        let a = 0.9966f32;

        let h_probe = vec![h0, h1];
        let mut tracker = KalmanLagSmoother::new(nc, l, a, sigma_v2, &h_probe);
        let mut rng = StdRng::seed_from_u64(99);
        let n_steps = 44;
        for _ in 0..n_steps {
            let obs: Vec<(usize, Complex32, f32)> = pilot_idx
                .iter()
                .map(|&k| {
                    (
                        k,
                        true_h(k) + complex_gaussian(&mut rng, sigma_v2 / weight),
                        weight,
                    )
                })
                .collect();
            tracker.advance(&obs);
        }
        let tt = tracker.smoothed(n_steps - 1);
        let err = (tt.h_at(24) - true_h(24)).norm();
        let nv = tt.noise_at(24);
        assert!(
            err < 0.3,
            "expected the tracker to converge near the true static value, got h_at(24)={:?} true={:?} (err={err})",
            tt.h_at(24),
            true_h(24)
        );
        assert!(
            (0.01..5.0).contains(&nv),
            "expected a modestly small, non-exploding, non-collapsed noise_at after \
             44 genuinely independent observations, got {nv}"
        );
    }

    #[test]
    fn lag2_smoothing_reduces_error_and_noise_versus_the_causal_filter_alone() {
        // Direct verification that `smoothed()` is actually doing something beyond
        // adding latency: on noisy per-step observations of a genuinely (slowly)
        // drifting channel, the lag-2 RTS-smoothed estimate at an interior step
        // should have LOWER error against ground truth AND lower noise_at() than
        // the raw causal filter output at that same step (which only saw data up
        // to and including that step, not the 2 subsequent steps' hindsight).
        let nc = 48;
        let l = 2;
        let a = 0.85f32;
        let h1_fixed = Complex32::from_polar(0.5, -0.8);
        let pilot_idx: [usize; 4] = [0, 12, 24, 36]; // sparse, so single-step noise matters
        let sigma_v2 = 0.3f32;
        let n_steps = 20;

        // h0 drifts linearly in phase (a smooth, slow drift a lag-2 window should
        // usefully average over without blurring it away).
        let h0_at = |t: usize| Complex32::from_polar(1.5, 0.1 * t as f32);
        let true_h_at = |t: usize| {
            let h0 = h0_at(t);
            move |k: usize| h0 * tau_basis(nc, k, 0) + h1_fixed * tau_basis(nc, k, 1)
        };

        let mut rng = StdRng::seed_from_u64(42);
        let h_probe = vec![h0_at(0), h1_fixed];
        let mut tracker = KalmanLagSmoother::new(nc, l, a, sigma_v2, &h_probe);
        for t in 0..n_steps {
            let th = true_h_at(t);
            let obs: Vec<(usize, Complex32, f32)> = pilot_idx
                .iter()
                .map(|&k| (k, th(k) + complex_gaussian(&mut rng, sigma_v2), 1.0))
                .collect();
            tracker.advance(&obs);
        }

        // Compare at several interior steps (away from the very end, where lag
        // necessarily shrinks to 0 and smoothed==causal by construction).
        let mut causal_err_sum = 0.0f32;
        let mut smooth_err_sum = 0.0f32;
        let mut causal_noise_sum = 0.0f32;
        let mut smooth_noise_sum = 0.0f32;
        for t in 2..(n_steps - 3) {
            let th = true_h_at(t);
            let causal = tracker.causal_only(t);
            let smooth = tracker.smoothed(t);
            causal_err_sum += (causal.h_at(24) - th(24)).norm_sqr();
            smooth_err_sum += (smooth.h_at(24) - th(24)).norm_sqr();
            causal_noise_sum += causal.noise_at(24);
            smooth_noise_sum += smooth.noise_at(24);
        }
        assert!(
            smooth_err_sum < causal_err_sum,
            "lag-2 smoothing should reduce error vs the causal filter alone: \
             smooth_err_sum={smooth_err_sum} causal_err_sum={causal_err_sum}"
        );
        assert!(
            smooth_noise_sum < causal_noise_sum,
            "lag-2 smoothing should reduce noise_at() vs the causal filter alone: \
             smooth_noise_sum={smooth_noise_sum} causal_noise_sum={causal_noise_sum}"
        );
    }

    fn complex_gaussian(rng: &mut StdRng, variance: f32) -> Complex32 {
        let std = (variance / 2.0).sqrt();
        let u1: f32 = rng.random::<f32>().max(1e-10);
        let u2: f32 = rng.random();
        let r = std * (-2.0 * u1.ln()).sqrt();
        let theta = std::f32::consts::TAU * u2;
        Complex32::new(r * theta.cos(), r * theta.sin())
    }

    #[test]
    fn ar1_coefficient_matches_watterson_autocorrelation_form() {
        // coppa-channel's own verified formula: rho(tau) = exp(-2 pi^2 sigma^2 tau^2).
        // At sigma=0.5 Hz, tau=0.2s the crate's test measures rho ~ 0.821 (see
        // watterson.rs::fading_process_autocorrelation_matches_gaussian_psd). Our
        // `a` at the same (sigma, tau) should reproduce that continuous-limit value
        // closely (the crate's own test notes the discrete grid perturbs its
        // measured rho somewhat off the continuous prediction; we're checking the
        // continuous formula here, so use a tight tolerance).
        let a = ar1_coefficient(0.5, 0.2);
        assert!(
            (a - 0.821).abs() < 0.01,
            "expected ~0.821 (continuous Gaussian-PSD autocorrelation), got {a}"
        );
    }

    #[test]
    fn ar1_coefficient_uses_the_empirically_calibrated_default() {
        // hf_standard: T_s = 1260/48000 ~ 26.25ms. `DEFAULT_SIGMA_D_HZ` (see its
        // doc) is an empirically-calibrated 4.0 Hz, NOT Watterson-Moderate's
        // literal 0.5 Hz physical Doppler spread (that literal value gives
        // a~0.9966, which was measured to diverge/over-integrate across a whole
        // frame — see the constant's doc for the full account). At 4.0 Hz, `a`
        // should land close to the ~0.80 the empirical sweep found best.
        let t_s = 1260.0 / 48_000.0;
        let a = ar1_coefficient(DEFAULT_SIGMA_D_HZ, t_s);
        assert!(
            (a - 0.80).abs() < 0.02,
            "expected a close to the empirically-tuned ~0.80, got {a}"
        );
    }

    /// Two-tap synthetic channel, matching `delay_domain.rs`'s test fixture.
    fn two_tap_h(nc: usize, h0: Complex32, h5: Complex32) -> impl Fn(usize) -> Complex32 {
        move |k: usize| h0 * tau_basis(nc, k, 0) + h5 * tau_basis(nc, k, 5)
    }

    #[test]
    fn tracks_a_slowly_drifting_two_tap_channel() {
        // The true channel's h0 tap amplitude ramps down linearly over 40 steps
        // (a stand-in for genuine Doppler-driven drift within a frame) while h5
        // stays fixed. A tracker with a reasonable `a` should follow the ramp
        // far better than freezing at the initial (step-0) estimate would.
        let nc = 48;
        let l = 6;
        let steps = 40;
        let pilot_idx: [usize; 8] = [0, 6, 12, 18, 24, 30, 36, 42];
        let mut rng = StdRng::seed_from_u64(11);
        let noise_var = 0.02;

        let h5 = Complex32::from_polar(0.6, -0.4);
        let a = 0.995; // similar order to the real hf_standard AR(1) value
        let h_probe: Vec<Complex32> = {
            let h0 = Complex32::from_polar(0.6, 0.2);
            let true_h = two_tap_h(nc, h0, h5);
            let obs: Vec<(usize, Complex32, f32)> = pilot_idx
                .iter()
                .map(|&k| (k, true_h(k) + complex_gaussian(&mut rng, noise_var), 1.0))
                .collect();
            super::super::delay_domain::DelayDomainEstimator::fit(nc, l, &obs)
                .taps()
                .to_vec()
        };
        let mut tracker = KalmanLagSmoother::new(nc, l, a, noise_var, &h_probe);

        let mut frozen_err_sum = 0.0f32;
        let mut tracked_err_sum = 0.0f32;
        let frozen = super::super::delay_domain::DelayDomainEstimator::fit(
            nc,
            l,
            &pilot_idx
                .iter()
                .map(|&k| {
                    let h0 = Complex32::from_polar(0.6, 0.2);
                    (
                        k,
                        two_tap_h(nc, h0, h5)(k) + complex_gaussian(&mut rng, noise_var),
                        1.0,
                    )
                })
                .collect::<Vec<_>>(),
        );

        for t in 0..steps {
            let frac = t as f32 / (steps - 1) as f32;
            let h0_amp = 0.6 * (1.0 - 0.9 * frac); // ramps from 0.6 down to 0.06
            let h0 = Complex32::from_polar(h0_amp, 0.2);
            let true_h = two_tap_h(nc, h0, h5);
            let obs: Vec<(usize, Complex32, f32)> = pilot_idx
                .iter()
                .map(|&k| (k, true_h(k) + complex_gaussian(&mut rng, noise_var), 1.0))
                .collect();
            tracker.advance(&obs);

            let mse_at = |est_h_at: &dyn Fn(usize) -> Complex32| -> f32 {
                (0..nc)
                    .map(|k| (est_h_at(k) - true_h(k)).norm_sqr())
                    .sum::<f32>()
                    / nc as f32
            };
            frozen_err_sum += mse_at(&|k| frozen.h_at(k));
        }
        for t in 0..steps {
            let tt = tracker.smoothed(t);
            let frac = t as f32 / (steps - 1) as f32;
            let h0_amp = 0.6 * (1.0 - 0.9 * frac);
            let h0 = Complex32::from_polar(h0_amp, 0.2);
            let true_h = two_tap_h(nc, h0, h5);
            tracked_err_sum += (0..nc)
                .map(|k| (tt.h_at(k) - true_h(k)).norm_sqr())
                .sum::<f32>()
                / nc as f32;
        }

        assert!(
            tracked_err_sum < 0.3 * frozen_err_sum,
            "tracker should track the drift far better than a frozen initial fit: \
             tracked_sum={tracked_err_sum}, frozen_sum={frozen_err_sum}"
        );
    }

    #[test]
    fn a_momentary_deep_fade_does_not_corrupt_the_track() {
        // Regression test matching the REAL per-window SNR-collapse scenario Task 1
        // hit in Watterson fading (a momentary deep fade — |h| collapsing toward
        // zero — makes one window's observations far less informative), as opposed
        // to a raw additive-noise-floor mismatch. This design deliberately uses a
        // FIXED, frame-global `sigma_v2` (the additive/thermal noise floor, derived
        // once from the probe) rather than re-deriving it per window — exactly
        // because Task 1's rejected per-window re-fit corrupted the LDPC-facing
        // noise variance by re-estimating noise scale from thin, sometimes-faded
        // windows. A genuine amplitude fade does NOT change the true additive noise
        // floor, so this tracker's fixed-`sigma_v2` assumption stays valid straight
        // through the fade, and the AR(1) prior should carry the state through the
        // low-information window without corruption.
        let nc = 48;
        let l = 6;
        let h5 = Complex32::from_polar(0.7, -1.0);
        let pilot_idx: [usize; 8] = [0, 6, 12, 18, 24, 30, 36, 42];
        let noise_var = 0.02; // stationary additive noise floor throughout
        let a = 0.995;
        let fade_step: usize = 6;
        let n_steps = 12;

        // h0's amplitude dips to near-zero at `fade_step` and recovers — a stand-in
        // for one window landing in a deep Rayleigh fade, then fading back up.
        let h0_at = |t: usize| -> Complex32 {
            let dist = (t as i32 - fade_step as i32).unsigned_abs() as f32;
            let amp = 0.7 * (1.0 - (-0.5 * dist * dist).exp() * 0.98); // ~0.014 at t=fade_step
            Complex32::from_polar(amp, 0.3)
        };
        let true_h_at = |t: usize| two_tap_h(nc, h0_at(t), h5);

        let mut rng = StdRng::seed_from_u64(3);
        let h_probe = super::super::delay_domain::DelayDomainEstimator::fit(
            nc,
            l,
            &pilot_idx
                .iter()
                .map(|&k| {
                    (
                        k,
                        true_h_at(0)(k) + complex_gaussian(&mut rng, noise_var),
                        1.0,
                    )
                })
                .collect::<Vec<_>>(),
        )
        .taps()
        .to_vec();
        let mut tracker = KalmanLagSmoother::new(nc, l, a, noise_var, &h_probe);

        for t in 0..n_steps {
            let true_h = true_h_at(t);
            let obs: Vec<(usize, Complex32, f32)> = pilot_idx
                .iter()
                .map(|&k| (k, true_h(k) + complex_gaussian(&mut rng, noise_var), 1.0))
                .collect();
            tracker.advance(&obs);
        }

        // Steps away from the fade should track the true (recovered) channel
        // accurately, and no noise_at() anywhere should have exploded — the AR(1)
        // prior should carry the state through the low-SNR window gracefully.
        let mut max_noise = 0.0f32;
        for t in 0..n_steps {
            let tt = tracker.smoothed(t);
            for k in 0..nc {
                max_noise = max_noise.max(tt.noise_at(k));
            }
            if !(fade_step.saturating_sub(2)..=fade_step + 2).contains(&t) {
                let true_h = true_h_at(t);
                let mse: f32 = (0..nc)
                    .map(|k| (tt.h_at(k) - true_h(k)).norm_sqr())
                    .sum::<f32>()
                    / nc as f32;
                assert!(
                    mse < 0.2,
                    "step {t} (away from the fade) should still track well, got mse={mse}"
                );
            }
        }
        assert!(
            max_noise < 50.0,
            "no per-carrier noise estimate should explode from one faded window, got max {max_noise}"
        );
    }

    #[test]
    fn equalize_recovers_known_symbols_on_a_clean_channel() {
        let nc = 48;
        let l = 6;
        let h0 = Complex32::from_polar(1.0, 0.0);
        let h5 = Complex32::from_polar(0.5, 0.5);
        let true_h = two_tap_h(nc, h0, h5);
        let pilot_idx: [usize; 8] = [0, 6, 12, 18, 24, 30, 36, 42];
        let h_probe: Vec<Complex32> = {
            let obs: Vec<(usize, Complex32, f32)> =
                pilot_idx.iter().map(|&k| (k, true_h(k), 1.0)).collect();
            super::super::delay_domain::DelayDomainEstimator::fit(nc, l, &obs)
                .taps()
                .to_vec()
        };
        let mut tracker = KalmanLagSmoother::new(nc, l, 0.999, 1e-6, &h_probe);
        for _ in 0..5 {
            let obs: Vec<(usize, Complex32, f32)> =
                pilot_idx.iter().map(|&k| (k, true_h(k), 1.0)).collect();
            tracker.advance(&obs);
        }
        let tt = tracker.smoothed(2);
        let tx_symbol = Complex32::new(1.0, 0.0);
        let carriers: Vec<Complex32> = (0..nc).map(|k| true_h(k) * tx_symbol).collect();
        let (xhat, _noise) = tt.equalize(&carriers);
        for &x in &xhat {
            assert!((x - tx_symbol).norm() < 0.05, "expected ~1.0+0j, got {x}");
        }
    }
}
