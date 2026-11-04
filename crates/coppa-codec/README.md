# coppa-codec

Modulation/demodulation: BPSK, QPSK, 8PSK, QAM, OFDM, AFSK 1200.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-dsp; used by coppa-bench, coppa-cli, coppa-daemon, coppa-engine and coppa-protocol.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
A runnable example is in the crate docs: `cargo doc -p coppa-codec --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
