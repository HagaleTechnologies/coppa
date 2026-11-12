# LinBPQ as a coppa gateway

LinBPQ's `DRIVER=VARA` uses coppa's VARA-style TCP command/data interface.
Both RF stations must run coppa with the same profile. coppa does not
interoperate with VARA waveforms. This recipe tests LinBPQ's BBS as a B2F
peer and stores a message for local delivery. It does not configure an RMS
account, a Winlink CMS connection or the BBS's `RMS` forwarding user.

## Build

On x86-64 Linux, install Rust and the ALSA development package, then run:

```sh
cargo build --release -p coppa-daemon --bin coppad --features cpal-backend
```

Use a release build. See [Pat's build instructions](PAT.md#build) for the
debug-build audio-overflow failure and [OPERATING.md](OPERATING.md) for radio setup.

## Get LinBPQ

Download the unmodified Linux binary at
<https://www.cantab.net/users/john.wiseman/Downloads/linbpq> at test time.
The measured build is LinBPQ 6.0.26.1 October 2026. It is a 32-bit i386
program, even on x86-64. The no-root route used by the experiment downloads
three hash-checked Ubuntu noble release packages and unpacks them with
`dpkg-deb -x`. `tools/bpq-loop/bpq_loop.py` records their URLs and digests.
The binary URL is unversioned. The experiment prints the digest and warns
when it differs from the recorded build; `--expect-linbpq-sha256 HEX`
requires an exact match.

The alternative root route below was not run for COP-172:

```sh
sudo dpkg --add-architecture i386
sudo apt-get update
sudo apt-get install -y libc6:i386 zlib1g:i386 libgcc-s1:i386
```

No source, HTML templates or external program binaries are vendored.

## Configure coppad

Save this as `coppad.toml`, replacing the audio device and radio address.
LinBPQ must use the same rigctld for its scanner. Its data port is always
its configured command port plus one.

```toml
[audio]
sample_rate = 48000
input_device = "your radio interface"
output_device = "your radio interface"

[radio]
ptt_method = "rigctld"
rigctld_address = "127.0.0.1:4532"

[host]
bind_address = "127.0.0.1"
vara_enabled = true
vara_command_port = 8300
vara_data_port = 8301

[engine]
profile = "HF_STANDARD"
callsign = "N0GW"
```

## Configure LinBPQ

Save this block verbatim as `bpq32.cfg` in LinBPQ's working directory.
The lab rewrites only network port numbers. Change the example credentials
before exposing the telnet service beyond loopback.

```text
SIMPLE
NODECALL=N0GW
NODEALIAS=GWAY
LOCATOR=NONE
LINMAIL

RADIO 1
 HAMLIB 127.0.0.1:4532
 8,7.1,USB,VW
 8,14.1,USB,VW
****

PORT
 ID=Telnet Server
 DRIVER=TELNET
 CONFIG
 LOGGING=1
 HTTPPORT=8080
 TCPPORT=8010
 FBBPORT=8011
 MAXSESSIONS=10
 USER=user,password,N0GW,"",SYSOP
ENDPORT

PORT
 ID=coppa VARA-style
 DRIVER=VARA
 CONFIG
 ADDR 127.0.0.1 8300
 BW2300
 RETRIES 20
 RADIO 1
ENDPORT

APPLICATION 1,BBS,,N0GW-1
```

`NODECALL` identifies the node. `LINMAIL` starts the local BBS.
`RADIO 1` uses rigctld to alternate between 7.1 and 14.1 MHz at an 8 s dwell.
`DRIVER=TELNET` provides the sysop's login and message listing.
`DRIVER=VARA` connects to coppa's command port in `ADDR` and data port +1.
The second `RADIO 1` attaches that port to the scanner.
`APPLICATION 1,BBS,,N0GW-1` routes a call to N0GW-1 to the BBS.
The application number must match the configured BBS application.

LinBPQ sends `MYCALL N0GW N0GW-1`, so coppa answers both callsigns.
`MYCALLS N0GW-1` in the VARA CONFIG block is an alternative that restricts
the listening callsign. BPQ32 on Windows uses the same port block, but
Windows was not run. Keep a dwell of at least 7 s: five dwell intervals
cover coppa's 35 s worst-case inbound handshake timeout. Shorter dwells
can expire the scanner's six-interval PENDING hold first.

## Gateway commands

| Command | Reply and effect |
|---|---|
| `MYCALL <call> [<call> ...]` | `OK` for one to five valid callsigns; first identifies transmissions, all answer calls. An invalid list gets `WRONG` and keeps the previous list. |
| `BW2300` | `OK`; accepted but does not change the configured profile. |
| `RETRIES 20`, `RETRIES=20` | `OK`; this client's next dial keys up to 21 requests. Valid range 0-20; default 0. |
| `LISTEN ON` | `OK`; answer incoming requests. |
| `LISTEN OFF` | `OK`; stop listening and end a pending inbound attempt with `CANCELPENDING`. Established sessions and outbound dials continue. |
| `CONNECT <source> <dest>` | `OK`; dial destination using the first MYCALL. |
| `DISCONNECT` | `OK`; drain queued data, then end the session. |
| `ABORT` | `OK`; end the session immediately. |
| `BUSY` | `BUSY ON` or `BUSY OFF`, a reliable reply. |
| `CALLTIMEOUT 600`, `PENDING`, `CANCELPENDING` | `WRONG`; these are not host commands. PENDING and CANCELPENDING are pushed notices. |

RETRIES settings belong to a command connection and reset on reconnect.
Each dial snapshots the setting. Requests are separated by at least
`max(2 * request airtime + 2 * turnaround, 5 s)` measured from keying.
At the current default HF_STANDARD level this reaches the 5 s floor.
The initial 2.730 s TX includes a station ID; the request itself is 1.365 s. A dial ends 30-35 s after
its last request. Repeats stop after 300 s regardless of retry count.
LinBPQ's black-box dial wait was at least 416 s, so CALLTIMEOUT is unnecessary.
The first 64 distinct unknown command words are logged once per client at INFO;
further unknown-word logs are suppressed to bound memory. All still get WRONG.
`IAMALIVE` continues every 60 s. With the default `busy_hold_ms = 0` there
is no busy-channel wait; a nonzero station-ID busy hold can delay commands
while a transmission waits, as described in [HOST-API.md](HOST-API.md).

## Run a session

Start rigctld, `coppad --config coppad.toml`, then LinBPQ in the directory
containing `bpq32.cfg`. A second coppa station can call N0GW-1 with Pat.
Compose a message addressed to N0GW, then `pat connect varahf:///N0GW-1`.
The gateway pushes PENDING, then CONNECTED with the dialled application
callsign. LinBPQ parks the scanner through DISCONNECTED and resumes about
15 s later. Check the BBS log, `Mail/m_*.mes` and the sysop's `L` listing.
A message for another callsign may be held when no forwarding route exists.
LinBPQ also holds mail from new users by default. The lab creates a minimal
`linmail.cfg` with `DontHoldNewUsers = 1`, `MaxRXSize = 99999` and
`MaxTXSize = 99999`, plus the BBS name and application number above.
This is for synthetic local callsigns only. A real gateway sysop chooses
its user-hold policy through the normal BBS administration interface.
[LinBPQ's mail configuration guide](https://www.cantab.net/users/john.wiseman/Documents/MailServerConfiguration.html)
describes the hold flag. The lab logs in to the BBS with the destination
callsign, answers the first-login name prompt, and checks `L`.

## Try it without a radio

This no-root experiment uses Python 3's standard library, bash, curl,
`dpkg-deb`, and sha256sum. It runs LinBPQ, Pat v1.0.0 and two release
coppad processes on userspace ALSA file PCMs and a half-duplex audio cable.
It uses a stand-in rigctld and a tmpfs LinBPQ working directory under
`/dev/shm` because i386 libc cannot stat inode numbers above 2**32 on XFS.
No sound card, radio, root or Docker daemon is needed.
The lab uses a 16384-sample receive ring to absorb process scheduling and
decode stalls; this also avoids the legacy Pat lab's zero-overflow check
failing after a successful message transfer with the smaller default ring.

```sh
tools/bpq-loop/run.sh --keep
```

It prints one result per session, stall and restart scenario, then PASS.
Exit 1 means a failed check; exit 2 means setup failed. `--help` works offline.
A cache stores downloaded programs and packages. Use `--cache DIR` for a
fresh download. See the [run record](testing/bpq-session.md) for actual
results and the clean-container reproduction commands.

## Details and limits

PENDING stops scanning; CONNECTED keeps it stopped until DISCONNECTED.
CANCELPENDING releases an incomplete attempt. If the scanner leaves early
with LISTEN OFF, coppa cancels the pending attempt without transmitting on
the frequency the scanner has left. On reconnect LinBPQ resends its startup
block. If the gateway dies during a session, Pat can wait several minutes
for dead-link detection before a later connection succeeds. An idle link
with no outstanding data may keep sending keepalives indefinitely after
its peer dies; explicitly ABORT it before redialling. The restart lab
interrupts an outstanding B2F write.
RETRIES applies only to VARA-style dials; a Pat caller defaults to one request.
RF, Raspberry Pi, BPQ32, a physical rig and CMS forwarding remain untested.

## Troubleshooting

- `Application not available`: match the application number to the BBS.
- Audio input overflow: use a release build.
- `cannot execute: required file not found`: install or unpack the i386 loader.
- `Value too large for defined data type`: use a tmpfs working directory for LinBPQ.
- A call times out: check LISTEN ON, every MYCALL, both profiles and audio routing.

## How this was verified

See [testing/bpq-session.md](testing/bpq-session.md). Work was clean-room,
using this repository, the ticket, public VARA command documentation and
LinBPQ's public configuration documentation. LinBPQ ran as an unmodified
black box. No LinBPQ source, HTML templates or external modem code was read
or copied. The [clean-room policy](DECISIONS/2026-11-09-clean-room-and-licensing-policy.md)
governs this boundary.
