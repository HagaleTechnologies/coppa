//! Generates the `CAPACITY_LEVEL_CORRECTION`/`SELECTIVITY_LEVEL_CORRECTION` tables
//! `coppa_ml::mcs::recommend_speed_level` uses to correct for the AWGN level-dependent bias
//! PR #51 confirmed and PR #52 root-caused to `SPEED_LEVELS[level].papr_target_db` (see
//! `docs/superpowers/specs/2026-07-22-rateloop-capacity-level-bias-correction-design.md`).
//!
//! Sounds every real speed level via `CoppaTransceiver::transmit` (so each level's own natural
//! `papr_target_db` applies, exactly as production does) at 5 AWGN SNR points (6/12/18/24/30 dB),
//! 200 trials/cell -- a higher trial count than the diagnostic benches used (40), since this one's
//! output is hardcoded into production. Prints per-(level, SNR) means, then both correction tables
//! as ready-to-paste Rust array literals, anchored so level 2 (the probe
//! `SPEED_LEVEL_MIN_CAPACITY` was itself calibrated against) is all-zeros by construction.
//!
//! Output: `DATA <snr> <level> <mean_capacity> <mean_selectivity> (n=<ok>/<TRIALS>)`, then the two
//! `RUST_TABLE` blocks.

use coppa_bench::scenario::{mode_for_level, profile_by_name, MODES};
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, channel_selectivity};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 200;
const SNRS: [f32; 5] = [6.0, 12.0, 18.0, 24.0, 30.0];
const ANCHOR_LEVEL: u8 = 2;

fn make_header(level: u8, len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len: len,
        codewords: 1,
    }
}

/// Mean `(capacity, selectivity)` over `TRIALS` AWGN-faded soundings of `level`, using
/// `CoppaTransceiver::transmit` so the level's real `SPEED_LEVELS` PAPR target applies.
fn sound_level(
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
    snr: f32,
    base: u64,
) -> (f32, f32, usize) {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let payload = vec![0x5Au8; pfb];
    let sig = tx
        .transmit(&make_header(level, pfb as u16), &payload)
        .expect("payload within this level's capacity");
    let (mut accc, mut accs, mut n) = (0.0f32, 0.0f32, 0usize);
    for t in 0..TRIALS {
        let seed = base
            .wrapping_add(t as u64)
            .wrapping_add((level as u64).wrapping_mul(0x1000_0000));
        let faded = coppa_channel::awgn_seeded(&sig, snr, seed ^ 0x5555);
        if let Some((_h, _eq, nv)) = modem.demodulate_frame(&faded) {
            accc += channel_capacity(&nv);
            accs += channel_selectivity(&nv);
            n += 1;
        }
    }
    if n > 0 {
        (accc / n as f32, accs / n as f32, n)
    } else {
        (0.0, 0.0, 0)
    }
}

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| {
            u64::from_str_radix(s.trim_start_matches("0x"), 16)
                .ok()
                .or_else(|| s.parse().ok())
        })
        .unwrap_or(0x000C_A11B);
    let profile = profile_by_name("robust").unwrap();
    eprintln!("capacity_level_correction_calibration seed=0x{seed:X}");

    // means[snr_idx][level_idx] = (mean_capacity, mean_selectivity)
    let mut means: Vec<Vec<(f32, f32)>> = Vec::new();
    for &snr in &SNRS {
        let mut row = Vec::new();
        for m in MODES {
            let (mc, ms, n) = sound_level(&profile, m.level, snr, seed);
            println!("DATA {snr:.0} {} {mc:.4} {ms:.4} (n={n}/{TRIALS})", m.level);
            row.push((mc, ms));
        }
        means.push(row);
    }

    let anchor_idx = MODES.iter().position(|m| m.level == ANCHOR_LEVEL).unwrap();

    println!();
    println!(
        "RUST_TABLE capacity (level order {:?}):",
        MODES.iter().map(|m| m.level).collect::<Vec<_>>()
    );
    println!("const CAPACITY_LEVEL_CORRECTION: [[f32; 5]; 9] = [");
    for (li, m) in MODES.iter().enumerate() {
        let row: Vec<String> = (0..SNRS.len())
            .map(|si| format!("{:.4}", means[si][li].0 - means[si][anchor_idx].0))
            .collect();
        println!("    [{}], // level {}", row.join(", "), m.level);
    }
    println!("];");

    println!();
    println!(
        "RUST_TABLE selectivity (level order {:?}):",
        MODES.iter().map(|m| m.level).collect::<Vec<_>>()
    );
    println!("const SELECTIVITY_LEVEL_CORRECTION: [[f32; 5]; 9] = [");
    for (li, m) in MODES.iter().enumerate() {
        let row: Vec<String> = (0..SNRS.len())
            .map(|si| format!("{:.4}", means[si][li].1 - means[si][anchor_idx].1))
            .collect();
        println!("    [{}], // level {}", row.join(", "), m.level);
    }
    println!("];");
}
