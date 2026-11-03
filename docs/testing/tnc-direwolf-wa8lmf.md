# coppa's AFSK 1200 TNC against Direwolf on WA8LMF TNC Test CD track 2: measurement record (COP-112)

This records the baseline that COP-99 measures against: how many AX.25 packets coppa's AFSK
1200 demodulator decodes from track 2 of the WA8LMF TNC Test CD, next to Direwolf's count from
the same file. COP-99's target is at least 95 percent of Direwolf's count. It is evidence about
the AFSK demodulator, not about RF performance.

## Setup

- Date: 2026-11-03. One Linux x86_64 container (Ubuntu 24.04.5 LTS), no sound card, no radios.
- coppa: `coppa-bench --example afsk_vs_direwolf`, release build, on the COP-112 branch (main
  at `2526b50` plus this change).
- Resampler: `rubato` 0.16.2 (from `Cargo.lock`). A change to the resampler can move coppa's
  count, so a new result must name the version it ran with.
- Direwolf: Ubuntu noble's `direwolf` package `1.7+dfsg-2build2` (banner `Dire Wolf version
  1.7`), unpacked without root with `apt-get download` and `dpkg -x` as
  [tools/tnc-bench/README.md](../../tools/tnc-bench/README.md) describes.
- flac: `flac 1.4.3` (Ubuntu package `1.4.3+ds-2.1ubuntu2`).
- Pins, all checked by `tools/tnc-bench/run.sh` on this run:
  - `TNC_Test_2.iso` SHA-256 `28a115891ba94aa9df9fe00abfe17dde190804f2013647c118b38225fc69e09c`
  - track 2 FLAC SHA-256 `534323df93cbb9ad52febbfed830dd473f9e7a01cef891b93205ee2264ed6d45`
  - track 2 FLAC MD5 of the audio (`metaflac --show-md5sum`) `69c07d25ae4d77f36910fcc20fa9758f`
  - decoded `track2.wav` SHA-256 (flac 1.4.3)
    `006e35196b05ce6e22c9e824111c49dd7928e80e29749f9fad1787069c8ae495`

## The track

The CD is published as an ISO at <http://wa8lmf.net/TNCtest/TNC_Test_2.iso>. Track 2 is the
25:50 de-emphasized recording of 144.39 MHz APRS traffic. Inside the ISO it is
`Audio-Tracks/02_100-Mic-E-Bursts-DE-emphasized.flac`, despite the name. Its length shows it:

```text
$ metaflac --show-total-samples --show-sample-rate 02_100-Mic-E-Bursts-DE-emphasized.flac
68351472
44100
$ metaflac --show-total-samples --show-sample-rate 03_100-Mic-E-Bursts-Flat.flac
13552812
44100
$ atest 03_100-Mic-E-Bursts-Flat.wav | tail -n 1
100 packets decoded in 2.711 seconds.  113.4 x realtime
```

File 02 is 1549.9 s (25:50). File 03 is 307.3 s and holds the 100 Mic-E bursts. The script
identifies track 2 by its SHA-256, not by its name. Both channels of track 2 are identical, and
both decoders read channel 0.

The page and the ISO state no licence, permission or redistribution terms, the recording carries
other stations' traffic, and the ISO is 254 MB. So `run.sh` fetches it at run time and nothing
is committed. The page asks: "Please DO NOT play these test tracks over-the-air! My callsign is
embedded in the beacons and will be igated."

## What is counted

- **A packet** is one AX.25 frame whose FCS checks. Every reception counts, so a digipeater's
  repeat of a frame counts again. Neither side removes duplicates. This is the rule `direwolf`
  and `atest` use for their own counts, and coppa's `Demodulator` keeps every FCS-valid frame
  (`crates/coppa-codec/src/afsk.rs`, the end of the frame check).
- **coppa** is the TNC's receive chain after the sound card. The 44.1 kHz file goes through
  `coppa_audio::resampler::ResamplingSource` to 48 kHz, the type `LiveSource` uses for a card
  that cannot run at 48 kHz (`crates/coppa-audio/src/cpal_backend.rs`). Then
  `afsk::Demodulator::process` takes 1024-sample reads, as `run_demod_worker` does
  (`crates/coppa-daemon/src/tnc.rs`). The KISS server stage after the demodulator is outside
  the count. The code is `crates/coppa-bench/src/afsk_direwolf.rs`.
- **direwolf** is `direwolf -c direwolf.conf -t 0` with channel 0 of the WAV on stdin as 16-bit
  little-endian samples. The config holds only the input/output lines and the two lines that
  stop it opening network ports, so every demodulator setting is Direwolf's default:

  ```text
  ADEVICE stdin null
  ACHANNELS 1
  ARATE 44100
  AGWPORT 0
  KISSPORT 0
  ```

- **atest** is `atest track2.wav` with no options. Its count of `[0]` frame lines must equal
  its own `N packets decoded in` summary, or the run fails.
- Both Direwolf counts are the lines that start `[0] ` or `[0.N] ` after ANSI escapes are
  stripped. Each program picks its own demodulator profile, and the report prints the one from
  its banner.
- **coppa / direwolf** is 100 times coppa's count divided by `direwolf`'s, to one decimal
  place. **coppa / atest** is the same against `atest`.
- **The 95 percent bar** is the smallest whole packet count at or above 95 percent of the
  reference, `(95 * reference).div_ceil(100)`. It is met when coppa's count is at or above it.

## Results

Verbatim `tools/tnc-bench/run.sh --keep` output, stderr and stdout together (scratch and
package paths shortened):

```text
$ tools/tnc-bench/run.sh --keep
tnc-bench: downloading http://wa8lmf.net/TNCtest/TNC_Test_2.iso to …/coppa-tnc-bench/
tnc-bench: TNC_Test_2.iso sha256 OK
tnc-bench: track 2 is Audio-Tracks/02_100-Mic-E-Bursts-DE-emphasized.flac, sha256 OK
tnc-bench: flac 1.4.3 decoded track2.wav, sha256 OK
tnc-bench: coppa 2526b50 (uncommitted changes), direwolf …/usr/bin/direwolf, atest …/usr/bin/atest
tnc-bench: building and running coppa-bench --example afsk_vs_direwolf (release)
tnc-bench: kept …/tnc-bench.NJKpzW
input: track2.wav, 44100 Hz, 2 channels, 16-bit, 1549.9 s
coppa:    412 packets (AFSK 1200 demodulator, resampled 44100 -> 48000 Hz)
coppa by start offset: 0:412 5:416 10:398 15:399 20:390 25:397 30:388 35:407 (spread 388 to 416)
direwolf: 1000 packets (Dire Wolf version 1.7, profile A+)
atest:    982 packets (profile A)
coppa / direwolf: 41.2% (95% bar: 950 packets, not met)
coppa / atest:    42.0% (95% bar: 933 packets, not met)
```

The run exited 0 and took 62 s of wall time, including the download and a release build of
`coppa-bench`.

| Decoder | Settings | Packets | coppa as a percentage of it | 95 percent bar |
|---|---|---:|---:|---|
| coppa AFSK 1200 demodulator | input resampled 44100 to 48000 Hz | 412 | | |
| Dire Wolf 1.7, `direwolf` | its default profile, `A+` | 1000 | 41.2 | 950, not met |
| Dire Wolf 1.7, `atest` | its default profile, `A` | 982 | 42.0 | 933, not met |

The headline reference is `direwolf`: it is the program an operator runs, and it gives the
stricter bar.

## Noise

coppa's count depends on where the input starts. With the input starting 0, 5, 10, 15, 20, 25,
30 and 35 samples later (all under one 1200-baud symbol at 44.1 kHz), coppa decodes 412, 416,
398, 399, 390, 397, 388 and 407 packets: a spread of 388 to 416, or 28 packets. The headline
count is the one at offset 0. A change smaller than this spread does not show an improvement.

## Not covered

- One track of the CD.
- One Direwolf build (Ubuntu noble's 1.7) on one platform.
- No sound card, KISS stage, running `coppa tnc` process or RF.
- No frame-by-frame comparison of which packets each decoder finds.
- No cause for the gap. The record states counts only.

## Reproduce

Run `tools/tnc-bench/run.sh`. Its requirements, options and exit codes are in
[tools/tnc-bench/README.md](../../tools/tnc-bench/README.md).
