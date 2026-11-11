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
- Coppa is **not** RF/waveform-compatible with VARA or any other proprietary modem
  and, by policy, will not be; the VARA-style TCP interface is an API shape only.

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
  the MSRV check, cargo-semver-checks, the C ABI compatibility check
  (`docs/testing/c-abi-check.md`), cargo-deny, the security audit, bench and fuzz builds, and
  `wiki-lint` on `wiki/`. Run the full `cargo test --workspace` locally before pushing; it is
  faster feedback than waiting for `ci-full.yml`.
- To exercise feature-gated code, use CI's feature set:
  `--features cpal-backend,websocket,kiss-tnc,gpio-ptt,ardop,mdns`. `cpal-backend` needs
  `libasound2-dev`. The `serial-ptt` feature (and so `--all-features`) also needs `libudev-dev`.

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
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --features cpal-backend,websocket,kiss-tnc,gpio-ptt,ardop,mdns`.
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
- Measurement programs live in `crates/coppa-bench/diagnostics/`. Each needs an
  `[[example]]` stanza in that crate's `Cargo.toml` because `autoexamples = false`;
  `tests/diagnostics_registered.rs` enforces registration.
- Put a new measurement log in `docs/benchmarks/<date>-<topic>.md`, then add one line to the
  summary and one row to the log index in [BENCHMARKS.md](BENCHMARKS.md).

## Golden vectors

`testdata/golden/` holds 42 reference vectors and a `manifest.toml` decoded by
`crates/coppa-protocol/tests/golden_vectors.rs` as a frozen regression corpus
([docs/SPEC.md](docs/SPEC.md) §14).

```bash
cargo run -p coppa-bench --release --example golden_vectors_gen
cargo test -p coppa-protocol --test golden_vectors
```

The generator is additive: it preserves every existing manifest entry and writes only
missing entries. To refresh a vector deliberately, delete its manifest entry and rerun.
Review `git diff --stat testdata` and commit only the intended vectors. Regeneration
across machines is not byte-reproducible; the [COP-129 log](docs/benchmarks/2026-11-08-cop-129-golden-vector-coverage.md)
records why existing WAVs are preserved.

## Adding a speed level or profile

A speed level touches all of these; [ADR-009](docs/adr/009-cop24-speed-ladder-renumber.md) is
the precedent for a ladder change, from before the air-protocol version 1 freeze described below:

- `SPEED_LEVELS` in `crates/coppa-codec/src/ofdm/coppa_modem.rs` (a fixed-size array).
- `speed_level_components`, `k_used_for_level` and `max_payload_for_level` in
  `crates/coppa-protocol/src/modem/speed_levels.rs`.
- `HF_MAX_SPEED_LEVEL` (`coppa-engine`'s `config.rs`, `engine.rs` and `lib.rs`, and
  `crates/coppa-daemon/src/event_loop.rs`) and `crates/coppa-engine/src/ladder.rs`.
- The `coppa-ml` tables in `rate_loop.rs` and `mcs.rs`.
- [docs/SPEC.md](docs/SPEC.md) §5.2 (rate matching) and §6 (speed-level table).
- The golden-vector grid in `golden_vectors_gen.rs` (see above).
- A `waterfall_bracket_level_N` test in `crates/coppa-bench/src/runner.rs`.
- `air_v1_speed_level_fec_parameters` in `crates/coppa-protocol/tests/air_v1_conformance.rs`,
  which pins the version-1 level table.
- An ADR.

A new profile touches the `CoppaProfile` constructors in `crates/coppa-codec/src/ofdm/mod.rs`,
`profile_by_name` and `select_profile` in `crates/coppa-bench/src/scenario.rs`, and the named
profiles in `crates/coppa-engine/src/profiles.rs`.

[docs/SPEC.md](docs/SPEC.md) §0.3 (COP-178) freezes air-protocol version 1, including the
per-level and profile parameters (`air_v1_profile_parameters` guards the latter). A change to a
frozen item, or an on-air addition that no existing negotiation mechanism gates, is incompatible
(§0.3.4). It must allocate a new air-protocol version, add a §0.1 history entry and a new freeze
table, and record the break in `CHANGELOG.md` (§0.3.1, §0.3.2). ADR-009 kept header `version: 1`
for its ladder change; §0.3.1 no longer allows that.

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
