//! COP-141: the Codex gate's auto-resolve loop must stop as soon as the PR head
//! moves.
//!
//! `codex-review-response` in `.github/workflows/wait-for-codex.yml` reads the PR
//! head once, in its `pr-context` step. The `compute-verdict` step then replies to
//! and resolves review threads one at a time, about five GitHub API calls each. If
//! a push lands during that loop, every later reply names a commit that is no
//! longer the head. The loop now re-reads the live head before each thread and
//! stops when it has moved (ported from propagation f8e1243). The rule and its
//! limits are in `docs/DECISIONS/2026-11-03-codex-gate-auto-resolve-head-check.md`.
//!
//! The workflow runs from the base branch under `pull_request_target` and
//! `issue_comment`, so a PR that edits it never exercises its own change. This
//! test is the only pre-merge evidence. It extracts the real `compute-verdict`
//! `run:` body, substitutes the two `${{ ... }}` baseline expressions, and runs it
//! under `bash --noprofile --norc -e` against a stateful mock `gh` placed first on
//! `PATH`, with a cleared environment. GitHub runs a `run:` step with no `shell:`
//! as `bash -e {0}`. A non-interactive shell reads no profile or rc file either
//! way, so the two extra flags only make that explicit. The mock moves the head
//! from H1 to H2 on the trigger each case names.
//!
//! The step writes its error logs to fixed `/tmp/*-err.log` paths. The harness
//! points those at the case's own scratch directory, so concurrent runs from
//! other checkouts or users cannot collide on them.
//!
//! The `run:` body is extracted by hand rather than with a YAML crate, for the
//! reason `tests/serena_project_config.rs` gives: a tooling guard test is not
//! worth a new crate in a dependency graph guarded by cargo-deny, a RustSec audit,
//! an MSRV job and Dependabot.
//!
//! Set `CODEX_GATE_WORKFLOW` to another workflow file to run these cases against
//! it. The pre-COP-141 file must fail them:
//!
//! ```text
//! git show 2526b50:.github/workflows/wait-for-codex.yml > /tmp/base.yml
//! CODEX_GATE_WORKFLOW=/tmp/base.yml cargo test -p coppa --test codex_gate_auto_resolve_head
//! ```
//!
//! The cases need `bash` and `jq`. Without them the cases print a skip reason
//! and pass, unless `CI` is set, in which case they fail.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Runs one case at a time. Each case writes an executable mock and then spawns
/// `bash`. Serialising keeps a spawn in another test thread from inheriting the
/// mock's open write descriptor, which would make its exec fail with ETXTBSY.
static SERIAL: Mutex<()> = Mutex::new(());

/// The two step expressions that `compute-verdict` reads from the `baseline`
/// step. The harness replaces both with the case's thread ids.
const BASELINE_EXPRESSIONS: [&str; 2] = [
    "'${{ steps.baseline.outputs.baseline_pure }}'",
    "'${{ steps.baseline.outputs.baseline_initiated }}'",
];

/// Stateful fake `gh`. State lives in files under `$MOCK_STATE`:
///
/// - `head`, `head_start`, `head_after`, `finding`, `ids`, `paths`: seeded by the test.
/// - `calls`: every invocation's argv, one line each, cut to 200 characters.
/// - `mutations`: one line per reply or resolve, ending in the first character
///   of the head at that moment (`headnow=a` for H1, `headnow=b` for H2).
/// - `replied.<id>`, `resolved.<id>`, `lookups`, `resolves`: progress.
/// - `unexpected`: any call the mock does not model. The step sends some
///   stderr to `/dev/null`, so the test reads this file rather than stderr.
///
/// Controls: `FLIP_ON_LABEL_POST=1` moves the head on the label POST, before the
/// first thread. `FLIP_AFTER_RESOLVES=<n>` moves it after the n-th resolve.
/// `LOOKUP_MODE=fail_with_body` makes every head lookup behave like real `gh api`
/// on an HTTP error: the JSON error body on stdout, a message on stderr, exit 1.
const MOCK_GH: &str = r##"#!/usr/bin/env bash
set -u
S="$MOCK_STATE"
line=$(printf '%s' "$*" | tr '\n' ' ' | cut -c1-200)
printf '%s\n' "$line" >> "$S/calls"
unexpected() { printf '%s\n' "$1" >> "$S/unexpected"; echo "UNEXPECTED gh call: $1" >&2; exit 99; }
inc() { local n; n=$(cat "$S/$1" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$S/$1"; echo "$n"; }
flip() { cp "$S/head_after" "$S/head"; }
arg_val() { local k="$1"; shift; for a in "$@"; do case "$a" in "$k="*) printf '%s' "${a#"$k="}"; return;; esac; done; }
[ "$#" -ge 2 ] || unexpected "$line"
if [ "$1" = label ] && [ "$2" = create ]; then exit 0; fi
[ "$1" = api ] || unexpected "$line"
case "$*" in
  *"--paginate repos/test/repo/issues/17/comments"*)
    printf '[{"body":"<!-- codex-review-request head_sha=%s -->","user":{"login":"trusted"}}]\n' "$(cat "$S/head_start")"
    exit 0;;
esac
if [ "$2" = "repos/test/repo/pulls/17" ]; then
  inc lookups >/dev/null
  case "${LOOKUP_MODE:-ok}" in
    fail_with_body)
      echo 'gh: Bad Gateway (HTTP 502)' >&2
      echo '{"message":"Bad Gateway","status":"502"}'
      exit 1;;
    *) cat "$S/head"; exit 0;;
  esac
fi
if [ "$2" = "-X" ] && [ "$#" -ge 4 ] && [ "$4" = "repos/test/repo/issues/17/labels" ]; then
  [ "${FLIP_ON_LABEL_POST:-}" = 1 ] && flip
  exit 0
fi
case "$2" in
  repos/test/repo/compare/*)
    fsha="${2#repos/test/repo/compare/}"; fsha="${fsha%%...*}"
    jq -n --arg f "$fsha" --rawfile p "$S/paths" \
      '{merge_base_commit: {sha: $f}, files: [$p | split("\n")[] | select(length > 0) | {filename: .}]}'
    exit 0;;
esac
if [ "$2" = graphql ]; then
  q=$(arg_val query "$@"); id=$(arg_val id "$@")
  case "$q" in
    *"reviewThreads(first: 100"*)
      jq -n --rawfile ids "$S/ids" --rawfile p "$S/paths" --arg f "$(cat "$S/finding")" '
        [($ids | split("\n") | map(select(length > 0))), ($p | split("\n") | map(select(length > 0)))]
        | transpose
        | map({id: .[0], isResolved: false, path: .[1],
               comments: {nodes: [{author: {login: "chatgpt-codex-connector"},
                                   createdAt: "2026-11-01T10:00:00Z", originalCommit: {oid: $f}}]}})
        | {data: {repository: {pullRequest: {reviewThreads: {pageInfo: {hasNextPage: false, endCursor: "x"}, nodes: .}}}}}'
      exit 0;;
    *addPullRequestReviewThreadReply*)
      touch "$S/replied.$id"
      echo "reply $id $(arg_val body "$@") headnow=$(cut -c1 "$S/head")" >> "$S/mutations"
      echo '{"data":{}}'
      exit 0;;
    *resolveReviewThread*)
      [ "$GH_TOKEN" = resolve-token ] || unexpected "resolve without RESOLVE_TOKEN: $line"
      touch "$S/resolved.$id"
      echo "resolve $id headnow=$(cut -c1 "$S/head")" >> "$S/mutations"
      n=$(inc resolves)
      [ "$n" = "${FLIP_AFTER_RESOLVES:-none}" ] && flip
      echo '{"data":{}}'
      exit 0;;
    *"node(id:"*)
      r=false; [ -e "$S/resolved.$id" ] && r=true
      extra=
      [ -e "$S/replied.$id" ] && extra=',{"author":{"login":"github-actions"},"createdAt":"2026-11-01T11:30:00Z"}'
      printf '{"data":{"node":{"isResolved":%s,"comments":{"nodes":[{"author":{"login":"chatgpt-codex-connector"},"createdAt":"2026-11-01T10:00:00Z"}%s]}}}}\n' "$r" "$extra"
      exit 0;;
  esac
fi
unexpected "$line"
"##;

fn h1() -> String {
    "a".repeat(40)
}

fn h2() -> String {
    "b".repeat(40)
}

fn finding_sha() -> String {
    "c".repeat(40)
}

/// The root `coppa` package's manifest dir is the repo root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflow_path() -> PathBuf {
    match std::env::var_os("CODEX_GATE_WORKFLOW") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => repo_root().join(".github/workflows/wait-for-codex.yml"),
    }
}

fn read_workflow() -> String {
    let path = workflow_path();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Returns the `run: |` block of the step whose id is `compute-verdict`, as the
/// YAML loader would: dedented by the first content line's indent, trailing
/// blank lines dropped, ending in one newline.
fn compute_verdict_body(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let ids: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim() == "id: compute-verdict")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "expected exactly one `id: compute-verdict` line, found {}",
        ids.len()
    );
    let id_at = ids[0];
    let id_indent = indent_of(lines[id_at]);
    let run_at = lines[id_at + 1..]
        .iter()
        .position(|l| l.trim() == "run: |")
        .map(|i| id_at + 1 + i)
        .expect("no `run: |` after `id: compute-verdict`");
    // The `run:` key must belong to the same step: no shallower line between them.
    for line in &lines[id_at + 1..run_at] {
        assert!(
            line.trim().is_empty() || indent_of(line) >= id_indent,
            "`run: |` found after `id: compute-verdict` belongs to a later step (crossed {line:?})"
        );
    }
    let key_indent = indent_of(lines[run_at]);
    let block: Vec<&str> = lines[run_at + 1..]
        .iter()
        .copied()
        .take_while(|l| l.trim().is_empty() || indent_of(l) > key_indent)
        .collect();
    let content_indent = block
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .expect("`compute-verdict` has an empty `run:` body");
    let mut out: Vec<&str> = block
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                ""
            } else {
                assert!(
                    indent_of(l) >= content_indent,
                    "line indented less than the block's first line: {l:?}"
                );
                &l[content_indent..]
            }
        })
        .collect();
    while out.last() == Some(&"") {
        out.pop();
    }
    let mut body = out.join("\n");
    body.push('\n');
    body
}

/// The step body with both baseline expressions replaced by `ids` as a JSON array.
fn runnable_body(src: &str, ids: &[String]) -> String {
    let json = format!(
        "[{}]",
        ids.iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut body = compute_verdict_body(src);
    for expr in BASELINE_EXPRESSIONS {
        assert!(
            body.contains(expr),
            "`compute-verdict` no longer reads {expr}"
        );
        body = body.replace(expr, &format!("'{json}'"));
    }
    assert!(
        !body.contains("${{"),
        "`compute-verdict` has a `${{{{ ... }}}}` expression this harness does not substitute"
    );
    body
}

/// Points the step's fixed `/tmp/` error-log paths at `dir`. The path is spliced
/// into unquoted shell redirections, so it is used only when every character is
/// shell-safe. Otherwise the step keeps `/tmp/`, where concurrent runs by the
/// same user can mix message text but not control flow.
fn redirect_tmp_logs(body: &str, dir: &Path) -> String {
    let safe = dir.to_str().filter(|d| {
        d.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    });
    match safe {
        Some(d) => body.replace("/tmp/", &format!("{d}/")),
        None => body.to_string(),
    }
}

/// Returns true when `bash` and `jq` both run. Panics instead of skipping when
/// `CI` is set, so CI cannot pass by skipping.
fn tools_available() -> bool {
    let missing: Vec<&str> = ["bash", "jq"]
        .into_iter()
        .filter(|tool| {
            !Command::new(tool)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .collect();
    if missing.is_empty() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "{missing:?} not runnable, and CI is set: install them rather than skip this test"
    );
    eprintln!("skipping: {missing:?} not runnable; install them to run this test");
    false
}

/// A per-process, per-case scratch directory, removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(case: &str) -> Self {
        let path = std::env::temp_dir().join(format!("coppa-cop141-{}-{case}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("failed to create {}: {e}", path.display()));
        Scratch { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Default)]
struct Case {
    name: &'static str,
    threads: usize,
    path_len: usize,
    flip_on_label_post: bool,
    flip_after_resolves: Option<usize>,
    lookup_fails_with_body: bool,
}

struct Run {
    exit: Option<i32>,
    output: String,
    calls: Vec<String>,
    mutations: Vec<String>,
    resolved: BTreeSet<String>,
    lookups: usize,
    github_output: String,
    unexpected: Option<String>,
}

impl Run {
    fn lines_with(&self, needle: &str) -> Vec<&str> {
        self.output.lines().filter(|l| l.contains(needle)).collect()
    }

    fn lookup_calls(&self) -> usize {
        self.calls.iter().filter(|c| is_lookup(c)).count()
    }

    fn compare_calls(&self) -> usize {
        self.calls.iter().filter(|c| is_compare(c)).count()
    }

    /// Exit 0, nothing the mock does not model, and `outcome=success` written.
    fn assert_clean(&self) {
        assert!(
            self.unexpected.is_none(),
            "the step made calls the mock does not model:\n{}\n--- step output ---\n{}",
            self.unexpected.as_deref().unwrap_or_default(),
            self.output
        );
        assert_eq!(
            self.exit,
            Some(0),
            "compute-verdict did not exit 0\n--- step output ---\n{}",
            self.output
        );
        assert!(
            self.github_output.lines().any(|l| l == "outcome=success"),
            "GITHUB_OUTPUT lacks outcome=success: {:?}",
            self.github_output
        );
    }
}

fn is_lookup(call: &str) -> bool {
    call == "api repos/test/repo/pulls/17" || call.starts_with("api repos/test/repo/pulls/17 ")
}

fn is_compare(call: &str) -> bool {
    call.starts_with("api repos/test/repo/compare/")
}

fn thread_ids(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("T{i}")).collect()
}

fn set_of(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| (*s).to_string()).collect()
}

fn write(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

fn read_or_empty(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Runs the real `compute-verdict` body for `case`. `None` means the tools are
/// missing outside CI and the case is skipped.
fn run_case(case: &Case) -> Option<Run> {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !tools_available() {
        return None;
    }

    let scratch = Scratch::new(case.name);
    let bin = scratch.path.join("bin");
    let state = scratch.path.join("state");
    let logs = scratch.path.join("logs");
    for dir in [&bin, &state, &logs] {
        fs::create_dir_all(dir)
            .unwrap_or_else(|e| panic!("failed to create {}: {e}", dir.display()));
    }

    let ids = thread_ids(case.threads);
    let body = redirect_tmp_logs(&runnable_body(&read_workflow(), &ids), &logs);
    let gh = bin.join("gh");
    write(&gh, MOCK_GH);
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("failed to chmod {}: {e}", gh.display()));

    let paths: Vec<String> = (0..case.threads)
        .map(|n| format!("src/f{n}{}.rs", "x".repeat(case.path_len)))
        .collect();
    write(&state.join("head"), &format!("{}\n", h1()));
    write(&state.join("head_start"), &h1());
    write(&state.join("head_after"), &format!("{}\n", h2()));
    write(&state.join("finding"), &finding_sha());
    write(&state.join("ids"), &format!("{}\n", ids.join("\n")));
    write(&state.join("paths"), &format!("{}\n", paths.join("\n")));

    let script = scratch.path.join("compute-verdict.sh");
    write(&script, &body);
    let github_output = scratch.path.join("github_output");
    write(&github_output, "");

    let mut path_env = bin.clone().into_os_string();
    if let Some(inherited) = std::env::var_os("PATH") {
        path_env.push(":");
        path_env.push(inherited);
    }

    let mut cmd = Command::new("bash");
    cmd.args(["--noprofile", "--norc", "-e"])
        .arg(&script)
        .env_clear()
        .env("PATH", path_env)
        .env("MOCK_STATE", &state)
        .env("GH_TOKEN", "github-token")
        .env("RESOLVE_TOKEN", "resolve-token")
        .env("REPO", "test/repo")
        .env("PR", "17")
        .env("HEAD_SHA", h1())
        .env("RUN_STARTED", "2026-11-01T11:00:00Z")
        .env("IS_MARKED", "true")
        .env("GITHUB_OUTPUT", &github_output);
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    if case.flip_on_label_post {
        cmd.env("FLIP_ON_LABEL_POST", "1");
    }
    if let Some(n) = case.flip_after_resolves {
        cmd.env("FLIP_AFTER_RESOLVES", n.to_string());
    }
    if case.lookup_fails_with_body {
        cmd.env("LOOKUP_MODE", "fail_with_body");
    }
    let out = cmd.output().expect("failed to spawn bash");

    let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
    output.push_str(&String::from_utf8_lossy(&out.stderr));

    let resolved = fs::read_dir(&state)
        .unwrap_or_else(|e| panic!("failed to list {}: {e}", state.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_prefix("resolved.").map(str::to_string)
        })
        .collect();
    let lookups = read_or_empty(&state.join("lookups"))
        .trim()
        .parse()
        .unwrap_or(0);

    Some(Run {
        exit: out.status.code(),
        output,
        calls: read_or_empty(&state.join("calls"))
            .lines()
            .map(str::to_string)
            .collect(),
        mutations: read_or_empty(&state.join("mutations"))
            .lines()
            .map(str::to_string)
            .collect(),
        resolved,
        lookups,
        github_output: read_or_empty(&github_output),
        unexpected: fs::read_to_string(state.join("unexpected")).ok(),
    })
}

#[test]
fn extracts_the_compute_verdict_step_body() {
    let body = compute_verdict_body(&read_workflow());
    assert!(
        body.starts_with("set -o pipefail\n"),
        "body does not start with the step's first line: {:?}",
        &body[..body.len().min(80)]
    );
    assert!(
        body.contains("resolveReviewThread"),
        "body lacks the resolve mutation"
    );
    assert!(
        !body.contains("name: Update the check-run"),
        "extraction ran into the next step"
    );
}

/// The ticket's scenario: auto-resolving for H1, the head moves to H2 after the
/// second thread is resolved, and the loop stops before the third.
#[test]
fn a_head_moves_after_second_resolve_stops_before_the_next_thread() {
    let Some(run) = run_case(&Case {
        name: "a",
        threads: 5,
        flip_after_resolves: Some(2),
        ..Case::default()
    }) else {
        return;
    };
    run.assert_clean();
    assert_eq!(
        run.resolved,
        set_of(&["T1", "T2"]),
        "only the threads before the head moved may resolve\n{}",
        run.output
    );
    let after_move: Vec<&String> = run
        .mutations
        .iter()
        .filter(|m| m.ends_with("headnow=b"))
        .collect();
    assert!(
        after_move.is_empty(),
        "mutations made after the head moved to H2: {after_move:?}"
    );
    let notices = run.lines_with("::notice::");
    assert_eq!(notices.len(), 1, "expected one notice: {notices:?}");
    assert!(
        notices[0].contains("mid-loop") && notices[0].contains(&h2()),
        "the notice should say the head moved mid-loop to H2: {}",
        notices[0]
    );
}

/// The head moves before the first thread (on the label POST, after
/// `pr-context` read it): nothing resolves and no thread work starts.
#[test]
fn b_head_moves_before_the_first_thread_resolves_nothing() {
    let Some(run) = run_case(&Case {
        name: "b",
        threads: 5,
        flip_on_label_post: true,
        ..Case::default()
    }) else {
        return;
    };
    run.assert_clean();
    assert!(
        run.resolved.is_empty(),
        "resolved {:?} after the head moved\n{}",
        run.resolved,
        run.output
    );
    assert_eq!(
        run.compare_calls(),
        0,
        "thread work started after the head moved: {:?}",
        run.calls
    );
    assert!(
        run.mutations.is_empty(),
        "mutations after the head moved: {:?}",
        run.mutations
    );
    assert_eq!(run.lookups, 1, "expected one head lookup");
    assert_eq!(run.lines_with("::notice::").len(), 1);
}

/// Control: the head never moves, so every thread resolves, each after its own
/// head lookup.
#[test]
fn c_head_never_moves_resolves_every_thread_after_a_lookup_each() {
    let Some(run) = run_case(&Case {
        name: "c",
        threads: 5,
        ..Case::default()
    }) else {
        return;
    };
    run.assert_clean();
    assert_eq!(
        run.resolved,
        set_of(&["T1", "T2", "T3", "T4", "T5"]),
        "{}",
        run.output
    );
    let replies: Vec<&String> = run
        .mutations
        .iter()
        .filter(|m| m.starts_with("reply "))
        .collect();
    assert_eq!(replies.len(), 5, "expected 5 replies: {replies:?}");
    let names_h1 = format!("re-reviewed commit {} ", h1());
    for reply in &replies {
        assert!(reply.contains(&names_h1), "reply does not name H1: {reply}");
    }
    assert_eq!(
        run.lookup_calls(),
        5,
        "expected one head lookup per thread: {:?}",
        run.calls
    );
    assert_eq!(run.lookups, 5);
    assert_eq!(run.compare_calls(), 5);
    for (i, call) in run.calls.iter().enumerate() {
        if is_compare(call) {
            assert!(
                i > 0 && is_lookup(&run.calls[i - 1]),
                "compare call {i} does not directly follow a head lookup: {:?}",
                run.calls
            );
        }
    }
    assert!(run.lines_with("::notice::").is_empty());
    assert!(run.lines_with("mid-loop").is_empty());
}

/// Every head lookup fails the way real `gh api` does on an HTTP error, with
/// the JSON error body on stdout. That is a failed lookup, not a moved head:
/// the loop warns and carries on with the captured head.
#[test]
fn d_failed_lookup_with_error_body_fails_open() {
    let Some(run) = run_case(&Case {
        name: "d",
        threads: 5,
        lookup_fails_with_body: true,
        ..Case::default()
    }) else {
        return;
    };
    run.assert_clean();
    assert_eq!(
        run.resolved,
        set_of(&["T1", "T2", "T3", "T4", "T5"]),
        "a failed lookup must not stop the loop\n{}",
        run.output
    );
    let warnings: Vec<&str> = run
        .lines_with("::warning::")
        .into_iter()
        .filter(|l| l.contains("mid-loop"))
        .collect();
    assert_eq!(
        warnings.len(),
        5,
        "expected one mid-loop warning per lookup: {warnings:?}"
    );
    assert!(
        run.lines_with("::notice::").is_empty(),
        "a failed lookup was read as a moved head: {:?}",
        run.lines_with("::notice::")
    );
}

/// A thread list larger than the pipe buffer, and a stop after the first
/// thread. Feeding the loop from a pipe would SIGPIPE `jq` on `break`, and
/// `pipefail` would fail the step with exit 141 after `outcome=success`.
#[test]
fn e_stop_with_a_large_thread_list_exits_zero() {
    let Some(run) = run_case(&Case {
        name: "e",
        threads: 160,
        path_len: 400,
        flip_after_resolves: Some(1),
        ..Case::default()
    }) else {
        return;
    };
    run.assert_clean();
    // Compared with `assert!`, not `assert_eq!`, so a failure prints a count
    // rather than up to 160 ids.
    assert!(
        run.resolved == set_of(&["T1"]),
        "expected only T1 to resolve, got {} threads",
        run.resolved.len()
    );
    assert_eq!(run.lines_with("::notice::").len(), 1);
}
