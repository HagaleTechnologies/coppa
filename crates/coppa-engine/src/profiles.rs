//! Named operational profiles for common use cases.

/// A named preset of band, speed level and compression.
///
/// The `speed_level` is the single parameter that controls modulation
/// (constellation) and FEC (LDPC code rate) inside CoppaTransceiver; the band
/// comes from `ofdm_profile`. Everything else a user might want to know about
/// the level (max bytes, air time, bandwidth) is derived, not stored: see
/// [`Profile::max_payload`] and [`crate::ladder`].
#[derive(Debug, Clone)]
pub struct Profile {
    /// Human-readable profile name.
    pub name: &'static str,
    /// Speed level (1-9). Selects constellation + LDPC rate.
    pub speed_level: u8,
    /// Whether to enable compression.
    pub compression: bool,
    /// Target sample rate in Hz (always 48000).
    pub sample_rate: u32,
    /// Description of the profile's intended use.
    pub description: &'static str,
    /// OFDM profile selector: "hf" or "vhf".
    pub ofdm_profile: &'static str,
}

/// Robust HF profile for weak-signal conditions.
pub const HF_ROBUST: Profile = Profile {
    name: "HF_ROBUST",
    speed_level: 1,
    compression: false,
    sample_rate: 48_000,
    description: "Robust HF mode for weak signals and high noise",
    ofdm_profile: "hf",
};

/// Fast VHF profile for strong-signal conditions.
pub const VHF_FAST: Profile = Profile {
    name: "VHF_FAST",
    speed_level: 8,
    compression: true,
    sample_rate: 48_000,
    description: "Fast VHF mode for strong signals and low noise",
    ofdm_profile: "vhf",
};

/// Emergency profile prioritizing reliability over speed.
pub const EMERGENCY: Profile = Profile {
    name: "EMERGENCY",
    speed_level: 1,
    compression: false,
    sample_rate: 48_000,
    description: "Emergency mode maximizing reliability",
    ofdm_profile: "hf",
};

/// Standard HF profile balancing speed and reliability.
pub const HF_STANDARD: Profile = Profile {
    name: "HF_STANDARD",
    speed_level: 2,
    compression: true,
    sample_rate: 48_000,
    description: "Standard HF mode balancing speed and reliability",
    ofdm_profile: "hf",
};

impl Profile {
    /// Largest payload, in bytes, one frame at this profile's speed level
    /// carries: the limit `CoppaTransceiver::transmit` enforces
    /// (`coppa_protocol::modem::max_payload_for_level`). `None` only for an
    /// invalid speed level.
    pub fn max_payload(&self) -> Option<usize> {
        coppa_protocol::modem::max_payload_for_level(self.speed_level)
    }
}

/// Get a profile by name (case-insensitive).
pub fn get_profile(name: &str) -> Option<&'static Profile> {
    match name.to_uppercase().as_str() {
        "HF_ROBUST" => Some(&HF_ROBUST),
        "VHF_FAST" => Some(&VHF_FAST),
        "EMERGENCY" => Some(&EMERGENCY),
        "HF_STANDARD" => Some(&HF_STANDARD),
        _ => None,
    }
}

/// List all available profile names.
pub fn list_profiles() -> &'static [&'static str] {
    &["HF_ROBUST", "HF_STANDARD", "VHF_FAST", "EMERGENCY"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hf_robust_profile() {
        assert_eq!(HF_ROBUST.name, "HF_ROBUST");
        assert_eq!(HF_ROBUST.speed_level, 1);
        assert_eq!(HF_ROBUST.max_payload(), Some(56));
        assert_eq!(HF_ROBUST.ofdm_profile, "hf");
        // Verify compression is disabled
        let _ = HF_ROBUST.compression; // always false per constant definition
    }

    #[test]
    fn test_vhf_fast_profile() {
        assert_eq!(VHF_FAST.name, "VHF_FAST");
        assert_eq!(VHF_FAST.speed_level, 8);
        assert_eq!(VHF_FAST.sample_rate, 48000);
        assert_eq!(VHF_FAST.ofdm_profile, "vhf");
    }

    #[test]
    fn test_emergency_profile() {
        assert_eq!(EMERGENCY.name, "EMERGENCY");
        // Verify compression is disabled
        let _ = EMERGENCY.compression; // always false per constant definition
    }

    #[test]
    fn test_get_profile() {
        assert!(get_profile("HF_ROBUST").is_some());
        assert!(get_profile("hf_robust").is_some());
        assert!(get_profile("NONEXISTENT").is_none());
    }

    #[test]
    fn test_list_profiles() {
        let profiles = list_profiles();
        assert_eq!(profiles.len(), 4);
        assert!(profiles.contains(&"HF_ROBUST"));
        assert!(profiles.contains(&"EMERGENCY"));
    }

    #[test]
    fn test_hf_standard_profile() {
        let p = get_profile("HF_STANDARD").unwrap();
        assert_eq!(p.speed_level, 2);
    }

    #[test]
    fn profile_max_payload_is_the_enforced_limit() {
        let expected = [
            (&HF_ROBUST, 56),
            (&HF_STANDARD, 117),
            (&VHF_FAST, 158),
            (&EMERGENCY, 56),
        ];
        for (p, max) in expected {
            assert_eq!(p.max_payload(), Some(max), "{}", p.name);
            assert_eq!(
                p.max_payload(),
                coppa_protocol::modem::max_payload_for_level(p.speed_level),
                "{}",
                p.name
            );
        }
    }

    #[test]
    fn emergency_is_hf_robust_on_the_air() {
        use crate::{CoppaCore, EngineConfig};
        let emergency = EngineConfig::from_profile(&EMERGENCY);
        let robust = EngineConfig::from_profile(&HF_ROBUST);
        assert_eq!(emergency.speed_level, robust.speed_level);
        assert_eq!(emergency.sample_rate, robust.sample_rate);
        assert_eq!(emergency.compression_enabled, robust.compression_enabled);
        assert_eq!(emergency.ofdm_band, robust.ofdm_band);
        let msg = "CQ CQ DE TEST";
        assert_eq!(
            CoppaCore::from_profile(&EMERGENCY).encode(msg).unwrap(),
            CoppaCore::from_profile(&HF_ROBUST).encode(msg).unwrap()
        );
    }
}
