# Coppa product, UX, and market review — 2026-09-05

**Scope.** A broad and deep review of coppa as a *product*: CLI and daemon UX,
host-API integration surfaces, documentation and onboarding, positioning and
market, modem fitness for purpose, Rust-library ergonomics, and the product
surfaces coppa does not yet have. Reviewed at `main` = `1c2ffc5`.

**Method.** Eight independent reviewers, one per dimension, each working from
the source, the release binaries (`--features cpal-backend,websocket`), live
daemon probes on loopback ports, fresh bench sweeps, and primary external
sources. Every reviewer's full report, with verbatim captures and `file:line`
evidence, is in this directory (index at the end). This document is the
synthesis: the cross-cutting verdict, a single deduplicated hit list with
stable IDs (`H-nn`), a sequenced roadmap, and the decisions only the owner can
make. Where a dimension report is cited, `C#9` means item 9 of the dimension-C
hit list.

**Directly re-verified by the synthesizer** (not taken on trust from a
reviewer): `cargo build --workspace --all-features` fails with `E0560` at
`coppa-cli/src/main.rs:961`; `coppa rx -i cq.wav` prints lowercase hex;
`coppad` takes `argv[1]` as the config path with no option parsing
(`coppa-daemon/src/main.rs:47-49`); `handle_audio_out` writes a whole frame
into a non-blocking ring and only logs the overflow
(`event_loop.rs:1483-1502`); `select_ofdm_profile` returns `vhf_wide()` for
every speed level `>= 5` (`coppa-engine/src/engine.rs:150-159`); the daemon's
VARA command handler matches only `LISTEN ON/OFF` and `TUNE`
(`event_loop.rs:886-911`); and the `CI` workflow on `main` has been red on the
`Security Audit` job for the last three runs (`2e94a49`, `a2de8cf`,
`1c2ffc5`).

---

## 1. Verdict

Coppa is a strong engineering codebase wearing a broken product. The PHY and
protocol work (NR BG2 LDPC with IR-HARQ, soft-ML Golay header, delay-domain
channel estimation, a normative wire-format spec with 20 CI-checked golden
WAVs, 3 kHz-referenced SNR with Wilson intervals, ADRs that record every
missed bar) is better than anything else in the open HF-modem field. The
measurement honesty in particular is a genuine differentiator, because the
nearest live competitor (Mercury v2) claims VARA parity with no published
numbers at all.

But nothing an end user touches is finished, and several things are broken in
ways that make the headline claims false today:

- **Neither the daemon nor the CLI can transmit a frame through a sound
  card.** Every TX writes a 65,520-sample frame into an 8,192-sample
  ring and drops the remaining 87 %. Three reviewers observed the
  daemon case independently (`dropped=57328 total=65520`,
  `dropped=39808 total=48000` for `TUNE 1`); the CLI's live `coppa tx`/
  `coppa tune` build the identical fixed-size ring and silently discard
  the same fraction. PTT is held for the full frame duration while only
  170 ms of audio plays.
- **The VARA-style TCP interface, which the README calls "Working", cannot
  process a single command from a real VARA client.** The command port
  requires `\n`; VARA and Pat send bare `\r`. Pat's entire startup sequence
  arrived as one 150-character line when the socket closed. Behind that: no
  `OK`/`WRONG`, `CONNECTED`/`DISCONNECTED` are generated and then dropped in
  `main.rs`, `MYCALL` is ignored, `BUFFER` counts frames not bytes, no
  `IAMALIVE`, and data-port bytes key the transmitter with no session.
- **On a real SSB rig the speed ladder tops out at level 4.** The engine
  routes levels 5–10 to the 350–5900 Hz `vhf_wide` profile, which decodes zero
  frames through a 300–2700 Hz SSB filter at any SNR and is illegal on US HF
  under the 2.8 kHz rule. Forcing `hf_standard` recovers levels 5–9, but level
  10 (64-QAM 5/6) then fails to clear FER <= 10% at any SNR through the SSB
  filter (90% FER at 30 dB, 100% at 40-60 dB, all LDPC non-convergence; clean
  unfiltered), a failure nobody had characterised because the SSB sweep only
  ever ran with VHF routing.
- **The modem has never been on the air**, and the simulated ARQ session
  bench survives the link on 0/5 CCIR-Moderate and 0/5 CCIR-Poor sessions
  (5/5 drop in both).
  Link drops under fading are exactly what the 2020 Winlink IONOS study used
  to separate ARDOP-class from VARA/PACTOR-class modems.
- **The README describes a different, older program** (BPSK + Costas loop +
  Viterbi, "OFDM partial", "QPSK not wired", "PTT stub", "EWMA predictor"),
  `coppa rx` prints hex where the tutorial promises text, and `coppad --help`
  starts the daemon. A ham lands on the repo, reads "Partial / Stub", tries
  the tutorial, sees hex, and leaves.
- **There is no go-to-market surface at all**: zero tags, zero releases, zero
  binaries, zero crates.io publications, no repo topics or homepage, no
  Discussions, no CHANGELOG, one star.

None of the P0 fixes is large. The daemon TX ring, the VARA line discipline,
the profile routing, the `coppad` CLI, the `rx` output, and the README are
each a day or less. What is large is the robustness work that decides whether
coppa can compete on HF (fade-diversity interleaving, a weak-signal mode, and
an ARQ policy that does not give up in a fade trough), and the evidence work
(an audio-cable two-host test, then real radios) that decides whether anyone
should believe the numbers.

### Where coppa can win

The market analysis (see `2026-09-05-competitive-landscape.md`) is
unambiguous about the opening. VARA is closed, Windows-only, and painful on
Raspberry Pi. ARDOP is slow, and ardopcf, the best open Linux/RPi
implementation, was discontinued by its author. FreeDATA's own README says
development has slowed. Mercury v2 is funded, packaged, and covered by the
press as "the VARA replacement", but its goodput ceiling is roughly 1.1 kbps
under ARQ, it has no GUI, no CWID, and no published head-to-head numbers.

Three wedges are realistic, in this order:

1. **The open modem for Linux/RPi Winlink gateways and Pat users.** Requires
   the VARA surface to be wire-faithful enough that `pat-vara` and LinBPQ
   drive coppa unmodified, static binaries, a systemd unit, and one shadow
   gateway posting connect logs. The chicken-and-egg with Winlink is solved
   the way VARA solved it: BPQ32 first, data second, WDT third.
2. **The only open HF modem with credible public benchmarks.** Coppa already
   has the harness. Publishing IONOS-format bytes/min curves with confidence
   intervals, including where coppa loses, makes it the reference everyone
   else is measured against.
3. **The Rust DSP/modem library.** No competitor is a library. Five crates
   build for `wasm32` unchanged; a browser demo is one `wasm-bindgen` shim
   away and would be the single best marketing asset the project could have.

## 2. Scorecard

| Dim | Dimension | Grade | One-line verdict | Report |
|---|---|---|---|---|
| A | `coppa` CLI UX | C- | Works as a dev harness; hex output, silent exit-0 failures, no config/env, two overlapping receivers, no `--json`/completions | `dim-A-cli-ux` |
| B | `coppad` operations | D+ (operator) / B- (substrate) | TX truncation bug, no CLI parsing, failures don't fail, no packaging, no CM108 PTT, TNC mode doesn't compile | `dim-B-daemon-ops` |
| C | Host APIs (VARA/WS/KISS/FFI) | D+ / D+ / C / B- | VARA port unusable by Pat; WS half-wired and non-JSON on RX; KISS framing bug; FFI good but tutorial snippet is UB | `dim-C-host-apis` |
| D | Docs, onboarding, positioning | C- | README and ARCHITECTURE materially false; first-5-minutes path broken; zero go-to-market surface | `dim-D-docs-positioning` |
| E | Competitive landscape | n/a | Real opening (ardopcf dead, VARA closed, Mercury numberless); blocked on binaries, OTA, Pat interop | `competitive-landscape` |
| F | Modem fitness | Research-grade, not on-air ready | AWGN mid-range competitive; fading and weak-signal far behind; L5+ not an SSB waveform; never on the air | `dim-F-modem-fitness` |
| G | Rust library | C+ (library) / B+ (engineering) | `--all-features` broken, `anyhow` in public APIs, reachable panics, zero publish metadata; wasm builds | `dim-G-rust-library` |
| H | Missing surfaces | n/a | Build: daemon-served web dashboard, chat tab, WASM demo. Defer: mobile app, Pi image | `dim-H-missing-surfaces` |

## 3. Master hit list

Priorities: **P0** = false claim, safety, or blocks any real use; **P1** =
required for the first credible release; **P2** = required to be world-class;
**P3** = polish. Effort: S < 1 day, M < 1 week, L > 1 week. `Src` points at
the dimension report item(s) carrying the evidence. Items are deduplicated
across the eight reports (they total roughly 380 raw items).

### 3.1 Correctness and safety blockers

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-001 | P0 | TX truncation, both daemon and CLI: `handle_audio_out`/`CpalSink::write` are drop-on-full into an 8,192-sample ring; a 65,520-sample frame loses 87 %. This is not daemon-only -- `coppa-cli/src/main.rs:377-408`'s live `coppa tx`/`coppa tune` build the same fixed-8,192-sample `CpalSink`, call `sink.write(samples)` once, and discard the returned partial-write count via `?`, so live CLI transmission also silently sends only the first 8,192 samples of a normal frame while reporting success. Chunk/pace the write at the sample rate (or size the ring >= max frame + guard) in both the daemon and the CLI, and derive PTT release from samples actually delivered. | M | B#1, H#9, C#48; CLI path new finding, `coppa-cli/src/main.rs:377-408` |
| H-002 | P0 | Stop routing speed levels ≥ 5 to `vhf_wide` on HF. `Profile.ofdm_profile` is read at initial construction (`CoppaCore::from_profile`) but ignored by `select_ofdm_profile` on a later `set_speed_level` reconfiguration -- fix that reconfiguration path to honour it (not delete the field, which is active) so every level has an HF-legal, SSB-passband profile. Levels 5–9 on `hf_standard` decode at 9/9/15/18 dB. | S | F#2, E#13, D#55 |
| H-003 | P0 | Level 10 (64-QAM 5/6) on `hf_standard` fails to clear FER <= 10% at any SNR through a 300–2700 Hz SSB filter to 60 dB (90-100% FER, LDPC non-convergence). Drop it from the HF ladder, narrow the top edge to ≤ 2600 Hz, or add edge-carrier erasure. Never characterised before this review. | S | F#2b; `results/review-2026-09-05/` |
| H-004 | P0 | VARA command port must accept bare `\r` (and `\n`, `\r\n`) as terminator; today `read_line` blocks until `\n`. | S | C#1 |
| H-005 | P0 | Forward `HostResponse::StatusUpdate` (CONNECTED/DISCONNECTED/PENDING) to the VARA command port and WebSocket; `main.rs:243` bridges only `DataOut`, so no host learns of a session transition as it happens. A WebSocket client CAN poll `{"type":"status"}` and get `WsStatus.connected` (recomputed from established sessions on decoded frames, `event_loop.rs:1241-1246`), but that value is stale until the next decode -- push delivery is what's missing, not all visibility. | S | C#2, H#1, H#2 |
| H-006 | P0 | Emit VARA responses with bare `\r`; Pat splits on `\r` and sees `"\nBUFFER 0"`. | S | C#3 |
| H-007 | P0 | Reply `OK` to every accepted command and `WRONG` to unknown ones; implement `MYCALL` as the runtime callsign source; send `IAMALIVE` every 60 s; format `CONNECTED <src> <dst> <bw>` (Pat panics on < 3 tokens). | S | C#4–7 |
| H-008 | P0 | Data-port bytes are transmitted immediately with no session (keys PTT for a host that merely pre-writes). Buffer or reject until `CONNECTED`, or require an explicit unconnected mode. | S | C#10 |
| H-009 | P0 | `BUFFER n` must be unacknowledged *bytes*, decremented on ACK, and must be emitted on the session TX path (today: frames, decremented at TX start, absent on session path; Pat's `Flush()` and backpressure both depend on it). | M | C#9 |
| H-010 | P0 | `coppad` has no argument parser: `--help`/`--version` are taken as a config path and start the daemon on the default mic/speaker. Add clap: `--config`, `--check`, `--log-level`, `--print-default-config`, `--tnc`. | S | B#3, A#47, D#3 |
| H-011 | P0 | `coppa rx` prints lowercase hex where README and tutorial promise text. Print UTF-8 when valid (escaped otherwise), keep `--hex`/`--raw` for bytes. | S | A#1, D#2 |
| H-012 | P0 | `cargo build --workspace --all-features` fails (`TncConfig` has no field `audio_device`); `coppa tnc` does not compile; CI never builds `kiss-tnc`. Fix and add `--all-features` to CI. | S | G#1, B#13 |
| H-013 | P0 | CI on `main` is red: `Security Audit` fails with "Resource not accessible by integration" (needs `checks: write`). | S | F#13 |
| H-014 | P0 | Daemon failures don't fail: host-port bind conflict, audio-stream open failure, unsupported sample rate, unreachable rigctld, and missing named audio device (silently falls back to the *system default*, i.e. the laptop mic) all end in `INFO Daemon ready`, exit 0. Make each a startup error, or at minimum ERROR-level plus a status flag. | S | B#4–8, B#17 |
| H-015 | P0 | PTT is never released on shutdown or fatal error (no `Drop` on `SerialPtt`/`RigctldClient`); a SIGTERM mid-TX leaves the rig keyed. `max_tx_duration_s` only caps the release timer. Add explicit unkey on every exit path and a PTT watchdog. | S | B#15, B#16, F#35 |
| H-016 | P0 | README Status/Features tables describe a BPSK/Costas/Viterbi modem with "OFDM partial", "QPSK not wired", "PTT stub", "EWMA predictor", "LDPC 6 rates". About 35 contradicted statements are tabulated in dimension D. Rewrite. | M | D#1, D "Stale statements" table, E#12, F#12, G#46 |
| H-017 | P0 | ARCHITECTURE.md TX/RX pipeline, dependency versions, LOC/test counts, and the entire "Not implemented" list are wrong (all four "not implemented" items are implemented). Rewrite from SPEC §1–10. | M | D#6 |
| H-018 | P0 | KISS: an unterminated frame straddling a TCP read boundary is emitted truncated and its tail discarded (silent AX.25 corruption). | S | C#33 |
| H-019 | P0 | `CoppaTransceiver::transmit` panics via `.expect("invalid speed level in header")` on a caller-constructible header with reserved level 8; return `TransmitError`. | S | G#2 |
| H-020 | P1 | `BusyGate` has no hysteresis: flaps ON/OFF 3–4×/s on a quiet input (108 lines in 32 s), floods every command client, defeats Pat's `waitIfBusy`, and would strobe any BUSY lamp. Add hold time / consecutive-block requirement. | S | C#18, H#10 |
| H-021 | P1 | Telemetry `try_send` on a 64-deep channel silently drops `PTT OFF`/`DISCONNECTED` for a slow host; never drop state transitions. | S | C#19 |
| H-022 | P1 | FFI tutorial calls `coppa_engine_destroy(engine)` against a `T**` signature (UB/segfault for anyone who follows it); no `#include`, no link line. | S | C#38 |
| H-023 | P1 | No config validation: `deny_unknown_fields` absent (typos in sections/keys accepted), unknown profile silently defaults yet logs the bogus name as valid, `buffer_size = 0` accepted (WARN storm), `sample_rate` is a knob on a fixed-48 kHz engine, example file documents a nonexistent `[session]` block. | S | B#7, B#8, B#28–30, F#17 |
| H-024 | P1 | Callsign optional and only WARNed; invalid callsign still allows TX. Require a valid callsign to enable any TX path (Part 97). Same in the CLI: `--callsign` is accepted, unvalidated, and unused. | S | B#31, A#5, A#39 |
| H-025 | P1 | **Connected-session data has no selective-repeat retransmission.** The established-session TX branch sends a `MacPdu` directly via `encode_bytes`/`transmit_samples`, returning before reaching `ArqTx::send`; `handle_session_data` (RX) forwards the payload straight to `DataOut` without going through `ArqRx`. `ArqTx`/`ArqRx` exist and are real for the unconnected/raw data-port path and for CP-negotiation control PDUs -- only the connected-session data path bypasses them. Fixing H-004--H-009 (VARA line discipline, `CONNECTED`/`BUFFER`) still leaves a connected Pat/Winlink-style session with an unreliable link on any dropped frame; wire connected-session data through `ArqTx`/`ArqRx` before claiming session support. | M | new finding, verified against `event_loop.rs:830-848,2683-2694`; D#ADR-008-row |
| H-026 | P1 | **Decoded RX payloads can be silently dropped at three separate `try_send` hops, not just one.** `main.rs:248`'s daemon-to-data-client `DataOut` bridge uses `try_send`; but so do the two upstream hops that feed it -- the raw/ARQ RX path (`event_loop.rs:1438-1446`, warns but still drops) and the established-session RX path (`event_loop.rs:2689-2693`, drops with no warning at all). Fixing only `main.rs:248` leaves a slow or bursty client able to lose decoded data at either of the other two points. Distinct from H-021 (telemetry `try_send`, `event_loop.rs:527-533`) -- these are the actual data path, not status lines. Use a bounded `send` with backpressure or timeout at all three hops, not silent drop. | S | C: dim-C-host-apis.md "Data port" table |
| H-027 | P1 | **H-002 alone does not implement the frequency-based HF/VHF regulatory gate H-150 promises.** H-002 only fixes `set_speed_level` to honour the *configured* `Profile.ofdm_profile` -- it does not add any way for the daemon to know what RF frequency the station is actually on. `RadioConfig` has no frequency/band field; `EventLoop::create_ptt` returns a type-erased `Box<dyn PttControl>`, so even the `rigctld` path (which COULD query frequency via `RigctldClient::get_frequency`) is inaccessible generically; serial, GPIO, VOX, and no-PTT configurations have no frequency source at all. Without this, a user can still select `VHF_FAST` on HF after H-002 lands -- the config-level fix and the frequency-based gate are two different pieces of work. Add an explicit band/frequency config setting, or retain/query a live `RadioControl`-capable source and fail closed when none is available, before treating H-150 as satisfied by H-002. | M | new finding, verified against `crates/coppa-daemon/src/config.rs:40-55`, `event_loop.rs:380-400` |

### 3.2 VARA-style TCP compatibility (beyond the blockers)

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-030 | P1 | Coalesce the data-port stream into modem-sized blocks; today one TCP `read` (≤ 4096 B) = one over-the-air frame. | M | C#11 |
| H-031 | P1 | Pair one command client with one data client; today all clients share TX/RX/telemetry and two attached apps would both transmit. | M | C#12 |
| H-032 | P1 | Act on `ABORT` (immediate teardown, purge queue, `DISCONNECTED`); `DISCONNECT` should drain the TX queue and ARQ first. | S | C#13, C#14 |
| H-033 | P1 | Accept `COMPRESSION OFF/TEXT/FILES`, `BW500/2300/2750`, `CHAT`, `PUBLIC`, `CWID`, `WINLINK SESSION`, `P2P SESSION`, `CLEANTXBUFFER`, `CQFRAME` with `OK`; map BW→profile cap, CWID→ID toggle, CLEANTXBUFFER→purge. Publish a `docs/TNC.md` coverage matrix (supported / no-op / missing) as Mercury does. | M | C#15, E#4, E "UX lessons" |
| H-034 | P1 | Emit `PENDING`/`CANCELPENDING` on inbound CONNECT_REQ; reply to `VERSION` with `CARGO_PKG_VERSION` (drop the hard-coded `Coppa 0.1.0` greeting); rename `SNR n` → `SN n` gated on `CHAT ON`. | S | C#8, C#16, C#17, B#49 |
| H-035 | P1 | End-to-end VARA conformance test driving a real `VaraServer` + `EventLoop` with Pat's exact `\r` sequence, asserting `OK`, `CONNECTED src dst bw`, `DISCONNECTED` on the socket. Then run actual Pat (`pat connect varahf:///CALL`) against two daemons over an audio loop, and document the Pat config. | M | C#21, C#22, E#5 |
| H-036 | P2 | Verify LinBPQ/BPQ32 can use coppa as a VARA-type port; recruit 3–5 gateway sysops for a shadow deployment before approaching the Winlink Development Team. | M | E#6 |
| H-037 | P2 | Verify VarAC and VARA Chat drive coppa on Windows; publish a setup page. Mercury already claims this. | S | E#7 |
| H-038 | P2 | Implement CWID (Morse ID at session end / 10-min timer), host-controllable via `CWID ON/OFF`. VARA has it; Mercury does not. Station ID today is a coppa-only beacon PDU. | S | E#15, B#32 |
| H-039 | P2 | Surface frequency/mode control to hosts (`RigctldClient` already implements get/set frequency; daemon uses it only as PTT), so gateways and a future QSY feature can tune through the modem. | M | B#36, H#25 |
| H-040 | P3 | Consider an ardop-style host protocol (port 8515) after VARA is solid; it is the second-largest installed base. | L | C "Missing surfaces" |

### 3.3 Daemon operations, packaging, and deployment

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-050 | P1 | Ship tagged releases with static binaries: linux x86-64/aarch64/armv7, Windows x86-64, macOS universal, built with `cpal-backend,websocket,serial-ptt,gpio-ptt,kiss-tnc` (H-012 makes the TNC build a release blocker; omitting the feature here would ship every binary without `coppa tnc`); `cargo binstall`; `[profile.release]` with `lto`, `codegen-units=1`, `strip`. | M | E#1, D#5, B#11, B#48, G#39 |
| H-051 | P1 | Ship `contrib/coppad.service` (systemd, `Restart=on-failure`, non-root with audio/dialout/gpio groups) and a launchd plist; document a Raspberry Pi 4 install end to end. | S | B#12, E#35, D#43 |
| H-052 | P1 | Config discovery: `--config`, then `./coppad.toml`, `~/.config/coppa/`, `/etc/coppa/`, `%APPDATA%`; print "no config found, using defaults; copy coppad.toml.example" when nothing is found. | S | B#22, D#48 |
| H-053 | P1 | Startup banner must state: version + git SHA, built features, bind address, every listening port, chosen audio devices (input/output), PTT method and its state. Disabled servers should be mentioned as disabled. | S | B#18, B#44, A#31 |
| H-054 | P1 | Logging: route every `println!`/`eprintln!` through `tracing`; disable ANSI when not a TTY; add `--log-format json`/`--log-file`; warn on an invalid `RUST_LOG` instead of silencing everything. | S | B#19–21, C#32 |
| H-055 | P1 | CM108/CM119 HID PTT (DigiRig, DRA, AIOC, RA-boards: the most common USB ham interface). Also expose `ptt_invert`, which the serial/GPIO backends already support. | M | B#9, B#45, E#19 |
| H-056 | P1 | `serial-ptt`/`gpio-ptt` are not in the default build and the README still calls them "Stub"; make them default features (or document the rebuild loudly). | S | B#10 |
| H-057 | P1 | `coppad --check` (config dry-run) and a health/status surface a supervisor can probe (`GET /health`, PID file, or `coppad status`). | M | B#23, E#35 |
| H-058 | P2 | `sysfs` GPIO is deprecated and disabled on Raspberry Pi 5 / kernel 6.6+; move to `gpiod` character-device GPIO. | M | B#46 |
| H-059 | P2 | Audio device selection: first-substring-match with no disambiguation; hard-coded mono; no resampler fallback -- `coppa-audio/src/resampler.rs` exists but is not declared as a `pub mod`, so it is neither compiled nor usable today; no left/right channel select for stereo interfaces. | M | B#25, B#26 |
| H-060 | P2 | RX/TX level metering with clipping/underdrive warnings (Direwolf prints level per decode; VARA has a VU). Needed for the dashboard too. | M | B#27, H#6 |
| H-061 | P2 | Hot reload (SIGHUP) and runtime reconfiguration of callsign/PTT/devices; second Ctrl-C should force-exit. | M | B#24, B#38 |
| H-062 | P1 | **Loopback binding is not a real mitigation for the WebSocket control plane.** Any web page open in a browser on the operator's own machine can open a cross-origin `WebSocket` to `ws://127.0.0.1:8400` and send `send`/`connect` commands that key the transmitter -- the server performs no `Origin` check and requires no token before accepting commands, so binding to loopback only stops a *different host* on the network, not a malicious or compromised page in the operator's own browser. Require an `Origin` allowlist and/or a shared-secret token on the WebSocket (and the VARA command port, for LAN use where Pat runs on a separate host from the radio Pi) regardless of bind address. | M | B#34, C#30, H#8; browser cross-origin attack surface new finding |
| H-063 | P2 | `apt` repo (Debian/RPi OS), Homebrew tap, Windows signed installer; Docker image; DigiPi integration PR. | M | E#2, E#33, E#34, H#34 |
| H-064 | P2 | Windows daemon build/test coverage is partial: CI's platform matrix does compile-check and lib-test the whole workspace on Windows (`cargo check --workspace`, `cargo test --workspace --lib`), plus an extra `coppa-audio --features cpal-backend` test, but there is no release build, no `serial-ptt`-feature build, and no functional runtime test of `coppad` on Windows. | M | B#37 |
| H-065 | P3 | Multi-instance: instance name in logs, non-colliding default config path, systemd template guidance. | S | B#35 |
| H-066 | P3 | TNC mode is not a Direwolf alternative: no digipeat, no beacon, single port, no AGW, no config, VOX is a no-op, blocks RX during TX, ignores TXDELAY/P/SLOTTIME. Either invest (see H-122) or reposition it as "KISS bring-up only". | L | B#14, B#40–42, C#34, C#35 |

### 3.4 CLI UX

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-070 | P1 | Silent exit-0 failures: wrong-sample-rate WAV, zero frames decoded, compressed-profile frame decoded with the default profile (prints garbage hex), unknown `--ptt` value, unmatched `--device` (falls back to the laptop speaker), rigctld connect failure (plays audio unkeyed). Each should be an error. | S | A#2–4, A#9, A#12, A#13 |
| H-071 | P1 | Merge `rx` (streaming, hex, no duration) and `listen` (batch re-decode of a growing window, text, `--duration`) into one receiver with one output grammar; `listen` re-decodes the whole window on every read. | M | A#19, A#42, D#47 |
| H-072 | P1 | Config file + env vars for the CLI (callsign, device, rigctld, profile), sharing `coppad.toml` sections and `COPPA_*` env. | M | A#38 |
| H-073 | P1 | Profile mental model: expose `--level N` and a `coppa profiles` table (level, modulation, code rate, max bytes, air time, bandwidth); `config -p HF_ROBUST` reports max payload 64 while the engine rejects > 56; `EMERGENCY` == `HF_ROBUST` on the wire; level 8 does not exist (levels 1–7, 9, 10). Rename `config` → `profiles`. | M | A#8, A#20–22, D#9, D#46 |
| H-074 | P1 | `--json` on `rx`/`devices`/`profiles`/`listen`; `-q`/`-v` short flags with counting; `conflicts_with` for `--quiet --verbose`. | S | A#6, A#14, A#15 |
| H-075 | P1 | `coppa doctor`: enumerate devices, confirm 48 kHz mono on the chosen device, TCP-connect rigctld and query PTT state without keying, dry-run lead/tail, print a pass/fail table. Most "it doesn't work" reports are plumbing. | M | A "Suggested commands" |
| H-076 | P2 | Clean Ctrl-C on live `rx`/`listen` (currently exit −2, no cleanup, no summary); progress/liveness during long ops; `CLIP` warning when |sample| ≥ 0.99 (SNR reads −23.8 dB on a clipped input that decodes fine). | S | A#10, A#30, A#43 |
| H-077 | P2 | Default WAV output to 16-bit PCM (32-bit float EXTENSIBLE breaks Python `wave` and older ham tools; the repo's own golden vectors are 16-bit); positional input for `rx`; stdin/stdout raw PCM streaming (`RawF32Source/Sink` exist but are not exposed). | S | A#27, A#34, A#35 |
| H-078 | P2 | Shell completions and man page (`clap_complete`, `clap_mangen`); `--version` with git SHA, features, wire-format version; `arg_required_else_help`. | S | A#31, A#33, A#36, D#56 |
| H-079 | P2 | Consistency: `--seconds` vs `--duration` vs none; `--ptt-lead-ms` vs `--single <SINGLE>`; long `about` bleeding into the command table; global flags rendered mid-list; `config -p BOGUS` exits 0 on stdout while every other subcommand exits 1. | S | A#7, A#16–18, A#40 |
| H-080 | P2 | Input validation: empty message, `--profile ""`, `tune --seconds 0`, `--single 30000` (above Nyquist), negative numbers → clap "unexpected argument". | S | A#23–26 |
| H-081 | P2 | New commands worth building once the daemon is trustworthy: `chat --to CALL`, `send <file>`/`recv`, `beacon`, `monitor` (ratatui), `tx --dry-run`, `levels`/`calibrate rx`. Keep `tnc` visible when not compiled in, with a rebuild hint. | M–L | A "Suggested commands", A#32, H#17 |
| H-082 | P3 | Uniform stdout/stderr discipline (status chatter to stderr; data to stdout); duplicated error on bad `-o` path; colour with `NO_COLOR`; `devices` should mark the default device and 48 kHz support. | S | A#28, A#29, A#37, A#41 |

### 3.5 Modem performance, robustness, and evidence

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-090 | P0 | Two-host audio-cable loopback test (two computers, two USB sound cards, no radio): decode 20/20 golden WAVs through the cable; 10-minute ARQ session with 0 drops; repeat with ±50 ppm resample; measure RX-last-sample → PTT-on. Publish the log and WAVs under `results/ota/`. This is the cheapest possible evidence and it does not exist. | M | F#1, F protocol stage 1, E#8 |
| H-091 | P1 | Fade-diversity across a coherence time. Correction: an eight-frame V2 cross-frame interleaver WAS already implemented and measured (`BENCHMARKS.md` "Transfer-level cross-frame interleaving"), and the hypothesis was falsified -- it made Watterson recovery worse (Moderate recovery dropped from ~45% to ~21% in a post-LDPC-fix rerun), and the design is shelved. Any further fade-diversity work needs to be a materially different mechanism (frequency diversity across the OFDM carriers, or a longer codeword deliberately spanning multiple fades) named explicitly, not a repeat of the tested cross-frame scheme. On CCIR-Moderate everything above QPSK 1/2 still sits on a 35-100% SNR-independent outage floor; on CCIR-Poor nothing clears FER <= 10%. | L | F#3; `results/review-2026-09-05/{mod,poor}-ssb-std`; `BENCHMARKS.md` "Transfer-level cross-frame interleaving" |
| H-092 | P1 | ARQ give-up policy under fading: 0/5 Moderate and 0/5 Poor sessions survive, 2–3/5 Good. Adaptive max-retransmit, drop-to-level-1 before giving up, "never drop while a header decodes". Gateway operators judge on drops, not peak bps. | M | F#5, E#27 |
| H-093 | P1 | A genuine weak-signal mode (≤ 100 bps, narrow, long coherent integration; the unreachable `hf_narrow` 8-carrier profile with BPSK 1/4, or a 4-FSK/chirp level). Floor today is ≈ +4 dB (3 kHz); VARA/PACTOR move data below 0 dB. | M | F#4, F#22, E#31 |
| H-094 | P1 | Wire multi-codeword frames into the daemon's ARQ path (currently one codeword per frame; 47 % fixed overhead at 64-QAM) and a short dedicated ACK frame instead of a full data-level frame per ACK. | M | F#6, F#18 |
| H-095 | P1 | Replace the vacuous Monte Carlo test (full-band noise convention ≈ 9 dB optimistic, sweep starts above every waterfall, 0/100 at all 71 points by construction; cited in CLAUDE.md as evidence) with a 3 kHz-convention sweep that spans each waterfall and asserts with a Wilson bound; run a reduced version in CI. Add a nightly reduced-trial bench job so no FER/goodput figure is unprotected. | S | F#9, F#33 |
| H-096 | P1 | Daemon↔daemon session integration test through Watterson + SSB filter + CFO + SCO on the real audio-dispatch path (the only fading-exercising integration test today bypasses the daemon). | M | F#8 |
| H-097 | P1 | Publish an honest headline table: SSB-filtered, `hf_standard` at every level, AWGN + Good/Moderate/Poor, ARQ user bps and drop rate, side by side with IONOS VARA/PACTOR curves, with "simulation only, no OTA" stated. Resolve the SNR-reference ambiguity in BENCHMARKS.md (two contradictory BPSK tables) by relabeling every pre-3 kHz table. Run the existing session-bench bytes/min metric at the IONOS study's fixed WGN/MPG/MPP SNR grid so it plots directly against VARA/ARDOP/PACTOR (a bytes/min number already exists; it just isn't at IONOS-comparable SNR points). | S | F#11, E#10, E#11, E#28, D#34 |
| H-098 | P2 | Rate loop misses its own bar (0.914/0.762 vs > 1.0/≥ 0.8). Active probing (`with_probing`) IS wired into the daemon (`event_loop.rs:289-293,569-587`). The project's own committed diagnostics falsified an oscillation/hysteresis-shape fix (a bounded-drop/leaky-raise sweep reproduced the mechanism working as designed but not moving the aggregate metric) and traced the real remaining lever to the underlying per-frame speed-level recommendation signal's volatility/low readings during fast fading, not hysteresis tuning. Fix that signal's quality/stability instead. | M | F#10; CLAUDE.md RateLoop diagnostic |
| H-099 | P2 | `turnaround_ms` and `Profile.arq_window/max_payload` are unenforced, not unread: `Profile.max_payload`/`arq_window` ARE read and printed to the user by the CLI's `cmd_config` (`coppa-cli/src/main.rs:935-936`) as if they were the profile's real limits, but the engine and daemon ARQ configuration ignore them entirely and use `ArqConfig::default()` regardless -- so the CLI is actively showing users stale, presentation-only numbers. Either wire these values into the engine/daemon or fix `cmd_config` to stop displaying limits that aren't enforced. (`Profile.ofdm_profile` IS read, at initial construction via `CoppaCore::from_profile` -- see the narrower gap in H-002/dim-F.) | S | F#17, B#30; CLI display, `coppa-cli/src/main.rs:935-936` |
| H-100 | P2 | Channel-model gaps: no impulsive noise, clipping/ALC, AGC, frequency tilt, or group-delay ripple; PAPR clip schedule (up to 14 dB at 64-QAM) is unverified against any ALC model. | M | F#14, F#29 |
| H-101 | P2 | CFO envelope ±50 Hz is one subcarrier spacing; add an integer-bin ambiguity search or wider acquisition (ARDOP does ±100 Hz). The stale `#[ignore = "OFDM sync has no CFO correction yet"]` test misleads. | M | F#15, F#31 |
| H-102 | P2 | Sync detector runs on unfiltered samples (−9 dB detection margin by design); sync is the dominant failure below 12 dB only at the two most robust levels (1–2) — from level 3 up, LDPC non-convergence already dominates at 6 dB. A cheap decimated pre-filter recovers the margin for the robust levels. | S | F#20; addendum tables |
| H-103 | P2 | `CpGate` threshold (2.5 ms) is mis-set against the estimator's 0.417 ms grid; `BusyGate` thresholds were never tuned on real recordings. | S | F#21 |
| H-104 | P2 | Turnaround/latency is unmeasured (audio poll 20 ms, retransmit poll 500 ms, ACK is a full frame); publish a measured RX-last-sample → PTT-on figure. | M | F#16 |
| H-105 | P2 | Golden vectors cover levels {1,2,5,6,9} only; add 3,4,7,10, multi-codeword, RV 1–3 retransmission, a CONNECT/ACK PDU vector, and a real-radio capture once H-090 exists. Add a second-implementation interop test (e.g. NumPy reference decoder) to back the "buildable from SPEC" claim. | S–M | F#23, F#40 |
| H-106 | P2 | SPEC covers PHY/FEC only; MAC/transport/ARQ/session wire format is not normative (the u8→u32 ACK-bitmap widening was a wire break documented only in an ADR). Add front-matter (spec version, date, wire version, changelog) and replace the dead `f725f7c` reference. No protocol version negotiation exists. | M | F#24, F#25, D#30 |
| H-107 | P2 | Buy or build a Teensy IONOS simulator (< USD 200, the Winlink team's method) and run the exact IONOS matrix; then the on-air protocol stages 3–6 in dimension F (VHF-FM two rigs, HF NVIS via a real Winlink client, KiwiSDR one-way capture, DX/QRN campaign). | M–L | E#9, F protocol |
| H-108 | P3 | Level-4 residual fading gap and level 3–6 Moderate regression shipped unresolved; `vhf_wide` CP (1.25 ms) is shorter than CCIR-Poor's 2 ms tap (document as line-of-sight only); Watterson module doc claims coherence time ≥ frame, contradicted by CLAUDE.md; `SPEED_LEVELS` still labels level 10 as rate 7/8. | S–M | F#19, F#27, F#38, D#32 |
| H-109 | P3 | `cargo bench`'s end-to-end OFDM benches (`engine_encode`/`engine_decode`) cover only the default speed level; add a multi-level sweep, a streaming/daemon-path latency bench, and an OTA goodput bench. | S | F#32 |

### 3.6 Documentation, onboarding, and positioning

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-110 | P0 | Fix `docs/tutorials/getting-started.md`: expected sample counts (46080 → 65520), expected `rx` output, the redundant `--features file-backend`, the understated `cpal-backend` requirement on both binaries, the rigctld prerequisite, the `coppad.toml` sample that contradicts the shipped example, the v1-only C snippet (see H-022). | S | D#15, A#45, B#43 |
| H-111 | P1 | GitHub repo metadata: description ("Open-source OFDM HF data modem for amateur radio, in Rust — VARA-style TCP API, C FFI"; add "Winlink-ready" only after H-035's real Pat test passes, not at Phase 0), topics (`ham-radio`, `amateur-radio`, `hf`, `ofdm`, `ldpc`, `modem`, `winlink`, `rust`, `dsp`, `tnc`, `ax25`), homepage, social preview; enable Discussions; issue templates including a "field report" template; `CHANGELOG.md`; `docs/README.md` index. Thirty minutes of settings. | S | D#4, D#17, D#21, D#27, D#28 |
| H-112 | P1 | README rewrite per the skeleton in dimension D: tagline that says OFDM/HF/Rust; "not RF-compatible with VARA, both ends run coppa" as a call-out framed as a feature; three "start here" audiences (operator / Pat-Winlink integrator / Rust DSP dev); waterfall GIF and a golden WAV as "what it sounds like"; performance-at-a-glance table with the simulation caveat; comparison table vs VARA/ARDOP/Mercury/PACTOR; install with prebuilt binaries; feature-flag table; honest status paragraph; documentation map. | M | D#10, D#11, D#16, D#33, D#41, D "Proposed README skeleton" |
| H-113 | P1 | `docs/PAT.md` (Pat `varahf` config → 8300/8301, which commands are honoured, two-station setup) and `docs/HOST-API.md` (VARA table with deviations, data-port semantics, WebSocket schema, KISS, ports, an `nc` walkthrough). Nothing today tells a Pat user how to connect; the WS schema exists only as serde derives. | M | D#12, D#13, C#20, C#31, H#36 |
| H-114 | P1 | Docs architecture: split the 81 KB CLAUDE.md into a ≤ 100-line agent brief plus `docs/LIMITATIONS.md` (Tony's global rule); purge ~35 references to gitignored `.superpowers/` and `docs/superpowers/` paths and `dispensa`-relative links; delete `PLAN-hardening.md` (all items done or obsolete) and `models/README.md` (describes a deleted registry); decide whether `wiki/` is for humans or agents and either render it or move it under `docs/notes/`. | M | D#7, D#8, D#18–20, D#36, D#52 |
| H-115 | P2 | Fix 47 `cargo doc` warnings and gate with `RUSTDOCFLAGS=-D warnings`; crate-level docs with a runnable example for `coppa-protocol` (one line today, 299 pub items, 0 doctests), `coppa-codec` (mentions no OFDM), `coppa-dsp`; per-crate READMEs; an "embedding coppa in your Rust app" guide (the tutorial covers CLI, daemon, and C but not Rust). | M | D#24, D#25, G#7, G#13, G#19, G#20, G#34 |
| H-116 | P2 | Reorganise BENCHMARKS.md (219 KB, reverse-chronological, corrections mid-file) into a short current-state page plus dated `docs/benchmarks/` logs; publish `docs/MODES.md` (per-level bandwidth, raw/net bps, SNR@FER≤10% per channel, frame time) as Mercury does; a standing CI-generated scoreboard page. | M | D#35, E#28, E#29 |
| H-117 | P2 | `ROADMAP.md` (Linear COP is private; the open items from the gap analysis and this review should be public); ADR index with an "Accepted/Superseded" column (ADR-002 is superseded by ADR-005 but says Accepted); "hardware tested" section even if empty; CONTRIBUTING additions (how to add a level, regenerate golden vectors, run benches; fix the stale "CI runs only `--lib`" line). | S | D#38, D#42, D#49–51 |
| H-118 | P2 | Naming: "coppa" is search-invisible (page 1 = the US children's-privacy law, a GCP CLI, a packaging consortium). Keep the name but always write "Coppa HF modem" in titles/topics/keywords, register `coppa-modem` or use `coppa-*` crate names, add a pronunciation line. GitHub Pages/mdBook site once the docs are true. | S–M | D#37, D#40, E#40 |
| H-119 | P3 | Small polish: neutral example callsign in README, MSRV out of the License section, dual-licence detection (GitHub shows Apache-2.0 only), `CITATION.cff` + Zenodo DOI for the SPEC on first release, stale `codex-clean:<sha>` labels. | S | D#31, D#44, D#45, D#53, D#54 |

### 3.7 Host-API completeness and new integration surfaces

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-120 | P1 | WebSocket: emit `data` as the JSON envelope (today a bare non-JSON string that breaks every parser), `connected`/`disconnected`/`error` from daemon status, forward session-path RX to WS, implement `mycall` (currently stuffed into a VARA command and ignored), binary/base64 payloads (B2F and compressed traffic cannot flow as `String`), typed push events (`ptt`, `busy`, `buffer`, `session`, `snr`, `level`, `heard`, `log`, `decode_fail`), request ids + acks, a `hello` with protocol version, spectrum-frame metadata (`fft_size`, `bin_hz`), a JSON Schema generated from the serde enums. Critically, once real application/session data flows over this channel it must NOT share the existing 64-entry broadcast channel that silently `continue`s on `RecvError::Lagged` (`websocket.rs:311-326`) -- a slow client (or one delayed by spectrum/log traffic on the same channel) can permanently lose decoded payloads with no error. Give `data` events a reliable per-client queue or a replay/sequence mechanism, and leave only genuinely disposable telemetry (spectrum, periodic status) on the lossy broadcast path. | M | C#23–29, C#31, H#3–5, H#13, H#15, H#16; lossy-broadcast finding new |
| H-121 | P1 | Serve a static SPA from the daemon (`GET /` + `/ws` on one port, axum), and the RX level meter event, as prerequisites for the dashboard. | S | H#6, H#7 |
| H-122 | P2 | AGWPE server (port 8000): `R G g X x k K M y` subset first. Winlink Express Packet, Outpost, UI-View, Xastir, YAAC expect AGW, not KISS. KISS: mask the port nibble, parse TXTAIL/FULLDUP/SETHW, honour TXDELAY/P/SLOTTIME with p-persistence CSMA using the busy gate; expose `TncConfig` via `[tnc]` in `coppad.toml`. | M | C#34–36, E#21 |
| H-123 | P2 | KISS-over-TCP datagram mode on the OFDM HF modem (not only AFSK): Reticulum consumes any KISS modem, Mercury verified this; also enables `kissattach`/IP-over-coppa. Publish `docs/RETICULUM.md`. | M | C#37, E#22 |
| H-124 | P2 | FFI: `cpp_compat = true` (`extern "C"` guard), `COPPA_API` export macro, `coppa_last_error()`/`coppa_strerror()`, distinguish "no message" from error, document thread safety (handles are internally locked), `sample_rate()`/`max_payload()` accessors, `struct_size` on `coppa_config_t` before v2 is a stable ABI, `deprecated` attributes on the v1 quartet, CI job compiling a C smoke test and publishing header + cdylib per platform, `coppa.pc`. | M | C#39–43, G#6 |
| H-125 | P2 | Link-level FFI (`coppa_link_connect/listen/send/recv/poll`) so embedders (iOS/Android, RadioMail-class apps) get session/ARQ without running the daemon; `bindings/python` (ctypes + WS/VARA client) on PyPI; a Go `cgo` example. | L | C#44, C#45, E#26 |
| H-126 | P3 | REST `/status`, `/health`, `/sessions`, Prometheus `/metrics`; mDNS `_coppa._tcp`/`_vara._tcp` advertisement. | M | C#46, B#23 |

### 3.8 Rust library and crate hygiene

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-130 | P1 | Publishing readiness: `repository`/`readme`/`keywords`/`categories`/`documentation` + `[package.metadata.docs.rs]` on every publishable crate; `version = "0.1"` alongside `path =` so `cargo publish` works and `deny.toml` `wildcards` can return to `deny`; `coppa-ffi/build.rs` must write `coppa.h` to `OUT_DIR` (it writes into the source tree, which fails `cargo publish` verification); `publish = false` on bench/cli/daemon; break the `coppa-protocol` dev-dep → `coppa-bench` → `coppa-protocol` cycle; tag `v0.1.0`; a `release.yml` that publishes in dependency order. The `coppa`, `coppa-dsp`, `coppa-engine` names are unclaimed on crates.io. | M | G#4–6, G#17, G#18, D#22, E#26 |
| H-131 | P1 | `coppa-audio`'s `default = ["cpal-backend"]` leaks into `coppa-cli`/`coppa-daemon`/`coppa-bench`, pulling in cpal/alsa dependencies even when their own local `cpal-backend` feature is off (their runtime audio-setup code still correctly gates on that local feature, so this is unwanted dependency activation, not a functional no-op) -- and it breaks aarch64-linux `cargo check` on `alsa-sys`. Consumers should use `default-features = false`. Add `wasm32` and `aarch64-linux` `cargo check` of the library crates to CI. | S | G#12, G#56, E#3 |
| H-132 | P1 | Error strategy: replace `anyhow::Result` in public signatures of 5 of 6 library crates (`Modem` trait, `CoppaCore::decode` with string errors, 37 `pub fn` in `coppa-protocol`) with per-crate `thiserror` enums; also include `coppa-host`, whose `WebSocketServer::run`, `VaraServer::run`, and `KissServer::start` are all `anyhow::Result` -- host API callers can't match server failure kinds either. `StreamFrame.payload: Result<Vec<u8>>` embeds an error in an event struct. | L | G#3, G#29, G#44 |
| H-133 | P1 | Panic safety on caller input: `coppa-dsp` public fns `assert!` on arguments (`try_*` variants exist but the plain names are what consumers reach for); 33 non-test panic sites in `coppa-protocol` (21 in `fec/ldpc`, public), 10 in `coppa-codec`; public constructors (`CrossFrameInterleaver`, `RateLoop`, `CoppaProfile`) assert on args. Return `Result` or `pub(crate)` them; add `# Panics` sections. | M | G#10, G#36–38 |
| H-134 | P2 | API ergonomics: `Debug`/`Clone` on every `coppa-dsp` struct (none today, so downstream cannot derive `Debug`); re-export `num_complex::Complex32` (manta must pin a matching `num-complex`); `#[must_use]` (zero in the workspace); `EngineConfig` `PartialEq`/`serde` (`Default` is already implemented); `#[non_exhaustive]` on public enums; document that `CoppaCore` is `Send + !Sync`; `forward_into`/`demap_soft_into` out-param variants to stop per-symbol allocation; consistent `u32` sample rate. | M | G#8, G#9, G#11, G#15, G#16, G#27, G#30–32, G#43 |
| H-135 | P2 | Lints: `#![warn(missing_docs)]` (25/43 `coppa-dsp` pub items undocumented, 44 in protocol, 35 in codec), `#![forbid(unsafe_code)]` on the 10 crates with none, `-D warnings` on the `--no-default-features` build (4 dead-code warnings); `cargo semver-checks` + `cargo public-api` snapshot once published (also enforces the manta `coppa-dsp` contract, which is currently a stub page with nothing enforcing it). | S | G#7, G#40, G#48, G#54, G#55 |
| H-136 | P2 | Portability: `coppa-protocol` compiles for wasm but `Session`'s API (`session.rs`) calls `Instant::now()` unconditionally (runtime panic on `wasm32-unknown-unknown` for any consumer using sessions); take a `now` parameter or a `Clock` trait (also the `no_std` path). `cp_negotiator.rs` does NOT call `Instant::now()` in production code -- its only production reference to `Instant` is an unused-outside-tests import, and every actual call is under `#[cfg(test)]` -- so it needs no such refactor. `coppa-channel` fails on wasm via `getrandom`; take an `Rng` parameter. | M | G#22, G#23, G#42 |
| H-137 | P3 | Move the 41 task-numbered diagnostics in `coppa-bench/examples/` to `src/bin/` or `diagnostics/`; rename `examples/bpsk_loopback.rs` (it is OFDM); prune 3 stale `deny.toml` advisory ignores; document the `Cargo.lock` and MSRV/toolchain-pin policy. | S | G#41, G#49, G#52, G#53, D#26 |

### 3.9 New product surfaces

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-140 | P1 | Browser WASM demo (`crates/coppa-wasm`, GitHub Pages): type text → pick level → play through speakers → decode from mic via `AudioWorklet` → waterfall + SNR + decoded text; "two phones talking acoustically" mode. Verified feasible: `coppa-engine` compiles for `wasm32` unmodified; a scratch cdylib is 490 KB before `wasm-opt`; nothing in the encode/streaming-decode path touches threads, `Instant::now()`, tokio, or cpal. The single best marketing and education asset available. | S–M | H#27–29, G#24 |
| H-141 | P1 | Web dashboard served by `coppad` (ardopcf pattern: one static HTML file, vanilla JS + canvas waterfall, embedded with `include_str!`): waterfall/spectrum, RX level meter, S/N sparkline, our and peer speed level, CFO, PTT/BUSY lamps, buffer, session panel (state, peer, in-flight/retries/RTO), heard list, TUNE buttons, log pane, MYCALL/connect/send box. Every reference modem has this; without it an operator cannot set a level or tell TX from a hang. Wireframe in dimension H. No native GUI, no Electron. | M | H#11–14, H#18, E#17 |
| H-142 | P2 | Heard-stations list from beacon RX (callsign, grid, level, SNR, last heard) + WS `heard` event; beacon-RX is log-only today. Then PSKReporter / own-map reporting. | S | H#12, E#24 |
| H-143 | P2 | Chat as a dashboard tab with the protocol in `coppa-protocol` so Pat and others can reuse it: wire the existing `AppPdu` framing (zero callers today), delivery receipts via ARQ ack, ping/SNR-report exchange, unconnected CQ text, file transfer with manifest/resume over `AppPdu::fragment`, persisted message log. QSY via rigctld and store-and-forward later. This is the VarAC audience, which is "a nightmare on Linux". | L | H#19–25, E#25 |
| H-144 | P2 | `coppa monitor` ratatui TUI reusing the WS client, for SSH-into-the-Pi operators. | S | H#17 |
| H-145 | P2 | Appliance-lite: `.deb` + `coppad.service` + a `/setup` first-run wizard page (callsign/grid, device pick from an enumerate API, PTT method incl. GPIO pin, rigctld, TUNE test, beacon interval), which needs a config read/write API. Contribute to DigiPi rather than maintain a Pi image. Mobile = the responsive dashboard on a LAN-bound daemon with a token, not a native app. | M | H#30–34 |
| H-146 | P3 | One-to-many broadcast file mode (FEC + repetition, no ARQ) à la hermes-broadcast; a low-rate ALE/beacon waveform (FT8/JS8-class, −15 to −20 dB) for connect-probe and spotting; VHF-FM-Wide-class data profile validated on a 9600-capable radio. | L | E#23, E#30, E#32, H#26 |

### 3.10 Regulatory

| ID | P | Item | Effort | Src |
|---|---|---|---|---|
| H-150 | P0 | Gate `vhf_wide` (350–5900 Hz) off below 29.7 MHz in the daemon, not by convention: 47 CFR 97.307(f)(3) caps HF data at 2.8 kHz since 2024-01-08. H-002 alone does not satisfy this -- it fixes the *configured* profile to persist, but the daemon has no frequency source to gate against at all; see H-027 for the actual frequency-awareness gap this bullet depends on. | S | E#13; H-027 |
| H-151 | P2 | Document band-plan compliance per profile: FCC 2.8 kHz occupied-bandwidth cap OK for all HF profiles including `hf_wide` (2450 Hz occupied width, under the FCC and IARU R1 2700 Hz bandwidth caps -- an earlier draft of this review wrongly flagged `hf_wide` as non-R1 by comparing its 2800 Hz upper edge against the 2700 Hz bandwidth figure); still verify `hf_wide`'s specific frequency placement against IARU R1 segment boundaries (not checked in this review); Canada allows 6 kHz on HF. Compression is fine because the SPEC publicly documents it (§97.309(a)(4)); ship a Message-Viewer-style offline decoder (`coppa rx --decompress --dump`) to close the loop. | S | E#14, E#16 |
| H-152 | P2 | Station ID: a completely silent session (never transmits) never IDs -- but this is by design, not a bug, since `transmit_samples` (the single TX chokepoint) already prepends the ID/beacon whenever `id_due()` is true, so any real transmission including a DISCONNECT frame already carries ID if the interval has elapsed. Remaining work: a CWID option (H-038). | S | F#34 |

## 4. Sequenced roadmap

Each phase is gated on the previous one being *demonstrably* true, because
the project's credibility problem is that its claims run ahead of its
evidence.

**Phase 0 — Make it true (about a week).** H-016, H-017, H-110, H-111,
H-011, H-010, H-012, H-013, H-114 (CLAUDE.md split and dead-path purge).
After this a visitor reads an accurate README, follows a tutorial that works,
and CI is green.

**Phase 1 — Make it work (two to three weeks).** H-001, H-002/H-150,
H-003, H-004–H-009, H-014, H-015, H-018–H-027, H-030, H-034, H-062 (WebSocket auth), H-070, H-090 (cable test),
H-035 (Pat for real), H-050–H-054, H-056, H-130, H-131. Cut `v0.1.0` with
binaries at the end. After this a Pat user on a Raspberry Pi can install a
binary, point Pat at 8300, and complete a session over an audio cable, and
there is a published log proving it.

**Phase 2 — Make it robust (one to two months, the real work).** H-091,
H-092, H-093, H-094, H-095–H-097, H-102, H-098, H-099, H-055, H-057,
H-107 (IONOS simulator, then VHF-FM, then HF NVIS). After this coppa holds a
link through CCIR-Moderate, has a mode below 0 dB, and has one on-air result
published next to the IONOS VARA/PACTOR curves.

**Phase 3 — Make it visible (in parallel with Phase 2, different skills).**
H-120, H-121, H-141 (dashboard), H-140 (WASM demo), H-113, H-115, H-116,
H-118 (site), H-132–H-135 (crate hygiene, then crates.io), H-036, H-037
(LinBPQ / VarAC interop), H-063 (apt/brew).

**Phase 4 — Make it grow.** H-143 (chat), H-142, H-122 (AGW), H-123
(Reticulum), H-125 (link FFI + Python), H-145 (appliance), H-146.

## 5. Decisions the owner must make

These are value judgements or scope calls; reviewers deliberately did not
decide them.

1. **Is the product a modem-for-hosts (Pat/Winlink/VarAC drive it) or a
   complete station (coppa ships its own chat/dashboard)?** The review
   recommends both in sequence (host first, because it is cheaper and the
   audience exists), but the chat/app layer (H-143) is a multi-month
   commitment that changes what the project is.
2. **Speed ladder on HF.** Drop level 10 from HF, narrow `hf_standard`'s top
   edge, or add erasure handling (H-003). And whether level numbering is
   renumbered 1–9 (a wire break) or documented as "1–7, 9, 10".
3. **How much of the VARA command surface to emulate**, and whether to
   invest in an ardop-style protocol too (H-040). Full VARA fidelity is the
   fastest route to Pat/LinBPQ/VarAC; it also means coppa is judged as a
   "VARA replacement" on VARA's terms.
4. **The TNC/AFSK path**: invest to Direwolf parity (digipeat, beacon, AGW,
   multi-port, ~weeks), or reposition it as a KISS bring-up demo and say so.
5. **Naming.** Keep "coppa" and fight for discoverability, or rename before
   the first release while there is nothing to lose.
6. **Release versioning policy**: whether `0.x` minor bumps track wire-format
   breaks, and whether `v0.1.0` ships before or after Phase 1's fixes.
7. **Hardware and money**: a second sound card and computer for H-090; a
   Teensy IONOS simulator (< USD 200) for H-107; two radios for the VHF and
   HF stages.
8. **The `.superpowers/` and `docs/superpowers/` references**: commit the
   referenced design docs or purge the references (H-114).
9. **Wiki audience**: agents or humans (H-114).

## 6. Dimension reports (this directory)

| File | Dimension | Items |
|---|---|---|
| `2026-09-05-dim-A-cli-ux.md` | `coppa` CLI UX, with every `--help` and error capture | 50 |
| `2026-09-05-dim-B-daemon-ops.md` | `coppad` config, first-run, deployment, safety; startup/error captures | 50 |
| `2026-09-05-dim-C-host-apis.md` | VARA compatibility matrix, Pat walkthrough, WebSocket, KISS, FFI; three probe transcripts | 48 |
| `2026-09-05-dim-D-docs-positioning.md` | Stale-statement table (~35 rows), onboarding walk-through, README skeleton, positioning statement, gap-analysis status | 57 |
| `2026-09-05-competitive-landscape.md` | VARA, ARDOP/ardopcf, FreeDATA, Mercury, PACTOR, VarAC, Direwolf, Reticulum, JS8Call; regulatory context; jobs-to-be-done; comparison table; wedge strategies; UX lessons; sources | 42 |
| `2026-09-05-dim-F-modem-fitness.md` | Measured numbers, per-level effective throughput, IONOS side-by-side, fresh SSB/`hf_standard` sweeps, on-air validation protocol | 41 |
| `2026-09-05-dim-G-rust-library.md` | Per-crate scorecard, `cargo doc`/`deny`/wasm/`tree` captures | 56 |
| `2026-09-05-dim-H-missing-surfaces.md` | Telemetry inventory, per-surface build/defer verdicts, dashboard wireframe, WS and wasm captures | 36 |

Raw CSVs for the fresh sweeps are under `results/review-2026-09-05/`.
