# Pat Winlink session over coppa: verification record (COP-46)

This records the runs behind [docs/PAT.md](../PAT.md): real Pat completing a Winlink P2P
session through two real `coppad` processes. It is evidence about the modem's VARA-style TCP
interface, not about RF performance.

## Setup

- Date: 2026-10-27. One Linux x86_64 container, no sound card, no kernel sound modules, no
  radios.
- coppad: `cargo build --release -p coppa-daemon --bin coppad --features cpal-backend`, on the
  COP-46 branch (main at `65d1a3b` plus this change). Both stations `profile = "HF_STANDARD"`,
  `ptt_method = "none"`, configs as in docs/PAT.md with OS-picked ports.
- Audio: each coppad opens a real CPAL/ALSA device, an ALSA `file` PCM on the `null` device.
  `tools/pat-loop/relay.py` joins them at real time and feeds a keyed station silence
  (half-duplex).
- Pat v1.0.0, `pat_1.0.0_linux_amd64.tar.gz`, sha256
  `331e4700c9e8b44098d7173e448da53204f59720c6c19ebc903685666aa377f7` (checked against the
  digest GitHub publishes for the release asset). The planning run also used Pat v0.13.1,
  sha256 `904cf4369ffb8437868e84afb60db02dd603fed274a389cfc233042b9f242f0b`.

## Results

Each block is verbatim `tools/pat-loop/run.sh` output (scratch paths shortened).

Both directions, one-line body:

```text
$ tools/pat-loop/run.sh --direction both
pat-loop: Pat v1.0.0 (sha256 OK), coppad …/target/release/coppad
pat-loop: scratch /tmp/pat-loop.4vURNo, ports A=38959/38960 B=57351/57352
N1CALL->N0CALL  exit=0  elapsed=38s  body_match=yes  overflow_lines=0  keepalives=0
N0CALL->N1CALL  exit=0  elapsed=35s  body_match=yes  overflow_lines=0  keepalives=0
PASS
```

Connect took about 6 s (`Connecting to N0CALL` at 20:09:40, `Connected` at 20:09:46).

A multi-frame message: a 6000-byte body of seeded random words, which Pat proposed as
`FD EM WKHSUFMWRFIO 6407 3940 0` (6407 bytes, 3940 bytes compressed):

```text
$ tools/pat-loop/run.sh --direction a --body-bytes 6000
pat-loop: Pat v1.0.0 (sha256 OK), coppad …/target/release/coppad
pat-loop: scratch /tmp/pat-loop.eHF8c4, ports A=56795/56796 B=38185/38186
N1CALL->N0CALL  (6000 B body)  exit=0  elapsed=225s  body_match=yes  overflow_lines=0  keepalives=0
PASS
```

That is about 17.5 B/s of compressed message on air, and no station sent a KEEPALIVE while
sending its own data (the fix below).

Pat v0.13.1 as the dialer, with its legacy `host`/`cmdPort`/`dataPort` config, against the
Pat v1.0.0 listener (the header line names the listener's Pat):

```text
$ tools/pat-loop/run.sh --dial-pat …/pat_0.13.1_linux_amd64/pat --legacy-config --direction a
pat-loop: Pat v1.0.0 (sha256 OK), coppad …/target/release/coppad
pat-loop: scratch /tmp/pat-loop.qQIr9n, ports A=33891/33892 B=41575/41576
N1CALL->N0CALL  exit=0  elapsed=40s  body_match=yes  overflow_lines=0  keepalives=0
PASS
```

Pat 0.13.1 logged `got a vara command I wasn't expecting:` for `VERSION Coppa 0.2.0`, `WRONG`
and `SNR 26`, which is harmless. The planning run of the same pairing took 99 s, about 60 s of
it in Pat 0.13's close (it sent `DISCONNECT` twice); this run did not show that delay.

After each run, `ps` showed no leftover `coppad`, `pat` or `relay.py` process.

### Debug build: fails, as documented

```text
$ tools/pat-loop/run.sh --coppad target/debug/coppad --allow-debug --direction a --timeout 300
pat-loop: Pat v1.0.0 (sha256 OK), coppad …/target/debug/coppad
pat-loop: scratch /tmp/pat-loop.dMiAgs, ports A=46281/46282 B=53449/53450
N1CALL->N0CALL  exit=1  elapsed=34s  body_match=no  overflow_lines=3  keepalives=0
FAIL
```

Pat: `Unable to establish connection to remote: connect timeout`. Station B logged
`Audio input buffer overflow dropped=25408` and `dropped=33088` while it decoded, so it lost
the connect request. The planning run saw the same failure 3 times out of 3. Without
`--allow-debug` the harness refuses a `target/debug` coppad (exit 2).

## The keepalive collision (fixed in COP-46)

Before this change, `drain_session_tx_queue` never called `Session::touch()` when it sent
session data, although `touch()` is documented as "call when any PDU is sent or received".
So a station sending a long message reached the 30 s keepalive threshold during its own
transfer. The planning run, with a 6078-byte body before the fix:

- Run 1: `exit=124 elapsed=900s body_match=yes`. The body arrived, but `pat connect` never
  exited. A keyed a KEEPALIVE at 14:52:38.184, 110 ms after its last data frame released at
  14:52:38.074. B's `FF` was keyed 14:52:38.475–14:52:40.116 while A was transmitting, so A
  never heard it. Session data has no ARQ (H-025), so the `FF` was lost for good, and
  keepalives then held the dead session open.
- Run 2: `exit=0 elapsed=292s`, but A sent 8 keepalives during its own transfer (about every
  35 s) and B sent 0.
- With the fix prototyped: `exit=0 elapsed=273s`, 0 keepalives on both stations.

The fix is one `touch()` call after a session chunk is actually keyed. The unit test
`session_send_counts_as_activity` (`crates/coppa-daemon/src/event_loop.rs`) fails without it
(`a keyed session data frame must reset the keepalive timer`) and passes with it. The harness
now logs and counts `Sending session KEEPALIVE`; the 6000-byte run above shows 0.

## CI counterpart

`crates/coppa-daemon/tests/pat_vara_session.rs` runs in CI's full test job. It replays Pat-Vara
v1.2.0's command sequence (start burst, `LISTEN ON`, `WINLINK SESSION`, `CONNECT`, flush on
`BUFFER 0`, `DISCONNECT`) against two in-process `EventLoop` stations wired by the same
`vara_wiring::serve_vara` as `coppad`, in both roles. It runs in virtual time, so it cannot
catch a real-time decode stall like the debug-build failure; this harness does.

## Not run

- Real radios, real sound cards, RF, and `snd-aloop` (`hw:Loopback`). The container has no
  sound modules. Hardware runs belong to COP-20.
- arm64, macOS or Windows Pat; Pat 0.14.x; Pat older than 0.15 as a listener (not possible).
- Multi-message B2F sessions, a lossy or fading channel between the stations, Winlink CMS or
  telnet.
