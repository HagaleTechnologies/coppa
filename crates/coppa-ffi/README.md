# coppa-ffi

C FFI bindings (cdylib + staticlib) with streaming decode API.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-engine; no other coppa crate depends on it.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
API docs: `cargo doc -p coppa-ffi --open`.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
