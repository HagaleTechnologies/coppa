//! Main Coppa engine: thin wrapper around CoppaTransceiver with compression and squelch.

use anyhow::Result;

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::compression::huffman::HuffmanCodec;
use coppa_protocol::compression::lz4::{lz4_compress, lz4_decompress};
use coppa_protocol::modem::streaming::StreamingReceiver;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

use crate::config::EngineConfig;
use crate::profiles::Profile;
use crate::rate_control::RateController;

/// Marker byte prepended to frames to indicate compression.
/// On decode, if the first byte of the payload is this marker, the rest is
/// Huffman+LZ4 compressed. Otherwise the payload is raw.
const COMPRESSION_MARKER: u8 = 0xFE;

/// One decode result surfaced by [`CoppaCore::push_samples`] for a single frame
/// `StreamingReceiver` completed.
///
/// `message` mirrors `decode`'s batch-path contract exactly: the frame's payload
/// goes through this engine's optional Huffman+LZ4 decompression (if the
/// compression marker byte is present) and then a UTF-8 conversion, either of
/// which can fail (both daemon and FFI callers want a UTF-8 `String`, the same
/// as the batch `decode` they used before Task 7 — see the Task 7 report for why
/// `push_samples` keeps that constraint rather than switching to raw bytes).
#[derive(Debug)]
pub struct StreamFrame {
    pub message: Result<String>,
    /// Real per-carrier-noise SNR estimate (dB) from `CoppaTransceiver::
    /// receive_with_metrics`, `10*log10(1/mean(noise_vars))` — NOT the crude
    /// whole-buffer RMS proxy `handle_audio_in` used to compute before Task 7.
    pub snr_db: f32,
    pub cfo_hz: f32,
    pub frame_start: u64,
}

/// Core engine for Coppa digital communications.
///
/// CoppaCore is a thin wrapper around [`CoppaTransceiver`] that adds optional
/// compression (Huffman + LZ4) and squelch gating. All modulation, FEC, and
/// framing are handled by the transceiver.
///
/// # Example
///
/// Encode a text message to audio samples and decode it back (loopback):
///
/// ```
/// use coppa_engine::CoppaCore;
///
/// let core = CoppaCore::new();
/// let samples = core.encode("Hello Coppa")?;
/// assert!(!samples.is_empty());
///
/// let decoded = core.decode(&samples)?;
/// assert_eq!(decoded, "Hello Coppa");
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct CoppaCore {
    transceiver: CoppaTransceiver,
    config: EngineConfig,
    rate_controller: RateController,
    /// Streaming receive path used by [`Self::push_samples`] (daemon/FFI). Owns its
    /// own internal `CoppaTransceiver` (built for the same profile as
    /// `transceiver` above) — see the Task 7 report for why `StreamingReceiver`'s
    /// locked constructor builds its own rather than sharing `transceiver`, and why
    /// that one-time extra per-level cache construction at startup is an
    /// acceptable, bounded cost (not a hot-path duplication).
    streaming: StreamingReceiver,
}

impl CoppaCore {
    /// Create a new engine with default configuration (speed_level=1, hf_standard profile).
    pub fn new() -> Self {
        let config = EngineConfig::default();
        Self::build(config)
    }

    /// Create a new engine with the given configuration.
    pub fn with_config(config: EngineConfig) -> Self {
        Self::build(config)
    }

    /// Create a new engine from a named [`Profile`].
    ///
    /// Uses the profile's `ofdm_profile` field ("hf" or "vhf") to select the
    /// OFDM profile, overriding the speed-level-based threshold.
    pub fn from_profile(profile: &Profile) -> Self {
        let config = EngineConfig::from_profile(profile);
        let ofdm_profile = match profile.ofdm_profile {
            "vhf" => CoppaProfile::vhf_wide(),
            _ => CoppaProfile::hf_standard(),
        };
        let transceiver = CoppaTransceiver::new(ofdm_profile.clone(), 1);
        let streaming = StreamingReceiver::new(ofdm_profile, 1);
        let rate_controller = RateController::new(0, 0, 10);
        Self {
            transceiver,
            config,
            rate_controller,
            streaming,
        }
    }

    /// Build the engine from a config, selecting the appropriate OFDM profile.
    fn build(config: EngineConfig) -> Self {
        let ofdm_profile = Self::select_ofdm_profile(config.speed_level);
        let transceiver = CoppaTransceiver::new(ofdm_profile.clone(), 1);
        let streaming = StreamingReceiver::new(ofdm_profile, 1);
        let rate_controller = RateController::new(0, 0, 10);
        Self {
            transceiver,
            config,
            rate_controller,
            streaming,
        }
    }

    /// Select OFDM profile based on speed level.
    /// Speed levels 1-4 use HF standard; 5+ use VHF wide.
    fn select_ofdm_profile(speed_level: u8) -> CoppaProfile {
        if speed_level >= 5 {
            CoppaProfile::vhf_wide()
        } else {
            CoppaProfile::hf_standard()
        }
    }

    /// Encode a text message to audio samples.
    pub fn encode(&self, message: &str) -> Result<Vec<f32>> {
        self.encode_bytes(message.as_bytes())
    }

    /// Encode arbitrary binary data to audio samples.
    ///
    /// Pipeline:
    /// 1. Optionally compress (Huffman + LZ4) with a marker byte
    /// 2. Build CoppaHeader
    /// 3. Transmit via CoppaTransceiver
    pub fn encode_bytes(&self, data: &[u8]) -> Result<Vec<f32>> {
        let payload = if self.config.compression_enabled {
            let huffman = HuffmanCodec::new();
            let huffman_bytes = huffman.encode(data);
            let lz4_bytes = lz4_compress(&huffman_bytes);
            let mut out = Vec::with_capacity(1 + lz4_bytes.len());
            out.push(COMPRESSION_MARKER);
            out.extend_from_slice(&lz4_bytes);
            out
        } else {
            data.to_vec()
        };

        if payload.len() > u16::MAX as usize {
            anyhow::bail!(
                "Payload too large ({} bytes, max {})",
                payload.len(),
                u16::MAX
            );
        }

        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: self.config.speed_level,
            seq_num: 0,
            payload_len: payload.len() as u16,
        };

        let samples = self.transceiver.transmit(&header, &payload);
        Ok(samples)
    }

    /// Decode audio samples back to a text message.
    pub fn decode(&self, samples: &[f32]) -> Result<String> {
        let data = self.decode_bytes(samples)?;
        let message = String::from_utf8(data)?;
        Ok(message)
    }

    /// Decode audio samples back to raw bytes.
    ///
    /// Pipeline:
    /// 1. Squelch check (reject silence)
    /// 2. Receive via CoppaTransceiver
    /// 3. Optionally decompress (LZ4 + Huffman)
    pub fn decode_bytes(&self, samples: &[f32]) -> Result<Vec<u8>> {
        if self.is_squelched(samples) {
            return Err(anyhow::anyhow!("No signal detected"));
        }

        if samples.is_empty() {
            return Err(anyhow::anyhow!("No samples to decode"));
        }

        let (_header, payload) = self
            .transceiver
            .receive(samples)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        self.decompress_if_needed(payload)
    }

    /// Feed a chunk of audio samples through the streaming receiver, returning
    /// zero or more decode results for any frames that completed as a result of
    /// this chunk.
    ///
    /// This is the streaming counterpart of `decode`/`decode_bytes` (used by the
    /// daemon and FFI, which previously re-ran a batch `decode` over a
    /// hand-managed sliding window — see the Task 7 report for the migration and
    /// why squelch is evaluated per pushed chunk here, rather than per
    /// accumulated window as the batch path did): each incoming chunk is
    /// squelch-gated exactly like `decode_bytes` before it reaches the
    /// `StreamingReceiver`, and each frame the receiver completes has this
    /// engine's Huffman+LZ4 decompression + UTF-8 conversion applied to its
    /// payload, exactly like `decode` applies to a whole buffer in the batch path.
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<StreamFrame> {
        if self.is_squelched(samples) {
            return Vec::new();
        }
        let frames = self.streaming.push_samples(samples);
        frames
            .into_iter()
            .map(|f| StreamFrame {
                message: self
                    .decompress_if_needed(f.payload)
                    .and_then(|data| String::from_utf8(data).map_err(Into::into)),
                snr_db: f.snr_db,
                cfo_hz: f.cfo_hz,
                frame_start: f.frame_start,
            })
            .collect()
    }

    /// RMS-power squelch check, shared by `decode_bytes` and `push_samples`.
    /// Disabled entirely when `squelch_threshold_db == f32::NEG_INFINITY` (the
    /// default in every profile shipped today, and the default `EngineConfig`).
    fn is_squelched(&self, samples: &[f32]) -> bool {
        if self.config.squelch_threshold_db == f32::NEG_INFINITY {
            return false;
        }
        let rms = if samples.is_empty() {
            0.0f32
        } else {
            (samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
        };
        let rms_db = if rms > 1e-10 {
            20.0 * rms.log10()
        } else {
            -120.0
        };
        rms_db < self.config.squelch_threshold_db
    }

    /// Undo this engine's optional Huffman+LZ4 compression if the marker byte is
    /// present, shared by `decode_bytes` and `push_samples`.
    fn decompress_if_needed(&self, payload: Vec<u8>) -> Result<Vec<u8>> {
        if self.config.compression_enabled
            && !payload.is_empty()
            && payload[0] == COMPRESSION_MARKER
        {
            let lz4_data = &payload[1..];
            let huffman_bytes = lz4_decompress(lz4_data)?;
            let huffman = HuffmanCodec::new();
            Ok(huffman.decode(&huffman_bytes))
        } else {
            Ok(payload)
        }
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Get the current MCS index from the rate controller.
    pub fn current_mcs(&self) -> u8 {
        self.rate_controller.current_mcs()
    }

    /// Get mutable access to the rate controller for SNR feedback.
    pub fn rate_controller_mut(&mut self) -> &mut RateController {
        &mut self.rate_controller
    }

    /// Rebuild the engine with a new configuration.
    pub fn reconfigure(&mut self, config: EngineConfig) {
        let new = Self::build(config);
        self.transceiver = new.transceiver;
        self.config = new.config;
        self.streaming = new.streaming;
    }
}

impl Default for CoppaCore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let core = CoppaCore::new();
        assert_eq!(core.config.sample_rate, 48000);
    }

    #[test]
    fn test_engine_with_config() {
        let config = EngineConfig {
            speed_level: 3,
            ..Default::default()
        };
        let core = CoppaCore::with_config(config);
        assert_eq!(core.config.speed_level, 3);
    }

    #[test]
    fn test_engine_default_trait() {
        let core = CoppaCore::default();
        assert_eq!(core.config.sample_rate, 48000);
    }

    #[test]
    fn test_encode_decode_loopback() {
        let core = CoppaCore::new();
        let message = "Hello Coppa";
        let samples = core.encode(message).expect("encode should succeed");
        assert!(!samples.is_empty(), "should produce audio samples");

        let decoded = core.decode(&samples).expect("decode should succeed");
        assert_eq!(decoded, message, "loopback should recover original message");
    }

    #[test]
    fn test_encode_decode_short_message() {
        let core = CoppaCore::new();
        let message = "Hi";
        let samples = core.encode(message).expect("encode short message");
        let decoded = core.decode(&samples).expect("decode short message");
        assert_eq!(decoded, message);
    }

    #[test]
    fn test_encode_produces_samples() {
        let core = CoppaCore::new();
        let samples = core.encode("test").expect("encode should succeed");
        assert!(
            samples.len() > 100,
            "should produce a reasonable number of samples"
        );
    }

    #[test]
    fn test_decode_empty_fails() {
        let core = CoppaCore::new();
        let result = core.decode(&[]);
        assert!(result.is_err(), "decoding empty samples should fail");
    }

    #[test]
    fn test_squelch_rejects_silence() {
        let config = EngineConfig {
            squelch_threshold_db: -40.0,
            ..Default::default()
        };
        let core = CoppaCore::with_config(config);
        let silence = vec![1e-6; 48000];
        let result = core.decode(&silence);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No signal detected"),
            "squelch should reject silence"
        );
    }

    #[test]
    fn test_squelch_disabled() {
        let config = EngineConfig {
            squelch_threshold_db: f32::NEG_INFINITY,
            ..Default::default()
        };
        let core = CoppaCore::with_config(config);
        // With squelch disabled, silence should not trigger "No signal detected"
        // (it will fail for another reason like sync failure)
        let silence = vec![1e-6; 48000];
        let result = core.decode(&silence);
        assert!(result.is_err());
        assert!(
            !result
                .unwrap_err()
                .to_string()
                .contains("No signal detected"),
            "disabled squelch should not reject signal"
        );
    }

    #[test]
    fn test_compression_roundtrip() {
        let config = EngineConfig {
            compression_enabled: true,
            ..Default::default()
        };
        let core = CoppaCore::with_config(config);
        let message = "CQ CQ CQ DE VK2ABC K";
        let samples = core.encode(message).expect("encode with compression");
        let decoded = core.decode(&samples).expect("decode with compression");
        assert_eq!(decoded, message);
    }

    #[test]
    fn test_binary_0xfe_roundtrip() {
        // Verify that binary data starting with 0xFE (the compression marker)
        // roundtrips correctly when compression is disabled.
        let core = CoppaCore::new();
        assert!(!core.config.compression_enabled);
        let data: Vec<u8> = vec![0xFE, 0x01, 0x02, 0x03];
        let samples = core.encode_bytes(&data).expect("encode 0xFE data");
        let decoded = core.decode_bytes(&samples).expect("decode 0xFE data");
        assert_eq!(decoded, data);
    }

    /// The streaming `SyncDetector` needs a clean silence baseline in its
    /// bootstrap window before a preamble arrives (see
    /// `coppa_codec::ofdm::sync_detector`'s own tests and
    /// `coppa_protocol::modem::streaming`'s tests, which all lead with silence
    /// before the first frame) — `encode()`'s output starts exactly at its own
    /// sample 0 with no such lead-in, so streaming tests prepend one. A trailing
    /// pad is needed too: the RX bandpass filter's group delay shifts the frame's
    /// content later in the filtered domain `StreamingReceiver` operates in, so
    /// without a little padding after the frame, `push_samples` sees end-of-input
    /// a few hundred samples before the (filtered-domain) frame is fully buffered.
    fn with_lead_and_trail(samples: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; 8192];
        out.extend_from_slice(samples);
        out.extend(std::iter::repeat_n(0.0f32, 2048));
        out
    }

    #[test]
    fn test_push_samples_roundtrip() {
        let mut core = CoppaCore::new();
        let samples = core
            .encode("Streaming works")
            .expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1, "expected exactly one decoded frame");
        assert_eq!(
            frames[0].message.as_deref().unwrap(),
            "Streaming works",
            "push_samples should recover the encoded message"
        );
        assert!(
            frames[0].snr_db.is_finite(),
            "snr_db should be a finite estimate on a clean channel"
        );
    }

    #[test]
    fn test_push_samples_compression_roundtrip() {
        let config = EngineConfig {
            compression_enabled: true,
            ..Default::default()
        };
        let mut core = CoppaCore::with_config(config);
        let message = "CQ CQ CQ DE VK2ABC K";
        let samples = core.encode(message).expect("encode with compression");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message.as_deref().unwrap(), message);
    }

    #[test]
    fn test_push_samples_fed_in_chunks() {
        let mut core = CoppaCore::new();
        let samples = core
            .encode("Chunked streaming")
            .expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let mut frames = Vec::new();
        for chunk in samples.chunks(777) {
            frames.extend(core.push_samples(chunk));
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message.as_deref().unwrap(), "Chunked streaming");
    }

    #[test]
    fn test_push_samples_squelch_rejects_silence() {
        let config = EngineConfig {
            squelch_threshold_db: -40.0,
            ..Default::default()
        };
        let mut core = CoppaCore::with_config(config);
        let silence = vec![1e-6; 48000];
        let frames = core.push_samples(&silence);
        assert!(
            frames.is_empty(),
            "squelched silence should never reach the streaming receiver"
        );
    }
}
