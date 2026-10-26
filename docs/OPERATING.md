# Operating Coppa

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
3. Key the calibration tone: `coppa tune` from the CLI (default 10 seconds,
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
| Restart after any of the above | coppad releases PTT at startup (rigctld `T 0`, or the serial/GPIO line lowered). If that fails, it logs `Could not release PTT at startup; ...` and starts anyway. |

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

### What is not covered

Nothing in coppad can release PTT in these cases:

- A SIGKILL to every process in coppad's cgroup, which kills the guardian too.
  systemd does this when a stop takes longer than `TimeoutStopSec`. `pkill -9
  coppad` does the same.
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
`websocket_enabled = true` under `[host]`, connect any WebSocket client (for
example `websocat ws://127.0.0.1:8400`) and send:

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

Known gap: the hold is receive-side. A station with several ARQ segments
queued re-keys for the next one the moment its own PTT releases, so the
other station's ACK can still collide with that next frame. Sender-side turn
discipline is a separate follow-up. The hardware acceptance test for this
behaviour is `docs/testing/cable-loop-turnaround.md`.
