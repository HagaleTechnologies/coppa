//! COP-85: SIGHUP reloads the configuration of the real `coppad` binary.
//!
//! Each test owns its VARA ports (picked by the OS) and its temporary
//! directory, so the tests run in parallel. Unix only (SIGHUP), and skipped
//! with `cpal-backend` for the same reason as `ptt_release.rs`.
#![cfg(all(unix, not(feature = "cpal-backend")))]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

mod support;
use support::{connect, free_ports, send_signal, Daemon};

const READY_DEADLINE: Duration = Duration::from_secs(30);
const LOG_DEADLINE: Duration = Duration::from_secs(10);
const EXIT_DEADLINE: Duration = Duration::from_secs(5);

const RELOADED: &str = "Configuration reloaded";
const RELOAD_FAILED: &str =
    "Configuration reload failed; coppad keeps running on its current settings";

/// A config with VARA on `ports`, no PTT, and the given extra lines in
/// `[radio]` and `[engine]`.
fn config(ports: (u16, u16), radio: &str, engine: &str) -> String {
    format!(
        "[radio]\n\
         ptt_method = \"none\"\n\
         {radio}\n\
         \n\
         [host]\n\
         vara_enabled = true\n\
         vara_command_port = {}\n\
         vara_data_port = {}\n\
         \n\
         [engine]\n\
         callsign = \"N0CALL\"\n\
         {engine}\n",
        ports.0, ports.1
    )
}

fn sighup(daemon: &Daemon) {
    send_signal("-HUP", &daemon.pid().to_string());
}

/// Send `VERSION` on the VARA command port and return the reply line.
fn version_reply(port: u16) -> String {
    let mut stream = connect(port);
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    stream.write_all(b"VERSION\r").expect("send VERSION");
    let start = Instant::now();
    let mut received = Vec::new();
    let mut buf = [0u8; 512];
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&received);
        if let Some(line) = text.split('\r').find(|l| l.starts_with("VERSION ")) {
            return line.to_string();
        }
    }
    panic!(
        "no VERSION reply on port {port}: {:?}",
        String::from_utf8_lossy(&received)
    );
}

/// The first output line that contains `needle`.
fn line_with(daemon: &Daemon, needle: &str) -> String {
    let output = daemon.output();
    output
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?}\n{output}"))
        .to_string()
}

#[test]
fn sighup_applies_live_settings_and_coppad_keeps_running() {
    let ports = free_ports();
    let mut daemon = Daemon::start("reload-live", &config(ports, "", ""), READY_DEADLINE);

    let edited = config(ports, "ptt_tail_delay_ms = 300\nturnaround_ms = 400", "")
        .replace("callsign = \"N0CALL\"", "callsign = \"N1CALL\"");
    daemon.rewrite_config(&edited);
    sighup(&daemon);
    daemon.wait_for_log(&format!("{RELOADED} applied="), LOG_DEADLINE);

    let applied = line_with(&daemon, &format!("{RELOADED} applied="));
    for key in [
        "[engine] callsign",
        "[radio] ptt_tail_delay_ms",
        "[radio] turnaround_ms",
    ] {
        assert!(applied.contains(key), "{key} missing: {applied}");
    }
    assert!(
        daemon
            .output()
            .contains("Local callsign set by configuration reload callsign=N1CALL"),
        "{}",
        daemon.output()
    );
    assert!(
        daemon
            .output()
            .contains("Received SIGHUP, reloading configuration"),
        "{}",
        daemon.output()
    );
    assert!(daemon.try_exit().is_none(), "{}", daemon.output());
    assert!(version_reply(ports.0).starts_with("VERSION "));

    send_signal("-TERM", &daemon.pid().to_string());
    let status = daemon.wait_exit(EXIT_DEADLINE);
    assert_eq!(status.code(), Some(0), "{}", daemon.output());
}

#[test]
fn sighup_with_an_invalid_file_keeps_the_running_settings() {
    let ports = free_ports();
    let mut daemon = Daemon::start("reload-invalid", &config(ports, "", ""), READY_DEADLINE);

    daemon.rewrite_config(&config(ports, "", "profile = \"HF_STANDRD\""));
    sighup(&daemon);
    daemon.wait_for_count(RELOAD_FAILED, 1, LOG_DEADLINE);
    assert!(
        line_with(&daemon, RELOAD_FAILED).contains("is not a known profile"),
        "{}",
        daemon.output()
    );
    assert!(daemon.try_exit().is_none(), "{}", daemon.output());

    daemon.rewrite_config(&config(ports, "", "").replace("\"none\"", "\"bogus\""));
    sighup(&daemon);
    daemon.wait_for_count(RELOAD_FAILED, 2, LOG_DEADLINE);
    assert!(
        daemon.output().contains("invalid [radio] ptt_method"),
        "{}",
        daemon.output()
    );
    assert!(daemon.try_exit().is_none(), "{}", daemon.output());

    daemon.rewrite_config(&config(ports, "ptt_tail_delay_ms = 250", ""));
    sighup(&daemon);
    daemon.wait_for_log(&format!("{RELOADED} applied="), LOG_DEADLINE);
    assert!(
        line_with(&daemon, &format!("{RELOADED} applied=")).contains("[radio] ptt_tail_delay_ms"),
        "{}",
        daemon.output()
    );
    assert!(version_reply(ports.0).starts_with("VERSION "));
}

#[test]
fn sighup_names_restart_only_changes_and_keeps_the_old_listener() {
    let ports = free_ports();
    let mut daemon = Daemon::start("reload-restart", &config(ports, "", ""), READY_DEADLINE);

    let (other, _) = free_ports();
    let moved = config(ports, "", "").replace(
        &format!("vara_command_port = {}", ports.0),
        &format!("vara_command_port = {other}"),
    );
    daemon.rewrite_config(&moved);
    sighup(&daemon);
    let warning = "Configuration reload kept the running values of settings that change only \
                   on restart";
    daemon.wait_for_log(warning, LOG_DEADLINE);
    let line = line_with(&daemon, warning);
    assert!(line.contains("WARN"), "{line}");
    assert!(
        line.contains("restart_required=[host] vara_command_port"),
        "{line}"
    );
    assert!(version_reply(ports.0).starts_with("VERSION "));
}

#[test]
fn sighup_with_an_unchanged_file_reports_no_changes() {
    let ports = free_ports();
    let mut daemon = Daemon::start("reload-same", &config(ports, "", ""), READY_DEADLINE);

    let unchanged = format!("{RELOADED}; no settings changed");
    sighup(&daemon);
    daemon.wait_for_count(&unchanged, 1, LOG_DEADLINE);
    sighup(&daemon);
    daemon.wait_for_count(&unchanged, 2, LOG_DEADLINE);
    assert!(daemon.try_exit().is_none(), "{}", daemon.output());
}

/// COP-85: a SIGHUP after a stop request does not reload, and does not
/// kill the daemon either; the orderly stop still exits 0.
#[test]
fn sighup_during_shutdown_is_ignored() {
    let ports = free_ports();
    let mut daemon = Daemon::start("reload-stop", &config(ports, "", ""), READY_DEADLINE);
    send_signal("-TERM", &daemon.pid().to_string());
    sighup(&daemon);
    let status = daemon.wait_exit(EXIT_DEADLINE);
    assert_eq!(status.code(), Some(0), "{}", daemon.output());
}
