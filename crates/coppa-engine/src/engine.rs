//! Main Coppa engine: thin wrapper around CoppaTransceiver with compression and squelch.

use anyhow::Result;

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::compression::huffman::HuffmanCodec;
use coppa_protocol::compression::lz4::{lz4_compress, lz4_decompress};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

use crate::config::EngineConfig;
use crate::profiles::Profile;
use crate::rate_control::RateController;

/// Marker byte prepended to frames to indicate compression.
/// On decode, if the first byte of the payload is this marker, the rest is
/// Huffman+LZ4 compressed. Otherwise the payload is raw.
const COMPRESSION_MARKER: u8 = 0xFE;

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
        let transceiver = CoppaTransceiver::new(ofdm_profile, 1);
        let rate_controller = RateController::new(0, 0, 10);
        Self {
            transceiver,
            config,
            rate_controller,
        }
    }

    /// Build the engine from a config, selecting the appropriate OFDM profile.
    fn build(config: EngineConfig) -> Self {
        let ofdm_profile = Self::select_ofdm_profile(config.speed_level);
        let transceiver = CoppaTransceiver::new(ofdm_profile, 1);
        let rate_controller = RateController::new(0, 0, 10);
        Self {
            transceiver,
            config,
            rate_controller,
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
        // Squelch: check RMS power before decode attempt
        if self.config.squelch_threshold_db > f32::NEG_INFINITY {
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
            if rms_db < self.config.squelch_threshold_db {
                return Err(anyhow::anyhow!("No signal detected"));
            }
        }

        if samples.is_empty() {
            return Err(anyhow::anyhow!("No samples to decode"));
        }

        let (_header, payload) = self
            .transceiver
            .receive(samples)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // Decompress if compression is enabled and the marker byte is present
        let data = if self.config.compression_enabled
            && !payload.is_empty()
            && payload[0] == COMPRESSION_MARKER
        {
            let lz4_data = &payload[1..];
            let huffman_bytes = lz4_decompress(lz4_data)?;
            let huffman = HuffmanCodec::new();
            huffman.decode(&huffman_bytes)
        } else {
            payload
        };

        Ok(data)
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
}
