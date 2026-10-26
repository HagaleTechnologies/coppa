//! Daemon configuration from TOML file.

pub use coppa_radio::band::FrequencyConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level daemon configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Minimum time in ms between the end of a frame this station decodes and
    /// this station keying PTT (half-duplex RX->TX turnaround, COP-60). The
    /// sender stays keyed for its own `ptt_tail_delay_ms` (or its rig's VOX
    /// hang time) after its last sample and cannot hear a reply sent sooner,
    /// so this must exceed that plus the peer rig's TX->RX switching time.
    /// `0` disables the guard.
    pub turnaround_ms: u64,
    /// COP-40: explicit operating (dial) frequency in Hz, a frequency source
    /// for the HF/VHF wideband gate. Only wideband (VHF) profiles need one;
    /// see [`RadioConfig::frequency_config`].
    pub frequency_hz: Option<u64>,
    /// COP-40: amateur band label (`"160m"`..`"23cm"`), a frequency source
    /// for the HF/VHF wideband gate when the exact dial frequency is not
    /// stated.
    pub band: Option<String>,
    /// COP-31: spawn the PTT guardian for rigctld/GPIO PTT (see
    /// `ptt_guardian`): a helper process that releases PTT if coppad dies
    /// while transmitting. Ignored for `none`, `vox` and serial PTT.
    pub ptt_guardian: bool,
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

    /// Validate [`Self::frequency_hz`] and [`Self::band`] into the stated
    /// frequency source the wideband gate reads. A typo'd band label, a zero
    /// or implausible frequency, or a frequency outside the stated band is an
    /// error (a typo never silently falls back to "no source").
    pub fn frequency_config(&self) -> Result<FrequencyConfig, String> {
        FrequencyConfig::parse(
            self.frequency_hz,
            self.band.as_deref(),
            "[radio] frequency_hz",
            "[radio] band",
        )
    }

    /// COP-60: the startup warning for a turnaround that cannot outlast a
    /// peer configured like this station. The peer's tail is unknown here,
    /// so this station's own `ptt_tail_delay_ms` stands in for it, and a
    /// short value only warns: asymmetric setups are legitimate. A long
    /// turnaround costs throughput but never collides, so it never warns.
    pub fn turnaround_warning(&self) -> Option<String> {
        (self.turnaround_ms <= self.ptt_tail_delay_ms).then(|| {
            format!(
                "[radio] turnaround_ms ({}) does not exceed ptt_tail_delay_ms ({}); \
                 a peer configured like this station will still be keyed when \
                 this station answers",
                self.turnaround_ms, self.ptt_tail_delay_ms
            )
        })
    }
}

/// Host interface configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Allow data-port and WebSocket host data to transmit without an established
    /// session. Off by default; intended for sessionless bench rigs. Enabling it
    /// lets any data-port client key the transmitter.
    pub sessionless_tx_enabled: bool,
}

/// Engine configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// With all three off (the shipped default) this subsystem is **structurally
    /// unreachable**, and the flag that makes it so is `arq_enabled` -- not this
    /// one. `check_arq_retransmits` returns at
    /// `crates/coppa-daemon/src/event_loop.rs:1715-1717` on `!arq_enabled`, which is
    /// *before* both the CP-control retransmit block (`:1814`) and the sole
    /// production call to `drive_cp_negotiation` (`:1873`). So on the shipped
    /// default neither COP-1's G1-G4 give-up triggers nor COP-2's desync canary run
    /// at all -- not "run on empty state and return immediately", which is what this
    /// paragraph used to claim. The whole residual cost is the one `bool` read in
    /// `check_arq_retransmits`, and nothing reaches the air.
    ///
    /// The per-tick cost only becomes "a couple of field reads" once `arq_enabled`
    /// *and* this flag are on; that is the condition under which the canary's own
    /// cost note should be read.
    ///
    /// Both stations must set `arq_enabled` and this flag locally: there is no
    /// peer-capability negotiation (see the design doc's "disjoint subsystems"
    /// finding for why), and a station with `cp_negotiation_enabled` off simply
    /// never proposes or acts on `TransportType::CpControl` PDUs.
    ///
    /// Review finding: `cp_gate_enabled` is NOT part of that "both stations"
    /// requirement — it is what makes a station INITIATE a proposal (its
    /// recommendation drives the propose decision). A station with only
    /// `arq_enabled` + this flag on (`cp_gate_enabled` off) still correctly
    /// RESPONDS to a peer's proposal and completes the handshake — see
    /// `cp_negotiation_enabled_alone_never_initiates_without_cp_gate`, which
    /// proves exactly that "alone" case. So an asymmetric deployment is valid:
    /// only the station that should ever initiate needs `cp_gate_enabled` too;
    /// a responder-only station should leave it off.
    pub cp_negotiation_enabled: bool,
}

/// Busy-channel courtesy / station-ID timer / beacon-mode configuration
/// (Phase 4 Task 3). All three sub-features are off by default (see each
/// field's doc); the station-ID timer and beacon mode additionally require
/// `[engine] callsign` to be set (see `EngineSection::callsign`'s doc) --
/// there is no meaningful "identify" or "beacon" transmission without one.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            turnaround_ms: 500,
            frequency_hz: None,
            band: None,
            ptt_guardian: true,
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
            sessionless_tx_enabled: false,
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

    /// COP-32: `[audio] sample_rate` must equal the configured profile's
    /// engine rate. coppad has no resampler, so any other rate plays and
    /// captures audio at the wrong speed. Checked by run mode before any
    /// hardware is touched; `--check` does not call it.
    pub fn check_sample_rate(&self) -> Result<(), String> {
        let engine_rate = coppa_engine::profiles::get_profile(&self.engine.profile).map_or_else(
            || coppa_engine::EngineConfig::default().sample_rate,
            |profile| profile.sample_rate,
        );
        if self.audio.sample_rate == engine_rate {
            return Ok(());
        }
        Err(format!(
            "[audio] sample_rate = {} is not supported: coppad has no resampler and \
             profile {} runs at {engine_rate} Hz; set sample_rate = {engine_rate}",
            self.audio.sample_rate, self.engine.profile
        ))
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
    fn test_default_config_serializes_and_roundtrips() {
        // `coppad --print-default-config` prints exactly this serialization.
        let printed = toml::to_string_pretty(&DaemonConfig::default()).unwrap();
        assert!(
            !printed.contains("grid"),
            "an unset grid is omitted: {printed}"
        );
        let mut grid_set = DaemonConfig::default();
        grid_set.engine.grid = Some("FN20".to_string());
        assert!(toml::to_string_pretty(&grid_set)
            .unwrap()
            .contains("grid = \"FN20\""));

        let parsed: DaemonConfig = toml::from_str(&printed).unwrap();
        assert_eq!(toml::to_string_pretty(&parsed).unwrap(), printed);
        assert_eq!(parsed.audio.sample_rate, 48_000);
        assert_eq!(parsed.radio.ptt_method, "none");
        assert_eq!(parsed.engine.profile, "HF_STANDARD");
        assert_eq!(parsed.engine.grid, None);
        assert_eq!(parsed.station_id.id_interval_secs, 540);
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
    fn test_sessionless_tx_defaults_to_disabled() {
        assert!(!DaemonConfig::default().host.sessionless_tx_enabled);
        let config: DaemonConfig = toml::from_str("[host]\nvara_enabled = true").unwrap();
        assert!(!config.host.sessionless_tx_enabled);
    }

    #[test]
    fn test_sessionless_tx_toml_override() {
        let config: DaemonConfig = toml::from_str("[host]\nsessionless_tx_enabled = true").unwrap();
        assert!(config.host.sessionless_tx_enabled);
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

    // ── COP-40: [radio] frequency_hz / band ─────────────────────────────

    #[test]
    fn radio_defaults_carry_no_frequency_source() {
        let radio = RadioConfig::default();
        assert!(radio.frequency_hz.is_none());
        assert!(radio.band.is_none());
        assert_eq!(
            radio.frequency_config().unwrap(),
            FrequencyConfig::default()
        );
    }

    #[test]
    fn frequency_hz_parses_from_toml() {
        let config: DaemonConfig = toml::from_str("[radio]\nfrequency_hz = 144500000\n").unwrap();
        assert_eq!(config.radio.frequency_hz, Some(144_500_000));
        assert_eq!(
            config.radio.frequency_config().unwrap().stated_hz,
            Some(144_500_000)
        );
    }

    #[test]
    fn band_label_parses_from_toml_case_insensitively() {
        let config: DaemonConfig = toml::from_str("[radio]\nband = \"2M\"\n").unwrap();
        assert_eq!(config.radio.band.as_deref(), Some("2M"));
        let parsed = config.radio.frequency_config().unwrap();
        assert_eq!(parsed.band.unwrap().label, "2m");
    }

    #[test]
    fn band_typo_is_a_hard_error() {
        let radio = RadioConfig {
            band: Some("21m".into()),
            ..RadioConfig::default()
        };
        let err = radio.frequency_config().unwrap_err();
        assert!(err.contains("invalid [radio] band \"21m\""), "{err}");
        assert!(err.contains("expected an amateur band label"), "{err}");
    }

    #[test]
    fn frequency_zero_is_a_hard_error() {
        let radio = RadioConfig {
            frequency_hz: Some(0),
            ..RadioConfig::default()
        };
        let err = radio.frequency_config().unwrap_err();
        assert!(
            err.contains("must be a positive dial frequency in Hz"),
            "{err}"
        );
    }

    #[test]
    fn frequency_above_30_ghz_is_a_hard_error() {
        let radio = RadioConfig {
            frequency_hz: Some(30_000_000_001),
            ..RadioConfig::default()
        };
        let err = radio.frequency_config().unwrap_err();
        assert!(err.contains("[radio] frequency_hz"), "{err}");
    }

    #[test]
    fn contradictory_frequency_and_band_is_a_hard_error() {
        let radio = RadioConfig {
            frequency_hz: Some(14_074_000),
            band: Some("2m".into()),
            ..RadioConfig::default()
        };
        let err = radio.frequency_config().unwrap_err();
        assert!(
            err.contains("[radio] frequency_hz = 14074000 lies outside band \"2m\""),
            "{err}"
        );
    }

    #[test]
    fn consistent_frequency_and_band_is_accepted() {
        let radio = RadioConfig {
            frequency_hz: Some(144_500_000),
            band: Some("2m".into()),
            ..RadioConfig::default()
        };
        let parsed = radio.frequency_config().unwrap();
        assert_eq!(parsed.stated_hz, Some(144_500_000));
        assert_eq!(parsed.band.unwrap().label, "2m");
    }

    #[test]
    fn frequency_config_defaults_to_absent_toml_override() {
        let config: DaemonConfig = toml::from_str("[radio]\nptt_method = \"none\"\n").unwrap();
        assert!(config.radio.frequency_hz.is_none());
        assert!(config.radio.band.is_none());
        let config: DaemonConfig =
            toml::from_str("[radio]\nfrequency_hz = 50313000\nband = \"6m\"\n").unwrap();
        let parsed = config.radio.frequency_config().unwrap();
        assert_eq!(parsed.stated_hz, Some(50_313_000));
        assert_eq!(parsed.band.unwrap().label, "6m");
    }

    #[test]
    fn ptt_guardian_defaults_on_and_can_be_disabled() {
        assert!(RadioConfig::default().ptt_guardian);
        let config: DaemonConfig = toml::from_str("[radio]\nptt_method = \"none\"\n").unwrap();
        assert!(config.radio.ptt_guardian, "absent key keeps the default");
        let config: DaemonConfig = toml::from_str("[radio]\nptt_guardian = false\n").unwrap();
        assert!(!config.radio.ptt_guardian);
    }

    #[test]
    fn test_turnaround_defaults_to_500_ms() {
        assert_eq!(DaemonConfig::default().radio.turnaround_ms, 500);
    }

    #[test]
    fn test_turnaround_toml_override() {
        let config: DaemonConfig = toml::from_str("[radio]\nturnaround_ms = 750").unwrap();
        assert_eq!(config.radio.turnaround_ms, 750);
    }

    #[test]
    fn test_turnaround_warning_when_not_above_the_tail() {
        let short = RadioConfig {
            turnaround_ms: 100,
            ..Default::default()
        };
        let warning = short
            .turnaround_warning()
            .expect("100 ms <= 200 ms tail warns");
        assert!(warning.contains("turnaround_ms (100)"), "{warning}");
        assert!(warning.contains("ptt_tail_delay_ms (200)"), "{warning}");

        let disabled = RadioConfig {
            turnaround_ms: 0,
            ..Default::default()
        };
        assert!(disabled.turnaround_warning().is_some());

        let equal = RadioConfig {
            turnaround_ms: 200,
            ptt_tail_delay_ms: 200,
            ..Default::default()
        };
        assert!(
            equal.turnaround_warning().is_some(),
            "a turnaround equal to the tail leaves no margin"
        );
    }

    #[test]
    fn test_turnaround_warning_absent_at_defaults() {
        assert_eq!(RadioConfig::default().turnaround_warning(), None);
    }

    #[test]
    fn test_example_config_documents_turnaround_under_radio() {
        // Read at runtime rather than `include_str!`, which would make the
        // crate unpackable without the workspace-root example file.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../coppad.toml.example");
        let text = std::fs::read_to_string(path).unwrap();

        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            value["radio"]["turnaround_ms"].as_integer(),
            Some(500),
            "coppad.toml.example must set [radio] turnaround_ms = 500"
        );

        // The key lives only in [radio]: no stray (dead) copy anywhere else,
        // commented or not.
        let mut table = String::new();
        for line in text.lines() {
            let trimmed = line.trim_start_matches('#').trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains(' ') {
                table = trimmed.to_string();
            }
            if trimmed.starts_with("turnaround_ms") {
                assert_eq!(
                    table, "[radio]",
                    "turnaround_ms line outside [radio]: {line:?}"
                );
            }
        }

        let config: DaemonConfig = toml::from_str(&text).unwrap();
        assert_eq!(config.radio.turnaround_ms, 500);
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

    #[test]
    fn check_sample_rate_accepts_the_engine_rate() {
        assert_eq!(DaemonConfig::default().check_sample_rate(), Ok(()));
    }

    #[test]
    fn check_sample_rate_rejects_any_other_rate() {
        let mut config = DaemonConfig::default();
        config.audio.sample_rate = 44_100;
        let err = config.check_sample_rate().unwrap_err();
        assert!(err.contains("sample_rate = 44100"), "{err}");
        assert!(err.contains("48000 Hz"), "{err}");
        assert!(err.contains("HF_STANDARD"), "{err}");
    }

    #[test]
    fn check_sample_rate_uses_the_default_engine_rate_for_an_unknown_profile() {
        let mut config = DaemonConfig::default();
        config.engine.profile = "NOT_A_PROFILE".into();
        assert_eq!(config.check_sample_rate(), Ok(()));
        config.audio.sample_rate = 96_000;
        assert!(config.check_sample_rate().is_err());
    }
}
