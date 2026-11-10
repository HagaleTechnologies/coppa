# Releasing coppa

This is the maintainer runbook for a tagged GitHub Release of the `coppa` and
`coppad` binaries. The decisions behind it are in
[docs/DECISIONS/2026-11-09-release-binaries-and-versioning.md](DECISIONS/2026-11-09-release-binaries-and-versioning.md).

coppa stays pre-1.0. Every release tag looks like `v0.MINOR.PATCH`, and the
release notes say in words that the build is pre-1.0 and simulation-validated
only, with no over-the-air validation yet (COP-20). The notes come from
[`tools/release/notes.md`](../tools/release/notes.md); do not edit them on the
Release page.

## How the workflow behaves

[`.github/workflows/release.yml`](../.github/workflows/release.yml) has six
jobs: `verify`, `notices`, `linux` (x86-64, aarch64 and armv7), `windows`,
`macos` and `publish`.

| Trigger | What runs |
|---|---|
| A pull request that changes `release.yml`, `tools/release/**` or `about.toml` | Everything except `publish`: build, smoke-test and package all five targets |
| `workflow_dispatch` (`gh workflow run release.yml --ref main`) | The same rehearsal, on the chosen ref |
| Pushing a `v0.MINOR.PATCH` tag | Everything, then `publish` creates the GitHub Release |

`verify` and `publish` both refuse a tag that:

- is not `v0.MINOR.PATCH` ("release tags look like v0.MINOR.PATCH; coppa
  stays pre-1.0");
- does not equal the `version` in both `[workspace.package]` and the root
  `[package]` of `Cargo.toml` ("tag v0.11.1 does not match Cargo.toml version
  0.11.0");
- points at a commit that is not on `origin/main` (if `origin/main` is missing
  the check exits 2, "cannot judge", and the run fails);
- already has a published release.

`publish` creates the release as a draft, uploads the five archives and
`SHA256SUMS`, attests build provenance for each archive, and only then makes
the release public and marks it Latest. A failure before the last step leaves
at most a draft, which only maintainers can see.

## Release checklist

1. **Decide the version.** The first release is `v0.11.0`. Each release after
   it bumps the minor version (0.11.0, then 0.12.0, then 0.13.0). If main's
   `Cargo.toml` already carries a minor version that was never released (the
   semver gate in
   [2026-11-02-public-rust-api-semver-gate.md](DECISIONS/2026-11-02-public-rust-api-semver-gate.md)
   bumps it for library API breaks), release that version. Otherwise merge a PR
   that bumps both `[workspace.package] version` and the root
   `[package] version` first.
2. **Update the version in the install docs** in the same PR: README
   "Install" and section 2 of
   [docs/tutorials/raspberry-pi-install.md](tutorials/raspberry-pi-install.md).
   Both name the version as `0.11.0` in their commands and asset names.
3. **Rehearse on main** and wait for it to pass:

   ```bash
   gh workflow run release.yml --ref main
   gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')" --exit-status
   ```

4. **Tag and push.** Tag `origin/main`, never a local branch:

   ```bash
   git fetch origin
   git show origin/main:Cargo.toml | grep -m1 '^version'   # must print the version you are releasing
   git tag -a v0.11.0 -m "coppa 0.11.0" origin/main
   git push origin v0.11.0
   ```

5. **Verify the published release** on a clean machine:

   ```bash
   v=0.11.0
   gh release download "v$v" --repo HagaleTechnologies/coppa \
       --pattern "coppa-$v-x86_64-unknown-linux-gnu.tar.gz" \
       --pattern "coppa-$v-aarch64-unknown-linux-gnu.tar.gz" --pattern SHA256SUMS
   sha256sum --ignore-missing -c SHA256SUMS
   gh attestation verify "coppa-$v-aarch64-unknown-linux-gnu.tar.gz" --repo HagaleTechnologies/coppa
   cargo binstall --git https://github.com/HagaleTechnologies/coppa coppa-cli@$v
   ```

   On a Mac, also run the `cargo binstall` line: binstall finds the
   `universal-apple-darwin` asset through its macOS fallback, which has not
   been tried on a real Mac yet.

## Recovery

- **The tag run failed before `publish`.** No release exists. Fix the problem
  on main through a normal PR, then move the tag to the new commit:
  `git push origin :refs/tags/v0.11.0`, re-create the annotated tag on the new
  `origin/main` and push it again. Do this only while no release exists for
  the tag.
- **`publish` failed and left a draft.** Delete the draft
  (`gh release delete v0.11.0 --yes`, which keeps the tag), then re-run the
  failed jobs of the same run from the Actions page. The build artifacts are
  kept for the run, and `publish` re-checks the tag before it uploads.
- **A published release is wrong.** Do not replace its assets: people may
  already have pinned them. Fix main and release the next version.

## The asset contract

The asset names and the archive layout are a public contract from the first
tag on:

- Asset names are `coppa-<version>-<target>.tar.gz`, or `.zip` for
  `x86_64-pc-windows-msvc`. The five targets are
  `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `armv7-unknown-linux-gnueabihf`, `x86_64-pc-windows-msvc` and
  `universal-apple-darwin`. `tools/release/release.py asset-name` is the one
  place the rule lives.
- Each archive has one top directory, `coppa-<version>-<target>/`, holding
  `coppa`, `coppad` (`.exe` on Windows), `README.md`, `LICENSE-APACHE`,
  `LICENSE-MIT`, `THIRD-PARTY-LICENSES.txt`, `coppad.toml.example` and
  `contrib/`.

`cargo binstall --git` reads the `[package.metadata.binstall]` sections of
`crates/coppa-cli/Cargo.toml` and `crates/coppa-daemon/Cargo.toml` from
main's current checkout, and applies them to every version a user pins,
including old ones. Changing an asset name, the archive format or the top
directory therefore breaks `cargo binstall ...@<old version>` for every past
release. If a change is unavoidable, keep the old names working with a
per-version override in the metadata. `tools/release/test_release.py` checks
that the metadata and `release.py` agree.

## Pinned tools and packages

Every download in the release path is pinned by version and sha256. To bump
one, change the version, the URL and the sha256 together, in the same commit,
and let the PR run of the release workflow prove the result.

| What | Where it is pinned |
|---|---|
| zig 0.13.0 and cargo-zigbuild 0.24.1 | `tools/release/install-zigbuild.sh` |
| Debian 11 `libasound2`, `libasound2-dev` (1.2.4-1.1) and `libc6` (2.31-13+deb11u11) for amd64, arm64 and armhf | `tools/release/debian-sysroot.sh` |
| cargo-about 0.9.3 | the `notices` job in `.github/workflows/release.yml` |
| Rust toolchain | `RUST_TOOLCHAIN` in `.github/workflows/release.yml`; keep it equal to `rust-toolchain.toml` |
| GitHub Actions | `uses:` lines in `.github/workflows/release.yml`, each a full commit SHA with a `# vX` comment |

Get a new sha256 from the project's published checksums where one exists, and
check it against your own download with `sha256sum`. For Debian packages, the
`Packages` index of the suite lists each file's sha256. `debian-sysroot.sh`
tries `deb.debian.org`, then `archive.debian.org`, then
`snapshot.debian.org`, and trusts only the sha256, because snapshot.debian.org
often answers HTTP 503.

The glibc floor (2.31) is the `.2.31` suffix on the `cargo zigbuild --target`
line in `release.yml` and the limit in `tools/release/smoke-linux.sh`. Raise
both together, and only with a decision record: it drops Debian 11 and
Ubuntu 20.04 users.

`about.toml`'s `accepted` list mirrors `deny.toml`'s `[licenses] allow`. When
one changes, change the other.

## Recommended repository settings

Checked-in files cannot enforce these. GitHub runs the `release.yml` stored in
the tagged commit, so the real control over who can publish is who can push a
`v*` tag.

- **A tag ruleset for `v*`** that limits tag creation, update and deletion to
  the repository owner.
- **Immutable releases**, so a published release's assets and tag cannot be
  changed afterwards.
