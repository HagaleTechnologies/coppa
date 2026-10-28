---
id: cli-settings
title: What will bite you when persisting coppa CLI settings?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-cli/src/settings.rs
  - crates/coppa-cli/src/main.rs
  - crates/coppa-daemon/src/config.rs
  - crates/coppa-radio/src/band.rs
  - coppad.toml.example
  - docs/tutorials/getting-started.md
verified:
  commit: b5144e3
  date: 2026-10-28
links:
---
The full setup and search order are in [Getting started](../../docs/tutorials/getting-started.md#persisting-your-cli-settings).

- Copying `coppad.toml.example` selects `HF_STANDARD`. The CLI's built-in no-file profile differs; both ends must use the same profile to decode each other.
- `band = ""` is an explicit, invalid band label. Remove the key if you want no band evidence.
- `coppa tnc` has a separate AFSK configuration path and does not read these settings. `devices` and `config` also ignore them.
