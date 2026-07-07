use std::collections::HashMap;

use crate::fec::ldpc::LdpcCodec;
use crate::fec::scrambler::scramble;
use crate::modem::speed_levels::speed_level_components;
use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
use coppa_codec::ofdm::frame::CoppaHeader;
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_codec::traits::ConstellationMapper;

/// LDPC coded block length (Z=81, 24 base columns) — constant for all code rates.
const CODED_BLOCK_LEN: usize = 1944;

/// Cached per-speed-level components: building the LDPC codec, interleaver, and
/// constellation mapper is ~0.105 ms and ~4801 allocs (~525 KB) per call — expensive
/// enough that doing it on every single `transmit`/`receive` (as the pre-Task-7 code
/// did) shows up directly in the per-frame decode budget. All of these depend only
/// on the speed level + this transceiver's fixed profile, so they're built once, in
/// `CoppaTransceiver::new`, for all 9 levels.
struct LevelComponents {
    codec: LdpcCodec,
    interleaver: BlockInterleaver,
    mapper: Box<dyn ConstellationMapper + Send>,
}

pub struct CoppaTransceiver {
    modem: CoppaModem,
    profile: CoppaProfile,
    /// RX-side SSB-audio-band bandpass (250-2850 Hz), mirroring the TX bandpass
    /// already applied in `CoppaModem::modulate_mapped`. Only meaningful for HF
    /// profiles (`phy_mode == 0`) received through an SSB radio's audio chain;
    /// `None` for non-HF profiles (see `CoppaModem::tx_bpf`'s doc for why VHF's
    /// wider carrier band and shorter cyclic prefix are incompatible with this
    /// filter's passband and 300-sample group delay). Reuses the exact same
    /// 601-tap / 250-2850 Hz design already verified by
    /// `coppa_dsp::fir::tests::bandpass_rejects_out_of_band_tones` (>=30 dB
    /// attenuation at 100 Hz and 4 kHz, flat passband at 500 Hz), so no new
    /// tap-count derivation is needed for this filter specifically.
    rx_bpf: Option<coppa_dsp::fir::Fir>,
    /// Per-speed-level cached codec/interleaver/mapper, built eagerly for all 9
    /// levels in `new` (see `LevelComponents`'s doc).
    codecs: HashMap<u8, LevelComponents>,
}

#[derive(Debug)]
pub enum ReceiveError {
    SyncFailed,
    HeaderCorrupt,
    LdpcNotConverged { iterations: usize },
    CrcMismatch,
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SyncFailed => write!(f, "preamble synchronization failed"),
            Self::HeaderCorrupt => write!(f, "header could not be parsed"),
            Self::LdpcNotConverged { iterations } => {
                write!(
                    f,
                    "LDPC decoder did not converge after {} iterations",
                    iterations
                )
            }
            Self::CrcMismatch => write!(f, "CRC mismatch on decoded payload"),
        }
    }
}

impl std::error::Error for ReceiveError {}

impl CoppaTransceiver {
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        // phy_mode 0 = HF/SSB; mirrors the TX bandpass gate in `CoppaModem::new`.
        let rx_bpf = (profile.phy_mode == 0).then(|| {
            coppa_dsp::fir::Fir::new(coppa_dsp::fir::design_bandpass(
                601,
                profile.sample_rate as f32,
                250.0,
                2850.0,
            ))
        });
        let modem = CoppaModem::new(profile.clone(), version);

        // Eagerly build every speed level's codec/interleaver/mapper (see
        // `LevelComponents`'s doc). Reserved/invalid levels (e.g. 8) simply have no
        // entry in the map; `transmit`/`receive` treat a missing entry the same way
        // the old per-call `speed_level_components` lookup treated an `Err`.
        let mut codecs = HashMap::with_capacity(SPEED_LEVELS.len());
        for sl in SPEED_LEVELS.iter() {
            if let Ok((mapper, code_rate)) = speed_level_components(sl.level) {
                let codec = LdpcCodec::new(code_rate);
                let interleaver = BlockInterleaver::new(CODED_BLOCK_LEN, profile.data_carriers);
                codecs.insert(
                    sl.level,
                    LevelComponents {
                        codec,
                        interleaver,
                        mapper,
                    },
                );
            }
        }

        Self {
            modem,
            profile,
            rx_bpf,
            codecs,
        }
    }

    /// The OFDM profile this transceiver was built for.
    pub fn profile(&self) -> &CoppaProfile {
        &self.profile
    }

    pub fn transmit(&self, header: &CoppaHeader, payload: &[u8]) -> Vec<f32> {
        let comp = self
            .codecs
            .get(&header.speed_level)
            .expect("invalid speed level in header");

        // 1. LDPC encode
        let info_bits = comp.codec.code().info_bits();
        let mut payload_bits = Vec::with_capacity(info_bits);
        for &byte in payload {
            for shift in (0..8).rev() {
                payload_bits.push((byte >> shift) & 1);
            }
        }
        payload_bits.resize(info_bits, 0u8);
        // Scramble info bits to randomize zero-padding (prevents degenerate LDPC codewords)
        scramble(&mut payload_bits);
        let coded_bits = comp.codec.encode(&payload_bits);

        // 2. Interleave
        let interleaved = comp.interleaver.interleave(&coded_bits);

        // 3. Constellation map
        let symbols = comp.mapper.map_bits(&interleaved);

        // 4. OFDM modulate
        // Look up PAPR target from speed level table
        let sl = SPEED_LEVELS
            .iter()
            .find(|s| s.level == header.speed_level)
            .expect("invalid speed level in header");
        self.modem
            .modulate_mapped(header, &symbols, sl.papr_target_db)
    }

    /// Peek at just the header of a buffered candidate window (samples starting at,
    /// or shortly before, a frame's preamble), without demodulating the payload.
    /// Applies the same RX bandpass `receive` does (HF profiles only) before
    /// delegating to [`CoppaModem::demodulate_header`] — see that method's doc for
    /// why no CFO correction is applied here.
    ///
    /// Unlike [`Self::receive_with_metrics`] (which re-derives its own timing via a
    /// fresh internal `SyncDetector::detect_all`, so tolerates arbitrary leading
    /// margin/silence before the frame), this takes an explicit `data_start` and
    /// does no timing search of its own — so the caller must ensure `samples`
    /// includes enough leading context before the header for this transceiver's
    /// one-shot block RX filter to settle before `data_start` (its group delay is
    /// 300 samples for the 601-tap HF filter; a slice starting exactly at the
    /// frame's preamble, with no leading context at all, shifts the correctly
    /// filtered header later by that much — see `StreamingReceiver::header_peek`
    /// in `coppa-protocol::modem::streaming`, its only caller, for how it handles
    /// this).
    pub fn demodulate_header(&self, samples: &[f32], data_start: usize) -> Option<CoppaHeader> {
        let filtered;
        let samples: &[f32] = match &self.rx_bpf {
            Some(bpf) => {
                filtered = bpf.filter_block(samples);
                &filtered
            }
            None => samples,
        };
        self.modem.demodulate_header(samples, data_start)
    }

    pub fn receive(&self, samples: &[f32]) -> Result<(CoppaHeader, Vec<u8>), ReceiveError> {
        self.receive_with_metrics(samples)
            .map(|(h, p, _snr)| (h, p))
    }

    /// Like [`Self::receive`], but also returns the frame's SNR estimate (dB),
    /// derived from the per-carrier noise-variance estimates the payload equalizer
    /// already produces: `snr_db = 10*log10(1 / mean(noise_vars))`. Added for
    /// `StreamingReceiver`'s `DecodedFrame::snr_db` (Task 7), so the daemon can feed
    /// the rate controller a real per-carrier-noise SNR instead of the crude
    /// whole-buffer RMS proxy it used before (`20*log10(rms) + 40`, flagged
    /// elsewhere as a known hack). `receive` itself is unchanged and still used by
    /// every existing (batch) call site.
    pub fn receive_with_metrics(
        &self,
        samples: &[f32],
    ) -> Result<(CoppaHeader, Vec<u8>, f32), ReceiveError> {
        // 0. RX bandpass: reject out-of-passband noise/interference before demod, mirroring
        // the TX bandpass already applied at modulate time (HF profiles only).
        let filtered;
        let samples: &[f32] = match &self.rx_bpf {
            Some(bpf) => {
                filtered = bpf.filter_block(samples);
                &filtered
            }
            None => samples,
        };

        // 1. Demodulate to soft symbols (coded symbol count derived from header speed level)
        let (header, eq_symbols, noise_vars) = self
            .modem
            .demodulate_frame(samples)
            .ok_or(ReceiveError::SyncFailed)?;

        // 2. Resolve speed level components
        let comp = self
            .codecs
            .get(&header.speed_level)
            .ok_or(ReceiveError::HeaderCorrupt)?;

        // 3. Soft demap: convert equalized symbols to LLRs
        let bps = comp.mapper.bits_per_symbol();
        let coded_bits_needed: usize = CODED_BLOCK_LEN;
        let symbols_needed = coded_bits_needed.div_ceil(bps);
        let mut llrs = Vec::with_capacity(coded_bits_needed);

        for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
            let nv = if i < noise_vars.len() {
                noise_vars[i].max(0.001)
            } else {
                0.01
            };
            llrs.extend(comp.mapper.demap_soft(sym, nv));
        }
        llrs.truncate(coded_bits_needed);
        llrs.resize(coded_bits_needed, 0.0);

        // Clip LLR magnitudes to prevent numerical overflow in BP decoder
        let llr_clip = 20.0f32;
        for llr in &mut llrs {
            *llr = llr.clamp(-llr_clip, llr_clip);
        }

        // 4. De-interleave
        let deinterleaved = comp.interleaver.deinterleave(&llrs);

        // 5. LDPC decode
        let (mut decoded_bits, converged) = comp.codec.decode_checked(&deinterleaved);
        // Descramble to undo TX-side scrambling
        scramble(&mut decoded_bits);

        if !converged {
            return Err(ReceiveError::LdpcNotConverged { iterations: 50 });
        }

        // 6. Extract payload bytes
        let payload_len = header.payload_len as usize;
        let mut payload = Vec::with_capacity(payload_len);
        for chunk in decoded_bits.chunks(8) {
            if chunk.len() == 8 && payload.len() < payload_len {
                let mut byte = 0u8;
                for (i, &bit) in chunk.iter().enumerate() {
                    byte |= (bit & 1) << (7 - i);
                }
                payload.push(byte);
            }
        }

        // SNR estimate from the same per-carrier noise variances used for LLR scaling.
        let mean_nv = if noise_vars.is_empty() {
            1.0
        } else {
            noise_vars.iter().sum::<f32>() / noise_vars.len() as f32
        };
        let snr_db = 10.0 * (1.0 / mean_nv.max(1e-6)).log10();

        Ok((header, payload, snr_db))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::frame::CoppaFrameType;

    /// Regression test: VHF-routed speed levels (5,6,7,9,10 via `select_profile` in
    /// coppa-bench, which chooses `vhf_wide` for level >= 5) previously fell back to
    /// an unconditioned TX path that never leveled the preamble against the much
    /// quieter header/payload body, leaving the transmitted peak above full scale
    /// (measured ~1.026 before the fix) and the payload badly underpowered relative
    /// to the whole-frame mean power any AWGN budget is referenced to. Exercise many
    /// random payloads through the full `CoppaTransceiver` (LDPC + interleave +
    /// mapping) at a VHF speed level with zero channel impairment.
    #[test]
    fn vhf_level5_transceiver_round_trips_with_bounded_peak() {
        use coppa_codec::ofdm::CoppaProfile;
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        let profile = CoppaProfile::vhf_wide();
        let tx = CoppaTransceiver::new(profile, 1);
        let mut ok_count = 0;
        for trial in 0..20u64 {
            let seed = 0xABCDu64.wrapping_add(trial);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload_bytes = 130usize; // level 5 payload size per bench MODES
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
            let header = CoppaHeader {
                version: 1,
                phy_mode: 0,
                frame_type: CoppaFrameType::Data,
                bandwidth: 1,
                fec_type: 0,
                speed_level: 5,
                seq_num: 0,
                payload_len: payload_bytes as u16,
            };
            let clean = tx.transmit(&header, &payload);
            let peak = clean.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                peak <= 0.5001,
                "trial {trial}: TX peak must be normalized to ~0.5 FS, got {peak}"
            );
            let result = tx.receive(&clean);
            if result.is_ok() {
                ok_count += 1;
            }
        }
        assert!(
            ok_count == 20,
            "all 20 clean-channel VHF trials should decode, got {ok_count}/20"
        );
    }

    fn make_header(speed_level: u8, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level,
            seq_num: 0,
            payload_len,
        }
    }

    #[test]
    fn loopback_survives_ssb_filter_and_50hz_mistune() {
        // The bar Phase 1 exists to clear: a real radio's passband + a realistic mistune.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xA7u8; 100];
        let header = make_header(2, payload.len() as u16);
        let s = tx.transmit(&header, &payload);
        let through_rig = coppa_channel::ssb_filter(&s, 48_000.0);
        let mistuned = coppa_channel::frequency_shift(&through_rig, 47.0, 48_000.0);
        let (_h, rx) = tx
            .receive(&mistuned)
            .expect("must decode through SSB filter + 47 Hz CFO");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn loopback_survives_ssb_filter_only() {
        // Same as above but with 0 Hz mistune: this is the part of the bar this task must
        // clear now (CFO correction on this mistuned path lands in Task 6).
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xA7u8; 100];
        let header = make_header(2, payload.len() as u16);
        let s = tx.transmit(&header, &payload);
        let through_rig = coppa_channel::ssb_filter(&s, 48_000.0);
        let untuned = coppa_channel::frequency_shift(&through_rig, 0.0, 48_000.0);
        let (_h, rx) = tx
            .receive(&untuned)
            .expect("must decode through SSB filter alone (no mistune)");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_cfo_correction() {
        // An 8 Hz CFO collapses the link without correction; the RX must estimate + remove it.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = b"CFO correction works";
        let header = make_header(2, payload.len() as u16);
        let samples = tx.transmit(&header, payload);
        let injected = coppa_codec::ofdm::sync::remove_cfo(&samples, -8.0, 48_000.0); // +8 Hz
        let (_h, rx) = tx
            .receive(&injected)
            .expect("should recover after CFO correction");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    /// Regression test for the sync timing-anchor fix in
    /// `docs/adr/004-strongest-path-timing.md`: `hf_standard`'s sparse (4-)pilot
    /// protected header must survive Watterson-Moderate fading at a level (1,
    /// BPSK 1/4) and SNR (21 dB) that pre-Phase-1 measurements
    /// (`results/rebaseline-2026-07/moderate.csv`) clear comfortably. The bug this
    /// guards against (`SyncDetector` anchoring on a weak-but-earliest multipath
    /// tap instead of the strongest one) floored this exact scenario at a ~65-70%
    /// success rate regardless of SNR; the 80% bar is comfortably below normal
    /// trial-to-trial variance at this operating point and well above that floor.
    #[test]
    fn hf_standard_header_survives_watterson_moderate_fading() {
        use coppa_channel::watterson::WattersonPreset;

        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0x5Au8; 20];
        let header = make_header(1, payload.len() as u16); // level 1 = BPSK 1/4
        let clean = tx.transmit(&header, &payload);

        const TRIALS: u64 = 30;
        let mut ok = 0u64;
        for trial in 0..TRIALS {
            let seed = 0xFADE_0000u64.wrapping_add(trial);
            let faded = coppa_channel::watterson::watterson(
                &clean,
                48_000.0,
                &WattersonPreset::Moderate.config(),
                seed,
            );
            let noisy = coppa_channel::awgn_seeded(&faded, 21.0, seed ^ 0x55AA);
            if matches!(tx.receive(&noisy), Ok((_, rx)) if rx[..payload.len()] == payload[..]) {
                ok += 1;
            }
        }
        assert!(
            ok * 100 >= TRIALS * 80,
            "hf_standard level-1 header should survive Watterson-Moderate fading at 21 dB in \
             the large majority of trials, got {ok}/{TRIALS} -- if this regresses, check the \
             sync timing anchor policy (docs/adr/004-strongest-path-timing.md)"
        );
    }

    #[test]
    fn test_transceiver_bpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = b"Hello Phase C!";
        let header = make_header(2, payload.len() as u16);

        let samples = tx.transmit(&header, payload);
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, header.speed_level);
        assert_eq!(rx_header.payload_len, header.payload_len);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_qpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let header = make_header(3, payload.len() as u16);

        let samples = tx.transmit(&header, payload.as_slice());
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 3);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_16qam_rate_1_2_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0x42u8; 100];
        let header = make_header(6, payload.len() as u16);

        let samples = tx.transmit(&header, payload.as_slice());
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 6);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_16qam_survives_flat_gain() {
        // A flat channel gain (0.5) shrinks the constellation; MMSE leaves the equalized symbols
        // at the wrong amplitude and 16QAM mis-decodes. Gain-normalization (Y/H) must restore it.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x3Cu8; 40];
        let header = make_header(6, payload.len() as u16); // 16QAM 1/2
        let mut samples = tx.transmit(&header, payload.as_slice());
        for s in samples.iter_mut() {
            *s *= 0.5; // flat channel gain
        }
        let (_h, rx) = tx
            .receive(&samples)
            .expect("16QAM should survive a flat 0.5 gain");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_hf_robust_bpsk_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = b"Hello robust HF profile!";
        let header = make_header(2, payload.len() as u16); // BPSK 1/2
        let samples = tx.transmit(&header, payload);
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("hf_robust decode should succeed");
        assert_eq!(rx_header.speed_level, 2);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_hf_robust_qpsk_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x5Au8; 60];
        let header = make_header(3, payload.len() as u16); // QPSK 1/2
        let samples = tx.transmit(&header, payload.as_slice());
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("hf_robust QPSK decode should succeed");
        assert_eq!(rx_header.speed_level, 3);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_header_survives_bit_errors_in_header_region() {
        // A few sign flips in the header OFDM symbols used to corrupt the frame
        // (unprotected header). With Golay+CRC protection the header must recover.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x5Au8; 40];
        let header = make_header(2, payload.len() as u16); // BPSK 1/2
        let mut samples = tx.transmit(&header, payload.as_slice());
        // Perturb a handful of samples inside the header region (after preamble +
        // fine-sync = 3 symbols). One robust symbol = fft_size + cp = 1260 samples.
        let sym = 1260;
        let header_start = 3 * sym;
        for i in 0..8 {
            let idx = header_start + i * 37;
            if idx < samples.len() {
                samples[idx] += 0.15; // small additive perturbation
            }
        }
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("protected header should recover from small perturbations");
        assert_eq!(rx_header.speed_level, 2);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }
}
