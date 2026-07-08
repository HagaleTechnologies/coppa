//! Diagnostic: WHY does the PHY collapse under Watterson fading?
//!
//! Phase-1 evidence gathering — categorizes the *receive outcome* (sync failure vs
//! decode failure vs success) across controlled channel conditions, to localize the
//! failure: sync (H1), frequency-selective multipath / equalizer (H2), or noise.

use coppa_channel::awgn_seeded;
use coppa_channel::watterson::{watterson, Tap, WattersonConfig, WattersonPreset};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::{CoppaTransceiver, ReceiveError};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const FS: f32 = 48_000.0;

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

fn select_profile(level: u8) -> CoppaProfile {
    if level >= 5 {
        CoppaProfile::vhf_wide()
    } else {
        CoppaProfile::hf_standard()
    }
}

fn payload_bytes(level: u8) -> usize {
    match level {
        1 => 60,
        2 => 121,
        3 => 121,
        4 => 182,
        5 => 162,
        6 => 121,
        7 => 182,
        9 => 162,
        10 => 212,
        _ => 121,
    }
}

#[derive(Default)]
struct Counts {
    correct: u32,
    ok_wrong: u32,
    sync_failed: u32,
    header_corrupt: u32,
    ldpc_fail: u32,
    crc: u32,
}

enum Chan {
    Awgn,
    Flat,
    Preset(WattersonPreset),
}

fn apply(kind: &Chan, clean: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    let faded = match kind {
        Chan::Awgn => return awgn_seeded(clean, snr_db, seed ^ 0x5555_5555_5555_5555),
        Chan::Flat => {
            // Single tap, no delay => flat (frequency-flat) fading: pure complex gain.
            let cfg = WattersonConfig {
                taps: vec![Tap {
                    delay_s: 0.0,
                    power: 1.0,
                }],
                doppler_spread_hz: 0.1,
            };
            watterson(clean, FS, &cfg, seed ^ 0x3333_3333_3333_3333)
        }
        Chan::Preset(p) => watterson(clean, FS, &p.config(), seed ^ 0x3333_3333_3333_3333),
    };
    awgn_seeded(&faded, snr_db, seed ^ 0x5555_5555_5555_5555)
}

fn run(level: u8, kind: &Chan, snr_db: f32, trials: u32) -> Counts {
    let tx = CoppaTransceiver::new(select_profile(level), 1);
    let pb = payload_bytes(level);
    let mut c = Counts::default();
    for t in 0..trials {
        let seed = 0x1000_0000u64 + t as u64;
        let mut rng = StdRng::seed_from_u64(seed);
        let payload: Vec<u8> = (0..pb).map(|_| rng.random::<u8>()).collect();
        let clean = tx.transmit(&make_header(level, pb as u16), &payload);
        let rx = apply(kind, &clean, snr_db, seed);
        match tx.receive(&rx) {
            Ok((_h, p)) => {
                if p.len() >= payload.len() && p[..payload.len()] == payload[..] {
                    c.correct += 1;
                } else {
                    c.ok_wrong += 1;
                }
            }
            Err(ReceiveError::SyncFailed) => c.sync_failed += 1,
            Err(ReceiveError::HeaderCorrupt) => c.header_corrupt += 1,
            Err(ReceiveError::LdpcNotConverged { .. }) => c.ldpc_fail += 1,
            Err(ReceiveError::CrcMismatch) => c.crc += 1,
        }
    }
    c
}

fn main() {
    let hf = CoppaProfile::hf_standard();
    let vhf = CoppaProfile::vhf_wide();
    println!("=== OFDM timing (CP vs multipath delay) ===");
    println!(
        "hf_standard: CP {} smp = {:.2} ms | useful sym {:.1} ms | {} pilots / {} data | spacing {:.0} Hz",
        hf.cp_samples, hf.cp_samples as f32 / FS * 1000.0, hf.fft_size as f32 / FS * 1000.0,
        hf.pilot_carriers, hf.data_carriers, FS / hf.fft_size as f32
    );
    println!(
        "vhf_wide:    CP {} smp = {:.2} ms | useful sym {:.1} ms | {} pilots / {} data | spacing {:.0} Hz",
        vhf.cp_samples, vhf.cp_samples as f32 / FS * 1000.0, vhf.fft_size as f32 / FS * 1000.0,
        vhf.pilot_carriers, vhf.data_carriers, FS / vhf.fft_size as f32
    );
    println!("Watterson delays: Good 0.5ms(24smp) Moderate 1ms(48) Poor 2ms(96)\n");

    let trials = 150;
    let conditions: &[(Chan, &str, f32)] = &[
        (Chan::Awgn, "awgn", 30.0),
        (Chan::Flat, "flat-fade", 30.0),
        (Chan::Preset(WattersonPreset::Good), "good-2tap", 30.0),
        (Chan::Preset(WattersonPreset::Good), "good-noiseless", 60.0),
        (Chan::Preset(WattersonPreset::Poor), "poor-2tap", 30.0),
    ];

    for (level, label) in [(1u8, "BPSK1/4"), (2, "BPSK1/2"), (3, "QPSK1/2")] {
        println!("--- level {} ({}), {} trials ---", level, label, trials);
        println!(
            "{:14} {:>4} | {:>7} {:>8} | {:>9} {:>4} {:>9}",
            "channel", "snr", "correct", "ok_wrong", "sync_fail", "hdr", "ldpc_fail"
        );
        for (kind, name, snr) in conditions {
            let c = run(level, kind, *snr, trials);
            println!(
                "{:14} {:>4.0} | {:>7} {:>8} | {:>9} {:>4} {:>9}",
                name, snr, c.correct, c.ok_wrong, c.sync_failed, c.header_corrupt, c.ldpc_fail
            );
            let _ = c.crc;
        }
        println!();
    }
}
