# coppa-audio

Audio backends: CPAL real-time I/O, WAV file read/write.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-dsp; used by coppa-cli and coppa-daemon.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
API docs: `cargo doc -p coppa-audio --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
