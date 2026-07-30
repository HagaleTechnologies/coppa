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
//! - **Proposer** (the station whose `CpGate` observed a qualifying
//!   transition): sends a `Propose(mode)` payload via `propose_payload`.
//!   Retried by the caller's `ArqTx` polling loop like ordinary data --
//!   this module has no state for the proposer side; once the proposer's
//!   Propose is acked, nothing further happens until a Confirm arrives.
//! - **Confirmer** (the station that received a Propose): accepts
//!   unconditionally (`on_content_received` on a Propose payload always
//!   yields `SendConfirm`), replies with a Confirm, and applies the new
//!   mode to its own RECEIVER immediately via `apply_as_confirmer` (it can
//!   do this the moment it decodes the Propose -- no need to wait for its
//!   own Confirm to be acked, since the confirmer isn't the one switching
//!   the risky side: it just needs to be ready to decode the proposer's
//!   future frames under the new CP).
//! - **Proposer, second half:** the proposer applies the new mode to its
//!   own ENCODER only once it sees its peer's bare ack for the peer's
//!   Confirm (`track_pending_confirm`/`on_confirm_acked`) -- this is what
//!   guarantees it never switches before the confirmer is proven ready.
//!
//! Concretely, with the roles named as in the design doc: call the station
//! that observed a calm channel and wants a change "B" (it sends Propose),
//! and the station whose transmissions B wants changed "A" (it receives
//! Propose, sends Confirm). B applies the new mode to its OWN RECEIVER as
//! soon as it receives A's Confirm content (`on_content_received` ->
//! `ApplyAsConfirmer`, then `apply_as_confirmer`). A applies the new mode
//! to its OWN ENCODER only once B's bare ack for A's Confirm arrives
//! (`track_pending_confirm` when A sends the Confirm, `on_confirm_acked`
//! when that ack lands).

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

/// What to do after decoding a received CpControl content payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentAction {
    /// Received a Propose; accept unconditionally (this design's "trust
    /// outright" decision) and send this Confirm payload back.
    SendConfirm(Vec<u8>),
    /// Received a Confirm; apply this mode to OUR OWN RECEIVER immediately
    /// -- the confirmer's half of the handshake, see the module doc.
    ApplyAsConfirmer(CpMode),
}

/// Negotiation state for one direction of one link. See the module doc for
/// the proposer/confirmer role split.
pub struct CpNegotiator {
    current: CpMode,
    /// Set when we (as confirmer) sent a Confirm and are waiting to see its
    /// ARQ seq acked before applying `mode` to our own encoder.
    pending_confirm: Option<(u8, CpMode)>,
}

impl CpNegotiator {
    pub fn new() -> Self {
        Self {
            current: CpMode::LongCp,
            pending_confirm: None,
        }
    }

    /// The mode this negotiator currently believes is in effect.
    pub fn current(&self) -> CpMode {
        self.current
    }

    /// Build a Propose payload for `mode` (proposer role).
    pub fn propose_payload(mode: CpMode) -> Vec<u8> {
        vec![KIND_PROPOSE, mode.to_wire()]
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

    /// Apply `mode` immediately (confirmer role, on receiving Confirm
    /// content -- see the module doc for why the confirmer doesn't wait
    /// for an ack first).
    pub fn apply_as_confirmer(&mut self, mode: CpMode) {
        self.current = mode;
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
        n.apply_as_confirmer(CpMode::ShortCp);
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
}
