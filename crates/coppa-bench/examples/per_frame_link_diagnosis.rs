//! Per-frame link diagnosis (systematic-debugging Phase 1 evidence-gathering).
//!
//! SP2 showed coppa's per-frame OFDM link averages BELOW the LDPC threshold under Watterson
//! fading (so cross-frame diversity hurts). This instrument localizes WHY, at level 2 (BPSK
//! 1/2) and 30 dB (noise negligible — any failure is channel-induced), by measuring the RAW
//! (pre-FEC) bit-error rate of a single frame under five channel conditions and decomposing it:
//!
//! * AWGN: baseline; raw BER should be ~0.
//! * Flat 1-tap Rayleigh: amplitude fading only (frequency-FLAT). Isolates candidate B
//!   (amplitude/flat fading) from candidate A (frequency-selective).
//! * Watterson Good/Mod/Poor: two EQUAL-power taps → deep frequency nulls spaced 1/delay apart
//!   (Good ~2 kHz ≈1 null in-band → Poor ~500 Hz ≈4 nulls).
//!
//! A-vs-B: if flat-1tap raw BER is low but 2-tap is high → frequency-selectivity is the cause.
//! A1-vs-A2 (position-independent): partition each frame's carriers by the equalizer's reported
//! noise variance nv = sigma^2/|H_est|^2 (the confidence it feeds the LDPC). If hard-errors
//! concentrate where nv is HIGH → the equalizer correctly flagged the nulls as erasures → A1
//! (FEC coverage problem). If errors land where nv is LOW/moderate → the equalizer was confident
//! and WRONG → A2 (pilots missed the null; mis-estimation).
//!
//! This is a DIAGNOSTIC. It proposes no fix; it gathers evidence to localize the failing
//! component. Run: `cargo run --release -p coppa-bench --example per_frame_link_diagnosis`.

use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::traits::FecCodec;
use coppa_protocol::fec::ldpc::LdpcCodec;
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{speed_level_components, speed_level_entry};
use coppa_protocol::modem::transceiver::{CoppaTransceiver, ReceiveError};

use coppa_bench::scenario::mode_for_level;
use coppa_channel::watterson::{watterson, Tap, WattersonConfig, WattersonPreset};

const CODED_BITS: usize = 1944;
const SNR_DB: f32 = 30.0; // high SNR: any failure is channel-induced, not noise-induced
const TRIALS: usize = 60;

/// Channel under test for the diagnosis.
enum Cond {
    Awgn,
    Fading(WattersonConfig),
}

fn make_header(level: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
    }
}

/// Flat (frequency-non-selective) single-tap Rayleigh fade — pure amplitude fading.
fn flat_config() -> WattersonConfig {
    WattersonConfig {
        taps: vec![Tap {
            delay_s: 0.0,
            power: 1.0,
        }],
        doppler_spread_hz: 0.1,
    }
}

struct Stats {
    raw_ber: f64,
    post_fec_success: f64,
    // A1/A2 partition: hard-error rate among carriers the equalizer flagged as high-noise
    // (nv > per-frame median) vs low-noise (nv <= median).
    err_rate_high_nv: f64,
    err_rate_low_nv: f64,
    // Fraction of carriers flagged "deeply faded" (nv > 10x per-frame median).
    flagged_frac: f64,
}

fn measure(
    label: &str,
    cond: &Cond,
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
) -> Stats {
    let profile = profile.clone();
    let modem = CoppaModem::new(profile.clone(), 1);
    let (mapper, code_rate) = speed_level_components(level).expect("level 2 components");
    let papr = speed_level_entry(level)
        .expect("level 2 entry")
        .papr_target_db;
    let info_bits = code_rate.info_bits();
    let data_carriers = profile.data_carriers;
    let pfb = mode_for_level(level).expect("level 2 mode").payload_bytes();
    let tx = CoppaTransceiver::new(profile.clone(), 1);

    let mut total_bits = 0usize;
    let mut total_errs = 0usize;
    let mut post_ok = 0usize;
    let mut errs_high = 0usize;
    let mut n_high = 0usize;
    let mut errs_low = 0usize;
    let mut n_low = 0usize;
    let mut flagged = 0usize;
    let mut flagged_denom = 0usize;
    // receive() outcome breakdown: where in the RX chain do frames die?
    let (mut n_ok, mut n_sync, mut n_header, mut n_ldpc, mut n_crc, mut n_mismatch) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);

    for t in 0..TRIALS {
        let seed = 0x0D1A_6005_u64.wrapping_mul(t as u64 + 1);
        // Deterministic pseudo-random payload from the seed (no rng dep needed here).
        let payload: Vec<u8> = (0..pfb)
            .map(|i| (seed.wrapping_add(i as u64).wrapping_mul(2654435761) >> 24) as u8)
            .collect();

        // --- Encode exactly as V1/transceiver does, keeping the transmitted (interleaved) bits.
        let mut bits = Vec::with_capacity(info_bits);
        for &byte in &payload {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1);
            }
        }
        bits.resize(info_bits, 0u8);
        scramble(&mut bits);
        let coded = LdpcCodec::new(code_rate).encode(&bits); // CODED_BITS
        let interleaved = BlockInterleaver::new(CODED_BITS, data_carriers).interleave(&coded);
        let symbols = mapper.map_bits(&interleaved);
        let signal = modem.modulate_mapped(&make_header(level, pfb as u16), &symbols, papr);

        // --- Channel (same faded signal feeds both the raw-BER probe and the post-FEC decode).
        let faded = match cond {
            Cond::Awgn => coppa_channel::awgn_seeded(&signal, SNR_DB, seed ^ 0x5555),
            Cond::Fading(cfg) => {
                let f = watterson(&signal, 48_000.0, cfg, seed ^ 0x3333);
                coppa_channel::awgn_seeded(&f, SNR_DB, seed ^ 0x5555)
            }
        };

        // --- Raw (pre-FEC) BER + nv partition.
        if let Some((_h, eq, nv)) = modem.demodulate_soft_coded(&faded) {
            let n = CODED_BITS.min(eq.len()).min(nv.len());
            if n > 0 {
                // Per-frame median nv for the A1/A2 partition.
                let mut sorted: Vec<f32> = nv[..n].to_vec();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = sorted[n / 2].max(1e-9);
                for i in 0..n {
                    let hard = if eq[i].re < 0.0 { 1u8 } else { 0u8 };
                    let err = hard != interleaved[i];
                    total_bits += 1;
                    if err {
                        total_errs += 1;
                    }
                    if nv[i] > median {
                        n_high += 1;
                        if err {
                            errs_high += 1;
                        }
                    } else {
                        n_low += 1;
                        if err {
                            errs_low += 1;
                        }
                    }
                    flagged_denom += 1;
                    if nv[i] > 10.0 * median {
                        flagged += 1;
                    }
                }
            }
        } else {
            // Sync failure counts as an all-error frame (every bit wrong-ish).
            total_bits += CODED_BITS;
            total_errs += CODED_BITS / 2;
        }

        // --- Post-FEC: decode the SAME faded signal through the real receive path,
        //     recording WHERE it dies.
        match tx.receive(&faded) {
            Ok((_h, p)) => {
                if p.len() >= pfb && p[..pfb] == payload[..] {
                    post_ok += 1;
                    n_ok += 1;
                } else {
                    n_mismatch += 1; // converged to the WRONG codeword
                }
            }
            Err(ReceiveError::SyncFailed) => n_sync += 1,
            Err(ReceiveError::HeaderCorrupt) => n_header += 1,
            Err(ReceiveError::LdpcNotConverged { .. }) => n_ldpc += 1,
            Err(ReceiveError::CrcMismatch) => n_crc += 1,
        }
    }

    println!(
        "  └ receive() outcomes: ok={n_ok} sync_fail={n_sync} header_fail={n_header} \
         ldpc_not_converged={n_ldpc} crc_fail={n_crc} wrong_codeword={n_mismatch}"
    );

    let s = Stats {
        raw_ber: total_errs as f64 / total_bits.max(1) as f64,
        post_fec_success: post_ok as f64 / TRIALS as f64,
        err_rate_high_nv: errs_high as f64 / n_high.max(1) as f64,
        err_rate_low_nv: errs_low as f64 / n_low.max(1) as f64,
        flagged_frac: flagged as f64 / flagged_denom.max(1) as f64,
    };
    println!(
        "{:<12} rawBER={:>6.2}%  postFEC={:>5.1}%   err@highNV={:>6.2}%  err@lowNV={:>6.2}%  flagged={:>5.2}%",
        label,
        s.raw_ber * 100.0,
        s.post_fec_success * 100.0,
        s.err_rate_high_nv * 100.0,
        s.err_rate_low_nv * 100.0,
        s.flagged_frac * 100.0,
    );
    s
}

fn main() {
    let profile_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "standard".to_string());
    let profile = match profile_name.as_str() {
        "standard" => coppa_codec::ofdm::CoppaProfile::hf_standard(),
        "robust" => coppa_codec::ofdm::CoppaProfile::hf_robust(),
        other => panic!("unknown profile '{other}' (expected: standard|robust)"),
    };
    println!(
        "Profile: {profile_name} ({} data / {} pilots)",
        profile.data_carriers, profile.pilot_carriers
    );
    let level: u8 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    println!("Level: {level}");
    println!("Per-frame link diagnosis — level 2 (BPSK 1/2), {SNR_DB} dB, {TRIALS} trials/cond");
    println!("(high SNR: failures are channel-induced, not noise-induced)\n");
    println!(
        "{:<12} {:>13}  {:>13}   {:>16}  {:>14}  {:>11}",
        "condition", "raw pre-FEC", "post-FEC", "A2-probe", "A1-probe", "nulls"
    );

    measure("AWGN", &Cond::Awgn, &profile, level);
    measure("flat-1tap", &Cond::Fading(flat_config()), &profile, level);
    measure(
        "Good-2tap",
        &Cond::Fading(WattersonPreset::Good.config()),
        &profile,
        level,
    );
    measure(
        "Moderate",
        &Cond::Fading(WattersonPreset::Moderate.config()),
        &profile,
        level,
    );
    measure(
        "Poor",
        &Cond::Fading(WattersonPreset::Poor.config()),
        &profile,
        level,
    );

    println!(
        "\nReading: A-vs-B — if flat-1tap rawBER is low but 2-tap is high, the cause is\n\
         frequency-selectivity (multipath nulls), not amplitude fading. A1-vs-A2 — if errors\n\
         concentrate at err@highNV, the equalizer correctly flags nulls as erasures (A1: FEC\n\
         coverage). If errors sit at err@lowNV, the equalizer is confident-and-wrong (A2:\n\
         pilots miss the nulls / mis-estimation)."
    );
}
