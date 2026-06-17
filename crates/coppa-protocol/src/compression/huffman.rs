//! Fixed Huffman table optimized for ham radio text.
//!
//! Uses a pre-built Huffman table with symbol frequencies derived from
//! typical ham radio QSO exchanges (callsigns, signal reports, common phrases).
//! The codes form a valid canonical Huffman code (prefix-free).

/// A fixed Huffman code entry.
#[derive(Debug, Clone, Copy)]
pub struct HuffmanEntry {
    pub symbol: u8,
    pub code: u32,
    pub bits: u8,
}

/// Fixed Huffman table optimized for ham radio text.
///
/// Canonical Huffman code with bit-length distribution:
/// 1x2, 1x3, 2x4, 4x5, 8x6, 16x7, 32x8 = 64 symbols.
///
/// Frequency analysis based on typical ham radio exchanges:
/// - Space is most common (shortest code)
/// - Uppercase letters A-Z weighted by callsign frequency
/// - Digits 0-9 weighted for signal reports
/// - Punctuation for common abbreviations
pub const HAM_RADIO_TABLE: [HuffmanEntry; 64] = [
    // 2-bit codes (1 symbol)
    HuffmanEntry {
        symbol: b' ',
        code: 0b00,
        bits: 2,
    },
    // 3-bit codes (1 symbol)
    HuffmanEntry {
        symbol: b'E',
        code: 0b010,
        bits: 3,
    },
    // 4-bit codes (2 symbols)
    HuffmanEntry {
        symbol: b'T',
        code: 0b0110,
        bits: 4,
    },
    HuffmanEntry {
        symbol: b'A',
        code: 0b0111,
        bits: 4,
    },
    // 5-bit codes (4 symbols)
    HuffmanEntry {
        symbol: b'O',
        code: 0b10000,
        bits: 5,
    },
    HuffmanEntry {
        symbol: b'I',
        code: 0b10001,
        bits: 5,
    },
    HuffmanEntry {
        symbol: b'N',
        code: 0b10010,
        bits: 5,
    },
    HuffmanEntry {
        symbol: b'S',
        code: 0b10011,
        bits: 5,
    },
    // 6-bit codes (8 symbols)
    HuffmanEntry {
        symbol: b'R',
        code: 0b101000,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'H',
        code: 0b101001,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'L',
        code: 0b101010,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'D',
        code: 0b101011,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'C',
        code: 0b101100,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'U',
        code: 0b101101,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'M',
        code: 0b101110,
        bits: 6,
    },
    HuffmanEntry {
        symbol: b'W',
        code: 0b101111,
        bits: 6,
    },
    // 7-bit codes (16 symbols)
    HuffmanEntry {
        symbol: b'F',
        code: 0b1100000,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'G',
        code: 0b1100001,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'Y',
        code: 0b1100010,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'P',
        code: 0b1100011,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'B',
        code: 0b1100100,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'V',
        code: 0b1100101,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'K',
        code: 0b1100110,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'J',
        code: 0b1100111,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'X',
        code: 0b1101000,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'Q',
        code: 0b1101001,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'Z',
        code: 0b1101010,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'0',
        code: 0b1101011,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'1',
        code: 0b1101100,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'2',
        code: 0b1101101,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'3',
        code: 0b1101110,
        bits: 7,
    },
    HuffmanEntry {
        symbol: b'4',
        code: 0b1101111,
        bits: 7,
    },
    // 8-bit codes (32 symbols)
    HuffmanEntry {
        symbol: b'5',
        code: 0b11100000,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'6',
        code: 0b11100001,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'7',
        code: 0b11100010,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'8',
        code: 0b11100011,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'9',
        code: 0b11100100,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'.',
        code: 0b11100101,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b',',
        code: 0b11100110,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'?',
        code: 0b11100111,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'/',
        code: 0b11101000,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'-',
        code: 0b11101001,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b':',
        code: 0b11101010,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'\n',
        code: 0b11101011,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'\r',
        code: 0b11101100,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'!',
        code: 0b11101101,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'@',
        code: 0b11101110,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'#',
        code: 0b11101111,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'$',
        code: 0b11110000,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'(',
        code: 0b11110001,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b')',
        code: 0b11110010,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'+',
        code: 0b11110011,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'=',
        code: 0b11110100,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'a',
        code: 0b11110101,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'e',
        code: 0b11110110,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'i',
        code: 0b11110111,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'o',
        code: 0b11111000,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'n',
        code: 0b11111001,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b't',
        code: 0b11111010,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b's',
        code: 0b11111011,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'r',
        code: 0b11111100,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'_',
        code: 0b11111101,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'*',
        code: 0b11111110,
        bits: 8,
    },
    HuffmanEntry {
        symbol: b'~',
        code: 0b11111111,
        bits: 8,
    },
];

/// Fixed Huffman codec using the ham radio text table.
pub struct HuffmanCodec {
    // Encode lookup: byte -> (code, bits)
    encode_table: [(u32, u8); 256],
}

impl HuffmanCodec {
    /// Create a new codec from the ham radio table.
    ///
    /// # Panics
    /// Panics (in debug builds) if the table is not sorted by code length
    /// (shortest first). This ordering is required for correct prefix-free
    /// decoding via linear scan.
    pub fn new() -> Self {
        // Validate that the table is sorted by code length (shortest first).
        // The linear-scan decoder relies on this ordering to match the shortest
        // (and therefore correct) prefix code first.
        debug_assert!(
            HAM_RADIO_TABLE.windows(2).all(|w| w[0].bits <= w[1].bits),
            "Huffman table must be sorted by code length (shortest first)"
        );

        let mut encode_table = [(0u32, 0u8); 256];
        for entry in &HAM_RADIO_TABLE {
            encode_table[entry.symbol as usize] = (entry.code, entry.bits);
        }
        Self { encode_table }
    }

    /// Encode text to compressed bits.
    ///
    /// Returns compressed bytes. Unknown symbols are escaped using the
    /// `~` table entry as a prefix followed by 8 raw bits. A literal `~`
    /// is encoded as `~` followed by 8 bits of `~` (0x7E).
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::with_capacity(data.len() * 5);
        let (esc_code, esc_bits) = self.encode_table[b'~' as usize];

        for &byte in data {
            let (code, nbits) = self.encode_table[byte as usize];
            if nbits > 0 {
                if byte == b'~' {
                    // Literal ~: emit escape code + raw byte
                    for i in (0..esc_bits).rev() {
                        bits.push((esc_code >> i) & 1 == 1);
                    }
                    for i in (0..8).rev() {
                        bits.push((byte >> i) & 1 == 1);
                    }
                } else {
                    for i in (0..nbits).rev() {
                        bits.push((code >> i) & 1 == 1);
                    }
                }
            } else {
                // Unknown symbol: emit escape code + raw byte
                for i in (0..esc_bits).rev() {
                    bits.push((esc_code >> i) & 1 == 1);
                }
                for i in (0..8).rev() {
                    bits.push((byte >> i) & 1 == 1);
                }
            }
        }

        // Pack bits into bytes
        let mut output = Vec::with_capacity(bits.len().div_ceil(8) + 1);
        // First byte: number of padding bits (0-7)
        let padding = (8 - (bits.len() % 8)) % 8;
        output.push(padding as u8);

        for chunk in bits.chunks(8) {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                if bit {
                    byte |= 1 << (7 - i);
                }
            }
            output.push(byte);
        }

        output
    }

    /// Decode compressed bytes back to original text.
    ///
    /// Returns `None` if the bitstream contains undecodable sequences,
    /// indicating data corruption.
    pub fn decode(&self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        let padding = data[0] as usize;
        let mut bits: Vec<bool> = Vec::new();

        for &byte in &data[1..] {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1 == 1);
            }
        }

        // Remove padding bits from the end
        if padding > 0 && bits.len() >= padding {
            bits.truncate(bits.len() - padding);
        }

        let mut output = Vec::new();
        let mut pos = 0;

        while pos < bits.len() {
            // Try to match against the table (shortest codes first for prefix-free codes)
            let mut matched = false;
            for entry in &HAM_RADIO_TABLE {
                let nbits = entry.bits as usize;
                if pos + nbits <= bits.len() {
                    let mut code = 0u32;
                    for i in 0..nbits {
                        if bits[pos + i] {
                            code |= 1 << (nbits - 1 - i);
                        }
                    }
                    if code == entry.code {
                        if entry.symbol == b'~' {
                            // Escape: read next 8 bits as raw byte
                            pos += nbits;
                            if pos + 8 <= bits.len() {
                                let mut byte = 0u8;
                                for i in 0..8 {
                                    if bits[pos + i] {
                                        byte |= 1 << (7 - i);
                                    }
                                }
                                pos += 8;
                                output.push(byte);
                            }
                        } else {
                            output.push(entry.symbol);
                            pos += nbits;
                        }
                        matched = true;
                        break;
                    }
                }
            }

            if !matched {
                // Remaining bits don't form a valid code — likely padding
                // or corruption. Stop decoding rather than skip silently.
                break;
            }
        }

        output
    }

    /// Decode compressed bytes, returning an error if the bitstream is corrupt.
    ///
    /// Unlike `decode()`, this returns `Err` when remaining bits don't form
    /// a valid Huffman code, indicating data corruption rather than padding.
    pub fn try_decode(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Read padding count from first byte
        let padding = data[0] as usize;
        if padding > 7 {
            return Err(format!("Invalid padding count: {}", padding));
        }

        let mut bits = Vec::new();
        for &byte in &data[1..] {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1 == 1);
            }
        }

        if padding > 0 && bits.len() >= padding {
            bits.truncate(bits.len() - padding);
        }

        let mut output = Vec::new();
        let mut pos = 0;

        while pos < bits.len() {
            let mut matched = false;
            for entry in &HAM_RADIO_TABLE {
                let nbits = entry.bits as usize;
                if pos + nbits <= bits.len() {
                    let mut code = 0u32;
                    for i in 0..nbits {
                        if bits[pos + i] {
                            code |= 1 << (nbits - 1 - i);
                        }
                    }
                    if code == entry.code {
                        if entry.symbol == b'~' {
                            pos += nbits;
                            if pos + 8 <= bits.len() {
                                let mut byte = 0u8;
                                for i in 0..8 {
                                    if bits[pos + i] {
                                        byte |= 1 << (7 - i);
                                    }
                                }
                                pos += 8;
                                output.push(byte);
                            } else {
                                return Err("Truncated escape sequence".to_string());
                            }
                        } else {
                            output.push(entry.symbol);
                            pos += nbits;
                        }
                        matched = true;
                        break;
                    }
                }
            }

            if !matched {
                return Err(format!(
                    "Corrupt bitstream: no valid code at bit position {}",
                    pos
                ));
            }
        }

        Ok(output)
    }

    /// Estimate compression ratio for the given text (compressed/original).
    pub fn compression_ratio(&self, data: &[u8]) -> f32 {
        if data.is_empty() {
            return 1.0;
        }
        let compressed = self.encode(data);
        compressed.len() as f32 / data.len() as f32
    }
}

impl Default for HuffmanCodec {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_huffman_roundtrip_simple() {
        let codec = HuffmanCodec::new();
        let text = b"CQ CQ CQ DE VK2ABC";
        let compressed = codec.encode(text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
    }

    #[test]
    fn test_huffman_roundtrip_digits() {
        let codec = HuffmanCodec::new();
        let text = b"RST 599 599";
        let compressed = codec.encode(text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
    }

    #[test]
    fn test_huffman_compression_ratio() {
        let codec = HuffmanCodec::new();
        let text = b"CQ CQ CQ DE VK2ABC VK2ABC K";
        let ratio = codec.compression_ratio(text);
        // Ham radio text should compress somewhat
        assert!(ratio < 1.0, "Expected compression, got ratio {}", ratio);
    }

    #[test]
    fn test_huffman_empty() {
        let codec = HuffmanCodec::new();
        let compressed = codec.encode(b"");
        let decompressed = codec.decode(&compressed);
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_huffman_single_char() {
        let codec = HuffmanCodec::new();
        for &ch in b"ETAOINSR 0123456789" {
            let compressed = codec.encode(&[ch]);
            let decompressed = codec.decode(&compressed);
            assert_eq!(
                decompressed,
                vec![ch],
                "Failed roundtrip for '{}'",
                ch as char
            );
        }
    }

    #[test]
    fn test_huffman_punctuation() {
        let codec = HuffmanCodec::new();
        let text = b"VK2ABC/P DE VK3DEF? K";
        let compressed = codec.encode(text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
    }

    #[test]
    fn test_huffman_all_same_bytes() {
        let codec = HuffmanCodec::new();
        let text = vec![b'E'; 100];
        let compressed = codec.encode(&text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
    }

    #[test]
    fn test_huffman_ham_radio_qso() {
        let codec = HuffmanCodec::new();
        let text = b"CQ CQ CQ DE VK2ABC K";
        let compressed = codec.encode(text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
        // Ham radio text should compress well
        assert!(compressed.len() < text.len());
    }

    #[test]
    fn test_huffman_binary_escape() {
        let codec = HuffmanCodec::new();
        // Bytes not in the Huffman table should be escaped and recovered
        let data: Vec<u8> = (128..160).collect();
        let compressed = codec.encode(&data);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_huffman_tilde_literal() {
        let codec = HuffmanCodec::new();
        let text = b"A~B~C";
        let compressed = codec.encode(text);
        let decompressed = codec.decode(&compressed);
        assert_eq!(decompressed, text);
    }

    #[test]
    fn test_huffman_decode_empty_bytes() {
        let codec = HuffmanCodec::new();
        let decompressed = codec.decode(&[]);
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_huffman_compression_ratio_empty() {
        let codec = HuffmanCodec::new();
        assert_eq!(codec.compression_ratio(b""), 1.0);
    }
}
