//! Root-cause diagnostic: compare hard-decision symbol accuracy (channel-estimate
//! quality, independent of soft-decision noise-variance/LLR calibration) and the
//! noise_variances CoppaModem::demodulate_frame returns, on the SAME real
//! Watterson-Moderate-faded frames, across many seeds at the level-2 FER@10%
//! crossing boundary (18 dB). Run this identically on the pre-Task-1 baseline and
//! on the Task-1 branch to see whether the regression is in the channel-estimate
//! itself (hard-decision BER differs) or downstream (BER similar, noise_var stats
//! differ).
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use num_complex::Complex32;

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
    };
    // Known BPSK-like symbol pattern (matches the existing clean-loopback test style).
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

    const SNR_DB: f32 = 18.0;
    let trials: u64 = if std::env::var("COPPA_DIAG").is_ok() {
        1
    } else {
        200
    };

    let mut total_bit_errors = 0u64;
    let mut total_bits = 0u64;
    let mut header_fail = 0u64;
    let mut sync_fail = 0u64;
    let mut noise_min = f32::MAX;
    let mut noise_max = f32::MIN;
    let mut noise_sum = 0.0f64;
    let mut noise_n = 0u64;

    for trial in 0..trials {
        let seed = 0xFADE_0000u64.wrapping_add(trial);
        let faded = coppa_channel::watterson::watterson(
            &clean,
            48_000.0,
            &coppa_channel::watterson::WattersonPreset::Moderate.config(),
            seed,
        );
        let p_clean = coppa_channel::mean_power(&clean);
        let noisy =
            coppa_channel::awgn_ref_seeded(&faded, SNR_DB, p_clean, 48_000.0, seed ^ 0x55AA);

        match modem.demodulate_frame(&noisy) {
            None => sync_fail += 1,
            Some((h, rx_symbols, noise_vars)) => {
                if h.speed_level != 2 {
                    header_fail += 1;
                    continue;
                }
                let n = rx_symbols.len().min(symbols.len());
                for i in 0..n {
                    let decided = if rx_symbols[i].re >= 0.0 { 1.0 } else { -1.0 };
                    let truth = symbols[i].re;
                    if decided != truth {
                        total_bit_errors += 1;
                    }
                    total_bits += 1;
                }
                for &nv in noise_vars.iter().take(n) {
                    noise_min = noise_min.min(nv);
                    noise_max = noise_max.max(nv);
                    noise_sum += nv as f64;
                    noise_n += 1;
                }
            }
        }
    }

    let ber = total_bit_errors as f64 / total_bits.max(1) as f64;
    println!("trials={trials} sync_fail={sync_fail} header_fail={header_fail}");
    println!(
        "hard-decision BER over {total_bits} symbols: {:.6} ({total_bit_errors} errors)",
        ber
    );
    println!(
        "noise_var: min={noise_min} max={noise_max} mean={}",
        noise_sum / noise_n.max(1) as f64
    );
}
