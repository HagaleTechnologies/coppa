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
   transmit audio (soundcard output level, or the coppa-daemon TX gain) all
   the way down.
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
