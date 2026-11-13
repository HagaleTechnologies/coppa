# Release binaries and versioning: tagged pre-1.0 releases with prebuilt binaries

Recorded by COP-47 on 2026-11-09. Before COP-47 the repository had no tags, no releases
and no release workflow, so every user had to clone the repository and run `cargo build`.
The maintainer runbook is [docs/RELEASING.md](../RELEASING.md).

## Owner direction

The ticket's comments are newer than its description, and two of them decide scope.

Comment 3, Catalyst Cloud relaying the repo owner, 2026-11-09T17:16:52Z:

> "Release authorized by the repo owner (2026-11-09): this ticket is no longer held. Cut
> tagged GitHub releases."
>
> "tag the current workspace version (v0.11.0) as the first public release unless the plan
> finds a reason otherwise, and bump minor for each release after."
>
> "Release notes must say plainly this is pre-1.0, simulation-validated only, with no
> over-the-air validation yet (COP-20)."

Comment 2, Tony Hagale, 2026-10-15T02:25:53Z:

> "versioning is decided: stay pre-1.0"
>
> "Name stays coppa."
>
> "patch bumps by default (0.5.4 -> 0.5.5), minor bump only for a wire-format break."
>
> "Still held as a public release step."

### Conflicts and how they were resolved

- **Version bumps.** Comment 2 asks for patch bumps by default; comment 3, which is newer,
  says "bump minor for each release after". This decision follows comment 3. The PR-level
  rule in [2026-11-02-public-rust-api-semver-gate.md](2026-11-02-public-rust-api-semver-gate.md)
  (a library API break bumps the minor version) is compatible with both and is unchanged.
- **Whether releases are held.** Comment 2 says "Still held as a public release step";
  comment 3 says "this ticket is no longer held". This decision follows comment 3.
- **"Static binaries."** The ticket description asks for static binaries, and no comment
  touches that word. On Linux the binaries are not fully static (D2 says why). They link
  glibc and `libasound.so.2` dynamically and every Rust dependency statically. On Windows
  the C runtime is linked statically (D11). On macOS the system frameworks are always
  dynamic.

## Decisions

| # | Decision | Evidence |
|---|---|---|
| D1 | The first tag is `v0.11.0`. A tag must equal both `Cargo.toml` versions and have major version `0`. Each later release bumps the minor version. The workflow enforces the first two rules; `docs/RELEASING.md` carries the third. | Comment 3. No document uses 0.11.0 for anything but the current version: `docs/SPEC.md` lists spec revision 4 at workspace 0.11.0, and `docs/HOST-API.md` says chat needs "coppad 0.11.0 or later", both true of a v0.11.0 release. Enforcing "minor bump per release" in CI would block an owner-approved patch release later with no gain. |
| D2 | Linux binaries are glibc-linked with a 2.31 floor, built with `cargo zigbuild --target <triple>.2.31` and linked against ALSA from pinned Debian 11 packages. Only libc, libm, libpthread, libdl, the loader and `libasound.so.2` stay dynamic. | A plain `cargo build --release` on Ubuntu 24.04 (glibc 2.39) produced a `coppad` that failed under Debian 11's loader with ``version `GLIBC_2.35' not found``. The zigbuild binaries for x86-64, aarch64 and armv7 needed at most `GLIBC_2.30` and ran `coppa --version`, `coppa loopback` and `coppad` against Debian 11's glibc 2.31 and ALSA 1.2.4 (x86-64 through `ld-2.31.so`, ARM through qemu-user). glibc 2.31 covers Debian 11+, Ubuntu 20.04+ and Raspberry Pi OS Bullseye, Bookworm and Trixie. Debian's `libasound2-dev` ships no `libasound.a`, and a static ALSA cannot load the plugins that route a desktop's "default" device through PipeWire or PulseAudio (reasoned, not tested). |
| D3 | `serialport` is built without its default `libudev` feature. | coppa only opens a named port and drives DTR/RTS (`crates/coppa-radio/src/ptt_serial.rs`); serialport uses libudev only for port enumeration, which coppa never calls. With the default feature, every cross build failed in `libudev-sys`; without it, the full-feature build succeeded with no libudev installed, and `cargo test -p coppa-radio --lib --features serial-ptt,gpio-ptt` and `cargo test -p coppa-daemon --features serial-ptt --test runtime_smoke` passed. |
| D4 | One release feature list on every OS, in one `cargo` invocation: `coppa-cli/cpal-backend,coppa-cli/kiss-tnc,coppa-daemon/cpal-backend,coppa-daemon/websocket,coppa-daemon/serial-ptt,coppa-daemon/gpio-ptt,coppa-daemon/kiss-tnc,coppa-daemon/ardop,coppa-daemon/mdns`. | The ticket's five features plus `ardop` and `mdns`, which stay off until enabled in `coppad.toml`; ci.yml's Windows step already called that set "the release feature set". `gpio-ptt` compiles on every OS and fails at runtime off Linux. Package-qualified names do not depend on how cargo applies unqualified features across two `-p` packages. `tools/release/test_release.py` fails when a new daemon feature appears and nobody decides whether it ships. |
| D5 | `[profile.release]` sets `lto = "fat"`, `codegen-units = 1` and `strip = "debuginfo"`. `panic` stays `unwind`. | Measured on x86-64, 32 cores, both binaries: the default profile took 111 s and gave a 10.1 MB `coppa` and 12.4 MB `coppad`; fat LTO with one codegen unit took the same 111 s and gave 5.1 MB and 6.0 MB with `strip = "symbols"`, or 6.0 MB and 7.3 MB with symbol names kept. Thin LTO was slower (139 s) and larger. Symbol names stay so a panic backtrace from the field is readable. `coppa-ffi` relies on `catch_unwind`. `cargo bench --no-run --workspace` grew from 246 s to 293 s; tests use the dev profile, so FER results do not change. |
| D6 | One archive per target holding both binaries, with versioned asset names, a `SHA256SUMS` file and `actions/attest-build-provenance` attestations. | One download per platform for people. cargo-binstall 1.26.2 picked one binary per package from the same archive against a local HTTPS stand-in. Attestations are free for public repositories. |
| D7 | cargo binstall works through `--git` with an explicit `@version`. Both binary packages carry `[package.metadata.binstall]`, and `[workspace.package]` carries `repository`. | Measured with cargo-binstall 1.26.2: `--git` takes one crate per command and reads the version from main's `Cargo.toml`, so with main ahead of the last release a bare `coppa-cli` asked for an asset that did not exist (404), while `coppa-cli@0.11.0` installed v0.11.0. Every doc names the version. Publishing to crates.io stays with H-130. |
| D8 | A normal (not prerelease) GitHub Release marked Latest, created as a draft, filled, then published. Notes come from `tools/release/notes.md`; GitHub's generated notes are added only when an earlier `v*` tag exists. | Comment 3 asks for the pre-1.0 wording in the notes, so it lives in a committed template with a test. Draft first means a failure never leaves a half-filled public release. With no earlier tag, generated notes would list every merged PR in the project's history. |
| D9 | A tag push publishes. A `pull_request` that touches the release machinery, and `workflow_dispatch`, build and smoke-test only. A read-only `verify` job gates everything and `publish` re-checks. Release builds use no `Swatinem/rust-cache`. Every action is pinned by commit SHA, and zig, cargo-zigbuild, cargo-about and the Debian packages by sha256. | Lessons from panino's PNO-104 and PNO-105 release hardening, and this repo's practice of pinning tool tarballs by sha256 (`ci-full.yml`'s semver job). A cache written by another ref must not feed release bytes. |
| D10 | macOS: native `cargo build` for `aarch64-apple-darwin` and `x86_64-apple-darwin` on `macos-15`, `lipo -create`, then an ad-hoc `codesign --force --sign -`, with `MACOSX_DEPLOYMENT_TARGET=11.0`. No notarization. | The macOS audio stack uses `objc2-core-audio` with no `bindgen`, so cross-architecture builds need nothing extra. Re-signing after `lipo` gives a valid ad-hoc signature. Notarization needs a Developer ID the project does not have. |
| D11 | Windows: `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static"` with `--target x86_64-pc-windows-msvc`. | Users need no Visual C++ redistributable; the workflow checks that neither `.exe` imports `VCRUNTIME*.dll` or `api-ms-win-crt-*.dll`. The target-scoped variable leaves build scripts and developer builds unchanged. |
| D12 | `chacha20` moves from the yanked 0.10.1 to 0.10.2, the `deny.toml` ignore for it is dropped, and `deny.toml` checks the six release targets with `all-features = true`. | `cargo tree -p coppa-daemon -e normal -i chacha20` showed 0.10.1 in `coppad` through `rand`. 0.10.1 was yanked because its SSE2 backend raises SIGILL on x86-64 CPUs without SSE4.1, which old shack PCs can be. Before COP-47, `deny.toml` checked three targets with default features, so the supply-chain job never saw `cpal`/`alsa`, `serialport`, `tokio-tungstenite`, `gpiocdev` or `mdns-sd`, all of which ship. cargo-deny 0.20.2 passed with the widened graph. |
| D13 | Every archive carries `THIRD-PARTY-LICENSES.txt`, generated by cargo-about 0.9.3 with `--workspace --all-features` from `about.toml` and `tools/release/about.hbs`. | MIT and BSD licences require the notice to travel with the binary. `serialport` is MPL-2.0, whose section 3.2(a) requires telling recipients of an executable where the source is, so the template prints each crate's repository URL or its crates.io URL. The test run produced 87 licence sections with no HTML escaping. |

## Not decided here

- Publishing the crates to crates.io (H-130 in
  `docs/reviews/2026-09-05-product-ux-market-review.md`). Until then binstall needs
  `--git` and `@version`.
- Code signing and notarization for macOS and Windows, and minisign signatures for
  binstall. Both need keys and accounts the project does not have.
- Fully static musl builds, ARMv6 (Raspberry Pi 1, Zero, Zero W), Windows arm64,
  `.deb`/`.rpm`/Homebrew packages and a `curl | sh` installer.
- Repository metadata beyond `repository` (COP-74).

## Upkeep

What is still unverified (no run on real Raspberry Pi hardware or a real sound card from a
release archive, unsigned binaries, binstall's macOS fallback) is listed in
`docs/LIMITATIONS.md` under COP-47. Bumping a pinned tool or Debian package, and the asset
contract, are covered in `docs/RELEASING.md`.
