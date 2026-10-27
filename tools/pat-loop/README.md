# pat-loop: real Pat against two coppad processes

`run.sh` runs a real Winlink P2P session: real Pat (its built-in `varahf` transport) dials one
release `coppad` with `pat connect varahf:///CALL`, and a second Pat, listening on the other
`coppad`, answers. The two coppads are joined by a userspace audio loop. It is the manual
end-to-end check for coppa's VARA-style TCP interface (COP-46); the operator guide is
[docs/PAT.md](../../docs/PAT.md).

## What it proves, and what it does not

It proves that Pat, two real `coppad` processes, their real CPAL/ALSA audio streams and the
modem in between complete a session: connect, a byte-exact message, and a clean disconnect,
in real time.

It does not cover RF, real radios, real sound cards or real radio timing (PTT delays, ALC,
fading). That is COP-20's hardware run. The audio loop adds no noise or fading. The only
loss it models is half-duplex: a frame sent while the other station is keyed is not heard.

## How it works

- `asoundrc.in` defines two ALSA devices, `coppa-loop-A` and `coppa-loop-B`, as ALSA `file`
  PCMs on the `null` device. They read and write four FIFOs in a scratch directory.
- `relay.py` copies the FIFOs at real time (48 kHz) and acts as the cable. It watches each
  station's VARA command port for `PTT ON`/`PTT OFF`, and feeds a keyed station silence, as a
  half-duplex radio is deaf while it transmits.
- `run.sh` writes a `coppad.toml` and a Pat `config.json` per station (the same values as
  docs/PAT.md, with ports picked by the OS), starts the relay and both coppads, runs the
  sessions and checks them.

Each process gets its own `HOME` and XDG directories under a `mktemp -d` scratch directory.
Pat's version reporting is disabled, and Pat's account-activation check is pre-seeded as
already confirmed, so Pat makes no Winlink network calls for the test callsigns. Both are
test-only settings.

## Requirements

- Linux x86_64 with `libasound2-dev` (to build coppad with `cpal-backend`), `python3`, `curl`.
- A **release** coppad:
  `cargo build --release -p coppa-daemon --bin coppad --features cpal-backend`.
  The script refuses a `target/debug` coppad unless `--allow-debug` is given: a debug build
  cannot decode in real time and is expected to fail.
- Network access once, to download Pat v1.0.0. The tarball is cached under
  `${XDG_CACHE_HOME:-~/.cache}/coppa-pat-loop/`. Its sha256 is pinned in `run.sh` and checked
  on every run before Pat is extracted; a mismatch stops the run before anything starts.

## Usage

```text
$ tools/pat-loop/run.sh --direction both
pat-loop: Pat v1.0.0 (sha256 OK), coppad /…/target/release/coppad
pat-loop: scratch /tmp/pat-loop.4vURNo, ports A=38959/38960 B=57351/57352
N1CALL->N0CALL  exit=0  elapsed=38s  body_match=yes  overflow_lines=0  keepalives=0
N0CALL->N1CALL  exit=0  elapsed=35s  body_match=yes  overflow_lines=0  keepalives=0
PASS
```

A row passes when Pat exits 0, the message filed in the listener's inbox matches the sent body
(CR stripped, since Pat stores CRLF), and neither coppad logged `Audio input buffer overflow`.
`keepalives` counts the session KEEPALIVE frames both coppads sent during the session; it is
reported, not judged. The script exits 0 when every row passed, 1 when any failed (it then
keeps the scratch directory and prints the log paths), and 2 on a setup error.

`run.sh --help` lists every option: `--pat`, `--dial-pat` (a different Pat for the dialer, for
example 0.13.1), `--legacy-config` (Pat 0.13.x's config form for the dialer), `--coppad`,
`--allow-debug`, `--direction a|b|both`, `--body-bytes N`, `--timeout SECS` and `--keep`.

Processes are stopped by PID when the script exits. It never uses `pkill -f`, whose pattern can
match the running shell's own command line.

The recorded runs are in [docs/testing/pat-session.md](../../docs/testing/pat-session.md).
