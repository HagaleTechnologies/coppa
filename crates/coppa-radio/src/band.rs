//! Operating-frequency evidence and the HF/VHF wideband regulatory gate
//! (COP-40).
//!
//! 47 CFR 97.307(f)(3) (as amended by FCC 23-93, effective 2024-01-08)
//! caps amateur data emissions below 29.7 MHz at 2.8 kHz occupied
//! bandwidth. Coppa's `vhf_wide` profile occupies 5600 Hz, so it must never
//! go on air below 29.7 MHz. [`gate`] is the one predicate both live-transmit
//! frontends (`coppad`'s `transmit_samples` and `coppa tx`/`coppa tune`)
//! evaluate before keying: narrowband emissions are always allowed; wideband
//! ones only with frequency evidence at or above [`US_WIDEBAND_MIN_HZ`].
//!
//! The band table below is a minimal US amateur subset (160 m - 23 cm). The
//! tenant-wide band convention (dispensa ADR-0006) is the canonical source;
//! this table only has to answer "does this band lie wholly above 29.7 MHz".
//! The threshold is US-only; other jurisdictions are out of scope here.

/// Lowest frequency (Hz) at which an emission wider than
/// [`HF_OCCUPIED_BANDWIDTH_CAP_HZ`] is permitted (US, 47 CFR 97.307(f)(3)).
pub const US_WIDEBAND_MIN_HZ: u64 = 29_700_000;

/// Occupied-bandwidth cap (Hz) for data emissions below
/// [`US_WIDEBAND_MIN_HZ`] (47 CFR 97.307(f)(3), FCC 23-93).
pub const HF_OCCUPIED_BANDWIDTH_CAP_HZ: u32 = 2_800;

/// Highest dial frequency (Hz) a frequency setting may state; anything above
/// is a typo, not a plausible amateur dial frequency.
pub const MAX_PLAUSIBLE_FREQUENCY_HZ: u64 = 30_000_000_000;

/// One amateur band: a label and its inclusive edges in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub label: &'static str,
    pub low_hz: u64,
    pub high_hz: u64,
}

impl Band {
    /// Whether `hz` lies within this band's edges (inclusive).
    pub fn contains(&self, hz: u64) -> bool {
        (self.low_hz..=self.high_hz).contains(&hz)
    }
}

/// US amateur bands, 160 m through 23 cm.
pub const BANDS: &[Band] = &[
    Band {
        label: "160m",
        low_hz: 1_800_000,
        high_hz: 2_000_000,
    },
    Band {
        label: "80m",
        low_hz: 3_500_000,
        high_hz: 4_000_000,
    },
    Band {
        label: "60m",
        low_hz: 5_330_500,
        high_hz: 5_403_500,
    },
    Band {
        label: "40m",
        low_hz: 7_000_000,
        high_hz: 7_300_000,
    },
    Band {
        label: "30m",
        low_hz: 10_100_000,
        high_hz: 10_150_000,
    },
    Band {
        label: "20m",
        low_hz: 14_000_000,
        high_hz: 14_350_000,
    },
    Band {
        label: "17m",
        low_hz: 18_068_000,
        high_hz: 18_168_000,
    },
    Band {
        label: "15m",
        low_hz: 21_000_000,
        high_hz: 21_450_000,
    },
    Band {
        label: "12m",
        low_hz: 24_890_000,
        high_hz: 24_990_000,
    },
    Band {
        label: "10m",
        low_hz: 28_000_000,
        high_hz: 29_700_000,
    },
    Band {
        label: "6m",
        low_hz: 50_000_000,
        high_hz: 54_000_000,
    },
    Band {
        label: "2m",
        low_hz: 144_000_000,
        high_hz: 148_000_000,
    },
    Band {
        label: "1.25m",
        low_hz: 222_000_000,
        high_hz: 225_000_000,
    },
    Band {
        label: "70cm",
        low_hz: 420_000_000,
        high_hz: 450_000_000,
    },
    Band {
        label: "33cm",
        low_hz: 902_000_000,
        high_hz: 928_000_000,
    },
    Band {
        label: "23cm",
        low_hz: 1_240_000_000,
        high_hz: 1_300_000_000,
    },
];

/// Look up a band by label, case-insensitively and ignoring surrounding
/// whitespace (`"20m"`, `"2M"`, `" 70cm "`).
pub fn band_for_label(label: &str) -> Option<&'static Band> {
    let label = label.trim();
    BANDS.iter().find(|b| b.label.eq_ignore_ascii_case(label))
}

/// The band containing `hz`, if any. For logs only: [`gate`] always compares
/// raw Hz for live and stated frequencies.
pub fn band_for_frequency(hz: u64) -> Option<&'static Band> {
    BANDS.iter().find(|b| b.contains(hz))
}

/// What the station knows about its operating frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyEvidence {
    /// A fresh answer from the rig itself (rigctld `f`).
    Live(u64),
    /// A frequency the operator stated in config or on the command line.
    Stated(u64),
    /// A band the operator stated; only its lower edge is known to hold.
    Band(&'static Band),
}

impl FrequencyEvidence {
    /// The frequency the gate compares against the threshold: the raw value
    /// for `Live`/`Stated`, the lower edge for `Band`.
    pub fn gate_hz(&self) -> u64 {
        match self {
            FrequencyEvidence::Live(hz) | FrequencyEvidence::Stated(hz) => *hz,
            FrequencyEvidence::Band(band) => band.low_hz,
        }
    }

    /// Human-readable description of this evidence for logs and errors.
    pub fn describe(&self) -> String {
        match self {
            FrequencyEvidence::Live(hz) => format!("rigctld reports {hz} Hz"),
            FrequencyEvidence::Stated(hz) => format!("the stated frequency is {hz} Hz"),
            FrequencyEvidence::Band(band) => {
                format!("band \"{}\" starts at {} Hz", band.label, band.low_hz)
            }
        }
    }
}

/// Why [`gate`] refused a transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRefusal {
    /// Wideband emission and no frequency evidence at all.
    NoSource,
    /// Wideband emission and the evidence places the station below
    /// [`US_WIDEBAND_MIN_HZ`].
    BelowThreshold { evidence_hz: u64, occupied_hz: u32 },
}

/// The HF/VHF wideband regulatory gate.
///
/// An emission occupying at most [`HF_OCCUPIED_BANDWIDTH_CAP_HZ`] is legal at
/// any frequency and is always allowed, with or without evidence. A wider one
/// is allowed only when `evidence` places the station at or above
/// [`US_WIDEBAND_MIN_HZ`]; no evidence refuses (fail closed).
pub fn gate(occupied_hz: u32, evidence: Option<&FrequencyEvidence>) -> Result<(), GateRefusal> {
    if occupied_hz <= HF_OCCUPIED_BANDWIDTH_CAP_HZ {
        return Ok(());
    }
    let evidence = evidence.ok_or(GateRefusal::NoSource)?;
    let evidence_hz = evidence.gate_hz();
    if evidence_hz >= US_WIDEBAND_MIN_HZ {
        Ok(())
    } else {
        Err(GateRefusal::BelowThreshold {
            evidence_hz,
            occupied_hz,
        })
    }
}

/// A validated, operator-stated frequency source: an explicit dial frequency,
/// a band label, or both (consistent with each other).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrequencyConfig {
    pub stated_hz: Option<u64>,
    pub band: Option<&'static Band>,
}

impl FrequencyConfig {
    /// Validate a stated frequency and band label. `freq_key`/`band_key` name
    /// the setting in error messages (`"[radio] frequency_hz"`, `"--band"`).
    ///
    /// Errors on an unknown band label, a zero or implausibly high
    /// frequency, and a frequency that lies outside the stated band.
    pub fn parse(
        frequency_hz: Option<u64>,
        band: Option<&str>,
        freq_key: &str,
        band_key: &str,
    ) -> Result<Self, String> {
        let band = match band {
            None => None,
            Some(label) => Some(band_for_label(label).ok_or_else(|| {
                format!(
                    "invalid {band_key} \"{label}\": expected an amateur band label like \
                     \"20m\", \"2m\", or \"70cm\""
                )
            })?),
        };
        if let Some(hz) = frequency_hz {
            if hz == 0 {
                return Err(format!(
                    "invalid {freq_key} = 0: must be a positive dial frequency in Hz"
                ));
            }
            if hz > MAX_PLAUSIBLE_FREQUENCY_HZ {
                return Err(format!(
                    "invalid {freq_key} = {hz}: above {MAX_PLAUSIBLE_FREQUENCY_HZ} Hz, \
                     not a plausible dial frequency in Hz"
                ));
            }
            if let Some(b) = band {
                if !b.contains(hz) {
                    return Err(format!(
                        "{freq_key} = {hz} lies outside band \"{}\" ({}-{} Hz): set only \
                         one, or make them consistent",
                        b.label, b.low_hz, b.high_hz
                    ));
                }
            }
        }
        Ok(Self {
            stated_hz: frequency_hz,
            band,
        })
    }

    /// The best stated evidence: an explicit frequency over a band label.
    pub fn static_evidence(&self) -> Option<FrequencyEvidence> {
        match (self.stated_hz, self.band) {
            (Some(hz), _) => Some(FrequencyEvidence::Stated(hz)),
            (None, Some(band)) => Some(FrequencyEvidence::Band(band)),
            (None, None) => None,
        }
    }

    /// Whether `live_hz` contradicts the stated source (outside the stated
    /// band, or a different stated frequency's band).
    pub fn contradicts(&self, live_hz: u64) -> bool {
        match self.static_evidence() {
            None => false,
            Some(FrequencyEvidence::Band(band)) => !band.contains(live_hz),
            Some(FrequencyEvidence::Stated(hz) | FrequencyEvidence::Live(hz)) => {
                band_for_frequency(hz) != band_for_frequency(live_hz)
            }
        }
    }
}

/// Resolve evidence in precedence order: a live rig answer over any stated
/// source.
pub fn resolve_evidence(
    live_hz: Option<u64>,
    stated: &FrequencyConfig,
) -> Option<FrequencyEvidence> {
    live_hz
        .map(FrequencyEvidence::Live)
        .or_else(|| stated.static_evidence())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_labels_parse_case_insensitively() {
        assert_eq!(band_for_label("20m").unwrap().low_hz, 14_000_000);
        assert_eq!(band_for_label("2M").unwrap().label, "2m");
        assert_eq!(band_for_label("70CM").unwrap().label, "70cm");
        assert_eq!(band_for_label(" 6m ").unwrap().label, "6m");
    }

    #[test]
    fn unknown_band_label_is_none() {
        assert!(band_for_label("21m").is_none());
        assert!(band_for_label("").is_none());
    }

    #[test]
    fn band_for_frequency_maps_dial_frequencies() {
        assert_eq!(band_for_frequency(14_074_000).unwrap().label, "20m");
        assert_eq!(band_for_frequency(144_500_000).unwrap().label, "2m");
        assert_eq!(band_for_frequency(28_074_000).unwrap().label, "10m");
    }

    #[test]
    fn no_amateur_band_contains_29_8_mhz() {
        assert!(band_for_frequency(29_800_000).is_none());
    }

    #[test]
    fn narrowband_is_always_allowed() {
        assert_eq!(gate(2400, None), Ok(()));
        assert_eq!(gate(2800, None), Ok(()));
        assert_eq!(
            gate(2400, Some(&FrequencyEvidence::Live(14_074_000))),
            Ok(())
        );
    }

    #[test]
    fn wideband_with_no_evidence_is_refused() {
        assert_eq!(gate(5600, None), Err(GateRefusal::NoSource));
        assert_eq!(gate(2801, None), Err(GateRefusal::NoSource));
    }

    #[test]
    fn wideband_below_threshold_is_refused() {
        assert_eq!(
            gate(5600, Some(&FrequencyEvidence::Live(29_699_999))),
            Err(GateRefusal::BelowThreshold {
                evidence_hz: 29_699_999,
                occupied_hz: 5600
            })
        );
    }

    #[test]
    fn wideband_at_or_above_threshold_is_allowed() {
        assert_eq!(
            gate(5600, Some(&FrequencyEvidence::Live(29_700_000))),
            Ok(())
        );
        assert_eq!(
            gate(5600, Some(&FrequencyEvidence::Live(144_500_000))),
            Ok(())
        );
    }

    #[test]
    fn wideband_on_a_vhf_band_label_is_allowed() {
        let two_m = band_for_label("2m").unwrap();
        assert_eq!(gate(5600, Some(&FrequencyEvidence::Band(two_m))), Ok(()));
        let six_m = band_for_label("6m").unwrap();
        assert_eq!(gate(5600, Some(&FrequencyEvidence::Band(six_m))), Ok(()));
    }

    #[test]
    fn wideband_on_10m_is_refused() {
        let ten_m = band_for_label("10m").unwrap();
        assert_eq!(
            gate(5600, Some(&FrequencyEvidence::Band(ten_m))),
            Err(GateRefusal::BelowThreshold {
                evidence_hz: 28_000_000,
                occupied_hz: 5600
            })
        );
    }

    #[test]
    fn stated_frequency_behaves_like_live() {
        assert_eq!(
            gate(5600, Some(&FrequencyEvidence::Stated(144_500_000))),
            Ok(())
        );
        assert!(matches!(
            gate(5600, Some(&FrequencyEvidence::Stated(14_074_000))),
            Err(GateRefusal::BelowThreshold { .. })
        ));
    }

    /// The predicate's whole input space, enumerated: every band (by label
    /// and by its edge frequencies) crossed with a narrowband and a wideband
    /// emission, plus the no-evidence case.
    #[test]
    fn gate_truth_table_is_total() {
        for occupied in [2400u32, 2800, 2801, 5600] {
            let wide = occupied > HF_OCCUPIED_BANDWIDTH_CAP_HZ;
            assert_eq!(gate(occupied, None).is_ok(), !wide);
            for band in BANDS {
                let above = band.low_hz >= US_WIDEBAND_MIN_HZ;
                let by_band = gate(occupied, Some(&FrequencyEvidence::Band(band)));
                assert_eq!(by_band.is_ok(), !wide || above, "{} {occupied}", band.label);
                for hz in [band.low_hz, band.high_hz] {
                    for ev in [FrequencyEvidence::Live(hz), FrequencyEvidence::Stated(hz)] {
                        let ok = gate(occupied, Some(&ev)).is_ok();
                        assert_eq!(ok, !wide || hz >= US_WIDEBAND_MIN_HZ, "{ev:?} {occupied}");
                    }
                }
            }
        }
    }

    #[test]
    fn frequency_config_rejects_typos_and_contradictions() {
        let err = FrequencyConfig::parse(None, Some("21m"), "f", "[radio] band").unwrap_err();
        assert!(err.contains("expected an amateur band label"), "{err}");
        let err = FrequencyConfig::parse(Some(0), None, "[radio] frequency_hz", "b").unwrap_err();
        assert!(
            err.contains("must be a positive dial frequency in Hz"),
            "{err}"
        );
        let err = FrequencyConfig::parse(Some(30_000_000_001), None, "f", "b").unwrap_err();
        assert!(err.contains("not a plausible dial frequency"), "{err}");
        let err = FrequencyConfig::parse(Some(14_074_000), Some("2m"), "f", "b").unwrap_err();
        assert!(err.contains("lies outside band \"2m\""), "{err}");
        let ok = FrequencyConfig::parse(Some(144_500_000), Some("2M"), "f", "b").unwrap();
        assert_eq!(ok.stated_hz, Some(144_500_000));
        assert_eq!(ok.band.unwrap().label, "2m");
    }

    #[test]
    fn evidence_precedence_is_live_then_frequency_then_band() {
        let both = FrequencyConfig::parse(Some(144_500_000), Some("2m"), "f", "b").unwrap();
        assert_eq!(
            resolve_evidence(Some(14_074_000), &both),
            Some(FrequencyEvidence::Live(14_074_000))
        );
        assert_eq!(
            resolve_evidence(None, &both),
            Some(FrequencyEvidence::Stated(144_500_000))
        );
        let band_only = FrequencyConfig::parse(None, Some("2m"), "f", "b").unwrap();
        assert!(matches!(
            resolve_evidence(None, &band_only),
            Some(FrequencyEvidence::Band(b)) if b.label == "2m"
        ));
        assert_eq!(resolve_evidence(None, &FrequencyConfig::default()), None);
    }

    #[test]
    fn a_live_frequency_outside_the_stated_band_is_a_contradiction() {
        let two_m = FrequencyConfig::parse(None, Some("2m"), "f", "b").unwrap();
        assert!(two_m.contradicts(14_074_000));
        assert!(!two_m.contradicts(145_000_000));
        let stated = FrequencyConfig::parse(Some(144_500_000), None, "f", "b").unwrap();
        assert!(stated.contradicts(14_074_000));
        assert!(!stated.contradicts(146_520_000));
        assert!(!FrequencyConfig::default().contradicts(14_074_000));
    }
}
