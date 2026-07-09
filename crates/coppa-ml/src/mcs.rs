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

/// Recommend a speed level from a frame's per-carrier noise variances in one call — the single
/// source of truth for the closed-loop rate feedback's receiver-side recommendation (fed back on
/// the ACK, see `coppa_ml::rate_loop::RateLoop`). Wraps `channel_capacity`, `channel_selectivity`,
/// and `select_speed_level_2d` together; kept as one small function (rather than inlined at each
/// call site) so it can later be pointed at per-codeword noise vars (once multi-codeword frames
/// exist) without touching more than one place. Returns level 1 for an empty `noise_vars` (no
/// channel information).
pub fn recommend_speed_level(noise_vars: &[f32]) -> u8 {
    if noise_vars.is_empty() {
        return 1;
    }
    select_speed_level_2d(channel_capacity(noise_vars), channel_selectivity(noise_vars))
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
        let nv = vec![0.001f32; 48]; // strong, flat channel
        assert_eq!(
            recommend_speed_level(&nv),
            select_speed_level_2d(channel_capacity(&nv), channel_selectivity(&nv))
        );
        assert_eq!(recommend_speed_level(&[]), 1);
    }
}
