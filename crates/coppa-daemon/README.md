# coppa-daemon

Long-running daemon (`coppad`) with event loop, audio, and host integration.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-audio, coppa-codec, coppa-dsp, coppa-engine, coppa-host, coppa-ml, coppa-protocol and coppa-radio; used by coppa-cli.

## Run

Not on crates.io. From a clone of the repository, `cargo run -p coppa-daemon -- --help`
prints the options of the `coppad` binary. `coppad --print-default-config` prints the
built-in configuration as TOML, and `coppad --check -c coppad.toml` checks a
configuration file without starting the service. Operating notes are in
[docs/OPERATING.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/OPERATING.md).

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
