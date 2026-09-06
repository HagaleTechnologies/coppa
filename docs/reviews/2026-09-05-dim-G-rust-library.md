# G — coppa as a Rust library product

Reviewer dimension: public API ergonomics, crate hygiene, docs.rs quality, portability, downstream-developer experience.
Repo: `/Users/thagale/Code/coppa` @ `1c2ffc5` (read-only). Toolchain: pinned `1.98.0` (`rust-toolchain.toml`), MSRV `1.85.0`.
All commands run 2026-09-05 on the review host (macOS/aarch64), `source ~/.cargo/env`.

## Summary

**Overall grade: C+ as a library product (B+ as an engineering codebase).** The internals are strong — zero `unsafe` outside `coppa-ffi`/`coppa-audio`, `cargo deny` fully green, `clippy -D warnings` clean on default features, five crates (dsp/codec/ml/protocol/engine) that compile for `wasm32-unknown-unknown` with no source changes (`coppa-channel` fails on `getrandom`; the remaining workspace crates were untested or unsupported for wasm), and a cold `cargo build -p coppa-dsp --release` of 5.6 s with only 10 crates in its graph. But nothing about the workspace is shaped for a downstream Rust consumer yet: no crate is on crates.io (`cargo info --registry crates-io coppa-dsp` → not found; the names are free), no crate has `repository`/`readme`/`keywords`/`categories`/`docs.rs` metadata, no `#![deny(missing_docs)]` anywhere (25 of 43 public items in `coppa-dsp` are undocumented), no `#[must_use]` anywhere, no `Debug`/`Clone`/`Default` on any `coppa-dsp` struct, and no re-export of `num_complex::Complex32` so a consumer must pin a matching `num-complex`. Error handling is `anyhow::Result` in the public API of 5 of 6 library crates (`coppa-codec::Modem` trait, `CoppaCore::encode/decode`, all of `coppa-audio`/`coppa-radio`), so callers cannot match on failure kinds; only `FftError`, `ReceiveError`, `TransmitError`, `PttConfigError` are typed and none use `thiserror`. Library code panics on caller input in several public entry points — most seriously `CoppaTransceiver::transmit` `.expect("invalid speed level in header")` on a `pub`-field header a consumer can construct with reserved level 8.

**Three biggest problems:**
1. **`cargo build --workspace --all-features` does not compile** (`E0560` in `coppa-cli/src/main.rs:961`, `TncConfig` has no field `audio_device`). CI never runs `--all-features`, so this rotted silently. A library workspace whose feature closure doesn't build is not publishable.
2. **Error-type strategy: `anyhow` in public signatures + panics on input.** `Modem::modulate -> anyhow::Result`, `CoppaCore::decode -> anyhow::Result` with string errors (`"No signal detected"`), 37 `pub fn ... -> Result<` in `coppa-protocol` using `anyhow`; 33 non-test panic sites in `coppa-protocol`, 10 in `coppa-codec`, 5 in `coppa-dsp` (all 5 are `assert!` on caller arguments).
3. **Zero publishing metadata and no versioning story.** Every crate is `0.1.0` via `workspace.package`, no git tags exist, no CHANGELOG, path-only inter-crate deps (`cargo deny` had to downgrade `wildcards` to `warn` for this reason, per its own comment), `coppa-ffi/build.rs` writes `coppa.h` into the source tree at build time (fails read-only `cargo publish` verification), and the root `coppa` package collides with the `coppa` binary in `cargo doc` output.

## Per-crate scorecard

pub items / undocumented counted by script over non-test regions (`pubitems.py`); panics = non-test `unwrap/expect/panic!/unreachable!/assert!` sites (`panics2.py`, cut at `#[cfg(test)] mod`). Doc warnings from `cargo doc --workspace --no-deps`.

| Crate | pub items (undoc) | doc warns | panics-in-lib | unsafe | error style | serde on config? | wasm32 builds? | publish-ready? |
|---|---|---|---|---|---|---|---|---|
| coppa-dsp | 43 (25) | 1 | 5 (all `assert!` on args) | 0 | `FftError` hand-rolled; rest panic | n/a (no config types; no derives at all) | **yes** | no: no metadata, no `Complex32` re-export |
| coppa-codec | 165 (35) | 21 | 10 | 0 | `anyhow` in `Modem` trait | no (`CoppaProfile` derives `Debug, Clone` only) | **yes** | no |
| coppa-protocol | 299 (44) | 12 | 33 | 0 | mixed: `ReceiveError`/`TransmitError` typed, 37 `anyhow` pub fns | no (`serde` is dev-dep only) | **yes** (compiles; `std::time::Instant::now()` in `session.rs:192`, `cp_negotiator.rs` will panic at runtime on wasm32-unknown-unknown) | no; dev-dep cycle with `coppa-bench` |
| coppa-ml | 45 (5) | 0 | 1 | 0 | none (returns plain values) | no | **yes** | closest to ready |
| coppa-channel | 20 (1) | 1 | 0 | 0 | none | n/a | **no** (`getrandom` needs `wasm_js` feature via `rand`) | no |
| coppa-engine | 32 (3) | 0 | 0 | 0 | `anyhow` everywhere | partial (`EngineConfig`: `Debug, Clone, Default`; no `PartialEq`, no `serde`) | **yes** | no |
| coppa-audio | 67 (10) | 0 | 0 | 5 (`unsafe impl Send`) | `anyhow` in traits | no | untested (cpal) | no |
| coppa-radio | 32 (7) | 0 | 0 | 0 | `anyhow` in traits | no | no (tokio) | no |
| coppa-host | 51 (7) | 3 | 0 | 0 | `anyhow` | partial (websocket only) | no (tokio) | no |
| coppa-ffi | 4 (0) | 2 | 0 (uses `catch_unwind` ×11) | 115 lines | C int codes; well documented | n/a | n/a | no; `build.rs` writes into src tree |
| coppa-daemon | 36 (4) | 0 | 3 | 0 | `anyhow` + `PttConfigError(String)` | yes (`config.rs`) | no | app crate |
| coppa-cli | 0 | 0 | 1 | 0 | `anyhow` | n/a | no | app; `--all-features` broken |
| coppa-bench | 96 (22) | 0 | 21 | 0 | `anyhow`/expect | partial | no | should be `publish = false` |

Doctests that actually run: `coppa_codec` 4, `coppa_dsp` 1, `coppa_engine` 1, `coppa_channel` 1 — **0** in `coppa_protocol` (299 pub items), `coppa_ml`, `coppa_audio`, `coppa_radio`, `coppa_host`.

## What's already good

- `unsafe` is confined to `coppa-ffi` (115 lines; 11 of 17 exported `extern "C"` functions -- the fallible engine operations -- are wrapped in `catch_unwind`, while simple accessors/frees (`coppa_engine_destroy`, `coppa_version`, `coppa_free_samples`, `coppa_free_string`, `coppa_free_frame_payload`, `coppa_stop_stream`) are not, 15 `# Safety` sections, poison/handle-lifetime semantics documented in crate docs) and 5 `unsafe impl Send` in `coppa-audio` with `// Safety:` comments. Everything else is 100% safe Rust with no arch intrinsics (`grep core::arch|target_feature|_mm_` → nothing).
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`; `deny.toml` is thoughtful (per-ignore rationale, registry pinning, license allowlist).
- `cargo clippy --workspace --all-targets` (default features) → zero warnings; CI runs it with `-D warnings`. `cargo fmt` gated.
- **`wasm32-unknown-unknown` builds for coppa-dsp, coppa-codec, coppa-ml, coppa-protocol, coppa-engine with no changes** — a browser demo is one `wasm-bindgen` shim away.
- `aarch64-unknown-linux-gnu` `cargo check` passes for dsp/codec/protocol/engine/ml/channel/ffi (RPi-ready at the library layer).
- Lean dependency graphs: `coppa-dsp` = 10 crates, `coppa-engine` = 23. Cold release build of `coppa-dsp` 5.6 s, `coppa-engine` 4.3 s incremental on top. `cargo tree -d` shows only `syn` 2.x/3.x duplicated (proc-macro only, via clap 4.6).
- `workspace.package` inheritance for version/edition/license/rust-version; MSRV job in CI; pinned toolchain with a documented reason.
- Both root examples (`bpsk_loopback`, `ofdm_roundtrip`) build and succeed in release; `CoppaCore` has a runnable doctest that round-trips.
- Typed, matchable `ReceiveError { SyncFailed, HeaderCorrupt, LdpcNotConverged{iterations}, CrcMismatch }` and `TransmitError` exist and are good models for the rest.
- `coppa-ffi` `cbindgen.toml` + checked-in `coppa.h` (91 `coppa_` symbols), FFI v1/v2 compatibility story explicit.
- `coppa-ml` crate docs honestly say what the crate is and isn't (no ML), and name the deleted dead code.
- `fuzz/` exists with 3 targets; proptest in protocol.

## Hit list

| # | Item | Category | Impact | Effort | Evidence |
|---|---|---|---|---|---|
| 1 | Fix `--all-features` build: `coppa-cli/src/main.rs:961` sets `TncConfig.audio_device` which no longer exists; add `cargo check --workspace --all-features` to CI `check` job | publishing | H | S | `cargo build --workspace --all-features` → `error[E0560]: struct TncConfig has no field named audio_device --> crates/coppa-cli/src/main.rs:961`; `.github/workflows/ci.yml:57-58` only tests `cpal-backend,websocket` and `--no-default-features` |
| 2 | `CoppaTransceiver::transmit` panics on caller-constructed header with reserved speed level (e.g. 8) instead of returning `TransmitError::InvalidSpeedLevel` | panic-safety | H | S | `crates/coppa-protocol/src/modem/transceiver.rs:650,670,721` `.expect("invalid speed level in header")`; `new()` comment at ~538: "Reserved/invalid levels (e.g. 8) simply have no [entry]"; `CoppaHeader` fields are all `pub` (`coppa-codec/src/ofdm/frame.rs:240`) |
| 3 | Replace `anyhow::Result` in public trait/API signatures with per-crate `thiserror` enums (`CodecError`, `EngineError`, `AudioError`, `RadioError`); keep `anyhow` for bins only | api-design | H | L | `coppa-codec/src/traits.rs`: `fn modulate(&self,..) -> anyhow::Result<Vec<f32>>`; `coppa-engine/src/engine.rs:297-321` `decode -> Result<String>` with `anyhow!("No signal detected")` string errors; 37 `pub fn -> Result<` in coppa-protocol use anyhow; `thiserror` not in `Cargo.toml` at all |
| 4 | Add `[package]` metadata to every publishable crate: `repository`, `homepage`, `readme`, `keywords`, `categories`, `documentation`; add `[package.metadata.docs.rs] all-features = true` (or explicit feature list) | publishing | H | S | `grep -rn 'docs.rs\|keywords\|categories\|repository\|readme' Cargo.toml crates/*/Cargo.toml` → empty |
| 5 | Add `version = "0.1"` alongside `path =` on all inter-crate deps so `cargo publish` works and `deny.toml` `[bans] wildcards` can go back to `deny` | publishing | H | S | `deny.toml:80-86` comment: "Until the workspace crates are prepared for crates.io publication (adding `version = "0.1"` alongside `path = ...`), we downgrade to warn" |
| 6 | `coppa-ffi/build.rs` writes `coppa.h` into `CARGO_MANIFEST_DIR` — breaks `cargo publish --dry-run` (read-only verify) and docs.rs sandbox; write to `OUT_DIR` and check in the header via a separate `make header`/CI diff step | publishing | H | S | `crates/coppa-ffi/build.rs:16` `bindings.write_to_file(format!("{}/coppa.h", crate_dir))` |
| 7 | Add `#![warn(missing_docs)]` (then `deny`) to dsp/codec/protocol/engine/ml; 25/43 public items in coppa-dsp undocumented, 44 in protocol, 35 in codec | docs | H | M | `pubitems.py` output; `grep -rn '^#!\[' crates/*/src/lib.rs` → nothing (no crate-level lints at all) |
| 8 | Derive `Debug` + `Clone` on every `coppa-dsp` struct (`FftProcessor`, `RrcFilter`, `AdaptiveAgc`, `CostasLoop`, `GardnerTimingRecovery`, `StreamingFir`, `Fir`); consumers cannot `#[derive(Debug)]` on anything containing them | api-design | H | S | `grep -n -B1 '^pub struct' crates/coppa-dsp/src/*.rs` → no `#[derive]` on any of the 7 structs; `FftProcessor` holds `Arc<dyn Fft>` so `Clone` is cheap |
| 9 | Re-export `num_complex::{Complex32, Complex}` from `coppa-dsp` (and `rustfft` if `FftProcessor` ever exposes it) so manta doesn't have to pin a matching `num-complex` | api-design | H | S | `grep -rn 'pub use num_complex' crates/*/src` → nothing; `FftProcessor::forward(&[Complex32])` is the contract manta consumes |
| 10 | `coppa-dsp` public fns `assert!` on arguments: `design_bandpass`/`design_hilbert` (odd taps), `FftProcessor::new/forward/inverse`; make the `try_*` variants primary and document panics with `# Panics` sections | panic-safety | H | S | `crates/coppa-dsp/src/fir.rs:5,29`; `fft.rs:62,79,107`; `try_new/try_forward/try_inverse` already exist (`fft.rs:70,94,127`) but `forward` is the one manta will reach for |
| 11 | `CoppaCore`/`CoppaTransceiver` are `Send + !Sync` (interior `Cell`/`RefCell`) — undocumented; either document loudly or move the counters to atomics so `&CoppaCore` can be shared across threads | api-design | M | M | compile probe: `sync::<coppa_engine::CoppaCore>()` → `Cell<u64> cannot be shared` (transceiver.rs:371,381), `RefCell<HarqRxBuffers>` (transceiver.rs:413), `RefCell<Option<LastFrameWorkspace>>` (coppa_modem.rs:354) |
| 12 | `coppa-audio`'s `default = ["cpal-backend", ...]` leaks into every consumer: `coppa-cli`, `coppa-daemon`, `coppa-bench` all pull cpal/alsa even when their own `cpal-backend` feature is off, making their feature flags no-ops and breaking aarch64-linux cross `check` on alsa-sys | deps | H | S | `cargo tree -p coppa-cli -e features` → `coppa-audio ... cpal-backend,default,file-backend`; `cargo check -p coppa-daemon --target aarch64-unknown-linux-gnu` → `failed to run custom build command for alsa-sys v0.4.0` (via `coppa-audio → coppa-daemon`); fix: `coppa-audio = { path=…, default-features = false, features=["file-backend"] }` in consumers |
| 13 | Fix the 47 `cargo doc` warnings (19 links to private items, 18 unresolved `[k]`/`[i]` math-index links, 3 unclosed HTML tags); add `RUSTDOCFLAGS=-D warnings cargo doc` to CI | docs | M | S | verbatim capture below; e.g. `coppa-codec/src/ofdm/equalizer.rs:154` `[k]`, `coppa-host/src/vara/protocol.rs:6` unclosed `<dest>`, `coppa-protocol/src/fec/ldpc/rate_match.rs:94` links private `k0_offset` |
| 14 | Root package `coppa` and `coppa-cli`'s bin `coppa` collide in rustdoc output; either rename the root facade (`coppa` lib is fine, rename bin target docs) or set `doc = false` on the bin | docs | M | S | `warning: output filename collision at /Users/thagale/Code/coppa/target/doc/coppa/index.html`; root `src/lib.rs` re-exports 4 crates |
| 15 | Add `#[must_use]` on all `-> Vec<f32>`/`-> Result` DSP producers and builder-style `with_*` methods | api-design | M | S | `grep -rc must_use crates/*/src` → 0 everywhere; `CoppaTransceiver::with_turbo(mut self, ..) -> Self` (transceiver.rs:580) silently discardable |
| 16 | Give `EngineConfig` `PartialEq` and `serde` derives (behind a `serde` feature) -- `Default` is already implemented (`config.rs:51-61`); today the daemon re-implements config parsing in `coppa-daemon/src/config.rs` | api-design | M | S | `coppa-engine/src/config.rs:11,51-61` `#[derive(Debug, Clone)]` plus a hand-written `impl Default` |
| 17 | Tag `v0.1.0`, add `CHANGELOG.md` (keep-a-changelog), and a `release.yml` that runs `cargo publish` in dependency order (dsp→codec→ml→protocol→engine→…) | publishing | H | M | `git tag` → empty; `ls CHANGELOG*` → none; no publish workflow in `.github/workflows/` |
| 18 | Mark `coppa-bench`, `coppa-cli`, `coppa-daemon`, `tools/gen_nr_bg2` `publish = false` (only gen_nr_bg2 is today) and break the `coppa-protocol` dev-dep → `coppa-bench` → `coppa-protocol` cycle (pulls `clap` into protocol's test graph) | publishing | M | S | `tools/gen_nr_bg2/Cargo.toml:8 publish = false` is the only one; `cargo tree -d` shows `clap v4.6.6 → coppa-bench → (dev) coppa-protocol`; `coppa-protocol/Cargo.toml` dev-deps `coppa-bench` |
| 19 | Per-crate `README.md` (docs.rs shows the crate README, not the workspace one) with a 10-line "add to Cargo.toml / encode / decode" snippet; wire via `readme = "README.md"` | docs | H | M | `ls crates/*/README.md` → none; root `README.md` Quick Start is `cargo build && cargo test` only, never `use coppa_engine::…` |
| 20 | "Embedding coppa in your Rust app" guide: `docs/tutorials/getting-started.md` covers CLI, daemon, and C FFI — but not Rust. Show `CoppaCore` batch, `push_samples` streaming, and `CoppaTransceiver` direct use | docs | H | M | `grep -nE 'cargo add|use coppa' docs/tutorials/getting-started.md` → 0 hits; section "Using the C FFI" exists at line 120 |
| 21 | Feature-flag matrix documentation: `coppa-host default = ["vara-tcp"]` and `kiss-tnc`/`websocket` undocumented in any README; `coppa-cli` `cpal-backend`/`kiss-tnc` not in `--help`; add a `## Features` section per crate | docs | M | S | `crates/coppa-host/Cargo.toml:15-19`; `crates/coppa-cli/Cargo.toml:31-35`; README mentions only `cpal-backend`, `serial-ptt`, `gpio-ptt` |
| 22 | `coppa-protocol` compiles for wasm but `session.rs`/`cp_negotiator.rs` call `std::time::Instant::now()` unconditionally → runtime panic on `wasm32-unknown-unknown`; take a `now: Instant` parameter or a `Clock` trait (also what `no_std` needs) | portability | M | M | `crates/coppa-protocol/src/session.rs:17,192,223,247,…`; `cp_negotiator.rs:221`; wasm build of `coppa-protocol` `Finished` (compiles, will panic) |
| 23 | `coppa-channel` fails on wasm because of `rand → getrandom`; add a `wasm_js` passthrough feature or take an `Rng` parameter instead of constructing one internally | portability | M | S | `cargo build -p coppa-channel --target wasm32-unknown-unknown` → `error: could not compile getrandom … enable the "wasm_js" crate feature` |
| 24 | Ship a `coppa-wasm` demo crate (`wasm-bindgen` + a 40-line web page that encodes text and plays audio / decodes mic input); it's the single best marketing asset and everything below engine already builds | portability | H | M | wasm builds verified for dsp/codec/ml/protocol/engine (capture below); no `wasm` mention in CI or docs |
| 25 | `no_std` + `alloc` for `coppa-dsp`: only blockers are `std::sync::Arc` (→ `alloc::sync::Arc`), `std::error::Error` (→ `core::error::Error`, stable since 1.81 < MSRV 1.85), `std::f32::consts` (→ `core`), and `rustfft` (has no `no_std`). Realistic path: a `std` default feature and a pure-Rust radix-2 fallback; document as a roadmap item for a hardware TNC | portability | L | L | `grep -n 'std::' crates/coppa-dsp/src/*.rs` → 5 non-test uses, all trivially `core`/`alloc`; `rustfft` is std-only |
| 26 | Document `Send`/`Sync`/thread model of `AudioSource`/`AudioSink`/`RadioControl` traits (`: Send` bound, `&mut self` everywhere, sync blocking I/O inside an otherwise-tokio daemon) | docs | M | S | `crates/coppa-audio/src/lib.rs:43-56`, `coppa-radio/src/lib.rs:41-55`; no doc comments on any trait method |
| 27 | `AudioDevice`, `HostFrame`, `HostEvent`, `RadioMode`, `PttState` have `pub` fields/variants with no doc comments and no `#[non_exhaustive]`; adding a variant later is a semver break | api-design | M | S | `coppa-audio/src/lib.rs:34-40`, `coppa-host/src/lib.rs:15-46`, `coppa-radio/src/lib.rs:23-38`; `grep -rn non_exhaustive crates/` → 0 |
| 28 | `PttConfigError(String)` newtype-over-String is an anti-pattern; make it an enum | api-design | L | S | `crates/coppa-daemon/src/config.rs:149-157` |
| 29 | Hand-rolled `Display`/`Error` impls for `FftError`, `ReceiveError`, `TransmitError` — switch to `thiserror` for consistency and `#[source]` chaining | polish | L | S | `coppa-dsp/src/fft.rs:16-31`, `transceiver.rs:417-471`; `thiserror` absent from `[workspace.dependencies]` |
| 30 | `ConstellationMapper::demap_hard/demap_soft -> Vec<u8>/Vec<f32>` allocate per symbol; CLAUDE.md already admits 16-QAM is "allocation-bound"; add `demap_soft_into(&mut [f32])` / return `SmallVec<[f32;6]>` | api-design | M | M | `crates/coppa-codec/src/traits.rs` `fn demap_soft(&self, symbol, noise_variance) -> Vec<f32>`; `CLAUDE.md:314` "identical, fixed Vec<f32> heap-allocation cost… dominates" |
| 31 | `FftProcessor::forward/inverse` allocate a fresh `Vec` per call; add `forward_into(&self, input: &[Complex32], out: &mut [Complex32])` / `process_in_place(&mut [Complex32])` like `rustfft`/`realfft` | api-design | M | S | `coppa-dsp/src/fft.rs:78-92` `let mut buffer = vec![...]` each call; `AdaptiveAgc::process`, `CostasLoop::process`, `RrcFilter::filter` same pattern; only `StreamingFir::process(x, out: &mut Vec<f32>)` is out-param style — inconsistent |
| 32 | `coppa-dsp` constructors take bare `f32`/`usize` positional args (`CostasLoop::new(carrier_freq, sample_rate, loop_bandwidth)`, `AdaptiveAgc::new(target_level, block_size)`) with hard-coded `beta_attack=0.6`, `max_gain=10000` that can't be set; add builder or config struct | api-design | M | M | `coppa-dsp/src/agc.rs:25-35` (6 private tunables, 2 exposed); `carrier_recovery.rs:23` |
| 33 | `coppa-codec` prelude exports only `BpskModem`; the flagship `CoppaModem`/`CoppaProfile`/`CoppaHeader` require `coppa_codec::ofdm::…` paths and aren't listed in crate docs, which still describe only PSK/QAM | docs | M | S | `crates/coppa-codec/src/lib.rs:19-20` `pub use bpsk::BpskModem;` only; crate doc says "implementations for BPSK, QPSK, 8PSK, 16QAM, and 64QAM" (no OFDM) |
| 34 | `coppa-protocol/src/lib.rs` has a 1-line crate doc and re-exports only `Frame`; 299 public items, 0 doctests. Add a crate-level overview with a `CoppaTransceiver` round-trip doctest | docs | H | S | `crates/coppa-protocol/src/lib.rs` (18 lines); `cargo test --doc` → `Doc-tests coppa_protocol … 0 passed` |
| 35 | `coppa-ml`, `coppa-audio`, `coppa-radio`, `coppa-host` have 0 doctests; each should have at least one runnable example on the main type (`RateLoop`, `LoopbackBackend`, `NullPtt`, `HostEvent`) | docs | M | M | `cargo test --workspace --doc` summary below |
| 36 | LDPC public API asserts on lengths (`decoder.rs:166 assert_eq!(llrs.len(), n)`, `rate_match.rs:47,52`, `encoder.rs:72`) — fine internally but these are `pub` in `coppa_protocol::fec::ldpc`; either `pub(crate)` them or return `Result` | panic-safety | M | M | `panics2.py`: coppa-protocol 33 sites, 21 of them in `fec/ldpc/*` |
| 37 | `CrossFrameInterleaver::new` `assert!(num_frames > 0)`, `RateLoop::new` `assert!(!levels.is_empty())`, `CoppaProfile` `ofdm/mod.rs:120 assert!` — public constructors that panic on args; return `Result` or document `# Panics` | panic-safety | M | S | `coppa-codec/src/ofdm/cross_frame_interleaver.rs:18,55`; `coppa-ml/src/rate_loop.rs:34`; `coppa-codec/src/ofdm/mod.rs:120` |
| 38 | `Qam16Mapper`/`Qam64Mapper` constructors `.unwrap()` on internal table lookups (`qam16.rs:38`, `qam64.rs:40,43`) — should be `const` tables or `expect` with invariant text | panic-safety | L | S | `crates/coppa-codec/src/qam16.rs:38`, `qam64.rs:40,43` |
| 39 | No `[profile.release]` section: no `overflow-checks` policy, no `lto`, no `codegen-units=1`, no `strip`; `coppad` is 5.5 MB, `libcoppa_ffi.a` 21 MB | polish | M | S | `grep -n '^\[profile' Cargo.toml` → none; `ls -la target/release/` sizes; 279 `as uN` casts in dsp/codec/protocol vs 30 `wrapping_/checked_/saturating_` |
| 40 | Add `cargo semver-checks` and `cargo public-api` to CI once published; today neither tool is installed and there's no API snapshot | publishing | M | S | `which cargo-public-api cargo-semver-checks cargo-machete cargo-udeps` → none found |
| 41 | `deny.toml` carries 3 `advisory-not-detected` stale ignores (RUSTSEC-2024-0384/0436, RUSTSEC-2026-0173) — prune | deps | L | S | `cargo deny check` capture below |
| 42 | `rand = "0.10"` in `coppa-channel` (a testing crate) pulls `chacha20`, `getrandom` into any consumer using channel models; take `impl Rng` parameters and gate the default RNG behind a feature | deps | L | S | `crates/coppa-channel/Cargo.toml`; `deny.toml:42` had to ignore yanked `chacha20@0.10.1` because of it |
| 43 | Sample type is `f32` throughout (good, consistent), but `sample_rate` is `u32` in `EngineConfig`/`AudioSource` and `f32` in `Modem::sample_rate()`/`CostasLoop::new` — pick one (u32 Hz) and convert at the DSP boundary | api-design | L | M | `coppa-engine/src/config.rs:16 pub sample_rate: u32`; `coppa-codec/src/traits.rs fn sample_rate(&self) -> f32`; `coppa-dsp/src/carrier_recovery.rs:23 sample_rate: f32` |
| 44 | `StreamFrame.payload: Result<Vec<u8>>` (anyhow) embeds an error inside an event struct; split into `Ok` frames + a separate `DecodeFailure` event or use a typed `ReceiveError` | api-design | M | S | `coppa-engine/src/engine.rs:47-49` `pub payload: Result<Vec<u8>>` |
| 45 | `coppa-engine` doc says "`EngineConfig` - runtime configuration types" but `reconfigure()`/`set_speed_level()`/`set_cp_profile()` are three parallel mutation paths with different validation; consolidate under a `Result`-returning `reconfigure` | api-design | L | M | `coppa-engine/src/engine.rs:401,434,459` |
| 46 | README status table is stale vs library reality: says "Channel prediction: EWMA predictor" and "Serial/GPIO PTT: Stub" while `coppa-ml` lib docs say the EWMA predictor was deleted and `coppa-radio` has real `ptt_serial`/`ptt_gpio` modules | docs | M | S | `README.md:24-26` vs `crates/coppa-ml/src/lib.rs:21-27`, `crates/coppa-radio/src/lib.rs:12-16` |
| 47 | README "Workspace Crates" table claims `coppa-dsp` has "IIR filters, resampling" — neither exists in `coppa-dsp` (resampler lives privately in `coppa-audio`) | docs | L | S | `README.md:72`; `ls crates/coppa-dsp/src` → agc, carrier_recovery, fft, filter, fir, timing_recovery; `coppa-audio/src/resampler.rs` not `pub mod` |
| 48 | The manta contract page is a pointer stub: it names "FFT, FIR filtering, and AGC" but lists no items, no version, and was verified at `c1d2676` (2026-07-07). Good news: `git diff c1d2676..HEAD -- crates/coppa-dsp` is **empty**, so no drift. Bad news: nothing enforces it. Add a `coppa-dsp` public-API snapshot test (`cargo public-api` diff) and list the consumed items on the page | polish | M | S | `wiki/pages/coppa-dsp-skimmer-interface.md` (in coppa repo; no manta clone present on this host to verify the consumer side); `git diff --stat c1d2676..HEAD -- crates/coppa-dsp` → no output |
| 49 | 41 examples under `crates/coppa-bench/examples/` are task-numbered internal diagnostics (`task3_fec_isolated_gate.rs`, `level9_profile_ab.rs`…) that show up as "examples" on docs.rs/GitHub; move to `coppa-bench/src/bin/` or a `diagnostics/` dir, keep 2-3 curated examples per library crate | docs | M | S | `find . -name '*.rs' -path '*examples*'` → 44 files, 41 in coppa-bench |
| 50 | `Modem::demodulate_soft(&mut self)` vs `ConstellationMapper` `&self`, `FecCodec::encode(&mut self)` vs `decode(&self)` — mutability is inconsistent across sibling trait methods with no doc explaining why | api-design | L | S | `crates/coppa-codec/src/traits.rs` |
| 51 | `coppa-audio` `list_devices()` returns `Ok(vec![])` when `cpal-backend` is off — silently lying is worse than a compile error; make the fn `cfg`-gated or return `Err(AudioError::NoBackend)` | api-design | L | S | `crates/coppa-audio/src/lib.rs:59-67` |
| 52 | No `Cargo.lock` policy statement; it is committed (correct for bins, and fine for libs since Cargo 1.83 guidance) — document in CONTRIBUTING | polish | L | S | `git ls-files Cargo.lock` → tracked; CONTRIBUTING.md silent |
| 53 | `rust-toolchain.toml` pins `1.98.0` while `rust-version = "1.85.0"` — 13 minor versions apart; MSRV CI job exists, but a consumer on 1.85 gets a `Cargo.lock` produced by 1.98 (lockfile v4 fine). Consider bumping MSRV to something within N-6 and stating the policy | polish | L | S | `rust-toolchain.toml:8`, `Cargo.toml:25` |
| 54 | Add `#![forbid(unsafe_code)]` to the 10 crates that have none, `#![deny(unsafe_op_in_unsafe_fn)]` in coppa-ffi | polish | M | S | `grep -rn '^#!\[' crates/*/src/lib.rs` → nothing; unsafe count table above |
| 55 | `coppa-cli` `--no-default-features` build emits 4 dead-code warnings (`stream_decode`, `print_stream_frame`, `TRAILING_FLUSH_SAMPLES`, unused `raw`) — CI's `check --no-default-features` doesn't use `-D warnings` | polish | L | S | `cargo build --workspace --no-default-features` warnings capture; `ci.yml:58` |
| 56 | Add `wasm32-unknown-unknown` and `aarch64-unknown-linux-gnu` `cargo check` of the library crates to CI so #24/#22 don't regress | portability | M | S | `ci.yml` `platform` matrix only tests `coppa-audio` on macOS/Windows (`ci.yml:182-201`) |

## Verbatim captures

### `cargo doc --workspace --no-deps` (warning summary)
```
warning: `coppa-host` (lib doc) generated 3 warnings
warning: `coppa-protocol` (lib doc) generated 12 warnings
warning: `coppa-codec` (lib doc) generated 21 warnings
warning: `coppa-channel` (lib doc) generated 1 warning
warning: `coppa-dsp` (lib doc) generated 1 warning
warning: `coppa-ffi` (lib doc) generated 2 warnings
warning: output filename collision at /Users/thagale/Code/coppa/target/doc/coppa/index.html
--- by kind (47 total lines beginning `warning`) ---
  19 warning: public documentation for X links to private item X
  18 warning: unresolved link to X
   3 warning: unclosed HTML tag X
```
Representative locations: `coppa-codec/src/ofdm/equalizer.rs:32,53,154` (`[k]`, `[STS]`), `coppa-codec/src/ofdm/frame.rs:3` (`[LTS+CP]`, `[DATA+CP]`), `coppa-codec/src/ofdm/coppa_modem.rs:1074,1078,1735`, `coppa-protocol/src/fec/ldpc/rate_match.rs:94,114,139` (private `k0_offset`/`matching_buffer`), `coppa-protocol/src/modem/transceiver.rs:593,857`, `coppa-host/src/vara/protocol.rs:6,8` (`<dest>`, `<digi>`, `<callsign>`), `coppa-dsp/src/fir.rs:26`, `coppa-ffi/src/lib.rs:371,423`, `coppa-channel/src/lib.rs:158`.

### `cargo deny check`
```
warning[advisory-not-detected]: advisory was not encountered   (deny.toml:23  RUSTSEC-2024-0384)
warning[advisory-not-detected]: advisory was not encountered   (deny.toml:28  RUSTSEC-2024-0436)
warning[advisory-not-detected]: advisory was not encountered   (deny.toml:33  RUSTSEC-2026-0173)
advisories ok, bans ok, licenses ok, sources ok
```

### wasm32-unknown-unknown builds
```
$ rustup target add wasm32-unknown-unknown            # installed fresh
$ cargo build -p coppa-dsp      --target wasm32-unknown-unknown   → Finished `dev` … in 1.81s
$ cargo build -p coppa-codec    --target wasm32-unknown-unknown   → Finished `dev` … in 0.80s
$ cargo build -p coppa-ml       --target wasm32-unknown-unknown   → Finished `dev` … in 0.77s
$ cargo build -p coppa-protocol --target wasm32-unknown-unknown   → Finished `dev` … in 0.68s
$ cargo build -p coppa-engine   --target wasm32-unknown-unknown   → Finished `dev` … in 0.14s
$ cargo build -p coppa-channel  --target wasm32-unknown-unknown   →
error: The wasm32-unknown-unknown targets are not supported by default; you may need to enable
the "wasm_js" crate feature … error: could not compile `getrandom` (lib) due to 1 previous error
```
Caveat: `coppa-protocol` uses `std::time::Instant::now()` in non-test code (`session.rs:192`, `cp_negotiator.rs`), which panics at runtime on this target.

### aarch64-unknown-linux-gnu `cargo check`
```
$ cargo check -p coppa-dsp -p coppa-codec -p coppa-protocol -p coppa-engine -p coppa-ml -p coppa-channel -p coppa-ffi --target aarch64-unknown-linux-gnu
    Finished `dev` profile … in 5.83s
$ cargo check -p coppa-daemon --target aarch64-unknown-linux-gnu
error: failed to run custom build command for `alsa-sys v0.4.0`
  (pkg-config has not been configured to support cross-compilation)
$ cargo tree -p coppa-daemon --target aarch64-unknown-linux-gnu -i alsa-sys
alsa-sys v0.4.0 └── alsa v0.11.0 └── cpal v0.18.1 └── coppa-audio v0.1.0 └── coppa-daemon v0.1.0
```
(coppa-daemon has `default = []` but coppa-audio's own default enables cpal — hit-list #12.)

### `cargo build --workspace --all-features`
```
error[E0560]: struct `TncConfig` has no field named `audio_device`
   --> crates/coppa-cli/src/main.rs:961:13
    |
961 |             audio_device: device,
    |             ^^^^^^^^^^^^ `TncConfig` does not have this field
    = note: available fields are: `bind_address`
error: could not compile `coppa-cli` (bin "coppa") due to 1 previous error
```

### `cargo tree -d --workspace -e normal`
```
syn v2.0.117
├── serde_derive v1.0.228 (proc-macro)  → serde → coppa-bench, coppa-daemon → coppa-cli
├── tokio-macros v2.7.0 (proc-macro)    → tokio v1.53.1 → coppa-cli, coppa-daemon, coppa-host, coppa-radio
└── tracing-attributes v0.1.31          → tracing v0.1.44 → coppa-daemon, tracing-subscriber
syn v3.0.3
└── clap_derive v4.6.4 (proc-macro)     → clap v4.6.6 → coppa-bench, coppa-cli
```
Only duplicate is `syn` 2/3 (proc-macro, build-time only).

### Send/Sync probe (scratch crate, `cargo check`)
```
error[E0277]: `Cell<u64>` cannot be shared between threads safely
   within `CoppaCore`, the trait `Sync` is not implemented for `Cell<u64>`
   --> crates/coppa-protocol/src/modem/transceiver.rs:334:12  (via coppa-engine/src/engine.rs:91)
error[E0277]: `RefCell<transceiver::HarqRxBuffers>` cannot be shared between threads safely
error[E0277]: `RefCell<Option<coppa_codec::ofdm::coppa_modem::LastFrameWorkspace>>` cannot be shared …
```
`CoppaCore`, `CoppaTransceiver`, `StreamingReceiver`, `FftProcessor` are all `Send`; `FftProcessor` is also `Sync`; `CoppaCore`/`CoppaTransceiver` are `!Sync`.

### Doctests (`cargo test --workspace --doc`)
```
coppa 0 | coppa_audio 0 | coppa_bench 0 | coppa_channel 1 | coppa_codec 4 | coppa_daemon 0
coppa_dsp 1 | coppa_engine 1 | coppa_host 0 | coppa_ml 0 | coppa_protocol 0 | coppa_radio 0
```

### Build times / sizes / graph
```
cold: CARGO_TARGET_DIR=<fresh> cargo build -p coppa-dsp --release    → 5.55s wall (real 5.58 user 9.10)
      then cargo build -p coppa-engine --release (same dir)         → 4.25s wall
cargo tree -e normal --prefix none | sort -u | wc -l:
      coppa-dsp 10 | coppa-engine 23 | coppa-cli 119 | coppa-daemon --all-features 146
target/release: coppa 2.37 MB | coppad 5.52 MB | libcoppa_ffi.dylib 1.28 MB | libcoppa_ffi.a 21.5 MB
```

### crates.io availability
```
$ cargo info --registry crates-io coppa-dsp    → error: could not find `coppa-dsp` in registry
$ cargo info --registry crates-io coppa        → error: could not find `coppa` in registry
$ cargo info --registry crates-io coppa-engine → error: could not find `coppa-engine` in registry
```
All names are unclaimed. `git tag` in the repo is empty (never released).

### Root examples
```
$ cargo run --release --example bpsk_loopback → "Round-trip succeeded." (65520 samples)
$ cargo run --release --example ofdm_roundtrip → level 1 @ 8 dB and level 3 @ 12 dB: recovered all 44 bytes
```

### Tooling present
`cargo-deny` installed; `cargo-public-api`, `cargo-semver-checks`, `cargo-machete`, `cargo-udeps` not installed (so no unused-dep or API-diff check was possible; `cargo clippy` default profile is clean).
