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

/// Whole data-frame slots the switch is **delayed by** while the handshake
/// completes — NOT slots that carry no data. Those five frames still transmit real
/// payload; they simply transmit it under the OLD profile, because the new one is
/// not in force yet. The handshake's own air is a separate charge, levied by
/// [`switch_airtime_s`]. See [`CpPolicy::new`]'s `latency_frames` parameter for the
/// same definition stated precisely.
///
/// This doc used to open "Data-frame slots a completed handshake displaces", which
/// reads as if the five slots were consumed by control traffic — and a PR review
/// thread duly read it that way and proposed replacing those five data iterations
/// with control frames to stop "double-counting" them. **That change is rejected,
/// and the imprecise wording that invited it is what is fixed here.** There is no
/// double-count to remove: `SWITCH_LATENCY_FRAMES` adds no airtime term anywhere,
/// it only selects which profile five frames of a FIXED workload transmit under,
/// and the bias it does introduce runs arm-B-PESSIMISTIC (five frames held on the
/// more expensive long CP). Making them control frames would give the CP-adaptive
/// arm 295 delivered slots against 300 for arms A / P / B0 and all 18 comparator
/// cells, breaking the FER/Wilson denominators and the fixed-workload basis of
/// every cross-arm delta, to chase a sub-0.1% effect that is not an error.
///
/// Each control frame does cost a full frame's airtime at the current level
/// (`airtime.rs:44-51`: `transmit` always rate-matches to a fixed
/// `CODED_BLOCK_LEN`, so a small CpControl PDU is not cheaper than a data frame) —
/// which is precisely why [`switch_airtime_s`] charges it separately rather than by
/// deleting data slots.
///
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
/// control frames of air spent for zero CP change.
///
/// **That case is what [`CpPolicy::applied_on`] exists for, and the comment here
/// used to get its justification exactly backwards.** It claimed [`same_profile`]
/// making the arm's rebuild a no-op "is also what `set_cp_profile` would do". It is
/// not: `CoppaCore::set_cp_profile`
/// (`crates/coppa-engine/src/engine.rs:459-463`) clones the config and calls
/// `reconfigure` **unconditionally**, with no equality guard, and the very doc that
/// claim cited (`cp_negotiator.rs:564-567`) describes calling it with the mode it
/// was already on as "a full transceiver + streaming-receiver rebuild that discards
/// mid-frame samples". The discarded state is real — `harq_rx` is per-transceiver
/// and freshly constructed in `CoppaTransceiver::new`, so a rebuild clears the
/// IR-HARQ LLR accumulator. Gating the arm's rebuild on profile inequality
/// therefore made a `from == to` switch rebuild-free in arm B while arm P — its
/// rebuild placebo, which rebuilds on every recorded switch — still rebuilt, so the
/// control silently stopped being a control. [`CpPolicy::applied_on`] reports switch
/// APPLICATION instead, so arm B's rebuild set equals `switches[].effective_from`
/// by construction, `from == to` included.
pub struct CpPolicy {
    gate: CpGate,
    latency_frames: usize,
    current: CpMode,
    pending: Option<CpSwitch>,
    switches: Vec<CpSwitch>,
    /// Frame index at which [`CpPolicy::mode_for_frame`] last applied a pending
    /// switch. Read by [`CpPolicy::applied_on`]; see that method's doc.
    applied_at: Option<usize>,
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
            applied_at: None,
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
                self.applied_at = Some(frame);
            }
        }
        self.current
    }

    /// Whether the [`CpPolicy::mode_for_frame`] call for `frame` **applied** a
    /// pending switch — i.e. whether `frame` is a switch's `effective_from`.
    ///
    /// This is the arm loop's rebuild trigger, and it is deliberately NOT "the
    /// profile changed". A `from == to` switch (see the type doc) applies a mode
    /// equal to the one already in force, so a profile-inequality test reports
    /// `false` there while a real `set_cp_profile` would still rebuild — which is
    /// what let arm B's rebuild set diverge from arm P's. Keyed to the frame rather
    /// than a bare flag so a caller that forgets to clear it cannot silently rebuild
    /// on every subsequent frame.
    ///
    /// Only meaningful immediately after `mode_for_frame(frame)`, which is the order
    /// the ordering contract already requires.
    pub fn applied_on(&self, frame: usize) -> bool {
        self.applied_at == Some(frame)
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
/// frames on `old` (the profile `sw.from` resolves to) plus
/// [`HANDSHAKE_FRAMES_NEW_PROFILE`] on `new` (`sw.to`'s), both at `sw.level`.
///
/// The split matters and is pinned in both directions: long→short and short→long
/// cost *different* amounts (at level 2 on the standard pair, 6.487 s vs 6.318 s),
/// so charging all five frames at one profile — the obvious wrong implementation —
/// is a test failure rather than a silent bias.
///
/// # The caller supplies the profiles; this function does NOT reconstruct them
///
/// It used to call [`profile_for`] on `sw.from`/`sw.to`, which hardcodes the
/// `hf_standard` pair — so the `robust` base (the DEFAULT base, whose pair is
/// `hf_robust`'s 36 data / 12 pilot carriers against a synthetic short-CP twin) was
/// charged the 44-carrier standard price. Fewer data carriers means MORE symbols
/// per frame (61 vs 52 at bits/symbol 1), so the robust handshake really costs
/// ~7.61 s against the ~6.487 s that was charged: a ~15% UNDERCHARGE, and an
/// undercharge inflates the CP-adaptive arm's goodput — the same pro-arm-B
/// direction as the four omissions below, but this one was undisclosed and
/// unintended. Every profile-agnostic caller must therefore pass its own pair;
/// [`profile_for`] remains the standard-pair constructor only. Pinned by
/// `switch_airtime_scales_with_the_pair_the_caller_passes`.
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
pub fn switch_airtime_s(sw: &CpSwitch, old: &CoppaProfile, new: &CoppaProfile) -> Option<f64> {
    let old_s = frame_airtime_s(sw.level, old)?;
    let new_s = frame_airtime_s(sw.level, new)?;
    Some(HANDSHAKE_FRAMES_OLD_PROFILE as f64 * old_s + HANDSHAKE_FRAMES_NEW_PROFILE as f64 * new_s)
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
/// 1024 clears [`MAX_FRAMES`] with room to spare.
///
/// This used to promise that "a larger override needs a larger stride, and the test
/// is what will say so". **It did not say so**: the test hardcoded a 300-frame run
/// and never read `COPPA_CL_FRAMES`, so nothing checked a larger override at all.
/// Both ends are now real — the test is written against [`MAX_FRAMES`], and the
/// example asserts its own `n` against the same constant — so the two cannot drift.
pub const SEED_STRIDE: u64 = 1024;

/// Hard ceiling on a run's frame count, enforced by `closed_loop_arq`'s `main`
/// against its `COPPA_CL_FRAMES` override and by
/// `frame_seed_is_distinct_across_seeds_within_a_run`.
///
/// The binding hazard is the **cross-seed channel-realization collision**:
/// `frame_seed(k, f) = k * SEED_STRIDE + f`, so once `f` can reach [`SEED_STRIDE`],
/// seed `k`'s tail reproduces seed `k+1`'s head value-for-value — and the seed
/// reaches the RNG directly (`apply_channel` passes `seed ^ 0x5555` / `seed ^
/// 0x3333` into `StdRng::seed_from_u64`), so the "independent" seeds the multi-seed
/// sign-consistency rule depends on would share channel realizations at an offset.
/// `n == SEED_STRIDE` is the last safe value: seed 0's frames run `0..=1023` and
/// seed 1's start at 1024.
///
/// # Why this is NOT `256 + 32`, despite what `closed_loop_arq` used to claim
///
/// That file's `make_header` doc asserted the load-bearing condition was
/// `N_FRAMES <= 256 + 32` — "a future `COPPA_CL_FRAMES` above ~288 would make the
/// IR-HARQ accumulator able to combine again". Two things are wrong with it, and
/// enforcing it as written would have **panicked the bench's own default run**
/// (`DEFAULT_N_FRAMES = 300`, and the committed 5 × 300-frame figures in
/// `BENCHMARKS.md` were measured there).
///
/// The reasoning does not hold either. `seq` wraps mod 256 and this bench advances
/// it by one per frame, so the reuse distance between two frames sharing a seq is a
/// constant **256 frames, independent of `n`** — and those 256 frames insert 255
/// distinct other seqs into a `HARQ_MAX_BUFFERS = 32` LRU map
/// (`transceiver.rs:300-312`: `get_or_create` evicts the least-recently-used entry
/// on every insert past the cap). 255 ≫ 32, so a reused seq's accumulator has
/// **always** been evicted, at every `n`. The safety condition is
/// `seq_reuse_distance > HARQ_MAX_BUFFERS`, which holds unconditionally here; it is
/// not a bound on `n` at all. A *smaller* `HARQ_MAX_BUFFERS` cannot break it either
/// — only a change to how this bench assigns `seq` could.
///
/// The stale claim is corrected at its source (`closed_loop_arq`'s `make_header`)
/// rather than restated here.
pub const MAX_FRAMES: usize = SEED_STRIDE as usize;

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

    /// `applied_on` reports switch APPLICATION, which is what the arm loop rebuilds
    /// on — and the case that separates it from a profile-inequality test is the
    /// `from == to` switch the type doc describes.
    ///
    /// The sequence is the documented one, and every step is load-bearing:
    /// frames 0-3 calm decide a `LongCp -> ShortCp` switch effective at 9; frame 4's
    /// threshold reading transitions the gate back to `LongCp` but is DROPPED by the
    /// re-entrancy guard (leaving the gate standing at `LongCp` with the pending
    /// switch intact); frames 5-8 stay at threshold so the gate cannot re-transition
    /// before the switch lands; frame 9 applies it; and four calm readings from
    /// frame 9 on then transition the gate to `ShortCp` again — deciding a switch
    /// whose `to` already equals `current`.
    ///
    /// The final assertion is the regression: `same_profile` is TRUE across that
    /// switch, so the old inequality-gated rebuild reported nothing to do while arm
    /// P — which rebuilds on every recorded switch — still rebuilt.
    #[test]
    fn cp_policy_reports_a_from_equals_to_switch_as_a_rebuild_point() {
        let mut policy = CpPolicy::new(SWITCH_LATENCY_FRAMES);

        for f in 0..3 {
            assert_eq!(policy.mode_for_frame(f), CpMode::LongCp);
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        assert_eq!(policy.mode_for_frame(3), CpMode::LongCp);
        let up = policy.observe(3, 2, Some(0.5)).expect("dwell of 4 reached");
        assert_eq!(up.effective_from, 9);

        // Dropped by the guard, but the gate itself still moves back to LongCp.
        assert_eq!(policy.mode_for_frame(4), CpMode::LongCp);
        assert_eq!(policy.observe(4, 2, Some(3.0)), None);
        for f in 5..9 {
            assert_eq!(policy.mode_for_frame(f), CpMode::LongCp);
            assert_eq!(policy.observe(f, 2, Some(3.0)), None);
            assert!(!policy.applied_on(f), "nothing applies before frame 9");
        }

        assert_eq!(policy.mode_for_frame(9), CpMode::ShortCp);
        assert!(
            policy.applied_on(9),
            "frame 9 is the switch's effective_from"
        );
        assert!(!policy.applied_on(8), "applied_on is keyed to the frame");
        assert_eq!(policy.observe(9, 2, Some(0.5)), None);

        for f in 10..12 {
            assert_eq!(policy.mode_for_frame(f), CpMode::ShortCp);
            assert!(!policy.applied_on(f));
            assert_eq!(policy.observe(f, 2, Some(0.5)), None);
        }
        assert_eq!(policy.mode_for_frame(12), CpMode::ShortCp);
        let noop = policy
            .observe(12, 2, Some(0.5))
            .expect("four calm frames since the drop re-transition the gate");
        assert_eq!(noop.from, CpMode::ShortCp);
        assert_eq!(noop.to, CpMode::ShortCp, "the from == to case");
        assert_eq!(noop.effective_from, 18);

        for f in 13..18 {
            assert_eq!(policy.mode_for_frame(f), CpMode::ShortCp);
            assert!(!policy.applied_on(f));
        }
        assert_eq!(policy.mode_for_frame(18), CpMode::ShortCp);
        assert!(
            policy.applied_on(18),
            "a from == to switch is still an APPLICATION, and the arm must rebuild \
             there — this is exactly where a profile-inequality test reports nothing \
             while arm P rebuilds anyway"
        );
        assert!(
            same_profile(&profile_for(noop.from), &profile_for(noop.to)),
            "test premise: the profiles really are equal across this switch, so \
             only application (not inequality) can detect it"
        );
        assert_eq!(policy.switches().len(), 2);
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

    /// The standard pair, passed explicitly — as every caller must now do. See
    /// `switch_airtime_scales_with_the_pair_the_caller_passes` for why.
    fn standard_pair_airtime(sw: &CpSwitch) -> Option<f64> {
        switch_airtime_s(sw, &profile_for(sw.from), &profile_for(sw.to))
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
        let t = standard_pair_airtime(&sw).expect("level 2 is valid");
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
        let t = standard_pair_airtime(&sw).expect("level 2 is valid");
        let expected = 3.0 * 1.196 + 2.0 * 1.365;
        assert!(
            (t - expected).abs() < 1e-6,
            "expected {expected}s (3 short + 2 long), got {t}"
        );
        let other = standard_pair_airtime(&CpSwitch {
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

    /// The regression this signature exists to make impossible: `switch_airtime_s`
    /// used to reconstruct the profiles from `profile_for`, so EVERY caller paid the
    /// 44-carrier `hf_standard` price regardless of the pair it was actually
    /// transmitting on — and the bench's DEFAULT base is `hf_robust` (36 data / 12
    /// pilot).
    ///
    /// Fewer data carriers means more symbols per frame: at bits/symbol 1,
    /// `header_syms = ceil(144/36) = 4` and `payload_syms = ceil(1944/36) = 54`, so
    /// 3 + 4 + 54 = **61** symbols against `hf_standard`'s 3 + 4 + 45 = **52**. The
    /// robust handshake is therefore STRICTLY more expensive at the same level and
    /// same CP samples, and an undercharge inflates the CP-adaptive arm's goodput.
    ///
    /// Asserted as a strict inequality plus the exact 61/52 ratio: the inequality is
    /// the property that matters, the ratio is what catches a future carrier-layout
    /// change silently making the two prices equal again.
    #[test]
    fn switch_airtime_scales_with_the_pair_the_caller_passes() {
        let sw = CpSwitch {
            from: CpMode::LongCp,
            to: CpMode::ShortCp,
            decided_at: 3,
            effective_from: 9,
            level: 1, // bits/symbol 1, the level the schedule boots on
        };

        let std_long = CoppaProfile::hf_standard();
        let std_short = CoppaProfile::hf_standard_short_cp();
        let rob_long = CoppaProfile::hf_robust();
        let rob_short = CoppaProfile {
            cp_samples: 144,
            bandwidth_id: 5,
            ..CoppaProfile::hf_robust()
        };
        assert_eq!(rob_long.data_carriers, 36, "test premise: robust is 36+12");
        assert_eq!(std_long.data_carriers, 44, "test premise: standard is 44+4");

        let std_price = switch_airtime_s(&sw, &std_long, &std_short).expect("level 1 is valid");
        let rob_price = switch_airtime_s(&sw, &rob_long, &rob_short).expect("level 1 is valid");

        assert!(
            rob_price > std_price,
            "the 36-carrier pair must price STRICTLY higher than the 44-carrier pair \
             at the same level: robust {rob_price}s vs standard {std_price}s"
        );
        assert!(
            (rob_price / std_price - 61.0 / 52.0).abs() < 1e-9,
            "the ratio is the symbol-count ratio 61/52; got {}",
            rob_price / std_price
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
        assert!(standard_pair_airtime(&sw).is_none());
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
    ///
    /// Written against [`MAX_FRAMES`], not a hardcoded 300. The hardcoded version
    /// could not do the job [`SEED_STRIDE`]'s doc claimed for it — it exercised only
    /// the committed default, so a `COPPA_CL_FRAMES` override large enough to
    /// collide would have sailed past it. `closed_loop_arq`'s `main` asserts its own
    /// `n` against the same constant, so the promise and the check cannot drift.
    #[test]
    fn frame_seed_is_distinct_across_seeds_within_a_run() {
        let frames = MAX_FRAMES;
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

    /// [`MAX_FRAMES`] must be exactly the largest `n` at which no two seeds share a
    /// frame seed — no lower (it would reject the bench's own committed 300-frame
    /// default) and no higher (it would admit the collision it exists to prevent).
    ///
    /// The upper end is checked by construction rather than by assertion:
    /// `frame_seed(0, MAX_FRAMES)` is the first value that belongs to seed 1.
    #[test]
    fn max_frames_is_exactly_the_cross_seed_collision_boundary() {
        assert_eq!(MAX_FRAMES as u64, SEED_STRIDE);
        assert_eq!(
            frame_seed(0, MAX_FRAMES - 1) + 1,
            frame_seed(1, 0),
            "seed 0's last admissible frame must sit immediately below seed 1's first"
        );
        // `closed_loop_arq::DEFAULT_N_FRAMES`, which this crate's lib cannot import
        // (it lives in an example). Bound rather than const-folded so clippy's
        // `assertions_on_constants` does not fire on what is a real cross-module
        // consistency check.
        let committed_default_frames: usize = 300;
        assert!(
            MAX_FRAMES >= committed_default_frames,
            "the ceiling must admit the committed 5 x {committed_default_frames}-frame \
             run; a ceiling of 256 + 32 = 288 (the stale IR-HARQ claim) would panic \
             the default"
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
