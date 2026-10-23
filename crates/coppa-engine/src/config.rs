//! Configuration types for the Coppa engine.

use crate::profiles::Profile;
use coppa_protocol::cp_negotiator::CpMode;

/// Which OFDM band an engine's waveform is bound to.
///
/// COP-23: before this existed, `CoppaCore::select_ofdm_profile` derived the band
/// from the speed level alone on every rebuild, so an engine built from an HF
/// [`Profile`] silently went on air as `vhf_wide` the moment `RateLoop` raised it
/// to level 5 (and a VHF engine silently dropped to HF below level 5). A named
/// profile now pins its band here, and every rebuild honors it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfdmBand {
    /// Historical rule: levels 1-4 HF, >= 5 VHF. The `EngineConfig::default()`
    /// value -- an engine with no operator band preference keeps today's behavior.
    LevelDerived,
    /// Stay on HF profiles at every speed level.
    Hf,
    /// Stay on VHF profiles at every speed level.
    Vhf,
}

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
    /// The **negotiated** CP profile, which is in effect whenever the engine is
    /// on an HF profile and merely *dormant* while it is on the VHF one (where no
    /// short-CP variant exists and `CoppaCore::select_ofdm_profile` ignores this
    /// field entirely). Which of the two applies is decided by `ofdm_band`: an
    /// `OfdmBand::Hf` engine keeps this mode in effect at every speed level, an
    /// `OfdmBand::Vhf` engine never uses it, and an `OfdmBand::LevelDerived`
    /// engine uses it below level 5 only. Crossing into VHF does not modify it,
    /// and returning to an HF profile therefore restores the negotiated profile
    /// automatically -- suspended, not lost.
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
    /// Consequence worth knowing: while `CoppaCore::on_vhf_profile()` is true this
    /// field describes the peer agreement, NOT the waveform on air. See
    /// `CoppaCore::set_speed_level`/`CoppaCore::cp_mode`'s docs,
    /// `coppa_protocol::cp_negotiator`, and
    /// `docs/superpowers/specs/2026-07-29-cp-switch-peer-negotiation-design.md`.
    /// The HF<->VHF *profile* desync (a `RateLoop` climb across the boundary
    /// put this station on `vhf_wide` while the peer was still on an HF profile)
    /// is closed by COP-23 for every engine whose `ofdm_band` is `Hf` or `Vhf`,
    /// which includes every engine built from a named [`Profile`]. The residual
    /// is `OfdmBand::LevelDerived` engines (no band preference), and the absence
    /// of any RF-frequency gate: an operator who configures a VHF profile while
    /// physically on HF is still unprotected (H-027/H-150).
    pub cp_mode: CpMode,
    /// The OFDM band this engine's waveform is bound to (COP-23). A named
    /// [`Profile`] pins it (`EngineConfig::from_profile`); the default,
    /// `OfdmBand::LevelDerived`, keeps the historical level-only rule.
    pub ofdm_band: OfdmBand,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            speed_level: 1,
            sample_rate: 48_000,
            compression_enabled: false,
            squelch_threshold_db: f32::NEG_INFINITY,
            cp_mode: CpMode::LongCp,
            ofdm_band: OfdmBand::LevelDerived,
        }
    }
}

impl EngineConfig {
    /// Create a config from a named profile.
    ///
    /// The profile's `ofdm_profile` ("hf" or "vhf") pins `ofdm_band`, so the band
    /// survives every later speed-level change; anything other than "vhf" is HF,
    /// matching `CoppaCore::from_profile`'s historical fallback.
    pub fn from_profile(profile: &Profile) -> Self {
        let ofdm_band = match profile.ofdm_profile {
            "vhf" => OfdmBand::Vhf,
            _ => OfdmBand::Hf,
        };
        Self {
            speed_level: profile.speed_level,
            sample_rate: profile.sample_rate,
            compression_enabled: profile.compression,
            squelch_threshold_db: f32::NEG_INFINITY,
            cp_mode: CpMode::LongCp,
            ofdm_band,
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
    fn engine_config_from_profile_carries_the_band() {
        use crate::profiles::{EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};
        assert_eq!(
            EngineConfig::from_profile(&HF_ROBUST).ofdm_band,
            OfdmBand::Hf
        );
        assert_eq!(
            EngineConfig::from_profile(&HF_STANDARD).ofdm_band,
            OfdmBand::Hf
        );
        assert_eq!(
            EngineConfig::from_profile(&EMERGENCY).ofdm_band,
            OfdmBand::Hf
        );
        assert_eq!(
            EngineConfig::from_profile(&VHF_FAST).ofdm_band,
            OfdmBand::Vhf
        );
        assert_eq!(
            EngineConfig::default().ofdm_band,
            OfdmBand::LevelDerived,
            "an engine with no operator band preference keeps the level-only rule"
        );
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
