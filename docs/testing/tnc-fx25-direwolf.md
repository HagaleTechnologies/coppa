# COP-114 FX.25 interoperability record

On 2026-11-05 the implementation run built Direwolf 1.7, commit
`de293a1f2526ec6639fe31fa411bd4f2319ecdf4`, with CMake Release outside the coppa
checkout. Both directions were exercised at 1200 baud, mono PCM16, 48 kHz.
Reproduction and fixture generation are in `tools/fx25-interop/README.md`.

## Direwolf to coppa

`gen_packets -B 1200 -r 48000 -X 32 -o direwolf-clean.wav packets.txt`
encoded `N0CALL>APRS:coppa COP-114 FX25 fixture` followed by a newline. This
selected tag 7 with 64 data bytes and 32 parity bytes. The clean WAV is retained
unchanged under `crates/coppa-codec/tests/fixtures/fx25/`.

The fixture generator exports the reference transmitter's data and parity
before modulation. It XORs data byte offsets 8 through 15 with ff, leaving
parity unchanged, then renders the damaged independent block with a test-only
Python NRZI/AFSK renderer. This is controlled byte damage, not recorded RF noise.
The derivative corrects eight erroneous bytes, within tag 7's sixteen-byte limit.

`recorded_direwolf_clean_and_damaged_audio` delivers exactly the original frame
from each WAV, including its correct FCS. Removing the damaged block's tag makes
the ordinary HDLC/FCS control deliver nothing. The daemon test
`damaged_direwolf_frame_reaches_kiss_without_fcs` exercises the service loop's
actual conversion and verifies the exact 43-byte KISS payload without FCS.
All eleven independently exported profile vectors also match coppa's parity and
recover zero, one and parity/2 errors, including first and last wire symbols.

The untouched reference starts its tone at sample 1300, which is 20 modulo the
receiver's 40-sample symbol period. It fails at input offset zero and succeeds
at offset 20. The test explicitly uses offset 20 without altering the fixture.
The damaged renderer starts at a symbol boundary and uses offset zero. No new
acquisition claim is made. The existing WA8LMF acquisition gap remains.

## Coppa to Direwolf

`tools/fx25-interop/run.sh DIREWOLF_SOURCE` exited 0. It generated one WAV through
`cargo run -p coppa-bench --example fx25_wav`, then ran the real reference decoder:

```text
atest -B 1200 -F 0 -d x -h -L 1 -G 1 coppa.wav
FX.25[0.0]: Matched correlation tag 0x07 with 0 bit errors. Expecting 64 data & 32 check bytes.
FX.25[0.0]: FEC complete with no errors.
[0] N0CALL>APRS:coppa COP-114 FX25 fixture<0x0a>
```

The checker independently requires the zero-error FEC diagnostic and compares
the complete AX.25 dump with these expected bytes:

```text
82 a0 a4 a6 40 40 e0 9c 60 86 82 98 98 e1 03 f0
63 6f 70 70 61 20 43 4f 50 2d 31 31 34 20 46 58
32 35 20 66 69 78 74 75 72 65 0a
```

Plain-HDLC bit repair was disabled with `-F 0`. This proves valid FX.25 framing,
shortening, parity and audio compatibility separately from the damaged inbound
fixture. It does not establish noisy-channel acquisition, sound-card or RF
performance, or COP-99's wider benchmark acceptance bar.
