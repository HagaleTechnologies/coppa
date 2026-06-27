//! Modulation and Coding Scheme (MCS) table and selection.

/// A single MCS entry defining modulation, coding rate, and throughput.
#[derive(Debug, Clone, Copy)]
pub struct McsEntry {
    /// MCS index (0 = most robust).
    pub index: u8,
    /// Human-readable name.
    pub name: &'static str,
    /// Minimum SNR in dB required for reliable operation.
    pub min_snr_db: f32,
    /// Approximate throughput in bits per second at 8 kHz sample rate.
    pub throughput_bps: f32,
    /// Bits per symbol for the modulation.
    pub bits_per_symbol: u8,
    /// FEC code rate numerator.
    pub fec_rate_num: u8,
    /// FEC code rate denominator.
    pub fec_rate_den: u8,
}

impl McsEntry {
    /// FEC code rate as a float.
    pub fn fec_rate(&self) -> f32 {
        self.fec_rate_num as f32 / self.fec_rate_den as f32
    }

    /// Spectral efficiency in bits/s/Hz.
    pub fn spectral_efficiency(&self) -> f32 {
        self.bits_per_symbol as f32 * self.fec_rate()
    }
}

/// MCS table ordered from most robust (index 0) to fastest (index 10).
pub const MCS_TABLE: [McsEntry; 11] = [
    McsEntry {
        index: 0,
        name: "BPSK 1/4",
        min_snr_db: -2.0,
        throughput_bps: 31.0,
        bits_per_symbol: 1,
        fec_rate_num: 1,
        fec_rate_den: 4,
    },
    McsEntry {
        index: 1,
        name: "BPSK 1/3",
        min_snr_db: 0.0,
        throughput_bps: 42.0,
        bits_per_symbol: 1,
        fec_rate_num: 1,
        fec_rate_den: 3,
    },
    McsEntry {
        index: 2,
        name: "BPSK 1/2",
        min_snr_db: 2.0,
        throughput_bps: 62.0,
        bits_per_symbol: 1,
        fec_rate_num: 1,
        fec_rate_den: 2,
    },
    McsEntry {
        index: 3,
        name: "BPSK 3/4",
        min_snr_db: 5.0,
        throughput_bps: 94.0,
        bits_per_symbol: 1,
        fec_rate_num: 3,
        fec_rate_den: 4,
    },
    McsEntry {
        index: 4,
        name: "QPSK 1/2",
        min_snr_db: 7.0,
        throughput_bps: 125.0,
        bits_per_symbol: 2,
        fec_rate_num: 1,
        fec_rate_den: 2,
    },
    McsEntry {
        index: 5,
        name: "QPSK 3/4",
        min_snr_db: 10.0,
        throughput_bps: 188.0,
        bits_per_symbol: 2,
        fec_rate_num: 3,
        fec_rate_den: 4,
    },
    McsEntry {
        index: 6,
        name: "8PSK 1/2",
        min_snr_db: 12.0,
        throughput_bps: 188.0,
        bits_per_symbol: 3,
        fec_rate_num: 1,
        fec_rate_den: 2,
    },
    McsEntry {
        index: 7,
        name: "8PSK 3/4",
        min_snr_db: 15.0,
        throughput_bps: 281.0,
        bits_per_symbol: 3,
        fec_rate_num: 3,
        fec_rate_den: 4,
    },
    McsEntry {
        index: 8,
        name: "16QAM 1/2",
        min_snr_db: 16.0,
        throughput_bps: 250.0,
        bits_per_symbol: 4,
        fec_rate_num: 1,
        fec_rate_den: 2,
    },
    McsEntry {
        index: 9,
        name: "16QAM 3/4",
        min_snr_db: 20.0,
        throughput_bps: 375.0,
        bits_per_symbol: 4,
        fec_rate_num: 3,
        fec_rate_den: 4,
    },
    McsEntry {
        index: 10,
        name: "64QAM 3/4",
        min_snr_db: 25.0,
        throughput_bps: 563.0,
        bits_per_symbol: 6,
        fec_rate_num: 3,
        fec_rate_den: 4,
    },
];

/// Select the best MCS for the given SNR with a safety margin.
///
/// Returns the highest-throughput MCS whose min_snr is at most `snr_db - margin_db`.
pub fn select_mcs(snr_db: f32, margin_db: f32) -> &'static McsEntry {
    let effective_snr = snr_db - margin_db;
    let mut best = &MCS_TABLE[0];
    for entry in &MCS_TABLE {
        if effective_snr >= entry.min_snr_db {
            best = entry;
        } else {
            break;
        }
    }
    best
}

/// Average per-carrier Shannon capacity (bits/s/Hz) from RX per-carrier noise variances
/// `nv[k] = sigma^2/|H[k]|^2` (per-carrier SNR = `1/nv[k]`). Captures BOTH effective SNR and
/// frequency selectivity in one number (deep nulls => huge nv => ~0 contribution). Mode-independent
/// because `nv` is pilot-derived. Supersedes the SNR-only `MCS_TABLE` for channel-adaptive selection.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcs_table_ordered() {
        for i in 1..MCS_TABLE.len() {
            assert!(
                MCS_TABLE[i].min_snr_db >= MCS_TABLE[i - 1].min_snr_db,
                "MCS table not ordered by SNR at index {}",
                i
            );
        }
    }

    #[test]
    fn test_select_mcs_low_snr() {
        let mcs = select_mcs(-5.0, 2.0);
        assert_eq!(mcs.index, 0); // Most robust
    }

    #[test]
    fn test_select_mcs_high_snr() {
        let mcs = select_mcs(30.0, 2.0);
        assert_eq!(mcs.index, 10); // Fastest
    }

    #[test]
    fn test_select_mcs_mid_snr() {
        let mcs = select_mcs(12.0, 2.0);
        // 12 - 2 = 10 dB effective, QPSK 3/4 needs 10 dB
        assert_eq!(mcs.index, 5);
    }

    #[test]
    fn test_mcs_fec_rate() {
        assert!((MCS_TABLE[2].fec_rate() - 0.5).abs() < 0.001);
        assert!((MCS_TABLE[3].fec_rate() - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_mcs_spectral_efficiency() {
        // BPSK 1/2: 1 * 0.5 = 0.5
        assert!((MCS_TABLE[2].spectral_efficiency() - 0.5).abs() < 0.001);
        // QPSK 3/4: 2 * 0.75 = 1.5
        assert!((MCS_TABLE[5].spectral_efficiency() - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_select_mcs_with_margin() {
        // At exactly 7 dB with 0 margin, should get QPSK 1/2
        let mcs = select_mcs(7.0, 0.0);
        assert_eq!(mcs.index, 4);

        // At 7 dB with 3 dB margin, effective = 4, gets BPSK 1/2
        let mcs = select_mcs(7.0, 3.0);
        assert_eq!(mcs.index, 2);
    }

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
}
