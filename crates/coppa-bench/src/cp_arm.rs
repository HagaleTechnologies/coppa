//! CP-adaptive arm mechanics for `closed_loop_arq` (COP-2 Phase 4): the CP-mode →
//! profile mapping, the switch policy (`CpGate` plus the daemon's transition
//! detection plus COP-1's re-entrancy guard), the airtime the CP handshake
//! charges, per-frame seed derivation, and the delay-spread diagnostic.
//!
//! # Why this is a lib module and not part of the example
//!
//! CI compiles `crates/coppa-bench/examples/*.rs` (via
//! `cargo clippy --workspace --all-targets`) but **never runs them**, so any
//! arithmetic or state machine left inside an example's `main()` is invisible to
//! `cargo test --workspace`. An inverted re-entrancy guard or a handshake charged
//! five frames at one profile instead of 3 + 2 would then only ever be caught by a
//! human reading a multi-minute run's output — and this repo has been burned twice
//! by exactly that: the stale IR-HARQ accumulator in `runner.rs` (PR #61/#62) and
//! the `calibrated_bias` saturation behind PR #69 were both invisible to
//! inspection and only fell out of tests that reproduced them. Everything here is
//! therefore pure (no transceiver, no channel) and unit-tested; the example keeps
//! only the transmit/receive loop that genuinely needs a `CoppaTransceiver`.
//!
//! # What this mirrors, and where it deliberately diverges from production
//!
//! - [`profile_for`] mirrors `CoppaCore::select_ofdm_profile`'s HF branch
//!   (`crates/coppa-engine/src/engine.rs:150-159`) — duplicated, not shared,
//!   because `coppa-bench` does not depend on `coppa-engine`. The divergence
//!   (production sends every level >= 5 to `vhf_wide` and ignores the CP mode
//!   there) is spelled out on that function and makes the CP-adaptive arm's
//!   benefit an **upper bound**.
//! - [`CpPolicy`] mirrors `coppa-daemon`'s `decode_and_dispatch_audio`
//!   (`crates/coppa-daemon/src/event_loop.rs:1093-1124`): observe only on a
//!   *decoded* frame, act only on a recommendation **transition**, and never start
//!   a second negotiation while one is in flight.
//! - [`switch_airtime_s`] charges the handshake's five droppable on-air frames,
//!   split 3 under the old profile / 2 under the new one, which is `cp_negotiator`'s
//!   own count (`crates/coppa-protocol/src/cp_negotiator.rs`'s six-step diagram;
//!   step 5 is a local state change and costs no air).
//!
//! What it does **not** model, all of which biases the CP-adaptive arm optimistic
//! and is why the report shows the switch cost at ×1 / ×6 / ×0 rather than a single
//! number: ARQ retransmissions of the control legs (`DEFAULT_MAX_RETRANSMIT = 5` is
//! the ×6 ceiling), the four half-duplex turnarounds at `DEFAULT_TURNAROUND = 150
//! ms`, and handshakes that fail and revert via G1-G4 having spent the full budget
//! for zero CP change. [`CpPolicy`] assumes every decided switch completes.

use coppa_codec::ofdm::CoppaProfile;
use coppa_ml::{CpGate, CpRecommendation};
use coppa_protocol::cp_negotiator::CpMode;
use coppa_protocol::modem::frame_airtime_s;

/// Mirror of `CoppaCore::select_ofdm_profile`'s HF branch (`engine.rs:150-159`).
/// `coppa-bench` does not depend on `coppa-engine`, so this mapping is duplicated
/// and pinned by `profile_for_*_geometry` rather than shared.
///
/// NOTE the deliberate divergence from production: `select_ofdm_profile` routes
/// EVERY level >= 5 to `vhf_wide()` and ignores `cp_mode` there. This bench
/// forces one HF profile family for all nine levels (as it already did with
/// `hf_robust`), so a CP mode applies at every level here -- which makes arm B's
/// benefit an UPPER BOUND on what production could collect.
pub fn profile_for(mode: CpMode) -> CoppaProfile {
    match mode {
        CpMode::LongCp => CoppaProfile::hf_standard(),
        CpMode::ShortCp => CoppaProfile::hf_standard_short_cp(),
    }
}

/// `CoppaProfile` derives only `Debug, Clone` (mod.rs:155) -- no `PartialEq` -- so
/// compare the two fields that distinguish the CP pair. Deliberately NOT adding a
/// derive to a `coppa-codec` public type from a bench phase. Pinned by a test.
pub fn same_profile(a: &CoppaProfile, b: &CoppaProfile) -> bool {
    a.cp_samples == b.cp_samples && a.bandwidth_id == b.bandwidth_id
}

/// On-air frames of one completed handshake sent under the OLD profile: steps 1
/// (`Propose`), 2 (`Confirm`) and 3 (the bare ack for the Confirm).
pub const HANDSHAKE_FRAMES_OLD_PROFILE: usize = 3;

/// ...and under the NEW profile: step 4 (`CpSwitched`, COP-1's third leg) and
/// step 6 (its bare ack). 3 + 2 = 5 is `cp_negotiator`'s own count of droppable
/// on-air frames; step 5 is B's local state change and costs no air.
pub const HANDSHAKE_FRAMES_NEW_PROFILE: usize = 2;

/// Data-frame slots a completed handshake displaces before the new mode takes
/// effect. Each control frame costs a full frame's airtime at the current level
/// (`airtime.rs:44-51`: `transmit` always rate-matches to a fixed
/// `CODED_BLOCK_LEN`, so a small CpControl PDU is not cheaper than a data frame).
/// DERIVED, NOT SWEPT -- same status as `SWITCH_PROBATION_SECS = 180`; the
/// zero-latency arm bounds its influence.
pub const SWITCH_LATENCY_FRAMES: usize = 5;

/// One applied CP switch: what the policy decided, when it decided it, and when
/// the new profile actually takes effect.
///
/// `level` is the speed level of the frame whose measurement *decided* the
/// switch, and it is what [`switch_airtime_s`] charges the handshake at. That is
/// an approximation with a named direction: the real CP-control `ArqTx` sends its
/// legs at whatever level the link is using when each leg goes out, and `RateLoop`
/// can move during the `effective_from - decided_at` window. Charging at the
/// deciding frame's level is the only level this type knows, and it is the level
/// the link was actually on at the moment the daemon would have sent the
/// `Propose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpSwitch {
    /// Mode in effect when the switch was decided — what steps 1-3 are sent on.
    pub from: CpMode,
    /// Mode the handshake converges on — what steps 4 and 6 are sent on.
    pub to: CpMode,
    /// Frame index whose delay-spread measurement produced the gate transition.
    pub decided_at: usize,
    /// First frame index transmitted under `to`: `decided_at + 1 + latency_frames`.
    pub effective_from: usize,
    /// Speed level of the deciding frame; see the type doc for why.
    pub level: u8,
}

/// The sender-side CP decision the CP-adaptive arm runs, composed from three
/// pieces of real production behaviour rather than invented for the bench:
///
/// 1. [`CpGate::default_coppa`]'s hysteresis — 4 consecutive frames strictly
///    below 2.5 ms to recommend short CP, one frame at or above it to drop back
///    (`crates/coppa-ml/src/cp_gate.rs:85`/`:91`).
/// 2. The daemon's **transition** detection: compare the gate's recommendation
///    before and after the observation and act only when it *changed*
///    (`crates/coppa-daemon/src/event_loop.rs:1093-1096`). A sustained `ShortCp`
///    recommendation must not re-propose every frame.
/// 3. COP-1's **re-entrancy guard**: a transition arriving while a switch is
///    already in flight is dropped, not queued
///    (`crates/coppa-daemon/src/event_loop.rs:1103-1124`, and
///    `cp_negotiator`'s "One negotiation at a time" section). There is one
///    `CpNegotiator` and one `cp_propose_seq` per daemon, so a second `Propose`
///    used to orphan the first seq. Modelling a queue here would credit the arm
///    with responsiveness the shipped daemon does not have.
///
/// # Undecoded frames: the gate is *frozen*, not fed a synthetic value
///
/// [`CpPolicy::observe`] with `delay_spread_ms == None` feeds the gate **nothing**,
/// because the daemon feeds it nothing: `CpGate::observe` has exactly two
/// behaviours ("calm, advance the run" and "at/above threshold, reset and drop"),
/// so any synthesized value for a frame that did not decode would invent one of
/// them. The daemon invents neither — it only reaches the gate inside the
/// `Ok(payload)` arm (`event_loop.rs:1078-1096`).
///
/// The consequence is real and must be reported rather than smoothed over: on a
/// stretch where nothing decodes, the recommendation is frozen at its last value,
/// so an arm parked on `ShortCp` when the channel collapses **cannot drop back**
/// until something decodes again. If a run exhibits that, it is a finding about
/// the shipped policy, not a bench defect.
///
/// # Ordering contract (load-bearing — a test depends on it)
///
/// `current` advances **only** inside [`CpPolicy::mode_for_frame`], and
/// [`CpPolicy::observe`] returns `None` unconditionally while a switch is pending.
/// The arm loop must therefore call `mode_for_frame(f)` (to pick the profile for
/// frame `f`) *before* `observe(f, ..)` (to feed frame `f`'s measurement), exactly
/// as the daemon does — it selects a profile to transmit on before it sees the
/// next decode. At the production `SWITCH_LATENCY_FRAMES = 5`, a
/// threshold-crossing frame arriving inside the latency window is therefore
/// dropped by the guard rather than producing an immediate revert; only at latency
/// 0 does the revert land on the very next frame. Both behaviours are pinned.
///
/// # A `from == to` switch is reachable, and is recorded rather than suppressed
///
/// Because a dropped transition leaves the gate's recommendation possibly equal to
/// the mode the pending switch then applies, the *next* transition can decide a
/// switch whose `to` already equals `current`. The daemon has the same property —
/// it derives the proposed mode from the gate transition and never compares it
/// against `cp_negotiator.current()` — so this type records it faithfully: five
/// control frames of air spent for zero CP change. [`same_profile`] makes the
/// arm's transceiver rebuild a no-op in that case, which is also what
/// `set_cp_profile` would do (`cp_negotiator::tick`'s doc calls out the same
/// "switch to the mode it was already on" case).
pub struct CpPolicy {
    gate: CpGate,
    latency_frames: usize,
    current: CpMode,
    pending: Option<CpSwitch>,
    switches: Vec<CpSwitch>,
}

impl CpPolicy {
    /// `latency_frames` is the number of whole data-frame slots between the frame
    /// that decides a switch and the first frame transmitted under the new
    /// profile — [`SWITCH_LATENCY_FRAMES`] for the production-shaped arm, `0` for
    /// the sensitivity arm that bounds its influence.
    ///
    /// Boots on [`CpMode::LongCp`], matching `CpNegotiator::new`
    /// (`cp_negotiator.rs:331-339`) and `CpGate`'s own initial recommendation —
    /// the one mode both stations are known to agree on before any negotiation.
    pub fn new(latency_frames: usize) -> Self {
        Self {
            gate: CpGate::default_coppa(),
            latency_frames,
            current: CpMode::LongCp,
            pending: None,
            switches: Vec::new(),
        }
    }

    /// Feed frame `frame`'s outcome. `delay_spread_ms` is `Some` on **any**
    /// successful decode (that is the daemon's own signal — it has no payload
    /// oracle) and `None` on a decode failure.
    ///
    /// Returns `Some(CpSwitch)` only when the gate's recommendation *transitioned*
    /// on this frame **and** no switch is already pending; `None` in every other
    /// case, including a transition dropped by the re-entrancy guard. See the type
    /// doc for all three rules and for why a dropped transition is not queued.
    pub fn observe(
        &mut self,
        frame: usize,
        level: u8,
        delay_spread_ms: Option<f32>,
    ) -> Option<CpSwitch> {
        // No observation at all on an undecoded frame — see the type doc's
        // "the gate is *frozen*" section. Note this deliberately happens BEFORE
        // the pending check: a pending switch does not change what the gate is
        // allowed to see, only what the policy is allowed to do about it.
        let spread_ms = delay_spread_ms?;

        // The daemon's transition test verbatim (`event_loop.rs:1093-1096`):
        // `before` is the gate's standing recommendation, `after` is the one this
        // frame produced.
        let before = self.gate.current();
        let after = self.gate.observe(spread_ms);
        if after == before {
            return None;
        }

        // COP-1's re-entrancy guard. Checked AFTER the gate observation, again
        // matching the daemon: the gate's hysteresis state keeps tracking the
        // channel while a negotiation is in flight; only the proposing is
        // suppressed.
        if self.pending.is_some() {
            return None;
        }

        let sw = CpSwitch {
            from: self.current,
            to: match after {
                CpRecommendation::ShortCp => CpMode::ShortCp,
                CpRecommendation::LongCp => CpMode::LongCp,
            },
            decided_at: frame,
            // `+ 1` because the decision is made *from* frame `frame`'s decode,
            // which has already been transmitted: the earliest frame that could
            // possibly carry the new profile is the next one, and
            // `latency_frames` displaces it further.
            effective_from: frame + 1 + self.latency_frames,
            level,
        };
        self.pending = Some(sw);
        self.switches.push(sw);
        Some(sw)
    }

    /// CP mode frame `frame` must be transmitted under, applying a pending switch
    /// once `frame` reaches its `effective_from`.
    ///
    /// This is the **only** place `current` advances (see the type doc's ordering
    /// contract), so the arm loop must call it once per frame, before that frame's
    /// [`CpPolicy::observe`], even when it already knows the profile — skipping it
    /// on a frame would silently postpone every scheduled switch.
    pub fn mode_for_frame(&mut self, frame: usize) -> CpMode {
        if let Some(sw) = self.pending {
            if frame >= sw.effective_from {
                self.current = sw.to;
                self.pending = None;
            }
        }
        self.current
    }

    /// The mode in effect as of the last [`CpPolicy::mode_for_frame`] call,
    /// without advancing anything. For reporting only.
    pub fn current(&self) -> CpMode {
        self.current
    }

    /// Every switch this policy decided, in decision order — including any still
    /// pending at the end of the run, which is deliberate: its handshake air was
    /// spent even if its new profile never carried a data frame.
    pub fn switches(&self) -> &[CpSwitch] {
        &self.switches
    }
}

/// On-air seconds one completed CP handshake costs: [`HANDSHAKE_FRAMES_OLD_PROFILE`]
/// frames at `sw.from`'s profile plus [`HANDSHAKE_FRAMES_NEW_PROFILE`] at
/// `sw.to`'s, both at `sw.level`.
///
/// The split matters and is pinned in both directions: long→short and short→long
/// cost *different* amounts (at level 2, 6.487 s vs 6.318 s), so charging all five
/// frames at one profile — the obvious wrong implementation — is a test failure
/// rather than a silent bias.
///
/// `None` for a reserved/unknown level, mirroring `frame_airtime_s`'s own contract
/// (`airtime.rs:101-104`) instead of substituting a zero that would inflate the
/// arm's goodput.
///
/// This charge is the **completed-handshake floor**: it excludes control-leg
/// retransmissions (the ×6 cost multiplier in the report bounds those with
/// `DEFAULT_MAX_RETRANSMIT = 5`), the four half-duplex turnarounds, and
/// handshakes that spend the full budget and then revert via G1-G4 for zero CP
/// change. All three omissions favour the CP-adaptive arm.
pub fn switch_airtime_s(sw: &CpSwitch) -> Option<f64> {
    let old = frame_airtime_s(sw.level, &profile_for(sw.from))?;
    let new = frame_airtime_s(sw.level, &profile_for(sw.to))?;
    Some(HANDSHAKE_FRAMES_OLD_PROFILE as f64 * old + HANDSHAKE_FRAMES_NEW_PROFILE as f64 * new)
}

/// Airtime-normalized goodput with the CP handshake's air charged at
/// `cost_multiplier` × [`switch_airtime_s`].
///
/// `delivered_bits / (data_airtime_s + cost_multiplier * switch_airtime_s)`, in
/// bits per second. The multiplier exists so the report can show one arm at three
/// costs instead of committing to one guess about a cost this bench cannot
/// measure: `×1` is the loss-free floor (every leg delivered first try), `×6` the
/// ARQ ceiling (`ArqTx::is_failed` gives up past `DEFAULT_MAX_RETRANSMIT = 5`, so
/// six attempts per leg), and `×0` the no-charge upper bound — which must equal
/// `delivered_bits / data_airtime_s` **exactly**, the property that proves the
/// multiplier scales only the handshake term and nothing else.
///
/// Returns `0.0` rather than `inf`/`NaN` on a non-positive denominator, matching
/// the convention `adaptive_goodput` uses for an empty run: a zero-airtime arm
/// delivered nothing and must not out-rank every real arm.
pub fn goodput_with_switch_cost_bps(
    delivered_bits: f64,
    data_airtime_s: f64,
    switch_airtime_s: f64,
    cost_multiplier: f64,
) -> f64 {
    let denom = data_airtime_s + cost_multiplier * switch_airtime_s;
    if denom <= 0.0 {
        return 0.0;
    }
    delivered_bits / denom
}

/// Run seeds for the multi-seed aggregate. Seed `0` is **first and mandatory**:
/// `frame_seed(0, f) == f`, which is exactly the per-frame seed the pre-COP-2
/// `closed_loop_arq` used, so seed 0's arm reproduces the historical channel
/// realizations frame for frame. Without that anchor, adding seed threading would
/// be a third confound alongside the metric swap and the CP change, and a moved
/// number could not be attributed.
pub const SEEDS: [u64; 5] = [0, 1, 2, 3, 4];

/// Spacing between consecutive run seeds' frame-seed blocks. Must exceed the run's
/// frame count or two runs share channel realizations at an offset — the failure
/// this and `frame_seed_is_distinct_across_seeds_within_a_run` exist to prevent.
/// 1024 clears the 300-frame default with room for a `COPPA_CL_FRAMES` override up
/// to 1024; a larger override needs a larger stride, and the test is what will say
/// so.
pub const SEED_STRIDE: u64 = 1024;

/// Per-frame channel seed for run `run_seed`, frame `f`.
///
/// `run_seed * SEED_STRIDE + f`, chosen over hashing or XOR-mixing for one
/// reason: it makes `frame_seed(0, f) == f` hold identically, preserving the
/// pre-COP-2 bench's channel realizations at seed 0 (see [`SEEDS`]). The
/// downstream `apply_channel` already decorrelates fading from noise by XORing
/// this value with distinct per-use constants, so an unmixed input is not a
/// correlation hazard here.
pub fn frame_seed(run_seed: u64, f: usize) -> u64 {
    run_seed.wrapping_mul(SEED_STRIDE).wrapping_add(f as u64)
}

/// Active carriers (`nc`) shared by every profile this bench puts in play:
/// `hf_standard`, `hf_standard_short_cp` and `hf_robust` all have 48 active
/// carriers (44 + 4, or 36 + 12). `nc` is what
/// `DelayDomainEstimator::delay_spread_ms` normalizes its delay grid by
/// (`delay_domain.rs:396-398`).
const SPREAD_NC: usize = 48;

/// FFT size shared by every profile in play (`mod.rs`'s HF profiles are all 960).
const SPREAD_FFT_SIZE: usize = 960;

/// Sample rate shared by every profile in play — all Coppa profiles are unified at
/// 48 kHz.
const SPREAD_SAMPLE_RATE: u32 = 48_000;

/// One delay-domain grid unit, in milliseconds: `(fft_size / nc) / sample_rate *
/// 1000` = `(960/48)/48000*1000` = 0.41666... ms. Written in the same operation
/// order as `DelayDomainEstimator::delay_spread_ms`'s own final line
/// (`delay_domain.rs:396-398`) so the two agree to f32 rounding rather than
/// approximately.
pub const SPREAD_GRID_MS: f32 =
    (SPREAD_FFT_SIZE as f32 / SPREAD_NC as f32) / SPREAD_SAMPLE_RATE as f32 * 1000.0;

/// How far from an exact grid multiple a reading may sit and still be bucketed,
/// in grid units. 0.05 units (≈0.021 ms) is ~4 orders of magnitude above f32
/// rounding on this arithmetic and far below the smallest discrepancy a real
/// `nc`/`fft_size` change would introduce (an `nc` of 36 would put grid unit 1 at
/// 1.33 units of this grid). So the tolerance absorbs float noise without
/// absorbing the assumption break it is here to detect.
const GRID_TOLERANCE_UNITS: f32 = 0.05;

/// Exhaustive histogram of the per-frame measured delay spreads a run observed.
///
/// **Exhaustive, not approximate, and that is load-bearing.**
/// `DelayDomainEstimator::delay_spread_ms` returns `(last - first) * (fft_size /
/// nc) / sample_rate * 1000` over its fitted taps, `fit` clamps the tap count to
/// `1..=8` (`delay_domain.rs:289`), and every profile in play shares `nc = 48` /
/// `fft_size = 960` / 48 kHz — so `last - first` is an integer in `0..=7` and the
/// metric can take exactly EIGHT values, 0.000 ms through 2.9167 ms. Bucketing on
/// that grid means the diagnostic is a complete accounting rather than a
/// resolution choice, and `off_grid` is the tripwire: any non-zero count in it
/// says the `nc`/tap-clamp assumption above has changed and the whole histogram is
/// no longer interpretable. That is deliberately louder than silently smearing the
/// reading into the nearest bucket.
///
/// `undecoded` is counted separately from bucket 0 for the reason
/// [`CpPolicy::observe`] documents: a frame that did not decode gives the gate
/// nothing at all, which is a different event from a frame that measured a flat
/// channel — folding them together would hide the frozen-gate case entirely.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpreadHistogram {
    /// Count per grid unit: `counts[n]` is frames measuring `n * SPREAD_GRID_MS`.
    pub counts: [usize; 8],
    /// Frames that did not decode, so produced no measurement at all.
    pub undecoded: usize,
    /// Readings not on the eight-value grid — see the type doc; expected to be 0.
    pub off_grid: usize,
}

impl SpreadHistogram {
    /// Record one frame's outcome: `None` for an undecoded frame, `Some(ms)` for
    /// the `delay_spread_ms` a decoded frame measured.
    pub fn observe(&mut self, delay_spread_ms: Option<f32>) {
        let Some(ms) = delay_spread_ms else {
            self.undecoded += 1;
            return;
        };
        let units = ms / SPREAD_GRID_MS;
        let nearest = units.round();
        // The range test also disposes of NaN and negatives (both compare false
        // against every bound), so the cast below cannot be out of range.
        if !(0.0..=7.0).contains(&nearest) || (units - nearest).abs() > GRID_TOLERANCE_UNITS {
            self.off_grid += 1;
            return;
        }
        self.counts[nearest as usize] += 1;
    }

    /// Frames recorded, over all three categories. Used for the report's
    /// percentages; equals the run's frame count if every frame was observed.
    pub fn total(&self) -> usize {
        self.counts.iter().sum::<usize>() + self.undecoded + self.off_grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::CoppaProfile;
    use coppa_protocol::cp_negotiator::CpMode;

    #[test]
    fn profile_for_long_cp_is_hf_standard_geometry() {
        let p = profile_for(CpMode::LongCp);
        assert_eq!(p.cp_samples, 300);
        assert_eq!(p.bandwidth_id, 1);
        assert_eq!(p.data_carriers, 44);
        assert_eq!(p.pilot_carriers, 4);
    }

    #[test]
    fn profile_for_short_cp_is_hf_standard_short_cp_geometry() {
        let p = profile_for(CpMode::ShortCp);
        assert_eq!(p.cp_samples, 144);
        assert_eq!(p.bandwidth_id, 4);
        assert_eq!(p.data_carriers, 44);
        assert_eq!(p.pilot_carriers, 4);
    }

    /// Both terms of `same_profile` are load-bearing, and the CP pair alone does
    /// not prove it: `hf_standard` and `hf_standard_short_cp` differ in
    /// `cp_samples` *and* `bandwidth_id`, so a `cp_samples`-only comparison would
    /// still separate them. `hf_wide` is the case that pins the `bandwidth_id`
    /// term — same 300-sample CP as `hf_standard`, different profile — so a
    /// future profile pair differing only in carrier layout cannot slip past the
    /// arm loop's rebuild check.
    #[test]
    fn same_profile_distinguishes_the_long_and_short_cp_pair() {
        let long = CoppaProfile::hf_standard();
        let short = CoppaProfile::hf_standard_short_cp();
        assert!(same_profile(&long, &CoppaProfile::hf_standard()));
        assert!(!same_profile(&long, &short));
        assert!(!same_profile(&long, &CoppaProfile::hf_wide()));
    }

    #[test]
    fn cp_policy_starts_on_long_cp() {
        let mut policy = CpPolicy::new(SWITCH_LATENCY_FRAMES);
        assert_eq!(policy.mode_for_frame(0), CpMode::LongCp);
    }

    #[test]
    fn cp_policy_switches_to_short_cp_after_four_calm_frames() {
        let mut policy = CpPolicy::new(SWITCH_LATENCY_FRAMES);
        assert_eq!(policy.observe(0, 2, Some(0.5)), None);
        assert_eq!(policy.observe(1, 2, Some(0.5)), None);
        assert_eq!(policy.observe(2, 2, Some(0.5)), None);
        let sw = policy.observe(3, 2, Some(0.5)).expect("dwell of 4 reached");
        assert_eq!(sw.from, CpMode::LongCp);
        assert_eq!(sw.to, CpMode::ShortCp);
        assert_eq!(sw.decided_at, 3);
        assert_eq!(sw.level, 2);
    }

    /// Written with `CpPolicy::new(0)` and `mode_for_frame` interleaved exactly as
    /// the arm loop does it, because the revert is only reachable that way: at the
    /// production `SWITCH_LATENCY_FRAMES = 5` the threshold observation arrives
    /// while the long→short switch is still pending, and the re-entrancy guard
    /// returns `None` (that path is
    /// `cp_policy_drops_a_gate_transition_while_a_switch_is_still_pending`). 2.5 ms
    /// is deliberately the threshold value itself, not above it: `CpGate` requires
    /// *strictly* below (`cp_gate.rs:91`), so 2.5 resets the run.
    #[test]
    fn cp_policy_reverts_to_long_cp_on_one_frame_at_threshold() {
        let mut policy = CpPolicy::new(0);
        for f in 0..3 {
            assert_eq!(policy.mode_for_frame(f), CpMode::LongCp);
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        assert_eq!(policy.mode_for_frame(3), CpMode::LongCp);
        let up = policy.observe(3, 2, Some(0.5)).expect("switch to short CP");
        assert_eq!(up.to, CpMode::ShortCp);

        assert_eq!(policy.mode_for_frame(4), CpMode::ShortCp);
        let down = policy
            .observe(4, 2, Some(2.5))
            .expect("2.5 is not strictly below the 2.5 ms threshold");
        assert_eq!(down.from, CpMode::ShortCp);
        assert_eq!(down.to, CpMode::LongCp);
        assert_eq!(down.decided_at, 4);
        assert_eq!(policy.mode_for_frame(5), CpMode::LongCp);
        assert_eq!(policy.switches().len(), 2);
    }

    #[test]
    fn cp_policy_ignores_undecoded_frames() {
        let mut policy = CpPolicy::new(SWITCH_LATENCY_FRAMES);
        assert_eq!(policy.observe(0, 2, Some(0.5)), None);
        assert_eq!(policy.observe(1, 2, Some(0.5)), None);
        assert_eq!(policy.observe(2, 2, Some(0.5)), None);
        assert_eq!(policy.observe(3, 2, None), None);
        assert!(policy.switches().is_empty());
        let sw = policy
            .observe(4, 2, Some(0.5))
            .expect("the undecoded frame neither advanced nor reset the run");
        assert_eq!(sw.to, CpMode::ShortCp);
        assert_eq!(sw.decided_at, 4);
    }

    #[test]
    fn cp_policy_applies_switch_after_configured_latency_frames() {
        let mut policy = CpPolicy::new(5);
        for f in 0..3 {
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        let sw = policy.observe(3, 2, Some(0.5)).expect("dwell reached");
        assert_eq!(sw.decided_at, 3);
        assert_eq!(sw.effective_from, 9);
        assert_eq!(policy.mode_for_frame(8), CpMode::LongCp);
        assert_eq!(policy.mode_for_frame(9), CpMode::ShortCp);
    }

    #[test]
    fn cp_policy_with_zero_latency_applies_on_the_next_frame() {
        let mut policy = CpPolicy::new(0);
        for f in 0..3 {
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        let sw = policy.observe(3, 2, Some(0.5)).expect("dwell reached");
        assert_eq!(sw.effective_from, sw.decided_at + 1);
        assert_eq!(policy.mode_for_frame(3), CpMode::LongCp);
        assert_eq!(policy.mode_for_frame(4), CpMode::ShortCp);
    }

    /// COP-1's re-entrancy guard (`event_loop.rs:1103-1124`). Without it the bench
    /// would model a queueing daemon that does not exist — and in the real one a
    /// second `Propose` orphaned the first seq outright.
    #[test]
    fn cp_policy_drops_a_gate_transition_while_a_switch_is_still_pending() {
        let mut policy = CpPolicy::new(SWITCH_LATENCY_FRAMES);
        for f in 0..3 {
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        assert!(policy.observe(3, 2, Some(0.5)).is_some());
        assert_eq!(
            policy.observe(4, 2, Some(3.0)),
            None,
            "a gate transition while a switch is pending must be dropped, not queued"
        );
        assert_eq!(policy.switches().len(), 1);
    }

    #[test]
    fn switch_airtime_long_to_short_charges_three_long_and_two_short_frames() {
        // level 2 / `hf_standard`: 52 symbols * 1260 samples / 48000 = 1.365 s
        // (`airtime.rs`'s own hand calc). Same 52 symbols on
        // `hf_standard_short_cp`: 52 * 1104 / 48000 = 1.196 s.
        let sw = CpSwitch {
            from: CpMode::LongCp,
            to: CpMode::ShortCp,
            decided_at: 3,
            effective_from: 9,
            level: 2,
        };
        let t = switch_airtime_s(&sw).expect("level 2 is valid");
        let expected = 3.0 * 1.365 + 2.0 * 1.196;
        assert!(
            (t - expected).abs() < 1e-6,
            "expected {expected}s (3 long + 2 short), got {t}"
        );
    }

    /// The reverse direction costs a *different* amount (3 short + 2 long), which
    /// is what catches the plausible bug of charging all five control frames at
    /// one profile — an implementation that would pass the long→short test alone.
    #[test]
    fn switch_airtime_short_to_long_charges_three_short_and_two_long_frames() {
        let sw = CpSwitch {
            from: CpMode::ShortCp,
            to: CpMode::LongCp,
            decided_at: 3,
            effective_from: 9,
            level: 2,
        };
        let t = switch_airtime_s(&sw).expect("level 2 is valid");
        let expected = 3.0 * 1.196 + 2.0 * 1.365;
        assert!(
            (t - expected).abs() < 1e-6,
            "expected {expected}s (3 short + 2 long), got {t}"
        );
        let other = switch_airtime_s(&CpSwitch {
            from: CpMode::LongCp,
            to: CpMode::ShortCp,
            decided_at: 3,
            effective_from: 9,
            level: 2,
        })
        .unwrap();
        assert!(
            (t - other).abs() > 1e-3,
            "the two directions must differ: {t} vs {other}"
        );
    }

    #[test]
    fn switch_airtime_is_none_for_reserved_level_8() {
        let sw = CpSwitch {
            from: CpMode::LongCp,
            to: CpMode::ShortCp,
            decided_at: 0,
            effective_from: 1,
            level: 8,
        };
        assert!(switch_airtime_s(&sw).is_none());
    }

    #[test]
    fn goodput_cost_multiplier_scales_only_the_handshake_term() {
        let bits = 100_000.0;
        let data_airtime = 300.0;
        let switch_airtime = 6.487 * 5.0;
        let bps = |m: f64| goodput_with_switch_cost_bps(bits, data_airtime, switch_airtime, m);
        assert_eq!(
            bps(0.0),
            bits / data_airtime,
            "at cost x0 the handshake term must vanish exactly"
        );
        assert!(bps(0.0) > bps(1.0), "{} vs {}", bps(0.0), bps(1.0));
        assert!(bps(1.0) > bps(6.0), "{} vs {}", bps(1.0), bps(6.0));
    }

    #[test]
    fn frame_seed_zero_reproduces_the_frame_index() {
        for f in 0..300 {
            assert_eq!(frame_seed(0, f), f as u64, "frame {f}");
        }
    }

    /// A seed stride at or below the frame count makes two runs share channel
    /// realizations at an offset, which would silently correlate the "independent"
    /// seeds the multi-seed aggregate's sign-consistency rule depends on.
    #[test]
    fn frame_seed_is_distinct_across_seeds_within_a_run() {
        let frames = 300;
        let mut seen = std::collections::HashSet::new();
        for run_seed in SEEDS {
            for f in 0..frames {
                assert!(
                    seen.insert(frame_seed(run_seed, f)),
                    "collision at run_seed {run_seed}, frame {f}"
                );
            }
        }
        assert_eq!(seen.len(), SEEDS.len() * frames);
        assert!(
            SEED_STRIDE >= frames as u64,
            "stride {SEED_STRIDE} must cover a {frames}-frame run"
        );
    }

    #[test]
    fn spread_histogram_buckets_on_the_estimator_grid() {
        let mut h = SpreadHistogram::default();
        h.observe(Some(0.0));
        h.observe(Some(6.0 * SPREAD_GRID_MS));
        h.observe(Some(7.0 * SPREAD_GRID_MS));
        h.observe(None);
        // 1.7 / 0.41667 = 4.08 grid units — not an integer multiple, so it must
        // land in `off_grid` rather than be smeared into bucket 4.
        h.observe(Some(1.7));
        assert_eq!(h.counts[0], 1);
        assert_eq!(h.counts[6], 1);
        assert_eq!(h.counts[7], 1);
        assert_eq!(h.undecoded, 1);
        assert_eq!(h.off_grid, 1);
        assert_eq!(h.counts.iter().sum::<usize>(), 3);
        assert_eq!(h.total(), 5);
    }
}
