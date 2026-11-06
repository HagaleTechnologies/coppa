# ADR-012: negotiated session erasure protection

Status: accepted defaults for COP-119. Version: 0.9.0.

COP-118 measured packet erasure coding as the useful fade-diversity arm.
Frequency repetition was rejected. See `../analysis/COP-118-fade-diversity.md`.
The normative wire definition is SPEC §22.

1. D1: advertise receive support and a request separately in CONNECT flags
   bits 2 and 3. Both peers must receive; either may request. An `off` peer
   wins. Default `accept` peers do not enable the mode with each other.
2. D2: MAC type 10 carries a four-byte outer header followed by one shard.
   The reconstructed segment is the existing Reliable transport PDU.
3. D3: use systematic RS(12,6), GF(256), polynomial 0x11d, identity data
   rows and Cauchy parity rows. Any six distinct valid shards suffice.
4. D4: reserve PHY keys 0x10 through 0xff using block sequence modulo 20
   and packet index. Retries keep their keys. Delivery evicts current and
   previous block keys; teardown clears the reserved range.
5. D5: delay ACK until the estimated burst end. Send two copies in one
   keying. A single ACK proved vulnerable in planning measurements. After a newly
   acknowledged block, the sender holds one additional ACK duration to avoid
   keying over the second copy. Duplicate ACKs do not extend that hold.
6. D6: do not compress RS packets. Variable packet lengths would violate
   the equal-shard construction. Host payloads may already be compressed.
7. D7: pin the speed level for the life of an in-flight block. Recutting
   shards during fallback would invalidate retained receiver packets.
8. D8: floor the RTO at keyed burst plus both turnarounds and two ACKs.
   Bound unknown peer ACK airtime with the slowest HF level, since the
   peer can transmit at a different level from the sender.
9. D9: when the burst plus possible ID and one-second margin exceeds the
   configured TX duration, send that segment on the plain path. Warn once.
10. D10: bump the minor version for new public Rust fields and
    enum variants, and record the negotiated wire extension in SPEC.
    The branch's own 0.6.0 → 0.7.0 bump was taken by main's COP-138
    (951030c) during the merge, and 0.8.0 was taken by COP-120
    (23e7696), so COP-119 ships as 0.9.0.
11. D11: measure the ticket against ideal-ACK simulated ten-minute
    Moderate sessions. Separately report ACK loss and clean throughput.

Sender selective resend, per-packet ACKs and a larger ARQ window are deferred.
The receiver retains valid packets across complete-burst retries. The plain
path remains available inside a negotiated session for the TX-duration guard.
No rate-loop auto-selection, host command or RF reliability claim is added.
