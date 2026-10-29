# Operating Coppa

## What coppad reports at startup

`coppad`'s first log line names the build, and just before `Daemon ready` (or
`Daemon running degraded`) it states what startup actually did: the host
listeners, the audio devices it opened, and the PTT backend in control. A
build without `cpal-backend`, with the default config, logs:

```
INFO coppad: coppad - Coppa Daemon starting version="0.3.0" git_sha="f1dcac4a6ac6" features=none
INFO coppad: Daemon configuration loaded profile=HF_STANDARD sample_rate=48000 ptt_method=none turnaround_ms=500
INFO coppad: Host listeners bind_address=127.0.0.1 vara_command=disabled vara_data=disabled websocket=not-built
WARN coppad: Audio devices: none; this coppad was built without the cpal-backend feature, so it can neither receive nor transmit
INFO coppad: PTT method="none" active=none state="not controlled" guardian="not used"
INFO coppad: Daemon ready
```

A `cpal-backend,websocket` build with rigctld PTT and the VARA and WebSocket
servers on logs, before `Daemon ready`:

```
INFO coppad: Audio input started device="Default Audio Device (alsa:default)" sample_rate=48000
INFO coppad: Audio output started device="Default Audio Device (alsa:default)" sample_rate=48000
INFO coppad: Host listeners bind_address=127.0.0.1 vara_command=127.0.0.1:8300 vara_data=127.0.0.1:8301 websocket=127.0.0.1:8400
INFO coppad: Audio devices input="Default Audio Device (alsa:default)" output="Default Audio Device (alsa:default)" sample_rate=48000
INFO coppad: PTT method="rigctld 127.0.0.1:4532" active=rigctld state="released" guardian="running, pid 28849"
```

| Line | Field | Values |
|---|---|---|
| `Coppa Daemon starting` | `version` | The `coppad` version. |
| | `git_sha` | The commit HEAD pointed at when `coppad` was built (12 hex digits), or `unknown` for a build from a tree without `.git`. Packagers building from a tarball set `COPPA_GIT_SHA` (7-40 hex digits) at build time. It does not flag uncommitted changes. |
| | `features` | `coppa-daemon`'s cargo features in this build (`cpal-backend`, `gpio-ptt`, `kiss-tnc`, `serial-ptt`, `websocket`), comma-separated, or `none`. |
| `Host listeners` | `bind_address` | `[host] bind_address`. |
| | `vara_command`, `vara_data`, `websocket` | The bound `ip:port` (the real port when 0 is configured), `disabled`, or, for `websocket` only, `not-built` in a build without the `websocket` feature. |
| `Host listeners accept connections from other hosts; ...` (WARN) | `bind_address` | Logged when a listener is bound to a non-loopback address such as `0.0.0.0`: anyone who can reach it can key the transmitter. |
| `Audio devices` | `input`, `output` | The device cpal opened, as its name and cpal id, for example `Default Audio Device (alsa:default)`. On ALSA, `default` is whatever PulseAudio, PipeWire or `.asoundrc` route it to; coppad cannot see further. A named device shows its own description and id. |
| `Audio devices: none; ...` (WARN) | | A build without `cpal-backend`: the daemon runs but moves no audio. |
| `PTT` | `method` | The configured method: `none`, `vox`, `rigctld <address>`, `serial <port> <dtr\|rts>` or `gpio <pin>`. |
| | `active` | The backend actually in control: `none`, `vox`, `rigctld`, `serial` or `gpio`. An unreachable rigctld shows `active=none`. |
| | `state` | `released` (the startup release succeeded), `release failed: <error>`, or `not controlled` (none, VOX, or an unreachable rigctld, where coppad drives no PTT line). |
| | `guardian` | `running, pid <n>`, `failed to start: <error>`, `off ([radio] ptt_guardian = false)`, or `not used`. See [The PTT guardian](#the-ptt-guardian). |

The `PTT` line is a WARN when PTT keeps the daemon from calling itself ready.
`--log-level warn` hides the INFO summary lines but not the WARN ones. Set
`NO_COLOR=1` when the log goes to a file or the journal, so each key stays next
to its `=`.

## TX level calibration (TUNE)

Before transmitting real traffic, calibrate your radio's audio drive level so it
runs clean — this is standard SSB digital-mode practice (the same procedure
you'd use with any VARA-style TNC or FT8/PSK31 sound-card app), and matters
even more for an OFDM waveform, where over-drive intermodulation splatters
across all carriers at once and wrecks decode on every one of them.

1. Set your radio to **USB** (or **CW**, if your rig's CW audio input bypasses
   speech processing) — *not* a voice/compression mode, and disable any
   speech processor.
2. Set your radio's ALC display or meter where you can see it, and back your
   transmit audio (your sound card's output level in the OS mixer; Coppa
   itself has no TX gain setting) all the way down.
3. Key the calibration tone: `coppa tune` from the CLI (device and PTT settings
   come from the same `coppad.toml`; default 10 seconds,
   `--seconds N` to change it), or `TUNE` (or `TUNE <seconds>`) on the daemon's
   VARA command port. This transmits a standard two-tone signal (700 Hz +
   1900 Hz, equal amplitude) — the same drive-level convention hams use for
   SSB two-tone tests.
4. Slowly advance your audio drive level until the ALC meter *just* begins to
   register (first needle movement / first LED).
5. Back off slightly from that point — a few dB is enough. ALC actively
   compensating (rather than merely twitching) means you're already
   over-driving and generating splatter.
6. For a wattmeter power reading instead of an ALC check, use
   `coppa tune --single 1500` — a two-tone signal's fluctuating envelope makes
   a peak-power reading ambiguous, so use the single-tone variant for that.

Re-run this any time you change antennas, radios, or soundcard levels.

## Where to tune: bandwidth and IARU Region 1 placement

Dial frequency places coppa's carrier grid from 350 Hz above the dial edge.
Profile widths, in both conventions the docs quote (edge-to-edge carrier
span / the `occupied_bandwidth_hz` figure the daemon reports):
hf_standard-class 2350/2400 Hz, hf_narrow 450 Hz, hf_wide 2450/2500 Hz
(`crates/coppa-engine/src/engine.rs:188-212`). All HF profiles are under
47 CFR 97.307(f)(3)'s 2.8 kHz authorized bandwidth (the hard limit
`coppad`'s band gate enforces).

IARU Region 1 band plans are voluntary recommendations (national rules
bind; in the US that is the 2.8 kHz figure above). A 2.45 kHz-wide hf_wide
signal fits bandwidth-wise in the 2700 Hz "all modes – digimode" segments
(e.g. 80 m 3600–3620, 40 m 7050–7053, 20 m 14101–14112, 15 m 21110–21120,
10 m 28300–28320 and 29200–29300 kHz) and does not belong in 500 Hz
digimode segments (all of 30 m, 10140–10150 kHz; the PSK/FT8 neighborhoods
on 80/40/20/15/10 m, e.g. 20 m 14070–14099 kHz). 40 m is the tight fit:
7050–7053 leaves roughly 275 Hz of margin per side (7040–7047 is a 500 Hz
narrowband-digimode segment, not a wideband one). hf_wide occupies the
same segments as wide-class VARA signals, not the narrow-digimode
convention frequencies. Always check the current IARU R1 band plan for
segment edges before settling on a frequency.

## PTT safety when coppad stops

coppad releases PTT when it stops, including in the middle of a
transmission. What does the release depends on how it stops:

| How coppad stops | What releases PTT |
|---|---|
| `systemctl stop`, SIGTERM or Ctrl-C (SIGINT) | coppad, before it exits. The log shows `Releasing PTT before shutdown`, and VARA hosts see `PTT OFF`. |
| An error or a panic | The PTT backend, as coppad unwinds. |
| coppad keeps running, but the task that ends a transmission has died | A check that runs every 500 ms. Once PTT has been keyed for `max_tx_duration_s` plus 2 s, it forces PTT off and logs `PTT backstop: keyed past max_tx_duration_s plus grace with no release; forcing PTT off`. |
| `kill -9`, an out-of-memory kill or a crash, with rigctld or GPIO PTT | The PTT guardian (below). |
| The same, with serial PTT | The operating system, which drops DTR/RTS when it closes the port. This has not been checked on real hardware. |
| Restart after any of the above | coppad releases PTT at startup (rigctld `T 0`, or the serial/GPIO line lowered). If that fails, it logs `Could not release PTT at startup; ...`, keeps running, and logs `Daemon running degraded` with `PTT release at startup failed: ...` in place of `Daemon ready`. |

The KISS TNC (`coppad --tnc`, `coppa tnc`) also releases PTT on SIGTERM and
Ctrl-C.

### The PTT guardian

With `ptt_method = "rigctld"` or `"gpio:<pin>"`, coppad starts a helper
process, `coppad --ptt-guardian ...`, and logs:

```
INFO coppad: PTT guardian started pid=6441
```

coppad tells the guardian each time it keys and releases PTT. If coppad exits
while PTT is keyed, however it died, the guardian opens its own rigctld
connection or GPIO pin, releases PTT and exits:

```
WARN coppad::ptt_guardian: coppad exited while PTT was keyed; releasing it ptt_method=rigctld
INFO coppad::ptt_guardian: PTT released
```

If every release attempt fails, it logs `Could not release PTT after coppad
exited; the transmitter may still be keyed` and exits with status 1. When
coppad exits with PTT released, the guardian exits without touching the rig.

The guardian runs in its own process group, so a SIGKILL sent to coppad's
process group does not reach it. It is not started for `none`, `vox` or
serial PTT, nor when rigctld was unreachable at startup. To turn it off:

```toml
[radio]
ptt_guardian = false
```

If the guardian cannot be started, coppad logs `Could not start the PTT
guardian; ...` and runs without it. If the guardian stops while coppad is
running, coppad logs `PTT guardian is no longer running; ...` once.

To run coppad as a systemd service without breaking the guardian, use the
shipped unit; see
[tutorials/raspberry-pi-install.md](tutorials/raspberry-pi-install.md).

### What is not covered

Nothing in coppad can release PTT in these cases:

- A SIGKILL to every process in coppad's cgroup, which kills the guardian too.
  `pkill -9 coppad` does this, because the guardian is also named `coppad`. So
  does systemd when a stop takes longer than `TimeoutStopSec` under the default
  `KillMode=control-group`. The shipped `contrib/coppad.service` sets
  `KillMode=process`, so its timeout kills coppad alone and the guardian still
  releases PTT.
- Power loss.
- Serial PTT, where release depends on the operating system dropping the line
  when the port closes. This has not been checked on real hardware.
- `ptt_guardian = false`, after `kill -9`, an out-of-memory kill or a crash.
- A coppad that is still running but completely stuck (for example,
  deadlocked) while PTT is keyed.
- `coppa tx` and `coppa tune` stopped with Ctrl-C. Errors and panics in them
  do release PTT.

Turn on your rig's TX time-out timer (TOT), set a little longer than
`max_tx_duration_s`. It is the only protection that works in every case
above.

### What Coppa sends

Every transmission leaves Coppa at a fixed **-6.02 dBFS** digital peak: the
modulator peak-normalizes each frame, and nothing applies gain after that. The
two-tone tune signal is -12.0 dBFS RMS. HF data frames run about -16.6 to
-21.1 dBFS RMS, depending on speed level. So a wattmeter shows lower average
power on data than on the tune tone. That is expected, and it is not a reason
to turn the drive up.

The daemon logs the real figures for every transmission:

```text
INFO coppad::event_loop: TX audio level peak_dbfs=-6.0 rms_dbfs=-16.8 duration_ms=1365
```

It cannot see your OS mixer or your radio's ALC, which is where over-drive
actually happens. Steps 1-6 above remain the only drive-level check. If the
daemon ever logs `TX audio is clipping before it leaves Coppa`, that is a Coppa
bug, not a level setting; please report it.

## RX level

`coppad` meters its raw receive audio every 100 ms and logs a line only when
the level state changes. A healthy input that starts OK and stays OK prints
nothing.

| Log line | Meaning | What to do |
|---|---|---|
| `WARN ... RX audio input is clipping: turn down the radio's audio output or the sound card's input gain` | Samples are reaching full scale | Turn the radio's audio output or the sound card's input gain down |
| `WARN ... RX audio input level is too low: turn up the radio's audio output or the sound card's input gain` | The input is close to the sound card's noise floor | Turn the radio's audio output or the sound card's input gain up |
| `WARN ... RX audio input is silent: check that the sound card input is connected and not muted` | The input is digital silence | Check the cable, the input device and the OS mixer's mute |
| `INFO ... RX audio input level OK` | The input has recovered | Nothing |

The thresholds:

- **Clipping:** any sample at or above 0.99 of full scale (-0.09 dBFS) in two
  consecutive 100 ms windows. A single pop, such as a T/R relay, does not warn.
  The warning clears after 3 s of clean input.
- **Too low:** RMS below -60 dBFS for 5 s. It clears after 1 s at or above
  -57 dBFS.

Both thresholds are reasoned from simulation, not yet checked against real
radios.

To set the RX level: put the radio on a quiet band and set its audio output
and the sound card's input gain so that no warning appears. As a rough target,
idle band noise should sit well above -60 dBFS RMS, and strong signals should
not reach `clipping`. The decode SNR is not a level indicator: a badly clipped
input can still decode with a misleading SNR.

The meter is paused while Coppa is transmitting, because the input then carries
the radio's muted output or its own monitor, not the band.

## Watching levels live

On a daemon built with the `websocket` feature, set
`websocket_enabled = true` and `websocket_token` under `[host]` (see
[WebSocket access](#websocket-access)), connect any WebSocket client with the
token (for example
`websocat "ws://127.0.0.1:8400/?token=$COPPA_WS_TOKEN"`) and send:

```text
{"type":"audio_level","enabled":true}
```

There is no reply. The daemon then sends ten `rx` messages per second of
received audio (none while transmitting) and one `tx` message per
transmission:

```text
{"type":"audio_level","direction":"rx","peak_dbfs":-12.1,"rms_dbfs":-31.4,"clip_fraction":0.0,"state":"ok","duration_ms":100,"timestamp_ms":1761452411402}
{"type":"audio_level","direction":"tx","peak_dbfs":-6.0,"rms_dbfs":-16.8,"clip_fraction":0.0,"state":"ok","duration_ms":1365,"timestamp_ms":1761452712300}
```

Send `{"type":"audio_level","enabled":false}` to stop. This stream is
independent of the `spectrum` stream.

| Field | Meaning |
|---|---|
| `direction` | `rx`: one 100 ms window of received audio. `tx`: one whole transmission, including any prepended station-ID frame |
| `peak_dbfs` | Highest sample magnitude, dB relative to full scale (sample value 1.0), floored at -120, rounded to 0.1 dB |
| `rms_dbfs` | RMS level, same convention. A full-scale sine reads -3.0 |
| `clip_fraction` | Fraction (0.0-1.0) of samples at or above 0.99 of full scale |
| `state` | `ok`, `clipping` or `underdrive` (RX, after this window, using the thresholds above). TX is only ever `ok` or `clipping` |
| `duration_ms` | Audio span the reading covers |
| `timestamp_ms` | Wall-clock time the message was sent (Unix epoch, ms) |

## WebSocket access

The WebSocket port can key the transmitter, so `coppad` refuses a client
unless it presents a shared-secret token. This applies on every
`bind_address`, loopback included: a web page open in your own browser can
reach `127.0.0.1` too.

### Setting the token

`websocket_enabled = true` needs `websocket_token` under `[host]`, or `coppad`
exits at startup with an error naming the key. The token must be at least 16
characters from `A-Z a-z 0-9 - . _ ~`. Generate one with:

```text
openssl rand -hex 32
```

```toml
[host]
websocket_enabled = true
websocket_token = "3f9c0e5b7a2d4c1e8f6a0b9d2c7e4a1f5b8d0c3e6a9f2b5d8c1e4a7f0b3d6c9e"
```

Make the config file readable only by the account `coppad` runs as
(`chmod 600`). `coppad --check` does not check the token; starting the daemon
does.

### Presenting the token

A client presents the token in either of two ways:

- `Authorization: Bearer <token>` on the upgrade request. Use this from
  scripts and programs that can set headers.
- A `token` query parameter: `ws://127.0.0.1:8400/?token=<token>`. Browser
  JavaScript cannot set headers on `new WebSocket(...)`, so a browser page
  must use this form.

```text
websocat -H "Authorization: Bearer $COPPA_WS_TOKEN" ws://127.0.0.1:8400/
websocat "ws://127.0.0.1:8400/?token=$COPPA_WS_TOKEN"
```

### Allowing a browser page

Browsers send an `Origin` header with every WebSocket handshake, naming the
page that opened it. When a handshake carries an `Origin`, it must match an
entry in `websocket_allowed_origins` exactly, even if the page also has the
token. The default is empty, so no web page can connect until you list it.
Scripts and programs that send no `Origin` header need only the token.

```toml
[host]
websocket_allowed_origins = ["http://localhost:3000", "https://dash.example.org"]
```

Each entry is `scheme://host[:port]`, exactly as the browser's address bar
shows the page's origin:

- `http` or `https` only, with no path, query, fragment or user name
  (`"http://localhost:3000/"` is refused; write `"http://localhost:3000"`).
- No wildcards. List each origin; another web app on a different local port
  is exactly the untrusted page this check keeps out.
- No default port: write `"http://localhost"`, not `"http://localhost:80"`,
  because browsers omit it.
- `"null"` is never allowed. It is the origin of sandboxed frames and
  `file://` pages.

Case does not matter. `coppad` checks every entry at startup and exits with an
error naming the bad one.

From an allowed page:

```js
const ws = new WebSocket("ws://127.0.0.1:8400/?token=" + encodeURIComponent(token));
ws.onopen = () => ws.send(JSON.stringify({ type: "status" }));
```

### What a refused client sees

The daemon refuses at the HTTP upgrade, before the connection becomes a
WebSocket, so a refused client never sends a command or receives a broadcast:

- An `Origin` that is not allowed gets `HTTP/1.1 403 Forbidden`.
- A missing or wrong token gets `HTTP/1.1 401 Unauthorized` with
  `www-authenticate: Bearer`.

A browser page sees only a failed connection: an `error` event, then close
code 1006. The daemon prints one line per refusal on stderr, without the token
or the request URI:

```text
WebSocket handshake from 127.0.0.1:50358 rejected (403): origin "https://evil.example" is not allowed
WebSocket handshake from 127.0.0.1:50360 rejected (401): token missing or invalid
```

On a non-loopback `bind_address`, the token crosses the network in clear text
over `ws://`. Anyone who can watch that traffic can reuse it.

### What this does not cover

The VARA-style command and data ports and the KISS TCP port have no
authentication, so that Pat and other VARA clients keep working. A web page
can still deliver a VARA command line to the command port inside an HTTP
`POST` body, and the daemon executes it. Until that is fixed, if you enable
`vara_enabled`, do not browse untrusted sites on the same machine, or run
`coppad` on a host you do not browse from.

## Connected sessions: delivery and BUFFER

Data a host writes during a connected session (`CONNECT` from a VARA-style
host such as Pat) is acknowledged and, if lost, sent again. The other
station's host gets every byte once, in order.

- `BUFFER` counts bytes the other station has not yet acknowledged. It falls
  when the other station's ACK arrives, not when PTT releases.
- The receiving station keys briefly after each data frame to send its ACK,
  so its host sees `PTT ON` / `PTT OFF` for every frame it receives.
- One frame is in flight at a time: the sender waits for the ACK before it
  sends the next frame. On a clean HF link this carries about 60% of what an
  unacknowledged stream did; on a link that loses frames it is the only mode
  that delivers the data intact.
- A frame that is still unacknowledged after 5 retransmissions stops using
  the normal budget but does not immediately end the session: coppad holds
  the link, retransmitting at the most robust speed level the frame still
  fits, for as long as the other station stays decodable (any decoded frame
  from it, or even a frame whose header decodes while its payload does not,
  counts as evidence and restarts the patience clock). Only after 180
  seconds with no evidence at all does the session end: coppad sends
  DISCONNECT to the other station, reports `BUFFER 0` and then
  `DISCONNECTED`. On a link that has gone completely silent this takes
  about 3 minutes for the budget plus up to 3 more for the hold; on a link
  that is only faded, the session survives the fade instead of dropping.
  A held session shows a `BUFFER` count that is not progressing, and a host
  `DISCONNECT` still ends it immediately.
- coppad holds one session at a time. While a session exists (connecting or
  connected), every `CONNECT_REQ`, including a repeat from the station already
  in the session, gets no answer, so that station's connect attempt times out.
  The one exception is a repeat from a caller whose inbound attempt is still
  pending (it did not hear the first answer): coppad answers it again (see
  "Inbound connect attempts" below).
  A host `CONNECT` is answered `DISCONNECTED`, to the dialling host only. Send
  `DISCONNECT` to end the current session before dialling again. A session
  nobody ends blocks every caller: an idle connected session does not time
  out, so a listening station whose peer vanished refuses callers until its
  host sends `DISCONNECT`.
- `[engine] arq_enabled` does not affect connected sessions; it only enables
  ARQ for sessionless traffic (`[host] sessionless_tx_enabled`).
- Both stations must run coppad 0.3.0 or later. An older station cannot
  decode this session format: its host receives extra header bytes in the
  data, it never acknowledges, and the newer station disconnects after the
  retry budget. coppad logs `Session data frame is not a reliable transport
  PDU` when an older station sends to it.

## Half-duplex turnaround

A half-duplex rig cannot receive while it is keyed, and a station that has just
sent a frame stays keyed for a while after its last audio sample: its
`[radio] ptt_tail_delay_ms` (default 200 ms), or its rig's VOX hang time. A
reply keyed in that window goes out to a station that cannot hear it.

`coppad` therefore holds every key-up that follows a decoded frame until
`[radio] turnaround_ms` (default 500) has passed since that frame decoded. This
covers data ACKs, CONNECT_ACK/CFM, CP-negotiation replies, and anything else
the daemon transmits in that window. A transmission more than `turnaround_ms`
after the last decoded frame keys at once, and a transmission queued behind
one already on the air is not held, since PTT is already asserted. While it
holds, the daemon keeps reading its audio input, so no received audio is lost.

```toml
[radio]
turnaround_ms = 500
```

Set it above the **other** station's PTT tail plus its radio's TX→RX switching
time:

- With `ptt_method = "vox"` on the other station, its rig holds TX for its own
  VOX delay, often 0.3-1 s. That is the number to beat, not
  `ptt_tail_delay_ms`.
- The cost is up to `turnaround_ms` of extra latency per reply. A long value
  only costs throughput; a short one loses replies.
- At startup `coppad` warns when `turnaround_ms` does not exceed this station's
  own `ptt_tail_delay_ms`, since a peer configured the same way would still be
  keyed. It only warns: the peer's real tail cannot be known locally.
  `coppad --check` does not print this warning.
- `turnaround_ms = 0` disables the hold, and replies key at once.

The key lives in `[radio]`. A `turnaround_ms` under `[session]` (from older
example configs) has never been read.

Known gap: the hold is receive-side. Connected sessions avoid the collision
by running one frame at a time: the sender waits for the ACK before it keys
the next frame (COP-38). Sessionless ARQ (`arq_enabled`, window 8) still has
the gap: a station with several ARQ segments queued re-keys for the next one
the moment its own PTT releases, so the other station's ACK can still collide
with that next frame. Sender-side turn discipline is a separate follow-up. The hardware acceptance test for this
behaviour is `docs/testing/cable-loop-turnaround.md`.

## Inbound connect attempts

A listening coppad (`LISTEN ON`) pushes `PENDING` to every host when it
answers another station's `CONNECT_REQ`. The attempt then ends in `CONNECTED`
when the caller confirms, or in `CANCELPENDING` when the handshake does not
complete: no confirmation within 30-35 s, the caller sent `DISCONNECT`, or its
confirmation was rejected. `CANCELPENDING` is not followed by `DISCONNECTED`,
except when the host itself sends `DISCONNECT` during the attempt: it then
gets `CANCELPENDING` and then `DISCONNECTED`. A repeat `CONNECT_REQ` from the
same caller while its attempt is pending is answered again, without a second
`PENDING`. A `CONNECT_REQ` coppad ignores (not listening, addressed to another
callsign, or unreadable) produces no line.

## Host clients that stop reading

coppad queues every command-port line for each connected host. If a host
stops reading and its socket buffers fill, coppad drops `BUSY` and `SNR`
lines once 64 lines are waiting; the next change sends a fresh value. It
keeps every other line (`PTT`, `PENDING`, `CANCELPENDING`, `CONNECTED`,
`DISCONNECTED`, `BUFFER`, `IAMALIVE` and command replies). If 1024 lines are waiting, coppad writes
what is queued, closes the connection and logs `VARA command-port client is
not reading; disconnecting it`. A host never misses a state line while its
connection stays open. Find out why the host stopped reading, then reconnect
it.

The data port keeps every received payload; nothing on it is dropped. A host
that reads again before 1024 payloads are waiting gets all of them, in order.
If 1024 payloads are waiting, coppad writes what is queued, closes the data
connection and logs `VARA data-port client is not reading; disconnecting it`.
The command port stays connected. A host never sees a gap in received data
while its data connection stays open.
