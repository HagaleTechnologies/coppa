//! PRBS scrambler for randomizing LDPC info bits.
//!
//! Uses polynomial x^15 + x^14 + 1 (DVB-S2 style) with a fixed seed.
//! XOR is self-inverse: scramble(scramble(data)) == data.

const PRBS_SEED: u16 = 0x4A80; // 15-bit seed with good initial mixing

/// XOR `bits` in-place with a deterministic PRBS sequence.
pub fn scramble(bits: &mut [u8]) {
    let mut lfsr: u16 = PRBS_SEED;
    for bit in bits.iter_mut() {
        let output = ((lfsr >> 14) ^ (lfsr >> 13)) & 1;
        *bit ^= output as u8;
        lfsr = ((lfsr << 1) | output) & 0x7FFF;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scramble_is_self_inverse() {
        let original = vec![0u8, 1, 0, 1, 1, 0, 0, 1, 0, 0];
        let mut data = original.clone();
        scramble(&mut data);
        assert_ne!(data, original, "scramble should change the data");
        scramble(&mut data);
        assert_eq!(data, original, "double scramble should restore original");
    }

    #[test]
    fn test_scramble_randomizes_zeros() {
        let mut zeros = vec![0u8; 972];
        scramble(&mut zeros);
        let ones_count: usize = zeros.iter().map(|&b| b as usize).sum();
        // Should be roughly 50% ones (486 +/- tolerance)
        assert!(
            ones_count > 400 && ones_count < 572,
            "PRBS should produce ~50% ones, got {}",
            ones_count
        );
    }

    #[test]
    fn test_scramble_deterministic() {
        let mut a = vec![0u8; 100];
        let mut b = vec![0u8; 100];
        scramble(&mut a);
        scramble(&mut b);
        assert_eq!(a, b, "same input should produce same output");
    }
}
