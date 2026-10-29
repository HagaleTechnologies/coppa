#![allow(dead_code)]
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn coppa_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coppa"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("COPPA_") {
            command.env_remove(name);
        }
    }
    command.env(
        "COPPA_CONFIG",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/empty-coppad.toml"),
    );
    command
}

pub struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("coppa_cli_settings_{}_{}", std::process::id(), id));
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
