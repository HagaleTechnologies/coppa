//! COP-86 (H-076): Ctrl-C (SIGINT) or SIGTERM ends `coppa rx` cleanly, with
//! the trailing flush and a summary line; a second Ctrl-C exits 130 at once.
#![cfg(all(unix, feature = "file-backend"))]

mod common;

use common::{coppa_command, Scratch};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

/// A running `coppa rx -` fed by a background thread, with both output
/// streams forwarded line by line.
struct Capture {
    child: Child,
    stdout: Receiver<String>,
    stderr: Receiver<String>,
}

/// `coppa tx MESSAGE -o -`'s raw s16le bytes.
fn frame(scratch: &Scratch, message: &str) -> Vec<u8> {
    let output = coppa_command()
        .current_dir(&scratch.0)
        .args(["tx", message, "-o", "-"])
        .output()
        .expect("run coppa tx");
    assert!(output.status.success());
    output.stdout
}

fn lines<R: std::io::Read + Send + 'static>(reader: R) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Spawn `coppa args`, writing `first` and then silence every 20 ms to its
/// stdin until it exits, like a live `arecord` pipe would.
fn spawn(scratch: &Scratch, args: &[&str], first: Vec<u8>, feed: bool) -> Capture {
    let mut child = coppa_command()
        .current_dir(&scratch.0)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn coppa");
    let mut stdin = child.stdin.take().expect("stdin");
    std::thread::spawn(move || {
        if stdin.write_all(&first).is_err() {
            return;
        }
        if !feed {
            // Hold stdin open without writing, so the read blocks.
            std::thread::sleep(TIMEOUT);
            return;
        }
        let silence = vec![0u8; 4800 * 2];
        while stdin.write_all(&silence).is_ok() {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let stdout = lines(child.stdout.take().expect("stdout"));
    let stderr = lines(child.stderr.take().expect("stderr"));
    Capture {
        child,
        stdout,
        stderr,
    }
}

/// Wait for a line containing `needle`, returning every line seen.
fn wait_for(lines: &Receiver<String>, needle: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match lines.recv_timeout(left) {
            Ok(line) => {
                let found = line.contains(needle);
                seen.push(line);
                if found {
                    return seen;
                }
            }
            Err(_) => break,
        }
    }
    panic!("no line containing {needle:?}; saw {seen:?}");
}

fn signal(child: &Child, name: &str) {
    let status = std::process::Command::new("kill")
        .arg(format!("-{name}"))
        .arg(child.id().to_string())
        .status()
        .expect("run kill");
    assert!(status.success());
}

/// Wait for exit and drain the rest of both streams.
fn finish(
    mut capture: Capture,
    mut out: Vec<String>,
    mut err: Vec<String>,
) -> (Option<i32>, String, String) {
    let status = capture.child.wait().expect("wait for coppa");
    out.extend(capture.stdout.iter());
    err.extend(capture.stderr.iter());
    (status.code(), out.join("\n"), err.join("\n"))
}

fn interrupted_capture(args: &[&str], sig: &str) -> (Option<i32>, String, String) {
    let scratch = Scratch::new();
    let capture = spawn(&scratch, args, frame(&scratch, "CQ CQ DE TEST"), true);
    let err = wait_for(&capture.stderr, "from stdin (Ctrl+C to stop)");
    let out = wait_for(&capture.stdout, "Decoded:");
    signal(&capture.child, sig);
    finish(capture, out, err)
}

#[test]
fn sigint_ends_stdin_capture_cleanly() {
    let (code, out, err) = interrupted_capture(&["rx", "-"], "INT");
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{err}");
    assert!(out.contains("Decoded: \"CQ CQ DE TEST\""), "{out}");
    assert!(err.contains("Stopped after "), "{err}");
    assert!(err.contains("1 frame decoded, 0 failed."), "{err}");
}

#[test]
fn sigterm_behaves_like_sigint() {
    let (code, out, err) = interrupted_capture(&["rx", "-"], "TERM");
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{err}");
    assert!(err.contains("1 frame decoded, 0 failed."), "{err}");
}

#[test]
fn listen_alias_handles_sigint() {
    let (code, out, err) = interrupted_capture(&["listen", "-"], "INT");
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{err}");
    assert!(err.contains("'coppa listen' is deprecated"), "{err}");
    assert!(err.contains("1 frame decoded, 0 failed."), "{err}");
}

#[test]
fn quiet_suppresses_summary() {
    let scratch = Scratch::new();
    let capture = spawn(
        &scratch,
        &["-q", "rx", "-"],
        frame(&scratch, "CQ CQ DE TEST"),
        true,
    );
    let out = wait_for(&capture.stdout, "Decoded:");
    signal(&capture.child, "INT");
    let (code, out, err) = finish(capture, out, Vec::new());
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{err}");
    assert!(!err.contains("Stopped after"), "{err}");
}

#[test]
fn second_sigint_forces_exit_130() {
    let scratch = Scratch::new();
    let capture = spawn(&scratch, &["rx", "-"], Vec::new(), false);
    let err = wait_for(&capture.stderr, "Reading raw s16le audio");
    signal(&capture.child, "INT");
    std::thread::sleep(Duration::from_millis(200));
    signal(&capture.child, "INT");
    let (code, out, err) = finish(capture, Vec::new(), err);
    assert_eq!(code, Some(130), "stdout:\n{out}\nstderr:\n{err}");
    assert!(
        err.contains("Interrupted again; exiting without cleanup."),
        "{err}"
    );
}
