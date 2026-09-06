# H — Missing product surfaces: what coppa doesn't have yet, and which of them matter

Reviewer H of 8. Scope: (1) operator UI, (2) chat/messaging/file-transfer app layer, (3) browser WASM demo, (4) mobile/tablet, (5) modem-as-appliance. Repo read-only at `/Users/thagale/Code/coppa` (HEAD `1c2ffc5`). Evidence: source reads, a live loopback `coppad` run probed over WebSocket and the VARA TCP ports, a wasm32 build, and a web sweep of VARA/VarAC/FreeDATA/ardopcf/Pat/JS8Call/fldigi/DigiPi/Pi-Star.

## Summary

Coppa has no operator-facing surface at all today: no GUI, no TUI, no web page, and `coppa listen` is the closest thing to a monitor. The daemon already produces most of the raw telemetry a dashboard needs (a 128-bin/4 Hz spectrum stream over WebSocket, per-frame SNR/CFO/speed-level/delay-spread, PTT, BUSY, BUFFER), but the WebSocket surface is a half-wired shell: the live probe shows it emits only opt-in `spectrum` frames and a `status` reply-on-request; PTT/BUSY/BUFFER go to the VARA port only, session connect/disconnect notifications are generated and then dropped on the floor in `main.rs`, connected-mode session data never reaches WebSocket clients at all, and unconnected decoded data is pushed as a bare non-JSON string. So the first, cheapest, highest-leverage work is not a UI — it is a typed push-event schema on the WebSocket (`ptt`, `busy`, `buffer`, `session`, `data`, `heard`, `level`, `log`) that the VARA path already computes. On top of that, the right first surface is an **ardopcf-style single-page web dashboard served by `coppad` itself** (waterfall, RX level meter, S/N, speed level, PTT/BUSY lamps, buffer, session state, log) — this is table stakes in every reference modem and is the only way a new operator can even calibrate a level. **Chat** should be built as a tab of that same dashboard, with the protocol pieces (message framing, delivery receipts, heard-list beacons, ping/SNR exchange, chunked file transfer) living in `coppa-protocol` so Pat/others can reuse them; `AppPdu` framing/fragmentation already exists but has zero callers. The **WASM demo is cheap and verified feasible**: `coppa-engine` compiles unmodified for `wasm32-unknown-unknown` and a release cdylib is 490 KB before `wasm-opt`; nothing in the encode/streaming-decode path touches threads, `Instant::now()`, tokio or cpal. **Mobile** should be the responsive web dashboard on a LAN-bound daemon (Pat's pattern), not a native app; it needs an auth token first. **Appliance** should defer to a `.deb`/arm64 binary + systemd unit + a first-run wizard page in the dashboard, and a DigiPi contribution, rather than a maintained Pi image. Two daemon bugs surfaced during the probe that would break any of these surfaces on day one: TX audio is truncated (`Audio output buffer overflow dropped=57328 total=65520`) because the 8192-sample ring is smaller than one frame, and the busy gate flaps ON/OFF every ~20 ms in silence.

## Telemetry inventory

| Signal | Exists? | Where (file:line) | Exposed via WebSocket? | Rate / format |
|---|---|---|---|---|
| Spectrum (128 bins, 300–2800 Hz, dB) | yes | `crates/coppa-daemon/src/spectrum.rs:23-37`, `event_loop.rs:1063-1095` | **yes**, opt-in (`{"type":"spectrum","enabled":true}`, `websocket.rs:29-31`, filter `:200-215`) | measured 31 msgs / 8 s ≈ 3.9 Hz; ~1.4 KB JSON each (`bins:[f32;128], timestamp_ms`) |
| SNR (per decoded frame, dB) | yes | `coppa-engine/src/engine.rs:52` (`StreamFrame.snr_db`); emitted `event_loop.rs:1133` | **pull only** — stored in `WsStatus.snr` (`event_loop.rs:1247`), returned on `{"type":"status"}`; never pushed | VARA port pushes `SNR n` per frame; WS has no push |
| Speed level of last decoded frame | yes | `engine.rs:59`; `event_loop.rs:1248` | pull only (`status.level`) | integer 1–10 |
| CFO (Hz) | yes | `engine.rs:53`; `event_loop.rs:1249` | pull only (`status.cfo`) | f32 |
| Delay spread (ms) | yes | `engine.rs:67`, feeds `CpGate` | **no** — only collapsed to `short_cp_ok: bool` | — |
| Peer-recommended level | yes | `engine.rs:63` | no | — |
| Our own TX speed level (`RateLoop`) | yes | `coppa-ml/src/rate_loop.rs:94` `current_level()` | no (status `level` is the *peer's* TX level) | — |
| `cp_desync_episodes` | yes | `websocket.rs:63-80` | pull only, always serialized | u32 |
| PTT state | yes | `event_loop.rs:2721` `emit_vara(Ptt)` | **no** — VARA port only (`PTT ON/OFF`) | on change |
| BUSY (spectral occupancy) | yes | `event_loop.rs:805,1039`; `coppa-ml/src/busy_gate.rs:79` | **no** — VARA only | observed flapping ON/OFF every ~20 ms in silence (see capture) |
| BUFFER (tx queue depth) | yes | `event_loop.rs:541,555` | **no** — VARA only | on change |
| Session connected / disconnected | generated, then **dropped** | `event_loop.rs:2613-2700` sends `HostResponse::StatusUpdate{"CONNECTED"/"DISCONNECTED"}`; `main.rs:242-254` bridges only `DataOut` and discards `StatusUpdate` | **no** (neither WS nor VARA; `VaraResponse::{Pending,Connected,Disconnected}` at `vara/protocol.rs:36-40` have zero emit sites) | — |
| Session state machine (`Idle/Connecting/Accepting/Established/Disconnecting`) | yes | `coppa-protocol/src/session.rs:23-35` | no; only `status.connected: bool` recomputed on next decode (`event_loop.rs:1241-1246`) | — |
| Connected-mode RX data | yes | `event_loop.rs:2684-2694` → `response_tx` → VARA data port only | **no** — `handle_session_data` never touches `ws_broadcast` | — |
| Unconnected/ARQ RX data | yes | `event_loop.rs:1449-1451` | **yes but malformed**: sends `String::from_utf8_lossy(bytes)` raw, not `WsServerMessage::Data` JSON envelope (`websocket.rs:91-93` defines the envelope; unused) | — |
| Beacon / station-ID RX (callsign, grid, level) | decoded | `event_loop.rs:2549-2556` `handle_beacon_rx` — `tracing::info!` only; `StationIdPayload` at `mac.rs:279` | **no** (no "heard" list anywhere) | — |
| Decode failures (CRC/LDPC fail count) | no counter | `event_loop.rs:~1460` `tracing::debug!` only | no | — |
| RX audio level (peak/RMS/clip) | **no** | nothing computes it (grep `rms|peak|clip` in daemon/audio: none) | no | — |
| TX audio level / ALC aid | tune tone only | `engine.rs:268` `tune_tone`, VARA `TUNE` cmd | no WS trigger | — |
| Constellation / EVM | **no** | not in `StreamFrame` | no | — |
| ARQ stats (in-flight, RTO, SRTT, retransmits) | yes, internal | `coppa-protocol/src/arq.rs:645-665` | no | — |
| Log stream | tracing to stderr only | `main.rs` | no | — |
| Config read/write, device list | CLI only (`coppa devices`, `coppa config`) | `coppa-cli/src/main.rs:143-152` | no | — |
| Auth token / TLS | **no** | `coppad.toml.example:33-37` warns bind≠loopback is an unauthenticated PTT | n/a | — |
| Static file / HTTP serving | **no** | `websocket.rs` is raw `tokio-tungstenite` on port 8400; no HTTP GET | n/a | — |

Also: `coppad --help` starts the daemon with default config instead of printing help (observed; `main.rs:47-51` treats argv[1] as config path).

## Per-surface verdicts

### 1. Visual operator UI — **BUILD** (daemon-served web dashboard). TUI: cheap optional. Native GUI: don't.

**Who it's for:** every operator, on day one. Every reference modem ships a "tune-and-trust" window (VARA: waterfall + S/N + level + speed level + BUFFER; ardopcf `--webgui`: Rcv Level meter, waterfall, Send2Tone, PTT indicator; FreeDATA: Vue GUI over REST+WS). Without one, an operator cannot set RX gain, cannot see if the daemon hears anything, and cannot tell TX from a hang. This is table stakes, not a differentiator.

**Tech choice:** follow ardopcf, not FreeDATA. One static HTML file (`crates/coppa-daemon/ui/index.html`, vanilla JS + `<canvas>` waterfall, zero build step) embedded with `include_str!` and served by the daemon. Replace the raw `tokio-tungstenite` listener with `axum` (already on tokio) serving `GET /` (page), `GET /ws` (upgrade; keep the existing JSON schema), `GET /api/status`. Preact/Svelte only if a chat tab grows past ~1.5k lines; egui/Tauri native and Electron are rejected — they add a packaging matrix for zero user gain when a browser is already on every operator's machine, and they kill the RPi/tablet story.

**MVP feature list:** waterfall + spectrum line (existing 4 Hz stream; bump to 8–10 Hz for smoothness), RX level meter with green/orange/red (needs new `level` event), S/N with 30 s sparkline, speed level (ours and peer's), CFO, PTT and BUSY lamps, BUFFER count, session panel (state, peer, since, in-flight/retries, RTO), heard list (from beacons), TUNE button (two-tone/single), event log pane (session/decode/PTT lines), MYCALL/connect/disconnect/send box for smoke tests, theme-aware, mobile-responsive.

**API prerequisites (all `api-prereq` in the hit list):** typed push events on the WS broadcast (`ptt`, `busy`, `buffer`, `session`, `data` envelope, `heard`, `level`, `log`, `decode_fail`); fix the dropped `StatusUpdate` and the raw-string data push; RX level computation in `handle_audio_in`; per-client rate for `spectrum`; HTTP static serving; token auth when `bind_address` ≠ loopback.

**Effort:** M — ~1 week for the API events, ~1–2 weeks for the page. TUI variant (`coppa monitor`, ratatui, same WS client) is S (~3 days) and worth it for SSH-into-the-Pi operators; do it after the web page shares the event schema.

```
+--------------------------------------------------------------------------------+
| coppa  W5AU  HF_STANDARD  ws://127.0.0.1:8400        [PTT ●RX] [BUSY ○] [ARQ ✓]  |
+---------------------------------------------+----------------------------------+
| WATERFALL 300–2800 Hz                       | S/N  14 dB  ▁▂▃▅▆▅▇▆▅ (30 s)    |
| ░░░░░▒▒▓▓█▓▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ | LEVEL RX [■■■■■■■□□□] -12 dBFS  |
| ░░░░░▒▒▓▓█▓▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ | SPEED  TX 4/10   RX 5/10  CFO +3 Hz|
| ░░░░░▒▒▓█▓▓▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ | CP long  spread 1.8 ms  desync 0   |
| ░░░░░░▒▓▓█▓▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ +----------------------------------+
| ~~~spectrum line~~~/\_____/\____________    | SESSION  ESTABLISHED  ⇄ N0CALL   |
+---------------------------------------------+ since 00:03:12  buffer 2  rtt 4.1s|
| HEARD (24 h)                                | inflight 3/8  retx 1  rto 5.0 s   |
| N0CALL  FN20  L5  14:02 -  8 dB   [connect] +----------------------------------+
| K5XYZ   EM12  L3  13:41 - 11 dB   [connect] | [TUNE 2-tone] [TUNE 1500] [Beacon]|
+---------------------------------------------+----------------------------------+
| LOG                                                                            |
| 14:02:11 RX beacon N0CALL FN20 snr 8                                           |
| 14:02:30 PTT TX (CONNECT_REQ → N0CALL)   14:02:33 PTT RX                       |
| 14:02:41 session ESTABLISHED N0CALL (caps: win 8, comp on)                     |
+--------------------------------------------------------------------------------+
| > MYCALL W5AU | connect [N0CALL] | send: [hello________________] [Send] [Disc]  |
+--------------------------------------------------------------------------------+
```

### 2. Keyboard-to-keyboard chat / messaging / file transfer — **BUILD**, as a tab of the dashboard + protocol pieces in `coppa-protocol`

**Who it's for:** the VarAC audience — the largest active HF-data community that is not Winlink. VarAC is separate from VARA because VARA is closed source and VarAC is a different author; that separation is an accident, not a design virtue (FreeDATA, JS8Call bundle it). For coppa, the moat is integration: the modem knows SNR/level/heard stations, so the chat can show link quality per contact natively. But the *protocol* must live in `coppa-protocol` so Pat or a future third-party app can speak it.

**Exists:** sessions with capability negotiation (`session.rs`), selective-repeat ARQ with RTT estimator (`arq.rs`), Huffman+LZ4 compression, `AppPdu` with `AppProtocol::{Text,Position,Telemetry,FileTransfer,…}` + `fragment()/reassemble()` (`app.rs:109-330`) — **zero callers in daemon/CLI/FFI**, beacon TX with callsign+grid+level (`mac.rs:279`, `event_loop.rs:1669-1740`), busy-channel courtesy, station-ID timer, rigctld CAT client (`coppa-radio`).

**Missing:** (a) use of `AppPdu` framing on the wire so a receiver can tell "text" from "file chunk"; (b) delivery receipts surfaced to the app (ARQ ack → "✓ delivered" per message id); (c) heard list built from `handle_beacon_rx`; (d) ping / SNR-report exchange (VARA `PING` — both sides learn S/N before connecting); (e) unconnected short text (CQ/beacon text, VarAC-style "CQ" with free text); (f) file transfer with resume (fragment ids exist; no manifest/checksum/resume offset, no accept/reject handshake); (g) QSY negotiation (rigctld exists; needs `set_frequency` + a `QSY` control PDU); (h) store-and-forward / VMail-style parking (defer; JS8Call-class scope); (i) persistence (SQLite/JSONL log of QSOs and messages); (j) PSKReporter/spotting hooks (later).

**MVP:** chat tab (per-peer conversation, timestamps, ✓ delivered / ✗ failed), heard list with click-to-connect, unconnected CQ text, ping, file send/receive ≤ 64 KB with progress and accept prompt, message log persisted on the daemon. Store-forward, QSY, image gallery, relays: v2.

**Effort:** L — ~2 weeks protocol (`AppPdu` wiring, receipts, ping, heard, file manifest) + ~2 weeks UI + persistence. Depends on §1's event schema and on the session events actually reaching hosts.

### 3. Browser WASM demo — **BUILD** (cheap, verified feasible, best marketing asset available)

**Feasibility (measured):** `cargo build -p coppa-protocol --target wasm32-unknown-unknown` and `-p coppa-engine` both succeed unmodified on the pinned 1.98.0 toolchain (verbatim below). A scratch cdylib exposing `encode_bytes` + `push_samples` builds in release at **489,853 bytes** (`opt-level="s"`, lto, no `wasm-opt`; expect ~350 KB after `wasm-opt -Oz`, ~120 KB gzipped). Blockers checked: no threads/rayon, no tokio/cpal in the engine graph (`coppa-engine` deps: dsp, codec, protocol, anyhow — `Cargo.toml`); `std::time::Instant` appears only in `arq.rs` where the caller passes `now` (`arq.rs:411,432,494`) and in `#[cfg(test)]` QAM benches — nothing in `CoppaCore::encode_bytes`/`push_samples` calls `Instant::now()`, so no runtime panic. CPU: sync scan is 97% of RX CPU at ~0.04× realtime native (gap analysis §3); even 10× slower under wasm is fine for a live mic demo.

**MVP:** `crates/coppa-wasm` (wasm-bindgen, `encode(text, level) -> Float32Array`, `Decoder.push(Float32Array) -> frames[]`), a GitHub Pages site: type text → pick speed level → play through speakers (WebAudio) → decode from mic via `AudioWorklet` at 48 kHz → show waterfall + SNR + decoded text. Second-screen mode: "open on two phones and talk acoustically" (ggwave-style) — that is the demo people will share. Also doubles as the interop/education page for `docs/SPEC.md`.

**Effort:** S–M — 3–5 days for the crate + page; a further 2–3 days if `push_samples` needs a resampler for 44.1 kHz devices. Note: `coppa-dsp` does not have a resampler — the only implementation is `coppa-audio/src/resampler.rs`, which is not a `pub mod` of that crate today, so a browser build needs it either moved/exported for wasm or reimplemented; verified via `crates/coppa-audio/src/lib.rs`.

### 4. Mobile / tablet — **DEFER**; the answer is the responsive web dashboard, not an app

Who: an operator with a phone/tablet on the same LAN as a Pi or laptop running `coppad` (Pat's mobile-friendly web UI, DigiPi's phone-first pattern). A native iOS/Android app driving a rig from the phone's audio jack is a niche with real audio-routing pain and app-store cost; don't. Prerequisites for the LAN story: token auth (§1), `bind_address` LAN + mDNS `coppad.local`, touch targets in the dashboard, and TLS or an explicit "trusted LAN only" banner. Effort once §1 exists: S.

### 5. Modem-as-appliance — **DEFER, then build lite** (binary + service + first-run wizard; contribute to DigiPi; no maintained Pi image)

Who: unattended gateway / club-station operators. Pi-Star's AP-then-web-config onboarding is the gold standard; DigiPi already bundles Direwolf/ARDOP/Pat with a web manager and is the natural distribution channel. Building and maintaining a pi-gen image is L effort and permanent maintenance for a v0.1 modem. Instead: (a) arm64/armhf release binaries + `.deb` with a `coppad.service` unit, (b) `/setup` first-run wizard page in the dashboard (callsign/grid, audio device pick from an enumerate API, PTT method incl. GPIO pin, rigctld address, TUNE test, beacon interval), which needs a config-write API and restart-safe config, (c) a DigiPi PR adding coppa next to ardopcf. Effort: M (packaging S, wizard M). Performance already targeted (ARCHITECTURE.md:242 RPi 4; gap analysis says Pi Zero 2 feasible after sync fixes).

## Hit list

| # | Item | Category | Impact | Effort | Evidence / Source |
|---|---|---|---|---|---|
| 1 | Bridge `HostResponse::StatusUpdate` CONNECTED/DISCONNECTED to hosts — today it is generated in 4 handlers and discarded | api-prereq | H | S | `main.rs:242-254` matches only `DataOut`; `event_loop.rs:2613-2700` |
| 2 | Emit `VaraResponse::{Pending,Connected,Disconnected}` on the VARA port (variants exist, zero emit sites; VARA-style clients wait for these) | api-prereq | H | S | `vara/protocol.rs:36-40`; VARA probe shows no PENDING/CONNECTED after CONNECT |
| 3 | Push typed WS events: `ptt`, `busy`, `buffer`, `session{state,peer}`, `snr`, `level`, `heard`, `log`, `decode_fail` (mirror of what `emit_vara` already sends) | api-prereq | H | S | `event_loop.rs:527-534` `emit_vara` only; WS probe emitted nothing on PTT/connect |
| 4 | Wrap unconnected RX data in the `WsServerMessage::Data` JSON envelope (currently a bare string that breaks any JSON client) | api-prereq | H | S | `event_loop.rs:1449-1451` vs `websocket.rs:91-93` |
| 5 | Forward connected-session RX data (`handle_session_data`) to WS broadcast too | api-prereq | H | S | `event_loop.rs:2684-2694` only `response_tx` |
| 6 | RX audio level meter event (peak/RMS dBFS + clip flag, ~10 Hz) computed in `handle_audio_in` | api-prereq | H | S | no level computation anywhere; ardopcf "Rcv Level" is its primary calibration tool |
| 7 | Serve a static SPA from the daemon (`GET /` + `/ws` upgrade on one port; axum or a minimal HTTP responder before `accept_async`) | api-prereq | H | S | `websocket.rs:262-270` raw TCP → WS only |
| 8 | Bearer token and/or `Origin` allowlist for WS/HTTP, required regardless of `bind_address` -- loopback binding does NOT stop a malicious/compromised web page open in the operator's own browser from opening a cross-origin WebSocket to `127.0.0.1` and issuing TX commands, since the server checks neither Origin nor a token today (`[host] auth_token`, sent as first message or `?token=`) | api-prereq | H | S | `coppad.toml.example:33-37` warns of unauthenticated PTT; browser cross-origin attack surface |
| 9 | Fix TX audio truncation: `buffer_size=8192` ring vs 65,520-sample frame drops 57,328 samples per TX | api-prereq | H | S | daemon log `Audio output buffer overflow dropped=57328 total=65520` (capture below); `coppad.toml.example:8` |
| 10 | Busy-gate hysteresis / hold time — flaps ON/OFF every ~20 ms in silence, would make a BUSY lamp strobe and defeats `busy_hold_ms` | api-prereq | M | S | VARA capture below; `busy_gate.rs:79` |
| 11 | Web dashboard MVP: waterfall+spectrum, level meter, S/N sparkline, speed levels, CFO, PTT/BUSY lamps, buffer, session panel, log, TUNE buttons | ui | H | M | §1; ardopcf/VARA/FreeDATA all have this |
| 12 | Heard-stations list from beacon RX (callsign, grid, level, SNR, last-heard, count) + WS `heard` event + `/api/heard` | ui | H | S | `event_loop.rs:2549` logs only; `StationIdPayload` has grid/level; VarAC Last Heard, JS8Call heard list |
| 13 | Spectrum stream: per-client rate (4/8/10 Hz) and optional binary frame (128×u8) to cut 1.4 KB JSON/frame | ui | M | S | `spectrum.rs:36`, WS probe 31 msgs/8 s |
| 14 | Session panel detail: expose `SessionState`, negotiated caps, ARQ in-flight/RTO/SRTT/retransmit count, our `RateLoop` level vs peer level | ui | M | S | `arq.rs:645-665`, `rate_loop.rs:94`, `session.rs:23` |
| 15 | Expose `delay_spread_ms` and `recommended_level` in status/events (currently collapsed to `short_cp_ok: bool`) | ui | L | S | `engine.rs:63,67` |
| 16 | Log stream over WS (`tracing` subscriber layer → broadcast) with level filter | ui | M | S | tracing to stderr only |
| 17 | `coppa monitor` ratatui TUI reusing the same WS client (SSH-to-Pi operators; Direwolf `-t` colored console as the floor) | ui | M | S | Direwolf `-t`; `coppa listen` is the only monitor today |
| 18 | Decode-failure counter and last-failure reason (sync found but CRC/LDPC failed) — a "hearing something but not decoding" indicator | ui | M | S | `event_loop.rs:~1460` `tracing::debug!` only |
| 19 | Wire `AppPdu` framing onto the session data path (text vs file vs control) — exists, unused | chat | H | S | `app.rs:109-330`; no callers in daemon/cli/ffi |
| 20 | Delivery receipts: per-message id → ARQ ack → `delivered`/`failed` event to the app | chat | H | M | `arq.rs:432 process_ack`, `is_failed` |
| 21 | Ping / SNR-report exchange (unconnected `PING <call>` → reply with heard S/N both ways) | chat | H | M | VARA PING; needs a MAC frame type |
| 22 | Unconnected CQ / short text beacon (free text in beacon payload, shown in heard list) | chat | M | S | VarAC CQ/beacon text; `build_beacon_mac_pdu` `event_loop.rs:1669` |
| 23 | Chat tab in the dashboard: per-peer threads, ✓/✗ receipts, click-to-connect from heard list, persisted JSONL/SQLite log | chat | H | M | §2 |
| 24 | File transfer: manifest (name/size/sha256/chunks), accept/reject prompt, progress, resume from chunk offset over `AppPdu::fragment` | chat | M | M | `app.rs:190-330`; VarAC/FreeDATA file transfer |
| 25 | QSY control PDU + `set_frequency` via rigctld (auto-QSY to a working slot and back) | chat | M | M | `coppa-radio` rigctld client exists; VarAC Auto-QSY |
| 26 | Store-and-forward "parked" messages with relay notification (JS8Call/VMail class) | chat | L | L | defer to v2 |
| 27 | `crates/coppa-wasm` (wasm-bindgen: `encode`, streaming `Decoder`) + GitHub Pages demo (text → speaker → mic → decode, waterfall, two-phone acoustic mode) | demo | H | S | builds today; 490 KB cdylib (capture below); ggwave/ft8js precedent |
| 28 | Publish the demo's waterfall/decoder as the same JS module the dashboard uses (one canvas waterfall implementation) | demo | M | S | avoids two waterfalls |
| 29 | Golden WAVs in the demo page ("hear what speed level 1 vs 10 sounds like at 0 dB SNR") — doubles as interop vectors | demo | M | S | gap analysis §4.5 "zero golden test vectors" |
| 30 | Responsive/touch layout + mDNS `coppad.local` + LAN bind guidance for phone/tablet use | ui | M | S | Pat web GUI mobile-friendly; §4 |
| 31 | Config read/write API (`GET/PUT /api/config`), audio device enumeration API, restart-safe apply | appliance | H | M | only `coppa devices`/`coppa config` CLI |
| 32 | First-run `/setup` wizard: callsign/grid, audio in/out pick, PTT method (serial/GPIO/rigctld), TUNE test, beacon interval | appliance | H | M | Pi-Star onboarding pattern |
| 33 | arm64/armhf release binaries + `.deb` with `coppad.service` (systemd) and `coppad --help`/`--config` proper flags | appliance | H | S | `coppad --help` starts the daemon (observed); no packaging |
| 34 | DigiPi integration PR (coppa next to ardopcf/Direwolf/Pat) instead of a maintained Pi image | appliance | M | S | digipi.org bundles ardopcf + Pat + web manager |
| 35 | Pat transport: confirm VARA-port OK/PENDING/CONNECTED semantics so Pat's `vara` transport works unmodified against `coppad` — then Pat *is* the Winlink UI and coppa needs none | api-prereq | H | S | VARA probe: no `OK` replies to MYCALL/LISTEN/CONNECT; Pat wiki (VARA transport over TCP 8300) |
| 36 | Public event-schema doc (`docs/API.md`: every WS message with example JSON, versioned `"v":1`) — prerequisite for anyone else building a GUI (FreeDATA publishes OpenAPI) | api-prereq | M | S | no API doc exists; ARCHITECTURE.md:84 "server scaffolded" |

## Verbatim captures

### A. WebSocket probe (loopback `coppad`, `--features cpal-backend,websocket`, config in `scratchpad/coppad-ws.toml`: `websocket_port=8411`, `vara_enabled=true`, `callsign="W5AU"`, `arq_enabled=true`, `ptt_method="none"`; Mac mic as RX, speakers as TX)

Client sequence: `status` → `spectrum enabled` → `mycall` → +1.5 s `send "hello from ws"` → +2.5 s `connect W5AU→N0CALL` → +4 s `status` → +4.5 s `not json` → +5 s `disconnect`.

```
[+0.01s] open
[+0.01s] recv (58 bytes): {"type":"status","connected":false,"cp_desync_episodes":0}
[+4.01s] recv (58 bytes): {"type":"status","connected":false,"cp_desync_episodes":0}
[+4.51s] recv (79 bytes): {"type":"error","message":"Invalid message: expected ident at line 1 column 2"}
[+8.01s] spectrum msgs received: 31; first: {"type":"spectrum","bins":[-108.59494,-106.83725,-107.293106,-94.35677,-91.15904,-96.53467,-107.1823,-112.43247,-104.856674,-104.49368,-105.963806,-103.58394,-108.394966,-102.296684,-99.751144,-101.52111,-103.16009,-108....
```

Nothing else arrived: no `connected`/`disconnected`/`pending`, no PTT, no busy, no buffer, no SNR push — even though the daemon log shows it keyed for the `send` and the `connect`:

```
2026-09-06T02:36:52.484103Z  INFO coppad::event_loop: Client connected client_id=1
2026-09-06T02:36:54.014387Z  INFO coppad::event_loop: PTT state change state="TX"
2026-09-06T02:36:54.066477Z  WARN coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=57328
2026-09-06T02:36:54.988252Z  INFO coppad::event_loop: Connect request client_id=1 destination=N0CALL
2026-09-06T02:36:55.014846Z  INFO coppad::event_loop: PTT state change state="TX"
2026-09-06T02:36:55.067120Z  WARN coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=114656
2026-09-06T02:36:55.803017Z  INFO coppad::event_loop: PTT state change state="RX"
2026-09-06T02:36:57.488324Z  INFO coppad::event_loop: Disconnect request client_id=1
2026-09-06T02:36:57.514931Z  INFO coppad::event_loop: PTT state change state="TX"
2026-09-06T02:36:59.303128Z  INFO coppad::event_loop: PTT state change state="RX"
```

### B. VARA TCP probe (same daemon, ports 8310/8311), for comparison — this is what the WS surface should mirror

```
[+0.00s] CMD<- "VERSION Coppa 0.1.0\r\n"
[+0.30s] CMD-> MYCALL W5AU
[+0.50s] CMD-> LISTEN ON
[+0.82s] CMD<- "BUSY ON\r\n"
[+0.84s] CMD<- "BUSY OFF\r\n"
[+0.90s] CMD<- "BUSY ON\r\n"
[+0.92s] CMD<- "BUSY OFF\r\n"
 ... (BUSY ON/OFF pairs every ~20 ms, 34 pairs in 8 s of a quiet room)
[+1.50s] CMD-> CONNECT W5AU N0CALL
[+3.04s] CMD<- "PTT ON\r\n"
[+3.50s] DATA-> hello
[+3.50s] CMD<- "BUFFER 1\r\n"
[+4.83s] CMD<- "PTT OFF\r\n"
[+4.83s] CMD<- "BUFFER 0\r\n"
[+4.85s] CMD<- "PTT ON\r\n"
[+5.50s] CMD-> DISCONNECT
[+6.64s] CMD<- "PTT OFF\r\n"
```

No `OK`, `PENDING`, `CONNECTED` or `DISCONNECTED` lines were ever sent. (Daemon killed after the probe; a second reviewer's `coppad ./coppad-review.toml` on 18300/18400 was left untouched.)

### C. wasm32 build

```
$ rustup target add wasm32-unknown-unknown --toolchain 1.98.0-aarch64-apple-darwin
info: component rust-std for target wasm32-unknown-unknown is up to date
$ cargo build -p coppa-protocol --target wasm32-unknown-unknown
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ cargo build -p coppa-engine --target wasm32-unknown-unknown
   Compiling coppa-engine v0.1.0 (/Users/thagale/Code/coppa/crates/coppa-engine)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.34s
$ ls target/wasm32-unknown-unknown/debug/*.rlib
libcoppa_codec.rlib  libcoppa_dsp.rlib  libcoppa_engine.rlib  libcoppa_ml.rlib  libcoppa_protocol.rlib
```

Scratch cdylib (`scratchpad/wasmdemo`, depends on `coppa-engine`, exports `coppa_wasm_encode` → `CoppaCore::encode_bytes` and `coppa_wasm_push` → `CoppaCore::push_samples`; `opt-level="s"`, `lto=true`, `panic="abort"`):

```
    Finished `release` profile [optimized] target(s) in 4.82s
-rwxr-xr-x  489853  target/wasm32-unknown-unknown/release/coppa_wasm_probe.wasm
```

(`wasm-opt`/`wasm-pack` not installed locally; the 490 KB figure is pre-`wasm-opt`.) The only `std::time::Instant` in the non-test engine graph is `coppa-protocol/src/arq.rs:30` where every public method takes `now: Instant` from the caller — no `Instant::now()` executes inside `encode_bytes`/`push_samples`, so no wasm runtime panic.

### D. Reference-UI sweep (web research, condensed)

- **VARA HF**: waterfall, S/N readout (rule of thumb: don't send below ~10 dB), RX/TX level indication for calibration, auto speed level indicator, PING returns S/N + level, BUFFER counter; native Win32 TNC controlled over TCP 8300 by Winlink Express/VarAC/Pat.
- **VarAC**: chat + Last Heard (24 h, tooltip time/freq), beacons/one-time beacon, file & image transfer (<2 KB unsolicited), Auto-QSY, link-speed indicator, VMail store-and-forward + parking/relay notification, gateways, QSO log, PSKReporter, scheduler.
- **FreeDATA**: headless server with REST + WebSocket, Vue/Electron GUI (also plain browser); waterfall, chat, file transfer, broadcast; publishes an OpenAPI surface.
- **ardopcf `-G/--webgui 8514`**: modem binary serves its own SPA: colour-coded Rcv Level meter (docs say trust it, not the AGC'd waterfall), waterfall, Send2Tone, PTT visual (white line on waterfall), dev-mode constellation.
- **Pat**: Go binary with embedded mobile-friendly HTTP UI; talks to ARDOP/VARA as external TCP services — a VARA-conformant coppa gets Pat's Winlink UI for free.
- **WSJT-X/JS8Call**: Wide Graph waterfall+spectrum; JS8Call's loved bits are the heard list populated by heartbeats, who-hears-whom graph, relay/ACK, store-and-forward with delivery ACKs.
- **fldigi**: waterfall/FFT/scope toggle, multi-signal browser. **Direwolf**: no GUI, `-t` coloured console.
- **Appliances**: DigiPi (Pi image bundling Direwolf/ardopcf/Pat + web manager, phone-first), Pi-Star (first-boot AP → web config with default creds), OpenWebRX (browser waterfall, no auth).
- **Browser WASM demos exist**: ggwave (data-over-sound), ft8js/webft8 (FT8 in WASM with mic input), wfweb; none for an ARQ HF modem — coppa would be first.
