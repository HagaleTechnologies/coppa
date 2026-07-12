//! Main Coppa engine: thin wrapper around CoppaTransceiver with compression and squelch.

use anyhow::Result;

use coppa_codec::ofdm::coppa_modem::TX_PEAK;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::compression::huffman::HuffmanCodec;
use coppa_protocol::compression::lz4::{lz4_compress, lz4_decompress};
use coppa_protocol::modem::streaming::StreamingReceiver;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

use crate::config::EngineConfig;
use crate::profiles::Profile;

/// Marker byte prepended to frames to indicate compression.
/// On decode, if the first byte of the payload is this marker, the rest is
/// Huffman+LZ4 compressed. Otherwise the payload is raw.
const COMPRESSION_MARKER: u8 = 0xFE;

/// Low tone of the standard SSB two-tone TX-level calibration signal (Hz).
/// 700 Hz + 1900 Hz is standard amateur-radio TUNE/two-tone practice: both
/// fall well inside the waveform's ~300-2700 Hz SSB passband (see
/// `CLAUDE.md`) and are far enough apart that ALC/scope readings aren't
/// confused by beat artifacts.
pub const TUNE_TONE_LOW_HZ: f32 = 700.0;
/// High tone of the standard SSB two-tone TX-level calibration signal (Hz).
pub const TUNE_TONE_HIGH_HZ: f32 = 1900.0;

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
    /// Speed level (1-10) the transmitting station encoded this frame at, from the
    /// decoded protected header (`DecodedFrame::header.speed_level`). Added for
    /// Phase 3 Task 7 so daemon/host telemetry (WebSocket `status`'s `level` field)
    /// can report the link's real current speed level rather than a placeholder.
    pub speed_level: u8,
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
        Self {
            transceiver,
            config,
            streaming,
        }
    }

    /// Build the engine from a config, selecting the appropriate OFDM profile.
    fn build(config: EngineConfig) -> Self {
        let ofdm_profile = Self::select_ofdm_profile(config.speed_level);
        let transceiver = CoppaTransceiver::new(ofdm_profile.clone(), 1);
        let streaming = StreamingReceiver::new(ofdm_profile, 1);
        Self {
            transceiver,
            config,
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
            codewords: 1,
        };

        let samples = self
            .transceiver
            .transmit(&header, &payload)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(samples)
    }

    /// Generate a TX-level calibration ("TUNE") tone: an operator keys this and
    /// advances their radio's audio drive until ALC just registers, then backs
    /// off — standard amateur SSB practice (analogous to VARA's `TUNE`).
    ///
    /// Defaults to the standard SSB two-tone test signal (`TUNE_TONE_LOW_HZ` +
    /// `TUNE_TONE_HIGH_HZ`, equal amplitude). Pass `single_hz` for a
    /// single-tone variant (e.g. for power measurement with a wattmeter, where
    /// a two-tone signal's fluctuating envelope makes a peak reading ambiguous).
    ///
    /// Peak-normalized to the same [`TX_PEAK`] frames use, so the drive level
    /// an operator sets here transfers directly to real traffic.
    pub fn tune_tone(&self, seconds: f32, single_hz: Option<f32>) -> Vec<f32> {
        let sample_rate = self.config.sample_rate as f32;
        let n = (seconds * sample_rate).round().max(0.0) as usize;
        let two_pi = std::f32::consts::TAU;

        let mut samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate;
                match single_hz {
                    Some(freq) => (two_pi * freq * t).sin(),
                    None => {
                        (two_pi * TUNE_TONE_LOW_HZ * t).sin()
                            + (two_pi * TUNE_TONE_HIGH_HZ * t).sin()
                    }
                }
            })
            .collect();

        let peak = samples.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        if peak > 1e-12 {
            let gain = TX_PEAK / peak;
            for s in &mut samples {
                *s *= gain;
            }
        }
        samples
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

        let (_header, payload, _recommended_level) = self
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
                speed_level: f.header.speed_level,
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

    // ── Task 1 (Phase 4): TX level calibration (TUNE) ─────────────────────

    mod tune_tone {
        use super::*;
        use coppa_dsp::fft::FftProcessor;
        use num_complex::Complex32;

        /// Magnitude spectrum of a 1-second buffer at `sample_rate`: with a
        /// 1-second window, FFT bin `k` corresponds to exactly `k` Hz, so tone
        /// frequencies land on exact integer bins with no spectral leakage.
        fn magnitude_spectrum(samples: &[f32], sample_rate: usize) -> Vec<f32> {
            assert_eq!(samples.len(), sample_rate);
            let fft = FftProcessor::new(sample_rate);
            let input: Vec<Complex32> = samples.iter().map(|&s| Complex32::new(s, 0.0)).collect();
            fft.forward(&input).iter().map(|c| c.norm()).collect()
        }

        #[test]
        fn test_duration_correct() {
            let core = CoppaCore::new();
            let sample_rate = core.config().sample_rate;

            assert_eq!(core.tune_tone(1.0, None).len(), sample_rate as usize);
            assert_eq!(
                core.tune_tone(2.5, None).len(),
                (2.5 * sample_rate as f32).round() as usize
            );
            assert_eq!(core.tune_tone(0.0, None).len(), 0);
        }

        #[test]
        fn test_default_duration_matches_cli_default_of_10s() {
            // The CLI/daemon default is 10 seconds (task brief); the engine
            // itself takes an explicit duration, but confirm 10s produces the
            // expected sample count at the default profile's sample rate.
            let core = CoppaCore::new();
            let sample_rate = core.config().sample_rate;
            assert_eq!(core.tune_tone(10.0, None).len(), sample_rate as usize * 10);
        }

        #[test]
        fn test_peak_equals_tx_peak() {
            let core = CoppaCore::new();
            let samples = core.tune_tone(1.0, None);
            let peak = samples.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                (peak - TX_PEAK).abs() < 1e-4,
                "two-tone peak {} should equal TX_PEAK {}",
                peak,
                TX_PEAK
            );

            let single = core.tune_tone(1.0, Some(1500.0));
            let single_peak = single.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                (single_peak - TX_PEAK).abs() < 1e-4,
                "single-tone peak {} should equal TX_PEAK {}",
                single_peak,
                TX_PEAK
            );
        }

        #[test]
        fn test_two_tone_buffer_is_exactly_700_and_1900_hz_equal_amplitude() {
            let core = CoppaCore::new();
            let sample_rate = core.config().sample_rate as usize;
            let samples = core.tune_tone(1.0, None);
            let mag = magnitude_spectrum(&samples, sample_rate);

            let bin_low = mag[TUNE_TONE_LOW_HZ as usize];
            let bin_high = mag[TUNE_TONE_HIGH_HZ as usize];
            assert!(
                bin_low > 0.0 && bin_high > 0.0,
                "both tones must be present"
            );
            assert!(
                (bin_low - bin_high).abs() / bin_low.max(bin_high) < 0.01,
                "tones should be equal amplitude: {} vs {}",
                bin_low,
                bin_high
            );

            // A real-valued signal's spectrum mirrors around Nyquist; the tone
            // energy should live entirely in these four bins (mirror images
            // included). Nothing else should carry meaningful energy.
            let mirror_low = mag[sample_rate - TUNE_TONE_LOW_HZ as usize];
            let mirror_high = mag[sample_rate - TUNE_TONE_HIGH_HZ as usize];
            let tone_energy = bin_low + bin_high + mirror_low + mirror_high;
            let total_energy: f32 = mag.iter().sum();
            assert!(
                tone_energy / total_energy > 0.98,
                "tone energy should dominate the spectrum: {} / {}",
                tone_energy,
                total_energy
            );
        }

        #[test]
        fn test_single_tone_variant_is_exactly_one_frequency() {
            let core = CoppaCore::new();
            let sample_rate = core.config().sample_rate as usize;
            let samples = core.tune_tone(1.0, Some(1500.0));
            let mag = magnitude_spectrum(&samples, sample_rate);

            let bin = mag[1500];
            assert!(bin > 0.0, "1500 Hz tone must be present");

            // Every other bin (excluding the tone's mirror image) should be
            // negligible in comparison.
            let mirror = mag[sample_rate - 1500];
            let tone_energy = bin + mirror;
            let total_energy: f32 = mag.iter().sum();
            assert!(
                tone_energy / total_energy > 0.98,
                "single tone should dominate the spectrum: {} / {}",
                tone_energy,
                total_energy
            );

            // The two-tone frequencies should carry no meaningful energy here.
            assert!(mag[TUNE_TONE_LOW_HZ as usize] / bin < 0.01);
            assert!(mag[TUNE_TONE_HIGH_HZ as usize] / bin < 0.01);
        }
    }
}
