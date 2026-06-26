//! Deep cross-frame block interleaver: distributes each of `N` LDPC codewords' coded bits
//! evenly across all `N` frames, so a single faded frame damages only 1/N of every codeword.
//! Generic over the element type so the SAME permutation serves TX bits (`u8`) and RX LLRs
//! (`f32`). Nested OUTSIDE the per-frame `BlockInterleaver` (this spreads across frames; the
//! block interleaver spreads within a frame).

/// A bijective permutation over `N * C` elements: `N` codewords (codeword `k` at
/// `[k*C..(k+1)*C]`) <-> `N` frame-blocks (frame `f` at `[f*C..(f+1)*C]`).
pub struct CrossFrameInterleaver {
    /// `forward[input_index] = output_index`. Input is codeword-major, output is frame-major.
    forward: Vec<usize>,
}

impl CrossFrameInterleaver {
    /// `num_frames` = N (number of codewords == number of frames).
    /// `coded_bits_per_codeword` = C (e.g. 1944). Each frame-block holds C elements.
    pub fn new(num_frames: usize, coded_bits_per_codeword: usize) -> Self {
        assert!(num_frames > 0, "num_frames must be > 0");
        assert!(
            coded_bits_per_codeword > 0,
            "coded_bits_per_codeword must be > 0"
        );
        let n = num_frames;
        let c = coded_bits_per_codeword;
        let cn = c / n; // even stripe size
        let even = cn * n; // == c when N | C; remainder = c - even
        let mut forward = vec![0usize; n * c];
        for k in 0..n {
            for i in 0..c {
                let input = k * c + i;
                let output = if i < even {
                    // Even region: stripe g of codeword k goes to frame (k+g) mod N,
                    // placed in that frame's block at intra-offset k*cn + b.
                    let g = i / cn;
                    let b = i % cn;
                    let f = (k + g) % n;
                    f * c + (k * cn + b)
                } else {
                    // Remainder region (only when N does not divide C): the trailing
                    // `c - even` bits of each codeword are assigned round-robin across
                    // frames into the tail of each frame-block. Bijective: for a fixed
                    // frame f, exactly one (k, r) pair lands in each tail slot r.
                    let r = i - even; // 0..(c - even)
                    let f = (k + r) % n;
                    f * c + even + r
                };
                forward[input] = output;
            }
        }
        Self { forward }
    }

    /// Map `N` codewords (codeword-major) to `N` frame-blocks (frame-major).
    pub fn interleave<T: Copy>(&self, codewords: &[T]) -> Vec<T> {
        assert_eq!(codewords.len(), self.forward.len(), "length must equal N*C");
        let mut out = vec![codewords[0]; codewords.len()];
        for (input, &outpos) in self.forward.iter().enumerate() {
            out[outpos] = codewords[input];
        }
        out
    }

    /// Inverse of `interleave`: map `N` frame-blocks back to `N` codewords.
    pub fn deinterleave<T: Copy>(&self, frames: &[T]) -> Vec<T> {
        assert_eq!(frames.len(), self.forward.len(), "length must equal N*C");
        let mut out = vec![frames[0]; frames.len()];
        for (input, &outpos) in self.forward.iter().enumerate() {
            out[input] = frames[outpos];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_identity_exact_division() {
        // N=8, C=1944 (the SP2 level-2 case; 1944/8 = 243 exact).
        let il = CrossFrameInterleaver::new(8, 1944);
        let input: Vec<u32> = (0..8 * 1944).map(|x| x as u32).collect();
        let interleaved = il.interleave(&input);
        let back = il.deinterleave(&interleaved);
        assert_eq!(
            back, input,
            "deinterleave(interleave(x)) must be identity (exact)"
        );
    }

    #[test]
    fn roundtrip_identity_with_remainder() {
        // N=3, C=8 (not divisible: 8/3 = 2 stripe, remainder 2) — exercises the remainder
        // branch and proves the permutation is still bijective.
        let il = CrossFrameInterleaver::new(3, 8);
        let input: Vec<f32> = (0..3 * 8).map(|x| x as f32 * 0.5).collect();
        let interleaved = il.interleave(&input);
        let back = il.deinterleave(&interleaved);
        assert_eq!(
            back, input,
            "deinterleave(interleave(x)) must be identity (remainder)"
        );
    }

    #[test]
    fn each_frame_holds_a_stripe_of_every_codeword() {
        // Tag every bit of codeword k with value k. After interleave, each frame-block must
        // contain exactly `Cn` (=C/N) bits from each codeword, and every codeword must appear
        // in every frame — the diversity guarantee.
        let n = 8usize;
        let c = 1944usize;
        let cn = c / n; // 243
        let il = CrossFrameInterleaver::new(n, c);
        let input: Vec<u8> = (0..n)
            .flat_map(|k| std::iter::repeat_n(k as u8, c))
            .collect();
        let frames = il.interleave(&input);
        for f in 0..n {
            let block = &frames[f * c..(f + 1) * c];
            let mut counts = vec![0usize; n];
            for &v in block {
                counts[v as usize] += 1;
            }
            for (k, &count) in counts.iter().enumerate() {
                assert_eq!(
                    count, cn,
                    "frame {f} must hold exactly {cn} bits of codeword {k}, got {count}"
                );
            }
        }
    }
}
