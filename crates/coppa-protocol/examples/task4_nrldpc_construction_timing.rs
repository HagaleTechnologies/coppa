//! Throwaway Task 4 diagnostic: how expensive is `NrLdpc::new()` (the
//! lifted-graph + core-parity GF(2) inversion construction), and how does it
//! compare to constructing all nine pre-Task-4 per-rate `LdpcCodec`s? This
//! surfaced because `tests/proptest_roundtrip.rs` (pre-existing, builds a
//! fresh `CoppaCore` -- which builds two independent `CoppaTransceiver`s --
//! per proptest case) got noticeably slower in an unoptimized debug build.
//! Not part of the bench gate itself (that's `decode CPU/frame`, a steady-
//! state per-frame cost, measured separately in `coppa-bench`); this is
//! purely about one-time construction cost.
use coppa_protocol::fec::ldpc::codes::CodeRate;
use coppa_protocol::fec::ldpc::{LdpcCodec, NrLdpc};
use std::time::Instant;

fn main() {
    const N: usize = 50;

    let t0 = Instant::now();
    for _ in 0..N {
        std::hint::black_box(NrLdpc::new());
    }
    let new_us = t0.elapsed().as_secs_f64() * 1e6 / N as f64;

    let old_rates = [
        CodeRate::Rate1_4,
        CodeRate::Rate1_2,
        CodeRate::Rate1_2,
        CodeRate::Rate3_4,
        CodeRate::Rate2_3,
        CodeRate::Rate1_2,
        CodeRate::Rate3_4,
        CodeRate::Rate2_3,
        CodeRate::Rate7_8,
    ];
    let t1 = Instant::now();
    for _ in 0..N {
        for &rate in &old_rates {
            std::hint::black_box(LdpcCodec::new(rate));
        }
    }
    let old_us = t1.elapsed().as_secs_f64() * 1e6 / N as f64;

    println!("NrLdpc::new() (one instance, all levels): {new_us:.1} us/call");
    println!("LdpcCodec::new() x9 (old, one per level):  {old_us:.1} us/call (all 9 combined)");
    println!("ratio (new / old-all-9): {:.2}x", new_us / old_us);
}
