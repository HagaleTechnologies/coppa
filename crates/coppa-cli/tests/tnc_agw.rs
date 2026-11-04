#![cfg(all(feature = "kiss-tnc", not(feature = "cpal-backend")))]
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::Duration,
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_coppa"))
}
fn packet(kind: u8) -> Vec<u8> {
    let mut h = vec![0; 36];
    h[4] = kind;
    h[8..14].copy_from_slice(b"N0CALL");
    h
}
#[test]
fn help_and_bind_conflicts() {
    let output = binary().args(["tnc", "--help"]).output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--agw-port"));
    assert!(help.contains("--bind-address"));
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    for args in [
        vec!["tnc", "--port", &port],
        vec!["tnc", "--port", "0", "--agw-port", &port],
    ] {
        let output = binary().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("Cannot bind"), "{error}");
        assert!(!error.contains("TNC ready."));
    }
}
#[test]
fn cli_registration_and_version_without_audio() {
    // A port reservation is released immediately before startup. Readiness is
    // signalled by stderr, rather than a delay or repeated socket guesses.
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    drop(held);
    let mut p = Process(
        binary()
            .args([
                "tnc",
                "--port",
                "0",
                "--agw-port",
                &port.to_string(),
                "--bind-address",
                "127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "info")
            .spawn()
            .unwrap(),
    );
    let stderr = p.0.stderr.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let logs = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let mut log = String::new();
    loop {
        let line = recv
            .recv_timeout(Duration::from_secs(10))
            .expect("TNC startup");
        let ready = line.contains("TNC ready.");
        log.push_str(&line);
        if ready {
            break;
        }
    }
    assert!(log.contains("AGWPE server listening"));
    assert!(log.contains("nothing will be sent or received"));
    let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    for (kind, expected) in [(b'R', vec![213, 7, 0, 0, 127, 0, 0, 0]), (b'X', vec![1])] {
        client.write_all(&packet(kind)).unwrap();
        let mut h = [0; 36];
        client.read_exact(&mut h).unwrap();
        assert_eq!(h[4], kind);
        let mut b = vec![0; u32::from_le_bytes(h[28..32].try_into().unwrap()) as usize];
        client.read_exact(&mut b).unwrap();
        assert_eq!(b, expected);
    }
    p.0.kill().unwrap();
    p.0.wait().unwrap();
    logs.join().unwrap();
}
