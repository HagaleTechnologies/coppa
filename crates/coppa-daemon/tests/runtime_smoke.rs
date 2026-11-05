//! COP-85: run the real `coppad` binary on every OS CI has, Windows
//! included. No signals, no process groups and no `/proc`: the daemon is
//! stopped with `Child::kill` (SIGKILL on Unix, TerminateProcess on
//! Windows).
//!
//! Skipped with `cpal-backend`, as `ptt_release.rs` is: that build opens the
//! host's default audio device, which a CI runner may not have.
#![cfg(not(feature = "cpal-backend"))]
// COP-142: the std-clock ban in clippy.toml targets event_loop only.
#![allow(clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

mod support;
use support::{connect, free_ports, wait_until, Daemon, FakeRigctld};

const READY_DEADLINE: Duration = Duration::from_secs(60);
const KEY_DEADLINE: Duration = Duration::from_secs(30);
const GUARDIAN_DEADLINE: Duration = Duration::from_secs(10);

/// Send `command` on a VARA command connection and return the first reply
/// line that starts with `prefix`.
fn reply(stream: &mut TcpStream, command: &str, prefix: &str) -> String {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    stream
        .write_all(format!("{command}\r").as_bytes())
        .expect("send VARA command");
    let start = Instant::now();
    let mut received = Vec::new();
    let mut buf = [0u8; 512];
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&received);
        if let Some(line) = text.split('\r').find(|l| l.starts_with(prefix)) {
            return line.to_string();
        }
    }
    panic!(
        "no {prefix:?} reply to {command:?}: {:?}",
        String::from_utf8_lossy(&received)
    );
}

fn vara_config(ports: (u16, u16), radio: &str) -> String {
    format!(
        "[radio]\n\
         {radio}\n\
         \n\
         [host]\n\
         vara_enabled = true\n\
         vara_command_port = {}\n\
         vara_data_port = {}\n\
         \n\
         [engine]\n\
         callsign = \"N0CALL\"\n",
        ports.0, ports.1
    )
}

#[test]
fn coppad_starts_answers_the_vara_command_port_and_stops() {
    let ports = free_ports();
    let mut daemon = Daemon::start(
        "smoke",
        &vara_config(ports, "ptt_method = \"none\""),
        READY_DEADLINE,
    );
    let mut stream = connect(ports.0);
    assert!(reply(&mut stream, "VERSION", "VERSION ").starts_with("VERSION Coppa "));
    assert_eq!(reply(&mut stream, "MYCALL N1CALL", "OK"), "OK");
    daemon.kill();
    assert!(daemon.try_exit().is_some());
}

/// COP-31's SIGKILL case, here so it runs on Windows too: a killed coppad
/// leaves rigctld PTT keyed, and the guardian releases it on a connection of
/// its own.
#[test]
fn a_killed_coppad_has_its_rigctld_ptt_released_by_the_guardian() {
    let fake = FakeRigctld::start();
    let ports = free_ports();
    let radio = format!(
        "ptt_method = \"rigctld\"\n\
         rigctld_address = \"{}\"\n\
         ptt_pre_delay_ms = 0\n\
         ptt_guardian = true",
        fake.address()
    );
    let mut daemon = Daemon::start(
        "smoke-guardian",
        &vara_config(ports, &radio),
        READY_DEADLINE,
    );
    assert!(daemon.guardian_pid().is_some(), "{}", daemon.output());

    let mut stream = connect(ports.0);
    stream.write_all(b"TUNE 20\r").expect("send TUNE");
    assert!(
        wait_until(KEY_DEADLINE, || fake
            .ptt_commands(0)
            .contains(&"T 1".to_string())),
        "TUNE did not key PTT; rigctld log {:?}\n{}",
        fake.log(),
        daemon.output()
    );

    daemon.kill();
    assert!(
        wait_until(GUARDIAN_DEADLINE, || fake.another_connection_released()),
        "no second rigctld connection sent T 0 within {GUARDIAN_DEADLINE:?}; log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    assert_eq!(fake.ptt(), 0, "rigctld log {:?}", fake.log());
}

/// H-064: the `serial-ptt` build fails startup cleanly on a port that does
/// not exist, naming it.
#[cfg(feature = "serial-ptt")]
#[test]
fn serial_ptt_on_a_missing_port_fails_startup_naming_the_port() {
    let port = if cfg!(windows) {
        "COM250"
    } else {
        "/dev/coppa-no-such-port"
    };
    let dir = support::TempDir::new("smoke-serial");
    let config = dir.path().join("coppad.toml");
    std::fs::write(
        &config,
        format!("[radio]\nptt_method = \"serial:{port}:dtr\"\n"),
    )
    .expect("write coppad.toml");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_coppad"))
        .arg("--config")
        .arg(&config)
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .expect("run coppad");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("failed to open serial PTT port") && stderr.contains(port),
        "{stderr}"
    );
}
