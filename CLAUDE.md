# Coppa - Claude Code Instructions

## Project Overview

Coppa is an open-source OFDM digital modem for amateur radio, written in Rust and published as a **reference implementation** of an HF modem's DSP/FEC/protocol stack. It includes a full DSP chain, a protocol stack with ARQ, an AFSK 1200/AX.25 TNC, CLI tools, a daemon, C FFI bindings, and a VARA-style TCP control interface (modeled on VARA's TCP TNC API; the modem is **not** RF/waveform-compatible with VARA and does not interoperate with it).

## Build & Test Commands

```bash
cargo build --workspace
cargo test --workspace --lib  # fast tier; what ci.yml runs
cargo test --workspace  # full suite; run before pushing
cargo test --workspace --features cpal-backend,websocket,kiss-tnc,ardop --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo bench --workspace
```

## Testing Policy

**Run `cargo test --workspace` locally before pushing.** CI runs the full suite too (`ci-full.yml`'s `test-full` job, Linux, `--features cpal-backend,websocket,ardop`), but a local run is faster feedback. At minimum, run `cargo test --workspace --lib`.

## Workspace Structure

13 crates under `crates/` (12 product crates plus the `coppa-bench` measurement harness):

| Crate | Role |
|-------|------|
| `coppa-dsp` | Pure DSP: FFT, FIR filters, AGC, carrier and timing recovery |
| `coppa-codec` | Modulation: BPSK, QPSK, 8PSK, QAM, OFDM, AFSK 1200 |
| `coppa-protocol` | CoppaTransceiver pipeline, NR BG2 LDPC, framing, ARQ, compression, sessions |
| `coppa-channel` | Channel models for testing (AWGN, Watterson fading, SSB filter, CFO, sinusoidal fade) |
| `coppa-audio` | Audio backends: CPAL (feature-gated), WAV file I/O |
| `coppa-radio` | Radio control via rigctld CAT; serial and GPIO PTT (feature-gated) |
| `coppa-ml` | Adaptive link control: capacity-based MCS selection, closed-loop rate control, spread-gated short-CP, spectrum sensing |
| `coppa-engine` | Core engine: thin wrapper around CoppaTransceiver |
| `coppa-host` | VARA-style TCP control server, WebSocket JSON API, KISS TNC server (feature-gated) |
| `coppa-ffi` | C FFI (cdylib + staticlib) with streaming decode |
| `coppa-cli` | CLI binary (`coppa`) |
| `coppa-daemon` | Daemon binary (`coppad`) |
| `coppa-bench` | Measurement harness (not a product crate); its lib tests guard each level's FER waterfall |

## Key Architecture

- **CoppaTransceiver** (in `coppa-protocol`) composes CoppaModem + LDPC + constellation mappers + block interleaver. This is the main encode/decode pipeline.
- **CoppaCore** (in `coppa-engine`) is a thin ~210-line wrapper around CoppaTransceiver.
- **9 speed levels, 1-9** (COP-24 renumber: old 9→8, old 10→9; HF engines carry 1-8) replace old mcs_index/fec_rate/modulation config. All profiles unified at 48kHz sample rate. Text written before COP-24 (`docs/LIMITATIONS.md`, `BENCHMARKS.md`, ADR-002 to ADR-008) uses the old numbers; ADR-009 has the mapping.
- FFI uses pointer-to-pointer semantics in `coppa_engine_destroy` to prevent double-free.

## CI and MSRV

- `ci.yml` (every push, PR and merge-queue run): `cargo check` with default, all and no default features; `cargo test --lib` with features; `cargo fmt --check`; and a Linux/macOS/Windows matrix (`--lib` only).
- `ci-full.yml` (dispatched once the Codex review gate passes, plus push and merge queue): full test suite, bench and fuzz builds, clippy `-D warnings`, MSRV, cargo-deny, security audit, and sunto's `wiki-lint` on `wiki/`.
- MSRV is 1.85.0, enforced in CI and `Cargo.toml`.

## Known limitations and where docs live

- `docs/LIMITATIONS.md` holds every measured gap, open bug and unmet acceptance bar. Read the bullets for the subsystem you are about to change; add new limitations there, not here.
- Decisions: `docs/adr/` (design), `docs/DECISIONS/` (process and owner calls). Measurements: `BENCHMARKS.md`. Normative wire format: `docs/SPEC.md`.
- `.superpowers/` and `docs/superpowers/` are local working notes and are gitignored. Never cite them from a tracked file; cite the ADR, BENCHMARKS.md section or PR instead (`docs/DECISIONS/2026-10-14-docs-architecture.md`).

## Knowledge wiki

`wiki/INDEX.md` is the map of accumulated knowledge — read it before deep
exploration; open pages relevant to your task. After substantive work, run
/wiki-update: distill new gotchas/decisions/corrections into the wiki (or
into docs/ if normative — the wiki points, it never restates). The wiki is
descriptive and always loses conflicts with code and docs/.

## Multi-agent hygiene

You are never alone in this repo — other agents may be working concurrently
in other clones, branches, or worktrees.

- **Start fresh:** `git fetch` and rebase onto `origin/main` before reading
  code or making decisions; stale context produces wrong work.
- **Claim before work:** search open PRs/issues first; open a draft PR early —
  the draft PR *is* the claim. Don't duplicate in-flight work.
- **Isolate:** always a branch (worktree preferred), never a shared checkout's
  main. Use per-session scratch dirs; don't bind fixed ports.
- **Flush at the end:** push (`--force-with-lease` only) and open/update your
  PR before finishing. Unpushed work is invisible work.
- **Main moves only by PR merge.**

## Code review convergence

Every review round fixes P1 findings inline. From round 2 onward, P2-and-
lower findings are not fixed inline — they're captured verbatim into a
follow-up ticket instead, so the PR converges instead of chasing
progressively finer findings across rounds. Round 1 is unrestricted (fix
everything reasonable). Full policy:
docs/DECISIONS/2026-08-07-pr-review-convergence-policy.md.
