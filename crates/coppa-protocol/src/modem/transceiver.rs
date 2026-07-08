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
    /// `ConstellationMapper` is only `: Send`, not `: Send + Sync` (see its
    /// definition in `coppa_codec::traits`), and `speed_level_components`
    /// returns `Box<dyn ConstellationMapper>` (no `Sync`). As a result,
    /// `CoppaTransceiver` — which embeds this cache — is intentionally
    /// `!Sync`. That's fine for `Send`-only use (no `Mutex`/similar needed to
    /// move it across a thread boundary), but a future caller cannot put a
    /// bare `CoppaTransceiver` behind `Arc<CoppaTransceiver>` for shared
    /// concurrent access without wrapping it in a `Mutex` (or similar) first.
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

/// Median of the per-carrier noise-variance estimates, used as the fallback
/// noise variance for carriers with a missing or degenerate (near-zero)
/// estimate -- see the call site in `receive_with_metrics` for why a
/// frame-local median is a better fallback than a fixed constant.
///
/// Returns `1.0` (a neutral value: neither artificially confident nor
/// artificially flat) if `noise_vars` is empty, since there is then no
/// frame-local data to derive a fallback from at all.
fn median_noise_variance(noise_vars: &[f32]) -> f32 {
    if noise_vars.is_empty() {
        return 1.0;
    }
    let mut sorted: Vec<f32> = noise_vars.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

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

    /// Data (non-pilot) carriers per OFDM symbol, as computed internally by the
    /// wrapped `CoppaModem` (`pilots.num_data()`). Exposed so callers (e.g.
    /// `StreamingReceiver`) can assert this coincides with their own,
    /// independently-derived `profile.data_carriers` — see
    /// `StreamingReceiver::new`'s `debug_assert_eq!`.
    pub fn data_carriers_per_symbol(&self) -> usize {
        self.modem.data_carriers_per_symbol()
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

        // Fallback noise variance for carriers with no estimate (or a degenerate
        // near-zero one): the median of the per-carrier estimates we do have, rather
        // than a fixed `0.01`/`0.001` magic constant. A fixed fallback either
        // over-trusts a carrier with no real estimate (too small a variance inflates
        // its LLR magnitude) or under-trusts it relative to the actual channel (too
        // large flattens it) -- the median of this frame's own measured noise floor
        // is a much better prior than an arbitrary constant tuned on a different
        // channel/SNR regime.
        let fallback_nv = median_noise_variance(&noise_vars);

        for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
            let nv = match noise_vars.get(i) {
                // A present-but-near-zero estimate is as uninformative as a missing
                // one (dividing by it would blow the LLR up towards +/-infinity), so
                // both cases fall back to the same median-based estimate.
                Some(&v) if v > 1e-6 => v,
                _ => fallback_nv,
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
        let mut deinterleaved = comp.interleaver.deinterleave(&llrs);

        // Known-bit pinning: info bits beyond the payload are zero-padded then scrambled
        // on TX, so RX knows their exact values -- pin to ±PIN (effective code shortening;
        // worth 1.5-3 dB on short payloads).
        const PIN: f32 = 64.0;
        let info_bits = comp.codec.code().info_bits();
        let payload_bits = (header.payload_len as usize) * 8;
        if payload_bits < info_bits {
            let pad_prbs = crate::fec::scrambler::prbs_bits(info_bits);
            for (i, &prbs_bit) in pad_prbs
                .iter()
                .enumerate()
                .take(info_bits)
                .skip(payload_bits)
            {
                deinterleaved[i] = if prbs_bit == 0 { PIN } else { -PIN };
            }
        }

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

    /// Step 1(c): the pinned positions computed in `receive` must equal the
    /// pad's actual scrambled (transmitted) value, for every position beyond
    /// the payload. Replicates `transmit`'s exact `payload_bits` construction
    /// (real payload bits + zero pad, whole vector scrambled) as ground truth,
    /// then checks it against `prbs_bits(info_bits)` at the same indices --
    /// this is exactly what `receive`'s pinning block relies on (see its
    /// "Known-bit pinning" comment).
    #[test]
    fn known_pad_prbs_matches_scrambled_pad_ground_truth() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let comp = tx.codecs.get(&2).expect("level 2 must exist"); // BPSK 1/2
        let info_bits = comp.codec.code().info_bits();
        let payload_bytes = 20usize;
        let payload_bits_count = payload_bytes * 8;
        assert!(
            payload_bits_count < info_bits,
            "test assumes real padding exists (972-bit info, 160 real bits)"
        );

        // Ground truth: exactly replicate `transmit`'s payload_bits construction.
        let payload = vec![0xA5u8; payload_bytes];
        let mut payload_bits = Vec::with_capacity(info_bits);
        for &byte in &payload {
            for shift in (0..8).rev() {
                payload_bits.push((byte >> shift) & 1);
            }
        }
        payload_bits.resize(info_bits, 0u8);
        scramble(&mut payload_bits);

        let pad_prbs = crate::fec::scrambler::prbs_bits(info_bits);
        assert_eq!(
            &payload_bits[payload_bits_count..],
            &pad_prbs[payload_bits_count..],
            "prbs_bits(info_bits)'s pad-region tail must match transmit()'s actual \
             scrambled pad -- this is the ground truth `receive`'s pinning relies on"
        );
    }

    /// Step 1(b): statistical integration test for known-pad LLR pinning (Task 3).
    ///
    /// `CoppaTransceiver::receive`'s full OFDM pipeline can't demonstrate this
    /// directly: a dedicated bench sweep (`coppa-bench`'s `task3_short_payload_gate`
    /// example; see the Task 3 report for the full before/after CSVs) found that
    /// for a 20-byte payload at level 2 on `hf_standard`/AWGN, *every* frame
    /// failure across the whole relevant SNR range is a sync/header failure, not
    /// an LDPC non-convergence -- confirmed by direct instrumentation showing the
    /// LDPC decode converges 100% of the time whenever sync succeeds, identically
    /// whether or not pad bits are pinned. OFDM sync is strictly the binding
    /// constraint here, so the pinning's effect on the LDPC margin is invisible
    /// end-to-end.
    ///
    /// To test the actual mechanism (not masked by sync), this replicates the
    /// exact code path `receive`/`transmit` use for the FEC layer -- `scramble`,
    /// `prbs_bits`, `LdpcCodec`, and `BpskMapper`'s (now-fixed) exact max-log LLR
    /// scale -- but maps coded bits directly to BPSK symbols and adds AWGN,
    /// bypassing OFDM/sync entirely. This is exactly the isolated measurement
    /// `coppa-bench`'s `task3_fec_isolated_gate` example performs; see the Task 3
    /// report for the full sweep. That sweep found: no-pin FER<=10% threshold =
    /// 2.0 dB, pinned threshold = -1.0 dB (a 3.0 dB shift, matching the brief's
    /// expected 1.5-3 dB). This test fixes the SNR at 1.5 dB below the no-pinning
    /// threshold (0.5 dB) and asserts pinning recovers the large majority of
    /// frames there (measured 393/400 = 98.25% in the full sweep at this exact
    /// point; 100 seeds here for a quick but still statistically meaningful
    /// check).
    #[test]
    #[ignore = "statistical (100 seeds); run manually: cargo test -p coppa-protocol --lib -- --ignored known_pad_pinning_recovers_below_no_pinning_threshold"]
    fn known_pad_pinning_recovers_below_no_pinning_threshold() {
        use crate::fec::ldpc::{CodeRate, LdpcCodec};
        use crate::fec::scrambler::prbs_bits;
        use coppa_codec::bpsk::BpskMapper;
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const PAYLOAD_BYTES: usize = 20;
        const PIN: f32 = 64.0;
        const LLR_CLIP: f32 = 20.0;
        // Measured no-pinning FER<=10% threshold (task3_fec_isolated_gate) is 2.0 dB;
        // 1.5 dB below that is 0.5 dB.
        const TEST_SNR_DB: f32 = 0.5;
        const SEEDS: u64 = 100;

        let codec = LdpcCodec::new(CodeRate::Rate1_2); // level 2: BPSK 1/2, 972 info bits
        let info_bits = codec.code().info_bits();
        let payload_bits_count = PAYLOAD_BYTES * 8;
        let mapper = BpskMapper;

        let mut successes = 0u64;
        for trial in 0..SEEDS {
            let seed = 0x9EED_0000u64.wrapping_add(trial);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| rng.random::<u8>()).collect();

            let mut info: Vec<u8> = Vec::with_capacity(info_bits);
            for &byte in &payload {
                for shift in (0..8).rev() {
                    info.push((byte >> shift) & 1);
                }
            }
            info.resize(info_bits, 0u8);
            scramble(&mut info);
            let coded = codec.encode(&info);

            let clean: Vec<f32> = coded.iter().map(|&b| mapper.map(&[b]).re).collect();
            let noisy =
                coppa_channel::awgn_seeded(&clean, TEST_SNR_DB, seed ^ 0x5A5A_5A5A_5A5A_5A5Au64);
            let nv = 10f32.powf(-TEST_SNR_DB / 10.0);
            let mut llrs: Vec<f32> = noisy
                .iter()
                .map(|&re| (4.0 * re / nv).clamp(-LLR_CLIP, LLR_CLIP))
                .collect();

            let pad_prbs = prbs_bits(info_bits);
            for (i, &prbs_bit) in pad_prbs
                .iter()
                .enumerate()
                .take(info_bits)
                .skip(payload_bits_count)
            {
                llrs[i] = if prbs_bit == 0 { PIN } else { -PIN };
            }

            let (mut decoded, converged) = codec.decode_checked(&llrs);
            if !converged {
                continue;
            }
            scramble(&mut decoded);

            let mut out = Vec::with_capacity(PAYLOAD_BYTES);
            for chunk in decoded.chunks(8) {
                if chunk.len() == 8 && out.len() < PAYLOAD_BYTES {
                    let mut byte = 0u8;
                    for (i, &bit) in chunk.iter().enumerate() {
                        byte |= (bit & 1) << (7 - i);
                    }
                    out.push(byte);
                }
            }
            if out == payload {
                successes += 1;
            }
        }

        assert!(
            successes * 100 >= SEEDS * 90,
            "known-pad pinning should recover the large majority of frames 1.5 dB below \
             the no-pinning FER<=10% threshold, got {successes}/{SEEDS}"
        );
    }
}
