---
id: release-binaries
title: What will bite you when building or installing coppa release binaries?
kind: gotcha
status: current
maintainer: agent
sources:
  - .github/workflows/release.yml
  - tools/release/release.py
  - docs/RELEASING.md
verified:
  commit: 4237273
  date: 2026-11-09
links:
  - cpal-feature-gate
  - phase4-field-readiness
  - overview
---
A plain `cargo build --release` on Ubuntu 24.04 makes Linux binaries that will
not start on Debian 11, Ubuntu 22.04 or a Raspberry Pi OS Bookworm image: they
need `GLIBC_2.39` (or `2.35`), and the loader refuses them with
``version `GLIBC_2.35' not found``. The release workflow builds Linux with
`cargo zigbuild --target <triple>.2.31` instead, and
`tools/release/smoke-linux.sh inspect` fails any binary whose highest needed
glibc version is above 2.31. Run it on a native build to see the trap.

`serialport` is built without its default `libudev` feature. coppa only opens a
named port, and libudev serves port listing, which coppa never calls. Turning
the default back on breaks every cross build in `libudev-sys` and adds a
runtime `libudev.so.1` dependency; ci.yml's `check` job fails if
`libudev-sys` reappears.

`cargo binstall --git` reads the version from main's `Cargo.toml`, not from the
latest release. Once main is ahead of the last tag, a bare
`cargo binstall --git ... coppa-cli` asks for an asset that does not exist and
gets a 404. Always name the version (`coppa-cli@0.11.0`), and give one crate
per command. The binstall metadata on main also applies to every older version
a user pins, so the asset names and archive layout cannot change without
breaking past releases.

snapshot.debian.org often answers HTTP 503. `tools/release/debian-sysroot.sh`
tries deb.debian.org, then archive.debian.org, then snapshot, and trusts only
the pinned sha256.

The runbook, recovery steps and how to bump a pinned tool are in
[docs/RELEASING.md](../../docs/RELEASING.md). The decisions and their evidence
are in
[2026-11-09-release-binaries-and-versioning.md](../../docs/DECISIONS/2026-11-09-release-binaries-and-versioning.md).
