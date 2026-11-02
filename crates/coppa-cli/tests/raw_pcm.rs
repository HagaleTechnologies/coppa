//! COP-86 (H-077): 16-bit WAV by default, raw PCM on stdout (`-o -`) and on
//! stdin (`rx -`).
#![cfg(feature = "file-backend")]

mod common;

use common::{coppa_command, Scratch};
use std::io::Write;
use std::process::{Output, Stdio};

fn run_in(scratch: &Scratch, args: &[&str]) -> Output {
    coppa_command()
        .current_dir(&scratch.0)
        .args(args)
        .output()
        .expect("run coppa")
}

/// Run `coppa args` with `input` on stdin.
fn run_with_stdin(scratch: &Scratch, args: &[&str], input: &[u8]) -> Output {
    let mut child = coppa_command()
        .current_dir(&scratch.0)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn coppa");
    let mut stdin = child.stdin.take().expect("stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        // rx may stop reading early (-d), so a closed pipe is fine here.
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().expect("wait for coppa");
    writer.join().expect("stdin writer");
    output
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

/// `coppa tx MESSAGE -o - [extra]`'s raw bytes.
fn tx_raw(scratch: &Scratch, message: &str, extra: &[&str]) -> Vec<u8> {
    let mut args = vec!["tx", message, "-o", "-"];
    args.extend_from_slice(extra);
    let output = run_in(scratch, &args);
    assert_ok(&output);
    output.stdout
}

#[test]
fn tx_to_stdout_is_headerless_s16le() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tx", "hi", "-o", "-"]);
    assert_ok(&output);
    assert_eq!(output.stdout.len(), 65520 * 2);
    assert_ne!(&output.stdout[..4], b"RIFF");
    assert!(
        stderr(&output).contains("Written to stdout (raw s16le, mono, 48000 Hz)"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn tx_to_stdout_f32le() {
    let scratch = Scratch::new();
    let bytes = tx_raw(&scratch, "hi", &["--sample-format", "f32le"]);
    assert_eq!(bytes.len(), 65520 * 4);
}

#[test]
fn tx_pipe_into_rx_decodes() {
    let scratch = Scratch::new();
    for format in ["s16le", "f32le"] {
        let raw = tx_raw(&scratch, "CQ CQ DE TEST", &["--sample-format", format]);
        let rx = run_with_stdin(&scratch, &["rx", "-", "--sample-format", format], &raw);
        assert_ok(&rx);
        assert!(
            stdout(&rx).starts_with("Decoded: \"CQ CQ DE TEST\""),
            "{format}: {}",
            stdout(&rx)
        );
        assert!(
            stderr(&rx).contains(&format!(
                "Reading raw {format} audio (mono, 48000 Hz) from stdin"
            )),
            "{}",
            stderr(&rx)
        );
    }
}

#[test]
fn rx_stdin_duration_caps_samples() {
    let scratch = Scratch::new();
    let mut raw = tx_raw(&scratch, "FIRST FRAME", &[]);
    raw.extend(std::iter::repeat_n(0u8, 48_000 * 2));
    raw.extend(tx_raw(&scratch, "SECOND FRAME", &[]));
    let all = run_with_stdin(&scratch, &["rx", "-"], &raw);
    assert_ok(&all);
    assert!(stdout(&all).contains("SECOND FRAME"), "{}", stdout(&all));

    let cut = run_with_stdin(&scratch, &["rx", "-", "-d", "2"], &raw);
    assert_ok(&cut);
    assert!(stdout(&cut).contains("FIRST FRAME"), "{}", stdout(&cut));
    assert!(!stdout(&cut).contains("SECOND FRAME"), "{}", stdout(&cut));
}

#[test]
fn rx_stdin_refuses_wav_header() {
    let scratch = Scratch::new();
    assert_ok(&run_in(&scratch, &["tx", "hi", "-o", "f.wav"]));
    let wav = std::fs::read(scratch.0.join("f.wav")).unwrap();
    let output = run_with_stdin(&scratch, &["rx", "-"], &wav);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("stdin starts with a WAV header"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn rx_stdin_no_frames_is_an_error() {
    let scratch = Scratch::new();
    let output = run_with_stdin(&scratch, &["rx", "-"], &vec![0u8; 48_000 * 2]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no frames decoded from stdin"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn rx_file_rejects_sample_format() {
    let scratch = Scratch::new();
    assert_ok(&run_in(&scratch, &["tx", "hi", "-o", "f.wav"]));
    let output = run_in(&scratch, &["rx", "f.wav", "--sample-format", "f32le"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(
            "--sample-format applies only to raw PCM on stdin (rx -); a WAV file's header sets its format"
        ),
        "{}",
        stderr(&output)
    );
}

#[test]
fn tune_to_stdout() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tune", "-d", "1", "-o", "-"]);
    assert_ok(&output);
    assert_eq!(output.stdout.len(), 48_000 * 2);
}

#[test]
fn wav_output_is_16_bit() {
    let scratch = Scratch::new();
    assert_ok(&run_in(&scratch, &["tx", "hi", "-o", "f.wav"]));
    let bytes = std::fs::read(scratch.0.join("f.wav")).unwrap();
    // Canonical 44-byte PCM header: format 1 (PCM), mono, 48 kHz, 16-bit.
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(
        u16::from_le_bytes([bytes[20], bytes[21]]),
        1,
        "PCM format tag"
    );
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1, "channels");
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        48_000
    );
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16, "bits");
    assert_eq!(bytes.len(), 44 + 65520 * 2);

    let rx = run_in(&scratch, &["rx", "f.wav"]);
    assert_ok(&rx);
    assert!(
        stdout(&rx).starts_with("Decoded: \"hi\""),
        "{}",
        stdout(&rx)
    );

    // The old float file on request.
    assert_ok(&run_in(
        &scratch,
        &["tx", "hi", "-o", "f32.wav", "--sample-format", "f32le"],
    ));
    let float = std::fs::read(scratch.0.join("f32.wav")).unwrap();
    assert!(float.len() > 65520 * 4, "{}", float.len());
}

#[test]
fn sample_format_requires_output() {
    let scratch = Scratch::new();
    let output = run_in(&scratch, &["tx", "hi", "--sample-format", "f32le"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}
