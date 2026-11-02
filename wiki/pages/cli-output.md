---
id: cli-output
title: Which stream and exit code does coppa use?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-cli/src/main.rs
  - crates/coppa-cli/src/interrupt.rs
  - crates/coppa-cli/src/style.rs
  - crates/coppa-audio/src/file_backend.rs
  - .github/workflows/ci.yml
  - docs/tutorials/getting-started.md
verified:
  commit: 9897cce
  date: 2026-11-02
links:
---
User-facing behaviour is in [Getting started](../../docs/tutorials/getting-started.md); the limits are in [LIMITATIONS](../../docs/LIMITATIONS.md). This page records what bites when you change the CLI (COP-86).

- stdout is data only: decoded frames, `--json`, `--raw`, raw PCM and the `profiles`/`devices`/`doctor`/`loopback`/`completions`/`man` reports. Status lines go to stderr. `#![cfg_attr(not(test), deny(clippy::print_stdout))]` in `main.rs` enforces it, so write stdout through `emit`/`emit_raw`/`write_json`, never `println!`.
- A closed stdout pipe is a quiet exit 0 (`is_broken_pipe` in `main`). Keep the `io::Error` as an error source (`.context(..)`, not `anyhow!("{e}")`) or the check cannot see it.
- Interrupted `rx` exits 0, or 1 on a decode failure. Interrupted live `tx`/`tune` exits 130 after releasing PTT. A second Ctrl-C exits 130 with no cleanup. The handler is installed only by `cmd_rx` and `transmit_live`.
- The COP-73 tutorial guard in `ci.yml` captures `2>&1`. Without it, it would pass vacuously once status left stdout.
- COP-51's library-diagnostic test now uses `RawPcmReader`'s truncated-sample `WARN`. `WavSink` no longer logs a second error after a failed `stop()`.
- `clap_complete` lists hidden subcommands, so `completion_command()` drops them before generating.
