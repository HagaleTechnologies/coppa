//! Scenario definitions: which mode, channel, and SNR points to measure.

use coppa_codec::ofdm::CoppaProfile;

/// Audio sample rate (Hz). All Coppa OFDM profiles run at 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;

/// Static description of a Coppa speed level, for labeling and payload sizing.
#[derive(Debug, Clone, Copy)]
pub struct ModeInfo {
    pub level: u8,
    pub name: &'static str,
    /// LDPC info bits per frame (1944 × code_rate).
    pub info_bits: usize,
}

impl ModeInfo {
    /// Maximum payload bytes that fit in one frame.
    pub fn payload_bytes(&self) -> usize {
        self.info_bits / 8
    }
}

/// The measurable speed levels (level 8 is reserved/32-QAM and excluded).
pub const MODES: &[ModeInfo] = &[
    ModeInfo {
        level: 1,
        name: "BPSK 1/4",
        info_bits: 486,
    },
    ModeInfo {
        level: 2,
        name: "BPSK 1/2",
        info_bits: 972,
    },
    ModeInfo {
        level: 3,
        name: "QPSK 1/2",
        info_bits: 972,
    },
    ModeInfo {
        level: 4,
        name: "QPSK 3/4",
        info_bits: 1458,
    },
    ModeInfo {
        level: 5,
        name: "8PSK 2/3",
        info_bits: 1296,
    },
    ModeInfo {
        level: 6,
        name: "16QAM 1/2",
        info_bits: 972,
    },
    ModeInfo {
        level: 7,
        name: "16QAM 3/4",
        info_bits: 1458,
    },
    ModeInfo {
        level: 9,
        name: "64QAM 2/3",
        info_bits: 1296,
    },
    ModeInfo {
        level: 10,
        name: "64QAM 7/8",
        info_bits: 1701,
    },
];

/// Look up a mode by speed level.
pub fn mode_for_level(level: u8) -> Option<&'static ModeInfo> {
    MODES.iter().find(|m| m.level == level)
}

/// Select the OFDM profile for a speed level, mirroring the engine's rule
/// (levels 1-4 use HF standard, 5+ use VHF wide).
pub fn select_profile(level: u8) -> CoppaProfile {
    if level >= 5 {
        CoppaProfile::vhf_wide()
    } else {
        CoppaProfile::hf_standard()
    }
}

/// Resolve a named override profile for benchmarking. `"default"` means "use the per-level
/// `select_profile` rule"; `"standard"`/`"robust"` force that profile for every level.
pub fn profile_by_name(name: &str) -> Option<CoppaProfile> {
    match name {
        "default" => None,
        "standard" => Some(CoppaProfile::hf_standard()),
        "robust" => Some(CoppaProfile::hf_robust()),
        other => panic!("unknown profile '{other}' (expected: default|standard|robust)"),
    }
}

/// Channel under test.
#[derive(Debug, Clone, Copy)]
pub enum ChannelSpec {
    /// AWGN only (no fading).
    Awgn,
    /// Watterson HF fading (applied before AWGN).
    Watterson(coppa_channel::watterson::WattersonPreset),
}

/// A measurement scenario: one mode swept over SNR points on one channel.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub level: u8,
    pub channel: ChannelSpec,
    pub snr_db_points: Vec<f32>,
    pub trials: usize,
    /// Base RNG seed; per-trial seeds are derived from this.
    pub seed: u64,
    /// Optional profile override; `None` uses `select_profile(level)`.
    pub profile_override: Option<CoppaProfile>,
    /// Carrier frequency offset (Hz) applied after the channel; 0.0 = none.
    pub cfo_hz: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_8_is_excluded() {
        assert!(mode_for_level(8).is_none());
    }

    #[test]
    fn payload_bytes_match_info_bits() {
        assert_eq!(mode_for_level(2).unwrap().payload_bytes(), 121);
        assert_eq!(mode_for_level(10).unwrap().payload_bytes(), 212);
    }

    #[test]
    fn profile_switches_at_level_5() {
        assert_ne!(
            select_profile(4).data_carriers,
            select_profile(5).data_carriers
        );
    }
}
