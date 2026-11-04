# coppa-dsp

Pure DSP primitives: FFT/IFFT, FIR design and filtering (windowed-sinc bandpass, Hilbert, root-raised-cosine), AGC, Costas carrier recovery, Gardner timing recovery.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on no other coppa crate; used by coppa-audio, coppa-channel, coppa-codec, coppa-daemon, coppa-engine and coppa-protocol.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
A runnable example is in the crate docs: `cargo doc -p coppa-dsp --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
