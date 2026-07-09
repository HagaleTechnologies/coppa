//! Closed-loop adaptive-rate validation. Runs the sender's `RateLoop` over a time-varying channel:
//! each frame is transmitted at the loop's current level, passed through the scheduled channel, and
//! decoded; the receiver's recommended level (the third element `CoppaTransceiver::receive` now
//! returns, Phase 3 Task 4) plus the delivery outcome drive the loop. Reports adaptive throughput vs
//! the best single fixed level vs a per-frame oracle, plus a level-vs-channel trace, to show the
//! link tracks the channel without thrashing.
//!
//! Throughput metric: total correctly-delivered info bits over N frame slots (all runs use the same
//! N slots and schedule, so this is a fair relative comparison; it deliberately ignores per-level
//! airtime differences).
//!
//! ## Honest result: this bench does NOT clear the plan's acceptance bar (adaptive/best-fixed > 1.0,
//! adaptive/oracle >= 0.8), and no `raise_dwell` value fixes that
//!
//! A sweep of `raise_dwell` (3/4/5/6/8/10/12/15) peaks at **5** (adaptive/best-fixed = 0.894,
//! adaptive/oracle = 0.751 -- the numbers `RateLoop::default_coppa()` now uses) and gets WORSE on
//! both sides of that peak, so this isn't a case of "needs a bit more damping": more dwell keeps
//! failing to converge on the same shape of problem.
//!
//! Root cause, from an ad-hoc diagnostic (temporary probes, not committed -- see caveat below):
//! `coppa_ml::recommend_speed_level`'s underlying capacity metric (`channel_capacity`/`noise_vars`,
//! from `CoppaTransceiver::receive_with_metrics`) appears NOT invariant to which speed level the
//! frame being measured happened to use -- at a fixed TRUE injected AWGN SNR, measuring via a
//! level-1 transmission read meaningfully lower "capacity" than measuring via a level-7
//! transmission (30-seed averages, both `hf_standard` and `hf_robust`, no fading at all). This
//! measurement was NOT committed as a reproducible bench/test -- treat it as a well-reasoned
//! hypothesis consistent with the evidence below, not as independently-verifiable fact, until a
//! committed diagnostic exists. But `SPEED_LEVEL_MIN_CAPACITY`
//! (the calibration table this recommendation is looked up in) was calibrated exclusively via a
//! FIXED level-2 probe frame (see `mcs_calibration.rs`/`adaptive_mcs_validation.rs`'s
//! `sound_capacity`, which always transmits at `mode_for_level(2)` regardless of the level being
//! evaluated) -- those benches never expose this level-dependence because they never vary the
//! probing level. This bench (and `CoppaTransceiver::receive()`'s real, shipped recommendation) DO
//! vary it, by design (measuring the actual in-flight frame, not a separate probe), which is exactly
//! what exposes a self-reinforcing bias: the higher the current level climbs, the more inflated its
//! own capacity reading becomes, so the loop keeps getting told to climb further regardless of the
//! real channel. If this hypothesis holds, it would be a pre-existing property of the shared
//! channel-estimation/capacity layer (consistent with the still-open channel-estimation
//! limitation in CLAUDE.md's Known Limitations), not a bug introduced by this bench or by
//! `RateLoop`'s hysteresis logic -- and fixing
//! it is out of this task's scope (it would mean either a level-invariant capacity metric or a
//! per-level-recalibrated threshold table, both belonging to the channel-estimation/MCS-calibration
//! work, not the rate-loop controller). See `.superpowers/sdd/task-4-report.md` for the full
//! measured evidence and reasoning.

use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{RateLoop, VALID_SPEED_LEVELS};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const N_FRAMES: usize = 300;

/// `seq` MUST vary per simulated frame (wrapping mod 256, matching real link seq numbering) --
/// NOT held at a constant 0. `CoppaTransceiver` now does IR-HARQ combining (Phase 3 Task 3):
/// coded LLRs are accumulated per `seq_num` across `receive()` calls until a CRC pass evicts the
/// buffer. A constant seq across logically-independent frames would make every subsequent frame's
/// LLRs combine into the previous (unrelated) frame's leftover accumulator on any decode failure,
/// corrupting every following attempt at that seq -- exactly the failure mode this bench hit before
/// this was fixed (levels 9/10 measured 0/300 successes even at a clean 30 dB AWGN point that a
/// fresh transceiver decodes reliably).
fn make_header(level: u8, len: u16, seq: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: seq,
        payload_len: len,
        codewords: 1,
    }
}

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

/// Time-varying channel schedule: AWGN SNR ramp up, ramp down, then Good then Poor fading.
fn schedule(f: usize) -> (ChannelSpec, f32) {
    let q = N_FRAMES / 3;
    if f < q {
        (ChannelSpec::Awgn, 3.0 + 27.0 * f as f32 / q as f32)
    } else if f < 2 * q {
        (ChannelSpec::Awgn, 30.0 - 27.0 * (f - q) as f32 / q as f32)
    } else if f < 2 * q + q / 2 {
        (ChannelSpec::Watterson(WattersonPreset::Good), 24.0)
    } else {
        (ChannelSpec::Watterson(WattersonPreset::Poor), 24.0)
    }
}

/// Transmit one known frame at `level` through the scheduled channel; return
/// (delivered_correctly, recommended_level_from_rx). The recommended level is
/// `receive()`'s own third tuple element (Phase 3 Task 4:
/// `coppa_ml::recommend_speed_level` over this frame's per-carrier noise vars) --
/// this bench doesn't recompute it separately, it just plumbs the real signal through.
fn run_frame(tx: &CoppaTransceiver, level: u8, f: usize) -> (bool, Option<u8>) {
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let payload: Vec<u8> = (0..pfb)
        .map(|i| (i as u64 * 0x9E37 + f as u64) as u8)
        .collect();
    let seq = (f % 256) as u8;
    let sig = tx
        .transmit(&make_header(level, pfb as u16, seq), &payload)
        .expect("payload sized from this level's own payload_bytes() always fits");
    let (ch, snr) = schedule(f);
    let faded = apply_channel(&sig, ch, snr, f as u64);
    match tx.receive(&faded) {
        Ok((_h, p, rec)) if p.len() >= pfb && p[..pfb] == payload[..] => (true, Some(rec)),
        Ok((_h, _p, rec)) => (false, Some(rec)),
        Err(_) => (false, None),
    }
}

fn info_bits(level: u8) -> usize {
    mode_for_level(level).unwrap().payload_bytes() * 8
}

fn main() {
    let profile = profile_by_name("robust").unwrap();

    // --- Fixed-level runs (for best-fixed and per-frame oracle) ---
    // Each run gets its OWN transceiver: seq numbers wrap mod 256 within a run's 300 frames
    // (necessary so IR-HARQ's per-seq LLR accumulator, Phase 3 Task 3, treats each of these
    // logically-independent frames independently -- see `make_header`'s doc), but sharing one
    // transceiver *across* runs would let a run boundary's seq-0 reuse combine into another
    // run's (or the adaptive run's) leftover accumulator from a different level entirely.
    let mut fixed_delivered: Vec<Vec<bool>> = Vec::new(); // [level_idx][frame]
    let mut fixed_bits = vec![0usize; VALID_SPEED_LEVELS.len()];
    for (li, &lvl) in VALID_SPEED_LEVELS.iter().enumerate() {
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let mut deliv = vec![false; N_FRAMES];
        for (f, slot) in deliv.iter_mut().enumerate() {
            let (ok, _rec) = run_frame(&tx, lvl, f);
            *slot = ok;
            if ok {
                fixed_bits[li] += info_bits(lvl);
            }
        }
        fixed_delivered.push(deliv);
        eprintln!(
            "fixed run {}/{}: L{} -> {} bits",
            li + 1,
            VALID_SPEED_LEVELS.len(),
            lvl,
            fixed_bits[li]
        );
    }
    let best_fixed = *fixed_bits.iter().max().unwrap();
    let best_fixed_level = VALID_SPEED_LEVELS[fixed_bits
        .iter()
        .enumerate()
        .max_by_key(|(_, b)| **b)
        .unwrap()
        .0];

    // Per-frame oracle: best delivered bits achievable at that frame across levels.
    // (Indexes `fixed_delivered[li]` by frame `f` across all levels per frame -- a transpose --
    // so a plain range loop over `f` is clearer here than an iterator adapter.)
    let mut oracle_bits = 0usize;
    #[allow(clippy::needless_range_loop)]
    for f in 0..N_FRAMES {
        let mut best = 0usize;
        for (li, &lvl) in VALID_SPEED_LEVELS.iter().enumerate() {
            if fixed_delivered[li][f] {
                best = best.max(info_bits(lvl));
            }
        }
        oracle_bits += best;
    }

    // --- Adaptive closed-loop run --- (its own fresh transceiver, same reasoning as above)
    let tx = CoppaTransceiver::new(profile, 1);
    let mut loop_ctl = RateLoop::default_coppa();
    let mut adaptive_bits = 0usize;
    let mut trace: Vec<(usize, f32, u8)> = Vec::new();
    for f in 0..N_FRAMES {
        let level = loop_ctl.current_level();
        let (ok, rec) = run_frame(&tx, level, f);
        if ok {
            adaptive_bits += info_bits(level);
        }
        match rec {
            Some(r) => loop_ctl.on_ack(r, ok),
            None => loop_ctl.on_timeout(),
        }
        let (_ch, snr) = schedule(f);
        if f % 15 == 0 {
            trace.push((f, snr, level));
        }
    }

    // --- Report ---
    println!("=== Closed-loop adaptive rate ({N_FRAMES} frames, robust profile) ===");
    println!("adaptive throughput : {adaptive_bits} bits");
    println!("best fixed (L{best_fixed_level})     : {best_fixed} bits");
    println!("per-frame oracle    : {oracle_bits} bits");
    println!(
        "adaptive/oracle = {:.3}   adaptive/best-fixed = {:.3}",
        adaptive_bits as f64 / oracle_bits.max(1) as f64,
        adaptive_bits as f64 / best_fixed.max(1) as f64,
    );
    println!("\n frame   snr(dB)  level   (level tracks the channel; no steady-state thrash)");
    for (f, snr, lvl) in &trace {
        println!("  {f:4}    {snr:5.1}     L{lvl}");
    }
}
