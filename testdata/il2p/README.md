# Independent Direwolf 1.7 fixtures

These files are outputs of the upstream Direwolf `1.7` encoder. Coppa code
was not used to generate them. Expected `.ax25` files are complete frames
without FCS, reconstructed by Direwolf's own decoder from its wire output.
`.wire` files contain the scrambled header, parity and payload, excluding
the preamble and sync. WAVs are mono 48 kHz signed PCM16.

Reproduce with a C compiler, CMake and an external upstream source checkout:

```sh
curl -L https://codeload.github.com/wb2osz/direwolf/tar.gz/refs/tags/1.7 -o direwolf-1.7.tar.gz
tar -xzf direwolf-1.7.tar.gz
python3 testdata/il2p/generate.py /path/to/direwolf-1.7
sha256sum testdata/il2p/*.wav testdata/il2p/*.wire testdata/il2p/*.ax25 testdata/il2p/vectors.txt
```

`generate.py` adds raw-frame test input and output capture glue to that
external `gen_packets`, builds it, invokes `-I 1` for maximum FEC, `-I 0`
for automatic FEC or `-i 0` for inverted automatic FEC, always with `-r
48000 -o <file> -`, and restores the external source in a finally block.
It enlarges only the test input line buffer to accept maximum-size frames.
All input bytes are specified in the helper. No Direwolf implementation is
copied into Coppa. Direwolf is GPL-2.0-or-later; the helper calls its existing
encoder and decoder as an external fixture oracle.

`ui-max` includes NUL, FEND and FESC and distinct final bytes 91 27.
`ui-auto-inverted` has poll/final set and automatic FEC with inverted tones.
`i`, `s` and `u` cover representative mod-8 I, RNR and UA receive headers.
`transparent` includes a WIDE1-1 path. `maximum` has 1023 information bytes
cycling through all byte values. `vectors.txt` covers 0, 1, 61/62, 123/124,
185/186, 239/240, 247/248, 477/478 and 1023 information bytes for both FEC
levels. Lines give a case identifier and the external encoder's hex bytes.
The tests compare these bytes directly, not just self-loopback results.

SHA-256 pins:

```text
9e32b2cc46cbf5160821e6acbb1e94bd449f59daef20222cd3ef427222e00d5b  i.ax25
f892ab4ee7b90e41beca417bbaa7069a6c7f28defaed4c5629b4fcfb293c2ce5  i.wav
19ed277c1e77c53df880e97a18b87a5867d6c4b178bff7bcfba4b35bc2a8940f  i.wire
0e633af26aaf6c5b6fbe1e31e7e92f8c5cf1c21e1f3761e6f6ab8437a6b2ed20  maximum.ax25
0bc1594c350849919baaa24c2bb4d11fa655fe41122d07421e7562262ea033db  maximum.wav
13d88cbbd3b92b121bb74c700de92a1d18bbd25ab32c116f08a408cb1e07d0bf  maximum.wire
6e2dc682a3074941fd677d7a5ed987bb081658d083e01c8b55c671d224c0c7aa  s.ax25
9d1b4cb1abdd56ebde3acad5ee0932d7556ebcb5efaf7f052c8a059fc1fa4c54  s.wav
967a7f7049e4e67c493b06571fe4b46da0e9de133440ddec0c200f24b4145f84  s.wire
3de71ce9b469ac98fc0db06894cc6c37d96769b2dfde68be131505c87ab5b844  transparent.ax25
2ee2514fc288b0c06b2fe561ee69a6204083dbdced1a1bb0db91f02963cb7e85  transparent.wav
90a0fd4217dd92686eede39ce410cabf9d3dc7a29c924ab9e891f829437375ca  transparent.wire
776eddf313556cd97d78a838de4b25b65ff40191bfcce949f28e3c473b86fd95  u.ax25
b80edea15db61bd0df8ca27e8e50757a8527769846a48c8b179983917a63e9a7  u.wav
42a1a1c88f7c4a316f5a4d69990a9775ab14e30e2cf09e72bf1f4cb3eb9aa503  u.wire
2e068ea7b56fce21e0007464e874e94f01468880fef7fd6cc68c6976e1b2b5ff  ui-auto-inverted.ax25
b0c25f86fc61ded4b272d704ee3e80da66e21de52d98fd96b3f66287bfcc3dbe  ui-auto-inverted.wav
48d78f3f3544a21e3c28f7b7786e44d796ec26591ac17db5b2db350490c4c0da  ui-auto-inverted.wire
4792d4d0630f71530570d065a5cf4053fdf41994d3301a1755c4ebbf29ac0815  ui-max.ax25
741815ad8152f5a7af519f4620559c1b7c3ad7957aaa37fac1a9a6e82c6478b6  ui-max.wav
56ad0fd7422db3db76af240091b245643f3aac60dac7a6eb001cacf34c520f47  ui-max.wire
0d1955e4c975f2cc91a24fecc089fba7900ef36fdc7dd29d216acc5f7aa077fd  vectors.txt
```
