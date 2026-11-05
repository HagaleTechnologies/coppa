# coppa-protocol

OFDM transceiver, NR BG2 LDPC payload coding, framing, ARQ, compression and session management.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-codec, coppa-dsp and coppa-ml; used by coppa-bench, coppa-cli, coppa-daemon and coppa-engine.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
A runnable example is in the crate docs: `cargo doc -p coppa-protocol --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
