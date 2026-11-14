coppa {version} is a pre-1.0 release. It is simulation-validated only: there has
been no over-the-air validation yet (COP-20 tracks the first on-air result).
Expect the wire format, the host APIs and the configuration file to change
between releases. Read the change list in docs/SPEC.md before you mix versions
on one link. Known gaps are listed in docs/LIMITATIONS.md.

## Downloads

| Platform | File |
|---|---|
| Linux x86-64, glibc 2.31 or later | `coppa-{version}-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64, e.g. Raspberry Pi 3/4/5 on a 64-bit OS | `coppa-{version}-aarch64-unknown-linux-gnu.tar.gz` |
| Linux armv7, e.g. Raspberry Pi 2/3/4 on a 32-bit OS | `coppa-{version}-armv7-unknown-linux-gnueabihf.tar.gz` |
| Windows 10 or later, x86-64 | `coppa-{version}-x86_64-pc-windows-msvc.zip` |
| macOS 11 or later, Apple silicon and Intel | `coppa-{version}-universal-apple-darwin.tar.gz` |

Each archive holds `coppa` (the CLI) and `coppad` (the daemon). `coppad` is built with
cpal-backend, websocket, serial-ptt, gpio-ptt, kiss-tnc, ardop and mdns; `coppa`
with cpal-backend and kiss-tnc. GPIO PTT works on Linux only. The Linux binaries
need `libasound.so.2` (package `libasound2`, or `libasound2t64` on newer
distributions). They do not run on ARMv6 boards (Raspberry Pi 1, Zero, Zero W).

Check a download:

```
sha256sum --ignore-missing -c SHA256SUMS
gh attestation verify coppa-{version}-aarch64-unknown-linux-gnu.tar.gz --repo HagaleTechnologies/coppa
```

Install with cargo-binstall. The crates are not on crates.io, so name the version:

```
cargo binstall --git https://github.com/HagaleTechnologies/coppa coppa-cli@{version}
cargo binstall --git https://github.com/HagaleTechnologies/coppa coppa-daemon@{version}
```

The macOS and Windows binaries are not signed. If macOS refuses to open a binary
you downloaded with a browser, run: `xattr -d com.apple.quarantine coppa coppad`.
Windows may show a SmartScreen warning.

Install guides: README "Install" and docs/tutorials/raspberry-pi-install.md.
