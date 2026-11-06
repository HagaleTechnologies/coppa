//! Coppa CLI - command-line interface for the Coppa digital communications system.
//!
//! Stream rule (COP-86): stdout carries data only (decoded frames, JSON, raw
//! PCM, reports) and is written through [`emit`]/[`emit_raw`], so a closed
//! pipe ends the run quietly instead of panicking. Status goes to stderr.
#![cfg_attr(not(test), deny(clippy::print_stdout))]

#[cfg(feature = "kiss-tnc")]
mod broadcast;
mod doctor;
mod interrupt;
mod style;

use anyhow::Result;
use clap::{Parser, Subcommand};
use coppa_engine::config::{EngineConfig, OfdmBand};
use coppa_engine::CoppaCore;
use std::path::PathBuf;
mod settings;
use serde::Serialize;
use settings::{CliEnv, Direction, Layers, RigSettings, SETTINGS_HELP};
use std::io::Write;

/// Output verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verbosity {
    Quiet,
    Normal,
    Verbose(u8),
}

impl Verbosity {
    fn is_verbose(self) -> bool {
        matches!(self, Self::Verbose(_))
    }

    fn is_extended(self) -> bool {
        matches!(self, Self::Verbose(count) if count >= 2)
    }
}

/// Every coppa-cli cargo feature and whether this build has it, for
/// `--version`. `cli_features_lists_every_feature_in_cargo_toml` keeps it
/// complete.
const CLI_FEATURES: [(&str, bool); 3] = [
    ("cpal-backend", cfg!(feature = "cpal-backend")),
    ("file-backend", cfg!(feature = "file-backend")),
    ("kiss-tnc", cfg!(feature = "kiss-tnc")),
];

/// The enabled CLI features, comma-separated, or `none`.
fn cli_features() -> String {
    let on: Vec<&str> = CLI_FEATURES
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(name, _)| *name)
        .collect();
    if on.is_empty() {
        "none".to_string()
    } else {
        on.join(",")
    }
}

/// `coppa --version` (COP-86): the build facts a bug report needs. The first
/// line is what `-V` prints, so `coppa --version | head -1` keeps working.
static LONG_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{}\ncommit: {}\nfeatures: {}\nwire format: {}",
        env!("CARGO_PKG_VERSION"),
        coppa_daemon::build_info::GIT_SHA,
        cli_features(),
        coppa_engine::WIRE_FORMAT_VERSION
    )
});

#[derive(Parser)]
#[command(
    name = "coppa",
    about = "Coppa - Ham Radio Digital Communications System",
    version,
    long_version = LONG_VERSION.as_str(),
    arg_required_else_help = true
)]
struct Cli {
    /// Increase diagnostic output; repeat for frame metrics.
    #[arg(
        short,
        long,
        global = true,
        action = clap::ArgAction::Count,
        conflicts_with = "quiet",
        help_heading = "Global options"
    )]
    verbose: u8,
    /// Suppress status output; decoded data, warnings and errors still print.
    #[arg(
        short,
        long,
        global = true,
        conflicts_with = "verbose",
        help_heading = "Global options"
    )]
    quiet: bool,
    /// Read settings from this file instead of searching for coppad.toml (also COPPA_CONFIG). The file must exist.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Global options"
    )]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

/// Flags for `coppa rx`, shared with its deprecated hidden alias
/// `coppa listen` (COP-56) so the two cannot drift apart.
#[derive(clap::Args, Debug, Default, PartialEq, Eq)]
struct RxArgs {
    /// WAV file to decode, or - for raw PCM on stdin (see --sample-format).
    /// Without one, captures live audio from an input device.
    #[arg(value_name = "FILE", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    file: Option<String>,
    /// WAV file to decode (same as FILE).
    #[arg(
        short,
        long,
        value_name = "FILE",
        conflicts_with = "file",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    input: Option<String>,
    /// Stop after this many seconds of audio: the first N seconds of a file,
    /// or N seconds of live capture (0 = no limit).
    #[arg(
        short,
        long,
        value_name = "SECONDS",
        default_value_t = 0,
        allow_negative_numbers = true,
        value_parser = rx_seconds
    )]
    duration: u64,
    /// Operational profile name.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    profile: Option<String>,
    /// Your callsign. Printed under --verbose; nothing else uses it yet.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    callsign: Option<String>,
    /// Print only the decoded payload as lowercase hex, one line per frame,
    /// with no labels (always hex, even for text).
    #[arg(long)]
    raw: bool,
    /// Show the payload as lowercase hex even when it is valid UTF-8 text.
    #[arg(long)]
    hex: bool,
    /// Emit one JSON object per decoded frame.
    #[arg(long, conflicts_with_all = ["raw", "hex"])]
    json: bool,
    /// Audio input device name (unique substring, live capture only).
    #[arg(long, conflicts_with = "device_id", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    device: Option<String>,
    /// Exact audio input device ID from `coppa devices`.
    #[arg(long, conflicts_with = "device", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    device_id: Option<String>,
    /// Zero-based input channel on the selected device.
    #[arg(long, default_value_t = 0)]
    input_channel: u16,
    /// Sample encoding of raw PCM on stdin (rx -): s16le (16-bit PCM) or
    /// f32le (32-bit float), mono 48000 Hz [default: s16le].
    #[arg(long, value_enum, value_name = "FORMAT", hide_possible_values = true)]
    sample_format: Option<SampleFormat>,
}

impl RxArgs {
    /// The WAV file to decode, from FILE or -i/--input (clap rejects both).
    fn path(&self) -> Option<&str> {
        self.file.as_deref().or(self.input.as_deref())
    }
}

/// Flags shared by `coppa profiles` and its deprecated alias `coppa config`, so
/// the two can never disagree about what they accept.
#[derive(clap::Args, Debug, Default, PartialEq, Eq)]
struct ProfilesArgs {
    /// Show one named profile (HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY).
    #[arg(short, long, conflicts_with = "level", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    profile: Option<String>,
    /// Show one speed level (1-9).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=9))]
    level: Option<u8>,
    /// Emit JSON output.
    #[arg(long)]
    json: bool,
}

/// Printed on stderr, after `WARNING:`, whenever the deprecated `coppa
/// config` alias runs, even under `--quiet`: a script still using it must
/// hear that it is going away.
const CONFIG_DEPRECATED: &str = "'coppa config' is deprecated and will be removed in the next release. Use 'coppa profiles' instead; it takes the same flags.";

/// The rate `coppa devices` reports support for: every profile's rate.
const DEVICES_RATE_HZ: u32 = 48_000;

/// One device's support for `DEVICES_RATE_HZ`, per direction: `None` when
/// the device advertises nothing that way. The outer `None` of
/// [`rate_support`] means unknown (the support listing failed or skipped it).
type RateSupport = (Option<bool>, Option<bool>);

/// Look up `device` (by id) in the support listing.
fn rate_support(
    device: &coppa_audio::AudioDevice,
    support: &Result<Vec<coppa_audio::DeviceSupport>>,
) -> Option<RateSupport> {
    let id = device.id.as_deref()?;
    let entry = support.as_ref().ok()?.iter().find(|entry| entry.id == id)?;
    let supports = |direction: &Option<coppa_audio::Support>| {
        direction.as_ref().map(|support| {
            matches!(support, coppa_audio::Support::Yes(format)
                if format.sample_rate == DEVICES_RATE_HZ)
        })
    };
    Some((supports(&entry.input), supports(&entry.output)))
}

/// Whether `device` is the host's default `default`, matched by id.
fn is_default(
    device: &coppa_audio::AudioDevice,
    default: Option<&coppa_audio::DefaultDevice>,
) -> bool {
    match (device.id.as_deref(), default.and_then(|d| d.id.as_deref())) {
        (Some(id), Some(default_id)) => id == default_id,
        _ => false,
    }
}

#[derive(Serialize)]
struct DeviceOutput {
    name: String,
    id: Option<String>,
    max_sample_rate_hz: u32,
    input_channels: u16,
    output_channels: u16,
    // COP-86: additive fields after the original five (COP-58 contract).
    input_supports_48000_hz: Option<bool>,
    output_supports_48000_hz: Option<bool>,
    is_default_input: bool,
    is_default_output: bool,
}

impl DeviceOutput {
    fn new(
        device: &coppa_audio::AudioDevice,
        support: &Result<Vec<coppa_audio::DeviceSupport>>,
        defaults: &coppa_audio::DefaultDevices,
    ) -> Self {
        let (input_supports, output_supports) = rate_support(device, support).unwrap_or_default();
        Self {
            name: device.name.clone(),
            id: device.id.clone(),
            max_sample_rate_hz: device.max_sample_rate,
            input_channels: device.input_channels,
            output_channels: device.output_channels,
            input_supports_48000_hz: input_supports,
            output_supports_48000_hz: output_supports,
            is_default_input: is_default(device, defaults.input.as_ref()),
            is_default_output: is_default(device, defaults.output.as_ref()),
        }
    }
}

const PAYLOAD_SCOPE: &str = "encoded_bytes_per_codeword";

#[derive(Serialize)]
struct ProfileOutput {
    name: &'static str,
    description: &'static str,
    speed_level: u8,
    max_payload_bytes: usize,
    max_payload_scope: &'static str,
    compression: bool,
    sample_rate_hz: u32,
    ofdm_profile: &'static str,
}

/// One speed level of `coppa_engine::ladder`, for `profiles --level N --json`.
/// `hf_*` are `null` on a VHF-only level. Air times describe a single-codeword frame.
#[derive(Serialize)]
struct LevelOutput {
    level: u8,
    modulation: &'static str,
    code_rate: String,
    max_payload_bytes: usize,
    max_payload_scope: &'static str,
    hf_bandwidth_hz: Option<u32>,
    hf_airtime_ms: Option<u32>,
    vhf_bandwidth_hz: u32,
    vhf_airtime_ms: u32,
}

impl From<&coppa_engine::ladder::LevelInfo> for LevelOutput {
    fn from(row: &coppa_engine::ladder::LevelInfo) -> Self {
        let ms = |s: f64| (s * 1000.0).round() as u32;
        Self {
            level: row.level,
            modulation: row.modulation,
            code_rate: format!("{}/{}", row.code_rate.0, row.code_rate.1),
            max_payload_bytes: row.max_payload_bytes,
            max_payload_scope: PAYLOAD_SCOPE,
            hf_bandwidth_hz: row.hf.map(|hf| hf.bandwidth_hz),
            hf_airtime_ms: row.hf.map(|hf| ms(hf.airtime_s)),
            vhf_bandwidth_hz: row.vhf.bandwidth_hz,
            vhf_airtime_ms: ms(row.vhf.airtime_s),
        }
    }
}

#[derive(Clone, Copy)]
enum RxOutputMode {
    Text { hex: bool },
    Raw,
    Json,
}

impl From<&RxArgs> for RxOutputMode {
    fn from(args: &RxArgs) -> Self {
        if args.json {
            Self::Json
        } else if args.raw {
            Self::Raw
        } else {
            Self::Text { hex: args.hex }
        }
    }
}

#[derive(Serialize)]
struct RxFrameOutput<'a> {
    payload_text: Option<&'a str>,
    payload_hex: String,
    snr_db: Option<f32>,
    cfo_hz: Option<f32>,
    frame_start: u64,
    speed_level: u8,
}

fn finite(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

impl<'a> RxFrameOutput<'a> {
    fn from_frame(frame: &coppa_engine::StreamFrame, bytes: &'a [u8]) -> Self {
        Self {
            payload_text: std::str::from_utf8(bytes).ok(),
            payload_hex: hex_string(bytes),
            snr_db: finite(frame.snr_db),
            cfo_hz: finite(frame.cfo_hz),
            frame_start: frame.frame_start,
            speed_level: frame.speed_level,
        }
    }
}

impl ProfileOutput {
    fn from_profile(profile: &coppa_engine::profiles::Profile) -> Result<Self> {
        let max_payload_bytes =
            coppa_protocol::modem::max_payload_for_level(profile.speed_level)
                .ok_or_else(|| anyhow::anyhow!("Invalid speed level for {}", profile.name))?;
        Ok(Self {
            name: profile.name,
            description: profile.description,
            speed_level: profile.speed_level,
            max_payload_bytes,
            max_payload_scope: PAYLOAD_SCOPE,
            compression: profile.compression,
            sample_rate_hz: profile.sample_rate,
            ofdm_profile: profile.ofdm_profile,
        })
    }
}

fn write_json<T: Serialize>(value: &T) -> Result<()> {
    let line = serde_json::to_string(value)?;
    emit(line)
}

/// Write one line of data to stdout. Unlike `println!`, a closed pipe comes
/// back as [`StdoutClosed`], which `main` turns into a quiet exit 0 (COP-86).
fn emit(line: impl std::fmt::Display) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(stdout_error)
}

/// Write bytes to stdout as they are, with `emit`'s closed-pipe handling.
fn emit_raw(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(stdout_error)
}

/// The reader of stdout went away. Only stdout writes produce it, so a
/// broken pipe on any other stream (a rigctld or TNC socket) stays an error.
#[derive(Debug)]
struct StdoutClosed;

impl std::fmt::Display for StdoutClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stdout was closed")
    }
}

impl std::error::Error for StdoutClosed {}

/// A failed stdout write: [`StdoutClosed`] for a broken pipe, else the error.
fn stdout_error(error: impl Into<anyhow::Error>) -> anyhow::Error {
    let error = error.into();
    if is_broken_pipe(&error) {
        StdoutClosed.into()
    } else {
        error
    }
}

/// Whether `rx` prints status lines: not under `--quiet`, and not when
/// stdout is a `--raw` or `--json` capture (COP-58).
fn status_enabled(verbosity: Verbosity, raw: bool, json: bool) -> bool {
    verbosity != Verbosity::Quiet && !raw && !json
}

/// True when `error` comes from writing to a pipe whose reader has gone.
fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// Printed on stderr, after `WARNING:`, every time the hidden `listen` alias
/// runs (COP-56). Unconditional, like this file's other `WARNING:` lines, so
/// `--quiet` scripts still see it; stderr keeps stdout captures clean.
const LISTEN_DEPRECATED: &str = "'coppa listen' is deprecated and will be \
    removed in the next release. Use 'coppa rx' instead; it takes the same flags, \
    and --raw prints hex.";

/// Sample encoding for `-o` and `rx -` (COP-86). A CLI-local enum, because
/// `coppa_audio::PcmFormat` exists only in a `file-backend` build.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SampleFormat {
    /// 16-bit signed PCM, little-endian.
    #[default]
    S16le,
    /// 32-bit float, little-endian.
    F32le,
}

#[cfg(feature = "file-backend")]
impl From<SampleFormat> for coppa_audio::PcmFormat {
    fn from(format: SampleFormat) -> Self {
        match format {
            SampleFormat::S16le => Self::S16Le,
            SampleFormat::F32le => Self::F32Le,
        }
    }
}

/// `-o` and `--sample-format`, shared by `tx` and `tune` so they cannot drift.
#[derive(clap::Args, Debug, Default, PartialEq, Eq)]
struct OutputArgs {
    /// Write the audio to a WAV file, or - for raw PCM on stdout, instead of playing it.
    #[arg(short, long, value_name = "FILE", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    output: Option<String>,
    /// Sample encoding for -o: s16le (16-bit PCM) or f32le (32-bit float) [default: s16le].
    #[arg(
        long,
        value_enum,
        value_name = "FORMAT",
        requires = "output",
        hide_possible_values = true
    )]
    sample_format: Option<SampleFormat>,
}

impl OutputArgs {
    fn target(&self) -> Option<OutputTarget<'_>> {
        self.output.as_deref().map(|path| OutputTarget {
            path,
            format: self.sample_format.unwrap_or_default(),
        })
    }
}

/// Where `tx`/`tune` write audio instead of playing it: a WAV file, or `-`
/// for raw PCM on stdout.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "file-backend"), allow(dead_code))]
struct OutputTarget<'a> {
    path: &'a str,
    format: SampleFormat,
}

/// Printed on stderr, after `WARNING:`, whenever the deprecated `tune
/// --seconds` runs, even under `--quiet`, like `LISTEN_DEPRECATED`.
const TUNE_SECONDS_DEPRECATED: &str = "'--seconds' is deprecated and will be \
    removed in the next release. Use '--duration' (-d); it takes the same value.";

#[derive(clap::Args, Debug, Default, PartialEq, Eq)]
struct RigArgs {
    /// Audio output device name (unique substring).
    #[arg(long, conflicts_with = "device_id", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    device: Option<String>,
    /// Exact audio output device ID from `coppa devices`.
    #[arg(long, conflicts_with = "device", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    device_id: Option<String>,
    /// Zero-based output channel on the selected device.
    #[arg(long, default_value_t = 0)]
    output_channel: u16,
    /// PTT method [default: none].
    // clap lists the values and rejects a typo even on the -o path (COP-86);
    // `validate_ptt_method` still checks env and coppad.toml values.
    #[arg(long, value_name = "METHOD", value_parser = ["none", "vox", "rigctld"])]
    ptt: Option<String>,
    /// rigctld address for CAT PTT [default: 127.0.0.1:4532].
    #[arg(long, value_name = "HOST:PORT", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    rigctld: Option<String>,
    /// Milliseconds to key PTT before audio starts playing [default: 50].
    #[arg(
        long,
        value_name = "MS",
        allow_negative_numbers = true,
        value_parser = whole_number
    )]
    ptt_lead_ms: Option<u64>,
    /// Milliseconds to hold PTT keyed after audio finishes playing [default: 200].
    #[arg(
        long,
        value_name = "MS",
        allow_negative_numbers = true,
        value_parser = whole_number
    )]
    ptt_tail_ms: Option<u64>,
    /// Explicit operating frequency in Hz for the wideband gate.
    #[arg(long, value_name = "HZ")]
    frequency_hz: Option<u64>,
    /// Operating band label ("160m".."23cm") for the wideband gate.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    band: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Encode and transmit a message.
    #[command(after_help = SETTINGS_HELP)]
    Tx {
        /// Message to transmit.
        #[arg(value_parser = clap::builder::NonEmptyStringValueParser::new())]
        message: String,
        #[command(flatten)]
        output: OutputArgs,
        /// Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY).
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        profile: Option<String>,
        /// Speed level 1-9, overriding the profile's level but never its band (see `coppa profiles`).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=9))]
        level: Option<u8>,
        /// Callsign for station identification.
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        callsign: Option<String>,
        #[command(flatten)]
        rig: RigArgs,
    },
    /// Receive and decode audio from a WAV file, stdin or a live audio input device.
    #[command(after_help = SETTINGS_HELP)]
    Rx(RxArgs),
    /// Run a loopback test (encode -> decode).
    #[command(after_help = SETTINGS_HELP)]
    Loopback {
        /// Message to test.
        #[arg(value_parser = clap::builder::NonEmptyStringValueParser::new())]
        message: String,
        /// Operational profile name.
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        profile: Option<String>,
        /// Speed level 1-9, overriding the profile's level but never its band (see `coppa profiles`).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=9))]
        level: Option<u8>,
    },
    /// Deprecated: use 'coppa rx'. Removed in the next release.
    // COP-56: kept for one release. Delete this variant, LISTEN_DEPRECATED
    // and the Listen dispatch arm in the release after it ships.
    #[command(hide = true)]
    #[command(after_help = SETTINGS_HELP)]
    Listen(RxArgs),
    #[command(
        about = "Transmit a TX-level calibration tone (two-tone by default)",
        long_about = "Transmit a TX-level calibration (\"TUNE\") tone: standard SSB two-tone \
            (700 Hz + 1900 Hz) by default, or a single tone via `--single`. Key this while \
            advancing your radio's audio drive level until ALC just registers, then back off. \
            See `docs/OPERATING.md`."
    )]
    #[command(after_help = SETTINGS_HELP)]
    Tune {
        /// How long to key the tone, in seconds (more than 0, at most 120).
        #[arg(
            short,
            long,
            value_name = "SECONDS",
            default_value = "10",
            allow_negative_numbers = true,
            value_parser = tune_seconds
        )]
        duration: f32,
        /// Deprecated: use --duration. Removed in the next release.
        // COP-86: kept for one release. Delete this field,
        // TUNE_SECONDS_DEPRECATED and its check in the Tune dispatch arm in
        // the release after it ships.
        #[arg(
            long,
            hide = true,
            value_name = "SECONDS",
            conflicts_with = "duration",
            allow_negative_numbers = true,
            value_parser = tune_seconds
        )]
        seconds: Option<f32>,
        /// Transmit a single tone at this frequency (300-3000 Hz) instead of
        /// the default two-tone signal, e.g. for power measurement with a
        /// wattmeter.
        #[arg(
            long,
            value_name = "HZ",
            allow_negative_numbers = true,
            value_parser = tune_single_hz
        )]
        single: Option<f32>,
        #[command(flatten)]
        output: OutputArgs,
        /// Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY).
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        profile: Option<String>,
        #[command(flatten)]
        rig: RigArgs,
    },
    /// List available audio devices.
    Devices {
        /// Emit a JSON array of audio devices.
        #[arg(long)]
        json: bool,
    },
    /// Check audio devices and rig control without transmitting.
    Doctor(doctor::DoctorArgs),
    #[command(
        about = "Show the speed levels and the named profiles built on them",
        long_about = "Show the speed levels (modulation, code rate, max bytes, air time, \
            bandwidth) and the named profiles built on them."
    )]
    Profiles(ProfilesArgs),
    // COP-55: kept for one release. Delete this variant, CONFIG_DEPRECATED and
    // the Config dispatch arm in the release after it ships.
    /// Deprecated: use 'coppa profiles'. Removed in the next release.
    #[command(hide = true)]
    Config(ProfilesArgs),
    /// Print a shell completion script (bash, elvish, fish, powershell, zsh).
    Completions {
        /// Shell to generate the script for.
        #[arg(value_enum, value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
    /// Print the coppa(1) man page in roff format.
    Man,
    /// Send or receive a repeated file over OFDM KISS datagrams.
    #[cfg(feature = "kiss-tnc")]
    Broadcast {
        #[command(subcommand)]
        command: broadcast::Command,
    },
    #[cfg(feature = "kiss-tnc")]
    /// Start the AFSK 1200 TNC (KISS TCP server).
    Tnc {
        /// KISS TCP port.
        #[arg(long, default_value = "8001")]
        port: u16,
        /// Input and output device name (unique substring).
        #[arg(long, conflicts_with_all = ["input_device_id", "output_device_id"])]
        device: Option<String>,
        /// Exact input device ID from `coppa devices`.
        #[arg(long, conflicts_with = "device")]
        input_device_id: Option<String>,
        /// Exact output device ID from `coppa devices`.
        #[arg(long, conflicts_with = "device")]
        output_device_id: Option<String>,
        /// Zero-based input channel.
        #[arg(long, default_value_t = 0)]
        input_channel: u16,
        /// Zero-based output channel.
        #[arg(long, default_value_t = 0)]
        output_channel: u16,
        /// rigctld address for PTT (e.g., localhost:4532).
        #[arg(long)]
        rig: Option<String>,
        /// Use VOX instead of CAT PTT.
        #[arg(long)]
        vox: bool,
        /// Transmit FX.25 when the frame fits; otherwise transmit AX.25
        #[arg(long)]
        fx25: bool,
        /// Digipeat as this alias: repeat a frame whose first unused path
        /// entry matches, marking it used. Off unless given.
        #[arg(long, value_name = "CALL[-SSID]")]
        digipeat: Option<String>,
    },
}

/// Resolve a profile name to an EngineConfig, or use the default.
fn resolve_config(profile: Option<&str>) -> Result<EngineConfig> {
    match profile {
        Some(name) => {
            let p = coppa_engine::profiles::get_profile(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown profile: {}. Available: {}",
                    name,
                    coppa_engine::profiles::list_profiles().join(", ")
                )
            })?;
            Ok(EngineConfig::from_profile(p))
        }
        None => Ok(EngineConfig::default()),
    }
}

/// Build the engine for `tx`/`loopback`: the profile (or the default), then
/// `--level` if one was given. The level changes the speed level and never the
/// band, as `coppa profiles` says; with no profile the band is HF, so `--level`
/// alone never builds the 5600 Hz VHF waveform that the wideband gate refuses
/// below 29.7 MHz. Without `--level` this is exactly the old
/// `resolve_config` + `with_config` path.
fn build_core(profile: Option<&str>, level: Option<u8>) -> Result<CoppaCore> {
    let mut config = resolve_config(profile)?;
    let Some(level) = level else {
        return Ok(CoppaCore::with_config(config));
    };
    if profile.is_none() {
        config.ofdm_band = OfdmBand::Hf;
    }
    let mut core = CoppaCore::with_config(config);
    let available = core.available_speed_levels();
    if !available.contains(&level) {
        let label = match profile.and_then(coppa_engine::profiles::get_profile) {
            Some(p) => p.name.to_string(),
            None => "the default HF profile".to_string(),
        };
        let lowest = available.first().copied().unwrap_or(1);
        let highest = available.last().copied().unwrap_or(1);
        anyhow::bail!(
            "level {level} is not available on {label}, which carries levels {lowest}-{highest}; \
             level {level} is VHF only (use --profile {})",
            vhf_profile_names().join(" or --profile ")
        );
    }
    // The engine stays the validator of record, as in the FFI's level override.
    core.set_speed_level(level)?;
    Ok(core)
}

/// Library diagnostics (PTT release failures, audio errors, `coppa tnc`
/// status) go through `tracing`; show them on stderr so stdout stays command
/// output. `RUST_LOG` overrides the `info` default.
fn init_logging() {
    use std::io::IsTerminal;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .without_time()
        .with_target(false)
        .with_ansi(coppa_daemon::logging::ansi_enabled(
            std::io::stderr().is_terminal(),
            std::env::var_os("NO_COLOR").as_deref(),
        ))
        .init();
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // The reader of stdout went away (`coppa rx f.wav | head -1`): it
        // chose to stop, so stop quietly, as `head` does.
        Err(error) if error.is::<StdoutClosed>() => std::process::ExitCode::SUCCESS,
        // Only returned once PTT was released and the audio stopped.
        Err(error) if error.is::<interrupt::Interrupted>() => {
            eprintln!("Interrupted: transmission stopped and PTT released.");
            std::process::ExitCode::from(130)
        }
        Err(error) => {
            // Byte-identical to anyhow's own `Termination` output.
            let prefix = style::paint("Error", style::Tone::BoldRed, style::stderr_color());
            eprintln!("{prefix}: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    init_logging();

    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else if cli.verbose > 0 {
        Verbosity::Verbose(cli.verbose)
    } else {
        Verbosity::Normal
    };

    let config_flag = cli.config;
    let env = CliEnv::from_process();
    match cli.command {
        Commands::Tx {
            message,
            output,
            profile,
            level,
            callsign,
            rig,
        } => {
            let layers = Layers::load(config_flag.as_deref(), env)?;
            let profile = layers.profile(profile.as_deref())?;
            let callsign = layers.callsign(callsign.as_deref())?;
            let rig = layers.rig(&rig)?;
            if verbosity.is_verbose() {
                layers.print_verbose_source();
                settings::print_setting("Callsign", &callsign);
                settings::print_setting("Profile", &profile);
                if output.output.is_none() {
                    rig.print_verbose();
                }
            }
            cmd_tx(
                &message,
                output.target(),
                profile.as_ref().map(|v| v.value.as_str()),
                level,
                &rig,
                verbosity,
            )?;
        }
        Commands::Rx(args) => resolve_rx_and_run(args, config_flag.as_deref(), env, verbosity)?,
        Commands::Listen(args) => {
            style::warn(LISTEN_DEPRECATED);
            resolve_rx_and_run(args, config_flag.as_deref(), env, verbosity)?;
        }
        Commands::Loopback {
            message,
            profile,
            level,
        } => {
            let layers = Layers::load(config_flag.as_deref(), env)?;
            let profile = layers.profile(profile.as_deref())?;
            if verbosity.is_verbose() {
                layers.print_verbose_source();
                settings::print_setting("Profile", &profile);
            }
            cmd_loopback(
                &message,
                profile.as_ref().map(|v| v.value.as_str()),
                level,
                verbosity,
            )?;
        }
        Commands::Tune {
            duration,
            seconds,
            single,
            output,
            profile,
            rig,
        } => {
            if seconds.is_some() {
                style::warn(TUNE_SECONDS_DEPRECATED);
            }
            let seconds = seconds.unwrap_or(duration);
            let layers = Layers::load(config_flag.as_deref(), env)?;
            let profile = layers.profile(profile.as_deref())?;
            let rig = layers.rig(&rig)?;
            if verbosity.is_verbose() {
                layers.print_verbose_source();
                settings::print_setting("Profile", &profile);
                if output.output.is_none() {
                    rig.print_verbose();
                }
            }
            cmd_tune(
                seconds,
                single,
                output.target(),
                profile.as_ref().map(|v| v.value.as_str()),
                &rig,
                verbosity,
            )?;
        }
        Commands::Devices { json } => cmd_devices(json)?,
        Commands::Doctor(args) => doctor::cmd_doctor(&args)?,
        Commands::Profiles(args) => cmd_profiles(&args)?,
        Commands::Config(args) => {
            style::warn(CONFIG_DEPRECATED);
            cmd_profiles(&args)?
        }
        #[cfg(feature = "kiss-tnc")]
        Commands::Broadcast { command } => broadcast::run(command, verbosity)?,
        #[cfg(feature = "kiss-tnc")]
        Commands::Tnc {
            port,
            device,
            input_device_id,
            output_device_id,
            input_channel,
            output_channel,
            rig,
            vox,
            fx25,
            digipeat,
        } => cmd_tnc(
            port,
            device,
            input_device_id,
            output_device_id,
            input_channel,
            output_channel,
            rig,
            vox,
            fx25,
            digipeat,
        )?,
        Commands::Completions { shell } => cmd_completions(shell)?,
        Commands::Man => cmd_man()?,
    }

    Ok(())
}

fn resolve_rx_and_run(
    mut args: RxArgs,
    config_flag: Option<&std::path::Path>,
    env: CliEnv,
    verbosity: Verbosity,
) -> Result<()> {
    // The deprecated alias shares the same resolver and receiver.
    let layers = Layers::load(config_flag, env)?;
    let profile = layers.profile(args.profile.as_deref())?;
    let callsign = layers.callsign(args.callsign.as_deref())?;
    // COP-64: an explicit --device-id is a flag, so it beats a file or
    // environment device name.
    let device = if args.device_id.is_some() {
        None
    } else {
        layers.device(args.device.as_deref(), Direction::Input)?
    };
    if verbosity.is_verbose() {
        layers.print_verbose_source();
        settings::print_setting("Callsign", &callsign);
        settings::print_setting("Profile", &profile);
        if args.path().is_none() {
            settings::print_setting("Device", &device);
        }
    }
    args.profile = profile.map(|v| v.value);
    args.callsign = callsign.map(|v| v.value);
    args.device = device.map(|v| v.value);
    cmd_rx(&args, verbosity)?;
    Ok(())
}

/// `coppa completions SHELL`. `clap_complete::generate` panics on a write
/// error, so the script is built in memory and written through `emit_raw`.
fn cmd_completions(shell: clap_complete::Shell) -> Result<()> {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut completion_command(), "coppa", &mut buf);
    emit_raw(&buf)
}

/// `Cli::command()` without its hidden subcommands. clap_complete offers
/// hidden subcommands too, and the deprecated `listen`/`config` aliases
/// should not be suggested to anyone.
fn completion_command() -> clap::Command {
    use clap::CommandFactory;
    let full = Cli::command();
    let visible: Vec<clap::Command> = full
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .cloned()
        .collect();
    clap::Command::new("coppa")
        .version(env!("CARGO_PKG_VERSION"))
        .args(full.get_arguments().cloned())
        .subcommands(visible)
}

/// `coppa man`: the top-level coppa(1) page, generated from the real flags.
fn cmd_man() -> Result<()> {
    use clap::CommandFactory;
    let mut buf = Vec::new();
    clap_mangen::Man::new(Cli::command()).render(&mut buf)?;
    emit_raw(&buf)
}

/// Refuse raw audio to a terminal: it is never what anyone wants.
#[cfg_attr(not(feature = "file-backend"), allow(dead_code))]
fn refuse_raw_stdout(is_terminal: bool) -> Result<()> {
    if is_terminal {
        anyhow::bail!(
            "refusing to write raw audio to a terminal; redirect stdout, e.g. \
             coppa tx MESSAGE -o - | aplay -f S16_LE -r 48000 -c 1"
        );
    }
    Ok(())
}

/// Refuse raw audio from a terminal: nobody types PCM.
#[cfg_attr(not(feature = "file-backend"), allow(dead_code))]
fn refuse_raw_stdin(is_terminal: bool) -> Result<()> {
    if is_terminal {
        anyhow::bail!(
            "refusing to read raw audio from a terminal; pipe it in, e.g. \
             arecord -f S16_LE -r 48000 -c 1 -t raw | coppa rx -"
        );
    }
    Ok(())
}

/// Write `samples` to a WAV file, or as raw PCM to stdout for `-`
/// (file-backend only) — shared by `cmd_tx`'s and `cmd_tune`'s output paths.
fn write_audio_output(
    target: OutputTarget<'_>,
    samples: &[f32],
    sample_rate: u32,
    verbosity: Verbosity,
) -> Result<()> {
    #[cfg(feature = "file-backend")]
    {
        use coppa_audio::AudioSink;
        use std::io::IsTerminal;
        let format = coppa_audio::PcmFormat::from(target.format);
        if target.path == "-" {
            refuse_raw_stdout(std::io::stdout().is_terminal())?;
            let mut sink = coppa_audio::RawPcmWriter::new(
                std::io::BufWriter::new(std::io::stdout()),
                format,
                sample_rate,
            );
            sink.start()?;
            sink.write(samples).map_err(stdout_error)?;
            sink.stop().map_err(stdout_error)?;
            if verbosity != Verbosity::Quiet {
                eprintln!("Written to stdout (raw {format}, mono, {sample_rate} Hz)");
            }
        } else {
            let mut sink =
                coppa_audio::WavSink::with_format(target.path, sample_rate, format.wav_format());
            sink.start()?;
            sink.write(samples)?;
            sink.stop()?;
            if verbosity != Verbosity::Quiet {
                eprintln!("Written to {}", target.path);
            }
        }
    }
    #[cfg(not(feature = "file-backend"))]
    {
        let _ = target;
        let _ = samples;
        let _ = sample_rate;
        if verbosity != Verbosity::Quiet {
            eprintln!("File output not available (compile with file-backend feature)");
        }
    }
    Ok(())
}

// COP-83 adds these to coppa-engine; switch to them when #192 lands.
/// Longest `coppa tune --duration`, in seconds.
const MAX_TUNE_SECONDS: f32 = 120.0;
/// Lowest `coppa tune --single` frequency, in Hz.
const TUNE_SINGLE_MIN_HZ: f32 = 300.0;
/// Highest `coppa tune --single` frequency, in Hz.
const TUNE_SINGLE_MAX_HZ: f32 = 3000.0;

/// Parse a finite number, or give `message` as clap's reason (COP-86).
fn parse_finite(s: &str, message: &str) -> Result<f32, String> {
    match s.trim().parse::<f32>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => Err(message.to_string()),
    }
}

/// `coppa tune --duration`: more than 0 and at most `MAX_TUNE_SECONDS`. NaN,
/// infinity and 0 used to write an empty file or panic (COP-86).
fn tune_seconds(s: &str) -> Result<f32, String> {
    let message = format!("must be more than 0 and at most {MAX_TUNE_SECONDS} seconds");
    let value = parse_finite(s, &message)?;
    if value > 0.0 && value <= MAX_TUNE_SECONDS {
        Ok(value)
    } else {
        Err(message)
    }
}

/// `coppa tune --single`: an audio tone inside the SSB passband.
fn tune_single_hz(s: &str) -> Result<f32, String> {
    let message = format!("must be between {TUNE_SINGLE_MIN_HZ} and {TUNE_SINGLE_MAX_HZ} Hz");
    let value = parse_finite(s, &message)?;
    if (TUNE_SINGLE_MIN_HZ..=TUNE_SINGLE_MAX_HZ).contains(&value) {
        Ok(value)
    } else {
        Err(message)
    }
}

/// A count such as `--ptt-lead-ms`: a whole number, 0 or more. clap's own
/// parser calls `-5` an unexpected argument; this names the rule instead.
fn whole_number(s: &str) -> Result<u64, String> {
    s.trim()
        .parse::<u64>()
        .map_err(|_| "must be a whole number, 0 or more".to_string())
}

/// `coppa rx --duration`: whole seconds, 0 for no limit.
fn rx_seconds(s: &str) -> Result<u64, String> {
    s.trim()
        .parse::<u64>()
        .map_err(|_| "must be a whole number of seconds, 0 or more (0 = no limit)".to_string())
}

/// Validate the `--ptt <method>` value (COP-40): an unrecognized value is a
/// hard error in every build, not only where `build_ptt` exists -- a
/// default (no-audio) build must reject a value the audio build would
/// reject, rather than silently accept it.
fn validate_ptt_method(ptt: &str) -> Result<()> {
    match ptt {
        "none" | "vox" | "rigctld" => Ok(()),
        other => anyhow::bail!(
            "unrecognized --ptt value {other:?}: expected \"none\", \"vox\", or \"rigctld\""
        ),
    }
}

/// Build the PTT controller for `--ptt <method>`, using `rigctld_addr` when
/// `method` is `"rigctld"`. Split out from `transmit_live` so the
/// `--rigctld`/CLI-flag plumbing can be unit-tested without a real audio
/// device.
///
/// A connected rigctld client is retained as `PttBackend::Rigctld` so the
/// wideband gate can read the rig's dial frequency (COP-40). An
/// unrecognized `--ptt` value is a hard error, matching the daemon's policy,
/// rather than silently keying nothing.
#[cfg(feature = "cpal-backend")]
fn build_ptt(ptt: &str, rigctld_addr: &str) -> Result<coppa_radio::PttBackend> {
    use coppa_radio::PttBackend;
    validate_ptt_method(ptt)?;
    match ptt {
        "rigctld" => Ok(PttBackend::Rigctld(
            coppa_radio::RigctldClient::connect(rigctld_addr)
                .map_err(|e| anyhow::anyhow!("rigctld connection failed: {e}"))?,
        )),
        "vox" => Ok(PttBackend::Other(Box::new(coppa_radio::VoxPtt::new()))),
        // Only "none" remains once `validate_ptt_method` has passed.
        _ => Ok(PttBackend::Other(Box::new(coppa_radio::NullPtt::new()))),
    }
}

/// Validate `--frequency-hz`/`--band` into the stated frequency evidence the
/// wideband gate falls back on (an explicit frequency over a band label).
/// A typo'd band, an implausible frequency, or a frequency outside the
/// stated band is an error.
fn cli_frequency_evidence(
    frequency_hz: Option<u64>,
    band: Option<&str>,
    freq_key: &str,
    band_key: &str,
) -> Result<Option<coppa_radio::band::FrequencyEvidence>> {
    let stated = coppa_radio::band::FrequencyConfig::parse(frequency_hz, band, freq_key, band_key)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(stated.static_evidence())
}

/// The wideband regulatory gate for live transmission (COP-40): the same
/// `coppa_radio::band::gate` the daemon applies. Evidence precedence is a
/// live rig answer over the stated flags. Errors (refuses) when a wideband
/// profile has no evidence at or above 29.7 MHz.
fn cli_gate_decision(
    profile: &str,
    occupied_hz: u32,
    live_hz: Option<u64>,
    stated: Option<coppa_radio::band::FrequencyEvidence>,
) -> Result<()> {
    use coppa_radio::band::{gate, FrequencyEvidence, GateRefusal};
    let evidence = live_hz.map(FrequencyEvidence::Live).or(stated);
    match gate(occupied_hz, evidence.as_ref()) {
        Ok(()) => Ok(()),
        Err(GateRefusal::NoSource) => anyhow::bail!(
            "refusing to transmit: profile {profile} emits {occupied_hz} Hz occupied bandwidth, \
             which 47 CFR 97.307(f)(3) caps below 29.7 MHz — pass --frequency-hz (>= {}) or \
             --band (6m and up), or point --ptt rigctld at a rig on a VHF+ band",
            coppa_radio::band::US_WIDEBAND_MIN_HZ
        ),
        Err(GateRefusal::BelowThreshold { evidence_hz, .. }) => anyhow::bail!(
            "refusing to transmit: profile {profile} emits {occupied_hz} Hz occupied bandwidth \
             but {}, below 29.7 MHz — 47 CFR 97.307(f)(3) caps emissions below 29.7 MHz at \
             2.8 kHz; use an HF profile or a VHF+ frequency/band",
            evidence.map_or_else(|| format!("{evidence_hz} Hz"), |e| e.describe())
        ),
    }
}

/// Time without downstream playback progress after which live TX gives up
/// rather than holding PTT keyed indefinitely.
#[cfg(feature = "cpal-backend")]
const PLAYBACK_STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Longest wait between playback polls while feeding and draining.
#[cfg(feature = "cpal-backend")]
const PLAYBACK_POLL: std::time::Duration = std::time::Duration::from_millis(2);

/// Clock and sleep used by [`play_keyed`], injected so its sequencing can be
/// tested against a simulated device without real multi-second sleeps.
#[cfg(feature = "cpal-backend")]
trait PlaybackClock {
    fn now(&self) -> std::time::Instant;
    fn sleep(&mut self, duration: std::time::Duration);
    /// Ctrl-C or SIGTERM has arrived: stop transmitting (COP-86).
    fn interrupted(&self) -> bool {
        false
    }
    /// Sleep `duration`, in slices of at most `PLAYBACK_POLL`, stopping
    /// early on an interrupt so a multi-second PTT lead or tail never delays
    /// the release. Returns whether it was interrupted.
    fn sleep_or_interrupt(&mut self, duration: std::time::Duration) -> bool {
        let end = self.now() + duration;
        loop {
            if self.interrupted() {
                return true;
            }
            let left = end.saturating_duration_since(self.now());
            if left.is_zero() {
                return false;
            }
            self.sleep(left.min(PLAYBACK_POLL));
        }
    }
}

/// The wall clock, a real thread sleep, and the process's signal flag.
#[cfg(feature = "cpal-backend")]
struct SystemClock<'a> {
    interrupt: Option<&'a interrupt::Interrupt>,
}

#[cfg(feature = "cpal-backend")]
impl PlaybackClock for SystemClock<'_> {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
    fn sleep(&mut self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }
    fn interrupted(&self) -> bool {
        self.interrupt.is_some_and(interrupt::Interrupt::is_set)
    }
}

/// `clock.sleep_or_interrupt` as a `Result`: `Err(Interrupted)` on Ctrl-C.
#[cfg(feature = "cpal-backend")]
fn sleep_unless_interrupted(
    clock: &mut dyn PlaybackClock,
    duration: std::time::Duration,
) -> Result<()> {
    if clock.sleep_or_interrupt(duration) {
        return Err(interrupt::Interrupted.into());
    }
    Ok(())
}

/// Key PTT, wait `lead`, feed every sample of `samples` into `sink`
/// (retaining the unwritten suffix on short writes and yielding while the
/// sink is full), wait until the last accepted sample is predicted to have
/// finished playing, wait `tail`, then unkey PTT and stop the sink.
///
/// Returns the number of samples delivered. Errors from the sink, from
/// `progress`, or five seconds without downstream progress still unkey PTT
/// and stop the sink before being returned; PTT and stop failures during
/// that cleanup do not skip each other.
#[cfg(feature = "cpal-backend")]
#[allow(clippy::too_many_arguments)]
fn play_keyed<S: coppa_audio::AudioSink>(
    sink: &mut S,
    progress: impl Fn(&S) -> Result<coppa_audio::PlaybackProgress>,
    finish: impl Fn(&mut S) -> Result<()>,
    ptt: &mut dyn coppa_radio::PttControl,
    samples: &[f32],
    lead: std::time::Duration,
    tail: std::time::Duration,
    clock: &mut dyn PlaybackClock,
) -> Result<u64> {
    use coppa_radio::PttState;

    // A Ctrl-C before keying never keys the rig at all.
    let keyed = if clock.interrupted() {
        Err(interrupt::Interrupted.into())
    } else {
        ptt.set_ptt(PttState::Tx)
            .map_err(|e| anyhow::anyhow!("failed to key PTT: {e}"))
    };
    let result = match keyed {
        Ok(()) => sleep_unless_interrupted(clock, lead)
            .and_then(|()| feed_until_played(sink, &progress, &finish, samples, clock))
            .and_then(|delivered| sleep_unless_interrupted(clock, tail).map(|()| delivered)),
        Err(e) => Err(e),
    };

    let release = ptt
        .set_ptt(PttState::Rx)
        .map_err(|e| anyhow::anyhow!("failed to release PTT: {e}"));
    let stopped = sink.stop();
    match result {
        // Keep the type, so `main` can report a clean interrupt with 130.
        Err(primary)
            if primary.is::<interrupt::Interrupted>() && release.is_ok() && stopped.is_ok() =>
        {
            Err(primary)
        }
        Err(primary) => {
            let mut message = primary.to_string();
            if let Err(e) = release {
                message.push_str(&format!("; cleanup: {e}"));
            }
            if let Err(e) = stopped {
                message.push_str(&format!("; cleanup: failed to stop audio: {e}"));
            }
            anyhow::bail!("{message}")
        }
        Ok(delivered) => {
            if let Err(e) = release {
                let suffix = stopped
                    .err()
                    .map(|stop| format!("; cleanup: failed to stop audio: {stop}"))
                    .unwrap_or_default();
                anyhow::bail!("{e}{suffix}");
            }
            stopped?;
            Ok(delivered)
        }
    }
}

/// The feeding/draining half of [`play_keyed`].
#[cfg(feature = "cpal-backend")]
fn feed_until_played<S: coppa_audio::AudioSink>(
    sink: &mut S,
    progress: &impl Fn(&S) -> Result<coppa_audio::PlaybackProgress>,
    finish: &impl Fn(&mut S) -> Result<()>,
    samples: &[f32],
    clock: &mut dyn PlaybackClock,
) -> Result<u64> {
    let mut offset = 0usize;
    let mut accepted = 0u64;
    let mut last_consumed = progress(sink)?.consumed_samples;
    let mut last_progress_at = clock.now();
    let mut finished = false;
    loop {
        if clock.interrupted() {
            return Err(interrupt::Interrupted.into());
        }
        if offset < samples.len() {
            let written = sink.write(&samples[offset..])?;
            let written = written.min(samples.len() - offset);
            offset += written;
            accepted += written as u64;
        }
        if offset == samples.len() && !finished {
            finish(sink)?;
            finished = true;
        }
        let snapshot = progress(sink)?;
        if finished && snapshot.has_played(accepted) {
            return Ok(accepted);
        }
        let now = clock.now();
        if snapshot.consumed_samples > last_consumed {
            last_consumed = snapshot.consumed_samples;
            last_progress_at = now;
        } else if snapshot.consumed_samples < accepted
            && now.saturating_duration_since(last_progress_at) >= PLAYBACK_STALL_TIMEOUT
        {
            anyhow::bail!(
                "audio output made no progress for {}s ({} of {} samples played)",
                PLAYBACK_STALL_TIMEOUT.as_secs(),
                snapshot.consumed_samples,
                samples.len()
            );
        }
        let wait = if snapshot.consumed_samples >= accepted && offset == samples.len() {
            // Everything is in the device; wait out its presentation delay.
            snapshot.remaining.min(PLAYBACK_POLL)
        } else {
            PLAYBACK_POLL
        };
        clock.sleep(wait.max(std::time::Duration::from_micros(100)));
    }
}

#[derive(Clone, Copy, Default)]
#[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))]
struct AudioOutputSelection<'a> {
    name: Option<&'a str>,
    id: Option<&'a str>,
    channel: u16,
}

#[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))]
impl<'a> AudioOutputSelection<'a> {
    /// COP-64: the output device a live `tx`/`tune` uses, from its resolved
    /// settings. An explicit `--device-id` replaces any layered name.
    fn from_rig(rig: &'a RigSettings) -> Self {
        Self {
            name: if rig.device_id.is_some() {
                None
            } else {
                rig.device.as_ref().map(|v| v.value.as_str())
            },
            id: rig.device_id.as_deref(),
            channel: rig.output_channel,
        }
    }
}

#[cfg(feature = "cpal-backend")]
fn resolve_cli_output_device(
    selection: AudioOutputSelection<'_>,
) -> Result<Option<coppa_audio::LiveDevice>> {
    match coppa_audio::resolve_output_device(selection.name, selection.id) {
        Ok(device) => Ok(device),
        Err(error) => match error.downcast_ref::<coppa_audio::DeviceSelectionError>() {
            Some(coppa_audio::DeviceSelectionError::MissingName { .. }) => {
                style::warn(format_args!(
                    "No output device matching '{}', using default",
                    selection.name.unwrap_or_default()
                ));
                Ok(None)
            }
            Some(coppa_audio::DeviceSelectionError::AmbiguousName { .. }) => {
                style::warn(&error);
                anyhow::bail!("ambiguous output audio device")
            }
            _ => Err(error),
        },
    }
}

#[cfg(feature = "cpal-backend")]
fn resolve_cli_input_device(
    name: Option<&str>,
    id: Option<&str>,
) -> Result<Option<coppa_audio::LiveDevice>> {
    match coppa_audio::resolve_input_device(name, id) {
        Ok(device) => Ok(device),
        Err(error) => match error.downcast_ref::<coppa_audio::DeviceSelectionError>() {
            Some(coppa_audio::DeviceSelectionError::MissingName { .. }) => {
                style::warn(format_args!(
                    "No input device matching '{}', using default",
                    name.unwrap_or_default()
                ));
                Ok(None)
            }
            Some(coppa_audio::DeviceSelectionError::AmbiguousName { .. }) => {
                style::warn(&error);
                anyhow::bail!("ambiguous input audio device")
            }
            _ => Err(error),
        },
    }
}

/// What the wideband gate needs to know about a live transmission: the
/// profile's label and occupied bandwidth, and the stated frequency flags.
struct LiveTxGate<'a> {
    profile: &'a str,
    occupied_hz: u32,
    stated: Option<coppa_radio::band::FrequencyEvidence>,
}

/// Key PTT, stream `samples` out through the resolved audio device, then
/// unkey — the live-playback sequencing shared by `cmd_tx` and `cmd_tune`.
#[cfg(feature = "cpal-backend")]
#[allow(clippy::too_many_arguments)]
fn transmit_live(
    samples: &[f32],
    sample_rate: u32,
    selection: AudioOutputSelection<'_>,
    ptt: &str,
    rigctld_addr: &str,
    ptt_lead_ms: u64,
    ptt_tail_ms: u64,
    gate: LiveTxGate<'_>,
    verbosity: Verbosity,
) -> Result<()> {
    use coppa_audio::AudioSink;

    // Create PTT controller
    let mut ptt_ctrl = build_ptt(ptt, rigctld_addr)?;
    // Before keying: a Ctrl-C from here on releases PTT (COP-86).
    let interrupt = interrupt::Interrupt::install()?;

    // COP-40: refuse a wideband emission below 29.7 MHz before any audio
    // device is opened or PTT keyed.
    let live_hz = if gate.occupied_hz > coppa_radio::band::HF_OCCUPIED_BANDWIDTH_CAP_HZ {
        ptt_ctrl.poll_frequency()
    } else {
        None
    };
    cli_gate_decision(gate.profile, gate.occupied_hz, live_hz, gate.stated)?;

    // Select and open audio before keying PTT. A missing explicit ID or
    // invalid channel is fatal; a missing legacy name retains its fallback.
    let selected_device = resolve_cli_output_device(selection)?;
    let mut sink = coppa_audio::LiveSink::new(
        selected_device,
        sample_rate,
        sample_rate,
        8192,
        selection.channel,
    )?;
    if sink.hardware_rate() != sample_rate {
        style::warn(format_args!(
            "output audio device {} uses {} Hz hardware rate (requested {} Hz, engine {} Hz)",
            sink.device_name(),
            sink.hardware_rate(),
            sample_rate,
            sample_rate
        ));
    }
    if verbosity.is_verbose() {
        eprintln!(
            "[verbose] Output audio: {} (hardware {} Hz, engine {} Hz)",
            sink.device_name(),
            sink.hardware_rate(),
            sample_rate
        );
    }
    sink.start()?;

    // Key, pace every sample through the sink's bounded ring, and unkey only
    // after the last one is predicted to have played plus the tail. Any
    // failure unkeys and stops before returning, and skips the success line.
    let delivered = play_keyed(
        &mut sink,
        |sink| sink.playback_progress(),
        |sink| sink.finish_input(),
        &mut ptt_ctrl,
        samples,
        std::time::Duration::from_millis(ptt_lead_ms),
        std::time::Duration::from_millis(ptt_tail_ms),
        &mut SystemClock {
            interrupt: Some(&interrupt),
        },
    )?;

    if verbosity != Verbosity::Quiet {
        let duration_s = delivered as f64 / sample_rate as f64;
        eprintln!("Transmitted {} samples ({:.2}s)", delivered, duration_s);
    }

    Ok(())
}

fn cmd_tx(
    message: &str,
    output: Option<OutputTarget<'_>>,
    profile: Option<&str>,
    level: Option<u8>,
    rig: &RigSettings,
    verbosity: Verbosity,
) -> Result<()> {
    let core = build_core(profile, level)?;

    if verbosity.is_verbose() {
        eprintln!(
            "[verbose] Config: sample_rate={}, speed_level={}",
            core.config().sample_rate,
            core.config().speed_level,
        );
    }

    if verbosity != Verbosity::Quiet {
        eprintln!("Encoding: \"{}\"", message);
    }
    let samples = core.encode(message)?;
    if verbosity != Verbosity::Quiet {
        eprintln!("Generated {} audio samples", samples.len());
    }
    if verbosity.is_verbose() {
        let duration_s = samples.len() as f64 / core.config().sample_rate as f64;
        eprintln!("[verbose] Audio duration: {:.3} s", duration_s);
    }

    match output {
        Some(target) => write_audio_output(target, &samples, core.config().sample_rate, verbosity)?,
        None => {
            // Offline WAV rendering (the `Some` arm) emits no RF and is never
            // gated; live transmission is (COP-40).
            let gate = LiveTxGate {
                profile: profile.unwrap_or("default"),
                occupied_hz: core.occupied_bandwidth_hz(),
                stated: rig.frequency_evidence()?,
            };
            #[cfg(feature = "cpal-backend")]
            {
                let ptt = rig.ptt_method()?;
                transmit_live(
                    &samples,
                    core.config().sample_rate,
                    AudioOutputSelection::from_rig(rig),
                    ptt,
                    &rig.rigctld.value,
                    rig.lead.value,
                    rig.tail.value,
                    gate,
                    verbosity,
                )?;
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                // No live audio in this build, so nothing reaches the air,
                // but the same arguments are validated: a typo'd --ptt is
                // rejected exactly as the audio build rejects it, and a
                // wideband request is still refused, not silently accepted.
                rig.ptt_method()?;
                cli_gate_decision(gate.profile, gate.occupied_hz, None, gate.stated)?;
                let _ = rig;
                if verbosity != Verbosity::Quiet {
                    eprintln!(
                        "Live audio output not available (compile with cpal-backend feature)"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Occupied bandwidth (Hz) of the calibration signal `cmd_tune` emits: the
/// two-tone spacing, or zero for a single tone. The tune signal is not an
/// OFDM frame, so the profile's bandwidth does not apply to it.
fn tune_occupied_hz(single: Option<f32>) -> u32 {
    match single {
        Some(_) => 0,
        None => (coppa_engine::TUNE_TONE_HIGH_HZ - coppa_engine::TUNE_TONE_LOW_HZ).round() as u32,
    }
}

/// TX level calibration ("TUNE"): generate the standard SSB two-tone signal
/// (or a single tone via `single`), key PTT, stream it out, then unkey —
/// mirroring `cmd_tx`'s live-playback path. See `docs/OPERATING.md` for the
/// calibration procedure this is meant to support.
fn cmd_tune(
    seconds: f32,
    single: Option<f32>,
    output: Option<OutputTarget<'_>>,
    profile: Option<&str>,
    rig: &RigSettings,
    verbosity: Verbosity,
) -> Result<()> {
    let config = resolve_config(profile)?;
    let core = CoppaCore::with_config(config);

    if verbosity != Verbosity::Quiet {
        match single {
            Some(freq) => eprintln!(
                "Generating single-tone calibration signal: {} Hz, {:.1}s",
                freq, seconds
            ),
            None => eprintln!(
                "Generating two-tone calibration signal: {} Hz + {} Hz, {:.1}s",
                coppa_engine::TUNE_TONE_LOW_HZ,
                coppa_engine::TUNE_TONE_HIGH_HZ,
                seconds
            ),
        }
    }

    let samples = core.tune_tone(seconds, single);
    if verbosity != Verbosity::Quiet {
        eprintln!("Generated {} audio samples", samples.len());
    }

    match output {
        Some(target) => write_audio_output(target, &samples, core.config().sample_rate, verbosity)?,
        None => {
            // Offline WAV rendering (the `Some` arm) emits no RF and is never
            // gated; live transmission is (COP-40).
            let gate = LiveTxGate {
                profile: profile.unwrap_or("default"),
                occupied_hz: tune_occupied_hz(single),
                stated: rig.frequency_evidence()?,
            };
            #[cfg(feature = "cpal-backend")]
            {
                let ptt = rig.ptt_method()?;
                transmit_live(
                    &samples,
                    core.config().sample_rate,
                    AudioOutputSelection::from_rig(rig),
                    ptt,
                    &rig.rigctld.value,
                    rig.lead.value,
                    rig.tail.value,
                    gate,
                    verbosity,
                )?;
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                // No live audio in this build, so nothing reaches the air,
                // but the same arguments are validated: a typo'd --ptt is
                // rejected exactly as the audio build rejects it, and a
                // wideband request is still refused, not silently accepted.
                rig.ptt_method()?;
                cli_gate_decision(gate.profile, gate.occupied_hz, None, gate.stated)?;
                let _ = rig;
                if verbosity != Verbosity::Quiet {
                    eprintln!(
                        "Live audio output not available (compile with cpal-backend feature)"
                    );
                }
            }
        }
    }

    Ok(())
}

/// `coppa rx`: the one receiver (COP-56). Streams audio -- a WAV file named
/// by FILE or `-i/--input`, or a live capture device when neither is given --
/// through `CoppaCore::push_samples` (the same `StreamingReceiver`-based path
/// the daemon uses), printing each decoded frame's payload + SNR as it
/// completes. The deprecated `coppa listen` alias runs this too.
///
/// `--duration <SECONDS>` (0 = no limit) decodes only the first N seconds of a
/// file, or stops a live capture after N seconds of wall clock; without it a
/// live capture runs until Ctrl+C.
///
/// Output is COP-13's grammar: `push_samples` never forces a UTF-8
/// conversion (Phase 4 Task 3.5), so frame payloads are raw bytes. Valid
/// UTF-8 is displayed as escaped, quoted text, anything else as labeled hex.
/// `--hex` forces hex; `--raw` prints only lowercase hex, one line per frame.
fn cmd_rx(args: &RxArgs, verbosity: Verbosity) -> Result<()> {
    let config = resolve_config(args.profile.as_deref())?;
    let mut core = CoppaCore::with_config(config);
    let output_mode = RxOutputMode::from(args);

    if verbosity.is_verbose() {
        eprintln!("[verbose] sample_rate: {}", core.config().sample_rate);
    }

    if args.sample_format.is_some() && args.path() != Some("-") {
        anyhow::bail!(
            "--sample-format applies only to raw PCM on stdin (rx -); a WAV file's header sets its format"
        );
    }
    // Before the first status line: callers (and the tests) may signal as
    // soon as they see it.
    let interrupt = interrupt::Interrupt::install()?;
    let chatty = status_enabled(verbosity, args.raw, args.json);
    #[cfg(not(any(feature = "file-backend", feature = "cpal-backend")))]
    let _ = (&interrupt, chatty);

    if args.path() == Some("-") {
        #[cfg(feature = "file-backend")]
        {
            use coppa_audio::AudioSource;
            use std::io::IsTerminal;
            refuse_raw_stdin(std::io::stdin().is_terminal())?;
            let rate = core.config().sample_rate;
            let format = coppa_audio::PcmFormat::from(args.sample_format.unwrap_or_default());
            let mut source = coppa_audio::RawPcmReader::new(std::io::stdin(), format, rate);
            // Status first: the header check blocks until the producer
            // writes, and the operator should see what coppa is waiting for.
            if chatty {
                eprintln!(
                    "Reading raw {format} audio (mono, {rate} Hz) from stdin (Ctrl+C to stop)..."
                );
            }
            if source.starts_with_wav_header()? {
                anyhow::bail!(
                    "stdin starts with a WAV header; pass the file instead (coppa rx FILE) or send headerless PCM"
                );
            }
            source.start()?;
            let total_samples =
                (args.duration > 0).then(|| file_sample_limit(usize::MAX, args.duration, rate));
            let summary = stream_decode(
                &mut core,
                &mut source,
                StopWhen {
                    total_samples,
                    interrupt: Some(&interrupt),
                    eof_on_empty_read: true,
                    ..StopWhen::default()
                },
                output_mode,
                args.profile.as_deref().unwrap_or("default"),
                verbosity,
            )?;
            source.stop()?;
            if chatty && summary.interrupted {
                eprintln!("{}", summary.stop_line(rate));
            }
            summary.finish(|| {
                format!(
                    "no frames decoded from stdin ({} samples at {} Hz; profile {}); check for a complete Coppa frame and a matching --profile",
                    summary.consumed_samples,
                    rate,
                    args.profile.as_deref().unwrap_or("default")
                )
            })?;
        }
        #[cfg(not(feature = "file-backend"))]
        {
            let _ = &mut core;
            if args.json {
                anyhow::bail!("Raw stdin input not available (compile with file-backend feature)");
            }
            if verbosity != Verbosity::Quiet {
                eprintln!("Raw stdin input not available (compile with file-backend feature)");
            }
        }
    } else if let Some(path) = args.path() {
        #[cfg(feature = "file-backend")]
        {
            use coppa_audio::{file_backend::WavSource, AudioSource};
            let mut source = WavSource::open(path)?;
            if source.sample_rate() != core.config().sample_rate {
                anyhow::bail!(
                    "WAV sample rate {} Hz does not match modem sample rate {} Hz; resample the WAV before using coppa rx",
                    source.sample_rate(),
                    core.config().sample_rate
                );
            }
            let total = source.total_samples();
            // The checked WAV rate is the engine rate; no resampling occurs.
            let limit = file_sample_limit(total, args.duration, source.sample_rate());
            if chatty {
                if limit < total {
                    eprintln!(
                        "Reading the first {} s ({} of {} samples) from {}",
                        args.duration, limit, total, path
                    );
                } else {
                    eprintln!("Reading {} samples from {}", total, path);
                }
            }
            source.start()?;
            let summary = stream_decode(
                &mut core,
                &mut source,
                StopWhen {
                    total_samples: Some(limit),
                    interrupt: Some(&interrupt),
                    ..StopWhen::default()
                },
                output_mode,
                args.profile.as_deref().unwrap_or("default"),
                verbosity,
            )?;
            source.stop()?;
            if chatty && summary.interrupted {
                eprintln!("{}", summary.stop_line(source.sample_rate()));
            }
            summary.finish(|| {
                format!(
                    "no frames decoded from {} ({} samples at {} Hz; profile {}); check for a complete Coppa frame and a matching --profile",
                    path,
                    summary.consumed_samples,
                    source.sample_rate(),
                    args.profile.as_deref().unwrap_or("default")
                )
            })?;
        }
        #[cfg(not(feature = "file-backend"))]
        {
            let _ = path;
            let _ = &mut core;
            if args.json {
                anyhow::bail!("File input not available (compile with file-backend feature)");
            }
            if verbosity != Verbosity::Quiet {
                eprintln!("File input not available (compile with file-backend feature)");
            }
        }
    } else {
        #[cfg(feature = "cpal-backend")]
        {
            use coppa_audio::AudioSource;
            let engine_rate = core.config().sample_rate;
            let selected_device =
                resolve_cli_input_device(args.device.as_deref(), args.device_id.as_deref())?;
            let mut source = coppa_audio::LiveSource::new(
                selected_device,
                engine_rate,
                engine_rate,
                8192,
                args.input_channel,
            )?;
            if source.hardware_rate() != engine_rate {
                style::warn(format_args!(
                    "input audio device {} uses {} Hz hardware rate (requested {} Hz, engine {} Hz)",
                    source.device_name(),
                    source.hardware_rate(),
                    engine_rate,
                    engine_rate
                ));
            }
            if verbosity.is_verbose() {
                eprintln!(
                    "[verbose] Input audio: {} (hardware {} Hz, engine {} Hz)",
                    source.device_name(),
                    source.hardware_rate(),
                    engine_rate
                );
            }
            source.start()?;
            if chatty {
                if args.duration > 0 {
                    eprintln!(
                        "Listening for live audio for {} s (Ctrl+C to stop)...",
                        args.duration
                    );
                } else {
                    eprintln!("Listening for live audio (Ctrl+C to stop)...");
                }
            }
            let deadline = live_deadline(std::time::Instant::now(), args.duration);
            let summary = stream_decode(
                &mut core,
                &mut source,
                StopWhen {
                    deadline,
                    interrupt: Some(&interrupt),
                    ..StopWhen::default()
                },
                output_mode,
                args.profile.as_deref().unwrap_or("default"),
                verbosity,
            )?;
            source.stop()?;
            if chatty {
                eprintln!("{}", summary.stop_line(engine_rate));
            }
            summary.check_failure()?;
        }
        #[cfg(not(feature = "cpal-backend"))]
        {
            let _ = (
                &args.device,
                &args.device_id,
                args.input_channel,
                args.duration,
            );
            let _ = &mut core;
            if args.json {
                anyhow::bail!("Live audio input not available (compile with cpal-backend feature)");
            }
            if verbosity != Verbosity::Quiet {
                eprintln!("Live audio input not available (compile with cpal-backend feature)");
            }
        }
    }

    Ok(())
}

/// How many samples of a `total`-sample file `--duration <seconds>` lets
/// `coppa rx` decode: all of them when `seconds` is 0 or covers the file.
#[cfg_attr(not(feature = "file-backend"), allow(dead_code))]
fn file_sample_limit(total: usize, seconds: u64, sample_rate: u32) -> usize {
    if seconds == 0 {
        return total;
    }
    let cap = seconds.saturating_mul(u64::from(sample_rate));
    usize::try_from(cap).map_or(total, |cap| cap.min(total))
}

/// When a live `coppa rx --duration <seconds>` stops. `None` means no limit:
/// 0, or a duration too large for `Instant` (effectively forever).
#[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))]
fn live_deadline(now: std::time::Instant, seconds: u64) -> Option<std::time::Instant> {
    if seconds == 0 {
        return None;
    }
    now.checked_add(std::time::Duration::from_secs(seconds))
}

/// Trailing silence (samples) fed to `core` once `stream_decode` stops (end
/// of a WAV file, its `--duration` cap, or a live deadline), so a frame whose
/// nominal end coincides with (or is close to) that stop can still complete.
///
/// `StreamingReceiver` needs `rx_group_delay` samples past a candidate's
/// nominal end before it will emit that frame -- the RX bandpass filter's
/// group delay for HF profiles (300 samples, `(601-1)/2`; VHF profiles have
/// no RX bandpass and need none) -- see
/// `coppa_protocol::modem::streaming`'s module doc and `header_peek`'s doc.
/// A golden-vector WAV (one frame, no trailing padding baked in) demonstrates
/// this concretely: without this flush, `coppa rx --input testdata/golden/
/// L1_clean.wav` decoded 0 frames even though the identical samples decode
/// cleanly via the batch `CoppaTransceiver::receive` API golden_vectors.rs
/// uses, which has no such requirement. 8192 comfortably covers every
/// profile's `rx_group_delay` with margin.
const TRAILING_FLUSH_SAMPLES: usize = 8192;

/// Push audio from `source` through `core`'s streaming decoder
/// (`CoppaCore::push_samples`) in fixed-size chunks, printing each decoded
/// frame as it completes (see `handle_stream_frame`).
///
/// It stops on the first of three conditions:
/// - `total_samples`, when known (a WAV file, already capped by
///   `--duration`): exactly that many samples are read -- never more, since
///   each read is sized to what the limit leaves -- so the loop terminates
///   without relying on a `read() == 0` EOF signal (live sources also return
///   0 whenever no new audio has arrived yet, which is not EOF).
/// - `deadline` (a live `--duration`): checked after every read, including
///   empty ones, so a stalled device still stops on time.
/// - Neither (a live capture with no `--duration`): runs until the process
///   is interrupted (Ctrl+C), briefly sleeping on an empty read to avoid a
///   busy-spin against the non-blocking device source.
///
/// Every normal return then pushes `TRAILING_FLUSH_SAMPLES` of silence so a
/// frame that ends right at the file's end (or just before a live deadline)
/// can still complete (see that constant's doc).
#[cfg_attr(
    not(any(feature = "file-backend", feature = "cpal-backend")),
    allow(dead_code)
)]
#[derive(Default)]
struct DecodeSummary {
    good_frames: usize,
    failed_frames: usize,
    first_error: Option<String>,
    consumed_samples: usize,
    report_live_errors: bool,
    /// Reading stopped because of Ctrl-C or SIGTERM (COP-86).
    interrupted: bool,
    /// Level warnings printed during the read, such as clipping (COP-86).
    warnings: Vec<String>,
}

#[cfg_attr(
    not(any(feature = "file-backend", feature = "cpal-backend")),
    allow(dead_code)
)]
impl DecodeSummary {
    /// The line an ended capture prints on stderr (COP-86), e.g.
    /// `Stopped after 42.3 s of audio: 1 frame decoded, 0 failed.`
    fn stop_line(&self, sample_rate: u32) -> String {
        let seconds = self.consumed_samples as f64 / f64::from(sample_rate.max(1));
        let noun = if self.good_frames == 1 {
            "frame"
        } else {
            "frames"
        };
        format!(
            "Stopped after {seconds:.1} s of audio: {} {noun} decoded, {} failed.",
            self.good_frames, self.failed_frames
        )
    }

    /// `coppa rx`'s exit for a finished read: a decode failure is an error;
    /// with no frames at all, an input that ended by itself (a file, or stdin
    /// at EOF) is an error too, while a capture the operator stopped is not.
    #[cfg_attr(not(feature = "file-backend"), allow(dead_code))]
    fn finish(&self, no_frames: impl FnOnce() -> String) -> Result<()> {
        self.check_failure()?;
        if self.good_frames == 0 && !self.interrupted {
            anyhow::bail!("{}", no_frames());
        }
        Ok(())
    }

    fn check_failure(&self) -> Result<()> {
        if let Some(error) = &self.first_error {
            anyhow::bail!("{error}");
        }
        Ok(())
    }
}

/// When `stream_decode` stops reading.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(
    not(any(feature = "file-backend", feature = "cpal-backend")),
    allow(dead_code)
)]
struct StopWhen<'a> {
    /// After this many samples (a file, or `rx - -d N`).
    total_samples: Option<usize>,
    /// When Ctrl-C or SIGTERM arrives.
    interrupt: Option<&'a interrupt::Interrupt>,
    /// At this instant (live `--duration`).
    deadline: Option<std::time::Instant>,
    /// At the first empty read: the source returns 0 only at end of stream
    /// (`rx -`), rather than when no audio has arrived yet (live capture).
    eof_on_empty_read: bool,
}

fn stream_decode(
    core: &mut CoppaCore,
    source: &mut dyn coppa_audio::AudioSource,
    stop: StopWhen<'_>,
    output_mode: RxOutputMode,
    profile: &str,
    verbosity: Verbosity,
) -> Result<DecodeSummary> {
    let StopWhen {
        total_samples,
        interrupt,
        deadline,
        eof_on_empty_read,
    } = stop;
    let mut buf = vec![0.0f32; 4096];
    // The daemon's meter and wording: a warning each time the input starts
    // clipping (COP-86). Underdrive is not reported, because it would fire
    // on the silent stretches of ordinary recordings.
    let mut meter = coppa_daemon::level_meter::RxLevelMeter::new(source.sample_rate());
    let mut summary = DecodeSummary {
        // A live capture or a stream (`rx -`, with or without -d) reports
        // failures as they happen; a file reports them at the end.
        report_live_errors: total_samples.is_none() || eof_on_empty_read,
        ..DecodeSummary::default()
    };
    while total_samples.is_none_or(|total| summary.consumed_samples < total) {
        let want = total_samples.map_or(buf.len(), |total| {
            buf.len().min(total - summary.consumed_samples)
        });
        let n = source.read(&mut buf[..want])?;
        if n > 0 {
            summary.consumed_samples += n;
            for update in meter.observe(&buf[..n]) {
                if update.transition == Some(coppa_daemon::level_meter::LevelState::Clipping) {
                    let message = coppa_daemon::level_meter::rx_transition_message(
                        coppa_daemon::level_meter::LevelState::Clipping,
                        &update.reading,
                    );
                    style::warn(message);
                    summary.warnings.push(message.to_string());
                }
            }
            for frame in core.push_samples(&buf[..n]) {
                handle_stream_frame(core, &frame, &mut summary, output_mode, profile, verbosity)?;
            }
        }
        if interrupt.is_some_and(interrupt::Interrupt::is_set) {
            summary.interrupted = true;
            break;
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
        if n == 0 && eof_on_empty_read {
            break;
        }
        if n == 0 && total_samples.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let silence = vec![0.0f32; TRAILING_FLUSH_SAMPLES];
    for frame in core.push_samples(&silence) {
        handle_stream_frame(core, &frame, &mut summary, output_mode, profile, verbosity)?;
    }

    Ok(summary)
}

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn format_decoded(bytes: &[u8], snr_db: f32, hex: bool) -> String {
    if !hex {
        if let Ok(text) = std::str::from_utf8(bytes) {
            // Debug formatting escapes controls in untrusted received text.
            return format!("Decoded: {:?}  (SNR: {:.1} dB)", text, snr_db);
        }
    }
    format!(
        "Decoded (hex): {}  (SNR: {:.1} dB)",
        hex_string(bytes),
        snr_db
    )
}

/// Print one decoded `StreamFrame` (`CoppaCore::push_samples`'s per-frame
/// result). Payloads are raw bytes -- never UTF-8-forced, see
/// `coppa_engine::StreamFrame::payload`'s doc. Valid UTF-8 is printed as
/// escaped, quoted text; other payloads fall back to labeled lowercase hex.
/// `hex` forces labeled hex. `raw` prints only the lowercase hex string,
/// one per line, on stdout, even when `hex` is also set.
/// Decode failures go to stderr (never stdout, so they never corrupt a
/// `--raw` or `--json` capture). Live decode failures are printed at once,
/// including under `--quiet`; every failure is kept in `summary` for a
/// final nonzero exit.
fn handle_stream_frame(
    core: &CoppaCore,
    frame: &coppa_engine::StreamFrame,
    summary: &mut DecodeSummary,
    output_mode: RxOutputMode,
    profile: &str,
    verbosity: Verbosity,
) -> Result<()> {
    match &frame.payload {
        Ok(bytes) => {
            if core.appears_compressed_with_wrong_profile(bytes) {
                summary.failed_frames += 1;
                let error = format!(
                    "decoded payload appears compressed but profile {profile} does not enable compression; try --profile HF_STANDARD for HF audio"
                );
                if summary.report_live_errors {
                    eprintln!("{error}");
                }
                summary.first_error.get_or_insert(error);
                return Ok(());
            }
            summary.good_frames += 1;
            match output_mode {
                RxOutputMode::Raw => emit(hex_string(bytes))?,
                RxOutputMode::Text { hex } => emit(format_decoded(bytes, frame.snr_db, hex))?,
                RxOutputMode::Json => write_json(&RxFrameOutput::from_frame(frame, bytes))?,
            }
            if verbosity.is_verbose() {
                eprintln!(
                    "[verbose] level={} cfo_hz={:.1} frame_start={}",
                    frame.speed_level, frame.cfo_hz, frame.frame_start
                );
            }
            if verbosity.is_extended() {
                eprintln!(
                    "[verbose] recommended_level={} delay_spread_ms={:.1}",
                    frame.recommended_level, frame.delay_spread_ms
                );
            }
        }
        Err(e) => {
            summary.failed_frames += 1;
            let error = format!("payload decode failed: {e}");
            if summary.report_live_errors {
                eprintln!("{error}");
            }
            summary.first_error.get_or_insert(error);
        }
    }
    Ok(())
}

fn cmd_loopback(
    message: &str,
    profile: Option<&str>,
    level: Option<u8>,
    verbosity: Verbosity,
) -> Result<()> {
    if verbosity != Verbosity::Quiet {
        emit("Coppa Loopback Test")?;
        emit("===================")?;
        emit(format_args!("Input: \"{}\"", message))?;
    }

    let core = build_core(profile, level)?;

    if verbosity.is_verbose() {
        eprintln!(
            "[verbose] Config: sample_rate={}, speed_level={}",
            core.config().sample_rate,
            core.config().speed_level,
        );
    }

    let samples = core.encode(message)?;
    if verbosity != Verbosity::Quiet {
        emit(format_args!("Encoded: {} samples", samples.len()))?;
    }

    let decoded = core.decode(&samples)?;
    if verbosity != Verbosity::Quiet {
        emit(format_args!("Decoded: \"{}\"", decoded))?;
    }

    if message == decoded {
        if verbosity != Verbosity::Quiet {
            emit("PASS: Loopback test successful!")?;
        }
    } else {
        // Best effort: a closed stdout must not turn this failure into exit 0.
        let _ = emit("FAIL: Messages don't match!")
            .and_then(|()| emit(format_args!("  Expected: \"{}\"", message)))
            .and_then(|()| emit(format_args!("  Got:      \"{}\"", decoded)));
        std::process::exit(1);
    }

    Ok(())
}

fn cmd_devices(json: bool) -> Result<()> {
    let devices = coppa_audio::list_devices()?;
    let support = coppa_audio::list_device_support(DEVICES_RATE_HZ);
    let defaults = coppa_audio::default_devices();
    if json {
        let output: Vec<_> = devices
            .iter()
            .map(|device| DeviceOutput::new(device, &support, &defaults))
            .collect();
        return write_json(&output);
    }
    for line in device_lines(&devices, &support, &defaults) {
        emit(line)?;
    }
    Ok(())
}

fn device_lines(
    devices: &[coppa_audio::AudioDevice],
    support: &Result<Vec<coppa_audio::DeviceSupport>>,
    defaults: &coppa_audio::DefaultDevices,
) -> Vec<String> {
    let mut lines = vec!["Available audio devices:".to_string()];
    if devices.is_empty() {
        lines.push("  (none found - CPAL backend may not be available)".to_string());
        return lines;
    }
    lines.extend(devices.iter().map(|dev| {
        let rate = match rate_support(dev, support) {
            None => "unknown",
            Some((Some(true), Some(true))) => "in+out",
            Some((Some(true), _)) => "in",
            Some((_, Some(true))) => "out",
            Some(_) => "no",
        };
        format!(
            "  {} [id: {}] (in: {}ch, out: {}ch, 48 kHz: {rate})",
            dev.name,
            dev.id.as_deref().unwrap_or("unavailable"),
            dev.input_channels,
            dev.output_channels,
        )
    }));
    for (label, default) in [("input", &defaults.input), ("output", &defaults.output)] {
        lines.push(match default {
            Some(device) => format!(
                "Default {label}: {} [id: {}]",
                device.name,
                device.id.as_deref().unwrap_or("unavailable")
            ),
            None => format!("Default {label}: none"),
        });
    }
    lines
}

/// `coppa profiles` (and the deprecated `coppa config`): print the speed ladder
/// and the named profiles, one speed level, or one profile's detail; `--json`
/// emits the same selection as JSON.
fn cmd_profiles(args: &ProfilesArgs) -> Result<()> {
    if args.json {
        return write_json_profiles(args);
    }
    emit_raw(render_profiles(args)?.as_bytes())
}

/// `profiles --json`: an array of every named profile, one profile object with
/// `-p`, or one speed-level object with `--level`.
fn write_json_profiles(args: &ProfilesArgs) -> Result<()> {
    if let Some(name) = args.profile.as_deref() {
        resolve_config(Some(name))?;
        let resolved = coppa_engine::profiles::get_profile(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown profile: {name}"))?;
        return write_json(&ProfileOutput::from_profile(resolved)?);
    }
    if let Some(level) = args.level {
        let row = coppa_engine::ladder::level_info(level)
            .ok_or_else(|| anyhow::anyhow!("no speed level {level}"))?;
        return write_json(&LevelOutput::from(&row));
    }
    let profiles: Vec<_> = named_profiles()
        .into_iter()
        .map(ProfileOutput::from_profile)
        .collect::<Result<_>>()?;
    write_json(&profiles)
}

/// Render `coppa profiles`. Every number comes from `coppa_engine::ladder` or
/// the profile constants, never from text here, so the output cannot drift from
/// what `tx` enforces (COP-55: `config` used to print a hand-typed max payload of
/// 64 bytes for a profile whose real limit is 56).
fn render_profiles(args: &ProfilesArgs) -> Result<String> {
    let ladder = coppa_engine::ladder::speed_ladder();
    if let Some(name) = args.profile.as_deref() {
        return render_profile_detail(name, &ladder);
    }
    let mut out = String::new();
    match args.level {
        Some(level) => {
            let row = ladder
                .iter()
                .find(|row| row.level == level)
                .ok_or_else(|| anyhow::anyhow!("no speed level {level}"))?;
            render_ladder_header(&mut out, &ladder);
            render_ladder_row(&mut out, row);
            render_payload_budget_note(&mut out, true);
        }
        None => {
            render_ladder(&mut out, &ladder);
            out.push('\n');
            render_profile_table(&mut out, &ladder);
        }
    }
    Ok(out)
}

/// A profile's band label and its cell on that band, if the band carries the
/// profile's level. `EngineConfig::from_profile` always pins HF or VHF.
fn profile_on_band(
    p: &coppa_engine::Profile,
    ladder: &[coppa_engine::ladder::LevelInfo],
) -> (&'static str, Option<coppa_engine::ladder::LevelOnBand>) {
    let row = ladder.iter().find(|row| row.level == p.speed_level);
    match EngineConfig::from_profile(p).ofdm_band {
        OfdmBand::Vhf => ("VHF", row.map(|row| row.vhf)),
        _ => ("HF", row.and_then(|row| row.hf)),
    }
}

/// Every named profile, in `list_profiles()` order.
fn named_profiles() -> Vec<&'static coppa_engine::Profile> {
    coppa_engine::profiles::list_profiles()
        .iter()
        .filter_map(|name| coppa_engine::profiles::get_profile(name))
        .collect()
}

/// Names of the profiles bound to the VHF band.
fn vhf_profile_names() -> Vec<&'static str> {
    named_profiles()
        .into_iter()
        .filter(|p| EngineConfig::from_profile(p).ofdm_band == OfdmBand::Vhf)
        .map(|p| p.name)
        .collect()
}

/// The first profile listed before `p` that builds the same engine, i.e. is
/// the same on the air. Derived, so the label disappears if they diverge.
fn same_on_air_as(p: &coppa_engine::Profile) -> Option<&'static str> {
    let mine = EngineConfig::from_profile(p);
    named_profiles()
        .into_iter()
        .take_while(|other| other.name != p.name)
        .find(|other| {
            let theirs = EngineConfig::from_profile(other);
            theirs.speed_level == mine.speed_level
                && theirs.sample_rate == mine.sample_rate
                && theirs.compression_enabled == mine.compression_enabled
                && theirs.cp_mode == mine.cp_mode
                && theirs.ofdm_band == mine.ofdm_band
        })
        .map(|other| other.name)
}

/// "A", "A and B", "A, B and C".
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn airtime_ms(airtime_s: f64) -> String {
    format!("{} ms", (airtime_s * 1000.0).round() as u32)
}

fn render_payload_budget_note(out: &mut String, ladder: bool) {
    out.push('\n');
    if ladder {
        out.push_str("MAX BYTES is the payload budget per codeword at the listed speed level.\n");
    }
    out.push_str(concat!(
        "Payload budgets are bytes after compression, before FEC, per codeword.\n",
        "They include any transport headers, but exclude the per-codeword CRC.\n",
        "With compression enabled, how much input fits depends on its contents.\n",
        "These are not limits on a daemon host write. The daemon chunks stream data\n",
        "for the current speed level and may use multiple codewords per frame.\n",
        "KISS datagrams have a separate limit; see docs/RETICULUM.md.\n",
    ));
}

fn render_ladder_header(out: &mut String, ladder: &[coppa_engine::ladder::LevelInfo]) {
    let hf_hz = ladder
        .iter()
        .find_map(|row| row.hf)
        .map_or(0, |hf| hf.bandwidth_hz);
    let vhf_hz = ladder.first().map_or(0, |row| row.vhf.bandwidth_hz);
    let vhf_header = format!("VHF, {vhf_hz} Hz");
    out.push_str(&format!("{:41}AIR TIME, 1-CODEWORD FRAME\n", ""));
    out.push_str(&format!(
        "{:<7}{:<12}{:<11}{:<11}{:<15}{}\n",
        "LEVEL",
        "MODULATION",
        "CODE RATE",
        "MAX BYTES",
        format!("HF, {hf_hz} Hz"),
        vhf_header,
    ));
}

fn render_ladder_row(out: &mut String, row: &coppa_engine::ladder::LevelInfo) {
    let hf = row
        .hf
        .map_or_else(|| "VHF only".to_string(), |hf| airtime_ms(hf.airtime_s));
    out.push_str(&format!(
        "{:<7}{:<12}{:<11}{:<11}{:<15}{}\n",
        row.level,
        row.modulation,
        format!("{}/{}", row.code_rate.0, row.code_rate.1),
        row.max_payload_bytes,
        hf,
        airtime_ms(row.vhf.airtime_s),
    ));
}

fn render_ladder(out: &mut String, ladder: &[coppa_engine::ladder::LevelInfo]) {
    let hf_top = ladder
        .iter()
        .filter(|row| row.hf.is_some())
        .map(|row| row.level)
        .max()
        .unwrap_or(0);
    let vhf_top = ladder.iter().map(|row| row.level).max().unwrap_or(0);
    let vhf_names = vhf_profile_names();
    let carries = if vhf_names.len() == 1 {
        "carries"
    } else {
        "carry"
    };
    out.push_str("Speed levels: pick one with --level N on tx or loopback. HF profiles carry\n");
    out.push_str(&format!(
        "levels 1-{hf_top}; {} {carries} 1-{vhf_top}. coppa rx detects each frame's level itself.\n\n",
        join_and(&vhf_names.iter().map(|n| n.to_string()).collect::<Vec<_>>()),
    ));
    render_ladder_header(out, ladder);
    for row in ladder {
        render_ladder_row(out, row);
    }
    render_payload_budget_note(out, true);
    let vhf_only: Vec<String> = ladder
        .iter()
        .filter(|row| row.hf.is_none())
        .map(|row| row.level.to_string())
        .collect();
    if !vhf_only.is_empty() {
        let (noun, verb, pronoun) = if vhf_only.len() == 1 {
            ("Level", "is", "it does")
        } else {
            ("Levels", "are", "they do")
        };
        out.push_str(&format!(
            "{noun} {} {verb} VHF only: {pronoun} not decode reliably through an HF SSB filter.\n",
            join_and(&vhf_only)
        ));
    }
}

fn render_profile_table(out: &mut String, ladder: &[coppa_engine::ladder::LevelInfo]) {
    out.push_str(
        "Profiles: pick one with --profile NAME; --level N changes its level, not its band.\n",
    );
    out.push_str(&format!(
        "{:<13}{:<6}{:<7}{:<13}{}\n",
        "PROFILE", "BAND", "LEVEL", "COMPRESSION", "DESCRIPTION"
    ));
    let mut wide = Vec::new();
    for p in named_profiles() {
        let (band, cell) = profile_on_band(p, ladder);
        let description = match same_on_air_as(p) {
            Some(twin) => format!("Same on the air as {twin}"),
            None => p.description.to_string(),
        };
        out.push_str(&format!(
            "{:<13}{:<6}{:<7}{:<13}{}\n",
            p.name,
            band,
            p.speed_level,
            if p.compression { "on" } else { "off" },
            description,
        ));
        if let Some(cell) = cell {
            if cell.bandwidth_hz > coppa_radio::band::HF_OCCUPIED_BANDWIDTH_CAP_HZ {
                wide.push((p.name, cell.bandwidth_hz));
            }
        }
    }
    if !wide.is_empty() {
        out.push('\n');
    }
    for (name, hz) in wide {
        out.push_str(&format!(
            "{name} is {hz} Hz wide and transmits live only at or above {} MHz.\n",
            coppa_radio::band::US_WIDEBAND_MIN_HZ as f64 / 1_000_000.0
        ));
    }
}

fn render_profile_detail(name: &str, ladder: &[coppa_engine::ladder::LevelInfo]) -> Result<String> {
    // Same error text (and exit code) `tx`/`rx`/`loopback` give for an unknown name.
    resolve_config(Some(name))?;
    let p = coppa_engine::profiles::get_profile(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown profile: {name}"))?;
    let (band, cell) = profile_on_band(p, ladder);
    let cell = cell.ok_or_else(|| {
        anyhow::anyhow!(
            "profile {} is level {}, which its band does not carry",
            p.name,
            p.speed_level
        )
    })?;
    let row = ladder
        .iter()
        .find(|row| row.level == p.speed_level)
        .ok_or_else(|| anyhow::anyhow!("no speed level {}", p.speed_level))?;
    let max_bytes = p
        .max_payload()
        .ok_or_else(|| anyhow::anyhow!("no speed level {}", p.speed_level))?;
    let mut out = String::new();
    out.push_str(&format!("Profile: {}\n", p.name));
    out.push_str(&format!("  Description: {}\n", p.description));
    if let Some(twin) = same_on_air_as(p) {
        out.push_str(&format!("  Same on the air as {twin}\n"));
    }
    out.push_str(&format!("  Band: {band}, {} Hz\n", cell.bandwidth_hz));
    out.push_str(&format!(
        "  Speed level: {} ({}, code rate {}/{})\n",
        p.speed_level, row.modulation, row.code_rate.0, row.code_rate.1
    ));
    out.push_str(&format!("  Payload bytes per codeword: {max_bytes}\n"));
    out.push_str(&format!(
        "  Air time, 1-codeword frame: {}\n",
        airtime_ms(cell.airtime_s)
    ));
    out.push_str(&format!(
        "  Compression: {}\n",
        if p.compression { "on" } else { "off" }
    ));
    out.push_str(&format!("  Sample rate: {} Hz\n", p.sample_rate));
    render_payload_budget_note(&mut out, false);
    Ok(out)
}

#[cfg(feature = "kiss-tnc")]
fn parse_digipeat_alias(s: &str) -> Result<(String, u8)> {
    let invalid = || {
        anyhow::anyhow!(
            "--digipeat expects CALL[-SSID] with a 1-6 character callsign and SSID 0-15, got '{s}'"
        )
    };
    let (call, suffix) = s.split_once('-').map_or((s, None), |(c, n)| (c, Some(n)));
    if call.is_empty() || call.len() > 6 || !call.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(invalid());
    }
    let ssid = match suffix {
        None => 0,
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => n
            .parse::<u8>()
            .ok()
            .filter(|n| *n <= 15)
            .ok_or_else(invalid)?,
        Some(_) => return Err(invalid()),
    };
    Ok((call.to_ascii_uppercase(), ssid))
}

#[cfg(feature = "kiss-tnc")]
#[allow(clippy::too_many_arguments)]
fn cmd_tnc(
    port: u16,
    device: Option<String>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    input_channel: u16,
    output_channel: u16,
    rig: Option<String>,
    vox: bool,
    fx25: bool,
    digipeat: Option<String>,
) -> Result<()> {
    let digipeat_alias = digipeat.as_deref().map(parse_digipeat_alias).transpose()?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let config = coppa_daemon::tnc::TncConfig {
            digipeat_alias,
            kiss_port: port,
            audio_device: device,
            input_device_id,
            output_device_id,
            input_channel,
            output_channel,
            rig_address: rig,
            vox_mode: vox,
            sample_rate: 48000,
            ..Default::default()
        };
        if fx25 {
            coppa_daemon::tnc::run_tnc_with_fx25(config).await
        } else {
            coppa_daemon::tnc::run_tnc(config).await
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_json_escapes_names_and_empty_lists() {
        let empty: Vec<DeviceOutput> = Vec::new();
        assert_eq!(serde_json::to_string(&empty).unwrap(), "[]");
        let device = coppa_audio::AudioDevice {
            name: "USB \"mic\"\nline".into(),
            id: Some("alsa:radio-a".into()),
            max_sample_rate: 48_000,
            input_channels: 1,
            output_channels: 2,
        };
        let unknown = Err(anyhow::anyhow!("listing failed"));
        let value = serde_json::to_value(DeviceOutput::new(&device, &unknown, &Default::default()))
            .unwrap();
        assert_eq!(value["name"], device.name);
        assert_eq!(value["id"], "alsa:radio-a");
        assert_eq!(value["max_sample_rate_hz"], 48_000);
        assert_eq!(value["input_channels"], 1);
        assert_eq!(value["output_channels"], 2);
        assert_eq!(value["input_supports_48000_hz"], serde_json::Value::Null);
        assert_eq!(value["output_supports_48000_hz"], serde_json::Value::Null);
        assert_eq!(value["is_default_input"], false);
        assert_eq!(value["is_default_output"], false);
        assert_eq!(value.as_object().unwrap().len(), 9);

        let support = Ok(vec![coppa_audio::DeviceSupport {
            name: device.name.clone(),
            id: "alsa:radio-a".into(),
            input: Some(coppa_audio::Support::Yes(coppa_audio::OpenedFormat {
                channels: 1,
                sample_rate: 48_000,
            })),
            output: None,
        }]);
        let defaults = coppa_audio::DefaultDevices {
            input: Some(coppa_audio::DefaultDevice {
                name: "USB".into(),
                id: Some("alsa:radio-a".into()),
            }),
            output: None,
        };
        let value = serde_json::to_value(DeviceOutput::new(&device, &support, &defaults)).unwrap();
        assert_eq!(value["input_supports_48000_hz"], true);
        assert_eq!(value["output_supports_48000_hz"], serde_json::Value::Null);
        assert_eq!(value["is_default_input"], true);
        assert_eq!(value["is_default_output"], false);
    }

    #[test]
    fn rx_json_non_finite_measurements_are_null() {
        let frame = coppa_engine::StreamFrame {
            payload: Ok(vec![0xff, 0x00]),
            snr_db: f32::NAN,
            cfo_hz: f32::INFINITY,
            frame_start: 270,
            speed_level: 1,
            recommended_level: 1,
            delay_spread_ms: 0.0,
        };
        let value = serde_json::to_value(RxFrameOutput::from_frame(&frame, &[0xff, 0x00])).unwrap();
        assert!(value["payload_text"].is_null());
        assert_eq!(value["payload_hex"], "ff00");
        assert!(value["snr_db"].is_null());
        assert!(value["cfo_hz"].is_null());
    }

    #[test]
    fn format_decoded_text() {
        assert_eq!(
            format_decoded(b"CQ CQ CQ DE VK2ABC K", 25.7, false),
            "Decoded: \"CQ CQ CQ DE VK2ABC K\"  (SNR: 25.7 dB)"
        );
    }

    #[test]
    fn format_decoded_binary() {
        assert_eq!(
            format_decoded(&[0xfe, 0x41], 25.7, false),
            "Decoded (hex): fe41  (SNR: 25.7 dB)"
        );
    }

    #[test]
    fn format_decoded_forced_hex() {
        assert_eq!(
            format_decoded(b"CQ CQ CQ DE VK2ABC K", 25.7, true),
            "Decoded (hex): 435120435120435120444520564b32414243204b  (SNR: 25.7 dB)"
        );
    }

    #[test]
    fn format_decoded_escapes_terminal_controls() {
        let output = format_decoded(b"a\x1b[31m", 25.7, false);
        assert_eq!(output, "Decoded: \"a\\u{1b}[31m\"  (SNR: 25.7 dB)");
        assert!(!output.as_bytes().contains(&0x1b));
    }

    #[test]
    fn format_decoded_unicode_and_empty_text() {
        assert_eq!(
            format_decoded("73 de Zoë ✓".as_bytes(), 25.7, false),
            "Decoded: \"73 de Zoë ✓\"  (SNR: 25.7 dB)"
        );
        assert_eq!(
            format_decoded(b"", 25.7, false),
            "Decoded: \"\"  (SNR: 25.7 dB)"
        );
    }

    #[test]
    fn test_loopback_roundtrip() {
        cmd_loopback("Hello", None, None, Verbosity::Normal).expect("loopback should succeed");
    }

    #[test]
    fn test_loopback_short_message() {
        cmd_loopback("Hi", None, None, Verbosity::Normal).expect("short loopback should succeed");
    }

    #[test]
    fn test_loopback_with_profile() {
        cmd_loopback("Test", Some("HF_STANDARD"), None, Verbosity::Normal)
            .expect("loopback with profile should succeed");
    }

    #[test]
    fn test_loopback_quiet() {
        cmd_loopback("Quiet", None, None, Verbosity::Quiet).expect("quiet loopback should succeed");
    }

    #[test]
    fn test_loopback_verbose() {
        cmd_loopback("Verbose", None, None, Verbosity::Verbose(1))
            .expect("verbose loopback should succeed");
    }

    #[test]
    fn test_tx_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tx.wav");
        let path_str = path.to_str().unwrap();
        cmd_tx(
            "test message",
            Some(OutputTarget {
                path: path_str,
                format: SampleFormat::F32le,
            }),
            None,
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:4532", 50, 200, None, None),
            Verbosity::Normal,
        )
        .expect("tx to file should succeed");
        #[cfg(feature = "file-backend")]
        {
            assert!(path.exists(), "WAV file should be created");
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn test_tune_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tune.wav");
        let path_str = path.to_str().unwrap();
        cmd_tune(
            0.5,
            None,
            Some(OutputTarget {
                path: path_str,
                format: SampleFormat::F32le,
            }),
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:4532", 50, 200, None, None),
            Verbosity::Normal,
        )
        .expect("tune to file should succeed");
        #[cfg(feature = "file-backend")]
        {
            assert!(path.exists(), "WAV file should be created");
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn test_tune_single_tone_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tune_single.wav");
        let path_str = path.to_str().unwrap();
        cmd_tune(
            0.5,
            Some(1500.0),
            Some(OutputTarget {
                path: path_str,
                format: SampleFormat::F32le,
            }),
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:4532", 50, 200, None, None),
            Verbosity::Quiet,
        )
        .expect("single-tone tune to file should succeed");
        #[cfg(feature = "file-backend")]
        {
            assert!(path.exists(), "WAV file should be created");
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn test_tx_verbose_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tx_cs.wav");
        let path_str = path.to_str().unwrap();
        cmd_tx(
            "CQ CQ",
            Some(OutputTarget {
                path: path_str,
                format: SampleFormat::F32le,
            }),
            None,
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:4532", 50, 200, None, None),
            Verbosity::Verbose(1),
        )
        .expect("tx with callsign should succeed");
        #[cfg(feature = "file-backend")]
        {
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn test_rx_nonexistent_file() {
        #[cfg(feature = "file-backend")]
        {
            let args = RxArgs {
                file: Some("/tmp/coppa_nonexistent_12345.wav".into()),
                ..Default::default()
            };
            let result = cmd_rx(&args, Verbosity::Normal);
            assert!(result.is_err(), "rx of nonexistent file should error");
        }
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("coppa").chain(args.iter().copied()))
    }

    fn profiles_args(args: &[&str]) -> ProfilesArgs {
        match parse(args).expect("valid profiles args").command {
            Commands::Profiles(args) | Commands::Config(args) => args,
            _ => panic!("expected profiles or config"),
        }
    }

    #[test]
    fn profiles_parses_profile_and_level() {
        assert_eq!(profiles_args(&["profiles"]), ProfilesArgs::default());
        assert_eq!(
            profiles_args(&["profiles", "-p", "HF_ROBUST"])
                .profile
                .as_deref(),
            Some("HF_ROBUST")
        );
        assert_eq!(profiles_args(&["profiles", "--level", "4"]).level, Some(4));
    }

    #[test]
    fn profiles_rejects_level_outside_1_to_9() {
        for level in ["0", "10"] {
            let err = parse(&["profiles", "--level", level])
                .err()
                .expect("rejected");
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{level}"
            );
        }
    }

    #[test]
    fn profiles_rejects_profile_with_level() {
        let err = parse(&["profiles", "-p", "HF_ROBUST", "--level", "2"])
            .err()
            .expect("rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn config_parses_into_the_same_args_as_profiles() {
        for flags in [vec![], vec!["-p", "HF_ROBUST"], vec!["--level", "9"]] {
            let profiles: Vec<&str> = std::iter::once("profiles").chain(flags.clone()).collect();
            let config: Vec<&str> = std::iter::once("config").chain(flags).collect();
            assert!(matches!(
                parse(&config).unwrap().command,
                Commands::Config(_)
            ));
            assert_eq!(profiles_args(&config), profiles_args(&profiles));
        }
    }

    #[test]
    fn top_level_help_lists_profiles_and_hides_config() {
        use clap::CommandFactory;
        let help = Cli::command().render_help().to_string();
        let subcommands: Vec<&str> = help
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert!(subcommands.contains(&"profiles"), "{help}");
        assert!(!subcommands.contains(&"config"), "{help}");
    }

    fn rendered(args: &[&str]) -> String {
        render_profiles(&profiles_args(args)).expect("render")
    }

    #[test]
    fn profiles_table_lists_every_level_once() {
        let out = rendered(&["profiles"]);
        let ladder = coppa_engine::ladder::speed_ladder();
        let rows: Vec<&str> = out
            .lines()
            .filter(|line| {
                line.split_whitespace()
                    .next()
                    .is_some_and(|first| first.parse::<u8>().is_ok())
            })
            .collect();
        assert_eq!(rows.len(), 9, "{out}");
        for (row, info) in rows.iter().zip(&ladder) {
            let hf = match info.hf {
                Some(hf) => format!("{} ms", (hf.airtime_s * 1000.0).round()),
                None => "VHF only".to_string(),
            };
            let vhf = format!("{} ms", (info.vhf.airtime_s * 1000.0).round());
            let expected = format!(
                "{:<7}{:<12}{:<11}{:<11}{:<15}{}",
                info.level,
                info.modulation,
                format!("{}/{}", info.code_rate.0, info.code_rate.1),
                info.max_payload_bytes,
                hf,
                vhf
            );
            assert_eq!(*row, expected);
        }
        let levels: Vec<u8> = ladder.iter().map(|info| info.level).collect();
        assert_eq!(levels, (1..=9).collect::<Vec<u8>>());
    }

    #[test]
    fn profiles_table_header_names_both_bandwidths() {
        let out = rendered(&["profiles"]);
        assert!(out.contains("HF, 2400 Hz"), "{out}");
        assert!(out.contains("VHF, 5600 Hz"), "{out}");
    }

    #[test]
    fn profiles_table_marks_emergency_as_hf_robust_on_the_air() {
        let out = rendered(&["profiles"]);
        let emergency = out
            .lines()
            .find(|line| line.starts_with("EMERGENCY"))
            .expect("EMERGENCY row");
        assert!(
            emergency.ends_with("Same on the air as HF_ROBUST"),
            "{emergency}"
        );
        let robust = out
            .lines()
            .find(|line| line.starts_with("HF_ROBUST"))
            .expect("HF_ROBUST row");
        assert!(!robust.contains("Same on the air"), "{robust}");
    }

    #[test]
    fn profile_detail_shows_the_enforced_max_payload() {
        for name in coppa_engine::profiles::list_profiles() {
            let p = coppa_engine::profiles::get_profile(name).unwrap();
            let out = rendered(&["profiles", "-p", name]);
            let max = coppa_protocol::modem::max_payload_for_level(p.speed_level).unwrap();
            assert!(
                out.contains(&format!("Payload bytes per codeword: {max}\n")),
                "{out}"
            );
            assert!(!out.contains("ARQ window"), "{out}");
        }
        let robust = rendered(&["profiles", "-p", "HF_ROBUST"]);
        assert!(
            robust.contains("Payload bytes per codeword: 56\n"),
            "{robust}"
        );
        assert!(!robust.contains("64 bytes"), "{robust}");
    }

    #[test]
    fn profiles_level_shows_one_row() {
        let out = rendered(&["profiles", "--level", "4"]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 11, "{out}");
        assert!(lines[0]
            .trim_start()
            .starts_with("AIR TIME, 1-CODEWORD FRAME"));
        assert!(lines[1].starts_with("LEVEL"));
        assert!(lines[2].starts_with("4 "), "{out}");
    }

    #[test]
    fn profiles_unknown_profile_is_an_error() {
        let args = ProfilesArgs {
            profile: Some("BOGUS".to_string()),
            ..ProfilesArgs::default()
        };
        let err = render_profiles(&args).unwrap_err().to_string();
        assert!(err.contains("Unknown profile: BOGUS. Available:"), "{err}");
    }

    #[test]
    fn tx_and_loopback_parse_level() {
        match parse(&["tx", "CQ", "--level", "4"]).unwrap().command {
            Commands::Tx { level, .. } => assert_eq!(level, Some(4)),
            _ => panic!("expected tx"),
        }
        match parse(&["loopback", "CQ", "--level", "9"]).unwrap().command {
            Commands::Loopback { level, .. } => assert_eq!(level, Some(9)),
            _ => panic!("expected loopback"),
        }
    }

    #[test]
    fn tx_and_loopback_reject_level_0_and_10() {
        for cmd in ["tx", "loopback"] {
            for level in ["0", "10"] {
                let err = parse(&[cmd, "CQ", "--level", level])
                    .err()
                    .expect("rejected");
                assert_eq!(
                    err.kind(),
                    clap::error::ErrorKind::ValueValidation,
                    "{cmd} {level}"
                );
            }
        }
    }

    #[test]
    fn build_core_without_level_is_todays_default() {
        let core = build_core(None, None).unwrap();
        assert_eq!(core.config().ofdm_band, OfdmBand::LevelDerived);
        assert_eq!(core.speed_level(), 1);
    }

    #[test]
    fn level_without_profile_stays_on_hf() {
        let core = build_core(None, Some(5)).unwrap();
        assert_eq!(core.speed_level(), 5);
        assert!(!core.on_vhf_profile());
        assert_eq!(core.occupied_bandwidth_hz(), 2400);
    }

    #[test]
    fn level_overrides_the_profile_level_but_keeps_band_and_compression() {
        let core = build_core(Some("HF_STANDARD"), Some(7)).unwrap();
        assert_eq!(core.speed_level(), 7);
        assert!(core.config().compression_enabled);
        assert_eq!(core.occupied_bandwidth_hz(), 2400);

        let core = build_core(Some("VHF_FAST"), Some(3)).unwrap();
        assert_eq!(core.speed_level(), 3);
        assert_eq!(core.occupied_bandwidth_hz(), 5600);
    }

    #[test]
    fn level_9_needs_a_vhf_profile() {
        let err = build_core(Some("hf_robust"), Some(9))
            .expect_err("level 9 on HF refused")
            .to_string();
        assert_eq!(
            err,
            "level 9 is not available on HF_ROBUST, which carries levels 1-8; \
             level 9 is VHF only (use --profile VHF_FAST)"
        );
        let err = build_core(None, Some(9))
            .expect_err("level 9 on the default refused")
            .to_string();
        assert!(err.contains("the default HF profile"), "{err}");
        assert!(build_core(Some("VHF_FAST"), Some(9)).is_ok());
    }

    #[test]
    fn loopback_at_an_explicit_level_round_trips() {
        cmd_loopback("CQ", None, Some(4), Verbosity::Quiet).expect("level 4 loopback");
    }

    #[test]
    fn test_resolve_config_default() {
        let config = resolve_config(None).unwrap();
        assert_eq!(config.sample_rate, 48000);
    }

    #[test]
    fn test_resolve_config_named() {
        let config = resolve_config(Some("HF_ROBUST")).unwrap();
        assert_eq!(config.sample_rate, 48000);
    }

    #[test]
    fn test_resolve_config_unknown() {
        let result = resolve_config(Some("NONEXISTENT"));
        assert!(result.is_err());
    }

    // ── Task 2 (Phase 4): --rigctld / --ptt-lead-ms / --ptt-tail-ms ──────

    #[test]
    fn test_cli_parses_tx_rigctld_and_ptt_timing_flags() {
        let cli = Cli::parse_from([
            "coppa",
            "tx",
            "hello",
            "--rigctld",
            "192.168.1.50:4532",
            "--ptt-lead-ms",
            "75",
            "--ptt-tail-ms",
            "500",
        ]);
        match cli.command {
            Commands::Tx { rig, .. } => {
                assert_eq!(rig.rigctld.as_deref(), Some("192.168.1.50:4532"));
                assert_eq!(rig.ptt_lead_ms, Some(75));
                assert_eq!(rig.ptt_tail_ms, Some(500));
            }
            _ => panic!("expected Tx command"),
        }
    }

    #[test]
    fn test_cli_tx_ptt_flags_default() {
        let cli = Cli::parse_from(["coppa", "tx", "hello"]);
        match cli.command {
            Commands::Tx { rig, .. } => {
                assert_eq!(rig.ptt, None);
                assert_eq!(rig.rigctld, None);
                assert_eq!(rig.ptt_lead_ms, None);
                assert_eq!(rig.ptt_tail_ms, None);
            }
            _ => panic!("expected Tx command"),
        }
    }

    #[test]
    fn test_cli_parses_tune_rigctld_and_ptt_timing_flags() {
        let cli = Cli::parse_from([
            "coppa",
            "tune",
            "--rigctld",
            "10.0.0.5:4532",
            "--ptt-lead-ms",
            "10",
            "--ptt-tail-ms",
            "20",
        ]);
        match cli.command {
            Commands::Tune { rig, .. } => {
                assert_eq!(rig.rigctld.as_deref(), Some("10.0.0.5:4532"));
                assert_eq!(rig.ptt_lead_ms, Some(10));
                assert_eq!(rig.ptt_tail_ms, Some(20));
            }
            _ => panic!("expected Tune command"),
        }
    }

    #[test]
    fn test_cli_tune_ptt_flags_default() {
        let cli = Cli::parse_from(["coppa", "tune"]);
        match cli.command {
            Commands::Tune { rig, .. } => {
                assert_eq!(rig.ptt, None);
                assert_eq!(rig.rigctld, None);
                assert_eq!(rig.ptt_lead_ms, None);
                assert_eq!(rig.ptt_tail_ms, None);
            }
            _ => panic!("expected Tune command"),
        }
    }

    /// Simulated output device for `play_keyed`: a bounded 8,192-sample
    /// ring (like `CpalSink`'s) drained at the sample rate as the injected
    /// clock advances, with a fixed presentation latency, recording every
    /// PTT/sleep/stop event with its simulated time.
    #[cfg(feature = "cpal-backend")]
    mod sim {
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        pub struct Device {
            pub buffer: VecDeque<f32>,
            pub capacity: usize,
            pub captured: Vec<f32>,
            pub now: Instant,
            pub start: Instant,
            pub deadline: Instant,
            pub latency: Duration,
            /// Device callbacks are delayed (consume nothing) until then.
            pub paused_until: Instant,
            pub write_limits: Vec<Option<usize>>,
            pub writes: usize,
            pub fail_write_after: Option<usize>,
            pub fail_progress: bool,
            pub fail_stop: bool,
            pub fail_key: bool,
            pub fail_unkey: bool,
            pub events: Vec<(Duration, String)>,
            fractional: f64,
            /// Simulated Ctrl-C at this offset from `start`.
            pub interrupt_at: Option<Duration>,
        }

        impl Device {
            pub fn new(latency: Duration) -> Arc<Mutex<Device>> {
                let now = Instant::now();
                Arc::new(Mutex::new(Device {
                    buffer: VecDeque::new(),
                    capacity: 8192,
                    captured: Vec::new(),
                    now,
                    start: now,
                    deadline: now,
                    latency,
                    paused_until: now,
                    write_limits: vec![None],
                    writes: 0,
                    fail_write_after: None,
                    fail_progress: false,
                    fail_stop: false,
                    fail_key: false,
                    fail_unkey: false,
                    events: Vec::new(),
                    fractional: 0.0,
                    interrupt_at: None,
                }))
            }

            pub fn log(&mut self, what: impl Into<String>) {
                let at = self.now - self.start;
                self.events.push((at, what.into()));
            }

            fn advance(&mut self, by: Duration) {
                self.now += by;
                if self.now <= self.paused_until {
                    return;
                }
                self.fractional += by.as_secs_f64() * 48_000.0;
                let n = (self.fractional as usize).min(self.buffer.len());
                self.fractional -= self.fractional.floor();
                if n > 0 {
                    self.captured.extend(self.buffer.drain(..n));
                    self.deadline = self.now + self.latency;
                }
            }
        }

        pub struct Sink(pub Arc<Mutex<Device>>);

        impl coppa_audio::AudioSink for Sink {
            fn write(&mut self, samples: &[f32]) -> anyhow::Result<usize> {
                let mut d = self.0.lock().unwrap();
                if d.fail_write_after.is_some_and(|n| d.writes >= n) {
                    anyhow::bail!("simulated device write failure");
                }
                let limit = d.write_limits[d.writes % d.write_limits.len()];
                d.writes += 1;
                let room = d.capacity - d.buffer.len();
                let take = samples.len().min(room).min(limit.unwrap_or(usize::MAX));
                d.buffer.extend(samples[..take].iter().copied());
                Ok(take)
            }
            fn sample_rate(&self) -> u32 {
                48_000
            }
            fn start(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
            fn stop(&mut self) -> anyhow::Result<()> {
                let mut d = self.0.lock().unwrap();
                d.log("stop");
                if d.fail_stop {
                    anyhow::bail!("simulated stop failure");
                }
                Ok(())
            }
        }

        pub fn progress(sink: &Sink) -> anyhow::Result<coppa_audio::PlaybackProgress> {
            let d = sink.0.lock().unwrap();
            if d.fail_progress {
                anyhow::bail!("simulated progress failure");
            }
            Ok(coppa_audio::PlaybackProgress {
                consumed_samples: d.captured.len() as u64,
                remaining: d.deadline.saturating_duration_since(d.now),
            })
        }

        pub struct Clock(pub Arc<Mutex<Device>>);

        impl super::super::PlaybackClock for Clock {
            fn now(&self) -> Instant {
                self.0.lock().unwrap().now
            }
            fn sleep(&mut self, duration: Duration) {
                let mut d = self.0.lock().unwrap();
                if duration >= Duration::from_millis(20) {
                    d.log(format!("sleep {}ms", duration.as_millis()));
                }
                d.advance(duration);
            }
            fn interrupted(&self) -> bool {
                let d = self.0.lock().unwrap();
                d.interrupt_at
                    .is_some_and(|at| d.now.saturating_duration_since(d.start) >= at)
            }
            // One logged sleep, cut short at the simulated interrupt.
            fn sleep_or_interrupt(&mut self, duration: Duration) -> bool {
                let (now, at) = {
                    let d = self.0.lock().unwrap();
                    (d.now, d.interrupt_at.map(|at| d.start + at))
                };
                match at {
                    Some(at) if at < now + duration => {
                        self.sleep(at.saturating_duration_since(now));
                        true
                    }
                    _ => {
                        self.sleep(duration);
                        false
                    }
                }
            }
        }

        pub struct Ptt(pub Arc<Mutex<Device>>);

        impl coppa_radio::PttControl for Ptt {
            fn set_ptt(&mut self, state: coppa_radio::PttState) -> anyhow::Result<()> {
                let mut d = self.0.lock().unwrap();
                let what = match state {
                    coppa_radio::PttState::Tx => "ptt TX",
                    coppa_radio::PttState::Rx => "ptt RX",
                };
                d.log(what);
                if state == coppa_radio::PttState::Tx && d.fail_key {
                    anyhow::bail!("simulated key failure");
                }
                if state == coppa_radio::PttState::Rx && d.fail_unkey {
                    anyhow::bail!("simulated unkey failure");
                }
                Ok(())
            }
            fn get_ptt(&mut self) -> anyhow::Result<coppa_radio::PttState> {
                Ok(coppa_radio::PttState::Rx)
            }
        }
    }

    #[cfg(feature = "cpal-backend")]
    fn run_play_keyed(
        device: &std::sync::Arc<std::sync::Mutex<sim::Device>>,
        samples: &[f32],
        lead_ms: u64,
        tail_ms: u64,
    ) -> Result<u64> {
        let mut sink = sim::Sink(device.clone());
        let mut ptt = sim::Ptt(device.clone());
        let mut clock = sim::Clock(device.clone());
        play_keyed(
            &mut sink,
            sim::progress,
            |_| Ok(()),
            &mut ptt,
            samples,
            std::time::Duration::from_millis(lead_ms),
            std::time::Duration::from_millis(tail_ms),
            &mut clock,
        )
    }

    #[cfg(feature = "cpal-backend")]
    fn event_names(device: &std::sync::Arc<std::sync::Mutex<sim::Device>>) -> Vec<String> {
        let d = device.lock().unwrap();
        d.events.iter().map(|(_, e)| e.clone()).collect()
    }

    #[cfg(feature = "cpal-backend")]
    fn event_time(
        device: &std::sync::Arc<std::sync::Mutex<sim::Device>>,
        name: &str,
    ) -> std::time::Duration {
        let d = device.lock().unwrap();
        d.events
            .iter()
            .find(|(_, e)| e == name)
            .map(|(t, _)| *t)
            .unwrap()
    }

    /// COP-22 AC1 (CLI): a real 65,520-sample encoded frame fed into a
    /// short-writing sink with zero-write intervals arrives whole and in
    /// order, and the delivered count is the full frame.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_delivers_full_frame_through_short_writes() {
        let frame = CoppaCore::new().encode_bytes(b"COP-22").unwrap();
        assert_eq!(frame.len(), 65_520);
        let device = sim::Device::new(std::time::Duration::from_millis(30));
        device.lock().unwrap().write_limits = vec![Some(700), Some(0), Some(0), None, Some(1)];
        let delivered = run_play_keyed(&device, &frame, 50, 200).unwrap();
        assert_eq!(delivered, 65_520);
        assert_eq!(device.lock().unwrap().captured, frame);
    }

    /// Order is key, lead, paced writes, final presentation, tail, unkey,
    /// stop -- and with device callbacks delayed by a second, unkey still
    /// waits for real playback instead of nominal-duration arithmetic.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_unkeys_after_final_presentation_and_tail() {
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32).sin()).collect();
        let device = sim::Device::new(std::time::Duration::from_millis(40));
        {
            let mut d = device.lock().unwrap();
            // Callbacks stay silent for one second after the 1 s lead.
            d.paused_until = d.now + std::time::Duration::from_secs(2);
        }
        run_play_keyed(&device, &samples, 1_000, 3_000).unwrap();
        let names = event_names(&device);
        assert_eq!(names.first().map(String::as_str), Some("ptt TX"));
        assert_eq!(names[1], "sleep 1000ms", "lead follows key");
        assert_eq!(
            &names[names.len() - 3..],
            ["sleep 3000ms", "ptt RX", "stop"]
        );
        // 1 s lead + 1 s delayed callbacks + 1 s of audio + 40 ms latency +
        // 3 s tail. Nominal arithmetic (lead + duration + tail) would unkey
        // at 5 s, before the audio had finished playing.
        let unkey = event_time(&device, "ptt RX");
        assert!(
            unkey >= std::time::Duration::from_millis(6_040),
            "unkeyed at {unkey:?}"
        );
        assert!(
            unkey < std::time::Duration::from_millis(6_100),
            "unkeyed at {unkey:?}"
        );
        assert_eq!(device.lock().unwrap().captured, samples);
    }

    /// A non-block-aligned final transmission asks the resampling sink to
    /// finalize its partial block before waiting for playback and unkeying.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_finishes_partial_tail_before_ptt_release() {
        let device = sim::Device::new(std::time::Duration::from_millis(10));
        let samples = vec![0.25; 1025];
        let mut sink = sim::Sink(device.clone());
        let mut ptt = sim::Ptt(device.clone());
        let mut clock = sim::Clock(device.clone());
        let finish_device = device.clone();
        let delivered = play_keyed(
            &mut sink,
            sim::progress,
            move |_| {
                finish_device.lock().unwrap().log("finish input");
                Ok(())
            },
            &mut ptt,
            &samples,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            &mut clock,
        )
        .unwrap();
        assert_eq!(delivered, samples.len() as u64);
        assert_eq!(device.lock().unwrap().captured, samples);
        let names = event_names(&device);
        assert_eq!(
            names.iter().filter(|name| *name == "finish input").count(),
            1
        );
        assert!(
            names
                .iter()
                .position(|name| name == "finish input")
                .unwrap()
                < names.iter().position(|name| name == "ptt RX").unwrap()
        );
        assert!(event_time(&device, "ptt RX") >= std::time::Duration::from_millis(30));
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_unkeys_and_stops_when_finishing_tail_fails() {
        let device = sim::Device::new(std::time::Duration::ZERO);
        let mut sink = sim::Sink(device.clone());
        let mut ptt = sim::Ptt(device.clone());
        let mut clock = sim::Clock(device.clone());
        let error = play_keyed(
            &mut sink,
            sim::progress,
            |_| anyhow::bail!("tail conversion failed"),
            &mut ptt,
            &[0.5; 7],
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            &mut clock,
        )
        .unwrap_err();
        assert!(error.to_string().contains("tail conversion failed"));
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);
    }

    /// TX and tune share the live path: a tune longer than the sink ring,
    /// custom lead/tail, an empty input, and a final fractional-millisecond
    /// sample period all deliver exactly.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_handles_tune_empty_and_fractional_lengths() {
        let tune = CoppaCore::new().tune_tone(0.5, None);
        assert!(tune.len() > 8192);
        let device = sim::Device::new(std::time::Duration::from_millis(5));
        assert_eq!(
            run_play_keyed(&device, &tune, 20, 30).unwrap(),
            tune.len() as u64
        );
        assert_eq!(device.lock().unwrap().captured, tune);

        let device = sim::Device::new(std::time::Duration::ZERO);
        assert_eq!(run_play_keyed(&device, &[], 50, 200).unwrap(), 0);
        assert_eq!(
            event_names(&device),
            ["ptt TX", "sleep 50ms", "sleep 200ms", "ptt RX", "stop"]
        );

        // 7 samples = 145.8 us: the final partial sample period still counts.
        let device = sim::Device::new(std::time::Duration::ZERO);
        assert_eq!(run_play_keyed(&device, &[0.5; 7], 0, 0).unwrap(), 7);
        assert_eq!(device.lock().unwrap().captured, vec![0.5; 7]);
    }

    /// Write failure, progress failure and a five-second stall each unkey
    /// and stop before returning an error (so no success line is printed),
    /// and a failing stop does not skip the unkey.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_failures_still_clean_up() {
        let samples = vec![0.25f32; 30_000];

        let device = sim::Device::new(std::time::Duration::ZERO);
        device.lock().unwrap().fail_write_after = Some(3);
        assert!(run_play_keyed(&device, &samples, 0, 200).is_err());
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);

        let device = sim::Device::new(std::time::Duration::ZERO);
        device.lock().unwrap().fail_progress = true;
        assert!(run_play_keyed(&device, &samples, 0, 200).is_err());
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);

        let device = sim::Device::new(std::time::Duration::ZERO);
        {
            let mut d = device.lock().unwrap();
            d.paused_until = d.now + std::time::Duration::from_secs(3600);
        }
        let err = run_play_keyed(&device, &samples, 0, 200).unwrap_err();
        assert!(err.to_string().contains("no progress"), "{err}");
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);
        assert!(event_time(&device, "ptt RX") >= PLAYBACK_STALL_TIMEOUT);

        let device = sim::Device::new(std::time::Duration::ZERO);
        {
            let mut d = device.lock().unwrap();
            d.fail_write_after = Some(0);
            d.fail_stop = true;
        }
        assert!(run_play_keyed(&device, &samples, 0, 0).is_err());
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);
    }

    /// COP-86: Ctrl-C during the lead, the feed or the tail unkeys and
    /// stops at once, and returns the `Interrupted` type `main` maps to 130.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_interrupted_releases_ptt() {
        let samples = vec![0.25f32; 48_000];
        // (interrupt at, lead, tail): during the 1 s lead, during the 1 s of
        // audio after a 100 ms lead, and during the 3 s tail.
        for (at_ms, lead_ms, tail_ms) in [(500, 1_000, 200), (600, 100, 200), (2_000, 100, 3_000)] {
            let device = sim::Device::new(std::time::Duration::ZERO);
            device.lock().unwrap().interrupt_at = Some(std::time::Duration::from_millis(at_ms));
            let error = run_play_keyed(&device, &samples, lead_ms, tail_ms).unwrap_err();
            assert!(error.is::<interrupt::Interrupted>(), "{at_ms} ms: {error}");
            let names = event_names(&device);
            assert_eq!(
                names.first().map(String::as_str),
                Some("ptt TX"),
                "{names:?}"
            );
            assert_eq!(&names[names.len() - 2..], ["ptt RX", "stop"], "{names:?}");
            let unkey = event_time(&device, "ptt RX");
            assert!(
                unkey >= std::time::Duration::from_millis(at_ms)
                    && unkey < std::time::Duration::from_millis(at_ms + 10),
                "{at_ms} ms: unkeyed at {unkey:?}"
            );
        }
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_interrupt_with_failed_release_is_a_plain_error() {
        let device = sim::Device::new(std::time::Duration::ZERO);
        {
            let mut d = device.lock().unwrap();
            d.interrupt_at = Some(std::time::Duration::from_millis(10));
            d.fail_unkey = true;
        }
        let error = run_play_keyed(&device, &[0.25; 48_000], 1_000, 200).unwrap_err();
        assert!(!error.is::<interrupt::Interrupted>(), "{error}");
        assert!(
            error
                .to_string()
                .starts_with("interrupted; cleanup: failed to release PTT"),
            "{error}"
        );
        assert_eq!(
            event_names(&device).last().map(String::as_str),
            Some("stop")
        );
    }

    /// The real clock's sliced sleep notices an interrupt between slices.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn default_sleep_or_interrupt_stops_between_slices() {
        struct Counting {
            now: std::time::Instant,
            sleeps: usize,
            interrupt_after: usize,
        }
        impl PlaybackClock for Counting {
            fn now(&self) -> std::time::Instant {
                self.now
            }
            fn sleep(&mut self, duration: std::time::Duration) {
                assert!(duration <= PLAYBACK_POLL);
                self.now += duration;
                self.sleeps += 1;
            }
            fn interrupted(&self) -> bool {
                self.sleeps >= self.interrupt_after
            }
        }
        let start = std::time::Instant::now();
        let mut clock = Counting {
            now: start,
            sleeps: 0,
            interrupt_after: 3,
        };
        assert!(clock.sleep_or_interrupt(std::time::Duration::from_secs(5)));
        assert_eq!(clock.sleeps, 3);
        let mut clock = Counting {
            now: start,
            sleeps: 0,
            interrupt_after: usize::MAX,
        };
        assert!(!clock.sleep_or_interrupt(std::time::Duration::from_millis(9)));
        assert_eq!(clock.now - start, std::time::Duration::from_millis(9));
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_key_and_unkey_failures_are_fatal() {
        let device = sim::Device::new(std::time::Duration::ZERO);
        device.lock().unwrap().fail_key = true;
        let error = run_play_keyed(&device, &[0.25; 16], 50, 200).unwrap_err();
        assert!(error.to_string().contains("failed to key PTT"), "{error}");
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);
        assert_eq!(device.lock().unwrap().writes, 0);

        let device = sim::Device::new(std::time::Duration::ZERO);
        device.lock().unwrap().fail_unkey = true;
        let error = run_play_keyed(&device, &[0.25; 16], 0, 0).unwrap_err();
        assert!(
            error.to_string().contains("failed to release PTT"),
            "{error}"
        );
        assert_eq!(event_names(&device), ["ptt TX", "ptt RX", "stop"]);

        let device = sim::Device::new(std::time::Duration::ZERO);
        {
            let mut d = device.lock().unwrap();
            d.fail_write_after = Some(0);
            d.fail_unkey = true;
            d.fail_stop = true;
        }
        let error = run_play_keyed(&device, &[0.25; 16], 0, 0).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("simulated device write failure")
                && text.contains("failed to release PTT")
                && text.contains("failed to stop audio"),
            "{text}"
        );
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn refused_rigctld_connection_is_an_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        let error = build_ptt("rigctld", &addr).err().unwrap();
        let message = error.to_string();
        assert!(
            message.contains("rigctld connection failed") && message.contains(&addr),
            "{message}"
        );
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn tx_and_tune_stop_at_rigctld_before_opening_audio() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        let rig = RigSettings::test_values(None, "rigctld", &addr, 0, 0, None, None);
        let tx = cmd_tx("HI", None, None, None, &rig, Verbosity::Quiet).unwrap_err();
        let tune = cmd_tune(0.1, None, None, None, &rig, Verbosity::Quiet).unwrap_err();
        for error in [tx, tune] {
            let message = error.to_string();
            assert!(
                message.contains("rigctld connection failed") && message.contains(&addr),
                "{message}"
            );
            assert!(
                !message.contains("ALSA") && !message.contains("audio"),
                "{message}"
            );
        }
    }

    /// "CLI flags reach the engine": `--rigctld` must actually be the address
    /// `build_ptt` connects to, not a hardcoded one. A real loopback
    /// `TcpListener` bound to an OS-assigned port stands in for rigctld --
    /// if the configured address is threaded through correctly, the
    /// listener observes a connection.
    #[cfg(feature = "cpal-backend")]
    #[test]
    fn test_build_ptt_rigctld_uses_configured_address() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = listener.accept();
            let _ = tx.send(result.is_ok());
        });

        let _ptt = build_ptt("rigctld", &addr);

        let connected = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("listener should have observed a connection attempt");
        assert!(
            connected,
            "rigctld PTT should have connected to the configured --rigctld address"
        );
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn test_build_ptt_none_and_vox_do_not_touch_network() {
        // Sanity check that non-rigctld methods don't try to connect anywhere.
        let none = build_ptt("none", "127.0.0.1:1").unwrap();
        let vox = build_ptt("vox", "127.0.0.1:1").unwrap();
        assert!(!none.is_rigctld());
        assert!(!vox.is_rigctld());
    }

    // ── COP-40: wideband HF/VHF gate for live transmission ───────────────

    /// The CLI applies the same rules as the daemon: narrowband always
    /// transmits; wideband needs evidence at or above 29.7 MHz, and a live
    /// rig answer outranks the stated flags.
    #[test]
    fn cli_gate_decision_matches_daemon_rules() {
        let vhf = coppa_radio::band::FrequencyEvidence::Stated(144_500_000);
        let ten_m = cli_frequency_evidence(None, Some("10m"), "--frequency-hz", "--band").unwrap();
        let two_m = cli_frequency_evidence(None, Some("2m"), "--frequency-hz", "--band").unwrap();

        let err = cli_gate_decision("VHF_FAST", 5600, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("refusing to transmit"), "{msg}");
        assert!(msg.contains("97.307(f)(3)"), "{msg}");
        assert!(msg.contains("--frequency-hz"), "{msg}");

        assert!(cli_gate_decision("VHF_FAST", 5600, None, Some(vhf)).is_ok());
        assert!(cli_gate_decision("VHF_FAST", 5600, None, two_m).is_ok());
        assert!(cli_gate_decision("HF_STANDARD", 2400, None, None).is_ok());
        assert!(cli_gate_decision("VHF_FAST", 5600, None, ten_m).is_err());
        assert!(cli_gate_decision("VHF_FAST", 5600, Some(144_500_000), None).is_ok());
        let err = cli_gate_decision("VHF_FAST", 5600, Some(14_074_000), Some(vhf)).unwrap_err();
        assert!(
            err.to_string().contains("rigctld reports 14074000 Hz"),
            "{err}"
        );
    }

    /// The gate reads the profile's real occupied bandwidth: every HF
    /// profile transmits with no flags, `VHF_FAST` does not.
    #[test]
    fn cli_gate_uses_the_profile_occupied_bandwidth() {
        let hf = CoppaCore::with_config(resolve_config(Some("HF_STANDARD")).unwrap());
        assert!(cli_gate_decision("HF_STANDARD", hf.occupied_bandwidth_hz(), None, None).is_ok());
        let vhf = CoppaCore::with_config(resolve_config(Some("VHF_FAST")).unwrap());
        assert!(cli_gate_decision("VHF_FAST", vhf.occupied_bandwidth_hz(), None, None).is_err());
    }

    /// `coppa tune` emits a one- or two-tone signal well under 2.8 kHz on
    /// any profile, so it is never refused for lack of a frequency source;
    /// the flags are still validated.
    #[test]
    fn cli_tune_is_gated_on_the_tone_not_the_profile() {
        assert_eq!(tune_occupied_hz(None), 1200);
        assert_eq!(tune_occupied_hz(Some(1500.0)), 0);
        assert!(cli_gate_decision("VHF_FAST", tune_occupied_hz(None), None, None).is_ok());
        assert!(cmd_tune(
            0.1,
            None,
            None,
            Some("VHF_FAST"),
            &RigSettings::test_values(None, "none", "127.0.0.1:1", 0, 0, None, Some("21m")),
            Verbosity::Quiet
        )
        .unwrap_err()
        .to_string()
        .contains("expected an amateur band label"));
    }

    #[test]
    fn cli_band_flag_typo_is_a_hard_error() {
        let err =
            cli_frequency_evidence(None, Some("21m"), "--frequency-hz", "--band").unwrap_err();
        assert!(
            err.to_string().contains("expected an amateur band label"),
            "{err}"
        );
        assert!(err.to_string().contains("--band"), "{err}");
    }

    #[test]
    fn cli_frequency_band_contradiction_is_a_hard_error() {
        let err = cli_frequency_evidence(Some(14_074_000), Some("2m"), "--frequency-hz", "--band")
            .unwrap_err();
        assert!(err.to_string().contains("lies outside band"), "{err}");
        assert!(
            cli_frequency_evidence(Some(144_500_000), Some("2m"), "--frequency-hz", "--band")
                .is_ok()
        );
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn cli_unknown_ptt_value_is_a_hard_error() {
        let err = build_ptt("rigctld-typo", "127.0.0.1:1").err().unwrap();
        assert!(
            err.to_string()
                .contains("expected \"none\", \"vox\", or \"rigctld\""),
            "{err}"
        );
    }

    #[test]
    fn cli_features_lists_every_feature_in_cargo_toml() {
        let manifest: toml::Table = include_str!("../Cargo.toml").parse().unwrap();
        let mut declared: Vec<&str> = manifest["features"]
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|name| *name != "default")
            .collect();
        declared.sort_unstable();
        let listed: Vec<&str> = CLI_FEATURES.iter().map(|(name, _)| *name).collect();
        assert_eq!(listed, declared);
        assert!(cli_features() == "none" || !cli_features().is_empty());
    }

    #[test]
    fn long_version_lines() {
        let lines: Vec<&str> = LONG_VERSION.lines().collect();
        assert_eq!(lines.len(), 4, "{}", *LONG_VERSION);
        assert_eq!(lines[0], env!("CARGO_PKG_VERSION"));
        assert!(lines[1].starts_with("commit: "), "{}", lines[1]);
        assert_eq!(lines[2], format!("features: {}", cli_features()));
        assert_eq!(lines[3], "wire format: 1");
    }

    #[test]
    fn completion_command_drops_hidden_subcommands() {
        let cmd = completion_command();
        let names: Vec<&str> = cmd.get_subcommands().map(|sub| sub.get_name()).collect();
        for visible in ["tx", "rx", "tune", "completions", "man"] {
            assert!(names.contains(&visible), "{names:?}");
        }
        assert!(
            !names.contains(&"listen") && !names.contains(&"config"),
            "{names:?}"
        );
    }

    #[test]
    fn clip_warning_fires_once_per_entry() {
        // 600 ms at full scale (six meter windows), then clean audio.
        let mut samples = vec![1.0f32; 48_000 * 6 / 10];
        samples.extend(std::iter::repeat_n(0.1f32, 48_000));
        let mut source = coppa_audio::RawF32Source::from_samples(samples.clone(), 48_000);
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            StopWhen {
                total_samples: Some(samples.len()),
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        // One entry into clipping, one warning, however long it lasts.
        assert_eq!(summary.warnings.len(), 1, "{:?}", summary.warnings);
        assert!(summary.warnings[0].starts_with("RX audio input is clipping"));
    }

    #[test]
    fn summary_line_wording() {
        let mut summary = DecodeSummary {
            consumed_samples: 48_000 * 42 + 14_400,
            good_frames: 1,
            ..DecodeSummary::default()
        };
        assert_eq!(
            summary.stop_line(48_000),
            "Stopped after 42.3 s of audio: 1 frame decoded, 0 failed."
        );
        summary.good_frames = 2;
        summary.failed_frames = 1;
        assert!(summary
            .stop_line(48_000)
            .ends_with(": 2 frames decoded, 1 failed."));
        summary.good_frames = 0;
        assert!(summary.stop_line(48_000).contains(": 0 frames decoded"));
    }

    #[test]
    fn interrupted_live_capture_with_no_frames_is_ok() {
        let summary = DecodeSummary {
            interrupted: true,
            ..DecodeSummary::default()
        };
        assert!(summary.finish(|| "no frames".to_string()).is_ok());
        let finished = DecodeSummary::default();
        let err = finished.finish(|| "no frames".to_string()).unwrap_err();
        assert_eq!(err.to_string(), "no frames");
    }

    #[test]
    fn interrupted_capture_with_a_failure_exits_one() {
        let summary = DecodeSummary {
            interrupted: true,
            good_frames: 1,
            failed_frames: 1,
            first_error: Some("payload decode failed: bad".to_string()),
            ..DecodeSummary::default()
        };
        let err = summary.finish(|| "no frames".to_string()).unwrap_err();
        assert_eq!(err.to_string(), "payload decode failed: bad");
    }

    #[test]
    fn interrupt_flag_stops_stream_decode() {
        struct Silence {
            reads: usize,
            flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }
        impl coppa_audio::AudioSource for Silence {
            fn read(&mut self, buf: &mut [f32]) -> anyhow::Result<usize> {
                self.reads += 1;
                if self.reads == 5 {
                    self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                buf.fill(0.0);
                Ok(buf.len())
            }
            fn sample_rate(&self) -> u32 {
                48_000
            }
            fn start(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
            fn stop(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let (interrupt, flag) = interrupt::Interrupt::manual();
        let mut source = Silence { reads: 0, flag };
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            StopWhen {
                interrupt: Some(&interrupt),
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        assert!(summary.interrupted);
        assert_eq!(source.reads, 5);
        assert_eq!(summary.consumed_samples, 5 * 4096);
    }

    #[test]
    fn interrupt_still_runs_the_trailing_flush() {
        // A frame that ends right where the interrupt lands still decodes,
        // because the trailing flush runs on every exit path.
        struct FrameThenStop {
            samples: Vec<f32>,
            position: usize,
            flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }
        impl coppa_audio::AudioSource for FrameThenStop {
            fn read(&mut self, buf: &mut [f32]) -> anyhow::Result<usize> {
                let n = buf.len().min(self.samples.len() - self.position);
                buf[..n].copy_from_slice(&self.samples[self.position..self.position + n]);
                self.position += n;
                if self.position == self.samples.len() {
                    self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(n)
            }
            fn sample_rate(&self) -> u32 {
                48_000
            }
            fn start(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
            fn stop(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let (interrupt, flag) = interrupt::Interrupt::manual();
        let mut source = FrameThenStop {
            samples: CoppaCore::new().encode("FLUSHED").unwrap(),
            position: 0,
            flag,
        };
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            StopWhen {
                interrupt: Some(&interrupt),
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        assert!(summary.interrupted);
        assert_eq!(summary.good_frames, 1);
    }

    #[test]
    fn raw_audio_refuses_a_terminal() {
        assert!(refuse_raw_stdout(false).is_ok());
        let err = refuse_raw_stdout(true).unwrap_err().to_string();
        assert!(
            err.starts_with("refusing to write raw audio to a terminal"),
            "{err}"
        );
        assert!(refuse_raw_stdin(false).is_ok());
        let err = refuse_raw_stdin(true).unwrap_err().to_string();
        assert!(
            err.starts_with("refusing to read raw audio from a terminal"),
            "{err}"
        );
        assert!(err.contains("| coppa rx -"), "{err}");
    }

    #[test]
    fn only_a_stdout_broken_pipe_becomes_stdout_closed() {
        let pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone");
        assert!(stdout_error(pipe).is::<StdoutClosed>());
        let other = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        assert!(!stdout_error(other).is::<StdoutClosed>());
        // A broken pipe that never went through a stdout write (a socket)
        // is not StdoutClosed, so `main` still reports it.
        let socket = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "rigctld",
        ));
        assert!(!socket.is::<StdoutClosed>());
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn play_keyed_never_keys_after_an_early_interrupt() {
        let device = sim::Device::new(std::time::Duration::ZERO);
        device.lock().unwrap().interrupt_at = Some(std::time::Duration::ZERO);
        let error = run_play_keyed(&device, &[0.25; 4800], 50, 200).unwrap_err();
        assert!(error.is::<interrupt::Interrupted>(), "{error}");
        assert!(!event_names(&device).iter().any(|e| e == "ptt TX"));
    }

    #[test]
    fn is_broken_pipe_finds_the_io_error_in_the_chain() {
        let pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone");
        assert!(is_broken_pipe(&anyhow::Error::new(pipe)));
        let wrapped =
            anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone"))
                .context("writing raw audio");
        assert!(is_broken_pipe(&wrapped));
        let other = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        assert!(!is_broken_pipe(&anyhow::Error::new(other)));
        assert!(!is_broken_pipe(&anyhow::anyhow!("Broken pipe")));
    }

    #[test]
    fn status_enabled_follows_quiet_raw_and_json() {
        assert!(status_enabled(Verbosity::Normal, false, false));
        assert!(status_enabled(Verbosity::Verbose(1), false, false));
        assert!(!status_enabled(Verbosity::Quiet, false, false));
        assert!(!status_enabled(Verbosity::Normal, true, false));
        assert!(!status_enabled(Verbosity::Normal, false, true));
    }

    #[test]
    fn tune_seconds_parser_bounds() {
        assert_eq!(tune_seconds("120"), Ok(120.0));
        assert_eq!(tune_seconds("0.01"), Ok(0.01));
        assert_eq!(tune_seconds(" 5 "), Ok(5.0));
        for bad in [
            "0", "-0", "120.01", "-1", "NaN", "nan", "inf", "-inf", "1e12", "ten", "",
        ] {
            assert_eq!(
                tune_seconds(bad),
                Err("must be more than 0 and at most 120 seconds".to_string()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn tune_single_parser_bounds() {
        assert_eq!(tune_single_hz("300"), Ok(300.0));
        assert_eq!(tune_single_hz("3000"), Ok(3000.0));
        assert_eq!(tune_single_hz("1000.5"), Ok(1000.5));
        for bad in ["299.99", "3000.01", "0", "-5", "NaN", "inf", "30000", "hz"] {
            assert_eq!(
                tune_single_hz(bad),
                Err("must be between 300 and 3000 Hz".to_string()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn whole_number_parser() {
        assert_eq!(whole_number("0"), Ok(0));
        assert_eq!(whole_number("250"), Ok(250));
        assert_eq!(rx_seconds("0"), Ok(0));
        assert_eq!(rx_seconds("60"), Ok(60));
        for bad in ["-5", "-0.5", "1.5", "NaN", "inf", "ms", ""] {
            assert_eq!(
                whole_number(bad),
                Err("must be a whole number, 0 or more".to_string()),
                "{bad:?}"
            );
            assert_eq!(
                rx_seconds(bad),
                Err("must be a whole number of seconds, 0 or more (0 = no limit)".to_string()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn tune_duration_and_deprecated_seconds_parse() {
        let cli = Cli::parse_from(["coppa", "tune", "-d", "2.5"]);
        let Commands::Tune {
            duration, seconds, ..
        } = cli.command
        else {
            panic!("expected Tune");
        };
        assert_eq!((duration, seconds), (2.5, None));

        let cli = Cli::parse_from(["coppa", "tune", "--seconds", "3"]);
        let Commands::Tune {
            duration, seconds, ..
        } = cli.command
        else {
            panic!("expected Tune");
        };
        assert_eq!((duration, seconds), (10.0, Some(3.0)));
        assert!(Cli::try_parse_from(["coppa", "tune", "-d", "2", "--seconds", "3"]).is_err());
    }

    /// The acceptance command `coppa tx --ptt rigctld-typo` must fail in
    /// every build, not only where audio exists (COP-40 plan, Phase 5
    /// manual check 3). In the audio build `transmit_live` -> `build_ptt`
    /// rejects it before any device is opened; in the default build the
    /// no-audio arm validates it directly.
    #[test]
    fn cli_unknown_ptt_value_is_a_hard_error_without_audio_too() {
        assert!(validate_ptt_method("none").is_ok());
        assert!(validate_ptt_method("vox").is_ok());
        assert!(validate_ptt_method("rigctld").is_ok());
        let err = validate_ptt_method("rigctld-typo").unwrap_err();
        assert!(
            err.to_string()
                .contains("expected \"none\", \"vox\", or \"rigctld\""),
            "{err}"
        );
        let err = cmd_tx(
            "hi",
            None,
            None,
            None,
            &RigSettings::test_values(None, "rigctld-typo", "127.0.0.1:1", 0, 0, None, None),
            Verbosity::Quiet,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("expected \"none\", \"vox\", or \"rigctld\""),
            "{err}"
        );
    }

    #[test]
    fn test_cli_parses_frequency_flags_on_tx_and_tune() {
        let cli = Cli::parse_from(["coppa", "tx", "hi", "--frequency-hz", "144500000"]);
        match cli.command {
            Commands::Tx { rig, .. } => {
                assert_eq!(rig.frequency_hz, Some(144_500_000));
                assert_eq!(rig.band, None);
            }
            _ => panic!("expected Tx command"),
        }
        let cli = Cli::parse_from(["coppa", "tune", "--band", "2m"]);
        match cli.command {
            Commands::Tune { rig, .. } => assert_eq!(rig.band.as_deref(), Some("2m")),
            _ => panic!("expected Tune command"),
        }
    }

    /// A live wideband `tx` with no frequency source is refused before any
    /// audio device is opened; the same request rendered to a WAV file
    /// (no RF) is never gated.
    #[test]
    fn cmd_tx_refuses_live_wideband_but_renders_wav() {
        let err = cmd_tx(
            "hi",
            None,
            Some("VHF_FAST"),
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:1", 0, 0, None, None),
            Verbosity::Quiet,
        )
        .unwrap_err();
        assert!(err.to_string().contains("refusing to transmit"), "{err}");

        let path = std::env::temp_dir().join(format!("cop40_{}.wav", std::process::id()));
        cmd_tx(
            "hi",
            Some(OutputTarget {
                path: path.to_str().unwrap(),
                format: SampleFormat::S16le,
            }),
            Some("VHF_FAST"),
            None,
            &RigSettings::test_values(None, "none", "127.0.0.1:1", 0, 0, None, None),
            Verbosity::Quiet,
        )
        .unwrap();
        std::fs::remove_file(&path).ok();
    }

    /// `cmd_rx`'s `--input` path streams through `CoppaCore::push_samples`
    /// in chunks (not a batch `core.decode()` call). This only proves that a
    /// freshly encoded WAV runs through `cmd_rx` without an error; what it
    /// prints is asserted by the integration tests, which run the real
    /// binary: `tests/rx_golden.rs` (`--raw` hex per golden vector),
    /// `tests/rx_text.rs` (text vs. hex) and `tests/rx_one_receiver.rs`
    /// (FILE, `--duration`, `--callsign` and the `listen` alias).
    #[test]
    fn test_rx_streams_encoded_wav_file() {
        #[cfg(feature = "file-backend")]
        {
            let dir = std::env::temp_dir();
            let path = dir.join("coppa_test_rx_streaming.wav");
            let path_str = path.to_str().unwrap();
            cmd_tx(
                "streaming rx roundtrip",
                Some(OutputTarget {
                    path: path_str,
                    format: SampleFormat::F32le,
                }),
                None,
                None,
                &RigSettings::test_values(None, "none", "127.0.0.1:4532", 50, 200, None, None),
                Verbosity::Quiet,
            )
            .expect("tx to file should succeed");

            let args = RxArgs {
                input: Some(path_str.into()),
                raw: true,
                ..Default::default()
            };
            let result = cmd_rx(&args, Verbosity::Quiet);
            std::fs::remove_file(&path).ok();
            result.expect("streaming rx of a freshly-encoded WAV should succeed");
        }
    }

    // COP-56: one receiver. `rx` takes a bare path, `--duration` and
    // `--callsign`; `listen` is a hidden alias parsing the same `RxArgs`.

    #[test]
    fn rx_parses_bare_path_duration_and_callsign() {
        let cli = Cli::parse_from([
            "coppa",
            "rx",
            "x.wav",
            "-d",
            "5",
            "--callsign",
            "VK2ABC",
            "--raw",
            "--hex",
            "--profile",
            "HF_ROBUST",
            "--device",
            "Mic",
        ]);
        let Commands::Rx(args) = cli.command else {
            panic!("expected Rx")
        };
        assert_eq!(args.path(), Some("x.wav"));
        assert_eq!(args.duration, 5);
        assert_eq!(args.callsign.as_deref(), Some("VK2ABC"));
        assert!(args.raw && args.hex);
        assert_eq!(args.profile.as_deref(), Some("HF_ROBUST"));
        assert_eq!(args.device.as_deref(), Some("Mic"));

        let cli = Cli::parse_from(["coppa", "rx", "-i", "y.wav"]);
        let Commands::Rx(args) = cli.command else {
            panic!("expected Rx")
        };
        assert_eq!(args.path(), Some("y.wav"));
        assert_eq!(args.duration, 0);
    }

    #[test]
    fn exact_device_and_channel_flags_parse_and_conflict() {
        let Commands::Rx(rx) = Cli::parse_from([
            "coppa",
            "rx",
            "--device-id",
            "alsa:input",
            "--input-channel",
            "1",
        ])
        .command
        else {
            panic!("expected Rx")
        };
        assert_eq!(rx.device_id.as_deref(), Some("alsa:input"));
        assert_eq!(rx.input_channel, 1);
        assert_eq!(
            Cli::try_parse_from([
                "coppa",
                "rx",
                "--device",
                "USB",
                "--device-id",
                "alsa:input"
            ])
            .err()
            .expect("conflict")
            .kind(),
            clap::error::ErrorKind::ArgumentConflict,
        );
        let Commands::Listen(listen) = Cli::parse_from([
            "coppa",
            "listen",
            "--device-id",
            "alsa:input",
            "--input-channel",
            "1",
        ])
        .command
        else {
            panic!("expected Listen")
        };
        assert_eq!(rx, listen);

        let Commands::Tx { rig, .. } = Cli::parse_from([
            "coppa",
            "tx",
            "hi",
            "--device-id",
            "alsa:output",
            "--output-channel",
            "1",
        ])
        .command
        else {
            panic!("expected Tx")
        };
        assert_eq!(rig.device_id.as_deref(), Some("alsa:output"));
        assert_eq!(rig.output_channel, 1);
        let Commands::Tune { rig, .. } = Cli::parse_from([
            "coppa",
            "tune",
            "--device-id",
            "alsa:output",
            "--output-channel",
            "1",
        ])
        .command
        else {
            panic!("expected Tune")
        };
        assert_eq!(rig.device_id.as_deref(), Some("alsa:output"));
        assert_eq!(rig.output_channel, 1);
        for args in [
            &[
                "coppa",
                "tx",
                "hi",
                "--device",
                "USB",
                "--device-id",
                "alsa:output",
            ][..],
            &[
                "coppa",
                "tune",
                "--device",
                "USB",
                "--device-id",
                "alsa:output",
            ][..],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).err().expect("conflict").kind(),
                clap::error::ErrorKind::ArgumentConflict
            );
        }
    }

    /// COP-64: `tx`/`tune` with `--device-id` select exactly that ID, never
    /// a layered name alongside it.
    #[test]
    fn output_selection_prefers_an_explicit_device_id() {
        let mut rig =
            RigSettings::test_values(Some("USB"), "none", "127.0.0.1:1", 0, 0, None, None);
        let selection = AudioOutputSelection::from_rig(&rig);
        assert_eq!(
            (selection.name, selection.id, selection.channel),
            (Some("USB"), None, 0)
        );
        rig.device_id = Some("alsa:radio".into());
        rig.output_channel = 1;
        let selection = AudioOutputSelection::from_rig(&rig);
        assert_eq!(
            (selection.name, selection.id, selection.channel),
            (None, Some("alsa:radio"), 1)
        );
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn explicit_missing_device_id_fails_without_default_fallback() {
        let id = "coppa:no-such-audio-device";
        let Err(input) = resolve_cli_input_device(None, Some(id)) else {
            panic!("input ID must fail")
        };
        assert!(
            input.to_string().contains("input audio device ID"),
            "{input}"
        );
        assert!(input.to_string().contains(id), "{input}");
        let Err(output) = resolve_cli_output_device(AudioOutputSelection {
            id: Some(id),
            ..Default::default()
        }) else {
            panic!("output ID must fail")
        };
        assert!(
            output.to_string().contains("output audio device ID"),
            "{output}"
        );
        assert!(output.to_string().contains(id), "{output}");
    }

    #[cfg(feature = "kiss-tnc")]
    #[test]
    fn tnc_digipeat_flag_parses() {
        for (args, expected) in [
            (vec!["coppa", "tnc"], None),
            (
                vec!["coppa", "tnc", "--digipeat", "WIDE1"],
                Some(("WIDE1".to_string(), 0)),
            ),
            (
                vec!["coppa", "tnc", "--digipeat", "WIDE1-1"],
                Some(("WIDE1".to_string(), 1)),
            ),
        ] {
            let Commands::Tnc { digipeat, .. } = Cli::try_parse_from(args).unwrap().command else {
                panic!("expected TNC")
            };
            assert_eq!(
                digipeat
                    .as_deref()
                    .map(parse_digipeat_alias)
                    .transpose()
                    .unwrap(),
                expected
            );
        }
        for invalid in [
            "toolongcallsign",
            "",
            "A-16",
            "A--1",
            "A-",
            "A-1-2",
            "A_B",
            "é",
        ] {
            let error = parse_digipeat_alias(invalid).unwrap_err().to_string();
            assert!(error.contains("--digipeat expects CALL[-SSID]"));
        }
        assert_eq!(
            parse_digipeat_alias("n0call-15").unwrap(),
            ("N0CALL".into(), 15)
        );
    }

    #[cfg(feature = "kiss-tnc")]
    #[test]
    fn tnc_directional_device_and_channel_flags_parse() {
        let Commands::Tnc {
            input_device_id,
            output_device_id,
            input_channel,
            output_channel,
            ..
        } = Cli::parse_from([
            "coppa",
            "tnc",
            "--input-device-id",
            "alsa:in",
            "--output-device-id",
            "alsa:out",
            "--input-channel",
            "0",
            "--output-channel",
            "1",
        ])
        .command
        else {
            panic!("expected Tnc")
        };
        assert_eq!(input_device_id.as_deref(), Some("alsa:in"));
        assert_eq!(output_device_id.as_deref(), Some("alsa:out"));
        assert_eq!((input_channel, output_channel), (0, 1));
        assert_eq!(
            Cli::try_parse_from([
                "coppa",
                "tnc",
                "--device",
                "USB",
                "--input-device-id",
                "alsa:in"
            ])
            .err()
            .expect("conflict")
            .kind(),
            clap::error::ErrorKind::ArgumentConflict,
        );
    }

    #[test]
    fn device_inventory_shows_ids_and_clean_empty_state() {
        let unknown = || Err(anyhow::anyhow!("listing failed"));
        assert_eq!(
            device_lines(&[], &unknown(), &Default::default()),
            [
                "Available audio devices:",
                "  (none found - CPAL backend may not be available)",
            ]
        );
        let devices = [
            coppa_audio::AudioDevice {
                name: "USB Audio".into(),
                id: Some("alsa:radio-a".into()),
                max_sample_rate: 48000,
                input_channels: 2,
                output_channels: 2,
            },
            coppa_audio::AudioDevice {
                name: "USB Audio".into(),
                id: None,
                max_sample_rate: 44100,
                input_channels: 2,
                output_channels: 0,
            },
        ];
        assert_eq!(
            device_lines(&devices, &unknown(), &Default::default()),
            [
                "Available audio devices:",
                "  USB Audio [id: alsa:radio-a] (in: 2ch, out: 2ch, 48 kHz: unknown)",
                "  USB Audio [id: unavailable] (in: 2ch, out: 0ch, 48 kHz: unknown)",
                "Default input: none",
                "Default output: none",
            ]
        );
    }

    #[test]
    fn device_lines_show_48k_support_and_defaults() {
        use coppa_audio::{AudioDevice, DeviceSupport, OpenedFormat, Support};
        let device = |id: &str| AudioDevice {
            name: format!("Card {id}"),
            id: Some(id.into()),
            max_sample_rate: 192_000,
            input_channels: 2,
            output_channels: 2,
        };
        let yes = || {
            Some(Support::Yes(OpenedFormat {
                channels: 1,
                sample_rate: 48_000,
            }))
        };
        let no = || Some(Support::No("no 48 kHz".into()));
        let entry = |id: &str, input, output| DeviceSupport {
            name: format!("Card {id}"),
            id: id.into(),
            input,
            output,
        };
        let devices = [
            device("both"),
            device("in"),
            device("out"),
            device("none"),
            device("missing"),
        ];
        let support = Ok(vec![
            entry("both", yes(), yes()),
            entry("in", yes(), no()),
            entry("out", None, yes()),
            entry("none", no(), no()),
        ]);
        let defaults = coppa_audio::DefaultDevices {
            input: Some(coppa_audio::DefaultDevice {
                name: "Default Audio Device".into(),
                id: Some("alsa:default".into()),
            }),
            output: None,
        };
        let lines = device_lines(&devices, &support, &defaults);
        assert_eq!(
            lines,
            [
                "Available audio devices:",
                "  Card both [id: both] (in: 2ch, out: 2ch, 48 kHz: in+out)",
                "  Card in [id: in] (in: 2ch, out: 2ch, 48 kHz: in)",
                "  Card out [id: out] (in: 2ch, out: 2ch, 48 kHz: out)",
                "  Card none [id: none] (in: 2ch, out: 2ch, 48 kHz: no)",
                "  Card missing [id: missing] (in: 2ch, out: 2ch, 48 kHz: unknown)",
                "Default input: Default Audio Device [id: alsa:default]",
                "Default output: none",
            ]
        );
        assert!(lines.iter().all(|line| !line.contains("max:")));
        let failed = device_lines(
            &devices[..1],
            &Err(anyhow::anyhow!("listing failed")),
            &defaults,
        );
        assert!(failed[1].ends_with("48 kHz: unknown)"), "{failed:?}");
    }

    #[test]
    fn device_help_explains_exact_ids_and_zero_based_channels() {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        for (command, id, channel) in [
            ("rx", "--device-id", "--input-channel"),
            ("tx", "--device-id", "--output-channel"),
            ("tune", "--device-id", "--output-channel"),
        ] {
            let help = cli
                .find_subcommand_mut(command)
                .unwrap()
                .render_long_help()
                .to_string();
            assert!(
                help.contains(id) && help.contains(channel) && help.contains("Zero-based"),
                "{help}"
            );
        }
    }

    #[test]
    fn rx_rejects_a_path_and_input_together() {
        let err = Cli::try_parse_from(["coppa", "rx", "a.wav", "-i", "b.wav"])
            .err()
            .expect("FILE and -i together must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn global_verbosity_flags_count_and_conflict() {
        for (flags, count) in [
            (vec!["-v"], 1),
            (vec!["-vv"], 2),
            (vec!["-vvv"], 3),
            (vec!["--verbose", "--verbose"], 2),
            (vec!["-v", "--verbose"], 2),
        ] {
            for before_command in [true, false] {
                let mut argv = vec!["coppa"];
                if before_command {
                    argv.extend(&flags);
                }
                argv.push("devices");
                if !before_command {
                    argv.extend(&flags);
                }
                assert_eq!(Cli::try_parse_from(argv).unwrap().verbose, count);
            }
        }
        for flag in ["-q", "--quiet"] {
            assert!(
                Cli::try_parse_from(["coppa", flag, "devices"])
                    .unwrap()
                    .quiet
            );
            assert!(
                Cli::try_parse_from(["coppa", "devices", flag])
                    .unwrap()
                    .quiet
            );
        }
        for args in [
            vec!["coppa", "-q", "-v", "devices"],
            vec!["coppa", "--quiet", "--verbose", "devices"],
            vec!["coppa", "--verbose", "--quiet", "devices"],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).err().unwrap().kind(),
                clap::error::ErrorKind::ArgumentConflict
            );
        }
    }

    #[test]
    fn file_sample_limit_honours_duration() {
        assert_eq!(file_sample_limit(179_040, 0, 48_000), 179_040);
        assert_eq!(file_sample_limit(179_040, 2, 48_000), 96_000);
        assert_eq!(file_sample_limit(179_040, 60, 48_000), 179_040);
        assert_eq!(file_sample_limit(100, u64::MAX, 48_000), 100);
    }

    #[test]
    fn live_deadline_is_none_for_zero_and_overflow() {
        let now = std::time::Instant::now();
        assert_eq!(live_deadline(now, 0), None);
        assert_eq!(live_deadline(now, u64::MAX), None);
        assert_eq!(
            live_deadline(now, 5),
            Some(now + std::time::Duration::from_secs(5))
        );
    }

    /// A live source with no audio yet: every read returns 0 samples.
    struct Stalled;

    impl coppa_audio::AudioSource for Stalled {
        fn read(&mut self, _buf: &mut [f32]) -> Result<usize> {
            Ok(0)
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// A busy live source: every read fills the whole buffer with silence.
    struct Endless;

    impl coppa_audio::AudioSource for Endless {
        fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
            buf.fill(0.0);
            Ok(buf.len())
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// Like `Endless`, but counts the samples it hands out.
    #[derive(Default)]
    struct Counting {
        handed_out: usize,
    }

    impl coppa_audio::AudioSource for Counting {
        fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
            buf.fill(0.0);
            self.handed_out += buf.len();
            Ok(buf.len())
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    struct FiniteSamples {
        samples: Vec<f32>,
        cursor: usize,
    }

    impl coppa_audio::AudioSource for FiniteSamples {
        fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
            let count = buf.len().min(self.samples.len() - self.cursor);
            buf[..count].copy_from_slice(&self.samples[self.cursor..self.cursor + count]);
            self.cursor += count;
            Ok(count)
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// A live-like source whose read fails after the frames have arrived,
    /// so stream_decode exits without ever returning a summary.
    struct InterruptedSamples(FiniteSamples);

    impl coppa_audio::AudioSource for InterruptedSamples {
        fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
            if self.0.cursor == self.0.samples.len() {
                anyhow::bail!("synthetic capture stopped");
            }
            self.0.read(buf)
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unbounded_live_receive_reports_errors_before_return() {
        const CHILD: &str = "COPPA_TEST_LIVE_DECODE_DIAGNOSTICS";
        if std::env::var_os(CHILD).is_some() {
            let compressed_profile = coppa_engine::profiles::get_profile("HF_STANDARD").unwrap();
            let compressed = CoppaCore::with_config(EngineConfig::from_profile(compressed_profile))
                .encode("BAD")
                .unwrap();
            let raw = CoppaCore::new()
                .encode_bytes(&[0xfe, 1, 2, 3, 4, 5])
                .unwrap();
            for (mut core, mut samples, profile) in [
                (CoppaCore::new(), compressed, "default"),
                (
                    CoppaCore::with_config(EngineConfig::from_profile(compressed_profile)),
                    raw,
                    "HF_STANDARD",
                ),
            ] {
                samples.extend(std::iter::repeat_n(0.0, TRAILING_FLUSH_SAMPLES));
                let mut source = InterruptedSamples(FiniteSamples { samples, cursor: 0 });
                let error = stream_decode(
                    &mut core,
                    &mut source,
                    StopWhen::default(),
                    RxOutputMode::Text { hex: false },
                    profile,
                    Verbosity::Quiet,
                )
                .err()
                .expect("synthetic live source should stop the decoder");
                assert!(error.to_string().contains("synthetic capture stopped"));
            }
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::unbounded_live_receive_reports_errors_before_return",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("appears compressed"), "{stderr}");
        assert!(stderr.contains("payload decode failed"), "{stderr}");
    }

    #[test]
    fn stream_decode_counts_good_and_failed_payload_frames() {
        let plain = CoppaCore::new().encode("GOOD").unwrap();
        let profile = coppa_engine::profiles::get_profile("HF_STANDARD").unwrap();
        let compressed = CoppaCore::with_config(EngineConfig::from_profile(profile))
            .encode("BAD")
            .unwrap();
        let mut samples = plain;
        samples.extend(std::iter::repeat_n(0.0, 48_000));
        samples.extend(compressed);
        let length = samples.len();
        let mut source = FiniteSamples { samples, cursor: 0 };
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            StopWhen {
                total_samples: Some(length),
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        assert_eq!(summary.consumed_samples, length);
        assert_eq!(summary.good_frames, 1);
        assert_eq!(summary.failed_frames, 1);
        assert!(summary
            .check_failure()
            .unwrap_err()
            .to_string()
            .contains("appears compressed"));
    }

    #[test]
    fn stream_decode_stops_a_stalled_live_source_at_its_deadline() {
        let start = std::time::Instant::now();
        let deadline = Some(start + std::time::Duration::from_millis(100));
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut Stalled,
            StopWhen {
                deadline,
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        assert_eq!(summary.good_frames, 0);
        summary.check_failure().unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(100),
            "{elapsed:?}"
        );
        assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn stream_decode_stops_a_streaming_live_source_at_its_deadline() {
        let start = std::time::Instant::now();
        let deadline = Some(start + std::time::Duration::from_millis(100));
        stream_decode(
            &mut CoppaCore::new(),
            &mut Endless,
            StopWhen {
                deadline,
                ..StopWhen::default()
            },
            RxOutputMode::Text { hex: false },
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(100),
            "{elapsed:?}"
        );
        assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn stream_decode_reads_no_further_than_its_sample_limit() {
        let mut source = Counting::default();
        let summary = stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            StopWhen {
                total_samples: Some(10_000),
                ..StopWhen::default()
            },
            RxOutputMode::Raw,
            "default",
            Verbosity::Quiet,
        )
        .unwrap();
        assert_eq!(source.handed_out, 10_000);
        assert_eq!(summary.consumed_samples, 10_000);
    }

    #[test]
    fn stream_frame_outcomes_keep_first_payload_error() {
        let core = CoppaCore::new();
        let frame = |payload| coppa_engine::StreamFrame {
            payload,
            snr_db: 20.0,
            cfo_hz: 0.0,
            frame_start: 0,
            speed_level: 1,
            recommended_level: 1,
            delay_spread_ms: 0.0,
        };
        let mut summary = DecodeSummary::default();
        for payload in [
            Ok(b"GOOD".to_vec()),
            Err(coppa_engine::EngineError::NoSignalDetected),
            Err(coppa_engine::EngineError::NoSamplesToDecode),
        ] {
            handle_stream_frame(
                &core,
                &frame(payload),
                &mut summary,
                RxOutputMode::Text { hex: false },
                "default",
                Verbosity::Quiet,
            )
            .unwrap();
        }
        assert_eq!(summary.good_frames, 1);
        assert_eq!(summary.failed_frames, 2);
        let error = summary.check_failure().unwrap_err().to_string();
        assert!(
            error.contains("No signal detected") && !error.contains("No samples to decode"),
            "{error}"
        );
    }

    #[test]
    fn listen_parses_into_the_same_args_as_rx() {
        let flags = [
            "x.wav",
            "-d",
            "5",
            "--callsign",
            "VK2ABC",
            "--raw",
            "--hex",
            "--profile",
            "HF_ROBUST",
            "--device",
            "Mic",
        ];
        let parse = |sub: &str| Cli::parse_from(["coppa", sub].into_iter().chain(flags)).command;
        let Commands::Rx(rx) = parse("rx") else {
            panic!("expected Rx")
        };
        let Commands::Listen(listen) = parse("listen") else {
            panic!("expected Listen")
        };
        assert_eq!(rx, listen);
    }

    #[test]
    fn top_level_help_hides_listen() {
        use clap::CommandFactory;
        let help = Cli::command().render_help().to_string();
        assert!(
            !help.lines().any(|l| l.trim_start().starts_with("listen")),
            "{help}"
        );
        assert!(
            help.lines().any(|l| l.trim_start().starts_with("rx ")),
            "{help}"
        );
    }

    #[cfg(feature = "kiss-tnc")]
    #[test]
    fn tnc_fx25_flag_defaults_and_help() {
        use clap::CommandFactory;
        for enabled in [false, true] {
            let mut args = vec![
                "coppa", "tnc", "--vox", "--port", "8123", "--device", "radio",
            ];
            if enabled {
                args.push("--fx25");
            }
            let cli = Cli::try_parse_from(args).unwrap();
            let Commands::Tnc {
                fx25,
                port,
                vox,
                device,
                ..
            } = cli.command
            else {
                panic!("tnc")
            };
            assert_eq!(fx25, enabled);
            assert_eq!(port, 8123);
            assert!(vox);
            assert_eq!(device.as_deref(), Some("radio"));
        }
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("tnc")
            .unwrap()
            .render_help()
            .to_string();
        assert!(help
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("--fx25 Transmit FX.25 when the frame fits; otherwise transmit AX.25"));
    }
}
