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
    assert!(help.contains("Enable the AGWPE TCP server on this port (normally 8000)"));
    assert!(help.contains("Bind the KISS and AGWPE TCP servers to this address"));
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    for (args, context) in [
        (vec!["tnc", "--port", &port], "(--port)"),
        (
            vec!["tnc", "--port", "0", "--agw-port", &port],
            "(--agw-port)",
        ),
    ] {
        let output = binary().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("Cannot bind"), "{error}");
        assert!(error.contains(context), "{error}");
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
/// COP-137 phase 3: without `--agw-port` the TNC starts KISS only, as it
/// did before, and opens no AGWPE listener.
#[test]
fn kiss_only_startup_is_unchanged_without_agw_port() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    drop(held);
    let mut p = Process(
        binary()
            .args(["tnc", "--port", &port.to_string()])
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
    assert!(log.contains("KISS server listening"), "{log}");
    assert!(!log.contains("AGWPE"), "{log}");
    TcpStream::connect(("127.0.0.1", port)).expect("KISS port accepts");
    p.0.kill().unwrap();
    p.0.wait().unwrap();
    logs.join().unwrap();
}
/// COP-164: the AGWPE `G` and `g` replies describe the modem `--baud`
/// selects, so a 9600 TNC reports a G3RUH 9600 port and baud code 3, and
/// the 1200 default keeps COP-137's bytes.
#[test]
fn agw_port_replies_report_the_configured_baud() {
    fn start_tnc(baud: &str, agw_port: u16) -> (Process, std::thread::JoinHandle<()>) {
        let mut p = Process(
            binary()
                .args([
                    "tnc",
                    "--baud",
                    baud,
                    "--port",
                    "0",
                    "--agw-port",
                    &agw_port.to_string(),
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
        loop {
            let line = recv
                .recv_timeout(Duration::from_secs(10))
                .expect("TNC startup");
            if line.contains("TNC ready.") {
                break;
            }
        }
        (p, logs)
    }
    for (baud, port_text, code) in [
        ("9600", &b"1;Port1 coppa G3RUH 9600;\0"[..], 3u8),
        ("1200", &b"1;Port1 coppa AFSK 1200;\0"[..], 0u8),
    ] {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        drop(held);
        let (mut p, logs) = start_tnc(baud, port);
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut capabilities = vec![0; 12];
        capabilities[0] = code;
        capabilities[1] = 0xFF;
        for (kind, expected) in [(b'G', port_text.to_vec()), (b'g', capabilities)] {
            client.write_all(&packet(kind)).unwrap();
            let mut h = [0; 36];
            client.read_exact(&mut h).unwrap();
            assert_eq!(h[4], kind, "--baud {baud}");
            let mut b = vec![0; u32::from_le_bytes(h[28..32].try_into().unwrap()) as usize];
            client.read_exact(&mut b).unwrap();
            assert_eq!(b, expected, "--baud {baud}, request {}", kind as char);
        }
        p.0.kill().unwrap();
        p.0.wait().unwrap();
        logs.join().unwrap();
    }
}
