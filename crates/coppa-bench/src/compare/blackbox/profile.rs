//! Declarative description of an unmodified modem binary.

use super::native::NativeFormat;
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
    /// COP-221: the format the modem opens its ALSA device in. Optional for
    /// serde so that `load` can name a missing key and its supported values.
    #[serde(default)]
    pub audio: Audio,
}

/// A profile's `[audio]` table, checked by [`Audio::native`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub format: Option<String>,
    pub rate: Option<u32>,
    pub channels: Option<u32>,
}

impl Audio {
    pub fn native(&self) -> Result<NativeFormat> {
        NativeFormat::declared(self.format.as_deref(), self.rate, self.channels)
    }
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
        profile
            .audio
            .native()
            .with_context(|| format!("checking {}", path.display()))?;
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
            let profile = Profile::load(
                &super::super::super::repo_root().join(format!("tools/compare/{name}.toml")),
            )
            .unwrap();
            // AC3: every coppad profile declares what coppad opens.
            assert_eq!(profile.audio.native().unwrap(), NativeFormat::CHANNEL);
        }
    }

    /// Replaces the profile's `[audio]` table with `audio`. A Windows
    /// checkout carries CRLF, so the source is normalized to LF first.
    fn splice_audio(source: &str, audio: &str) -> String {
        let source = source.replace("\r\n", "\n");
        let start = source.find("\n[audio]\nformat").unwrap();
        let end = start + 1 + source[start + 1..].find("\n\n").unwrap();
        format!("{}{}{}", &source[..start], audio, &source[end..])
    }

    #[test]
    fn audio_splice_survives_a_crlf_checkout() {
        let source = std::fs::read_to_string(
            super::super::super::repo_root().join("tools/compare/coppad.toml"),
        )
        .unwrap()
        .replace("\r\n", "\n");
        let table = "\n[audio]\nformat = \"f32le\"\nrate = 48000\nchannels = 1";
        assert!(source.contains(table), "coppad.toml lost its [audio] table");
        let crlf = source.replace('\n', "\r\n");
        assert_eq!(splice_audio(&crlf, "\n"), source.replace(table, "\n"));
    }

    fn load_with_audio(name: &str, audio: &str) -> String {
        let source = std::fs::read_to_string(
            super::super::super::repo_root().join("tools/compare/coppad.toml"),
        )
        .unwrap();
        let source = splice_audio(&source, audio);
        let dir = std::env::temp_dir().join(format!("coppa-profile-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");
        std::fs::write(&path, source).unwrap();
        let result = Profile::load(&path);
        let _ = std::fs::remove_dir_all(&dir);
        format!("{:#}", result.unwrap_err())
    }

    /// AC2: a profile with no format, or one the cable cannot carry, is
    /// refused at load with the key and the values it accepts.
    #[test]
    fn undeclared_or_unsupported_audio_is_refused_by_key() {
        let error = load_with_audio("undeclared", "\n");
        assert!(error.contains("audio.format is undeclared"), "{error}");
        assert!(error.contains("supported values: f32le, s16le"), "{error}");
        let error = load_with_audio(
            "unsupported",
            "\n[audio]\nformat = \"u8\"\nrate = 48000\nchannels = 1\n",
        );
        assert!(
            error.contains("audio.format \"u8\" is unsupported"),
            "{error}"
        );
        assert!(error.contains("supported values: f32le, s16le"), "{error}");
        let error = load_with_audio(
            "rate",
            "\n[audio]\nformat = \"s16le\"\nrate = 11025\nchannels = 1\n",
        );
        assert!(error.contains("audio.rate 11025 is unsupported"), "{error}");
        assert!(
            error.contains("supported values: 8000, 12000, 16000, 24000, 48000"),
            "{error}"
        );
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
            audio: Audio::default(),
        };
        assert!(p
            .verify_binary()
            .unwrap_err()
            .to_string()
            .contains("sha256 mismatch"));
    }
}
