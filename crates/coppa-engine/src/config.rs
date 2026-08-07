//! Configuration types for the Coppa engine.

use crate::profiles::Profile;
use coppa_protocol::cp_negotiator::CpMode;

/// Runtime configuration for [`CoppaCore`](crate::CoppaCore).
///
/// The `speed_level` selects the constellation and LDPC code rate used by
/// [`CoppaTransceiver`](coppa_protocol::modem::transceiver::CoppaTransceiver).
/// All other modulation parameters are determined internally.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Speed level (1-9). Maps to constellation + LDPC rate in CoppaTransceiver.
    pub speed_level: u8,
    /// Sample rate in Hz. All profiles use 48000.
    pub sample_rate: u32,
    /// Whether to apply Huffman + LZ4 compression before encoding.
    pub compression_enabled: bool,
    /// Squelch threshold in dBFS. Signals below this level are rejected.
    /// Set to `f32::NEG_INFINITY` to disable squelch.
    pub squelch_threshold_db: f32,
    /// The **negotiated** CP profile, which is in effect within the HF range
    /// (levels 1-4) and merely *dormant* at VHF levels (>=5, where no short-CP
    /// variant exists and `CoppaCore::select_ofdm_profile` ignores this field
    /// entirely). Crossing into VHF does not modify it, and dropping back below
    /// level 5 therefore restores the negotiated profile automatically --
    /// suspended, not lost.
    ///
    /// COP-2 changed this. `CoppaCore::set_speed_level` used to actively reset
    /// this field to `CpMode::LongCp` on crossing the threshold, permanently:
    /// dropping back below level 5 rebuilt onto `hf_standard` (CP 300) while a
    /// peer that had negotiated short CP and was never told anything stayed on
    /// `hf_standard_short_cp` (CP 144) -- mutually undecodable, both stations
    /// back inside the HF range where each believes the link should work, with
    /// no give-up trigger armed anywhere (COP-1's G1-G4 all watch *in-flight*
    /// negotiations, and this one had already succeeded) and no reachable
    /// recovery path. That reset was also redundant with
    /// `select_ofdm_profile`'s own VHF branch, so deleting it cost nothing.
    ///
    /// Consequence worth knowing: at `speed_level >= 5` this field describes
    /// the peer agreement, NOT the waveform on air. See
    /// `CoppaCore::set_speed_level`/`CoppaCore::cp_mode`'s docs,
    /// `coppa_protocol::cp_negotiator`, and
    /// `docs/superpowers/specs/2026-07-29-cp-switch-peer-negotiation-design.md`.
    /// The HF<->VHF *profile* desync (a `RateLoop` climb across the boundary
    /// puts this station on `vhf_wide` while the peer is still on an HF
    /// profile) is a separate, still-open gap this does not close.
    pub cp_mode: CpMode,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            speed_level: 1,
            sample_rate: 48_000,
            compression_enabled: false,
            squelch_threshold_db: f32::NEG_INFINITY,
            cp_mode: CpMode::LongCp,
        }
    }
}

impl EngineConfig {
    /// Create a config from a named profile.
    pub fn from_profile(profile: &Profile) -> Self {
        Self {
            speed_level: profile.speed_level,
            sample_rate: profile.sample_rate,
            compression_enabled: profile.compression,
            squelch_threshold_db: f32::NEG_INFINITY,
            cp_mode: CpMode::LongCp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EngineConfig::default();
        assert_eq!(config.speed_level, 1);
        assert_eq!(config.sample_rate, 48000);
        assert!(!config.compression_enabled);
        assert_eq!(config.squelch_threshold_db, f32::NEG_INFINITY);
    }

    #[test]
    fn test_from_profile() {
        use crate::profiles::HF_ROBUST;
        let config = EngineConfig::from_profile(&HF_ROBUST);
        assert_eq!(config.speed_level, 1);
        assert_eq!(config.sample_rate, 48000);
        assert!(!config.compression_enabled);
    }

    #[test]
    fn test_from_profile_vhf() {
        use crate::profiles::VHF_FAST;
        let config = EngineConfig::from_profile(&VHF_FAST);
        assert_eq!(config.speed_level, 9);
        assert!(config.compression_enabled);
    }

    #[test]
    fn test_from_profile_standard() {
        use crate::profiles::HF_STANDARD;
        let config = EngineConfig::from_profile(&HF_STANDARD);
        assert_eq!(config.speed_level, 2);
        assert!(config.compression_enabled);
    }

    #[test]
    fn test_default_cp_mode_is_long_cp() {
        let config = EngineConfig::default();
        assert_eq!(
            config.cp_mode,
            coppa_protocol::cp_negotiator::CpMode::LongCp
        );
    }
}
