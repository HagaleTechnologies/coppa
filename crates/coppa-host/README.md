# coppa-host

Host interfaces: VARA-style TCP control server, WebSocket JSON API, KISS TNC TCP server (`kiss-tnc` feature).

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-engine; used by coppa-cli and coppa-daemon.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
API docs: `cargo doc -p coppa-host --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
