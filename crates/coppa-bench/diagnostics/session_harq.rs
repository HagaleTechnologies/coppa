//! COP-186 `session_harq`: the per-level retransmission-rescue measurement
//! that fills `coppa_protocol::arq::RV_SCHEDULE`.
//!
//! Method (COP-168 plan, Implementation Approach item 4 / Phase 2, binding;
//! helpers in `coppa_bench::session_harq`):
//!
//! 1. Session-like frames: a random payload of `max_multi_payload_for_level(
//!    level, cw) - 21` bytes, 1200 zero samples appended after the frame,
//!    Watterson-Moderate fading then AWGN at the HF 3 kHz noise convention,
//!    SNR referenced to the clean frame's own power, a fresh channel seed per
//!    physical transmission.
//! 2. Per HF level 1-8, calibrate the SNR (0.5 dB steps) where single-frame
//!    FER is 25-35%; the curve is printed (a coarse 2 dB pre-pass brackets
//!    the waterfall so the 0.5 dB pass only measures the interesting window).
//! 3. Up to `SESSION_HARQ_MAX_TRIALS` trials per level, stopping at
//!    `SESSION_HARQ_TARGET_FAILURES` failed RV0 frames. Every failure runs
//!    paired arms over identical channel draws: Chase (RV0 again, combined)
//!    and each of the six orders of {1,2,3} for retransmissions 1-3.
//! 4. Per level and arm: rescues after retransmission 1, 2 and 3, Wilson 95%
//!    intervals, and discordant pairs against Chase.
//! 5. One level-4 row with 8-codeword frames. Multi-codeword receive has no
//!    cross-attempt IR combining (Phase 3 Task 5 scope cut, ADR-007), so that
//!    row's "rescues" are retransmissions that decode stand-alone; it is
//!    recorded for the transmit follow-up, not fed to the selection rule.
//! 6. `std::thread::scope` over levels.
//!
//! Wall-time control: `SESSION_HARQ_CAL_TRIALS` (default 400),
//! `SESSION_HARQ_MAX_TRIALS` (default 2000), `SESSION_HARQ_TARGET_FAILURES`
//! (default 400) and `SESSION_HARQ_BUDGET_SECS` (default none; a per-level
//! soft deadline for the main loop) can shrink the run. Whatever counts are
//! achieved are printed and must be pasted into `BENCHMARKS.md` with the
//! chosen rows; never ship a row without the recorded measurement behind it.
//!
//! Run: `cargo run --release -p coppa-bench --example session_harq`.

use coppa_bench::metrics::wilson_ci95;
use coppa_bench::session_harq::{
    channel_seed, frame_power, payload_matches, select_row, session_channel, session_header,
    session_payload, ORDERS,
};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use std::time::{Duration, Instant};

const LEVELS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const PRESET: WattersonPreset = WattersonPreset::Moderate;

/// Chase arm first, then the six orders of {1,2,3} (ORDERS listing order).
/// `arm_rv(0, k) == 0` (Chase repeats RV0); `arm_rv(i, k) == ORDERS[i-1][k-1]`.
fn arm_rv(arm: usize, retransmission: u32) -> u8 {
    if arm == 0 {
        0
    } else {
        ORDERS[arm - 1][(retransmission - 1) as usize]
    }
}

fn arm_label(arm: usize) -> String {
    if arm == 0 {
        "Chase (RV0,0,0)".to_string()
    } else {
        let o = ORDERS[arm - 1];
        format!("RV order {}/{}/{}", o[0], o[1], o[2])
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .max(1)
}

/// Single-frame FER at `snr_db` over `trials` fresh draws (calibration).
fn first_tx_fer(
    tx: &CoppaTransceiver,
    level: u8,
    codewords: u8,
    snr_db: f32,
    trials: usize,
) -> f64 {
    let mut failures = 0usize;
    for trial in 0..trials {
        let payload = session_payload(level, codewords, 0xCA11_0000 + trial as u64);
        let clean = tx
            .transmit(
                &session_header(level, codewords, 0, 0, payload.len()),
                &payload,
            )
            .expect("payload within capacity");
        let p = frame_power(&clean);
        let noisy = session_channel(
            &clean,
            Some(PRESET),
            snr_db,
            p,
            channel_seed(trial as u64, 0),
        );
        let ok = payload_matches(&tx.receive(&noisy), &payload);
        tx.harq_evict(0);
        if !ok {
            failures += 1;
        }
    }
    failures as f64 / trials as f64
}

/// Calibrate the operating SNR: coarse 2 dB pass to bracket ~30% FER, then
/// 0.5 dB steps inside the bracket, `cal_trials` trials per point. Prints
/// every measured point. A level whose Watterson-Moderate FER never drops to
/// 0.20 (the fading outage floor documented for high levels in
/// `docs/LIMITATIONS.md`'s Phase 3 Task 8 bullet) is measured at the floor's
/// best coarse point with a three-point refinement, and flagged. Returns
/// (snr_db, fer, points_measured, floored).
fn calibrate(
    tx: &CoppaTransceiver,
    level: u8,
    codewords: u8,
    cal_trials: usize,
    deadline: Option<Instant>,
) -> (f32, f64, usize, bool) {
    const COARSE_TRIALS: usize = 16;
    const MAX_SNR_DB: f32 = 44.0;
    println!("  calibration: coarse 2 dB pass, {COARSE_TRIALS} trials/point ...");
    let mut points = 0usize;
    let mut coarse = Vec::new();
    let mut bracketed = false;
    let mut step = 0i32;
    while (step as f32) * 2.0 <= MAX_SNR_DB {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let snr = (step as f32) * 2.0;
        let fer = first_tx_fer(tx, level, codewords, snr, COARSE_TRIALS);
        points += 1;
        println!("    snr_db={snr:>5.1} first_tx_fer={fer:.3} (coarse)");
        coarse.push((snr, fer));
        if fer <= 0.20 {
            // Safely past the waterfall; the 25-35% window is behind us.
            bracketed = true;
            break;
        }
        step += 1;
    }
    let mut best = *coarse
        .iter()
        .min_by(|a, b| {
            (a.1 - 0.30)
                .abs()
                .partial_cmp(&(b.1 - 0.30).abs())
                .expect("FERs are finite")
        })
        .expect("at least one coarse point");
    let floored = !bracketed;
    if bracketed {
        // Fine 0.5 dB pass over the window that straddles 30%.
        let lo = coarse
            .iter()
            .rev()
            .find(|(_, f)| *f >= 0.40)
            .map(|(s, _)| *s)
            .unwrap_or(coarse[0].0);
        let hi = coarse
            .iter()
            .find(|(_, f)| *f <= 0.20)
            .map(|(s, _)| *s)
            .unwrap_or(coarse.last().expect("non-empty").0);
        println!(
            "  calibration: fine 0.5 dB pass over [{lo:.1}, {hi:.1}] dB, \
             {cal_trials} trials/point ...",
        );
        let mut half_db = lo;
        while half_db <= hi + f32::EPSILON {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let fer = first_tx_fer(tx, level, codewords, half_db, cal_trials);
            points += 1;
            println!("    snr_db={half_db:>5.1} first_tx_fer={fer:.3}");
            if (fer - 0.30).abs() < (best.1 - 0.30).abs() {
                best = (half_db, fer);
            }
            half_db += 0.5;
        }
    } else {
        // Outage floor: refine around the best coarse point only.
        println!(
            "  calibration: no SNR in [0, {MAX_SNR_DB}] dB reaches FER<=0.20 -- \
             Watterson-Moderate outage floor; refining around the best coarse point",
        );
        let mut half_db = (best.0 - 1.0).max(0.0);
        while half_db <= best.0 + 1.0 {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let fer = first_tx_fer(tx, level, codewords, half_db, cal_trials);
            points += 1;
            println!("    snr_db={half_db:>5.1} first_tx_fer={fer:.3}");
            if (fer - 0.30).abs() < (best.1 - 0.30).abs() {
                best = (half_db, fer);
            }
            half_db += 0.5;
        }
    }
    println!(
        "  calibrated operating point: snr_db={:.1} first_tx_fer={:.3} \
         (target 0.30, window 0.25-0.35{})",
        best.0,
        best.1,
        if floored {
            ", FLOORED: window unreachable"
        } else {
            ""
        },
    );
    (best.0, best.1, points, floored)
}

/// One failed RV0 frame replayed through one arm: receive the (already
/// corrupted) RV0 samples again, then up to 3 retransmissions on the paired
/// draws `(trial, 1..=3)`. Returns which retransmission first rescued the
/// frame (1, 2 or 3), or None.
fn run_arm(
    tx: &CoppaTransceiver,
    level: u8,
    codewords: u8,
    arm: usize,
    payload: &[u8],
    noisy0: &[f32],
    p_clean: f32,
    snr_db: f32,
    trial: u64,
) -> Option<u32> {
    // Replay the failed RV0 receive so the arm's soft buffer starts from the
    // identical RV0 contribution every other arm sees.
    let ok0 = payload_matches(&tx.receive(noisy0), payload);
    debug_assert!(!ok0, "screening pass already failed this RV0 frame");
    for retransmission in 1..=3u32 {
        let rv = arm_rv(arm, retransmission);
        let clean = tx
            .transmit(
                &session_header(level, codewords, 0, rv, payload.len()),
                payload,
            )
            .expect("retransmission within capacity");
        let noisy = session_channel(
            &clean,
            Some(PRESET),
            snr_db,
            p_clean,
            channel_seed(trial, retransmission),
        );
        if payload_matches(&tx.receive(&noisy), payload) {
            return Some(retransmission);
        }
    }
    None
}

/// Per-level (or per-cw8-row) result block.
struct LevelResult {
    level: u8,
    codewords: u8,
    snr_db: f32,
    cal_fer: f64,
    cal_points: usize,
    cal_trials: usize,
    floored: bool,
    trials: usize,
    failures: usize,
    /// Rescue step per arm per failure: arms x failures, Some(step).
    rescued: Vec<Vec<Option<u32>>>,
    stopped_early: bool,
    elapsed: Duration,
}

impl LevelResult {
    fn print(&self) {
        println!(
            "\n== level {} (codewords={}, watterson {:?}) snr={:.1} dB \
             (calibrated FER {:.3}, {} calibration points x {} trials{})",
            self.level,
            self.codewords,
            PRESET,
            self.snr_db,
            self.cal_fer,
            self.cal_points,
            self.cal_trials,
            if self.floored {
                ", Watterson outage floor"
            } else {
                ""
            },
        );
        println!(
            "   trials={} failed_RV0={}{} elapsed={:.1}s",
            self.trials,
            self.failures,
            if self.stopped_early {
                " (stopped early on budget)"
            } else {
                ""
            },
            self.elapsed.as_secs_f64(),
        );
        println!(
            "   {:<16} rescue<=1   Wilson95      rescue<=2   rescue<=3   \
             discordant vs Chase (b/c)",
            "arm",
        );
        let chase_steps: Vec<Option<u32>> = self.rescued.first().cloned().unwrap_or_default();
        for (arm, steps) in self.rescued.iter().enumerate() {
            let r1 = steps.iter().filter(|s| s.is_some_and(|v| v <= 1)).count();
            let r2 = steps.iter().filter(|s| s.is_some_and(|v| v <= 2)).count();
            let r3 = steps.iter().filter(|s| s.is_some()).count();
            let (w1lo, w1hi) = wilson_ci95(r1, self.failures);
            // Discordant pairs per retransmission step k: arm rescued by k
            // while Chase was not, and vice versa (cumulative to that step).
            let disc: Vec<(usize, usize)> = (1..=3u32)
                .map(|k| {
                    let a = |v: &Option<u32>| v.is_some_and(|x| x <= k);
                    let (mut bc, mut cb) = (0, 0);
                    for (x, y) in steps.iter().zip(chase_steps.iter()) {
                        match (a(x), a(y)) {
                            (true, false) => bc += 1,
                            (false, true) => cb += 1,
                            _ => {}
                        }
                    }
                    (bc, cb)
                })
                .collect();
            println!(
                "   {:<16} {:>4}/{}  [{:.3},{:.3}]  {:>6}      {:>6}      \
                 k1 {}/{}  k2 {}/{}  k3 {}/{}",
                arm_label(arm),
                r1,
                self.failures,
                w1lo,
                w1hi,
                r2,
                r3,
                disc[0].0,
                disc[0].1,
                disc[1].0,
                disc[1].1,
                disc[2].0,
                disc[2].1,
            );
        }
    }

    /// Apply the selection rule (single-codeword rows only) and print it.
    fn print_selected_row(&self) {
        let mut r1 = [0usize; 6];
        let mut r2 = [0usize; 6];
        for (i, steps) in self.rescued.iter().skip(1).enumerate() {
            r1[i] = steps.iter().filter(|s| s.is_some_and(|v| v <= 1)).count();
            r2[i] = steps.iter().filter(|s| s.is_some_and(|v| v <= 2)).count();
        }
        let row = select_row(&r1, &r2);
        println!(
            "   selected row for level {}: {:?}  (rule: best rescue<=1; tie -> \
             rescue<=2; tie -> closest to 3GPP [0,2,3,1])",
            self.level, row
        );
    }
}

fn measure_level(
    level: u8,
    codewords: u8,
    cal_trials: usize,
    max_trials: usize,
    target_failures: usize,
    budget: Option<Duration>,
) -> LevelResult {
    let started = Instant::now();
    let deadline = budget.map(|b| started + b);
    let profile = CoppaProfile::hf_standard();
    let cal_tx = CoppaTransceiver::new(profile.clone(), 1);
    let (snr_db, cal_fer, cal_points, floored) =
        calibrate(&cal_tx, level, codewords, cal_trials, deadline);
    drop(cal_tx);

    // One transceiver per arm (plus the screener): buffers are evicted after
    // every trial so arms never inherit foreign LLRs.
    let arm_txs: Vec<CoppaTransceiver> = (0..=ORDERS.len())
        .map(|_| CoppaTransceiver::new(profile.clone(), 1))
        .collect();
    let screen_tx = CoppaTransceiver::new(profile, 1);
    let mut rescued: Vec<Vec<Option<u32>>> = vec![Vec::new(); ORDERS.len() + 1];
    let mut trials = 0usize;
    let mut stopped_early = false;
    while trials < max_trials && rescued[0].len() < target_failures {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                stopped_early = true;
                break;
            }
        }
        let trial = trials as u64;
        let payload = session_payload(level, codewords, 0xFEED_0000 + trial);
        let clean0 = screen_tx
            .transmit(
                &session_header(level, codewords, 0, 0, payload.len()),
                &payload,
            )
            .expect("payload within capacity");
        let p = frame_power(&clean0);
        let noisy0 = session_channel(&clean0, Some(PRESET), snr_db, p, channel_seed(trial, 0));
        let ok0 = payload_matches(&screen_tx.receive(&noisy0), &payload);
        screen_tx.harq_evict(0);
        trials += 1;
        if ok0 {
            continue;
        }
        for (arm, tx) in arm_txs.iter().enumerate() {
            let step = run_arm(
                tx, level, codewords, arm, &payload, &noisy0, p, snr_db, trial,
            );
            rescued[arm].push(step);
            tx.harq_evict(0);
        }
    }
    LevelResult {
        level,
        codewords,
        snr_db,
        cal_fer,
        cal_points,
        cal_trials,
        floored,
        trials,
        failures: rescued[0].len(),
        rescued,
        stopped_early,
        elapsed: started.elapsed(),
    }
}

fn main() {
    let cal_trials = env_usize("SESSION_HARQ_CAL_TRIALS", 400);
    let max_trials = env_usize("SESSION_HARQ_MAX_TRIALS", 2000);
    let target_failures = env_usize("SESSION_HARQ_TARGET_FAILURES", 400);
    let budget = std::env::var("SESSION_HARQ_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);
    println!(
        "session_harq (COP-186): targets cal_trials={cal_trials} max_trials={max_trials} \
         target_failures={target_failures} budget={:?}",
        budget.map(|b| b.as_secs_f64()),
    );
    println!(
        "channel: Watterson {:?} + AWGN (3 kHz reference), fresh seed per TX",
        PRESET
    );

    // 6. thread::scope over levels (plus the level-4 cw8 row).
    let jobs: Vec<(u8, u8)> = LEVELS
        .iter()
        .map(|&l| (l, 1))
        .chain(std::iter::once((4, 8)))
        .collect();
    let mut results: Vec<Option<LevelResult>> = jobs.iter().map(|_| None).collect();
    let run_started = Instant::now();
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .zip(results.iter_mut())
            .map(|(&(level, cw), slot)| {
                scope.spawn(move || {
                    let r = measure_level(
                        level,
                        cw,
                        cal_trials,
                        if cw > 1 {
                            max_trials.min(600)
                        } else {
                            max_trials
                        },
                        if cw > 1 {
                            target_failures.min(80)
                        } else {
                            target_failures
                        },
                        // The 8-codeword row gets double budget: its frames
                        // cost ~8x a single codeword to encode and decode.
                        budget.map(|b| if cw > 1 { b * 2 } else { b }),
                    );
                    *slot = Some(r);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("level measurement thread panicked");
        }
    });
    println!(
        "\n===== session_harq full table (total {:.1}s) =====",
        run_started.elapsed().as_secs_f64()
    );
    for r in results.into_iter().flatten() {
        r.print();
        if r.codewords == 1 {
            r.print_selected_row();
        } else {
            println!(
                "   (informational row for the transmit follow-up: cw>1 receive has \
                 no cross-attempt IR combining, so these rescues decode stand-alone)"
            );
        }
    }
    println!(
        "\nFill arq::RV_SCHEDULE rows 1-8 from the selected rows above; row 9 copies \
         row 8 (VHF-only level, not measured here)."
    );
}
