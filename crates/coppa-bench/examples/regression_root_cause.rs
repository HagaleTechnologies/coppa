//! Root-cause diagnostic (investigation only, no fix implied): three fix attempts
//! (Task 1's delay-domain estimator, Task 7's AR(1) tracker, the "Replace" and
//! "Cascaded" drift-tracker designs, PRs #41/#42) have all targeted the same theory
//! -- that a "stale coarse-delay reference" explains the within-frame |H|^2 decay
//! Task 1's original diagnostic found on Watterson-Moderate/level 2 -- and all four
//! land on the same 18.0-30.0 dB floor. This tool checks whether that theory is even
//! right, by:
//!
//! 1. Categorizing every dropped frame's failure mode (SyncFailed/HeaderCorrupt/
//!    LdpcNotConverged/CrcMismatch/WrongCodeword) across the SNR range where the
//!    regression bites, at real trial counts.
//! 2. Isolating genuine Rayleigh AMPLITUDE fading from multipath/delay-reference
//!    effects entirely: a FLAT single-tap channel has no multipath, so "stale
//!    coarse-delay reference" isn't even a coherent failure mode for it -- yet it
//!    uses the SAME Doppler spread as real Moderate. If a flat single-tap channel at
//!    Moderate's real Doppler ALSO shows severe FER degradation vs AWGN, that is
//!    direct, delay-reference-independent evidence that the loss is dominated by
//!    genuine amplitude fading, not the coarse-delay theory.
//!
//! Run: `cargo run --release -p coppa-bench --example regression_root_cause`

use coppa_bench::scenario::mode_for_level;
use coppa_channel::watterson::{watterson, Tap, WattersonConfig, WattersonPreset};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::{CoppaTransceiver, ReceiveError};

const LEVEL: u8 = 2;
const TRIALS: usize = 400;

fn make_header(payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: LEVEL,
        seq_num: 0,
        payload_len,
        codewords: 1,
    }
}

/// Flat (frequency-non-selective) single-tap channel at the given Doppler spread --
/// no multipath, so any FER cost here cannot be explained by a stale coarse-delay
/// reference (there is nothing for that reference to be stale ABOUT).
fn flat_config(doppler_spread_hz: f32) -> WattersonConfig {
    WattersonConfig {
        taps: vec![Tap {
            delay_s: 0.0,
            power: 1.0,
        }],
        doppler_spread_hz,
    }
}

#[derive(Default)]
struct Outcomes {
    ok: usize,
    sync_fail: usize,
    header_fail: usize,
    ldpc_fail: usize,
    crc_fail: usize,
    wrong_codeword: usize,
}

impl Outcomes {
    fn fer(&self) -> f64 {
        1.0 - self.ok as f64 / TRIALS as f64
    }
}

fn run(
    tx: &CoppaTransceiver,
    clean: &[f32],
    payload: &[u8],
    snr_db: f32,
    cfg: Option<&WattersonConfig>,
    seed_base: u64,
) -> Outcomes {
    let mut o = Outcomes::default();
    for t in 0..TRIALS {
        let seed = seed_base.wrapping_add(t as u64);
        let (faded, clean_power) = match cfg {
            Some(c) => {
                let f = watterson(clean, 48_000.0, c, seed);
                (f, coppa_channel::mean_power(clean))
            }
            None => (clean.to_vec(), coppa_channel::mean_power(clean)),
        };
        let noisy =
            coppa_channel::awgn_ref_seeded(&faded, snr_db, clean_power, 48_000.0, seed ^ 0x55AA);
        match tx.receive(&noisy) {
            Ok((_h, p, _lvl)) => {
                if p.len() >= payload.len() && p[..payload.len()] == payload[..] {
                    o.ok += 1;
                } else {
                    o.wrong_codeword += 1;
                }
            }
            Err(ReceiveError::SyncFailed) => o.sync_fail += 1,
            Err(ReceiveError::HeaderCorrupt) => o.header_fail += 1,
            Err(ReceiveError::LdpcNotConverged { .. }) => o.ldpc_fail += 1,
            Err(ReceiveError::CrcMismatch) => o.crc_fail += 1,
        }
    }
    o
}

fn main() {
    let profile = CoppaProfile::hf_standard();
    let tx = CoppaTransceiver::new(profile, 1);
    let pfb = mode_for_level(LEVEL).expect("level 2 mode").payload_bytes();
    let payload: Vec<u8> = (0..pfb)
        .map(|i| ((i as u64).wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let header = make_header(pfb as u16);
    let clean = tx.transmit(&header, &payload).expect("transmit level 2");

    println!(
        "=== Part 1: receive() outcome breakdown, real Watterson-Moderate, level 2, SNR sweep ==="
    );
    println!(
        "(payload_len={pfb}, frame_samples={}, {TRIALS} trials/point)\n",
        clean.len()
    );
    println!(
        "{:>6} {:>6} {:>5} {:>5} {:>8} {:>5} {:>13} {:>8}",
        "snr", "ok", "sync", "hdr", "ldpc_ncv", "crc", "wrong_cw", "FER%"
    );
    let moderate = WattersonPreset::Moderate.config();
    for &snr in &[9.0f32, 12.0, 15.0, 18.0, 21.0, 24.0] {
        let o = run(
            &tx,
            &clean,
            &payload,
            snr,
            Some(&moderate),
            0xC0FF_EE00_u64.wrapping_add(snr as u64 * 1000),
        );
        println!(
            "{:>6.1} {:>6} {:>5} {:>5} {:>8} {:>5} {:>13} {:>8.2}",
            snr,
            o.ok,
            o.sync_fail,
            o.header_fail,
            o.ldpc_fail,
            o.crc_fail,
            o.wrong_codeword,
            o.fer() * 100.0
        );
    }

    println!("\n=== Part 2: flat single-tap (NO multipath) at Moderate's real Doppler (0.5 Hz) vs AWGN ===");
    println!(
        "(a flat channel has no multipath, so a 'stale coarse-delay reference' cannot apply --"
    );
    println!(
        " any FER cost here is genuine Rayleigh AMPLITUDE fading, isolated from delay tracking)\n"
    );
    println!(
        "{:>6} {:>10} {:>10} {:>10}",
        "snr", "AWGN_FER%", "flat_FER%", "2tap_Mod_FER%"
    );
    let flat_moderate_doppler = flat_config(0.5);
    for &snr in &[9.0f32, 12.0, 15.0, 18.0, 21.0, 24.0] {
        let awgn = run(
            &tx,
            &clean,
            &payload,
            snr,
            None,
            0xA000_u64.wrapping_add(snr as u64 * 1000),
        );
        let flat = run(
            &tx,
            &clean,
            &payload,
            snr,
            Some(&flat_moderate_doppler),
            0xB000_u64.wrapping_add(snr as u64 * 1000),
        );
        let twotap = run(
            &tx,
            &clean,
            &payload,
            snr,
            Some(&moderate),
            0xC0FF_EE00_u64.wrapping_add(snr as u64 * 1000),
        );
        println!(
            "{:>6.1} {:>10.2} {:>10.2} {:>10.2}",
            snr,
            awgn.fer() * 100.0,
            flat.fer() * 100.0,
            twotap.fer() * 100.0
        );
    }

    println!("\nReading Part 2: if flat_FER% is close to AWGN_FER%, amplitude fading alone (no");
    println!(
        "multipath) costs little -- the coarse-delay theory remains plausible as the dominant"
    );
    println!(
        "driver of the 2-tap gap. If flat_FER% is substantially worse than AWGN and approaches"
    );
    println!("2tap_Mod_FER%, genuine Rayleigh amplitude fading (which no coarse-delay tracker can");
    println!(
        "fix) is doing most of the work, and multipath/delay-reference effects are secondary."
    );
}
