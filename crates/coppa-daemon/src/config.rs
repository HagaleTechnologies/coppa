//! Daemon configuration from TOML file.

pub use coppa_radio::band::FrequencyConfig;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// File name `coppad` looks for when no config path is given (COP-49): the
/// first search candidate, relative to the working directory, and the file
/// name inside every config directory.
pub const CONFIG_FILE_NAME: &str = "coppad.toml";

/// System-wide config file, last in the Unix search order (COP-49). The same
/// path COP-48's systemd unit passes explicitly.
pub const SYSTEM_CONFIG_PATH: &str = "/etc/coppa/coppad.toml";

/// Top-level daemon configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    /// Input device name (empty = default).
    pub input_device: String,
    /// Output device name (empty = default).
    pub output_device: String,
    /// Exact host-specific CPAL input device ID (empty = use name or default).
    pub input_device_id: String,
    /// Exact host-specific CPAL output device ID (empty = use name or default).
    pub output_device_id: String,
    /// Zero-based input lane of a multichannel device.
    pub input_channel: u16,
    /// Zero-based output lane; all other output lanes are silent.
    pub output_channel: u16,
    /// Preferred hardware sample rate in Hz; the engine retains its profile rate.
    pub sample_rate: u32,
    /// Ring buffer size in samples; must be greater than 0.
    pub buffer_size: usize,
}

/// Radio control configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

/// A shared secret read from the config file. Never log the value; `Debug`
/// redacts it (COP-53).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretToken(String);

impl SecretToken {
    /// The secret itself, for the one place that must compare it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretToken(<redacted>)")
    }
}

/// Host interface configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    /// Address all host servers bind to. Defaults to "127.0.0.1" (loopback only).
    ///
    /// WARNING: binding to a non-loopback address (e.g. "0.0.0.0") exposes the
    /// VARA-style and ARDOP ports, which have no authentication and can key a
    /// transmitter, to anyone who can reach this host. The WebSocket port needs
    /// `websocket_token`, but over `ws://` the token crosses the network in
    /// clear text. Only change this on a trusted, firewalled network.
    pub bind_address: String,
    /// Enable VARA-style TCP control interface (not RF/waveform-compatible with VARA).
    pub vara_enabled: bool,
    /// VARA command port.
    pub vara_command_port: u16,
    /// VARA data port.
    pub vara_data_port: u16,
    /// COP-97: enable the ARDOP TNC host interface (command + data TCP
    /// ports). A host *protocol* interface: over the air coppa speaks
    /// coppa, so both ends of a session run coppa. Like the VARA ports,
    /// there is no token — any local process can key the transmitter
    /// through it (see docs/ARDOP.md).
    pub ardop_enabled: bool,
    /// COP-97: ARDOP command port. ARDOP clients derive the data port as
    /// command port + 1, so keep that relationship if you change it.
    pub ardop_command_port: u16,
    /// COP-97: ARDOP data port (the ARDOP convention is command port + 1).
    pub ardop_data_port: u16,
    /// Enable WebSocket interface.
    pub websocket_enabled: bool,
    /// WebSocket port.
    pub websocket_port: u16,
    /// COP-53: shared secret every WebSocket handshake must present, as
    /// `Authorization: Bearer <token>` or a `token` query parameter. Required
    /// when `websocket_enabled` is true, whatever `bind_address` is: 16 or
    /// more characters from `A-Z a-z 0-9 - . _ ~`. Keep the config file
    /// readable only by the account running `coppad`.
    pub websocket_token: Option<SecretToken>,
    /// COP-53: exact browser origins (`scheme://host[:port]`) allowed to open
    /// the WebSocket. Empty (the default) refuses every web page; scripts that
    /// send no `Origin` header need only the token.
    pub websocket_allowed_origins: Vec<String>,
    /// Allow data-port and WebSocket host data to transmit without an established
    /// session. Off by default; intended for sessionless bench rigs. Enabling it
    /// lets any data-port client key the transmitter.
    pub sessionless_tx_enabled: bool,
}

/// Engine configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSection {
    /// Operational profile name.
    pub profile: String,
    /// Station callsign. Required to transmit at all (COP-37): while it is
    /// empty (the default) or invalid, the daemon still starts and receives,
    /// but refuses every transmission, logging why, until a valid callsign
    /// is set here or by a command-port `MYCALL`. The station-ID timer and
    /// beacon mode (Phase 4 Task 3) also stay off without one, regardless of
    /// their interval settings.
    pub callsign: String,
    /// Optional free-text Maidenhead grid locator (e.g. "FN20"), included in
    /// the station-ID/beacon payload alongside the callsign when set. No
    /// validation beyond being a plain string -- this is an operator-supplied
    /// field, not a parsed/validated grid square.
    pub grid: Option<String>,
    /// Enable selective-repeat ARQ for sessionless traffic (raw data-port or
    /// WebSocket bytes sent with `[host] sessionless_tx_enabled`). Data inside a
    /// connected session is always acknowledged and retransmitted (COP-38),
    /// whatever this is set to.
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
#[serde(default, deny_unknown_fields)]
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
    /// against clock/scheduling jitter. Only fires if `[engine] callsign` is
    /// set. The first transmission after startup always carries an ID
    /// (COP-61); after that, one is prepended to the next outgoing
    /// transmission once this interval has passed. An ID is never sent
    /// standalone on a bare timer -- an idle station never needs to
    /// identify. `0` turns automatic ID off, first transmission included.
    /// Note this default alone does NOT enable the feature: with
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
            input_device_id: String::new(),
            output_device_id: String::new(),
            input_channel: 0,
            output_channel: 0,
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
            ardop_enabled: false,
            ardop_command_port: 8515,
            ardop_data_port: 8516,
            websocket_enabled: false,
            websocket_port: 8400,
            websocket_token: None,
            websocket_allowed_origins: Vec::new(),
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

/// The environment config discovery reads (COP-49). Injected rather than
/// read inside [`config_candidates`] so every platform's search order is
/// testable on any host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigEnv {
    /// Use the Windows search order.
    pub windows: bool,
    /// `HOME`.
    pub home: Option<OsString>,
    /// `XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<OsString>,
    /// `APPDATA` (Windows only).
    pub appdata: Option<OsString>,
}

impl ConfigEnv {
    /// This process's environment and platform.
    pub fn from_process() -> Self {
        Self {
            windows: cfg!(windows),
            home: std::env::var_os("HOME"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            appdata: std::env::var_os("APPDATA"),
        }
    }
}

/// COP-49: the config files `coppad` tries, in order, when no path is given:
///
/// 1. `coppad.toml` in the working directory (relative, as printed);
/// 2. Unix: `$XDG_CONFIG_HOME/coppa/coppad.toml` if that variable is
///    absolute, else `$HOME/.config/coppa/coppad.toml` if `HOME` is;
///    Windows: `%APPDATA%\coppa\coppad.toml`;
/// 3. Unix only: [`SYSTEM_CONFIG_PATH`].
///
/// Empty variables are ignored, and so are relative `XDG_CONFIG_HOME` and
/// `HOME` values (the XDG spec says to ignore relative paths). `APPDATA` is
/// set by the OS and only needs to be non-empty.
pub fn config_candidates(env: &ConfigEnv) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from(CONFIG_FILE_NAME)];
    if env.windows {
        if let Some(appdata) = non_empty(&env.appdata) {
            candidates.push(Path::new(appdata).join("coppa").join(CONFIG_FILE_NAME));
        }
    } else {
        let user_dir = rooted(&env.xdg_config_home)
            .map(PathBuf::from)
            .or_else(|| rooted(&env.home).map(|home| Path::new(home).join(".config")));
        if let Some(dir) = user_dir {
            candidates.push(dir.join("coppa").join(CONFIG_FILE_NAME));
        }
        candidates.push(PathBuf::from(SYSTEM_CONFIG_PATH));
    }
    candidates
}

fn non_empty(value: &Option<OsString>) -> Option<&OsStr> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// A non-empty value that is an absolute Unix path. `has_root` rather than
/// `is_absolute` so the Unix order is also testable on a Windows host, where
/// `/home/op` has no drive prefix; on Unix the two are the same.
fn rooted(value: &Option<OsString>) -> Option<&OsStr> {
    non_empty(value).filter(|value| Path::new(value).has_root())
}

/// The error for a discovered file that exists but cannot be used. The same
/// wording the implicit `./coppad.toml` has always used.
fn parse_error(path: &Path, e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("Failed to parse config {}: {}", path.display(), e)
}

/// Keys shared with the CLI, preserving whether a key was written in TOML.
/// Typed daemon defaults cannot distinguish an omitted key from an explicit one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedSettings {
    pub callsign: Option<String>,
    pub profile: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub ptt_method: Option<String>,
    pub rigctld_address: Option<String>,
    pub ptt_pre_delay_ms: Option<u64>,
    pub ptt_tail_delay_ms: Option<u64>,
    pub frequency_hz: Option<u64>,
    pub band: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SharedToml {
    engine: SharedEngine,
    audio: SharedAudio,
    radio: SharedRadio,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct SharedEngine {
    callsign: Option<String>,
    profile: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct SharedAudio {
    input_device: Option<String>,
    output_device: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct SharedRadio {
    ptt_method: Option<String>,
    rigctld_address: Option<String>,
    ptt_pre_delay_ms: Option<u64>,
    ptt_tail_delay_ms: Option<u64>,
    frequency_hz: Option<u64>,
    band: Option<String>,
}
impl SharedSettings {
    fn from_validated_toml(text: &str) -> anyhow::Result<Self> {
        let view: SharedToml = toml::from_str(text)?;
        let nonempty = |v: Option<String>| v.filter(|s| !s.is_empty());
        Ok(Self {
            callsign: nonempty(view.engine.callsign),
            profile: view.engine.profile,
            input_device: nonempty(view.audio.input_device),
            output_device: nonempty(view.audio.output_device),
            ptt_method: view.radio.ptt_method,
            rigctld_address: view.radio.rigctld_address,
            ptt_pre_delay_ms: view.radio.ptt_pre_delay_ms,
            ptt_tail_delay_ms: view.radio.ptt_tail_delay_ms,
            frequency_hz: view.radio.frequency_hz,
            band: view.radio.band,
        })
    }
}

/// The result of [`DaemonConfig::discover`].
#[derive(Debug, Clone)]
pub struct Discovered {
    /// The loaded file's configuration, or the defaults if none was found.
    pub config: DaemonConfig,
    /// Explicitly written CLI-shared keys from the same file.
    #[allow(dead_code)]
    // coppad compiles this module too; the CLI consumes this field through the library.
    pub shared: SharedSettings,
    /// The file that was loaded, if any.
    pub source: Option<PathBuf>,
    /// Every candidate examined, in order, up to and including `source`.
    pub searched: Vec<PathBuf>,
    /// Candidates skipped because a parent directory could not be entered.
    pub permission_denied: Vec<PathBuf>,
}

impl Discovered {
    /// `searched` as a comma-separated list, marking each candidate in
    /// `permission_denied` with `(permission denied)`.
    pub fn search_summary(&self) -> String {
        self.searched
            .iter()
            .map(|path| {
                if self.permission_denied.contains(path) {
                    format!("{} (permission denied)", path.display())
                } else {
                    path.display().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl DaemonConfig {
    /// Load configuration from a TOML file.
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        Ok(Self::load_with_shared(path)?.0)
    }

    /// Load the strict daemon config and the presence-aware CLI view in one read.
    pub fn load_with_shared<P: AsRef<Path>>(path: P) -> anyhow::Result<(Self, SharedSettings)> {
        let contents = std::fs::read_to_string(path.as_ref())?;
        let config: Self = toml::from_str(&contents)?;
        let shared = SharedSettings::from_validated_toml(&contents)?;
        Ok((config, shared))
    }

    /// COP-49: load the first of `candidates` that exists, or the built-in
    /// defaults when none does. Files are never merged.
    ///
    /// A candidate is skipped when `fs::metadata` reports it absent
    /// (`NotFound`, `NotADirectory`) or unreachable (`PermissionDenied`, e.g.
    /// a home directory hidden by systemd's `ProtectHome=yes`; recorded in
    /// [`Discovered::permission_denied`]). `Path::exists` cannot make that
    /// distinction: it reports an unreachable file as absent. Any other
    /// error, or a visible file that cannot be read or parsed (a directory,
    /// an unreadable file, bad TOML, an unknown key), is fatal and names the
    /// path.
    pub fn discover(candidates: &[PathBuf]) -> anyhow::Result<Discovered> {
        let mut searched = Vec::new();
        let mut permission_denied = Vec::new();
        for path in candidates {
            searched.push(path.clone());
            match std::fs::metadata(path) {
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                    continue
                }
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    permission_denied.push(path.clone());
                    continue;
                }
                Err(e) => return Err(parse_error(path, e)),
            }
            let (config, shared) =
                Self::load_with_shared(path).map_err(|e| parse_error(path, e))?;
            return Ok(Discovered {
                config,
                shared,
                source: Some(path.clone()),
                searched,
                permission_denied,
            });
        }
        Ok(Discovered {
            config: Self::default(),
            shared: SharedSettings::default(),
            source: None,
            searched,
            permission_denied,
        })
    }

    /// COP-36: `[engine] profile` must name a built-in profile
    /// (case-insensitive). `EventLoop::new` would otherwise build a
    /// level-derived engine from the typo with no warning. Checked by run mode
    /// before any hardware is touched; `--check` does not call it.
    pub fn check_profile(&self) -> Result<(), String> {
        if coppa_engine::profiles::get_profile(&self.engine.profile).is_some() {
            return Ok(());
        }
        Err(format!(
            "[engine] profile = \"{}\" is not a known profile; use one of: {}",
            self.engine.profile,
            coppa_engine::profiles::list_profiles().join(", ")
        ))
    }

    /// COP-36: `[audio] buffer_size` sizes the RX and TX audio rings, and a
    /// zero-capacity ring silently drops every sample. Checked by run mode
    /// before any hardware is touched; `--check` does not call it.
    pub fn check_buffer_size(&self) -> Result<(), String> {
        if self.audio.buffer_size > 0 {
            return Ok(());
        }
        Err(format!(
            "[audio] buffer_size = 0 is not supported: a zero-sample ring buffer drops \
             all audio; set buffer_size = {}",
            AudioConfig::default().buffer_size
        ))
    }

    /// COP-50: `[host] websocket_enabled = true` needs a build with the
    /// `websocket` feature (`built`); without it no listener would start and
    /// nothing would say so. Checked by run mode before any hardware is touched.
    pub fn check_websocket_built(&self, built: bool) -> Result<(), String> {
        if built || !self.host.websocket_enabled {
            return Ok(());
        }

        Err(
            "[host] websocket_enabled = true is set, but this coppad was built without the \
             websocket feature and starts no WebSocket server; rebuild with --features \
             websocket, or set websocket_enabled = false"
                .to_string(),
        )
    }

<<<<<<< HEAD
    /// COP-97: an ARDOP-enabled config needs a build with the `ardop`
    /// feature, checked in every build (the keys deserialize in every
    /// build).
    pub fn check_ardop_built(&self, built: bool) -> Result<(), String> {
        if built || !self.host.ardop_enabled {
            return Ok(());
        }

        Err(
            "[host] ardop_enabled = true is set, but this coppad was built without the \
             ardop feature and starts no ARDOP host interface; rebuild with --features \
             ardop, or set ardop_enabled = false"
                .to_string(),
        )
    }
    /// COP-32: `[audio] sample_rate` must equal the configured profile's
    /// engine rate. coppad has no resampler, so any other rate plays and
    /// captures audio at the wrong speed. Checked by run mode before any
    /// hardware is touched; `--check` does not call it.
=======
    /// A positive preferred hardware rate is required before opening audio.
    /// The live backend picks a supported rate and converts to the engine rate.
>>>>>>> 0887a09cde28a06c63cc0a7cbc9623081cf24f10
    pub fn check_sample_rate(&self) -> Result<(), String> {
        if self.audio.sample_rate > 0 {
            return Ok(());
        }
        Err(
            "[audio] sample_rate = 0 is not supported: set a positive preferred hardware rate"
                .into(),
        )
    }

    /// A name and ID for the same direction would make selection ambiguous.
    pub fn check_audio_selectors(&self) -> Result<(), String> {
        for (direction, name, id) in [
            (
                "input",
                &self.audio.input_device,
                &self.audio.input_device_id,
            ),
            (
                "output",
                &self.audio.output_device,
                &self.audio.output_device_id,
            ),
        ] {
            if !name.is_empty() && !id.is_empty() {
                return Err(format!(
                    "[audio] {direction}_device and {direction}_device_id cannot both be set"
                ));
            }
        }
        Ok(())
    }

    /// COP-53: a WebSocket-enabled daemon needs a usable `[host]
    /// websocket_token` and a well-formed `[host] websocket_allowed_origins`,
    /// whatever `bind_address` is: a web page in the operator's own browser
    /// reaches a loopback port too. Checked by run mode before any socket is
    /// bound; `--check` does not call it.
    pub fn check_websocket_auth(&self) -> Result<(), String> {
        if !self.host.websocket_enabled {
            return Ok(());
        }
        let Some(token) = &self.host.websocket_token else {
            return Err(format!(
                "[host] websocket_enabled = true requires [host] websocket_token: set it to \
                 a random value of at least {MIN_WEBSOCKET_TOKEN_CHARS} characters, for \
                 example the output of `openssl rand -hex 32`"
            ));
        };
        let token = token.expose();
        let length = token.chars().count();
        if length < MIN_WEBSOCKET_TOKEN_CHARS {
            return Err(format!(
                "[host] websocket_token is {length} characters long; it must be at least \
                 {MIN_WEBSOCKET_TOKEN_CHARS}, for example the output of `openssl rand -hex 32`"
            ));
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
        {
            return Err(
                "[host] websocket_token may contain only letters, digits, '-', '.', \
                 '_' and '~'"
                    .to_string(),
            );
        }
        for entry in &self.host.websocket_allowed_origins {
            check_origin_entry(entry).map_err(|reason| {
                format!("[host] websocket_allowed_origins entry {entry:?} {reason}")
            })?;
        }
        Ok(())
    }
}

/// COP-53: the shortest `[host] websocket_token` run mode accepts.
const MIN_WEBSOCKET_TOKEN_CHARS: usize = 16;

/// COP-53: why `entry` is not a browser origin (`scheme "://" host [":"
/// port]`), or `Ok`. The caller prefixes the key and the entry. The first
/// matching rule decides the message.
fn check_origin_entry(entry: &str) -> Result<(), String> {
    const SHAPE: &str =
        "is not an origin: write scheme://host[:port], for example \"http://localhost:3000\"";
    if entry.contains('*') {
        return Err(
            "is not an origin: wildcards are not supported; list each scheme://host[:port]"
                .to_string(),
        );
    }
    if entry.eq_ignore_ascii_case("null") {
        return Err(
            "is never allowed: it is the origin of sandboxed frames and file:// pages".to_string(),
        );
    }
    let Some((scheme, rest)) = entry.split_once("://") else {
        return Err(SHAPE.to_string());
    };
    if rest.is_empty() {
        return Err(SHAPE.to_string());
    }
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("is not an origin: only http and https origins are supported".to_string());
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if let Some((_, host_port)) = authority.rsplit_once('@') {
        return Err(format!(
            "is not an origin: an origin has no user name; write \"{scheme}://{host_port}\""
        ));
    }
    if !tail.is_empty() {
        return Err(format!(
            "is not an origin: an origin has no path, query or fragment; write \
             \"{scheme}://{authority}\""
        ));
    }
    let (host, port) = split_host_port(authority)?;
    let Some(port) = port else {
        return Ok(());
    };
    let port: u16 = match port.parse() {
        Ok(p) if p > 0 && port.bytes().all(|b| b.is_ascii_digit()) => p,
        _ => return Err("is not an origin: it has an invalid port".to_string()),
    };
    let default_port = if scheme == "http" { 80 } else { 443 };
    if port == default_port {
        return Err(format!(
            "names the default port, which browsers omit: write \"{scheme}://{host}\""
        ));
    }
    Ok(())
}

/// Split an origin's authority into a validated host and an optional raw
/// port string, for [`check_origin_entry`].
fn split_host_port(authority: &str) -> Result<(&str, Option<&str>), String> {
    const HOST: &str = "is not an origin: its host may contain only letters, digits, '-' and \
                        '.' (write internationalized names in their xn-- form)";
    if authority.starts_with('[') {
        let Some(close) = authority.find(']') else {
            return Err(HOST.to_string());
        };
        let (host, after) = authority.split_at(close + 1);
        let inner = &host[1..host.len() - 1];
        if inner.is_empty()
            || !inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(HOST.to_string());
        }
        return match after {
            "" => Ok((host, None)),
            _ => match after.strip_prefix(':') {
                Some(port) => Ok((host, Some(port))),
                None => Err(HOST.to_string()),
            },
        };
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err(HOST.to_string());
    }
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();
        assert_eq!(config.audio.sample_rate, 48_000);
        assert_eq!(config.audio.input_channel, 0);
        assert_eq!(config.audio.output_channel, 0);
        assert!(config.audio.input_device_id.is_empty());
        assert!(config.audio.output_device_id.is_empty());
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

        // COP-53: an unset token is omitted; the empty allowlist is printed.
        assert!(
            printed.contains("websocket_allowed_origins = []"),
            "{printed}"
        );
        assert!(!printed.contains("websocket_token"), "{printed}");

        let parsed: DaemonConfig = toml::from_str(&printed).unwrap();
        assert_eq!(toml::to_string_pretty(&parsed).unwrap(), printed);
        assert_eq!(parsed.audio.sample_rate, 48_000);
        assert_eq!(parsed.audio.input_channel, 0);
        assert_eq!(parsed.audio.output_channel, 0);
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

    // ── COP-49: config discovery ─────────────────────────────────────

    /// A uniquely named directory under the system temp dir, removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "coppa-config-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Self(path)
        }

        /// Write `contents` to `name` (which may contain subdirectories).
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).expect("create parent directory");
            std::fs::write(&path, contents).expect("write scratch file");
            path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                // Restore any permissions a test removed, so cleanup works.
                use std::os::unix::fs::PermissionsExt;
                fn unlock(dir: &Path) {
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                                unlock(&entry.path());
                            }
                        }
                    }
                }
                unlock(&self.0);
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unix_env(home: Option<&str>, xdg: Option<&str>) -> ConfigEnv {
        ConfigEnv {
            windows: false,
            home: home.map(OsString::from),
            xdg_config_home: xdg.map(OsString::from),
            appdata: Some(OsString::from("C:\\ignored")),
        }
    }

    fn user_candidate(dir: &str) -> PathBuf {
        Path::new(dir).join("coppa").join(CONFIG_FILE_NAME)
    }

    const GOOD: &str = "0123456789abcdef0123456789abcdef";

    /// A config with `[host] websocket_enabled = true` and the given token
    /// and allowlist (COP-53).
    fn ws(token: Option<&str>, origins: &[&str]) -> DaemonConfig {
        let mut config = DaemonConfig::default();
        config.host.websocket_enabled = true;
        config.host.websocket_token = token.map(|t| SecretToken(t.to_string()));
        config.host.websocket_allowed_origins = origins.iter().map(|o| o.to_string()).collect();
        config
    }

    #[test]
    fn websocket_auth_defaults_to_no_token_and_no_origins() {
        let host = HostConfig::default();
        assert_eq!(host.websocket_token, None);
        assert!(host.websocket_allowed_origins.is_empty());
    }

    /// COP-97: the ARDOP interface defaults off with the ardopcf-convention
    /// ports 8515/8516, so every existing config file keeps its exact
    /// behaviour.
    #[cfg(feature = "ardop")]
    #[test]
    fn ardop_defaults_are_off_with_8515_8516() {
        let host = HostConfig::default();
        assert!(!host.ardop_enabled);
        assert_eq!(host.ardop_command_port, 8515);
        assert_eq!(host.ardop_data_port, 8516);
    }

    /// COP-97: the three keys parse from a `[host]` section, and an unknown
    /// key is still rejected (`deny_unknown_fields`, COP-36 behaviour
    /// unchanged).
    #[cfg(feature = "ardop")]
    #[test]
    fn ardop_keys_parse_and_unknown_keys_still_rejected() {
        let config: DaemonConfig = toml::from_str(
            "[host]\nardop_enabled = true\nardop_command_port = 8515\nardop_data_port = 8516\n",
        )
        .unwrap();
        assert!(config.host.ardop_enabled);
        assert_eq!(config.host.ardop_command_port, 8515);
        assert_eq!(config.host.ardop_data_port, 8516);

        let err = toml::from_str::<DaemonConfig>("[host]\nardop_port = 1\n").unwrap_err();
        assert!(
            err.to_string().contains("ardop_port"),
            "the typo'd key must be named: {err}"
        );
    }

    /// COP-97: an enabled ARDOP interface refuses to start in a build
    /// without the feature (the `check_websocket_built` precedent).
    #[test]
    fn check_ardop_built_gates_an_enabled_config() {
        let mut config = DaemonConfig::default();
        assert_eq!(config.check_ardop_built(false), Ok(()));
        config.host.ardop_enabled = true;
        let err = config.check_ardop_built(false).unwrap_err();
        assert!(err.contains("ardop feature"), "{err}");
        assert_eq!(config.check_ardop_built(true), Ok(()));
    }

    #[test]
    fn secret_token_debug_never_shows_the_value() {
        let config = ws(Some(GOOD), &[]);
        let debug = format!("{:?}", config.host);
        assert!(!debug.contains(GOOD), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn check_websocket_auth_skips_a_disabled_websocket() {
        let mut config = ws(None, &["*"]);
        config.host.websocket_enabled = false;
        assert_eq!(config.check_websocket_auth(), Ok(()));
    }

    #[test]
    fn check_websocket_auth_requires_a_token_when_enabled() {
        let err = ws(None, &[]).check_websocket_auth().unwrap_err();
        assert!(
            err.starts_with("[host] websocket_enabled = true requires [host] websocket_token"),
            "{err}"
        );
        assert!(err.contains("openssl rand -hex 32"), "{err}");
    }

    #[test]
    fn check_websocket_auth_rejects_a_short_token() {
        let err = ws(Some("0123456789abcde"), &[])
            .check_websocket_auth()
            .unwrap_err();
        assert!(
            err.starts_with("[host] websocket_token is 15 characters long"),
            "{err}"
        );
        assert!(err.contains("at least 16"), "{err}");
        assert_eq!(
            ws(Some("0123456789abcdef"), &[]).check_websocket_auth(),
            Ok(())
        );
    }

    #[test]
    fn check_websocket_auth_rejects_token_characters_outside_unreserved() {
        for bad in ['\u{20}', '&', '#', '%', '+', '/', 'é'] {
            let token: String = std::iter::once(bad)
                .chain(std::iter::repeat_n('a', 19))
                .collect();
            let err = ws(Some(&token), &[]).check_websocket_auth().unwrap_err();
            assert!(
                err.starts_with("[host] websocket_token may contain only"),
                "{bad:?}: {err}"
            );
            assert!(!err.contains(&token), "the error must not echo the token");
        }
        assert_eq!(ws(Some(GOOD), &[]).check_websocket_auth(), Ok(()));
        assert_eq!(
            ws(Some("abc-def.ghi_jkl~mno"), &[]).check_websocket_auth(),
            Ok(())
        );
    }

    #[test]
    fn check_websocket_auth_accepts_a_valid_token_and_origins() {
        let origins = [
            "http://localhost:3000",
            "https://dash.example.org",
            "http://127.0.0.1:8080",
            "http://[::1]:3000",
            "HTTP://LOCALHOST:3000",
            "http://localhost",
        ];
        assert_eq!(ws(Some(GOOD), &origins).check_websocket_auth(), Ok(()));
    }

    #[test]
    fn check_websocket_auth_rejects_malformed_origin_entries() {
        let cases = [
            ("*", "wildcards are not supported"),
            ("null", "never allowed"),
            ("localhost:3000", "write scheme://host[:port]"),
            ("http://localhost:3000/", "write \"http://localhost:3000\""),
            ("http://localhost:3000/app", "no path, query or fragment"),
            ("http://localhost:3000?x=1", "no path, query or fragment"),
            ("http://user@localhost:3000", "no user name"),
            ("http://localhost:80", "write \"http://localhost\""),
            ("https://example.org:443", "write \"https://example.org\""),
            ("ftp://example.org", "only http and https"),
            ("http://", "write scheme://host[:port]"),
            ("http://local host", "host may contain only"),
            ("http://localhost:0", "port"),
            ("http://localhost:65536", "port"),
            ("http://localhost:30x0", "port"),
            ("", "write scheme://host[:port]"),
        ];
        for (entry, fragment) in cases {
            let err = ws(Some(GOOD), &[entry]).check_websocket_auth().unwrap_err();
            let prefix = format!("[host] websocket_allowed_origins entry {entry:?}");
            assert!(err.starts_with(&prefix), "{entry:?}: {err}");
            assert!(err.contains(fragment), "{entry:?}: {err}");
        }
    }

    #[test]
    fn test_example_config_documents_websocket_auth_keys() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../coppad.toml.example");
        let text = std::fs::read_to_string(path).unwrap();

        // Both keys appear as commented `key = value` lines inside [host].
        let mut table = String::new();
        let mut token_line = false;
        let mut origins_line = false;
        for line in text.lines() {
            let trimmed = line.trim_start_matches('#').trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains(' ') {
                table = trimmed.to_string();
            }
            let commented = line.trim_start().starts_with('#');
            if commented && trimmed.starts_with("websocket_token = ") {
                assert_eq!(table, "[host]", "websocket_token outside [host]: {line:?}");
                token_line = true;
            }
            if commented && trimmed.starts_with("websocket_allowed_origins = ") {
                assert_eq!(
                    table, "[host]",
                    "websocket_allowed_origins outside [host]: {line:?}"
                );
                origins_line = true;
            }
        }
        assert!(token_line, "no commented websocket_token line in [host]");
        assert!(
            origins_line,
            "no commented websocket_allowed_origins line in [host]"
        );
        assert!(text.contains("openssl rand -hex 32"));

        let config: DaemonConfig = toml::from_str(&text).unwrap();
        assert!(!config.host.websocket_enabled);
    }

    #[test]
    fn unix_candidates_are_cwd_then_home_config_then_etc() {
        assert_eq!(
            config_candidates(&unix_env(Some("/home/op"), None)),
            vec![
                PathBuf::from("coppad.toml"),
                Path::new("/home/op")
                    .join(".config")
                    .join("coppa")
                    .join("coppad.toml"),
                PathBuf::from("/etc/coppa/coppad.toml"),
            ]
        );
    }

    #[test]
    fn an_absolute_xdg_config_home_replaces_the_home_candidate() {
        assert_eq!(
            config_candidates(&unix_env(Some("/home/op"), Some("/srv/xdg"))),
            vec![
                PathBuf::from(CONFIG_FILE_NAME),
                user_candidate("/srv/xdg"),
                PathBuf::from(SYSTEM_CONFIG_PATH),
            ]
        );
    }

    #[test]
    fn a_relative_or_empty_xdg_config_home_falls_back_to_home() {
        let home_only = config_candidates(&unix_env(Some("/home/op"), None));
        for xdg in ["", "relative/xdg", "./xdg"] {
            assert_eq!(
                config_candidates(&unix_env(Some("/home/op"), Some(xdg))),
                home_only,
                "XDG_CONFIG_HOME={xdg:?}"
            );
        }
    }

    #[test]
    fn an_unset_empty_or_relative_home_has_no_user_candidate() {
        for home in [None, Some(""), Some("relative/home")] {
            assert_eq!(
                config_candidates(&unix_env(home, None)),
                vec![
                    PathBuf::from(CONFIG_FILE_NAME),
                    PathBuf::from(SYSTEM_CONFIG_PATH)
                ],
                "HOME={home:?}"
            );
        }
    }

    #[test]
    fn windows_candidates_are_cwd_then_appdata_and_ignore_home_and_xdg() {
        let appdata = "C:\\Users\\op\\AppData\\Roaming";
        let env = ConfigEnv {
            windows: true,
            home: Some(OsString::from("/home/op")),
            xdg_config_home: Some(OsString::from("/srv/xdg")),
            appdata: Some(OsString::from(appdata)),
        };
        assert_eq!(
            config_candidates(&env),
            vec![PathBuf::from(CONFIG_FILE_NAME), user_candidate(appdata)]
        );

        for appdata in [None, Some(OsString::new())] {
            let env = ConfigEnv {
                appdata,
                ..env.clone()
            };
            assert_eq!(
                config_candidates(&env),
                vec![PathBuf::from(CONFIG_FILE_NAME)],
                "{env:?}"
            );
        }
    }

    #[test]
    fn from_process_uses_this_platform() {
        assert_eq!(ConfigEnv::from_process().windows, cfg!(windows));
    }

    #[test]
    fn discover_uses_the_first_existing_candidate() {
        let dir = ScratchDir::new("first-wins");
        let missing = dir.0.join("missing.toml");
        let first = dir.write("a/coppad.toml", "[engine]\ncallsign = \"W5AU\"\n");
        let second = dir.write("b/coppad.toml", "[engine]\ncallsign = \"VK2ABC\"\n");
        let candidates = vec![missing, first.clone(), second];
        let found = DaemonConfig::discover(&candidates).unwrap();
        assert_eq!(found.source.as_ref(), Some(&first));
        assert_eq!(found.config.engine.callsign, "W5AU");
        assert_eq!(found.shared.callsign.as_deref(), Some("W5AU"));
        assert_eq!(found.shared.profile, None);
        assert_eq!(found.searched, candidates[..2]);
        assert!(found.permission_denied.is_empty());
    }

    #[test]
    fn discover_falls_back_to_defaults_when_nothing_exists() {
        let dir = ScratchDir::new("none");
        let candidates = vec![
            dir.0.join("coppad.toml"),
            dir.0.join("no/such/dir/coppad.toml"),
        ];
        let found = DaemonConfig::discover(&candidates).unwrap();
        assert_eq!(found.source, None);
        assert_eq!(found.searched, candidates);
        assert_eq!(found.config.engine.callsign, "");
        assert_eq!(found.shared, SharedSettings::default());
        assert_eq!(found.config.audio.sample_rate, 48_000);
        assert_eq!(
            found.search_summary(),
            format!("{}, {}", candidates[0].display(), candidates[1].display())
        );
    }

    #[test]
    fn discover_skips_a_candidate_under_a_regular_file() {
        let dir = ScratchDir::new("not-a-dir");
        let file = dir.write("plain", "");
        let valid = dir.write("valid/coppad.toml", "");
        let found = DaemonConfig::discover(&[file.join("coppad.toml"), valid.clone()]).unwrap();
        assert_eq!(found.source, Some(valid));
    }

    #[test]
    fn discover_fails_on_a_directory_naming_it() {
        let dir = ScratchDir::new("is-dir");
        let candidate = dir.0.join("coppad.toml");
        std::fs::create_dir(&candidate).unwrap();
        let valid = dir.write("valid/coppad.toml", "");
        let err = DaemonConfig::discover(&[candidate.clone(), valid])
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("Failed to parse config {}", candidate.display())),
            "{err}"
        );
    }

    #[test]
    fn discover_fails_on_an_unknown_key_without_trying_the_next_file() {
        let dir = ScratchDir::new("typo");
        let broken = dir.write("a/coppad.toml", "[engine]\ncalsign = \"W5AU\"\n");
        let valid = dir.write("b/coppad.toml", "[engine]\ncallsign = \"W5AU\"\n");
        let err = DaemonConfig::discover(&[broken.clone(), valid])
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("Failed to parse config {}", broken.display())),
            "{err}"
        );
        assert!(err.contains("unknown field `calsign`"), "{err}");
    }

    #[test]
    fn discover_fails_on_malformed_toml_naming_it() {
        let dir = ScratchDir::new("malformed");
        let broken = dir.write("coppad.toml", "this is not valid [[[toml");
        let err = DaemonConfig::discover(std::slice::from_ref(&broken))
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("Failed to parse config {}", broken.display())),
            "{err}"
        );
    }

    #[test]
    fn shared_settings_preserve_presence_and_empty_values() {
        let defaults = SharedSettings::from_validated_toml("").unwrap();
        assert_eq!(defaults.profile, None);
        assert_eq!(DaemonConfig::default().engine.profile, "HF_STANDARD");
        let explicit = SharedSettings::from_validated_toml(
            "[engine]\nprofile = \"HF_STANDARD\"\ncallsign = \"\"\n[audio]\ninput_device = \"\"\noutput_device = \"\"\n[radio]\nband = \"\"\n"
        ).unwrap();
        assert_eq!(explicit.profile.as_deref(), Some("HF_STANDARD"));
        assert_eq!(explicit.callsign, None);
        assert_eq!(explicit.input_device, None);
        assert_eq!(explicit.output_device, None);
        assert_eq!(explicit.band.as_deref(), Some(""));
    }

    #[test]
    fn shared_view_covers_every_cli_key_in_a_full_config() {
        let config = DaemonConfig::default();
        let text = toml::to_string(&config).unwrap();
        let shared = SharedSettings::from_validated_toml(&text).unwrap();
        assert!(shared.profile.is_some());
        assert!(shared.ptt_method.is_some());
        assert!(shared.rigctld_address.is_some());
        assert!(shared.ptt_pre_delay_ms.is_some());
        assert!(shared.ptt_tail_delay_ms.is_some());
        // Optional frequency and band are absent in the daemon default;
        // writing them makes the presence view report them too.
        let mut config = config;
        config.engine.callsign = "W5AU".into();
        config.audio.input_device = "in".into();
        config.audio.output_device = "out".into();
        config.radio.frequency_hz = Some(14_074_000);
        config.radio.band = Some("20m".into());
        let full = SharedSettings::from_validated_toml(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(full.callsign.as_deref(), Some("W5AU"));
        assert_eq!(full.input_device.as_deref(), Some("in"));
        assert_eq!(full.output_device.as_deref(), Some("out"));
        assert_eq!(full.frequency_hz, Some(14_074_000));
        assert_eq!(full.band.as_deref(), Some("20m"));
    }

    #[test]
    fn strict_config_rejects_typo_before_shared_view() {
        let path = std::env::temp_dir().join(format!("cop57_typo_{}.toml", std::process::id()));
        std::fs::write(&path, "[engine]\ncalsign = \"W5AU\"\n").unwrap();
        let error = DaemonConfig::load_with_shared(&path)
            .unwrap_err()
            .to_string();
        assert_eq!(error, DaemonConfig::load(&path).unwrap_err().to_string());
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("unknown field `calsign`"), "{error}");
        assert!(error.contains("line 2, column 1"), "{error}");
    }

    #[test]
    fn search_summary_marks_permission_denied_candidates() {
        let found = Discovered {
            config: DaemonConfig::default(),
            shared: SharedSettings::default(),
            source: None,
            searched: vec![
                PathBuf::from("coppad.toml"),
                PathBuf::from("/home/op/.config/coppa/coppad.toml"),
                PathBuf::from("/etc/coppa/coppad.toml"),
            ],
            permission_denied: vec![PathBuf::from("/home/op/.config/coppa/coppad.toml")],
        };
        assert_eq!(
            found.search_summary(),
            "coppad.toml, /home/op/.config/coppa/coppad.toml (permission denied), \
             /etc/coppa/coppad.toml"
        );
    }

    #[cfg(unix)]
    #[test]
    fn discover_skips_a_dangling_symlink() {
        let dir = ScratchDir::new("dangling");
        let link = dir.0.join("coppad.toml");
        std::os::unix::fs::symlink(dir.0.join("nowhere.toml"), &link).unwrap();
        let valid = dir.write("valid/coppad.toml", "[engine]\ncallsign = \"W5AU\"\n");
        let found = DaemonConfig::discover(&[link, valid.clone()]).unwrap();
        assert_eq!(found.source, Some(valid));
        assert_eq!(found.config.engine.callsign, "W5AU");
    }

    /// Remove every permission from `dir`. Returns false when the process can
    /// still list it (it runs as root), so a permission test cannot run.
    #[cfg(unix)]
    fn lock(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(dir).is_ok() {
            eprintln!(
                "skipping: {} is still readable at mode 000 (root?)",
                dir.display()
            );
            return false;
        }
        true
    }

    #[cfg(unix)]
    #[test]
    fn discover_skips_and_records_a_candidate_under_an_unenterable_directory() {
        let dir = ScratchDir::new("locked-parent");
        let hidden = dir.write("home/.config/coppa/coppad.toml", "");
        let valid = dir.write("etc/coppad.toml", "[engine]\ncallsign = \"W5AU\"\n");
        if !lock(&dir.0.join("home/.config")) {
            return;
        }
        let candidates = vec![hidden.clone(), valid.clone()];
        let found = DaemonConfig::discover(&candidates).unwrap();
        assert_eq!(found.source, Some(valid));
        assert_eq!(found.config.engine.callsign, "W5AU");
        assert_eq!(found.searched, candidates);
        assert_eq!(found.permission_denied, vec![hidden.clone()]);
        assert!(
            found
                .search_summary()
                .starts_with(&format!("{} (permission denied), ", hidden.display())),
            "{}",
            found.search_summary()
        );
    }

    #[cfg(unix)]
    #[test]
    fn discover_fails_on_an_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = ScratchDir::new("locked-file");
        let unreadable = dir.write("a/coppad.toml", "");
        let valid = dir.write("b/coppad.toml", "");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&unreadable).is_ok() {
            eprintln!(
                "skipping: {} is still readable at mode 000 (root?)",
                unreadable.display()
            );
            return;
        }
        let err = DaemonConfig::discover(&[unreadable.clone(), valid])
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("Failed to parse config {}", unreadable.display())),
            "{err}"
        );
        assert!(err.contains("ermission denied"), "{err}");
    }

    #[test]
    fn check_sample_rate_accepts_the_engine_rate() {
        assert_eq!(DaemonConfig::default().check_sample_rate(), Ok(()));
    }

    #[test]
    fn check_sample_rate_accepts_another_positive_hardware_rate() {
        let mut config = DaemonConfig::default();
        config.audio.sample_rate = 44_100;
        assert_eq!(config.check_sample_rate(), Ok(()));
        assert_eq!(
            coppa_engine::profiles::get_profile(&config.engine.profile)
                .unwrap()
                .sample_rate,
            48_000
        );
    }

    #[test]
    fn check_sample_rate_rejects_zero() {
        let mut config = DaemonConfig::default();
        config.audio.sample_rate = 0;
        assert!(config
            .check_sample_rate()
            .unwrap_err()
            .contains("sample_rate = 0"));
    }

    #[test]
    fn audio_ids_channels_roundtrip_and_conflicting_selectors_fail() {
        let config: DaemonConfig = toml::from_str(
            "[audio]\ninput_device_id = \"alsa:rx\"\noutput_device_id = \"alsa:tx\"\ninput_channel = 1\noutput_channel = 2\nsample_rate = 44100\n"
        ).unwrap();
        assert_eq!(config.audio.input_device_id, "alsa:rx");
        assert_eq!(config.audio.output_device_id, "alsa:tx");
        assert_eq!(config.audio.input_channel, 1);
        assert_eq!(config.audio.output_channel, 2);
        assert_eq!(config.check_audio_selectors(), Ok(()));
        let printed = toml::to_string(&config).unwrap();
        assert_eq!(
            toml::from_str::<DaemonConfig>(&printed)
                .unwrap()
                .audio
                .output_channel,
            2
        );

        let conflict: DaemonConfig =
            toml::from_str("[audio]\ninput_device = \"USB\"\ninput_device_id = \"alsa:rx\"\n")
                .unwrap();
        assert!(conflict
            .check_audio_selectors()
            .unwrap_err()
            .contains("input_device and input_device_id"));
        let conflict: DaemonConfig =
            toml::from_str("[audio]\noutput_device = \"USB\"\noutput_device_id = \"alsa:tx\"\n")
                .unwrap();
        assert!(conflict
            .check_audio_selectors()
            .unwrap_err()
            .contains("output_device and output_device_id"));
    }

    #[test]
    fn an_unknown_key_in_a_section_is_a_parse_error_naming_it() {
        let err = toml::from_str::<DaemonConfig>("[engine]\ncalsign = \"W5AU\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `calsign`"), "{err}");
        assert!(err.contains("`callsign`"), "{err}");
    }

    #[test]
    fn an_unknown_table_or_top_level_key_is_a_parse_error_naming_it() {
        let err = toml::from_str::<DaemonConfig>("[radoi]\nptt_method = \"rigctld\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `radoi`"), "{err}");
        assert!(err.contains("`radio`"), "{err}");

        let err = toml::from_str::<DaemonConfig>("top_level_extra = 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `top_level_extra`"), "{err}");
    }

    #[test]
    fn every_section_rejects_unknown_keys() {
        for section in ["audio", "radio", "host", "engine", "station_id"] {
            let err = toml::from_str::<DaemonConfig>(&format!("[{section}]\nnot_a_key = 1\n"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("unknown field `not_a_key`"),
                "[{section}]: {err}"
            );
        }
    }

    #[test]
    fn check_profile_accepts_every_built_in_name_case_insensitively() {
        let mut config = DaemonConfig::default();
        for name in coppa_engine::profiles::list_profiles() {
            config.engine.profile = (*name).to_string();
            assert_eq!(config.check_profile(), Ok(()), "{name}");
        }
        config.engine.profile = "hf_standard".into();
        assert_eq!(config.check_profile(), Ok(()));
    }

    #[test]
    fn check_profile_rejects_an_unknown_name_listing_the_valid_ones() {
        let mut config = DaemonConfig::default();
        config.engine.profile = "HF_STANDRD".into();
        let err = config.check_profile().unwrap_err();
        assert!(
            err.starts_with("[engine] profile = \"HF_STANDRD\" is not a known profile"),
            "{err}"
        );
        for name in ["HF_ROBUST", "HF_STANDARD", "VHF_FAST", "EMERGENCY"] {
            assert!(err.contains(name), "{name} missing: {err}");
        }
    }

    #[test]
    fn websocket_enabled_needs_a_websocket_build() {
        let mut config = DaemonConfig::default();
        assert!(config.check_websocket_built(false).is_ok());
        config.host.websocket_enabled = true;
        assert!(config.check_websocket_built(true).is_ok());
        let err = config.check_websocket_built(false).unwrap_err();
        assert!(err.contains("--features websocket"), "{err}");
        assert!(err.contains("websocket_enabled = false"), "{err}");
    }

    #[test]
    fn check_buffer_size_rejects_zero_and_accepts_the_default() {
        let mut config = DaemonConfig::default();
        assert_eq!(config.check_buffer_size(), Ok(()));
        config.audio.buffer_size = 0;
        let err = config.check_buffer_size().unwrap_err();
        assert!(
            err.starts_with("[audio] buffer_size = 0 is not supported"),
            "{err}"
        );
        assert!(err.contains("set buffer_size = 8192"), "{err}");
    }
}
