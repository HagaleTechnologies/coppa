//! Extended binary Golay (24,12,8) code: per 24-bit codeword, corrects up to 3 bit
//! errors and detects 4. Systematic generator G = [I_12 | A]; parity-check
//! H = [A^T | I_12]. Used to protect the frame header (see `header_fec`).
//!
//! Constants are verified offline: G has minimum distance 8; H annihilates all 4096
//! codewords; the 2325 weight-<=3 error syndromes are all distinct; every weight-4
//! error has a syndrome outside that table (so it is detected, not miscorrected).

use std::collections::HashMap;
use std::sync::OnceLock;

/// Row masks (12-bit, MSB = column 0) of `A` in the systematic generator G = [I | A].
const A_ROWS: [u16; 12] = [
    0x7FF, 0xEE2, 0xB71, 0xDB8, 0xADC, 0x96E, 0x8B7, 0xC5B, 0xE2D, 0xF16, 0xB8B, 0xDC5,
];

/// Row masks (12-bit) of `A^T`, used to build the parity-check H = [A^T | I].
const AT_ROWS: [u16; 12] = [
    0x7FF, 0xD1D, 0xE8E, 0xB47, 0xDA3, 0xED1, 0xF68, 0xBB4, 0x9DA, 0x8ED, 0xC76, 0xA3B,
];

/// Encode 12 info bits (low 12 bits of `info`, MSB = bit 11) into a 24-bit codeword
/// `[info(12) | parity(12)]`, with info in bits 23..12.
pub fn golay24_encode(info: u16) -> u32 {
    let mut cw: u32 = 0;
    for (i, &a_row) in A_ROWS.iter().enumerate() {
        if (info >> (11 - i)) & 1 == 1 {
            cw ^= (1u32 << (23 - i)) | a_row as u32;
        }
    }
    cw
}

/// 12-bit syndrome of a 24-bit received word under H = [A^T | I].
fn syndrome(r: u32) -> u16 {
    let mut s: u16 = 0;
    for (i, &at_row) in AT_ROWS.iter().enumerate() {
        let h_row: u32 = ((at_row as u32) << 12) | (1u32 << (11 - i));
        s = (s << 1) | ((r & h_row).count_ones() & 1) as u16;
    }
    s
}

/// Lazily-built map: syndrome -> coset-leader error pattern, over all weight-<=3
/// errors on 24 bits (2325 entries; all syndromes distinct).
fn syndrome_table() -> &'static HashMap<u16, u32> {
    static TABLE: OnceLock<HashMap<u16, u32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(syndrome(0), 0u32);
        for a in 0..24 {
            let e1 = 1u32 << a;
            m.entry(syndrome(e1)).or_insert(e1);
            for b in (a + 1)..24 {
                let e2 = e1 | (1u32 << b);
                m.entry(syndrome(e2)).or_insert(e2);
                for c in (b + 1)..24 {
                    let e3 = e2 | (1u32 << c);
                    m.entry(syndrome(e3)).or_insert(e3);
                }
            }
        }
        m
    })
}

/// Decode a 24-bit received word. Returns `(info12, n_corrected)` when the error is
/// correctable (weight <= 3), else `None` (>= 4 errors detected).
pub fn golay24_decode(received: u32) -> Option<(u16, u8)> {
    let r = received & 0x00FF_FFFF;
    let s = syndrome(r);
    let err = *syndrome_table().get(&s)?;
    let corrected = r ^ err;
    let info = ((corrected >> 12) & 0xFFF) as u16;
    Some((info, err.count_ones() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_roundtrip() {
        for info in [0u16, 0xFFF, 0xABC, 0x001, 0x800, 0x555, 0xAAA] {
            let cw = golay24_encode(info);
            assert_eq!(golay24_decode(cw), Some((info, 0)), "info={info:#05x}");
        }
    }

    #[test]
    fn corrects_all_weight_le_3_errors_on_zero_codeword() {
        let base = golay24_encode(0);
        assert_eq!(base, 0);
        for a in 0..24u32 {
            let e1 = 1 << a;
            assert_eq!(golay24_decode(base ^ e1), Some((0, 1)), "1-bit at {a}");
            for b in (a + 1)..24 {
                let e2 = e1 | (1 << b);
                assert_eq!(golay24_decode(base ^ e2), Some((0, 2)), "2-bit {a},{b}");
                for c in (b + 1)..24 {
                    let e3 = e2 | (1 << c);
                    assert_eq!(golay24_decode(base ^ e3), Some((0, 3)), "3-bit {a},{b},{c}");
                }
            }
        }
    }

    #[test]
    fn corrects_three_errors_on_nonzero_codeword() {
        let info = 0xA5Cu16;
        let cw = golay24_encode(info);
        let corrupted = cw ^ 0b1 ^ (1 << 10) ^ (1 << 23);
        assert_eq!(golay24_decode(corrupted), Some((info, 3)));
    }

    #[test]
    fn detects_weight_4_errors() {
        let cw = golay24_encode(0x123);
        for combo in [
            0b1111u32,
            (1 << 0) | (1 << 6) | (1 << 12) | (1 << 18),
            (1 << 3) | (1 << 9) | (1 << 15) | (1 << 21),
        ] {
            assert_eq!(golay24_decode(cw ^ combo), None, "combo={combo:#b}");
        }
    }

    #[test]
    fn syndrome_table_has_2325_distinct_entries() {
        assert_eq!(syndrome_table().len(), 2325);
    }
}
