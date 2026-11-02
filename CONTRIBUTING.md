# Contributing to Coppa

Thanks for your interest in contributing!

## Project ethos: stay honest

Coppa is an **honest reference implementation** of an HF modem's DSP/FEC/protocol
stack (OFDM, LDPC, ARQ). Its value is clear, correct, well-tested Rust —
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
cargo test --workspace            # full suite (integration, proptest, doctests)
cargo test --workspace --lib      # quick feedback; what ci.yml runs on every push
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- `rust-toolchain.toml` pins Rust 1.98.0, and rustup uses it for every `cargo` command in this
  tree. **MSRV is 1.85.0**: CI checks it with `cargo +1.85.0 check --workspace`, and you can run
  the same command locally after `rustup toolchain install 1.85.0`.
- CI has two tiers. `ci.yml` runs on every push, pull request and merge-queue run: `cargo check`
  (default, all and no default features), `cargo test --lib` with features, `cargo fmt --check`,
  the tutorial checks, rustdoc with `-D warnings`, and a Linux/macOS/Windows matrix (`--lib`
  only). `ci-full.yml` runs the full `cargo test --workspace` suite, clippy with `-D warnings`,
  the MSRV check, cargo-deny, the security audit, bench and fuzz builds, and `wiki-lint` on
  `wiki/`. Run the full `cargo test --workspace` locally before pushing; it is faster feedback
  than waiting for `ci-full.yml`.
- To exercise feature-gated code, use CI's feature set:
  `--features cpal-backend,websocket,kiss-tnc,gpio-ptt,ardop`. `cpal-backend` needs
  `libasound2-dev`. The `serial-ptt` feature (and so `--all-features`) also needs `libudev-dev`.

## Submitting changes

- Keep pull requests small and focused — one logical change per PR.
- Make sure CI is green: clippy (`-D warnings`), `fmt`, and tests across the
  Linux/macOS/Windows matrix.
- Match the surrounding code's style, naming, and comment density.

## Process

- **Claim before you work.** Search the open pull requests first, then open a draft pull request
  early: the draft is the claim. Main moves only by pull request merge, through GitHub's merge
  queue ([docs/DECISIONS/2026-09-15-native-merge-queue-cutover.md](docs/DECISIONS/2026-09-15-native-merge-queue-cutover.md)).
- **Review rounds converge.** Every round fixes P1 findings in the pull request. From round 2,
  P2-and-lower findings go into a follow-up ticket instead
  ([docs/DECISIONS/2026-08-07-pr-review-convergence-policy.md](docs/DECISIONS/2026-08-07-pr-review-convergence-policy.md)).
- Open items and the evidence for each are in [ROADMAP.md](ROADMAP.md).

## Documentation

- Rustdoc must build without warnings. CI runs
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`, which needs
  `libudev-dev`. Without it, run
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --features cpal-backend,websocket,kiss-tnc,gpio-ptt,ardop`.
  Do not link a private item from public docs; write its name in backticks instead. Escape
  indices in math as `\[k\]` so rustdoc does not read them as links.
- The [embedding guide](docs/tutorials/embedding.md) is doctested: the root crate includes it
  under `#[cfg(doctest)]`, so `cargo test --workspace` compiles and runs its `rust` blocks.
- Add a new limitation at the end of [docs/LIMITATIONS.md](docs/LIMITATIONS.md).
- Record a design decision as an ADR in [docs/adr/](docs/adr/) and add its row to the
  [ADR index](docs/adr/README.md). Process and owner calls go in [docs/DECISIONS/](docs/DECISIONS/).
- `wiki/` is descriptive and always loses a conflict with code and `docs/`. When you edit a
  page, update its frontmatter `verified` commit and date and its entry in `wiki/manifest.json`
  (the `hash` of the page body, `verified_commit` and `verified_date`); `wiki-lint` checks them.
- Cite tracked files only. Local working notes are gitignored, so a tracked file must never
  link them; cite the ADR, `docs/benchmarks/` log or pull request instead
  ([docs/DECISIONS/2026-10-14-docs-architecture.md](docs/DECISIONS/2026-10-14-docs-architecture.md)).

## Benchmarks

- Criterion micro-benchmarks run with `cargo bench --workspace`. CI only compiles them
  (`cargo bench --no-run --workspace`).
- The measurement harness runs by hand: `cargo run -p coppa-bench --release -- --trials 50`
  for a waterfall sweep, and `cargo run -p coppa-bench --release --example <name>` for a named
  experiment. `cargo test -p coppa-bench --lib waterfall_bracket` runs the per-level FER
  regression guards.
- Put a new measurement log in `docs/benchmarks/<date>-<topic>.md`, then add one line to the
  summary and one row to the log index in [BENCHMARKS.md](BENCHMARKS.md).

## Golden vectors

`testdata/golden/` holds 20 reference WAVs and a `manifest.toml` that
`crates/coppa-protocol/tests/golden_vectors.rs` decodes as a frozen regression corpus
([docs/SPEC.md](docs/SPEC.md) §14).

```bash
cargo run -p coppa-bench --release --example golden_vectors_gen   # from the workspace root
cargo test -p coppa-protocol --test golden_vectors
```

Regeneration is not byte-reproducible across machines. On one machine it rewrote 16 of the
20 WAVs and one `manifest.toml` entry, while the golden test still passed on both the old and
the regenerated corpus; the cause was not established. So regenerate only for a deliberate
wire-format change, review `git diff --stat testdata` before committing, and commit only the
vectors the change is meant to affect.

## Adding a speed level or profile

A speed level touches all of these; [ADR-009](docs/adr/009-cop24-speed-ladder-renumber.md) is
the precedent for a ladder change:

- `SPEED_LEVELS` in `crates/coppa-codec/src/ofdm/coppa_modem.rs` (a fixed-size array).
- `speed_level_components`, `k_used_for_level` and `max_payload_for_level` in
  `crates/coppa-protocol/src/modem/speed_levels.rs`.
- `HF_MAX_SPEED_LEVEL` (`coppa-engine`'s `config.rs`, `engine.rs` and `lib.rs`, and
  `crates/coppa-daemon/src/event_loop.rs`) and `crates/coppa-engine/src/ladder.rs`.
- The `coppa-ml` tables in `rate_loop.rs` and `mcs.rs`.
- [docs/SPEC.md](docs/SPEC.md) §5.2 (rate matching) and §6 (speed-level table).
- The golden-vector grid in `golden_vectors_gen.rs` (see above).
- A `waterfall_bracket_level_N` test in `crates/coppa-bench/src/runner.rs`.
- An ADR.

A new profile touches the `CoppaProfile` constructors in `crates/coppa-codec/src/ofdm/mod.rs`,
`profile_by_name` and `select_profile` in `crates/coppa-bench/src/scenario.rs`, and the named
profiles in `crates/coppa-engine/src/profiles.rs`.

## Licensing

By contributing, you agree that your contributions are dual-licensed under
**MIT OR Apache-2.0**, the same terms as the project (see the README's License
section).
