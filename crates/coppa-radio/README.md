# coppa-radio

Radio control: rigctld CAT, serial PTT (DTR/RTS, `serial-ptt` feature), GPIO PTT (Linux sysfs, `gpio-ptt` feature).

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on no other coppa crate; used by coppa-cli and coppa-daemon.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
API docs: `cargo doc -p coppa-radio --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
