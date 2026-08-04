//! COP-6 NR BG2 decode timing harness.
//!
//! Do not measure while another Cargo process is running on this host. Part A
//! forces a non-convergent decode through each iteration cap and reports the
//! minimum, median, and spread of five repeats plus a least-squares cost fit.

use coppa_protocol::fec::ldpc::decoder::NrBg2Decoder;
use std::hint::black_box;
use std::time::Instant;

const ALPHA: f32 = 0.75;
const CAPS: [usize; 7] = [1, 2, 3, 5, 10, 20, 30];
const REPEATS: usize = 5;
const CALLS: usize = 300;
const WARMUP: usize = 50;

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
}
