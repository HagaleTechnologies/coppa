//! Hypothesis-1 diagnostic (coarse-delay drift tracker investigation): is
//! `DriftTracker`'s own per-window delay estimate the bottleneck, independent
//! of what consumes it downstream? Both the "Replace" (fresh per-window LS
//! refit) and "Cascaded" (AR(1) tap tracker) designs measured Watterson-
//! Moderate/level 2 FER<=10% at exactly 18.0 dB -- two architecturally
//! different consumers landing on the identical number points at the shared
//! upstream input, not either consumer.
//!
//! Uses `CoppaModem::diagnose_drift_tracking` (scratch diagnostic method,
//! not a production path) to capture DriftTracker's filtered tau_by_window
//! AND the raw pre-Kalman observe_drift observations on real Watterson-
//! Moderate/level 2 frames across a representative SNR range, and correlates
//! against hard-decision BER (a decode-quality proxy, same technique
//! `estimator_diagnosis.rs` uses) from the SAME samples via the production
//! `demodulate_frame` path.
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use num_complex::Complex32;

const SNR_POINTS: [f32; 5] = [12.0, 15.0, 18.0, 21.0, 24.0];
const TRIALS_PER_SNR: u64 = 150;

fn main() {
    let profile = CoppaProfile::hf_standard();
    let modem = CoppaModem::new(profile, 1);
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: 2,
        seq_num: 0,
        payload_len: 121,
        codewords: 1,
    };
    let n_symbols = 3000;
    let symbols: Vec<Complex32> = (0..n_symbols)
        .map(|i| {
            if i % 3 == 0 {
                Complex32::new(-1.0, 0.0)
            } else {
                Complex32::new(1.0, 0.0)
            }
        })
        .collect();
    let clean = modem.modulate_mapped(&header, &symbols, 6.0);

    println!("channel,snr_db,trials,sync_or_header_fail,mean_hard_ber,mean_tau_range,mean_tau_vs_coarse_delay,mean_tau_vs_raw_z,corr_tau_range_vs_ber");

    for (channel_name, use_fading) in [("awgn_control", false), ("watterson_moderate", true)] {
        for snr_db in SNR_POINTS {
            let mut fails = 0u64;
            let mut ok_trials = 0u64;
            let mut ber_sum = 0.0f64;
            let mut tau_range_sum = 0.0f64;
            let mut tau_vs_coarse_sum = 0.0f64;
            let mut tau_vs_raw_sum = 0.0f64;
            let mut raw_count = 0u64;
            let mut per_frame: Vec<(f32, f32)> = Vec::new(); // (tau_range, hard_ber)

            for trial in 0..TRIALS_PER_SNR {
                let seed = 0xD817_0000u64.wrapping_add(trial);
                let faded = if use_fading {
                    coppa_channel::watterson::watterson(
                        &clean,
                        48_000.0,
                        &coppa_channel::watterson::WattersonPreset::Moderate.config(),
                        seed,
                    )
                } else {
                    clean.clone()
                };
                let p_clean = coppa_channel::mean_power(&clean);
                let noisy = coppa_channel::awgn_ref_seeded(
                    &faded,
                    snr_db,
                    p_clean,
                    48_000.0,
                    seed ^ 0x55AA,
                );

                let diag = modem.diagnose_drift_tracking(&noisy);
                let decode = modem.demodulate_frame(&noisy);

                let (Some(diag), Some((h, rx_symbols, _noise_vars))) = (diag, decode) else {
                    fails += 1;
                    continue;
                };
                if h.speed_level != 2 {
                    fails += 1;
                    continue;
                }

                let n = rx_symbols.len().min(symbols.len());
                let mut bit_errors = 0u64;
                for i in 0..n {
                    let decided = if rx_symbols[i].re >= 0.0 { 1.0 } else { -1.0 };
                    if decided != symbols[i].re {
                        bit_errors += 1;
                    }
                }
                let ber = bit_errors as f64 / n.max(1) as f64;

                let tau_min = diag.tau_by_window.iter().cloned().fold(f32::MAX, f32::min);
                let tau_max = diag.tau_by_window.iter().cloned().fold(f32::MIN, f32::max);
                let tau_range = tau_max - tau_min;

                let tau_vs_coarse: f32 = diag
                    .tau_by_window
                    .iter()
                    .map(|&t| (t - diag.coarse_delay).abs())
                    .sum::<f32>()
                    / diag.tau_by_window.len().max(1) as f32;

                let mut vs_raw_sum = 0.0f32;
                let mut vs_raw_n = 0u64;
                for (t, obs) in diag.tau_by_window.iter().zip(diag.raw_observations.iter()) {
                    if let Some((z, _r)) = obs {
                        vs_raw_sum += (t - z).abs();
                        vs_raw_n += 1;
                    }
                }
                if vs_raw_n > 0 {
                    tau_vs_raw_sum += (vs_raw_sum / vs_raw_n as f32) as f64;
                    raw_count += 1;
                }

                ok_trials += 1;
                ber_sum += ber;
                tau_range_sum += tau_range as f64;
                tau_vs_coarse_sum += tau_vs_coarse as f64;
                per_frame.push((tau_range, ber as f32));
            }

            let mean_ber = ber_sum / ok_trials.max(1) as f64;
            let mean_tau_range = tau_range_sum / ok_trials.max(1) as f64;
            let mean_tau_vs_coarse = tau_vs_coarse_sum / ok_trials.max(1) as f64;
            let mean_tau_vs_raw = tau_vs_raw_sum / raw_count.max(1) as f64;

            // Pearson correlation between per-frame tau_range and hard-decision BER.
            let corr = pearson(&per_frame);

            println!(
            "{channel_name},{snr_db},{TRIALS_PER_SNR},{fails},{mean_ber:.6},{mean_tau_range:.6},{mean_tau_vs_coarse:.6},{mean_tau_vs_raw:.6},{corr:.4}"
        );
        }
    }
}

fn pearson(pairs: &[(f32, f32)]) -> f32 {
    let n = pairs.len();
    if n < 2 {
        return f32::NAN;
    }
    let mean_x = pairs.iter().map(|&(x, _)| x).sum::<f32>() / n as f32;
    let mean_y = pairs.iter().map(|&(_, y)| y).sum::<f32>() / n as f32;
    let mut cov = 0.0f32;
    let mut var_x = 0.0f32;
    let mut var_y = 0.0f32;
    for &(x, y) in pairs {
        let dx = x - mean_x;
        let dy = y - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    if var_x <= 1e-12 || var_y <= 1e-12 {
        return f32::NAN;
    }
    cov / (var_x.sqrt() * var_y.sqrt())
}
