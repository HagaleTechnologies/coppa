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
    /// Busy-channel courtesy / station-ID / beacon-mode configuration
    /// (Phase 4 Task 3).
    pub station_id: StationIdConfig,
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
    /// Station callsign. Required (non-empty) for the station-ID timer and
    /// beacon mode (Phase 4 Task 3) to activate; empty (the default) keeps
    /// both off regardless of their interval settings.
    pub callsign: String,
    /// Optional free-text Maidenhead grid locator (e.g. "FN20"), included in
    /// the station-ID/beacon payload alongside the callsign when set. No
    /// validation beyond being a plain string -- this is an operator-supplied
    /// field, not a parsed/validated grid square.
    pub grid: Option<String>,
    /// Enable ARQ (Automatic Repeat reQuest) transport layer.
    pub arq_enabled: bool,
    /// Frames between `coppa_ml::RateLoop` active overshoot probes (see
    /// `RateLoop::with_probing`). `0` (the default) disables probing entirely --
    /// an explicit opt-in. Only takes effect when `arq_enabled` is also
    /// `true` -- probing needs the ARQ ACK/timeout feedback loop to attribute a
    /// probe's outcome. See `docs/superpowers/specs/
    /// 2026-07-25-rateloop-daemon-probe-wiring-design.md`.
    pub rate_loop_probe_interval: u32,
    /// Index-steps up the speed-level ladder a probe reaches for (see
    /// `RateLoop::with_probing`'s `probe_offset`). Ignored when
    /// `rate_loop_probe_interval == 0`.
    pub rate_loop_probe_offset: usize,
    /// Enable `coppa_ml::CpGate`'s live spread-gated short-CP recommendation.
    /// `false` (the default) is an explicit opt-in, matching
    /// `rate_loop_probe_interval`'s convention. When enabled, every fully
    /// decoded frame's measured delay spread feeds a live `CpGate`, and its
    /// current recommendation is exposed via the WebSocket `status` reply's
    /// `short_cp_ok` field. **This does NOT switch the engine's CP profile
    /// automatically** -- it is measurement/telemetry only; actually
    /// switching `CoppaProfile` mid-session needs the peer-negotiation
    /// handshake in `coppa_protocol::cp_negotiator`, gated separately by
    /// `cp_negotiation_enabled` (see that field's own doc). See
    /// `docs/superpowers/specs/2026-07-25-cpgate-daemon-wiring-design.md`
    /// for the full reasoning behind this module's own telemetry-only scope.
    ///
    /// Reverse cross-reference (COP-2): `cp_negotiation_enabled` has no
    /// **proposer** without this flag. The only code that can send a `Propose`
    /// lives inside this gate, so with this off a negotiation-enabled station
    /// can only ever *answer* a peer -- never start one.
    pub cp_gate_enabled: bool,
    /// Enable the CP-switch peer-negotiation handshake (see
    /// `coppa_protocol::cp_negotiator` and `docs/superpowers/specs/
    /// 2026-07-29-cp-switch-peer-negotiation-design.md`). `false` (the
    /// default) is an explicit opt-in, matching `cp_gate_enabled`'s and
    /// `rate_loop_probe_interval`'s convention exactly.
    ///
    /// **Turning live CP negotiation on is a conjunction of THREE flags, not
    /// this one** (COP-2 -- promoted from a parenthetical, because reading it
    /// as one flag is the mistake this doc exists to prevent):
    ///
    /// - `cp_negotiation_enabled` gates *acting on* the handshake at all: the
    ///   inbound `TransportType::CpControl` dispatch (`handle_cp_control`), the
    ///   CP-control retransmit loop, and `drive_cp_negotiation`'s give-up
    ///   triggers (COP-1's G1-G4).
    /// - `cp_gate_enabled` gates the only code that can *initiate* one. With it
    ///   off, `CpGate::observe` is never called, so its recommendation can
    ///   never transition and the propose block is structurally unreachable.
    /// - `arq_enabled` gates both the inbound `TransportPdu` parse (the sole
    ///   route to any CpControl PDU) and, redundantly, the propose itself.
    ///   CP-control traffic rides its own dedicated `ArqTx`/`ArqRx` pair, but
    ///   those paths are gated on `arq_enabled` for consistency with the rest
    ///   of this daemon's reliable-delivery machinery.
    ///
    /// So: **this flag alone is never a proposer** without `cp_gate_enabled`,
    /// and **only ever a responder** once `arq_enabled` is on -- a real,
    /// asymmetric behavior change worth deliberate thought, since a peer can
    /// then talk this station onto short CP without it ever asking. Enforced,
    /// not merely documented, by `coppa-daemon/src/event_loop.rs`'s
    /// `cp_negotiation_enabled_alone_never_initiates_without_cp_gate` (which
    /// asserts both halves) and by `test_cp_negotiation_requires_two_more_flags_by_default`
    /// below.
    ///
    /// With all three off (the shipped default) the only live effect of this
    /// subsystem is `drive_cp_negotiation` and the CP-control retransmit loop
    /// running on empty state every 500 ms poll -- both return immediately, so
    /// the cost is a couple of field reads per tick and nothing reaches the
    /// air.
    ///
    /// Both stations must set all three locally: there is no peer-capability
    /// negotiation (see the design doc's "disjoint subsystems" finding for
    /// why), and a station with `cp_negotiation_enabled` off simply never
    /// proposes or acts on `TransportType::CpControl` PDUs.
    pub cp_negotiation_enabled: bool,
}

/// Busy-channel courtesy / station-ID timer / beacon-mode configuration
/// (Phase 4 Task 3). All three sub-features are off by default (see each
/// field's doc); the station-ID timer and beacon mode additionally require
/// `[engine] callsign` to be set (see `EngineSection::callsign`'s doc) --
/// there is no meaningful "identify" or "beacon" transmission without one.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StationIdConfig {
    /// Busy-channel gate: once a TX request finds the channel busy (via
    /// `coppa_ml::BusyGate`), the poll interval (ms) used while waiting for
    /// it to clear. `0` (the default) disables the busy-channel gate
    /// entirely -- TX proceeds immediately regardless of busy state, exactly
    /// as before this feature existed. When non-zero, a TX request that
    /// finds the channel busy is deferred until it reads clear, followed by
    /// a randomized 0.5-2s courtesy holdoff (so multiple deferred stations
    /// don't all key up in the same instant) before actually transmitting.
    /// Does NOT add any delay to a TX request when the channel is already
    /// clear at request time.
    pub busy_hold_ms: u64,
    /// Station-ID timer: minimum interval (seconds) between automatic ID
    /// transmissions. Defaults to 540 (9 minutes) -- FCC Part 97.119 requires
    /// identification at least every 10 minutes; 9 minutes leaves margin
    /// against clock/scheduling jitter. Only actually fires if `[engine]
    /// callsign` is set AND at least one real transmission has happened
    /// since the last ID (an ID is prepended to the next outgoing frame, not
    /// sent standalone on a bare timer -- an idle station never needs to
    /// identify). Note this default alone does NOT enable the feature: with
    /// the default empty callsign, no ID is ever sent regardless of this
    /// value (see `EngineSection::callsign`'s doc) -- this is why "ID timer
    /// off by default" and "id_interval_secs defaults to the FCC-safe 540"
    /// are both true simultaneously.
    pub id_interval_secs: u64,
    /// Beacon mode: interval (seconds) between automatic standalone beacon
    /// transmissions (sent whenever the channel reads clear at the interval
    /// tick; skipped -- not deferred -- if busy, and retried on the next
    /// tick). `0` (the default) disables beacon mode. Also requires
    /// `[engine] callsign` to be set.
    pub beacon_interval_secs: u64,
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
            station_id: StationIdConfig::default(),
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
            grid: None,
            arq_enabled: false,
            rate_loop_probe_interval: 0,
            rate_loop_probe_offset: 0,
            cp_gate_enabled: false,
            cp_negotiation_enabled: false,
        }
    }
}

impl Default for StationIdConfig {
    fn default() -> Self {
        Self {
            busy_hold_ms: 0,
            id_interval_secs: 540,
            beacon_interval_secs: 0,
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
    fn test_rate_loop_probing_defaults_to_disabled() {
        let config = DaemonConfig::default();
        assert_eq!(
            config.engine.rate_loop_probe_interval, 0,
            "active overshoot probing must be off by default"
        );
        assert_eq!(config.engine.rate_loop_probe_offset, 0);
    }

    #[test]
    fn test_rate_loop_probing_toml_override() {
        let toml = r#"
[engine]
rate_loop_probe_interval = 2
rate_loop_probe_offset = 1
"#;
        let config: DaemonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.engine.rate_loop_probe_interval, 2);
        assert_eq!(config.engine.rate_loop_probe_offset, 1);
    }

    #[test]
    fn test_cp_gate_defaults_to_disabled() {
        let config = DaemonConfig::default();
        assert!(
            !config.engine.cp_gate_enabled,
            "CpGate daemon wiring must be off by default"
        );
    }

    #[test]
    fn test_cp_gate_toml_override() {
        let toml = r#"
[engine]
cp_gate_enabled = true
"#;
        let config: DaemonConfig = toml::from_str(toml).unwrap();
        assert!(config.engine.cp_gate_enabled);
    }

    #[test]
    fn test_cp_negotiation_defaults_to_disabled() {
        let config = DaemonConfig::default();
        assert!(
            !config.engine.cp_negotiation_enabled,
            "CP negotiation must stay off by default -- the flag alone never initiates \
             (needs `cp_gate_enabled` to propose and `arq_enabled` to receive), so \
             flipping it would mislead operators without changing behavior. See COP-2."
        );
    }

    /// The tripwire that makes a future *partial* flip impossible to land
    /// silently: enabling CP negotiation for real is a **conjunction of three
    /// flags**, so a change that flips one of them and calls the feature
    /// "enabled" is wrong, and must break here rather than in the field.
    /// Asserted on a single `DaemonConfig::default()` deliberately -- three
    /// separate one-flag tests can all pass while the conjunction is broken.
    /// See `cp_negotiation_enabled`'s field doc for what each of the three
    /// gates, and `coppa-daemon/src/event_loop.rs`'s
    /// `cp_negotiation_enabled_alone_never_initiates_without_cp_gate` for the
    /// enforcing behavioral test.
    #[test]
    fn test_cp_negotiation_requires_two_more_flags_by_default() {
        let config = DaemonConfig::default();
        assert!(
            !config.engine.arq_enabled
                && !config.engine.cp_gate_enabled
                && !config.engine.cp_negotiation_enabled,
            "live CP negotiation is the CONJUNCTION arq_enabled && cp_gate_enabled && \
             cp_negotiation_enabled; all three must default off, and no partial flip \
             may land without revisiting COP-2's flip gate (arq_enabled={}, \
             cp_gate_enabled={}, cp_negotiation_enabled={})",
            config.engine.arq_enabled,
            config.engine.cp_gate_enabled,
            config.engine.cp_negotiation_enabled
        );
    }

    #[test]
    fn test_cp_negotiation_toml_override() {
        let toml = r#"
[engine]
cp_negotiation_enabled = true
"#;
        let config: DaemonConfig = toml::from_str(toml).unwrap();
        assert!(config.engine.cp_negotiation_enabled);
    }

    // ── StationIdConfig defaults (Phase 4 Task 3) ─────────────────────

    #[test]
    fn test_station_id_config_defaults() {
        let config = DaemonConfig::default();
        assert_eq!(
            config.station_id.busy_hold_ms, 0,
            "busy-channel gate must be off by default"
        );
        assert_eq!(
            config.station_id.id_interval_secs, 540,
            "station-ID interval should default to 9 minutes (FCC margin)"
        );
        assert_eq!(
            config.station_id.beacon_interval_secs, 0,
            "beacon mode must be off by default"
        );
        assert_eq!(
            config.engine.callsign, "",
            "callsign must be unset by default"
        );
        assert_eq!(config.engine.grid, None);
    }

    #[test]
    fn test_parse_station_id_toml() {
        let toml = r#"
[engine]
callsign = "VK2ABC"
grid = "QF22"

[station_id]
busy_hold_ms = 250
id_interval_secs = 300
beacon_interval_secs = 600
"#;
        let config: DaemonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.engine.grid.as_deref(), Some("QF22"));
        assert_eq!(config.station_id.busy_hold_ms, 250);
        assert_eq!(config.station_id.id_interval_secs, 300);
        assert_eq!(config.station_id.beacon_interval_secs, 600);
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
