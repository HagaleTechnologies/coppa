# Docs architecture: short CLAUDE.md brief, LIMITATIONS.md, local-only notes, humans-first wiki

Decided by Tony Hagale on 2026-10-14 (owner decisions batch), recorded here by COP-77.
Answers `docs/reviews/2026-09-05-product-ux-market-review.md` §5 items 8-9 (H-114): where
the agent's brief ends and the limitations record lives, what happens to the `.superpowers/`
notes, and who `wiki/` is for.

## Decisions

1. **CLAUDE.md is the agent brief, at most 100 lines** (the fleet rule). The measured gaps,
   open bugs and unmet acceptance bars moved to `docs/LIMITATIONS.md`. `AGENTS.md` stays a
   symlink to `CLAUDE.md`.
2. **`.superpowers/` and `docs/superpowers/` are local working notes, never committed.** Both
   paths are gitignored, and tracked files never cite them — cite the ADR, the BENCHMARKS.md
   section or the PR instead. The one report that had been committed now lives in
   `docs/analysis/2026-08-01-vhf-timing-backoff-fix-report.md`.
3. **The wiki is for humans first and stays machine-readable.** It keeps its frontmatter and
   `manifest.json`, linted by sunto's `wiki-lint` in `ci-full.yml`, stays at `wiki/` with
   `visibility = "public"`, unrendered, and is linked from the README.

## Rejected alternatives

- Committing the missing `.superpowers/`/`docs/superpowers/` notes (owner: "don't commit
  those docs").
- Moving the wiki to `docs/notes/` or rendering it with mdBook/GitHub Pages; sunto's layout
  keeps it at `wiki/`, and GitHub already renders its Markdown for humans.
- Marking the wiki agent-only; sunto's schema allows only `public | private`, and the owner
  called it humans-first.
