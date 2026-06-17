//! Configuration types for the Coppa engine.

use crate::profiles::Profile;

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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            speed_level: 1,
            sample_rate: 48_000,
            compression_enabled: false,
            squelch_threshold_db: f32::NEG_INFINITY,
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
}
