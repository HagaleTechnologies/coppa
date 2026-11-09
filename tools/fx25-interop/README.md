# FX.25 interoperability

Normal cargo tests use committed fixtures and need no external tools. The external
oracle is Direwolf 1.7 at `de293a1f2526ec6639fe31fa411bd4f2319ecdf4`.
Build it in a disposable directory outside this repository:

```sh
git clone --branch 1.7 --depth 1 https://github.com/wb2osz/direwolf.git "$scratch/direwolf"
cmake -S "$scratch/direwolf" -B "$scratch/direwolf/build" -DCMAKE_BUILD_TYPE=Release
cmake --build "$scratch/direwolf/build" -j4
tools/fx25-interop/run.sh "$scratch/direwolf"
```

Set `scratch` to a session-private directory first. Build prerequisites include
CMake, a C compiler and ALSA development headers on Linux. `run.sh` checks the
reference commit, generates audio using coppa's public FX.25 API, runs the actual
`atest -B 1200 -F 0 -d x -h -L 1 -G 1`, and checks tag 7, zero-error FEC, exactly
one decoded packet and its complete AX.25 hex dump. Missing tools or diagnostics
fail the command. This is a manual acceptance gate; ordinary cargo tests never
install or execute Direwolf.

Regenerate fixtures with a disposable source checkout at that same commit:

```sh
python3 tools/fx25-interop/generate.py "$scratch/direwolf" crates/coppa-codec/tests/fixtures/fx25
```

The generator inserts a small serialization hook into the external transmitter
immediately after Direwolf computes parity. It does not replace its framing,
FCS, stuffing or RS algorithm. This modifies the disposable external checkout.
`tag01.bin` through `tag11.bin` contain transmitted data followed by parity.
Their payload includes runs of ones and flag-like bytes. `manifest.json` records
provenance, SHA-256 sums and exact damage locations. Generated test data is
retained; no GPL implementation code is copied into this repository.

The untouched WAV is from Direwolf `gen_packets`. Its damaged derivative keeps
the independent tag-7 block, XORs eight data bytes with ff and uses the separate
Python NRZI/AFSK renderer. The source packet reader retains a trailing newline;
the expected AX.25 payload includes it. Since COP-113's recovered symbol clock,
coppa's receiver decodes both the untouched Direwolf recording and the
independently rendered derivative from input sample 0. See
`docs/testing/tnc-fx25-direwolf.md`.

`python3 -m unittest discover -s tools/fx25-interop` tests the output parser using
captured reference output and negative cases. A plain HDLC decode, a successful
exit code, a wrong payload or a nonzero-error FEC diagnostic alone cannot pass.
