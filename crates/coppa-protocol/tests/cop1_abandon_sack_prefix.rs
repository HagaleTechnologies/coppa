//! COP-1 verify-phase coverage: `ArqTx::abandon`'s advance loop must consume a
//! *selectively-acked* segment, not stop at it.
//!
//! `abandon`'s doc claims its prefix advance "mirrors `process_ack`'s cumulative
//! advance", and `process_ack` treats a slot as resolved when it is either empty
//! or holds an already-acked segment. `abandon` therefore has three arms:
//! `None => {}` (advance), `Some(s) if s.acked => clear + advance`, and
//! `_ => break`. The branch's own unit tests only ever reach the first and
//! third — nothing calls `process_ack` with a SACK bitmap before `abandon`, so
//! the middle arm is never exercised.
//!
//! That arm is load-bearing: if it were `break` instead of consume, `send_base`
//! would stall behind a SACK'd-but-not-cumulatively-acked segment and the
//! two-slot CP-control pair would still leak the slot `abandon` exists to
//! reclaim — the exact failure COP-1 added `abandon` to prevent, reintroduced
//! silently.
//!
//! Lives in `tests/` rather than `arq.rs`'s inline `mod tests` because the
//! verify phase is read-only with respect to application source.

use coppa_protocol::arq::{ArqConfig, ArqTx};
use std::time::Instant;

#[test]
fn abandon_advances_send_base_over_a_selectively_acked_segment() {
    let mut tx = ArqTx::new(ArqConfig {
        window_size: 2,
        ..ArqConfig::default()
    });
    let now = Instant::now();

    let s0 = tx.send(vec![1], now).unwrap();
    let s1 = tx.send(vec![2], now).unwrap();
    assert_eq!(tx.in_flight(), 2, "both slots occupied");

    // Selectively ack s1 only, leaving s0 unacked and still at `send_base`.
    // `process_ack`'s SACK loop reads `seq = ack_num + bit + 1`, so a bitmap of
    // bit 0 with `ack_num == s0` targets s1 while the cumulative loop is a no-op
    // (`send_base == ack_num` already).
    let newly_acked = tx.process_ack(s0, 0b1, now);
    assert_eq!(newly_acked, vec![s1], "only s1 should be newly acked");
    assert_eq!(
        tx.in_flight(),
        2,
        "a SACK alone must not advance send_base past the unacked s0"
    );

    // Giving up on s0 now resolves the whole prefix: slot s0 is emptied, and the
    // advance loop must then CONSUME the acked s1 rather than break on it.
    tx.abandon(s0);

    assert_eq!(
        tx.in_flight(),
        0,
        "abandon must advance send_base over the selectively-acked s1 too, \
         mirroring process_ack -- stopping at it would leak a window slot"
    );
    assert!(tx.get_segment_data(s0).is_none());
    assert!(tx.get_segment_data(s1).is_none());
    assert!(
        tx.can_send(),
        "the CP-control pair must be usable again after the prefix resolves"
    );
}
