# coppa-cli

CLI binary (`coppa`): `tx`, `rx`, `loopback`, `tune`, `devices`, `profiles`, `config`, and `tnc` (`kiss-tnc` feature).

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-audio, coppa-codec, coppa-daemon, coppa-engine, coppa-host, coppa-protocol and coppa-radio; no other coppa crate depends on it.

## Run

Not on crates.io. From a clone of the repository, `cargo run -p coppa-cli -- --help`
lists the `coppa` subcommands. The
[getting-started tutorial](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/getting-started.md) walks through them.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
