# C ABI compatibility check (COP-178)

`tools/check_ffi_abi.py` decides whether a change to `coppa-ffi` keeps the promise in
[docs/DECISIONS/2026-11-03-c-abi-versioning.md](../DECISIONS/2026-11-03-c-abi-versioning.md):
a program built against a header keeps working with a library of the same ABI major and an
equal or higher minor. CI runs it as the `C ABI compatibility` job in
`.github/workflows/ci-full.yml`, and the required `CI` check fails when it fails.

The Rust `semver` job (`cargo-semver-checks`,
[2026-11-02-public-rust-api-semver-gate.md](../DECISIONS/2026-11-02-public-rust-api-semver-gate.md))
is a different check. It compares Rust items and knows nothing about the C header, struct
layouts or `#define` values, so a C break can pass it. Only this job protects the C ABI.

## Running it

```text
python3 -m venv /tmp/abi-venv && /tmp/abi-venv/bin/pip install 'libclang==18.1.1'
COPPA_FFI_STRICT_HEADER=1 cargo build -p coppa-ffi
/tmp/abi-venv/bin/python -m unittest discover -s tools/tests -p 'test_check_ffi_abi.py'
/tmp/abi-venv/bin/python tools/check_ffi_abi.py --base-ref origin/main
C ABI baseline: bootstrap 42372733033dcac0908fe1b05a622fb17f34a2ee (no release tags)
C ABI: 1.0 -> 1.1
C ABI compatibility: PASS (compatible additions)
```

Requirements: Python 3.9 or later, the `libclang` Python package (its wheel bundles
`libclang.so`), a C compiler as `cc` (and `c++` for `--smoke`), `nm`, `git` and a built
`target/debug/libcoppa_ffi.so`. A missing requirement is an error, not a skip.
`COPPA_FFI_STRICT_HEADER=1` makes `build.rs` fail on a cbindgen error instead of leaving the
checked-in `coppa.h` stale; CI then runs `git diff --exit-code -- crates/coppa-ffi/coppa.h`
so the compared header is the one the source generates.

Exit status: 0 compatible, 1 incompatible change or bad ABI version metadata, 2 the
comparison could not be made (no compiler or libclang, unreadable baseline, failed build,
failed tag fetch). Each problem line names the function, struct, enum or constant, its old
and new value, and the ABI version it requires.

Options CI uses:

- `--fetch-tags` runs `git fetch --tags --force origin` first. A failed fetch fails the
  check; it is never read as "no tags".
- `--baseline-exports build` builds `coppa-ffi` at the baseline in a temporary `git worktree`
  and reads its exports with `nm`. Without it (the local default) the baseline's exports are
  taken to be the functions its header declares, and the current library's exports are
  compared over declared functions only.
- `--smoke` compiles the baseline's `crates/coppa-ffi/tests/c/error_strings.c` against the
  baseline header, as C11 and C++17, and runs it against the current library (an ABI 1.0
  caller on the new library). After a valid ABI major bump the baseline caller is skipped,
  since a major bump may break it. It also compiles the current header as C11 and C++17 and checks
  that `coppa_abi_version()` equals the header's version macros.
- `--write-manifests DIR` writes `baseline.json`, `current.json` and `diff.json`; CI uploads
  them as the `c-abi-manifests` artifact even when the check fails.

## What it compares

Declarations come from the Clang AST of each header, not from regular expressions. Values
come from a probe program the C compiler builds against each header. The manifest holds:

- every function: canonical return and parameter types, including pointer constness;
- every struct and union: field names, canonical types and order, `sizeof`, `_Alignof` and
  each field's `offsetof` (opaque forward declarations are recorded as opaque);
- every typedef's canonical underlying type and every enum constant's value;
- every object-like `COPPA_*` macro, which must be an integer constant expression;
- the library's exported `coppa_*` symbols. Every function the header declares must be
  exported, and a baseline export that disappears is a break even if a stale header still
  declares it.

A declaration kind the checker does not understand, a non-integer `COPPA_*` macro or a
header that does not parse fails extraction, so new syntax cannot slip out of the
comparison.

## Rules

- Removing or changing any function, struct or union layout, typedef, enum value, public
  constant or export is incompatible. It needs `COPPA_ABI_VERSION_MAJOR` above the
  baseline's and `COPPA_ABI_VERSION_MINOR` reset to 0.
- An addition needs the same major and a larger minor, or a valid major bump.
- The ABI version may not go down, and a major bump with a nonzero minor fails.
- `COPPA_ABI_VERSION_MAJOR` and `COPPA_ABI_VERSION_MINOR` are checked as version metadata
  under these rules, not as frozen constants.
- The crate version cannot authorize a C break; the Rust `semver` job still applies on its
  own terms.

## Baseline

The baseline is the newest stable `vMAJOR.MINOR.PATCH` tag reachable from `--base-ref`
whose workspace `Cargo.toml` version equals the tag. Pre-release tags, other tag names and
tags on other branches are ignored. CI passes the merge base with `origin/main` for a pull
request revision (`workflow_dispatch`) and `HEAD` for a push to main or a merge-queue
entry, so the check still runs when `HEAD` is main.

While the repository has no such tag, the baseline is the bootstrap commit pinned in
`tools/ffi-abi-baseline.json`: `42372733033dcac0908fe1b05a622fb17f34a2ee`, ABI 1.0, the
last main commit before COP-178. Its header is read from git with `git show`; the pull
request's own declarations are never copied into the baseline. The first release tag
replaces it automatically. A selected baseline without `crates/coppa-ffi/coppa.h` or without
the ABI version macros fails the check.

## Limits

- Linux x86_64 only. Canonical types such as `unsigned long` and the probed sizes and
  offsets are for that target; another platform's ABI is not measured.
- Behaviour is not compared. A function that keeps its signature but changes what it does
  passes; the C and Python smoke tests and the crate's tests cover behaviour.
- No release tag exists yet, so no comparison against a published release has run. The
  tagged-release path is covered by the unit tests' temporary git repositories.
