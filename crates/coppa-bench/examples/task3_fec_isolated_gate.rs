//! Phase 2 Task 3 bench: known-pad LLR pinning's effect on the LDPC decode step,
//! *isolated* from OFDM sync/header recovery.
//!
//! `task3_short_payload_gate` (the full end-to-end `CoppaTransceiver::receive`
//! sweep) found the pre-Task-3 and post-Task-3 code produce byte-identical FER
//! curves for a 20-byte payload at level 2 -- confirmed not a measurement bug
//! (see the Task 3 report): every single frame failure across the tested SNR
//! range is a `ReceiveError::SyncFailed`/`HeaderCorrupt`, and *every* frame that
//! reaches the LDPC decode step converges and recovers the payload whether or
//! not the pad bits are pinned. In other words: for this profile/level/payload,
//! OFDM sync is the binding constraint over the whole SNR range where LDPC could
//! plausibly fail, so pinning's effect on the LDPC decode margin is invisible in
//! a full-system sweep.
//!
//! This bench isolates exactly the layer known-pad pinning acts on: BPSK-map the
//! coded bits directly (bypassing OFDM entirely), add AWGN at a known variance,
//! demap with the exact max-log scale, and decode with vs without pad-bit
//! pinning. This measures the real SNR margin the code-shortening effect buys,
//! uncorrupted by sync robustness.
use coppa_codec::bpsk::BpskMapper;
use coppa_codec::traits::ConstellationMapper;
use coppa_protocol::fec::ldpc::{CodeRate, LdpcCodec};
use coppa_protocol::fec::scrambler::{prbs_bits, scramble};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const PAYLOAD_BYTES: usize = 20;
const TRIALS: usize = 400;
const SEED: u64 = 0x00C0_FFEE;
const PIN: f32 = 64.0;
const LLR_CLIP: f32 = 20.0;

/// One trial: random 20-byte payload -> LDPC-encode (with scrambled zero pad) ->
/// BPSK-map -> AWGN -> exact-scale LLR demap -> (optionally pin pad bits) -> decode.
/// Returns whether the payload bytes were recovered exactly.
fn run_trial(codec: &LdpcCodec, snr_db: f32, seed: u64, pin: bool) -> bool {
    let info_bits = codec.code().info_bits();
    let payload_bits = PAYLOAD_BYTES * 8;

    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| rng.random::<u8>()).collect();

    let mut info: Vec<u8> = Vec::with_capacity(info_bits);
    for &byte in &payload {
        for shift in (0..8).rev() {
            info.push((byte >> shift) & 1);
        }
    }
    info.resize(info_bits, 0u8);
    scramble(&mut info); // matches CoppaTransceiver::transmit exactly

    let coded = codec.encode(&info);

    let mapper = BpskMapper;
    let clean: Vec<f32> = coded.iter().map(|&b| mapper.map(&[b]).re).collect();

    // Mean square of an all-+-1 BPSK signal is exactly 1.0, so `awgn_seeded`'s own
    // "signal_power / 10^(snr/10)" noise-power convention gives an exact,
    // analytically-known noise variance -- no separate estimation step needed.
    let noisy = coppa_channel::awgn_seeded(&clean, snr_db, seed ^ 0x5A5A_5A5A_5A5A_5A5Au64);
    let nv = 10f32.powf(-snr_db / 10.0);

    let mut llrs: Vec<f32> = noisy
        .iter()
        .map(|&re| (4.0 * re / nv).clamp(-LLR_CLIP, LLR_CLIP))
        .collect();

    if pin && payload_bits < info_bits {
        let pad_prbs = prbs_bits(info_bits);
        for (i, &prbs_bit) in pad_prbs
            .iter()
            .enumerate()
            .take(info_bits)
            .skip(payload_bits)
        {
            llrs[i] = if prbs_bit == 0 { PIN } else { -PIN };
        }
    }

    let (mut decoded, converged) = codec.decode_checked(&llrs);
    if !converged {
        return false;
    }
    scramble(&mut decoded);

    let mut out = Vec::with_capacity(PAYLOAD_BYTES);
    for chunk in decoded.chunks(8) {
        if chunk.len() == 8 && out.len() < PAYLOAD_BYTES {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            out.push(byte);
        }
    }
    out == payload
}

fn sweep(codec: &LdpcCodec, pin: bool, snr_points: &[f32]) -> Vec<(f32, usize, usize)> {
    let mut out = Vec::with_capacity(snr_points.len());
    for (si, &snr_db) in snr_points.iter().enumerate() {
        let mut successes = 0;
        for trial in 0..TRIALS {
            let seed = SEED
                .wrapping_add((si as u64) << 32)
                .wrapping_add(trial as u64);
            if run_trial(codec, snr_db, seed, pin) {
                successes += 1;
            }
        }
        out.push((snr_db, successes, TRIALS));
    }
    out
}

/// Lowest SNR at which FER <= 10% (simple point estimate, no CI -- this bench is
/// about locating the threshold shift, not publishing a calibrated FER curve).
fn fer10_threshold(points: &[(f32, usize, usize)]) -> Option<f32> {
    points
        .iter()
        .filter(|&&(_, s, n)| (n - s) as f64 / n as f64 <= 0.10)
        .map(|&(snr, _, _)| snr)
        .min_by(|a, b| a.total_cmp(b))
}

fn main() {
    let codec = LdpcCodec::new(CodeRate::Rate1_2); // level 2: BPSK 1/2, 972 info bits

    let mut snr_points = Vec::new();
    let mut s = -9.0f32;
    while s <= 3.0 + 1e-6 {
        snr_points.push(s);
        s += 0.5;
    }

    for (label, pin) in [("no-pin", false), ("pinned", true)] {
        eprintln!("Measuring FEC-isolated level-2/20-byte-payload sweep: {label}...");
        let points = sweep(&codec, pin, &snr_points);
        println!("## {label}");
        for &(snr, s, n) in &points {
            println!(
                "  {snr:>5.1} dB: {s}/{n} success (fer={:.4})",
                (n - s) as f64 / n as f64
            );
        }
        let t10 = fer10_threshold(&points);
        println!("SUMMARY label={label} fer10_threshold_db={:?}", t10);
    }
}
