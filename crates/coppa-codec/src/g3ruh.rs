//! G3RUH 9600 baud direct-FSK modem for AX.25 (COP-116).
//!
//! TX applies HDLC stuffing, NRZI, then an output-fed x^17+x^12+1
//! scrambler and emits rectangular ±0.5 levels at 48 kHz. RX uses five
//! staggered integrate-and-dump slicers, input-fed descrambling and NRZI.
//! Direwolf TX/RX verification: `docs/testing/tnc-direwolf-g3ruh-9600.md`.
pub use crate::hdlc::append_fcs;
use crate::hdlc::{bit_stuff, byte_to_bits_lsb, nrzi_encode, HdlcDecoder};
use std::collections::VecDeque;
/// Symbol rate in baud.
pub const BAUD_RATE: u32 = 9600;
/// Required engine sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per symbol at the engine rate.
pub const SAMPLES_PER_SYMBOL: u32 = 5;
/// Output amplitude, relative to full scale.
pub const TX_LEVEL: f32 = 0.5;
/// Exact-byte duplicate suppression window in milliseconds.
pub const DUP_WINDOW_MS: u64 = 150;
const PREAMBLE_FLAGS: usize = 36;
const POSTAMBLE_FLAGS: usize = 2;

fn scramble(bit: u8, state: &mut u32) -> u8 {
    let out = bit ^ ((*state >> 11) as u8 & 1) ^ ((*state >> 16) as u8 & 1);
    *state = ((*state << 1) | u32::from(out)) & 0x1ffff;
    out
}
fn descramble(bit: u8, state: &mut u32) -> u8 {
    let out = bit ^ ((*state >> 11) as u8 & 1) ^ ((*state >> 16) as u8 & 1);
    *state = ((*state << 1) | u32::from(bit)) & 0x1ffff;
    out
}
/// Modulate a frame that already includes its two-byte AX.25 FCS.
pub fn modulate(frame_bytes: &[u8]) -> Vec<f32> {
    let flag = byte_to_bits_lsb(0x7e);
    let mut bits = flag.repeat(PREAMBLE_FLAGS);
    let body: Vec<_> = frame_bytes
        .iter()
        .flat_map(|&b| byte_to_bits_lsb(b))
        .collect();
    bits.extend(bit_stuff(&body));
    bits.extend(flag.repeat(POSTAMBLE_FLAGS));
    let mut state = 0;
    nrzi_encode(&bits)
        .into_iter()
        .flat_map(|b| {
            let level = if scramble(u8::from(b), &mut state) == 1 {
                TX_LEVEL
            } else {
                -TX_LEVEL
            };
            [level; SAMPLES_PER_SYMBOL as usize]
        })
        .collect()
}

#[derive(Debug, Default)]
struct Bank {
    sum: f32,
    count: u32,
    state: u32,
    prev: u8,
    hdlc: HdlcDecoder,
}
/// Streaming five-phase receiver. Returned frames include the FCS.
/// Exact-byte copies inside 150 ms are suppressed across all banks.
#[derive(Debug)]
pub struct Demodulator {
    dc_mean: f32,
    sample_index: u64,
    banks: [Bank; 5],
    recent: VecDeque<(Vec<u8>, u64)>,
    frames: Vec<Vec<u8>>,
}
impl Default for Demodulator {
    fn default() -> Self {
        Self::new()
    }
}
impl Demodulator {
    /// Construct an idle receiver.
    pub fn new() -> Self {
        Self {
            dc_mean: 0.0,
            sample_index: 0,
            banks: std::array::from_fn(|_| Bank::default()),
            recent: VecDeque::new(),
            frames: Vec::new(),
        }
    }
    /// Process an arbitrary streaming block of 48 kHz samples.
    pub fn process(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.dc_mean += 0.00002 * (sample - self.dc_mean);
            let x = sample - self.dc_mean;
            let now = self.sample_index;
            while self
                .recent
                .front()
                .is_some_and(|(_, t)| now - t >= SAMPLE_RATE as u64 * DUP_WINDOW_MS / 1000)
            {
                self.recent.pop_front();
            }
            for (phase, bank) in self.banks.iter_mut().enumerate() {
                if now < phase as u64 {
                    continue;
                }
                bank.sum += x;
                bank.count += 1;
                if bank.count == SAMPLES_PER_SYMBOL {
                    let level = u8::from(bank.sum >= 0.0);
                    let symbol = descramble(level, &mut bank.state);
                    bank.hdlc.process_bit(u8::from(symbol == bank.prev));
                    bank.prev = symbol;
                    bank.sum = 0.0;
                    bank.count = 0;
                    for frame in bank.hdlc.take_frames() {
                        if !self.recent.iter().any(|(f, _)| f == &frame) {
                            self.recent.push_back((frame.clone(), now));
                            self.frames.push(frame);
                        }
                    }
                }
            }
            self.sample_index += 1;
        }
    }
    /// Drain the FCS-valid frames recovered so far.
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }
    /// Reset all phase banks and duplicate history.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decode(audio: &[f32]) -> Vec<Vec<u8>> {
        let mut d = Demodulator::new();
        let mut frames = Vec::new();
        for chunk in audio.chunks(1024) {
            d.process(chunk);
            frames.extend(d.take_frames());
        }
        frames
    }
    fn noisy_frames() -> (Vec<Vec<u8>>, Vec<f32>) {
        let frames: Vec<_> = (0..10)
            .map(|i| append_fcs(format!("COP-116 frame {i}").as_bytes()))
            .collect();
        let audio: Vec<_> = frames.iter().flat_map(|f| modulate(f)).collect();
        (frames, coppa_channel::awgn_seeded(&audio, 10.0, 42))
    }
    #[test]
    fn scrambler_round_trips_and_resynces() {
        let bits: Vec<u8> = (0..200).map(|i| u8::from(i % 7 < 3)).collect();
        let (mut tx, mut rx) = (0x12345, 0);
        let recovered: Vec<_> = bits
            .iter()
            .map(|&b| descramble(scramble(b, &mut tx), &mut rx))
            .collect();
        assert_eq!(recovered[17..], bits[17..]);
    }
    #[test]
    fn descrambler_is_taps_12_17() {
        let input: Vec<u8> = (0..64).map(|i| u8::from(i % 5 < 2)).collect();
        let mut state = 0;
        for i in 0..input.len() {
            let expected = input[i]
                ^ if i >= 12 { input[i - 12] } else { 0 }
                ^ if i >= 17 { input[i - 17] } else { 0 };
            assert_eq!(descramble(input[i], &mut state), expected);
        }
    }
    #[test]
    fn modulate_length_is_exact() {
        let f = append_fcs(&[0xff; 55]);
        assert_eq!(f.len(), 57);
        let bits: Vec<_> = f.iter().flat_map(|&b| byte_to_bits_lsb(b)).collect();
        assert_eq!(
            modulate(&f).len(),
            (36 * 8 + bit_stuff(&bits).len() + 2 * 8) * 5
        );
    }
    #[test]
    fn modulate_levels_are_rectangular_pm_half() {
        let audio = modulate(&append_fcs(b"levels"));
        assert!(audio.iter().all(|x| x.abs() == 0.5));
        assert!(audio.contains(&0.5) && audio.contains(&-0.5));
    }
    #[test]
    fn clean_loopback_decodes_one_frame() {
        let f = append_fcs(b"clean frame");
        assert_eq!(decode(&modulate(&f)), vec![f]);
    }
    #[test]
    fn loopback_at_10db_awgn_decodes_all_frames() {
        let (frames, audio) = noisy_frames();
        assert_eq!(decode(&audio), frames);
    }
    #[test]
    fn decode_is_polarity_invariant() {
        let (frames, audio) = noisy_frames();
        assert_eq!(
            decode(&audio.iter().map(|x| -x).collect::<Vec<_>>()),
            frames
        );
    }
    #[test]
    fn decode_survives_dc_offset() {
        let f = append_fcs(b"DC offset");
        assert_eq!(
            decode(&modulate(&f).iter().map(|x| x + 0.2).collect::<Vec<_>>()),
            vec![f]
        );
    }
    #[test]
    fn decode_works_at_every_start_offset_0_to_4() {
        let f = append_fcs(b"offset");
        for offset in 0..5 {
            let mut audio = vec![0.0; offset];
            audio.extend(modulate(&f));
            assert_eq!(decode(&audio), vec![f.clone()]);
        }
    }
    #[test]
    fn phase_bank_copies_delivered_once() {
        let f = append_fcs(b"dedupe");
        assert_eq!(decode(&modulate(&f)), vec![f]);
    }
    #[test]
    fn identical_frames_a_window_apart_deliver_twice() {
        let f = append_fcs(b"repeat");
        for (gap, count) in [(0, 1), (SAMPLE_RATE as usize / 5, 2)] {
            let mut audio = modulate(&f);
            audio.extend(vec![0.0; gap]);
            audio.extend(modulate(&f));
            assert_eq!(decode(&audio), vec![f.clone(); count]);
        }
    }
    #[test]
    fn reset_clears_partial_and_duplicate_state() {
        let f = append_fcs(b"reset");
        let audio = modulate(&f);
        let mut d = Demodulator::new();
        d.process(&audio[..123]);
        d.reset();
        d.process(&audio);
        assert_eq!(d.take_frames(), vec![f.clone()]);
        d.reset();
        d.process(&audio);
        assert_eq!(d.take_frames(), vec![f]);
    }
}
