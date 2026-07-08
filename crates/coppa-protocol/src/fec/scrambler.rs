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

/// Return the first `len` bits of the PRBS keystream `scramble` XORs in, in order.
///
/// The LFSR state transition in `scramble` depends only on its own running state
/// (`output` is derived purely from `lfsr`, never from the input data), so the
/// keystream at a given index is the same regardless of what data was scrambled
/// before it. Consequently, XOR-ing a bit that is `0` on TX with the keystream
/// leaves the keystream value unchanged: `scramble`d zero-padding at info-bit
/// index `i` is *exactly* `prbs_bits(n)[i]` for any `n > i`. This is what makes
/// known-pad LLR pinning possible in `CoppaTransceiver::receive`: the RX knows
/// the zero-padded tail of `payload_bits` was scrambled to this exact value on TX,
/// without needing to know the payload itself.
pub fn prbs_bits(len: usize) -> Vec<u8> {
    let mut bits = vec![0u8; len];
    scramble(&mut bits);
    bits
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

    #[test]
    fn test_prbs_bits_matches_scramble_of_zeros() {
        let len = 972;
        let mut zeros = vec![0u8; len];
        scramble(&mut zeros);
        assert_eq!(prbs_bits(len), zeros);
    }

    /// The keystream at a given index doesn't depend on data scrambled before it
    /// (the LFSR only ever mixes its own prior state), so a prefix of a longer
    /// `prbs_bits` call must equal the shorter call outright -- this is exactly
    /// what lets `receive` compute the pad's ground truth as
    /// `prbs_bits(info_bits)[payload_bits..]` regardless of what the payload bits
    /// before the pad were.
    #[test]
    fn test_prbs_bits_prefix_is_stable_regardless_of_length() {
        let long = prbs_bits(972);
        let short = prbs_bits(300);
        assert_eq!(&long[..300], short.as_slice());
    }
}
