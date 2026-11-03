# Codex gate: auto-resolve stops as soon as the PR head moves

Recorded by COP-141 on 2026-11-03. Ports the loop half of
`HagaleTechnologies/propagation@f8e12437c06d9964de1f7f0011487cf537dc4e73`, with the two
hardenings pancetta merged for the same change (PR #553, PAN-254).

## Context

`codex-review-response` in `.github/workflows/wait-for-codex.yml` reads the PR head once, in
its `pr-context` step, and stores it as `HEAD_SHA`. The `head-check` step after it only acts on
`pull_request_review` events, which the workflow no longer subscribes to, so nothing re-reads
the head after `pr-context`.

On a clean round, the `compute-verdict` step then walks the resolvable baseline threads one at a
time. Each thread costs about five API calls: a compare, a live thread read, a reply that says
"Codex re-reviewed commit `$HEAD_SHA`", a second live read, and the resolve. Before COP-141, a
push that landed during that walk went unnoticed. The loop kept replying to and resolving
threads for a head that was gone. Threads are PR-global, so it could resolve a finding that the
new head reintroduced. Run against a mock `gh` with the head moved after the second of five
threads, the old step resolved all five and made six mutations after the move.

## Decision

1. **One head lookup per thread, at the top of the loop.** Before each thread, the loop reads
   `repos/$REPO/pulls/$PR` `.head.sha`. The check sits right after the empty-id skip, so an
   empty thread list makes no lookup. It also covers the window between `pr-context` and the
   first thread.
2. **`break` on a moved head.** When the live head differs from `HEAD_SHA`, the loop prints one
   `::notice::` line ("head advanced from H1 to H2 mid-loop -- stopping auto-resolve here") and
   stops. Every remaining thread is equally stale, so `continue` would only repeat the lookup.
3. **Fail open on a failed lookup, decided by `gh`'s exit status.** A failed lookup prints a
   `::warning::` line and the loop carries on with the captured head. The next iteration checks
   again; there is no retry. The lookup is
   `LIVE_HEAD_SHA=$(gh api ... --jq '.head.sha' 2>...) || LIVE_HEAD_SHA=""`. Upstream's
   `$(gh api ... || echo "")` form is wrong: on an HTTP error `gh api` still prints the JSON error
   body on stdout, even with `--jq`. Checked live: `gh api repos/HagaleTechnologies/coppa/pulls/99999999 --jq .head.sha`
   exits 1 and prints `{"message":"Not Found",...,"status":"404"}`. Upstream's form captures that
   body and reads it as a head that moved, so every API blip would leave a clean round's threads
   unresolved.
4. **The loop reads a here-string, not a pipe.** The thread list is captured into
   `resolvable_tsv` and fed with `done <<< "$resolvable_tsv"`. With the old
   `jq ... | while read` form, `break` closes the pipe's read end while `jq` may still have
   output queued past the pipe buffer. `jq` then dies of SIGPIPE, and `set -o pipefail` fails
   the step with exit 141 after `outcome=success` has already been written. The loop is the
   step's last command and the body has no `exit`, so running it in the main shell instead of a
   pipeline subshell changes nothing else.

## Limits

- **One thread can still resolve after a push.** A push that lands after a thread's own check
  and before its reply lets that one thread resolve against the old head. GitHub has no
  compare-and-set on review-thread mutations, so any check before a mutation is
  check-then-act.
- **A stop changes no verdict.** `outcome=success` is written before the loop, so it stands, and
  the `Update the check-run` step still concludes H1's check-run and dispatches `ci-full.yml`
  for H1, as before. That is correct for H1. The threads left unresolved stay unresolved and
  count as baseline in H2's own request and response round.

## Test

`tests/codex_gate_auto_resolve_head.rs` extracts the real `compute-verdict` `run:` body, runs it
under `bash --noprofile --norc -e` against a stateful mock `gh`, and checks five cases:

- the head moves after the second resolve (the ticket's scenario);
- the head moves before the first thread;
- the head never moves;
- every lookup fails with a JSON error body on stdout;
- 160 threads with long paths and a stop after the first.

The last two fail against upstream's lookup form and against the pipe form respectively.

The test belongs to the root `coppa` package, so `cargo test --workspace` and the `test-full`
job in `ci-full.yml` run it; `cargo test --workspace --lib` does not. It needs `bash` and `jq`.
Without them it skips with a printed reason, unless `CI` is set, in which case it fails.

This test is the only pre-merge evidence for a change to this step. `wait-for-codex.yml` runs
from the base branch under `pull_request_target` and `issue_comment`, so a pull request that
edits it never runs its own edit. To check a workflow file other than the working copy, set
`CODEX_GATE_WORKFLOW` to its path; the test's module comment shows how to run it against the
pre-COP-141 file, which must fail.

## Not ported, and rejected alternatives

- **The request-marker POST retry, the other hunk of `f8e1243`.** It fixes a different failure
  in the request job, and its context does not match coppa's (the pre-existing check-run
  condition has no title clause here). It needs its own ticket and test.
- **A second head check just before the reply mutation.** It would narrow the one-thread window
  above by one API call and still not close it, at the cost of another call per thread.
- **A retry on the in-loop lookup.** The next iteration already checks again.
