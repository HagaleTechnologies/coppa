//! Phase 3 Task 3 retransmission-efficiency bench: IR-HARQ vs Chase-combining
//! vs plain (no combining) retransmission strategies.
//!
//! Fixes a "poor channel" operating point -- low SNR, substantial first-TX
//! failure -- calibrated (measured directly below, not assumed) to land close
//! to a 50% first-transmission FER, then simulates all three retransmission
//! strategies through repeated real `CoppaTransceiver::transmit`/`receive`
//! round trips over independently-corrupted copies of the same underlying
//! channel draws per (trial, attempt) -- a paired comparison, so all three
//! strategies see the exact same sequence of channel conditions and differ
//! only in how the sequence of transmissions is encoded (which RV) and
//! decoded (whether previous attempts' LLRs are combined):
//!
//! - **IR** (incremental redundancy): each retransmission cycles RV per
//!   `coppa_protocol::arq::rv_for_attempt` (the real wire behavior --
//!   `[0,2,3,1][attempt % 4]`), and `CoppaTransceiver::receive` combines every
//!   attempt's LLRs into the seq's running mother-domain accumulator
//!   automatically (Phase 3 Task 3).
//! - **Chase**: every (re)transmission re-sends the identical RV0 encoding;
//!   `receive` still combines across attempts (its combining logic doesn't
//!   care whether successive RVs differ), but no NEW redundancy is ever sent
//!   -- pure repetition/diversity combining, the classical Chase-combining
//!   baseline IR-HARQ improves on.
//! - **Plain** (no combining): every (re)transmission also re-sends RV0, but
//!   the receiver's IR-HARQ buffer for that seq is explicitly evicted after
//!   every attempt (win or lose) via `CoppaTransceiver::harq_evict`, so each
//!   attempt is decoded from scratch with no memory of previous attempts --
//!   the pre-Task-3 behavior.
//!
//! ## Level/channel choice
//!
//! Uses [`LEVEL`] 10 (64-QAM, rate 5/6 -- this ladder's least redundant code)
//! with AWGN only (see [`USE_FADING`]'s doc). Two earlier attempts at this
//! bench are worth recording (see the Task 3 report for the full data):
//! level 2 (BPSK, rate 1/2) under Watterson-Moderate fading gave a real,
//! reproducible (independent of Task 5 turbo re-estimation) result where IR
//! did NOT beat Chase/Plain -- level 2's RV2/RV3 windows land entirely in
//! deep parity with no direct payload-bit evidence at all, so IR's second
//! attempt adds much less *directly useful* new information than Chase's
//! literal repeat of the one window that actually contains the payload bits,
//! at a code rate already redundant enough per single shot that the marginal
//! benefit of IR's extra parity is small. Level 10 under Watterson-Moderate
//! fading hits its OWN irreducible outage floor (100% FER even at +18 dB,
//! deep multipath fades knocking out 64-QAM's dense constellation regardless
//! of SNR -- the same phenomenon as level 2's Watterson-Poor floor, just at a
//! higher-order modulation), so AWGN alone is used here to get a calibratable
//! operating point at all for this high-rate regime, which IS where IR-HARQ's
//! benefit is expected (and measured) to be clearest: less single-shot
//! redundancy means genuinely NEW parity from RV cycling matters more.
//!
//! Expectation (decision 3 / the task brief): mean transmissions-to-success
//! IR < Chase < Plain. Run with `cargo run -p coppa-bench --release --example
//! task3_harq_ir_bench`.

use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::arq::rv_for_attempt;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVEL: u8 = 10; // 64-QAM, rate 5/6 (least redundant code -- see task-3-report)
const SAMPLE_RATE: f32 = 48_000.0;
const CALIBRATION_TRIALS: usize = 150;
const MAIN_TRIALS: usize = 300;
const MAX_ATTEMPTS: u32 = 6;
/// Watterson-Poor is known (see `CLAUDE.md`'s "irreducible-outage-floor
/// channel" note) to floor well above 10% FER for level 2 at ANY SNR -- an
/// early wide calibration sweep confirmed this bench's own AWGN+fading
/// convention agrees (FER still >90% at +1.5 dB, dropping only ~1-2 points
/// per 0.5 dB). A literal 50% first-TX FER operating point isn't reachable
/// on Poor for this level, so this bench uses Watterson-MODERATE instead --
/// "a poor channel" in the sense the brief actually needs (a channel harsh
/// enough to produce substantial, SNR-tunable first-TX failure), not the
/// specific `WattersonPreset::Poor` enum variant.
const CHANNEL_PRESET: WattersonPreset = WattersonPreset::Moderate;

fn make_header(payload_len: u16, seq_num: u8, rv: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: rv, // low 2 bits = RV, Phase 3 Task 3 (see RV_MASK's doc)
        speed_level: LEVEL,
        seq_num,
        payload_len,
        codewords: 1,
    }
}

/// One transmission's channel: Watterson-Poor fading + AWGN at `snr_db`
/// (referenced to the clean signal's own power, per `awgn_ref_seeded`'s doc),
/// seeded so a given `(trial, attempt)` pair produces the IDENTICAL fading +
/// noise draw regardless of which strategy is calling it (a paired
/// comparison -- see this file's module doc).
fn corrupt(clean: &[f32], snr_db: f32, trial: u64, attempt: u32) -> Vec<f32> {
    let base = 0xC0FF_EE00_0000_0000u64
        .wrapping_add(trial.wrapping_mul(1_000))
        .wrapping_add(attempt as u64);
    let p_clean = coppa_channel::mean_power(clean);
    let faded = if USE_FADING {
        coppa_channel::watterson::watterson_preset(
            clean,
            SAMPLE_RATE,
            CHANNEL_PRESET,
            base ^ 0x3333_3333_3333_3333,
        )
    } else {
        clean.to_vec()
    };
    coppa_channel::awgn_ref_seeded(
        &faded,
        snr_db,
        p_clean,
        SAMPLE_RATE,
        base ^ 0x5555_5555_5555_5555,
    )
}

/// Whether [`corrupt`] applies Watterson fading on top of AWGN. `false` for
/// level 10 (64-QAM, rate 5/6): an early sweep found level 10 hits its own
/// irreducible Watterson-fading outage floor (100% FER even at +18 dB --
/// deep multipath fades knock out 64-QAM's dense constellation regardless of
/// AWGN SNR, the same phenomenon as level 2's Watterson-Poor floor, just at a
/// higher-order modulation), so this bench uses AWGN alone at level 10 to get
/// a calibratable ~50% first-TX operating point at all (still a "poor
/// channel" in the sense the brief needs: low SNR, substantial first-TX
/// failure -- just without fading specifically).
const USE_FADING: bool = false;

/// Measure the first-transmission-only (no retransmission) FER at `snr_db`,
/// to calibrate the ~50% first-TX operating point the bench brief asks for.
fn first_tx_fer(snr_db: f32, payload_bytes: usize, profile: &CoppaProfile) -> f64 {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let header = make_header(payload_bytes as u16, 0, 0);
    let mut failures = 0usize;
    for trial in 0..CALIBRATION_TRIALS {
        let seed = 0xCA11_0000u64.wrapping_add(trial as u64);
        let mut rng = StdRng::seed_from_u64(seed);
        let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
        let clean = tx
            .transmit(&header, &payload)
            .expect("payload within this level's capacity");
        let noisy = corrupt(&clean, snr_db, trial as u64, 0);
        let ok = matches!(tx.receive(&noisy), Ok((_, rx, _)) if rx[..payload.len()] == payload[..]);
        if !ok {
            failures += 1;
        }
        tx.harq_evict(0); // each calibration trial is its own independent seq-0 draw
    }
    failures as f64 / CALIBRATION_TRIALS as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strategy {
    Ir,
    Chase,
    Plain,
}

impl Strategy {
    fn rv_for(&self, attempt: u32) -> u8 {
        match self {
            Strategy::Ir => rv_for_attempt(attempt as u8),
            Strategy::Chase | Strategy::Plain => 0,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Strategy::Ir => "IR-HARQ",
            Strategy::Chase => "Chase-combining",
            Strategy::Plain => "plain (no combining)",
        }
    }
}

/// Simulate one trial's retransmission sequence for `strategy`, returning
/// `Some(attempts_used)` (1-indexed: 1 = succeeded on the first, original
/// transmission) if it succeeded within `MAX_ATTEMPTS`, `None` if it never
/// did.
fn simulate_trial(
    strategy: Strategy,
    profile: &CoppaProfile,
    payload_bytes: usize,
    snr_db: f32,
    trial: u64,
) -> Option<u32> {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    const SEQ: u8 = 0;
    let mut rng = StdRng::seed_from_u64(0xFEED_0000u64.wrapping_add(trial));
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();

    for attempt in 0..MAX_ATTEMPTS {
        let rv = strategy.rv_for(attempt);
        let header = make_header(payload.len() as u16, SEQ, rv);
        let clean = tx
            .transmit(&header, &payload)
            .expect("payload within this level's capacity");
        let noisy = corrupt(&clean, snr_db, trial, attempt);
        let ok = matches!(tx.receive(&noisy), Ok((_, rx, _)) if rx[..payload.len()] == payload[..]);

        if strategy == Strategy::Plain {
            // No combining: forget this attempt's LLRs regardless of outcome.
            tx.harq_evict(SEQ);
        }
        // IR/Chase: `receive` already evicts the buffer internally on a
        // CRC-passing decode (Task 3), so nothing extra is needed here on
        // success; on failure the accumulator is left in place so the next
        // attempt's `receive` call combines with it automatically.

        if ok {
            return Some(attempt + 1);
        }
    }
    None
}

fn main() {
    let profile = CoppaProfile::hf_standard();
    let payload_bytes = coppa_protocol::modem::speed_levels::max_payload_for_level(LEVEL)
        .expect("level 2 must exist");

    // 1. Calibrate: find the SNR closest to a 50% first-TX FER for this
    // (level, channel, payload) operating point. `HARQ_BENCH_SNR` (env var)
    // skips the sweep and re-verifies a previously-calibrated point directly
    // -- used to re-run just the main comparison without repeating an
    // already-completed sweep.
    let (best_snr, _fer_at_best) = if let Ok(v) = std::env::var("HARQ_BENCH_SNR") {
        let snr_db: f32 = v.parse().expect("HARQ_BENCH_SNR must be a float");
        let fer = first_tx_fer(snr_db, payload_bytes, &profile);
        println!(
            "Using pre-calibrated operating point: snr_db={snr_db:.1} (re-verified \
             first-TX FER={fer:.3}, target 0.500)\n"
        );
        (snr_db, fer)
    } else {
        println!(
            "Calibrating first-TX FER (level={LEVEL}, hf_standard, {}, \
             payload={payload_bytes}B, {CALIBRATION_TRIALS} trials/point)...",
            if USE_FADING {
                "watterson-moderate"
            } else {
                "AWGN-only"
            }
        );
        let mut best_snr = 0.0f32;
        let mut best_gap = f64::MAX;
        let mut fer_at_best = 0.0;
        for snr_step in 0..=20 {
            let snr_db = snr_step as f32 * 2.0; // 0.0 .. 40.0 dB in 2 dB steps
            let fer = first_tx_fer(snr_db, payload_bytes, &profile);
            let gap = (fer - 0.5).abs();
            println!("  snr_db={snr_db:.1} first_tx_fer={fer:.3}");
            if gap < best_gap {
                best_gap = gap;
                best_snr = snr_db;
                fer_at_best = fer;
            }
        }
        println!(
            "Calibrated operating point: snr_db={best_snr:.1} (first-TX FER={fer_at_best:.3}, \
             target 0.500)\n"
        );
        (best_snr, fer_at_best)
    };

    // 2. Main measurement: mean transmissions-to-success per strategy.
    for strategy in [Strategy::Ir, Strategy::Chase, Strategy::Plain] {
        let mut attempts_on_success = Vec::with_capacity(MAIN_TRIALS);
        let mut gave_up = 0usize;
        for trial in 0..MAIN_TRIALS as u64 {
            match simulate_trial(strategy, &profile, payload_bytes, best_snr, trial) {
                Some(n) => attempts_on_success.push(n as f64),
                None => gave_up += 1,
            }
        }
        let n_success = attempts_on_success.len();
        let mean_on_success = if n_success > 0 {
            attempts_on_success.iter().sum::<f64>() / n_success as f64
        } else {
            f64::NAN
        };
        // Censored mean: trials that never succeeded within MAX_ATTEMPTS
        // count as having used the full budget -- a fair aggregate that
        // doesn't let a strategy look artificially good by simply excluding
        // its worst trials.
        let censored_sum: f64 =
            attempts_on_success.iter().sum::<f64>() + (gave_up as f64) * MAX_ATTEMPTS as f64;
        let censored_mean = censored_sum / MAIN_TRIALS as f64;

        println!(
            "{:<22} success={}/{} ({:.1}%)  mean_transmissions_to_success={:.3}  \
             censored_mean(incl. give-ups @ {}) ={:.3}",
            strategy.label(),
            n_success,
            MAIN_TRIALS,
            100.0 * n_success as f64 / MAIN_TRIALS as f64,
            mean_on_success,
            MAX_ATTEMPTS,
            censored_mean,
        );
    }
}
