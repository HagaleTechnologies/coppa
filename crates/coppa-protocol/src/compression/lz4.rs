//! LZ4 compression wrapper using lz4_flex.

use anyhow::{anyhow, Result};

/// Maximum decompressed size (16 KiB). Prevents OOM from crafted input.
/// This is 64x the maximum frame payload (255 bytes), providing generous
/// headroom while still bounding allocation from malicious inputs.
const MAX_DECOMPRESS_SIZE: usize = 16_384;

/// Compress data using LZ4.
pub fn lz4_compress(data: &[u8]) -> Vec<u8> {
    lz4_flex::compress_prepend_size(data)
}

/// Decompress LZ4-compressed data.
///
/// Rejects inputs that claim a decompressed size larger than 16 KiB
/// to prevent out-of-memory attacks from crafted payloads.
pub fn lz4_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() >= 4 {
        let claimed_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if claimed_size > MAX_DECOMPRESS_SIZE {
            return Err(anyhow!(
                "LZ4 claimed decompressed size {} exceeds limit {}",
                claimed_size,
                MAX_DECOMPRESS_SIZE
            ));
        }
    }
    lz4_flex::decompress_size_prepended(data)
        .map_err(|e| anyhow!("LZ4 decompression failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lz4_roundtrip() {
        let data = b"CQ CQ CQ DE VK2ABC VK2ABC VK2ABC K";
        let compressed = lz4_compress(data);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz4_empty() {
        let compressed = lz4_compress(b"");
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_lz4_compresses_repeated_data() {
        let data = "ABCDEF".repeat(100);
        let compressed = lz4_compress(data.as_bytes());
        assert!(
            compressed.len() < data.len(),
            "LZ4 should compress repeated data: {} -> {}",
            data.len(),
            compressed.len()
        );
    }

    #[test]
    fn test_lz4_invalid_data() {
        let result = lz4_decompress(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn test_lz4_oversized_claim() {
        // Craft a payload that claims 2 MiB decompressed size
        let mut bad = vec![0u8; 8];
        let size: u32 = 2_000_000;
        bad[0..4].copy_from_slice(&size.to_le_bytes());
        let result = lz4_decompress(&bad);
        assert!(
            result.is_err(),
            "Should reject oversized decompression claim"
        );
    }

    #[test]
    fn test_lz4_single_byte() {
        let data = b"X";
        let compressed = lz4_compress(data);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz4_all_same_bytes() {
        let data = vec![0x42; 1000];
        let compressed = lz4_compress(&data);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_lz4_ham_radio_text() {
        let data = b"CQ CQ CQ DE VK2ABC VK2ABC K";
        let compressed = lz4_compress(data);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz4_binary_data() {
        let data: Vec<u8> = (0..=255).collect();
        let compressed = lz4_compress(&data);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lz4_truncated_data() {
        let data = b"Hello world test data";
        let compressed = lz4_compress(data);
        if compressed.len() > 4 {
            let truncated = &compressed[..compressed.len() / 2];
            let result = lz4_decompress(truncated);
            assert!(result.is_err());
        }
    }
}
