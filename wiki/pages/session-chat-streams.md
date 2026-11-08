---
id: session-chat-streams
title: What will bite you about session chat streams?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/SPEC.md
  - docs/adr/013-cop121-session-app-messages.md
  - crates/coppa-protocol/src/app_stream.rs
  - crates/coppa-daemon/src/event_loop.rs
  - crates/coppa-daemon/src/web/index.html
verified:
  commit: f6e9951
  date: 2026-11-08
links:
  - coppa-protocol
  - session-fade-protection
  - overview
---
The transport `session_id` nibble means "stream" only after both stations
negotiated `app_messages`. A 0.10.0 receiver, or any session without the bit,
ignores it and hands a stream 1 segment to the host as data. Never send stream
1 on a session that did not negotiate it. SPEC §23 has the wire rules and
ADR-013 the choices.

Chat bytes stay out of `BUFFER` and the WebSocket `buffer` count. Pat waits
for `BUFFER 0`, so counting chat there would stall its flush. A host
`DISCONNECT` still waits for queued chat to drain.

The sender's per-segment stream and its delivery count assume one segment in
flight. A compile-time assertion on `SESSION_ARQ_WINDOW` fails the build if
someone raises the window without revisiting the receipt code.

On the dashboard page, `main { display: grid; }` overrides the browser's
`[hidden]` rule. The tab panels need their own
`[role="tabpanel"][hidden] { display: none; }` rule, or a hidden panel still
shows.
