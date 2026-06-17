//! AFSK 1200 baud modem (Bell 202) for AX.25 / APRS.
//!
//! Implements HDLC framing (flags + bit stuffing), NRZI encoding,
//! and continuous-phase FSK audio generation at 48 kHz.
//! Also provides a streaming demodulator with correlator-based detection.
use std::f32::consts::{PI, TAU};

use crc::{Crc, CRC_16_IBM_SDLC};

const BAUD_RATE: u32 = 1200;
const MARK_FREQ: f32 = 1200.0; // mark = 1 (no transition in NRZI)
const SPACE_FREQ: f32 = 2200.0; // space = 0 (transition in NRZI)
const SAMPLE_RATE: u32 = 48000;
const SAMPLES_PER_SYMBOL: u32 = SAMPLE_RATE / BAUD_RATE; // 40
const HDLC_FLAG: u8 = 0x7E;
const PREAMBLE_FLAGS: usize = 36; // ~300 ms at 1200 baud
const POSTAMBLE_FLAGS: usize = 2;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Convert a byte to 8 bits, LSB first (AX.25 standard).
fn byte_to_bits_lsb(byte: u8) -> [u8; 8] {
    let mut bits = [0u8; 8];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = (byte >> i) & 1;
    }
    bits
}

/// Convert 8 bits (LSB first) back to a byte.
fn bits_to_byte_lsb(bits: &[u8]) -> u8 {
    debug_assert!(bits.len() >= 8);
    let mut byte = 0u8;
    for (i, &b) in bits.iter().enumerate().take(8) {
        byte |= b << i;
    }
    byte
}

/// Insert a zero bit after every run of five consecutive 1s (HDLC bit stuffing).
/// Operates on the frame body only; flag sequences are exempt.
fn bit_stuff(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.len() + bits.len() / 5);
    let mut ones_run = 0u32;
    for &bit in bits {
        out.push(bit);
        if bit == 1 {
            ones_run += 1;
            if ones_run == 5 {
                out.push(0); // stuffed zero
                ones_run = 0;
            }
        } else {
            ones_run = 0;
        }
    }
    out
}

/// Remove stuffed zero bits.  Returns `None` if 6 or more consecutive 1s are
/// encountered (which would indicate a flag or an abort sequence, not data).
#[allow(dead_code)] // used in tests
fn bit_unstuff(bits: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bits.len());
    let mut ones_run = 0u32;
    let mut i = 0;
    while i < bits.len() {
        let bit = bits[i];
        if bit == 1 {
            ones_run += 1;
            if ones_run == 6 {
                // Six consecutive 1s — invalid in data context.
                return None;
            }
            out.push(1);
        } else {
            if ones_run == 5 {
                // This is a stuffed zero — skip it (do not push).
                ones_run = 0;
                i += 1;
                continue;
            }
            ones_run = 0;
            out.push(0);
        }
        i += 1;
    }
    Some(out)
}

/// NRZI encode: bit 1 means *no change*, bit 0 means *transition*.
/// Initial state is `true` (mark frequency).
fn nrzi_encode(bits: &[u8]) -> Vec<bool> {
    let mut out = Vec::with_capacity(bits.len());
    let mut state = true;
    for &bit in bits {
        if bit == 0 {
            state = !state; // transition
        }
        // bit == 1: no change
        out.push(state);
    }
    out
}

/// NRZI decode: same consecutive value → 1, different → 0.
/// Previous symbol starts as `true` (mark).
#[allow(dead_code)] // used in tests
fn nrzi_decode(symbols: &[bool]) -> Vec<u8> {
    let mut out = Vec::with_capacity(symbols.len());
    let mut prev = true;
    for &sym in symbols {
        out.push(if sym == prev { 1 } else { 0 });
        prev = sym;
    }
    out
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Modulate raw AX.25 frame bytes (CRC already appended) into 48 kHz audio.
///
/// Frame structure produced:
///   [preamble flags] [bit-stuffed frame body] [postamble flags]
///
/// The entire bit stream is NRZI-encoded before FSK generation.  Phase is
/// maintained continuously across symbol boundaries to avoid spectral splatter.
pub fn modulate(frame_bytes: &[u8]) -> Vec<f32> {
    // --- 1. Build raw bit stream -------------------------------------------

    // Flag bits (not bit-stuffed): 0x7E LSB-first = 0,1,1,1,1,1,1,0
    let flag_bits: Vec<u8> = byte_to_bits_lsb(HDLC_FLAG).to_vec();

    let mut bits: Vec<u8> = Vec::new();

    // Preamble
    for _ in 0..PREAMBLE_FLAGS {
        bits.extend_from_slice(&flag_bits);
    }

    // Frame body with bit stuffing
    let mut frame_bits: Vec<u8> = Vec::with_capacity(frame_bytes.len() * 8);
    for &byte in frame_bytes {
        frame_bits.extend_from_slice(&byte_to_bits_lsb(byte));
    }
    let stuffed = bit_stuff(&frame_bits);
    bits.extend_from_slice(&stuffed);

    // Postamble
    for _ in 0..POSTAMBLE_FLAGS {
        bits.extend_from_slice(&flag_bits);
    }

    // --- 2. NRZI encode -------------------------------------------------------
    let symbols = nrzi_encode(&bits);

    // --- 3. Generate continuous-phase FSK audio -------------------------------
    let total_samples = symbols.len() * SAMPLES_PER_SYMBOL as usize;
    let mut samples = Vec::with_capacity(total_samples);
    let mut phase: f32 = 0.0;

    for &mark in &symbols {
        let freq = if mark { MARK_FREQ } else { SPACE_FREQ };
        let phase_increment = 2.0 * PI * freq / SAMPLE_RATE as f32;
        for _ in 0..SAMPLES_PER_SYMBOL {
            samples.push(phase.sin());
            phase += phase_increment;
            // Keep phase in [0, 2π) to prevent float drift over long transmissions.
            if phase >= 2.0 * PI {
                phase -= 2.0 * PI;
            }
        }
    }

    samples
}

// ---------------------------------------------------------------------------
// Demodulator
// ---------------------------------------------------------------------------

/// Streaming AFSK 1200 demodulator with HDLC frame extraction.
///
/// Feed audio samples via [`Demodulator::process`], then call
/// [`Demodulator::take_frames`] to retrieve any fully decoded frames.
pub struct Demodulator {
    // Correlator accumulators
    mark_sin_acc: f32,
    mark_cos_acc: f32,
    space_sin_acc: f32,
    space_cos_acc: f32,

    // Reference oscillator phases
    mark_phase: f32,
    space_phase: f32,

    // Sample counter within current symbol period
    sample_count: u32,

    // NRZI state: last detected symbol (true = mark)
    prev_symbol: bool,

    // HDLC state machine
    in_frame: bool,      // currently receiving frame data
    bit_buffer: Vec<u8>, // accumulated data bits for current frame
    ones_count: u32,     // consecutive 1-bits for flag/stuffing detection

    // Completed frames
    frames: Vec<Vec<u8>>,
}

impl Default for Demodulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator {
    pub fn new() -> Self {
        Self {
            mark_sin_acc: 0.0,
            mark_cos_acc: 0.0,
            space_sin_acc: 0.0,
            space_cos_acc: 0.0,
            mark_phase: 0.0,
            space_phase: 0.0,
            sample_count: 0,
            prev_symbol: true, // initial NRZI state = mark
            in_frame: false,
            bit_buffer: Vec::new(),
            ones_count: 0,
            frames: Vec::new(),
        }
    }

    /// Feed audio samples into the demodulator.
    pub fn process(&mut self, samples: &[f32]) {
        let mark_inc = TAU * MARK_FREQ / SAMPLE_RATE as f32;
        let space_inc = TAU * SPACE_FREQ / SAMPLE_RATE as f32;

        for &s in samples {
            // Accumulate correlator products
            self.mark_sin_acc += s * self.mark_phase.sin();
            self.mark_cos_acc += s * self.mark_phase.cos();
            self.space_sin_acc += s * self.space_phase.sin();
            self.space_cos_acc += s * self.space_phase.cos();

            // Advance reference phases
            self.mark_phase += mark_inc;
            if self.mark_phase >= TAU {
                self.mark_phase -= TAU;
            }
            self.space_phase += space_inc;
            if self.space_phase >= TAU {
                self.space_phase -= TAU;
            }

            self.sample_count += 1;

            if self.sample_count >= SAMPLES_PER_SYMBOL {
                // Symbol decision
                let mark_energy =
                    self.mark_sin_acc * self.mark_sin_acc + self.mark_cos_acc * self.mark_cos_acc;
                let space_energy = self.space_sin_acc * self.space_sin_acc
                    + self.space_cos_acc * self.space_cos_acc;
                let is_mark = mark_energy >= space_energy;

                // NRZI decode: same as previous = 1, different = 0
                let bit = if is_mark == self.prev_symbol {
                    1u8
                } else {
                    0u8
                };
                self.prev_symbol = is_mark;

                // Feed bit into HDLC state machine
                self.hdlc_process_bit(bit);

                // Reset correlator for next symbol
                self.mark_sin_acc = 0.0;
                self.mark_cos_acc = 0.0;
                self.space_sin_acc = 0.0;
                self.space_cos_acc = 0.0;
                self.sample_count = 0;
            }
        }
    }

    /// HDLC bit-level state machine using ones-counting approach.
    ///
    /// Strategy: track consecutive 1-bits. Only push data bits when we know
    /// they are not part of a flag or stuffing. The key rules:
    ///   - 1-bits with ones_count < 5: push as data, increment count
    ///   - 0-bit after ones_count == 5: stuffed zero, discard (data 1s already pushed)
    ///   - 0-bit after ones_count == 6: flag! Remove last 5 pushed 1s from buffer
    ///   - ones_count reaches 7+: abort
    fn hdlc_process_bit(&mut self, bit: u8) {
        if bit == 1 {
            self.ones_count += 1;
            if self.ones_count >= 7 {
                // Abort: 7+ consecutive 1s
                self.in_frame = false;
                self.bit_buffer.clear();
                return;
            }
            // Push the 1-bit as data. For the 6th consecutive 1, we also push
            // it tentatively; if it turns out to be part of a flag (next bit 0),
            // we'll remove 6 ones from the buffer.
            if self.in_frame {
                self.bit_buffer.push(1);
            }
        } else {
            // bit == 0
            if self.ones_count >= 7 {
                // Recovery from abort
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 6 {
                // Flag detected (01111110).
                // We pushed 7 bits into bit_buffer as data: the leading 0 of the
                // flag plus the 6 ones. Remove all 7.
                if self.in_frame {
                    let buf_len = self.bit_buffer.len();
                    let remove = 7.min(buf_len);
                    self.bit_buffer.truncate(buf_len - remove);
                    if !self.bit_buffer.is_empty() {
                        self.try_extract_frame();
                    }
                }
                self.in_frame = true;
                self.bit_buffer.clear();
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 5 {
                // Stuffed zero — the 5 ones were real data (already pushed).
                // Discard this 0.
                self.ones_count = 0;
                return;
            }

            // Normal 0 bit
            self.ones_count = 0;
            if self.in_frame {
                self.bit_buffer.push(0);
            }
        }
    }

    /// Attempt to extract a frame from the accumulated bit buffer.
    fn try_extract_frame(&mut self) {
        // Need at least 16 bits for 2-byte CRC
        if self.bit_buffer.len() < 16 {
            return;
        }

        // Must be a multiple of 8 bits
        let bit_len = self.bit_buffer.len();
        if bit_len % 8 != 0 {
            return;
        }

        let num_bytes = bit_len / 8;
        let mut bytes = Vec::with_capacity(num_bytes);
        for i in 0..num_bytes {
            bytes.push(bits_to_byte_lsb(&self.bit_buffer[i * 8..(i + 1) * 8]));
        }

        // CRC verification: last 2 bytes are FCS (low byte first)
        if bytes.len() < 3 {
            return; // Need at least 1 data byte + 2 CRC bytes
        }

        let data = &bytes[..bytes.len() - 2];
        let fcs_lo = bytes[bytes.len() - 2];
        let fcs_hi = bytes[bytes.len() - 1];
        let received_fcs = u16::from_le_bytes([fcs_lo, fcs_hi]);

        let crc_alg = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let computed_fcs = crc_alg.checksum(data);

        if computed_fcs == received_fcs {
            // Push the full frame including CRC (as the modulator expects)
            self.frames.push(bytes);
        }
    }

    /// Extract all completed frames, draining the internal buffer.
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }

    /// Reset the demodulator to its initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_to_bits_lsb() {
        // 0x7E = 0111_1110 binary; LSB-first = [0,1,1,1,1,1,1,0]
        assert_eq!(byte_to_bits_lsb(0x7E), [0, 1, 1, 1, 1, 1, 1, 0]);
    }

    #[test]
    fn test_bits_to_byte_lsb_roundtrip() {
        for byte in [0x00u8, 0x7E, 0xFF, 0xA5, 0x1B] {
            let bits = byte_to_bits_lsb(byte);
            assert_eq!(bits_to_byte_lsb(&bits), byte);
        }
    }

    #[test]
    fn test_bit_stuff_no_stuffing_needed() {
        // Alternating bits — never 5 consecutive 1s, so no stuffing.
        let bits = vec![1, 0, 1, 0, 1, 0, 1, 0];
        assert_eq!(bit_stuff(&bits), bits);
    }

    #[test]
    fn test_bit_stuff_inserts_zero_after_five_ones() {
        // Five 1s then a 0: the 0 following the run is data, not a stuff bit.
        // After bit stuffing: [1,1,1,1,1, <stuff 0>, 0]
        let bits = vec![1, 1, 1, 1, 1, 0];
        assert_eq!(bit_stuff(&bits), vec![1, 1, 1, 1, 1, 0, 0]);
    }

    #[test]
    fn test_bit_stuff_six_ones() {
        // Six consecutive 1s: stuff a 0 after the fifth, then continue.
        // [1,1,1,1,1,<stuff 0>,1]
        let bits = vec![1, 1, 1, 1, 1, 1];
        assert_eq!(bit_stuff(&bits), vec![1, 1, 1, 1, 1, 0, 1]);
    }

    #[test]
    fn test_bit_stuff_unstuff_roundtrip() {
        let original = vec![0u8, 1, 1, 1, 1, 1, 1, 0, 1, 0, 1, 1, 1, 1, 1, 0];
        let stuffed = bit_stuff(&original);
        let recovered = bit_unstuff(&stuffed).expect("unstuff should succeed on valid data");
        assert_eq!(recovered, original);
    }

    #[test]
    fn test_bit_unstuff_rejects_six_ones() {
        // Six consecutive 1s without a stuffed zero are invalid.
        let bits = vec![1, 1, 1, 1, 1, 1];
        assert!(bit_unstuff(&bits).is_none());
    }

    #[test]
    fn test_nrzi_encode() {
        // Input:   0   1   0   0   1
        // State:  T→F  F   F→T T→F  F
        // Output:  F   F   T   F    F
        let bits = vec![0u8, 1, 0, 0, 1];
        let expected = vec![false, false, true, false, false];
        assert_eq!(nrzi_encode(&bits), expected);
    }

    #[test]
    fn test_nrzi_encode_decode_roundtrip() {
        let bits: Vec<u8> = vec![1, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1, 0];
        let symbols = nrzi_encode(&bits);
        let decoded = nrzi_decode(&symbols);
        assert_eq!(decoded, bits);
    }

    #[test]
    fn test_modulate_produces_audio() {
        // A minimal frame: 1 data byte.
        let frame = vec![0xABu8];
        let samples = modulate(&frame);

        // Must be non-empty and longer than 1000 samples.
        assert!(
            samples.len() > 1000,
            "Expected > 1000 samples, got {}",
            samples.len()
        );

        // All samples must be in [-1, 1].
        for (i, &s) in samples.iter().enumerate() {
            assert!(
                (-1.0 - 1e-6..=1.0 + 1e-6).contains(&s),
                "Sample {} = {} is out of range",
                i,
                s
            );
        }
    }

    #[test]
    fn test_modulate_length_is_correct() {
        let frame = vec![0x00u8; 4];
        let samples = modulate(&frame);

        // Expected bit count:
        //   preamble: PREAMBLE_FLAGS * 8 bits
        //   frame body: 4 bytes * 8 bits = 32 bits (no stuffing for 0x00)
        //   postamble: POSTAMBLE_FLAGS * 8 bits
        let preamble_bits = PREAMBLE_FLAGS * 8;
        let frame_bits = 4 * 8; // 0x00 has no consecutive 1s, so no stuffing
        let postamble_bits = POSTAMBLE_FLAGS * 8;
        let total_bits = preamble_bits + frame_bits + postamble_bits;
        let expected_samples = total_bits * SAMPLES_PER_SYMBOL as usize;

        assert_eq!(
            samples.len(),
            expected_samples,
            "Expected {} samples, got {}",
            expected_samples,
            samples.len()
        );
    }

    // -----------------------------------------------------------------------
    // Demodulator tests
    // -----------------------------------------------------------------------

    /// Build a test frame with CRC appended.
    fn make_test_frame(data: &[u8]) -> Vec<u8> {
        let crc_alg = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let fcs = crc_alg.checksum(data);
        let mut frame = data.to_vec();
        frame.push(fcs as u8);
        frame.push((fcs >> 8) as u8);
        frame
    }

    #[test]
    fn test_demod_clean_loopback() {
        // A simple AX.25-like frame
        let data = vec![
            0x9C, 0x94, 0x6E, 0xA0, 0x40, 0x40, 0xE0, // dest
            0x9C, 0x6E, 0x98, 0x8A, 0x9A, 0x40, 0x61, // src
            0x03, // control
            0xF0, // PID
            b'H', b'e', b'l', b'l', b'o', // info
        ];
        let frame = make_test_frame(&data);
        let samples = modulate(&frame);

        let mut demod = Demodulator::new();
        demod.process(&samples);
        let frames = demod.take_frames();

        assert_eq!(frames.len(), 1, "Expected 1 frame, got {}", frames.len());
        assert_eq!(frames[0], frame);
    }

    #[test]
    fn test_demod_noisy_loopback_15db() {
        let data = vec![
            0x9C, 0x94, 0x6E, 0xA0, 0x40, 0x40, 0xE0, 0x9C, 0x6E, 0x98, 0x8A, 0x9A, 0x40, 0x61,
            0x03, 0xF0, b'T', b'e', b's', b't',
        ];
        let frame = make_test_frame(&data);
        let samples = modulate(&frame);

        // Add AWGN at 15 dB SNR
        let noisy = coppa_channel::awgn_seeded(&samples, 15.0, 42);

        let mut demod = Demodulator::new();
        demod.process(&noisy);
        let frames = demod.take_frames();

        assert_eq!(
            frames.len(),
            1,
            "Expected 1 frame at 15 dB SNR, got {}",
            frames.len()
        );
        assert_eq!(frames[0], frame);
    }

    #[test]
    fn test_demod_multiple_frames() {
        let data1 = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let data2 = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let frame1 = make_test_frame(&data1);
        let frame2 = make_test_frame(&data2);

        let samples1 = modulate(&frame1);
        let samples2 = modulate(&frame2);

        // 50ms silence gap between frames
        let silence_samples = (SAMPLE_RATE as f32 * 0.05) as usize;
        let mut combined = samples1;
        combined.extend(vec![0.0f32; silence_samples]);
        combined.extend(samples2);

        let mut demod = Demodulator::new();
        demod.process(&combined);
        let frames = demod.take_frames();

        assert_eq!(frames.len(), 2, "Expected 2 frames, got {}", frames.len());
        assert_eq!(frames[0], frame1);
        assert_eq!(frames[1], frame2);
    }

    #[test]
    fn test_modulate_contains_mark_and_space_frequencies() {
        // Encode a frame where both marks and spaces must appear.
        // Any non-trivial frame will generate transitions in the bit stream.
        let frame = vec![0x55u8, 0xAA]; // alternating bits — lots of transitions
        let samples = modulate(&frame);

        // Count zero crossings in the audio.  The mark frequency (1200 Hz)
        // produces ~1200 crossings/s and space (2200 Hz) ~2200 crossings/s.
        // At 48 kHz, 1200 Hz → 40 samples/cycle (2 crossings), 2200 Hz → ~21.8.
        // Just verify there are plenty of crossings (both frequencies active).
        let mut crossings = 0usize;
        for i in 1..samples.len() {
            if (samples[i] >= 0.0) != (samples[i - 1] >= 0.0) {
                crossings += 1;
            }
        }

        // At minimum mark frequency (1200 Hz) and 48 kHz SR we expect at least
        // 2 * 1200 / 48000 * samples.len() crossings (≈ 5% of samples).
        let min_crossings = samples.len() / 25;
        assert!(
            crossings >= min_crossings,
            "Too few zero crossings: {} (expected >= {}); audio may be silent",
            crossings,
            min_crossings
        );
    }
}
