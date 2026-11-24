//! Executable contracts for the full-CI runner and its failure reporting.
#![cfg(unix)]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const WORKFLOW: &str = include_str!("../.github/workflows/ci-full.yml");
const GROUPS: [&str; 4] = ["workspace", "daemon-unit", "daemon-integration", "no-audio"];
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "cop200-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn executable(&self, name: &str, body: &str) {
        let p = self.0.join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn runner(s: &Scratch, group: &str, fail: usize) -> std::process::Output {
    Command::new("bash")
        .arg("tools/ci/run-full-tests.sh")
        .arg(group)
        .env(
            "PATH",
            format!("{}:{}", s.0.display(), std::env::var("PATH").unwrap()),
        )
        .env("MOCK_STATE", &s.0)
        .env("FAIL_AT", fail.to_string())
        .output()
        .unwrap()
}
fn mock_cargo(s: &Scratch) {
    s.executable(
        "cargo",
        r#"#!/usr/bin/env bash
set -eu
n=0
[ ! -f "$MOCK_STATE/count" ] || n=$(cat "$MOCK_STATE/count")
n=$((n+1))
echo "$n" > "$MOCK_STATE/count"
printf '%s\n' "$*" >> "$MOCK_STATE/args"
[ "$n" != "$FAIL_AT" ] || exit 37
"#,
    );
}
fn profile() -> String {
    fs::read_to_string("Cargo.toml")
        .unwrap()
        .split("[profile.ci]")
        .nth(1)
        .expect("CI profile missing")
        .split("\n[")
        .next()
        .unwrap()
        .to_owned()
}
#[test]
fn ci_profile_keeps_debug_checks() {
    let p = profile();
    for setting in [
        "inherits = \"test\"",
        "opt-level = 2",
        "debug = 1",
        "debug-assertions = true",
        "overflow-checks = true",
        "incremental = false",
    ] {
        assert!(p.contains(setting), "{setting}");
    }
}
#[test]
fn ci_profile_executes_debug_checks() {
    let s = Scratch::new();
    fs::create_dir(s.0.join("src")).unwrap();
    fs::write(s.0.join("Cargo.toml"), format!("[package]\nname=\"profile-probe\"\nversion=\"0.0.0\"\nedition=\"2021\"\n[workspace]\n[profile.ci]\n{}", profile())).unwrap();
    fs::write(
        s.0.join("src/main.rs"),
        r#"
fn main() {
    assert!(std::panic::catch_unwind(|| debug_assert!(false)).is_err());
    let value = std::hint::black_box(u32::MAX);
    assert!(std::panic::catch_unwind(|| std::hint::black_box(value + 1)).is_err());
}
"#,
    )
    .unwrap();
    assert!(Command::new("cargo")
        .env_remove("CARGO_TARGET_DIR")
        .args(["run", "--offline", "--profile", "ci", "--manifest-path"])
        .arg(s.0.join("Cargo.toml"))
        .status()
        .unwrap()
        .success());
}
/// COP-239: `cargo test` optimizes dependencies and the DSP-path crates.
/// Only opt-level changes, so debug assertions and overflow checks stay on.
#[test]
fn test_profile_optimizes_dependencies_and_keeps_checks() {
    let manifest = fs::read_to_string("Cargo.toml").unwrap();
    let section = |header: &str| {
        manifest
            .split(header)
            .nth(1)
            .map(|rest| rest.split("\n[").next().unwrap().to_owned())
    };
    for package in [
        "\"*\"",
        "coppa-dsp",
        "coppa-codec",
        "coppa-protocol",
        "coppa-channel",
        "coppa-ml",
        "coppa-engine",
    ] {
        let header = format!("[profile.test.package.{package}]");
        let body = section(&header).unwrap_or_else(|| panic!("{header} missing"));
        assert!(body.contains("opt-level = 2"), "{header}");
    }
    assert!(section("[profile.test.package.coppa-daemon]").is_none());
    for block in manifest.split("\n[profile.test").skip(1) {
        let block = block.split("\n[").next().unwrap();
        assert!(!block.contains("debug-assertions"), "{block}");
        assert!(!block.contains("overflow-checks"), "{block}");
    }
}
#[test]
fn runner_propagates_failure() {
    for group in GROUPS {
        let s = Scratch::new();
        mock_cargo(&s);
        let ok = runner(&s, group, 0);
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        let n: usize = fs::read_to_string(s.0.join("count"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for fail in 1..=n {
            fs::remove_file(s.0.join("count")).unwrap();
            let out = runner(&s, group, fail);
            assert_eq!(out.status.code(), Some(37), "{group} position {fail}");
            assert_eq!(
                fs::read_to_string(s.0.join("count")).unwrap().trim(),
                fail.to_string()
            );
            assert!(String::from_utf8_lossy(&out.stdout).contains("exit=37"));
        }
    }
    let s = Scratch::new();
    mock_cargo(&s);
    assert!(!runner(&s, "unknown", 0).status.success());
    assert!(!s.0.join("count").exists());
}
fn shell(step: &str) -> String {
    let after = WORKFLOW.split(step).nth(1).expect("step missing");
    let body = after.split("        run: |\n").nth(1).expect("run missing");
    body.lines()
        .take_while(|l| l.is_empty() || l.starts_with("          "))
        .map(|l| l.strip_prefix("          ").unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn fanin_rejects_unsuccessful_test_group() {
    let body = shell("- name: Check all jobs (including the fast tier's own native results)")
        .replace("${{ needs.create-ci-check.result }}", "success");
    // Everything after this loop polls fast CI. Test that separately from group aggregation.
    let body = body.split("# ci.yml's own fast tier").next().unwrap();
    for group in GROUPS {
        for result in ["success", "failure", "cancelled", "skipped", "timed_out"] {
            let s = Scratch::new();
            let mut cmd = Command::new("bash");
            cmd.args(["-e", "-c", body])
                .env("GITHUB_OUTPUT", s.0.join("out"));
            for key in [
                "BENCH_FUZZ",
                "CLIPPY",
                "MSRV",
                "DENY",
                "SEMVER",
                "FFI_ABI",
                "AUDIT",
                "WIKI_LINT",
                "CHECKS_QUEUE_WATCHDOG",
            ] {
                cmd.env(key, "success");
            }
            let out = cmd.env("TEST_FULL", result).output().unwrap();
            assert_eq!(
                out.status.success(),
                result == "success",
                "{group}: {result}"
            );
        }
    }
}
#[test]
fn workflow_tracks_every_test_group() {
    let job = WORKFLOW
        .split("  test-full:\n")
        .nth(1)
        .unwrap()
        .split("\n  bench-fuzz:")
        .next()
        .unwrap();
    assert!(job.contains("name: Full test suite (${{ matrix.group }})"));
    assert!(job.contains("fail-fast: false"));
    assert!(!job.contains("continue-on-error"));
    assert!(job.contains("group: [workspace, daemon-unit, daemon-integration, no-audio]"));
    assert!(job.contains(
        "github.event_name == 'workflow_dispatch' && (inputs.head_sha || github.sha) || github.sha"
    ));
    assert!(job.contains("bash tools/ci/run-full-tests.sh ${{ matrix.group }}"));
    let watch = shell("- name: Cancel the run if any downstream job never starts");
    for group in GROUPS {
        assert!(watch.contains(&format!("\"Full test suite ({group})\"")));
    }
    assert!(WORKFLOW.contains("TEST_FULL: ${{ needs.test-full.result }}"));
    assert!(WORKFLOW.contains("name=\"CI\""));
}
#[test]
fn watchdog_requires_each_group_to_start() {
    let body = shell("- name: Cancel the run if any downstream job never starts")
        .replace("local deadline=2700", "local deadline=0");
    for event in ["workflow_dispatch", "push", "merge_group"] {
        for status in ["in_progress", "completed", "queued"] {
            for group in GROUPS {
                let s = Scratch::new();
                // jq remains real. Only the API response and deadline are fixtures.
                let names = [
                    "Create CI check-run on the reviewed SHA",
                    "Bench & Fuzz Compilation",
                    "Clippy",
                    "MSRV Check",
                    "Supply-chain check (cargo-deny)",
                    "Semver check (cargo-semver-checks)",
                    "C ABI compatibility",
                    "Security Audit",
                    "Wiki lint",
                ]
                .into_iter()
                .map(str::to_owned)
                .chain(GROUPS.map(|g| format!("Full test suite ({g})")));
                let jobs = names
                    .map(|n| {
                        format!(
                            r#"{{"name":"{n}","status":"{}"}}"#,
                            if n == format!("Full test suite ({group})") {
                                status
                            } else {
                                "completed"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                fs::write(s.0.join("jobs"), format!(r#"{{"jobs":[{jobs}]}}"#)).unwrap();
                s.executable("gh", "#!/bin/sh\ncat \"$MOCK_STATE/jobs\"\n");
                let out = Command::new("bash")
                    .args(["-e", "-c", &body])
                    .env(
                        "PATH",
                        format!("{}:{}", s.0.display(), std::env::var("PATH").unwrap()),
                    )
                    .env("MOCK_STATE", &s.0)
                    .env("EVENT_NAME", event)
                    .env("REPO", "test/repo")
                    .env("RUN_ID", "1")
                    .env("GITHUB_OUTPUT", s.0.join("out"))
                    .output()
                    .unwrap();
                assert_eq!(
                    out.status.success(),
                    status != "queued",
                    "{event} {group} {status}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
    }
}
#[test]
fn groups_preserve_test_coverage() {
    let expected: [(&str, &[&str]); 4] = [
        ("workspace", &["--workspace --exclude coppa-daemon", "--lib --bins --tests", "--doc"]),
        ("daemon-unit", &["-p coppa-daemon", "--lib --bins"]),
        ("daemon-integration", &["-p coppa-daemon", "--test *", "--doc", "--examples --no-run"]),
        ("no-audio", &["-p coppa-cli --features kiss-tnc --test tnc_agw", "-p coppa-daemon --features websocket,mdns,kiss-tnc --test cli --test ptt_release --test reload --test runtime_smoke"]),
    ];
    for (group, selections) in expected {
        let s = Scratch::new();
        mock_cargo(&s);
        assert!(runner(&s, group, 0).status.success());
        let args = fs::read_to_string(s.0.join("args")).unwrap();
        for selection in selections {
            assert!(args.contains(selection), "{group}: {selection}\n{args}");
        }
        for line in args.lines() {
            assert!(line.contains("--locked --profile ci"));
            if group == "workspace" {
                assert!(line.contains("--features cpal-backend,websocket,ardop,kiss-tnc"));
            } else if group != "no-audio" {
                assert!(line.contains("--features cpal-backend,websocket,ardop,mdns,kiss-tnc"));
            }
        }
        assert!(!args.contains("--ignored"));
        assert!(!args.contains("--skip"));
    }
    // Compare Cargo's actual default test targets to the union of real runner
    // selections. The negative control proves that an omitted target is detected.
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut commands = Vec::new();
    for group in GROUPS {
        let s = Scratch::new();
        mock_cargo(&s);
        assert!(runner(&s, group, 0).status.success());
        commands.extend(
            fs::read_to_string(s.0.join("args"))
                .unwrap()
                .lines()
                .map(str::to_owned),
        );
    }
    let mut targets = Vec::new();
    for package in metadata["packages"].as_array().unwrap() {
        let name = package["name"].as_str().unwrap();
        for target in package["targets"].as_array().unwrap() {
            for kind in target["kind"].as_array().unwrap() {
                let kind = kind.as_str().unwrap();
                if kind == "bench" || kind == "custom-build" {
                    continue;
                }
                targets.push((
                    name.to_owned(),
                    target["name"].as_str().unwrap().to_owned(),
                    kind.to_owned(),
                ));
            }
        }
    }
    let omissions = |commands: &[String]| {
        targets
            .iter()
            .filter(|(package, target, kind)| {
                !commands.iter().any(|cmd| {
                    let package_selected = if cmd.contains("--workspace") {
                        package != "coppa-daemon"
                    } else {
                        cmd.contains(&format!("-p {package} "))
                    };
                    let target_selected = match kind.as_str() {
                        "lib" | "cdylib" | "staticlib" => cmd.contains("--lib"),
                        "bin" => cmd.contains("--bins"),
                        "example" => cmd.contains("--examples --no-run"),
                        "test" => {
                            cmd.contains("--tests")
                                || cmd.contains("--test *")
                                || cmd.contains(&format!("--test {target}"))
                        }
                        other => panic!("unmodeled Cargo target kind {other}"),
                    };
                    package_selected && target_selected
                })
            })
            .count()
    };
    assert_eq!(omissions(&commands), 0, "a Cargo target lost coverage");
    let incomplete: Vec<_> = commands
        .into_iter()
        .filter(|cmd| !cmd.contains("--test *"))
        .collect();
    assert!(
        omissions(&incomplete) > 0,
        "negative control must omit daemon integration targets"
    );
}
#[test]
fn group_time_budget_is_configured() {
    let job = WORKFLOW
        .split("  test-full:\n")
        .nth(1)
        .unwrap()
        .split("\n  bench-fuzz:")
        .next()
        .unwrap();
    for setting in [
        "timeout-minutes: 25",
        "RUST_TEST_THREADS: \"2\"",
        "key: ci-${{ matrix.group }}",
        "COPPA_REQUIRE_BROWSER: \"1\"",
    ] {
        assert!(job.contains(setting), "{setting}");
    }
    let runner = fs::read_to_string(Path::new("tools/ci/run-full-tests.sh")).unwrap();
    assert!(runner.contains("RUST_TEST_THREADS=${RUST_TEST_THREADS:-2}"));
    assert!(runner.contains("compile"));
    assert!(runner.contains("execute"));
}
// COP-207: the fan-in waits for a slow-but-passing fast-tier check instead of failing it,
// and a check that never finishes fails as "still running", not as unsuccessful.
const WINDOWS: &str = "Platform Check (windows-latest)";
/// The Windows platform check's state in one fake check-runs response.
#[derive(Clone, Copy)]
enum Windows {
    Running,
    Absent,
    Passed,
    Failed,
}
/// One run of the fan-in's real fast-tier poll against a fake `gh`.
struct FanIn {
    out: std::process::Output,
    /// How many times the step called `gh`.
    polls: usize,
    /// What the step wrote to `$GITHUB_OUTPUT`.
    github_output: String,
}
impl FanIn {
    /// Every `::error::` annotation line the step printed.
    fn errors(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.out.stdout)
            .lines()
            .filter(|l| l.starts_with("::error::"))
            .map(str::to_owned)
            .collect()
    }
}
fn fast_tier(windows_pending_polls: usize, wait_s: &str) -> std::process::Output {
    fast_tier_with(windows_pending_polls, wait_s, "", "")
}
fn fast_tier_with(
    windows_pending_polls: usize,
    wait_s: &str,
    bad: &str,
    fail: &str,
) -> std::process::Output {
    fast_tier_run(
        Windows::Running,
        Windows::Passed,
        windows_pending_polls,
        wait_s,
        bad,
        fail,
    )
    .out
}
/// The six fast-tier check names the fan-in waits for, in ci-full.yml's order.
const FAST_TIER: [&str; 6] = [
    "Check",
    "Test with Features (fast, lib-only)",
    "Format",
    "Platform Check (ubuntu-latest)",
    "Platform Check (macos-latest)",
    WINDOWS,
];
/// Runs the fan-in step with its wait cut to `wait_s` seconds. Windows reports `before` for
/// the first `pending_polls` calls and `after` from then on; the other five checks pass.
fn fast_tier_run(
    before: Windows,
    after: Windows,
    pending_polls: usize,
    wait_s: &str,
    bad: &str,
    fail: &str,
) -> FanIn {
    let runs = |windows: Windows| {
        FAST_TIER
            .iter()
            .filter_map(|n| {
                let conclusion = match (*n == WINDOWS, windows) {
                    (true, Windows::Absent) => return None,
                    (true, Windows::Running) => None,
                    (true, Windows::Failed) => Some("failure"),
                    _ => Some("success"),
                };
                Some(row(n, "github-actions", "2026-11-13T19:07:14Z", conclusion))
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    fan_in(
        &runs(before),
        &runs(after),
        pending_polls,
        wait_s,
        bad,
        fail,
    )
}
/// One fake check-run row. `app` is the slug of the GitHub App that owns it. A `None`
/// conclusion means the check-run is still `in_progress`.
fn row(name: &str, app: &str, started: &str, conclusion: Option<&str>) -> String {
    let (status, conclusion, completed) = match conclusion {
        None => ("in_progress", "null".to_owned(), "null".to_owned()),
        Some(c) => ("completed", format!("\"{c}\""), format!("\"{started}\"")),
    };
    format!(
        r#"{{"name":"{name}","app":{{"slug":"{app}"}},"started_at":"{started}","completed_at":{completed},"status":"{status}","conclusion":{conclusion}}}"#
    )
}
/// Runs the fan-in step with its wait cut to `wait_s` seconds against a fake `gh`. The first
/// `pending_polls` calls return the comma-joined check-run rows in `pending`, and later calls
/// return `done`. `bad` and `fail` list the call numbers that return an error body or fail.
fn fan_in(
    pending: &str,
    done: &str,
    pending_polls: usize,
    wait_s: &str,
    bad: &str,
    fail: &str,
) -> FanIn {
    const DEADLINE: &str = "fast_tier_deadline=$((SECONDS + fast_tier_wait_minutes * 60))";
    let body = shell("- name: Check all jobs (including the fast tier's own native results)")
        .replace("${{ needs.create-ci-check.result }}", "success");
    // Fail loudly on a rename instead of silently running the real 75-minute wait.
    assert!(body.contains(DEADLINE), "fan-in deadline line changed");
    assert!(body.contains("sleep 30"), "fan-in poll interval changed");
    let body = body
        .replace(
            DEADLINE,
            &format!("fast_tier_deadline=$((SECONDS + {wait_s}))"),
        )
        .replace("sleep 30", "sleep 0.05");
    let s = Scratch::new();
    fs::write(
        s.0.join("pending"),
        format!(r#"{{"check_runs":[{pending}]}}"#),
    )
    .unwrap();
    fs::write(s.0.join("done"), format!(r#"{{"check_runs":[{done}]}}"#)).unwrap();
    s.executable(
        "gh",
        &format!(
            r#"#!/usr/bin/env bash
n=0
[ ! -f "$MOCK_STATE/polls" ] || n=$(cat "$MOCK_STATE/polls")
echo $((n+1)) > "$MOCK_STATE/polls"
case " ${{MOCK_BAD:-}} " in *" $n "*) echo '{{"message":"Server Error"}}'; exit 0;; esac
case " ${{MOCK_FAIL:-}} " in *" $n "*) echo 'HTTP 502' >&2; exit 1;; esac
if [ "$n" -lt {pending_polls} ]; then cat "$MOCK_STATE/pending"; else cat "$MOCK_STATE/done"; fi
"#
        ),
    );
    let mut cmd = Command::new("bash");
    cmd.args(["-e", "-c", &body])
        .env(
            "PATH",
            format!("{}:{}", s.0.display(), std::env::var("PATH").unwrap()),
        )
        .env("MOCK_STATE", &s.0)
        .env("REPO", "test/repo")
        .env("HEAD_SHA", "d1ee7d15")
        .env("GITHUB_OUTPUT", s.0.join("out"))
        .env("TEST_FULL", "success")
        .env("MOCK_BAD", bad)
        .env("MOCK_FAIL", fail);
    for key in [
        "BENCH_FUZZ",
        "CLIPPY",
        "MSRV",
        "DENY",
        "SEMVER",
        "FFI_ABI",
        "AUDIT",
        "WIKI_LINT",
        "CHECKS_QUEUE_WATCHDOG",
    ] {
        cmd.env(key, "success");
    }
    let out = cmd.output().unwrap();
    FanIn {
        out,
        polls: fs::read_to_string(s.0.join("polls")).map_or(0, |n| n.trim().parse().unwrap()),
        github_output: fs::read_to_string(s.0.join("out")).unwrap_or_default(),
    }
}
#[test]
fn fanin_waits_for_slow_fast_tier_then_fails_closed() {
    // Scenario 1: Windows is still running for two polls, then passes.
    let run = fast_tier_run(Windows::Running, Windows::Passed, 2, "7200", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=success"), "{stdout}");
    assert!(run.polls > 2, "{} polls: {stdout}", run.polls);
    // Scenario 2: still running at the deadline, or never reported, fails closed. The one
    // annotation line names the Windows check and no other.
    for (windows, error) in [
        (
            Windows::Running,
            format!(
                "::error::Fast-tier check(s) still running on d1ee7d15 after 75 minutes (timed out waiting, not failed): [\"{WINDOWS}\"]"
            ),
        ),
        (
            Windows::Absent,
            format!(
                "::error::Missing fast-tier check-run(s) for d1ee7d15 after 75 minutes: [\"{WINDOWS}\"] -- ci.yml may not have run at all, ran against a different SHA, or is still queued."
            ),
        ),
    ] {
        let run = fast_tier_run(windows, windows, 0, "2", "", "");
        let stdout = String::from_utf8_lossy(&run.out.stdout);
        assert!(!run.out.status.success(), "{stdout}");
        assert!(run.github_output.contains("outcome=failure"), "{stdout}");
        assert!(run.polls > 1, "{} polls: {stdout}", run.polls);
        assert_eq!(run.errors(), [error], "{stdout}");
    }
    // A Windows check that fails is reported on the first poll, without waiting.
    let run = fast_tier_run(Windows::Failed, Windows::Failed, 0, "7200", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(!run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=failure"), "{stdout}");
    assert_eq!(run.polls, 1, "{stdout}");
    assert_eq!(
        run.errors(),
        [format!(
            "::error::One or more fast-tier checks did not succeed on d1ee7d15: [{{\"name\":\"{WINDOWS}\",\"conclusion\":\"failure\"}}]"
        )],
        "{stdout}"
    );
}
#[test]
fn fanin_waits_for_a_slow_passing_fast_tier_check() {
    let out = fast_tier(5, "7200");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
#[test]
fn fanin_reports_a_hung_fast_tier_check_as_still_running() {
    let out = fast_tier(1_000_000, "1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(stdout.contains("still running"), "{stdout}");
    assert!(
        stdout.contains("Platform Check (windows-latest)"),
        "{stdout}"
    );
}
/// The merge queue's `check_response_timeout_minutes`. It lives in the "Coppa - Native Merge
/// Queue" ruleset (id 23405672), outside this repo; read with `gh api` on 2026-11-14.
const QUEUE_CHECK_RESPONSE_TIMEOUT_MINUTES: u32 = 120;
/// The first whole number after `key` in `text`.
fn minutes_after(text: &str, key: &str) -> u32 {
    let after = text
        .split(key)
        .nth(1)
        .unwrap_or_else(|| panic!("{key:?} missing"));
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("{key:?} has no number"))
}
#[test]
fn fanin_wait_fits_job_and_queue_limits() {
    // COP-203 Scenario 3: the fan-in's own named error must fire before the job is
    // cancelled and before the merge queue drops the group.
    let job = WORKFLOW.split("\n  ci:\n").nth(1).expect("ci job missing");
    let test_full = WORKFLOW
        .split("\n  test-full:\n")
        .nth(1)
        .expect("test-full job missing");
    let check = job
        .split("- name: Check all jobs (including the fast tier's own native results)")
        .nth(1)
        .and_then(|after| after.split("- name: ").next())
        .expect("Check step missing");
    let wait = minutes_after(job, "fast_tier_wait_minutes=");
    let check_timeout = minutes_after(check, "\n        timeout-minutes: ");
    let job_timeout = minutes_after(job, "\n    timeout-minutes: ");
    let test_full_timeout = minutes_after(test_full, "\n    timeout-minutes: ");
    // The slowest Windows leg seen is 63.5 minutes; it must fit with no head start.
    assert!(wait >= 64, "wait {wait} is under the slowest fast-tier leg");
    // The Check step's own cap leaves room for the always() mint and relay steps after it.
    assert!(
        wait < check_timeout && check_timeout < job_timeout,
        "wait {wait} < Check step {check_timeout} < ci job {job_timeout} must hold"
    );
    assert!(
        test_full_timeout + wait < QUEUE_CHECK_RESPONSE_TIMEOUT_MINUTES,
        "test-full {test_full_timeout} + wait {wait} must end before the queue's timeout"
    );
    assert!(job.contains(&format!(
        "check_response_timeout_minutes of {QUEUE_CHECK_RESPONSE_TIMEOUT_MINUTES}"
    )));
    assert!(job.contains("id 23405672"));
    assert!(!WORKFLOW.contains("after 30 minutes"));
}
#[test]
fn fast_tier_jobs_keep_their_time_limit() {
    // COP-207's caps free a hung runner. The fan-in no longer outwaits them (COP-203 keeps its
    // wait under the queue timeout and fails a still-running check by name), but each cap must
    // stay and must sit above the slowest leg seen (63.5 min) so it never cancels a passing run.
    let ci = include_str!("../.github/workflows/ci.yml");
    for job in [
        "Test with Features (fast, lib-only)",
        "Platform Check (${{ matrix.os }})",
    ] {
        let after = ci.split(&format!("name: {job}\n")).nth(1).unwrap();
        let limit: u32 = after
            .lines()
            .take_while(|l| !l.starts_with("  ") || l.starts_with("    "))
            .find_map(|l| l.trim().strip_prefix("timeout-minutes: "))
            .unwrap_or_else(|| panic!("{job} has no timeout-minutes"))
            .parse()
            .unwrap();
        assert!(limit >= 64, "{job}: {limit}m would cancel a passing leg");
    }
}
#[test]
fn relay_token_is_minted_after_the_fast_tier_wait() {
    // An installation token lives 60 min; the fast-tier wait can run 75 (COP-203).
    let check = WORKFLOW
        .find("- name: Check all jobs (including the fast tier's own native results)")
        .unwrap();
    // rfind: the watchdog job earlier in the file mints its own token under the same name.
    let mint = WORKFLOW
        .rfind("- name: Mint a hagale-agent installation token for check-run writes")
        .unwrap();
    let retry = WORKFLOW
        .rfind("- name: Retry minting a hagale-agent installation token if that failed")
        .unwrap();
    let relay = WORKFLOW
        .find("- name: Relay this result onto the reviewed SHA's CI check-run")
        .unwrap();
    assert!(check < mint && mint < retry && retry < relay);
    for step in [&WORKFLOW[mint..retry], &WORKFLOW[retry..relay]] {
        assert!(step.contains("if: always() && github.event_name == 'workflow_dispatch'"));
    }
}
#[test]
fn fanin_retries_a_bad_check_run_response() {
    // Poll 1 returns an error body without check_runs, poll 2 fails outright; both are retried.
    let out = fast_tier_with(4, "7200", "1", "2");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("lookup on d1ee7d15 failed; retrying"));
}
#[test]
fn fanin_accepts_a_pass_from_an_earlier_fast_tier_run_on_the_same_sha() {
    // COP-225: marking a PR ready for review (or reopening it) starts a second ci.yml run on
    // the same SHA, and GitHub lists each run's check-runs separately. Run 1 is the first
    // ci.yml run on PR #281's head; run 2 is the one its ready event started.
    const RUN1: &str = "2026-11-17T08:57:35Z";
    const RUN2: &str = "2026-11-17T10:01:20Z";
    const ACTIONS: &str = "github-actions";
    // The five checks other than Windows pass in run 1 and have no run-2 check-run.
    let with_windows = |windows: [String; 2]| {
        FAST_TIER
            .iter()
            .filter(|n| **n != WINDOWS)
            .map(|n| row(n, ACTIONS, RUN1, Some("success")))
            .chain(windows)
            .collect::<Vec<_>>()
            .join(",")
    };

    // AC1, the incident: run 1 passed all six checks and run 2 has all six still running.
    // The pass counts on the first poll; the old newest-wins rule timed out here.
    let rows = FAST_TIER
        .iter()
        .map(|n| row(n, ACTIONS, RUN1, Some("success")))
        .chain(FAST_TIER.iter().map(|n| row(n, ACTIONS, RUN2, None)))
        .collect::<Vec<_>>()
        .join(",");
    let run = fan_in(&rows, &rows, 0, "2", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=success"), "{stdout}");
    assert_eq!(run.polls, 1, "{stdout}");

    // AC2: an earlier pass beats a later failure of the same name on the same SHA.
    let rows = with_windows([
        row(WINDOWS, ACTIONS, RUN1, Some("success")),
        row(WINDOWS, ACTIONS, RUN2, Some("failure")),
    ]);
    let run = fan_in(&rows, &rows, 0, "7200", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=success"), "{stdout}");

    // AC3: with no pass in any run, the newest check-run decides. Run 1 failed Windows and
    // run 2 is still running it, so the fan-in waits and then names only Windows.
    let rows = with_windows([
        row(WINDOWS, ACTIONS, RUN1, Some("failure")),
        row(WINDOWS, ACTIONS, RUN2, None),
    ]);
    let run = fan_in(&rows, &rows, 0, "2", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(!run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=failure"), "{stdout}");
    assert!(run.polls > 1, "{} polls: {stdout}", run.polls);
    assert_eq!(
        run.errors(),
        [format!(
            "::error::Fast-tier check(s) still running on d1ee7d15 after 75 minutes (timed out waiting, not failed): [\"{WINDOWS}\"]"
        )],
        "{stdout}"
    );

    // AC4: another app's check-run under a fast-tier name does not count, so its success
    // cannot hide a GitHub Actions failure.
    let rows = with_windows([
        row(WINDOWS, ACTIONS, RUN1, Some("failure")),
        row(WINDOWS, "some-other-app", RUN2, Some("success")),
    ]);
    let run = fan_in(&rows, &rows, 0, "7200", "", "");
    let stdout = String::from_utf8_lossy(&run.out.stdout);
    assert!(!run.out.status.success(), "{stdout}");
    assert!(run.github_output.contains("outcome=failure"), "{stdout}");
    assert_eq!(run.polls, 1, "{stdout}");
    assert_eq!(
        run.errors(),
        [format!(
            "::error::One or more fast-tier checks did not succeed on d1ee7d15: [{{\"name\":\"{WINDOWS}\",\"conclusion\":\"failure\"}}]"
        )],
        "{stdout}"
    );
}
