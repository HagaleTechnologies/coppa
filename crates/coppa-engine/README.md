# coppa-engine

CoppaTransceiver wrapper with compression, squelch and streaming receive.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-codec, coppa-dsp and coppa-protocol; used by coppa-cli, coppa-daemon, coppa-ffi and coppa-host.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
A runnable example is in the crate docs: `cargo doc -p coppa-engine --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
