# What will bite you about this repo's agent tooling (Serena and humanlayer thoughts)?

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
`.serena/memories/*.md` are committed. `.serena/cache/` (Serena's own index)
and an optional per-developer `.serena/project.local.yml` override are
gitignored via `.serena/.gitignore` — the same split the `catalyst-cloud`
reference config uses. `tests/serena_project_config.rs` guards the committed
config's shape (exists, parses, names this repo, declares `rust`, stays
`read_only`, ignores the right noise, shadows no workspace member, ships the
`codebase_map` memory) under `cargo test --workspace`, so a rename or tidy-up
that would silently break activation fails a real test instead.

**Re-indexing.** From the repo root:

```sh
uvx --from git+https://github.com/oraios/serena serena project index .
```

(or the equivalent `index-project` entrypoint if your installed Serena
predates the `serena project` command group). This writes into
`.serena/cache/`, which is gitignored — confirm `git status --porcelain` stays
clean afterward. The index pass is an optimization, not a prerequisite:
Serena indexes on demand at activation too.

**Traps:**

- **Install ALSA headers first on Linux** (`sudo apt-get install -y
  libasound2-dev`). Serena's Rust backend is rust-analyzer, which runs `cargo
  metadata`/`cargo check` over the workspace; `coppa-audio`'s `cpal`
  dependency needs those headers, and without them the backend can come up
  partially broken in a way that reads as "Serena found nothing" rather than
  as a build error. CI installs them before every cargo job for exactly this
  reason.
- **First activation is slow.** rust-analyzer has to resolve and check a
  14-member workspace whose graph includes tokio, cpal, rustfft, and
  criterion. One-time per-machine cost, amortized into `.serena/cache/` and
  `target/`.
- `read_only: true` is deliberate: none of the three consuming subagents holds
  a Serena write/edit tool, so this costs nothing and removes any path by
  which an agent could mutate source outside the normal Edit-tool review flow.
- `testdata/golden` is the one `ignored_paths` entry that's genuinely
  load-bearing rather than defensive: `.gitignore` explicitly *un*-ignores
  those ~1.5 MB of frozen WAV decode-regression vectors
  (`!testdata/golden/*.wav`), so `ignore_all_files_in_gitignore` does not
  cover them. They hold no symbols and no prose.
- `fuzz` is excluded because it's `[workspace] exclude`d in `Cargo.toml` and
  needs a nightly toolchain — pointing a stable rust-analyzer at it invites
  backend errors. `search_for_pattern` won't reach the fuzz targets as a
  result, but `Grep` still will.

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

**Workaround (per affected machine).** Give the named profile its own
explicit `thoughtsRepo` in `~/.humanlayer/config.json`, pointing at the same
central thoughts checkout the top-level config already uses:

```jsonc
{
  "thoughts": {
    "thoughtsRepo": "~/thoughts",          // top-level default, already present
    "profiles": {
      "HagaleTechnologies": {
        "thoughtsRepo": "~/thoughts",      // <-- add this: the missing field
        "reposDir": "repos",
        "globalDir": "global"
      }
    }
  }
}
```

Use whatever path that machine's top-level `thoughtsRepo` already names —
don't invent a new one. Verify with `humanlayer thoughts status` from a
`coppa` checkout; it should print a real sync status. (If it instead prints
`Thoughts not configured. Run "humanlayer thoughts init" first.`, that's the
clean, intended "not configured" branch — a different situation, meaning this
machine has no thoughts config at all, and `humanlayer thoughts init` is the
right response there.)

**Upstream issue.** Not yet filed as of this writing — this container has no
credential to file one. See [[COP-11]] for the drafted issue text (title,
symptom, cause with exact `dist/index.js` line numbers, repro steps, and a
suggested two-part fix: add the missing `?? config.*` fallback, and validate
at the config-resolution boundary so a resolved non-string `thoughtsRepo`
fails loudly instead of reaching `expandPath`). Once filed, update this
section with the issue URL.

**Pending `shared/pm/` content.** If a developer machine has local `coppa`
thoughts writes that never made it to the central pool because `sync`
couldn't be trusted, re-run `humanlayer thoughts sync` from that machine once
its local config is repaired per the workaround above.
