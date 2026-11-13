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
