//! COP-51: a library diagnostic (here `WavSink`'s drop-time flush failure)
//! reaches the user as a levelled tracing line on stderr, never on stdout and
//! never with colour codes when stderr is not a terminal.
#![cfg(feature = "file-backend")]

use std::path::PathBuf;
use std::process::Command;

/// A per-process temp directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("coppa-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_wav_flush_failure_is_an_error_line_on_stderr() {
    let dir = TempDir::new("library-diagnostics");
    let output = dir.0.join("missing-dir").join("x.wav");
    let result = Command::new(env!("CARGO_BIN_EXE_coppa"))
        .args(["tx", "hello", "--callsign", "VK2ABC", "--output"])
        .arg(&output)
        .env_remove("NO_COLOR")
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("run coppa");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(
        !result.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("ERROR") && line.contains("WavSink")),
        "no ERROR WavSink line on stderr:\n{stderr}"
    );
    assert!(!stdout.contains("WavSink"), "WavSink on stdout:\n{stdout}");
    assert!(
        !stderr.contains('\u{1b}'),
        "ANSI escape on stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Error: Failed to create WAV file"),
        "the exit report is gone:\n{stderr}"
    );
}
