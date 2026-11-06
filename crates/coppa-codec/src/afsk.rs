//! AFSK 1200 baud modem (Bell 202) for AX.25 / APRS.
//!
//! Implements HDLC framing (flags + bit stuffing), NRZI encoding,
//! and continuous-phase FSK audio generation at 48 kHz.
//! Also provides a streaming demodulator with correlator-based detection.
use std::f32::consts::{PI, TAU};

pub use crate::hdlc::append_fcs;
use crate::hdlc::{bit_stuff, byte_to_bits_lsb, nrzi_encode, HdlcDecoder};
#[cfg(test)]
use crate::hdlc::{bit_unstuff, bits_to_byte_lsb, nrzi_decode};

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

/// Modulate raw AX.25 frame bytes (CRC already appended, e.g. by [`append_fcs`])
/// into 48 kHz audio.
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
#[derive(Debug)]
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

    hdlc: HdlcDecoder,
}

impl Default for Demodulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator {
    /// Creates a demodulator in the idle state: no frame in progress, NRZI
    /// reference set to mark, and no decoded frames queued.
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
            hdlc: HdlcDecoder::default(),
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
                self.hdlc.process_bit(bit);

                // Reset correlator for next symbol
                self.mark_sin_acc = 0.0;
                self.mark_cos_acc = 0.0;
                self.space_sin_acc = 0.0;
                self.space_cos_acc = 0.0;
                self.sample_count = 0;
            }
        }
    }

    /// Extract all completed frames, draining the internal buffer.
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        self.hdlc.take_frames()
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
    use crc::{Crc, CRC_16_IBM_SDLC};

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
    fn test_append_fcs_known_answer() {
        // CRC-16/X.25 check value: FCS("123456789") = 0x906E, sent low byte first.
        let framed = append_fcs(b"123456789");
        assert_eq!(&framed[..9], b"123456789");
        assert_eq!(&framed[9..], &[0x6E, 0x90]);
    }

    #[test]
    fn test_append_fcs_round_trips_through_demodulator() {
        let data = b"N0CALL>APRS:>append_fcs round trip";
        let framed = append_fcs(data);
        let mut demod = Demodulator::new();
        demod.process(&modulate(&framed));
        assert_eq!(demod.take_frames(), vec![framed]);
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
