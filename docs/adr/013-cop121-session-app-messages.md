# ADR-013: chat as a negotiated second stream inside the session ARQ

Status: accepted for COP-121. Version: 0.11.0.

Operators asked to chat with the peer from the dashboard and see when a
message arrived. Before COP-121 the dashboard's `send` box pushed raw host
bytes, which merge and split across frames, and its `ack` only meant
"accepted". `AppPdu` had framing but no length, so it could not delimit
itself inside a byte stream. The normative wire definition is SPEC §23.

1. D1: carry chat on stream 1 of the existing session ARQ, named by the
   transport `session_id` nibble. The nibble was always sent as 0 and
   ignored on receive (SPEC §18.1), and it survives plain frames,
   retransmissions and RS(12,6) bursts. Host bytes keep byte-identical
   frames and zero overhead, no frozen SPEC §21 vector changes, and one
   sequence space keeps ACKs and retransmissions as they are. Rejected:
   wrapping all host bytes in records (changes every coppad-to-coppad
   session's bytes and the `BUFFER` count); a new transport type 6 (frozen
   vector N06 pins it as unknown); a new MAC type 11 (N01 pins it, and an
   RS(12,6) burst cannot carry a MAC type); plain text lines in the host
   stream (no message boundary for receipts, and they mix with VARA host
   data).
2. D2: negotiate it with CONNECT flags bit 4, `app_messages`, as the AND of
   both sides, and require CONNECT_CFM to carry exactly that AND
   (`AppMessagesMismatch`). A 0.10.0 receiver delivers a nibble-1 segment as
   host bytes, which COP-121's research reproduced, so stream 1 is sent only
   when both stations agreed.
3. D3: coppad advertises `app_messages` only when it was built with
   `websocket` and `[host] websocket_enabled = true`. "Delivered" then always
   means the message reached a coppad whose dashboard can show it, now or
   later through `chat_history`. A default-configuration coppad keeps its
   0.9.0 capability bytes. Rejected: a separate config key (the WebSocket
   API switch is the control).
4. D4: stream 1 carries `[u16 BE length][AppPdu]` records, and
   `AppProtocol::Text` is chat. The length is needed because `AppPdu` has
   none and records span segments. The `AppPdu` fragment fields are unused.
   A length below 4 makes the rest of that session's stream 1 unreadable;
   the receiver warns once and drops it while ACKs continue. Other protocol
   ids are skipped by their length, leaving `Control` and `FileTransfer` free
   for ping, SNR exchange and file transfer later. Invalid UTF-8 becomes
   U+FFFD.
5. D5: a message is delivered when the sender's ARQ ACK covers its record's
   last byte. There is no receipt frame. The ACK handler reads the in-flight
   segment's length and stream before processing the ACK. This relies on
   stop-and-wait (`SESSION_ARQ_WINDOW = 1`), which a compile-time assertion
   guards. Rejected: a receipt frame (a reverse-direction frame per message)
   and read receipts (the same cost, and a person must act).
6. D6: a message fails when its session ends with it unacknowledged, or when
   the daemon refuses it. The reasons are fixed strings in
   `coppa_protocol::app_stream::chat_reason`. There is no per-message
   timeout: a message waits as long as the session, fade hold included.
7. D7: queued chat goes ahead of queued host bytes. The segment after the
   one in flight takes from the chat backlog first, and each segment
   carries one stream only. A Winlink transfer can therefore be delayed by
   at most 32 messages of 518 bytes. Ordering between `send` and `chat` from
   the same operator is not kept.
8. D8: chat bytes are not counted in the VARA `BUFFER` or the WebSocket
   `buffer` value, because Pat's flush waits for `BUFFER 0` and must not see
   bytes it never wrote. A host `DISCONNECT` still waits for queued chat to
   drain, so `73` followed by a disconnect is delivered or failed, never
   dropped silently.
9. D9: a chat message is 1 to 512 UTF-8 bytes with at least one
   non-whitespace character; a session holds at most 32 unacknowledged
   messages; coppad keeps the last 100 messages in memory as history. The
   WebSocket task and the daemon share `check_chat_text`, and the daemon
   checks the waiting count.

The WebSocket protocol grows additively within version 1 (`chat`,
`chat_history`, `chat_receipt`, optional `status.chat`; see
`docs/OPERATING.md`). The public API changes (`LinkCapabilities` gains a
field; `HostEvent`, `WsClientMessage` and `WsServerMessage` gain variants)
need a minor version bump, so COP-121 ships as 0.11.0.

Not done here: chat in the library `Link`, the C FFI and the VARA, ARDOP and
KISS ports; persisting history across a restart; a WebSocket `listen`
request (COP-76); ping, SNR exchange and file transfer over a session. See
`docs/LIMITATIONS.md`.
