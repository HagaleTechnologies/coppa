//! COP-41: split one host `DataReceived` event into chunks that each fit a
//! speed level's one-codeword payload budget after framing and compression.
//!
//! One TCP read (<= 4096 B) is not one over-the-air frame: every TX path
//! but the connected-session link encodes with `codewords: 1`, and
//! `CoppaTransceiver::transmit` rejects encoded payloads above the level
//! budget (56-198 bytes per codeword across levels 1-9). Every TX
//! path in `event_loop.rs` runs its host bytes through [`chunk_to_fit`] so an
//! ordinary Pat/B2F chunk becomes several properly-sized frames instead of
//! one that silently fails to encode (or squeezes through as a single
//! oversized frame). The session link with a negotiated multi-codeword cap
//! (COP-69) cuts against the cap's full budget instead, through
//! [`next_growing_chunk_len`]: the chunk may grow past the raw target while
//! its compressed frame still fits, up to 4096 raw bytes, and the link then
//! sends the fewest codewords that hold the compressed frame (COP-110).
//!
//! Sizing measures rather than predicts: post-compression size is
//! content-dependent by ~2x (102 random bytes -> 182 encoded; 102 cycling
//! unknown-high bytes -> 146), so no static chunk formula is correct for both
//! flavors. Callers pass a `fits` closure that frames a candidate chunk
//! exactly as their path will (MAC PDU / TransportPdu / raw) and measures it
//! via `CoppaCore::encoded_payload_len`.

/// Split `data` into chunks that each satisfy `fits` when framed with
/// `header_len` overhead against `budget`. Optimistic target first
/// (`budget - header_len - 1`), halved while the framed chunk doesn't fit,
/// then bisected between the last rejected and the accepted length so each
/// chunk is the largest prefix `fits` accepts (COP-38, [`next_chunk_len`]).
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
    let mut rest = data;
    while !rest.is_empty() {
        let take = next_chunk_len(rest, header_len, budget, &fits);
        chunks.push(rest[..take].to_vec());
        rest = &rest[take..];
    }
    chunks
}

/// Length of the first chunk [`chunk_to_fit`] would cut from `rest` (`rest`
/// non-empty): the optimistic target, halved while `fits` rejects it, then
/// bisected between the last rejected and the accepted length so the chunk is
/// the largest prefix `fits` accepts (for a monotone `fits`; COP-38). Never a
/// length `fits` rejected, except the 1-byte fallback. The COP-38 session
/// drain uses it to cut one chunk from its backlog without chunking the rest.
pub fn next_chunk_len(
    rest: &[u8],
    header_len: usize,
    budget: usize,
    fits: impl Fn(&[u8]) -> bool,
) -> usize {
    // Optimistic target: leave room for `header_len` framing bytes plus one
    // byte of margin, never less than 1 (a 1-byte chunk terminates the
    // halving loop; see `chunk_to_fit`'s doc).
    let optimistic = budget.saturating_sub(header_len + 1).max(1);
    let mut target = optimistic.min(rest.len());
    let mut rejected = None;
    while target > 1 && !fits(&rest[..target]) {
        rejected = Some(target);
        target /= 2;
    }
    if let Some(mut hi) = rejected {
        // Halving stopped at `target`; one rejection can waste nearly half a
        // frame, so search (target, hi) for the largest accepted prefix. A
        // 1-byte `target` that `fits` rejects is the fallback: keep it.
        let mut lo = target;
        if lo > 1 || fits(&rest[..lo]) {
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if fits(&rest[..mid]) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            target = lo;
        }
    }
    target.max(1)
}

/// COP-110: like [`next_chunk_len`], but a chunk that compresses well may
/// grow past the optimistic target. `rest` is first limited to `max_len`
/// bytes. When [`next_chunk_len`] returns its unshrunk optimistic target and
/// input remains, the length doubles while `fits` accepts it, then bisects
/// between the last accepted and the first rejected length. The result is
/// always a length `fits` accepted, or [`next_chunk_len`]'s own answer
/// (including its 1-byte fallback). The session link uses it to fill a
/// multi-codeword frame with compressible data, where the raw-byte target
/// leaves most of the frame as padding.
pub fn next_growing_chunk_len(
    rest: &[u8],
    header_len: usize,
    budget: usize,
    max_len: usize,
    fits: impl Fn(&[u8]) -> bool,
) -> usize {
    let rest = &rest[..rest.len().min(max_len.max(1))];
    let first = next_chunk_len(rest, header_len, budget, &fits);
    let optimistic = budget.saturating_sub(header_len + 1).max(1);
    if first < optimistic || first >= rest.len() {
        return first;
    }
    // The optimistic target fitted unshrunk: double while the frame still
    // fits, then search (lo, hi) for the largest accepted prefix.
    let mut lo = first;
    let mut hi = loop {
        let next = (lo * 2).min(rest.len());
        if !fits(&rest[..next]) {
            break next;
        }
        lo = next;
        if lo == rest.len() {
            return lo;
        }
    };
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(&rest[..mid]) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
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

    #[test]
    fn a_rejected_target_is_refined_to_the_largest_fitting_prefix() {
        // budget 100, header 0 -> optimistic 99 is rejected; halving alone stops
        // at 49 (accepted). Bisection between 49 and 99 finds 70, the largest
        // prefix `fits` accepts. Halving only gave [49, 49, 49, 49, 49, 49, 6].
        let data = vec![4u8; 300];
        let lens: Vec<usize> = chunk_to_fit(&data, 0, 100, fits_up_to(70))
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(lens, vec![70, 70, 70, 70, 20]);
    }

    #[test]
    fn next_chunk_len_is_the_first_chunk_of_chunk_to_fit() {
        let data: Vec<u8> = (0..300).map(|i| i as u8).collect();
        for max in [1usize, 10, 49, 50, 70, 99, 200] {
            let first = chunk_to_fit(&data, 0, 100, fits_up_to(max))[0].len();
            assert_eq!(
                next_chunk_len(&data, 0, 100, fits_up_to(max)),
                first,
                "max={max}"
            );
        }
    }

    #[test]
    fn a_non_monotone_fits_still_only_yields_accepted_chunks() {
        // Compression can make `fits` non-monotone in prefix length. Bisection
        // may then miss the true maximum (here 64) but must never return a
        // length `fits` rejected.
        let fits = |c: &[u8]| c.len() <= 30 || (60..=64).contains(&c.len());
        for chunk in chunk_to_fit(&[0u8; 500], 0, 100, fits) {
            assert!(fits(&chunk), "{} bytes", chunk.len());
        }
    }

    #[test]
    fn a_growing_chunk_stops_at_max_len() {
        // `fits` accepts anything: the chunk grows past the optimistic target
        // (99) but never past `max_len`.
        assert_eq!(
            next_growing_chunk_len(&[0u8; 5000], 0, 100, 4096, |_| true),
            4096
        );
    }

    #[test]
    fn a_growing_chunk_bisects_to_the_largest_fitting_prefix() {
        // Doubling from 99 reaches 792 (accepted) and 1584 (rejected); the
        // bisection between them finds 1500.
        let data = vec![6u8; 5000];
        assert_eq!(
            next_growing_chunk_len(&data, 0, 100, 4096, fits_up_to(1500)),
            1500
        );
    }

    #[test]
    fn a_shrunk_chunk_does_not_grow() {
        // The optimistic target was rejected, so growth never starts: the
        // answer is `next_chunk_len`'s.
        let data = vec![8u8; 300];
        assert_eq!(
            next_growing_chunk_len(&data, 0, 100, 4096, fits_up_to(10)),
            next_chunk_len(&data, 0, 100, fits_up_to(10))
        );
    }

    #[test]
    fn a_growing_chunk_takes_all_of_a_short_input() {
        assert_eq!(
            next_growing_chunk_len(&[2u8; 300], 0, 100, 4096, |_| true),
            300
        );
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
