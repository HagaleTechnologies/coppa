//! Keep the sysop recipe in step with the runnable experiment.
use std::path::PathBuf;
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn block(heading: &str, language: &str) -> String {
    let doc = std::fs::read_to_string(root().join("docs/BPQ.md")).unwrap();
    let section = doc.split(heading).nth(1).unwrap();
    section
        .split(&format!("```{language}\n"))
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap()
        .into()
}
#[test]
fn bpq_doc_carries_the_harness_config_verbatim() {
    assert_eq!(
        block("## Configure LinBPQ", "text"),
        std::fs::read_to_string(root().join("tools/bpq-loop/bpq32.cfg")).unwrap()
    );
}
#[test]
fn bpq_doc_coppad_config_passes_check() {
    let dir = std::env::temp_dir().join(format!("coppa-bpq-doc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("coppad.toml");
    std::fs::write(&config, block("## Configure coppad", "toml")).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_coppad"))
        .arg("--config")
        .arg(config)
        .arg("--check")
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn bpq_doc_lists_every_command_coppa_accepts_from_linbpq() {
    let doc = std::fs::read_to_string(root().join("docs/BPQ.md")).unwrap();
    for command in [
        "MYCALL",
        "BW2300",
        "RETRIES",
        "LISTEN ON",
        "LISTEN OFF",
        "CONNECT",
        "DISCONNECT",
        "ABORT",
        "BUSY",
    ] {
        assert!(doc.contains(command), "{command}");
    }
}
