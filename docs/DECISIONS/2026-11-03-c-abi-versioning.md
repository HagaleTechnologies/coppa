# C ABI versioning: COPPA_ABI_VERSION_MAJOR.MINOR, independent of the crate version

Recorded by COP-139 (H-124) on 2026-11-03. Answers the "ABI versioning" technical note of
COP-139.

## Context

Before COP-139 a C or Python caller had no way to tell which C interface a loaded
`libcoppa_ffi` offered. `coppa_version()` returns the crate version, which moves for Rust
API breaks and wire breaks (`docs/DECISIONS/2026-11-02-public-rust-api-semver-gate.md`,
ADR-009, ADR-010) that leave the C interface unchanged.

## Decision

1. **The C ABI has its own version, `MAJOR.MINOR`, starting at 1.0 with COP-139.**
   `coppa.h` defines `COPPA_ABI_VERSION_MAJOR` and `COPPA_ABI_VERSION_MINOR`, and
   `coppa_abi_version()` returns `(MAJOR << 16) | MINOR` for the library actually loaded.
2. **MAJOR changes when an exported function, struct layout or constant changes
   incompatibly or is removed.** MINOR resets to 0 then.
3. **MINOR changes when something is added**: a function, a constant, or a field at the
   end of a struct that starts with `struct_size`.
4. **A program built against a header works with a loaded library whose major is equal
   and whose minor is equal or higher.** The Python binding checks exactly this at load and
   refuses a library with no `coppa_abi_version` (built before COP-139) as too old.
5. **The ABI version is independent of the crate version and of the wire version.**
   Read `docs/SPEC.md`'s change list to decide whether two stations interoperate, the
   crate version for Rust API compatibility, and the ABI version for C compatibility.
6. **Every new extensible C struct starts with `uint32_t struct_size`**, which the caller
   sets to `sizeof` the struct it was built with. `coppa_link_config_t` is the first. The
   library rejects a `struct_size` smaller than its own struct.

## Not done yet, with the measured reason

- **No Linux SONAME.** With `cargo:rustc-cdylib-link-arg=-Wl,-soname,libcoppa_ffi.so.1`,
  a program linked with `-Ltarget/debug -lcoppa_ffi` records `NEEDED libcoppa_ffi.so.1`
  and fails at start with `error while loading shared libraries: libcoppa_ffi.so.1:
  cannot open shared object file`, because cargo writes only `libcoppa_ffi.so`. That
  breaks the getting-started tutorial and the CI C smoke step. A SONAME and its symlink
  belong with packaging, which COP-139 puts out of scope.
- **No `struct_size` on `coppa_config_t`.** Adding `pub struct_size: u32` to
  `CoppaConfig` fails the CI semver gate (`constructible_struct_adds_field`, "semver
  requires new major version", `cargo-semver-checks` 0.51.0) and breaks every compiled C
  caller of `coppa_engine_new_with`. It comes with the next ABI major.

## Upkeep

When a pull request changes the C interface, bump `COPPA_ABI_VERSION_MINOR` (additions)
or `COPPA_ABI_VERSION_MAJOR` (breaks) in `crates/coppa-ffi/src/lib.rs`, regenerate
`coppa.h`, and update `REQUIRED_ABI` in `bindings/python/coppa/_ffi.py` when the binding
starts to use the addition.
