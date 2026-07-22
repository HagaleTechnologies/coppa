//! Modulation and Coding Scheme (MCS) selection.
//!
//! The channel-adaptive selectors below (`channel_capacity`/`channel_selectivity`/
//! `select_speed_level_2d`) are the only selection path in this module. An earlier,
//! now-deleted `MCS_TABLE`/`select_mcs` (a stale, SNR-only, 8 kHz-era 11-entry table) was
//! superseded by this capacity-based approach and had no real caller left (only its own
//! predictor test) — see `docs/superpowers/plans/2026-07-03-phase3-system-layer.md` decision 5.

/// Average per-carrier Shannon capacity (bits/s/Hz) from RX per-carrier noise variances
/// `nv[k] = sigma^2/|H[k]|^2` (per-carrier SNR = `1/nv[k]`). Captures BOTH effective SNR and
/// frequency selectivity in one number (deep nulls => huge nv => ~0 contribution). Mode-independent
/// because `nv` is pilot-derived.
pub fn channel_capacity(noise_vars: &[f32]) -> f32 {
    if noise_vars.is_empty() {
        return 0.0;
    }
    let sum: f32 = noise_vars
        .iter()
        .map(|&nv| (1.0 + 1.0 / nv.max(1e-9)).log2())
        .sum();
    sum / noise_vars.len() as f32
}

/// Channel frequency-selectivity metric: the standard deviation of per-carrier capacity
/// `log2(1 + 1/nv[k])` across carriers. ~0 for a flat (AWGN) channel; larger when carrier SNRs
/// spread out (frequency-selective fading). Two channels can share the same average capacity `C`
/// but differ in selectivity — and higher-order modes are more sensitive to the spread — so this is
/// the second feature that resolves the channel-dependent ambiguity `C` alone leaves.
pub fn channel_selectivity(noise_vars: &[f32]) -> f32 {
    if noise_vars.is_empty() {
        return 0.0;
    }
    let caps: Vec<f32> = noise_vars
        .iter()
        .map(|&nv| (1.0 + 1.0 / nv.max(1e-9)).log2())
        .collect();
    let mean = caps.iter().sum::<f32>() / caps.len() as f32;
    let var = caps.iter().map(|c| (c - mean).powi(2)).sum::<f32>() / caps.len() as f32;
    var.sqrt()
}

/// (speed level, spectral efficiency η = bits_per_symbol × code_rate) for the real 9 levels,
/// ascending by η, lower modulation order listed first at an η tie (more fading-robust).
pub const SPEED_LEVEL_EFFICIENCY: [(u8, f32); 9] = [
    (1, 0.25),
    (2, 0.50),
    (3, 1.00),
    (4, 1.50),
    (5, 2.00), // 8PSK 2/3 (tie with 16QAM 1/2; lower order first)
    (6, 2.00), // 16QAM 1/2
    (7, 3.00),
    (9, 4.00),
    (10, 5.25),
];

/// Highest speed level whose spectral efficiency fits `capacity - margin` (Shannon-to-practical
/// coding gap). At an η tie the first-listed (lower-order) level wins. Returns level 1 if none fit.
///
/// This flat-margin rule reaches ~0.83 of oracle; for the calibrated table that closes more of the
/// gap (and handles the channel-dependent gap nonlinearity), prefer `select_speed_level_calibrated`.
pub fn select_speed_level(capacity: f32, margin: f32) -> u8 {
    let budget = capacity - margin;
    let mut best_level = SPEED_LEVEL_EFFICIENCY[0].0;
    let mut best_eta = -1.0f32;
    for &(level, eta) in &SPEED_LEVEL_EFFICIENCY {
        if eta <= budget && eta > best_eta {
            best_level = level;
            best_eta = eta;
        }
    }
    best_level
}

/// (speed level, minimum average per-carrier capacity C at which the level is goodput-optimal),
/// ascending. Calibrated from a measured grid sweep (`mcs_calibration` example, robust profile,
/// seed 0xCA11B, 8-frame averaged sounding): thresholds are anchored by the clean regions —
/// Good/Moderate→16QAM 1/2 (L6) at C≈4–6.4, AWGN→16QAM 3/4 (L7) @5.9 / 64QAM 2/3 (L9) @7.25 /
/// 64QAM 7/8 (L10) @8.1 — and made conservative in the C≈5.9–6.4 overlap where the goodput-optimal
/// level is channel-dependent (AWGN wants L7, Good wants L6), so the safe L6 is chosen there.
pub const SPEED_LEVEL_MIN_CAPACITY: [(u8, f32); 9] = [
    (1, 0.0),
    (2, 1.5),
    (3, 2.2),
    (4, 2.6),
    (5, 3.0),
    (6, 4.0),
    (7, 6.5),
    (9, 7.2),
    (10, 8.0),
];

/// Selectivity reference (≈ the selectivity the per-level thresholds were calibrated at, i.e. the
/// fading channels) and the per-unit-selectivity capacity correction. Derived from the calibration
/// grid: at C≈6, a flat channel (selectivity ≈0.6) is goodput-optimal at 16QAM 3/4 (L7) while a
/// selective one (≈2.0) is at 16QAM 1/2 (L6); the correction shifts effective capacity to separate
/// them.
const SELECTIVITY_REF: f32 = 1.5;
const SELECTIVITY_GAIN: f32 = 0.7;

/// Channel-adaptive speed-level selection using BOTH average capacity and frequency selectivity.
/// A flatter channel (low selectivity) supports a higher modulation order at the same average
/// capacity than a selective one, which the calibrated capacity table alone cannot express. This
/// applies a selectivity correction to the capacity (flat → boost, selective → penalty) and looks
/// the result up in the per-level threshold table, resolving the channel-dependent overlap.
pub fn select_speed_level_2d(capacity: f32, selectivity: f32) -> u8 {
    let effective = capacity + SELECTIVITY_GAIN * (SELECTIVITY_REF - selectivity);
    select_speed_level_calibrated(effective)
}

/// Select the highest speed level whose calibrated minimum capacity `C_min(L)` is at most the
/// measured channel capacity. Returns level 1 if none qualify. Unlike the flat-margin rule, the
/// per-level thresholds absorb the channel-dependent, nonlinear Shannon-to-practical gap.
pub fn select_speed_level_calibrated(capacity: f32) -> u8 {
    let mut best_level = SPEED_LEVEL_MIN_CAPACITY[0].0;
    for &(level, c_min) in &SPEED_LEVEL_MIN_CAPACITY {
        if capacity >= c_min {
            best_level = level;
        }
    }
    best_level
}

/// SNR grid points (dB) the `*_LEVEL_CORRECTION` tables below are calibrated at. Measured under
/// AWGN by `crates/coppa-bench/examples/capacity_level_correction_calibration.rs` (profile
/// `robust`, seed `0xCA11B`, 200 trials/cell) -- see `BENCHMARKS.md`'s dated section for the real
/// run this table was generated from.
const CORRECTION_SNR_GRID: [f32; 5] = [6.0, 12.0, 18.0, 24.0, 30.0];

/// Levels in the same order as `SPEED_LEVEL_EFFICIENCY`/`SPEED_LEVEL_MIN_CAPACITY` (level 8 is
/// reserved and excluded).
const CORRECTION_LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];

/// `CAPACITY_LEVEL_CORRECTION[level_idx][snr_idx]` = mean AWGN `channel_capacity` at that level and
/// SNR, minus the same at level 2 (the probe `SPEED_LEVEL_MIN_CAPACITY` was itself calibrated
/// against in `mcs_calibration.rs`) -- see PR #51/#52's diagnosis (`BENCHMARKS.md`) for why this
/// bias exists (the per-level PAPR clip target distorts the pilot subcarriers the noise-variance
/// estimate is read from) and why it grows with SNR rather than being a flat per-level offset.
/// Level 2's row is all-zeros by construction.
const CAPACITY_LEVEL_CORRECTION: [[f32; 5]; 9] = [
    [0.0988, 0.1214, 0.1591, 0.1883, 0.1980], // level 1
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000], // level 2
    [0.1779, 0.4456, 0.9628, 1.5613, 1.9017], // level 3
    [0.3748, 0.5513, 1.0393, 1.6476, 1.9984], // level 4
    [0.5117, 0.7046, 1.3565, 2.3573, 3.1329], // level 5
    [0.6852, 0.8296, 1.4354, 2.3614, 3.0481], // level 6
    [0.7263, 0.8348, 1.4593, 2.4551, 3.2263], // level 7
    [0.8057, 0.9124, 1.5354, 2.5364, 3.3269], // level 9
    [0.8784, 0.9243, 1.5268, 2.5208, 3.3047], // level 10
];

/// Same as `CAPACITY_LEVEL_CORRECTION` but for `channel_selectivity` -- the same clip-induced bias
/// separately depresses apparent selectivity at higher levels, which compounds the capacity bias in
/// `select_speed_level_2d` (lower selectivity reads as flatter, boosting effective capacity
/// further).
const SELECTIVITY_LEVEL_CORRECTION: [[f32; 5]; 9] = [
    [-0.0099, -0.0133, -0.0144, -0.0148, -0.0149], // level 1
    [0.0000, 0.0000, 0.0000, 0.0000, 0.0000],      // level 2
    [-0.0743, -0.0910, -0.0967, -0.0982, -0.0985], // level 3
    [-0.0403, -0.0489, -0.0510, -0.0515, -0.0516], // level 4
    [-0.0752, -0.0970, -0.1038, -0.1057, -0.1063], // level 5
    [-0.0806, -0.1022, -0.1077, -0.1090, -0.1093], // level 6
    [-0.0878, -0.1150, -0.1229, -0.1249, -0.1254], // level 7
    [-0.0953, -0.1236, -0.1321, -0.1343, -0.1348], // level 9
    [-0.1051, -0.1453, -0.1649, -0.1743, -0.1780], // level 10
];

/// SNR estimate (dB) from per-carrier noise variances -- same formula
/// `coppa_protocol::modem::transceiver` computes independently for its own SNR telemetry
/// (`10*log10(1/mean(noise_vars))`), duplicated here rather than shared to avoid coppa-ml
/// depending on coppa-protocol.
fn estimate_snr_db(noise_vars: &[f32]) -> f32 {
    let mean_nv = if noise_vars.is_empty() {
        1.0
    } else {
        noise_vars.iter().sum::<f32>() / noise_vars.len() as f32
    };
    10.0 * (1.0 / mean_nv.max(1e-6)).log10()
}

/// Linearly interpolate `table`'s row for `level` at `snr_db`, clamped to `CORRECTION_SNR_GRID`'s
/// range (no extrapolation beyond the calibrated 6-30 dB span). Levels absent from
/// `CORRECTION_LEVELS` (shouldn't occur -- headers only ever carry real levels) get a `0.0`
/// correction rather than panicking, the same "if we don't know, don't correct" default
/// `select_speed_level_calibrated` uses for an unmatched capacity.
fn interpolate_correction(table: &[[f32; 5]; 9], level: u8, snr_db: f32) -> f32 {
    let Some(row_idx) = CORRECTION_LEVELS.iter().position(|&l| l == level) else {
        return 0.0;
    };
    let row = &table[row_idx];
    let snr = snr_db.clamp(
        CORRECTION_SNR_GRID[0],
        CORRECTION_SNR_GRID[CORRECTION_SNR_GRID.len() - 1],
    );
    for i in 0..CORRECTION_SNR_GRID.len() - 1 {
        let (lo, hi) = (CORRECTION_SNR_GRID[i], CORRECTION_SNR_GRID[i + 1]);
        if snr <= hi {
            let t = (snr - lo) / (hi - lo);
            return row[i] + t * (row[i + 1] - row[i]);
        }
    }
    row[row.len() - 1]
}

/// Correct a raw `(capacity, selectivity)` reading measured at `measured_at_level` for that
/// level's known, deterministic PAPR-clip-induced self-noise floor (PR #51/#52), bringing it onto
/// the level-2 probe's scale `SPEED_LEVEL_MIN_CAPACITY` was calibrated against.
fn correct_for_level_bias(
    capacity: f32,
    selectivity: f32,
    snr_db: f32,
    measured_at_level: u8,
) -> (f32, f32) {
    let cap_correction =
        interpolate_correction(&CAPACITY_LEVEL_CORRECTION, measured_at_level, snr_db);
    let sel_correction =
        interpolate_correction(&SELECTIVITY_LEVEL_CORRECTION, measured_at_level, snr_db);
    (capacity - cap_correction, selectivity - sel_correction)
}

/// Recommend a speed level from a frame's per-carrier noise variances in one call — the single
/// source of truth for the closed-loop rate feedback's receiver-side recommendation (fed back on
/// the ACK, see `coppa_ml::rate_loop::RateLoop`). `measured_at_level` is the speed level the frame
/// these `noise_vars` came from was actually sent at (the decoded header's `speed_level`) --
/// needed because `channel_capacity`/`channel_selectivity` carry a known, level-dependent
/// self-noise floor from that level's own PAPR clip target (PR #51/#52); `correct_for_level_bias`
/// removes it, bringing the reading onto the fixed level-2 probe's scale, before the calibrated
/// lookup. Wraps `channel_capacity`, `channel_selectivity`, and `select_speed_level_2d` together;
/// kept as one small function (rather than inlined at each call site) so it can later be pointed
/// at per-codeword noise vars (once multi-codeword frames exist) without touching more than one
/// place. Returns level 1 for an empty `noise_vars` (no channel information). Currently regresses
/// `closed_loop_arq`'s Watterson-fading tail (net negative on that bench, though not on the
/// AWGN-only case this correction was built and validated against) — see `BENCHMARKS.md`'s
/// "RateLoop capacity/selectivity level-bias correction" section for the measured numbers and the
/// known next step (a selectivity-scaled correction).
pub fn recommend_speed_level(noise_vars: &[f32], measured_at_level: u8) -> u8 {
    if noise_vars.is_empty() {
        return 1;
    }
    let raw_capacity = channel_capacity(noise_vars);
    let raw_selectivity = channel_selectivity(noise_vars);
    // Carries the same residual level bias as raw_capacity/raw_selectivity (the PAPR-clip effect
    // that inflates capacity at higher levels also lowers mean noise variance) -- an approximation
    // of the frame's true SNR, not the true SNR itself.
    let snr_db = estimate_snr_db(noise_vars);
    let (capacity, selectivity) =
        correct_for_level_bias(raw_capacity, raw_selectivity, snr_db, measured_at_level);
    select_speed_level_2d(capacity, selectivity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_reflects_snr_and_nulls() {
        let strong = vec![0.001f32; 48];
        let c_strong = channel_capacity(&strong);
        assert!(c_strong > 9.0, "strong channel capacity {c_strong}");
        let mut half = vec![0.001f32; 24];
        half.extend(vec![1e6f32; 24]);
        let c_half = channel_capacity(&half);
        assert!(
            (c_half - c_strong / 2.0).abs() < 1.0,
            "half-nulled {c_half} vs {}",
            c_strong / 2.0
        );
        assert_eq!(channel_capacity(&[]), 0.0);
    }

    #[test]
    fn select_speed_level_is_capacity_aware() {
        assert_eq!(select_speed_level(10.0, 1.0), 10);
        assert_eq!(select_speed_level(2.5, 1.0), 4);
        assert_eq!(select_speed_level(2.0, 0.0), 5); // eta=2.0 tie -> lower-order 8PSK (level 5)
        assert_eq!(select_speed_level(0.1, 1.0), 1);
    }

    #[test]
    fn selectivity_zero_for_flat_high_for_spread() {
        // Flat channel: all carriers equal nv => zero selectivity.
        assert!(channel_selectivity(&[0.01f32; 48]) < 1e-4);
        // Spread: half strong, half weak => non-trivial selectivity.
        let mut spread = vec![0.001f32; 24];
        spread.extend(vec![10.0f32; 24]);
        assert!(
            channel_selectivity(&spread) > 1.0,
            "selective channel should read high"
        );
        assert_eq!(channel_selectivity(&[]), 0.0);
    }

    #[test]
    fn select_speed_level_calibrated_uses_thresholds() {
        assert_eq!(select_speed_level_calibrated(0.0), 1); // below all but L1's 0.0
        assert_eq!(select_speed_level_calibrated(2.4), 3); // >=2.2 (L3), <2.6 (L4)
        assert_eq!(select_speed_level_calibrated(5.0), 6); // >=4.0 (L6), <6.5 (L7)
        assert_eq!(select_speed_level_calibrated(6.0), 6); // conservative overlap region
        assert_eq!(select_speed_level_calibrated(7.25), 9); // >=7.2 (L9), <8.0 (L10)
        assert_eq!(select_speed_level_calibrated(8.5), 10);
    }

    #[test]
    fn select_speed_level_2d_separates_overlap() {
        // Same average capacity (~6), different selectivity: a flat channel (AWGN-like) should
        // reach a higher order than a selective one (fading), resolving the C-alone ambiguity.
        let flat = select_speed_level_2d(6.0, 0.6);
        let selective = select_speed_level_2d(6.0, 2.1);
        assert!(
            flat > selective,
            "flat {flat} should exceed selective {selective} at equal C"
        );
        assert_eq!(flat, 7); // 16QAM 3/4
        assert_eq!(selective, 6); // 16QAM 1/2
                                  // At the calibration reference selectivity it matches the 1D table.
        assert_eq!(
            select_speed_level_2d(5.0, super::SELECTIVITY_REF),
            select_speed_level_calibrated(5.0)
        );
    }

    #[test]
    fn select_speed_level_calibrated_is_monotonic() {
        let mut prev = 0u8;
        let mut c = 0.0f32;
        while c < 10.0 {
            let lvl = select_speed_level_calibrated(c);
            // level index in the ordered table must not decrease as C grows
            let idx = |l: u8| {
                SPEED_LEVEL_MIN_CAPACITY
                    .iter()
                    .position(|&(x, _)| x == l)
                    .unwrap()
            };
            assert!(
                prev == 0 || idx(lvl) >= idx(prev),
                "non-monotonic at C={c}: {prev}->{lvl}"
            );
            prev = lvl;
            c += 0.25;
        }
    }

    #[test]
    fn recommend_speed_level_matches_2d_selector_and_handles_empty() {
        // Level 2's correction is 0.0 at every SNR by construction, so recommend_speed_level's
        // level-2 behavior must be bit-for-bit identical to the uncorrected 2D selector -- a
        // natural regression guard for the correction logic.
        let nv = vec![0.001f32; 48]; // strong, flat channel
        assert_eq!(
            recommend_speed_level(&nv, 2),
            select_speed_level_2d(channel_capacity(&nv), channel_selectivity(&nv))
        );
        assert_eq!(recommend_speed_level(&[], 2), 1);
    }

    #[test]
    fn recommend_speed_level_corrects_high_level_readings_downward() {
        // A borderline channel whose RAW level-10 reading would recommend level 10, but whose
        // level-2-equivalent (corrected) reading is lower -- reproduces the self-reinforcing bias
        // PR #51/#52 diagnosed: measuring via a high level should no longer read artificially
        // better than measuring the same real channel via level 2.
        let nv = vec![0.00335f32; 48]; // raw capacity ~= 8.2 bits/s/Hz, ~24 dB by estimate_snr_db
        let raw_capacity = channel_capacity(&nv);
        let raw_selectivity = channel_selectivity(&nv);
        let uncorrected = select_speed_level_2d(raw_capacity, raw_selectivity);
        let corrected_via_level10 = recommend_speed_level(&nv, 10);
        // Level 10's correction at ~24 dB is a ~2.5 bits/s/Hz downward shift plus a selectivity
        // correction that ALSO reduces the effective-capacity boost -- the corrected
        // recommendation must not exceed the uncorrected one.
        assert!(
            corrected_via_level10 <= uncorrected,
            "corrected {corrected_via_level10} should not exceed uncorrected {uncorrected}"
        );
    }

    #[test]
    fn level_bias_correction_is_zero_at_anchor_level() {
        for &snr in &[0.0, 6.0, 15.0, 24.0, 40.0] {
            assert_eq!(
                interpolate_correction(&CAPACITY_LEVEL_CORRECTION, 2, snr),
                0.0
            );
            assert_eq!(
                interpolate_correction(&SELECTIVITY_LEVEL_CORRECTION, 2, snr),
                0.0
            );
        }
    }

    #[test]
    fn level_bias_correction_interpolates_between_grid_points() {
        // Level 10's capacity correction row: [0.8784, 0.9243, 1.5268, 2.5208, 3.3047] at
        // SNR grid [6, 12, 18, 24, 30]. At 15 dB (halfway between 12 and 18), the interpolated
        // value should be the midpoint of 0.9243 and 1.5268.
        let expected = 0.9243 + 0.5 * (1.5268 - 0.9243);
        let got = interpolate_correction(&CAPACITY_LEVEL_CORRECTION, 10, 15.0);
        assert!(
            (got - expected).abs() < 1e-3,
            "got {got}, expected {expected}"
        );
    }

    #[test]
    fn level_bias_correction_clamps_outside_grid() {
        // Level 10's capacity row's first/last entries: 0.8784 (6 dB), 3.3047 (30 dB). Below 6 dB
        // or above 30 dB must clamp to the nearest grid endpoint, not extrapolate.
        assert!(
            (interpolate_correction(&CAPACITY_LEVEL_CORRECTION, 10, 0.0) - 0.8784).abs() < 1e-3
        );
        assert!(
            (interpolate_correction(&CAPACITY_LEVEL_CORRECTION, 10, 50.0) - 3.3047).abs() < 1e-3
        );
    }

    #[test]
    fn level_bias_correction_falls_back_to_zero_for_unknown_level() {
        // Level 8 is reserved and never appears in CORRECTION_LEVELS.
        assert_eq!(
            interpolate_correction(&CAPACITY_LEVEL_CORRECTION, 8, 18.0),
            0.0
        );
        assert_eq!(
            interpolate_correction(&SELECTIVITY_LEVEL_CORRECTION, 8, 18.0),
            0.0
        );
    }

    #[test]
    fn capacity_correction_matches_calibration_bench_raw_data() {
        // Independent spot-check against `capacity_level_correction_calibration.rs`'s own raw
        // DATA output (not the RUST_TABLE block this const was transcribed from): "DATA 24 10
        // 10.7188 ..." and "DATA 24 2 8.1981 ...". Level 10's correction at 24 dB must equal
        // their difference -- catches a transcription slip in either copy.
        let level10_raw_24db = 10.7188_f32;
        let level2_raw_24db = 8.1981_f32;
        let expected = level10_raw_24db - level2_raw_24db;
        // Level 10 is CORRECTION_LEVELS' last entry (index 8); 24 dB is CORRECTION_SNR_GRID's
        // index 3.
        assert!(
            (CAPACITY_LEVEL_CORRECTION[8][3] - expected).abs() < 0.001,
            "level 10 @ 24dB correction {} != raw diff {}",
            CAPACITY_LEVEL_CORRECTION[8][3],
            expected
        );
    }

    #[test]
    fn selectivity_correction_matches_calibration_bench_raw_data() {
        // Same independent spot-check for selectivity: "DATA 24 10 ... 0.0170" and
        // "DATA 24 2 ... 0.1913".
        let level10_raw_24db = 0.0170_f32;
        let level2_raw_24db = 0.1913_f32;
        let expected = level10_raw_24db - level2_raw_24db;
        assert!(
            (SELECTIVITY_LEVEL_CORRECTION[8][3] - expected).abs() < 0.001,
            "level 10 @ 24dB correction {} != raw diff {}",
            SELECTIVITY_LEVEL_CORRECTION[8][3],
            expected
        );
    }

    #[test]
    fn estimate_snr_db_matches_transceiver_formula() {
        // Same formula as coppa_protocol::modem::transceiver's own SNR estimate
        // (10*log10(1/mean(noise_vars))), duplicated here to avoid coppa-ml depending on
        // coppa-protocol.
        let nv = vec![0.01_f32; 48]; // mean_nv = 0.01 -> 10*log10(100) = 20 dB
        let got = estimate_snr_db(&nv);
        assert!((got - 20.0).abs() < 1e-3, "got {got}");
        // Empty input falls back to mean_nv = 1.0 -> 10*log10(1) = 0.0 dB.
        assert_eq!(estimate_snr_db(&[]), 0.0);
    }
}
