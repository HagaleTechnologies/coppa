//! Frozen compression conformance vectors (COP-92, SPEC.md §15.6).
//!
//! Locks the exact wire bytes of the payload compression envelope — the
//! fixed Huffman table, the padding-count convention, the `~` escape (and
//! that a literal `~` takes it too), the LZ4 4-byte LE size prefix, and the
//! `0xFE` marker — so a future refactor cannot silently change the wire
//! format SPEC.md §15 freezes. These are documentation vectors first and
//! tests second: the hex strings below are the ones printed in SPEC.md
//! §15.6, and both must be regenerated *together*, deliberately, if either
//! ever changes (see §15.6's note on `lz4_flex` upgrades).
//!
//! Expected outputs were reproduced against the public
//! `coppa_protocol::compression` API at commit 23e867c. The envelope test
//! mirrors `CoppaCore::compress_payload`'s transform order (SPEC.md §15.2)
//! using the protocol-crate primitives: Huffman encode, LZ4 compress,
//! prepend the `0xFE` marker.

use coppa_protocol::compression::{lz4_compress, HuffmanCodec};

/// SPEC.md §15.6's `COMPRESSION_MARKER` value (engine-side constant;
/// duplicated here because the envelope is assembled from this crate's
/// primitives).
const COMPRESSION_MARKER: u8 = 0xFE;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn assert_huffman_vector(input: &[u8], expected_hex: &str) {
    let codec = HuffmanCodec::new();
    let encoded = codec.encode(input);
    assert_eq!(
        hex(&encoded),
        expected_hex,
        "Huffman vector for {:?} drifted from SPEC.md §15.6",
        String::from_utf8_lossy(input)
    );
    // A frozen vector that no longer round-trips would be a bug, not a
    // documentation drift: the lossy `decode` is the shipped decoder (§15.3).
    assert_eq!(codec.decode(&encoded), input);
}

/// The four Huffman-layer vectors of SPEC.md §15.6: empty input, plain ham
/// text (padding path), the escape path via a literal `~`, and the escape
/// path via a byte not in the table at all.
#[test]
fn huffman_vectors_match_spec_15_6() {
    assert_huffman_vector(b"", "00");
    assert_huffman_vector(b"CQ CQ CQ DE W1AW", "01b3496692cd2568bf63de");
    assert_huffman_vector(b"A~B", "057ff7ec80");
    assert_huffman_vector(&[0x7F], "00ff7f");
}

/// The full envelope vector of SPEC.md §15.6: marker + LZ4(4-byte LE size
/// prefix + block) of the Huffman stream, for `"CQ CQ CQ DE W1AW K"` — the
/// transform order a conforming transmitter MUST apply (§15.2) and the bytes
/// a conforming receiver MUST invert.
#[test]
fn envelope_vector_matches_spec_15_6() {
    let codec = HuffmanCodec::new();
    let huffman = codec.encode(b"CQ CQ CQ DE W1AW K");
    let mut envelope = Vec::with_capacity(1 + 4 + huffman.len() * 2);
    envelope.push(COMPRESSION_MARKER);
    envelope.extend_from_slice(&lz4_compress(&huffman));
    assert_eq!(
        hex(&envelope),
        "fe0c000000c000b3496692cd2568bf63de66",
        "envelope vector for \"CQ CQ CQ DE W1AW K\" drifted from SPEC.md §15.6"
    );

    // Inverting per §15.2 must recover the plaintext: LZ4 decompress, then
    // Huffman decode.
    let lz4 = &envelope[1..];
    let recovered_huffman =
        coppa_protocol::compression::lz4_decompress(lz4).expect("frozen envelope must decompress");
    assert_eq!(codec.decode(&recovered_huffman), b"CQ CQ CQ DE W1AW K");
}
