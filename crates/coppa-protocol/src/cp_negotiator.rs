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
//! ## Give-up triggers -- every wait state converges on `LongCp`
//!
//! | # | Who waits | For what | Trigger | Action |
//! |---|---|---|---|---|
//! | G1 | B | its `Propose` acked | `ArqTx::is_failed(propose_seq)` | abandon segment, clear state (already `LongCp`) |
//! | G2 | A | its `Confirm` acked | `ArqTx::is_failed(confirm_seq)` | abandon segment, `abort()` (already `LongCp`) |
//! | G3 | B | `CpSwitched` from A | `tick` past the probation deadline | **revert engine + negotiator to `LongCp`** |
//! | G4 | A | its `CpSwitched` acked | `ArqTx::is_failed(switched_seq)` | abandon segment, `abort()`, **revert engine to `LongCp`** |
//!
//! Every single-leg loss fires a trigger on *both* stations, which is what
//! makes convergence total:
//!
//! | Lost leg | B fires | A fires | Converges to |
//! |---|---|---|---|
//! | `Propose` | G1 | -- (never saw anything) | `LongCp` |
//! | `Confirm` | G1 (its Propose is never acked either) | G2 | `LongCp` |
//! | bare ack | G3 | G2 | `LongCp` |
//! | `CpSwitched` | G3 | G4 | `LongCp` |
//!
//! `LongCp` is the convergence target because it is the mode both stations
//! boot into (`CpNegotiator::new`) and therefore the only mode a station
//! can safely assume a confused peer is on.
//!
//! This module owns only G3's clock; G1/G2/G4 are ARQ-budget-driven and
//! live in the daemon, which polls `pending_confirm_seq`/
//! `pending_switched_seq` against `ArqTx::is_failed`.

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
    /// B's side: `(deadline, mode we switched away from, mode we switched
    /// to)`. Armed by `apply_as_confirmer`, disarmed by `on_peer_switched`
    /// for the matching target mode, fired once by `tick` (G3).
    probation: Option<(Instant, CpMode, CpMode)>,
}

impl CpNegotiator {
    pub fn new() -> Self {
        Self {
            current: CpMode::LongCp,
            pending_confirm: None,
            pending_switched: None,
            probation: None,
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
    pub fn track_pending_confirm(&mut self, seq: u8, mode: CpMode) {
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
        let previous = self.current;
        self.current = mode;
        self.probation = Some((
            now + Duration::from_secs(SWITCH_PROBATION_SECS),
            previous,
            mode,
        ));
    }

    /// Disarm probation on proof the peer switched to the mode we switched
    /// to (B's side, on receiving A's third leg). A `PeerSwitched` naming a
    /// *different* mode is ignored -- a stale or garbled leg from some other
    /// negotiation cannot be proof of *this* switch, and accepting it would
    /// silently cancel the one safety net that makes the failure case
    /// recoverable.
    ///
    /// A no-op when no probation is armed.
    pub fn on_peer_switched(&mut self, mode: CpMode) {
        if matches!(self.probation, Some((_, _, target)) if target == mode) {
            self.probation = None;
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

    /// Give up on the in-flight negotiation and return to the conservative
    /// default. Idempotent, and safe to call from any state.
    ///
    /// Callers that may already have switched their engine (G4) must pair
    /// this with `engine.set_cp_profile(CpMode::LongCp)` -- this method only
    /// owns the bookkeeping half.
    pub fn abort(&mut self) {
        self.pending_confirm = None;
        self.pending_switched = None;
        self.probation = None;
        self.current = CpMode::LongCp;
    }

    /// Drive wall-clock deadlines (give-up trigger G3). Returns at most one
    /// timeout per call and clears the state that produced it, so a timeout
    /// fires exactly once no matter how often the caller polls -- the daemon
    /// calls this from a 500 ms tick, and a re-firing revert would spam
    /// `set_cp_profile` (a full engine rebuild) twice a second.
    pub fn tick(&mut self, now: Instant) -> Option<CpTimeout> {
        if let Some((deadline, previous, _target)) = self.probation {
            if now >= deadline {
                self.probation = None;
                self.current = previous;
                return Some(CpTimeout::RevertTo(previous));
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
    fn abort_clears_all_wait_state_and_restores_long_cp() {
        let t0 = Instant::now();
        let mut n = CpNegotiator::new();
        n.track_pending_confirm(3, CpMode::ShortCp);
        n.track_pending_switched(4);
        n.apply_as_confirmer(CpMode::ShortCp, t0);
        n.abort();
        assert_eq!(n.current(), CpMode::LongCp);
        assert_eq!(n.pending_confirm_seq(), None);
        assert_eq!(n.pending_switched_seq(), None);
        assert_eq!(
            n.tick(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1)),
            None
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
    fn unknown_kinds_still_return_none_after_adding_switched() {
        assert!(CpNegotiator::on_content_received(&[0x04, 0x00]).is_none());
        assert!(CpNegotiator::on_content_received(&[0x00, 0x00]).is_none());
    }
}
