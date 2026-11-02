# Contributing to Coppa

Thanks for your interest in contributing!

## Project ethos: stay honest

Coppa is an **honest reference implementation** of an HF modem's DSP/FEC/protocol
stack (OFDM, LDPC, Viterbi, ARQ). Its value is clear, correct, well-tested Rust —
not marketing. Please keep it that way:

- **Don't add aspirational framing.** Describe what the code *does*, not what it
  might someday do.
- **Document limitations plainly.** If something is a stub, partial, or untested
  against realistic conditions, say so (see the status table in the README).
- **No unbacked performance claims.** Don't state throughput, BER, or
  "near-Shannon"-style numbers without checked-in data to support them.
- Coppa is **not** RF/waveform-compatible with VARA and does not aim to be; the
  VARA-style TCP interface is an API shape only.

## Building and testing

```bash
cargo build --workspace
cargo test --workspace            # full suite (integration + proptest)
cargo test --workspace --lib      # quick feedback; this is what CI runs
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- **MSRV is 1.85.0** (enforced in CI).
- CI runs only `--lib` tests to save minutes, so **run the full
  `cargo test --workspace` locally before pushing.**
- To exercise feature-gated code, build with `--features cpal-backend,websocket`.

## Submitting changes

- Keep pull requests small and focused — one logical change per PR.
- Make sure CI is green: clippy (`-D warnings`), `fmt`, and tests across the
  Linux/macOS/Windows matrix.
- Match the surrounding code's style, naming, and comment density.

## Licensing

By contributing, you agree that your contributions are dual-licensed under
**MIT OR Apache-2.0**, the same terms as the project (see the README's License
section).
