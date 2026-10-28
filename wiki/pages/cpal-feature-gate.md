---
id: cpal-feature-gate
title: What will bite you about the cpal-backend feature flag?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-audio/Cargo.toml
  - crates/coppa-daemon/src/**
verified:
  commit: f1dcac4
  date: 2026-10-28
links:
  - overview
---
The daemon (`coppad`) compiles and runs without the `cpal-backend` feature flag,
but will not move any audio — it runs the event loop with no-op audio
I/O. This is not an error; it is intentional for headless/testing setups. The
same applies to `coppa devices` in the CLI: without `--features cpal-backend`
the command compiles but finds nothing.

## Symptom

You build and run `coppad` and it starts cleanly, but no audio is captured or
played, and no transmit/receive activity occurs even with correct config.
Since COP-50 the startup log says so: `WARN coppad: Audio devices: none; this
coppad was built without the cpal-backend feature, so it can neither receive
nor transmit`. Since COP-32, setting `[audio] input_device`/`output_device` in
such a build is a startup error, because the build can never open a named
device.

## Cause and workaround

`coppa-audio` gates the CPAL backend behind `cpal-backend` so that the crate
compiles on platforms without ALSA/CoreAudio/WASAPI headers. When the feature is
absent, the audio backend is replaced with a stub that immediately returns empty
buffers.

Build with the feature to get real audio:
```
cargo build --features cpal-backend
cargo test --workspace --features cpal-backend,websocket
```

CI runs `test-full` on Linux with both features enabled (requires
`libasound2-dev` apt package). Platform-specific CI jobs (`macos-latest`,
`windows-latest`) test `coppa-audio --lib --features cpal-backend` directly.
