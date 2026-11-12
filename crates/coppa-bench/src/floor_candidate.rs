//! COP-169 experimental 16-FSK candidate. This is a measurement-only format,
//! not an allocated air version or a production receiver. No external modem
//! implementation was consulted. Frequencies and framing are original.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_acquisition_crosses_windows_and_delivers_once() {
        let c = Candidate::new();
        for chunk_size in [317, 48000, CAPTURE_LIMIT + 13] {
            let mut audio = vec![0.0; 2 * SAMPLE_RATE + 137];
            audio.extend(c.encode(b"first").unwrap());
            audio.extend(vec![0.0; 719]);
            audio.extend(c.encode(b"second").unwrap());
            audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
            let mut rx = CandidateStream::default();
            let mut got = Vec::new();
            for chunk in audio.chunks(chunk_size) {
                got.extend(rx.push(chunk));
                assert!(rx.retained_samples() <= CAPTURE_LIMIT);
            }
            assert_eq!(got, [b"first".to_vec(), b"second".to_vec()]);
            assert!(rx.decode_attempts() <= (audio.len() / SAMPLE_RATE) as u64);
        }
    }

    #[test]
    fn continuous_acquisition_rejects_nonfinite_and_truncated_frames() {
        let c = Candidate::new();
        let audio = c.encode(b"incomplete").unwrap();
        let mut rx = CandidateStream::default();
        assert!(rx.push(&audio[..audio.len() - SYMBOL_SAMPLES]).is_empty());
        assert!(rx.push(&[f32::NAN, f32::INFINITY]).is_empty());
        assert_eq!(rx.retained_samples(), 0);
        assert!(rx.push(&vec![0.0; 2 * CAPTURE_LIMIT]).is_empty());
        assert!(rx.retained_samples() <= CAPTURE_LIMIT);
        let mut corrupt = c.encode(b"bad header").unwrap();
        corrupt[SYNC.len() * SYMBOL_SAMPLES..(SYNC.len() + HEADER_SYMBOLS) * SYMBOL_SAMPLES]
            .fill(0.0);
        assert!(rx.push(&corrupt).is_empty());
        assert!(rx.push(&vec![0.0; SAMPLE_RATE]).is_empty());
        let mut good = c.encode(b"recovered").unwrap();
        good.extend(vec![0.0; SAMPLE_RATE]);
        assert_eq!(rx.push(&good), [b"recovered".to_vec()]);
    }

    // Long scientific check, run explicitly in release mode.
    #[test]
    #[ignore = "300 seconds each of three noise inputs"]
    fn continuous_acquisition_300_seconds_noise() {
        for kind in 0..3 {
            let mut rx = CandidateStream::default();
            for second in 0..300 {
                let mut noise = coppa_channel::awgn_ref_seeded(
                    &vec![0.0; SAMPLE_RATE],
                    -6.0,
                    0.5,
                    SAMPLE_RATE as f32,
                    0x00C0FFEE + second,
                );
                if kind == 1 {
                    noise = coppa_channel::ssb_filter(&noise, SAMPLE_RATE as f32);
                } else if kind == 2 {
                    for (n, x) in noise.iter_mut().enumerate() {
                        *x +=
                            (std::f32::consts::TAU * 1000.0 * n as f32 / SAMPLE_RATE as f32).cos();
                    }
                }
                assert!(rx.push(&noise).is_empty(), "kind={kind}, second={second}");
                assert!(rx.retained_samples() <= CAPTURE_LIMIT);
                assert!(rx.decode_attempts() <= second + 1);
            }
            assert_eq!(rx.decode_attempts(), 293);
        }
    }

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
                        .zip(&c.oscillators[6 + 2 * usize::from(tone)][..SYMBOL_SAMPLES])
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
    fn cfo_search_decodes_unknown_offset_in_batch_and_chunks() {
        let candidate = Candidate::new();
        let payload = b"CFO acquisition regression";
        let clean = candidate.encode(payload).unwrap();
        for cfo in [-150.0, -75.0, 75.0, 150.0] {
            let mut audio = vec![0.0; 137];
            audio.extend(coppa_channel::frequency_shift(
                &clean,
                cfo,
                SAMPLE_RATE as f32,
            ));
            assert_eq!(candidate.receive(&audio), Ok(payload.to_vec()), "CFO {cfo}");
            audio.extend(vec![0.0; SAMPLE_RATE]);
            let mut receiver = CandidateStream::default();
            let mut delivered = Vec::new();
            for chunk in audio.chunks(317) {
                delivered.extend(receiver.push(chunk));
                assert!(receiver.retained_samples() <= CAPTURE_LIMIT);
            }
            assert_eq!(delivered, [payload.to_vec()], "stream CFO {cfo}");
        }
    }

    #[test]
    fn cfo_characterization_never_delivers_wrong_bytes() {
        let c = Candidate::new();
        let payload: Vec<_> = (0..MAX_PAYLOAD as u8).collect();
        let audio = c.encode(&payload).unwrap();
        for cfo in [-150.0, -100.0, -50.0, 0.0, 50.0, 100.0, 150.0] {
            let shifted = coppa_channel::frequency_shift(&audio, cfo, SAMPLE_RATE as f32);
            let mut capture = vec![0.0; 1907];
            capture.extend(shifted);
            capture.extend(vec![0.0; SYMBOL_SAMPLES]);
            assert_eq!(c.receive(&capture), Ok(payload.clone()), "CFO {cfo}");
        }
    }

    #[test]
    fn cfo_acquisition_preserves_payload_checks_and_handles_ssb() {
        let c = Candidate::new();
        let payload = b"CFO integrity regression";
        let audio = c.encode(payload).unwrap();
        for cfo in [-150.0, 0.0, 150.0] {
            let shifted = coppa_channel::frequency_shift(&audio, cfo, SAMPLE_RATE as f32);
            // Flush the FIR delay rather than dropping the last payload symbol.
            let mut padded = shifted.clone();
            padded.resize(padded.len() + SYMBOL_SAMPLES, 0.0);
            let filtered = coppa_channel::ssb_filter(&padded, SAMPLE_RATE as f32);
            assert_eq!(c.receive(&filtered), Ok(payload.to_vec()), "SSB CFO {cfo}");
            let noisy = coppa_channel::awgn_ref_seeded(
                &shifted,
                -6.0,
                coppa_channel::mean_power(&audio),
                SAMPLE_RATE as f32,
                0x00C0FFEE,
            );
            assert_eq!(c.receive(&noisy), Ok(payload.to_vec()), "AWGN CFO {cfo}");
            let mut corrupt = shifted;
            corrupt[(SYNC.len() + HEADER_SYMBOLS) * SYMBOL_SAMPLES..].fill(0.0);
            assert!(c.receive(&corrupt).is_err(), "corrupt CFO {cfo}");
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
// 450..1500 Hz in 25 Hz steps covers all 16 tones at every CFO hypothesis.
const SEARCH_BINS: usize = 43;
const TEMPLATE_SAMPLES: usize = 2 * SYMBOL_SAMPLES;
const CFO_HYPOTHESES: [usize; 13] = [6, 5, 7, 4, 8, 3, 9, 2, 10, 1, 11, 0, 12];
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

/// Continuous experimental receiver. Decode one overlapping window per second,
/// independently of caller chunking. Retain at most ten seconds of audio and
/// try at most 16 separated sync candidates per CFO hypothesis, with 13 hypotheses
/// spanning -150..150 Hz in 25 Hz steps. It is not a selected production waveform.
#[derive(Debug, Default)]
pub struct CandidateStream {
    candidate: Candidate,
    audio: Vec<f32>,
    pending: usize,
    consumed: u64,
    delivered_until: u64,
    attempts: u64,
}
impl CandidateStream {
    /// Append arbitrary audio chunks and return CRC-validated frames once each.
    /// Nonfinite input breaks acquisition history, including partial frames.
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
                if let Ok((payload, end)) = self.candidate.receive_after(&self.audio, min_start) {
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
    /// Build reusable codec and 450..1500 Hz search templates in 25 Hz steps.
    #[must_use]
    pub fn new() -> Self {
        let oscillators = (0..SEARCH_BINS)
            .map(|tone| {
                (0..TEMPLATE_SAMPLES)
                    .map(|n| {
                        Complex32::from_polar(
                            1.0,
                            std::f32::consts::TAU * (450 + 25 * tone) as f32 * n as f32
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
            audio.extend(
                self.oscillators[6 + 2 * tone][..SYMBOL_SAMPLES]
                    .iter()
                    .map(|z| z.re),
            );
        }
        Ok(audio)
    }

    fn energies(&self, audio: &[f32]) -> Vec<[f32; SEARCH_BINS]> {
        // Sliding matched filters evaluate all eight symbol phases. The 25 Hz
        // search grid repeats every two symbols, including half-bin offsets.
        if audio.len() < SYMBOL_SAMPLES {
            return Vec::new();
        }
        let mut sums = [Complex32::new(0.0, 0.0); SEARCH_BINS];
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
                    *sum += self.oscillators[tone][(n + SYMBOL_SAMPLES) % TEMPLATE_SAMPLES]
                        * audio[n + SYMBOL_SAMPLES]
                        - self.oscillators[tone][n % TEMPLATE_SAMPLES] * audio[n];
                }
            }
            start += HOP;
        }
        out
    }

    /// Search unknown sample alignment and 13 CFO hypotheses, trying at most 16
    /// separated candidates per hypothesis,
    /// and deliver only a CRC-validated PDU. No timing or header oracle is used.
    /// The fixed search grid is -150..150 Hz in 25 Hz steps. It is not tracking
    /// intra-frame drift or guaranteeing reception between grid points.
    /// # Errors
    /// Reports the deepest receiver stage reached; never delivers unchecked bits.
    pub fn receive(&self, audio: &[f32]) -> Result<Vec<u8>, CandidateError> {
        self.receive_after(audio, 0).map(|(payload, _)| payload)
    }

    fn receive_after(
        &self,
        audio: &[f32],
        min_start: usize,
    ) -> Result<(Vec<u8>, usize), CandidateError> {
        if audio.len() > CAPTURE_LIMIT || audio.iter().any(|x| !x.is_finite()) {
            return Err(CandidateError::Acquisition);
        }
        let bank = self.energies(audio);
        let mut deepest = CandidateError::Acquisition;
        // Reuse the same matched-filter bank across hypotheses. Only a single
        // 16-tone energy view is allocated at a time. Zero CFO is tried first.
        for offset in CFO_HYPOTHESES {
            let energy: Vec<[f32; 16]> = bank
                .iter()
                .map(|row| std::array::from_fn(|tone| row[offset + 2 * tone]))
                .collect();
            match self.receive_energies(&energy, min_start) {
                Ok(frame) => return Ok(frame),
                Err(CandidateError::Payload) => deepest = CandidateError::Payload,
                Err(CandidateError::Header) if deepest == CandidateError::Acquisition => {
                    deepest = CandidateError::Header;
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
    ) -> Result<(Vec<u8>, usize), CandidateError> {
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
            let Some(header) = decode_header(energy, base, stride) else {
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
            return Ok((
                bytes[..usize::from(header[1])].to_vec(),
                start * HOP + FRAME_SAMPLES,
            ));
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
