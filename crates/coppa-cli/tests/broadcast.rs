#![cfg(feature = "kiss-tnc")]
use coppa_host::kiss::{kiss_encode_data, KissDecoder, KissFrame};
use coppa_protocol::broadcast_file::encode_file;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "coppa-broadcast-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn cli() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_coppa"));
    c.env("NO_COLOR", "1");
    c
}
fn stderr(out: &Output) -> String {
    assert!(out.stdout.is_empty());
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn sends_repeated_packets_and_exact_status() {
    let dir = Scratch::new();
    std::fs::write(dir.file("hello.txt"), b"hello").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut wire = Vec::new();
        s.read_to_end(&mut wire).unwrap();
        let frames = KissDecoder::new().push(&wire);
        assert_eq!(frames.len(), 3);
        for f in frames {
            assert!(matches!(f, KissFrame::Data(p) if p == encode_file(b"hello").unwrap()[0]));
        }
    });
    let out = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "send",
            "hello.txt",
            "--kiss",
            &addr.to_string(),
            "--repeat",
            "3",
            "--interval-ms",
            "1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "Submitting 5 bytes in 1 chunk per copy to KISS.\nSubmitted 3 copies to KISS; delivery is unconfirmed.\n");
    peer.join().unwrap();
}

#[test]
fn receive_split_coalesced_binary_empty_quiet_and_no_return_data() {
    for (data, quiet) in [
        (vec![0xc0; 900], false),
        (vec![], true),
        (b"hello".to_vec(), false),
    ] {
        let dir = Scratch::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let wire: Vec<u8> = encode_file(&data)
            .unwrap()
            .iter()
            .flat_map(|p| kiss_encode_data(p))
            .collect();
        let peer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            s.write_all(&wire[..7]).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            s.write_all(&wire[7..]).unwrap();
            let mut b = [0; 1];
            assert_eq!(s.read(&mut b).unwrap(), 0, "receiver transmitted");
        });
        let mut c = cli();
        if quiet {
            c.arg("--quiet");
        }
        let out = c
            .current_dir(&dir.0)
            .args([
                "broadcast",
                "receive",
                "--kiss",
                &addr.to_string(),
                "--output",
                "received.txt",
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(std::fs::read(dir.file("received.txt")).unwrap(), data);
        if quiet {
            assert_eq!(stderr(&out), "");
        } else {
            assert_eq!(stderr(&out), format!("Waiting for broadcast file chunks.\nReceived {} bytes into received.txt; checksum verified.\n", data.len()));
        }
        peer.join().unwrap();
    }
}

#[test]
fn refusal_bounds_parser_and_socket_failure() {
    let dir = Scratch::new();
    std::fs::write(dir.file("existing"), b"keep").unwrap();
    std::fs::File::create(dir.file("large"))
        .unwrap()
        .set_len(1_048_577)
        .unwrap();
    for (args, message) in [
        (
            vec!["receive", "--output", "existing"],
            "broadcast output already exists: existing",
        ),
        (
            vec!["send", "large"],
            "broadcast file exceeds 1048576 bytes",
        ),
        (vec!["send", "existing", "--repeat", "0"], "invalid value"),
        (
            vec!["send", "existing", "--interval-ms", "0"],
            "invalid value",
        ),
        (
            vec!["receive", "--output", "out", "--idle-timeout", "0"],
            "invalid value",
        ),
    ] {
        let out = cli()
            .current_dir(&dir.0)
            .arg("broadcast")
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(stderr(&out).contains(message), "{}", stderr(&out));
    }
    assert_eq!(std::fs::read(dir.file("existing")).unwrap(), b"keep");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let out = cli()
        .args([
            "broadcast",
            "receive",
            "--output",
            dir.file("out").to_str().unwrap(),
            "--kiss",
            &addr.to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Error:"));
    assert!(!dir.file("out").exists());
}

#[test]
fn duplicate_traffic_does_not_extend_timeout() {
    let dir = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let p = kiss_encode_data(&encode_file(&vec![1; 800]).unwrap()[0]);
        for _ in 0..15 {
            if s.write_all(&p).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let out = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "receive",
            "--output",
            "out",
            "--kiss",
            &addr.to_string(),
            "--idle-timeout",
            "1",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("broadcast incomplete: no new chunks for 1 seconds"));
    assert!(!dir.file("out").exists());
    peer.join().unwrap();
}

#[cfg(unix)]
#[test]
fn ctrl_c_receive_exits_130_without_output() {
    let dir = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "receive",
            "--output",
            "out",
            "--kiss",
            &listener.local_addr().unwrap().to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (_socket, _) = listener.accept().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    assert_eq!(child.wait().unwrap().code(), Some(130));
    assert!(!dir.file("out").exists());
}

#[test]
fn empty_send_quiet_and_peer_close() {
    let dir = Scratch::new();
    std::fs::write(dir.file("empty"), []).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut wire = Vec::new();
        socket.read_to_end(&mut wire).unwrap();
        assert_eq!(wire, kiss_encode_data(&encode_file(b"").unwrap()[0]));
    });
    let out = cli()
        .current_dir(&dir.0)
        .args([
            "--quiet",
            "broadcast",
            "send",
            "empty",
            "--kiss",
            &addr.to_string(),
            "--repeat",
            "1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(stderr(&out), "");
    peer.join().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        drop(listener.accept().unwrap());
    });
    let out = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "receive",
            "--output",
            "out",
            "--kiss",
            &addr.to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("connection closed before completion"));
    assert!(!dir.file("out").exists());
    peer.join().unwrap();
}

#[cfg(unix)]
#[test]
fn ctrl_c_sender_does_not_claim_delivery() {
    let dir = Scratch::new();
    std::fs::write(dir.file("hello"), b"hello").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let child = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "send",
            "hello",
            "--kiss",
            &listener.local_addr().unwrap().to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut socket, _) = listener.accept().unwrap();
    let mut wire = [0; 4096];
    assert!(socket.read(&mut wire).unwrap() > 0);
    assert!(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    assert!(!stderr(&out).contains("Submitted"));
}

#[test]
fn excessive_timeout_returns_error_without_panicking() {
    let dir = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buffer = [0; 1];
        assert_eq!(socket.read(&mut buffer).unwrap(), 0);
    });
    let out = cli()
        .current_dir(&dir.0)
        .args([
            "broadcast",
            "receive",
            "--output",
            "out",
            "--kiss",
            &addr.to_string(),
            "--idle-timeout",
            "18446744073709551615",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("timeout exceeds clock range"));
    assert!(!dir.file("out").exists());
    peer.join().unwrap();
}
