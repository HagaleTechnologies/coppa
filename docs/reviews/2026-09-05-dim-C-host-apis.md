# coppa review — dimension C: host integration surfaces (VARA-style TCP, WebSocket JSON, KISS, C FFI)

Reviewer scope: what a host-app author (Pat / Winlink Express / VarAC / RMS / custom EmComm tool) or a ham pointing an existing VARA-speaking app at coppa actually experiences. Repo at `1c2ffc5` (`/Users/thagale/Code/coppa`). Read-only; daemon exercised on loopback (ports 18300/18301/18400, `ptt_method = "none"`) with the release binaries built `--features cpal-backend,websocket`. Reference material: EA5HVK "VARA Protocol Native TNC Commands" (Feb 2022, mirrored in `n8jja/Pat-Vara`), Pat's VARA transport source (`github.com/n8jja/Pat-Vara/vara`, used by Pat ≥ 0.13), ardopcf `Host_Interface_Commands.md`, AGWPE API spec.

## Summary

**Overall grade: D+ for the VARA/WS surfaces, B- for the FFI, C for KISS.** The README calls the VARA-style TCP interface "Working"; against a real VARA client it is not usable at all today. Three independent, wire-level defects each block Pat before the first command is even processed: (1) the command port reads lines with `read_line`, i.e. it requires `\n`, while VARA (and Pat, `writeCmd` → `cmd + "\r"`) terminate commands with a bare `\r` — I sent Pat's exact startup sequence and every command was buffered, then delivered to the event loop as one 150-character mashed line when the socket closed (`command=VERSIONMYCALL W5AUCOMPRESSION OFF...`, verbatim capture below); (2) coppa emits responses as `\r\n` while VARA emits bare `\r` — Pat splits the read buffer on `\r` and skips only empty strings, so every response after the first in a TCP segment arrives as `"\nBUFFER 0"` and fails Pat's `HasPrefix("BUFFER ")` match; (3) `CONNECTED` / `DISCONNECTED` / `PENDING` are never delivered to the command port: the daemon produces them as `HostResponse::StatusUpdate` but `main.rs:243` forwards only `DataOut` and silently drops every `StatusUpdate`. On top of that no command ever gets an `OK`/`WRONG` reply, `MYCALL` is ignored (callsign comes only from the TOML), `COMPRESSION`/`BW*`/`CHAT`/`PUBLIC`/`CWID`/`WINLINK SESSION`/`P2P SESSION`/`CLEANTXBUFFER`/`CQFRAME` are no-ops, and `IAMALIVE` is never sent (Pat's command socket has a 2-minute read deadline). Bytes written to the data port are transmitted immediately regardless of session state — with a real PTT that keys the transmitter for a host that merely pre-writes. The WebSocket API is the best-designed of the three text surfaces but is also incomplete: the `data`, `connected`, `disconnected` server messages are defined but never emitted (decoded payloads are broadcast as *raw text*, not JSON, `event_loop.rs:1450`), `mycall` is silently discarded, and payloads are `String` (no binary path, so Winlink B2F/compressed traffic cannot flow). KISS has a real framing bug (an unterminated frame straddling a TCP read boundary is emitted truncated) and ignores the port nibble, and there is no AGWPE. The FFI is the most polished piece — good ownership docs, panic-safety, a versioned v2 binary API, checked-in cbindgen header — but the header has no `extern "C"` guard for C++, no `coppa_*` error-string accessor, the official tutorial's C snippet calls `coppa_engine_destroy(engine)` with the wrong signature (UB), and nothing is packaged or published for Python/Go/C# consumers.

Three biggest problems, in order: **(1)** VARA command-port line discipline (`\r` in, `\r`/`\r\n` out) + dropped `StatusUpdate`s → zero VARA-app interoperability; **(2)** no `OK`/`WRONG`, no `MYCALL`, no `IAMALIVE`, no `PENDING`/`CANCELPENDING` → even after (1) is fixed a Winlink session cannot be brought up or torn down cleanly from Pat; **(3)** WebSocket `data`/`connected`/`disconnected` never emitted and decoded RX broadcast as non-JSON text → any dashboard that parses JSON will throw on the first decoded frame.

Out-of-dimension but observed in every TX capture and worth escalating to whoever owns the audio path: every transmission logged `Audio output buffer overflow dropped=57328 total=65520` (87% of each frame's samples dropped into an 8192-sample ring) — nothing coppa transmits over the host APIs is currently reaching the sound card intact.

## What's already good

- Clean separation: `coppa-host` owns transports, `HostEvent`/`HostResponse` is the single seam into the daemon (`crates/coppa-host/src/lib.rs`), so fixing the VARA wire details is contained to two files.
- Sensible DoS hygiene on every listener: 16-connection semaphores, 4 KiB max command line, 1 MiB / 256 KiB WS message/frame caps, 64 KiB max KISS frame (`vara/server.rs:16`, `vara/command.rs:13`, `websocket.rs:~60`, `kiss.rs:~30`).
- Loopback-only default bind with an explicit, well-worded warning in `coppad.toml.example` about the unauthenticated control plane keying a transmitter.
- The four VARA telemetry lines that *do* exist (`PTT ON/OFF`, `BUFFER n`, `BUSY ON/OFF`, `SNR n`) are emitted at the right moments (PTT at the physical PTT edge `event_loop.rs:2721`; BUFFER on enqueue and dequeue `event_loop.rs:541,555`) and have real tests (`event_loop.rs:6225-6880`).
- `TUNE [seconds]` two-tone calibration through the real PTT path is a genuinely useful operator feature VARA itself lacks on the TCP port.
- WebSocket: `#[serde(tag = "type")]` discriminated union, opt-in `spectrum` stream (avoids flooding non-dashboard clients), structured `error` reply on bad input, graceful handling of `Lagged` broadcast receivers.
- KISS: FEND/FESC/TFEND/TFESC escaping is correct both directions with tests; TCP server shape (mpsc TX, broadcast RX) is right.
- FFI: explicit error-code table, `catch_unwind` on every entry point (28 exported fns), poisoned-lock semantics documented, exact-length free contract documented, `coppa_engine_destroy(T**)` nulls the caller's pointer, v2 binary API (`coppa_encode_bytes` / `coppa_engine_feed_samples` / `coppa_next_frame` with SNR/CFO/level/seq metadata) is the right shape for a streaming modem, header is checked in and regenerated by `build.rs`.

## VARA compatibility matrix

Legend: **impl** = implemented and behaves like VARA; **partial** = parsed but incomplete / wrong format; **no-op** = accepted (no `WRONG`) but ignored; **missing** = not recognised (and, because there is no `WRONG`, silently ignored too); **differs** = behaves differently from VARA.

Precondition for the whole table: today none of the host→modem rows are reachable from a real VARA client because of the `\r` terminator bug (`vara/command.rs:55`). Rows describe behaviour once a `\r\n`-terminated line reaches the parser.

### Host → modem (command port)

| VARA command | VARA semantics | coppa status | Evidence |
|---|---|---|---|
| `CONNECT src dst` | start outbound session; reply `OK`, then `PENDING`/`CONNECTED src dst BW` or `DISCONNECTED` | **partial** — parsed (`protocol.rs:69`), CONNECT_REQ transmitted (`event_loop.rs:913-985`); no `OK`; source arg ignored; result `CONNECTED`/`DISCONNECTED` produced as `StatusUpdate` and dropped (`main.rs:243`) | capture 2 |
| `CONNECT src dst via d1 d2` (FM) | digipeat | **differs** — `splitn(4)` swallows `via ...` silently | `protocol.rs:52` |
| `DISCONNECT` | graceful, after TX buffer empties; `DISCONNECTED` when done | **partial** — sends DISCONNECT PDU immediately (`event_loop.rs:987-1030`), does not wait for `tx_queue` to drain, no `DISCONNECTED` reaches host | capture 2 |
| `ABORT` | dirty disconnect now | **no-op** — parsed (`protocol.rs:88`) but daemon handles only `LISTEN`/`TUNE` (`event_loop.rs:886-911`) | capture 2: `ABORT` → nothing |
| `LISTEN ON` / `LISTEN OFF` | enable/disable inbound; VARA drops an active link if received mid-connection | **impl** (flag only, `event_loop.rs:889-894`); does not drop active session | log: "Listening for incoming connections" |
| `MYCALL c1 [c2..c5]` | set up to 5 callsigns; `OK`/`WRONG` | **no-op** — parsed (`protocol.rs:56`) then discarded; local callsign only from TOML `[engine] callsign`; no multi-call/SSID support | `event_loop.rs:886` never matches `MYCALL` |
| `COMPRESSION OFF/TEXT/FILES` | Huffman modes | **no-op** — parser only knows `ON`/`OFF` (`protocol.rs:82`); `TEXT`/`FILES` parse as `Compression(false)`; nothing consumes it | |
| `BW500` / `BW2300` / `BW2750` | select waveform BW; `OK`/`WRONG` | **no-op** — parsed (`protocol.rs:85-87`), ignored; coppa has profiles/speed levels not BW modes; nothing maps them | |
| `CHAT ON/OFF` | chat timing, enables `SN` reports, implies `LISTEN ON` | **missing** | `protocol.rs:96` → `Unknown` |
| `PUBLIC ON/OFF` | allow unregistered links | **missing** | |
| `CWID ON/OFF` | Morse ID after session | **missing** (daemon has its own station-ID timer, not host-controllable) | |
| `WINLINK SESSION` / `P2P SESSION` | retry cycle 4.0 s vs 4.6 s | **missing** — Pat sends one of these on every dial | |
| `CQFRAME src [BW]` | send CQ frame | **missing** | |
| `VERSION` | reply `VERSION x.y.z` | **differs** — no reply to the command; instead an unsolicited `VERSION Coppa 0.1.0` greeting on connect (`command.rs:37`), hard-coded string not `CARGO_PKG_VERSION`. Pat's `Version()` blocks forever waiting for a reply prefixed `VERSION ` (no timeout in Pat) | capture 1/2 |
| `CLEANTXBUFFER` | flush TX queue (VARA FM/HF later builds) | **missing** | |
| `TUNE [s]` | not a VARA command | **extra** — coppa-specific two-tone (`event_loop.rs:895-910`) | capture 1 log |
| `KISS ON/OFF` (VARA FM) | switch data port to KISS framing | **missing** | |
| any unknown | `WRONG` | **differs** — silently ignored | capture 2: `FOOBAR` → nothing |

### Modem → host (command port)

| VARA response | When | coppa status | Evidence |
|---|---|---|---|
| `OK` | after every accepted command | **missing** — `VaraResponse::Ok` exists (`protocol.rs:113`) but has zero emit sites in the daemon | `grep VaraResponse::Ok crates/coppa-daemon` → none |
| `WRONG` | bad command | **missing** (same) | |
| `VERSION x` | reply to `VERSION` | **differs** — unsolicited greeting only, see above | |
| `PENDING` / `CANCELPENDING` | inbound connect request detected / abandoned | **missing** — variant `Pending` exists, never emitted; nothing emitted on incoming CONNECT_REQ (`event_loop.rs:2585-2610`) | |
| `CONNECTED src dst BW` | link up | **differs & dropped** — formatted as `CONNECTED <remote>` (two tokens, `event_loop.rs:2626,2649`); Pat's inbound parser `panic`s if `len(parts) < 3`; and the message never reaches the socket (`main.rs:243`) | capture 2 |
| `DISCONNECTED` | link down (either end, timeout) | **dropped** — produced at `event_loop.rs:694,927,941,981,1024,2675`, all discarded by the bridge | capture 2: 30 s wait, nothing |
| `PTT ON` / `PTT OFF` | key/unkey order to host | **impl** (`event_loop.rs:2721`) | capture 1/2 |
| `BUFFER n` | bytes queued in TX buffer, on add and on ACK-removal | **differs** — `n` is *frames in the daemon's tx_queue*, not bytes (`event_loop.rs:541,555`); decremented at transmit start, not on ACK → Pat's `Flush()` (waits `BUFFER 0`) returns before the peer has acked, and `Write()`'s `bufferCount >= 7*len(b)` backpressure never engages | capture 2: `BUFFER 1` → `BUFFER 0` for a 5-byte write |
| `BUSY ON` / `BUSY OFF` | channel occupancy transitions | **impl but twitchy** — no hysteresis/hold in `BusyGate::observe` (`coppa-ml/src/busy_gate.rs:79-106`); on a quiet mic it flapped ~3-4 transitions/s (108 lines in 32 s) | capture 2 |
| `SN n` | per-block S/N, only with `CHAT ON` | **differs** — emitted as `SNR n` (`protocol.rs:129`), wrong keyword, and unconditionally | |
| `IAMALIVE` | every 60 s | **missing** — Pat sets a 2-minute read deadline on the command socket; on a quiet channel with no BUSY chatter Pat will time out the modem | capture 2: 32 s idle, nothing but BUSY |
| `REGISTERED call`, `LINK REGISTERED/UNREGISTERED` | licensing | **missing** — harmless (Pat treats as no-op) | |
| `ENCRYPTION READY/DISABLED` | VARA encryption | **missing** — harmless | |
| `MISSING SOUNDCARD` | audio device died | **missing** — coppa only `eprintln!`s audio failures | `main.rs:120,167` |
| `CQFRAME src BW` | CQ decoded | **missing** | |

### Data port

| Aspect | VARA | coppa | Evidence |
|---|---|---|---|
| Framing | raw byte stream, no framing, only meaningful when `CONNECTED` | raw stream **at all times** — every `read()` chunk becomes one `DataReceived` → one over-the-air frame, transmitted even with no session (`event_loop.rs:819-884`) | capture 2: 5 bytes → `PTT ON` while merely "connecting" |
| Segmentation | modem re-blocks stream into its own frames | one TCP `read` (≤ 4096 B, `data.rs:33`) = one frame; a 20 KB Winlink message arriving in 5 segments becomes 5 uncoalesced frames; a 100-byte write becomes a full frame | |
| Flow control | `BUFFER` bytes; host stops writing when large | `BUFFER` counts frames (see above); mpsc 64-deep with `try_send` → silent drops under load (`main.rs:248`) | |
| RX delivery | decoded payload written to data port | **impl** (`DataOut` broadcast to all data clients, `main.rs:243-252`) | |
| Client pairing | one host = one cmd + one data socket | cmd and data clients are unrelated ID spaces (`server.rs:38-41`); telemetry and RX data broadcast to *every* client; two apps attached simultaneously would both transmit and both receive | |

### Would Pat complete a Winlink session against coppa today?

No. Walkthrough of `pat connect varahf:///K7XYZ` against the code and captures:

1. Pat dials 8300, sends `PUBLIC ON\r`, `CWID ON\r`, `COMPRESSION TEXT\r`, `MYCALL W5AU\r`, `LISTEN OFF\r` (Pat-Vara `start()`). **Breaks here**: none of them end in `\n`, `read_line` at `command.rs:55` never returns; the greeting `VERSION Coppa 0.1.0\r\n` Pat did not ask for is published to no subscriber (harmless).
2. Hypothetically past (1): Pat sends `BW2300\r`, `WINLINK SESSION\r`, calls `waitIfBusy()` — coppa's flapping `BUSY` will let it through at some `BUSY OFF` — then `CONNECT W5AU K7XYZ\r` and subscribes to `CONNECTED`/`DISCONNECTED`. coppa transmits CONNECT_REQ and keys PTT. **Breaks here**: the outcome (`CONNECTED …` / `DISCONNECTED` after the 30 s session timeout) is a `StatusUpdate` that `main.rs:243` drops. Pat's dial blocks until its own context timeout, then sends `DISCONNECT`.
3. Hypothetically past (2): coppa's `CONNECTED K7XYZ` has two tokens; Pat's inbound handler does `parts := strings.Split(cmd, " "); if len(parts) < 3 { panic }` on the listener path, and on the dial path `newConn(url.Target)` succeeds. Pat writes the B2F handshake to 8301; coppa wraps each TCP segment into a MAC data PDU (`event_loop.rs:832-853`, session path) — plausible. Pat then calls `Flush()` and waits for `BUFFER 0`; coppa's `BUFFER` here is only emitted on the *raw* path (`enqueue_tx`), the session path calls `transmit_samples` directly and never emits `BUFFER` at all, so Pat's flush waits on the 60 s timeout.
4. Every response line coppa writes is `\r\n`-terminated; Pat splits on `\r` and keeps `"\nPTT ON"` as a non-empty string → PTT and BUFFER handling silently fails for any line that is not first in its TCP segment.
5. No `IAMALIVE`; Pat's 2-minute read deadline on the command socket eventually fires on a quiet channel.

What breaks first: the `\r` terminator. Second: dropped `StatusUpdate`. Third: `BUFFER` semantics / missing on session path. Fourth: `\r\n` output. Fifth: `IAMALIVE`.

## WebSocket JSON API

Schema (`websocket.rs:8-60`): client `mycall|connect|disconnect|send|status|spectrum`; server `status|data|error|connected|disconnected|spectrum`. Findings:

- **`data`, `connected`, `disconnected` are never emitted.** `grep WsServerMessage::` in the daemon finds only `Spectrum` (`event_loop.rs:1092`) and the on-demand `Status` reply built inside the server. Decoded RX bytes are pushed onto the broadcast channel as **raw lossy-UTF-8 text** (`event_loop.rs:1450-1451`), not as `{"type":"data",...}` — a JSON-parsing dashboard throws on the first decoded frame. Session-path RX (`handle_session_data`, `event_loop.rs:2686-2693`) goes only to VARA `DataOut`, never to WS at all.
- **`mycall` is silently discarded**: the server wraps the *entire JSON text* into `HostEvent::VaraCommand` (`websocket.rs:345`), which the daemon uppercases and compares to `LISTEN ON`/`TUNE` (`event_loop.rs:888-895`). Log shows `command={"type":"mycall","callsign":"W5AU"}`. No ack, no error.
- **No request/response correlation**: no `id` field; `status` is the only request that gets a reply. `connect`/`disconnect`/`send` produce nothing (capture 3). A client cannot tell whether `connect` was accepted, rejected for missing callsign, or is pending.
- **No error events from the daemon**: the daemon's `StatusUpdate("DISCONNECTED")` for "no callsign configured" / invalid callsign (`event_loop.rs:920-945`) never reaches WS; `Error` is only produced for JSON parse failures.
- **Text-only payloads**: `send.data: String` → `into_bytes()` (`websocket.rs:364`) and RX → `from_utf8_lossy`. No base64 or binary WS frames, so any non-UTF-8 payload (B2F, compressed, images) is corrupted. Binary WS frames from the client are ignored (`msg.is_text()` only).
- **No versioning / capability handshake**: no `hello` message, no `protocol_version`, no server version. The only greeting is nothing (capture 3: `[no frames within 0.5s]`).
- **`status` is a polled snapshot, not an event stream**: `WsStatus.connected` only updates on a decoded frame (`event_loop.rs:1241-1246`, with the acknowledged "stays stale until next decode" gap); there is no push on PTT, BUSY, session state, or TX/RX indicators. Dashboard authors need `ptt`, `busy`, `session`, `tx_progress`, `snr`, `cfo`, `level` as *events*.
- **Spectrum frame**: `bins: Vec<f32>` + `timestamp_ms`; no `sample_rate`, `fft_size`, `bin_hz`, `db_floor` metadata, so the client must hardcode coppa internals to label the axis. Works (4 frames/1.2 s captured, ~128 bins).
- **No constellation / ARQ-state / session-log / heard-list events** — nothing a waterfall+status dashboard needs beyond spectrum and a polled status.
- **Backpressure**: broadcast channel depth 64 (`websocket.rs:193`), `Lagged` silently `continue`s (`websocket.rs:326`) — a slow client loses `data` frames with no indication. `is_spectrum_broadcast` re-parses every broadcast JSON per client (`websocket.rs:314`) — O(clients × messages) JSON parsing; a typed broadcast enum would avoid it.
- **No auth / origin check**: any page in a browser on the same machine can `new WebSocket("ws://127.0.0.1:8400")` and key the transmitter (`send` → immediate TX, capture 3 log). At minimum check `Origin`, or require a token from the config file.
- **No schema document**: nothing under `docs/` describes the messages; the only reference is the Rust enum. No JSON Schema, no example session, no `wscat`/`websocat` walkthrough in the tutorial.
- Mixed logging: `println!/eprintln!` (`websocket.rs:257,273,286,293,409`) instead of `tracing`, so WS connection logs bypass `RUST_LOG` filtering (visible in capture 3 log as unprefixed lines).

## KISS

`crates/coppa-host/src/kiss.rs`, only reachable via `coppad --tnc` (`kiss-tnc` feature, AFSK 1200 path, `tnc.rs`).

- **Bug — partial frame emitted as complete**: `kiss_decode`'s inner loop `while i < data.len() && data[i] != FEND` (`kiss.rs:115`) accepts end-of-buffer as a frame terminator. `handle_kiss_client` calls `kiss_decode(&accum)` on every TCP read (`kiss.rs:302`) and then drains through the last FEND (`kiss.rs:314`). A frame split across two reads (any frame > one segment, or just unlucky timing) is (a) emitted truncated on the first read and (b) its tail is discarded on the second (the tail starts with a non-FEND byte, which the outer loop skips). Silent AX.25 corruption on the TX path.
- **Port nibble ignored**: the command byte is matched whole (`kiss.rs:144-172`: `CMD_DATA = 0x00`), so `0x10` (port 1 data), `0x20`… fall through `_ => continue` and are dropped. Multi-port hosts (Direwolf-style `port<<4|cmd`) silently lose frames; spec says mask with `0x0F`.
- **TXDELAY / P / SLOTTIME parsed but unused**: `handle_kiss_client` forwards only `KissFrame::Data` (`kiss.rs:304`); `TxTail (0x04)`, `FullDuplex (0x05)`, `SetHardware (0x06)` aren't parsed at all. The TNC transmits immediately with no persistence/slot CSMA (`tnc.rs:223-230`), no TXDELAY preamble control, and does not honour busy channel — collision-prone on shared packet frequencies.
- **No `Return` handling** (0xFF exits KISS mode) — fine to ignore, but document.
- TNC mode has **no config file / CLI options**: `TncConfig::default()` is used unconditionally (`main.rs:36`), so port 8001, no rigctld, default audio devices — `--tnc` cannot be pointed at a rig without recompiling. `rig_address` / `vox_mode` fields exist but are unreachable.
- `println!` logging, `NullPtt` fallback on rigctld failure (`tnc.rs:77-84`) contradicts the daemon's own "hard error on bad PTT config" policy.
- **AGWPE is missing.** Winlink Express, Outpost, PinPoint, UI-View, Xastir, YAAC, APRSIS32 and most Windows EmComm apps speak AGWPE (port 8000) not KISS. Even a minimal subset — `R` (version), `G` (port info), `g` (port caps), `X`/`x` (register), `k` (raw on), `K` (raw frame both ways), `M` (unproto), `y` (outstanding frames) — with the 36-byte header (port, datakind, pid, callfrom[10], callto[10], datalen u32 LE) would unlock those. No serial/pty KISS either (many Linux apps expect `/dev/ttyXX` or `kissattach`).
- No KISS over the OFDM modem: KISS is AFSK-only; there is no way to run AX.25/`kissattach`/`ax25d` over the HF OFDM waveform, which would be an easy and compelling "IP over coppa" story.

## C FFI

`crates/coppa-ffi/src/lib.rs` (1693 lines), `coppa.h` (429 lines, cbindgen 0.29, checked in).

- **Docs/tutorial ABI mismatch (crash bug for anyone who follows it)**: `docs/tutorials/getting-started.md:127-148` declares `typedef void* CoppaHandle; extern void coppa_engine_destroy(CoppaHandle)` and calls `coppa_engine_destroy(engine)`. The real signature is `void coppa_engine_destroy(struct coppa_engine_t **)` (`coppa.h:129`): the function dereferences the handle as a pointer-to-pointer → frees garbage / segfaults. The tutorial also omits `#include "coppa.h"` and never tells the reader where the header or library live or how to link (`-lcoppa_ffi`, `DYLD_LIBRARY_PATH`).
- **No `extern "C"` guard**: `cbindgen.toml` lacks `cpp_compat = true`; the header has no `#ifdef __cplusplus extern "C" {`. C++ consumers get mangled symbols/link errors. Also no export/visibility macro (`COPPA_API`) for Windows DLL builds.
- **Cosmetic**: `[fn] prefix = ""` leaves a leading space on every declaration (` struct coppa_engine_t *coppa_engine_create(void);`). Drop the key.
- **No error string**: callers get `-3 encode failed` with no reason (e.g. "payload 1500 B exceeds 1024 B for level 3"). Add `const char *coppa_last_error(coppa_engine_t*)` or an `coppa_strerror(int)`.
- **Ambiguous null return**: `coppa_get_decoded` returns null for both "no message" and every error (`coppa.h:401-414`); `coppa_engine_create`/`coppa_engine_new_with` return null for OOM, panic and config-rejection alike with no way to distinguish.
- **Thread safety undocumented**: internally every field is behind a `Mutex`, so a handle is `Send + Sync` and concurrent `feed_samples`/`next_frame` from two threads is safe — but the header never says so, so binding authors will add their own locks or (worse) assume single-threaded and share across audio callback + UI thread without knowing it's fine.
- **ABI versioning**: `coppa_version()` returns the crate semver; there is no `COPPA_ABI_VERSION` macro/function, and the header has no `#define COPPA_VERSION_MAJOR` for compile-time checks. The deprecated v1 stream quartet is "kept for one release" without a `__attribute__((deprecated))`/cbindgen `deprecated` annotation, so C compilers cannot warn.
- **No sample-format/rate accessor**: consumers must know 48 kHz mono f32 from reading Rust docs; add `coppa_engine_sample_rate(h)` and `coppa_engine_max_payload(h)`.
- **Config struct growth**: `coppa_config_t` is a plain struct with no `size`/`version` field, so adding a field later is an ABI break. Common fix: `uint32_t struct_size` as first field, or an opaque builder (`coppa_config_new/set_*/free`).
- **Callsign field discarded** (documented, `coppa.h:65-76`) — a config field that is validated then dropped will surprise every binding author; either remove it from the struct or keep it and add the accessor.
- **No streaming-encode / no session/ARQ at the FFI level**: the FFI exposes raw frame encode/decode only; MAC/session/ARQ (`coppa-protocol`) is daemon-internal, so an app embedding libcoppa cannot get a connected link without re-implementing the daemon. A C-level `coppa_link_*` API (or exposing the daemon as a library with a callback for audio I/O) is the thing Swift/iOS or Android integrators actually need.
- **Nothing packaged**: no CI job builds/publishes the `cdylib`/header (`.github` has no cbindgen/dylib references), no `pkg-config` file, no `include/` install layout, no Python (`ctypes`/`cffi`) wrapper, no Go (`cgo`) or C# (`DllImport`) example — the crate docs promise "C, Python, Swift" but only the Rust side exists. `tests/` has no C-compiled smoke test of the header.
- Good: `catch_unwind` everywhere, poisoned-handle semantics spelled out, frame metadata (SNR/CFO/level/seq) on `coppa_frame_t`, zeroed out-param on "no frame".

## Missing surfaces worth considering

| Surface | Who needs it | Notes |
|---|---|---|
| Plain TCP line "chat"/terminal protocol | keyboard-to-keyboard, EmComm scripts, `nc` users | One port, `\n`-delimited text in, text out with `<call>: ` prefix plus `!status` lines; trivially scriptable; the VARA data port with `CHAT ON` is what VarAC/vARIM use today |
| AGWPE (port 8000) | Windows EmComm apps (see KISS section) | Highest-leverage missing surface for the AFSK/packet side |
| Serial/pty KISS | Linux `kissattach`, Direwolf-style setups | `socat`-able but a `--kiss-pty` flag is friendlier |
| HTTP REST + `/metrics` | monitoring, Grafana, RMS gateway ops | `GET /status`, `GET /health`, `GET /sessions`, Prometheus `coppa_ptt_seconds_total`, `coppa_frames_decoded_total{level}`, `coppa_snr_db` |
| mDNS/DNS-SD advertisement (`_coppa._tcp`, `_vara._tcp`) | LAN dashboards, Pat auto-discovery | tiny, and nice on a Raspberry Pi in a go-box |
| MQTT publisher | home-automation / EmComm net status boards | low priority |
| D-Bus | Linux desktop integration | low priority |
| Python client package (`pip install coppa`) | dashboard/tooling authors, SDR folks | thin WS + VARA client, ~200 lines; ship in `bindings/python` |
| Go client (`github.com/.../go-coppa`) | Pat contributors | Pat already has a VARA transport; if coppa becomes VARA-wire-faithful Pat needs nothing new — that argues strongly for fixing the VARA surface over inventing a Go client |
| ardop-style host protocol (port 8515, `c:`/`d:` length-prefixed) | second-largest installed base after VARA (Pat, Winlink Express, ardopcf users) | bigger job; consider after VARA is solid |

## Hit list

Ranked by (impact × reach) then effort. Category: bug / compat-gap / polish / missing-feature / docs. Impact H/M/L, Effort S/M/L.

| # | Item | Category | Impact | Effort | Evidence |
|---|---|---|---|---|---|
| 1 | VARA command port must accept bare `\r` as line terminator (and `\n`, `\r\n`) — currently `read_line` only returns on `\n`; Pat/VARA send `cmd + "\r"` | bug | H | S | `vara/command.rs:55`; capture 1 log line `command=VERSIONMYCALL W5AU…` |
| 2 | Deliver `StatusUpdate` (CONNECTED/DISCONNECTED/CONNECTING) to the VARA command port — the response bridge only forwards `DataOut` | bug | H | S | `main.rs:243-252`; capture 2 (no CONNECTED/DISCONNECTED ever) |
| 3 | Emit responses with bare `\r` (VARA spec `OK<cr>`); Pat splits on `\r` and would see `"\nBUFFER 0"` | compat-gap | H | S | `vara/protocol.rs:113-129`; Pat-Vara `vara.go` split loop |
| 4 | Reply `OK` to every accepted command and `WRONG` to unknown ones (including `FOOBAR`) | compat-gap | H | S | `VaraResponse::Ok` has no emit site; capture 2 |
| 5 | Implement `MYCALL` (1–5 calls with SSIDs) as the runtime source of the local callsign; `WRONG` on invalid | compat-gap | H | S | `event_loop.rs:886-911`; `protocol.rs:56` |
| 6 | Format `CONNECTED <src> <dst> <bw>` (three+ tokens); include local call and a bandwidth token (e.g. profile name or `2300`) | compat-gap | H | S | `event_loop.rs:2626,2649`; Pat panics on `len(parts) < 3` |
| 7 | Send `IAMALIVE` every 60 s on the command port | compat-gap | H | S | Pat 2-min read deadline; capture 2 (32 s idle → nothing) |
| 8 | Emit `PENDING` on incoming CONNECT_REQ and `CANCELPENDING` if handshake fails/times out | compat-gap | M | S | `event_loop.rs:2585-2610` emits nothing |
| 9 | `BUFFER n` must be **bytes** still unacknowledged (decrement on ACK / on TX for non-ARQ), and must also be emitted on the session (MAC) TX path | bug | H | M | `event_loop.rs:541,555` counts frames; session path `event_loop.rs:832-853` never emits; Pat `Flush()`/`Write()` rely on it |
| 10 | Do not transmit data-port bytes unless a session is established (or an explicit unconnected/FEC mode is selected); buffer or reject otherwise | bug | H | S | `event_loop.rs:819-884`; capture 2 `HELLO` → `PTT ON` while not connected |
| 11 | Coalesce data-port stream into modem-sized blocks instead of "one TCP `read` = one frame"; 4096-byte reads become undersized/oversized frames | bug | H | M | `vara/data.rs:33-45` |
| 12 | Pair one command client with one data client (or make the second command connection read-only); today all clients share TX/RX/telemetry | bug | M | M | `server.rs:38-41`, `main.rs:243-252` broadcast |
| 13 | Accept and act on `ABORT` (immediate teardown, purge `tx_queue`, `DISCONNECTED`) | compat-gap | M | S | parsed `protocol.rs:88`, unhandled |
| 14 | `DISCONNECT` should wait for `tx_queue` (and ARQ) to drain before sending the DISCONNECT PDU | compat-gap | M | S | `event_loop.rs:987-1030` |
| 15 | Accept `COMPRESSION OFF/TEXT/FILES`, `BW500/2300/2750`, `CHAT ON/OFF`, `PUBLIC ON/OFF`, `CWID ON/OFF`, `WINLINK SESSION`, `P2P SESSION`, `CLEANTXBUFFER` with `OK` (map BW→profile/speed cap where meaningful, CWID→station-ID toggle, CLEANTXBUFFER→purge queue) | compat-gap | M | M | `protocol.rs:96` → `Unknown`; VARA PDF |
| 16 | Rename `SNR n` → `SN n` and gate on `CHAT ON` (or keep `SNR` as a documented coppa extension in addition) | compat-gap | L | S | `protocol.rs:129` |
| 17 | Reply to `VERSION` (with `CARGO_PKG_VERSION`), drop or keep the unsolicited greeting but never hard-code `Coppa 0.1.0` | polish | M | S | `command.rs:37` |
| 18 | Add hysteresis/hold-time to `BusyGate` (e.g. N consecutive blocks or ≥ 250 ms) — flapped 3–4×/s on a quiet input, floods every command client and defeats Pat's `waitIfBusy` | bug | M | S | `busy_gate.rs:79-106`; capture 2 (108 BUSY lines / 32 s) |
| 19 | Telemetry send path uses `try_send` on a 64-deep channel — a slow host silently loses `PTT OFF`/`DISCONNECTED`; use bounded `send` with timeout or drop only BUSY/SNR, never state transitions | bug | M | S | `event_loop.rs:527-533`, `server.rs:143` |
| 20 | Write a `docs/HOST-API.md` (or `wiki/pages/host-interfaces.md`): VARA command/response table with coppa deviations, data-port semantics, WS message schema, KISS notes, ports, an `nc` walkthrough | docs | H | M | none exists; `ARCHITECTURE.md:82-84` is two lines |
| 21 | Add an end-to-end VARA conformance test that drives a real `VaraServer` + `EventLoop` with Pat's exact `\r` command sequence and asserts `OK`, `CONNECTED src dst bw`, `DISCONNECTED` on the socket | polish | H | M | existing tests only cover telemetry (`event_loop.rs:6225+`) and CRLF |
| 22 | Try Pat for real (Pat ≥ 0.13 has `varahf` built in) in CI or a documented manual test: two coppad instances + audio loopback + `pat connect varahf:///CALL` | polish | H | M | — |
| 23 | WS: emit `data` as JSON (`{"type":"data","data":…}`), not raw text; emit `connected`/`disconnected`/`error` from daemon `StatusUpdate`s; also forward session-path RX to WS | bug | H | S | `event_loop.rs:1450-1451`, `2686-2693`; `WsServerMessage::Data/Connected/Disconnected` never constructed |
| 24 | WS: implement `mycall` (it is currently stuffed into `VaraCommand` as raw JSON and ignored) | bug | M | S | `websocket.rs:345`; capture 3 log |
| 25 | WS: binary payloads — `send.data` accept `{"encoding":"base64"}` or binary WS frames; `data` events carry base64/binary; never `from_utf8_lossy` | bug | H | S | `websocket.rs:364`, `event_loop.rs:1450` |
| 26 | WS: request ids + typed acks (`{"type":"ack","id":…}` / `{"type":"error","id":…,"code":…}`) for every client message; error `code` enum not just free text | polish | M | M | capture 3: `connect`/`send`/`disconnect` yield nothing |
| 27 | WS: `hello` on connect with `protocol_version`, server version, callsign, profile, sample rate; version the schema | polish | M | S | capture 3: empty greeting |
| 28 | WS: push events for `ptt`, `busy`, `session` (state machine), `tx_queue`, `snr/cfo/level` per frame, `arq` (window/retries), `heard` list; make `status` a full snapshot of the same fields | missing-feature | H | M | `WsStatus` is polled only, `event_loop.rs:1241-1246` |
| 29 | WS: spectrum frame metadata (`fft_size`, `sample_rate`, `bin_hz`, `first_bin_hz`, dB reference) and optional constellation (`iq: [[i,q],…]`) message | polish | M | S | `websocket.rs` `Spectrum { bins, timestamp_ms }` |
| 30 | WS: `Origin` check and/or shared-secret token (config `[host] websocket_token`), since any local web page can key the TX | bug | H | S | no auth anywhere; `config.rs:173` warning only |
| 31 | WS: publish a JSON Schema (or TypeScript `.d.ts`) generated from the serde enums (`schemars`), and a `wscat` example in the tutorial | docs | M | S | none |
| 32 | WS: replace per-client `is_spectrum_broadcast` JSON re-parse with a typed broadcast enum; log via `tracing` not `println!` | polish | L | S | `websocket.rs:314,257-409` |
| 33 | KISS: fix partial-frame emission — keep bytes after the last FEND *and* only emit frames that are FEND-terminated | bug | H | S | `kiss.rs:115,302-316` |
| 34 | KISS: mask port nibble (`cmd & 0x0F`, `port = cmd >> 4`); parse TXTAIL/FULLDUP/SETHW; honour TXDELAY/P/SLOTTIME with p-persistence CSMA using the busy gate | bug | M | M | `kiss.rs:144-172`, `tnc.rs:223-230` |
| 35 | KISS/TNC: expose `TncConfig` via `coppad.toml [tnc]` / CLI flags (port, bind, rigctld, VOX, audio devices); hard-error on PTT misconfig like the main daemon | polish | M | S | `main.rs:36` uses `TncConfig::default()` |
| 36 | Add AGWPE server (port 8000): `R G g X x k K M y` subset first | missing-feature | H | M | none; needed by Winlink Express/Outpost/UI-View |
| 37 | Allow KISS/AX.25 over the OFDM modem (not only AFSK), enabling `kissattach`/IP-over-coppa | missing-feature | M | M | `tnc.rs` is AFSK-only |
| 38 | FFI: fix tutorial C snippet (`coppa_engine_destroy(&engine)`, include header, link line) | docs | H | S | `getting-started.md:127-148` vs `coppa.h:129` |
| 39 | FFI: `cpp_compat = true` in cbindgen (`extern "C"` guard), `COPPA_API` export macro, remove `[fn] prefix = ""` leading-space artefact, add `deprecated` attrs on v1 stream quartet | polish | M | S | `cbindgen.toml`, `coppa.h:116-427` |
| 40 | FFI: `coppa_last_error()`/`coppa_strerror()`; distinguish "no message" from error in `coppa_get_decoded`/`create` | polish | M | S | `coppa.h:401-414` |
| 41 | FFI: document thread-safety (handle is internally locked; safe to share) in header + `coppa_engine_sample_rate()`/`max_payload()` accessors | docs | M | S | no mention in `coppa.h` |
| 42 | FFI: `coppa_config_t` needs a `struct_size`/version field or opaque builder before v2 ships as stable ABI; drop or wire the discarded `callsign` | polish | M | S | `coppa.h:78-83` |
| 43 | FFI: CI job that compiles a C smoke test against `coppa.h`, builds the cdylib for macOS/Linux/Windows, uploads header+lib as release artefacts; add `coppa.pc` | polish | H | M | no `.github` references to cbindgen/cdylib; no `*.c` in repo |
| 44 | FFI: link-level API (`coppa_link_connect/listen/send/recv/poll` with callbacks) so embedders get session/ARQ without running the daemon | missing-feature | H | L | `coppa-protocol` session/MAC is daemon-only |
| 45 | Ship `bindings/python` (ctypes wrapper + WS/VARA client) and a Go `cgo` example; publish to PyPI | missing-feature | M | M | none |
| 46 | REST/`/metrics` endpoint (`/status`, `/health`, Prometheus) and mDNS advertisement | missing-feature | M | M | none |
| 47 | Plain TCP chat line protocol (or make VARA `CHAT ON` + data port behave like VarAC expects: `SN` per block, CQFRAME) | missing-feature | M | M | none |
| 48 | Out-of-scope escalation: every TX drops ~87% of samples (`Audio output buffer overflow dropped=57328 total=65520`) with default `buffer_size = 8192` — host-API work cannot be validated over the air until this is fixed | bug | H | ? | captures 1-3 daemon log; `main.rs` audio_out ring = `config.audio.buffer_size` |

## Verbatim captures

Config used (`coppad-review.toml`): `ptt_method = "none"`, `bind_address = "127.0.0.1"`, `vara_command_port = 18300`, `vara_data_port = 18301`, `websocket_port = 18400`, `callsign = "W5AU"`, `arq_enabled = false`, `profile = "HF_STANDARD"`. Daemon: `RUST_LOG=debug target/release/coppad ./coppad-review.toml`. `>` = sent by probe, `<` = received.

### Capture 1 — Pat's real command sequence with bare `\r` terminators (`vara_probe.py`)

```
# connected to command port ('127.0.0.1', 18300)
< b'VERSION Coppa 0.1.0'
< b'BUSY ON'
< b'BUSY OFF'
< b'BUSY ON'
< b'BUSY OFF'
  ... (BUSY flapping continues throughout, 4-8 transitions per 0.7 s window)
# connected to data port ('127.0.0.1', 18301)
> b'VERSION\r'
> b'MYCALL W5AU\r'
> b'COMPRESSION OFF\r'
> b'BW2300\r'
> b'CHAT OFF\r'
> b'PUBLIC OFF\r'
> b'CWID OFF\r'
< [no data within 0.7s]
> b'WINLINK SESSION\r'
> b'LISTEN OFF\r'
> b'LISTEN ON\r'
> b'CONNECT W5AU K7XYZ\r'
# push 5 bytes on data port while 'connecting'
< b'BUFFER 1'
< b'BUFFER 0'
< b'PTT ON'
<data [no data within 0.5s]
> b'ABORT\r'
< b'PTT OFF'
> b'DISCONNECT\r'
> b'CLEANTXBUFFER\r'
> b'FOOBAR\r'
> b'LISTEN OFF\r'
# TUNE 0.2
  (only BUSY lines; TUNE never executed)
# done
```

Daemon log for the same session (ANSI stripped, spectrum lines removed):

```
INFO  coppad::event_loop: Client connected client_id=1
DEBUG coppad::event_loop: Data received from client client_id=2147483649 bytes=5
INFO  coppad::event_loop: PTT state change state="TX"
WARN  coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=57328
INFO  coppad::event_loop: PTT state change state="RX"
DEBUG coppad::event_loop: VARA command received client_id=1 command=VERSIONMYCALL W5AUCOMPRESSION OFFBW2300CHAT OFFPUBLIC OFFCWID OFFWINLINK SESSIONLISTEN OFFLISTEN ONCONNECT W5AU K7XYZABORTDISCONNECTCLEANTXBUFFERFOOBARLISTEN OFFTUNE 0.2
INFO  coppad::event_loop: Client disconnected client_id=1
```

(Every `\r`-terminated command sat in `read_line` until the socket closed, then arrived as a single line. Only the data-port write — which needs no terminator — did anything, and it keyed PTT with no session.)

### Capture 2 — same sequence with `\r\n` terminators (`vara_probe_crlf.py`; BUSY lines counted, not shown)

```
# connected to command port ('127.0.0.1', 18300)
< b'VERSION Coppa 0.1.0'
< [... 2 BUSY ON/OFF lines elided ...]
# connected to data port ('127.0.0.1', 18301)
> b'VERSION\r\n'
< [... 2 BUSY ON/OFF lines elided ...]          <- no VERSION reply
> b'MYCALL W5AU\r\n'
< [... 4 BUSY ON/OFF lines elided ...]          <- no OK
> b'COMPRESSION OFF\r\n'
< [... 4 BUSY ON/OFF lines elided ...]
> b'BW2300\r\n'
< [no data within 0.7s]
> b'CHAT OFF\r\n'
> b'PUBLIC OFF\r\n'
> b'CWID OFF\r\n'
> b'WINLINK SESSION\r\n'
< [no data within 0.7s]
> b'LISTEN OFF\r\n'
> b'LISTEN ON\r\n'
< [no data within 0.7s]                          <- no OK
> b'CONNECT W5AU K7XYZ\r\n'
< b'PTT ON'                                      <- no OK, no PENDING
# push 5 bytes on data port while 'connecting'
< b'BUFFER 1'
< b'PTT OFF'
< b'BUFFER 0'
< b'PTT ON'                                      <- raw bytes transmitted while unconnected
< [... 8 BUSY ON/OFF lines elided ...]
<data [no data within 0.5s]
> b'ABORT\r\n'
< b'PTT OFF'                                     <- coincidental end of TX, ABORT itself ignored
> b'DISCONNECT\r\n'
< b'PTT ON'                                      <- DISCONNECT PDU transmitted, no DISCONNECTED
> b'CLEANTXBUFFER\r\n'
< b'PTT OFF'
> b'FOOBAR\r\n'
< [... 6 BUSY ON/OFF lines elided ...]          <- no WRONG
> b'LISTEN OFF\r\n'
# wait for the 30 s session connect timeout to see what the host is told
< [... 108 BUSY ON/OFF lines elided ...]        <- no DISCONNECTED, no IAMALIVE in 32 s
# done
```

Daemon log excerpt:

```
DEBUG coppad::event_loop: VARA command received client_id=2 command=LISTEN ON
INFO  coppad::event_loop: Listening for incoming connections
INFO  coppad::event_loop: Connect request client_id=2 destination=K7XYZ
INFO  coppad::event_loop: PTT state change state="TX"
WARN  coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=401296
DEBUG coppad::event_loop: Data received from client client_id=2147483650 bytes=5
INFO  coppad::event_loop: PTT state change state="RX"
INFO  coppad::event_loop: PTT state change state="TX"
DEBUG coppad::event_loop: VARA command received client_id=2 command=ABORT
INFO  coppad::event_loop: Disconnect request client_id=2
INFO  coppad::event_loop: PTT state change state="TX"
DEBUG coppad::event_loop: VARA command received client_id=2 command=CLEANTXBUFFER
DEBUG coppad::event_loop: VARA command received client_id=2 command=FOOBAR
DEBUG coppad::event_loop: VARA command received client_id=2 command=LISTEN OFF
INFO  coppad::event_loop: Stopped listening for incoming connections
```

### Capture 3 — WebSocket JSON API (`ws_probe.py`, hand-rolled RFC 6455 client; spectrum frames counted, not shown)

```
# handshake: HTTP/1.1 101 Switching Protocols
< [no frames within 0.5s]                                       <- no hello/version
> {"type":"status"}
< (opcode 1) {"type":"status","connected":false,"cp_desync_episodes":0}
> {"type":"mycall","callsign":"W5AU"}
< [no frames within 0.7s]                                       <- silently ignored
> {"type":"bogus"}
< (opcode 1) {"type":"error","message":"Invalid message: unknown variant `bogus`, expected one of `mycall`, `connect`, `disconnect`, `send`, `status`, `spectrum` at line 1 column 15"}
> not json
< (opcode 1) {"type":"error","message":"Invalid message: expected ident at line 1 column 2"}
> {"type":"connect","source":"W5AU","destination":"K7XYZ"}
< [no frames within 1.5s]                                       <- CONNECT_REQ transmitted, no ack/event
> {"type":"send","data":"hello from ws"}
< [no frames within 2.0s]                                       <- transmitted raw while unconnected, no ack
> {"type":"spectrum","enabled":true}
# spectrum frames received in 1.2s: 4                            <- {"type":"spectrum","bins":[...128 f32...],"timestamp_ms":...}
> {"type":"spectrum","enabled":false}
> {"type":"disconnect"}
< [no frames within 0.7s]                                       <- DISCONNECT PDU transmitted, no event
> {"type":"status"}
< (opcode 1) {"type":"status","connected":false,"cp_desync_episodes":0}
# done
```

Daemon log excerpt:

```
WebSocket client 1 connected from 127.0.0.1:53787               <- println!, not tracing
DEBUG tungstenite::handshake::server: Server handshake done.
INFO  coppad::event_loop: Client connected client_id=1
DEBUG coppad::event_loop: VARA command received client_id=1 command={"type":"mycall","callsign":"W5AU"}
INFO  coppad::event_loop: Connect request client_id=1 destination=K7XYZ
INFO  coppad::event_loop: PTT state change state="TX"
WARN  coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=57328
DEBUG coppad::event_loop: Data received from client client_id=1 bytes=13
INFO  coppad::event_loop: PTT state change state="RX"
INFO  coppad::event_loop: PTT state change state="TX"
WARN  coppad::event_loop: Audio output buffer overflow dropped=57328 total=65520 cumulative_dropped=114656
INFO  coppad::event_loop: PTT state change state="RX"
WARN  coppad::event_loop: Session timed out session_id=0         <- never reported to the WS client
```

### Reference excerpts used for the matrix

EA5HVK "VARA Protocol Native TNC Commands" (Feb 13 2022), verbatim lines: `CONNECT Source Destination<cr>`, `LISTEN ON<cr> … This command will cause a disconnection if it is received in the middle of a VARA connection`, `MYCALL Call1 Call2 Call3 Call4 Call5<cr>`, `DISCONNECT<cr> Disconnect the link, once the TX buffer is empty.`, `ABORT<cr> Disconnect the link inmediately. (dirty disconnect)`, `COMPRESSION OFF|TEXT|FILES<cr>`, `BW500|BW2300|BW2750<cr>`, `CHAT ON|OFF<cr>`, `CQFRAME Source BW<cr>`, `WINLINK SESSION<cr> (By default)`, `P2P SESSION<cr>`; modem→host: `CONNECTED Source Destination BW<cr>`, `DISCONNECTED<cr>`, `PTT OFF|ON<cr>`, `BUFFER Bytes<cr> Reports number of bytes in transmit buffer queue. Sent when VARA adds data to queue or VARA removes acked bytes from queue`, `PENDING<cr>`, `CANCELPENDING<cr>`, `BUSY OFF|ON<cr>`, `REGISTERED Call<cr>`, `LINK REGISTERED|UNREGISTERED<cr>`, `IAMALIVE<cr> Sent every 60 seconds`, `MISSING SOUNDCARD<cr>`, `CQFRAME Source BW<cr>`, `SN value<cr> … only if the CHAT ON command is active`, `OK<cr> Response to a received command`, `WRONG<cr> Wrong command`.

Pat-Vara `vara.go` (Pat's VARA transport): `m.cmdConn.Write([]byte(cmd + "\r"))`; reader `m.cmdConn.SetReadDeadline(time.Now().Add(2 * time.Minute)); … cmds := strings.Split(string(buf[:l]), "\r"); for _, c := range cmds { if c == "" { continue }; m.handleCmd(c); m.cmds.Publish(c) }`; `start()` sends `PUBLIC ON`, `CWID ON` (varahf), `COMPRESSION TEXT`, `MYCALL %s`, `LISTEN OFF` and does **not** wait for `OK`; `Version()` subscribes to prefixes `VERSION`/`WRONG` and blocks with no timeout; dial sets `BW<bw>`, `WINLINK SESSION`|`P2P SESSION`, `waitIfBusy()`, subscribes `CONNECTED`/`DISCONNECTED`, sends `CONNECT <my> <target>`; inbound `parts := strings.Split(cmd, " "); if len(parts) < 3 { panic(...) }`; `conn.go` `Write` blocks while `bufferCount >= 7*len(b)`, `Flush` waits for `BUFFER 0` (60 s timeout), `Close` sends `DISCONNECT` and waits ≤ 60 s for `DISCONNECTED` then `Abort()`; pubsub matching is `strings.HasPrefix`.

ardopcf host interface (for contrast): single TCP port 8515, commands `ABORT ARQBW ARQCALL ARQTIMEOUT AUTOBREAK BREAK BUFFER BUSYBLOCK BUSYDET CAPTURE CLOSE CWID DATATOSEND DISCONNECT DRIVELEVEL FECMODE FECSEND GRIDSQUARE INITIALIZE LISTEN MONITOR MYAUX MYCALL PING PROTOCOLMODE PURGEBUFFER RADIOPTTON/OFF SENDID SQUELCH STATE TUNINGRANGE TWOTONETEST TXLEVEL VERSION …`.

AGWPE header (36 bytes): port u8 @0, reserved[3], DataKind u8 @4, reserved, PID u8 @6, reserved, CallFrom[10] @8, CallTo[10] @18, DataLen u32 LE @28, User u32 @32; DataKinds `P R G g X x y Y H m C d D c v M V K k I S U T`.

Probe scripts and raw transcripts: `vara_probe.py`, `vara_probe_crlf.py`, `ws_probe.py`, `vara_transcript.txt`, `vara_transcript_crlf.txt`, `ws_transcript.txt`, `coppad-review.log`, `coppad-review2.log`, `vara_tnc_commands.txt` (PDF text) — all in this scratchpad directory.
