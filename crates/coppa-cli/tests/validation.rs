//! COP-86 (H-079, H-080): bad input is a clap usage error with exit 2, and
//! `tune` takes `--duration`/`-d` like `rx`.

mod common;

use common::{coppa_command, Scratch};
use std::process::Output;

fn run(args: &[&str]) -> Output {
    run_in(&Scratch::new(), args)
}

fn run_in(scratch: &Scratch, args: &[&str]) -> Output {
    coppa_command()
        .current_dir(&scratch.0)
        .args(args)
        .output()
        .expect("run coppa")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Both streams, for status lines whose stream is not what a test pins.
fn all_output(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

#[track_caller]
fn assert_usage_error(args: &[&str], needle: &str) {
    let output = run(args);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{args:?} should be a usage error\nstdout:\n{}\nstderr:\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(needle),
        "{args:?}: stderr should contain {needle:?}:\n{}",
        stderr(&output)
    );
}

#[track_caller]
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

#[test]
fn empty_message_is_a_usage_error() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tx", "", "-o", "x.wav"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("a value is required for '<MESSAGE>'"),
        "{}",
        stderr(&output)
    );
    assert!(!scratch.0.join("x.wav").exists(), "no file is written");
    assert_usage_error(&["loopback", ""], "a value is required for '<MESSAGE>'");
}

#[test]
fn empty_string_flags_are_usage_errors() {
    for flag in [
        "--profile",
        "--callsign",
        "--device",
        "--device-id",
        "--rigctld",
        "--band",
        "-o",
    ] {
        assert_usage_error(&["tx", "hi", flag, ""], "a value is required for");
    }
    assert_usage_error(&["rx", ""], "a value is required for");
    assert_usage_error(&["rx", "-i", ""], "a value is required for");
    assert_usage_error(&["profiles", "-p", ""], "a value is required for");
    assert_usage_error(&["doctor", "--device", ""], "a value is required for");
    assert_usage_error(&["doctor", "--rigctld", ""], "a value is required for");
}

#[test]
fn empty_env_profile_is_still_unset() {
    // COP-57: an empty COPPA_* value means unset; only the flag is strict.
    let scratch = Scratch::new();
    let output = coppa_command()
        .current_dir(&scratch.0)
        .env("COPPA_PROFILE", "")
        .args(["tx", "hi", "-o", "x.wav"])
        .output()
        .expect("run coppa");
    assert_ok(&output);
    assert!(scratch.0.join("x.wav").exists());
}

#[test]
fn tune_duration_rejects_out_of_range() {
    for value in ["0", "-1", "nan", "inf", "1e12", "121"] {
        assert_usage_error(
            &["tune", "--duration", value, "-o", "t.wav"],
            "must be more than 0 and at most 120 seconds",
        );
    }
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tune", "--duration", "0.5", "-o", "t.wav"]);
    assert_ok(&output);
    assert!(
        all_output(&output).contains("Generated 24000 audio samples"),
        "{}",
        all_output(&output)
    );
    assert!(scratch.0.join("t.wav").exists());
}

#[test]
fn tune_single_rejects_out_of_band() {
    for value in ["0", "299", "3001", "30000", "-5"] {
        assert_usage_error(
            &["tune", "--single", value, "-o", "t.wav"],
            "must be between 300 and 3000 Hz",
        );
    }
    let scratch = Scratch::new();
    let output = run_in(
        &scratch,
        &["tune", "--single", "1000", "--duration", "1", "-o", "t.wav"],
    );
    assert_ok(&output);
    assert!(scratch.0.join("t.wav").exists());
}

#[test]
fn tune_seconds_alias_warns_and_works() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tune", "--seconds", "2", "-o", "t.wav"]);
    assert_ok(&output);
    assert!(
        stderr(&output).contains("WARNING: '--seconds' is deprecated"),
        "{}",
        stderr(&output)
    );
    assert!(
        all_output(&output).contains("Generated 96000 audio samples"),
        "{}",
        all_output(&output)
    );
    assert!(scratch.0.join("t.wav").exists());

    // The warning prints even under --quiet, so scripts hear about it.
    let quiet = run_in(&scratch, &["-q", "tune", "--seconds", "1", "-o", "q.wav"]);
    assert_ok(&quiet);
    assert!(stderr(&quiet).contains("WARNING: '--seconds' is deprecated"));

    assert_usage_error(
        &["tune", "-d", "2", "--seconds", "3", "-o", "t.wav"],
        "cannot be used with",
    );
}

#[test]
fn negative_whole_numbers_get_a_range_message() {
    for flag in ["--ptt-lead-ms", "--ptt-tail-ms"] {
        assert_usage_error(
            &["tx", "hi", "-o", "x.wav", flag, "-5"],
            "must be a whole number, 0 or more",
        );
    }
    assert_usage_error(
        &["rx", "-d", "-1", "x.wav"],
        "must be a whole number of seconds, 0 or more",
    );
}

#[test]
fn bad_ptt_is_a_usage_error_even_offline() {
    assert_usage_error(
        &["tx", "hi", "-o", "x.wav", "--ptt", "bogus"],
        "possible values: none, vox, rigctld",
    );
    assert_usage_error(
        &["tune", "-d", "1", "-o", "t.wav", "--ptt", "bogus"],
        "possible values: none, vox, rigctld",
    );
}

#[test]
fn help_layout() {
    let top = run(&["--help"]);
    assert_ok(&top);
    let top = stdout(&top);
    assert!(top.contains("Global options:"), "{top}");
    let commands: Vec<&str> = top
        .lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .collect();
    assert!(!commands.is_empty(), "{top}");
    for line in commands {
        assert!(line.len() <= 100, "command row too long: {line:?}");
    }

    let tune = run(&["tune", "--help"]);
    assert_ok(&tune);
    let tune = stdout(&tune);
    for needle in [
        "-d, --duration <SECONDS>",
        "--single <HZ>",
        "--ptt-lead-ms <MS>",
    ] {
        assert!(
            tune.contains(needle),
            "tune --help lacks {needle:?}:\n{tune}"
        );
    }
    assert!(!tune.contains("--seconds"), "{tune}");

    let tx = run(&["tx", "--help"]);
    assert_ok(&tx);
    let tx = stdout(&tx);
    let heading = tx.find("Global options:").expect("Global options heading");
    let quiet = tx.find("-q, --quiet").expect("--quiet row");
    assert!(quiet > heading, "{tx}");
}
