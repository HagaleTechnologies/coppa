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
- Coppa is **not** RF/waveform-compatible with VARA or any other proprietary modem
  and, by policy, will not be; the VARA-style TCP interface is an API shape only.

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
- If your change breaks a public Rust API in a library crate, bump the minor
  `version` in the root `Cargo.toml` (`[workspace.package]` and `[package]`) in
  the same PR. CI's semver job fails otherwise; see
  `docs/DECISIONS/2026-11-02-public-rust-api-semver-gate.md`.
- Confirm in the PR description that the work is clean-room. The PR template has
  the checklist. `gh pr create --body` skips the template, so paste the checklist in.

## Licensing and clean-room rules

By contributing, you agree that your contributions are dual-licensed under
**MIT OR Apache-2.0**, the same terms as the project (see the README's License
section).

Every contribution, from a person or an agent, follows these rules. The full policy,
with what counts as public documentation and the history before it, is
[docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md](docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md).

- **Implement from public documentation only.** Standards, published papers,
  published protocol and command documents, and coppa's own [spec](docs/SPEC.md).
- **Never read, port or copy copyleft or proprietary modem code.** This covers the
  source of any modem, TNC or radio-protocol implementation under a copyleft licence
  (GPL, AGPL, LGPL) or a proprietary one, such as Direwolf, srsRAN, VARA or PACTOR.
  You may run an unmodified program as a black-box test oracle outside this
  repository and commit its outputs with a note of where they came from.
- **Copied code from any source needs the owner's explicit approval first.** This
  includes permissively licensed projects and other Hagale Technologies repositories.
  Ask in a GitHub issue before you copy, then link the approval and the source's
  licence in the PR, and keep the source's copyright notice.
- **cargo-deny's licence allowlist is the enforcement point.** `deny.toml`
  `[licenses] allow` lists the licences a dependency may carry. The
  "Supply-chain check (cargo-deny)" job in `ci-full.yml` fails a PR that adds a
  dependency outside the list, and adding a licence to it is an owner decision.
  The check reads dependency metadata, not source files, so pasted code passes it.
  The PR template's clean-room confirmation and review cover that gap.
