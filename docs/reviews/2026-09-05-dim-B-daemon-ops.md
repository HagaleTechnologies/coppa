# B — `coppad` daemon: configuration, first-run, deployment, operations, safety

Reviewer dimension: operator lifecycle of the daemon. Read-only review of
`/Users/thagale/Code/coppa` at `1c2ffc5` plus live runs of the prebuilt
`target/release/coppad` / `coppa` (`--features cpal-backend,websocket`) on
loopback ports with `ptt_method = "none"`. No radio was keyed; TX audio was
routed to the "Jump Desktop Audio" virtual device.

## Summary

Overall grade: **D+ as an operator-facing product; B- as an engineering
substrate.** The internals are careful (single TX chokepoint, hard-error on
unknown PTT method, busy gate, station-ID timer, spectral telemetry, bounded
host servers, extensive unit tests) but almost nothing an operator touches has
been finished: there is no CLI parsing at all (`coppad --help` and
`coppad --version` are silently treated as config file paths and start the
daemon on the system mic/speaker), no config validation beyond TOML syntax
(section/key typos, unknown profile names, unsupported sample rates and
`buffer_size = 0` are all accepted and the daemon reports "Daemon ready"), no
packaging (no `cargo install` docs, no release binaries, no deb/rpm/brew/AUR,
no Docker, no aarch64 builds, no systemd unit), no health/status command, and
no supported install path for the two most common ham PTT interfaces (CM108
HID is not implemented at all; serial DTR/RTS is behind a non-default feature
and the README still calls it a "Stub").

The three biggest problems:

1. **Daemon TX is truncated on air with the default config (safety/correctness
   bug).** `transmit_samples` writes the whole frame into an 8192-sample
   non-blocking ring; anything beyond that is dropped. Observed: `TUNE 1`
   (48,000 samples) logged `dropped=39808 total=48000`; PTT was held 1.42 s
   while only 170 ms of audio reached the device. A normal ~46k-sample frame
   suffers the same fate, so nothing the daemon sends via a real sound card
   can be decoded by a peer.
2. **Failures do not fail.** Bind conflicts (`Address already in use`),
   audio-stream failures (`Sample rate 44100 Hz is not supported`), unreachable
   rigctld, and a missing named audio device (silently falls back to the
   system default — i.e. the laptop microphone) all end in `INFO Daemon ready`
   with exit code 0. A gateway operator has no way to know their station is
   deaf, mute, or PTT-less short of reading logs line by line.
3. **The VARA-style control port does not answer.** Only `LISTEN ON/OFF` and
   `TUNE` are handled in the daemon; `MYCALL`, `COMPRESSION`, `BW*`, `VERSION`,
   `ABORT` and unknown commands get no `OK`/`WRONG` at all -- a real VARA
   compatibility gap, though the host-API reviewer's walkthrough finds the
   command that actually blocks Pat first is the bare-`\r` line terminator,
   not a missing `OK` (Pat's own startup sequence does not wait for one). Either
   way, the headline "VARA-style TCP interface: Working" claim in the README
   will not survive a first contact. (Protocol detail belongs to the host-API
   reviewer; the ops consequence — "Pat cannot use this modem" — is squarely
   a first-run problem.)

Secondary: the AFSK/KISS TNC path is not shippable — `coppa tnc` does not
compile (`E0560: TncConfig has no field named audio_device`), `coppad --tnc`
is not in the built binary, and CI never builds the `kiss-tnc` feature.

## What's already good

- Unknown/unbuilt `ptt_method` values are hard startup errors with actionable
  messages (`config.rs:97-152`, observed: `unknown PTT method "serial";
  expected one of "none", "vox", "rigctld", "serial:<port>:<dtr|rts>",
  "gpio:<pin>"` and `PTT method 'serial' requires coppad to be built with
  --features serial-ptt`).
- TOML syntax errors are fatal with line/column context (observed `TOML parse
  error at line 1, column 6`).
- Every TX path funnels through `transmit_samples` (`event_loop.rs:1529`),
  which applies PTT lead/tail, busy-channel deferral, station-ID prepend and
  a `max_tx_duration_s` cap on the PTT-release timer.
- Host servers default to `127.0.0.1`, with a clear warning in both
  `config.rs:159-164` and `coppad.toml.example` about the unauthenticated
  control plane; per-server concurrent-connection caps and bounded line/frame
  sizes (`vara/command.rs:13`, `websocket.rs:118-125`, `kiss.rs`).
- SIGINT and SIGTERM both produce a clean, logged shutdown with exit 0
  (observed for both signals).
- Station-ID timer defaults to 540 s (Part 97.119 margin) and is prepended to
  real traffic only; busy gate and beacon are opt-in; the three-flag CP
  negotiation gate is documented and tripwired by tests.
- `coppa devices` gives a useful device listing with channel counts and max
  sample rate.
- Serial PTT (DTR/RTS, inverted option) and Linux sysfs GPIO PTT are real,
  unit-tested implementations with sensible module docs (Raspberry Pi
  `gpio` group guidance in `ptt_gpio.rs:12-31`).
- `SECURITY.md` is honest about the no-auth surface and the reference-impl
  status.

## Hit list

| # | Item | Category | Impact | Effort | Evidence |
|---|------|----------|--------|--------|----------|
| 1 | Daemon TX truncated to `buffer_size` samples: `handle_audio_out` writes the whole frame into a non-blocking ring that drops on full; `CpalSink::write` is also drop-on-full into its own 8192 ring. Need a blocking/chunked writer that paces at the sample rate (or a ring sized >= max frame + guard), and the PTT-release timer must be derived from samples actually delivered. | bug / safety | H | M | `event_loop.rs:1483-1502`, `ringbuf.rs:42-50`, `cpal_backend.rs:262-264,268`; observed `WARN Audio output buffer overflow dropped=39808 total=48000` on `TUNE 1`, PTT TX 02:36:11.601 -> RX 02:36:13.023 |
| 2 | VARA command port never replies `OK`/`WRONG`; `MYCALL`, `COMPRESSION`, `BW*`, `VERSION`, `ABORT` are parsed but ignored by the daemon. This is a real VARA compatibility gap, though Pat's own startup sequence does not wait for `OK` -- the host-API reviewer's walkthrough identifies the actual Pat-blocking sequence as the bare-`\r` line terminator, dropped session `StatusUpdate`s, `BUFFER` semantics, `\r\n` vs `\r` responses, and missing `IAMALIVE`. Overlaps host-API reviewer; listed here because the missing `OK`/`WRONG` still blocks first-run with any client that does wait for it. | bug | H | M | `event_loop.rs:886-912` handles only `LISTEN ON/OFF`/`TUNE`; observed `printf 'VERSION\r\nMYCALL W5AU\r\n...' | nc` returned only the greeting `VERSION Coppa 0.1.0`; dim-C walkthrough |
| 3 | No CLI argument parsing: `--help`, `--version`, `-c`, `--config`, `--check`, `--log-level` all absent. `args().nth(1)` is taken as the config path; `--help` therefore starts the daemon and opens the default mic/speaker. | bug / polish | H | S | `main.rs:46-49`; observed `coppad --help` ran until SIGTERM with `Daemon ready` |
| 4 | Host server bind failure is not fatal: `eprintln!("VARA server error: ...")` from a spawned task, then `Daemon ready`, exit 0, and the daemon keeps running with no control plane. Same for WebSocket. | bug / safety | H | S | `main.rs:262-266,308-312`; observed `VARA server error: Address already in use (os error 48)` followed by `INFO coppad: Daemon ready` |
| 5 | Audio stream failure is not fatal and is reported via `eprintln!` rather than the logger: `Failed to start audio input: ... Sample rate 44100 Hz is not supported` then `Daemon ready`. A daemon with no RX audio is indistinguishable from a healthy one at INFO. | bug / safety | H | S | `main.rs:120-122,137,178-180,195`; observed with `sample_rate = 44100` |
| 6 | Named audio device not found silently falls back to the **system default** device (could be the laptop mic/speakers, or the wrong USB interface after re-enumeration). Should be a hard error, or at minimum an ERROR-level log and a `status` flag. | safety | H | S | `main.rs:106-113,161-168`; observed `WARN Audio input device not found, falling back to default device=NoSuchDevice` then `Daemon ready` |
| 7 | No config schema validation: `#[serde(default)]` without `deny_unknown_fields`, so `[radoi]`, `calsign = ...` and any misspelled key are silently ignored. Direwolf/ardopcf reject unknown keywords. | bug | H | S | `config.rs:8-9,25,38,157,190,296` (no `deny_unknown_fields` anywhere); observed typo config accepted and daemon started |
| 8 | Unknown `[engine] profile` silently falls back to `EngineConfig::default()` and the startup log prints the bogus name as if valid (`profile=HF_STANDRD`). | bug | H | S | `event_loop.rs:225-230`; observed `Daemon configuration loaded profile=HF_STANDRD` |
| 9 | CM108/CM119 HID PTT (the most common USB ham interface: DigiRig, DRA-series, RA-boards, Masters Communications) is not implemented. Also no `hamlib` native (rigctld only) and no VOX-tone/"vox" that actually does anything (`VoxPtt` is a state holder; nothing drives it). | missing-feature | H | M | `config.rs:113-116` parse list; observed `unknown PTT method "cm108"`; `vox_ptt.rs:36-44` |
| 10 | `serial-ptt` and `gpio-ptt` are not in the documented/default build; the README status table still says `Serial/GPIO PTT | Stub | no hardware access` although both are implemented. Operators will not know they must rebuild. | docs / packaging | H | S | `README.md:26`; `coppa-daemon/Cargo.toml:41-42`; observed `PTT method 'serial' requires coppad to be built with --features serial-ptt` |
| 11 | No packaging at all: no `cargo install` instructions, no GitHub releases (`gh release list` empty), no release/cross-build job in CI (`ci.yml` has no `aarch64`, `release`, or artifact upload), no deb/rpm/brew/AUR, no Docker, no Raspberry Pi guidance beyond a doc comment in `ptt_gpio.rs`. | packaging | H | L | `.github/workflows/ci.yml` grep for `release|artifact|aarch64|cross` matched nothing; repo has no `packaging/`, `*.service`, `Dockerfile` |
| 12 | No systemd unit / launchd plist / Windows service, and no guidance on running unprivileged with `audio`/`dialout`/`gpio` group membership, `Restart=on-failure`, or `KillSignal`. | packaging / docs | H | S | `find . -name '*.service' -o -name '*.plist'` empty; `docs/OPERATING.md` covers only TUNE |
| 13 | `coppa tnc` does not compile: CLI constructs `TncConfig { audio_device, .. }` but the struct has `bind_address` and no `audio_device`. CI never enables `kiss-tnc`, so this has rotted unnoticed. | bug | H | S | `crates/coppa-cli/src/main.rs:959-965` vs `tnc.rs:14-20`; observed `cargo check -p coppa-cli --features kiss-tnc --release` -> `error[E0560]: struct TncConfig has no field named audio_device`; `ci.yml:57,92` only `cpal-backend,websocket` |
| 14 | `coppad --tnc` uses `TncConfig::default()` with no config file, no port/device/PTT selection, and is not in the shipped feature set; TNC mode also has no RX/TX device selection at all (always default device). | missing-feature | M | M | `main.rs:34-44`, `tnc.rs:22-31,145,177`; observed `Error: TNC mode requires the kiss-tnc feature` |
| 15 | PTT is never explicitly released on shutdown or on fatal error; `run()` just returns. A SIGTERM mid-transmission (systemd stop, crash-loop restart) leaves rigctld/serial PTT keyed until the OS drops the line, and rigctld PTT is not dropped at all. No `Drop` impl on `SerialPtt`/`RigctldClient`. | safety | H | S | `event_loop.rs:660-664` (Shutdown path), `main.rs:327-331`; `ptt_serial.rs` and `rigctld.rs` have no `Drop`; only `SysfsGpio` unexports (`ptt_gpio.rs:84-91`) |
| 16 | `max_tx_duration_s` only caps the PTT-release *timer*; it does not stop audio or abort a queued TX, and with the TX-queue design the next frame can key up immediately after. There is no hardware/external watchdog (e.g. rigctld-side timeout, GPIO heartbeat) if the process dies keyed. `max_tx_duration_s = 0` yields an immediate unkey while audio still plays. | safety | M | M | `event_loop.rs:1566-1585` |
| 17 | rigctld unreachable at startup degrades to `NullPtt` with a WARN and the daemon runs "ready" with no PTT (acknowledged as a "known residual gap" in the wiki). No reconnect attempt either; a rigctld restart permanently orphans PTT. | safety | H | S | `event_loop.rs:389-400`; `wiki/pages/phase4-field-readiness.md:52-54`; observed `WARN Failed to connect to rigctld; falling back to no PTT ... Connection refused` then `Daemon ready` |
| 18 | Startup log does not say what is listening where: VARA logs "starting" (not "listening" with bind address), WebSocket logs via `println!` (`WebSocket server listening on ws://...`) outside the tracing pipeline, disabled servers are not mentioned, bind address is never printed, chosen default audio device names are never printed. | polish | M | S | `main.rs:224-228,283`, `websocket.rs:257`; observed startup transcript below |
| 19 | Mixed `eprintln!`/`println!`/`tracing` output: audio errors, VARA/WS server errors, KISS errors, WS client connect/handshake failures all bypass tracing (no timestamp, no level, unfilterable, not in journald-structured form). | polish | M | S | `main.rs:120,137,178,195,266,312`; `vara/server.rs:134,154,160,178,195,201`; `websocket.rs:257,273,286,293`; `kiss.rs:249` |
| 20 | ANSI colour escapes are emitted even when stdout is not a TTY (systemd/journald and log files get `\x1b[2m...`). No `--log-format json`, no `--log-file`, no journald target. | polish | M | S | `main.rs:20-26` (`fmt().with_env_filter(...).init()` with default ansi); observed raw escapes in `out.txt` |
| 21 | `RUST_LOG` is the only verbosity control; an odd/invalid value silently suppresses *all* logging with no diagnostic. | polish | M | S | `main.rs:22-25`; observed `RUST_LOG='garbage='` produced only the WS `println!` line |
| 22 | No config discovery: only `./coppad.toml` relative to cwd (or argv[1]); no `~/.config/coppa/coppad.toml`, `/etc/coppa/`, XDG, or `%APPDATA%`. Running from a systemd unit with a different `WorkingDirectory` silently uses defaults with no "no config file found, using defaults" notice. | missing-feature | M | S | `main.rs:46-49`, `config.rs:352-367`; observed no-config run printed only `No callsign configured` |
| 23 | No `coppad --check-config` / dry-run mode and no `coppad status` / health endpoint / PID file; nothing a supervisor can probe (`systemd` `ExecStartPre`, Nagios, a Pat pre-flight). The WS `status` message exists but requires the websocket feature and a WS client. | missing-feature | M | M | no such code paths in `main.rs`; `websocket.rs:24-26` is the only status surface |
| 24 | No hot reload (SIGHUP) and no runtime reconfiguration of callsign/PTT/devices; `MYCALL` from the host is ignored (see #2), so the callsign lives only in the file and requires a restart. | missing-feature | M | M | grep `SIGHUP|reload` in `crates/coppa-daemon/src` empty |
| 25 | Audio device selection is first-substring-match, case-insensitive, with no index/UID option and no disambiguation warning when two devices match (common: two "USB Audio CODEC" interfaces on a two-radio host). | polish | M | S | `cpal_backend.rs:322-350` (`.find(...contains(needle))`) |
| 26 | Audio channel handling is hard-coded mono (`channels: 1`); devices exposing only stereo configs fail to open, and there is no left/right channel selection for stereo interfaces (Direwolf `ACHANNELS`/`ADEVICE` idiom). Sample rate is used verbatim with no resampler fallback even though `coppa-audio` ships `resampler.rs`. | missing-feature | M | M | `cpal_backend.rs:126-131,152-156,222-227,248-252`; observed `Sample rate 44100 Hz is not supported` |
| 27 | No RX/TX level metering or clipping/underdrive warnings in the daemon (`OPERATING.md` describes a manual ALC procedure only). Direwolf prints audio level with every decoded frame; VARA shows a VU meter. | missing-feature | M | M | `event_loop.rs:1034-1044` has busy gate + spectrum but no level/clip stats; `docs/OPERATING.md` |
| 28 | `buffer_size = 0` (and any tiny value) is accepted; the daemon then spams a WARN every 20 ms forever. Needs validation (minimum, and a floor relative to max frame length once #1 is fixed). | bug | M | S | `config.rs:33-35` no validation; observed continuous `Audio input buffer overflow dropped=1024 ...` |
| 29 | `[audio] sample_rate` is configurable but the engine is fixed at 48 kHz; the example file even says "Must match engine (48000 for OFDM)". A non-48k value produces wrong-speed audio/timing math rather than an error. Either remove the knob or validate `== 48000` (or resample). | bug | M | S | `coppad.toml.example:5-6`, `event_loop.rs:1560-1566` uses configured rate for timing; observed daemon "ready" at 44100 |
| 30 | `coppad.toml.example` documents a `[session]` block (`arq_window`, `max_retries`, `turnaround_ms`, ...) that has no corresponding config struct — it is dead documentation. `turnaround_ms`/`max_frames_per_turn` are hard-coded. | docs | M | S | `coppad.toml.example:87-104`; `config.rs` has no `session`; `event_loop.rs:357-358` hard-codes `max_frames_per_turn: 4, turnaround_ms: 500` |
| 31 | Callsign is optional and only WARNed; an invalid callsign is only WARNed (`station ID/beacon and connect handling will be unavailable`) and the daemon still runs and can TX (TUNE, raw data path) unidentified. For a Part 97 station a non-empty valid callsign should be required to enable any TX path. | safety | M | S | `main.rs:53-55`, `event_loop.rs:237-259`; observed with `callsign = "not a call"` |
| 32 | Station-ID is prepended as a `Beacon` MAC PDU at speed level 1 — machine-readable to Coppa only. Part 97.119 requires ID in a recognised form; for a proprietary/undocumented-to-listeners waveform, a CW or plain-text ID option (as ardop/VARA do with CW ID) should be offered and documented. | safety / docs | M | M | `event_loop.rs:1626-1666` (`build_beacon_mac_pdu`/`encode_id_beacon_frame`) |
| 33 | Busy-channel detection is off by default (`busy_hold_ms = 0`) and has no threshold/dB knob; gateway operators expect an on-by-default busy detector with adjustable sensitivity and a host-visible override. | polish | M | S | `config.rs:272-283`; `coppad.toml.example:61-66` |
| 34 | No authentication or allow-list option at all for the control plane; the only mitigation is "leave it on loopback". Even a shared-secret line on the command port, a per-client `AUTH` command, or a Unix-socket option would allow safe LAN use (Pat on a different host than the radio Pi is a very common topology). | safety / missing-feature | M | M | `SECURITY.md` "no authentication"; `config.rs:159-164` |
| 35 | Single-instance design: no per-instance names in logs, no way to run two radios except two configs on different ports; `coppad.toml` default path collides. No `instance`/`name` field, no `%i`-friendly systemd template guidance. | polish | L | S | `main.rs:46-49`; `config.rs` |
| 36 | No frequency/mode control surfaced to hosts: `RigctldClient` implements `get/set_frequency`/`set_mode` but the daemon uses it only as `PttControl`; the VARA/WS APIs expose no `FREQ`/QSY, so gateways (RMS, scanning) cannot QSY through the modem. | missing-feature | M | M | `rigctld.rs:89-148`, `event_loop.rs:380-400` boxes as `dyn PttControl` |
| 37 | Windows daemon coverage is partial: CI's `windows-latest` leg does `cargo check --workspace` (compiles the default `coppad` binary) and `cargo test --workspace --lib`, plus an extra `coppa-audio --features cpal-backend` test -- but there is no release build, `serial-ptt` (COM ports) is never built on Windows, and there is no functional runtime test of `coppad` or a Windows service/tray story. `docs/tutorials/getting-started.md` claims "No additional dependencies on macOS or Windows". | docs / packaging | M | M | `ci.yml:183-201`; `getting-started.md:7` |
| 38 | Second Ctrl-C does not force-exit: tokio's `ctrl_c()` handler replaces the default; if the event loop is blocked (e.g. in `wait_for_clear_channel`'s sleep loop) shutdown is deferred and a second SIGINT is swallowed. | polish | L | S | `main.rs:289-294`; `event_loop.rs:1590-1610` sleeps on the same task that handles `Shutdown` |
| 39 | Shutdown does not stop/drain audio streams or flush TX; `shutdown_flag` is set and `run()` returns, then `main` exits while audio threads may still be mid-callback. Fine in practice, but a TX in flight is cut without unkeying (see #15). | polish | L | S | `event_loop.rs:660-664`, `main.rs:327-331` |
| 40 | TNC mode blocks its whole select loop with `tokio::time::sleep` for the duration of each TX, so RX frames decoded during TX are not forwarded until TX ends, and KISS `TXDELAY`/`P`/`SLOTTIME` are parsed but ignored (no p-persistence CSMA, no DCD). | missing-feature | M | M | `tnc.rs:215-224`; `kiss.rs:24-26,151-167` parsed only |
| 41 | TNC lacks Direwolf table stakes: no digipeating, no beaconing, single KISS port, no AGWPE port, no APRS-IS iGate, no 9600/G3RUH, no `coppa tnc` config file, no audio level display. As-is it is a KISS-over-TCP bring-up demo, not a Direwolf alternative. | missing-feature | M | L | `tnc.rs` (242 lines total); `afsk.rs:10-17` fixed 1200 baud/48 kHz |
| 42 | TNC PTT: `rigctld` fallback to `NullPtt` on connect failure with `eprintln!`, and VOX "mode" is a no-op state holder (no tone/VOX-hold logic), so `--vox` neither keys nor delays anything. | bug | M | S | `tnc.rs:69-89`; `vox_ptt.rs:36-44` |
| 43 | `getting-started.md` still tells users to `cargo run --bin coppad` with `vara_enabled = true` and no mention of `--features cpal-backend`; without it the daemon "starts cleanly but no audio flows" (acknowledged in `wiki/pages/cpal-feature-gate.md`). No first-run checklist (devices -> config -> TUNE -> connect). | docs | M | S | `docs/tutorials/getting-started.md:85-118`; `wiki/pages/cpal-feature-gate.md:20-26` |
| 44 | The startup banner does not state the feature set the binary was built with (cpal/websocket/serial-ptt/gpio-ptt/kiss-tnc). Combined with #10/#43 this is the number-one "why doesn't it work" support question. | polish | M | S | `main.rs:28-31` logs only `version` |
| 45 | `ptt_pre_delay_ms`/`ptt_tail_delay_ms` are honoured but there is no `ptt_invert`/`inverted` config even though `SerialPtt::open(.., inverted)` and `GpioPtt::open(.., inverted)` support it (always passed `false`). | missing-feature | L | S | `event_loop.rs:411` passes `false`; `ptt_serial.rs:100`, `ptt_gpio.rs:132` |
| 46 | `SysfsGpio` uses the deprecated `/sys/class/gpio` interface (removed/disabled on Raspberry Pi 5 / kernel 6.6+ distributions); should use `gpiod`/character-device GPIO. Also unexports on drop while possibly still high. | bug | M | M | `ptt_gpio.rs:1-8,49-91` |
| 47 | No Debian/Ubuntu/RPi runtime dependency note for `serialport` (`libudev`) and ALSA at runtime; only the build-time `libasound2-dev` hint exists. | docs | L | S | `getting-started.md:6`; `Cargo.toml` workspace deps |
| 48 | Release profile is default (no `lto`, `codegen-units`, `strip`), giving a 5.5 MB daemon; irrelevant until #11 exists but should ship with it. | packaging | L | S | root `Cargo.toml` has no `[profile.release]`; `ls -la target/release/coppad` = 5,523,328 bytes |
| 49 | Version string is hard-coded in two places (`VERSION Coppa 0.1.0` in `vara/command.rs:37` vs `env!("CARGO_PKG_VERSION")` in `main.rs:30`); no git SHA/build date in any banner. | polish | L | S | `vara/command.rs:37`, `main.rs:28-31` |
| 50 | `websocket` and `vara-tcp` servers print client connect/disconnect at INFO with no peer address on the VARA side, and WS prints the address via `println!`; no rate limiting on reconnect storms; no per-client idle timeout on the command port. | polish | L | S | `event_loop.rs:813-818`; `websocket.rs:286`; `vara/command.rs` |

## Comparison with what operators expect

- **Direwolf** (`direwolf.conf`): one heavily-commented config with `ADEVICE`,
  `ACHANNELS`, `ARATE`, `PTT CM108` / `PTT /dev/ttyUSB0 RTS` / `PTT GPIO 17`,
  `DCD`, `TXDELAY`, `TXTAIL`, `PERSIST`, `SLOTTIME`, `MYCALL`, `DIGIPEAT`,
  `PBEACON`, `AGWPORT`, `KISSPORT`, `IGSERVER`, `LOGDIR`, and an audio-level
  readout on every decode. Ships with a `direwolf.service`, deb/rpm/brew/AUR,
  and `dw-start.sh`. Coppa's TNC mode has KISS-over-TCP and 1200 AFSK only.
- **ardopcf**: single binary, `ardopcf 8515 plughw:1,0 plughw:1,0` with
  `-p /dev/ttyUSB0` (RTS), `--ptt CM108:/dev/hidraw0`, `-g` GPIO, `-k`/`-K`
  hex PTT strings, `-H` host-command tracing, `-l` log dir, `-c` CAT string,
  busy-detector `-B` and a `TUNE` equivalent. Coppa lacks CM108, log dir,
  host-command tracing, and CAT strings.
- **VARA HF**: GUI, but its TCP protocol contract (`OK` after every command,
  `PTT ON/OFF`, `BUFFER n`, `BUSY ON/OFF`, `MYCALL`, `LISTEN`, `CWID`,
  `CLEANTXBUFFER`, `PUBLIC`, `BW`) is what Pat, Winlink Express and RMS
  Trimode drive. Coppa emits the telemetry but does not acknowledge commands
  (#2) and has no `CWID`.
- **FreeDATA**: `config.ini` with `[AUDIO] input_device/output_device` by
  index or name, `[RADIO] control = rigctld|serial|tci`, `ptt_port`,
  `[STATION] mycall/mygrid`, a web UI on a documented port, a `--config` flag,
  a modem watchdog and a bundled AppImage/exe. Coppa is close on the config
  vocabulary but far behind on discovery, validation and packaging.
- **Pat / RMS gateway operators** run the modem under systemd on a Raspberry
  Pi, non-root, with `Restart=on-failure`, and rely on `rigctld` for QSY and a
  PTT that is guaranteed to drop on process death. Items #11-#17 cover the
  gap.

## Verbatim captures

Startup with no config in cwd (ANSI stripped); note no "config not found"
notice and no "listening on"/bind-address line:

```
$ coppad            # (also identical for `coppad --help` and `coppad --version`)
2026-09-06T02:32:49.704219Z  INFO coppad: coppad - Coppa Daemon starting version="0.1.0"
2026-09-06T02:32:49.704270Z  WARN coppad: No callsign configured. Set [engine] callsign in config.
2026-09-06T02:32:49.704275Z  INFO coppad: Daemon configuration loaded profile=HF_STANDARD sample_rate=48000 ptt_method=none
2026-09-06T02:32:49.886090Z  INFO coppad: Audio input started sample_rate=48000
2026-09-06T02:32:49.890709Z  INFO coppad: Audio output started sample_rate=48000
2026-09-06T02:32:49.890752Z  INFO coppad: Daemon ready
2026-09-06T02:32:49.890758Z  INFO coppad::event_loop: Event loop started profile=HF_STANDARD
2026-09-06T02:32:52.705317Z  INFO coppad: Received SIGTERM, shutting down
2026-09-06T02:32:52.705359Z  INFO coppad::event_loop: Shutdown signal received
2026-09-06T02:32:52.705373Z  INFO coppad: Daemon stopped
[still running after 3s -> SIGTERM]  exit=0
```

Raw log bytes as written to a file (colour escapes leak into non-TTY output):

```
[2m2026-09-06T02:32:41.677931Z[0m [32m INFO[0m [2mcoppad[0m[2m:[0m coppad - Coppa Daemon starting [3mversion[0m[2m=[0m"0.1.0"
```

Invalid TOML (good):

```
$ coppad bad.toml
2026-09-06T02:33:22.543469Z  INFO coppad: coppad - Coppa Daemon starting version="0.1.0"
Error: Failed to parse config bad.toml: TOML parse error at line 1, column 6
  |
1 | this is not [[[ toml
  |      ^
key with no value, expected `=`
exit=1
```

PTT config errors (good):

```
$ coppad badptt.toml      # ptt_method = "serial"
Error: failed to start daemon event loop: invalid [radio] ptt_method: unknown PTT method "serial"; expected one of "none", "vox", "rigctld", "serial:<port>:<dtr|rts>", "gpio:<pin>"
$ coppad serial.toml      # ptt_method = "serial:/dev/ttyUSB0:dtr" on the shipped binary
Error: failed to start daemon event loop: PTT method 'serial' requires coppad to be built with --features serial-ptt
$ coppad cm108.toml       # ptt_method = "cm108"
Error: failed to start daemon event loop: invalid [radio] ptt_method: unknown PTT method "cm108"; expected one of ...
```

Typo'd section/key, unknown profile, 44.1 kHz, missing device — all accepted,
daemon "ready" with no input stream:

```
$ cat typos.toml
[radoi]
ptt_method = "bogus"
[engine]
calsign = "W5AU"
profile = "HF_STANDRD"
[audio]
sample_rate = 44100
input_device = "NoSuchDevice"
$ coppad typos.toml
2026-09-06T02:33:30.575756Z  WARN coppad: No callsign configured. Set [engine] callsign in config.
2026-09-06T02:33:30.575762Z  INFO coppad: Daemon configuration loaded profile=HF_STANDRD sample_rate=44100 ptt_method=none
2026-09-06T02:33:30.657988Z  WARN coppad: Audio input device not found, falling back to default device=NoSuchDevice
Failed to start audio input: Failed to build input stream: Sample rate 44100 Hz is not supported
2026-09-06T02:33:30.754587Z  INFO coppad: Audio output started sample_rate=44100
2026-09-06T02:33:30.754672Z  INFO coppad: Daemon ready
2026-09-06T02:33:30.754690Z  INFO coppad::event_loop: Event loop started profile=HF_STANDRD
```

Invalid callsign + unreachable rigctld — daemon runs with no PTT:

```
2026-09-06T02:33:34.617265Z  WARN coppad::event_loop: Failed to connect to rigctld; falling back to no PTT address=127.0.0.1:1 error=Failed to connect to rigctld at 127.0.0.1:1: Connection refused (os error 61)
2026-09-06T02:33:34.631642Z  WARN coppad::event_loop: Invalid [engine] callsign; station ID/beacon and connect handling will be unavailable callsign=not a call error=Callsign too long: 10 chars (max 8)
2026-09-06T02:33:35.011095Z  INFO coppad: Daemon ready
```

`buffer_size = 0` accepted; WARN storm every ~20 ms:

```
2026-09-06T02:33:38.820202Z  WARN coppad::event_loop: Audio input buffer overflow dropped=1024 cumulative_dropped=1024
2026-09-06T02:33:38.841075Z  WARN coppad::event_loop: Audio input buffer overflow dropped=1024 cumulative_dropped=2048
... (≈ 90 lines in 2 s)
```

Port already in use — non-fatal, exit 0, still "ready":

```
$ nc -l 127.0.0.1 18300 &  ;  coppad full.toml
2026-09-06T02:33:43.783460Z  INFO coppad: VARA TCP server starting command_port=18300 data_port=18301
2026-09-06T02:33:43.783496Z  INFO coppad: WebSocket server starting port=18400
VARA server error: Address already in use (os error 48)
2026-09-06T02:33:43.783685Z  INFO coppad: Daemon ready
2026-09-06T02:33:43.783693Z  INFO coppad::event_loop: Event loop started profile=HF_STANDARD
WebSocket server listening on ws://127.0.0.1:18400
$ lsof ... -> only ws 18400 listening; exit=0 on SIGTERM
```

VARA command port session — only the greeting comes back, no OK/WRONG:

```
$ printf 'VERSION\r\nMYCALL W5AU\r\nCOMPRESSION ON\r\nBW2300\r\nFOOBAR\r\nLISTEN ON\r\n' | nc -w 2 127.0.0.1 18300 | cat -v
VERSION Coppa 0.1.0^M
(daemon log) INFO coppad::event_loop: Client connected client_id=1
(daemon log) INFO coppad::event_loop: Listening for incoming connections
(daemon log) INFO coppad::event_loop: Client disconnected client_id=1
```

Plain HTTP to the WebSocket port (mixed println!/eprintln! output):

```
WebSocket client 1 connected from 127.0.0.1:52911
WebSocket handshake failed for 127.0.0.1:52911: WebSocket protocol error: No "Connection: upgrade" header
```

`RUST_LOG='garbage='` — all tracing output disappears, only the `println!` survives:

```
WebSocket server listening on ws://127.0.0.1:18400
```

TX truncation (`TUNE 1` = 48,000 samples into the default 8192-sample ring,
output routed to the "Jump Desktop Audio" virtual device, `ptt_method = "none"`):

```
2026-09-06T02:36:11.600636Z  INFO coppad::event_loop: TUNE: transmitting TX-level calibration tone seconds=1.0
2026-09-06T02:36:11.601158Z  INFO coppad::event_loop: PTT state change state="TX"
2026-09-06T02:36:11.652027Z  WARN coppad::event_loop: Audio output buffer overflow dropped=39808 total=48000 cumulative_dropped=39808
2026-09-06T02:36:13.023537Z  INFO coppad::event_loop: PTT state change state="RX"
```

`coppad --tnc` on the shipped binary, and `coppa tnc` absent from the CLI:

```
$ coppad --tnc
Error: TNC mode requires the kiss-tnc feature
$ coppa tnc --help
error: unrecognized subcommand 'tnc'
  tip: a similar subcommand exists: 'tune'
```

`coppa-cli` with the `kiss-tnc` feature does not compile:

```
$ cargo check -p coppa-cli --features kiss-tnc --release
error[E0560]: struct `TncConfig` has no field named `audio_device`
   --> crates/coppa-cli/src/main.rs:961:13
    |
961 |             audio_device: device,
    |             ^^^^^^^^^^^^ `TncConfig` does not have this field
    = note: available fields are: `bind_address`
error: could not compile `coppa-cli` (bin "coppa") due to 1 previous error
```

`coppa devices` (works, useful):

```
Available audio devices:
  LC49G95T (in: 0ch, out: 2ch, max: 48000 Hz)
  Elgato XLR Dock (in: 1ch, out: 2ch, max: 96000 Hz)
  Arctis Nova Pro Wireless (in: 1ch, out: 2ch, max: 48000 Hz)
  Mac mini Speakers (in: 0ch, out: 2ch, max: 96000 Hz)
  Jump Desktop Microphone (in: 8ch, out: 8ch, max: 192000 Hz)
  Jump Desktop Audio (in: 8ch, out: 8ch, max: 192000 Hz)
```

Clean signal handling (both SIGINT and SIGTERM):

```
2026-09-06T02:33:48.809848Z  INFO coppad: Received SIGINT, shutting down
2026-09-06T02:33:48.809876Z  INFO coppad::event_loop: Shutdown signal received
2026-09-06T02:33:48.809891Z  INFO coppad: Daemon stopped
exit=0
```
