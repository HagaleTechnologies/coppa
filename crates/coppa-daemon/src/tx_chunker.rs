//! COP-41: split one host `DataReceived` event into chunks that each fit a
//! speed level's one-codeword payload budget after framing and compression.
//!
//! One TCP read (<= 4096 B) is not one over-the-air frame: the engine encodes
//! with `codewords: 1`, and `CoppaTransceiver::transmit` rejects encoded
//! payloads above the level budget (56-198 bytes across levels 1-9). Every TX
//! path in `event_loop.rs` runs its host bytes through [`chunk_to_fit`] so an
//! ordinary Pat/B2F chunk becomes several properly-sized frames instead of
//! one that silently fails to encode (or squeezes through as a single
//! oversized frame).
//!
//! Sizing measures rather than predicts: post-compression size is
//! content-dependent by ~2x (102 random bytes -> 182 encoded; 102 cycling
//! unknown-high bytes -> 146), so no static chunk formula is correct for both
//! flavors. Callers pass a `fits` closure that frames a candidate chunk
//! exactly as their path will (MAC PDU / TransportPdu / raw) and measures it
//! via `CoppaCore::encoded_payload_len`.

/// Split `data` into chunks that each satisfy `fits` when framed with
/// `header_len` overhead against `budget`. Optimistic target first
/// (`budget - header_len - 1`), halved while the framed chunk doesn't fit.
/// A one-byte chunk that still doesn't fit is returned as-is (callers'
/// encode-failure warn covers it); unreachable for the current ladder, where
/// a 1-host-byte PDU fits even the ladder-minimum budget. Empty input yields
/// no chunks (an empty host write produces no frame).
pub fn chunk_to_fit(
    data: &[u8],
    header_len: usize,
    budget: usize,
    fits: impl Fn(&[u8]) -> bool,
) -> Vec<Vec<u8>> {
    let mut chunks = Vec::new();
    if data.is_empty() {
        return chunks;
    }
    // Optimistic target: leave room for `header_len` framing bytes plus one
    // byte of margin, never less than 1 (a 1-byte chunk terminates the
    // halving loop; see the doc above).
    let optimistic = budget.saturating_sub(header_len + 1).max(1);
    let mut rest = data;
    while !rest.is_empty() {
        let mut target = optimistic.min(rest.len());
        while target > 1 && !fits(&rest[..target]) {
            target /= 2;
        }
        let take = target.max(1);
        chunks.push(rest[..take].to_vec());
        rest = &rest[take..];
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fits` accepting any chunk of at most `max` bytes (after framing):
    /// models the real `fits` closures, which measure a framed PDU against
    /// a budget.
    fn fits_up_to(max: usize) -> impl Fn(&[u8]) -> bool {
        move |c: &[u8]| c.len() <= max
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_to_fit(&[], 0, 100, fits_up_to(100)).is_empty());
    }

    #[test]
    fn under_budget_input_is_one_chunk() {
        let chunks = chunk_to_fit(&[7u8; 40], 0, 100, fits_up_to(100));
        assert_eq!(chunks, vec![vec![7u8; 40]]);
    }

    #[test]
    fn over_budget_input_splits_at_the_optimistic_target() {
        // budget 100, header_len 0 -> optimistic target 99: exact 99/99/99/3.
        let data: Vec<u8> = (0..300).map(|i| i as u8).collect();
        let chunks = chunk_to_fit(&data, 0, 100, fits_up_to(100));
        let lens: Vec<usize> = chunks.iter().map(Vec::len).collect();
        assert_eq!(lens, vec![99, 99, 99, 3]);
    }

    #[test]
    fn header_len_shrinks_the_optimistic_target() {
        // budget 117, header_len 14 -> optimistic target 102, one 102-byte
        // chunk and the remainder (the session path's shape at level 2).
        let data = vec![5u8; 150];
        let chunks = chunk_to_fit(&data, 14, 117, fits_up_to(117));
        let lens: Vec<usize> = chunks.iter().map(Vec::len).collect();
        assert_eq!(lens, vec![102, 48]);
    }

    #[test]
    fn rejecting_fits_halves_down_to_an_acceptable_chunk() {
        // The optimistic target (99) and every halving above 10 are rejected;
        // halving must reach a chunk `fits` accepts (6 <= 10).
        let data = vec![9u8; 300];
        let chunks = chunk_to_fit(&data, 0, 100, fits_up_to(10));
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.len() <= 10, "chunk of {} bytes must fit", chunk.len());
        }
    }

    #[test]
    fn one_byte_that_never_fits_is_returned_as_is() {
        // Termination contract: even a `fits` that rejects everything gets
        // 1-byte chunks back, never an infinite loop or a silent drop.
        let chunks = chunk_to_fit(&[1, 2, 3], 0, 100, |_| false);
        assert_eq!(chunks, vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn input_already_at_target_is_unchanged() {
        let data = vec![3u8; 99];
        let chunks = chunk_to_fit(&data, 0, 100, fits_up_to(100));
        assert_eq!(chunks, vec![data]);
    }

    /// Property-style check across sizes and header lengths: concatenating
    /// the chunks reproduces `data` exactly, and every chunk satisfies
    /// `fits` (the invariant every TX path relies on).
    #[test]
    fn chunks_partition_the_input_and_each_fit() {
        let mut seed: u32 = 0x41_41_41_41;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            seed
        };
        for len in [1usize, 2, 7, 48, 49, 117, 200, 1024, 4096] {
            let data: Vec<u8> = (0..len).map(|_| (next() >> 16) as u8).collect();
            for (header_len, budget) in [(0usize, 117usize), (14, 117), (7, 56)] {
                let max = budget.saturating_sub(header_len + 1).max(1);
                let chunks = chunk_to_fit(&data, header_len, budget, fits_up_to(max));
                let rejoined: Vec<u8> = chunks.concat();
                assert_eq!(rejoined, data, "len={len} header={header_len}");
                assert!(
                    chunks.iter().all(|c| c.len() <= max),
                    "len={len} header={header_len}: every chunk must fit"
                );
            }
        }
    }
}
