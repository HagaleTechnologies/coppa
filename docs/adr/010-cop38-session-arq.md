# ADR-010: Connected-session ARQ and the 0.3.0 session wire break

## Status

Accepted, COP-38, 2026-10-27.

## Context

A connected (Pat/Winlink-style) session sent each host chunk as a bare
`MacPdu::Data` frame and forgot it. The receiver forwarded the payload to its
host and never acknowledged it. A frame lost on the air was never delivered
and never retransmitted; the sending host was told `BUFFER 0` at PTT release
anyway, and both sessions stayed `CONNECTED`. One lost frame silently
corrupted a multi-frame B2F transfer.

The COP-38 plan reproduced this on unmodified `main` twice: at the
`decode_and_dispatch_audio` level (with `arq_enabled = true` on both
stations, which did not help, because session data bypassed `ArqTx`/`ArqRx`)
and through two real `run()` loops over the half-duplex cable simulator with
the data frame zeroed. In both, the data was never delivered, nothing was
retransmitted and the session stayed up. A keepalive also refreshes the
session's idle timer, so a session whose peer is gone never times out.

`ArqTx`/`ArqRx` were real and already used by the sessionless raw path and
by the CP-control pair.

## Decision

1. **Session reliability is unconditional (D1).** It does not depend on
   `[engine] arq_enabled`, which keeps governing only sessionless traffic and
   `RateLoop`. The session retransmit pass runs on `run()`'s 500 ms
   retransmit tick.
2. **One session link per daemon (D2).** `EventLoop::session_link` holds the
   established session's `ArqTx`/`ArqRx`, a byte backlog and a give-up
   deadline. It is rebuilt at establishment on both sides, so a new session
   starts at seq 0, and dropped when the session ends.
3. **Wire shape (D3).** See the table below. ACK fields in data frames are
   zero, so a retransmission is byte-identical to the first send.
4. **Window 1, stop-and-wait (D4).** With one ACK per frame and a
   receive-side-only turnaround hold, a second in-flight frame would collide
   with the peer's ACK.
5. **Retransmission and link failure (D7).** `ArqConfig::default()`'s budget
   (5 retransmissions, 5 s initial RTO, 60 s cap) with the RTO floor computed
   from the engine's level and `turnaround_ms + ptt_pre_delay_ms +
   ptt_tail_delay_ms`. An attempt counts even when its encode fails. A
   refusing wideband gate skips the pass without spending backoff or budget.
   Once the in-flight frame has used its last retransmission, one more RTO
   is allowed for a late ACK; then the daemon sends a best-effort
   DISCONNECT, reports `BUFFER 0` and then `DISCONNECTED`.
6. **`BUFFER` is ACK-driven (D6):** backlog plus unacknowledged bytes.
7. **The wire break is documented, with a minor version bump and no
   negotiation (D11).** The workspace goes from 0.2.0 to 0.3.0, following
   ADR-009's rule and the ADR-003/005 no-negotiation precedent.

| Frame | Before (0.2.0) | After (0.3.0) |
|---|---|---|
| Session data | `MacPdu{Data}` with the host chunk as payload | `MacPdu{Data}`, payload = `TransportPdu::new_reliable(0, seq, 0, chunk)`: 7-byte header `[0x0 session_id \| 0x1 Reliable][seq][ack=0][bitmap=0×4]` + chunk |
| Session ACK | none | `MacPdu{Ack (type 1)}`, payload = `TransportPdu::new_ack(0, ack_num, bitmap)` (7 bytes) |
| CONNECT_REQ/ACK/CFM, DISCONNECT, KEEPALIVE, BEACON | unchanged | unchanged |

The MAC header `version` stays 1.

## Rejected options

- **A window above 1.** The peer's per-frame ACK would collide with our next
  frame, because the turnaround hold is receive-side only
  (`docs/OPERATING.md`, "Half-duplex turnaround"). A burst/block-ACK design
  is a follow-up.
- **Gating session ARQ on `arq_enabled`.** The flag defaults off, and a Pat
  user has no reason to know about it; the ticket asks that connected
  sessions retransmit.
- **Negotiating the format through `LinkCapabilities`.** It has no spare
  capability bit, and adding one would itself break the handshake.
- **Leaving `BUFFER` PTT-driven.** It reported bytes as gone before the peer
  had them, which COP-30's "decremented on ACK" contract never intended.

## Limits

- Verified in-process only: daemon-to-daemon dispatch tests and the
  two-`run()`-loop cable simulator. No live two-radio test and no run of real
  Pat.
- Clean-link throughput falls to about 0.57-0.67x of the unacknowledged
  stream (simulated and analytic, see BENCHMARKS.md's COP-38 section).
- The RTO margin over a real RTT is computed, not measured on air; a
  spurious retransmission is a harmless duplicate.
- Simultaneous bidirectional session writes can still put one station's
  data frame on top of the other's ACK.
- An Established session with nothing in flight still never times out while
  keepalives are sent; COP-38 detects a dead link only while data is in
  flight.
- A lost CONNECT_CFM is still unhandled (`Session::maybe_retry_cfm` is
  unwired).
- One session link per daemon. Session ACKs do not feed `RateLoop`.
- `DataOut` is still `try_send`: a full host channel drops bytes that were
  already ACKed (COP-39).
- A speed-level change mid-flight can leave a retransmission that no longer
  encodes; it then fails the link after the budget.
