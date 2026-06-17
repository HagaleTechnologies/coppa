//! Core traits for modulation, constellation mapping, and FEC codecs.
//!
//! These traits are the intended abstraction boundaries of the codec, not a
//! fully plug-and-play framework. How "live" each one is varies:
//!
//! - [`ConstellationMapper`] is genuinely pluggable: it has five implementors
//!   (BPSK, QPSK, 8PSK, 16QAM, 64QAM) and is selected at runtime as a
//!   `Box<dyn ConstellationMapper>` by the speed-level configuration in
//!   `coppa-protocol`.
//! - [`ChannelEstimator`] is used via `&dyn ChannelEstimator` by the OFDM
//!   equalizer, but currently has a single implementor.
//! - [`Modem`] is a uniform interface implemented by `BpskModem` and
//!   `OfdmModem`. It is an extension point rather than the hot path: the
//!   flagship `CoppaModem` does not implement it and is called directly, and
//!   nothing dispatches over `dyn Modem`.
//! - [`FecCodec`] has no implementor in this crate. The real codecs
//!   (convolutional and LDPC) implement it in `coppa-protocol`, and the
//!   reference pipeline wires them in concretely rather than through this
//!   trait. Treat it as the intended FEC abstraction boundary.
use num_complex::Complex32;

/// Maps bits to complex symbols and back, with soft demapping.
///
/// Each implementation handles a specific modulation scheme (BPSK, QPSK, etc.)
/// with Gray coding. The soft demapper produces log-likelihood ratios (LLRs)
/// for use with soft-decision FEC decoders.
pub trait ConstellationMapper: Send {
    /// Number of bits per symbol for this modulation scheme.
    fn bits_per_symbol(&self) -> usize;

    /// Map a group of bits to a complex constellation point.
    /// `bits` must have exactly `bits_per_symbol()` elements, each 0 or 1.
    fn map(&self, bits: &[u8]) -> Complex32;

    /// Map multiple groups of bits to complex symbols.
    fn map_bits(&self, bits: &[u8]) -> Vec<Complex32> {
        let bps = self.bits_per_symbol();
        bits.chunks_exact(bps)
            .map(|chunk| self.map(chunk))
            .collect()
    }

    /// Hard-decision demapping: find the closest constellation point and return its bits.
    fn demap_hard(&self, symbol: Complex32) -> Vec<u8>;

    /// Soft demapping: compute LLRs for each bit given a received symbol and noise variance.
    /// Positive LLR means bit is more likely 0, negative means more likely 1.
    /// Uses max-log-MAP approximation for efficiency.
    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32>;
}

/// Complete modem: modulates bytes to audio samples, demodulates samples to bytes.
pub trait Modem: Send {
    /// Modulate bits to audio samples.
    fn modulate(&self, bits: &[u8]) -> anyhow::Result<Vec<f32>>;

    /// Demodulate audio samples to soft symbols.
    /// Positive = likely bit 0, negative = likely bit 1.
    fn demodulate_soft(&mut self, samples: &[f32]) -> anyhow::Result<Vec<f32>>;

    /// Sample rate in Hz.
    fn sample_rate(&self) -> f32;

    /// Samples per symbol.
    fn samples_per_symbol(&self) -> usize;
}

/// Forward error correction codec.
///
/// Encodes bits with redundancy and decodes soft LLRs back to bits.
pub trait FecCodec: Send {
    /// Code rate as a fraction (e.g., 0.5 for rate-1/2).
    fn rate(&self) -> f32;

    /// Encode input bits, producing coded output bits.
    fn encode(&mut self, bits: &[u8]) -> Vec<u8>;

    /// Decode soft symbols (LLRs) back to information bits.
    /// Positive = likely 0, negative = likely 1.
    fn decode(&self, soft_symbols: &[f32]) -> Vec<u8>;
}

/// Estimates channel frequency response from pilots, used by OFDM equalizer.
pub trait ChannelEstimator: Send {
    /// Update channel estimate from pilot subcarriers.
    /// `pilots` contains (subcarrier_index, received_value, known_value) tuples.
    fn update(&mut self, pilots: &[(usize, Complex32, Complex32)]);

    /// Get the estimated channel response H[k] for a given subcarrier.
    fn estimate(&self, subcarrier: usize) -> Complex32;

    /// Get noise variance estimate (sigma^2).
    fn noise_variance(&self) -> f32;
}
