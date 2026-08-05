//! Airtime-normalized goodput accounting for closed-loop adaptive-rate benches.
//!
//! `closed_loop_arq` originally scored every arm as delivered info bits per frame
//! SLOT, which is blind to the fact that a higher speed level -- and a shorter
//! cyclic prefix -- occupies less air. This implements the project's canonical
//! goodput convention instead (`BENCHMARKS.md`: `payload_bits * (1 - FER) /
//! frame_airtime`), generalized to a per-frame level and profile:
//!
//! ```text
//! goodput_bps = sum(info_bits over DELIVERED frames)
//!               / sum(frame_airtime_s over ALL TRANSMITTED frames)
//! ```
//!
//! The denominator counts undelivered frames because they still occupy the
//! channel; for a fixed-level arm the expression reduces algebraically to
//! `info_bits * (1 - FER) / frame_airtime_s`, i.e. the canonical form exactly. A
//! delivered-only denominator would collapse to `info_bits / airtime` (the on-air
//! rate) and drop FER from the metric entirely.
//!
//! `coppa_bench::metrics::aggregate` computes a goodput too, from a measured
//! signal length rather than from `frame_airtime_s`. That duplication is
//! deliberate and left alone: it serves the whole FER/BER sweep harness, and
//! unifying the two would silently re-base every table `runner.rs` feeds.

use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::frame_airtime_s;
use coppa_protocol::modem::speed_levels::max_payload_for_level;

/// One transmitted frame slot: what it cost on air, and what it delivered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSlot {
    pub level: u8,
    /// Application payload bits this frame carries when it decodes:
    /// `max_payload_for_level(level) * 8` -- NOT `ModeInfo::info_bits` (k_used),
    /// which is 4 bytes larger because it includes the CRC-32 trailer.
    pub info_bits: usize,
    /// On-air seconds this frame occupies, delivered or not.
    pub airtime_s: f64,
    pub delivered: bool,
}

impl FrameSlot {
    /// `None` for a reserved/invalid level (8, or anything outside
    /// `SPEED_LEVELS`), or a degenerate zero-data-carrier profile -- propagated
    /// rather than defaulted, so a bad level can never contribute 0.0 airtime
    /// and blow the denominator up.
    pub fn for_level(level: u8, profile: &CoppaProfile, delivered: bool) -> Option<Self> {
        Some(Self {
            level,
            // `max_payload_for_level` already subtracts the CRC-32 trailer
            // `CoppaTransceiver::transmit` appends, which is why this is NOT
            // `k_used_for_level` / `ModeInfo::info_bits`: those count the trailer
            // as payload and would over-report every arm by 4 bytes/frame.
            info_bits: max_payload_for_level(level)? * 8,
            // The profile is taken per call rather than per struct so a caller
            // can switch profiles mid-run (COP-2 Phase 4's CP arm) with no
            // signature churn here.
            airtime_s: frame_airtime_s(level, profile)?,
            delivered,
        })
    }

    /// Dinkelbach objective at shadow price `lambda` (bits/s).
    fn score(&self, lambda: f64) -> f64 {
        // An undelivered frame contributes NO bits but its full airtime, which is
        // precisely what turns the objective on a dead frame into `-lambda *
        // airtime`, i.e. `argmin(airtime)` -- see `oracle_goodput_bps`'s doc.
        let bits = if self.delivered {
            self.info_bits as f64
        } else {
            0.0
        };
        bits - lambda * self.airtime_s
    }
}

/// Airtime-normalized goodput (bits/s) for one arm. `0.0` for an empty arm or a
/// zero-airtime denominator (mirrors `metrics::aggregate`'s own guard).
pub fn goodput_bps(slots: &[FrameSlot]) -> f64 {
    let airtime_s: f64 = slots.iter().map(|s| s.airtime_s).sum();
    // `<= 0.0` rather than `== 0.0`: it also rejects the (impossible via
    // `for_level`, possible via a hand-built slot) negative case, and it is
    // false for NaN, which then propagates a NaN goodput -- deliberately, since
    // every argmax here uses a strict `>` that NaN loses, so a NaN arm can never
    // silently win a comparison. Mirrors `metrics::aggregate`'s guard
    // (`crates/coppa-bench/src/metrics.rs:106-110`).
    if airtime_s <= 0.0 {
        return 0.0;
    }
    delivered_bits(slots) as f64 / airtime_s
}

/// LEGACY (airtime-blind) numerator: total delivered info bits. Retained only so
/// the pre-COP-2 bits-per-slot figures can be reported as a refactor control.
pub fn delivered_bits(slots: &[FrameSlot]) -> usize {
    slots
        .iter()
        .filter(|s| s.delivered)
        .map(|s| s.info_bits)
        .sum()
}

/// Best fixed arm by airtime-normalized goodput: `(arm_index, goodput_bps)`.
/// Ties go to the LOWEST index (strict `>`), i.e. the more conservative level.
pub fn best_arm(arms: &[Vec<FrameSlot>]) -> Option<(usize, f64)> {
    // A manual fold, not `max_by_key`: `f64` is not `Ord`, so the pre-COP-2
    // `max_by_key(|(_, b)| **b)` over integer bit totals
    // (`closed_loop_arq.rs:186-191`) does not carry over. `partial_cmp` +
    // `unwrap` would panic on a NaN denominator; the strict `>` below instead
    // makes NaN lose every comparison and keeps the lowest index on a tie, which
    // is what makes the reported `best_fixed_level` reproducible run to run.
    let mut best_idx = 0usize;
    let mut best_goodput = goodput_bps(arms.first()?);
    for (idx, arm) in arms.iter().enumerate().skip(1) {
        let goodput = goodput_bps(arm);
        if goodput > best_goodput {
            best_idx = idx;
            best_goodput = goodput;
        }
    }
    Some((best_idx, best_goodput))
}

/// Per-frame oracle under an airtime denominator. `frames[f]` holds one candidate
/// per cell -- what that cell actually did on frame `f`. Returns
/// `(goodput_bps, chosen_candidate_index_per_frame)`.
///
/// Maximizing `sum(bits) / sum(airtime)` over independent per-frame choices is a
/// LINEAR-FRACTIONAL program: the oracle's own choice moves the denominator, so
/// greedily taking the best per-frame `bits/airtime` is NOT optimal. Solved by
/// Dinkelbach: at shadow price `lambda`, pick `argmax(bits - lambda*airtime)`,
/// then set `lambda` to the resulting ratio, to a fixed point (cap 64
/// iterations, which returns the last feasible -- hence UNDER-stating, never
/// over-stating -- lambda).
///
/// On a frame where NO cell delivered, the rule degenerates to `argmin(airtime)`
/// -- the oracle minimizes wasted air rather than being undefined -- but ONLY for
/// `lambda > 0`. On iteration 1 (`lambda = 0`) every candidate on such a frame
/// scores `0.0` and the strict-`>` tie-break picks index 0, i.e. the LOWEST level,
/// which is the most expensive. That is not a correctness bug (the iteration
/// recovers from step 2), but it means the load-bearing guarantee is
/// `oracle_is_never_worse_than_any_fixed_arm` -- which follows from convergence,
/// not from iteration 1 -- and NOT any claim about iteration 1 reproducing the
/// legacy choice. Ties go to the LOWEST candidate index, for determinism.
pub fn oracle_goodput_bps(frames: &[Vec<FrameSlot>]) -> (f64, Vec<usize>) {
    /// Iteration cap. Dinkelbach terminates exactly on a finite candidate set
    /// (each round either strictly raises `lambda` or repeats the previous
    /// choice), so this is a guard against a pathological/NaN input rather than a
    /// tuning parameter -- 64 is far beyond the handful of rounds any real bench
    /// schedule needs.
    const MAX_ITERS: usize = 64;

    let mut lambda = 0.0f64;
    let mut choices: Vec<usize> = vec![0; frames.len()];

    for _ in 0..MAX_ITERS {
        // Per-frame argmax of the shadow-priced objective. Ties keep the LOWEST
        // index (strict `>`), so the whole function is deterministic even when
        // several cells score identically -- which happens on every frame no cell
        // delivered at `lambda = 0`.
        let picks: Vec<usize> = frames
            .iter()
            .map(|candidates| {
                let mut best_idx = 0usize;
                let mut best_score = f64::NEG_INFINITY;
                for (idx, candidate) in candidates.iter().enumerate() {
                    let score = candidate.score(lambda);
                    if score > best_score {
                        best_idx = idx;
                        best_score = score;
                    }
                }
                best_idx
            })
            .collect();

        let mut bits = 0usize;
        let mut airtime_s = 0.0f64;
        for (frame, &idx) in picks.iter().enumerate() {
            // `get` rather than `[idx]`: a frame with NO candidates records index
            // 0 and contributes neither bits nor airtime, instead of panicking on
            // an empty candidate list.
            if let Some(candidate) = frames[frame].get(idx) {
                if candidate.delivered {
                    bits += candidate.info_bits;
                }
                airtime_s += candidate.airtime_s;
            }
        }
        if airtime_s <= 0.0 {
            // No air spent at all (empty run, or every frame candidate-less):
            // same 0.0-not-NaN convention as `goodput_bps`.
            return (0.0, picks);
        }

        let ratio = bits as f64 / airtime_s;
        if ratio <= lambda {
            // Fixed point: the argmax at this `lambda` cannot beat `lambda`
            // itself, so the previously accepted (`lambda`, `choices`) pair is
            // optimal. Breaking BEFORE adopting `picks` keeps the returned
            // `lambda` exactly the ratio the returned `choices` achieve -- the
            // same reason the `MAX_ITERS` bail-out under-states rather than
            // over-states.
            break;
        }
        lambda = ratio;
        choices = picks;
    }

    (lambda, choices)
}

/// LEGACY (airtime-blind) best-arm: argmax of `delivered_bits`, strict `>`, ties
/// low. NOT interchangeable with `best_arm` -- see test 14.
pub fn best_arm_by_bits(arms: &[Vec<FrameSlot>]) -> Option<(usize, usize)> {
    // Same shape as `best_arm`'s fold rather than `max_by_key`, so the two
    // legacy/primary argmaxes break ties identically (lowest index) and a
    // difference between them can only ever come from the METRIC, never from the
    // tie-break -- which is the whole point of reporting both.
    let mut best_idx = 0usize;
    let mut best_bits = delivered_bits(arms.first()?);
    for (idx, arm) in arms.iter().enumerate().skip(1) {
        let bits = delivered_bits(arm);
        if bits > best_bits {
            best_idx = idx;
            best_bits = bits;
        }
    }
    Some((best_idx, best_bits))
}

/// LEGACY (airtime-blind) oracle: per frame, the most info bits any cell
/// delivered. Exactly the pre-COP-2 transpose-max at `closed_loop_arq.rs:196-206`.
pub fn oracle_bits(frames: &[Vec<FrameSlot>]) -> usize {
    frames
        .iter()
        .map(|candidates| {
            candidates
                .iter()
                .filter(|c| c.delivered)
                .map(|c| c.info_bits)
                .max()
                // A frame no cell delivered contributes nothing, exactly as the
                // pre-COP-2 `best = 0` initialization did.
                .unwrap_or(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_ml::VALID_SPEED_LEVELS;

    /// Number of frames in the deterministic matrix tests 11, 13 and 14 share.
    const MATRIX_FRAMES: usize = 40;

    /// The deterministic 40-frame x 9-level delivery matrix that
    /// `oracle_is_never_worse_than_any_fixed_arm`,
    /// `legacy_delivered_bits_and_oracle_bits_reproduce_the_slot_metric` and
    /// `best_arm_by_bits_is_the_legacy_argmax_and_can_differ_from_the_goodput_argmax`
    /// all measure against: `delivered = (f + level_idx) % 3 != 0`. No RNG on
    /// purpose -- every total the tests assert is hand-derivable from that one
    /// congruence (see each test's own derivation comment), so a future change to
    /// this helper cannot quietly re-base the expected numbers.
    ///
    /// Returns `(arms, frames)`: `arms[level_idx][frame]` (what one fixed-level
    /// policy did) and its transpose `frames[frame][level_idx]` (what every cell
    /// did on one frame), which is exactly the pair of shapes `best_arm` and
    /// `oracle_goodput_bps` consume.
    fn deterministic_matrix() -> (Vec<Vec<FrameSlot>>, Vec<Vec<FrameSlot>>) {
        let profile = CoppaProfile::hf_robust();
        let arms: Vec<Vec<FrameSlot>> = VALID_SPEED_LEVELS
            .iter()
            .enumerate()
            .map(|(li, &lvl)| {
                (0..MATRIX_FRAMES)
                    .map(|f| {
                        FrameSlot::for_level(lvl, &profile, (f + li) % 3 != 0)
                            .expect("every VALID_SPEED_LEVELS entry is a real level")
                    })
                    .collect()
            })
            .collect();
        let frames: Vec<Vec<FrameSlot>> = (0..MATRIX_FRAMES)
            .map(|f| arms.iter().map(|arm| arm[f]).collect())
            .collect();
        (arms, frames)
    }

    /// Test 1. Reconciles this module against `BENCHMARKS.md`'s canonical
    /// `payload_bits * (1 - FER) / frame_airtime` convention: for a fixed-level
    /// arm the ratio-of-sums form must reduce to it *exactly*, not approximately.
    #[test]
    fn fixed_arm_goodput_matches_the_canonical_payload_bits_times_one_minus_fer_formula() {
        let profile = CoppaProfile::hf_robust();
        let slots: Vec<FrameSlot> = (0..100)
            .map(|f| FrameSlot::for_level(2, &profile, f < 80).expect("level 2 is valid"))
            .collect();

        // 80/100 delivered => FER 0.2; level 2 on hf_robust carries 936 payload
        // bits in 1.60125 s (derived in
        // `frame_slot_airtime_matches_hand_calculated_hf_robust_values`).
        let expected = 936.0 * (1.0 - 0.2) / 1.60125;
        let got = goodput_bps(&slots);
        assert!(
            (got - expected).abs() < 1e-9,
            "expected {expected} bps (canonical form), got {got}"
        );
    }

    /// Test 2. The single most likely wrong implementation is a delivered-only
    /// denominator, which collapses the metric to the on-air rate and drops FER
    /// entirely. Pinned by measuring the SAME 80 delivered frames twice: alone,
    /// and with 20 lost frames appended. A delivered-only denominator returns the
    /// same number for both; a correct one does not.
    #[test]
    fn undelivered_frames_stay_in_the_airtime_denominator() {
        let profile = CoppaProfile::hf_robust();
        let delivered: Vec<FrameSlot> = (0..80)
            .map(|_| FrameSlot::for_level(2, &profile, true).expect("level 2 is valid"))
            .collect();
        let mut with_losses = delivered.clone();
        with_losses.extend(
            (0..20).map(|_| FrameSlot::for_level(2, &profile, false).expect("level 2 is valid")),
        );

        let lossless = goodput_bps(&delivered);
        let lossy = goodput_bps(&with_losses);
        assert!(
            (lossless - 936.0 / 1.60125).abs() < 1e-9,
            "80 delivered frames alone: expected {} bps, got {lossless}",
            936.0 / 1.60125
        );
        assert!(
            (lossy - 936.0 * 0.8 / 1.60125).abs() < 1e-9,
            "same 80 plus 20 lost: expected {} bps, got {lossy}",
            936.0 * 0.8 / 1.60125
        );
        assert!(
            lossy < lossless,
            "20 lost frames must COST goodput, not be free: {lossy} vs {lossless}"
        );
    }

    /// Test 3. Every zero-denominator path returns 0.0, never NaN/inf -- mirrors
    /// `metrics::aggregate`'s own `frame_airtime_s > 0.0` guard
    /// (`crates/coppa-bench/src/metrics.rs:106-110`). A NaN here would silently
    /// win or lose every strict-`>` argmax downstream.
    #[test]
    fn empty_or_zero_airtime_run_is_zero_not_nan() {
        assert_eq!(goodput_bps(&[]), 0.0);

        // A hand-built degenerate slot: the only way to get zero airtime past
        // `for_level`, which returns `None` instead.
        let degenerate = FrameSlot {
            level: 2,
            info_bits: 936,
            airtime_s: 0.0,
            delivered: true,
        };
        assert_eq!(goodput_bps(&[degenerate]), 0.0);

        let no_frames: [Vec<FrameSlot>; 0] = [];
        assert_eq!(oracle_goodput_bps(&no_frames), (0.0, vec![]));
    }

    /// Test 4. `info_bits` is APPLICATION payload bits, so it must come from
    /// `max_payload_for_level` (which subtracts the 4-byte CRC-32 trailer
    /// `CoppaTransceiver::transmit` appends), not from `ModeInfo::info_bits`
    /// (k_used: 972 / 1620, `crates/coppa-bench/src/scenario.rs:14`). Swapping
    /// them is silent and inflates every goodput figure by ~4%.
    #[test]
    fn frame_slot_info_bits_are_application_payload_bits_not_k_used() {
        let profile = CoppaProfile::hf_robust();
        let l2 = FrameSlot::for_level(2, &profile, true).expect("level 2 is valid");
        let l10 = FrameSlot::for_level(10, &profile, true).expect("level 10 is valid");

        // max_payload_for_level: 972/8 - 4 = 117 bytes; 1620/8 - 4 = 198 bytes.
        assert_eq!(l2.info_bits, 936);
        assert_eq!(l10.info_bits, 1584);
        assert_ne!(l2.info_bits, 972, "972 is k_used, not the payload budget");
        assert_ne!(
            l10.info_bits, 1620,
            "1620 is k_used, not the payload budget"
        );
    }

    /// Test 5. Pins the airtime numbers every other test's expected value is
    /// derived from, against a hand calculation rather than against
    /// `frame_airtime_s` itself (which would be circular).
    #[test]
    fn frame_slot_airtime_matches_hand_calculated_hf_robust_values() {
        // hf_robust: 48 active carriers - 12 pilots => data_per_sym = 36, so
        // header_syms = ceil(PROTECTED_HEADER_CODED_BITS/36) = ceil(144/36) = 4,
        // and symbol_len = fft_size + cp_samples = 960 + 300 = 1260 at 48 kHz.
        // Frame total is PREAMBLE_SYMS(3) + header_syms(4) + payload_syms.
        //   L2  (BPSK,  1 bit/sym): coded_symbols = ceil(1944/1) = 1944,
        //       payload_syms = ceil(1944/36) = 54  => 61 syms => 61*1260/48000 = 1.60125 s
        //   L4  (QPSK,  2): 972 => 27 => 34 syms => 34*1260/48000 = 0.89250 s
        //   L7  (16QAM, 4): 486 => ceil(486/36) = 14 => 21 syms => 21*1260/48000 = 0.55125 s
        //   L10 (64QAM, 6): 324 => 9 => 16 syms => 16*1260/48000 = 0.42000 s
        let profile = CoppaProfile::hf_robust();
        for (level, expected) in [(2u8, 1.60125), (4, 0.89250), (7, 0.55125), (10, 0.42000)] {
            let slot = FrameSlot::for_level(level, &profile, true).expect("valid level");
            assert!(
                (slot.airtime_s - expected).abs() < 1e-9,
                "level {level}: expected {expected}s, got {}",
                slot.airtime_s
            );
        }
    }

    /// Test 6. A reserved or out-of-range level must propagate `None`, never a
    /// 0.0-airtime slot: one such slot in an arm makes that arm's goodput
    /// infinite (or, worse, plausible) instead of failing loudly.
    #[test]
    fn frame_slot_rejects_reserved_level_8() {
        let profile = CoppaProfile::hf_robust();
        assert!(
            FrameSlot::for_level(8, &profile, true).is_none(),
            "level 8 is reserved (32-QAM) and absent from SPEED_LEVELS"
        );
        assert!(FrameSlot::for_level(255, &profile, true).is_none());
    }

    /// Test 7. The seam Phase 4's CP arm rests on: `hf_standard_short_cp` differs
    /// from `hf_standard` only in `cp_samples` (144 vs 300), and `frame_airtime_s`
    /// factorizes as `total_syms(level) * (fft_size + cp_samples) / sample_rate`
    /// with `total_syms` identical across the pair (both 44 data / 4 pilot). So
    /// the CP enters as ONE constant divisor -- `1104/1260` at every level -- which
    /// is exactly why a uniform CP change cannot move an airtime-normalized RATIO
    /// (see `crates/coppa-protocol/src/modem/airtime.rs`'s
    /// `short_cp_scales_frame_airtime_by_one_constant_at_every_level`).
    #[test]
    fn short_cp_reduces_frame_airtime_by_the_cp_ratio() {
        let long = CoppaProfile::hf_standard();
        let short = CoppaProfile::hf_standard_short_cp();
        let expected = 1104.0 / 1260.0;
        for level in [2u8, 4, 7, 10] {
            let l = FrameSlot::for_level(level, &long, true).expect("valid level");
            let s = FrameSlot::for_level(level, &short, true).expect("valid level");
            let ratio = s.airtime_s / l.airtime_s;
            assert!(
                (ratio - expected).abs() < 1e-12,
                "level {level}: short/long = {ratio}, expected exactly {expected}"
            );
        }

        // The absolute anchor, matching airtime.rs's own hand calculation
        // (`frame_airtime_level2_hf_standard_matches_hand_calc`): 52 syms *
        // 1260 / 48000 = 1.365 s.
        let l2 = FrameSlot::for_level(2, &long, true).expect("level 2 is valid");
        assert!(
            (l2.airtime_s - 1.365).abs() < 1e-9,
            "level 2 / hf_standard: expected 1.365s, got {}",
            l2.airtime_s
        );
    }

    /// Test 8. The oracle's dead-frame rule degenerates to `argmin(airtime)`, and
    /// test 10's "cheapest cell is the HIGHEST level" expectation only holds
    /// because airtime never rises as the ladder climbs. Pinned here so a future
    /// `SPEED_LEVELS` edit that broke the monotonicity fails with a clear message
    /// rather than making test 10 look like an oracle bug.
    #[test]
    fn airtime_is_non_increasing_along_the_speed_ladder() {
        for profile in [CoppaProfile::hf_robust(), CoppaProfile::hf_standard()] {
            let mut prev = f64::INFINITY;
            for &level in VALID_SPEED_LEVELS.iter() {
                let slot = FrameSlot::for_level(level, &profile, true).expect("valid level");
                assert!(
                    slot.airtime_s <= prev,
                    "level {level}: airtime {} rose above the previous level's {prev}",
                    slot.airtime_s
                );
                prev = slot.airtime_s;
            }
        }
    }

    /// Test 9. The counterexample that makes the Dinkelbach loop load-bearing.
    /// Frame 0 offers a big-but-slow candidate (1000 bits / 0.2 s = 5000 bps) and
    /// a tiny-but-fast one (100 bits / 0.01 s = 10 000 bps); frame 1 is a dead
    /// 10-second frame either way. A greedy per-frame `bits/airtime` argmax takes
    /// the 10 000 bps candidate and scores `100/10.01` -- about 9.99 bps -- because
    /// frame 1's fixed 10 s of air dwarfs the saving. The ratio-of-sums optimum
    /// takes the other and scores `1000/10.2` = 98.0392... bps, roughly 10x
    /// better. Asserted against the EXPRESSIONS, not rounded literals: `98.04`
    /// within `1e-6` would fail against a correct implementation by ~800x.
    #[test]
    fn oracle_maximizes_the_ratio_of_sums_not_the_per_frame_rate() {
        let frames = vec![
            vec![
                FrameSlot {
                    level: 2,
                    info_bits: 1000,
                    airtime_s: 0.2,
                    delivered: true,
                },
                FrameSlot {
                    level: 10,
                    info_bits: 100,
                    airtime_s: 0.01,
                    delivered: true,
                },
            ],
            vec![
                FrameSlot {
                    level: 2,
                    info_bits: 1000,
                    airtime_s: 10.0,
                    delivered: false,
                },
                FrameSlot {
                    level: 10,
                    info_bits: 100,
                    airtime_s: 10.0,
                    delivered: false,
                },
            ],
        ];

        let (goodput, choice) = oracle_goodput_bps(&frames);
        let optimum = 1000.0 / 10.2;
        let greedy = 100.0 / 10.01;
        assert!(
            (goodput - optimum).abs() < 1e-9,
            "expected the ratio-of-sums optimum {optimum} bps, got {goodput}"
        );
        assert!(
            goodput > greedy,
            "a greedy per-frame bits/airtime argmax scores {greedy} bps; the oracle must beat it, got {goodput}"
        );
        assert_eq!(
            choice[0], 0,
            "frame 0 must take the big-but-slow candidate, not the fast one"
        );
    }

    /// Test 10. On a frame where nothing decoded, the objective has no bits to
    /// gain, so it minimizes wasted air: the CHEAPEST cell (0.42 s), not the one
    /// with the largest nominal payload. An `argmax(bits)` oracle would burn
    /// 1.60 s of air for zero bits. Needs a live frame in the run too, so that
    /// `lambda` becomes positive -- at `lambda = 0` every dead candidate scores
    /// 0.0 and the tie-break picks index 0 (see `oracle_goodput_bps`'s doc).
    ///
    /// COP-2's plan predicted the L10 index (8) here; the real `hf_robust`
    /// airtime table makes that **arithmetically impossible** and the prediction
    /// is corrected rather than forced. L9 and L10 are both 64-QAM (6 bits per
    /// constellation symbol), so both need `ceil(1944/6) = 324` coded symbols =
    /// `ceil(324/36) = 9` payload OFDM symbols and cost the *identical* 0.42 s
    /// (verified against `frame_airtime_s` directly). The minimum-airtime cell is
    /// therefore not unique, and `oracle_goodput_bps`'s own documented
    /// ties-to-lowest-index rule -- the thing that makes the reported choice
    /// reproducible -- must select L9 (index 7). The property under test is
    /// "cheapest air, not most bits", so it is asserted as exactly that, plus the
    /// tie-break's index: an `argmax(nominal bits)` oracle picks index 8 and an
    /// `argmax(delivered bits)` one ties at index 0 (L1, 1.60125 s), so both
    /// still fail.
    #[test]
    fn oracle_picks_the_cheapest_level_on_a_frame_where_nothing_delivered() {
        let profile = CoppaProfile::hf_robust();
        let live: Vec<FrameSlot> = VALID_SPEED_LEVELS
            .iter()
            .map(|&lvl| FrameSlot::for_level(lvl, &profile, true).expect("valid level"))
            .collect();
        let dead: Vec<FrameSlot> = VALID_SPEED_LEVELS
            .iter()
            .map(|&lvl| FrameSlot::for_level(lvl, &profile, false).expect("valid level"))
            .collect();

        let cheapest_airtime = dead
            .iter()
            .map(|c| c.airtime_s)
            .fold(f64::INFINITY, f64::min);
        assert!(
            (cheapest_airtime - 0.42).abs() < 1e-9,
            "the cheapest hf_robust cell is L9/L10's 0.42s, got {cheapest_airtime}"
        );

        let frames = vec![live, dead.clone()];
        let (_goodput, choice) = oracle_goodput_bps(&frames);
        let picked = dead[choice[1]];
        assert!(
            (picked.airtime_s - cheapest_airtime).abs() < 1e-12,
            "a dead frame must cost the least air available ({cheapest_airtime}s), \
             not the most bits: picked L{} at {}s",
            picked.level,
            picked.airtime_s
        );
        assert_eq!(
            choice[1], 7,
            "L9 and L10 tie at 0.42s, so the documented lowest-index tie-break picks L9"
        );
        assert_eq!(VALID_SPEED_LEVELS[7], 9);
    }

    /// Test 11. The structural guarantee: every constant-level policy is in the
    /// oracle's own feasible set (pick the same cell on every frame), so the
    /// oracle can never score below the best fixed arm. Catches sign and
    /// denominator errors that each individual test above would pass.
    #[test]
    fn oracle_is_never_worse_than_any_fixed_arm() {
        let (arms, frames) = deterministic_matrix();
        let (oracle, _choice) = oracle_goodput_bps(&frames);
        for (li, arm) in arms.iter().enumerate() {
            let fixed = goodput_bps(arm);
            assert!(
                oracle >= fixed - 1e-9,
                "oracle {oracle} bps fell below fixed arm {li} (L{}) at {fixed} bps",
                VALID_SPEED_LEVELS[li]
            );
        }

        let (best_idx, best_fixed) = best_arm(&arms).expect("nine arms");
        assert!(
            oracle >= best_fixed - 1e-9,
            "oracle {oracle} bps fell below best fixed arm {best_idx} at {best_fixed} bps"
        );
    }

    /// Test 12. Determinism: an argmax over `f64` has no canonical tie-break, and
    /// the report names a LEVEL, so a coin-flip between two equally-good arms
    /// would make the recorded `best_fixed_level` unreproducible. Strict `>`
    /// keeps the lowest index, i.e. the more conservative level.
    #[test]
    fn best_arm_breaks_ties_toward_the_lower_level() {
        let profile = CoppaProfile::hf_robust();
        let arm = vec![
            FrameSlot::for_level(2, &profile, true).expect("level 2 is valid"),
            FrameSlot::for_level(2, &profile, false).expect("level 2 is valid"),
        ];
        let (idx, goodput) = best_arm(&[arm.clone(), arm]).expect("two arms");
        assert_eq!(idx, 0);
        assert!((goodput - 936.0 / (2.0 * 1.60125)).abs() < 1e-9);

        // Two DIFFERENTLY-shaped arms with identical goodput, so the tie is not
        // an artifact of comparing an arm against a copy of itself.
        let slow = vec![FrameSlot {
            level: 2,
            info_bits: 2000,
            airtime_s: 2.0,
            delivered: true,
        }];
        let fast = vec![FrameSlot {
            level: 10,
            info_bits: 1000,
            airtime_s: 1.0,
            delivered: true,
        }];
        assert_eq!(best_arm(&[slow, fast]), Some((0, 1000.0)));

        let no_arms: [Vec<FrameSlot>; 0] = [];
        assert_eq!(best_arm(&no_arms), None);
    }

    /// Test 13. The legacy (airtime-blind) functions are what the side-by-side
    /// refactor-control block in `closed_loop_arq` reports, so they must be
    /// provably the SAME quantity the pre-COP-2 record measured -- otherwise a
    /// divergence in that block cannot distinguish "the metric swap perturbed
    /// something" from "the legacy helper is a different function now".
    #[test]
    fn legacy_delivered_bits_and_oracle_bits_reproduce_the_slot_metric() {
        let (arms, frames) = deterministic_matrix();

        // Delivered count per arm: `(f + li) % 3 != 0` fails for the 13 or 14
        // frames in 0..40 with `f = (-li) mod 3`. Residue populations in 0..40
        // are 14 (r=0: 0,3,..,39), 13 (r=1: 1,4,..,37) and 13 (r=2: 2,5,..,38),
        // so li % 3 == 0 loses 14 frames and every other li loses 13.
        // info_bits per level = max_payload_for_level*8: L1 448, L2 936, L3 936,
        // L4 1424, L5 1264, L6 936, L7 1424, L9 1264, L10 1584.
        let expected_bits = [
            26 * 448,  // L1  = 11648
            27 * 936,  // L2  = 25272
            27 * 936,  // L3  = 25272
            26 * 1424, // L4  = 37024
            27 * 1264, // L5  = 34128
            27 * 936,  // L6  = 25272
            26 * 1424, // L7  = 37024
            27 * 1264, // L9  = 34128
            27 * 1584, // L10 = 42768
        ];
        for (li, arm) in arms.iter().enumerate() {
            assert_eq!(
                delivered_bits(arm),
                expected_bits[li],
                "arm {li} (L{})",
                VALID_SPEED_LEVELS[li]
            );
        }

        // Per-frame max over DELIVERED cells. L10 (1584, li=8) is undelivered
        // exactly when (f + 8) % 3 == 0, i.e. f % 3 == 1 (13 of the 40 frames);
        // on those the undelivered set is li in {2,5,8}, leaving L4/L7 (1424) as
        // the best surviving cell. So 27*1584 + 13*1424 = 42768 + 18512 = 61280.
        assert_eq!(oracle_bits(&frames), 61280);

        let no_frames: [Vec<FrameSlot>; 0] = [];
        assert_eq!(oracle_bits(&no_frames), 0);
    }

    /// Test 14. Required, not decorative. The pre-COP-2 record's `best fixed (L4)
    /// : 357424 bits` is an argmax over BITS; `best_arm` is an argmax over
    /// GOODPUT, and this phase's whole hypothesis is that the two disagree
    /// (predicted L4 -> L7 on `hf_robust`). Reusing `best_arm` for the legacy
    /// block would manufacture a false control failure and send a reader hunting
    /// a regression that does not exist.
    #[test]
    fn best_arm_by_bits_is_the_legacy_argmax_and_can_differ_from_the_goodput_argmax() {
        let (arms, _frames) = deterministic_matrix();
        // From test 13's totals, L10 (li 8) wins on bits (42768) -- and on this
        // particular matrix it also wins on goodput, which is why the divergence
        // needs its own hand-built case below.
        assert_eq!(best_arm_by_bits(&arms), Some((8, 42768)));

        // Arm 0: two delivered L2 frames -- 1872 bits, but 2 * 1.60125 = 3.2025 s
        // of air (584.5 bps). Arm 1: one delivered L10 frame -- fewer bits
        // (1584) in 0.42 s (3771.4 bps). Bits pick arm 0; goodput picks arm 1.
        let profile = CoppaProfile::hf_robust();
        let bits_heavy =
            vec![FrameSlot::for_level(2, &profile, true).expect("level 2 is valid"); 2];
        let air_cheap = vec![FrameSlot::for_level(10, &profile, true).expect("level 10 is valid")];
        let two = [bits_heavy, air_cheap];

        assert_eq!(best_arm_by_bits(&two), Some((0, 1872)));
        assert_eq!(
            best_arm(&two).expect("two arms").0,
            1,
            "the airtime-normalized argmax must be able to disagree with the bits argmax"
        );

        let no_arms: [Vec<FrameSlot>; 0] = [];
        assert_eq!(best_arm_by_bits(&no_arms), None);
    }
}
