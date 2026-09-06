# A — `coppa` CLI user experience review

Reviewed: `/Users/thagale/Code/coppa` @ `1c2ffc5` (ci: pin exact Rust toolchain, COP-9). Binary under test: `target/release/coppa` (0.1.0, built with `cpal-backend,websocket`; `kiss-tnc` NOT in this build). All runs from the scratchpad dir; no radio keyed (`--ptt none` or `-o file.wav`; one 1.4 s live playback to the default speaker to capture the warning path).

## Summary

**Grade: C-.** The CLI is a thin, clap-derived developer harness that happens to work, not an end-user tool yet. The DSP underneath is solid (every WAV I generated round-tripped at 25+ dB SNR, multi-frame files decode, stereo/16-bit PCM are handled), but the user-facing layer contradicts its own docs, hides failures, and lacks every "table-stakes" affordance of a modern Rust CLI. The three biggest problems: **(1) `coppa rx` prints lowercase hex, not text** — `coppa rx -i cq.wav` yields `Decoded: 435120435120...`, while README/getting-started promise `Decoded: "CQ CQ CQ DE VK2ABC K"`; a ham cannot read their own traffic. **(2) Silent, exit-0 failure is the norm:** a wrong-sample-rate WAV, a wrong-profile decode, a compressed-profile frame decoded with the default profile (prints garbage hex), an unknown `--ptt` method, an unknown `--device` with `-o`, `--quiet --verbose`, and `config -p BOGUS` all "succeed". There is no "0 frames decoded" summary, no sample-rate check, no signal-exit cleanup. **(3) The mental model is incoherent:** four named profiles (`HF_ROBUST`…) wrap 1–10 "speed levels" that the help text never names, `config` claims to "show current configuration" but there is no configuration (no config file, no env vars, no persisted callsign), `--callsign` is accepted and ignored, and `coppa config -p HF_ROBUST` reports `Max payload: 64 bytes` while the engine rejects anything over 56. Add zero `--json`, no `-q/-v` short flags, no completions/man page, no colour, inconsistent flag names across subcommands (`--seconds` vs `--duration`), and a sibling `coppad` that has no argument parser at all (`coppad --help` starts the daemon). None of this is hard to fix; most items are S-effort, and the fixes are the difference between "demo" and "tool I'd put in a go-box".

## What's already good

- Clap 4 derive: `--help`/`-h`/`-V` work everywhere, typo suggestions (`--out` → `--output`, `tnc` → `tune`), exit code 2 for usage errors, `help <sub>` works.
- Correct stdout/stderr discipline in `rx`: decoded payloads to stdout, diagnostics/`[verbose]` to stderr; `--raw` gives a clean one-line-per-frame pipe format. `Decode failed:` goes to stderr so it never corrupts a `--raw` capture.
- Unknown profile error lists the valid names. Profile lookup is case-insensitive.
- Oversized payload is a hard error (not silent truncation) and states the limit.
- `rx --input` handles 32-bit float, 16-bit PCM, and stereo (channel 0) WAVs; multi-frame files decode every frame; the trailing-silence flush is documented and works on the golden vectors.
- `tune` is a genuinely useful ham feature (two-tone + `--single` for wattmeter) with a good `docs/OPERATING.md` procedure.
- `--ptt-lead-ms`/`--ptt-tail-ms`/`--rigctld` are unit-tested to actually reach the PTT path.
- Unicode messages round-trip in loopback.

## Hit list

| # | Item | Category | Impact | Effort | Evidence |
|---|------|----------|--------|--------|----------|
| 1 | `rx` prints payload as lowercase hex; README + getting-started promise readable text. Print UTF-8 when valid (lossy or escaped otherwise), keep `--hex`/`--raw` for bytes. | bug/docs | H | S | `coppa rx -i cq.wav` → `Decoded: 435120435120435120444520564b32414243204b  (SNR: 25.7 dB)`; README.md "CLI Examples" + `docs/tutorials/getting-started.md:52-55` show `Decoded: "CQ CQ CQ DE VK2ABC K"`. `main.rs:733-741`. |
| 2 | `rx` on a compressed-profile frame with the default profile prints garbage hex, exit 0, no warning. Compression flag is per-profile, not signalled in the frame, so users must know the TX profile. Either put the flag in the header or auto-try/warn. | bug | H | M | `coppa tx "CQ CQ DE W5AU" -o std.wav --profile HF_STANDARD` then `coppa rx -i std.wav` → `Decoded: fe0a000000a007b3496692b45fc0f680`; with `--profile HF_STANDARD` → `43512043512044452057354155`. |
| 3 | Wrong-sample-rate WAV decodes nothing, silently, exit 0. `WavSource` knows the rate (`file_backend.rs:42`) but `cmd_rx` never compares it to `core.config().sample_rate`. Error (or resample) instead. | bug | H | S | `coppa rx -i cq44k.wav` → `Reading 60197 samples from cq44k.wav` `[exit=0]`; same for `noise8k.wav`. |
| 4 | No end-of-run summary or non-zero exit when zero frames decode from a file. Scripts can't distinguish "nothing there" from "decoded". Print `0 frames decoded` on stderr, exit 1 (or a distinct code) for `--input`. | bug | H | S | `coppa rx -i noise48.wav` → `Reading 96000 samples from noise48.wav` `[exit=0]`; `coppa rx -i cq.wav --profile VHF_FAST` → same, exit 0. |
| 5 | `--callsign` is accepted by `tx`/`listen` but never used (only echoed under `--verbose`). No validation, not placed in the frame, not persisted. Either wire it in (ID beacon / header) or drop it until it does something. | bug/consistency | H | M | `coppa tx hi -o cq.wav --callsign "not a callsign !!!"` → exit 0, no complaint; `grep -rn callsign crates/coppa-engine/src` → no hits; `main.rs:439-441, 816-818, 902`. |
| 6 | `--quiet --verbose` together silently resolves to quiet. Should be a clap `conflicts_with` error, or `-v` should win with a warning. | bug | M | S | `coppa --quiet --verbose loopback "both flags"` → no output, exit 0. `main.rs:188-194`. |
| 7 | `config -p BOGUS` prints "Unknown profile" to **stdout** and exits **0**; every other subcommand exits 1 for the same mistake. | bug/consistency | M | S | `coppa config -p BOGUS` → `Unknown profile: BOGUS` / `Available: …` `[exit=0]` vs `coppa rx -i cq.wav --profile BOGUS` → `Error: …` `[exit=1]`. `main.rs:939-944`. |
| 8 | `coppa config -p HF_ROBUST` reports `Max payload: 64 bytes` but the engine rejects >56 (`k_used/8 - 4`). HF_STANDARD says 128, real is 117 (before compression). The `Profile.max_payload` field is decorative and wrong. | bug | M | S | `coppa --quiet loopback AAA…(57)` → `Error: payload too large for this speed level (max 56 bytes)`; `profiles.rs:31,67`; `speed_levels.rs:87-90`. |
| 9 | Unknown `--ptt` value silently becomes "no PTT". A typo (`--ptt rigctl`) would transmit unkeyed with no warning. Use a clap `ValueEnum`. | bug | M | S | `coppa tx hi -o cq.wav --ptt bogus` → exit 0; `build_ptt` `_ =>` arm `main.rs:334`. coppad by contrast hard-errors (`coppad/src/main.rs:64-68`). |
| 10 | Ctrl+C on live `rx`/`listen` is uncaught: process dies with signal (exit 130/-2), `source.stop()` never runs, no "Stopped" line, no frame count. Install a ctrlc handler and finish cleanly. | bug | M | S | SIGINT after 3 s: `coppa rx` → `exit=-2`, stdout `Listening for live audio (Ctrl+C to stop)...\n`, nothing else. Same for `listen`. `main.rs:685-713`. |
| 11 | Payload-too-large error is not actionable: doesn't say the current size, which level/profile, or that a higher-speed profile / compression allows more. | polish | M | S | `Error: payload too large for this speed level (max 56 bytes)` (input was 300 bytes, profile unnamed). `transceiver.rs:461`. |
| 12 | `--device` is silently ignored whenever `-o`/`-i` is given (device only matters live), and "no matching device → using default" is only a WARNING, so a typo plays audio out of the laptop speaker instead of the rig. Make an unmatched `--device` a hard error. | bug | M | S | `coppa tx hi -o cq.wav --device NoSuchDevice` → exit 0 silent; live: `WARNING: No output device matching 'NoSuchDevice', using default` then `Transmitted 65520 samples (1.36s)` exit 0. `main.rs:385-391`. |
| 13 | rigctld connect failure downgrades to NullPtt with a WARNING and still plays audio, exit 0. For a transmit path that is data loss at best and an unkeyed blast at worst; should abort. | bug | M | S | `coppa tx hi --ptt rigctld --rigctld 127.0.0.1:1 …` → `WARNING: rigctld connect failed (… Connection refused (os error 61)), using no PTT` … `[exit=0]`. `main.rs:326-331`. |
| 14 | No machine-readable output anywhere: no `--json` on `rx`/`devices`/`config`/`listen`. Frame metadata (SNR, level, CFO, frame_start) only appears as free-text `[verbose]` on stderr. | missing-feature | H | M | `coppa devices --json` → `error: unexpected argument '--json' found`. |
| 15 | No `-q`/`-v` short flags; `--verbose` is a bool, not counting (`-vv`). Every modern CLI (rg, cargo, gh) has these. | polish | M | S | `coppa -q loopback hi` → `error: unexpected argument '-q' found`. `main.rs:23-28`. |
| 16 | Global `--verbose/--quiet` are rendered in the middle of every subcommand's option list (clap default ordering), splitting related flags. Use `display_order`/`next_display_order` or `help_heading`. | polish | L | S | `coppa tx --help`: `-o, --output … --verbose … --profile … --quiet … --callsign …`. |
| 17 | Long `about` strings for `rx` and `tune` bleed into the top-level command table (3-line rows). Use short `about` + `long_about`. | polish | M | S | `coppa --help`: `tune      Transmit a TX-level calibration ("TUNE") tone: standard SSB two-tone (700 Hz + 1900 Hz) by default, or a single tone via …`. `main.rs:64-65, 107-110`. |
| 18 | Duration flag naming is inconsistent: `listen -d/--duration <u64 s>`, `tune --seconds <f32>` (no short), `rx` has no duration at all. Standardise on `--duration` (accept `10s`/`500ms`) everywhere. | consistency | M | S | `coppa tune --duration 1 -o t.wav` → `error: unexpected argument '--duration' found`. |
| 19 | `rx` (streaming, hex, SNR, no duration) and `listen` (batch `core.decode` on a growing window, text, `--duration`, `--callsign`) are two different receivers with different engines and output formats. Users won't know which to use; `listen` re-decodes the whole window on every read (`main.rs:873`). Merge into one `rx`. | consistency | H | M | `coppa rx --help` vs `coppa listen --help`; `main.rs:576-652` vs `799-911`. |
| 20 | `config` "Show current configuration" actually lists profiles; there is no configuration (no config file, no env vars, no `~/.config/coppa`). Rename to `profiles` (or make `config` real). | consistency | M | S | `coppa config --help` → `Show current configuration`; `main.rs:144-149, 929-953`; `grep -n env main.rs` → only `temp_dir`. |
| 21 | Profile mental model: help says `HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY` but the engine's real knob is speed level 1–10 (8 reserved) and `rx --verbose` reports `level=1`. `--profile L5` / `--speed 5` are not accepted; `config` doesn't show the constellation/rate/bytes-per-frame table. Expose `--level N` and a `profiles` table with level, modulation, code rate, max bytes, air time. | consistency/docs | H | M | `coppa loopback Hello --profile L5` → `Unknown profile: L5…`; `speed_levels.rs:15-27`; `profiles.rs`. |
| 22 | `EMERGENCY` and `HF_ROBUST` are the same on the wire (level 1, no compression); differences (`max_payload`, `arq_window`) are unused by the CLI. Misleading choice. | consistency | L | S | `profiles.rs:29-60`; `EngineConfig::from_profile` copies only `speed_level/sample_rate/compression` (`config.rs:65-73`). |
| 23 | Empty message is accepted and produces a full 1.37 s frame of nothing. Reject `""` (or require `--allow-empty`). | polish | L | S | `coppa tx "" -o empty.wav` → `Encoding: ""` / `Generated 65520 audio samples` exit 0. |
| 24 | `--profile ""` gives `Unknown profile: .` — empty string should say "empty". | polish | L | S | `coppa tx hi -o z.wav --profile ""` → `Error: Unknown profile: . Available: …`. |
| 25 | `tune --seconds 0` writes a 0-sample WAV; `--single 0` and `--single 30000` (above Nyquist at 48 kHz) are accepted. Validate ranges (`--seconds > 0`, `100 ≤ f ≤ 3000` Hz for SSB). | bug | L | S | `coppa tune -o tune3.wav --seconds 0` → `Generated 0 audio samples` exit 0; `--single 30000 --seconds 1` → exit 0. |
| 26 | Negative numbers give a clap "unexpected argument '-1'" instead of a range error. Use `value_parser!(u64).range(1..)` / `allow_negative_numbers` + validation. | polish | L | S | `coppa tune -o t.wav --seconds -1` → `error: unexpected argument '-1' found`; `--ptt-lead-ms -5` same. |
| 27 | Output WAV is 32-bit float WAVE_FORMAT_EXTENSIBLE. Python's `wave`, older fldigi/soundmodem/Windows tools choke; ham convention is 16-bit PCM 48 k (the repo's own golden vectors are 16-bit PCM). Default to `pcm_s16le`, offer `--float`. | polish | M | S | `ffprobe cq.wav` → `pcm_f32le ([3][0][0][0] / 0x0003)`; `python3 wave.open("cq.wav")` → `wave.Error: unknown extended format: 00000003-…`; `file_backend.rs:127-132`; `testdata/golden/manifest.toml:3`. |
| 28 | `tx -o` to a bad path prints the error twice (`WavSink::drop` + `Error:`). | polish | L | S | `coppa tx hi -o /nonexistent/dir/x.wav` → `WavSink::drop: failed to flush samples to …` then `Error: Failed to create WAV file: …`. |
| 29 | `tx`/`tune` print "Encoding/Generated N samples" to **stdout** but the live-path "Transmitted N samples" to **stderr**; `Written to <path>` is stdout. Status chatter should be uniformly stderr so stdout is reserved for data. | consistency | L | S | `coppa tx hi -o z.wav --verbose 2>/dev/null` still shows `Encoding: "hi"` / `Generated …` / `Written to z.wav`; `main.rs:450-454, 416`. |
| 30 | No progress/feedback during long ops: `rx` on a 10-minute WAV is silent until the end; live `rx` has no liveness dot (only `listen` has one, every 5 s, as a bare `.`). No audio level/VU, no "carrier detected" indication. | missing-feature | M | M | `main.rs:685-723` (no progress); `main.rs:859-863` (listen's `.`). |
| 31 | `--version` is bare `coppa 0.1.0`: no git SHA, build date, enabled features (cpal/kiss-tnc/websocket), or wire-format/protocol version. Given the repo's documented wire-format breaks (`docs/adr/003`, `speed_levels.rs:57-63`) a protocol-version line is important for interop bug reports. | polish | M | S | `coppa --version` → `coppa 0.1.0`. |
| 32 | Feature-gated `tnc` subcommand vanishes without a trace when not compiled in; the user gets "unrecognized subcommand … similar: 'tune'". Keep the subcommand and error with "rebuild with `--features kiss-tnc`" instead. Same for `devices` without cpal ("(none found - CPAL backend may not be available)"). | polish | M | S | `coppa tnc --help` → `error: unrecognized subcommand 'tnc'` / `tip: a similar subcommand exists: 'tune'`; `main.rs:150-151`. |
| 33 | No-arg invocation prints a two-line banner and exits 0 instead of the help (cargo/gh/rg print usage, exit ≠ 0 or 0-with-help). Use `arg_required_else_help = true`. | polish | L | S | `coppa` → `Coppa - Ham Radio Digital Communications System` / `Use --help for available commands` `[exit=0]`. |
| 34 | `rx` requires `-i`; a bare positional path is rejected. `coppa rx cq.wav` is what everyone will type first (sox/ffmpeg/rg convention). Accept a positional input; `-` for stdin. | polish | M | S | `coppa rx cq.wav` → `error: unexpected argument 'cq.wav' found`. |
| 35 | No stdin/stdout streaming: `rx` can't read raw PCM from a pipe (direwolf `-`/`stdin`, ardopcf), `tx` can't write to stdout for `| aplay`/`| sox`. `RawF32Source/Sink` exist in `coppa-audio` but aren't exposed. | missing-feature | M | M | `file_backend.rs:22-34`; `main.rs` never references stdin. |
| 36 | No shell completions (`clap_complete`) and no man page (`clap_mangen`); neither crate is in `Cargo.lock`. | missing-feature | M | S | `grep clap_complete Cargo.lock` → nothing. |
| 37 | No colour or TTY detection: PASS/FAIL, WARNING, Error are plain text; no `NO_COLOR`/`--color` handling (harmless today, but the `WARNING:` lines about unkeyed TX deserve emphasis). | polish | L | S | `coppa --help \| grep -c $'\e'` → 0; no `colorchoice`/`anstyle` usage in `coppa-cli`. |
| 38 | No config file or env vars for the CLI (callsign, device, rigctld address, profile). Every invocation re-types `--device "Elgato" --ptt rigctld --rigctld host:4532 --callsign W5AU`. `coppad` has `coppad.toml`; the CLI should read the same `[engine]/[radio]/[audio]` sections and honour `COPPA_CALLSIGN` etc. | missing-feature | H | M | `main.rs` — no `env =` on any `#[arg]`, no file loading; `coppad.toml.example` exists. |
| 39 | Callsign is never validated (regex `^[A-Z0-9]{1,3}[0-9][A-Z0-9]{0,3}[A-Z](/[A-Z0-9]+)?$`-ish + uppercase) and never required for TX; a digital-mode TX tool should at least warn when transmitting live without an ID. | polish | M | S | `coppa tx hi --ptt rigctld …` live path has no callsign check; `main.rs:423-492`. |
| 40 | Unit naming mixes styles: `--ptt-lead-ms <u64>` (unit in name), `--seconds <f32>` (unit is the name), `--duration <u64>` (unit only in help), `--single <SINGLE>` (a frequency, value name says nothing). Adopt `--lead 50ms`-style humantime or at least consistent `<MS>`/`<HZ>`/`<SECS>` value names. | consistency | L | S | `coppa tune --help`: `--single <SINGLE> … frequency (Hz)`; `--seconds <SECONDS>`; `coppa tx --help`: `--ptt-lead-ms <PTT_LEAD_MS>`. |
| 41 | `devices` output gives no default-device marker, no index, no input/output split, and truncates rate to one "max" number; it doesn't say whether the device supports 48 kHz (what the engine needs). | polish | M | S | `coppa devices` → `  LC49G95T (in: 0ch, out: 2ch, max: 48000 Hz)` … `main.rs:913-927`. |
| 42 | `rx --raw` help says "(lowercase hex)" while `listen --raw` says "decoded text"; `loopback` prints the message in quotes; `rx` prints hex + SNR. Three output grammars for one concept. | consistency | M | S | `coppa rx --help` / `coppa listen --help` / `main.rs:782, 878`. |
| 43 | SNR reported for a heavily clipped input is `-23.8 dB` while the frame decodes perfectly — the number will confuse users calibrating levels (which is exactly when clipping happens). Either label it (`est. SNR`) or clamp/flag clipping (`CLIP` warning when |sample| ≥ 0.99). | polish | M | M | `ffmpeg -af volume=20` then `coppa rx -i clip.wav` → `Decoded: 4351… (SNR: -23.8 dB)`; `quiet.wav` (−34 dB) → `25.9 dB`. |
| 44 | `listen` help `--raw` says "Print only the decoded text" but with `--raw` it prints with `print!` (no newline) so consecutive frames run together. | bug | L | S | `main.rs:875-877`. |
| 45 | Getting-started doc is stale in three places: expected sample count `46080` (actual `65520`), `Written 46080 samples to cq.wav` (actual `Written to cq.wav`), `Reading from cq.wav` + quoted text (actual `Reading 65520 samples from…` + hex). It also says `file-backend` must be passed (it is a default feature). | docs | M | S | `docs/tutorials/getting-started.md:24-58` vs captures below. |
| 46 | README says `coppa config` shows "available operating profiles" and the status table says CLI is "Working — File-based I/O", never mentioning that decoded output is hex, that compression state must match, or that live `rx` exists. | docs | L | S | `README.md` "CLI Examples" and status table row `CLI (loopback, tx, rx)`. |
| 47 | Sibling binary `coppad` has no arg parser: `coppad --help` **starts the daemon** (treats `--help` as the config path, falls back to defaults, opens audio, runs until SIGTERM). `coppad --version` presumably the same. | bug | H | S | `./target/release/coppad --help` → `INFO coppad - Coppa Daemon starting version="0.1.0"` … `Daemon ready` (ran 3 h until killed); `coppad/src/main.rs:33-49`. |
| 48 | Global options accepted *after* the subcommand (`coppa loopback --quiet …`) and before; fine — but `coppa --help tx` shows the top-level help, not `tx` help. Minor clap default; document `coppa help tx`. | polish | L | S | `coppa --help tx` → top-level help. |
| 49 | `--rigctld` default `127.0.0.1:4532` is shown as `[default: …]` and repeated inside the help text for `--ptt` ("(default: "none") [default: none]") — duplicated. | polish | L | S | `coppa tx --help`: `--ptt <PTT>  PTT method: "rigctld", "vox", "none" (default: "none") [default: none]`. |
| 50 | Binary/subcommand naming vs ham convention: `tx`/`rx` are good; `loopback` should be `selftest`/`test`; `tune` collides with the ham meaning of *antenna* tune (ATU) — many users will expect it to key a carrier for the tuner, which it partly does; docs should say "level-set / two-tone" up front. | polish | L | S | `coppa --help` command list. |

## Suggested new commands

- **`coppa doctor`** (or `check`) — enumerate audio devices, confirm 48 kHz mono capture/playback on the chosen device, TCP-connect to rigctld and query `\get_ptt` without keying, dry-run the PTT lead/tail timing, print a pass/fail table. Most "it doesn't work" reports are audio/PTT plumbing; this is the highest-value addition.
- **`coppa profiles`** — replace `config` with a table: name, level, modulation, LDPC rate, max bytes/frame, compression, air time per frame; `--json`.
- **`coppa completions <shell>`** and a generated man page — zero-cost with `clap_complete`/`clap_mangen`.
- **`coppa chat --to <CALL>`** — keyboard-to-keyboard ARQ session over the existing `coppa-protocol` session/ARQ layer; this is what makes an HF modem *usable* without a host app.
- **`coppa send <file>` / `coppa recv [-o dir]`** — chunked file transfer with progress bar using the multi-codeword frames; the natural showcase for ARQ + compression.
- **`coppa beacon --interval 10m --callsign W5AU "…"`** — periodic ID/beacon with PTT sequencing; trivial on top of `tx`, and required by Part 97 §97.119 ID rules for unattended ops.
- **`coppa monitor`** — live terminal SNR/level meter + carrier-detect + decoded-frame log (ratatui); `rx --verbose` already has all the numbers.
- **`coppa tx --dry-run`** — encode, report air time / level / bytes, key nothing. Cheap, and prevents the "why did it TX" surprises.
- **`coppa version --verbose`** — features, git SHA, wire-format/protocol version, build target.
- **`coppa levels`/`calibrate rx`** — capture N seconds, report peak/RMS dBFS and clipping so the user can set the *input* level (the `tune` command only covers TX).
- **`coppa tnc`** should remain visible when not compiled in (error with rebuild hint), and `coppa daemon` could wrap `coppad` for a single entry point.

## Verbatim captures

### Top-level

```
$ coppa
Coppa - Ham Radio Digital Communications System
Use --help for available commands
[exit=0]

$ coppa --version
coppa 0.1.0
[exit=0]

$ coppa --help
Coppa - Ham Radio Digital Communications System

Usage: coppa [OPTIONS] [COMMAND]

Commands:
  tx        Encode and transmit a message
  rx        Receive and decode audio (streaming: WAV file if --input is given, otherwise live capture from an audio input device until Ctrl+C)
  loopback  Run a loopback test (encode -> decode)
  listen    Listen for incoming transmissions
  tune      Transmit a TX-level calibration ("TUNE") tone: standard SSB two-tone (700 Hz + 1900 Hz) by default, or a single tone via `--single`. Key this while advancing your radio's audio drive level until ALC just registers, then back off. See `docs/OPERATING.md`
  devices   List available audio devices
  config    Show current configuration
  help      Print this message or the help of the given subcommand(s)

Options:
      --verbose  Enable verbose output (SNR, sample counts, DSP diagnostics)
      --quiet    Suppress all output except decoded messages
  -h, --help     Print help
  -V, --version  Print version
[exit=0]
```

### Subcommand help

```
$ coppa tx --help
Encode and transmit a message

Usage: coppa tx [OPTIONS] <MESSAGE>

Arguments:
  <MESSAGE>  Message to transmit

Options:
  -o, --output <OUTPUT>            Output file for audio samples (WAV format)
      --verbose                    Enable verbose output (SNR, sample counts, DSP diagnostics)
      --profile <PROFILE>          Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY)
      --quiet                      Suppress all output except decoded messages
      --callsign <CALLSIGN>        Callsign for station identification
      --device <DEVICE>            Audio output device name (substring match)
      --ptt <PTT>                  PTT method: "rigctld", "vox", "none" (default: "none") [default: none]
      --rigctld <RIGCTLD>          rigctld address for CAT PTT (only used when --ptt rigctld) [default: 127.0.0.1:4532]
      --ptt-lead-ms <PTT_LEAD_MS>  Milliseconds to key PTT before audio starts playing [default: 50]
      --ptt-tail-ms <PTT_TAIL_MS>  Milliseconds to hold PTT keyed after audio finishes playing [default: 200]
  -h, --help                       Print help

$ coppa rx --help
Receive and decode audio (streaming: WAV file if --input is given, otherwise live capture from an audio input device until Ctrl+C)

Usage: coppa rx [OPTIONS]

Options:
  -i, --input <INPUT>      Input file containing audio samples (WAV format). If omitted, captures live audio from an input device instead
      --verbose            Enable verbose output (SNR, sample counts, DSP diagnostics)
      --profile <PROFILE>  Operational profile name
      --quiet              Suppress all output except decoded messages
      --raw                Print only the decoded payload (lowercase hex) with no labels
      --device <DEVICE>    Audio input device name (substring match, live capture only)
  -h, --help               Print help

$ coppa loopback --help
Run a loopback test (encode -> decode)

Usage: coppa loopback [OPTIONS] <MESSAGE>

Arguments:
  <MESSAGE>  Message to test

Options:
      --profile <PROFILE>  Operational profile name
      --verbose            Enable verbose output (SNR, sample counts, DSP diagnostics)
      --quiet              Suppress all output except decoded messages
  -h, --help               Print help

$ coppa listen --help
Listen for incoming transmissions

Usage: coppa listen [OPTIONS]

Options:
  -d, --duration <DURATION>  Duration in seconds (0 = indefinite) [default: 0]
      --verbose              Enable verbose output (SNR, sample counts, DSP diagnostics)
      --profile <PROFILE>    Operational profile name
      --quiet                Suppress all output except decoded messages
      --callsign <CALLSIGN>  Callsign for station identification
      --raw                  Print only the decoded text with no labels
      --device <DEVICE>      Audio input device name (substring match)
  -h, --help                 Print help

$ coppa tune --help
Transmit a TX-level calibration ("TUNE") tone: standard SSB two-tone (700 Hz + 1900 Hz) by default, or a single tone via `--single`. Key this while advancing your radio's audio drive level until ALC just registers, then back off. See `docs/OPERATING.md`

Usage: coppa tune [OPTIONS]

Options:
      --seconds <SECONDS>          Duration to key the tone, in seconds [default: 10]
      --verbose                    Enable verbose output (SNR, sample counts, DSP diagnostics)
      --quiet                      Suppress all output except decoded messages
      --single <SINGLE>            Transmit a single tone at this frequency (Hz) instead of the default two-tone signal, e.g. for power measurement with a wattmeter
  -o, --output <OUTPUT>            Output file for audio samples (WAV format) instead of live playback
      --profile <PROFILE>          Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY)
      --device <DEVICE>            Audio output device name (substring match)
      --ptt <PTT>                  PTT method: "rigctld", "vox", "none" (default: "none") [default: none]
      --rigctld <RIGCTLD>          rigctld address for CAT PTT (only used when --ptt rigctld) [default: 127.0.0.1:4532]
      --ptt-lead-ms <PTT_LEAD_MS>  Milliseconds to key PTT before audio starts playing [default: 50]
      --ptt-tail-ms <PTT_TAIL_MS>  Milliseconds to hold PTT keyed after audio finishes playing [default: 200]
  -h, --help                       Print help

$ coppa devices --help
List available audio devices

Usage: coppa devices [OPTIONS]

Options:
      --verbose  Enable verbose output (SNR, sample counts, DSP diagnostics)
      --quiet    Suppress all output except decoded messages
  -h, --help     Print help

$ coppa config --help
Show current configuration

Usage: coppa config [OPTIONS]

Options:
  -p, --profile <PROFILE>  Profile name to show
      --verbose            Enable verbose output (SNR, sample counts, DSP diagnostics)
      --quiet              Suppress all output except decoded messages
  -h, --help               Print help

$ coppa tnc --help
error: unrecognized subcommand 'tnc'

  tip: a similar subcommand exists: 'tune'

Usage: coppa [OPTIONS] [COMMAND]

For more information, try '--help'.
[exit=2]
```

### Happy paths

```
$ coppa loopback "Hello from Coppa"
Coppa Loopback Test
===================
Input: "Hello from Coppa"
Encoded: 65520 samples
Decoded: "Hello from Coppa"
PASS: Loopback test successful!
[exit=0]

$ coppa loopback "héllo wörld — 73 de W5AU ✓ 日本"
… Decoded: "héllo wörld — 73 de W5AU ✓ 日本"
PASS: Loopback test successful!
[exit=0]

$ coppa tx "CQ CQ CQ DE VK2ABC K" -o cq.wav
Encoding: "CQ CQ CQ DE VK2ABC K"
Generated 65520 audio samples
Written to cq.wav
[exit=0]

$ coppa rx -i cq.wav
Reading 65520 samples from cq.wav
Decoded: 435120435120435120444520564b32414243204b  (SNR: 25.7 dB)
[exit=0]

$ coppa rx -i cq.wav --raw
435120435120435120444520564b32414243204b
[exit=0]

$ coppa rx -i cq.wav --verbose
[verbose] sample_rate: 48000
Reading 65520 samples from cq.wav
Decoded: 435120435120435120444520564b32414243204b  (SNR: 25.7 dB)
[verbose] level=1 cfo_hz=0.0 frame_start=270
[exit=0]

$ coppa rx -i uni.wav        # tx "héllo ✓"
Reading 65520 samples from uni.wav
Decoded: 68c3a96c6c6f20e29c93  (SNR: 25.2 dB)

$ coppa rx -i two.wav        # ffmpeg concat of cq.wav + z.wav, pcm_s16le
Reading 131040 samples from two.wav
Decoded: 435120435120435120444520564b32414243204b  (SNR: 25.7 dB)
Decoded: 6869  (SNR: 25.6 dB)
[exit=0]

$ coppa rx -i cqstereo.wav   # 2-ch pcm_s16le
Decoded: 435120435120435120444520564b32414243204b  (SNR: 25.8 dB)

$ coppa config
Available profiles:
  HF_ROBUST - Robust HF mode for weak signals and high noise
  HF_STANDARD - Standard HF mode balancing speed and reliability
  VHF_FAST - Fast VHF mode for strong signals and low noise
  EMERGENCY - Emergency mode maximizing reliability
[exit=0]

$ coppa config -p HF_STANDARD
Profile: HF_STANDARD
  Description: Standard HF mode balancing speed and reliability
  Speed level: 2
  Max payload: 128 bytes
  ARQ window: 8
  Compression: true
  Sample rate: 48000 Hz
[exit=0]

$ coppa devices
Available audio devices:
  LC49G95T (in: 0ch, out: 2ch, max: 48000 Hz)
  Elgato XLR Dock (in: 1ch, out: 2ch, max: 96000 Hz)
  Arctis Nova Pro Wireless (in: 1ch, out: 2ch, max: 48000 Hz)
  Mac mini Speakers (in: 0ch, out: 2ch, max: 96000 Hz)
  Jump Desktop Microphone (in: 8ch, out: 8ch, max: 192000 Hz)
  Jump Desktop Audio (in: 8ch, out: 8ch, max: 192000 Hz)
[exit=0]

$ coppa tune -o tune.wav
Generating two-tone calibration signal: 700 Hz + 1900 Hz, 10.0s
Generated 480000 audio samples
Written to tune.wav
[exit=0]

$ coppa listen -d 2
Listening for 2 seconds...

Stopped listening.
[exit=0]

$ ffprobe cq.wav
  Stream #0:0: Audio: pcm_f32le ([3][0][0][0] / 0x0003), 48000 Hz, 1 channels (FL), flt, 1536 kb/s
```

### Errors and silent failures triggered

```
$ coppa loopback Hello --profile HF-ROBUST
Coppa Loopback Test
===================
Input: "Hello"
Error: Unknown profile: HF-ROBUST. Available: HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY
[exit=1]

$ coppa loopback "$(python3 -c 'print("A"*300)')"
…
Error: payload too large for this speed level (max 56 bytes)
[exit=1]
$ coppa --quiet loopback AAAA…(56)      → [exit=0]
$ coppa --quiet loopback AAAA…(57)      → Error: payload too large for this speed level (max 56 bytes) [exit=1]
$ coppa --quiet loopback AAAA…(255) --profile HF_STANDARD → [exit=0]   (compression makes it fit)
$ coppa --quiet loopback AAAA…(57) --profile EMERGENCY    → Error: payload too large … (max 56 bytes)

$ coppa loopback ""
…Input: ""  Encoded: 65520 samples  Decoded: ""  PASS: Loopback test successful!
[exit=0]

$ coppa --quiet --verbose loopback "both flags"
[exit=0]                                  (no output at all)

$ coppa -q loopback hi
error: unexpected argument '-q' found
Usage: coppa [OPTIONS] [COMMAND]
[exit=2]

$ coppa loopback hi --callsign W5AU
error: unexpected argument '--callsign' found
  tip: to pass '--callsign' as a value, use '-- --callsign'
[exit=2]

$ coppa rx cq.wav
error: unexpected argument 'cq.wav' found
Usage: coppa rx [OPTIONS]
[exit=2]

$ coppa rx -i /nonexistent/file.wav
Error: Failed to open WAV file: No such file or directory (os error 2)
[exit=1]

$ coppa rx -i notawav.wav
Error: Failed to open WAV file: Ill-formed WAVE file: no RIFF tag found
[exit=1]

$ coppa rx -i empty.bin
Error: Failed to open WAV file: Failed to read enough bytes.
[exit=1]

$ coppa rx -i noise48.wav           # 2 s gaussian noise, 48 k
Reading 96000 samples from noise48.wav
[exit=0]                                  (no "0 frames" summary)

$ coppa rx -i noise8k.wav           # 8 kHz WAV
Reading 16000 samples from noise8k.wav
[exit=0]

$ coppa rx -i cq44k.wav             # cq.wav resampled to 44.1 k
Reading 60197 samples from cq44k.wav
[exit=0]                                  (silent: no rate check)

$ coppa rx -i cq.wav --profile VHF_FAST
Reading 65520 samples from cq.wav
[exit=0]

$ coppa tx "CQ CQ DE W5AU" -o std.wav --profile HF_STANDARD
$ coppa rx -i std.wav
Reading 65520 samples from std.wav
Decoded: fe0a000000a007b3496692b45fc0f680  (SNR: 25.7 dB)     ← compressed bytes, not the message
[exit=0]
$ coppa rx -i std.wav --profile HF_STANDARD
Decoded: 43512043512044452057354155  (SNR: 25.7 dB)

$ coppa rx -i clip.wav              # ffmpeg volume=20 (hard clipped)
Decoded: 435120435120435120444520564b32414243204b  (SNR: -23.8 dB)

$ coppa tx hi -o cq.wav --ptt bogus
Encoding: "hi"
Generated 65520 audio samples
Written to cq.wav
[exit=0]

$ coppa tx hi -o cq.wav --callsign "not a callsign !!!"
…Written to cq.wav
[exit=0]

$ coppa tx hi -o cq.wav --device NoSuchDevice
…Written to cq.wav
[exit=0]

$ coppa tx hi --ptt rigctld --rigctld 127.0.0.1:1 --device NoSuchDevice --ptt-tail-ms 0
Encoding: "hi"
Generated 65520 audio samples
WARNING: rigctld connect failed (Failed to connect to rigctld at 127.0.0.1:1: Connection refused (os error 61)), using no PTT
WARNING: No output device matching 'NoSuchDevice', using default
Transmitted 65520 samples (1.36s)
[exit=0]                                  (played on the Mac speaker)

$ coppa tx hi -o /nonexistent/dir/x.wav
Encoding: "hi"
Generated 65520 audio samples
WavSink::drop: failed to flush samples to /nonexistent/dir/x.wav: Failed to create WAV file: No such file or directory (os error 2)
Error: Failed to create WAV file: No such file or directory (os error 2)
[exit=1]

$ coppa tx hi -o cq.wav --ptt-lead-ms -5
error: unexpected argument '-5' found
  tip: to pass '-5' as a value, use '-- -5'
[exit=2]

$ coppa tx hi -o z.wav --profile ""
Error: Unknown profile: . Available: HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY
[exit=1]

$ coppa config -p BOGUS
Unknown profile: BOGUS
Available: HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY
[exit=0]

$ coppa config HF_STANDARD
error: unexpected argument 'HF_STANDARD' found
[exit=2]

$ coppa devices --json
error: unexpected argument '--json' found
[exit=2]

$ coppa tune -o tune3.wav --seconds 0
Generating two-tone calibration signal: 700 Hz + 1900 Hz, 0.0s
Generated 0 audio samples
Written to tune3.wav
[exit=0]

$ coppa tune -o tune4.wav --single 0
Generating single-tone calibration signal: 0 Hz, 10.0s
Generated 480000 audio samples
[exit=0]

$ coppa tune -o tune5.wav --single 30000 --seconds 1
Generating single-tone calibration signal: 30000 Hz, 1.0s
Generated 48000 audio samples
[exit=0]

$ coppa tune -o tune2.wav --seconds -1
error: unexpected argument '-1' found
[exit=2]

$ coppa tune --duration 1 -o t.wav
error: unexpected argument '--duration' found
[exit=2]

$ coppa tune --seconds abc -o z.wav
error: invalid value 'abc' for '--seconds <SECONDS>': invalid float literal
[exit=2]

$ coppa tx hi --out x.wav
error: unexpected argument '--out' found
  tip: a similar argument exists: '--output'
Usage: coppa tx --output <OUTPUT> <MESSAGE>
[exit=2]

# SIGINT after 3 s (python subprocess, send_signal(SIGINT)):
coppa rx        -> exit=-2  stdout='Listening for live audio (Ctrl+C to stop)...\n' stderr=''
coppa listen    -> exit=-2  stdout='Listening indefinitely (Ctrl+C to stop)...\n'  stderr=''
coppa rx --raw  -> exit=-2  stdout='' stderr=''

# stream separation:
$ coppa rx -i cq.wav 2>/dev/null
Reading 65520 samples from cq.wav
Decoded: 6869  (SNR: 25.6 dB)
$ coppa rx -i cq.wav --verbose 1>/dev/null
[verbose] sample_rate: 48000
[verbose] level=1 cfo_hz=0.0 frame_start=270
$ coppa tx hi -o z.wav --verbose 2>/dev/null
Encoding: "hi"
Generated 65520 audio samples
Written to z.wav

# python wave module on coppa's output:
wave.Error: unknown extended format: 00000003-0000-0010-8000-00aa00389b71

# sibling binary:
$ ./target/release/coppad --help
2026-09-05T14:39:43Z  INFO coppad: coppad - Coppa Daemon starting version="0.1.0"
2026-09-05T14:39:43Z  WARN coppad: No callsign configured. Set [engine] callsign in config.
2026-09-05T14:39:43Z  INFO coppad: Daemon configuration loaded profile=HF_STANDARD sample_rate=48000 ptt_method=none
2026-09-05T14:39:43Z  INFO coppad: Audio input started sample_rate=48000
2026-09-05T14:39:43Z  INFO coppad: Audio output started sample_rate=48000
2026-09-05T14:39:43Z  INFO coppad: Daemon ready
… (ran until SIGTERM ~3 h later)
```
