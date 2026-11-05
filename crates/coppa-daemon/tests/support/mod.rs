//! COP-85: helpers shared by the daemon integration tests that run the real
//! `coppad` binary (`ptt_release.rs`, `reload.rs`, `runtime_smoke.rs`): a
//! recording fake rigctld, temporary directories, free ports and a daemon
//! runner. Each test crate uses a subset, hence the `dead_code` allowance.
//! Only `process_alive` and `send_signal` are Unix-only.
#![allow(dead_code)]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Poll `pred` every 10 ms until it holds or `timeout` passes.
pub fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if pred() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Check that `pred` stays false for the whole of `period`.
pub fn stays_false(period: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < period {
        if pred() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    !pred()
}

/// A uniquely named directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "coppad-test-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two distinct free TCP ports on loopback, picked by the OS.
pub fn free_ports() -> (u16, u16) {
    let a = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    let b = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    (
        a.local_addr().unwrap().port(),
        b.local_addr().unwrap().port(),
    )
}

/// Remove ANSI escape sequences (tracing's colour codes) from `text`.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether `pid` is a live process. A zombie (exited, not yet reaped by
/// its new parent) counts as gone.
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the parenthesised command name.
        return match stat.rfind(')') {
            Some(i) => !stat[i + 1..].trim_start().starts_with('Z'),
            None => true,
        };
    }
    if Path::new("/proc/self/stat").exists() {
        // procfs is mounted and has no entry for `pid`.
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
pub fn send_signal(signal: &str, target: &str) {
    let status = Command::new("kill")
        .args([signal, "--", target])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {signal} -- {target} failed");
}

/// A running `coppad` on a config file the test owns, with stdout and
/// stderr captured to files in its own temporary directory. Killed and
/// reaped on drop.
pub struct Daemon {
    child: Child,
    dir: TempDir,
    config_path: PathBuf,
    reaped: bool,
}

impl Daemon {
    /// Write `config` to `coppad.toml` in a new temporary directory, start
    /// `coppad --config` on it and wait for `Daemon ready`.
    pub fn start(tag: &str, config: &str, ready: Duration) -> Self {
        let dir = TempDir::new(tag);
        let config_path = dir.path().join("coppad.toml");
        std::fs::write(&config_path, config).expect("write coppad.toml");
        let stdout = File::create(dir.path().join("stdout")).expect("create stdout capture");
        let stderr = File::create(dir.path().join("stderr")).expect("create stderr capture");
        let child = Command::new(env!("CARGO_BIN_EXE_coppad"))
            .arg("--config")
            .arg(&config_path)
            .current_dir(dir.path())
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("spawn coppad");
        let mut daemon = Self {
            child,
            dir,
            config_path,
            reaped: false,
        };
        daemon.wait_for_log("Daemon ready", ready);
        daemon
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Replace the config file the daemon started from.
    pub fn rewrite_config(&self, config: &str) {
        std::fs::write(&self.config_path, config).expect("rewrite coppad.toml");
    }

    /// Everything coppad has printed so far, without colour codes.
    pub fn output(&self) -> String {
        let read =
            |name: &str| std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default();
        strip_ansi(&format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}",
            read("stdout"),
            read("stderr")
        ))
    }

    /// How many times `needle` appears in the output so far.
    pub fn count(&self, needle: &str) -> usize {
        self.output().matches(needle).count()
    }

    /// Wait until the output holds `needle` at least `times` times.
    pub fn wait_for_count(&mut self, needle: &str, times: usize, timeout: Duration) {
        let start = Instant::now();
        loop {
            if self.count(needle) >= times {
                return;
            }
            if let Some(status) = self.try_exit() {
                panic!(
                    "coppad exited with {status} before logging {needle:?} {times} times\n{}",
                    self.output()
                );
            }
            if start.elapsed() >= timeout {
                panic!(
                    "coppad did not log {needle:?} {times} times within {timeout:?}\n{}",
                    self.output()
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_for_log(&mut self, needle: &str, timeout: Duration) {
        self.wait_for_count(needle, 1, timeout);
    }

    pub fn try_exit(&mut self) -> Option<ExitStatus> {
        let status = self.child.try_wait().expect("poll coppad");
        if status.is_some() {
            self.reaped = true;
        }
        status
    }

    pub fn wait_exit(&mut self, timeout: Duration) -> ExitStatus {
        let mut status = None;
        wait_until(timeout, || {
            status = self.try_exit();
            status.is_some()
        });
        status
            .unwrap_or_else(|| panic!("coppad did not exit within {timeout:?}\n{}", self.output()))
    }
}

impl Daemon {
    /// Kill coppad alone (SIGKILL on Unix, TerminateProcess on Windows)
    /// and reap it.
    pub fn kill(&mut self) {
        self.child.kill().expect("kill coppad");
        self.child.wait().expect("reap coppad");
        self.reaped = true;
    }

    /// The guardian's pid, from `PTT guardian started pid=<n>`.
    pub fn guardian_pid(&self) -> Option<u32> {
        const STARTED: &str = "PTT guardian started pid=";
        let output = self.output();
        let rest = &output[output.find(STARTED)? + STARTED.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

/// Kill a left-over process by pid, ignoring errors: `kill -KILL` on Unix,
/// `taskkill /F /PID` on Windows.
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.args(["-KILL", &pid.to_string()]);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.args(["/F", "/PID", &pid.to_string()]);
        command
    };
    #[cfg(any(unix, windows))]
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let guardian = self.guardian_pid();
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // A failing test must not leak the guardian either. A guardian that
        // already exited is a harmless no-op here.
        if let Some(pid) = guardian {
            kill_pid(pid);
        }
    }
}

/// Connect to a loopback port, retrying for up to 5 s.
pub fn connect(port: u16) -> TcpStream {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = None;
    wait_until(Duration::from_secs(5), || {
        stream = TcpStream::connect(addr).ok();
        stream.is_some()
    });
    stream.unwrap_or_else(|| panic!("could not connect to {addr}"))
}

// ---------------------------------------------------------------------------
// Fake rigctld
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connect,
    Cmd(String),
    Eof,
}

/// One logged event: which connection, what happened, and the rig's PTT
/// state (`0`/`1`) right after it.
#[derive(Debug, Clone)]
pub struct Record {
    conn: usize,
    event: Event,
    ptt: u8,
}

#[derive(Default)]
pub struct RigState {
    ptt: u8,
    connections: usize,
    log: Vec<Record>,
}

/// A rigctld stand-in that accepts any number of connections, answers the
/// commands `RigctldClient` sends, and records everything. PTT state is
/// shared across connections, as it is on a real rig.
pub struct FakeRigctld {
    addr: SocketAddr,
    state: Arc<Mutex<RigState>>,
    stop: Arc<AtomicBool>,
}

impl FakeRigctld {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rigctld");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake rigctld listener");
        let addr = listener.local_addr().expect("fake rigctld address");
        let state = Arc::new(Mutex::new(RigState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let state = state.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // BSD/macOS accepted sockets inherit the
                            // listener's O_NONBLOCK; serve() needs blocking
                            // reads or WouldBlock reads as EOF.
                            stream
                                .set_nonblocking(false)
                                .expect("blocking fake rigctld stream");
                            let conn = {
                                let mut s = state.lock().unwrap();
                                let conn = s.connections;
                                s.connections += 1;
                                let ptt = s.ptt;
                                s.log.push(Record {
                                    conn,
                                    event: Event::Connect,
                                    ptt,
                                });
                                conn
                            };
                            let state = state.clone();
                            thread::spawn(move || serve(conn, stream, &state));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
        }
        Self { addr, state, stop }
    }

    pub fn address(&self) -> String {
        self.addr.to_string()
    }

    pub fn ptt(&self) -> u8 {
        self.state.lock().unwrap().ptt
    }

    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    pub fn log(&self) -> Vec<Record> {
        self.state.lock().unwrap().log.clone()
    }

    pub fn events(&self, conn: usize) -> Vec<Event> {
        self.log()
            .into_iter()
            .filter(|r| r.conn == conn)
            .map(|r| r.event)
            .collect()
    }

    /// The PTT commands (`T 0`/`T 1`) connection `conn` sent, in order.
    pub fn ptt_commands(&self, conn: usize) -> Vec<String> {
        self.events(conn)
            .into_iter()
            .filter_map(|e| match e {
                Event::Cmd(c) if c.starts_with("T ") => Some(c),
                _ => None,
            })
            .collect()
    }

    /// Whether any connection other than the daemon's own (connection 0)
    /// has sent `T 0`.
    pub fn another_connection_released(&self) -> bool {
        self.log()
            .iter()
            .any(|r| r.conn > 0 && r.event == Event::Cmd("T 0".to_string()))
    }

    pub fn has_eof(&self, conn: usize) -> bool {
        self.events(conn).contains(&Event::Eof)
    }

    /// The rig's PTT state when connection `conn` closed.
    pub fn ptt_at_eof(&self, conn: usize) -> Option<u8> {
        self.log()
            .into_iter()
            .find(|r| r.conn == conn && r.event == Event::Eof)
            .map(|r| r.ptt)
    }
}

impl Drop for FakeRigctld {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(conn: usize, stream: TcpStream, state: &Mutex<RigState>) {
    let mut writer = stream.try_clone().expect("clone fake rigctld stream");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).unwrap_or(0);
        let mut s = state.lock().unwrap();
        if n == 0 {
            let ptt = s.ptt;
            s.log.push(Record {
                conn,
                event: Event::Eof,
                ptt,
            });
            return;
        }
        let cmd = line.trim().to_string();
        let reply = if cmd == "f" {
            "14074000\n".to_string()
        } else if cmd == "t" {
            format!("{}\n", s.ptt)
        } else if let Some(value) = cmd.strip_prefix("T ") {
            s.ptt = u8::from(value.trim() != "0");
            "RPRT 0\n".to_string()
        } else if cmd == "m" {
            "USB\n2400\n".to_string()
        } else {
            "RPRT 0\n".to_string()
        };
        let ptt = s.ptt;
        s.log.push(Record {
            conn,
            event: Event::Cmd(cmd),
            ptt,
        });
        drop(s);
        if writer.write_all(reply.as_bytes()).is_err() {
            // The peer is gone; the next read reports the EOF.
            continue;
        }
    }
}
