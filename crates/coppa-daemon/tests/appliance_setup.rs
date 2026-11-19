#![cfg(all(target_os = "linux", feature = "cpal-backend", feature = "websocket"))]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    child: std::process::Child,
    port: u16,
    dashboard_port: u16,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "coppa-appliance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir_all(root.join("cred")).unwrap();
        fs::write(
            root.join("cred/setup-token"),
            "0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();
        let reserve = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let dashboard_port = reserve.local_addr().unwrap().port();
        drop(reserve);
        let mut child = Command::new(env!("CARGO_BIN_EXE_coppad"))
            .arg("--appliance")
            .env("STATE_DIRECTORY", root.join("state"))
            .env("CREDENTIALS_DIRECTORY", root.join("cred"))
            .env("COPPA_APPLIANCE_ADMIN_CONFIG", root.join("absent.toml"))
            .env("COPPA_APPLIANCE_SETUP_PORT", "0")
            .env("COPPA_APPLIANCE_DASHBOARD_PORT", dashboard_port.to_string())
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(value) = line.split("addr=127.0.0.1:").nth(1) {
                    if let Ok(port) = value.trim().parse() {
                        let _ = tx.send(port);
                        continue;
                    }
                }
            }
        });
        let port = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("setup listener address");
        Self {
            root,
            child,
            port,
            dashboard_port,
        }
    }

    fn request(&self, request: &str) -> String {
        http(self.port, request)
    }
}

fn http(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fresh_appliance_serves_setup_without_audio() {
    let fixture = Fixture::new();
    let page = fixture.request(&format!(
        "GET /setup HTTP/1.0\r\nHost: 127.0.0.1:{}\r\n\r\n",
        fixture.port
    ));
    assert!(page.starts_with("HTTP/1.0 200"), "{page}");
    assert!(page.contains("Set up Coppa"));
    assert!(!fixture.root.join("state/coppad.toml").exists());
}

#[test]
fn setup_requests_enforce_access_and_limits() {
    let fixture = Fixture::new();
    let denied = fixture.request(&format!(
        "GET /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\n\r\n",
        fixture.port
    ));
    assert!(denied.starts_with("HTTP/1.0 401"), "{denied}");
    let foreign = fixture.request("GET /api/devices HTTP/1.0\r\nHost: evil.example\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\n\r\n");
    assert!(foreign.starts_with("HTTP/1.0 403"), "{foreign}");
    let allowed = fixture.request(&format!("GET /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\n\r\n", fixture.port));
    assert!(allowed.starts_with("HTTP/1.0 200"), "{allowed}");
    let bad_origin = fixture.request(&format!("PUT /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nOrigin: http://evil.example\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}", fixture.port));
    assert!(bad_origin.starts_with("HTTP/1.0 403"), "{bad_origin}");
    let oversized = "x".repeat(8193);
    let large = fixture.request(&format!("PUT /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", fixture.port, fixture.port, oversized.len(), oversized));
    assert!(large.starts_with("HTTP/1.0 413"), "{large}");
    let unknown = r#"{"callsignn":"K1ABC"}"#;
    let invalid = fixture.request(&format!("PUT /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", fixture.port, fixture.port, unknown.len(), unknown));
    assert!(invalid.contains("\"field\":\"callsignn\""), "{invalid}");
    assert!(!fixture.root.join("state/coppad.toml").exists());
}

#[test]
fn save_starts_modem_and_restart_reuses_config() {
    let mut fixture = Fixture::new();
    let body = r#"{"callsign":"K1ABC","grid":"FN20","input_device_id":"alsa:null","output_device_id":"alsa:null","ptt":{"method":"none"},"beacon_interval_secs":0}"#;
    let response = fixture.request(&format!(
        "PUT /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        fixture.port, fixture.port, body.len(), body
    ));
    assert!(response.starts_with("HTTP/1.0 200"), "{response}");
    let config = fs::read_to_string(fixture.root.join("state/coppad.toml")).unwrap();
    assert!(config.contains("K1ABC"));
    assert!(config.contains("alsa:null"));
    assert!(config.contains("websocket_enabled = true"));
    let health = || {
        for _ in 0..100 {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", fixture.dashboard_port)) {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                stream
                    .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
                    .unwrap();
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_ok() && response.contains("200 OK") {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("dashboard health did not become available");
    };
    health();
    fixture.child.kill().unwrap();
    fixture.child.wait().unwrap();
    fixture.child = Command::new(env!("CARGO_BIN_EXE_coppad"))
        .arg("--appliance")
        .env("STATE_DIRECTORY", fixture.root.join("state"))
        .env("CREDENTIALS_DIRECTORY", fixture.root.join("cred"))
        .env(
            "COPPA_APPLIANCE_ADMIN_CONFIG",
            fixture.root.join("absent.toml"),
        )
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    health();
}

#[test]
fn failed_or_concurrent_saves_keep_config_consistent() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let body = r#"{"callsign":"K1ABC","grid":"","input_device_id":"alsa:null","output_device_id":"alsa:null","ptt":{"method":"none"},"beacon_interval_secs":0}"#;
    let request = format!("PUT /api/config HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nAuthorization: Bearer 0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", fixture.port, fixture.port, body.len(), body);
    let state = fixture.root.join("state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    let failed = fixture.request(&request);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.starts_with("HTTP/1.0 500"), "{failed}");
    assert!(!state.join("coppad.toml").exists());
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| http(fixture.port, &request));
        let b = scope.spawn(|| http(fixture.port, &request));
        (a.join().unwrap(), b.join().unwrap())
    });
    let successes = [a.as_str(), b.as_str()]
        .iter()
        .filter(|r| r.starts_with("HTTP/1.0 200"))
        .count();
    let conflicts = [a.as_str(), b.as_str()]
        .iter()
        .filter(|r| r.starts_with("HTTP/1.0 409"))
        .count();
    assert_eq!((successes, conflicts), (1, 1), "{a} {b}");
    assert!(state.join("coppad.toml").exists());
}

#[test]
fn legacy_config_takes_precedence() {
    let mut fixture = Fixture::new();
    fixture.child.kill().unwrap();
    fixture.child.wait().unwrap();
    fs::write(fixture.root.join("state/coppad.toml"), "this is not TOML").unwrap();
    let admin = fixture.root.join("admin.toml");
    fs::write(
        &admin,
        format!(
            r#"
[audio]
input_device_id = "alsa:null"
output_device_id = "alsa:null"
[engine]
callsign = "K1ABC"
[host]
vara_enabled = false
websocket_enabled = true
websocket_port = {}
websocket_token = "0123456789abcdef0123456789abcdef"
websocket_allowed_origins = ["http://127.0.0.1:{}"]
"#,
            fixture.dashboard_port, fixture.dashboard_port
        ),
    )
    .unwrap();
    fixture.child = Command::new(env!("CARGO_BIN_EXE_coppad"))
        .arg("--appliance")
        .env("STATE_DIRECTORY", fixture.root.join("state"))
        .env("CREDENTIALS_DIRECTORY", fixture.root.join("cred"))
        .env("COPPA_APPLIANCE_ADMIN_CONFIG", &admin)
        .env("NO_COLOR", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut healthy = false;
    for _ in 0..100 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", fixture.dashboard_port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            stream
                .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            healthy = stream.read_to_string(&mut response).is_ok() && response.contains("200 OK");
            if healthy {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        healthy,
        "admin config should start despite invalid managed TOML"
    );
}
