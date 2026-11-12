> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-129: golden vectors for levels 3, 4, 7 and 9, multi-codeword frames and retransmission

Measured 2026-11-08 on x86_64 Linux, release build. The golden corpus
(`testdata/golden/`, `docs/SPEC.md` §14) grew from 20 to 42 vectors. The
generator is now additive: it kept the 20 existing entries and WAVs byte for
byte and appended 22. Regenerating the original 20 on this machine does not
reproduce the committed files (1-LSB differences in 15 WAVs, and a different
`L5_awgn12` seed), which is why it no longer rewrites them. A second run
reports `Kept 42, wrote 0` and leaves every file's SHA-256 unchanged.

Generator output for the new entries
(`cargo run -p coppa-bench --release --example golden_vectors_gen`):

```
L3_clean: seed=0x22BFEC334B90DB61 attempts=1 payload_bytes=117 samples=37800
L3_awgn12: seed=0x22BFEC334B90DB72 attempts=1 payload_bytes=117 samples=37800
L3_poor25: seed=0x22BFEC334B90DB45 attempts=1 payload_bytes=117 samples=37800
L3_ssbcfo: seed=0x22BFEC334B90DBE2 attempts=1 payload_bytes=117 samples=37800
L4_clean: seed=0x22BFE7334B90D2E0 attempts=1 payload_bytes=178 samples=37800
L4_awgn12: seed=0x22BFE7334B90D2F3 attempts=1 payload_bytes=178 samples=37800
L4_poor25: seed=0x22BFE7334B90D2C4 attempts=1 payload_bytes=178 samples=37800
L4_ssbcfo: seed=0x22BFE7334B90D263 attempts=1 payload_bytes=178 samples=37800
L7_clean: seed=0x22BFE8334B90D095 attempts=1 payload_bytes=178 samples=10200
L7_awgn12: seed=0x22BFE8334B90D086 attempts=1 payload_bytes=178 samples=10200
L7_poor25: seed=0x22BFE8334B90D0B1 attempts=1 payload_bytes=178 samples=23940
L7_ssbcfo: seed=0x22BFE8334B90D016 attempts=1 payload_bytes=178 samples=23940
L9_clean: seed=0x22BFF2334B90E197 attempts=1 payload_bytes=198 samples=9180
L9_awgn12: seed=0x22BFF2334B90E184 attempts=1 payload_bytes=198 samples=9180
L4_awgn12_cw3: seed=0x08839B168289708C attempts=1 payload_bytes=532 samples=93240 codewords=3
L7_awgn12_cw3: seed=0x1DD62646F1DE0817 attempts=1 payload_bytes=533 samples=20400 codewords=3
L9_awgn12_cw8: seed=0x43AB07E9C9262B80 attempts=1 payload_bytes=1577 samples=30600 codewords=8
L9_awgn12_rtx_rv2: seed=0x661851E7B89F7D89 attempts=1 payload_bytes=198 samples=32760 rv_sequence=[0, 2]
L9_awgn12_rtx_rv3: seed=0x661850E7B89F7BD6 attempts=1 payload_bytes=198 samples=32760 rv_sequence=[0, 3]
L9_awgn12_rtx_rv1: seed=0x66184EE7B89F7870 attempts=1 payload_bytes=198 samples=32760 rv_sequence=[0, 1]
L9_awgn12_rtx_rv0: seed=0x66184FE7B89F7A26 attempts=4 payload_bytes=198 samples=32760 rv_sequence=[0, 0]
L4_awgn12_rtx_rv2: seed=0x023E565E4057B082 attempts=3 payload_bytes=178 samples=90000 rv_sequence=[0, 2]
Kept 20, wrote 22 golden vectors to testdata/golden (42 in manifest)
```

A seed is accepted only if fresh receivers decode the 16-bit-quantized
samples the WAV stores: the batch `CoppaTransceiver::receive` returns the exact
payload and codeword count (single-frame entries), and a `StreamingReceiver`
fed 4,800-sample chunks plus 8,192 trailing zeros returns exactly one frame
with the exact payload. A retransmission entry also needs its first frame
alone to return no frame.

Operating points and why:

- Level 9 has only `clean` and `awgn12`. It is VHF-only (ADR-009), and
  `L9_ssbcfo` found no seed in 500 attempts.
- L7 and L9 `awgn12` run at 24 dB, the value L8 already used. At 12 dB
  neither found a seed in 500 attempts on `vhf_wide`.
- Multi-codeword entries use their level's `awgn12` point (L4 12 dB, L7 and
  L9 24 dB). The payloads (532, 533 and 1577 bytes) do not split evenly over
  the codewords, so each exercises SPEC §9.1's `+1` rule.
- Retransmission entries sit where the first frame alone fails 30 of 30 or
  close to it: L9 IR pairs at 19 dB, the L9 Chase pair at 20 dB, and L4 RV0
  then RV2 at 3 dB. L2, L3, L5 and L6 have no retransmission vector (see the
  table below).

Retransmission probe (plan phase). Each case is 30 seeds. The WAV is 4,800
zeros, RV0 frame, 4,800 zeros, RVk frame, 4,800 zeros, with AWGN over the
whole buffer referenced to frame 1's mean power, quantized to 16 bits and
decoded with a fresh `StreamingReceiver` (4,800-sample chunks, 8,192 trailing
zeros). "f1 fails" counts seeds where frame 1 alone gives no frame. "pair"
counts seeds where the whole WAV gives exactly one frame with the right
payload, carrying RVk.

| Case | SNR | f1 fails | pair |
|---|---|---|---|
| L9 RV0+RV2 | 17 / 19 / 21 dB | 30 / 30 / 24 | 21 / 25 / 22 |
| L9 RV0+RV3, RV0+RV1 | 19 dB | 30, 30 | 25, 25 |
| L9 RV0+RV0 (Chase) | 19 / 20 / 21 dB | 30 / 30 / 24 | 9 / 15 / 23 |
| L4 RV0+RV2 | 2.5 / 3 / 3.5 dB | 30 / 28 / 21 | 6 / 12 / 14 |
| L7 RV0+RV2 | 13.5 dB | 29 | 18 |
| L3 RV0+RV2 | 1 / 2 / 3 dB | 29 / 24 / 11 | 0 / 0 / 0 |
| L3 RV0+RV1, RV0+RV3 | 3 dB | 11, 11 | 6, 7 |

At L3, RV2 never rescued a failed RV0 frame, while RV1 and RV3 did (see
`docs/LIMITATIONS.md`). At L5 and L6, RV0 fails 30 of 30 at 12 dB and only 0
to 3 of 30 at 13 dB, so too few failures were left to measure combining.
`L4_awgn12_rtx_rv2` and the Chase vector come from windows where 40 to 50% of
seeds pass; other platforms were not measured. If one flips, regenerate that
entry by deleting it from the manifest.

The two golden suites take longer in the debug profile with the larger
corpus: `cargo test -p coppa-protocol --test golden_vectors` 35.6 s (was
18.9 s) and `cargo test -p coppa-cli --test rx_golden` 44.0 s (was 21.8 s).

