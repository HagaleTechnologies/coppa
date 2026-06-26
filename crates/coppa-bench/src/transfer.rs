//! Transfer-level measurement: a payload spread over N frames through a *correlated*
//! multi-frame channel, scored by payload-recovery fraction. Foundation for comparing
//! the baseline (V1) PHY against the future interleaved (V2) PHY.

use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::cross_frame_interleaver::CrossFrameInterleaver;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_codec::traits::{ConstellationMapper, FecCodec};
use coppa_protocol::fec::ldpc::codes::CodeRate;
use coppa_protocol::fec::ldpc::LdpcCodec;
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{speed_level_components, speed_level_entry};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fmt::Write as _;

use crate::scenario::{mode_for_level, select_profile, ChannelSpec, SAMPLE_RATE};

/// A PHY's transfer strategy: encode a full-transfer payload into N frame-signals,
/// decode N received frame-windows back to recovered bytes.
pub trait TransferPhy {
    /// Frames per transfer.
    fn frames_per_transfer(&self) -> usize;
    /// Total payload bytes carried by one transfer.
    fn payload_bytes(&self) -> usize;
    /// Encode a `payload_bytes()`-long payload into `frames_per_transfer()` signals.
    fn encode_transfer(&self, payload: &[u8]) -> Vec<Vec<f32>>;
    /// Decode received per-frame windows back to recovered bytes (length == payload_bytes()).
    fn decode_transfer(&self, frame_windows: &[&[f32]]) -> Vec<u8>;
}

/// Baseline PHY: N independent, self-contained codeword-frames (no cross-frame coding).
pub struct V1Phy {
    level: u8,
    frames: usize,
    per_frame_bytes: usize,
    tx: CoppaTransceiver,
}

impl V1Phy {
    pub fn new(level: u8, frames: usize) -> Self {
        let per_frame_bytes = mode_for_level(level)
            .unwrap_or_else(|| panic!("unknown speed level {level}"))
            .payload_bytes();
        let tx = CoppaTransceiver::new(select_profile(level), 1);
        Self {
            level,
            frames,
            per_frame_bytes,
            tx,
        }
    }

    fn make_header(&self, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: self.level,
            seq_num: 0,
            payload_len,
        }
    }
}

impl TransferPhy for V1Phy {
    fn frames_per_transfer(&self) -> usize {
        self.frames
    }

    fn payload_bytes(&self) -> usize {
        self.frames * self.per_frame_bytes
    }

    fn encode_transfer(&self, payload: &[u8]) -> Vec<Vec<f32>> {
        let pfb = self.per_frame_bytes;
        (0..self.frames)
            .map(|k| {
                let chunk = &payload[k * pfb..(k + 1) * pfb];
                self.tx.transmit(&self.make_header(pfb as u16), chunk)
            })
            .collect()
    }

    fn decode_transfer(&self, frame_windows: &[&[f32]]) -> Vec<u8> {
        let pfb = self.per_frame_bytes;
        let mut out = vec![0u8; self.payload_bytes()];
        for (k, window) in frame_windows.iter().enumerate() {
            if let Ok((_h, bytes)) = self.tx.receive(window) {
                let n = bytes.len().min(pfb);
                out[k * pfb..k * pfb + n].copy_from_slice(&bytes[..n]);
            }
        }
        out
    }
}

/// Number of coded bits in one LDPC codeword (fixed across all rates in this codec).
const CODED_BITS: usize = 1944;

/// V2 PHY: N codewords are cross-frame interleaved so each frame carries a 1/N stripe of
/// every codeword. Rate-neutral vs `V1Phy` (same payload, frames, and airtime); the only
/// difference is the `CrossFrameInterleaver` nested outside the per-frame `BlockInterleaver`.
pub struct V2Phy {
    level: u8,
    frames: usize,
    per_frame_bytes: usize,
    profile: CoppaProfile,
    modem: CoppaModem,
    mapper: Box<dyn ConstellationMapper>,
    code_rate: CodeRate,
    cross: CrossFrameInterleaver,
    papr_db: f32,
}

impl V2Phy {
    pub fn new(level: u8, frames: usize) -> Self {
        let per_frame_bytes = mode_for_level(level)
            .unwrap_or_else(|| panic!("unknown speed level {level}"))
            .payload_bytes();
        let profile = select_profile(level);
        let modem = CoppaModem::new(profile.clone(), 1);
        let (mapper, code_rate) =
            speed_level_components(level).unwrap_or_else(|e| panic!("speed level {level}: {e}"));
        let papr_db = speed_level_entry(level)
            .unwrap_or_else(|| panic!("no speed-level entry for {level}"))
            .papr_target_db;
        let cross = CrossFrameInterleaver::new(frames, CODED_BITS);
        Self {
            level,
            frames,
            per_frame_bytes,
            profile,
            modem,
            mapper,
            code_rate,
            cross,
            papr_db,
        }
    }

    fn make_header(&self, seq_num: u8, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: self.level,
            seq_num,
            payload_len,
        }
    }
}

impl TransferPhy for V2Phy {
    fn frames_per_transfer(&self) -> usize {
        self.frames
    }

    fn payload_bytes(&self) -> usize {
        self.frames * self.per_frame_bytes
    }

    fn encode_transfer(&self, payload: &[u8]) -> Vec<Vec<f32>> {
        let n = self.frames;
        let pfb = self.per_frame_bytes;
        let info_bits = self.code_rate.info_bits();
        let data_carriers = self.profile.data_carriers;

        // 1. LDPC-encode each chunk into one codeword; concatenate codeword-major.
        let mut all_coded: Vec<u8> = Vec::with_capacity(n * CODED_BITS);
        for k in 0..n {
            let chunk = &payload[k * pfb..(k + 1) * pfb];
            let mut bits = Vec::with_capacity(info_bits);
            for &byte in chunk {
                for shift in (0..8).rev() {
                    bits.push((byte >> shift) & 1);
                }
            }
            bits.resize(info_bits, 0u8);
            scramble(&mut bits);
            let mut codec = LdpcCodec::new(self.code_rate);
            let coded = codec.encode(&bits); // CODED_BITS long
            all_coded.extend_from_slice(&coded);
        }

        // 2. Cross-frame interleave (the diversity step).
        let interleaved = self.cross.interleave(&all_coded); // frame-major, n*CODED_BITS

        // 3. Per frame: intra-frame block interleave -> constellation map -> OFDM modulate.
        (0..n)
            .map(|f| {
                let frame_bits = &interleaved[f * CODED_BITS..(f + 1) * CODED_BITS];
                let bi = BlockInterleaver::new(CODED_BITS, data_carriers);
                let block = bi.interleave(frame_bits);
                let symbols = self.mapper.map_bits(&block);
                self.modem.modulate_mapped(
                    &self.make_header(f as u8, pfb as u16),
                    &symbols,
                    self.papr_db,
                )
            })
            .collect()
    }

    fn decode_transfer(&self, frame_windows: &[&[f32]]) -> Vec<u8> {
        let n = self.frames;
        let pfb = self.per_frame_bytes;
        let data_carriers = self.profile.data_carriers;
        let bps = self.mapper.bits_per_symbol();
        let symbols_needed = CODED_BITS.div_ceil(bps);

        // 1. Per frame: demodulate -> soft LLRs -> intra-frame de-interleave -> frame LLR block.
        //    A frame that fails sync contributes an all-zero (erasure) block.
        let mut frame_llrs: Vec<f32> = vec![0.0; n * CODED_BITS];
        for (f, window) in frame_windows.iter().enumerate().take(n) {
            if let Some((_h, eq_symbols, noise_vars)) = self.modem.demodulate_soft_coded(window) {
                let mut llrs = Vec::with_capacity(CODED_BITS);
                for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
                    let nv = if i < noise_vars.len() {
                        noise_vars[i].max(0.001)
                    } else {
                        0.01
                    };
                    llrs.extend(self.mapper.demap_soft(sym, nv));
                }
                llrs.truncate(CODED_BITS);
                llrs.resize(CODED_BITS, 0.0);
                for v in &mut llrs {
                    *v = v.clamp(-20.0, 20.0);
                }
                let bi = BlockInterleaver::new(CODED_BITS, data_carriers);
                let deint = bi.deinterleave(&llrs);
                frame_llrs[f * CODED_BITS..(f + 1) * CODED_BITS].copy_from_slice(&deint);
            }
        }

        // 2. Cross-frame de-interleave: frame-major LLRs -> codeword-major LLRs.
        let codeword_llrs = self.cross.deinterleave(&frame_llrs);

        // 3. Per codeword: LDPC decode -> descramble -> bytes -> place in slot k.
        let mut out = vec![0u8; self.payload_bytes()];
        for k in 0..n {
            let llr = &codeword_llrs[k * CODED_BITS..(k + 1) * CODED_BITS];
            let codec = LdpcCodec::new(self.code_rate);
            let (mut bits, converged) = codec.decode_checked(llr);
            if !converged {
                continue;
            }
            scramble(&mut bits); // involution: undoes TX-side scrambling
            let mut bytes = Vec::with_capacity(pfb);
            for chunk in bits.chunks(8) {
                if chunk.len() == 8 && bytes.len() < pfb {
                    let mut byte = 0u8;
                    for (i, &b) in chunk.iter().enumerate() {
                        byte |= (b & 1) << (7 - i);
                    }
                    bytes.push(byte);
                }
            }
            let m = bytes.len().min(pfb);
            out[k * pfb..k * pfb + m].copy_from_slice(&bytes[..m]);
        }
        out
    }
}

/// Concatenate the per-frame signals, apply the channel ONCE (so fading is correlated
/// across frames), then split back into per-frame windows of the original length.
pub fn apply_transfer_channel(
    frames: &[Vec<f32>],
    channel: ChannelSpec,
    snr_db: f32,
    seed: u64,
) -> Vec<Vec<f32>> {
    let l = frames.first().map(|f| f.len()).unwrap_or(0);
    let concat: Vec<f32> = frames.iter().flatten().copied().collect();

    let faded = match channel {
        ChannelSpec::Awgn => {
            coppa_channel::awgn_seeded(&concat, snr_db, seed ^ 0x5555_5555_5555_5555)
        }
        ChannelSpec::Watterson(preset) => {
            let f = coppa_channel::watterson::watterson(
                &concat,
                SAMPLE_RATE as f32,
                &preset.config(),
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_seeded(&f, snr_db, seed ^ 0x5555_5555_5555_5555)
        }
    };

    (0..frames.len())
        .map(|k| faded[k * l..(k + 1) * l].to_vec())
        .collect()
}

/// Fraction of bytes in `recovered` that match `sent`.
pub fn recovery_fraction(sent: &[u8], recovered: &[u8]) -> f64 {
    if sent.is_empty() {
        return 1.0;
    }
    let n = sent.len().min(recovered.len());
    let correct = sent[..n]
        .iter()
        .zip(&recovered[..n])
        .filter(|(a, b)| a == b)
        .count();
    correct as f64 / sent.len() as f64
}

fn channel_label(c: ChannelSpec) -> &'static str {
    match c {
        ChannelSpec::Awgn => "awgn",
        ChannelSpec::Watterson(WattersonPreset::Good) => "watterson-good",
        ChannelSpec::Watterson(WattersonPreset::Moderate) => "watterson-moderate",
        ChannelSpec::Watterson(WattersonPreset::Poor) => "watterson-poor",
    }
}

/// One transfer measurement point.
#[derive(Debug, Clone)]
pub struct TransferPoint {
    pub phy_name: &'static str,
    pub level: u8,
    pub channel: &'static str,
    pub snr_db: f32,
    pub frames: usize,
    pub trials: usize,
    pub recovery_fraction: f64,
    pub goodput_bps: f64,
    pub latency_s: f64,
}

/// Sweep a transfer PHY over SNR on one channel.
pub fn run_transfer_scenario(
    phy: &dyn TransferPhy,
    phy_name: &'static str,
    level: u8,
    channel: ChannelSpec,
    snr_db_points: &[f32],
    trials: usize,
    base_seed: u64,
) -> Vec<TransferPoint> {
    assert!(trials > 0, "run_transfer_scenario needs at least one trial");
    let total_bytes = phy.payload_bytes();
    let mut points = Vec::with_capacity(snr_db_points.len());

    for (si, &snr_db) in snr_db_points.iter().enumerate() {
        let mut recov_sum = 0.0f64;
        let mut frame_samples_total = 0usize;
        for t in 0..trials {
            let seed = base_seed
                .wrapping_add((si as u64) << 32)
                .wrapping_add(t as u64);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..total_bytes).map(|_| rng.random::<u8>()).collect();
            let frames = phy.encode_transfer(&payload);
            frame_samples_total = frames.iter().map(|f| f.len()).sum();
            let windows_owned = apply_transfer_channel(&frames, channel, snr_db, seed);
            let windows: Vec<&[f32]> = windows_owned.iter().map(|w| w.as_slice()).collect();
            let recovered = phy.decode_transfer(&windows);
            recov_sum += recovery_fraction(&payload, &recovered);
        }
        let recovery = recov_sum / trials as f64;
        let airtime_s = frame_samples_total as f64 / SAMPLE_RATE as f64;
        let goodput_bps = if airtime_s > 0.0 {
            (total_bytes * 8) as f64 * recovery / airtime_s
        } else {
            0.0
        };
        points.push(TransferPoint {
            phy_name,
            level,
            channel: channel_label(channel),
            snr_db,
            frames: phy.frames_per_transfer(),
            trials,
            recovery_fraction: recovery,
            goodput_bps,
            latency_s: airtime_s,
        });
    }
    points
}

/// CSV header for transfer results.
pub const TRANSFER_CSV_HEADER: &str =
    "phy,level,channel,snr_db,frames,trials,recovery_fraction,goodput_bps,latency_s";

/// Build a CSV document from transfer points.
pub fn transfer_to_csv(points: &[TransferPoint]) -> String {
    let mut out = String::from(TRANSFER_CSV_HEADER);
    out.push('\n');
    for p in points {
        let _ = writeln!(
            out,
            "{},{},{},{:.1},{},{},{:.6},{:.2},{:.3}",
            p.phy_name,
            p.level,
            p.channel,
            p.snr_db,
            p.frames,
            p.trials,
            p.recovery_fraction,
            p.goodput_bps,
            p.latency_s
        );
    }
    out
}

/// Build a markdown table (recovery fraction per SNR) for one channel.
pub fn transfer_to_markdown(points: &[TransferPoint], title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## {title}\n");
    let _ = writeln!(
        out,
        "| PHY | Level | SNR | Recovery | Goodput (bps) | Latency (s) |"
    );
    let _ = writeln!(
        out,
        "|-----|-------|-----|----------|---------------|-------------|"
    );
    for p in points {
        let _ = writeln!(
            out,
            "| {} | {} | {:.0} dB | {:.1}% | {:.0} | {:.2} |",
            p.phy_name,
            p.level,
            p.snr_db,
            p.recovery_fraction * 100.0,
            p.goodput_bps,
            p.latency_s
        );
    }
    out
}

/// One paired V1-vs-V2 measurement point (both PHYs through identical channel draws).
#[derive(Debug, Clone)]
pub struct PairedPoint {
    pub level: u8,
    pub channel: &'static str,
    pub snr_db: f32,
    pub trials: usize,
    pub v1_recovery: f64,
    pub v2_recovery: f64,
    pub delta: f64, // v2_recovery - v1_recovery
}

/// Run V1 and V2 through IDENTICAL channel draws and score both, per SNR point.
///
/// The fading seed (`chan_seed`) depends only on the trial index, not the SNR index, so the
/// SAME fading realization is reused across all SNR points (clean monotone curves; only AWGN
/// changes with SNR). At each (SNR, trial) V1 and V2 see the same Watterson fade and AWGN, so
/// the comparison is genuinely paired.
pub fn run_paired_comparison(
    v1: &dyn TransferPhy,
    v2: &dyn TransferPhy,
    level: u8,
    channel: ChannelSpec,
    snr_db_points: &[f32],
    trials: usize,
    base_seed: u64,
) -> Vec<PairedPoint> {
    assert!(trials > 0, "run_paired_comparison needs at least one trial");
    assert_eq!(
        v1.payload_bytes(),
        v2.payload_bytes(),
        "V1 and V2 must carry equal payloads to be comparable"
    );
    let total_bytes = v1.payload_bytes();
    let mut points = Vec::with_capacity(snr_db_points.len());

    for &snr_db in snr_db_points {
        let mut v1_sum = 0.0f64;
        let mut v2_sum = 0.0f64;
        for t in 0..trials {
            let chan_seed = base_seed.wrapping_add(t as u64);
            let payload_seed = base_seed ^ 0xA5A5_A5A5_0000_0000 ^ (t as u64);
            let mut rng = StdRng::seed_from_u64(payload_seed);
            let payload: Vec<u8> = (0..total_bytes).map(|_| rng.random::<u8>()).collect();

            let f1 = v1.encode_transfer(&payload);
            let w1 = apply_transfer_channel(&f1, channel, snr_db, chan_seed);
            let r1: Vec<&[f32]> = w1.iter().map(|w| w.as_slice()).collect();
            v1_sum += recovery_fraction(&payload, &v1.decode_transfer(&r1));

            let f2 = v2.encode_transfer(&payload);
            let w2 = apply_transfer_channel(&f2, channel, snr_db, chan_seed);
            let r2: Vec<&[f32]> = w2.iter().map(|w| w.as_slice()).collect();
            v2_sum += recovery_fraction(&payload, &v2.decode_transfer(&r2));
        }
        let v1r = v1_sum / trials as f64;
        let v2r = v2_sum / trials as f64;
        points.push(PairedPoint {
            level,
            channel: channel_label(channel),
            snr_db,
            trials,
            v1_recovery: v1r,
            v2_recovery: v2r,
            delta: v2r - v1r,
        });
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_transfer_clean_loopback_recovers_payload() {
        // No channel: encode, then decode the clean frame-signals directly.
        let phy = V1Phy::new(2, 4); // BPSK 1/2, 4 frames
        let total = phy.payload_bytes();
        let payload: Vec<u8> = (0..total).map(|i| (i * 7 + 1) as u8).collect();

        let frames = phy.encode_transfer(&payload);
        assert_eq!(frames.len(), 4);
        let windows: Vec<&[f32]> = frames.iter().map(|f| f.as_slice()).collect();
        let recovered = phy.decode_transfer(&windows);

        assert_eq!(
            recovered, payload,
            "clean loopback must recover the full transfer"
        );
    }

    #[test]
    fn v1_transfer_recovers_over_awgn_at_high_snr() {
        let phy = V1Phy::new(2, 4);
        let total = phy.payload_bytes();
        let payload: Vec<u8> = (0..total).map(|i| (i * 13 + 5) as u8).collect();
        let frames = phy.encode_transfer(&payload);
        let windows_owned = apply_transfer_channel(&frames, ChannelSpec::Awgn, 30.0, 99);
        let windows: Vec<&[f32]> = windows_owned.iter().map(|w| w.as_slice()).collect();
        let recovered = phy.decode_transfer(&windows);
        assert_eq!(
            recovered, payload,
            "AWGN at 30 dB should recover the full transfer"
        );
    }

    #[test]
    fn fading_is_correlated_across_adjacent_frames() {
        use coppa_channel::watterson::WattersonPreset;
        // 16 identical deterministic "frames"; estimate each frame's real gain by
        // projecting the faded window onto the clean frame. Use the Poor preset
        // (1.0 Hz Doppler): its coherence time (~0.1-0.2 s) is short enough that this
        // ~1.4 s transfer spans several coherence times, so per-frame fading varies
        // meaningfully while adjacent frames stay more correlated than distant ones --
        // the regime where SP2's time interleaving buys diversity. (Good's 0.1 Hz
        // coherence time exceeds this transfer length, so its gains barely vary.)
        let l = 4096usize;
        let n = 16usize;
        let frame: Vec<f32> = (0..l).map(|i| (i as f32 * 0.13).sin() * 0.5).collect();
        let concat: Vec<f32> = (0..n).flat_map(|_| frame.iter().copied()).collect();
        let faded = coppa_channel::watterson::watterson(
            &concat,
            48_000.0,
            &WattersonPreset::Poor.config(),
            7,
        );
        let denom: f32 = frame.iter().map(|x| x * x).sum();
        let gains: Vec<f32> = (0..n)
            .map(|k| {
                let w = &faded[k * l..(k + 1) * l];
                w.iter().zip(&frame).map(|(a, b)| a * b).sum::<f32>() / denom
            })
            .collect();

        // Fading varies meaningfully across the transfer (there is diversity to exploit).
        let mean = gains.iter().sum::<f32>() / n as f32;
        let var = gains.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / n as f32;
        assert!(
            var > 1e-3,
            "fading should vary meaningfully across the transfer (var={var})"
        );

        // Adjacent frames are more similar than distant ones → correlated in time,
        // decorrelating with separation (the premise SP2 relies on).
        let adj: f32 = (0..n - 1)
            .map(|k| (gains[k + 1] - gains[k]).abs())
            .sum::<f32>()
            / (n - 1) as f32;
        let dist: f32 = (0..n - 8)
            .map(|k| (gains[k + 8] - gains[k]).abs())
            .sum::<f32>()
            / (n - 8) as f32;
        assert!(
            adj < dist,
            "adjacent gains should be closer than distant (adj={adj}, dist={dist})"
        );
    }

    #[test]
    fn v2_encode_produces_uniform_frames() {
        let phy = V2Phy::new(2, 8);
        let total = phy.payload_bytes();
        assert_eq!(
            total,
            V1Phy::new(2, 8).payload_bytes(),
            "V2 must be rate-neutral vs V1"
        );
        let payload: Vec<u8> = (0..total).map(|i| (i * 3 + 1) as u8).collect();
        let frames = phy.encode_transfer(&payload);
        assert_eq!(frames.len(), 8, "must emit N frame-signals");
        let l0 = frames[0].len();
        assert!(l0 > 0);
        assert!(
            frames.iter().all(|f| f.len() == l0),
            "frame signals must be uniform length"
        );
    }

    #[test]
    fn v2_clean_loopback_recovers_payload() {
        let phy = V2Phy::new(2, 8);
        let total = phy.payload_bytes();
        let payload: Vec<u8> = (0..total).map(|i| (i * 11 + 3) as u8).collect();
        let frames = phy.encode_transfer(&payload);
        let windows: Vec<&[f32]> = frames.iter().map(|f| f.as_slice()).collect();
        let recovered = phy.decode_transfer(&windows);
        assert_eq!(
            recovered, payload,
            "clean loopback must recover the full V2 transfer"
        );
    }

    #[test]
    fn v2_recovers_over_awgn_at_high_snr() {
        let phy = V2Phy::new(2, 8);
        let total = phy.payload_bytes();
        let payload: Vec<u8> = (0..total).map(|i| (i * 17 + 7) as u8).collect();
        let frames = phy.encode_transfer(&payload);
        let windows_owned = apply_transfer_channel(&frames, ChannelSpec::Awgn, 30.0, 4242);
        let windows: Vec<&[f32]> = windows_owned.iter().map(|w| w.as_slice()).collect();
        let recovered = phy.decode_transfer(&windows);
        assert_eq!(
            recovered, payload,
            "V2 over AWGN at 30 dB should recover fully"
        );
    }

    #[test]
    fn recovery_fraction_counts_matching_bytes() {
        assert_eq!(recovery_fraction(&[1, 2, 3, 4], &[1, 2, 3, 4]), 1.0);
        assert_eq!(recovery_fraction(&[1, 2, 3, 4], &[1, 9, 3, 9]), 0.5);
    }

    #[test]
    fn transfer_scenario_recovers_over_awgn() {
        let phy = V1Phy::new(2, 4);
        let points = run_transfer_scenario(&phy, "v1", 2, ChannelSpec::Awgn, &[30.0], 3, 0xABCD);
        assert_eq!(points.len(), 1);
        assert!(
            points[0].recovery_fraction > 0.99,
            "AWGN should recover (got {})",
            points[0].recovery_fraction
        );
        assert!(points[0].goodput_bps > 0.0);
        assert!(points[0].latency_s > 0.0);
    }

    #[test]
    fn transfer_csv_and_markdown_render() {
        let p = TransferPoint {
            phy_name: "v1",
            level: 2,
            channel: "watterson-good",
            snr_db: 18.0,
            frames: 8,
            trials: 50,
            recovery_fraction: 0.12,
            goodput_bps: 90.0,
            latency_s: 10.4,
        };
        let csv = transfer_to_csv(std::slice::from_ref(&p));
        assert_eq!(csv.lines().next().unwrap(), TRANSFER_CSV_HEADER);
        assert_eq!(csv.lines().nth(1).unwrap().split(',').count(), 9);
        let md = transfer_to_markdown(&[p], "Test");
        assert!(md.contains("| v1 | 2 |"));
    }

    #[test]
    fn paired_comparison_is_deterministic() {
        let v1 = V1Phy::new(2, 8);
        let v2 = V2Phy::new(2, 8);
        let a = run_paired_comparison(&v1, &v2, 2, ChannelSpec::Awgn, &[24.0], 2, 0x1234);
        let b = run_paired_comparison(&v1, &v2, 2, ChannelSpec::Awgn, &[24.0], 2, 0x1234);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].v1_recovery, b[0].v1_recovery);
        assert_eq!(a[0].v2_recovery, b[0].v2_recovery);
    }

    #[test]
    fn channel_draw_depends_only_on_seed() {
        // Same seed + same signal => identical faded output (so V1/V2 at a point are paired).
        let frames = vec![vec![0.3f32; 2048], vec![-0.2f32; 2048]];
        let a = apply_transfer_channel(
            &frames,
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            18.0,
            777,
        );
        let b = apply_transfer_channel(
            &frames,
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            18.0,
            777,
        );
        assert_eq!(
            a, b,
            "identical seed+signal must give identical channel realization"
        );
    }

    #[test]
    #[ignore = "slow characterization (~minutes in debug); run with `cargo test -- --ignored`"]
    fn interleaving_helps_strong_channels_but_hurts_marginal_ones() {
        // CHARACTERIZATION of cross-frame interleaving AFTER the LDPC scale-bug fix (see
        // BENCHMARKS.md). The interleaver is a lossless permutation (AWGN unchanged). Its
        // effect under fading depends on the regime relative to the LDPC threshold:
        //   * Strong channel (Watterson Good at decent SNR): the per-frame link is mostly
        //     above threshold, so spreading mops up V1's residual frame failures → V2 >= V1.
        //   * Marginal channel (Watterson Moderate): the per-frame average is below threshold,
        //     so spreading turns V1's lucky survivors into uniform failure → V2 < V1.
        // This is the textbook interleaving trade-off, and it's why cross-frame diversity is
        // NOT a general robustness win here.
        let v1 = V1Phy::new(2, 8);
        let v2 = V2Phy::new(2, 8);

        // (a) AWGN: the interleaver is lossless — both recover fully.
        let awgn = run_paired_comparison(&v1, &v2, 2, ChannelSpec::Awgn, &[30.0], 5, 0x5EED);
        assert!(awgn[0].v1_recovery > 0.99 && awgn[0].v2_recovery > 0.99);

        // (b) Strong channel (Good @ 24 dB): interleaving does NOT hurt (it helps or ties).
        let good = run_paired_comparison(
            &v1,
            &v2,
            2,
            ChannelSpec::Watterson(WattersonPreset::Good),
            &[24.0],
            10,
            0x5EED,
        );
        assert!(
            good[0].v2_recovery + 1e-9 >= good[0].v1_recovery,
            "on a strong channel V2 should not underperform V1 (v1={:.3}, v2={:.3})",
            good[0].v1_recovery,
            good[0].v2_recovery
        );

        // (c) Marginal channel (Moderate): interleaving still HURTS (below-threshold regime).
        let moderate = run_paired_comparison(
            &v1,
            &v2,
            2,
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            &[18.0],
            10,
            0x5EED,
        );
        assert!(
            moderate[0].v2_recovery < moderate[0].v1_recovery,
            "on a marginal channel V2 should underperform V1 (v1={:.3}, v2={:.3})",
            moderate[0].v1_recovery,
            moderate[0].v2_recovery
        );
    }
}
