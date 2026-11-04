# Operating Coppa

## What coppad reports at startup

`coppad`'s first log line names the build, and just before `Daemon ready` (or
`Daemon running degraded`) it states what startup actually did: the host
listeners, the audio devices it opened, and the PTT backend in control. A
build without `cpal-backend`, with the default config, logs:

```
INFO coppad: coppad - Coppa Daemon starting version="0.3.0" git_sha="f1dcac4a6ac6" features=none
INFO coppad: Daemon configuration loaded profile=HF_STANDARD preferred_hardware_rate=48000 ptt_method=none turnaround_ms=500
INFO coppad: Host listeners bind_address=127.0.0.1 vara_command=disabled vara_data=disabled ardop_command=not-built ardop_data=not-built websocket=not-built kiss=not-built
WARN coppad: Audio devices: none; this coppad was built without the cpal-backend feature, so it can neither receive nor transmit
INFO coppad: PTT method="none" active=none state="not controlled" guardian="not used"
INFO coppad: Daemon ready
```

A `cpal-backend,websocket` build with rigctld PTT and the VARA and WebSocket
servers on logs, before `Daemon ready`:

```
INFO coppad: Audio input started device="Default Audio Device (alsa:default)" hardware_rate=48000 engine_rate=48000
INFO coppad: Audio output started device="Default Audio Device (alsa:default)" hardware_rate=48000 engine_rate=48000
INFO coppad: Host listeners bind_address=127.0.0.1 vara_command=127.0.0.1:8300 vara_data=127.0.0.1:8301 ardop_command=disabled ardop_data=disabled websocket=127.0.0.1:8400 kiss=disabled
INFO coppad: Audio devices input="Default Audio Device (alsa:default)" output="Default Audio Device (alsa:default)" input_hardware_rate=48000 output_hardware_rate=48000 engine_rate=48000
INFO coppad: PTT method="rigctld 127.0.0.1:4532" active=rigctld state="released" guardian="running, pid 28849"
```

| Line | Field | Values |
|---|---|---|
| `Coppa Daemon starting` | `version` | The `coppad` version. |
| | `git_sha` | The commit HEAD pointed at when `coppad` was built (12 hex digits), or `unknown` for a build from a tree without `.git`. Packagers building from a tarball set `COPPA_GIT_SHA` (7-40 hex digits) at build time. It does not flag uncommitted changes. |
| | `features` | `coppa-daemon`'s cargo features in this build (`cpal-backend`, `gpio-ptt`, `kiss-tnc`, `mdns`, `serial-ptt`, `websocket`), comma-separated, or `none`. |
| `Host listeners` | `bind_address` | `[host] bind_address`. |
| | `vara_command`, `vara_data`, `ardop_command`, `ardop_data`, `websocket`, `kiss` | The bound `ip:port` (the real port when 0 is configured), `disabled`, or, `not-built` when the listener feature (`websocket`, `ardop` or `kiss-tnc`) is absent. The `websocket` port also serves the operator dashboard over plain HTTP at `GET /` (COP-79, COP-83; see [Operator dashboard](#operator-dashboard)) and answers `GET /health` (COP-140, see [Health check](#health-check)). |
| `Host listeners accept connections from other hosts; ...` (WARN) | `bind_address` | Logged when a listener is bound to a non-loopback address such as `0.0.0.0`: anyone who can reach it can key the transmitter. |
| `mDNS advertisement` | `host`, `service`, `port`, `addresses` | Only with `[host] mdns_enabled = true` in an `mdns` build. The advertised host (`coppad.local.`), service instance (`coppad._coppa._tcp.local.`), bound WebSocket port, and `every non-loopback IPv4 address` (`bind_address = "0.0.0.0"`), `every non-loopback address` (`"[::]"`) or the single bound address. See [Finding coppad on the network](#finding-coppad-on-the-network-mdns). |
| `mDNS name already in use on this network; ...` (WARN) | `original`, `new_name` | Another host answers for the name, so the responder renamed this daemon, for example to `coppad-2.local.`. Clients must use the new name. |
| `mDNS responder error` (WARN) | `error` | A socket or interface problem the responder hit after startup. |
| `Audio devices` | `input`, `output` | The device cpal opened, as its name and cpal id, for example `Default Audio Device (alsa:default)`. On ALSA, `default` is whatever PulseAudio, PipeWire or `.asoundrc` route it to; coppad cannot see further. A named device shows its own description and id. |
| `Audio input/output started` | `hardware_rate`, `engine_rate` | The opened stream rate and modem profile rate; they differ when Coppa resamples. |
| `Audio devices` | `input_hardware_rate`, `output_hardware_rate`, `engine_rate` | The effective input and output hardware rates and the modem profile rate. |
| `Audio devices: none; ...` (WARN) | | A build without `cpal-backend`: the daemon runs but moves no audio. |
| `PTT` | `method` | The configured method: `none`, `vox`, `rigctld <address>`, `serial <port> <dtr\|rts>` or `gpio <pin>`. |
| | `active` | The backend actually in control: `none`, `vox`, `rigctld`, `serial` or `gpio`. An unreachable rigctld shows `active=none`. |
| | `state` | `released` (the startup release succeeded), `release failed: <error>`, or `not controlled` (none, VOX, or an unreachable rigctld, where coppad drives no PTT line). |
| | `guardian` | `running, pid <n>`, `failed to start: <error>`, `off ([radio] ptt_guardian = false)`, or `not used`. See [The PTT guardian](#the-ptt-guardian). |

The `PTT` line is a WARN when PTT keeps the daemon from calling itself ready.
`--log-level warn` hides the INFO summary lines but not the WARN ones. Set
`NO_COLOR=1` when the log goes to a file or the journal, so each key stays next
to its `=`.

## Selecting a radio audio device

Run `coppa devices` with a `cpal-backend` build to see the host's audio device
names, IDs, and available input/output channel counts. Copy an ID into
`[audio] input_device_id` or `output_device_id` in `coppad.toml` for exact
selection. Leave the corresponding `input_device` or `output_device` name empty
when setting an ID. A name is a case-insensitive substring selector only when
it identifies one direction-capable device. If it matches several, startup
reports their IDs and fails; if an explicit ID is missing or has the wrong
direction, startup fails rather than using the default. IDs are host-specific
and can change when the audio host reconfigures devices.

`[audio] input_channel` and `output_channel` are zero-based and default to 0.
Set either to 1 to use the second lane of a stereo interface. Coppa keeps
modem audio mono: RX reads only the selected input lane; TX sends only to the
selected output lane and silences the others. An out-of-range channel or an
unsupported audio format is a startup error. `[audio] sample_rate` is the
preferred *hardware* rate, not the modem engine rate. If the selected device
cannot run that rate, Coppa chooses a supported `f32` rate nearest the engine
rate and converts the stream, reporting both effective hardware and engine
rates. The OFDM engine continues at its profile rate.

CLI `rx`/`listen` accept `--device-id` and `--input-channel`; `tx`/`tune`
accept `--device-id` and `--output-channel`; `tnc` accepts separate
`--input-device-id` and `--output-device-id` flags and both channel flags.
The legacy `--device` name option remains for a unique match. Warnings go to
stderr, including with `--quiet`, so decoded output on stdout stays clean.
The CLI reads a device *name* from `coppad.toml` (`[audio] input_device` and
`output_device`, see the CLI settings section of the getting-started guide),
but not the `_device_id` keys or the channel keys: pass `--device-id` and the
channel flag on the command line. A `--device-id` flag replaces any device
name from the file or `COPPA_DEVICE`.

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
   `--duration N` (`-d N`, more than 0 and at most 120) to change it; the old
   `--seconds N` still works for one release with a deprecation warning), or `TUNE` (or `TUNE <seconds>`) on the daemon's
   VARA command port. This transmits a standard two-tone signal (700 Hz +
   1900 Hz, equal amplitude) — the same drive-level convention hams use for
   SSB two-tone tests. The daemon caps a TUNE at `[radio] max_tx_duration_s`
   (and never more than 120 s). From the dashboard or any WebSocket client,
   send `{"type":"tune"}` (optional `seconds`, `single_hz`); see
   [Operator dashboard](#operator-dashboard).
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
- A second Ctrl-C in `coppa tx` or `coppa tune`, which exits at once without
  cleanup. The first Ctrl-C (or SIGTERM) releases PTT and stops the audio
  before exiting with status 130 (COP-86). Errors and panics in them also
  release PTT.

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

A browser can show the same RX reading without a separate client: the
operator dashboard at `GET /` on the WebSocket port draws it as a meter. See
[Operator dashboard](#operator-dashboard).

| Field | Meaning |
|---|---|
| `direction` | `rx`: one 100 ms window of received audio. `tx`: one whole transmission, including any prepended station-ID frame |
| `peak_dbfs` | Highest sample magnitude, dB relative to full scale (sample value 1.0), floored at -120, rounded to 0.1 dB |
| `rms_dbfs` | RMS level, same convention. A full-scale sine reads -3.0 |
| `clip_fraction` | Fraction (0.0-1.0) of samples at or above 0.99 of full scale |
| `state` | `ok`, `clipping` or `underdrive` (RX, after this window, using the thresholds above). TX is only ever `ok` or `clipping` |
| `duration_ms` | Audio span the reading covers |
| `timestamp_ms` | Wall-clock time the message was sent (Unix epoch, ms) |

## Monitoring and discovery

### Health check

On a daemon with the WebSocket listener (built with `--features websocket`,
run with `[host] websocket_enabled = true`), the WebSocket port answers
`GET /health` and `HEAD /health` (COP-140). No token is needed. After
`Daemon ready`:

```console
$ curl -i http://127.0.0.1:8400/health
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 16
Cache-Control: no-store
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
Content-Security-Policy: frame-ancestors 'none'
Connection: close

{"status":"ok"}
```

After `Daemon running degraded` (for example `[radio] ptt_method = "rigctld"`
with rigctld unreachable), the status line is the same and the body is:

```text
{"status":"degraded"}
```

- Both values return HTTP 200. A monitor that alerts on degradation matches
  the `status` field, not the status code. `degraded` means startup found the
  problems that the `Daemon running degraded problems=...` log line lists;
  the body never repeats them.
- The value is the startup verdict. A fault that starts later does not change
  it, and a build without `cpal-backend` reports `ok`. `docs/LIMITATIONS.md`
  has the details.
- The body carries only the status: no version, callsign, frequency, PTT state
  or session data. The query string is ignored, and the response has no
  `Access-Control-Allow-Origin` header, so a page on another site cannot read
  it.
- `HEAD` returns the `GET` head, `Content-Length` included, with no body. This
  holds for every path `GET` answers. Other methods get `405` with
  `Allow: GET, HEAD`.
- A connection must send its whole request head within 5 s, or the daemon
  closes it without a response.
- Without the WebSocket listener there is no port, so a probe gets "connection
  refused".

A probe in a systemd unit or a shell script:

```bash
curl -fsS http://127.0.0.1:8400/health | grep -q '"status":"ok"'
```

A Nagios or Icinga `check_http` probe that goes critical unless the daemon is
healthy (Uptime Kuma: an HTTP(s) - Keyword monitor with the keyword
`"status":"ok"`):

```bash
check_http -H 127.0.0.1 -p 8400 -u /health -s '"status":"ok"'
```

### Finding coppad on the network (mDNS)

A `coppad` built with the `mdns` feature can advertise itself on the local
network, so a phone, tablet or laptop opens it as `http://coppad.local:8400/`
without knowing its address. The feature turns on `websocket` too:

```bash
cargo build --release -p coppa-daemon --features cpal-backend,websocket,mdns
```

```toml
[host]
bind_address = "0.0.0.0"
websocket_enabled = true
mdns_enabled = true
```

`mdns_enabled = true` needs all three of:

- a `coppad` built with `--features mdns`;
- `websocket_enabled = true`, because the WebSocket port is the only one
  advertised;
- a `bind_address` other hosts can reach, such as `"0.0.0.0"` or this host's
  LAN address. Read the `bind_address` warning in `coppad.toml.example` first,
  and use it only on a trusted, firewalled network.

Otherwise `coppad` stops at startup, before it touches audio or PTT, with one
of:

```text
Error: [host] mdns_enabled = true is set, but this coppad was built without the mdns feature and advertises nothing; rebuild with --features mdns, or set mdns_enabled = false
Error: [host] mdns_enabled = true needs [host] websocket_enabled = true: coppad advertises its WebSocket port (the operator page and /health) and nothing else
Error: [host] mdns_enabled = true needs a bind_address other hosts can reach, but bind_address = "127.0.0.1" is loopback, so coppad.local would send clients to a port that refuses them; set bind_address = "0.0.0.0" or this host's LAN address, on a trusted, firewalled network
```

A `bind_address` host name that resolves to loopback, such as `localhost`, gets
the third message after the listener binds, naming the bound address.

mDNS uses multicast on UDP port 5353; allow it through the host firewall.
The responder runs inside `coppad` and needs no Avahi or Bonjour daemon. The
daemon advertises:

| Record | Value |
|---|---|
| Host name (A, and AAAA for `[::]`) | `coppad.local.` |
| Service instance | `coppad._coppa._tcp.local.` |
| SRV | `coppad.local.`, the bound WebSocket port (8400 by default) |
| TXT | `txtvers=1`, `path=/`, `health=/health` |

The VARA-style command and data ports, the ARDOP ports and KISS are not
advertised. On a clean shutdown the daemon withdraws the records.

Phones, tablets, macOS, Windows and browsers resolve `.local` names
themselves. A Linux client needs nss-mdns (Avahi's `libnss-mdns` package with
`mdns4_minimal` in `/etc/nsswitch.conf`); without it `getent hosts
coppad.local` and `curl http://coppad.local:8400/` fail. On such a host,
`avahi-browse -rt _coppa._tcp` lists the daemon; on macOS,
`dns-sd -B _coppa._tcp` does.

To open the operator page by its mDNS name, add that origin to
`websocket_allowed_origins`, or the page reports `Refused`:

```toml
websocket_allowed_origins = ["http://coppad.local:8400"]
```

The name is fixed at `coppad.local`. If another host on the LAN already
answers for it, the responder picks a new name (for example `coppad-2.local`)
and logs `mDNS name already in use on this network; advertising under the new
name` with `original` and `new_name`. Use the new name, and add its origin if
you open the page by it. Set `RUST_LOG=mdns_sd=debug` to see the responder's
own log.

## WebSocket access

The WebSocket port can key the transmitter, so `coppad` refuses a client
unless it presents a shared-secret token. This applies on every
`bind_address`, loopback included: a web page open in your own browser can
reach `127.0.0.1` too.

For a host author's overview of every port, including the WebSocket
handshake outcomes and request table, see [HOST-API.md](HOST-API.md). This
section owns the token and Origin setup.

### Dashboard protocol v1

After authentication the first text frame is a versioned greeting:

```json
{"type":"hello","version":1,"server":"coppa 0.3.0"}
```

All application messages are JSON text frames. The [protocol schema](schema/websocket-protocol-v1.json)
defines requests and replies. Version 1 replaces the earlier unversioned API;
`session` replaces the old `connected` and `disconnected` message types.
Connecting subscribes the client to data and state events. Version 1 grows
only by adding optional fields and new message types; a client must ignore
fields and types it does not know.

Requests may include an unsigned integer `id`. Each request with an id gets
one reply with that id: `ack`, `status`, or `error`. An ack means accepted
for processing. It does not mean a connection or transmission completed.
Callsign validation happens in the daemon; an accepted but invalid `mycall`
can still be rejected there with a warning log.

```json
{"type":"mycall","callsign":"N0CALL","id":1}
{"type":"ack","id":1}
{"type":"status","id":2}
{"type":"status","id":2,"connected":false,"cp_desync_episodes":0}
{"type":"connect","source":"N0CALL","destination":"W1AW","id":3}
{"type":"ack","id":3}
{"type":"session","state":"connected","remote_call":"W1AW"}
{"type":"send","data":"hello dashboard","encoding":"utf8","id":4}
{"type":"ack","id":4}
{"type":"data","data":"hello dashboard","encoding":"utf8"}
{"type":"data","data":"AP8=","encoding":"base64"}
{"type":"ptt","state":true}
{"type":"busy","state":false}
{"type":"snr","db":14}
{"type":"level","level":2}
{"type":"heard","stations":[{"call":"W1AW","last_heard_ms":1761452712300,"snr_db":14,"level":2}]}
{"type":"buffer","bytes":15}
{"type":"buffer","bytes":0}
{"type":"disconnect","id":5}
{"type":"ack","id":5}
{"type":"session","state":"disconnected"}
{"type":"error","message":"Invalid message"}
```

For `send`, omitted `encoding` means `utf8`. Use `base64` for arbitrary bytes;
invalid base64 produces an error with the request id. Received `data` always
specifies its encoding and preserves every byte. Raw WebSocket binary frames
are not payload messages. `heard` is a full snapshot of up to 32 recent session
peers, including the last-heard timestamp, rather than a list of all RF activity.
`buffer` is sent when the count of host bytes not yet sent or acknowledged
changes, the same moment the VARA `BUFFER` line goes out.

Data, session, PTT, busy, SNR, level, heard, buffer and speed-level events use
a reliable queue for each client. A client either receives them in order or
its connection closes. If 256 pending messages fill that queue, the server
disconnects that client; it never silently skips a state or data event.
Replies go directly to the requesting socket. Reconnect and query `status`
after losing a connection.

#### What `status` reports

A daemon fills in more of the `status` reply than the minimal example above.
Each field appears only once the daemon has published it, so a server with no
daemon behind it still sends `{"type":"status","id":2,"connected":false,"cp_desync_episodes":0}`.
Idle, a reply looks like this:

```json
{"type":"status","id":1,"connected":false,"cp_desync_episodes":0,"ptt":false,"busy":false,"waiting_for_clear_channel":false,"buffer":0,"speed_level":2,"speed_level_mode":"auto","speed_levels":[1,2,3,4,5,6,7,8],"mycall":"N0CALL","max_tx_s":30,"max_tune_s":30,"loop_age_ms":212,"heard":[]}
```

A client that joins 1.2 s into a transmission learns it at once:

```json
{"type":"status","id":1,"connected":false,"cp_desync_episodes":0,"ptt":true,"ptt_keyed_ms":1234,"busy":false,"waiting_for_clear_channel":false,"buffer":0,"speed_level":2,"speed_level_mode":"auto","speed_levels":[1,2,3,4,5,6,7,8],"mycall":"N0CALL","max_tx_s":30,"max_tune_s":30,"loop_age_ms":95,"heard":[]}
```

In a session, `remote_call`, `snr`, `level` and `cfo` appear as before, and
`link` is added while the session has an ARQ link:

```json
"link":{"in_flight":1,"retries":1,"max_retries":10,"rto_ms":5200,"fade_hold":false}
```

| Field | Meaning |
|---|---|
| `ptt` | PTT state the daemon last commanded |
| `ptt_keyed_ms` | Milliseconds PTT has been keyed, computed when the reply is built. Present only while `ptt` is true |
| `busy` | Last `busy` push value (RX-band occupancy) |
| `waiting_for_clear_channel` | True while a transmission is held by `[station_id] busy_hold_ms` for a clear channel |
| `buffer` | Host bytes not yet sent or acknowledged (the VARA `BUFFER` count) |
| `speed_level` | Speed level our next frame is encoded at |
| `speed_level_mode` | `auto` (the rate loop chooses) or `manual` (pinned by `set_speed_level`) |
| `speed_levels` | Levels `set_speed_level` accepts on this engine (HF 1-8, VHF 1-9) |
| `mycall` | Callsign in force; absent when none |
| `max_tx_s` | `[radio] max_tx_duration_s` |
| `max_tune_s` | Longest TUNE accepted: the smaller of `max_tx_s` and 120 |
| `loop_age_ms` | Milliseconds since the daemon's event loop last completed its 500 ms housekeeping tick or a busy-channel wait step, computed when the reply is built |
| `heard` | The same snapshot the `heard` push carries |
| `link` | Session ARQ link: `in_flight`, `retries` (transmissions of the in-flight segment minus one), `max_retries`, `rto_ms`, `fade_hold`. Absent without a session link |

Every reliable state push (`session`, `ptt`, `busy`, `snr`, `level`, `heard`,
`buffer`, `speed_level`) has a matching `status` field, and the daemon updates
the field before it pushes. A client that sends `status` after connecting
therefore never ends on an older value than the pushes it then receives.

#### TUNE and speed level requests

`tune` keys the TX-level calibration tone, exactly like VARA `TUNE`.
`seconds` defaults to 10 and must be greater than 0 and at most `max_tune_s`.
Without `single_hz` the tone is the 700 + 1900 Hz two-tone; with it, a single
tone at that frequency, between 300 and 3000 Hz (the CLI's
`coppa tune --single`). A daemon with no callsign in force refuses it:

```json
{"type":"tune","id":2}
{"type":"ack","id":2}
{"type":"ptt","state":true}
{"type":"ptt","state":false}
{"type":"tune","seconds":5,"single_hz":1500,"id":3}
{"type":"ack","id":3}
{"type":"tune","seconds":120,"id":4}
{"type":"error","message":"TUNE seconds must be greater than 0 and at most 30","id":4}
{"type":"tune","seconds":5,"single_hz":5000,"id":5}
{"type":"error","message":"TUNE single_hz must be between 300 and 3000 Hz","id":5}
{"type":"tune","id":6}
{"type":"error","message":"No callsign is set; set MYCALL before TUNE","id":6}
```

`set_speed_level` takes an integer level or the string `"auto"`. A request
without `level` is an error, so a typo cannot silently mean `auto`:

```json
{"type":"set_speed_level","level":3,"id":7}
{"type":"ack","id":7}
{"type":"speed_level","level":3,"mode":"manual"}
{"type":"set_speed_level","level":9,"id":8}
{"type":"error","message":"Speed level 9 is not available on this profile (levels 1-8)","id":8}
{"type":"set_speed_level","level":"auto","id":9}
{"type":"ack","id":9}
{"type":"speed_level","level":2,"mode":"auto"}
```

`set_speed_level` pins the level every new frame uses, sessionless or in a
session, until `{"level":"auto"}` or a restart. While pinned, the rate loop
keeps measuring but does not change the level and does not probe. COP-68
fade-hold retransmissions still use the fade-hold level. `level` (push and
`status`) is the peer's level as decoded; `speed_level` is ours. The
`speed_level` push is sent whenever the level our next frame uses or the mode
changes, including the rate loop's own changes.

Spectrum, audio levels, logs and decode-failure aggregates are disposable
telemetry on a separate lossy channel. Lag can skip these messages without
affecting reliable events. Spectrum, audio levels and logs each require an
independent opt-in:

```json
{"type":"spectrum","enabled":true}
{"type":"audio_level","enabled":true}
{"type":"log","enabled":true}
```

Set `enabled` to false to stop a stream. Spectrum frames contain 128 dB bins,
`timestamp_ms`, `freq_low_hz` and `freq_high_hz`. The bins span the engine's
current `[freq_low_hz, freq_high_hz)` interval, queried on each update: 350 to
2750 Hz for HF standard, 350 to 5950 Hz for VHF wide. The wider band has
coarser bins, about 43.75 Hz each, sampled from an approximately 11.7 Hz FFT.

```json
{"type":"log","level":"info","message":"Frame decoded successfully","timestamp_ms":1761452712300}
{"type":"decode_fail","count":6,"timestamp_ms":1761452712300}
```

Logs carry INFO, WARN and ERROR events with the console line's `key=value`
fields appended to the message, so a refusal shows its reason:

```json
{"type":"log","level":"warn","message":"coppad::event_loop: MYCALL rejected callsign=!!bad!! error=Invalid callsign character: '!'","timestamp_ms":1793600129709}
```

Each message is truncated to 2 KiB, fields included.
`decode_fail` is unconditional and reports a cumulative failure count at most
once per 500 ms. Neither diagnostic stream is a delivery receipt.

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

The VARA-style command and data ports and both KISS TCP ports have no
authentication, so that Pat and other VARA clients keep working. A web page
can still deliver a VARA command line to the command port inside an HTTP
`POST` body, and the daemon executes it. Until that is fixed, if you enable
`vara_enabled`, do not browse untrusted sites on the same machine, or run
`coppad` on a host you do not browse from.

## Operator dashboard

`coppad` serves an operator dashboard at `GET /` on the WebSocket port
(COP-79, COP-83): one static page that sets a speed level, shows S/N, keys
TUNE and tells transmitting from a hang without a terminal.

1. Build `coppad` with the `websocket` feature, and set `websocket_enabled =
   true` and `websocket_token` under `[host]` (see
   [Setting the token](#setting-the-token)).
2. Add the page's own origin, exactly as the browser's address bar shows it,
   to `websocket_allowed_origins`: for example `"http://127.0.0.1:8400"`, and
   also `"http://localhost:8400"` if you open it by that name. The page's
   refusal line names the exact string to add.
3. Open `http://127.0.0.1:8400/`, type the token and press Connect. Loading
   the page needs no token. The page keeps the token in memory only, so a
   reload asks for it again; an automatic reconnect reuses it.

The page makes no external requests. Each card:

- **Header**: `Connection:`, `Daemon:`, `MYCALL`, the PTT lamp and the
  Channel (BUSY) lamp. See the table below.
- **Waterfall**: one row per `spectrum` frame (4 per second, only while the
  daemon receives audio), with a spectrum line underneath, a red line where
  PTT went on and a grey line where it went off.
- **RX level**: the `audio_level` RX reading as a bar from -60 to 0 dBFS
  (see [RX level](#rx-level)), and the last transmission's TX level.
- **S/N**: the last decoded frame's S/N, how long ago it arrived, and a
  sparkline of the last 2 minutes. When the connection drops the age is
  dropped too; after a reconnect the page takes the S/N from `status`, which
  has no arrival time, so the age is missing until the next frame decodes.
  CFO is the last decoded frame's offset.
- **Speed level**: `TX <n> · auto` or `TX <n> · manual` (ours), `RX <n>` (the
  peer's last decoded frame), `Auto` plus one button per level the profile
  accepts, and the buffer count.
- **TUNE**: `Seconds` (default 10, at most `max_tune_s`), `Two-tone` and
  `Single tone 1500 Hz`. Disabled while PTT is keyed. A TUNE cannot be stopped
  early; it ends after its seconds.
- **Session**: the session state, the ARQ link (in flight, retries, RTO, fade
  hold) and the heard list of recent session peers.
- **Log**: INFO, WARN and ERROR lines with their `key=value` fields, and
  received data.
- **Command box**: `MYCALL` and `Set`, `Call station` with `Call` and
  `Disconnect`, and `Send` (enabled only while a session is connected).

A request's result shows as `<request>: accepted` or
`<request>: refused: <reason>`. Accepted means accepted for processing; a
refusal the daemon makes later (an invalid `MYCALL`, for example) shows as a
WARN line in the log pane.

### Reading the header: transmitting or hung?

| Header shows | What it means | What to check |
|---|---|---|
| `Connection: Not connected` | The page has not connected, or you closed it | Type the token and press Connect |
| `Connection: Connecting…` | The WebSocket handshake is in progress | Wait; it should finish within a second |
| `Connection: Connected` | The page speaks protocol version 1 with the daemon | Nothing |
| `Connection: Refused or unreachable: check the token, and that [host] websocket_allowed_origins lists "<origin>"` | The socket never opened: wrong token (401), origin not allowed (403) or daemon not reachable. The browser cannot tell which | The token, the quoted origin in `websocket_allowed_origins`, and that `coppad` is running. The page does not retry |
| `Connection: Connection lost; reconnecting in <n> s…` | The socket was open and closed | Whether `coppad` is still running. The page retries after 1, 2, 4, 8, then every 15 s |
| `Connection: Connected, but the daemon speaks protocol version <n>; this page expects protocol version 1` | The daemon and the page are from different releases | Use the page the daemon serves |
| `Daemon: —` | Not connected | The `Connection:` line |
| `Daemon: running` | A `status` reply arrived within 5 s and the event loop ticked within 3 s | Nothing |
| `Daemon: waiting for a clear channel to transmit` | A transmission is held by `[station_id] busy_hold_ms` until the channel clears | The Channel lamp; another station is on frequency |
| `Daemon: event loop stalled for <n> s` | The process answers, but its event loop has not ticked for 3 s or more | The daemon log; a stuck radio, audio or PTT call. Restart `coppad` if it does not recover |
| `Daemon: not answering for <n> s` | The socket is open but no `status` reply came for 5 s | Whether the `coppad` process is alive and not starved of CPU |
| `PTT: —` | PTT state unknown (not connected) | The `Connection:` line |
| `PTT: RX` | The transmitter is not keyed | Nothing |
| `PTT: TX <x.x> s of <max_tx_s> s` | The transmitter is keyed, for that long, against `[radio] max_tx_duration_s` | Nothing; a frame or TUNE ends on its own |
| `PTT: TX <n> s: past the <max_tx_s> s limit. Check the radio and the PTT line.` | PTT has been keyed more than 2 s past the limit, so the daemon's own backstop should already have released it | The radio, the PTT line and the daemon log; see [PTT safety](#ptt-safety-when-coppad-stops) |

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

## Ending a session (DISCONNECT)

A host `DISCONNECT` gets `OK` at once. coppad then sends the data the host
already queued for the session, one frame at a time, each ACKed by the peer
(COP-38), and only after the last ACK keys the DISCONNECT frame on its own.
The host sees `BUFFER` fall to `BUFFER 0`, then one `DISCONNECTED`. With
nothing queued or un-acked, `DISCONNECTED` follows at once, as before.

- A repeated `DISCONNECT` changes nothing: the host still gets one
  `DISCONNECTED`.
- While a frame waits for its ACK, COP-38's retry budget bounds the drain:
  a peer that never ACKs ends it after about 3 minutes. If nothing can key
  for 10 s, for example because the frequency gate holds wideband TX,
  coppad drops the rest sooner. Either way it
  drops what is left, tries to send the DISCONNECT frame and pushes
  `DISCONNECTED`. A block that held the data also refuses that frame, and
  the peer then ends the session on its own timeout. A long queue that keeps
  moving drains in full, so allow for its airtime: at levels 1-2 each frame
  is about 1.4 s on air, plus the peer's ACK.
- A `CONNECT` sent before `DISCONNECTED` arrives ends the drain at once: the
  rest is dropped and the DISCONNECT frame goes out before the new
  `CONNECT`.
- A CONNECT_REQ from another station, or from the draining peer itself,
  gets no answer while the drain runs (the draining session is still the
  one session coppad allows), so that caller's handshake times out and it
  must call again after `DISCONNECTED`.
- Host data written after `DISCONNECT` is not sent on that session.
- Data the peer sends while the drain runs is still ACKed and delivered to
  the host, so two stations that both sent `DISCONNECT` with data queued
  drain each other's queues instead of retrying until the budget runs out.
- `ABORT` during the drain ends it at once (COP-42): the rest is dropped,
  no DISCONNECT frame is sent, and the host gets one `DISCONNECTED`.
- A remote DISCONNECT or a session timeout still drops unsent data at once.

When CW ID is on, the DISCONNECT frame that ends a session is followed by
this station's callsign in Morse in the same transmission. See "Morse (CW)
ID at session end" below.

## Morse (CW) ID at session end

When CW ID is on, a station that ends a session keys its callsign in Morse
right after its DISCONNECT frame, in the same transmission. The ID is a
1000 Hz tone with 5 ms raised-cosine edges, at the same peak level as the
OFDM audio. It goes through the same transmit path as every frame, so the
callsign gate, the wideband gate, the busy-channel wait, turnaround and the
PTT rules all apply to it.

```toml
[station_id]
cwid = "host"
cwid_wpm = 20
```

`cwid` decides who turns it on and off:

| `cwid` | CW ID | Host `CWID ON` | Host `CWID OFF` |
|---|---|---|---|
| `"host"` (default) | off until a host sends `CWID ON` | `OK`, turns it on | `OK`, turns it off |
| `"always"` | on | `OK`, no change | `WRONG`, stays on |
| `"never"` | off | `WRONG`, stays off | `OK`, no change |

The state applies to the whole daemon. With several hosts connected, the
last one to send `CWID ON` or `CWID OFF` wins. `CWID` alone, or with any
other argument (`CWID TRUE`, `CWID MAYBE`), gets `WRONG` and changes
nothing. coppad logs each change at `INFO` (`CW ID turned on by VARA client
3`) and each refusal (`CWID OFF refused: [station_id] cwid = "always"`). At
startup it logs the current state, for example:

```text
INFO coppad::event_loop: CW ID after each session this station ends: off until a host sends CWID ON ([station_id] cwid = "host", 20 WPM)
```

Pat's VARA HF driver sends `CWID ON` every time it starts. Under the
default `cwid = "host"`, that turns the CW ID on for the daemon, and every
Pat session this station ends gains about 4.7 s of Morse for a 6-character
call at 20 WPM. Set `cwid = "never"` if you do not want it.

ARDOP hosts send `CWID TRUE` or `CWID FALSE` on the ARDOP command port.
coppad always echoes the line, because a `WRONG` there would break Winlink
Express init. Under `"host"` it applies the change. Under `"always"` or
`"never"` the state does not change, and coppad logs, for example,
`ARDOP CWID FALSE echoed but not applied: [station_id] cwid = "always"`.

The CW ID follows every DISCONNECT frame this station sends to end a
session:

- a host `DISCONNECT`, once the queued data has drained;
- a host `DISCONNECT` of a pending inbound attempt;
- a session that COP-38's retry budget ends as a link failure;
- a stalled drain that coppad ends;
- a `CONNECT` that ends a drain.

One transmission then carries the frame and the ID:

```text
PTT on | pre-delay | [station-ID frame, if due] | DISCONNECT frame | 7 units of silence | callsign in Morse | PTT tail | PTT off
```

There is no CW ID after `ABORT` (no frame is sent), after the other station
disconnects (this station sends nothing), after a session times out with no
drain, or after `TUNE`, a beacon or sessionless data. The OFDM station-ID
frame still covers the 10-minute rule as before.

coppad keys the callsign in force (`MYCALL`, or `[engine] callsign`) with
any `-SSID` dropped, because the SSID is a network address and not part of
the licensed call. `N1CALL-10` keys `N1CALL`. A `/` portable designator
stays, so `N1CALL/P` keys `N1CALL/P`.

`cwid_wpm` takes 10 to 20 (default 20). 47 CFR 97.119(b)(1) caps a CW ID
sent by an automatic device at 20 WPM. Any other value stops coppad at
startup:

```text
Error: [station_id] cwid_wpm = 25 is outside 10-20; 47 CFR 97.119(b)(1) caps an automatic CW ID at 20 WPM
```

`coppad --check` checks syntax and types only, so it does not report this.
A `cwid` value other than the three above fails both, naming the key.

Timing follows PARIS: one unit is 1.2 / WPM seconds. Airtime of the Morse
part, including its 7-unit lead-in:

| Callsign | Units | 20 WPM | 10 WPM |
|---|---|---|---|
| `N1CALL` | 78 | 4.68 s | 9.36 s |
| `VE3ABCD` | 80 | 4.80 s | 9.60 s |
| `N1CALL/P` | 108 | 6.48 s | 12.96 s |
| `00000000` (longest 8-character call) | 180 | 10.80 s | 21.60 s |

`[radio] max_tx_duration_s` (default 30) cuts any transmission that runs
longer. If the DISCONNECT frame plus the CW ID would not fit inside
`max_tx_duration_s` less a 2 s margin, coppad sends the frame alone and
logs a warning:

```text
WARN coppad::event_loop: CW ID skipped: the DISCONNECT frame (1.9 s) plus the CW ID (4.7 s) would exceed [radio] max_tx_duration_s = 3 less a 2 s margin
```

The margin leaves room for a prepended station-ID frame and the PTT
pre-delay. At the 30 s default, the longest call at 10 WPM still fits.

## Rig frequency and mode from a host (FREQ, MODE)

With `[radio] ptt_method = "rigctld"`, a host on the VARA command port can
read the rig's frequency and mode. If the operator allows it, the host can
also set them. These commands are coppa extensions, like `TUNE`. VARA has no
such commands.

| Line | Reply | Effect |
|---|---|---|
| `FREQ` | `FREQ <hz>`, the rig's answer to rigctld `f`; `WRONG` with no rigctld backend or no clean answer | none |
| `FREQ <hz>` (decimal Hz, one argument) | `FREQ <hz>` as the rig reports it after the set; `WRONG` if refused or the rig did not accept it | sends `F <hz>`, then `f` |
| `MODE` | `MODE <token>`, the rig's own token (`USB`, `PKTLSB`, `CW`, `RTTY`, ...); `WRONG` as for `FREQ` | none |
| `MODE USB` / `LSB` / `PKTUSB` / `PKTLSB` (any case) | `MODE <token>` as the rig reports it after the set; `WRONG` if refused or rejected | sends `M <TOKEN> 0`, then `m` |
| `FREQ 7.074`, `FREQ -1`, `FREQ 1 2`, `MODE CW`, `MODE USB 2400`, `FREQUENCY` | `WRONG` | none |

The `0` in `M <TOKEN> 0` asks hamlib for the radio backend's default
passband. coppad never answers from a cached or default value: every reply
is a fresh reading from the rig.

Reads work whenever the PTT backend is rigctld. Sets also need:

```toml
[radio]
host_rig_control = true
```

`host_rig_control` is `false` by default. The command port has no
authentication, and any web page you visit can send it a VARA line with a
no-CORS POST (see `docs/LIMITATIONS.md`). With sets allowed, such a page
could retune the rig.

coppad refuses a set with `WRONG` and logs one `INFO` line with the reason.
It checks in this order and reports the first reason that applies:

1. `[radio] host_rig_control` is `false`.
2. The PTT backend is not rigctld. This includes a rigctld that was
   unreachable at startup, where coppad fell back to no PTT.
3. The frequency is 0 or above 30 GHz.
4. coppad is transmitting, or has sessionless data queued to send.
5. A session is live (connecting, connected or disconnecting).

With the default config, the log line reads:

```text
INFO coppad::event_loop: FREQ 7074000 refused: host rig control is off; set [radio] host_rig_control = true to allow it
```

The reply to a set is what the rig reports afterwards, not what the host
asked for. If the rig accepts `F 7074005` but reports 7074000, the host gets
`FREQ 7074000` and coppad logs a warning. A host should compare the reply
with its request.

```text
WARN coppad::event_loop: FREQ 7074005: rig reports 7074000 Hz after the set
```

A retune does not get around the frequency gate (COP-40). On a wideband
profile, coppad reads the rig's frequency before each transmission and
refuses it below 29.7 MHz. A host retune below 29.7 MHz on `VHF_FAST`
therefore gets a warning, and every later transmission is refused until the
rig is retuned:

```text
WARN coppad::event_loop: FREQ 14074000: the rig is now below 29.7 MHz; the VHF_FAST profile's wideband gate will refuse every transmission until it is retuned
```

coppad talks to rigctld on its main event loop, as it does for PTT. A
healthy rigctld answers in milliseconds. A dead one stalls the loop for up
to about 4 s per query and about 6 s per set (a 2 s reconnect and a 2 s
read for each step).

Only the VARA command port has these commands. The WebSocket API, the ARDOP
command port and the C FFI have no frequency or mode control.

## BUSY lines on the command port

coppad watches the 300-2800 Hz band of the received audio and sends `BUSY ON`
or `BUSY OFF` to every connected host when the channel's occupancy changes.
A new state is reported only after it has held for 250 ms of received audio,
so:

- two consecutive `BUSY` lines are at least 250 ms of audio apart;
- a station that keys up is reported about 250 ms later, and its end is
  reported about 250 ms after it stops (a long broadband signal can read
  clear earlier, see the known gap below). When coppad reads a long stretch of
  buffered audio at once (after a stall, or while it waits to transmit) and
  reports a change for it, the next change can be reported up to that
  stretch's length later: at most the length of coppad's input ring,
  `[audio] buffer_size` samples (about 170 ms at the default 8192 samples
  and 48 kHz; a larger `buffer_size` raises this bound in proportion);
- a burst shorter than 250 ms is never reported.

Detection always runs. `[station_id] busy_hold_ms` does not change it: that
key only enables coppad's own courtesy wait before it transmits.

Known gap: the noise floor is estimated across the whole audio spectrum, not
only the 300-2800 Hz band. A receiver whose audio is much quieter outside
that band, which is typical of an SSB rig, can read as one long `BUSY ON` on
a quiet band. The floor also adapts to a steady broadband signal, so a long
transmission can read as `BUSY ON` followed by `BUSY OFF` within half a second
while it is still on the air, and coppad's own courtesy wait then lets its
transmission out over it.

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

For the OFDM KISS datagram port (8100), see [RETICULUM.md](RETICULUM.md).

## Session fade protection

Set `[session] fade_protection = "request"` to ask a 0.8.0 peer for RS(12,6)
erasure protection. The default `"accept"` uses it only when a peer asks.
`"off"` disables it even if a peer requests it. Two default stations keep the
existing session data path. Older peers cannot negotiate this mode.

Protection sends twelve packets in one keying and rebuilds a segment from any
six. At level 2 a burst takes about 16.4 seconds, before turnaround and ACKs.
This trades clean-channel throughput and latency for fade tolerance. When
`[radio] max_tx_duration_s` is too short for a burst with its safety margin,
the daemon sends the segment as a plain frame and warns once. This is a
session config setting; there is no VARA or ARDOP command to request it.
See SPEC §22 and ADR-012 for the protocol and BENCHMARKS for measurements.
