//! Closed-loop adaptive-rate validation, scored on airtime-normalized goodput. Runs the sender's
//! `RateLoop` over a time-varying channel: each frame is transmitted at the loop's current level,
//! passed through the scheduled channel, and decoded; the receiver's recommended level (the fourth
//! element `CoppaTransceiver::receive_with_metrics` returns, Phase 3 Task 4) plus the delivery
//! outcome drive the loop. Reports adaptive goodput vs the best single fixed configuration vs a
//! per-frame oracle, plus a level-vs-channel trace, to show the link tracks the channel without
//! thrashing.
//!
//! # Metric (COP-2): airtime-normalized goodput, not bits per frame slot
//!
//! ```text
//! goodput_bps = sum(info_bits over DELIVERED frames)
//!               / sum(frame_airtime_s over ALL TRANSMITTED frames)
//! ```
//!
//! This is the project's canonical convention (`BENCHMARKS.md`'s `payload_bits * (1 - FER) /
//! frame_airtime`), and the arithmetic lives in `coppa_bench::adaptive_goodput` -- a lib module, not
//! this file, because **CI compiles bench examples but never runs them**, so anything left inside
//! `main()` is invisible to `cargo test --workspace`.
//!
//! Before COP-2 this bench scored delivered info bits per frame **slot** and said so ("it
//! deliberately ignores per-level airtime differences"). That was the single exception to the repo's
//! own convention, and it mattered: a higher speed level -- and a shorter cyclic prefix -- occupies
//! less air, which a per-slot count cannot see. The denominator counts undelivered frames because
//! they still occupy the channel; for a fixed-level arm the expression reduces algebraically to
//! `info_bits * (1 - FER) / frame_airtime_s`, i.e. the canonical form exactly.
//!
//! ## NON-COMPARABILITY LABEL (COP-2) -- read before reusing any number quoted below
//!
//! **Every ratio and bit-count in the historical narrative sections that follow (0.894/0.751,
//! 0.801/0.667, 0.931/0.775, 0.914/0.762, and the 326680 / 357424 / 428936 bit totals) is
//! NON-COMPARABLE to this bench's current primary output -- non-comparable, NOT superseded.** The
//! narrative is retained verbatim because it is the real history of this bench's shortfall and it is
//! still the best account of *why* the gap exists. Two independent reasons it cannot be compared
//! against the new figures, both of which must travel with these numbers wherever they are requoted:
//!
//! 1. **Metric.** Every figure below counts bits per frame *slot*. The primary metric is now bits per
//!    *second*. The change alone moves `best_fixed` from L4 to L7 on `hf_robust` (a ~+45.8%
//!    denominator) with zero CP involvement, so a CP delta must NEVER be reported against the
//!    pre-change 0.914/0.762.
//! 2. **Profile base.** The CP-contrast arms run on the `hf_standard` family, the only exact CP-only
//!    profile pair in the workspace (see `coppa_bench::cp_arm::profile_for`). Every figure below was
//!    measured on `hf_robust`, whose 36 data / 12 pilot layout differs in more than CP length and
//!    which has no short-CP twin anywhere.
//!
//! The legacy bits-per-slot block is retained beside the primary table as a labeled **refactor
//! control**: on the default `robust` base at seed 0 it reproduces the recorded 326680 / 357424 (L4)
//! / 428936 and 0.914 / 0.762 exactly, which is what proves the metric swap changed accounting only.
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
//!
//! ## 2026-07-25 update: active overshoot probing closes most of the remaining gap, still short of
//! the bar
//!
//! Several follow-up investigations (level-bias correction, hysteresis tuning, a probe-level-accuracy
//! diagnosis -- see `CLAUDE.md`'s RateLoop bullet for the full history) converged on: the passive
//! per-frame recommendation is itself a weak same-frame predictor of what a frame could actually
//! support when measured via a low-order probe, and that accuracy scales with the probing modulation
//! order. This bench now demonstrates `RateLoop`'s active overshoot probing
//! (`with_probing`/`level_for_next_transmission`/`on_probe_result`): periodically transmit above the
//! current level on purpose to get real ground truth instead of a noisy passive read. An exhaustive
//! sweep of `(probe_interval, probe_offset)` (plus `raise_dwell`, plus a rejected slow-start growing
//! offset variant, plus stall-gating -- which measurably helped and is now permanent behavior) found
//! `probe_interval=2, probe_offset=1` the peak: **adaptive/best-fixed = 0.931, adaptive/oracle =
//! 0.775** at the unchanged `raise_dwell=5` default -- clearly better than both this bench's prior
//! `main` state (0.801/0.667) and the historical pre-level-bias-correction baseline (0.894/0.751),
//! but still short of the plan's `>1.0`/`>=0.8` bar. Multiple refinements (dwell tuning, slow-start,
//! stall-gating) all converged on the same ~0.78/~0.93 plateau, suggesting this is a real ceiling for
//! this family of designs on this bench, not a remaining tuning gap. Shipped anyway per the same
//! honest-partial-progress precedent as the level-bias correction (PR #53): real, verified,
//! substantial improvement, gap documented rather than hidden. See
//! `docs/superpowers/specs/2026-07-25-rateloop-active-overshoot-probing-design.md`'s "Outcome"
//! section for the full sweep data.
//!
//! # The ratio-invariance result (COP-2 Phase 1): why an airtime lever cannot move this bar
//!
//! `frame_airtime_s` factorizes exactly as `total_syms(level, profile) * (fft_size + cp_samples) /
//! sample_rate`, and `total_syms` depends on the profile only through `data_carriers_per_symbol` --
//! identical for `hf_standard` and `hf_standard_short_cp` (both 44 data / 4 pilot). So a uniform CP
//! change is **one constant divisor, independent of the level sequence**: short/long airtime is
//! `1104/1260 = 0.876190...` at every single level (pinned by `coppa_protocol::modem::airtime`'s
//! `short_cp_scales_frame_airtime_by_one_constant_at_every_level`).
//!
//! Consequence, and it is arithmetic rather than a prediction: **`adaptive/best-fixed` and
//! `adaptive/oracle` can only move through FER or `RateLoop`-trajectory differences, never through
//! the airtime saving itself**, once the comparator set is allowed the same lever. A short-CP arm
//! scored against a comparator *denied* short CP gains a spurious `1260/1104 = +14.13%` -- which
//! would move 0.914 -> ~1.043 and 0.762 -> ~0.870 and *falsely clear both bars on accounting alone*.
//! For scale: the gap from 0.914 to the `> 1.0` bar is +9.41%, and from 0.762 to `>= 0.8` is +4.99%,
//! so the accounting artifact alone is ~1.5x and ~2.8x the required move. `BENCHMARKS.md` already
//! observed the same thing from the other direction: the fixed-profile short-CP bench's AWGN
//! +14.1%/+14.2% figures are "the harness behaving as expected, not a coherence-time result."
//!
//! That is why the acceptance ratio is reported against the **joint 18-cell (level x CP)** comparator
//! set (COP-2 decision D4), with the 9-cell long-CP-only set alongside for continuity only, and why
//! the airtime lever's real magnitude is reported where it actually appears: as the **denominator
//! delta** in absolute bps (D4a).
//!
//! # Arms, comparators, and what the switch cost does and does not charge
//!
//! Arms (all five appear as their own row in the report):
//!
//! - **A -- long-CP control.** One fixed profile (`CpMode::LongCp`) for the whole run, adaptive rate.
//!   The baseline every other arm is read against.
//! - **P -- rebuild placebo.** Identical to A, but rebuilds the transceiver on exactly the frames
//!   arm B switched on, with the *same* profile. A CP switch here is a transceiver **rebuild** (there
//!   is no profile setter -- `CoppaTransceiver::new` takes the profile by value), and this repo has
//!   twice shipped bugs that were invisible to inspection and only showed up as state carried across
//!   a rebuild boundary (the `runner.rs` stale-HARQ poisoning behind PR #61/#62, the VHF
//!   `calibrated_bias` saturation behind PR #69). So the rebuild's inertness is measured, not argued.
//!   **If P differs from A by more than this bench's run-to-run band, arm B must be read against P.**
//! - **B -- CP-adaptive.** `CpGate` driven from real per-frame measured `delay_spread_ms`, with the
//!   daemon's own transition detection and COP-1's re-entrancy guard, at
//!   `cp_arm::SWITCH_LATENCY_FRAMES`.
//! - **B0 -- CP-adaptive, zero latency.** Sensitivity arm bounding `SWITCH_LATENCY_FRAMES`'s
//!   influence, since that constant is DERIVED, NOT SWEPT.
//! - **C -- fixed short CP.** The best fixed arm over the short-CP half of the 18-cell set. Bounds
//!   the best case and answers the secondary question directly: **does CP *adaptivity* add anything
//!   over just always running short CP?**
//!
//! The switch cost charged to arm B is `cp_arm::HANDSHAKE_FRAMES_OLD_PROFILE` (3: `Propose`,
//! `Confirm`, and the bare ack for the Confirm) plus `cp_arm::HANDSHAKE_FRAMES_NEW_PROFILE` (2:
//! COP-1's `CpSwitched` third leg and its bare ack) = the five droppable on-air frames
//! `cp_negotiator` itself counts. Step 5 is B's local state change and costs no air.
//!
//! **Four things the switch cost does NOT charge, and all four bias arm B optimistic:**
//!
//! 1. **Retransmissions.** Every handshake leg is ARQ-tracked; a lost leg is retried. Only the
//!    loss-free path is charged.
//! 2. **Half-duplex turnarounds.** A six-step handshake has four turnarounds at
//!    `DEFAULT_TURNAROUND = 150 ms` each, none of which appear in the denominator.
//! 3. **Failed negotiations.** A handshake that reverts via COP-1's G1-G4 give-up triggers spends
//!    its full ARQ budget for **zero** CP change. Those are not modeled at all.
//! 4. **The level >= 5 divergence from production.** `select_ofdm_profile` routes every level >= 5 to
//!    `vhf_wide()` and ignores `cp_mode` there, but this bench forces one HF profile family for all
//!    nine levels (as it already did with `hf_robust`), so a CP mode applies at every level here.
//!    Production can never collect the CP saving above level 4 -- which is why the report splits the
//!    contrast into levels <= 4 (the production-reachable claim) and levels >= 5 (an explicit
//!    upper-bound overshoot).
//!
//! Rather than leave those implicit, the report prints arm B's goodput at switch-cost multipliers
//! **x0 / x1 / x6** -- the no-charge upper bound, the loss-free nominal, and the ARQ-budget ceiling
//! (`is_failed` at `transmit_count > DEFAULT_MAX_RETRANSMIT = 5`).
//!
//! This bench models the **converged** CP switch and charges its cost; it deliberately does not wire
//! `CpNegotiator`, `ArqTx`/`ArqRx`, a session, or a second station (D6). Handshake correctness is
//! COP-1's shipped, tested subject -- one loss-injection test per droppable frame, both directions,
//! through the real `decode_and_dispatch_audio`. Rebuilding `EventLoop` inside a throughput bench
//! would answer a question this ticket does not ask.
//!
//! # Reproduce
//!
//! ```text
//! cargo run -p coppa-bench --release --example closed_loop_arq              # robust base (default)
//! cargo run -p coppa-bench --release --example closed_loop_arq -- standard  # hf_standard CP pair
//! COPPA_CL_FRAMES=24 COPPA_CL_SEEDS=1 cargo run -p coppa-bench --release --example closed_loop_arq
//! ```
//!
//! CI never runs this file, so these figures carry **no regression protection**. The
//! `COPPA_CL_FRAMES` / `COPPA_CL_SEEDS` overrides exist so a wiring bug shows up in seconds instead
//! of only at full scale -- a bench nobody will re-run is a bench that rots.
use coppa_bench::adaptive_goodput::{
    best_arm, best_arm_by_bits, delivered_bits, goodput_bps, oracle_bits, oracle_goodput_bps,
    FrameSlot,
};
use coppa_bench::cp_arm::{
    self, frame_seed, goodput_with_switch_cost_bps, same_profile, switch_airtime_s, CpPolicy,
    CpSwitch, SpreadHistogram, MAX_FRAMES, SEEDS, SEED_STRIDE, SWITCH_LATENCY_FRAMES,
};
use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_ml::{RateLoop, VALID_SPEED_LEVELS};
use coppa_protocol::cp_negotiator::CpMode;
use coppa_protocol::modem::frame_airtime_s;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

/// Default frame count. Overridable via `COPPA_CL_FRAMES` so the wiring is
/// smoke-runnable in seconds -- CI never runs this file, so a cheap runnable path
/// is the only guard against a wiring bug that appears at full scale.
const DEFAULT_N_FRAMES: usize = 300;

/// The CP-mode profile pair an arm switches between.
///
/// Two pairs exist, for two different questions (COP-2 D1 / D1c):
///
/// - `standard` -- `hf_standard` <-> `hf_standard_short_cp`, via
///   [`cp_arm::profile_for`]. The ONLY exact CP-only pair in the workspace, and
///   production-faithful: `CoppaCore::select_ofdm_profile` resolves
///   `CpMode::LongCp`/`ShortCp` to exactly those two constructors. This is the
///   **shippable claim**.
/// - `robust` -- `hf_robust` <-> a bench-local synthetic short-CP counterpart.
///   `CoppaProfile`'s fields are all `pub`, so this is a two-line local profile and
///   **not** a spec change: nothing is added to `select_ofdm_profile` or
///   `docs/SPEC.md`. It answers the ticket's literal question (does CP airtime move
///   *this* metric, whose entire baseline is `hf_robust`) with ZERO base-profile
///   confound. Its limitation is equally real and is printed with every result: it
///   has no allocated `bandwidth_id` and the engine can never negotiate it, so it
///   measures the lever's **magnitude**, not a shippable feature.
struct CpProfilePair {
    long: CoppaProfile,
    short: CoppaProfile,
    /// `true` for the synthetic `hf_robust` pair, which gates the caveat line.
    synthetic: bool,
    label: &'static str,
}

impl CpProfilePair {
    fn for_base(base: &str) -> Self {
        match base {
            "standard" => Self {
                long: cp_arm::profile_for(CpMode::LongCp),
                short: cp_arm::profile_for(CpMode::ShortCp),
                synthetic: false,
                label: "hf_standard <-> hf_standard_short_cp (production-faithful, D1)",
            },
            "robust" => Self {
                long: CoppaProfile::hf_robust(),
                // D1c: `bandwidth_id: 5` is a bench-local placeholder codepoint,
                // deliberately NOT allocated in `docs/SPEC.md` -- see the type doc.
                short: CoppaProfile {
                    cp_samples: 144,
                    bandwidth_id: 5,
                    ..CoppaProfile::hf_robust()
                },
                synthetic: true,
                label: "hf_robust <-> synthetic robust short-CP (metric question, D1c)",
            },
            other => panic!(
                "closed_loop_arq needs a CP-pair base; got '{other}' (expected: robust|standard)"
            ),
        }
    }

    fn for_mode(&self, mode: CpMode) -> CoppaProfile {
        match mode {
            CpMode::LongCp => self.long.clone(),
            CpMode::ShortCp => self.short.clone(),
        }
    }
}

/// `seq` MUST vary per simulated frame (wrapping mod 256, matching real link seq numbering) --
/// NOT held at a constant 0. `CoppaTransceiver` does IR-HARQ combining (Phase 3 Task 3):
/// coded LLRs are accumulated per `seq_num` across `receive()` calls until a CRC pass evicts the
/// buffer. A constant seq across logically-independent frames would make every subsequent frame's
/// LLRs combine into the previous (unrelated) frame's leftover accumulator on any decode failure,
/// corrupting every following attempt at that seq -- exactly the failure mode this bench hit before
/// this was fixed (levels 9/10 measured 0/300 successes even at a clean 30 dB AWGN point that a
/// fresh transceiver decodes reliably).
///
/// **COP-2 correction to the paragraph above.** Varying `seq` is still correct and still what a real
/// link does, but the IR-HARQ justification does not actually bind here at all:
/// `HARQ_MAX_BUFFERS = 32` with LRU eviction, against a seq reuse distance of 256 frames, means a
/// reused seq's accumulator has ALWAYS been evicted before it comes round again, so combining is an
/// add-to-zero no-op.
///
/// **Second COP-2 correction — to the first one.** That paragraph used to continue "the load-bearing
/// condition is therefore `N_FRAMES <= 256 + 32`; a future `COPPA_CL_FRAMES` above ~288, or a smaller
/// `HARQ_MAX_BUFFERS`, would make the accumulator able to combine again". **Both halves are wrong,
/// and the first is self-refuting**: `DEFAULT_N_FRAMES` is 300, so the condition it declares
/// load-bearing is violated by this file's own default and by every committed figure in
/// `BENCHMARKS.md`. The real condition is `seq_reuse_distance > HARQ_MAX_BUFFERS`, i.e. `256 > 32` --
/// and because this bench advances `seq` by exactly one per frame, that reuse distance is a constant
/// 256 **independent of `N_FRAMES`**. There is no frame count at which IR-HARQ combining returns, and
/// a smaller `HARQ_MAX_BUFFERS` only widens the margin. What WOULD make the hazard live is a change
/// to how this bench assigns `seq` (batching, reuse, or a stride > 1). The genuine `N_FRAMES` ceiling
/// is the cross-seed channel-realization collision instead -- see `cp_arm::MAX_FRAMES`, which `main`
/// now asserts against.
///
/// `bandwidth` is read from the transmitting profile rather than hardcoded to 1, mirroring
/// `CoppaCore::encode_bytes` (`crates/coppa-engine/src/engine.rs:197`) -- required here because a
/// CP-adaptive arm transmits under two different `bandwidth_id`s within one run.
fn make_header(level: u8, len: u16, seq: u8, bandwidth: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth,
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
///
/// `n` is a parameter rather than a `const` read from inside because `COPPA_CL_FRAMES` has to be
/// able to shrink the run for a smoke test; with `N_FRAMES` baked in here the override could not be
/// implemented at all.
fn schedule(f: usize, n: usize) -> (ChannelSpec, f32) {
    let q = n / 3;
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

/// Schedule segment index for `f`: 0 = AWGN ramp up, 1 = AWGN ramp down, 2 = Watterson Good,
/// 3 = Watterson Poor. At the default 300 frames this is the 100/100/50/50 split the report's row
/// counts assume.
fn segment(f: usize, n: usize) -> usize {
    let q = n / 3;
    if f < q {
        0
    } else if f < 2 * q {
        1
    } else if f < 2 * q + q / 2 {
        2
    } else {
        3
    }
}

const SEGMENT_NAMES: [&str; 4] = [
    "awgn ramp up",
    "awgn ramp down",
    "watterson good",
    "watterson poor",
];

struct FrameOutcome {
    delivered: bool,
    recommended: Option<u8>,
    /// `Some` on ANY successful decode, not only on a delivered payload. That asymmetry is the
    /// faithful mirror: the daemon sees only `Ok`/`Err` and has no payload oracle
    /// (`event_loop.rs:1077-1095`). Gating the gate's input on this bench's private payload
    /// comparison would model a receiver that cannot exist.
    delay_spread_ms: Option<f32>,
}

/// Transmit one known frame at `level` through the scheduled channel.
fn run_frame(tx: &CoppaTransceiver, level: u8, f: usize, n: usize, run_seed: u64) -> FrameOutcome {
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let payload: Vec<u8> = (0..pfb)
        .map(|i| (i as u64 * 0x9E37 + f as u64) as u8)
        .collect();
    let seq = (f % 256) as u8;
    let sig = tx
        .transmit(
            &make_header(level, pfb as u16, seq, tx.profile().bandwidth_id),
            &payload,
        )
        .expect("payload sized from this level's own payload_bytes() always fits");
    let (ch, snr) = schedule(f, n);
    let faded = apply_channel(&sig, ch, snr, frame_seed(run_seed, f));
    match tx.receive_with_metrics(&faded) {
        Ok((_h, p, _snr, rec, spread)) => FrameOutcome {
            delivered: p.len() >= pfb && p[..pfb] == payload[..],
            recommended: Some(rec),
            delay_spread_ms: Some(spread),
        },
        Err(_) => FrameOutcome {
            delivered: false,
            recommended: None,
            delay_spread_ms: None,
        },
    }
}

#[derive(Clone)]
enum ArmKind {
    /// One fixed level and one fixed CP mode: a comparator cell.
    Fixed { level: u8, mode: CpMode },
    /// Adaptive rate, one fixed CP mode for the whole run. `LongCp` is arm A (the control);
    /// `ShortCp` is not run as an arm -- arm C is derived from the fixed short-CP cells.
    FixedCpAdaptiveRate { mode: CpMode },
    /// Arm P: identical to arm A, but rebuilds the transceiver on exactly the frames arm B
    /// switched on, with the SAME profile. Isolates the rebuild from the CP change.
    Placebo { rebuild_at: Vec<usize> },
    /// Arms B / B0: `CpGate`-driven CP switching at the given switch latency.
    CpAdaptive { latency: usize },
}

struct ArmResult {
    slots: Vec<FrameSlot>,
    switches: Vec<CpSwitch>,
    hist: [SpreadHistogram; 4],
    trace: Vec<(usize, f32, u8)>,
    /// Total handshake airtime charged for this arm's switches (0 for non-CP arms).
    switch_air_s: f64,
    /// D1a: airtime that ran under short CP, expressed on the CONTROL arm's (long-CP) basis so the
    /// share is pre-reduction, split by production-reachable levels (<= 4) vs the levels >= 5
    /// overshoot that `select_ofdm_profile` would route to `vhf_wide` in production.
    short_air_basis_le4: f64,
    short_air_basis_ge5: f64,
    /// Denominator of the two shares above: this arm's whole run on the long-CP basis.
    total_air_basis: f64,
    rebuilds: usize,
}

fn run_arm(kind: &ArmKind, pair: &CpProfilePair, run_seed: u64, n: usize) -> ArmResult {
    let mut slots: Vec<FrameSlot> = Vec::with_capacity(n);
    let mut trace: Vec<(usize, f32, u8)> = Vec::new();
    let mut hist: [SpreadHistogram; 4] = Default::default();
    let mut rebuilds = 0usize;

    // Each arm gets its OWN transceiver: seq numbers wrap mod 256 within a run's frames
    // (necessary so IR-HARQ's per-seq LLR accumulator treats each of these logically-independent
    // frames independently -- see `make_header`'s doc), but sharing one transceiver *across* arms
    // would let an arm boundary's seq-0 reuse combine into another arm's leftover accumulator
    // from a different level entirely.
    let boot_mode = match kind {
        ArmKind::Fixed { mode, .. } | ArmKind::FixedCpAdaptiveRate { mode } => *mode,
        // Both boot on LongCp, matching `CpNegotiator::new` and `CpGate`'s initial state.
        ArmKind::Placebo { .. } | ArmKind::CpAdaptive { .. } => CpMode::LongCp,
    };
    let mut tx = CoppaTransceiver::new(pair.for_mode(boot_mode), 1);

    let mut policy = match kind {
        ArmKind::CpAdaptive { latency } => Some(CpPolicy::new(*latency)),
        _ => None,
    };
    let mut loop_ctl = match kind {
        ArmKind::Fixed { .. } => None,
        // `.with_probing(2, 1)` is the (probe_interval, probe_offset) combination measured best via
        // an exhaustive sweep; `raise_dwell = 5` is `RateLoop::default_coppa()`'s unchanged default.
        // Left byte-for-byte as it was pre-COP-2: this ticket must not confound a metric change
        // with a tuning change.
        _ => Some(RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 5, 1).with_probing(2, 1)),
    };

    let mut switch_air_s = 0.0f64;
    let mut short_air_basis_le4 = 0.0f64;
    let mut short_air_basis_ge5 = 0.0f64;
    let mut total_air_basis = 0.0f64;

    for f in 0..n {
        // The ordering contract `CpPolicy` documents: pick the profile for frame `f` BEFORE
        // feeding frame `f`'s measurement, exactly as the daemon does.
        let (mode, applied_this_frame) = match policy.as_mut() {
            Some(p) => {
                let m = p.mode_for_frame(f);
                (m, p.applied_on(f))
            }
            None => (boot_mode, false),
        };
        if policy.is_some() {
            let want = pair.for_mode(mode);
            // Rebuild on switch APPLICATION, not merely on profile inequality. The
            // inequality term stays as a defensive backstop, but `applied_on` is what
            // makes arm B's rebuild set equal `switches[].effective_from` BY
            // CONSTRUCTION -- which is what arm P rebuilds on. Gating on inequality
            // alone silently skipped the `from == to` case (reachable at latency 5, and
            // printed as "NO-OP" by the switch-accounting block) while arm P still
            // rebuilt there, so the placebo stopped being a control. See
            // `CpPolicy::applied_on`.
            if applied_this_frame || !same_profile(tx.profile(), &want) {
                // No profile setter exists: `CoppaTransceiver::new` takes the profile BY VALUE
                // (transceiver.rs:526). A CP switch is a REBUILD.
                tx = CoppaTransceiver::new(want, 1);
                rebuilds += 1;
            }
        }
        if let ArmKind::Placebo { rebuild_at } = kind {
            if rebuild_at.contains(&f) {
                // Same profile, same rebuild. This is the control that makes the rebuild's
                // inertness measured rather than argued.
                tx = CoppaTransceiver::new(pair.for_mode(boot_mode), 1);
                rebuilds += 1;
            }
        }

        let (level, is_probe) = match loop_ctl.as_mut() {
            Some(l) => l.level_for_next_transmission(),
            None => match kind {
                ArmKind::Fixed { level, .. } => (*level, false),
                _ => unreachable!("only Fixed arms have no RateLoop"),
            },
        };

        let out = run_frame(&tx, level, f, n, run_seed);

        // Airtime bookkeeping on the CONTROL (long-CP) basis, per D1a.
        let long_air = frame_airtime_s(level, &pair.long).unwrap_or(0.0);
        total_air_basis += long_air;
        if mode == CpMode::ShortCp {
            if level <= 4 {
                short_air_basis_le4 += long_air;
            } else {
                short_air_basis_ge5 += long_air;
            }
        }

        slots.push(
            FrameSlot::for_level(level, tx.profile(), out.delivered)
                .expect("levels come from VALID_SPEED_LEVELS and the profile has data carriers"),
        );
        hist[segment(f, n)].observe(out.delay_spread_ms);

        // Review finding (P2, declined): production feeds `cp_gate.observe` on EVERY
        // successfully decoded frame (event_loop.rs:1119-1147), data or CP-control alike,
        // so a Propose/Confirm/ack/Switched reception also advances or resets the gate's
        // dwell state. This loop only observes the data-frame slot below; the five
        // control-frame legs of a handshake are priced as pure airtime (switch_air_s
        // above) and never touch `policy`'s dwell state. Left as a documented scope
        // boundary rather than simulated: this harness never runs a decode for a
        // control frame at all (only `switch_airtime_s` prices it), so there is no
        // measured `delay_spread_ms` to feed the policy with for those legs — inventing
        // one would model noise, not the daemon. The gap can only bias switch
        // timing/counts during the brief handshake window itself, not the steady-state
        // dwell behavior the benchmark exists to compare.
        if let Some(p) = policy.as_mut() {
            if let Some(sw) = p.observe(f, level, out.delay_spread_ms) {
                // Price the handshake on THIS run's pair, not on the `hf_standard` pair
                // `cp_arm::profile_for` builds: the default base is `robust`, whose 36
                // data carriers make every frame 61 symbols instead of 52, so
                // reconstructing the profiles here undercharged the robust runs by ~15%
                // -- an undercharge that inflates arm B. See `switch_airtime_s`'s doc.
                switch_air_s +=
                    switch_airtime_s(&sw, &pair.for_mode(sw.from), &pair.for_mode(sw.to))
                        .expect("switch level comes from VALID_SPEED_LEVELS");
            }
        }

        if let Some(l) = loop_ctl.as_mut() {
            if is_probe {
                l.on_probe_result(level, out.delivered);
            } else {
                match out.recommended {
                    Some(r) => l.on_ack(r, out.delivered),
                    None => l.on_timeout(),
                }
            }
        }

        let (_ch, snr) = schedule(f, n);
        if f % 15 == 0 {
            trace.push((f, snr, level));
        }
    }

    ArmResult {
        slots,
        switches: policy
            .as_ref()
            .map(|p| p.switches().to_vec())
            .unwrap_or_default(),
        hist,
        trace,
        switch_air_s,
        short_air_basis_le4,
        short_air_basis_ge5,
        total_air_basis,
        rebuilds,
    }
}

/// Wilson score interval for a delivery rate, at 95%.
fn wilson(k: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let z = 1.96f64;
    let nf = n as f64;
    let p = k as f64 / nf;
    let d = 1.0 + z * z / nf;
    let c = p + z * z / (2.0 * nf);
    let m = z * ((p * (1.0 - p) / nf) + z * z / (4.0 * nf * nf)).sqrt();
    ((c - m) / d, (c + m) / d)
}

/// One seed's row of headline goodput figures, in bits/s. Every ratio and delta the report prints is
/// built from these numbers, so they are named rather than carried as a tuple -- a tuple of `f64` is
/// exactly the shape where a transposed field silently reports the wrong quantity.
///
/// # `arm_b` is the x0 (data-airtime-only) figure, and every consumer prints the x1 band beside it
///
/// `arm_b` was for a long time the ONLY arm-B number any headline consumed, and it is
/// `delivered_bits / data_airtime` -- identically the x0 no-charge bound. The handshake airtime the
/// run computed was accumulated into `ArmResult::switch_air_s` and then spent nowhere except the
/// later SWITCH ACCOUNTING block, so the bar ratios, the aggregate mean, the per-seed D9 deltas and
/// the primary table all quietly reported an arm B that paid nothing for its own switches (~3.3-4.5%
/// pro-arm-B on this schedule) with no label saying so.
///
/// The fix is NOT to silently swap the primary to x1: `goodput_with_switch_cost_bps`'s doc records a
/// deliberate refusal to commit to one guess about a cost this bench cannot measure. So both are
/// carried, and every consumer prints the band.
struct SeedAgg {
    arm_a: f64,
    arm_p: f64,
    /// Arm B at cost x0 -- data airtime only. See the type doc.
    arm_b: f64,
    /// Arm B at cost x1 -- the loss-free nominal, with one handshake's air charged per switch.
    arm_b_x1: f64,
    bf_long: f64,
    bf_joint: f64,
    orc_long: f64,
    orc_joint: f64,
}

fn delivered_count(slots: &[FrameSlot]) -> usize {
    slots.iter().filter(|s| s.delivered).count()
}

fn total_airtime(slots: &[FrameSlot]) -> f64 {
    slots.iter().map(|s| s.airtime_s).sum()
}

/// One seed's full 22-run measurement.
struct SeedResult {
    arm_a: ArmResult,
    arm_p: ArmResult,
    arm_b: ArmResult,
    arm_b0: ArmResult,
    /// The 18 comparator cells, `[LongCp 9 levels..., ShortCp 9 levels...]`.
    cells: Vec<Vec<FrameSlot>>,
}

impl SeedResult {
    fn long_cells(&self) -> &[Vec<FrameSlot>] {
        &self.cells[..VALID_SPEED_LEVELS.len()]
    }
    fn short_cells(&self) -> &[Vec<FrameSlot>] {
        &self.cells[VALID_SPEED_LEVELS.len()..]
    }
    /// Per-frame candidate lists for the oracle: `frames[f]` holds one candidate per cell.
    fn oracle_frames(cells: &[Vec<FrameSlot>], n: usize) -> Vec<Vec<FrameSlot>> {
        (0..n)
            .map(|f| cells.iter().map(|c| c[f]).collect())
            .collect()
    }
}

fn run_seed(pair: &CpProfilePair, seed: u64, n: usize) -> SeedResult {
    // 18 comparator cells: level x CP mode (D4).
    let mut cells: Vec<Vec<FrameSlot>> = Vec::with_capacity(2 * VALID_SPEED_LEVELS.len());
    for mode in [CpMode::LongCp, CpMode::ShortCp] {
        for &lvl in VALID_SPEED_LEVELS.iter() {
            let r = run_arm(&ArmKind::Fixed { level: lvl, mode }, pair, seed, n);
            eprintln!(
                "  seed {seed} fixed {:?} L{lvl}: {}/{} delivered, {:.1} bps",
                mode,
                delivered_count(&r.slots),
                n,
                goodput_bps(&r.slots)
            );
            cells.push(r.slots);
        }
    }

    let arm_a = run_arm(
        &ArmKind::FixedCpAdaptiveRate {
            mode: CpMode::LongCp,
        },
        pair,
        seed,
        n,
    );
    eprintln!(
        "  seed {seed} arm A  : {:.1} bps",
        goodput_bps(&arm_a.slots)
    );

    let arm_b = run_arm(
        &ArmKind::CpAdaptive {
            latency: SWITCH_LATENCY_FRAMES,
        },
        pair,
        seed,
        n,
    );
    eprintln!(
        "  seed {seed} arm B  : {:.1} bps, {} switches",
        goodput_bps(&arm_b.slots),
        arm_b.switches.len()
    );

    let arm_b0 = run_arm(&ArmKind::CpAdaptive { latency: 0 }, pair, seed, n);
    eprintln!(
        "  seed {seed} arm B0 : {:.1} bps, {} switches",
        goodput_bps(&arm_b0.slots),
        arm_b0.switches.len()
    );

    // Arm P rebuilds on exactly arm B's switch frames, so it must run after B.
    let rebuild_at: Vec<usize> = arm_b.switches.iter().map(|s| s.effective_from).collect();
    let arm_p = run_arm(&ArmKind::Placebo { rebuild_at }, pair, seed, n);
    // Arm B's rebuild count is printed BESIDE arm P's, not just P's own. The placebo is only a
    // control while the two rebuild sets match, and `CpPolicy::applied_on` now makes them match by
    // construction -- but a mismatch printed is a mismatch someone can see, whereas the divergence
    // this replaced was invisible in the output for exactly as long as it existed.
    eprintln!(
        "  seed {seed} arm P  : {:.1} bps ({} rebuilds; arm B had {})",
        goodput_bps(&arm_p.slots),
        arm_p.rebuilds,
        arm_b.rebuilds
    );
    if arm_p.rebuilds != arm_b.rebuilds {
        eprintln!(
            "  seed {seed} WARNING: arm P rebuilt {} times but arm B rebuilt {} -- the placebo is \
             NOT a control for this seed and arm B must not be read against it",
            arm_p.rebuilds, arm_b.rebuilds
        );
    }

    SeedResult {
        arm_a,
        arm_p,
        arm_b,
        arm_b0,
        cells,
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "robust".to_string());
    // Validate against the shared name table too, so a typo fails the same way every other bench
    // fails it rather than only tripping `CpProfilePair`'s own panic.
    if base != "robust" && base != "standard" {
        let _ = profile_by_name(&base);
        panic!("closed_loop_arq needs a CP-pair base: expected robust|standard, got '{base}'");
    }
    let pair = CpProfilePair::for_base(&base);
    let n = env_usize("COPPA_CL_FRAMES", DEFAULT_N_FRAMES);
    // `env_usize` is parse-or-default with no range check, in contrast to the seed count on the
    // next line, which IS clamped. `cp_arm::MAX_FRAMES` is the cross-seed collision boundary
    // (= SEED_STRIDE): above it, seed k's tail reproduces seed k+1's head value-for-value and the
    // seeds stop being independent. An ASSERT rather than a clamp, because the override's
    // documented purpose is to SHRINK the run: someone passing a larger value wants more frames,
    // and silently giving them fewer would be worse than refusing.
    //
    // NOT the `256 + 32` this file's `make_header` doc used to name as the binding ceiling -- that
    // number is wrong and is violated by DEFAULT_N_FRAMES itself. See `cp_arm::MAX_FRAMES`.
    assert!(
        n > 0 && n <= MAX_FRAMES,
        "COPPA_CL_FRAMES={n} is out of range: must be 1..={MAX_FRAMES}, the frame_seed stride \
         (SEED_STRIDE = {SEED_STRIDE}) -- above it two run seeds share channel realizations at an \
         offset. The override exists to shrink the run for a smoke test."
    );
    let n_seeds = env_usize("COPPA_CL_SEEDS", SEEDS.len())
        .min(SEEDS.len())
        .max(1);
    let seeds = &SEEDS[..n_seeds];

    println!("=== Closed-loop adaptive rate + CP contrast (COP-2) ===");
    println!("base pair    : {}", pair.label);
    println!(
        "frames/run   : {n}   seeds: {seeds:?}   runs: {}",
        n_seeds * 22
    );
    if pair.synthetic {
        println!(
            "NOTE         : the short-CP half of this pair is a BENCH-LOCAL SYNTHETIC profile with an\n\
                            unallocated bandwidth_id (5). `select_ofdm_profile` can never select it and the\n\
                            engine can never negotiate it, so this base measures the CP lever's MAGNITUDE,\n\
                            not a shippable feature. Use the `standard` base for the shippable claim."
        );
    }
    println!();

    let results: Vec<SeedResult> = seeds.iter().map(|&s| run_seed(&pair, s, n)).collect();

    // ---- Per-seed primary table -------------------------------------------------
    println!("\n=== PRIMARY: airtime-normalized goodput (bits/s) ===");
    println!(
        "The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY\n\
         arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the\n\
         SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in\n\
         this table's denominator. Arms A / P / B0 / C and all 18 comparator cells have no handshake\n\
         air at all, so for them x0 is the whole story."
    );
    println!(
        "{:<6} {:<5} {:>9} {:>10} {:>11} {:>10} {:>7} {:>17}",
        "seed", "arm", "deliv", "bits", "airtime(s)", "goodput", "FER", "delivery 95% CI"
    );
    for (si, r) in results.iter().enumerate() {
        let arms: [(&str, &ArmResult); 4] = [
            ("A", &r.arm_a),
            ("P", &r.arm_p),
            ("B", &r.arm_b),
            ("B0", &r.arm_b0),
        ];
        for (name, a) in arms {
            let d = delivered_count(&a.slots);
            let air = total_airtime(&a.slots);
            let (lo, hi) = wilson(d, a.slots.len());
            println!(
                "{:<6} {:<5} {:>4}/{:<4} {:>10} {:>11.2} {:>10.1} {:>6.1}% {:>7.3}-{:<7.3}",
                seeds[si],
                name,
                d,
                a.slots.len(),
                delivered_bits(&a.slots),
                air,
                goodput_bps(&a.slots),
                100.0 * (1.0 - d as f64 / a.slots.len() as f64),
                lo,
                hi
            );
        }
        // Arm C: best fixed arm over the short-CP half.
        if let Some((ci, cg)) = best_arm(r.short_cells()) {
            let c = &r.short_cells()[ci];
            let d = delivered_count(c);
            let (lo, hi) = wilson(d, c.len());
            println!(
                "{:<6} {:<5} {:>4}/{:<4} {:>10} {:>11.2} {:>10.1} {:>6.1}% {:>7.3}-{:<7.3}  (L{})",
                seeds[si],
                "C",
                d,
                c.len(),
                delivered_bits(c),
                total_airtime(c),
                cg,
                100.0 * (1.0 - d as f64 / c.len() as f64),
                lo,
                hi,
                VALID_SPEED_LEVELS[ci]
            );
        }
    }

    // ---- Comparators ------------------------------------------------------------
    println!("\n=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===");
    println!(
        "{:<6} {:>14} {:>6} {:>14} {:>6} {:>13} {:>13}",
        "seed", "bestfix(long)", "lvl", "bestfix(joint)", "cell", "oracle(long)", "oracle(joint)"
    );
    let mut agg: Vec<SeedAgg> = Vec::new();
    for (si, r) in results.iter().enumerate() {
        let (li, lg) = best_arm(r.long_cells()).expect("9 long cells");
        let (ji, jg) = best_arm(&r.cells).expect("18 joint cells");
        let (ol, _) = oracle_goodput_bps(&SeedResult::oracle_frames(r.long_cells(), n));
        let (oj, _) = oracle_goodput_bps(&SeedResult::oracle_frames(&r.cells, n));
        let jcell = if ji < VALID_SPEED_LEVELS.len() {
            format!("longL{}", VALID_SPEED_LEVELS[ji])
        } else {
            format!(
                "shortL{}",
                VALID_SPEED_LEVELS[ji - VALID_SPEED_LEVELS.len()]
            )
        };
        println!(
            "{:<6} {:>14.1} {:>6} {:>14.1} {:>6} {:>13.1} {:>13.1}",
            seeds[si],
            lg,
            format!("L{}", VALID_SPEED_LEVELS[li]),
            jg,
            jcell,
            ol,
            oj
        );
        agg.push(SeedAgg {
            arm_a: goodput_bps(&r.arm_a.slots),
            arm_p: goodput_bps(&r.arm_p.slots),
            arm_b: goodput_bps(&r.arm_b.slots),
            arm_b_x1: goodput_with_switch_cost_bps(
                delivered_bits(&r.arm_b.slots) as f64,
                total_airtime(&r.arm_b.slots),
                r.arm_b.switch_air_s,
                1.0,
            ),
            bf_long: lg,
            bf_joint: jg,
            orc_long: ol,
            orc_joint: oj,
        });
    }

    // ---- Ratios against the bar -------------------------------------------------
    println!("\n=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===");
    println!(
        "Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and\n\
         x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows\n\
         used to report, unlabelled -- an arm B that paid nothing for its own switches."
    );
    println!(
        "{:<6} {:>17} {:>17} {:>17} {:>17}",
        "seed", "B/bf(long)", "B/orc(long)", "B/bf(joint)", "B/orc(joint)"
    );
    let band =
        |x0: f64, x1: f64, d: f64| format!("{:.3}/{:.3}", x0 / d.max(1e-9), x1 / d.max(1e-9));
    for (si, s) in agg.iter().enumerate() {
        println!(
            "{:<6} {:>17} {:>17} {:>17} {:>17}",
            seeds[si],
            band(s.arm_b, s.arm_b_x1, s.bf_long),
            band(s.arm_b, s.arm_b_x1, s.orc_long),
            band(s.arm_b, s.arm_b_x1, s.bf_joint),
            band(s.arm_b, s.arm_b_x1, s.orc_joint),
        );
    }
    println!("(each cell is x0/x1)");
    println!(
        "\nREMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it\n\
         cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious\n\
         1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are\n\
         the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is\n\
         structurally blind to an airtime lever."
    );

    // ---- Legacy (airtime-blind) control ----------------------------------------
    println!("\n=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===");
    println!(
        "These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must"
    );
    println!("reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /");
    println!("adaptive-oracle 0.762 / adaptive-best-fixed 0.914.");
    for (si, r) in results.iter().enumerate() {
        let ab = delivered_bits(&r.arm_a.slots);
        let (bi, bb) = best_arm_by_bits(r.long_cells()).expect("9 long cells");
        let ob = oracle_bits(&SeedResult::oracle_frames(r.long_cells(), n));
        println!(
            "seed {:<3} adaptive {:>7} bits | best fixed (L{:<2}) {:>7} bits | oracle {:>7} bits | \
             a/o = {:.3}  a/bf = {:.3}",
            seeds[si],
            ab,
            VALID_SPEED_LEVELS[bi],
            bb,
            ob,
            ab as f64 / ob.max(1) as f64,
            ab as f64 / bb.max(1) as f64
        );
    }
    println!(
        "NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's\n\
         best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that\n\
         is the metric change working, not a regression."
    );

    // ---- Switch accounting ------------------------------------------------------
    println!("\n=== SWITCH ACCOUNTING (arm B) ===");
    println!(
        "Charged per switch: {} frames under the old profile (Propose, Confirm, bare ack) + {} under\n\
         the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.",
        cp_arm::HANDSHAKE_FRAMES_OLD_PROFILE,
        cp_arm::HANDSHAKE_FRAMES_NEW_PROFILE
    );
    println!(
        "{:<6} {:>9} {:>8} {:>12} {:>9} {:>11} {:>11} {:>11}",
        "seed", "switches", "frames", "switch_air", "%of arm B", "gp x0", "gp x1", "gp x6"
    );
    for (si, r) in results.iter().enumerate() {
        let b = &r.arm_b;
        let data_air = total_airtime(&b.slots);
        let bits = delivered_bits(&b.slots) as f64;
        let frames = b.switches.len()
            * (cp_arm::HANDSHAKE_FRAMES_OLD_PROFILE + cp_arm::HANDSHAKE_FRAMES_NEW_PROFILE);
        println!(
            "{:<6} {:>9} {:>8} {:>12.2} {:>8.2}% {:>11.1} {:>11.1} {:>11.1}",
            seeds[si],
            b.switches.len(),
            frames,
            b.switch_air_s,
            100.0 * b.switch_air_s / data_air.max(1e-9),
            goodput_with_switch_cost_bps(bits, data_air, b.switch_air_s, 0.0),
            goodput_with_switch_cost_bps(bits, data_air, b.switch_air_s, 1.0),
            goodput_with_switch_cost_bps(bits, data_air, b.switch_air_s, 6.0),
        );
        for sw in &b.switches {
            println!(
                "         switch {:?} -> {:?} decided@{} effective@{} L{}{}",
                sw.from,
                sw.to,
                sw.decided_at,
                sw.effective_from,
                sw.level,
                if sw.from == sw.to {
                    "   (NO-OP: five control frames for zero CP change)"
                } else {
                    ""
                }
            );
        }
    }
    println!(
        "x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling\n\
         (DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:\n\
         leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and\n\
         handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change."
    );

    // ---- Delay-spread histogram -------------------------------------------------
    println!("\n=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===");
    println!(
        "Grid = (fft_size/nc)/sample_rate = {:.5} ms; DelayDomainEstimator clamps taps to 1..=8, so\n\
         the metric takes only EIGHT values, 0.000-{:.4} ms. CpGate::default_coppa() threshold is\n\
         2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.",
        cp_arm::SPREAD_GRID_MS,
        7.0 * cp_arm::SPREAD_GRID_MS
    );
    print!("{:<17}", "segment");
    for b in 0..8 {
        print!(
            " {:>7}",
            format!("{:.3}", b as f32 * cp_arm::SPREAD_GRID_MS)
        );
    }
    println!(" {:>9} {:>9}", "undecoded", "off_grid");
    for (si, r) in results.iter().enumerate() {
        for (sg, h) in r.arm_b.hist.iter().enumerate() {
            print!("s{} {:<14}", seeds[si], SEGMENT_NAMES[sg]);
            for b in 0..8 {
                print!(" {:>7}", h.counts[b]);
            }
            println!(" {:>9} {:>9}", h.undecoded, h.off_grid);
        }
    }
    let off_grid_total: usize = results
        .iter()
        .flat_map(|r| r.arm_b.hist.iter())
        .map(|h| h.off_grid)
        .sum();
    println!(
        "off_grid total = {off_grid_total} (MUST be 0; any nonzero value means the nc=48 / 8-tap\n\
         assumption is wrong and this diagnostic is invalid)"
    );

    // ---- Aggregate + the ticket's answer ---------------------------------------
    let mean =
        |f: &dyn Fn(&SeedAgg) -> f64| -> f64 { agg.iter().map(f).sum::<f64>() / agg.len() as f64 };
    let m_a = mean(&|s| s.arm_a);
    let m_p = mean(&|s| s.arm_p);
    let m_b = mean(&|s| s.arm_b);
    let m_b_x1 = mean(&|s| s.arm_b_x1);
    let m_bf_long = mean(&|s| s.bf_long);
    let m_bf_joint = mean(&|s| s.bf_joint);
    let m_orc_long = mean(&|s| s.orc_long);
    let m_orc_joint = mean(&|s| s.orc_joint);

    println!("\n=== TICKET COP-2 DELTA ===");
    println!("Both answers below were PRE-COMMITTED in the plan (D4a) before this run.\n");

    println!("PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):");
    println!(
        "  best-fixed:  long-CP-only {:>9.1} bps  ->  joint {:>9.1} bps   ({:+.2}%)",
        m_bf_long,
        m_bf_joint,
        100.0 * (m_bf_joint / m_bf_long.max(1e-9) - 1.0)
    );
    println!(
        "  oracle    :  long-CP-only {:>9.1} bps  ->  joint {:>9.1} bps   ({:+.2}%)",
        m_orc_long,
        m_orc_joint,
        100.0 * (m_orc_joint / m_orc_long.max(1e-9) - 1.0)
    );
    println!(
        "  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is\n\
         working exactly as arithmetic predicts, NOT that the bar moved)"
    );

    println!("\nSECONDARY -- does CP *adaptivity* add anything over fixed short CP?");
    let m_c: f64 = results
        .iter()
        .filter_map(|r| best_arm(r.short_cells()).map(|(_, g)| g))
        .sum::<f64>()
        / results.len() as f64;
    println!(
        "  arm B (CP-adaptive) {:>9.1} bps   vs   arm C (fixed short CP) {:>9.1} bps  ({:+.2}%)   [x0]",
        m_b,
        m_c,
        100.0 * (m_b / m_c.max(1e-9) - 1.0)
    );
    println!(
        "  arm B (CP-adaptive) {:>9.1} bps   vs   arm C (fixed short CP) {:>9.1} bps  ({:+.2}%)   [x1]",
        m_b_x1,
        m_c,
        100.0 * (m_b_x1 / m_c.max(1e-9) - 1.0)
    );
    println!(
        "  arm B / best-fixed(joint) = {:.3} (x0) / {:.3} (x1)  (bar: > 1.0)\n  \
         arm B / oracle(joint)     = {:.3} (x0) / {:.3} (x1)  (bar: >= 0.8)",
        m_b / m_bf_joint.max(1e-9),
        m_b_x1 / m_bf_joint.max(1e-9),
        m_b / m_orc_joint.max(1e-9),
        m_b_x1 / m_orc_joint.max(1e-9)
    );
    println!(
        "  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C\n\
         has no handshake air at all, so only arm B's side of each comparison moves."
    );

    // D9's pre-committed decision rule: sign consistency across every seed, NOT a mean crossing a
    // threshold. 5 seeds is not a significance test and this is labeled a directional result, per
    // COP-3's "diagnostic rather than statistical acceptance thresholds" precedent.
    println!("\nD9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:");
    println!(
        "Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign\n\
         that survives x0 but not x1 is a result the no-charge accounting manufactured."
    );
    println!(
        "{:<6} {:>21} {:>21} {:>17}",
        "seed", "B-A (bps) x0/x1", "B-C (bps) x0/x1", "B/bf(joint)"
    );
    let mut pos_ba = 0usize;
    let mut pos_bc = 0usize;
    let mut pos_ba_x1 = 0usize;
    let mut pos_bc_x1 = 0usize;
    for (si, r) in results.iter().enumerate() {
        let s = &agg[si];
        let c = best_arm(r.short_cells()).map(|(_, g)| g).unwrap_or(0.0);
        let d_ba = s.arm_b - s.arm_a;
        let d_bc = s.arm_b - c;
        let d_ba_x1 = s.arm_b_x1 - s.arm_a;
        let d_bc_x1 = s.arm_b_x1 - c;
        if d_ba > 0.0 {
            pos_ba += 1;
        }
        if d_bc > 0.0 {
            pos_bc += 1;
        }
        if d_ba_x1 > 0.0 {
            pos_ba_x1 += 1;
        }
        if d_bc_x1 > 0.0 {
            pos_bc_x1 += 1;
        }
        println!(
            "{:<6} {:>21} {:>21} {:>17}",
            seeds[si],
            format!("{d_ba:+.1}/{d_ba_x1:+.1}"),
            format!("{d_bc:+.1}/{d_bc_x1:+.1}"),
            format!(
                "{:.3}/{:.3}",
                s.arm_b / s.bf_joint.max(1e-9),
                s.arm_b_x1 / s.bf_joint.max(1e-9)
            ),
        );
    }
    let n_seeds_run = results.len();
    println!(
        "  x0: B > A on {}/{} seeds; B > C on {}/{} seeds.",
        pos_ba, n_seeds_run, pos_bc, n_seeds_run
    );
    println!(
        "  x1: B > A on {}/{} seeds; B > C on {}/{} seeds.",
        pos_ba_x1, n_seeds_run, pos_bc_x1, n_seeds_run
    );
    println!(
        "  Sign-consistent (all {} seeds one way) => a DIRECTIONAL result. Mixed signs => no result,\n\
         and the write-up must say the delta is inside this bench's own run-to-run band rather than\n\
         reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once\n\
         delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit\n\
         the same levels on the same frames.",
        n_seeds_run
    );

    println!(
        "\nREBUILD PLACEBO -- arm A {:.1} bps vs arm P {:.1} bps ({:+.2}%)",
        m_a,
        m_p,
        100.0 * (m_p / m_a.max(1e-9) - 1.0)
    );
    println!(
        "  If these differ by more than this bench's run-to-run band, arm B must be read against P,\n\
         not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert."
    );

    println!("\nD1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):");
    for (si, r) in results.iter().enumerate() {
        let b = &r.arm_b;
        println!(
            "  seed {:<3} levels<=4 {:>6.2}%   levels>=5 {:>6.2}%   (total short-CP {:>6.2}%)",
            seeds[si],
            100.0 * b.short_air_basis_le4 / b.total_air_basis.max(1e-9),
            100.0 * b.short_air_basis_ge5 / b.total_air_basis.max(1e-9),
            100.0 * (b.short_air_basis_le4 + b.short_air_basis_ge5) / b.total_air_basis.max(1e-9)
        );
    }
    println!(
        "  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level\n\
         >= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit\n\
         UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a)."
    );

    println!(
        "\nTRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this"
    );
    println!(
        "schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs"
    );
    println!(
        "arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a"
    );
    println!("latched single switch as adaptive control.");
    let total_sw: usize = results.iter().map(|r| r.arm_b.switches.len()).sum();
    println!(
        "  transitions: {} across {} seed(s) (mean {:.1}/run)",
        total_sw,
        results.len(),
        total_sw as f64 / results.len() as f64
    );

    // ---- Level trace ------------------------------------------------------------
    println!(
        "\n frame   snr(dB)  level   (arm B, seed {}; level tracks the channel)",
        seeds[0]
    );
    for (f, snr, lvl) in &results[0].arm_b.trace {
        println!("  {f:4}    {snr:5.1}     L{lvl}");
    }
}
