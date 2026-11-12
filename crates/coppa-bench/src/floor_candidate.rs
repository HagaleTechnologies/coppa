//! COP-169 experimental 16-FSK candidate. This is a measurement-only format,
//! not an allocated air version or a production receiver. No external modem
//! implementation was consulted. Frequencies and framing are original.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_connect_and_enforces_capacity_and_airtime() {
        use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
        use coppa_protocol::session::LinkCapabilities;
        for frame_type in [
            MacFrameType::ConnectReq,
            MacFrameType::ConnectAck,
            MacFrameType::ConnectCfm,
        ] {
            let pdu = MacPdu::new(
                frame_type,
                Callsign::new("W1AW").unwrap(),
                Callsign::new("K1ABC").unwrap(),
                0,
                LinkCapabilities::default().to_bytes(),
            )
            .to_bytes();
            let c = Candidate::new();
            assert!(pdu.len() <= MAX_PAYLOAD);
            assert!(c.encode(&[0; MAX_PAYLOAD + 1]).is_err());
            let audio = c.encode(&pdu).unwrap();
            assert_eq!(audio.len(), FRAME_SAMPLES);
            assert!(audio.len() <= 8 * SAMPLE_RATE);
            assert_eq!(c.receive(&audio).unwrap(), pdu);
        }
    }

    #[test]
    fn unknown_offset_and_arbitrary_chunks() {
        let c = Candidate::new();
        let sent: Vec<_> = (0..MAX_PAYLOAD as u8).collect();
        for offset in [1, 119, 481, 1907] {
            let mut audio = vec![0.0; offset];
            audio.extend(c.encode(&sent).unwrap());
            audio.extend(vec![0.0; 960]);
            assert_eq!(c.receive(&audio).unwrap(), sent);
        }
        let mut audio = vec![0.0; 137];
        audio.extend(c.encode(&sent).unwrap());
        let mut stream = Capture::default();
        for chunk in audio.chunks(317) {
            stream.push(chunk);
        }
        assert_eq!(c.receive(stream.samples()).unwrap(), sent);
    }

    #[test]
    fn header_version_crc_and_truncation_reject_without_wrong_delivery() {
        let c = Candidate::new();
        let mut audio = c.encode(b"test").unwrap();
        assert!(c.receive(&audio[..audio.len() - SYMBOL_SAMPLES]).is_err());
        audio[SYNC.len() * SYMBOL_SAMPLES..(SYNC.len() + HEADER_SYMBOLS) * SYMBOL_SAMPLES]
            .fill(0.0);
        assert!(c.receive(&audio).is_err());
        let mut header = make_header(4);
        header[0] ^= 1;
        assert!(!valid_header(&header));
        header = make_header(4);
        header[0] = 1;
        let crc = CRC16.checksum(&header[..2]).to_be_bytes();
        header[2..].copy_from_slice(&crc);
        assert!(!valid_header(&header));
    }

    #[test]
    fn header_length_erasure_recovers_from_crc_redundancy() {
        let c = Candidate::new();
        let sent = b"header fade regression!";
        let mut audio = c.encode(sent).unwrap();
        // Erase the length's low nibble in all repetitions. The remaining
        // length and CRC tones still distinguish the legal header codewords.
        for rep in 0..4 {
            let start = (SYNC.len() + rep * 8 + 3) * SYMBOL_SAMPLES;
            audio[start..start + SYMBOL_SAMPLES].fill(0.0);
        }
        assert_eq!(c.receive(&audio).unwrap(), sent);
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
    fn payload_crc_binds_length_even_when_padding_is_zero() {
        let c = Candidate::new();
        let mut audio = c.encode(b"test").unwrap();
        let forged = make_header(5);
        for rep in 0..4 {
            for (byte, value) in forged.iter().enumerate() {
                for (nibble, tone) in [value >> 4, value & 15].into_iter().enumerate() {
                    let start = (SYNC.len() + rep * 8 + byte * 2 + nibble) * SYMBOL_SAMPLES;
                    for (sample, tone) in audio[start..start + SYMBOL_SAMPLES]
                        .iter_mut()
                        .zip(&c.oscillators[usize::from(tone)])
                    {
                        *sample = tone.re;
                    }
                }
            }
        }
        assert_eq!(c.receive(&audio), Err(CandidateError::Payload));
    }

    #[test]
    fn rejects_payload_corruption_and_nonfinite_audio() {
        let c = Candidate::new();
        let mut audio = c.encode(b"crc regression").unwrap();
        audio[(SYNC.len() + HEADER_SYMBOLS) * SYMBOL_SAMPLES..].fill(0.0);
        assert_eq!(c.receive(&audio), Err(CandidateError::Payload));
        audio[0] = f32::NAN;
        assert_eq!(c.receive(&audio), Err(CandidateError::Acquisition));
        assert_eq!(
            c.receive(&vec![0.0; CAPTURE_LIMIT + 1]),
            Err(CandidateError::Acquisition)
        );
    }

    #[test]
    fn cfo_characterization_never_delivers_wrong_bytes() {
        let c = Candidate::new();
        let payload = b"CFO characterization";
        let audio = c.encode(payload).unwrap();
        for cfo in [-150.0, 150.0] {
            let shifted = coppa_channel::frequency_shift(&audio, cfo, SAMPLE_RATE as f32);
            if let Ok(got) = c.receive(&shifted) {
                assert_eq!(got, payload);
            }
        }
    }

    #[test]
    fn rejects_noise_and_bounds_capture() {
        let c = Candidate::new();
        let noise = coppa_channel::awgn_ref_seeded(
            &vec![0.0; FRAME_SAMPLES + 960],
            -6.0,
            0.5,
            SAMPLE_RATE as f32,
            123,
        );
        assert!(c.receive(&noise).is_err());
        let mut stream = Capture::default();
        for _ in 0..400 {
            stream.push(&[0.0; 48000]);
        }
        assert_eq!(stream.samples().len(), CAPTURE_LIMIT);
    }
}

use coppa_protocol::fec::ldpc::{
    rate_match::{rate_dematch, rate_match},
    NrLdpc,
};
use crc::{Crc, CRC_16_IBM_SDLC, CRC_32_ISO_HDLC};
use num_complex::Complex32;
use std::collections::VecDeque;

/// All candidate audio uses the workspace's 48 kHz clock.
pub const SAMPLE_RATE: usize = 48000;
/// 50 baud, orthogonal tones separated by 50 Hz.
pub const SYMBOL_SAMPLES: usize = 960;
/// Fixed PDU envelope. Four additional bytes protect the payload with CRC32.
pub const MAX_PAYLOAD: usize = 40;
const K: usize = 352;
const E: usize = 1408;
const HOP: usize = 120;
const SYNC: [usize; 16] = [0, 9, 4, 15, 2, 13, 6, 11, 1, 14, 7, 10, 3, 12, 5, 8];
const HEADER_SYMBOLS: usize = 32;
/// Exact fixed airtime, including sync and four header repetitions: 8 seconds.
pub const FRAME_SAMPLES: usize = (SYNC.len() + HEADER_SYMBOLS + E / 4) * SYMBOL_SAMPLES;
/// Retain at most a frame plus two seconds. Capture decoding is explicitly
/// delayed until requested; this is not a production streaming detector.
pub const CAPTURE_LIMIT: usize = FRAME_SAMPLES + 2 * SAMPLE_RATE;
const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Failure stage for conservative sweep accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateError {
    /// No eligible sync candidate survived the bounded search.
    Acquisition,
    /// Sync found but no complete, trusted header.
    Header,
    /// Trusted header but truncated, non-converged or CRC-failed payload.
    Payload,
    /// Caller supplied more than the fixed PDU envelope.
    Capacity,
}

/// Bounded capture adapter for chunk partition tests. Call receive explicitly
/// after capturing a frame. It makes no low-latency streaming claim.
#[derive(Debug, Default)]
pub struct Capture {
    samples: VecDeque<f32>,
    contiguous: Vec<f32>,
}
impl Capture {
    /// Append audio and evict oldest samples beyond the fixed bound.
    pub fn push(&mut self, chunk: &[f32]) {
        if chunk.len() >= CAPTURE_LIMIT {
            self.samples.clear();
            self.samples.extend(&chunk[chunk.len() - CAPTURE_LIMIT..]);
        } else {
            let excess = (self.samples.len() + chunk.len()).saturating_sub(CAPTURE_LIMIT);
            self.samples.drain(..excess);
            self.samples.extend(chunk);
        }
        self.contiguous.clear();
        self.contiguous.extend(self.samples.iter().copied());
    }
    /// Current bounded audio history.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.contiguous
    }
}

/// Measurement-only candidate, with cached NR BG2 codec and tone templates.
#[derive(Debug)]
pub struct Candidate {
    fec: NrLdpc,
    oscillators: Vec<Vec<Complex32>>,
}
impl Default for Candidate {
    fn default() -> Self {
        Self::new()
    }
}
impl Candidate {
    /// Build reusable codec and 600..1350 Hz tone templates.
    #[must_use]
    pub fn new() -> Self {
        let oscillators = (0..16)
            .map(|tone| {
                (0..SYMBOL_SAMPLES)
                    .map(|n| {
                        Complex32::from_polar(
                            1.0,
                            std::f32::consts::TAU * (600 + 50 * tone) as f32 * n as f32
                                / SAMPLE_RATE as f32,
                        )
                    })
                    .collect()
            })
            .collect();
        Self {
            fec: NrLdpc::new(),
            oscillators,
        }
    }

    /// Encode a fixed-size NR BG2 envelope with independently repeated header.
    /// No v1 PHY or MAC constants are changed.
    /// # Errors
    /// Returns Capacity above MAX_PAYLOAD.
    pub fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, CandidateError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(CandidateError::Capacity);
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
                tones.push((byte >> 4) as usize);
                tones.push((byte & 15) as usize);
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
            audio.extend(self.oscillators[tone].iter().map(|z| z.re));
        }
        Ok(audio)
    }

    fn energies(&self, audio: &[f32]) -> Vec<[f32; 16]> {
        // Sliding matched filters evaluate all eight symbol phases. Templates
        // repeat every symbol, so subtract/add updates are exact up to rounding.
        if audio.len() < SYMBOL_SAMPLES {
            return Vec::new();
        }
        let mut sums = [Complex32::new(0.0, 0.0); 16];
        for (tone, sum) in sums.iter_mut().enumerate() {
            for (n, &x) in audio[..SYMBOL_SAMPLES].iter().enumerate() {
                *sum += self.oscillators[tone][n] * x;
            }
        }
        let mut out = Vec::new();
        let mut start = 0;
        loop {
            out.push(sums.map(|z| z.norm_sqr().max(0.0)));
            if start + SYMBOL_SAMPLES + HOP > audio.len() {
                break;
            }
            for (tone, sum) in sums.iter_mut().enumerate() {
                for n in start..start + HOP {
                    *sum += self.oscillators[tone][n % SYMBOL_SAMPLES]
                        * (audio[n + SYMBOL_SAMPLES] - audio[n]);
                }
            }
            start += HOP;
        }
        out
    }

    /// Search unknown sample alignment, decode at most 16 separated candidates,
    /// and deliver only a CRC-validated PDU. No timing or header oracle is used.
    /// This initial candidate searches zero CFO only.
    /// # Errors
    /// Reports the deepest receiver stage reached; never delivers unchecked bits.
    pub fn receive(&self, audio: &[f32]) -> Result<Vec<u8>, CandidateError> {
        if audio.len() > CAPTURE_LIMIT || audio.iter().any(|x| !x.is_finite()) {
            return Err(CandidateError::Acquisition);
        }
        let energy = self.energies(audio);
        let stride = SYMBOL_SAMPLES / HOP;
        let mut candidates = Vec::new();
        for start in 0..energy.len().saturating_sub((SYNC.len() - 1) * stride) {
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
        let mut error = CandidateError::Acquisition;
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
            if error == CandidateError::Acquisition {
                error = CandidateError::Header;
            }
            let base = start + SYNC.len() * stride;
            if base + (HEADER_SYMBOLS - 1) * stride >= energy.len() {
                continue;
            }
            let Some(header) = decode_header(&energy, base, stride) else {
                continue;
            };
            error = CandidateError::Payload;
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
            // Our envelope is NOT scrambled. The non-punctured information
            // tail is known zero, unlike the production scrambler's pad.
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
            return Ok(bytes[..usize::from(header[1])].to_vec());
        }
        Err(error)
    }
}
fn make_header(len: usize) -> [u8; 4] {
    let mut header = [2, len as u8, 0, 0];
    let crc = CRC16.checksum(&header[..2]).to_be_bytes();
    header[2..].copy_from_slice(&crc);
    header
}
fn valid_header(header: &[u8; 4]) -> bool {
    header[0] == 2
        && usize::from(header[1]) <= MAX_PAYLOAD
        && CRC16.checksum(&header[..2]).to_be_bytes() == header[2..]
}

// Decode the repeated header as a short code over its legal length/CRC
// combinations. Independent hard nibble decisions throw away CRC redundancy
// before checking it, so one faded nibble can discard an otherwise good frame.
// The version is still checked independently; this candidate does not negotiate
// an air version. Payload FEC, CRC32 and zero padding remain mandatory.
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
    for (nibble, expected) in [0usize, 2].into_iter().enumerate() {
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
