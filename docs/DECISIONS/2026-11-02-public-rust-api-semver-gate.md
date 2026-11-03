# Public Rust API semver gate: a library API break bumps the minor version

Recorded by COP-90 (H-135) on 2026-11-02. Answers the `cargo semver-checks` part of
`docs/reviews/2026-09-05-product-ux-market-review.md` row H-135.

## Context

The workspace crates are not published to crates.io, but they already have downstream
users. skimmer depends on `coppa-dsp`, and `wiki/pages/coppa-dsp-skimmer-interface.md`
noted that nothing enforced that contract. Before COP-90, a pull request could remove or
change a public item in any library crate and CI stayed green.

## Decision

1. **`ci-full.yml` runs a "Semver check (cargo-semver-checks)" job.** It runs
   `cargo-semver-checks` 0.51.0, installed from the release tarball and verified by
   sha256, against `git merge-base HEAD origin/main`. The check therefore covers only the
   revision's own changes. When the merge base is `HEAD` (a push to main) there is nothing
   to compare and the job exits 0, because the `ci` fan-in requires every job to succeed.
2. **A public API break in a library crate bumps the minor version while the workspace is
   at 0.x.** Bump both `[workspace.package] version` and the root `[package] version` in
   `Cargo.toml`, in the same PR. That is the tool's own mechanism: at 0.x it treats a minor
   bump as a major change, so no custom CI logic is needed. COP-90 bumped 0.3.0 to 0.4.0 for
   its own breaks (`#[non_exhaustive]` on six enums, `Session` taking `now: Instant`).
3. **A minor bump no longer implies a wire break by itself.** ADR-009 and ADR-010 bump the
   minor version for an incompatible wire change. That rule stands; this decision adds a
   second reason for the same bump. `docs/SPEC.md`'s change list stays the record of wire
   breaks, so read it, not the version number, to decide whether two stations interoperate.

## Gated and excluded packages

Gated: the 11 library packages an embedding developer can depend on: the root `coppa`
package, `coppa-dsp`, `coppa-codec`, `coppa-protocol`, `coppa-channel`, `coppa-audio`,
`coppa-radio`, `coppa-ml`, `coppa-engine`, `coppa-host` and `coppa-ffi`.

Excluded:

- `coppa-cli` has no library target.
- `coppa-daemon`'s library exists to serve its own binary and tests.
- `coppa-bench` and `tools/gen_nr_bg2` are internal measurement and code-generation tools.

## Rejected alternatives

- **`cargo public-api` snapshots.** The tool reads nightly rustdoc JSON, and the repo pins
  stable 1.98.0 in `rust-toolchain.toml` (COP-9). Revisit when the crates are published
  (H-130).
- **A custom check that a break bumps the major version.** At 0.x that would mean 1.0.0 on
  the first break, which the project has not decided to ship.

## Upkeep

`cargo-semver-checks` reads rustdoc JSON, whose format follows the compiler. When
`rust-toolchain.toml` moves to a new Rust release, move the tool to a release that supports
it, and update the pinned version, download URL and sha256 in `ci-full.yml` together.
