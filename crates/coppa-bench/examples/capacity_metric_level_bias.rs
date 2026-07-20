//! Diagnose whether `coppa_ml::channel_capacity`'s reading is level-dependent on the SAME
//! underlying channel -- CLAUDE.md's RateLoop known-limitation entry names this as the
//! suspected root cause of RateLoop not meeting its acceptance bar, but that diagnosis was
//! informal/uncommitted. This bench sounds at EVERY real speed level (unlike
//! `mcs_calibration.rs`'s `sound()`, which hardcodes a level-2 probe) across AWGN (negative
//! control -- no fading drift, so duration should not matter there) and Watterson
//! Good/Moderate/Poor, many trials per cell, and reports whether mean capacity trends with
//! level beyond each cell's own noise floor.
//!
//! See docs/superpowers/specs/2026-07-20-rateloop-capacity-metric-bias-diagnosis-design.md.
//!
//! Output: `DATA <channel> <snr> <level> <mean_capacity> <std_capacity> <mean_selectivity>`
//! followed by a per-channel SUMMARY line. Pass a seed as arg 1 (default 0xCA11B, matching
//! `mcs_calibration.rs`'s convention).

use coppa_bench::scenario::{profile_by_name, ChannelSpec, ModeInfo, MODES, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, channel_selectivity};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 40;

fn apply_channel(sig: &[f32], ch: ChannelSpec, snr: f32, seed: u64) -> Vec<f32> {
    match ch {
        ChannelSpec::Awgn => coppa_channel::awgn_seeded(sig, snr, seed ^ 0x5555),
        ChannelSpec::Watterson(p) => {
            let f = coppa_channel::watterson::watterson(
                sig,
                SAMPLE_RATE as f32,
                &p.config(),
                seed ^ 0x3333,
            );
            coppa_channel::awgn_seeded(&f, snr, seed ^ 0x5555)
        }
    }
}

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

/// Mean and population standard deviation of a slice.
fn mean_std(xs: &[f32]) -> (f32, f32) {
    let n = xs.len() as f32;
    let mean = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Sound `mode.level` `TRIALS` times against `ch`/`snr`, returning per-trial
/// `(capacity, selectivity)` for every trial that synced and demodulated.
fn sound_level(
    profile: &coppa_codec::ofdm::CoppaProfile,
    mode: &ModeInfo,
    ch: ChannelSpec,
    snr: f32,
    base: u64,
) -> Vec<(f32, f32)> {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode.payload_bytes();
    let payload = vec![0x5Au8; pfb];
    let sig = tx
        .transmit(&make_header(mode.level, pfb as u16), &payload)
        .expect("payload within this level's capacity");
    let mut out = Vec::with_capacity(TRIALS);
    for t in 0..TRIALS {
        let seed = base
            .wrapping_add(t as u64)
            .wrapping_add((mode.level as u64).wrapping_mul(0x1000_0000));
        let faded = apply_channel(&sig, ch, snr, seed);
        if let Some((_h, _eq, nv)) = modem.demodulate_frame(&faded) {
            out.push((channel_capacity(&nv), channel_selectivity(&nv)));
        }
    }
    out
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
    let channels = [
        (ChannelSpec::Awgn, "AWGN"),
        (ChannelSpec::Watterson(WattersonPreset::Good), "Good"),
        (
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            "Moderate",
        ),
        (ChannelSpec::Watterson(WattersonPreset::Poor), "Poor"),
    ];
    let snrs = [6.0f32, 12.0, 18.0, 24.0, 30.0];
    eprintln!("capacity_metric_level_bias seed=0x{seed:X}");

    // cells[channel_idx][snr_idx][level_idx] = (mean_capacity, std_capacity)
    let mut cells: Vec<Vec<Vec<(f32, f32)>>> = Vec::new();

    for (ch, cname) in channels {
        let mut ch_rows = Vec::new();
        for &snr in &snrs {
            let mut row = Vec::new();
            for m in MODES {
                let samples = sound_level(&profile, m, ch, snr, seed);
                let caps: Vec<f32> = samples.iter().map(|&(c, _)| c).collect();
                let sels: Vec<f32> = samples.iter().map(|&(_, s)| s).collect();
                let (mean_c, std_c) = mean_std(&caps);
                let (mean_s, _std_s) = mean_std(&sels);
                println!(
                    "DATA {cname} {snr:.0} {} {mean_c:.3} {std_c:.3} {mean_s:.3}",
                    m.level
                );
                row.push((mean_c, std_c));
            }
            ch_rows.push(row);
        }
        cells.push(ch_rows);
    }

    // Summary: per channel, average (across the SNR grid) of (highest-level mean capacity -
    // lowest-level mean capacity), and whether that gap exceeds 2 standard errors (std /
    // sqrt(TRIALS)) of either endpoint -- a simple, honest significance check, not just a raw
    // number.
    println!();
    println!("SUMMARY (per channel, averaged across SNR grid):");
    for (ci, (_ch, cname)) in channels.iter().enumerate() {
        let mut gaps = Vec::new();
        let mut se_bounds = Vec::new();
        for row in &cells[ci] {
            let (lo_mean, lo_std) = row[0];
            let (hi_mean, hi_std) = row[row.len() - 1];
            gaps.push(hi_mean - lo_mean);
            let se_lo = lo_std / (TRIALS as f32).sqrt();
            let se_hi = hi_std / (TRIALS as f32).sqrt();
            se_bounds.push(2.0 * (se_lo + se_hi));
        }
        let (mean_gap, _) = mean_std(&gaps);
        let (mean_se_bound, _) = mean_std(&se_bounds);
        let verdict = if mean_gap.abs() > mean_se_bound {
            "LIKELY REAL TREND"
        } else {
            "within noise"
        };
        println!(
            "  {cname}: level1->level10 mean capacity gap = {mean_gap:+.3} bits/s/Hz \
             (2*SE bound = {mean_se_bound:.3}) -> {verdict}"
        );
    }
}
