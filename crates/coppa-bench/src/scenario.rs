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
    /// Maximum application-payload bytes that fit in one frame, i.e.
    /// `CoppaTransceiver::transmit`'s actual per-level capacity
    /// (`coppa_protocol::modem::speed_levels::max_payload_for_level`): this
    /// level's raw `info_bits/8` byte capacity minus the 4-byte CRC-32 trailer
    /// `transmit` appends (Phase 3 Task 1). Was `info_bits/8` pre-Task-1, back
    /// when a full-capacity payload had no trailer competing for the same
    /// bits.
    pub fn payload_bytes(&self) -> usize {
        coppa_protocol::modem::speed_levels::max_payload_for_level(self.level)
            .unwrap_or_else(|| panic!("unknown speed level {}", self.level))
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
        // Task 4 (NR BG2 mother code) moved level 10's rate from 7/8 to 5/6
        // (wire-format break -- see CLAUDE.md's Known Limitations and
        // docs/adr/005-nr-bg2-ldpc.md). k_used = 1620, not the pre-Task-4
        // 1701.
        name: "64QAM 5/6",
        info_bits: 1620,
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
    /// Emulate a realistic SSB rig's audio passband (`coppa_channel::ssb_filter`,
    /// 300-2700 Hz) applied to the clean TX signal before fading/noise. `false`
    /// (the default) benches against the idealized full-band signal, matching
    /// all pre-existing scenarios. Kept as a sibling `Scenario` field alongside
    /// `cfo_hz` rather than folded into `ChannelSpec`: `ChannelSpec` is consumed
    /// by `coppa-bench`'s examples/`transfer.rs` via bare enum pattern matches,
    /// and `cfo_hz` already established the precedent that "impairments applied
    /// around the channel" live on `Scenario`, not inside `ChannelSpec` itself.
    pub ssb: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_8_is_excluded() {
        assert!(mode_for_level(8).is_none());
    }

    #[test]
    fn payload_bytes_match_info_bits_minus_crc_trailer() {
        // info_bits/8 - 4 (CRC-32 trailer, Phase 3 Task 1): 972/8=121-4=117; 1620/8=202-4=198.
        assert_eq!(mode_for_level(2).unwrap().payload_bytes(), 117);
        assert_eq!(mode_for_level(10).unwrap().payload_bytes(), 198);
    }

    #[test]
    fn profile_switches_at_level_5() {
        assert_ne!(
            select_profile(4).data_carriers,
            select_profile(5).data_carriers
        );
    }
}
