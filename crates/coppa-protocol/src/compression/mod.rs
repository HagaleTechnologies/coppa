//! Data compression for Coppa protocol payloads.

pub mod huffman;
pub mod lz4;

pub use huffman::{HuffmanCodec, HAM_RADIO_TABLE};
pub use lz4::{lz4_compress, lz4_decompress};
