//! Transfer-level measurement: a payload spread over N frames through a *correlated*
//! multi-frame channel, scored by payload-recovery fraction. Foundation for comparing
//! the baseline (V1) PHY against the future interleaved (V2) PHY.

use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
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
}
