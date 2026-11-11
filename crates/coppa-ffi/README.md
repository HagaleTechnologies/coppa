# coppa-ffi

C FFI bindings (cdylib + staticlib) with streaming decode API.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-engine; no other coppa crate depends on it.

## Use

Not on crates.io. Depend on it from git; see the
[embedding guide](https://github.com/HagaleTechnologies/coppa/blob/main/docs/tutorials/embedding.md).
API docs: `cargo doc -p coppa-ffi --open`.

## Versions

Four independent numbers, none needing a handle: `coppa_version()` (crate
semver), `coppa_air_protocol_version()` (on-air protocol, `docs/SPEC.md`
§0.3), `coppa_host_api_version()` (host API, `docs/HOST-API.md`) and
`coppa_abi_version()` (C ABI, `(MAJOR << 16) | MINOR`). The two middle ones
need C ABI 1.1. Within one ABI major, existing functions, struct layouts and
constants do not change.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
