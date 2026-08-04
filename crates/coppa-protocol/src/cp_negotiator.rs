//! Peer-negotiation handshake so `coppa_ml::CpGate`'s recommendation (PR
//! #60, telemetry-only until this module) can actually switch the live
//! OFDM CP profile between two Coppa stations. See
//! `docs/superpowers/specs/2026-07-29-cp-switch-peer-negotiation-design.md`
//! for the full design and rationale.
//!
//! This module is pure decision logic -- no `ArqTx`/`ArqRx`/`TransportPdu`
//! dependency, deliberately, so it's testable in isolation (mirrors
//! `coppa_ml::CpGate`'s own shape). The daemon layer (`coppa-daemon`) owns
//! a small, DEDICATED `ArqTx`/`ArqRx` pair for CP-control traffic (its own
//! sequence space, separate from ordinary data) and calls into this module
//! for the pure "what does this payload mean, what should I do" decisions.
//!
//! ## Roles per handshake
//!
//! Call the station that observed a calm channel and wants a CP change "B"
//! (it sends Propose), and the station whose transmissions B wants changed
//! "A" (it receives Propose, sends Confirm).
//!
//! **Read the A/B labels carefully -- plain English and the code disagree
//! here, and that collision has already produced inverted prose twice** (in
//! `CLAUDE.md`'s Known Limitations bullet and in PR #67's own body, both
//! corrected by COP-1). The station performing the plain-English act of
//! *confirming* -- acknowledging that it heard and accepted -- is **B**,
//! the proposer, whose action variant is (misleadingly)
//! `ContentAction::ApplyAsConfirmer` and whose tracing string is
//! `"CP profile switched (proposer role, own receiver)"`. **A** is the
//! station that *sends the `Confirm` payload*. When in doubt, trust the
//! wire direction, not the word: A sends `Confirm`, B receives it. The
//! variant name is left alone deliberately (renaming it is zero-behavior
//! churn across every call site and test); the doc carries the correction.
//!
//! - **B (proposer):** sends a `Propose(mode)` payload via
//!   `propose_payload`, retried by the caller's `ArqTx` polling loop like
//!   ordinary data -- this module holds no state for B's side of that
//!   send (the daemon keeps that seq itself, as `EventLoop::cp_propose_seq`,
//!   so it can drive give-up trigger G1). When B receives A's Confirm
//!   content (`on_content_received` -> `ContentAction::ApplyAsConfirmer`),
//!   B calls `apply_as_confirmer` to switch its OWN RECEIVER to the new
//!   mode immediately -- no need to wait for anything further, since B
//!   isn't the one switching the riskier side (the encoder); it just needs
//!   to be ready to decode A's future frames under the new CP. That call
//!   also **arms probation** (COP-1): B is now deaf to A's old profile, so
//!   it needs a bounded deadline by which A must prove it switched too.
//! - **A (confirmer):** accepts a Propose unconditionally
//!   (`on_content_received` on Propose content always yields
//!   `ContentAction::SendConfirm`), replies with a Confirm, and records
//!   the ARQ seq it sent that Confirm at via `track_pending_confirm`. A
//!   applies the new mode to its OWN ENCODER only once it sees B's bare
//!   ack for that Confirm (`on_confirm_acked`) -- this is what guarantees
//!   A never switches before B is proven ready to receive under the new
//!   CP. Having switched, A immediately sends the **third leg**
//!   (`switched_payload`, COP-1) under the NEW profile and tracks it via
//!   `track_pending_switched`.
//!
//! ## The three legs (COP-1)
//!
//! ```text
//! 1. B: CpGate transition  -> Propose(mode)    [ARQ-tracked, OLD profile]
//! 2. A: on Propose         -> Confirm(mode)    [ARQ-tracked, OLD profile]
//!                             A does NOT switch yet
//! 3. B: on Confirm         -> bare ack         [untracked,   OLD profile]
//!                             apply_as_confirmer -> switches + arms probation
//! 4. A: on that bare ack   -> switch, then CpSwitched(mode)
//!                                              [ARQ-tracked, NEW profile]
//! 5. B: on CpSwitched      -> on_peer_switched (disarms probation) + bare ack
//! 6. A: on that bare ack   -> on_switched_acked -> done
//! ```
//!
//! Legs 1 and 2 were the original PR #67 handshake. The bare ack at step 3
//! is un-retryable *by construction* (it never reaches `ArqTx::send`), and
//! a retransmitted `Confirm` cannot re-elicit it either -- `ArqRx::receive`
//! returns an empty `delivered` for an already-delivered seq, so the
//! duplicate is swallowed by the dedupe before it can reach the
//! ack-sending code. Losing it therefore used to strand the two stations on
//! mutually-undecodable profiles forever. Steps 4-6 plus the give-up
//! triggers below are COP-1's fix.
//!
//! **Note that the six-step diagram contains FIVE droppable frames, not
//! four** (steps 1, 2, 3, 4 and 6 all go on the air; step 5 is B's local
//! state change). The first four are covered by the give-up triggers below;
//! step 6 -- the bare ack for the third leg -- is covered instead by making
//! it *re-elicitable*, see "The fifth droppable frame" below.
//!
//! ## Give-up triggers -- every wait state converges on the pre-negotiation mode
//!
//! | # | Who waits | For what | Trigger | Action |
//! |---|---|---|---|---|
//! | G1 | B | its `Propose` acked | `ArqTx::is_failed(propose_seq)` | abandon segment, clear seq (B never switched) |
//! | G2 | A | its `Confirm` acked | `ArqTx::is_failed(confirm_seq)` | abandon segment, `abort()` (A never switched) |
//! | G3 | B | `CpSwitched` from A | `tick` past the probation deadline | **revert engine + negotiator** |
//! | G4 | A | its `CpSwitched` acked | `ArqTx::is_failed(switched_seq)` | abandon segment, `abort()`, **revert engine** |
//!
//! Every single-leg loss fires a trigger on *both* stations, which is what
//! makes convergence total:
//!
//! | Lost frame | B fires | A fires | Converges to |
//! |---|---|---|---|
//! | 1. `Propose` | G1 | -- (never saw anything) | pre-negotiation mode |
//! | 2. `Confirm` | G1 (its Propose is never acked either) | G2 | pre-negotiation mode |
//! | 3. bare ack for the `Confirm` | G3 | G2 | pre-negotiation mode |
//! | 4. `CpSwitched` | G3 | G4 | pre-negotiation mode |
//! | 6. bare ack for the `CpSwitched` | -- | -- (A's retransmit re-elicits it) | the NEW mode |
//!
//! ### The convergence target is the **pre-negotiation mode**, not `LongCp`
//!
//! Both give-up paths -- `abort()` (G2/G4) and `tick()` (G3) -- revert to
//! the mode this station was on *before* this negotiation started, via the
//! single private `revert()` helper so there is exactly one rule and the two
//! paths cannot drift apart. That mode is the last one both stations are
//! known to have *agreed* on, since a negotiation only ever starts from a
//! converged state.
//!
//! For the first negotiation of a session that mode is `LongCp` (both
//! stations boot into it, `CpNegotiator::new`), which is why an earlier
//! revision of this module hardcoded `LongCp` in `abort()` and described the
//! target as "always `LongCp`". That was **wrong for the second and later
//! negotiations**, and `CpGate` produces those routinely: it reverts to
//! `CpRecommendation::LongCp` on any single frame at or above threshold
//! ("drop fast", `coppa_ml::cp_gate`), and the daemon turns that transition
//! into a real `Propose(LongCp)` -- so a `ShortCp` -> `LongCp` negotiation
//! is a first-class production path, and it is the one that runs exactly
//! when the channel is degrading and legs get lost. With two different
//! targets, three of the five droppable frames desynced the link
//! permanently in that direction:
//!
//! - **`Confirm` lost:** A never switched at all, yet the old `abort()`
//!   dragged it to `LongCp` while B (G1, also never switched) stayed on
//!   `ShortCp`.
//! - **bare ack lost:** A's G2 reached `LongCp` and B's `tick` was *already*
//!   on `LongCp` (it had applied the proposed mode) -- so the old G3, by
//!   restoring `previous` while `abort` forced `LongCp`, actively *broke* an
//!   agreement the two stations had already reached.
//! - **`CpSwitched` lost:** same shape as the bare ack case.
//!
//! Reverting to the pre-negotiation mode is correct in all five cases and is
//! bit-identical to the old behavior for a `LongCp` -> `ShortCp` negotiation
//! (where the pre-negotiation mode *is* `LongCp`).
//!
//! ### The fifth droppable frame (step 6's bare ack)
//!
//! Step 6 is B's bare ack for A's third leg. It is un-retryable by
//! construction for the same reason step 3 is, and `on_peer_switched`
//! disarms B's probation the moment the third leg arrives -- so if step 6
//! were simply lost, A's G4 would fire and revert A while B, with no timer
//! left at all, stayed on the new mode: exactly the permanent desync COP-1
//! exists to eliminate, reintroduced one step later.
//!
//! The fix is to make step 6 **re-elicitable** rather than to give B yet
//! another deadline. A's third leg is ARQ-tracked, so A retransmits it; the
//! daemon's `handle_cp_control` now emits a bare ack for a *duplicate*
//! `CpSwitched` content PDU too (one whose seq `ArqRx` has already delivered,
//! so `delivered` comes back empty) instead of dropping it silently. Step 6 is
//! therefore covered by ordinary retransmission, the same mechanism that
//! covers legs 1, 2 and 4 -- and the handshake completes on the NEW mode
//! rather than giving up.
//!
//! That re-ack is deliberately restricted to the `CpSwitched` kind, and step 6
//! is the only frame that needs it. Re-acking *every* duplicate would silently
//! disarm the give-up triggers the table above depends on: a retransmitted
//! `Propose` would get acked, clearing B's `cp_propose_seq` so G1 could never
//! fire on the row-2 (`Confirm` lost) case, and a retransmitted `Confirm`
//! would be acked under the NEW profile that A -- which has not switched --
//! cannot decode, so the ack buys nothing while still costing A's G2 its
//! trigger. Only for `CpSwitched` are both stations already on the same, new
//! profile, which is exactly what makes the re-ack meaningful there and
//! harmful everywhere else.
//!
//! Giving B a second deadline instead was considered and rejected: nothing
//! could clear it on an idle link (the only remaining signal would be "some
//! frame decoded later", which is precisely the traffic-dependent trigger
//! the third leg exists to replace), so it would reintroduce the
//! idle-link-churn failure mode documented in this feature's design doc.
//!
//! **Residual, stated plainly:** if step 6 *and* every retransmission of the
//! third leg are lost, A's G4 fires and reverts A while B stays on the new
//! mode. That is multi-leg loss, which this design does not claim to cover
//! for any leg (see `CLAUDE.md`'s COP-1 bullet); single-leg loss of all five
//! droppable frames does converge.
//!
//! This module owns only G3's clock; G1/G2/G4 are ARQ-budget-driven and
//! live in the daemon, which polls `pending_confirm_seq`/
//! `pending_switched_seq` against `ArqTx::is_failed`.
//!
//! ## One negotiation at a time -- enforced, not assumed
//!
//! There is exactly **one** `CpNegotiator` per daemon: one `current`, one
//! `revert_to`, one `probation`, one `pending_confirm`, one
//! `pending_switched`. Nothing about the roles above makes a station
//! intrinsically an A or a B -- both stations run `CpGate` over the same
//! channel and either may observe a transition first -- so the roles are not
//! disjoint by construction. An earlier revision of the daemon's
//! `drive_cp_negotiation` doc claimed they were ("only B arms probation and
//! only A tracks a pending `Switched`, so they can never both fire on the same
//! station"). That was **false**: two stations transitioning at once cross
//! their `Propose`s, and a station that accepted an inbound `Propose` while
//! its own was in flight would hold both roles in one set of single-slot
//! fields.
//!
//! The daemon now enforces the invariant instead of assuming it, via
//! [`CpNegotiator::negotiation_in_flight`] plus its own `cp_propose_seq`:
//! a `CpGate` transition does not start a `Propose` while a negotiation is in
//! flight, and an inbound `Propose` arriving in that state is dropped without
//! a `Confirm` and without an ack -- so the peer's G1 fires and it converges on
//! its pre-negotiation mode rather than being left waiting. Both stations in a
//! crossed-`Propose` collision drop each other's, both fire G1, and both stay
//! on the mode they already agreed on; the next `CpGate` transition proposes
//! again.
//!
//! **Residual, stated plainly:** the guard cannot see the window between B's
//! `Propose` being *acked* (which clears `cp_propose_seq`) and A's `Confirm`
//! arriving, because nothing is tracked in between. A `Propose` inbound in
//! exactly that window is still accepted and can put one station in both
//! roles. Every give-up trigger still converges in that case -- the daemon's
//! give-up block reads and abandons *all* tracked legs before clearing any
//! bookkeeping, and reverts once -- so the outcome is a wasted negotiation,
//! not a desync. Closing it properly means keying negotiator state by
//! negotiation rather than by station, which is a larger change than this
//! ticket takes on.

use std::time::{Duration, Instant};

/// A CP profile choice. Wire values: `LongCp = 0x00`, `ShortCp = 0x01`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpMode {
    /// `CoppaProfile::hf_standard()` (6.25 ms CP).
    LongCp,
    /// `CoppaProfile::hf_standard_short_cp()` (3.0 ms CP).
    ShortCp,
}

impl CpMode {
    pub fn to_wire(self) -> u8 {
        match self {
            CpMode::LongCp => 0x00,
            CpMode::ShortCp => 0x01,
        }
    }

    pub fn from_wire(byte: u8) -> Self {
        if byte == 0x01 {
            CpMode::ShortCp
        } else {
            CpMode::LongCp
        }
    }
}

const KIND_PROPOSE: u8 = 0x01;
const KIND_CONFIRM: u8 = 0x02;
/// Third leg (COP-1): "I have switched my own encoder to `mode`." Sent by A
/// under the NEW profile immediately after `set_cp_profile`, ARQ-tracked, so
/// B has a deterministic proof-of-switch instead of having to infer one from
/// whatever traffic happens to arrive. On an idle link no such traffic
/// arrives at all, which is why inferring is not good enough.
const KIND_SWITCHED: u8 = 0x03;

/// How long B waits, after switching its own engine, for A's `CpSwitched`
/// before concluding the handshake failed and reverting to `LongCp` (give-up
/// trigger G3).
///
/// Must comfortably outlast A's ARQ give-up time for the `CpSwitched` leg,
/// or B would revert while A is still legitimately retrying. With the
/// CP-control pair's inherited defaults -- [`crate::arq::DEFAULT_RTO_SECS`]
/// = 5.0, [`crate::arq::DEFAULT_MAX_RETRANSMIT`] = 5, exponential backoff
/// capped at 60 s -- the worst case is roughly 5 + 10 + 20 + 40 + 60 = 135 s.
/// 180 s leaves margin.
///
/// This is a derived-and-rounded bound, **not a swept one**; no bench
/// measures it (the same caveat `coppa_ml::CpGate::default_coppa` carries for
/// its own constants). Probation only costs anything in the failure case,
/// where the link is already dead, so erring long is the conservative
/// direction.
pub const SWITCH_PROBATION_SECS: u64 = 180;

/// What to do after decoding a received CpControl content payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentAction {
    /// Received a Propose; accept unconditionally (this design's "trust
    /// outright" decision) and send this Confirm payload back.
    SendConfirm(Vec<u8>),
    /// Received a Confirm; apply this mode to OUR OWN RECEIVER immediately
    /// -- this is what B (the proposer) does upon receiving A's Confirm,
    /// see the module doc. (The variant name is a known misnomer: this is
    /// B's action, not A's. See the module doc's role warning.)
    ApplyAsConfirmer(CpMode),
    /// Received A's third leg: A has switched its own encoder to `mode`.
    /// Proof our own switch was not made in vain -- disarms probation.
    PeerSwitched(CpMode),
}

/// What [`CpNegotiator::tick`] found had timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpTimeout {
    /// Probation expired without proof the peer switched (G3). The caller
    /// **must** call `engine.set_cp_profile` with this mode so the engine
    /// and this negotiator's `current()` do not drift apart -- keeping those
    /// two in agreement is the entire point of COP-1.
    RevertTo(CpMode),
}

/// Negotiation state for one direction of one link. See the module doc for
/// the proposer/confirmer role split and the G1-G4 give-up table.
pub struct CpNegotiator {
    current: CpMode,
    /// A's side: set when we sent a Confirm and are waiting to see its ARQ
    /// seq acked before applying `mode` to our own encoder.
    pending_confirm: Option<(u8, CpMode)>,
    /// A's side: the ARQ seq of the `CpSwitched` third leg we sent and are
    /// waiting to see acked (G4).
    pending_switched: Option<u8>,
    /// B's side: `(deadline, mode we switched to)`. Armed by
    /// `apply_as_confirmer`, disarmed by `on_peer_switched` for the matching
    /// target mode, fired once by `tick` (G3).
    probation: Option<(Instant, CpMode)>,
    /// The mode to return to if the in-flight negotiation gives up: the mode
    /// this station was on before it started, which is the last mode both
    /// stations are known to have agreed on. `None` when no negotiation is in
    /// flight.
    ///
    /// Armed at whichever end of the handshake this station entered it from
    /// (`track_pending_confirm` for A, `apply_as_confirmer` for B), and read
    /// by the single [`CpNegotiator::revert`] helper that backs *both*
    /// give-up paths -- see the module doc's "The convergence target is the
    /// pre-negotiation mode" section for why one shared rule matters and what
    /// two divergent ones broke.
    revert_to: Option<CpMode>,
}

impl CpNegotiator {
    pub fn new() -> Self {
        Self {
            current: CpMode::LongCp,
            pending_confirm: None,
            pending_switched: None,
            probation: None,
            revert_to: None,
        }
    }

    /// The mode this negotiator currently believes is in effect.
    pub fn current(&self) -> CpMode {
        self.current
    }

    /// Build a Propose payload for `mode` (proposer role, B).
    pub fn propose_payload(mode: CpMode) -> Vec<u8> {
        vec![KIND_PROPOSE, mode.to_wire()]
    }

    /// Build the third leg's payload for `mode` (COP-1, sent by A under the
    /// NEW profile right after it switches its own encoder). See the module
    /// doc's three-leg diagram.
    pub fn switched_payload(mode: CpMode) -> Vec<u8> {
        vec![KIND_SWITCHED, mode.to_wire()]
    }

    /// Decode a received CpControl content payload (always exactly 2 bytes:
    /// `[kind, mode]`) and decide what to do. Returns `None` for a
    /// malformed payload (wrong length or unrecognized kind) -- the caller
    /// should log and ignore, not panic; a corrupted or foreign PDU that
    /// happened to parse as a valid `TransportPdu` shell is not this
    /// module's problem to recover from further.
    pub fn on_content_received(payload: &[u8]) -> Option<ContentAction> {
        let [kind, mode_byte] = payload else {
            return None;
        };
        let mode = CpMode::from_wire(*mode_byte);
        match *kind {
            KIND_PROPOSE => Some(ContentAction::SendConfirm(vec![
                KIND_CONFIRM,
                mode.to_wire(),
            ])),
            KIND_CONFIRM => Some(ContentAction::ApplyAsConfirmer(mode)),
            KIND_SWITCHED => Some(ContentAction::PeerSwitched(mode)),
            _ => None,
        }
    }

    /// Record that we (as confirmer) just sent a Confirm for `mode` at ARQ
    /// sequence `seq`, so `on_confirm_acked` can recognize when it's safe
    /// to apply `mode` to our own encoder.
    ///
    /// This is A's entry point into a negotiation, so it also arms the
    /// give-up target: whatever mode we are on now is the one both stations
    /// agreed on, and therefore the one to come back to if any later leg is
    /// lost. `get_or_insert` rather than a plain assignment so a second
    /// Confirm inside one negotiation cannot overwrite the genuine
    /// pre-negotiation mode with a mid-handshake one.
    pub fn track_pending_confirm(&mut self, seq: u8, mode: CpMode) {
        self.revert_to.get_or_insert(self.current);
        self.pending_confirm = Some((seq, mode));
    }

    /// Call with the newly-acked sequence numbers from the dedicated
    /// CP-control `ArqTx::process_ack`. If our tracked Confirm is among
    /// them, applies its mode to `current` and returns it; otherwise
    /// returns `None` (including when there's no pending confirm at all --
    /// a no-op, not an error).
    pub fn on_confirm_acked(&mut self, newly_acked: &[u8]) -> Option<CpMode> {
        let (seq, mode) = self.pending_confirm?;
        if newly_acked.contains(&seq) {
            self.pending_confirm = None;
            self.current = mode;
            Some(mode)
        } else {
            None
        }
    }

    /// Apply `mode` immediately and arm probation -- this is B's (the
    /// proposer's) step on receiving Confirm content, see the module doc for
    /// why B doesn't wait for an ack first (unlike A, which does, via
    /// `track_pending_confirm`/`on_confirm_acked`).
    ///
    /// Arming probation (COP-1) is what bounds the failure case: having
    /// switched, B is deaf to A's old profile, so A now has
    /// [`SWITCH_PROBATION_SECS`] to prove it switched too (via the third
    /// leg -> `on_peer_switched`) before `tick` reverts B to `LongCp`.
    ///
    /// `now` follows `crate::arq`'s convention of the caller supplying the
    /// clock, which keeps this module deterministic under test -- a daemon
    /// test can inject a synthetic future `Instant` instead of sleeping out
    /// a 180-second probation.
    pub fn apply_as_confirmer(&mut self, mode: CpMode, now: Instant) {
        // B's entry point into a negotiation: arm the give-up target before
        // switching, for the same reason `track_pending_confirm` does on A's
        // side. `tick` (G3) reads it through `revert`, so B and A come back to
        // the same mode.
        self.revert_to.get_or_insert(self.current);
        self.current = mode;
        self.probation = Some((now + Duration::from_secs(SWITCH_PROBATION_SECS), mode));
    }

    /// Disarm probation on proof the peer switched to the mode we switched
    /// to (B's side, on receiving A's third leg). A `PeerSwitched` naming a
    /// *different* mode is ignored -- a stale or garbled leg from some other
    /// negotiation cannot be proof of *this* switch, and accepting it would
    /// silently cancel the one safety net that makes the failure case
    /// recoverable.
    ///
    /// Returns whether the leg was **accepted** -- `false` for a leg naming a
    /// different mode, and for one arriving with no probation armed at all
    /// (a no-op, not an error).
    ///
    /// The caller must ack only an accepted leg. Acking a rejected one
    /// resolves the sender's G4 -- telling it the handshake completed -- while
    /// this station's own probation stays armed and later reverts it: the
    /// permanent desync COP-1 exists to eliminate, reached by a different
    /// route. Leaving a rejected leg unacked instead lets the sender's G4 fire
    /// and both stations converge on their pre-negotiation mode.
    pub fn on_peer_switched(&mut self, mode: CpMode) -> bool {
        if matches!(self.probation, Some((_, target)) if target == mode) {
            self.probation = None;
            // The negotiation is complete on our side, so there is no longer a
            // pre-negotiation mode to fall back to; the mode we just settled on
            // becomes the baseline for the NEXT negotiation. Leaving a stale
            // `revert_to` armed would make a later `abort()` rewind two
            // negotiations instead of one.
            self.revert_to = None;
            true
        } else {
            false
        }
    }

    /// Record that we (as A) just sent the third leg at ARQ sequence `seq`,
    /// so the daemon can poll `pending_switched_seq` against
    /// `ArqTx::is_failed` for give-up trigger G4.
    pub fn track_pending_switched(&mut self, seq: u8) {
        self.pending_switched = Some(seq);
    }

    /// Call with the newly-acked sequence numbers from the dedicated
    /// CP-control `ArqTx::process_ack`. Returns `true` (once) when our
    /// tracked third leg is among them -- the handshake is then fully
    /// complete on our side. Returns `false` when there is nothing pending,
    /// which is a no-op rather than an error.
    pub fn on_switched_acked(&mut self, newly_acked: &[u8]) -> bool {
        match self.pending_switched {
            Some(seq) if newly_acked.contains(&seq) => {
                self.pending_switched = None;
                // Handshake complete on our side -- clear the give-up target
                // for the same reason `on_peer_switched` does on B's side.
                self.revert_to = None;
                true
            }
            _ => false,
        }
    }

    /// The ARQ seq of our outstanding Confirm, if any (give-up trigger G2).
    pub fn pending_confirm_seq(&self) -> Option<u8> {
        self.pending_confirm.map(|(seq, _)| seq)
    }

    /// The ARQ seq of our outstanding third leg, if any (give-up trigger G4).
    pub fn pending_switched_seq(&self) -> Option<u8> {
        self.pending_switched
    }

    /// Whether this negotiator is holding state for a negotiation that has
    /// not converged yet -- an unacked Confirm, an unacked third leg, or an
    /// armed probation.
    ///
    /// The daemon uses this as a **re-entrancy guard**: there is exactly one
    /// `CpNegotiator` per daemon (one `current`/`revert_to`/`probation` and
    /// one of each `pending_*`), so a station that took on a second
    /// negotiation while one was in flight would be running both roles
    /// through the same single-slot state. See the module doc's
    /// "One negotiation at a time" section for the residual case this
    /// predicate cannot see.
    pub fn negotiation_in_flight(&self) -> bool {
        self.pending_confirm.is_some()
            || self.pending_switched.is_some()
            || self.probation.is_some()
    }

    /// The one and only give-up rule: return `current` to the mode this
    /// station was on before the in-flight negotiation started, and report
    /// which mode that is.
    ///
    /// Both give-up paths (`abort` for G2/G4, `tick` for G3) go through here
    /// so they cannot possibly disagree -- they used to, and three of the five
    /// droppable frames desynced a `ShortCp` -> `LongCp` negotiation as a
    /// result. See the module doc's "The convergence target is the
    /// pre-negotiation mode" section.
    ///
    /// With nothing armed (no negotiation in flight) this leaves `current`
    /// alone: there is no negotiation to rewind, and forcing `LongCp` here is
    /// exactly the bug described above.
    fn revert(&mut self) -> CpMode {
        if let Some(mode) = self.revert_to.take() {
            self.current = mode;
        }
        self.current
    }

    /// Give up on the in-flight negotiation and return to the pre-negotiation
    /// mode (G2/G4). Idempotent, and safe to call from any state.
    ///
    /// Returns the mode reverted to. Callers that may already have switched
    /// their engine (G4) **must** pair this with
    /// `engine.set_cp_profile(returned_mode)` -- this method only owns the
    /// bookkeeping half, and passing anything other than the returned mode is
    /// how the engine and this negotiator drift apart.
    pub fn abort(&mut self) -> CpMode {
        self.pending_confirm = None;
        self.pending_switched = None;
        self.probation = None;
        self.revert()
    }

    /// Drive wall-clock deadlines (give-up trigger G3). Returns at most one
    /// timeout per call and clears the state that produced it, so a timeout
    /// fires exactly once no matter how often the caller polls -- the daemon
    /// calls this from a 500 ms tick, and a re-firing revert would spam
    /// `set_cp_profile` (a full engine rebuild) twice a second.
    ///
    /// Returns `None` -- while still disarming the expired probation -- when
    /// there is no pre-negotiation mode left to go back to (`revert_to` is
    /// `None`, i.e. the negotiation this probation belonged to already
    /// completed by some other route). `revert()` would return `current`
    /// unchanged there, and reporting that as a `RevertTo` made the daemon
    /// call `set_cp_profile` with the mode it was already on: a full
    /// transceiver + streaming-receiver rebuild that discards mid-frame
    /// samples, plus a "reverted" operator warning naming no actual change.
    pub fn tick(&mut self, now: Instant) -> Option<CpTimeout> {
        if let Some((deadline, _target)) = self.probation {
            if now >= deadline {
                self.probation = None;
                // Disarmed either way; only *report* a revert when there is
                // genuinely a pre-negotiation mode to go back to.
                self.revert_to?;
                return Some(CpTimeout::RevertTo(self.revert()));
            }
        }
        None
    }
}

impl Default for CpNegotiator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_long_cp() {
        let n = CpNegotiator::new();
        assert_eq!(n.current(), CpMode::LongCp);
    }

    #[test]
    fn wire_roundtrip() {
        assert_eq!(CpMode::from_wire(CpMode::LongCp.to_wire()), CpMode::LongCp);
        assert_eq!(
            CpMode::from_wire(CpMode::ShortCp.to_wire()),
            CpMode::ShortCp
        );
    }

    #[test]
    fn propose_payload_is_two_bytes_kind_then_mode() {
        let payload = CpNegotiator::propose_payload(CpMode::ShortCp);
        assert_eq!(payload.len(), 2);
        assert_eq!(payload[0], KIND_PROPOSE);
        assert_eq!(CpMode::from_wire(payload[1]), CpMode::ShortCp);
    }

    #[test]
    fn receiving_a_propose_yields_a_confirm_for_the_same_mode() {
        let propose = CpNegotiator::propose_payload(CpMode::ShortCp);
        match CpNegotiator::on_content_received(&propose) {
            Some(ContentAction::SendConfirm(confirm)) => {
                assert_eq!(confirm[0], KIND_CONFIRM);
                assert_eq!(CpMode::from_wire(confirm[1]), CpMode::ShortCp);
            }
            other => panic!("expected SendConfirm, got {other:?}"),
        }
    }

    #[test]
    fn receiving_a_confirm_yields_apply_as_confirmer() {
        let confirm = vec![KIND_CONFIRM, CpMode::ShortCp.to_wire()];
        match CpNegotiator::on_content_received(&confirm) {
            Some(ContentAction::ApplyAsConfirmer(mode)) => assert_eq!(mode, CpMode::ShortCp),
            other => panic!("expected ApplyAsConfirmer, got {other:?}"),
        }
    }

    #[test]
    fn malformed_payload_is_ignored_not_panicked() {
        assert!(CpNegotiator::on_content_received(&[]).is_none());
        assert!(CpNegotiator::on_content_received(&[0xFF]).is_none());
        assert!(CpNegotiator::on_content_received(&[0xFF, 0x00]).is_none());
        assert!(CpNegotiator::on_content_received(&[0x01, 0x00, 0x00]).is_none());
    }

    #[test]
    fn apply_as_confirmer_updates_current() {
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, Instant::now());
        assert_eq!(n.current(), CpMode::ShortCp);
    }

    #[test]
    fn confirm_ack_applies_mode_and_clears_pending() {
        let mut n = CpNegotiator::new();
        n.track_pending_confirm(5, CpMode::ShortCp);
        assert_eq!(
            n.on_confirm_acked(&[1, 2, 3]),
            None,
            "unrelated seqs must not trigger"
        );
        assert_eq!(n.current(), CpMode::LongCp, "must not have changed yet");
        assert_eq!(n.on_confirm_acked(&[5]), Some(CpMode::ShortCp));
        assert_eq!(n.current(), CpMode::ShortCp);
        // Second call with no pending confirm must be a no-op, not a panic.
        assert_eq!(n.on_confirm_acked(&[5]), None);
    }

    // ── COP-1: third leg (`CpSwitched`) and the probation deadline ────────

    #[test]
    fn switched_payload_is_two_bytes_kind_then_mode() {
        let p = CpNegotiator::switched_payload(CpMode::ShortCp);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], KIND_SWITCHED);
        assert_eq!(CpMode::from_wire(p[1]), CpMode::ShortCp);
    }

    #[test]
    fn receiving_a_switched_yields_peer_switched() {
        let p = CpNegotiator::switched_payload(CpMode::ShortCp);
        match CpNegotiator::on_content_received(&p) {
            Some(ContentAction::PeerSwitched(mode)) => assert_eq!(mode, CpMode::ShortCp),
            other => panic!("expected PeerSwitched, got {other:?}"),
        }
    }

    #[test]
    fn probation_is_armed_by_apply_as_confirmer_and_expires() {
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        assert_eq!(n.current(), CpMode::ShortCp);
        // Not yet expired.
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS - 1)),
            None
        );
        assert_eq!(n.current(), CpMode::ShortCp);
        // Expired -> revert to the pre-switch mode.
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            Some(CpTimeout::RevertTo(CpMode::LongCp))
        );
        assert_eq!(n.current(), CpMode::LongCp);
    }

    #[test]
    fn probation_expiring_is_reported_once_not_every_tick() {
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        let late = t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1);
        assert!(n.tick(late).is_some());
        assert_eq!(n.tick(late), None, "must not re-fire after reverting");
    }

    #[test]
    fn peer_switched_disarms_probation() {
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        n.on_peer_switched(CpMode::ShortCp);
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            None,
            "a confirmed peer switch must cancel probation permanently"
        );
        assert_eq!(n.current(), CpMode::ShortCp);
    }

    #[test]
    fn peer_switched_for_a_different_mode_does_not_disarm() {
        // A stale/garbled PeerSwitched naming the mode we did NOT switch to
        // must not be accepted as proof of this switch.
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        n.on_peer_switched(CpMode::LongCp);
        assert!(n
            .tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1))
            .is_some());
    }

    #[test]
    fn switched_leg_is_tracked_and_resolved_by_its_ack() {
        let mut n = CpNegotiator::new();
        n.track_pending_switched(7);
        assert!(
            !n.on_switched_acked(&[1, 2]),
            "unrelated seqs must not resolve"
        );
        assert!(n.on_switched_acked(&[7]));
        assert!(
            !n.on_switched_acked(&[7]),
            "second call is a no-op, not a panic"
        );
    }

    #[test]
    fn pending_seq_accessors_expose_what_the_daemon_must_poll_for_failure() {
        let mut n = CpNegotiator::new();
        assert_eq!(n.pending_confirm_seq(), None);
        assert_eq!(n.pending_switched_seq(), None);
        n.track_pending_confirm(3, CpMode::ShortCp);
        n.track_pending_switched(4);
        assert_eq!(n.pending_confirm_seq(), Some(3));
        assert_eq!(n.pending_switched_seq(), Some(4));
    }

    #[test]
    fn abort_clears_all_wait_state_and_reverts_to_the_pre_negotiation_mode() {
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.track_pending_confirm(3, CpMode::ShortCp);
        n.track_pending_switched(4);
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        // Pre-negotiation mode here is the LongCp this negotiator booted into.
        assert_eq!(n.abort(), CpMode::LongCp);
        assert_eq!(n.current(), CpMode::LongCp);
        assert_eq!(n.pending_confirm_seq(), None);
        assert_eq!(n.pending_switched_seq(), None);
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            None
        );
    }

    // ── COP-1 remediation: ONE convergence target, the pre-negotiation mode ──
    //
    // `abort()` (G2/G4) used to hardcode `LongCp` while `tick()` (G3) restored
    // the pre-switch mode. Equivalent only while the stations were on LongCp
    // before the negotiation -- so every one of these tests passes trivially in
    // the LongCp -> ShortCp direction and is about the ShortCp -> LongCp one,
    // which `CpGate`'s "drop fast" revert makes a first-class production path.

    /// Put a negotiator in the state a station is really in after one
    /// successful negotiation: converged on `mode`, nothing pending, no stale
    /// give-up target left armed.
    fn converged_on(mode: CpMode) -> CpNegotiator {
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(mode, Instant::now());
        n.on_peer_switched(mode);
        assert_eq!(n.current(), mode);
        n
    }

    #[test]
    fn abort_reverts_to_the_pre_negotiation_mode_not_unconditionally_long_cp() {
        // A is converged on ShortCp and now confirms a ShortCp -> LongCp
        // downshift. If its Confirm (or its third leg) is lost, it must come
        // back to ShortCp -- the mode the peer is still on -- not to LongCp.
        let mut a = converged_on(CpMode::ShortCp);
        a.track_pending_confirm(1, CpMode::LongCp);
        assert_eq!(a.abort(), CpMode::ShortCp);
        assert_eq!(a.current(), CpMode::ShortCp);
    }

    #[test]
    fn abort_and_tick_converge_on_the_same_mode_in_the_short_to_long_direction() {
        // The heart of the desync: A gives up via `abort` (G2/G4) and B via
        // `tick` (G3) on the same failed negotiation. The two MUST agree.
        let t0 = Instant::now();

        let mut a = converged_on(CpMode::ShortCp);
        a.track_pending_confirm(1, CpMode::LongCp);
        let a_target = a.abort();

        let mut b = converged_on(CpMode::ShortCp);
        b.apply_as_confirmer(CpMode::LongCp, t0);
        let b_target = match b.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)) {
            Some(CpTimeout::RevertTo(mode)) => mode,
            other => panic!("expected a probation timeout, got {other:?}"),
        };

        assert_eq!(
            a_target, b_target,
            "the two give-up paths must have one convergence target"
        );
        assert_eq!(
            a_target,
            CpMode::ShortCp,
            "and it is the pre-negotiation mode"
        );
        assert_eq!(a.current(), b.current());
    }

    #[test]
    fn giving_up_with_no_negotiation_in_flight_leaves_the_settled_mode_alone() {
        // A bare `abort()` must not drag a station off a mode it legitimately
        // settled on -- there is no negotiation to rewind.
        let mut n = converged_on(CpMode::ShortCp);
        assert_eq!(n.abort(), CpMode::ShortCp);
        assert_eq!(n.current(), CpMode::ShortCp);
    }

    #[test]
    fn a_completed_negotiation_clears_the_give_up_target_on_both_sides() {
        // Otherwise a later `abort()` rewinds two negotiations instead of one.
        let t0 = Instant::now();

        // A's side: the third leg being acked completes the handshake.
        let mut a = CpNegotiator::new();
        a.track_pending_confirm(1, CpMode::ShortCp);
        assert_eq!(a.on_confirm_acked(&[1]), Some(CpMode::ShortCp));
        a.track_pending_switched(2);
        assert!(a.on_switched_acked(&[2]));
        // Second negotiation, ShortCp -> LongCp: giving up returns to ShortCp,
        // not all the way to the LongCp the FIRST negotiation started from.
        a.track_pending_confirm(3, CpMode::LongCp);
        assert_eq!(a.abort(), CpMode::ShortCp);

        // B's side: the third leg arriving completes the handshake.
        let mut b = CpNegotiator::new();
        b.apply_as_confirmer(CpMode::ShortCp, t0);
        b.on_peer_switched(CpMode::ShortCp);
        b.apply_as_confirmer(CpMode::LongCp, t0);
        assert_eq!(
            b.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            Some(CpTimeout::RevertTo(CpMode::ShortCp))
        );
    }

    #[test]
    fn a_fresh_negotiator_never_times_out() {
        let mut n = CpNegotiator::new();
        let t0 = Instant::now();
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS * 10)),
            None
        );
    }

    #[test]
    fn probation_expiring_with_nothing_to_revert_to_reports_no_timeout() {
        // Review finding: `tick` used to report `RevertTo(current)` here, so
        // the daemon rebuilt the transceiver onto the mode it was already on
        // and warned "reverted" about a change that never happened. Reachable
        // when the negotiation this probation belonged to completed by another
        // route -- which is what `on_switched_acked` does to `revert_to`.
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        n.track_pending_switched(9);
        assert!(n.on_switched_acked(&[9]), "test setup: clears revert_to");

        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            None,
            "no pre-negotiation mode left, so nothing to report"
        );
        assert_eq!(
            n.current(),
            CpMode::ShortCp,
            "and the settled mode must be left alone"
        );
        // Probation is still disarmed, so this cannot re-fire later either.
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS * 10)),
            None
        );
    }

    #[test]
    fn negotiation_in_flight_tracks_every_wait_state() {
        let t0 = Instant::now();

        let mut fresh = CpNegotiator::new();
        assert!(!fresh.negotiation_in_flight());
        assert_eq!(fresh.abort(), CpMode::LongCp);
        assert!(!fresh.negotiation_in_flight());

        let mut a = CpNegotiator::new();
        a.track_pending_confirm(1, CpMode::ShortCp);
        assert!(a.negotiation_in_flight(), "an unacked Confirm counts");
        assert_eq!(a.on_confirm_acked(&[1]), Some(CpMode::ShortCp));
        assert!(
            !a.negotiation_in_flight(),
            "an acked Confirm alone leaves nothing tracked"
        );
        a.track_pending_switched(2);
        assert!(a.negotiation_in_flight(), "an unacked third leg counts");
        assert!(a.on_switched_acked(&[2]));
        assert!(!a.negotiation_in_flight());

        let mut b = CpNegotiator::new();
        b.apply_as_confirmer(CpMode::ShortCp, t0);
        assert!(b.negotiation_in_flight(), "armed probation counts");
        b.on_peer_switched(CpMode::ShortCp);
        assert!(!b.negotiation_in_flight());
    }

    #[test]
    fn unknown_kinds_still_return_none_after_adding_switched() {
        assert!(CpNegotiator::on_content_received(&[0x04, 0x00]).is_none());
        assert!(CpNegotiator::on_content_received(&[0x00, 0x00]).is_none());
    }
}
