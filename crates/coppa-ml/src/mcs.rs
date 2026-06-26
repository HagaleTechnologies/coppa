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
}
