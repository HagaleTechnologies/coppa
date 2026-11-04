# tnc-bench: coppa's AFSK 1200 TNC against Direwolf on WA8LMF track 2

`run.sh` counts the AX.25 packets that coppa's AFSK 1200 demodulator and Direwolf decode from
track 2 of the WA8LMF TNC Test CD, and prints coppa's count as a percentage of Direwolf's. It
sets the baseline that COP-99 measures against (COP-112). The latest run is recorded in
[docs/testing/tnc-direwolf-wa8lmf.md](../../docs/testing/tnc-direwolf-wa8lmf.md), which also
defines the metric.

## What it measures, and what it does not

coppa's side is the TNC's receive chain after the sound card: the track goes through the same
44.1-to-48 kHz resampler `coppa tnc` uses for a card that cannot run at 48 kHz, then into
`afsk::Demodulator` in 1024-sample reads, as the TNC's demod worker does. Direwolf's side is
the `direwolf` program with an input/output-only config, and Direwolf's offline `atest` program
with no options.

It does not cover the KISS server, RF, a sound card, or a running `coppa tnc` process. It does
not compare which packets each decoder finds, only how many.

## The track

The WA8LMF TNC Test CD is published as an ISO at <http://wa8lmf.net/TNCtest/>. Track 2 is the
25:50 de-emphasized recording of 144.39 MHz APRS traffic. Inside the ISO it is the file
`Audio-Tracks/02_100-Mic-E-Bursts-DE-emphasized.flac`, despite that name: the file is 1549.9 s
long, while `03_100-Mic-E-Bursts-Flat.flac` holds the 307.3 s of bursts. The script identifies
the track by SHA-256, never by name.

No redistribution terms are stated on the page or in the ISO, so the script fetches the ISO at
run time and nothing is committed to this repo. The page asks: "Please DO NOT play these test
tracks over-the-air! My callsign is embedded in the beacons and will be igated."

The script checks three SHA-256 pins: the ISO before extraction, the track's FLAC after it, and
the decoded WAV. A mismatch on the ISO or FLAC stops the run with exit 2. `flac -d` checks the
audio against the MD5 stored in the FLAC header (the record lists it as a fourth pin), so a WAV
that differs from its pin (the header bytes depend on the `flac` build) only prints a note.

## Requirements

- Linux or macOS.
- `python3`, `flac`, `sha256sum` (or `shasum` on macOS), `cargo`, and `curl` for the first run.
- Direwolf 1.7: the `direwolf` and `atest` programs (Debian/Ubuntu: `apt install direwolf flac`).
- About 600 MB of disk (the 254 MB cached ISO under
  `${XDG_CACHE_HOME:-~/.cache}/coppa-tnc-bench/`, and 341 MB of scratch) and about 1 GB of RAM.

Without root on Ubuntu 24.04, unpack the packages into one directory instead:

```text
mkdir -p ~/dw/debs && cd ~/dw/debs
apt-get download direwolf flac libflac12t64 libogg0 libgps30t64 libhamlib4t64 \
    libusb-1.0-0 libnova-0.16-0t64 libindiclient1
for d in *.deb; do dpkg -x "$d" ~/dw/root; done
export PATH=~/dw/root/usr/bin:$PATH
export LD_LIBRARY_PATH=~/dw/root/usr/lib/x86_64-linux-gnu:~/dw/root/usr/lib
```

Without `libindiclient1`, `direwolf` fails with `libindiclient.so.1: cannot open shared object
file`.

## Usage

```text
$ tools/tnc-bench/run.sh
tnc-bench: downloading http://wa8lmf.net/TNCtest/TNC_Test_2.iso to /home/op/.cache/coppa-tnc-bench/
tnc-bench: TNC_Test_2.iso sha256 OK
tnc-bench: track 2 is Audio-Tracks/02_100-Mic-E-Bursts-DE-emphasized.flac, sha256 OK
tnc-bench: flac 1.4.3 decoded track2.wav, sha256 OK
tnc-bench: coppa 2526b50 (uncommitted changes), direwolf /usr/bin/direwolf, atest /usr/bin/atest
tnc-bench: building and running coppa-bench --example afsk_vs_direwolf (release)
input: track2.wav, 44100 Hz, 2 channels, 16-bit, 1549.9 s
coppa:    412 packets (AFSK 1200 demodulator, resampled 44100 -> 48000 Hz)
coppa by start offset: 0:412 5:416 10:398 15:399 20:390 25:397 30:388 35:407 (spread 388 to 416)
direwolf: 1000 packets (Dire Wolf version 1.7, profile A+)
atest:    982 packets (profile A)
coppa / direwolf: 41.2% (95% bar: 950 packets, not met)
coppa / atest:    42.0% (95% bar: 933 packets, not met)
```

The `tnc-bench:` progress lines go to stderr and the seven report lines go to stdout. The
`downloading` line appears only when the ISO is not cached yet. The `coppa` line says
`(tree clean)` when `git status --porcelain --untracked-files=no` prints nothing.

The report lines:

- `input`: the WAV the decoders read. Channel 0 is used (both channels of this track are equal).
- `coppa`: packets with a valid FCS from coppa's demodulator, input starting at sample 0. This is
  the headline count.
- `coppa by start offset`: the same count with the input starting 0 to 35 samples later, and its
  spread. A change smaller than the spread is not evidence of an improvement.
- `direwolf` and `atest`: each program's channel 0 packet count, with the profile from its
  banner. `atest`'s count must equal its own `N packets decoded in` summary or the run fails.
- `coppa / direwolf` and `coppa / atest`: coppa's count as a percentage of each, and the
  95 percent bar (the smallest whole count at or above 95 percent of the reference). The bar
  never changes the exit status.

Options: `--iso PATH` uses a local ISO, `--direwolf PATH` and `--atest PATH` pick the programs,
and `--keep` keeps the scratch directory (the WAV, `direwolf.conf`, `direwolf.txt` and
`atest.txt`) and prints its path. Exit status: 0 report printed, 1 a decoder failed, 2 setup
error (a missing tool, a failed download, a checksum mismatch, a failed extraction or
decode, or a failed build of the diagnostic).

The program underneath also runs on any 16-bit PCM WAV:

```text
cargo run --release -q -p coppa-bench --example afsk_vs_direwolf -- --wav FILE.wav
```

## Gotchas

- `direwolf` and `atest` pick different default profiles on this 44.1 kHz file (`A+` and `A`)
  and decode different counts. The report prints both and uses `direwolf` as the headline,
  because it is the program an operator runs. `atest`'s usage text names `E+` as its default;
  the report prints what the banner says.
- coppa's demodulator is fixed at 48 kHz. Fed the 44.1 kHz audio directly, it decodes nothing,
  which is why the resampler is part of the chain.
- coppa's count moves by tens of packets when the input starts a few samples later. Compare a
  change against the spread the report prints.
- A change to the resampler (`rubato` in `Cargo.lock`) can move coppa's count. Record the
  version with any new result.
