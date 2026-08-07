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
/// `select_profile` rule"; `"standard"`/`"standard-short-cp"`/`"robust"` force that profile for
/// every level.
///
/// `"standard-short-cp"` is `hf_standard`'s cyclic-prefix variant (`cp_samples: 144` vs. `300`,
/// every other PHY field identical). It is the ONLY pair in this workspace that isolates CP
/// length from carrier layout, which is why a CP contrast has to be based on `"standard"` and
/// not on `"robust"` (36 data / 12 pilot, no short-CP twin anywhere) -- see COP-2's plan, D1.
/// Named for the base profile it varies rather than a bare `"short-cp"` so it stays unambiguous
/// if a second short-CP profile is ever added.
pub fn profile_by_name(name: &str) -> Option<CoppaProfile> {
    match name {
        "default" => None,
        "standard" => Some(CoppaProfile::hf_standard()),
        "standard-short-cp" => Some(CoppaProfile::hf_standard_short_cp()),
        "robust" => Some(CoppaProfile::hf_robust()),
        other => panic!(
            "unknown profile '{other}' (expected: default|standard|standard-short-cp|robust)"
        ),
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
    /// Relative receiver sampling-clock offset in ppm; `0.0` disables it.
    /// Uses [`coppa_channel::sample_clock_offset`]'s signed-rate convention.
    pub sco_ppm: f32,
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

    /// Pins COP-2's D1: `"standard-short-cp"` must stay the CP-only variant of
    /// `"standard"`. If a future edit re-points the name at a differently-shaped
    /// profile (different carrier layout, FFT size, or phy_mode), the CP contrast
    /// it exists to serve stops being a CP contrast, and this test fails.
    #[test]
    fn standard_short_cp_resolves_and_differs_from_standard_only_in_cp_length() {
        let long = profile_by_name("standard").expect("standard is an override, not the default");
        let short = profile_by_name("standard-short-cp")
            .expect("standard-short-cp is an override, not the default");

        // The two fields that differ, and only these two.
        assert_eq!(long.cp_samples, 300);
        assert_eq!(short.cp_samples, 144);
        assert_eq!(long.bandwidth_id, 1);
        assert_eq!(short.bandwidth_id, 4);

        // Everything the carrier layout / waveform geometry depends on is identical.
        assert_eq!(short.fft_size, long.fft_size);
        assert_eq!(short.sample_rate, long.sample_rate);
        assert_eq!(short.data_carriers, long.data_carriers);
        assert_eq!(short.pilot_carriers, long.pilot_carriers);
        assert_eq!(short.phy_mode, long.phy_mode);
        assert_eq!(short.carrier_offset, long.carrier_offset);
    }

    /// The airtime saving is an exact constant factor, because `frame_airtime_s`
    /// factorizes as `total_syms(level) * (fft_size + cp_samples) / sample_rate`
    /// and `total_syms` depends on the profile only through
    /// `data_carriers_per_symbol` -- identical for these two profiles. So the CP
    /// enters as one divisor: 1104/1260 exactly, at every level.
    #[test]
    fn standard_short_cp_costs_less_airtime_at_identical_carrier_layout() {
        let long = profile_by_name("standard").expect("standard is an override");
        let short = profile_by_name("standard-short-cp").expect("standard-short-cp is an override");

        let t_long = coppa_protocol::modem::frame_airtime_s(2, &long).expect("level 2 is valid");
        let t_short = coppa_protocol::modem::frame_airtime_s(2, &short).expect("level 2 is valid");

        assert!(
            t_short < t_long,
            "short CP must cost less airtime: {t_short} vs {t_long}"
        );
        assert!(
            ((t_short / t_long) - 1104.0 / 1260.0).abs() < 1e-12,
            "expected the exact (960+144)/(960+300) symbol-length ratio, got {}",
            t_short / t_long
        );
    }

    /// A near-miss name must still be refused, with the *full* list of valid
    /// names -- so a typo cannot silently fall back to a plausible-looking wrong
    /// profile. `"hf_standard_short_cp"` is the plausible wrong guess (the Rust
    /// constructor's name rather than the kebab-case CLI name).
    #[test]
    #[should_panic(expected = "default|standard|standard-short-cp|robust")]
    fn unknown_profile_name_panic_lists_the_short_cp_name() {
        let _ = profile_by_name("hf_standard_short_cp");
    }

    /// Regression lock for the ten existing `profile_by_name("robust")` call
    /// sites (and `"standard"`/`"default"`): adding a name must not perturb the
    /// ones already calibrated against. Passes before the short-CP arm is added
    /// too -- it is a lock, not a Red test.
    #[test]
    fn existing_profile_names_are_unchanged() {
        assert!(
            profile_by_name("default").is_none(),
            "\"default\" means per-level select_profile, i.e. no override"
        );

        let standard = profile_by_name("standard").expect("standard is an override");
        assert_eq!(standard.cp_samples, 300);
        assert_eq!(standard.data_carriers, 44);
        assert_eq!(standard.bandwidth_id, 1);

        let robust = profile_by_name("robust").expect("robust is an override");
        assert_eq!(robust.pilot_carriers, 12);
        assert_eq!(robust.bandwidth_id, 3);
        assert_eq!(robust.cp_samples, 300);
    }
}
