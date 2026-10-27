# Winlink with Pat over coppa

## What this is for

[Pat](https://getpat.io) is a Winlink client. Its built-in `varahf` transport talks to a modem
over VARA's TCP control interface: a command port and a data port. `coppad` serves a
VARA-style TCP interface on the same two ports, so Pat can use coppa as its modem for Winlink
peer-to-peer (P2P) sessions between two coppa stations.

coppa is **not** RF- or waveform-compatible with VARA. Only the TCP control interface is
VARA-style, so both ends of the radio link must run coppa. A VARA station, or a Winlink RMS
gateway running VARA, cannot hear a coppa station.

This guide was verified with real Pat v1.0.0 against two real `coppad` processes on one Linux
machine, joined by a userspace audio loop (see [How this was verified](#how-this-was-verified)).
It was not run over real radios.

## Build

Build coppad **in release mode** with live audio:

```bash
sudo apt install libasound2-dev
cargo build --release -p coppa-daemon --bin coppad --features cpal-backend
```

A debug build (`cargo build --workspace`) decodes too slowly for real-time audio. It drops
input while decoding (`Audio input buffer overflow dropped=…` in coppad's log), misses the
connect request, and Pat reports `Unable to establish connection to remote: connect timeout`.

Pat itself comes from its [releases page](https://github.com/la5nta/pat/releases). Pat 1.0.0
was tested; Pat 0.15 or newer is needed to *listen* for calls (see
[Details and limits](#details-and-limits)).

## Configure coppad

Each station runs its own `coppad` with its own config file. Station B's file is the same as
station A's except for the three marked lines.

```toml
# station-a.toml (N1CALL). Station B is identical except the three marked lines.
[audio]
sample_rate = 48000
input_device = "coppa-loop-A"     # B: "coppa-loop-B" (your radio's sound card in real use)
output_device = "coppa-loop-A"    # B: "coppa-loop-B"

[radio]
ptt_method = "none"               # real radio: "rigctld", see docs/OPERATING.md

[host]
bind_address = "127.0.0.1"
vara_enabled = true
vara_command_port = 8300          # B: 8400. Pat's `addr` config uses command port + 1 for data,
vara_data_port = 8301             # B: 8401. so keep data = command + 1.

[engine]
profile = "HF_STANDARD"           # both stations MUST use the same profile
callsign = "N1CALL"               # B: "N0CALL"
```

- `callsign` is required: coppad refuses to transmit without a valid one.
- Both stations must use the same `profile`. Pat sends `BW2300` (or `BW500`/`BW2750`), and
  coppad accepts it but does not act on it, so the waveform is chosen here, not by Pat.
- Pat's current config form (`addr`) derives the data port as the command port + 1. Keep
  `vara_data_port` one above `vara_command_port`.
- On one machine the two stations need different ports. With two machines both can keep
  8300/8301.

## Configure Pat

Pat reads `~/.config/pat/config.json` by default, or the file passed with `--config`. Only the
keys below matter here; Pat fills in the rest from its defaults.

Station A (N1CALL), the dialer:

```json
{
  "mycall": "N1CALL",
  "listen": [],
  "varahf": { "addr": "localhost:8300", "bandwidth": 2300 }
}
```

Station B (N0CALL), the listener:

```json
{
  "mycall": "N0CALL",
  "listen": ["varahf"],
  "varahf": { "addr": "localhost:8400", "bandwidth": 2300 }
}
```

`varahf.addr` is coppad's **command** port. Pat 0.13.x does not know `addr`; it uses the older
form, which was verified for dialing only:

```json
"varahf": { "host": "localhost", "cmdPort": 8300, "dataPort": 8301, "bandwidth": 2300 }
```

## Run a session

Start both coppads, then Pat on the listening station, then compose and dial on the other:

```text
# terminal 1 and 2: the two coppads
$ ./target/release/coppad --config station-a.toml
$ ./target/release/coppad --config station-b.toml

# terminal 3: Pat on B, listening
$ pat --config b.json --listen=varahf http
2026/10/27 20:09:40 Listening for incoming traffic on varahf...
2026/10/27 20:09:40 VARA modem (Coppa 0.2.0) initialized

# terminal 4: Pat on A, compose then dial
$ echo "Hello N0CALL from N1CALL" | pat --config a.json compose --p2p-only -s "coppa test" N0CALL
Message posted
$ pat --config a.json connect varahf:///N0CALL
2026/10/27 20:09:40 VARA modem (Coppa 0.2.0) initialized
2026/10/27 20:09:40 Connecting to N0CALL (varahf)...
2026/10/27 20:09:46 Connected to N0CALL (vara)
;FW: N0CALL
[Pat-1.0.0-B2FHMG$]
; N1CALL DE N0CALL ()>
>FD EM LXOTUKICBG5P 366 282 0
>F> 4B
FS +
Remote accepted LXOTUKICBG5P
Transmitting [coppa test] [offset 0]
FF
>FQ
2026/10/27 20:10:18 Disconnected.
```

`--p2p-only` keeps the message off the Winlink CMS network: it is delivered only over a P2P
connection such as this one. On B, Pat's log shows the matching receive side and files the
message in B's inbox:

```text
2026/10/27 20:09:48 Got connect (varahf:N1CALL)
2026/10/27 20:09:48 Connected to N1CALL (vara)
;FW: N1CALL
[Pat-1.0.0-B2FHMG$]
; N0CALL DE N1CALL ()
FD EM LXOTUKICBG5P 366 282 0
F> 4B
1 proposal(s) received
Accepting LXOTUKICBG5P
>FS +
Receiving [coppa test] [offset 0]
>FF
FQ
2026/10/27 20:10:19 Disconnected.
```

These lines are from the verification run (Pat's progress and `GZIP_EXPERIMENT` lines left
out, subject shortened). Either station can dial; swap the roles by starting the listener on A.

On a real radio, set `[radio]` for your PTT (`ptt_method = "rigctld"`, see
[OPERATING.md](OPERATING.md)) and point `[audio]` at the radio's sound card. That setup has not
been run with Pat yet.

## Try it without a radio

`tools/pat-loop/run.sh` runs the whole session above on one Linux machine, with no root, no
sound card and no network except one pinned Pat download:

```bash
cargo build --release -p coppa-daemon --bin coppad --features cpal-backend
tools/pat-loop/run.sh --direction both
```

It prints one line per session and `PASS` or `FAIL`. See
[tools/pat-loop/README.md](../tools/pat-loop/README.md).

For an interactive loop with your own Pat, the `snd-aloop` recipe in [TNC.md](TNC.md) gives two
named loopback devices. It has not been tested with coppad and Pat. Note that `snd-aloop` does
not model a half-duplex radio, which is more forgiving than a real one.

## Details and limits

Each of these was observed in the verification runs.

- **`BW` and `COMPRESSION` are accepted and ignored.** Match `[engine] profile` on both
  stations yourself.
- **Data port = command port + 1.** Pat has no setting for the data port in its current config
  form; only the Pat 0.13.x legacy form sets `dataPort` itself.
- **Pat 0.15 or newer is needed to listen.** VARA listening first appears in Pat 0.15.0. Pat
  0.13.1 dials fine with the legacy config. Its close can take about 60 s longer (one run saw it
  send `DISCONNECT` twice), and it logs harmless `got a vara command I wasn't expecting` lines
  for coppad's `WRONG`, `SNR` and `VERSION` replies. With the current config form, Pat 0.13.1 fails
  with `'varahf' is not a valid listen method`.
- **Connected-session data has no ARQ** (H-025). On a lossy channel a lost frame is not resent,
  and the session stalls until Pat's own timers give up.
- **`DISCONNECT` discards data still queued in coppad** (H-032). Pat flushes (waits for
  `BUFFER 0`) before it closes, so a normal Pat session is not affected.
- **Throughput is low.** At `HF_STANDARD`, a 6000-byte message (3940 bytes after Pat's
  compression) took 225 s, about 17 B/s.
- **Station identification.** The first transmission, and every one after `id_interval_secs`,
  carries a station-ID frame, which adds about 1.4 s.
- **PTT.** With `ptt_method = "none"` nothing keys a radio. For a real radio use rigctld, as in
  [OPERATING.md](OPERATING.md).

## Troubleshooting

- **`Unable to establish connection to remote: connect timeout`.**
  - Look for `Audio input buffer overflow` in either coppad's log. If it is there, you are
    probably running a debug build: rebuild with `--release`.
  - Check that both stations use the same `[engine] profile`.
  - Check that each `vara_data_port` is its `vara_command_port` + 1.
- **Pat 0.13: `'varahf' is not a valid listen method`.** Use the legacy `host`/`cmdPort`/
  `dataPort` config to dial, or upgrade Pat to listen.
- **Pat asks about Winlink account activation and waits.** Before `connect`, Pat checks the
  *dialing* callsign's Winlink account over the internet. An unregistered callsign gets an
  interactive prompt. A licensed operator with a Winlink account never sees it; for a test
  callsign, answer `c` to continue. This happens with every Pat transport, not just coppa.

## How this was verified

[docs/testing/pat-session.md](testing/pat-session.md) records the runs behind this guide: Pat
versions and checksums, the harness commands and their output, the debug-build failure, and
what was not run. CI runs `crates/coppa-daemon/tests/pat_vara_session.rs`, which replays Pat's
VARA command sequence against two in-process stations on every push.
