# IL2P interoperability with Direwolf 1.7

COP-115 was measured on 2026-11-05 using Dire Wolf version 1.7, Ubuntu
package `1.7+dfsg-2build2`, and the upstream `1.7` tag for fixture generation.
The format is Direwolf's IL2P with no trailing CRC, not the later draft v0.6.

## Independent audio to Coppa

`testdata/il2p/README.md` pins the fixtures and generation commands. Seven
mono 48 kHz PCM16 recordings decode to their complete expected FCS-free
AX.25 frames. The test compares bytes, not packet counts or monitor text.
It covers normal and inverted polarity, automatic and maximum FEC, type-1
UI/I/S/U headers, transparent type 0 with a digipeater, binary KISS escape
bytes, and the maximum 1023-byte multi-block information field.

Run without any external decoder installed:

```sh
cargo test --test il2p_interop direwolf_recordings_decode_byte_exactly
cargo test -p coppa-daemon --features kiss-tnc --lib il2p_and_ax25_share_fcs_free_kiss_adapter
```

Both passed. The second runs the production dual receiver and its FCS
normalization, so it also detects stripping the final two IL2P payload bytes
or delivering duplicates. The codec tests compare independent wire bytes at
all planned payload boundaries for both FEC levels, correct one header
symbol and up to parity/2 symbols in each payload block, and reject known
uncorrectable and omitted-padding error patterns.

## Coppa audio to real Direwolf

Run the external test explicitly. A missing or different-version oracle fails
this command with a setup error rather than silently skipping it:

```sh
DIREWOLF=/path/to/direwolf cargo test --test il2p_interop direwolf_accepts_coppa -- --ignored --nocapture
DIREWOLF=/path/to/direwolf cargo test -p coppa-daemon --features kiss-tnc --lib direwolf_accepts_tnc_il2p -- --ignored --nocapture
```

This run used `/tmp/cop115/dw/usr/bin/direwolf` with its extracted libraries
in `LD_LIBRARY_PATH`. The test invokes `direwolf -c <temporary-config> -t 0
-d 22`. The config uses `ADEVICE stdin null`, `ACHANNELS 1`, `ARATE 48000`,
`MYCALL N0CALL`, `AGWPORT 0` and a dynamically allocated loopback KISS port.
It waits for the KISS listener before writing signed little-endian PCM16,
retains lead-in and tail silence, and keeps stdin open until KISS delivery.

Actual per-case KISS comparisons passed:

- UI with binary payload and escape bytes: 28 bytes equal.
- Transparent AX.25 with a digipeater path: 39 bytes equal.
- Maximum multi-block UI: 1039 bytes equal, including all 1023 information bytes.

The daemon test selects the production `TxMode::Il2p` adapter before passing
its audio to the same external oracle. No invalid headers or decode failures
were reported. All reported header corrections were zero. Every decoder in
Direwolf's final FEC status spectrum reported either zero corrections or no
candidate. Direwolf 1.7 labels this IL2P status as `FX.25`; the test checks the
numeric spectrum rather than treating that label as the wire format.

These are clean audio tests. No claim is made about live RF, noisy-channel
performance, or COP-99's 95-percent decoding target. IL2P's RS code has no
trailing CRC in this version. Some errors beyond its correction bound can
be miscorrected and delivered undetected.
