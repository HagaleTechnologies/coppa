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
