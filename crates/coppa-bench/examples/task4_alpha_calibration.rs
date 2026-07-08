//! Phase 2 Task 4 Step 3: normalized min-sum alpha calibration sweep for the
//! layered NR BG2 decoder.
//!
//! Sweeps the decoder's `scale` (alpha) parameter at a fixed representative
//! operating point (level 2, BPSK rate 1/2, k_used=972 -- the primary
//! acceptance-gate level) across a band of SNRs around its FER<=10%
//! threshold, isolated at the FEC layer (bypasses OFDM entirely, same
//! methodology as `task4_bg2_ldpc_gate`/`task3_fec_isolated_gate`). Records
//! FER and average early-exit iteration count per alpha so the choice in
//! `NrBg2Decoder`'s `NR_DEFAULT_SCALE` constant is a measured pick, not a
//! guess.
//!
//! Run: `cargo run -p coppa-bench --release --example task4_alpha_calibration`

use coppa_protocol::fec::ldpc::decoder::NrBg2Decoder;
use coppa_protocol::fec::ldpc::encoder::NrBg2Encoder;
use coppa_protocol::fec::ldpc::rate_match::{rate_dematch, rate_match};
use coppa_protocol::fec::ldpc::{nr_bg2, pin_known_pad};
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{k_used_for_level, speed_level_components};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const CODED_LEN: usize = 1944;
const LEVEL: u8 = 2;
const TRIALS: usize = 300;
// A band of SNRs around level 2's isolated-layer FER<=10% neighborhood
// (found empirically to sit a few dB below the pre-Task-4 codec's ~2.0 dB
// threshold, per the Task 4 report).
const SNR_POINTS: [f32; 5] = [-4.0, -3.0, -2.0, -1.0, 0.0];

fn ground_truth_info(payload_bits: usize, total_len: usize, seed: u64) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut v: Vec<u8> = (0..payload_bits)
        .map(|_| rng.random_range(0..2u8))
        .collect();
    v.resize(total_len, 0u8);
    v
}

fn awgn_symbols(
    symbols: &[num_complex::Complex32],
    snr_db: f32,
    seed: u64,
) -> Vec<num_complex::Complex32> {
    let signal_power: f32 =
        symbols.iter().map(|s| s.norm_sqr()).sum::<f32>() / symbols.len() as f32;
    let noise_power = signal_power / 10f32.powf(snr_db / 10.0);
    let noise_std = (noise_power / 2.0).sqrt();
    let mut rng = StdRng::seed_from_u64(seed);
    symbols
        .iter()
        .map(|&s| {
            let u1: f32 = rng.random::<f32>().max(1e-10);
            let u2: f32 = rng.random();
            let n_re =
                noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            let u3: f32 = rng.random::<f32>().max(1e-10);
            let u4: f32 = rng.random();
            let n_im =
                noise_std * (-2.0 * u3.ln()).sqrt() * (2.0 * std::f32::consts::PI * u4).cos();
            s + num_complex::Complex32::new(n_re, n_im)
        })
        .collect()
}

/// Run one trial with an explicit decoder (custom alpha), returning
/// (converged_and_correct, iterations_used).
fn trial(
    enc: &NrBg2Encoder,
    dec: &NrBg2Decoder,
    payload_bits: usize,
    snr_db: f32,
    seed: u64,
) -> (bool, usize) {
    let k_used = k_used_for_level(LEVEL).unwrap();
    let (mapper, _) = speed_level_components(LEVEL).unwrap();

    let mut info = ground_truth_info(payload_bits, nr_bg2::KB * nr_bg2::ZC, seed);
    scramble(&mut info);
    let mother = enc.encode_mother(&info);
    let matched = rate_match(&mother, k_used, CODED_LEN, 0);

    let symbols = mapper.map_bits(&matched);
    let noisy = awgn_symbols(&symbols, snr_db, seed ^ 0xA11CE);
    let nv = 10f32.powf(-snr_db / 10.0);
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for &s in &noisy {
        llrs.extend(mapper.demap_soft(s, nv));
    }
    llrs.truncate(CODED_LEN);

    let mut dematched = rate_dematch(
        &llrs,
        k_used,
        CODED_LEN,
        0,
        (nr_bg2::BASE_COLS - nr_bg2::PUNCTURED_INFO_COLS) * nr_bg2::ZC,
    );
    pin_known_pad(&mut dematched, payload_bits, k_used, 64.0);

    // Prepend the always-punctured leading columns (decoder operates on the
    // full graph) -- mirrors `NrLdpc::decode_soft_stats` internally.
    let punctured_len = nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC;
    let mut full_llrs = Vec::with_capacity(nr_bg2::BASE_COLS * nr_bg2::ZC);
    full_llrs.resize(punctured_len, 0.0f32);
    full_llrs.extend_from_slice(&dematched);

    let (posterior, iterations, converged) = dec.decode(&full_llrs);
    if !converged {
        return (false, iterations);
    }
    let mut decoded: Vec<u8> = posterior[..nr_bg2::KB * nr_bg2::ZC]
        .iter()
        .map(|&l| if l >= 0.0 { 0 } else { 1 })
        .collect();
    scramble(&mut decoded);
    let truth = ground_truth_info(payload_bits, nr_bg2::KB * nr_bg2::ZC, seed);
    (decoded[..payload_bits] == truth[..payload_bits], iterations)
}

fn main() {
    let enc = NrBg2Encoder::new();
    let k_used = k_used_for_level(LEVEL).unwrap();
    let payload_bits = k_used - 8; // near-full capacity, same convention as task4_bg2_ldpc_gate

    let alphas = [0.60f32, 0.65, 0.70, 0.75, 0.80, 0.85, 0.90, 1.00];
    let max_iterations = 30;

    println!("## Task 4 Step 3: normalized min-sum alpha calibration (layered decoder, level 2)\n");
    println!("| alpha | SNR (dB) | FER | avg iterations (converged only) |");
    println!("|---|---|---|---|");

    let mut best_alpha = alphas[0];
    let mut best_score = f64::MAX; // lower avg FER across the SNR band = better

    for &alpha in &alphas {
        let dec = NrBg2Decoder::with_params(alpha, max_iterations);
        let mut band_fer_sum = 0.0;
        for &snr in &SNR_POINTS {
            let mut fails = 0usize;
            let mut iter_sum = 0u64;
            let mut converged_count = 0u64;
            for t in 0..TRIALS {
                let seed = 0xA15A_0000u64
                    .wrapping_add(((alpha * 100.0) as u64) << 40)
                    .wrapping_add(((snr * 10.0) as i64 as u64) << 20)
                    .wrapping_add(t as u64);
                let (ok, iters) = trial(&enc, &dec, payload_bits, snr, seed);
                if !ok {
                    fails += 1;
                } else {
                    iter_sum += iters as u64;
                    converged_count += 1;
                }
            }
            let fer = fails as f64 / TRIALS as f64;
            let avg_iters = if converged_count > 0 {
                iter_sum as f64 / converged_count as f64
            } else {
                f64::NAN
            };
            println!("| {alpha:.2} | {snr:.1} | {fer:.3} | {avg_iters:.2} |");
            band_fer_sum += fer;
        }
        let band_avg_fer = band_fer_sum / SNR_POINTS.len() as f64;
        if band_avg_fer < best_score {
            best_score = band_avg_fer;
            best_alpha = alpha;
        }
    }

    println!("\nBest alpha (lowest average FER across the SNR band): {best_alpha:.2} (avg FER={best_score:.4})");
}
