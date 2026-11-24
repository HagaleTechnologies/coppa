//! Declarative description of an unmodified modem binary.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub adapter: String,
    pub binary: PathBuf,
    #[serde(default)]
    pub sha256: String,
    pub mode: String,
    pub args: Vec<String>,
    pub ready_timeout_s: u64,
    pub ports: Ports,
    pub files: Vec<StationFile>,
    pub datagram_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ports {
    pub log_line: String,
    pub command_key: String,
    pub data_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StationFile {
    pub path: String,
    pub contents: String,
}

impl Profile {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let profile: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if profile.name.is_empty() || profile.name.contains('/') || profile.name.contains('\\') {
            bail!("name must be a non-empty filename component");
        }
        if !matches!(profile.adapter.as_str(), "vara" | "ardop" | "kiss") {
            bail!("adapter must be vara, ardop or kiss");
        }
        if profile.ready_timeout_s == 0 {
            bail!("ready_timeout_s must be positive");
        }
        if profile.adapter == "kiss" && profile.datagram_bytes.is_none() {
            bail!("kiss adapter requires datagram_bytes");
        }
        Ok(profile)
    }

    pub fn binary_path(&self) -> PathBuf {
        if self.binary.is_absolute() {
            self.binary.clone()
        } else {
            super::super::repo_root().join(&self.binary)
        }
    }

    pub fn verify_binary(&self) -> Result<String> {
        let path = self.binary_path();
        let output = Command::new("sha256sum")
            .arg(&path)
            .output()
            .with_context(|| format!("hashing {}", path.display()))?;
        if !output.status.success() {
            bail!(
                "sha256sum failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let hash = String::from_utf8(output.stdout)?
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if hash.len() != 64 {
            bail!("sha256sum returned no digest for {}", path.display());
        }
        if !self.sha256.is_empty() && !self.sha256.eq_ignore_ascii_case(&hash) {
            bail!(
                "sha256 mismatch for {}: profile {}, binary {}",
                path.display(),
                self.sha256,
                hash
            );
        }
        Ok(hash)
    }
}

pub struct Substitutions<'a> {
    pub dir: &'a Path,
    pub home: &'a Path,
    pub device: &'a str,
    pub pcm: &'a str,
    pub call: &'a str,
    pub cmd_port: u16,
    pub data_port: u16,
}

pub fn substitute(template: &str, values: &Substitutions<'_>) -> String {
    let mut text = template.to_string();
    for (key, value) in [
        ("{dir}", values.dir.display().to_string()),
        ("{home}", values.home.display().to_string()),
        ("{device}", values.device.to_string()),
        ("{pcm}", values.pcm.to_string()),
        ("{call}", values.call.to_string()),
        ("{cmd_port}", values.cmd_port.to_string()),
        ("{data_port}", values.data_port.to_string()),
    ] {
        text = text.replace(key, &value);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_profiles_parse() {
        for name in ["coppad", "coppad-ardop", "coppad-kiss"] {
            Profile::load(
                &super::super::super::repo_root().join(format!("tools/compare/{name}.toml")),
            )
            .unwrap();
        }
    }

    /// COP-244: KISS datagrams are never resent, so each coppad's receive ring
    /// must outlast a decode on a busy host. 16384 samples (0.34 s) lost a
    /// datagram with 4 busy loops on 2 CPUs; 131072 (2.7 s) did not.
    #[test]
    fn kiss_profile_ring_outlasts_a_contended_decode() {
        let profile =
            Profile::load(&super::super::super::repo_root().join("tools/compare/coppad-kiss.toml"))
                .unwrap();
        let station: toml::Table = toml::from_str(&profile.files[0].contents).unwrap();
        let ring = station["audio"]["buffer_size"].as_integer().unwrap();
        assert!(ring >= 131_072, "coppad-kiss ring {ring} samples");
    }

    #[test]
    fn missing_key_is_named() {
        let error = toml::from_str::<Profile>("name = 'x'")
            .unwrap_err()
            .to_string();
        assert!(error.contains("adapter"), "{error}");
    }

    #[test]
    fn placeholders_substitute() {
        let values = Substitutions {
            dir: Path::new("/tmp/d"),
            home: Path::new("/tmp/h"),
            device: "dev",
            pcm: "pcm",
            call: "N0AAA",
            cmd_port: 42,
            data_port: 43,
        };
        let text = substitute(
            "{dir} {home} {device} {pcm} {call} {cmd_port} {data_port}",
            &values,
        );
        assert_eq!(text, "/tmp/d /tmp/h dev pcm N0AAA 42 43");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sha256_mismatch_is_refused() {
        let p = Profile {
            name: "x".into(),
            adapter: "vara".into(),
            binary: PathBuf::from("/bin/true"),
            sha256: "0".repeat(64),
            mode: "x".into(),
            args: vec![],
            ready_timeout_s: 1,
            ports: Ports {
                log_line: "".into(),
                command_key: "".into(),
                data_key: "".into(),
            },
            files: vec![],
            datagram_bytes: None,
        };
        assert!(p
            .verify_binary()
            .unwrap_err()
            .to_string()
            .contains("sha256 mismatch"));
    }
}
