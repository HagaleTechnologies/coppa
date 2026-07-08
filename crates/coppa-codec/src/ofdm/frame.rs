//! OFDM superframe structure.
//!
//! [STS][STS][LTS+CP][LTS+CP][SIGNAL+CP][DATA+CP]...[DATA+CP]
use super::OfdmProfile;
use num_complex::Complex32;

/// OFDM frame types carried in the SIGNAL field.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum FrameType {
    Data = 0,
    Ack = 1,
    Control = 2,
    Beacon = 3,
}

impl FrameType {
    pub fn from_u8(val: u8) -> Self {
        match val & 0x03 {
            0 => FrameType::Data,
            1 => FrameType::Ack,
            2 => FrameType::Control,
            _ => FrameType::Beacon,
        }
    }
}

/// SIGNAL field contents (decoded from the first OFDM data symbol).
#[derive(Debug, Clone)]
pub struct SignalField {
    /// Modulation and Coding Scheme index (0-10).
    pub mcs_index: u8,
    /// Frame type.
    pub frame_type: FrameType,
    /// Payload length in bytes.
    pub payload_length: u16,
}

impl SignalField {
    /// Encode SIGNAL field to 32 bits.
    pub fn to_bits(&self) -> Vec<u8> {
        let mut bits = Vec::with_capacity(32);

        // MCS index: 4 bits
        for i in (0..4).rev() {
            bits.push((self.mcs_index >> i) & 1);
        }

        // Frame type: 2 bits
        let ft = self.frame_type as u8;
        bits.push((ft >> 1) & 1);
        bits.push(ft & 1);

        // Payload length: 16 bits
        for i in (0..16).rev() {
            bits.push(((self.payload_length >> i) & 1) as u8);
        }

        // Reserved: 10 bits
        bits.resize(bits.len() + 10, 0);

        bits
    }

    /// Decode SIGNAL field from 32 bits.
    pub fn from_bits(bits: &[u8]) -> Option<Self> {
        if bits.len() < 32 {
            return None;
        }

        let mut mcs_index = 0u8;
        for (i, &bit) in bits[..4].iter().enumerate() {
            mcs_index |= (bit & 1) << (3 - i);
        }

        let ft = ((bits[4] & 1) << 1) | (bits[5] & 1);
        let frame_type = FrameType::from_u8(ft);

        let mut payload_length = 0u16;
        for i in 0..16 {
            payload_length |= ((bits[6 + i] & 1) as u16) << (15 - i);
        }

        // Validate mcs_index is within MCS_TABLE range (0-10)
        if mcs_index > 10 {
            return None;
        }

        Some(Self {
            mcs_index,
            frame_type,
            payload_length,
        })
    }
}

/// Generates the Short Training Sequence (STS) for coarse timing/CFO.
///
/// Only even-indexed subcarrier bins are populated (Hermitian-symmetric layout).
/// This ensures the N-point IFFT produces a time-domain waveform whose first
/// N/2 samples are identical to the second N/2 samples, which is required by
/// the Schmidl-Cox synchronization algorithm.
pub fn generate_sts(profile: &OfdmProfile) -> Vec<Complex32> {
    let n = profile.fft_size;
    let mut freq = vec![Complex32::new(0.0, 0.0); n];

    let n_active = profile.active_carriers();
    let half = n_active.div_ceil(2);

    // Place values only on even-indexed bins using Hermitian symmetry layout,
    // with a simple PN-like pattern (+1, -1, +1, -1, ...) for active even bins.
    for (pn_idx, i) in (0..n_active).step_by(2).enumerate() {
        let val = if pn_idx % 2 == 0 {
            Complex32::new(1.0, 0.0)
        } else {
            Complex32::new(-1.0, 0.0)
        };

        if i < half {
            // Positive frequency bins: indices 2, 4, 6, ...
            let bin = (i + 1) * 2; // even bin index (2, 6, 10, ...)
            if bin < n {
                freq[bin] = val;
                // Hermitian conjugate in the negative frequency mirror
                freq[n - bin] = val.conj();
            }
        } else {
            // Negative frequency bins mapped to upper half of FFT
            let offset = n - n_active + i;
            let bin = offset - (offset % 2); // snap to even bin
            if bin > 0 && bin < n {
                freq[bin] = val;
                // Hermitian conjugate
                freq[n - bin] = val.conj();
            }
        }
    }

    // Ensure only even bins are non-zero (clear any odd-bin artifacts)
    for k in (1..n).step_by(2) {
        freq[k] = Complex32::new(0.0, 0.0);
    }

    freq
}

/// Generates the Long Training Sequence (LTS) for fine timing and channel estimation.
///
/// All active subcarriers are modulated with known BPSK values.
/// Hermitian symmetry is enforced: X[N-k] = conj(X[k]) so the IFFT
/// produces a real-valued time-domain waveform.
pub fn generate_lts(profile: &OfdmProfile) -> Vec<Complex32> {
    let n = profile.fft_size;
    let mut freq = vec![Complex32::new(0.0, 0.0); n];

    let n_active = profile.active_carriers();
    let half = n_active.div_ceil(2);

    // Place positive-frequency bins (indices 1..half) with a PN-like BPSK pattern,
    // then mirror into negative-frequency bins with conjugate symmetry.
    for i in 0..half {
        let val = if i % 2 == 0 {
            Complex32::new(1.0, 0.0)
        } else {
            Complex32::new(-1.0, 0.0)
        };

        let bin = i + 1; // positive frequency bin
        if bin < n {
            freq[bin] = val;
            // Enforce Hermitian symmetry: X[N-k] = conj(X[k])
            freq[n - bin] = val.conj();
        }
    }

    freq
}

// ---------------------------------------------------------------------------
// Coppa Protocol frame header types
// ---------------------------------------------------------------------------

/// Coppa Protocol frame type carried in the 48-bit CoppaHeader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoppaFrameType {
    Data = 0,
    Ack = 1,
    Nak = 2,
    Connect = 3,
    ConnectAck = 4,
    Disconnect = 5,
    Beacon = 6,
}

impl CoppaFrameType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(CoppaFrameType::Data),
            1 => Some(CoppaFrameType::Ack),
            2 => Some(CoppaFrameType::Nak),
            3 => Some(CoppaFrameType::Connect),
            4 => Some(CoppaFrameType::ConnectAck),
            5 => Some(CoppaFrameType::Disconnect),
            6 => Some(CoppaFrameType::Beacon),
            _ => None,
        }
    }
}

/// Coppa Protocol 48-bit frame header.
///
/// Bit layout (6 bytes, MSB first within each field):
/// ```text
/// byte 0: [version:4][phy_mode:4]
/// byte 1: [frame_type:4][bandwidth:4]
/// byte 2: [fec_type:4][speed_level:4]
/// byte 3: [seq_num:8]
/// byte 4: [payload_len high 8 bits]
/// byte 5: [payload_len low 4 bits][reserved:4]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoppaHeader {
    /// Protocol version (4 bits).
    pub version: u8,
    /// PHY mode identifier (4 bits).
    pub phy_mode: u8,
    /// Frame type (4 bits).
    pub frame_type: CoppaFrameType,
    /// Bandwidth class (4 bits).
    pub bandwidth: u8,
    /// FEC scheme identifier (4 bits). As of Phase 3 Task 3 (IR-HARQ), the
    /// low 2 bits (`fec_type & 0x03`) carry the frame's redundancy version
    /// (RV, 0-3) for incremental-redundancy HARQ combining -- see
    /// `coppa_protocol::modem::transceiver`'s `RV_MASK` and
    /// `coppa_protocol::arq::rv_for_attempt`. The high 2 bits are reserved/
    /// unused (always `0` today), available for a future FEC-scheme selector
    /// without another wire-format break. A fresh (non-retransmitted) frame
    /// always has `fec_type = 0` (RV0), matching the pre-Task-3 wire format
    /// exactly.
    pub fec_type: u8,
    /// Speed/MCS level (4 bits).
    pub speed_level: u8,
    /// Sequence number (8 bits).
    pub seq_num: u8,
    /// Payload length in bytes (12 bits, max 4095).
    pub payload_len: u16,
}

impl CoppaHeader {
    /// Pack the header into 6 bytes (the on-air field layout).
    pub fn to_bytes(&self) -> [u8; 6] {
        let payload_hi = ((self.payload_len >> 4) & 0xFF) as u8;
        let payload_lo = ((self.payload_len & 0x0F) as u8) << 4; // low nibble in upper half of byte 5
        [
            (self.version << 4) | (self.phy_mode & 0x0F),
            ((self.frame_type as u8) << 4) | (self.bandwidth & 0x0F),
            (self.fec_type << 4) | (self.speed_level & 0x0F),
            self.seq_num,
            payload_hi,
            payload_lo, // reserved nibble is 0
        ]
    }

    /// Reconstruct a `CoppaHeader` from 6 bytes. Returns `None` on an invalid frame type.
    pub fn from_bytes(bytes: &[u8; 6]) -> Option<Self> {
        let version = (bytes[0] >> 4) & 0x0F;
        let phy_mode = bytes[0] & 0x0F;
        let frame_type_raw = (bytes[1] >> 4) & 0x0F;
        let bandwidth = bytes[1] & 0x0F;
        let fec_type = (bytes[2] >> 4) & 0x0F;
        let speed_level = bytes[2] & 0x0F;
        let seq_num = bytes[3];
        let payload_len = ((bytes[4] as u16) << 4) | ((bytes[5] as u16) >> 4);
        let frame_type = CoppaFrameType::from_u8(frame_type_raw)?;
        Some(CoppaHeader {
            version,
            phy_mode,
            frame_type,
            bandwidth,
            fec_type,
            speed_level,
            seq_num,
            payload_len,
        })
    }

    /// Pack the header into 48 bits (one bit per element, MSB first).
    pub fn to_bits(&self) -> Vec<u8> {
        let bytes = self.to_bytes();
        let mut bits = Vec::with_capacity(48);
        for byte in &bytes {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1);
            }
        }
        bits
    }

    /// Reconstruct a `CoppaHeader` from 48 bits (one bit per element, MSB first).
    /// Returns `None` if the bit slice is too short or contains an invalid frame type.
    pub fn from_bits(bits: &[u8]) -> Option<Self> {
        if bits.len() < 48 {
            return None;
        }
        let mut bytes = [0u8; 6];
        for (i, byte) in bytes.iter_mut().enumerate() {
            for bit_idx in 0..8 {
                *byte |= (bits[i * 8 + bit_idx] & 1) << (7 - bit_idx);
            }
        }
        Self::from_bytes(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_field_roundtrip() {
        let signal = SignalField {
            mcs_index: 7,
            frame_type: FrameType::Data,
            payload_length: 1234,
        };
        let bits = signal.to_bits();
        assert_eq!(bits.len(), 32);

        let decoded = SignalField::from_bits(&bits).unwrap();
        assert_eq!(decoded.mcs_index, 7);
        assert_eq!(decoded.frame_type, FrameType::Data);
        assert_eq!(decoded.payload_length, 1234);
    }

    #[test]
    fn test_signal_field_all_mcs() {
        for mcs in 0..11u8 {
            let signal = SignalField {
                mcs_index: mcs,
                frame_type: FrameType::Ack,
                payload_length: 0,
            };
            let bits = signal.to_bits();
            let decoded = SignalField::from_bits(&bits).unwrap();
            assert_eq!(decoded.mcs_index, mcs);
        }
    }

    #[test]
    fn test_sts_generation() {
        let profile = OfdmProfile::HF_STANDARD;
        let sts = generate_sts(&profile);
        let n = profile.fft_size;
        assert_eq!(sts.len(), n);

        // Count non-zero subcarriers
        let active: usize = sts.iter().filter(|s| s.norm() > 0.5).count();
        assert!(active > 0, "STS should have some active subcarriers");

        // Only even-indexed bins should be non-zero
        for (k, val) in sts.iter().enumerate() {
            if k % 2 == 1 {
                assert!(
                    val.norm() < 1e-6,
                    "Odd bin {} should be zero, got {:?}",
                    k,
                    val
                );
            }
        }

        // Verify the two-identical-halves property via IFFT:
        // When only even bins are non-zero, the IFFT time-domain signal
        // has time[0..N/2] == time[N/2..N].
        use rustfft::num_complex::Complex;
        use rustfft::FftPlanner;
        let mut planner = FftPlanner::<f32>::new();
        let ifft = planner.plan_fft_inverse(n);
        let mut time: Vec<Complex<f32>> = sts.iter().map(|c| Complex::new(c.re, c.im)).collect();
        ifft.process(&mut time);
        let tol = 1e-4;
        for i in 0..n / 2 {
            let diff = (time[i] - time[n / 2 + i]).norm();
            assert!(
                diff < tol,
                "STS two-halves mismatch at index {}: {:?} vs {:?}",
                i,
                time[i],
                time[n / 2 + i]
            );
        }
    }

    #[test]
    fn test_coppa_header_roundtrip() {
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 2,
            fec_type: 3,
            speed_level: 5,
            seq_num: 42,
            payload_len: 512,
        };
        let bits = header.to_bits();
        assert_eq!(bits.len(), 48);

        let decoded = CoppaHeader::from_bits(&bits).unwrap();
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.phy_mode, 0);
        assert_eq!(decoded.frame_type, CoppaFrameType::Data);
        assert_eq!(decoded.bandwidth, 2);
        assert_eq!(decoded.fec_type, 3);
        assert_eq!(decoded.speed_level, 5);
        assert_eq!(decoded.seq_num, 42);
        assert_eq!(decoded.payload_len, 512);
    }

    #[test]
    fn test_coppa_header_all_frame_types() {
        let variants = [
            CoppaFrameType::Data,
            CoppaFrameType::Ack,
            CoppaFrameType::Nak,
            CoppaFrameType::Connect,
            CoppaFrameType::ConnectAck,
            CoppaFrameType::Disconnect,
            CoppaFrameType::Beacon,
        ];
        for ft in &variants {
            let header = CoppaHeader {
                version: 1,
                phy_mode: 1,
                frame_type: *ft,
                bandwidth: 1,
                fec_type: 1,
                speed_level: 2,
                seq_num: 0,
                payload_len: 100,
            };
            let bits = header.to_bits();
            let decoded = CoppaHeader::from_bits(&bits).unwrap();
            assert_eq!(
                decoded.frame_type, *ft,
                "Frame type {:?} roundtrip failed",
                ft
            );
        }
    }

    #[test]
    fn test_coppa_header_max_payload_len() {
        let header = CoppaHeader {
            version: 0,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 0,
            fec_type: 0,
            speed_level: 10,
            seq_num: 255,
            payload_len: 4095,
        };
        let bits = header.to_bits();
        assert_eq!(bits.len(), 48);

        let decoded = CoppaHeader::from_bits(&bits).unwrap();
        assert_eq!(decoded.payload_len, 4095);
        assert_eq!(decoded.seq_num, 255);
        assert_eq!(decoded.speed_level, 10);
    }

    #[test]
    fn test_coppa_header_bytes_roundtrip() {
        let header = CoppaHeader {
            version: 1,
            phy_mode: 2,
            frame_type: CoppaFrameType::Data,
            bandwidth: 3,
            fec_type: 4,
            speed_level: 6,
            seq_num: 200,
            payload_len: 1234,
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 6);
        assert_eq!(CoppaHeader::from_bytes(&bytes), Some(header.clone()));
        let mut expected_bits = Vec::new();
        for byte in &bytes {
            for shift in (0..8).rev() {
                expected_bits.push((byte >> shift) & 1);
            }
        }
        assert_eq!(header.to_bits(), expected_bits);
    }

    #[test]
    fn test_lts_generation() {
        let profile = OfdmProfile::HF_STANDARD;
        let lts = generate_lts(&profile);
        let n = profile.fft_size;
        assert_eq!(lts.len(), n);

        // Verify Hermitian symmetry: X[N-k] = conj(X[k])
        for k in 1..n {
            let diff = (lts[k] - lts[n - k].conj()).norm();
            assert!(
                diff < 1e-6,
                "LTS Hermitian symmetry violated at bin {}: X[{}]={:?}, X[{}]={:?}",
                k,
                k,
                lts[k],
                n - k,
                lts[n - k]
            );
        }

        // Verify the IFFT produces a real-valued signal
        use rustfft::num_complex::Complex;
        use rustfft::FftPlanner;
        let mut planner = FftPlanner::<f32>::new();
        let ifft = planner.plan_fft_inverse(n);
        let mut time: Vec<Complex<f32>> = lts.iter().map(|c| Complex::new(c.re, c.im)).collect();
        ifft.process(&mut time);
        for (i, t) in time.iter().enumerate() {
            assert!(
                t.im.abs() < 1e-3,
                "LTS IFFT should be real-valued, but sample {} has im={}",
                i,
                t.im
            );
        }
    }
}
