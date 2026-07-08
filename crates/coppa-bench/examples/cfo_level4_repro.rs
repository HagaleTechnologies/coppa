//! Standalone repro for the CFO x level-4 investigation
//! (`.superpowers/sdd/p2-cfo-level4-investigation-report.md`): reproduces the
//! flat, SNR-unresponsive FER floor that a nonzero CFO induces at level 4
//! (QPSK 3/4) via `CoppaModem::calibrated_bias`'s inability to represent the
//! CFO-induced sync-timing jitter. Parametrized entirely by env vars so it can
//! be re-run before/after a fix without editing source:
//!
//!   LEVELS="4"                       comma list of speed levels
//!   CFOS="0,33,36,38,39,39.5,40,40.5,41,42,45"   comma list of CFO Hz values
//!   SNRS="6,9,12,15,18,21,24,27,30"   comma list of SNR (dB) points
//!   TRIALS="60"                      trials per (level, cfo, snr) point
//!   CHANNEL="awgn"                   awgn | watterson-good | watterson-moderate | watterson-poor
//!   SEED="0xC0FFEE"                  base seed (hex or decimal)
//!
//! This is deliberately built on `coppa-bench`'s existing `Scenario`/`run_scenario`
//! (already supports `cfo_hz`), not bespoke instrumentation, so it stays available
//! after this task (unlike the investigation's own reverted temporary example).
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{ChannelSpec, Scenario};
use coppa_channel::watterson::WattersonPreset;

fn parse_list_f32(var: &str, default: &str) -> Vec<f32> {
    std::env::var(var)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(|s| s.trim().parse::<f32>().expect("bad float in list"))
        .collect()
}

fn parse_list_u8(var: &str, default: &str) -> Vec<u8> {
    std::env::var(var)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(|s| s.trim().parse::<u8>().expect("bad u8 in list"))
        .collect()
}

fn parse_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn parse_seed(var: &str, default: u64) -> u64 {
    match std::env::var(var) {
        Ok(s) => {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).expect("bad hex seed")
            } else {
                s.parse().expect("bad decimal seed")
            }
        }
        Err(_) => default,
    }
}

fn channel_spec(name: &str) -> ChannelSpec {
    match name {
        "awgn" => ChannelSpec::Awgn,
        "watterson-good" => ChannelSpec::Watterson(WattersonPreset::Good),
        "watterson-moderate" => ChannelSpec::Watterson(WattersonPreset::Moderate),
        "watterson-poor" => ChannelSpec::Watterson(WattersonPreset::Poor),
        other => panic!("unknown CHANNEL '{other}'"),
    }
}

fn main() {
    let levels = parse_list_u8("LEVELS", "4");
    let cfos = parse_list_f32("CFOS", "0,33,36,38,39,39.5,40,40.5,41,42,45");
    let snrs = parse_list_f32("SNRS", "6,9,12,15,18,21,24,27,30");
    let trials = parse_usize("TRIALS", 60);
    let channel_name = std::env::var("CHANNEL").unwrap_or_else(|_| "awgn".to_string());
    let channel = channel_spec(&channel_name);
    let seed = parse_seed("SEED", 0x00C0_FFEE);

    println!("level,cfo_hz,snr_db,fer,attempts,frames,channel");
    for &level in &levels {
        for &cfo in &cfos {
            let scenario = Scenario {
                level,
                channel,
                snr_db_points: snrs.clone(),
                trials,
                seed,
                profile_override: None,
                cfo_hz: cfo,
                ssb: false,
            };
            let points = run_scenario(&scenario);
            for p in &points {
                println!(
                    "{},{},{},{:.4},{},{},{}",
                    level, cfo, p.snr_db, p.fer, trials, trials, channel_name
                );
            }
        }
    }
}
