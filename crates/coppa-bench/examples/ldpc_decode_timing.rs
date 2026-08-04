//! COP-6 NR BG2 decode timing harness.
//!
//! Do not measure while another Cargo process is running on this host. Part A
//! forces a non-convergent decode through each iteration cap and reports the
//! minimum, median, and spread of five repeats plus a least-squares cost fit.

use coppa_protocol::fec::ldpc::decoder::NrBg2Decoder;
use coppa_protocol::fec::ldpc::rate_match::{rate_dematch, rate_match};
use coppa_protocol::fec::ldpc::{pin_known_pad, CodeRate, LdpcCodec, NrLdpc};
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{
    k_used_for_level, max_payload_for_level, speed_level_components,
};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::hint::black_box;
use std::time::Instant;

const ALPHA: f32 = 0.75;
const CAPS: [usize; 7] = [1, 2, 3, 5, 10, 20, 30];
const REPEATS: usize = 5;
const CALLS: usize = 300;
const WARMUP: usize = 50;
const LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];
const TIMING_SNR: [(u8, f32); 9] = [
    (1, 3.0),
    (2, 5.0),
    (3, 8.0),
    (4, 10.5),
    (5, 13.5),
    (6, 14.5),
    (7, 18.0),
    (9, 20.5),
    (10, 25.0),
];
const CODED_LEN: usize = 1944;

fn input() -> Vec<f32> {
    (0..52 * 176)
        .map(|i| if i % 3 == 0 { 3.0 } else { -3.0 })
        .collect()
}

fn percentile(values: &[f64], index: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[index]
}

fn old_code_rate(level: u8) -> CodeRate {
    match level {
        1 => CodeRate::Rate1_4,
        2 | 3 | 6 => CodeRate::Rate1_2,
        4 | 7 => CodeRate::Rate3_4,
        5 | 9 => CodeRate::Rate2_3,
        10 => CodeRate::Rate7_8,
        _ => unreachable!(),
    }
}

fn noisy_llrs(level: u8, coded: &[u8], snr_db: f32, seed: u64) -> Vec<f32> {
    let (mapper, _) = speed_level_components(level).unwrap();
    let symbols = mapper.map_bits(coded);
    let noise_variance = 10f32.powf(-snr_db / 10.0);
    let noise_std = (noise_variance / 2.0).sqrt();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for symbol in symbols {
        let u1 = rng.random::<f32>().max(1e-10);
        let u2 = rng.random::<f32>();
        let u3 = rng.random::<f32>().max(1e-10);
        let u4 = rng.random::<f32>();
        let radius1 = noise_std * (-2.0 * u1.ln()).sqrt();
        let radius2 = noise_std * (-2.0 * u3.ln()).sqrt();
        let noisy = symbol
            + num_complex::Complex32::new(
                radius1 * (std::f32::consts::TAU * u2).cos(),
                radius2 * (std::f32::consts::TAU * u4).cos(),
            );
        llrs.extend(mapper.demap_soft(noisy, noise_variance));
    }
    llrs.truncate(CODED_LEN);
    llrs
}

fn checksum(bits: &[u8], converged: bool) -> u64 {
    bits.iter()
        .enumerate()
        .fold(converged as u64, |sum, (i, bit)| {
            sum.rotate_left(5) ^ (u64::from(*bit) + i as u64)
        })
}

fn time_level(level: u8, snr_db: f32, payload_bits: usize) -> (f64, f64, f64, u64, u64) {
    let old = LdpcCodec::new(old_code_rate(level));
    let new = NrLdpc::new();
    let k_used = k_used_for_level(level).unwrap();
    let seed = 0x51DE_0000 + u64::from(level) * 17 + payload_bits as u64;

    let mut old_info = vec![0u8; old.code().info_bits()];
    scramble(&mut old_info);
    let old_llrs = noisy_llrs(level, &old.encode(&old_info), snr_db, seed);

    let mut new_info = vec![0u8; NrLdpc::INFO_LEN];
    scramble(&mut new_info);
    let matched = rate_match(&new.encode(&new_info), k_used, CODED_LEN, 0);
    let channel_llrs = noisy_llrs(level, &matched, snr_db, seed);
    let mut new_llrs = rate_dematch(&channel_llrs, k_used, CODED_LEN, 0, NrLdpc::MOTHER_LEN);
    pin_known_pad(&mut new_llrs, payload_bits, k_used, 64.0);

    for _ in 0..WARMUP {
        black_box(old.decode_checked(black_box(&old_llrs)));
        black_box(new.decode_soft_stats(black_box(&new_llrs)));
    }
    let start = Instant::now();
    let mut old_sum = 0u64;
    for _ in 0..CALLS {
        let result = old.decode_checked(black_box(&old_llrs));
        assert!(result.1, "old decoder did not converge at level {level}");
        old_sum = old_sum.wrapping_add(checksum(&result.0, result.1));
        black_box(&result);
    }
    let old_us = start.elapsed().as_secs_f64() * 1e6 / CALLS as f64;
    let start = Instant::now();
    let mut new_sum = 0u64;
    let mut iterations = 0usize;
    for _ in 0..CALLS {
        let result = new.decode_soft_stats(black_box(&new_llrs));
        assert!(result.2, "new decoder did not converge at level {level}");
        iterations += result.3;
        new_sum = new_sum.wrapping_add(checksum(&result.1, result.2));
        black_box(&result);
    }
    let new_us = start.elapsed().as_secs_f64() * 1e6 / CALLS as f64;
    (
        old_us,
        new_us,
        iterations as f64 / CALLS as f64,
        old_sum,
        new_sum,
    )
}

fn main() {
    let llrs = input();
    let mut points = Vec::with_capacity(CAPS.len());

    println!("## Part A: forced-iteration cost");
    println!("| cap | min us/call | median us/call | spread us/call |");
    println!("|---:|---:|---:|---:|");
    for cap in CAPS {
        let decoder = NrBg2Decoder::with_params(ALPHA, cap);
        for _ in 0..WARMUP {
            let result = decoder.decode(black_box(&llrs));
            assert_eq!(result.1, cap);
            black_box(result);
        }
        let mut repeats = Vec::with_capacity(REPEATS);
        for _ in 0..REPEATS {
            let start = Instant::now();
            for _ in 0..CALLS {
                let result = decoder.decode(black_box(&llrs));
                assert_eq!(result.1, cap);
                black_box(result);
            }
            repeats.push(start.elapsed().as_secs_f64() * 1e6 / CALLS as f64);
        }
        let min = percentile(&repeats, 0);
        let median = percentile(&repeats, REPEATS / 2);
        let max = percentile(&repeats, REPEATS - 1);
        println!("| {cap} | {min:.2} | {median:.2} | {:.2} |", max - min);
        points.push((cap as f64, min));
    }

    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / count;
    let slope = points
        .iter()
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>()
        / points
            .iter()
            .map(|(x, _)| (x - mean_x).powi(2))
            .sum::<f64>();
    let intercept = mean_y - slope * mean_x;
    println!("\nfit: {slope:.2} us/iteration + {intercept:.2} us/call");

    println!("\n## Part B: frozen-SNR per-level decode cost");
    println!("| input | level | SNR | old us | new us | ratio | avg iters | old checksum | new checksum | verdict |");
    println!("|:---|---:|---:|---:|---:|---:|---:|---:|---:|:---:|");
    for input_kind in ["legacy-input", "realistic-payload"] {
        for level in LEVELS {
            let snr = TIMING_SNR.iter().find(|(l, _)| *l == level).unwrap().1;
            let payload_bits = if input_kind == "legacy-input" {
                0
            } else {
                max_payload_for_level(level).unwrap() * 8
            };
            let (old_us, new_us, iterations, old_sum, new_sum) =
                time_level(level, snr, payload_bits);
            let ratio = new_us / old_us;
            let verdict = if ratio <= 3.0 { "met" } else { "NOT met" };
            println!(
                "| {input_kind} | {level} | {snr:.1} | {old_us:.2} | {new_us:.2} | {ratio:.2}x | {iterations:.2} | {old_sum:016x} | {new_sum:016x} | {verdict} |"
            );
        }
    }
}
