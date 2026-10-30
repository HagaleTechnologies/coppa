//! Main Coppa engine: thin wrapper around CoppaTransceiver with compression and squelch.

use anyhow::Result;

use coppa_codec::ofdm::coppa_modem::{SPEED_LEVELS, TX_PEAK};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::compression::huffman::HuffmanCodec;
use coppa_protocol::compression::lz4::{lz4_compress, lz4_decompress};
use coppa_protocol::cp_negotiator::CpMode;
use coppa_protocol::modem::streaming::StreamingReceiver;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

use crate::config::{EngineConfig, OfdmBand, HF_MAX_SPEED_LEVEL};
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
/// `payload` is raw bytes: the frame's payload after this engine's optional
/// Huffman+LZ4 decompression (if the compression marker byte is present), with
/// NO UTF-8 conversion. This deliberately differs from `decode`'s batch-path
/// contract (`decode` -> `decode_bytes` + `String::from_utf8`), which is a
/// separate, text-message-only API used only by `coppa-cli`. `push_samples` is
/// the daemon's and FFI's real streaming-decode path, and real session/ARQ
/// traffic (`MacPdu`/`TransportPdu`, especially once Huffman+LZ4-compressed) is
/// essentially never valid UTF-8 — forcing a UTF-8 conversion here silently
/// dropped every such frame (Phase 4 Task 3.5 fix: this field used to be
/// `message: Result<String>` and forced exactly that conversion). Callers that
/// want a text message (e.g. FFI's existing text-message contract) can still
/// attempt `String::from_utf8` themselves on the raw bytes.
#[derive(Debug)]
pub struct StreamFrame {
    pub payload: Result<Vec<u8>>,
    /// Real per-carrier-noise SNR estimate (dB) from `CoppaTransceiver::
    /// receive_with_metrics`, `10*log10(1/mean(noise_vars))` — NOT the crude
    /// whole-buffer RMS proxy `handle_audio_in` used to compute before Task 7.
    pub snr_db: f32,
    pub cfo_hz: f32,
    pub frame_start: u64,
    /// Speed level (1-9) the transmitting station encoded this frame at, from the
    /// decoded protected header (`DecodedFrame::header.speed_level`). Added for
    /// Phase 3 Task 7 so daemon/host telemetry (WebSocket `status`'s `level` field)
    /// can report the link's real current speed level rather than a placeholder.
    pub speed_level: u8,
    /// This station's recommended speed level for its PEER's next transmission
    /// to it (`DecodedFrame::recommended_level`) — fed back on an outgoing ACK so
    /// `coppa_ml::RateLoop` on the peer's side can apply it.
    pub recommended_level: u8,
    /// This frame's measured multipath delay spread (ms), from
    /// `DecodedFrame::delay_spread_ms`. Feeds `coppa_ml::CpGate` in the daemon —
    /// see `docs/superpowers/specs/2026-07-25-cpgate-daemon-wiring-design.md`.
    pub delay_spread_ms: f32,
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
    /// The profile's `ofdm_profile` field ("hf" or "vhf") pins the engine's OFDM
    /// band via [`EngineConfig::from_profile`], so the band holds across every
    /// later `set_speed_level`/`set_cp_profile` rebuild instead of being
    /// re-derived from the speed level (COP-23).
    pub fn from_profile(profile: &Profile) -> Self {
        Self::build(EngineConfig::from_profile(profile))
    }

    /// Build the engine from a config, selecting the appropriate OFDM profile.
    fn build(config: EngineConfig) -> Self {
        let ofdm_profile =
            Self::select_ofdm_profile(config.speed_level, config.cp_mode, config.ofdm_band);
        let transceiver = CoppaTransceiver::new(ofdm_profile.clone(), 1);
        let streaming = StreamingReceiver::new(ofdm_profile, 1);
        Self {
            transceiver,
            config,
            streaming,
        }
    }

    /// Whether `speed_level` on an engine bound to `band` resolves to the VHF
    /// profile. The one place the HF/VHF decision is made, shared by
    /// `select_ofdm_profile` and `on_vhf_profile` so the two cannot disagree.
    pub(crate) fn resolves_to_vhf(speed_level: u8, band: OfdmBand) -> bool {
        match band {
            OfdmBand::Vhf => true,
            OfdmBand::Hf => false,
            OfdmBand::LevelDerived => speed_level >= 5,
        }
    }

    pub(crate) fn level_allowed_on_band(level: u8, band: OfdmBand) -> bool {
        Self::resolves_to_vhf(level, band) || level <= HF_MAX_SPEED_LEVEL
    }

    /// Select the OFDM profile for a speed level, negotiated CP mode, and band.
    ///
    /// The band decides HF vs VHF (see `resolves_to_vhf`): an `OfdmBand::Hf`
    /// engine stays on HF at every level, an `OfdmBand::Vhf` engine stays on
    /// VHF at every level, and an `OfdmBand::LevelDerived` engine uses HF for
    /// levels 1-4 and VHF for 5+. On HF the profile is HF standard, long or
    /// short CP per `cp_mode`; on VHF it is VHF wide regardless of `cp_mode`
    /// (no VHF short-CP variant exists).
    pub(crate) fn select_ofdm_profile(
        speed_level: u8,
        cp_mode: CpMode,
        ofdm_band: OfdmBand,
    ) -> CoppaProfile {
        if Self::resolves_to_vhf(speed_level, ofdm_band) {
            CoppaProfile::vhf_wide()
        } else {
            match cp_mode {
                CpMode::LongCp => CoppaProfile::hf_standard(),
                CpMode::ShortCp => CoppaProfile::hf_standard_short_cp(),
            }
        }
    }

    /// Whether the currently selected OFDM profile is the VHF one -- i.e. whether
    /// the negotiated HF CP mode (`cp_mode()`) is dormant rather than on air.
    ///
    /// Always `false` for an `OfdmBand::Hf` engine and always `true` for an
    /// `OfdmBand::Vhf` one, whatever the speed level; only an
    /// `OfdmBand::LevelDerived` engine (no band preference) crosses at level 5.
    pub fn on_vhf_profile(&self) -> bool {
        Self::resolves_to_vhf(self.config.speed_level, self.config.ofdm_band)
    }

    /// Occupied bandwidth of the OFDM profile in use, in Hz: active carriers
    /// times carrier spacing (2400 on the HF profiles, long or short CP; 5600
    /// on VHF wide). The VARA `CONNECTED` line reports it.
    pub fn occupied_bandwidth_hz(&self) -> u32 {
        Self::profile_occupied_hz(self.transceiver.profile())
    }

    /// Spectrum interval [low, high), in Hz, covering the active carriers.
    /// The exclusive upper edge is one carrier spacing above the last carrier.
    pub fn occupied_band_edges_hz(&self) -> (f32, f32) {
        let profile = self.transceiver.profile();
        let spacing = profile.carrier_spacing_hz();
        let first = profile.first_active_bin();
        (
            first as f32 * spacing,
            (first + profile.total_active_carriers()) as f32 * spacing,
        )
    }

    /// Occupied bandwidth, in Hz, of the profile `set_speed_level(level)` would
    /// select on this engine's band and CP mode, without rebuilding anything
    /// (COP-21). A caller that holds RF-frequency evidence asks this before
    /// committing to a level: 5600 for a level-derived engine at level 5+, 2400
    /// for any level on an HF-bound engine. `level` is not validated.
    ///
    /// It shares `select_ofdm_profile` with every rebuild, so the answer cannot
    /// disagree with what `set_speed_level` would build.
    pub fn occupied_bandwidth_hz_at(&self, level: u8) -> u32 {
        Self::profile_occupied_hz(&Self::select_ofdm_profile(
            level,
            self.config.cp_mode,
            self.config.ofdm_band,
        ))
    }

    /// Occupied bandwidth of `profile` in Hz: active carriers times carrier
    /// spacing, rounded to whole Hz. Shared by `occupied_bandwidth_hz`,
    /// `occupied_bandwidth_hz_at` and `crate::ladder`.
    pub(crate) fn profile_occupied_hz(profile: &CoppaProfile) -> u32 {
        (profile.total_active_carriers() as f32 * profile.carrier_spacing_hz()).round() as u32
    }

    /// Encode a text message to audio samples.
    pub fn encode(&self, message: &str) -> Result<Vec<f32>> {
        self.encode_bytes(message.as_bytes())
    }

    /// The exact payload `encode_bytes` would hand the transceiver for
    /// `data`: optionally Huffman+LZ4-compressed with the marker byte
    /// prepended, otherwise `data` unchanged (COP-41 refactor out of
    /// `encode_bytes`, which calls this — the two can never measure and send
    /// different bytes).
    fn compress_payload(&self, data: &[u8]) -> Vec<u8> {
        if self.config.compression_enabled {
            let huffman = HuffmanCodec::new();
            let huffman_bytes = huffman.encode(data);
            let lz4_bytes = lz4_compress(&huffman_bytes);
            let mut out = Vec::with_capacity(1 + lz4_bytes.len());
            out.push(COMPRESSION_MARKER);
            out.extend_from_slice(&lz4_bytes);
            out
        } else {
            data.to_vec()
        }
    }

    /// Exact length of the payload `encode_bytes` would hand the transceiver
    /// for `data` (post-compression, marker included) — without building a
    /// header or synthesizing samples. Callers use this to size writes that
    /// must fit a speed level's payload budget (COP-41): a chunk whose
    /// `encoded_payload_len` is at most `max_payload_for_level(level)` is
    /// exactly a chunk `encode_bytes` will accept at that level.
    pub fn encoded_payload_len(&self, data: &[u8]) -> usize {
        self.compress_payload(data).len()
    }

    /// Encode arbitrary binary data to audio samples.
    ///
    /// Pipeline:
    /// 1. Optionally compress (Huffman + LZ4) with a marker byte
    /// 2. Build CoppaHeader
    /// 3. Transmit via CoppaTransceiver
    pub fn encode_bytes(&self, data: &[u8]) -> Result<Vec<f32>> {
        self.encode_bytes_at(data, self.config.speed_level)
    }

    /// COP-68: `encode_bytes`' exact pipeline (optional Huffman+LZ4
    /// compression with the marker byte, `CoppaFrameType::Data`, `seq_num`/
    /// `fec_type` stamped exactly like every other session frame) at an
    /// explicit `level` (1-9), overriding `self.config.speed_level` for this
    /// one call.
    ///
    /// This is the session fade-hold's fallback encode: a retransmission
    /// whose frame bytes are byte-identical to the original (rebuilt from
    /// the segment data) but whose PHY level drops to the most robust level
    /// whose payload budget still fits the frame. It shares
    /// [`Self::encode_bytes_at`] with [`Self::encode_bytes`] so the two can
    /// never diverge -- unlike [`Self::encode_bytes_at_level`] (Beacon frame
    /// type, no compression, the station-ID/beacon path), which is left
    /// alone for its existing callers.
    ///
    /// Same profile rule as [`Self::encode_bytes_at_level`]: the OFDM
    /// profile is never rebuilt here, so `level` means "most robust
    /// modulation/FEC within the profile already in use", and level 9 is
    /// rejected while this engine is on an HF profile.
    pub fn encode_bytes_with_level(&self, data: &[u8], level: u8) -> Result<Vec<f32>> {
        self.check_level(level, self.on_vhf_profile())?;
        self.encode_bytes_at(data, level)
    }

    /// The shared body of [`Self::encode_bytes`] and
    /// [`Self::encode_bytes_with_level`]: compress, build a `Data` header at
    /// `level`, transmit. Private so the two public entry points (and their
    /// exact-payload twin [`Self::encoded_payload_len`], which shares
    /// `compress_payload`) cannot drift apart.
    fn encode_bytes_at(&self, data: &[u8], level: u8) -> Result<Vec<f32>> {
        let payload = self.compress_payload(data);

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
            bandwidth: self.transceiver.profile().bandwidth_id,
            fec_type: 0,
            speed_level: level,
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

    /// Encode arbitrary binary data to audio samples at an explicit `level`
    /// (1-9), overriding `self.config.speed_level` for this one call.
    ///
    /// Reuses `self.transceiver` directly rather than constructing a second
    /// transceiver/engine instance: `CoppaTransceiver::new` eagerly builds
    /// per-speed-level modulation/FEC state (`LevelComponents`) for *every*
    /// speed level up front, keyed purely off `CoppaHeader::speed_level` --
    /// see `CoppaTransceiver::transmit`. The one thing that stays fixed is the
    /// OFDM PHY layer itself (FFT size/carrier layout/HF-bandpass, chosen by
    /// whichever `CoppaProfile` this engine was built with) -- forcing
    /// `level` here does NOT switch between the HF and VHF profiles. That's
    /// intentional: a level-1 frame sent over a *different* OFDM profile than
    /// the rest of this link's traffic would be a physically different
    /// waveform a fixed-profile `StreamingReceiver` (this engine's own, and
    /// any peer's) couldn't sync to -- level here means "most robust
    /// modulation/FEC within the profile already in use for this link", not
    /// "force HF profile regardless of link config".
    ///
    /// No compression is applied (unlike `encode_bytes`): callers needing a
    /// forced-level frame (station ID / beacon, Phase 4 Task 3) send small,
    /// simple payloads where compression overhead isn't worth the complexity.
    ///
    /// Because the profile is never rebuilt here, the HF ceiling is checked
    /// against the profile already in use: level 9 is rejected whenever this
    /// engine is on an HF profile, including a level-derived engine below level 5.
    pub fn encode_bytes_at_level(&self, data: &[u8], level: u8) -> Result<Vec<f32>> {
        self.check_level(level, self.on_vhf_profile())?;
        if data.len() > u16::MAX as usize {
            anyhow::bail!("Payload too large ({} bytes, max {})", data.len(), u16::MAX);
        }

        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Beacon,
            bandwidth: self.transceiver.profile().bandwidth_id,
            fec_type: 0,
            speed_level: level,
            seq_num: 0,
            payload_len: data.len() as u16,
            codewords: 1,
        };

        let samples = self
            .transceiver
            .transmit(&header, data)
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
    /// engine's Huffman+LZ4 decompression applied to its payload (no UTF-8
    /// conversion — see `StreamFrame::payload`'s doc for why this differs from
    /// `decode`'s batch-path contract).
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<StreamFrame> {
        if self.is_squelched(samples) {
            return Vec::new();
        }
        let frames = self.streaming.push_samples(samples);
        frames
            .into_iter()
            .map(|f| StreamFrame {
                payload: self.decompress_if_needed(f.payload),
                snr_db: f.snr_db,
                cfo_hz: f.cfo_hz,
                frame_start: f.frame_start,
                speed_level: f.header.speed_level,
                recommended_level: f.recommended_level,
                delay_spread_ms: f.delay_spread_ms,
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

    /// Detect a likely compressed frame received with compression disabled.
    /// An exact compressor round trip is still ambiguous with deliberately
    /// constructed raw bytes; a future wire envelope would resolve that.
    pub fn appears_compressed_with_wrong_profile(&self, payload: &[u8]) -> bool {
        if self.config.compression_enabled || payload.first() != Some(&COMPRESSION_MARKER) {
            return false;
        }
        let Ok(huffman_bytes) = lz4_decompress(&payload[1..]) else {
            return false;
        };
        let huffman = HuffmanCodec::new();
        let original = huffman.decode(&huffman_bytes);
        let recompressed = {
            let mut bytes = vec![COMPRESSION_MARKER];
            bytes.extend(lz4_compress(&huffman.encode(&original)));
            bytes
        };
        recompressed == payload
    }

    /// COP-68: pass-through of [`StreamingReceiver::header_only_decodes`] --
    /// how many candidates decoded a protected header but failed payload FEC
    /// since this engine's streaming receiver was built. The daemon reads it
    /// as a before/after delta around each `push_samples` call to treat a
    /// header-only decode as peer evidence for the session fade-hold.
    pub fn header_only_decodes(&self) -> u32 {
        self.streaming.header_only_decodes()
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

    /// Change this engine's configured speed level in place (Phase 4 Task 6,
    /// added for FFI v2's `coppa_engine_set_speed_level`).
    ///
    /// This is not a cheap field mutation: it rebuilds the transceiver and
    /// streaming receiver exactly like `reconfigure` does -- any samples already
    /// buffered in the streaming receiver (not yet resolved into a completed
    /// frame) are discarded as a result.
    ///
    /// The level does NOT change the OFDM band of an engine whose `ofdm_band` is
    /// `OfdmBand::Hf` or `OfdmBand::Vhf` -- which includes every engine built from
    /// a named [`Profile`] -- so an HF-configured engine stays HF-legal at every
    /// level (COP-23; before that, `select_ofdm_profile` re-derived the band from
    /// the level alone and an HF engine went on air as `vhf_wide` at level 5).
    /// Only an `OfdmBand::LevelDerived` engine (the `EngineConfig::default()`, no
    /// band preference) still crosses to VHF at level 5.
    ///
    /// Crossing into VHF does NOT touch `cp_mode`. It does not need to:
    /// `select_ofdm_profile` already ignores `cp_mode` entirely at VHF (see
    /// `test_select_ofdm_profile_ignores_cp_mode_in_vhf_range`), so a retained
    /// `ShortCp` cannot select a nonexistent VHF short-CP profile. `cp_mode` is the
    /// *negotiated HF CP mode*: in effect while the engine is on an HF profile,
    /// retained (dormant) while `on_vhf_profile()` -- so a `RateLoop` excursion
    /// into VHF and back cannot silently discard a mode the peer still believes is
    /// in force. Before COP-2 this reset
    /// was permanent, and dropping back below level 5 rebuilt onto `hf_standard`
    /// while the peer stayed on `hf_standard_short_cp`: a mutually-undecodable link
    /// with no give-up trigger armed anywhere (the negotiation had already
    /// succeeded) and no reachable recovery path.
    ///
    /// Returns an error, leaving this engine's current config untouched, if
    /// `level` is not a valid wire speed level (1-9) or is unavailable on this band -- see
    /// `coppa_protocol::modem::speed_level_components`, the single source of
    /// truth for valid levels that this deliberately defers to rather than
    /// duplicating.
    pub fn set_speed_level(&mut self, level: u8) -> Result<()> {
        self.check_level(level, Self::resolves_to_vhf(level, self.config.ofdm_band))?;
        let mut config = self.config.clone();
        config.speed_level = level;
        self.reconfigure(config);
        Ok(())
    }

    /// Rebind this engine's OFDM band in place (COP-21), keeping its speed level
    /// and negotiated CP mode.
    ///
    /// Like `set_speed_level`, this rebuilds the transceiver and streaming
    /// receiver (any samples buffered mid-frame in the streaming receiver are
    /// discarded). The engine itself stays frequency-blind: a caller that holds
    /// RF-frequency evidence (`coppad` does) decides when to call this, typically
    /// after `occupied_bandwidth_hz_at` shows a level would be wideband.
    ///
    /// Returns an error, leaving this engine's current config untouched, if the
    /// current speed level is not available on `band` (level 9 on HF): lower the
    /// level first.
    pub fn set_ofdm_band(&mut self, band: OfdmBand) -> Result<()> {
        self.check_level(
            self.config.speed_level,
            Self::resolves_to_vhf(self.config.speed_level, band),
        )?;
        let mut config = self.config.clone();
        config.ofdm_band = band;
        self.reconfigure(config);
        Ok(())
    }

    /// Validate `level` for a frame sent on the VHF profile (`on_vhf_profile`)
    /// or on an HF profile, which carries levels 1-8 only.
    fn check_level(&self, level: u8, on_vhf_profile: bool) -> Result<()> {
        coppa_protocol::modem::speed_level_components(level)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        if !on_vhf_profile && level > HF_MAX_SPEED_LEVEL {
            anyhow::bail!(
                "speed level {level} is not available on an HF profile: HF profiles carry levels 1-8 (COP-24)"
            );
        }
        Ok(())
    }

    /// Wire speed levels `set_speed_level` accepts on this engine's configured band.
    /// HF profiles carry levels 1-8; VHF and level-derived engines carry 1-9.
    /// `encode_bytes_at_level` also rejects 9 while a level-derived engine is
    /// still on HF (below level 5), because it never rebuilds the profile.
    pub fn available_speed_levels(&self) -> Vec<u8> {
        SPEED_LEVELS
            .iter()
            .filter(|speed| Self::level_allowed_on_band(speed.level, self.config.ofdm_band))
            .map(|speed| speed.level)
            .collect()
    }

    /// The speed level currently configured for outgoing `encode_bytes` calls.
    /// Added alongside `set_speed_level` (also FFI-v2-motivated) so callers —
    /// including tests — can verify a `set_speed_level` call actually took effect
    /// without reaching into private config state.
    pub fn speed_level(&self) -> u8 {
        self.config.speed_level
    }

    /// Change this engine's configured CP mode in place. Only affects HF
    /// profile selection (has no on-air effect while `on_vhf_profile()`) -- see
    /// `select_ofdm_profile`.
    /// Unlike `set_speed_level`, there's no invalid `CpMode` value to
    /// reject, so this is infallible.
    ///
    /// Like `set_speed_level`, this rebuilds the transceiver and streaming
    /// receiver (any samples buffered mid-frame in the streaming receiver
    /// are discarded).
    pub fn set_cp_profile(&mut self, mode: CpMode) {
        let mut config = self.config.clone();
        config.cp_mode = mode;
        self.reconfigure(config);
    }

    /// The CP mode currently configured for outgoing `encode_bytes` calls.
    ///
    /// While `on_vhf_profile()` is true this reports the *retained negotiated HF
    /// mode*, NOT the profile actually on air -- `select_ofdm_profile` builds
    /// `vhf_wide` there regardless (see `set_speed_level`'s doc for why the mode is
    /// retained rather than reset). That happens at `speed_level() >= 5` only for
    /// an `OfdmBand::LevelDerived` engine; an `OfdmBand::Hf` engine keeps this
    /// mode on air at every level (COP-23). A caller reading this to infer the live
    /// waveform must check `on_vhf_profile()` first; it answers "what CP mode did
    /// this station and its peer agree on", which is what COP-2's
    /// engine/negotiator invariant is about.
    pub fn cp_mode(&self) -> CpMode {
        self.config.cp_mode
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
    fn compression_probe_requires_exact_compressor_output_not_just_marker() {
        let plain = CoppaCore::new();
        let compressed = CoppaCore::with_config(EngineConfig {
            compression_enabled: true,
            ..Default::default()
        });
        let payload = compressed.compress_payload(b"CQ CQ DE W5AU");
        assert!(plain.appears_compressed_with_wrong_profile(&payload));
        assert!(!compressed.appears_compressed_with_wrong_profile(&payload));
        for bytes in [
            vec![0xfe],
            vec![0xfe, 1, 2, 3, 4, 5],
            vec![0xfe, 0, 0, 0, 0, 0],
            vec![0xfe, 0xff, 0xff, 0xff, 0xff],
        ] {
            assert!(
                !plain.appears_compressed_with_wrong_profile(&bytes),
                "{bytes:?}"
            );
        }
        // The actual L1_awgn12 golden payload starts with the marker byte.
        let golden_hex = "fe6780024ccce40d8954f8265b2991487e4d4ddcb869b6ad36e768b345326da44baf351a463bd401589f5dc5a88f9238580190af6d873868";
        let golden: Vec<u8> = golden_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert!(!plain.appears_compressed_with_wrong_profile(&golden));
        let mut corrupted = payload;
        corrupted.push(0);
        assert!(!plain.appears_compressed_with_wrong_profile(&corrupted));
    }

    #[test]
    fn test_engine_creation() {
        let core = CoppaCore::new();
        assert_eq!(core.config.sample_rate, 48000);
    }

    #[test]
    fn occupied_band_edges_are_profile_true() {
        let mut core =
            CoppaCore::with_config(EngineConfig::from_profile(&crate::profiles::HF_STANDARD));
        assert_eq!(core.occupied_band_edges_hz(), (350.0, 2750.0));
        core.set_ofdm_band(OfdmBand::Vhf).unwrap();
        assert_eq!(core.occupied_band_edges_hz(), (350.0, 5950.0));
        core.set_ofdm_band(OfdmBand::Hf).unwrap();
        assert_eq!(core.occupied_band_edges_hz(), (350.0, 2750.0));
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

    /// COP-41: `encoded_payload_len` is exact — for a matrix of input
    /// flavors × levels {1, 2, 9} × compression on/off,
    /// `encoded_payload_len(p) <= max_payload_for_level(l)` holds iff
    /// `set_speed_level(l)` + `encode_bytes(p)` succeeds. This is the
    /// invariant the daemon's TX chunker is built on, and it pins the
    /// `compress_payload` extraction (`encode_bytes` must measure and send
    /// the same bytes). Input lengths are swept across every budget on the
    /// ladder (56/117/198) so both directions of the iff are exercised.
    #[test]
    fn encoded_payload_len_predicts_encode_bytes_acceptance() {
        use coppa_protocol::modem::max_payload_for_level;

        // Deterministic input flavors spanning the compression pipeline's
        // behavior space (see COP-41's research probes): ham text compresses
        // well, LCG-random bytes are incompressible, all-unknown-high-bytes
        // are escape-heavy, and empty is the degenerate case.
        let mut seed: u32 = 0xC0_FF_EE;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            seed
        };
        let base_flavors: Vec<Vec<u8>> = vec![
            b"cq de vk3abc qth melbourne winlink b2f chunk counter 000123456789 abcdefg".to_vec(),
            (0..220).map(|_| (next() >> 8) as u8).collect(),
            (0..220).map(|i| 0x80 | (i % 0x40) as u8).collect(),
            Vec::new(),
        ];
        for compression in [false, true] {
            for level in [1u8, 2, 9] {
                let budget = max_payload_for_level(level).unwrap();
                let mut core = CoppaCore::with_config(EngineConfig {
                    compression_enabled: compression,
                    ..Default::default()
                });
                core.set_speed_level(level)
                    .expect("levels 1/2/9 are valid on a level-derived engine");
                for flavor in &base_flavors {
                    for len in [0usize, 20, 40, 56, 80, 117, 150, 200, 220] {
                        let input = &flavor[..len.min(flavor.len())];
                        let predicted_fits = core.encoded_payload_len(input) <= budget;
                        let encoded_ok = core.encode_bytes(input).is_ok();
                        assert_eq!(
                            predicted_fits, encoded_ok,
                            "compression={compression} level={level} len={len}: \
                             encoded_payload_len must predict encode_bytes acceptance"
                        );
                    }
                }
            }
        }
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

    /// Regression test for a confirmed real bug: a spurious sync candidate near
    /// the tail of frame 1 fails `StreamingReceiver::header_peek`, and the
    /// resulting one-symbol-length skip-ahead used to overshoot the real frame
    /// 2 candidate -- already queued by the time the failure was resolved -- by
    /// a handful of samples, dropping it as stale and silently losing the
    /// frame. Confirmed via direct instrumentation at VHF speed level 7 with a
    /// 30ms inter-frame gap: a spurious candidate advanced `resume_from` one
    /// sample past the real candidate's `frame_start`. Deterministic at this
    /// exact level/gap/payload combination (verified repeatedly) -- reproduced
    /// directly here rather than swept. Exercises the same `CoppaCore::
    /// encode_bytes` -> `push_samples` path the original bug report used; a
    /// raw `CoppaTransceiver::transmit`-built frame does not reproduce this
    /// (content differences from `encode_bytes`'s own framing/compression
    /// evidently matter to whether the spurious candidate lands in the fatal
    /// window), so this test intentionally goes through the public engine API
    /// rather than `coppa-protocol`'s lower-level primitives.
    #[test]
    fn second_frame_survives_spurious_tail_candidate_at_level7() {
        let config = EngineConfig {
            speed_level: 7,
            ..Default::default()
        };
        let mut core = CoppaCore::with_config(config);

        let payload1: Vec<u8> = (0..15u8).collect();
        let payload2: Vec<u8> = (0..15u8).map(|i| (i * 7 + 3) % 251).collect();
        let samples1 = core.encode_bytes(&payload1).expect("encode payload1");
        let samples2 = core.encode_bytes(&payload2).expect("encode payload2");

        // 30ms gap at 48kHz -- the confirmed-failing gap length from direct
        // instrumentation of this exact repro.
        let gap = vec![0.0f32; 30 * 48];

        let mut chunk1 = samples1;
        chunk1.extend_from_slice(&gap);
        let frames1 = core.push_samples(&chunk1);
        assert_eq!(
            frames1.len(),
            1,
            "frame 1 should decode on its own chunk, got {} frames",
            frames1.len()
        );
        assert_eq!(
            frames1[0].payload.as_ref().expect("payload1 decompresses"),
            &payload1
        );

        let mut chunk2 = samples2;
        chunk2.extend_from_slice(&gap);
        let frames2 = core.push_samples(&chunk2);
        assert_eq!(
            frames2.len(),
            1,
            "frame 2 must decode; a spurious tail candidate near frame 1's end \
             must not cause the real second frame to be dropped, got {} frames",
            frames2.len()
        );
        assert_eq!(
            frames2[0].payload.as_ref().expect("payload2 decompresses"),
            &payload2
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
            frames[0].payload.as_deref().unwrap(),
            "Streaming works".as_bytes(),
            "push_samples should recover the encoded message's raw bytes"
        );
        assert!(
            frames[0].snr_db.is_finite(),
            "snr_db should be a finite estimate on a clean channel"
        );
        assert!(
            frames[0].delay_spread_ms.is_finite() && frames[0].delay_spread_ms >= 0.0,
            "delay_spread_ms should be a finite, non-negative estimate on a clean channel"
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
        assert_eq!(frames[0].payload.as_deref().unwrap(), message.as_bytes());
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
        assert_eq!(
            frames[0].payload.as_deref().unwrap(),
            "Chunked streaming".as_bytes()
        );
    }

    // ── Task 3.5 (Phase 4): streaming decode must not force UTF-8 ─────────
    //
    // Regression tests for a real, pre-existing bug: `StreamFrame` used to be
    // `message: Result<String>`, and `push_samples` forced
    // `String::from_utf8` on every decoded frame's payload. Real binary
    // session/ARQ traffic (`MacPdu`/`TransportPdu`, especially once this
    // engine's own Huffman+LZ4 compression is applied) is essentially never
    // valid UTF-8, so every such frame landed in the `Err` branch and was
    // silently dropped by both real consumers (`coppa-daemon`,
    // `coppa-ffi`) -- real session establishment over actual RF audio
    // through the daemon's real streaming decode path has likely never
    // worked. Fixed by switching `StreamFrame::message: Result<String>` to
    // `StreamFrame::payload: Result<Vec<u8>>` with no UTF-8 conversion.

    #[test]
    fn test_push_samples_recovers_non_utf8_binary_payload() {
        // A payload containing an invalid UTF-8 byte sequence (0xFF/0xFE are
        // never valid UTF-8 in this position, per se). Under the OLD code,
        // `frames[0].message` would have been `Err(FromUtf8Error)` here and
        // the frame would have been unusable to any caller that only
        // handled `Ok` (which is exactly what both real consumers did).
        let mut core = CoppaCore::new();
        let data: Vec<u8> = vec![b'A', b'B', 0xFF, 0xFE, b'C', 0x00, 0x01, 0x02, b'D'];
        assert!(
            String::from_utf8(data.clone()).is_err(),
            "test payload must actually be invalid UTF-8 for this regression test to mean anything"
        );

        let samples = core.encode_bytes(&data).expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1, "expected exactly one decoded frame");
        assert_eq!(
            frames[0].payload.as_deref().unwrap(),
            data.as_slice(),
            "binary payload should roundtrip exactly via push_samples, not be \
             silently dropped as a UTF-8 conversion error"
        );
    }

    #[test]
    fn test_push_samples_recovers_real_binary_mac_pdu() {
        // The exact real-world shape this bug broke: a `MacPdu` (packed
        // 6-bit callsigns + a binary payload) pushed through the engine's
        // real streaming decode path, then re-parsed with `MacPdu::from_bytes`
        // exactly like `coppa-daemon::event_loop::decode_and_dispatch_audio`
        // does. Under the OLD code this would have failed at the UTF-8
        // conversion step, before `MacPdu::from_bytes` ever ran.
        use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};

        let mut core = CoppaCore::new();
        let dest = Callsign::new("VK3ABC").unwrap();
        let src = Callsign::new("W1AW").unwrap();
        // Binary inner payload guaranteed not to be valid UTF-8.
        let inner_payload: Vec<u8> = vec![0xFF, 0xFE, 0x00, 0x80, 0x81, 0xC0];
        let mac_pdu = MacPdu::new(MacFrameType::Data, dest, src, 0, inner_payload);
        let pdu_bytes = mac_pdu.to_bytes();

        let samples = core
            .encode_bytes(&pdu_bytes)
            .expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1, "expected exactly one decoded frame");

        let decoded_payload = frames[0]
            .payload
            .as_deref()
            .expect("payload should decode successfully, not be dropped as a UTF-8 error");
        assert_eq!(decoded_payload, pdu_bytes.as_slice());

        let decoded_pdu =
            MacPdu::from_bytes(decoded_payload).expect("MacPdu::from_bytes should parse cleanly");
        assert_eq!(decoded_pdu.frame_type, MacFrameType::Data);
        assert_eq!(decoded_pdu.dest.as_str(), "VK3ABC");
        assert_eq!(decoded_pdu.src.as_str(), "W1AW");
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

    // ── Task 3 (Phase 4): forced-level encoding (station ID / beacon) ─────

    #[test]
    fn test_encode_bytes_at_level_forces_level_regardless_of_config() {
        // Engine configured at speed_level 3, but we force level 1.
        let config = EngineConfig {
            speed_level: 3,
            ..Default::default()
        };
        let core = CoppaCore::with_config(config);
        let data = b"VK3ABC";
        let samples = core
            .encode_bytes_at_level(data, 1)
            .expect("encode at forced level 1 should succeed");
        assert!(!samples.is_empty());

        // Decoding via the transceiver directly should report speed_level 1,
        // not the engine's configured level 3.
        let decoded = core.decode_bytes(&samples).expect("decode should succeed");
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_encode_bytes_at_level_roundtrips_via_push_samples() {
        let mut core = CoppaCore::new(); // default speed_level 1
        let data = b"ID payload";
        let samples = core
            .encode_bytes_at_level(data, 1)
            .expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].speed_level, 1);
    }

    #[test]
    fn stream_frame_carries_recommended_level() {
        let config = EngineConfig {
            speed_level: 2,
            ..Default::default()
        };
        let mut core = CoppaCore::with_config(config);
        let samples = core.encode_bytes(b"hello").expect("encode should succeed");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1);
        // A clean loopback should recommend a real level in the valid 1-9 range,
        // not a placeholder/zero value.
        let level = frames[0].recommended_level;
        assert!(
            (1..=9).contains(&level),
            "expected a valid recommended level, got {level}"
        );
    }

    #[test]
    fn test_encode_bytes_at_level_rejects_oversized_payload() {
        let core = CoppaCore::new();
        let data = vec![0u8; u16::MAX as usize + 1];
        assert!(core.encode_bytes_at_level(&data, 1).is_err());
    }

    // ── COP-68: compression-preserving forced-level encode (fade hold) ─────

    /// COP-68: `encode_bytes_with_level` keeps `encode_bytes`' compression
    /// pipeline and Data frame type, so a fallback frame decodes through the
    /// normal streaming path with the forced level in the header and the
    /// original payload after decompression.
    #[test]
    fn encode_bytes_with_level_roundtrips_with_compression() {
        let mut core = CoppaCore::with_config(EngineConfig {
            speed_level: 4,
            compression_enabled: true,
            ..Default::default()
        });
        let data = b"hold at level one";
        let samples = core
            .encode_bytes_with_level(data, 1)
            .expect("small payload fits level 1's budget");
        let samples = with_lead_and_trail(&samples);
        let frames = core.push_samples(&samples);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].speed_level, 1,
            "the fallback level must be the one in the header"
        );
        assert_eq!(
            frames[0].payload.as_ref().expect("decode succeeds"),
            data,
            "the compressed fallback frame must round-trip to the original bytes"
        );
    }

    /// COP-68: the forced level is validated against the profile in use and
    /// the target level's payload budget applies exactly as `encode_bytes`'s
    /// own does.
    #[test]
    fn encode_bytes_with_level_rejects_invalid_level_and_oversized_payload() {
        let core = CoppaCore::new();
        for level in [0u8, 10] {
            assert!(
                core.encode_bytes_with_level(b"x", level).is_err(),
                "level {level} is not a wire level"
            );
        }
        assert!(
            core.encode_bytes_with_level(&[0u8; 80], 1).is_err(),
            "80 encoded bytes cannot fit level 1's 56-byte budget"
        );
        core.encode_bytes_with_level(&[0u8; 80], 2)
            .expect("level 2's 117-byte budget fits 80 bytes");
    }

    // ── Task 6 (Phase 4): `set_speed_level` mutator for FFI v2 ────────────

    #[test]
    fn available_speed_levels_follow_the_configured_band() {
        for profile in [&crate::HF_STANDARD, &crate::HF_ROBUST, &crate::EMERGENCY] {
            let mut core = CoppaCore::from_profile(profile);
            assert_eq!(core.available_speed_levels(), (1..=8).collect::<Vec<_>>());
            core.set_speed_level(8).expect("HF accepts 64-QAM 2/3");
        }
        for mut core in [CoppaCore::from_profile(&crate::VHF_FAST), CoppaCore::new()] {
            assert_eq!(core.available_speed_levels(), (1..=9).collect::<Vec<_>>());
            core.set_speed_level(9).expect("VHF accepts 64-QAM 5/6");
        }
        for name in crate::profiles::list_profiles() {
            let profile = crate::profiles::get_profile(name).unwrap();
            assert!(CoppaCore::from_profile(profile)
                .available_speed_levels()
                .contains(&profile.speed_level));
        }
    }

    #[test]
    fn explicit_level_encoding_rejects_invalid_wire_levels() {
        let core = CoppaCore::new();
        for level in [0, 10, 255] {
            assert!(core.encode_bytes_at_level(b"beacon", level).is_err());
        }
    }

    #[test]
    fn hf_engine_rejects_vhf_only_level_without_mutating_config() {
        for profile in [&crate::HF_STANDARD, &crate::HF_ROBUST, &crate::EMERGENCY] {
            let mut core = CoppaCore::from_profile(profile);
            let before = core.speed_level();
            assert!(core.set_speed_level(9).is_err());
            assert_eq!(core.speed_level(), before);
            assert!(core.encode_bytes_at_level(b"HF beacon", 9).is_err());
            core.set_speed_level(8).expect("HF accepts its top level");
        }
    }

    /// `encode_bytes_at_level` never rebuilds the transceiver, so a level-derived
    /// engine still on `hf_standard` (level < 5) must not emit level 9 on HF.
    #[test]
    fn level_derived_engine_on_hf_profile_rejects_forced_level_9() {
        let mut core = CoppaCore::new();
        assert!(!core.on_vhf_profile());
        assert!(core.encode_bytes_at_level(b"HF beacon", 9).is_err());
        core.encode_bytes_at_level(b"HF beacon", 8)
            .expect("HF accepts its top level");
        core.set_speed_level(5).unwrap();
        assert!(core.on_vhf_profile());
        core.encode_bytes_at_level(b"VHF beacon", 9)
            .expect("level 9 is allowed once the transceiver is on vhf_wide");
    }

    #[test]
    fn test_set_speed_level_valid_updates_config_and_roundtrips() {
        let mut core = CoppaCore::new(); // default speed_level 1
        core.set_speed_level(3).expect("level 3 is valid");
        assert_eq!(core.config().speed_level, 3);

        let data = b"level three";
        let samples = core.encode_bytes(data).expect("encode should succeed");
        let decoded = core.decode_bytes(&samples).expect("decode should succeed");
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_set_speed_level_rejects_removed_level_10() {
        let mut core = CoppaCore::new();
        let before = core.config().speed_level;
        assert!(core.set_speed_level(10).is_err(), "level 10 is invalid");
        assert_eq!(
            core.config().speed_level,
            before,
            "rejected level must not mutate the existing config"
        );
    }

    #[test]
    fn test_set_speed_level_rejects_out_of_range() {
        let mut core = CoppaCore::new();
        assert!(core.set_speed_level(0).is_err());
        assert!(core.set_speed_level(11).is_err());
        assert!(core.set_speed_level(255).is_err());
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

    #[test]
    fn test_select_ofdm_profile_respects_cp_mode_within_hf_range() {
        use coppa_protocol::cp_negotiator::CpMode;
        let long_cp = CoppaCore::select_ofdm_profile(2, CpMode::LongCp, OfdmBand::LevelDerived);
        let short_cp = CoppaCore::select_ofdm_profile(2, CpMode::ShortCp, OfdmBand::LevelDerived);
        assert_eq!(long_cp.cp_samples, CoppaProfile::hf_standard().cp_samples);
        assert_eq!(
            short_cp.cp_samples,
            CoppaProfile::hf_standard_short_cp().cp_samples
        );
        assert_ne!(long_cp.cp_samples, short_cp.cp_samples);

        // COP-23: an HF-banded engine is in the HF range at EVERY level, so the
        // negotiated CP mode is respected at level 7 too, not ignored.
        let hf_long_cp = CoppaCore::select_ofdm_profile(7, CpMode::LongCp, OfdmBand::Hf);
        let hf_short_cp = CoppaCore::select_ofdm_profile(7, CpMode::ShortCp, OfdmBand::Hf);
        assert_eq!(
            hf_long_cp.cp_samples,
            CoppaProfile::hf_standard().cp_samples
        );
        assert_eq!(
            hf_short_cp.cp_samples,
            CoppaProfile::hf_standard_short_cp().cp_samples
        );
    }

    #[test]
    fn test_select_ofdm_profile_ignores_cp_mode_in_vhf_range() {
        use coppa_protocol::cp_negotiator::CpMode;
        let long_cp = CoppaCore::select_ofdm_profile(5, CpMode::LongCp, OfdmBand::LevelDerived);
        let short_cp = CoppaCore::select_ofdm_profile(5, CpMode::ShortCp, OfdmBand::LevelDerived);
        assert_eq!(long_cp.cp_samples, CoppaProfile::vhf_wide().cp_samples);
        assert_eq!(short_cp.cp_samples, CoppaProfile::vhf_wide().cp_samples);

        // COP-23: a VHF-banded engine is in the VHF range at EVERY level, so a
        // retained ShortCp is ignored at level 1 too.
        let vhf_long_cp = CoppaCore::select_ofdm_profile(1, CpMode::LongCp, OfdmBand::Vhf);
        let vhf_short_cp = CoppaCore::select_ofdm_profile(1, CpMode::ShortCp, OfdmBand::Vhf);
        assert_eq!(vhf_long_cp.cp_samples, CoppaProfile::vhf_wide().cp_samples);
        assert_eq!(vhf_short_cp.cp_samples, CoppaProfile::vhf_wide().cp_samples);
    }

    #[test]
    fn occupied_bandwidth_hz_matches_the_profile_on_air() {
        use coppa_protocol::cp_negotiator::CpMode;
        let mut hf = CoppaCore::from_profile(&crate::profiles::HF_STANDARD);
        assert_eq!(hf.occupied_bandwidth_hz(), 2400); // 48 carriers x 50 Hz
        hf.set_cp_profile(CpMode::ShortCp);
        assert_eq!(hf.occupied_bandwidth_hz(), 2400); // short CP: same carriers
        assert_eq!(
            CoppaCore::from_profile(&crate::profiles::VHF_FAST).occupied_bandwidth_hz(),
            5600 // 112 x 50 Hz
        );
    }

    #[test]
    fn test_set_cp_profile_switches_cp_samples() {
        use coppa_protocol::cp_negotiator::CpMode;
        let mut core = CoppaCore::new(); // speed_level 1, hf_standard, LongCp default
        assert_eq!(core.cp_mode(), CpMode::LongCp);
        core.set_cp_profile(CpMode::ShortCp);
        assert_eq!(core.cp_mode(), CpMode::ShortCp);
        // Encoding still succeeds under the new profile.
        assert!(core.encode_bytes(b"hello").is_ok());
    }

    #[test]
    fn test_set_speed_level_preserves_cp_mode_within_hf_range() {
        use coppa_protocol::cp_negotiator::CpMode;
        let mut core = CoppaCore::new();
        core.set_cp_profile(CpMode::ShortCp);
        core.set_speed_level(3).unwrap(); // still HF range (< 5)
        assert_eq!(
            core.cp_mode(),
            CpMode::ShortCp,
            "a speed-level change within HF range must not silently drop the negotiated CP mode"
        );
    }

    /// COP-2: crossing into VHF must RETAIN the negotiated CP mode, and coming
    /// back down must land on the profile the peer is still listening on.
    ///
    /// This is a **deliberate inversion** of the pinning test that used to live
    /// here (`test_set_speed_level_resets_cp_mode_when_crossing_into_vhf`), not
    /// a test deleted to turn a suite green. The reset it pinned was
    /// redundant -- `select_ofdm_profile` ignores `cp_mode` entirely at
    /// `speed_level >= 5`, which
    /// `test_select_ofdm_profile_ignores_cp_mode_in_vhf_range` (kept unchanged)
    /// asserts directly, so a retained `ShortCp` cannot select a nonexistent
    /// VHF short-CP profile. It was also actively harmful: the reset was
    /// permanent, so a `RateLoop` excursion to level 5 and back rebuilt onto
    /// `hf_standard` (CP 300) while the peer stayed on `hf_standard_short_cp`
    /// (CP 144) -- a mutually-undecodable link with no give-up trigger armed
    /// anywhere (the negotiation had already succeeded) and no reachable
    /// recovery path. See `crates/coppa-daemon/src/event_loop.rs`'s
    /// `a_rate_loop_vhf_excursion_does_not_desync_a_negotiated_short_cp` for
    /// the same property proved end-to-end through the real daemon path.
    ///
    /// BOTH halves assert the PROFILE, not just the field: each reads the wire
    /// `bandwidth` id back out through a receiver built for the profile that half
    /// expects, the same waveform-level style as
    /// `test_encode_bytes_writes_real_bandwidth_id_not_hardcoded_one` below. A
    /// `cp_mode()` accessor check alone would pass even if `select_ofdm_profile`
    /// silently disagreed with the field.
    ///
    /// The level-5 half's waveform assertion matters specifically because this
    /// branch made `speed_level >= 5 && cp_mode == ShortCp` reachable through
    /// `set_speed_level` for the FIRST time, so `select_ofdm_profile`'s VHF branch
    /// is now entered with a retained `ShortCp` on a live engine. The only thing
    /// pinning "a retained ShortCp cannot corrupt a VHF frame" used to be
    /// `test_select_ofdm_profile_ignores_cp_mode_in_vhf_range`, which calls the pure
    /// function directly and never exercises `build()` / `encode_bytes()` -- so the
    /// property was proved of the mapping but never of a real transmitted frame.
    #[test]
    fn test_set_speed_level_preserves_cp_mode_across_a_vhf_excursion() {
        use coppa_protocol::cp_negotiator::CpMode;
        use coppa_protocol::modem::transceiver::CoppaTransceiver;

        let mut core = CoppaCore::new();
        core.set_cp_profile(CpMode::ShortCp);

        core.set_speed_level(5).unwrap(); // crosses into VHF
        assert_eq!(
            core.cp_mode(),
            CpMode::ShortCp,
            "crossing into VHF must RETAIN the negotiated CP mode (dormant while \
             `select_ofdm_profile` builds vhf_wide), not discard it -- the peer \
             still believes it is in force"
        );

        // The retained mode must be DORMANT, not merely retained: a real frame
        // encoded here has to be `vhf_wide` on the wire, decodable by a peer that
        // knows nothing about the CP negotiation.
        let vhf_samples = core.encode_bytes(b"x").unwrap();
        let vhf_rx = CoppaTransceiver::new(CoppaProfile::vhf_wide(), 1);
        let (vhf_header, _payload, _recommended_level) = vhf_rx
            .receive(&vhf_samples)
            .expect("a frame sent while dwelling at level 5 must decode as vhf_wide");
        assert_eq!(
            vhf_header.bandwidth,
            CoppaProfile::vhf_wide().bandwidth_id,
            "a RETAINED ShortCp must not leak into the VHF waveform -- \
             select_ofdm_profile ignores cp_mode at level >= 5"
        );

        core.set_speed_level(3).unwrap(); // back into the HF range
        assert_eq!(core.cp_mode(), CpMode::ShortCp);
        let samples = core.encode_bytes(b"x").unwrap();
        let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard_short_cp(), 1);
        let (header, _payload, _recommended_level) = transceiver
            .receive(&samples)
            .expect("a frame sent after returning from VHF must still decode under short CP");
        assert_eq!(
            header.bandwidth,
            CoppaProfile::hf_standard_short_cp().bandwidth_id,
            "returning below level 5 must rebuild onto hf_standard_short_cp, not hf_standard"
        );
    }

    /// Decode `samples` through a receiver built for `profile` and return the wire
    /// `bandwidth` id, panicking with `context` if it does not decode. The
    /// waveform-level check every COP-23 test below uses: a frame encoded under a
    /// different OFDM profile (e.g. `vhf_wide` vs `hf_standard`) cannot sync here.
    fn decode_under(profile: CoppaProfile, samples: &[f32], context: &str) -> u8 {
        use coppa_protocol::modem::transceiver::CoppaTransceiver;
        let rx = CoppaTransceiver::new(profile, 1);
        let (header, _payload, _recommended_level) = rx
            .receive(samples)
            .unwrap_or_else(|e| panic!("{context}: {e}"));
        header.bandwidth
    }

    /// COP-23, the ticket's Gherkin scenario, executable: given an engine built
    /// from an HF profile, when `RateLoop` raises the speed level to 5 or above,
    /// then the OFDM profile stays HF-legal. Before the fix `select_ofdm_profile`
    /// re-derived the band from the level alone and built `vhf_wide` here.
    #[test]
    fn hf_configured_engine_stays_hf_across_a_speed_level_raise() {
        let mut core = CoppaCore::from_profile(&crate::profiles::HF_STANDARD);
        for level in [5u8, 6, 7, 8] {
            core.set_speed_level(level).unwrap();
            let samples = core.encode_bytes(b"stay on HF").unwrap();
            let bandwidth = decode_under(
                CoppaProfile::hf_standard(),
                &samples,
                &format!("an HF-configured engine at level {level} must still be on hf_standard"),
            );
            assert_eq!(bandwidth, CoppaProfile::hf_standard().bandwidth_id);
        }
        assert!(core.set_speed_level(9).is_err());
    }

    #[test]
    fn vhf_configured_engine_stays_vhf_across_a_speed_level_drop() {
        let mut core = CoppaCore::from_profile(&crate::profiles::VHF_FAST);
        core.set_speed_level(2).unwrap();
        let samples = core.encode_bytes(b"stay on VHF").unwrap();
        let bandwidth = decode_under(
            CoppaProfile::vhf_wide(),
            &samples,
            "a VHF-configured engine dropped to level 2 must still be on vhf_wide",
        );
        assert_eq!(bandwidth, CoppaProfile::vhf_wide().bandwidth_id);
    }

    /// The FFI/CLI construction path: they never call `CoppaCore::from_profile`,
    /// they build `EngineConfig::from_profile` and hand it to `with_config`. Before
    /// COP-23 that path lost the band at construction, so this was `vhf_wide`.
    #[test]
    fn with_config_from_hf_profile_stays_hf_at_level_8() {
        let mut core =
            CoppaCore::with_config(EngineConfig::from_profile(&crate::profiles::HF_ROBUST));
        core.set_speed_level(8).unwrap();
        let samples = core.encode_bytes(b"level eight on HF").unwrap();
        let bandwidth = decode_under(
            CoppaProfile::hf_standard(),
            &samples,
            "an HF_ROBUST-configured engine at level 8 must still be on hf_standard",
        );
        assert_eq!(bandwidth, CoppaProfile::hf_standard().bandwidth_id);
    }

    /// COP-23: on an HF-configured engine the negotiated short CP is no longer
    /// dormant above level 5 -- the engine never leaves HF, so the mode stays on air.
    #[test]
    fn hf_configured_engine_keeps_negotiated_short_cp_in_effect_at_high_levels() {
        let mut core = CoppaCore::from_profile(&crate::profiles::HF_ROBUST);
        core.set_cp_profile(CpMode::ShortCp);
        core.set_speed_level(5).unwrap();
        assert!(
            !core.on_vhf_profile(),
            "an HF-configured engine at level 5 must not report the VHF profile"
        );
        let samples = core.encode_bytes(b"short CP at level 5").unwrap();
        let bandwidth = decode_under(
            CoppaProfile::hf_standard_short_cp(),
            &samples,
            "an HF-configured engine with ShortCp negotiated must stay on \
             hf_standard_short_cp at level 5",
        );
        assert_eq!(bandwidth, CoppaProfile::hf_standard_short_cp().bandwidth_id);
    }

    #[test]
    fn on_vhf_profile_truth_table() {
        let at = |speed_level: u8, ofdm_band: OfdmBand| {
            CoppaCore::with_config(EngineConfig {
                speed_level,
                ofdm_band,
                ..Default::default()
            })
            .on_vhf_profile()
        };
        assert!(!at(4, OfdmBand::LevelDerived), "LevelDerived below 5 is HF");
        assert!(at(5, OfdmBand::LevelDerived), "LevelDerived at 5 is VHF");
        assert!(!at(8, OfdmBand::Hf), "Hf stays HF at the top of the ladder");
        assert!(
            at(1, OfdmBand::Vhf),
            "Vhf stays VHF at the bottom of the ladder"
        );
    }

    #[test]
    fn test_encode_bytes_writes_real_bandwidth_id_not_hardcoded_one() {
        use coppa_protocol::cp_negotiator::CpMode;
        use coppa_protocol::modem::transceiver::CoppaTransceiver;

        let mut core = CoppaCore::new(); // hf_standard, bandwidth_id = 1
        core.set_cp_profile(CpMode::ShortCp); // hf_standard_short_cp, bandwidth_id = 4
        let samples = core.encode_bytes(b"x").unwrap();

        // Decode the header back out via CoppaTransceiver::receive (confirmed
        // signature: `pub fn receive(&self, samples: &[f32]) -> Result<(CoppaHeader,
        // Vec<u8>, u8), ReceiveError>`) to confirm the wire bandwidth field is
        // really 4, not the old hardcoded 1.
        let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard_short_cp(), 1);
        let (header, _payload, _recommended_level) =
            transceiver.receive(&samples).expect("frame should decode");
        assert_eq!(
            header.bandwidth,
            CoppaProfile::hf_standard_short_cp().bandwidth_id
        );
    }

    // ── COP-21: band rebinding for frequency-aware callers ────────────────

    #[test]
    fn occupied_bandwidth_hz_at_reports_the_profile_a_level_would_select() {
        let level_derived = CoppaCore::new();
        assert_eq!(level_derived.occupied_bandwidth_hz_at(4), 2400);
        assert_eq!(level_derived.occupied_bandwidth_hz_at(5), 5600);
        assert_eq!(level_derived.occupied_bandwidth_hz_at(9), 5600);
        let hf = CoppaCore::from_profile(&crate::HF_STANDARD);
        assert_eq!(hf.occupied_bandwidth_hz_at(5), 2400);
        assert_eq!(hf.occupied_bandwidth_hz_at(8), 2400);
        let vhf = CoppaCore::from_profile(&crate::VHF_FAST);
        assert_eq!(vhf.occupied_bandwidth_hz_at(1), 5600);
        // A pure query: nothing was rebuilt.
        assert_eq!(level_derived.speed_level(), 1);
        assert_eq!(level_derived.occupied_bandwidth_hz(), 2400);
    }

    #[test]
    fn set_ofdm_band_hf_moves_a_level_derived_engine_off_vhf_wide() {
        let mut core = CoppaCore::new();
        core.set_speed_level(5).unwrap();
        assert!(
            core.on_vhf_profile(),
            "test premise: level-derived level 5 is vhf_wide"
        );
        core.set_ofdm_band(OfdmBand::Hf)
            .expect("level 5 is available on HF");
        assert_eq!(core.config().ofdm_band, OfdmBand::Hf);
        assert_eq!(core.speed_level(), 5, "the level is kept");
        assert!(!core.on_vhf_profile());
        assert_eq!(core.occupied_bandwidth_hz(), 2400);
        let samples = core.encode("cop21").unwrap();
        assert_eq!(
            core.decode(&samples).unwrap(),
            "cop21",
            "a real hf_standard level-5 frame"
        );
        assert_eq!(core.available_speed_levels(), (1..=8).collect::<Vec<_>>());
        core.set_speed_level(8).unwrap();
        assert_eq!(
            core.occupied_bandwidth_hz(),
            2400,
            "later level changes stay on HF"
        );
    }

    #[test]
    fn set_ofdm_band_keeps_the_negotiated_cp_mode() {
        let mut core = CoppaCore::new();
        core.set_cp_profile(CpMode::ShortCp);
        core.set_speed_level(5).unwrap(); // vhf_wide; ShortCp dormant
        core.set_ofdm_band(OfdmBand::Hf).unwrap();
        assert_eq!(core.cp_mode(), CpMode::ShortCp);
        assert_eq!(
            core.transceiver.profile().cp_samples,
            CoppaProfile::hf_standard_short_cp().cp_samples
        );
    }

    #[test]
    fn set_ofdm_band_rejects_a_level_the_band_cannot_carry() {
        let mut core = CoppaCore::new();
        core.set_speed_level(9).unwrap();
        let err = core.set_ofdm_band(OfdmBand::Hf).unwrap_err().to_string();
        assert!(err.contains("not available on an HF profile"), "{err}");
        assert_eq!(core.config().ofdm_band, OfdmBand::LevelDerived, "untouched");
        assert_eq!(core.speed_level(), 9);
        assert!(core.on_vhf_profile());
    }
}
