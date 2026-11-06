# G3RUH 9600 Direwolf comparison (COP-116)

On 2026-11-04, coppa delivered 100 frames from the pinned 100-frame 10 dB
recording. Direwolf delivered 100, so the 95-frame acceptance bar was met.
This measures the TNC demodulator after the sound card. It does not test RF,
a sound card, the KISS server, or a running TNC process.

## Method and pins

The generator is Direwolf 1.7, Ubuntu `1.7+dfsg-2build2`, unpacked without root.
The input is the 100 synthetic N0CALL lines in `testdata/g3ruh/frames.txt`.
`gen_packets -B 9600 -r 48000 -a 50` creates clean scrambled baseband audio.
`g3ruh_fixture_gen` adds `coppa_channel::awgn_seeded(samples, 10.0, 42)`.
Noise power is mean-square clean signal power divided by 10. Quantization rounds
and saturates to signed 16-bit PCM. The 48000 Hz mono WAV is 388813 samples,
777670 bytes, about 8.1 seconds.

Fixture: `testdata/g3ruh/g3ruh_9600_100frames_10db.wav`.
SHA-256: `11487f60768c86e6eb841fab197016232f85f0447565806eab2fcff28b7d80e5`.
The offline test `delivers_at_least_95_percent_of_direwolf` pins the reference
count at 100 and decodes in the daemon's 1024-sample block size.
The measurement used a release build on base `7b00317` plus COP-116 changes.
No resampling was needed.

A packet is an FCS-valid AX.25 frame. Coppa merges exact-byte copies from the
five phase banks within 150 ms; frames.txt has 100 distinct frames. Direwolf
counts every reception. The ratio is `100 * coppa / reference`, to one decimal
place. The bar is `(95 * reference).div_ceil(100)`.

Direwolf's command was `direwolf -c direwolf.conf -t 0`, receiving channel-0
16-bit little-endian PCM on stdin, with this exact configuration:

```text
ADEVICE stdin null
ACHANNELS 1
ARATE 48000
CHANNEL 0
MODEM 9600
AGWPORT 0
KISSPORT 0
```

`atest` ran as `atest -B 9600 testdata/g3ruh/g3ruh_9600_100frames_10db.wav`.
Both counts are channel-0 frame lines after stripping ANSI escapes. The
`atest` count also matched its own summary. Its banner has a blank profile,
which the report labels `unknown`.

## Live output

Generator output, ANSI colours stripped, first six lines:

```text
Data rate set to 9600 bits / second.
Using scrambled baseband signal rather than AFSK.
Audio sample rate set to 48000 samples / second.
Amplitude set to 50%.
Output file set to /tmp/cop116-fixture/clean.wav
Reading from testdata/g3ruh/frames.txt ...
```

The remaining generator lines echo the 100 input messages. Direwolf banner:

```text
Dire Wolf version 1.7
Includes optional support for:  gpsd hamlib cm108-ptt
Warning: Could not open 'symbols-new.txt'.
The "new" OVERLAID character information will not be available.

Reading config file /tmp/cop116-fixture/direwolf.conf
Audio input device for receive: stdin  (channel 0)
Audio out device for transmit: null  (channel 0)
Channel 0: 9600 baud, K9NG/G3RUH, +, 48000 sample rate x 3.
The ratio of audio samples per sec (48000) to data rate in baud (9600) is 5.0
```

Verbatim release diagnostic report:

```text
input: g3ruh_9600_100frames_10db.wav, 48000 Hz, 1 channels, 16-bit, 8.1 s
coppa:    100 packets (G3RUH 9600 demodulator, input at 48000 Hz)
coppa by start offset: 0:100 1:100 2:100 3:100 4:100 (spread 100 to 100)
direwolf: 100 packets (Dire Wolf version 1.7, profile +)
atest:    100 packets (profile unknown)
coppa / direwolf: 100.0% (95% bar: 95 packets, met)
coppa / atest:    100.0% (95% bar: 95 packets, met)

```

| Decoder | Settings | Frames | coppa as a percentage of it | 95 percent bar |
|---|---|---:|---:|---|
| coppa G3RUH 9600 | 48000 Hz, no resampling | 100 | | |
| Direwolf 1.7, direwolf | MODEM 9600, profile + | 100 | 100.0 | 95, met |
| Direwolf 1.7, atest | -B 9600, blank profile | 100 | 100.0 | 95, met |

Rust TX was also checked separately. A temporary Rust integration test wrote
`g3ruh::modulate(&afsk::append_fcs(frame))` as mono 16-bit 48000 Hz PCM.
The addressed AX.25 UI frame carried `COP-116 Rust G3RUH TX probe`.
Direwolf 1.7 decoded the output:

```text
$ atest -B 9600 /tmp/cop116-fixture/rust-tx.wav
1 from /tmp/cop116-fixture/rust-tx.wav
1 packets decoded in 0.000 seconds.  234.7 x realtime
```

## Limits

This is one synthetic recording, one noise seed and one Direwolf build.
The receiver has five fixed phases and no symbol-timing recovery. It does not
prove tolerance to sample-clock drift or radio filtering. The plan's Python
prototype recovered 57 to 64 frames per individual bank and 100 from their
union on its own same-shape fixture. Those per-bank prototype figures are
not a fresh measurement on the committed Rust-noised WAV.
Identical payloads received within 150 ms can be merged, including real repeats.

## How to reproduce

Install `direwolf`, or use the rootless bundle recipe in
[tools/tnc-bench/README.md](../../tools/tnc-bench/README.md).

```sh
tools/tnc-bench/gen_g3ruh_9600.sh --parity
cargo run --release -q -p coppa-bench --example g3ruh_vs_direwolf -- \
    --wav testdata/g3ruh/g3ruh_9600_100frames_10db.wav --log-dir /tmp/coppa-parity
cargo test -p coppa-codec --test g3ruh_9600_fixture
```

The generation script compares SHA-256 before replacing an existing fixture.
Only `--force` allows a differing recording. Update the integrity pin and rerun
both reference decoders before accepting a deliberate regeneration.
