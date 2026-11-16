//! Launch a modem without modifying its executable or source.

use super::profile::{substitute, Profile, Substitutions};
use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Station {
    child: Child,
    log: PathBuf,
    pub command_addr: String,
    pub data_addr: String,
}

impl Station {
    pub fn launch(profile: &Profile, dir: &Path, home: &Path, side: char) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let (device, pcm, call) = if side == 'A' {
            ("coppa-cmp-A", "coppa_cmp_a", "N0AAA")
        } else {
            ("coppa-cmp-B", "coppa_cmp_b", "N0BBB")
        };
        let values = Substitutions {
            dir,
            home,
            device,
            pcm,
            call,
            cmd_port: 0,
            data_port: 0,
        };
        let sub = |s: &str| substitute(s, &values);
        for f in &profile.files {
            let path = PathBuf::from(sub(&f.path));
            anyhow::ensure!(
                !path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
                "profile file path may not contain '..': {}",
                path.display()
            );
            anyhow::ensure!(
                path.starts_with(dir),
                "profile file path escapes station directory: {}",
                path.display()
            );
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, sub(&f.contents))?;
        }
        let log = dir.join("coppad.log");
        let stdout = File::create(&log)?;
        let stderr = stdout.try_clone()?;
        let child = Command::new(profile.binary_path())
            .args(profile.args.iter().map(|s| sub(s)))
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("launching {}", profile.binary.display()))?;
        let mut station = Self {
            child,
            log,
            command_addr: String::new(),
            data_addr: String::new(),
        };
        station.wait_ready(profile)?;
        Ok(station)
    }

    fn wait_ready(&mut self, profile: &Profile) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(profile.ready_timeout_s);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "modem exited {status}: {}",
                    fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            let text = fs::read_to_string(&self.log)?;
            if let Some(line) = text.lines().find(|l| l.contains(&profile.ports.log_line)) {
                let cmd = field(line, &profile.ports.command_key)
                    .ok_or_else(|| anyhow::anyhow!("missing command address in {line}"))?;
                let data = field(line, &profile.ports.data_key).unwrap_or_else(|| cmd.clone());
                self.command_addr = cmd;
                self.data_addr = data;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!(
            "timed out waiting for {:?}: {}",
            profile.ports.log_line,
            fs::read_to_string(&self.log).unwrap_or_default()
        )
    }
}

impl Drop for Station {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn field(line: &str, key: &str) -> Option<String> {
    let value = line.split(key).nth(1)?.split_whitespace().next()?;
    Some(value.trim_matches(['"', ',', '}']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_field_finds_port() {
        assert_eq!(
            field(
                "command_addr=127.0.0.1:1234 data_addr=127.0.0.1:1235",
                "command_addr="
            )
            .unwrap(),
            "127.0.0.1:1234"
        );
    }
    #[test]
    fn profile_file_cannot_escape_station_scratch() {
        let root = super::super::super::repo_root();
        let mut profile = Profile::load(&root.join("tools/compare/coppad.toml")).unwrap();
        profile.files[0].path = "{dir}/../escape.toml".into();
        let dir = std::env::temp_dir().join(format!("coppa-station-path-{}", std::process::id()));
        let home = dir.join("home");
        let error = Station::launch(&profile, &dir, &home, 'A')
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("may not contain '..'"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
