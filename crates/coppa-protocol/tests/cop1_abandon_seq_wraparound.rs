//! COP-1 verify-phase coverage: `ArqTx::abandon` across the u8 sequence-space
//! wraparound.
//!
//! `abandon` maps a seq to a ring slot with `seq as usize % MAX_WINDOW_SIZE`
//! and walks `send_base` forward with `wrapping_add`, exactly as
//! `process_ack`'s cumulative loop does. Both are correct across the 255 -> 0
//! boundary only because `256 % MAX_WINDOW_SIZE == 0`, so the seq -> slot
//! mapping stays continuous through the wrap and the `send_base != next_seq`
//! terminator still holds.
//!
//! Every one of the branch's six inline `abandon` tests starts at seq 0 and
//! never reaches the wrap, so nothing distinguishes the current arithmetic from
//! a non-wrapping variant. On the two-slot CP-control pair a wrap is reached
//! after 256 CP-control segments -- rare, but a long-lived link does get there,
//! and the failure mode (a `send_base` that can never again reach `next_seq`,
//! so `can_send` is false forever) would wedge CP negotiation permanently with
//! no give-up trigger able to recover it. Asserted rather than assumed.
//!
//! Lives in `tests/` rather than `arq.rs`'s inline `mod tests` because the
//! verify phase is read-only with respect to application source.

use coppa_protocol::arq::{ArqConfig, ArqTx};
use std::time::Instant;

/// Advance `next_seq`/`send_base` to `target` by sending and cumulatively
/// acking one segment at a time, so the pair really walks the sequence space
/// rather than being poked into position.
fn wind_to(tx: &mut ArqTx, target: u8, now: Instant) {
    while tx.next_seq() != target {
        let seq = tx.send(vec![0xAB], now).expect("window has room");
        // Cumulative ack of everything through `seq`.
        tx.process_ack(seq.wrapping_add(1), 0, now);
    }
    assert_eq!(tx.in_flight(), 0, "wind_to must leave the window empty");
}

#[test]
fn abandon_advances_send_base_across_the_u8_sequence_wraparound() {
    let mut tx = ArqTx::new(ArqConfig {
        window_size: 2,
        ..ArqConfig::default()
    });
    let now = Instant::now();

    // Park the pair one seq short of the wrap so the two segments below
    // straddle it: 255 and then 0.
    wind_to(&mut tx, 255, now);

    let s_last = tx.send(vec![1], now).expect("window has room");
    let s_wrapped = tx.send(vec![2], now).expect("window has room");
    assert_eq!(
        s_last, 255,
        "test setup: first segment is the last pre-wrap seq"
    );
    assert_eq!(s_wrapped, 0, "test setup: second segment wrapped to 0");
    assert_eq!(tx.in_flight(), 2, "both slots occupied across the wrap");
    assert!(!tx.can_send(), "test setup: the two-slot window is full");

    // Give up on the whole outstanding set, the way the daemon's CP give-up
    // block does. The advance loop must walk send_base 255 -> 0 -> 1.
    tx.abandon(s_last);
    tx.abandon(s_wrapped);

    assert!(tx.get_segment_data(s_last).is_none());
    assert!(tx.get_segment_data(s_wrapped).is_none());
    assert_eq!(
        tx.in_flight(),
        0,
        "send_base must have wrapped past 255 to meet next_seq -- otherwise \
         the pair can never send again"
    );
    assert!(
        tx.can_send(),
        "the CP-control pair must be usable again after wrapping"
    );

    // And the pair genuinely keeps working past the wrap.
    let after = tx.send(vec![3], now).expect("window must have room again");
    assert_eq!(after, 1, "sequence numbering continues past the wrap");
}

#[test]
fn abandon_of_a_wrapped_seq_does_not_clear_an_aliasing_pre_wrap_slot() {
    // Slot index is `seq % 32`, so seq 0 and seq 256 would alias -- the u8
    // sequence space wraps at exactly 256, a whole multiple of the ring size,
    // so the guard `seg.seq_num == seq` is what keeps an abandon from clearing
    // some other generation's segment that happens to share the slot.
    let mut tx = ArqTx::new(ArqConfig {
        window_size: 2,
        ..ArqConfig::default()
    });
    let now = Instant::now();

    wind_to(&mut tx, 254, now);
    let s254 = tx.send(vec![1], now).expect("window has room");
    assert_eq!(s254, 254);

    // A seq that maps to the SAME ring slot as 254 (254 % 32 == 30, and
    // 254 - 32 == 222 -> also 30) but is not the segment actually stored there.
    tx.abandon(222);

    assert!(
        tx.get_segment_data(s254).is_some(),
        "abandon must match on seq_num, not merely on ring slot"
    );
    assert_eq!(tx.in_flight(), 1, "the live segment must be untouched");
}
