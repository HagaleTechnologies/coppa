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
