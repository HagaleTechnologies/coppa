//! Binary-level checks for `coppa doctor`, including its rigctld wire traffic.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const ROWS: [&str; 8] = [
    "audio backend",
    "audio devices",
    "input device",
    "input 48 kHz mono",
    "output device",
    "output 48 kHz mono",
    "rigctld connection",
    "rig CAT read",
];

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coppa"))
        .args(args)
        .env_remove("RUST_LOG")
        .env_remove("NO_COLOR")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("run coppa")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn row<'a>(report: &'a str, name: &str) -> &'a str {
    report
        .lines()
        .find(|line| {
            line.starts_with(name)
                && line[name.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
        })
        .unwrap_or_else(|| panic!("missing row {name:?} in:\n{report}"))
}

fn result<'a>(report: &'a str, name: &str) -> &'a str {
    row(report, name)[name.len()..]
        .split_whitespace()
        .next()
        .expect("row result")
}

struct FakeRig {
    address: String,
    connections: Arc<AtomicUsize>,
    closed_connections: Arc<AtomicUsize>,
    commands: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeRig {
    fn new(answer: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rigctld");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("local address").to_string();
        let connections = Arc::new(AtomicUsize::new(0));
        let closed_connections = Arc::new(AtomicUsize::new(0));
        let commands = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let connections = Arc::clone(&connections);
            let closed_connections = Arc::clone(&closed_connections);
            let commands = Arc::clone(&commands);
            let stop = Arc::clone(&stop);
            let answer = answer.to_owned();
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::SeqCst);
                            record_connection(stream, &commands, &stop, &answer);
                            closed_connections.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("accept fake rigctld: {error}"),
                    }
                }
            })
        };
        Self {
            address,
            connections,
            closed_connections,
            commands,
            stop,
            thread: Some(thread),
        }
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("commands lock").clone()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn wait_for_close(&self) {
        for _ in 0..200 {
            if self.closed_connections.load(Ordering::SeqCst) == self.connections() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fake rigctld connection did not close");
    }
}

fn record_connection(
    mut stream: TcpStream,
    commands: &Mutex<Vec<String>>,
    stop: &AtomicBool,
    answer: &str,
) {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("fake read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake stream"));
    loop {
        let mut command = String::new();
        match reader.read_line(&mut command) {
            Ok(0) => break,
            Ok(_) => {
                commands
                    .lock()
                    .expect("commands lock")
                    .push(command.trim_end_matches(['\r', '\n']).to_owned());
                stream.write_all(answer.as_bytes()).expect("fake response");
                stream.write_all(b"\n").expect("fake response newline");
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(error) => panic!("read fake command: {error}"),
        }
    }
}

impl Drop for FakeRig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread
            .take()
            .expect("fake thread")
            .join()
            .expect("join fake rigctld");
    }
}

#[test]
fn top_level_help_lists_doctor() {
    let output = run(&["--help"]);
    let report = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert!(report.contains("doctor"), "{report}");
    assert!(
        report.contains("Check audio devices and rig control without transmitting"),
        "{report}"
    );
}

#[test]
fn doctor_prints_every_check_in_order_and_its_exit_matches_its_failures() {
    let rig = FakeRig::new("14074000");
    let output = run(&["doctor", "--rigctld", &rig.address]);
    let report = stdout(&output);
    assert!(
        report.starts_with("Checking audio devices and rig control. Nothing is transmitted and PTT is never keyed.\n"),
        "{report}"
    );
    assert!(report.contains("Audio devices ("), "{report}");
    assert!(
        report.contains("CHECK               RESULT  DETAIL"),
        "{report}"
    );
    let positions: Vec<_> = ROWS
        .iter()
        .map(|name| {
            let line = row(&report, name);
            assert!(
                matches!(result(&report, name), "PASS" | "FAIL" | "SKIP"),
                "{line}"
            );
            report.find(line).expect("row position")
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{report}"
    );
    assert!(report.contains("Summary: "), "{report}");
    let failed = ROWS.iter().any(|name| result(&report, name) == "FAIL");
    assert_eq!(
        output.status.code(),
        Some(if failed { 1 } else { 0 }),
        "{report}\n{}",
        stderr(&output)
    );
}

#[test]
fn doctor_reads_the_dial_frequency_and_sends_nothing_else() {
    let rig = FakeRig::new("14074000");
    let output = run(&["doctor", "--rigctld", &rig.address]);
    let report = stdout(&output);
    assert_eq!(result(&report, "rigctld connection"), "PASS", "{report}");
    assert_eq!(result(&report, "rig CAT read"), "PASS", "{report}");
    assert!(
        row(&report, "rig CAT read").contains("dial 14.074000 MHz"),
        "{report}"
    );
    assert_eq!(rig.connections(), 1);
    rig.wait_for_close();
    assert_eq!(rig.commands(), ["f"]);
}

#[test]
fn an_unanswering_rig_fails_cat_read_and_gets_no_ptt_command() {
    let rig = FakeRig::new("RPRT -5");
    let output = run(&["doctor", "--rigctld", &rig.address]);
    let report = stdout(&output);
    assert_eq!(result(&report, "rig CAT read"), "FAIL", "{report}");
    assert!(row(&report, "rig CAT read").contains("RPRT -5"), "{report}");
    assert_eq!(
        output.status.code(),
        Some(1),
        "{report}\n{}",
        stderr(&output)
    );
    assert_eq!(rig.connections(), 1);
    rig.wait_for_close();
    assert_eq!(rig.commands(), ["f"]);
}

#[test]
fn unreachable_rigctld_fails_with_exit_1_and_an_error_line() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let address = listener.local_addr().expect("port address").to_string();
    drop(listener);
    let output = run(&["doctor", "--rigctld", &address]);
    let report = stdout(&output);
    assert_eq!(result(&report, "rigctld connection"), "FAIL", "{report}");
    assert_eq!(result(&report, "rig CAT read"), "SKIP", "{report}");
    assert_eq!(
        output.status.code(),
        Some(1),
        "{report}\n{}",
        stderr(&output)
    );
    let error = stderr(&output);
    assert!(
        error.contains("Error: ") && error.contains("checks failed"),
        "{error}"
    );
}

#[test]
fn no_rigctld_skips_both_rig_checks_without_connecting() {
    let rig = FakeRig::new("14074000");
    let output = run(&["doctor", "--no-rigctld"]);
    let report = stdout(&output);
    for name in ["rigctld connection", "rig CAT read"] {
        assert_eq!(result(&report, name), "SKIP", "{report}");
        assert!(row(&report, name).contains("--no-rigctld"), "{report}");
    }
    assert_eq!(rig.connections(), 0);
    assert!(rig.commands().is_empty());
}

#[test]
fn rigctld_and_no_rigctld_conflict() {
    let output = run(&["doctor", "--rigctld", "127.0.0.1:4532", "--no-rigctld"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_unmatched_device_name_fails_its_rows_and_never_falls_back() {
    let output = run(&[
        "doctor",
        "--device",
        "NONEXISTENT_DEVICE_XYZ_12345",
        "--no-rigctld",
    ]);
    let report = stdout(&output);
    for direction in ["input", "output"] {
        let device_row = format!("{direction} device");
        let format_row = format!("{direction} 48 kHz mono");
        assert_eq!(result(&report, &device_row), "FAIL", "{report}");
        assert!(
            row(&report, &device_row).contains(&format!(
                "no {direction} device name contains \"NONEXISTENT_DEVICE_XYZ_12345\""
            )),
            "{report}"
        );
        assert_eq!(result(&report, &format_row), "SKIP", "{report}");
    }
    assert_eq!(
        output.status.code(),
        Some(1),
        "{report}\n{}",
        stderr(&output)
    );
}

#[cfg(feature = "cpal-backend")]
#[test]
fn the_audio_backend_row_passes_in_a_cpal_build() {
    let output = run(&["doctor", "--no-rigctld"]);
    let report = stdout(&output);
    assert_eq!(result(&report, "audio backend"), "PASS", "{report}");
}

#[cfg(not(feature = "cpal-backend"))]
#[test]
fn the_audio_backend_row_fails_with_a_rebuild_hint_in_a_default_build() {
    let output = run(&["doctor", "--no-rigctld"]);
    let report = stdout(&output);
    assert_eq!(result(&report, "audio backend"), "FAIL", "{report}");
    assert!(
        row(&report, "audio backend").contains("built without the cpal-backend feature"),
        "{report}"
    );
    assert!(
        report.contains("To fix:") && report.contains("--features cpal-backend"),
        "{report}"
    );
}
