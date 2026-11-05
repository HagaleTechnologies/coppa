# ADR-011: A sans-IO library link in coppa-engine, exposed to C and Python

## Status

Accepted, COP-139, 2026-11-03.

## Context

Before COP-139 the C library exposed frame encode and decode only. The code
that turns `Session`, `ArqTx` and `ArqRx` into a connected link (the CONNECT
handshake, stop-and-wait ARQ, retransmission, keepalive and disconnect) was
private to coppad, in `crates/coppa-daemon/src/event_loop.rs` (`SessionLink`,
`session_data_frame`, `session_ack_frame` and the handshake, data and ACK
handlers). A C or Python program could send frames but could not hold a
reliable link without running coppad. H-124 and H-125 (COP-89) ask for that,
and COP-139's first scenario is two Python processes holding a link.

The building blocks are public, synchronous and clock-free in `coppa-protocol`
(COP-90 made `Session` take `now: Instant`). A spike over public APIs alone
delivered 100 bytes with the first data frame dropped, and 1,000 bytes in 30
frames, over a 960-sample-per-20-ms lockstep cable.

## Decision

1. **`coppa_engine::link::Link` lives in `coppa-engine` and reads no clock
   (D5).** It needs `CoppaCore`, and `coppa-engine` must compile for
   `wasm32-unknown-unknown`. Every method that needs the time takes
   `now: Instant`. `crates/coppa-engine/clippy.toml` bans `Instant::now` and
   `Instant::elapsed`, as `crates/coppa-protocol/clippy.toml` does.
2. **The C API maps `now_ms` onto an `Instant` captured at `coppa_link_new`
   (D6).** C has no `Instant`; `base + Duration::from_millis(now_ms)` serves
   real time and virtual (test) time with the same calls. A decreasing
   `now_ms` is rejected with `COPPA_ERR_INVALID_PARAM`.
3. **`Link::process(now, rx, tx) -> usize` is shaped like an audio callback
   (D7).** It ignores `rx` while it has samples to transmit, as a half-duplex
   radio does, writes its signal as a prefix of `tx`, zero-fills the rest and
   returns the signal length, which the caller uses as its PTT signal. At most
   one frame is keyed per call.
4. **The wire frames copy coppad's exactly (D8).** Data is
   `MacPdu::new_data(remote, local, ssid, TransportPdu::new_reliable(0, seq, 0, chunk))`,
   ACK is `MacPdu::new(Ack, remote, local, ssid, TransportPdu::new_ack(0, ack_num, bitmap))`,
   the REQ ssid is 0 and the responder echoes it, a duplicate reliable frame
   is re-ACKed, and `Beacon`, `Relay` and frames for other stations are
   ignored. A library station and coppad use the same frames (ADR-010).
5. **The link adds sender-side handshake recovery that coppad lacks but
   answers correctly (D9).** CONNECT_REQ is re-sent every `initial_rto` up to
   `connect_retries` times (default `MAX_CONNECT_RETRIES` = 3), then the link
   ends with `HandshakeFailed`. Until the first ACK the initiator sends its
   cached CONNECT_CFM before each data retransmission. coppad as responder
   re-ACKs a repeat REQ while `Accepting` and ignores a repeat CFM.
6. **Retry exhaustion ends the link (D10)** with `RetriesExhausted` and one
   best-effort DISCONNECT frame, as coppad's `fail_session_link` does. There is
   no fade hold.
7. **Defaults match coppad (D11)**: turnaround 500 ms, `max_retransmit` 5,
   `initial_rto` 5 s, `connect_retries` 3, keepalive 30 s and session timeout
   120 s, all in `LinkConfig`. The C config exposes `turnaround_ms`.
8. **Python uses `ctypes` and the standard library only**, through the C API,
   with an ABI check at load (`docs/DECISIONS/2026-11-03-c-abi-versioning.md`).

## Rejected alternatives

- **Moving `tx_chunker` into `coppa-engine` now.** Open PR #204 (COP-110)
  edits `tx_chunker.rs`. `Link` carries a private copy of `next_chunk_len`'s
  single-codeword algorithm instead; unify after #204 lands.
- **Making coppad run its session on `Link` now.** That touches a
  22,000-line event loop with fade hold, PTT and host notices, and neither
  COP-139 scenario needs it.
- **A per-handle error string.** Handle-less failures (`NULL` from
  `coppa_link_new` or `coppa_engine_new_with`, a `NULL` handle) need a slot
  that is not on a handle, so the error is thread-local (`coppa_last_error`).

## Consequences

- C and Python programs can hold a connected, reliable link without coppad.
- The link is a subset of coppad's session: no fade hold (COP-68), one
  codeword per frame (COP-69), no busy-channel wait, PTT, beacon or CW ID.
  `docs/LIMITATIONS.md` lists these, and that it has not run against a live
  coppad.
- Both ends must use the same profile, because a receiver with compression
  off cannot parse a compressed frame.
- Link tests are about 11 times slower in debug than in release, so they keep
  payloads at 24 to 100 bytes and shorten timers through `LinkConfig`. Two of
  them (handshake failure and retry exhaustion) use `initial_rto = 4 s`,
  because B's ACK lands about 3.3 s after the REQ starts and a 3 s REQ retry
  would key over it. In the lost-data test the initiator sends 5 frames, not 4:
  no ACK has arrived yet, so the cached CFM goes out before the data
  retransmission.
