//! Exercise decoded text and byte output through the real tx/rx commands.
#![cfg(feature = "file-backend")]

mod common;

use std::path::Path;
use std::process::Command;

const MESSAGE: &str = "CQ CQ CQ DE VK2ABC K";
const PAYLOAD_HEX: &str = "435120435120435120444520564b32414243204b";

fn run(command: &mut Command) -> String {
    let output = command.output().expect("failed to spawn coppa");
    assert!(
        output.status.success(),
        "coppa failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI stdout should be UTF-8")
}

fn rx(path: &Path, flags: &[&str]) -> String {
    run(common::coppa_command()
        .args(["rx", "-i"])
        .arg(path)
        .args(flags))
}

#[test]
fn rx_text_and_explicit_hex_roundtrip() {
    let path = std::env::temp_dir().join(format!("coppa_rx_text_{}.wav", std::process::id()));
    run(common::coppa_command()
        .args(["tx", MESSAGE, "-o"])
        .arg(&path));

    let text = rx(&path, &[]);
    let hex = rx(&path, &["--hex"]);
    let raw = rx(&path, &["--raw"]);
    let raw_hex = rx(&path, &["--raw", "--hex"]);
    std::fs::remove_file(path).expect("remove temporary WAV");

    assert!(text.contains("Decoded: \"CQ CQ CQ DE VK2ABC K\""), "{text}");
    assert!(!text.contains("435120"), "{text}");
    assert!(
        hex.contains(&format!("Decoded (hex): {PAYLOAD_HEX}")),
        "{hex}"
    );
    assert_eq!(raw.trim(), PAYLOAD_HEX);
    assert_eq!(raw_hex, raw);
}

#[test]
fn rx_binary_payload_falls_back_to_labeled_hex() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/L1_clean.wav");
    let output = rx(&path, &[]);
    assert!(
        output.contains("Decoded (hex): 8639c483ddec99e9"),
        "{output}"
    );
}
