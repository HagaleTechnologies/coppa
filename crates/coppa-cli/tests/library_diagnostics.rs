//! COP-51: a library diagnostic reaches the user as a levelled tracing line
//! on stderr, never on stdout and never with colour codes when stderr is not
//! a terminal. The vehicle is `coppa_audio::RawPcmReader`'s warning for raw
//! PCM on stdin that ends mid-sample (COP-86); `WavSink`'s drop-time error,
//! the earlier vehicle, no longer fires once `stop()` has reported a failure.
#![cfg(feature = "file-backend")]

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn a_truncated_raw_pcm_stream_is_a_warn_line_on_stderr() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_coppa"))
        .args(["rx", "-"])
        .env_remove("NO_COLOR")
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .env(
            "COPPA_CONFIG",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/empty-coppad.toml"
            ),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run coppa");
    // One whole s16le sample, then one byte of the next.
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(b"abc").expect("write stdin");
    drop(stdin);
    let result = child.wait_with_output().expect("wait for coppa");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(
        !result.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("WARN") && line.contains("mid-sample")),
        "no WARN mid-sample line on stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("mid-sample"),
        "diagnostic on stdout:\n{stdout}"
    );
    assert!(
        !stderr.contains('\u{1b}'),
        "ANSI escape on stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Error: no frames decoded from stdin"),
        "the exit report is gone:\n{stderr}"
    );
}
