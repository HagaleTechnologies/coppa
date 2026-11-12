//! Air-protocol version 2 low-rate floor framing (COP-190, ADR-014).
//!
//! A floor frame carries one PDU of at most [`MAX_PAYLOAD`] bytes in a fixed
//! 8.0 s airtime: a 16-symbol sync alphabet, four independently repeated
//! 4-byte headers (version, length, CRC16), and an NR BG2 LDPC codeword
//! carrying the payload plus a CRC32 check and zero padding. The header
//! version nibble is stamped [`FLOOR_AIR_PROTOCOL_VERSION`], the second
//! allocation after air-protocol version 1 (SPEC §0.3.3).
//!
//! The receiver searches unknown sample alignment and 13 CFO hypotheses
//! spanning ±150 Hz in 25 Hz steps, decodes the repeated header jointly over
//! its legal length/CRC combinations (COP-189's repair), and delivers only
//! CRC-validated PDUs, never unchecked bits. [`FloorStream`] wraps the same
//! receiver as a continuous detector with bounded audio retention.
//!
//! This framing is a separate air format from the OFDM speed ladder: it
//! changes no version-1 constant, vector or serializer. The engine and
//! daemon session controller that switch onto the floor are not built yet
//! (`docs/LIMITATIONS.md`, COP-169 section).

use crate::fec::ldpc::rate_match::{rate_dematch, rate_match};
use crate::fec::ldpc::NrLdpc;
use coppa_codec::floor::{FloorFsk, HOP, SAMPLE_RATE, SYMBOL_SAMPLES};
use crc::{Crc, CRC_16_IBM_SDLC, CRC_32_ISO_HDLC};

/// The air-protocol version of the floor framing, allocated in order by
/// SPEC §0.3.3 (value 0 reserved, value 1 the OFDM contract).
pub const FLOOR_AIR_PROTOCOL_VERSION: u8 = 2;
/// Fixed PDU envelope. Four additional code bits-protected bytes carry a
/// CRC32 over the payload inside the LDPC information word.
pub const MAX_PAYLOAD: usize = 40;
/// LDPC information length used by the floor envelope, in bits.
const K: usize = 352;
/// Rate-matched coded length of the payload section, in bits.
const E: usize = 1408;
/// Sync alphabet: 16 symbols whose tone pattern is unique up to rotation.
const SYNC: [usize; 16] = [0, 9, 4, 15, 2, 13, 6, 11, 1, 14, 7, 10, 3, 12, 5, 8];
/// Four independently repeated 4-byte headers, two symbols per byte.
const HEADER_SYMBOLS: usize = 32;
/// CFO hypotheses as search-bin offsets, zero CFO first. Offsets 0..12 span
/// -150..150 Hz in 25 Hz steps.
const CFO_HYPOTHESES: [usize; 13] = [6, 5, 7, 4, 8, 3, 9, 2, 10, 1, 11, 0, 12];
/// Exact fixed airtime, including sync and four header repetitions: 8 seconds.
pub const FRAME_SAMPLES: usize = (SYNC.len() + HEADER_SYMBOLS + E / 4) * SYMBOL_SAMPLES;
/// Retain at most a frame plus two seconds of audio in the streaming
/// receiver, independently of stream duration.
pub const CAPTURE_LIMIT: usize = FRAME_SAMPLES + 2 * SAMPLE_RATE;
const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Failure stage of a floor receive attempt, deepest stage first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorError {
    /// No eligible sync candidate survived the bounded search.
    Acquisition,
    /// Sync found but no complete, trusted header.
    Header,
    /// Trusted header but truncated, non-converged or CRC-failed payload.
    Payload,
    /// Caller supplied more than the fixed PDU envelope.
    Capacity,
}

/// The floor frame codec: modulation from [`FloorFsk`], FEC from
/// [`NrLdpc`], both built once and reused.
#[derive(Debug)]
pub struct FloorFrame {
    fsk: FloorFsk,
    fec: NrLdpc,
}

impl Default for FloorFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorFrame {
    /// Build the reusable codec and tone templates.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fsk: FloorFsk::new(),
            fec: NrLdpc::new(),
        }
    }

    /// Encode a fixed-size NR BG2 envelope with independently repeated
    /// header. No version-1 PHY or MAC constant is changed.
    ///
    /// # Errors
    /// Returns [`FloorError::Capacity`] above [`MAX_PAYLOAD`].
    pub fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, FloorError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(FloorError::Capacity);
        }
        let mut bytes = [0u8; MAX_PAYLOAD + 4];
        bytes[..payload.len()].copy_from_slice(payload);
        let checksum = CRC32.checksum(payload).to_be_bytes();
        bytes[MAX_PAYLOAD..].copy_from_slice(&checksum);
        let mut info = vec![0; NrLdpc::INFO_LEN];
        for (i, b) in bytes.iter().enumerate() {
            for bit in 0..8 {
                info[i * 8 + bit] = (b >> (7 - bit)) & 1;
            }
        }
        let coded = rate_match(&self.fec.encode(&info), K, E, 0);
        let mut tones = SYNC.to_vec();
        for _ in 0..4 {
            for byte in make_header(payload.len()) {
                tones.push(usize::from(byte >> 4));
                tones.push(usize::from(byte & 15));
            }
        }
        // A bijection spreads neighboring code bits across time and tones.
        for symbol in 0..E / 4 {
            let mut tone = 0;
            for bit in 0..4 {
                tone = (tone << 1) | usize::from(coded[((symbol * 4 + bit) * 31) % E]);
            }
            tones.push(tone);
        }
        let mut audio = Vec::with_capacity(FRAME_SAMPLES);
        for tone in tones {
            audio.extend_from_slice(self.fsk.tone_waveform(tone));
        }
        Ok(audio)
    }

    /// Search unknown sample alignment and 13 CFO hypotheses, trying at most
    /// 16 separated sync candidates per hypothesis, and deliver only a
    /// CRC-validated PDU. No timing or header oracle is used. The fixed
    /// search grid is ±150 Hz in 25 Hz steps; intra-frame drift is not
    /// tracked and reception between grid points is not guaranteed.
    ///
    /// # Errors
    /// Reports the deepest receiver stage reached; never delivers unchecked
    /// bits.
    pub fn receive(&self, audio: &[f32]) -> Result<Vec<u8>, FloorError> {
        self.receive_after(audio, 0).map(|(payload, _)| payload)
    }

    fn receive_after(
        &self,
        audio: &[f32],
        min_start: usize,
    ) -> Result<(Vec<u8>, usize), FloorError> {
        if audio.len() > CAPTURE_LIMIT || audio.iter().any(|x| !x.is_finite()) {
            return Err(FloorError::Acquisition);
        }
        let bank = self.fsk.energies(audio);
        let mut deepest = FloorError::Acquisition;
        // Reuse the same matched-filter bank across hypotheses. Only a single
        // 16-tone energy view is allocated at a time. Zero CFO is tried first.
        for offset in CFO_HYPOTHESES {
            let energy: Vec<[f32; 16]> = bank
                .iter()
                .map(|row| std::array::from_fn(|tone| row[offset + 2 * tone]))
                .collect();
            match self.receive_energies(&energy, min_start) {
                Ok(frame) => return Ok(frame),
                Err(FloorError::Payload) => deepest = FloorError::Payload,
                Err(FloorError::Header) if deepest == FloorError::Acquisition => {
                    deepest = FloorError::Header;
                }
                Err(_) => {}
            }
        }
        Err(deepest)
    }

    fn receive_energies(
        &self,
        energy: &[[f32; 16]],
        min_start: usize,
    ) -> Result<(Vec<u8>, usize), FloorError> {
        let stride = SYMBOL_SAMPLES / HOP;
        let mut candidates = Vec::new();
        for start in 0..energy.len().saturating_sub((SYNC.len() - 1) * stride) {
            if start * HOP < min_start {
                continue;
            }
            let score: f32 = SYNC
                .iter()
                .enumerate()
                .map(|(i, &tone)| {
                    let row = energy[start + i * stride];
                    row[tone] / row.iter().sum::<f32>().max(1e-20)
                })
                .sum::<f32>()
                / SYNC.len() as f32;
            if score > 0.16 {
                candidates.push((start, score));
            }
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut tried = Vec::new();
        let mut error = FloorError::Acquisition;
        for (start, _) in candidates {
            if tried
                .iter()
                .any(|&old: &usize| old.abs_diff(start) < stride)
            {
                continue;
            }
            if tried.len() == 16 {
                break;
            }
            tried.push(start);
            if error == FloorError::Acquisition {
                error = FloorError::Header;
            }
            let base = start + SYNC.len() * stride;
            if base + (HEADER_SYMBOLS - 1) * stride >= energy.len() {
                continue;
            }
            let Some(header) = decode_header(energy, base, stride) else {
                continue;
            };
            error = FloorError::Payload;
            let base = base + HEADER_SYMBOLS * stride;
            if base + (E / 4 - 1) * stride >= energy.len() {
                continue;
            }
            let mut soft = vec![0.0; E];
            for symbol in 0..E / 4 {
                let row = energy[base + symbol * stride];
                let scale = row.iter().sum::<f32>().max(1e-20) / 16.0;
                for bit in 0..4 {
                    let mut zero = 0.0f32;
                    let mut one = 0.0f32;
                    for (tone, e) in row.iter().enumerate() {
                        if tone & (1 << (3 - bit)) == 0 {
                            zero = zero.max(*e);
                        } else {
                            one = one.max(*e);
                        }
                    }
                    soft[((symbol * 4 + bit) * 31) % E] = ((zero - one) / scale).clamp(-20.0, 20.0);
                }
            }
            let mut mother = rate_dematch(&soft, K, E, 0, NrLdpc::MOTHER_LEN);
            // The floor envelope is not scrambled. The non-punctured
            // information tail is known zero, unlike the production
            // scrambler's pad.
            mother[..NrLdpc::INFO_LEN - K].fill(40.0);
            let (_, info, converged) = self.fec.decode_soft(&mother);
            if !converged {
                continue;
            }
            let bytes: Vec<u8> = info[..K]
                .chunks_exact(8)
                .map(|bits| bits.iter().fold(0, |acc, b| (acc << 1) | b))
                .collect();
            if CRC32
                .checksum(&bytes[..usize::from(header[1])])
                .to_be_bytes()
                != bytes[MAX_PAYLOAD..]
            {
                continue;
            }
            if bytes[usize::from(header[1])..MAX_PAYLOAD]
                .iter()
                .any(|&b| b != 0)
            {
                continue;
            }
            return Ok((
                bytes[..usize::from(header[1])].to_vec(),
                start * HOP + FRAME_SAMPLES,
            ));
        }
        Err(error)
    }
}

/// Continuous floor receiver. Decode one overlapping window per second,
/// independently of caller chunking. Retain at most [`CAPTURE_LIMIT`]
/// samples of audio and try at most 16 separated sync candidates per CFO
/// hypothesis, with 13 hypotheses spanning ±150 Hz in 25 Hz steps.
#[derive(Debug, Default)]
pub struct FloorStream {
    frame: FloorFrame,
    audio: Vec<f32>,
    pending: usize,
    consumed: u64,
    delivered_until: u64,
    attempts: u64,
}

impl FloorStream {
    /// Append arbitrary audio chunks and return CRC-validated floor PDUs
    /// once each. Nonfinite input breaks acquisition history, including
    /// partial frames.
    pub fn push(&mut self, chunk: &[f32]) -> Vec<Vec<u8>> {
        let mut delivered = Vec::new();
        // Incremental ingestion also bounds memory for oversized caller chunks.
        for &sample in chunk {
            self.consumed += 1;
            if !sample.is_finite() {
                self.audio.clear();
                self.pending = 0;
                continue;
            }
            self.audio.push(sample);
            self.pending += 1;
            if self.pending < SAMPLE_RATE {
                continue;
            }
            self.pending = 0;
            if self.audio.len() >= FRAME_SAMPLES {
                self.attempts += 1;
                let origin = self.consumed - self.audio.len() as u64;
                // Allow one symbol of timing uncertainty between adjacent frames.
                let min_start = self
                    .delivered_until
                    .saturating_sub(SYMBOL_SAMPLES as u64)
                    .saturating_sub(origin) as usize;
                if let Ok((payload, end)) = self.frame.receive_after(&self.audio, min_start) {
                    self.delivered_until = origin + end as u64;
                    delivered.push(payload);
                }
            }
            // Leave space for the next one-second block without exceeding the cap.
            let excess = self.audio.len().saturating_sub(CAPTURE_LIMIT - SAMPLE_RATE);
            self.audio.drain(..excess);
        }
        delivered
    }

    /// Samples currently retained, bounded independently of stream duration.
    #[must_use]
    pub fn retained_samples(&self) -> usize {
        self.audio.len()
    }

    /// Window decodes attempted, at most one per second of finite input.
    #[must_use]
    pub fn decode_attempts(&self) -> u64 {
        self.attempts
    }
}

/// Build the 4-byte header: version, length, CRC16 over the first two bytes.
fn make_header(len: usize) -> [u8; 4] {
    let mut header = [FLOOR_AIR_PROTOCOL_VERSION, len as u8, 0, 0];
    let crc = CRC16.checksum(&header[..2]).to_be_bytes();
    header[2..].copy_from_slice(&crc);
    header
}

/// A header is trusted only with the floor version, a legal length and a
/// matching CRC16.
fn valid_header(header: &[u8; 4]) -> bool {
    header[0] == FLOOR_AIR_PROTOCOL_VERSION
        && usize::from(header[1]) <= MAX_PAYLOAD
        && CRC16.checksum(&header[..2]).to_be_bytes() == header[2..]
}

// Decode the repeated header as a short code over its legal length/CRC
// combinations. Independent hard nibble decisions throw away CRC redundancy
// before checking it, so one faded nibble can discard an otherwise good frame.
// The version nibble is still checked independently; the floor framing does
// not negotiate an air version. Payload FEC, CRC32 and zero padding remain
// mandatory.
fn decode_header(energy: &[[f32; 16]], base: usize, stride: usize) -> Option<[u8; 4]> {
    let mut combined = [[0.0f32; 16]; 8];
    for (nibble, row) in combined.iter_mut().enumerate() {
        for rep in 0..4 {
            let observation = energy[base + (rep * 8 + nibble) * stride];
            let total = observation.iter().sum::<f32>().max(1e-20);
            for (dst, e) in row.iter_mut().zip(observation) {
                *dst += e / total;
            }
        }
    }
    for (nibble, expected) in [0usize, usize::from(FLOOR_AIR_PROTOCOL_VERSION)]
        .into_iter()
        .enumerate()
    {
        let row = combined[nibble];
        if row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?.0 != expected {
            return None;
        }
    }
    let mut best = None;
    let mut best_score = 0.0f32;
    let mut runner_up = 0.0f32;
    for len in 0..=MAX_PAYLOAD {
        let header = make_header(len);
        let score = header
            .iter()
            .enumerate()
            .skip(1)
            .map(|(byte, value)| {
                combined[byte * 2][usize::from(value >> 4)]
                    + combined[byte * 2 + 1][usize::from(value & 15)]
            })
            .sum::<f32>()
            / 24.0;
        if score > best_score {
            runner_up = best_score;
            best_score = score;
            best = Some(header);
        } else {
            runner_up = runner_up.max(score);
        }
    }
    // Require positive header evidence, rather than turning noise or a tie
    // into a valid header merely because every enumerated codeword has a CRC.
    if best_score > 0.16 && best_score > runner_up {
        best.filter(valid_header)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_rejection() {
        let header = make_header(4);
        assert!(valid_header(&header));
        let mut corrupt = header;
        corrupt[0] ^= 1;
        assert!(!valid_header(&corrupt));
        let mut wrong_version = make_header(4);
        wrong_version[0] = 1;
        let crc = CRC16.checksum(&wrong_version[..2]).to_be_bytes();
        wrong_version[2..].copy_from_slice(&crc);
        assert!(!valid_header(&wrong_version));
    }

    #[test]
    fn joint_header_decodes_every_capacity_and_rejects_missing_evidence() {
        for len in 0..=MAX_PAYLOAD {
            let header = make_header(len);
            let mut energy = vec![[0.0; 16]; HEADER_SYMBOLS];
            for rep in 0..4 {
                for (byte, value) in header.iter().enumerate() {
                    energy[rep * 8 + byte * 2][usize::from(value >> 4)] = 1.0;
                    energy[rep * 8 + byte * 2 + 1][usize::from(value & 15)] = 1.0;
                }
            }
            assert_eq!(decode_header(&energy, 0, 1), Some(header));
            // The known version alone must not manufacture a trusted length.
            for rep in 0..4 {
                energy[rep * 8 + 2..rep * 8 + 8].fill([1.0; 16]);
            }
            assert_eq!(decode_header(&energy, 0, 1), None);
        }
        assert_eq!(decode_header(&[[0.0; 16]; HEADER_SYMBOLS], 0, 1), None);
    }

    #[test]
    fn frame_constants_match_the_adr_freeze() {
        assert_eq!(FLOOR_AIR_PROTOCOL_VERSION, 2);
        assert_eq!(FRAME_SAMPLES, 8 * SAMPLE_RATE);
        assert_eq!(CAPTURE_LIMIT, FRAME_SAMPLES + 2 * SAMPLE_RATE);
    }
}
