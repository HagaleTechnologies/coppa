# G3RUH 9600 fixture

`g3ruh_9600_100frames_10db.wav` is self-generated synthetic N0CALL traffic.
It contains no third-party recording or traffic, and is distributed under this
repository's licence. `frames.txt` holds the exact 100 TNC2 monitoring lines.

Direwolf 1.7, Ubuntu package `1.7+dfsg-2build2`, generated the clean signal:

```sh
gen_packets -B 9600 -r 48000 -a 50 -o clean.wav testdata/g3ruh/frames.txt
```

`g3ruh_fixture_gen` applies `coppa_channel::awgn_seeded(samples, 10.0, 42)`.
Noise power is the mean square of the clean samples divided by 10.
The result is quantized to signed 16-bit PCM, rounded and saturated.
It is mono at 48000 Hz, 388813 samples, 777670 bytes, about 8.1 seconds.

SHA-256: `11487f60768c86e6eb841fab197016232f85f0447565806eab2fcff28b7d80e5`.
Both `direwolf` with `MODEM 9600` and `atest -B 9600` decode 100 frames.
The live record is [tnc-direwolf-g3ruh-9600.md](../../docs/testing/tnc-direwolf-g3ruh-9600.md).

Regenerate and compare with the pin:

```sh
tools/tnc-bench/gen_g3ruh_9600.sh --no-parity
```

`--force` deliberately replaces a differing fixture. A changed pin requires a
fresh reference measurement and an explicit update to the integrity test.
