//! Profile constructors used by the level-9 geometry/CP diagnostic.

use coppa_codec::ofdm::CoppaProfile;

/// VHF-wide geometry with only the cyclic prefix changed to the HF-standard length.
pub fn vhf_wide_long_cp() -> CoppaProfile {
    CoppaProfile {
        cp_samples: 300,
        ..CoppaProfile::vhf_wide()
    }
}

/// The five profile arms used by the level-9 A/B measurement.
pub fn profile_arms() -> impl Iterator<Item = (&'static str, CoppaProfile)> {
    [
        ("vhf_wide", CoppaProfile::vhf_wide()),
        ("vhf_wide_long_cp", vhf_wide_long_cp()),
        ("vhf_narrow", CoppaProfile::vhf_narrow()),
        ("hf_standard", CoppaProfile::hf_standard()),
        ("hf_robust", CoppaProfile::hf_robust()),
    ]
    .into_iter()
}

/// Number of rate-matched bits carried by a single current-format frame.
///
/// Profile geometry changes symbol count and airtime, not the FEC wire block.
pub fn coded_bits_for(level: u8, _profile: &CoppaProfile) -> usize {
    assert!(coppa_protocol::modem::speed_levels::k_used_for_level(level).is_some());
    1944
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp_isolated_arm_differs_from_vhf_wide_only_in_cp() {
        let base = coppa_codec::ofdm::CoppaProfile::vhf_wide();
        let arm = vhf_wide_long_cp();
        assert_eq!(arm.cp_samples, 300);
        assert_ne!(arm.cp_samples, base.cp_samples);
        assert_eq!(arm.fft_size, base.fft_size);
        assert_eq!(arm.data_carriers, base.data_carriers);
        assert_eq!(arm.pilot_carriers, base.pilot_carriers);
        assert_eq!(arm.phy_mode, base.phy_mode);
        assert_eq!(arm.bandwidth_id, base.bandwidth_id);
        assert_eq!(arm.carrier_offset, base.carrier_offset);
        assert_eq!(arm.sample_rate, base.sample_rate);
    }

    #[test]
    fn level_9_information_content_is_profile_independent() {
        use coppa_protocol::modem::speed_levels::{k_used_for_level, max_payload_for_level};
        assert_eq!(k_used_for_level(9), Some(1296));
        assert_eq!(max_payload_for_level(9), Some(158));
        for profile in profile_arms().map(|(_, profile)| profile) {
            assert_eq!(coded_bits_for(9, &profile), 1944);
        }
    }

    #[test]
    fn cp_covers_watterson_second_tap_only_on_long_cp_profiles() {
        let wide = coppa_codec::ofdm::CoppaProfile::vhf_wide();
        let standard = coppa_codec::ofdm::CoppaProfile::hf_standard();
        assert_eq!(wide.cp_samples, 60);
        assert_eq!(standard.cp_samples, 300);
        assert!(96 > wide.cp_samples);
        assert!(96 < standard.cp_samples);
    }
}
