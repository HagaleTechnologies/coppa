# Native GitHub merge queue cutover (retiring Mergify)

coppa moved from Mergify's queue to GitHub's native `merge_queue` branch-ruleset rule,
following the same pattern already live on `HagaleTechnologies/widdershins`, `manta`,
`vanity`, `cqdx`, and `banco` (fleet CI redesign, wave 3).

## What changed

- `auto-merge-trigger.yml` (new): author-scoped (`thagale`, `catalyst-cloud-connector[bot]`,
  `dependabot[bot]`) `gh pr merge --auto --squash`, replacing Mergify's own
  `queue_rules`/`pull_request_rules`.
- `queue-retry-handler.yml` (new): one automatic retry per PR head SHA on a merge-group CI
  failure, then hands off to a human via a `needs-human` label.
- Ruleset split, not a single in-place PATCH: coppa's existing `Public - Coppa` ruleset (id
  `18627140`) protects `main` **and** `release/*`/`releases/*` via wildcard ref-name
  conditions, and GitHub rejects a `merge_queue` rule on any ruleset whose conditions include
  a wildcard ref. Rather than narrowing that ruleset's scope (which would have silently
  dropped release-branch protection), a new ruleset (`Coppa - Native Merge Queue`, id
  `23405672`) was created scoped to `~DEFAULT_BRANCH`/`refs/heads/main` only, carrying just
  the native `merge_queue` rule (SQUASH, group 1, concurrency 1, 60 min check-response
  timeout — matching the rest of the fleet). `Public - Coppa` itself is untouched; GitHub
  unions rules from every ruleset that matches a given ref, so `main` now enforces both
  rulesets' rules (six total: `deletion`, `merge_queue`, `non_fast_forward`, `pull_request`,
  `required_linear_history`, `required_status_checks`) while `release/*`/`releases/*` keep
  exactly their original five, unchanged.
- Landed via `HagaleTechnologies/coppa#108`, merged through the _existing_ Mergify queue
  (coppa's `.mergify.yml` still had `queue_rules` defined at that point) — the new ruleset
  was created immediately after, once #108's own admission workflows existed to use it.

## Three real correctness bugs fixed during #108's own review (not deferred, per this repo's
own convergence policy's "critical correctness bug" exception)

1. **Disarm-on-pause-label eviction**: a pending `labeled` run (needs-review/needs-human) can
   itself be evicted from the `admission-<PR>` concurrency group by a later event, since
   Actions retains only one pending run per group. The replacement run's live label check
   correctly declined to re-arm but never disarmed an already-armed/queued PR from before the
   label landed. Fixed: every branch that observes a live pause label now calls the disarm
   helper, not just the dedicated `labeled` branch.
2. **Missing needs-human handoff on a failed disarm**: `queue-retry-handler.yml`'s
   worst-case branch (retry-tracking label failed to attach AND the subsequent disarm/dequeue
   attempt also failed) exited before ever reaching `attach_needs_human_or_fail`, leaving no
   persistent handoff signal and risking an unbounded re-queue of the same revision. Fixed by
   moving the helper functions earlier so both branches can call it before exiting.
3. **Dependabot major-bump classification via an unreachable event**: `dependabot-auto-
   merge.yml` attaches `needs-review` to a semver-major bump using `secrets.GITHUB_TOKEN`.
   GitHub's anti-recursion rule means a `labeled` event authored by the default token never
   triggers any workflow run — including the very job meant to disarm on it. Fixed by
   classifying the update inline in `auto-merge-trigger.yml`'s own run (via
   `dependabot/fetch-metadata`), so the decision no longer depends on a cross-workflow event
   that structurally cannot reach back.

## This PR: live-fire verification

This PR is the first real merge-group completion against the native `merge_queue` rule and
against `queue-retry-handler.yml`'s fixed workflow_run trigger. `.mergify.yml` is deleted in
a follow-up once this is confirmed to merge cleanly through the native path.
