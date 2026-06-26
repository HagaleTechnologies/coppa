//! Transfer-level measurement: a payload spread over N frames through a *correlated*
//! multi-frame channel, scored by payload-recovery fraction. Foundation for comparing
//! the baseline (V1) PHY against the future interleaved (V2) PHY.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

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
}
