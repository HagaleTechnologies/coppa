//! COP-85: helpers shared by the daemon integration tests that run the real
//! `coppad` binary (`ptt_release.rs`, `reload.rs`). Each test crate uses a
//! subset, hence the `dead_code` allowance.
#![allow(dead_code)]

use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
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

impl Drop for Daemon {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
