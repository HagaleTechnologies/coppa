---
id: c-ffi-and-python-link
title: What will bite you when calling coppa from C or Python?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-ffi/src/lib.rs
  - docs/SPEC.md
  - crates/coppa-ffi/cbindgen.toml
  - crates/coppa-engine/src/link.rs
  - bindings/python/coppa/_ffi.py
verified:
  commit: 4237273
  date: 2026-11-09
links:
  - overview
  - coppa-engine
---
COP-139 added readable FFI errors (`coppa_last_error`), an ABI version
(`coppa_abi_version`), the `coppa_link_*` connected-link API over
`coppa_engine::link::Link`, and a `ctypes` binding in `bindings/python`. How to
use them is in [Getting started](../../docs/tutorials/getting-started.md#using-the-c-ffi)
and [bindings/python/README.md](../../bindings/python/README.md); the design is
[ADR-011](../../docs/adr/011-cop139-library-link.md); the ABI rule is
[the C ABI decision](../../docs/DECISIONS/2026-11-03-c-abi-versioning.md); the gaps
are in [LIMITATIONS](../../docs/LIMITATIONS.md). These are the traps.

## A SONAME breaks `-L target/... -lcoppa_ffi` runs

With `-Wl,-soname,libcoppa_ffi.so.1` the linked program asks the loader for
`libcoppa_ffi.so.1`, which cargo never writes, so the tutorial and the CI smoke
step fail at start. Leave the SONAME to packaging.

## `struct_size` on `coppa_config_t` is a semver major

Adding a field to the public `CoppaConfig` fails `cargo-semver-checks`
(`constructible_struct_adds_field`) and breaks compiled callers. New extensible
structs start with `uint32_t struct_size` instead; `coppa_config_t` waits for
the next ABI major.

`struct_size` does not make growth free either. `coppa_link_new` refuses a
caller whose `struct_size` is smaller than the current struct, so a tail field
added to `coppa_link_config_t` breaks an older binary too. Since COP-178 every
layout change to an existing public struct counts as an ABI major (the
[C ABI decision](../../docs/DECISIONS/2026-11-03-c-abi-versioning.md)); add a
new function instead.

## Four version numbers, four meanings

`coppa_version()` is the crate semver, `coppa_abi_version()` the C ABI,
`coppa_air_protocol_version()` the on-air format and
`coppa_host_api_version()` the TCP/WebSocket host API. They move
independently: an ABI minor bump says nothing about the air protocol. The two
newer accessors need ABI 1.1, so check `coppa_abi_version()` before resolving
them from an older library. What air version 1 freezes is in
[SPEC §0.3](../../docs/SPEC.md); the TCP and WebSocket forms are in
[HOST-API](../../docs/HOST-API.md).

## `build.rs` can leave a stale header

`build.rs` prints `cbindgen: skipping header generation (parse error)` and keeps
the old `coppa.h` when cbindgen cannot parse `lib.rs`. After an FFI change,
check that `coppa.h` actually changed; CI's `git diff --exit-code` step catches
a stale header only if the build regenerated it.
A stale header also hides a removed export, so a header diff alone is not an
ABI check: compare the built library's `coppa_*` symbols as well.

## Link tests are about 11 times slower in debug

The streaming receiver runs on every block, silence included. A 5-frame
exchange took 0.52 s in release and 5.81 s in debug. Keep link payloads small,
one exchange per test, and shorten timers through `LinkConfig`.

## Both ends must share a profile

`CoppaCore` compresses whenever the profile turns compression on
(`HF_STANDARD` does, the default config does not), and a receiver with
compression off cannot parse a compressed frame. Two links with different
profiles connect to nothing.

## C examples avoid designated initializers

Every C program CI builds also compiles as C++17, which lacks designated
initializers. Use `memset` and field assignments, and set
`cfg.struct_size = (uint32_t)sizeof cfg`.

## The v1 streaming quartet warns under `-Werror`

`coppa_start_stream`, `coppa_feed_samples`, `coppa_get_decoded` and
`coppa_stop_stream` are marked `COPPA_DEPRECATED`. A `-Werror` build that calls
them needs `#pragma GCC diagnostic ignored "-Wdeprecated-declarations"` around
the calls, as `crates/coppa-ffi/tests/c/error_strings.c` does.

## Handshake retries must leave time for the reply

The library link bounds CONNECT retries by request/response airtime plus two
configured turnarounds, with `initial_rto` as the minimum. A five-second timer
alone interrupts the level-1 ACK when the turnaround is three seconds.
Session frames also match the wire SSID as well as both callsigns, including
repeat CONNECT_REQs while accepting. See `link.rs` and the regressions
`connect_retry_waits_for_slow_turnaround_ack` and
`peer_frames_must_match_session_ssid` in `link/tests.rs`.
