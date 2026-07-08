//! Task 4 diagnostic: where does `NrLdpc::decode_soft`'s per-frame CPU cost
//! actually go? Breaks down: (a) `NrLdpc::decode_soft_stats`'s wrapper
//! overhead (Vec allocations for the punctured-prefix prepend + posterior/
//! info collection) vs (b) `NrBg2Decoder::decode`'s own internal allocations
//! (`total_llr`, `check_to_var`) vs (c) the per-iteration layered-update
//! compute itself (measured by forcing 1 vs 5 vs 30 iterations on the same
//! non-convergent input and looking at the marginal per-iteration cost).
use coppa_protocol::fec::ldpc::decoder::NrBg2Decoder;
use coppa_protocol::fec::ldpc::nr_bg2;
use coppa_protocol::fec::ldpc::NrLdpc;
use std::time::Instant;

const N: usize = 5000;

fn main() {
    let ldpc = NrLdpc::new();
    let full_len = nr_bg2::BASE_COLS * nr_bg2::ZC;
    let mother_len = NrLdpc::MOTHER_LEN;

    // A pathological (never-converges) full-graph LLR pattern, so every
    // iteration count runs to its cap -- isolates iteration-count-dependent
    // cost from early-exit variance.
    let full_llrs: Vec<f32> = (0..full_len)
        .map(|i| if i % 3 == 0 { 3.0 } else { -3.0 })
        .collect();
    let mother_llrs: Vec<f32> = full_llrs[nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC..].to_vec();
    assert_eq!(mother_llrs.len(), mother_len);

    // (a) Full NrLdpc::decode_soft_stats wrapper (includes prepend + info
    // collection) vs (b) NrBg2Decoder::decode called directly on a
    // pre-built full_llrs (no prepend, no info collection).
    let t0 = Instant::now();
    for _ in 0..N {
        std::hint::black_box(ldpc.decode_soft_stats(std::hint::black_box(&mother_llrs)));
    }
    let wrapper_us = t0.elapsed().as_secs_f64() * 1e6 / N as f64;

    let dec_default = NrBg2Decoder::new();
    let t1 = Instant::now();
    for _ in 0..N {
        std::hint::black_box(dec_default.decode(std::hint::black_box(&full_llrs)));
    }
    let raw_decode_us = t1.elapsed().as_secs_f64() * 1e6 / N as f64;

    println!(
        "NrLdpc::decode_soft_stats (wrapper, incl. prepend+info-collect): {wrapper_us:.2} us/call"
    );
    println!("NrBg2Decoder::decode (raw, no wrapper):                          {raw_decode_us:.2} us/call");
    println!(
        "wrapper overhead: {:.2} us/call ({:.1}%% of total)",
        wrapper_us - raw_decode_us,
        100.0 * (wrapper_us - raw_decode_us) / wrapper_us
    );
    println!();

    // (c) Marginal per-iteration cost: force max_iterations = 1, 5, 30 on
    // the SAME pathological (never-converging) input, so total cost scales
    // with iterations run, and the y-intercept (iterations=0 extrapolated)
    // reveals the fixed per-call overhead (allocations) vs the slope
    // (per-iteration compute).
    println!("Per-call cost vs forced max_iterations (pathological, never converges):");
    println!("| max_iterations | us/call | us/iteration (marginal from iter=1) |");
    println!("|---|---|---|");
    let mut base_1iter = 0.0;
    for &iters in &[1usize, 2, 5, 10, 20, 30] {
        let dec = NrBg2Decoder::with_params(0.8, iters);
        let t = Instant::now();
        for _ in 0..N {
            std::hint::black_box(dec.decode(std::hint::black_box(&full_llrs)));
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / N as f64;
        if iters == 1 {
            base_1iter = us;
        }
        let marginal = if iters > 1 {
            (us - base_1iter) / (iters - 1) as f64
        } else {
            f64::NAN
        };
        println!("| {iters} | {us:.2} | {marginal:.2} |");
    }
}
