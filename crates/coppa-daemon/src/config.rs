//! Daemon configuration from TOML file.

use serde::Deserialize;
use std::path::Path;

/// Top-level daemon configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Audio subsystem configuration.
    pub audio: AudioConfig,
    /// Radio control configuration.
    pub radio: RadioConfig,
    /// Host interface configuration.
    pub host: HostConfig,
    /// Engine configuration.
    pub engine: EngineSection,
}

/// Audio configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Input device name (empty = default).
    pub input_device: String,
    /// Output device name (empty = default).
    pub output_device: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Ring buffer size in samples.
    pub buffer_size: usize,
}

/// Radio control configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RadioConfig {
    /// PTT method/config string. Either a simple backend name --
    /// `"none"`, `"vox"`, `"rigctld"` -- for backward compatibility, or an
    /// extended `method:args` form for backends that need extra parameters:
    /// `"serial:/dev/ttyUSB0:dtr"`, `"serial:/dev/ttyUSB0:rts"`,
    /// `"gpio:17"`. See [`PttConfig::parse`].
    pub ptt_method: String,
    /// rigctld address (e.g., "127.0.0.1:4532").
    pub rigctld_address: String,
    /// Delay in ms after asserting PTT before transmitting audio.
    pub ptt_pre_delay_ms: u64,
    /// Delay in ms after audio ends before releasing PTT.
    pub ptt_tail_delay_ms: u64,
    /// Maximum TX duration in seconds before forced PTT unkey (safety).
    pub max_tx_duration_s: u64,
}

/// Which serial control line a `PttConfig::Serial` should drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttSerialLine {
    Dtr,
    Rts,
}

/// Parsed form of [`RadioConfig::ptt_method`] -- which PTT backend to
/// construct, and any backend-specific parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PttConfig {
    /// No PTT control. Only reachable via an explicit `"none"` (or blank)
    /// config string -- an unrecognized string is a parse error instead, so
    /// a typo never silently falls back to no PTT.
    None,
    Vox,
    Rigctld,
    Serial {
        port: String,
        line: PttSerialLine,
    },
    Gpio {
        pin: String,
    },
}

impl PttConfig {
    /// Parse a `ptt_method` config string.
    ///
    /// Backward-compatible with the pre-Phase-4 flat values (`"none"`,
    /// `"vox"`, `"rigctld"`); `"serial"`/`"gpio"` require the extended
    /// `method:args` syntax since they need extra parameters.
    pub fn parse(s: &str) -> Result<Self, PttConfigError> {
        let trimmed = s.trim();
        let (method, rest) = match trimmed.split_once(':') {
            Some((m, r)) => (m, Some(r)),
            None => (trimmed, None),
        };

        match (method.to_ascii_lowercase().as_str(), rest) {
            ("" | "none", None) => Ok(PttConfig::None),
            ("vox", None) => Ok(PttConfig::Vox),
            ("rigctld", None) => Ok(PttConfig::Rigctld),
            ("serial", Some(rest)) => {
                let (port, line) = rest.rsplit_once(':').ok_or_else(|| {
                    PttConfigError(format!(
                        "serial PTT config {s:?} is missing the line; expected \
                         \"serial:<port>:<dtr|rts>\""
                    ))
                })?;
                let line = match line.to_ascii_lowercase().as_str() {
                    "dtr" => PttSerialLine::Dtr,
                    "rts" => PttSerialLine::Rts,
                    other => {
                        return Err(PttConfigError(format!(
                        "unknown serial PTT line {other:?} in {s:?}; expected \"dtr\" or \"rts\""
                    )))
                    }
                };
                if port.is_empty() {
                    return Err(PttConfigError(format!(
                        "serial PTT config {s:?} is missing the port; expected \
                         \"serial:<port>:<dtr|rts>\""
                    )));
                }
                Ok(PttConfig::Serial {
                    port: port.to_string(),
                    line,
                })
            }
            ("gpio", Some(pin)) => {
                if pin.is_empty() {
                    return Err(PttConfigError(format!(
                        "gpio PTT config {s:?} is missing the pin; expected \"gpio:<pin>\""
                    )));
                }
                Ok(PttConfig::Gpio {
                    pin: pin.to_string(),
                })
            }
            _ => Err(PttConfigError(format!(
                "unknown PTT method {s:?}; expected one of \"none\", \"vox\", \"rigctld\", \
                 \"serial:<port>:<dtr|rts>\", \"gpio:<pin>\""
            ))),
        }
    }
}

/// A `ptt_method` config string failed to parse. `Display`s a
/// human-readable, actionable message suitable for a hard daemon-startup
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PttConfigError(String);

impl std::fmt::Display for PttConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PttConfigError {}

impl RadioConfig {
    /// Parse [`Self::ptt_method`] into a [`PttConfig`].
    pub fn ptt_config(&self) -> Result<PttConfig, PttConfigError> {
        PttConfig::parse(&self.ptt_method)
    }
}

/// Host interface configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// Address all host servers bind to. Defaults to "127.0.0.1" (loopback only).
    ///
    /// WARNING: binding to a non-loopback address (e.g. "0.0.0.0") exposes an
    /// unauthenticated control plane that can key a transmitter to anyone who can
    /// reach this host. Only change this on a trusted, firewalled network.
    pub bind_address: String,
    /// Enable VARA-style TCP control interface (not RF/waveform-compatible with VARA).
    pub vara_enabled: bool,
    /// VARA command port.
    pub vara_command_port: u16,
    /// VARA data port.
    pub vara_data_port: u16,
    /// Enable WebSocket interface.
    pub websocket_enabled: bool,
    /// WebSocket port.
    pub websocket_port: u16,
}

/// Engine configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EngineSection {
    /// Operational profile name.
    pub profile: String,
    /// Station callsign.
    pub callsign: String,
    /// Enable ARQ (Automatic Repeat reQuest) transport layer.
    pub arq_enabled: bool,
}

// Sub-structs have non-trivial defaults (custom port numbers, strings, etc.),
// so we keep explicit Default impls rather than deriving.
#[allow(clippy::derivable_impls)]
impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            radio: RadioConfig::default(),
            host: HostConfig::default(),
            engine: EngineSection::default(),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            output_device: String::new(),
            sample_rate: 48_000,
            buffer_size: 8192,
        }
    }
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            ptt_method: "none".to_string(),
            rigctld_address: "127.0.0.1:4532".to_string(),
            ptt_pre_delay_ms: 50,
            ptt_tail_delay_ms: 200,
            max_tx_duration_s: 30,
        }
    }
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            vara_enabled: false,
            vara_command_port: 8300,
            vara_data_port: 8301,
            websocket_enabled: false,
            websocket_port: 8400,
        }
    }
}

impl Default for EngineSection {
    fn default() -> Self {
        Self {
            profile: "HF_STANDARD".to_string(),
            callsign: String::new(),
            arq_enabled: false,
        }
    }
}

impl DaemonConfig {
    /// Load configuration from a TOML file.
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref())?;
        let config: Self = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Load configuration, falling back to defaults if file doesn't exist.
    ///
    /// Returns an error if the config file exists but has parse errors (E3).
    /// Falls back to defaults only if the file does not exist.
    pub fn load_or_default<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        match Self::load(path.as_ref()) {
            Ok(config) => Ok(config),
            Err(e) => {
                if path.as_ref().exists() {
                    // E3: Config file exists but has errors — this is fatal
                    Err(anyhow::anyhow!(
                        "Failed to parse config {}: {}",
                        path.as_ref().display(),
                        e
                    ))
                } else {
                    Ok(Self::default())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();
        assert_eq!(config.audio.sample_rate, 48_000);
        assert_eq!(config.host.vara_command_port, 8300);
        assert!(!config.host.vara_enabled);
        assert!(!config.host.websocket_enabled);
        assert_eq!(config.radio.ptt_method, "none");
        assert_eq!(config.radio.ptt_tail_delay_ms, 200);
        assert_eq!(config.radio.max_tx_duration_s, 30);
    }

    #[test]
    fn test_parse_toml() {
        let toml = r#"
[audio]
sample_rate = 44100
buffer_size = 4096

[radio]
ptt_method = "rigctld"

[host]
vara_enabled = true
websocket_enabled = true

[engine]
profile = "HF_ROBUST"
callsign = "VK2ABC"
"#;
        let config: DaemonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.audio.sample_rate, 44100);
        assert_eq!(config.radio.ptt_method, "rigctld");
        assert!(config.host.websocket_enabled);
        assert_eq!(config.engine.callsign, "VK2ABC");
    }

    #[test]
    fn test_load_nonexistent_file() {
        let path = std::env::temp_dir().join("nonexistent_coppa_config.toml");
        let config = DaemonConfig::load_or_default(path.to_str().unwrap()).unwrap();
        assert_eq!(config.audio.sample_rate, 48_000);
    }

    // ── PttConfig::parse ──────────────────────────────────────────────

    #[test]
    fn test_ptt_parse_none() {
        assert_eq!(PttConfig::parse("none").unwrap(), PttConfig::None);
        assert_eq!(PttConfig::parse("").unwrap(), PttConfig::None);
        assert_eq!(PttConfig::parse("  none  ").unwrap(), PttConfig::None);
    }

    #[test]
    fn test_ptt_parse_vox_and_rigctld_backward_compatible() {
        assert_eq!(PttConfig::parse("vox").unwrap(), PttConfig::Vox);
        assert_eq!(PttConfig::parse("rigctld").unwrap(), PttConfig::Rigctld);
    }

    #[test]
    fn test_ptt_parse_serial_dtr() {
        assert_eq!(
            PttConfig::parse("serial:/dev/ttyUSB0:dtr").unwrap(),
            PttConfig::Serial {
                port: "/dev/ttyUSB0".to_string(),
                line: PttSerialLine::Dtr,
            }
        );
    }

    #[test]
    fn test_ptt_parse_serial_rts_case_insensitive() {
        assert_eq!(
            PttConfig::parse("serial:/dev/ttyUSB0:RTS").unwrap(),
            PttConfig::Serial {
                port: "/dev/ttyUSB0".to_string(),
                line: PttSerialLine::Rts,
            }
        );
    }

    #[test]
    fn test_ptt_parse_serial_missing_line_errors() {
        let err = PttConfig::parse("serial:/dev/ttyUSB0").unwrap_err();
        assert!(err.to_string().contains("serial:<port>:<dtr|rts>"));
    }

    #[test]
    fn test_ptt_parse_serial_unknown_line_errors() {
        let err = PttConfig::parse("serial:/dev/ttyUSB0:bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn test_ptt_parse_gpio_pin() {
        assert_eq!(
            PttConfig::parse("gpio:17").unwrap(),
            PttConfig::Gpio {
                pin: "17".to_string()
            }
        );
    }

    #[test]
    fn test_ptt_parse_gpio_missing_pin_errors() {
        let err = PttConfig::parse("gpio").unwrap_err();
        assert!(err.to_string().contains("gpio:<pin>"));
    }

    #[test]
    fn test_ptt_parse_unknown_method_errors() {
        let err = PttConfig::parse("carrier-pigeon").unwrap_err();
        assert!(err.to_string().contains("carrier-pigeon"));
        assert!(err.to_string().contains("none"));
    }

    #[test]
    fn test_radio_config_ptt_config_helper() {
        let radio = RadioConfig {
            ptt_method: "gpio:4".to_string(),
            ..RadioConfig::default()
        };
        assert_eq!(
            radio.ptt_config().unwrap(),
            PttConfig::Gpio {
                pin: "4".to_string()
            }
        );
    }

    #[test]
    fn test_load_invalid_config_is_fatal() {
        // E3: Write a file with invalid TOML and verify it returns Err.
        // Process-unique path so parallel test binaries can't race on a shared file.
        let path = std::env::temp_dir().join(format!(
            "coppa_test_invalid_config_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "this is not valid [[[toml").unwrap();
        let result = DaemonConfig::load_or_default(path.to_str().unwrap());
        assert!(
            result.is_err(),
            "Parse error on existing file should be fatal"
        );
        std::fs::remove_file(&path).ok();
    }
}
