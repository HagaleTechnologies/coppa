//! COP-86 (H-082): stdout carries data only; status lines go to stderr, and
//! a closed stdout pipe ends the run quietly instead of panicking.
#![cfg(feature = "file-backend")]

mod common;

use common::{coppa_command, Scratch};
use std::process::Output;

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

#[track_caller]
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

/// `coppa tx hi -o f.wav` in `scratch`.
fn write_hi_wav(scratch: &Scratch) {
    assert_ok(&run_in(scratch, &["tx", "hi", "-o", "f.wav"]));
}

#[test]
fn tx_to_file_prints_nothing_on_stdout() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tx", "hi", "-o", "f.wav"]);
    assert_ok(&output);
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    for needle in [
        "Encoding: \"hi\"",
        "Generated 65520 audio samples",
        "Written to",
    ] {
        assert!(err.contains(needle), "stderr lacks {needle:?}:\n{err}");
    }
}

#[test]
fn tune_to_file_prints_nothing_on_stdout() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tune", "-d", "1", "-o", "t.wav"]);
    assert_ok(&output);
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    for needle in ["Generating two-tone calibration signal", "Written to"] {
        assert!(err.contains(needle), "stderr lacks {needle:?}:\n{err}");
    }
}

#[test]
fn rx_file_stdout_is_only_decoded_lines() {
    let scratch = Scratch::new();
    write_hi_wav(&scratch);
    let output = run_in(&scratch, &["rx", "f.wav"]);
    assert_ok(&output);
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out}");
    assert!(lines[0].starts_with("Decoded: \"hi\"  (SNR: "), "{out}");
    assert!(
        stderr(&output).contains("Reading 65520 samples from"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn quiet_hides_status_but_not_data() {
    let scratch = Scratch::new();
    write_hi_wav(&scratch);
    let output = run_in(&scratch, &["-q", "rx", "f.wav"]);
    assert_ok(&output);
    assert!(!stderr(&output).contains("Reading"), "{}", stderr(&output));
    assert!(stdout(&output).contains("Decoded:"), "{}", stdout(&output));
}

#[test]
fn loopback_report_stays_on_stdout() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["loopback", "hi"]);
    assert_ok(&output);
    assert!(
        stdout(&output).contains("PASS: Loopback test successful!"),
        "{}",
        stdout(&output)
    );
}

#[cfg(unix)]
#[test]
fn closed_pipe_exits_zero_without_panic() {
    use std::process::Stdio;
    let scratch = Scratch::new();
    write_hi_wav(&scratch);
    for args in [&["rx", "f.wav"][..], &["profiles"][..]] {
        let mut child = coppa_command()
            .current_dir(&scratch.0)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn coppa");
        // Close the read end before coppa writes its first data line.
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("wait for coppa");
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {err}");
        assert!(!err.contains("panicked"), "{args:?}: {err}");
        assert!(!err.contains("Broken pipe"), "{args:?}: {err}");
    }
}
