---
id: agent-tooling
title: What will bite you about this repo's agent tooling (Serena and humanlayer thoughts)?
kind: gotcha
status: current
maintainer: agent
sources:
  - .serena/project.yml
  - tests/serena_project_config.rs
  - .catalyst/config.json
verified:
  commit: 7d1590d
  date: 2026-09-06
links:
---
COP-11 filed two unrelated developer-tooling defects together: catalyst-dev's
subagents had no indexed Serena project to activate in this repo, and
`humanlayer thoughts status`/`sync` crashed instead of reporting real status.
They're documented on one page because both are "what you need to know about
this repo's agent tooling," not because they share a mechanism.

## Serena

`.serena/project.yml` gives the catalyst-dev `codebase-analyzer`,
`codebase-locator`, and `codebase-pattern-finder` subagents an indexed project
to activate against, so their `mcp__serena__*` symbol/reference lookups
(`find_symbol`, `find_referencing_symbols`, `get_symbols_overview`) return real
rust-analyzer-backed results instead of silently falling back to plain
grep/glob. Without this file, `activate_project` has nothing to activate and
that fallback is invisible — nothing tells the caller it happened.

**What's committed vs. machine-local.** `.serena/project.yml` and
`.serena/memories/*.md` are committed; `.serena/cache/` (Serena's own index)
and an optional per-developer `.serena/project.local.yml` override are
gitignored via `.serena/.gitignore` — the same split `catalyst-cloud` uses.
`tests/serena_project_config.rs` guards the committed config's shape (exists,
parses, names this repo, declares `rust`, stays `read_only`, ignores the right
noise, shadows no workspace member, ships `codebase_map`) under
`cargo test --workspace`, so a silent-breaking rename or tidy-up fails a real
test instead.

**Re-indexing** (optional — Serena also indexes on demand at activation):
`uvx --from git+https://github.com/oraios/serena serena project index .` from
the repo root (or `index-project` on older Serena versions). Writes into the
gitignored `.serena/cache/` — confirm `git status --porcelain` stays clean
afterward.

**Traps:**

- **Install ALSA headers first on Linux** (`sudo apt-get install -y
  libasound2-dev`) — rust-analyzer runs `cargo metadata`/`cargo check` over
  the workspace, and `coppa-audio`'s `cpal` dependency needs those headers, or
  the backend comes up partially broken in a way that reads as "Serena found
  nothing" rather than as a build error. CI installs them before every cargo
  job for exactly this reason.
- **First activation is slow**: rust-analyzer resolves and checks a
  14-member workspace whose graph includes tokio, cpal, rustfft, and
  criterion. One-time per-machine cost, amortized into `.serena/cache/` and
  `target/`.
- **`language_servers`, not the legacy `languages`.** The old key makes
  `ProjectConfig._load_yaml_dict` treat `project.yml` as incomplete and
  unconditionally re-save it, stripping every comment
  (`serena_config.py`'s `RENAMED_FIELDS`, verified against 1.7.0) — guarded by
  `serena_project_config_has_every_field_needed_to_avoid_a_rewrite`.
- **No `rust-analyzer` on `PATH`?** Serena's `DependencyProvider` tries
  `rustup which` → `rustup component add` → `shutil.which`; `rust-toolchain.toml`
  doesn't list `rust-analyzer`, so it falls to auto-install, which needs
  network access and can fail in a sandboxed container — drop a standalone
  release binary earlier on `PATH` as a workaround.
- `read_only: true` is deliberate — none of the three consuming subagents
  holds a Serena write/edit tool, so this costs nothing and removes any path
  by which an agent could mutate source outside the normal Edit-tool review
  flow.
- `testdata/golden` is the one `ignored_paths` entry that's genuinely
  load-bearing rather than defensive: `.gitignore` explicitly *un*-ignores
  those ~1.5 MB of frozen WAV decode-regression vectors
  (`!testdata/golden/*.wav`), so `ignore_all_files_in_gitignore` misses them.
- `fuzz` is excluded — it's `[workspace] exclude`d in `Cargo.toml` and needs a
  nightly toolchain, so a stable rust-analyzer would error on it.
  `search_for_pattern`/`Grep` still reach the fuzz targets directly.

## humanlayer thoughts

`humanlayer thoughts status` (and `humanlayer thoughts sync`, which fails the
same way before it ever touches git) can crash with:

```
Error checking thoughts status: TypeError: Cannot read properties of undefined (reading 'startsWith')
```

**This is not a bug in this repository, and no file in this repo can fix
it.** It's a genuine defect in the installed `humanlayer`/`hlyr` npm package
(confirmed against `0.17.2-npm`'s own bundled source,
`dist/index.js`): `resolveProfileForRepo()` has four return branches, and
three fall back to the top-level `config.thoughtsRepo`/`reposDir`/`globalDir`
when a value is missing — the named-profile branch (`:669-679`) does not, so a
`thoughts.profiles.<name>` entry that sets `reposDir`/`globalDir` but not its
own `thoughtsRepo` yields `thoughtsRepo: undefined`. `expandPath()`
(`:583-588`) then calls `filePath.startsWith("~/")` with no guard and throws.

`coppa`'s own `.catalyst/config.json` sets `thoughts.profile:
"HagaleTechnologies"` — an org-wide profile name, not a per-repo one — which
is exactly the mapping shape that walks into the buggy branch. The same crash
was observed across 8 unrelated repos in a 2026-09-05 fleet sweep, consistent
with one shared, underspecified `profiles.HagaleTechnologies` entry in
whatever bootstraps each developer's `~/.humanlayer/config.json`, rather than
8 independent misconfigurations.

**Workaround (per affected machine).** In `~/.humanlayer/config.json`, add the
missing `thoughtsRepo` to the named profile, pointing at whatever path the
top-level `thoughtsRepo` already uses (don't invent a new one):
`thoughts.profiles.HagaleTechnologies.thoughtsRepo = thoughts.thoughtsRepo`.
Verify with `humanlayer thoughts status` from a `coppa` checkout; it should
print a real sync status. (`Thoughts not configured. Run "humanlayer thoughts
init" first.` is the clean, intended branch for a machine with no thoughts
config at all — a different situation, and `init` is the right response
there.)

**Upstream issue.** Not yet filed as of this writing — this container has no
credential to file one. See COP-11 for the drafted issue text (title,
symptom, cause with exact `dist/index.js` line numbers, repro steps, and a
suggested two-part fix: add the missing `?? config.*` fallback, and validate
at the config-resolution boundary so a resolved non-string `thoughtsRepo`
fails loudly instead of reaching `expandPath`). Once filed, update this
section with the issue URL. Once a machine's config is repaired per the
workaround above, re-run `humanlayer thoughts sync` there to flush any local
`coppa` `shared/pm/` writes that never reached the central pool.
