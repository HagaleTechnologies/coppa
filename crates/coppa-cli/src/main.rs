//! Coppa CLI - command-line interface for the Coppa digital communications system.

mod doctor;

use anyhow::Result;
use clap::{Parser, Subcommand};
use coppa_engine::config::EngineConfig;
use coppa_engine::CoppaCore;

/// Output verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

#[derive(Parser)]
#[command(
    name = "coppa",
    about = "Coppa - Ham Radio Digital Communications System",
    version
)]
struct Cli {
    /// Enable verbose output (SNR, sample counts, DSP diagnostics).
    #[arg(long, global = true)]
    verbose: bool,
    /// Suppress all output except decoded messages.
    #[arg(long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Flags for `coppa rx`, shared with its deprecated hidden alias
/// `coppa listen` (COP-56) so the two cannot drift apart.
#[derive(clap::Args, Debug, Default, PartialEq, Eq)]
struct RxArgs {
    /// WAV file to decode. Without one, captures live audio from an input device.
    #[arg(value_name = "FILE")]
    file: Option<String>,
    /// WAV file to decode (same as FILE).
    #[arg(short, long, value_name = "FILE", conflicts_with = "file")]
    input: Option<String>,
    /// Stop after this many seconds of audio: the first N seconds of a file,
    /// or N seconds of live capture (0 = no limit).
    #[arg(short, long, value_name = "SECONDS", default_value_t = 0)]
    duration: u64,
    /// Operational profile name.
    #[arg(long)]
    profile: Option<String>,
    /// Your callsign. Printed under --verbose; nothing else uses it yet.
    #[arg(long)]
    callsign: Option<String>,
    /// Print only the decoded payload as lowercase hex, one line per frame,
    /// with no labels (always hex, even for text).
    #[arg(long)]
    raw: bool,
    /// Show the payload as lowercase hex even when it is valid UTF-8 text.
    #[arg(long)]
    hex: bool,
    /// Audio input device name (substring match, live capture only).
    #[arg(long)]
    device: Option<String>,
}

impl RxArgs {
    /// The WAV file to decode, from FILE or -i/--input (clap rejects both).
    fn path(&self) -> Option<&str> {
        self.file.as_deref().or(self.input.as_deref())
    }
}

/// Printed on stderr every time the hidden `listen` alias runs (COP-56).
/// Unconditional, like this file's other `WARNING:` lines, so `--quiet`
/// scripts still see it; stderr keeps stdout captures clean.
const LISTEN_DEPRECATED: &str = "WARNING: 'coppa listen' is deprecated and will be \
    removed in the next release. Use 'coppa rx' instead; it takes the same flags, \
    and --raw prints hex.";

#[derive(Subcommand)]
enum Commands {
    /// Encode and transmit a message.
    Tx {
        /// Message to transmit.
        message: String,
        /// Output file for audio samples (WAV format).
        #[arg(short, long)]
        output: Option<String>,
        /// Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY).
        #[arg(long)]
        profile: Option<String>,
        /// Callsign for station identification.
        #[arg(long)]
        callsign: Option<String>,
        /// Audio output device name (substring match).
        #[arg(long)]
        device: Option<String>,
        /// PTT method: "rigctld", "vox", "none" (default: "none")
        #[arg(long, default_value = "none")]
        ptt: String,
        /// rigctld address for CAT PTT (only used when --ptt rigctld).
        #[arg(long, default_value = "127.0.0.1:4532")]
        rigctld: String,
        /// Milliseconds to key PTT before audio starts playing.
        #[arg(long, default_value = "50")]
        ptt_lead_ms: u64,
        /// Milliseconds to hold PTT keyed after audio finishes playing.
        #[arg(long, default_value = "200")]
        ptt_tail_ms: u64,
        /// Explicit operating frequency in Hz for the wideband gate (a
        /// wideband profile only transmits live at or above 29.7 MHz).
        #[arg(long)]
        frequency_hz: Option<u64>,
        /// Operating band label ("160m".."23cm") for the wideband gate.
        #[arg(long)]
        band: Option<String>,
    },
    /// Receive and decode audio from a WAV file, or live from an audio input device.
    Rx(RxArgs),
    /// Run a loopback test (encode -> decode).
    Loopback {
        /// Message to test.
        message: String,
        /// Operational profile name.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Deprecated: use 'coppa rx'. Removed in the next release.
    // COP-56: kept for one release. Delete this variant, LISTEN_DEPRECATED
    // and the Listen dispatch arm in the release after it ships.
    #[command(hide = true)]
    Listen(RxArgs),
    /// Transmit a TX-level calibration ("TUNE") tone: standard SSB two-tone
    /// (700 Hz + 1900 Hz) by default, or a single tone via `--single`. Key
    /// this while advancing your radio's audio drive level until ALC just
    /// registers, then back off. See `docs/OPERATING.md`.
    Tune {
        /// Duration to key the tone, in seconds.
        #[arg(long, default_value = "10")]
        seconds: f32,
        /// Transmit a single tone at this frequency (Hz) instead of the
        /// default two-tone signal, e.g. for power measurement with a
        /// wattmeter.
        #[arg(long)]
        single: Option<f32>,
        /// Output file for audio samples (WAV format) instead of live playback.
        #[arg(short, long)]
        output: Option<String>,
        /// Operational profile name (e.g., HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY).
        #[arg(long)]
        profile: Option<String>,
        /// Audio output device name (substring match).
        #[arg(long)]
        device: Option<String>,
        /// PTT method: "rigctld", "vox", "none" (default: "none")
        #[arg(long, default_value = "none")]
        ptt: String,
        /// rigctld address for CAT PTT (only used when --ptt rigctld).
        #[arg(long, default_value = "127.0.0.1:4532")]
        rigctld: String,
        /// Milliseconds to key PTT before audio starts playing.
        #[arg(long, default_value = "50")]
        ptt_lead_ms: u64,
        /// Milliseconds to hold PTT keyed after audio finishes playing.
        #[arg(long, default_value = "200")]
        ptt_tail_ms: u64,
        /// Explicit operating frequency in Hz for the wideband gate (a
        /// wideband profile only transmits live at or above 29.7 MHz).
        #[arg(long)]
        frequency_hz: Option<u64>,
        /// Operating band label ("160m".."23cm") for the wideband gate.
        #[arg(long)]
        band: Option<String>,
    },
    /// List available audio devices.
    Devices,
    /// Check audio devices and rig control without transmitting.
    Doctor(doctor::DoctorArgs),
    /// Show current configuration.
    Config {
        /// Profile name to show.
        #[arg(short, long)]
        profile: Option<String>,
    },
    /// Start the AFSK 1200 TNC (KISS TCP server).
    #[cfg(feature = "kiss-tnc")]
    Tnc {
        /// KISS TCP port.
        #[arg(long, default_value = "8001")]
        port: u16,
        /// Audio device name (substring match).
        #[arg(long)]
        device: Option<String>,
        /// rigctld address for PTT (e.g., localhost:4532).
        #[arg(long)]
        rig: Option<String>,
        /// Use VOX instead of CAT PTT.
        #[arg(long)]
        vox: bool,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging();

    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else if cli.verbose {
        Verbosity::Verbose
    } else {
        Verbosity::Normal
    };

    match cli.command {
        Some(Commands::Tx {
            message,
            output,
            profile,
            callsign,
            device,
            ptt,
            rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
            frequency_hz,
            band,
        }) => cmd_tx(
            &message,
            output.as_deref(),
            profile.as_deref(),
            callsign.as_deref(),
            device.as_deref(),
            &ptt,
            &rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
            frequency_hz,
            band.as_deref(),
            verbosity,
        )?,
        Some(Commands::Rx(args)) => cmd_rx(&args, verbosity)?,
        Some(Commands::Loopback { message, profile }) => {
            cmd_loopback(&message, profile.as_deref(), verbosity)?
        }
        Some(Commands::Listen(args)) => {
            eprintln!("{LISTEN_DEPRECATED}");
            cmd_rx(&args, verbosity)?
        }
        Some(Commands::Tune {
            seconds,
            single,
            output,
            profile,
            device,
            ptt,
            rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
            frequency_hz,
            band,
        }) => cmd_tune(
            seconds,
            single,
            output.as_deref(),
            profile.as_deref(),
            device.as_deref(),
            &ptt,
            &rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
            frequency_hz,
            band.as_deref(),
            verbosity,
        )?,
        Some(Commands::Devices) => cmd_devices()?,
        Some(Commands::Doctor(args)) => doctor::cmd_doctor(&args)?,
        Some(Commands::Config { profile }) => cmd_config(profile.as_deref())?,
        #[cfg(feature = "kiss-tnc")]
        Some(Commands::Tnc {
            port,
            device,
            rig,
            vox,
        }) => cmd_tnc(port, device, rig, vox)?,
        None => {
            println!("Coppa - Ham Radio Digital Communications System");
            println!("Use --help for available commands");
        }
    }

    Ok(())
}

/// Write `samples` to a WAV file at `path` (file-backend only) — shared by
/// `cmd_tx`'s and `cmd_tune`'s file-output paths.
fn write_wav_output(
    path: &str,
    samples: &[f32],
    sample_rate: u32,
    verbosity: Verbosity,
) -> Result<()> {
    #[cfg(feature = "file-backend")]
    {
        use coppa_audio::{file_backend::WavSink, AudioSink};
        let mut sink = WavSink::new(path, sample_rate);
        sink.start()?;
        sink.write(samples)?;
        sink.stop()?;
        if verbosity != Verbosity::Quiet {
            println!("Written to {}", path);
        }
    }
    #[cfg(not(feature = "file-backend"))]
    {
        let _ = path;
        let _ = samples;
        let _ = sample_rate;
        if verbosity != Verbosity::Quiet {
            println!("File output not available (compile with file-backend feature)");
        }
    }
    Ok(())
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
        "rigctld" => match coppa_radio::RigctldClient::connect(rigctld_addr) {
            Ok(client) => Ok(PttBackend::Rigctld(client)),
            Err(e) => {
                eprintln!("WARNING: rigctld connect failed ({}), using no PTT", e);
                Ok(PttBackend::Other(Box::new(coppa_radio::NullPtt::new())))
            }
        },
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
) -> Result<Option<coppa_radio::band::FrequencyEvidence>> {
    let stated =
        coppa_radio::band::FrequencyConfig::parse(frequency_hz, band, "--frequency-hz", "--band")
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
}

/// The wall clock and a real thread sleep.
#[cfg(feature = "cpal-backend")]
struct SystemClock;

#[cfg(feature = "cpal-backend")]
impl PlaybackClock for SystemClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
    fn sleep(&mut self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }
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
fn play_keyed<S: coppa_audio::AudioSink>(
    sink: &mut S,
    progress: impl Fn(&S) -> Result<coppa_audio::PlaybackProgress>,
    ptt: &mut dyn coppa_radio::PttControl,
    samples: &[f32],
    lead: std::time::Duration,
    tail: std::time::Duration,
    clock: &mut dyn PlaybackClock,
) -> Result<u64> {
    use coppa_radio::PttState;

    if let Err(e) = ptt.set_ptt(PttState::Tx) {
        eprintln!("WARNING: PTT key failed: {}", e);
    }
    clock.sleep(lead);

    let result = feed_until_played(sink, &progress, samples, clock).inspect(|_| clock.sleep(tail));

    if let Err(e) = ptt.set_ptt(PttState::Rx) {
        eprintln!("WARNING: PTT unkey failed: {}", e);
    }
    let stopped = sink.stop();
    let delivered = result?;
    stopped?;
    Ok(delivered)
}

/// The feeding/draining half of [`play_keyed`].
#[cfg(feature = "cpal-backend")]
fn feed_until_played<S: coppa_audio::AudioSink>(
    sink: &mut S,
    progress: &impl Fn(&S) -> Result<coppa_audio::PlaybackProgress>,
    samples: &[f32],
    clock: &mut dyn PlaybackClock,
) -> Result<u64> {
    let mut offset = 0usize;
    let mut accepted = 0u64;
    let mut last_consumed = progress(sink)?.consumed_samples;
    let mut last_progress_at = clock.now();
    loop {
        if offset < samples.len() {
            let written = sink.write(&samples[offset..])?;
            let written = written.min(samples.len() - offset);
            offset += written;
            accepted += written as u64;
        }
        let snapshot = progress(sink)?;
        if offset == samples.len() && snapshot.has_played(accepted) {
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
    device: Option<&str>,
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

    // COP-40: refuse a wideband emission below 29.7 MHz before any audio
    // device is opened or PTT keyed.
    let live_hz = if gate.occupied_hz > coppa_radio::band::HF_OCCUPIED_BANDWIDTH_CAP_HZ {
        ptt_ctrl.poll_frequency()
    } else {
        None
    };
    cli_gate_decision(gate.profile, gate.occupied_hz, live_hz, gate.stated)?;

    // Create audio sink
    let mut sink = match device {
        Some(name) => match coppa_audio::find_output_device_by_name(name) {
            Some(dev) => {
                if verbosity != Verbosity::Quiet {
                    eprintln!("Using output device matching '{}'", name);
                }
                coppa_audio::cpal_backend::CpalSink::from_device(dev, sample_rate, 8192)?
            }
            None => {
                eprintln!(
                    "WARNING: No output device matching '{}', using default",
                    name
                );
                coppa_audio::cpal_backend::CpalSink::new(sample_rate, 8192)?
            }
        },
        None => coppa_audio::cpal_backend::CpalSink::new(sample_rate, 8192)?,
    };
    sink.start()?;

    // Key, pace every sample through the sink's bounded ring, and unkey only
    // after the last one is predicted to have played plus the tail. Any
    // failure unkeys and stops before returning, and skips the success line.
    let delivered = play_keyed(
        &mut sink,
        |sink| sink.playback_progress(),
        &mut ptt_ctrl,
        samples,
        std::time::Duration::from_millis(ptt_lead_ms),
        std::time::Duration::from_millis(ptt_tail_ms),
        &mut SystemClock,
    )?;

    if verbosity != Verbosity::Quiet {
        let duration_s = delivered as f64 / sample_rate as f64;
        eprintln!("Transmitted {} samples ({:.2}s)", delivered, duration_s);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_tx(
    message: &str,
    output: Option<&str>,
    profile: Option<&str>,
    callsign: Option<&str>,
    device: Option<&str>,
    ptt: &str,
    rigctld: &str,
    ptt_lead_ms: u64,
    ptt_tail_ms: u64,
    frequency_hz: Option<u64>,
    band: Option<&str>,
    verbosity: Verbosity,
) -> Result<()> {
    let config = resolve_config(profile)?;
    let core = CoppaCore::with_config(config);

    if verbosity == Verbosity::Verbose {
        if let Some(cs) = callsign {
            eprintln!("[verbose] Callsign: {}", cs);
        }
        eprintln!(
            "[verbose] Config: sample_rate={}, speed_level={}",
            core.config().sample_rate,
            core.config().speed_level,
        );
    }

    if verbosity != Verbosity::Quiet {
        println!("Encoding: \"{}\"", message);
    }
    let samples = core.encode(message)?;
    if verbosity != Verbosity::Quiet {
        println!("Generated {} audio samples", samples.len());
    }
    if verbosity == Verbosity::Verbose {
        let duration_s = samples.len() as f64 / core.config().sample_rate as f64;
        eprintln!("[verbose] Audio duration: {:.3} s", duration_s);
    }

    match output {
        Some(path) => write_wav_output(path, &samples, core.config().sample_rate, verbosity)?,
        None => {
            // Offline WAV rendering (the `Some` arm) emits no RF and is never
            // gated; live transmission is (COP-40).
            let gate = LiveTxGate {
                profile: profile.unwrap_or("default"),
                occupied_hz: core.occupied_bandwidth_hz(),
                stated: cli_frequency_evidence(frequency_hz, band)?,
            };
            #[cfg(feature = "cpal-backend")]
            {
                transmit_live(
                    &samples,
                    core.config().sample_rate,
                    device,
                    ptt,
                    rigctld,
                    ptt_lead_ms,
                    ptt_tail_ms,
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
                validate_ptt_method(ptt)?;
                cli_gate_decision(gate.profile, gate.occupied_hz, None, gate.stated)?;
                let _ = device;
                let _ = rigctld;
                let _ = ptt_lead_ms;
                let _ = ptt_tail_ms;
                if verbosity != Verbosity::Quiet {
                    println!("Live audio output not available (compile with cpal-backend feature)");
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
#[allow(clippy::too_many_arguments)]
fn cmd_tune(
    seconds: f32,
    single: Option<f32>,
    output: Option<&str>,
    profile: Option<&str>,
    device: Option<&str>,
    ptt: &str,
    rigctld: &str,
    ptt_lead_ms: u64,
    ptt_tail_ms: u64,
    frequency_hz: Option<u64>,
    band: Option<&str>,
    verbosity: Verbosity,
) -> Result<()> {
    let config = resolve_config(profile)?;
    let core = CoppaCore::with_config(config);

    if verbosity != Verbosity::Quiet {
        match single {
            Some(freq) => println!(
                "Generating single-tone calibration signal: {} Hz, {:.1}s",
                freq, seconds
            ),
            None => println!(
                "Generating two-tone calibration signal: {} Hz + {} Hz, {:.1}s",
                coppa_engine::TUNE_TONE_LOW_HZ,
                coppa_engine::TUNE_TONE_HIGH_HZ,
                seconds
            ),
        }
    }

    let samples = core.tune_tone(seconds, single);
    if verbosity != Verbosity::Quiet {
        println!("Generated {} audio samples", samples.len());
    }

    match output {
        Some(path) => write_wav_output(path, &samples, core.config().sample_rate, verbosity)?,
        None => {
            // Offline WAV rendering (the `Some` arm) emits no RF and is never
            // gated; live transmission is (COP-40).
            let gate = LiveTxGate {
                profile: profile.unwrap_or("default"),
                occupied_hz: tune_occupied_hz(single),
                stated: cli_frequency_evidence(frequency_hz, band)?,
            };
            #[cfg(feature = "cpal-backend")]
            {
                transmit_live(
                    &samples,
                    core.config().sample_rate,
                    device,
                    ptt,
                    rigctld,
                    ptt_lead_ms,
                    ptt_tail_ms,
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
                validate_ptt_method(ptt)?;
                cli_gate_decision(gate.profile, gate.occupied_hz, None, gate.stated)?;
                let _ = device;
                let _ = rigctld;
                let _ = ptt_lead_ms;
                let _ = ptt_tail_ms;
                if verbosity != Verbosity::Quiet {
                    println!("Live audio output not available (compile with cpal-backend feature)");
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

    if verbosity == Verbosity::Verbose {
        if let Some(cs) = &args.callsign {
            eprintln!("[verbose] Callsign: {}", cs);
        }
        eprintln!("[verbose] sample_rate: {}", core.config().sample_rate);
    }

    if let Some(path) = args.path() {
        #[cfg(feature = "file-backend")]
        {
            use coppa_audio::{file_backend::WavSource, AudioSource};
            let mut source = WavSource::open(path)?;
            let total = source.total_samples();
            // The file's own rate: WavSource does not resample, so "the
            // first N seconds" is N times the rate the samples were written at.
            let limit = file_sample_limit(total, args.duration, source.sample_rate());
            if verbosity != Verbosity::Quiet && !args.raw {
                if limit < total {
                    println!(
                        "Reading the first {} s ({} of {} samples) from {}",
                        args.duration, limit, total, path
                    );
                } else {
                    println!("Reading {} samples from {}", total, path);
                }
            }
            source.start()?;
            stream_decode(
                &mut core,
                &mut source,
                Some(limit),
                None,
                args.raw,
                args.hex,
                verbosity,
            )?;
            source.stop()?;
        }
        #[cfg(not(feature = "file-backend"))]
        {
            let _ = path;
            let _ = &mut core;
            if verbosity != Verbosity::Quiet {
                println!("File input not available (compile with file-backend feature)");
            }
        }
    } else {
        #[cfg(feature = "cpal-backend")]
        {
            use coppa_audio::AudioSource;
            let engine_rate = core.config().sample_rate;
            let mut source = match args.device.as_deref() {
                Some(name) => match coppa_audio::find_input_device_by_name(name) {
                    Some(dev) => {
                        if verbosity != Verbosity::Quiet {
                            eprintln!("Using input device matching '{}'", name);
                        }
                        coppa_audio::cpal_backend::CpalSource::from_device(dev, engine_rate, 8192)?
                    }
                    None => {
                        eprintln!(
                            "WARNING: No input device matching '{}', using default",
                            name
                        );
                        coppa_audio::cpal_backend::CpalSource::new(engine_rate, 8192)?
                    }
                },
                None => coppa_audio::cpal_backend::CpalSource::new(engine_rate, 8192)?,
            };
            source.start()?;
            let chatty = verbosity != Verbosity::Quiet && !args.raw;
            if chatty {
                if args.duration > 0 {
                    println!(
                        "Listening for live audio for {} s (Ctrl+C to stop)...",
                        args.duration
                    );
                } else {
                    println!("Listening for live audio (Ctrl+C to stop)...");
                }
            }
            let deadline = live_deadline(std::time::Instant::now(), args.duration);
            stream_decode(
                &mut core,
                &mut source,
                None,
                deadline,
                args.raw,
                args.hex,
                verbosity,
            )?;
            source.stop()?;
            if chatty && args.duration > 0 {
                println!("Stopped listening after {} s.", args.duration);
            }
        }
        #[cfg(not(feature = "cpal-backend"))]
        {
            let _ = (&args.device, args.duration);
            let _ = &mut core;
            if verbosity != Verbosity::Quiet {
                println!("Live audio input not available (compile with cpal-backend feature)");
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
/// frame as it completes (see `print_stream_frame`).
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
fn stream_decode(
    core: &mut CoppaCore,
    source: &mut dyn coppa_audio::AudioSource,
    total_samples: Option<usize>,
    deadline: Option<std::time::Instant>,
    raw: bool,
    hex: bool,
    verbosity: Verbosity,
) -> Result<()> {
    let mut buf = vec![0.0f32; 4096];
    let mut consumed = 0usize;
    while total_samples.is_none_or(|total| consumed < total) {
        let want = total_samples.map_or(buf.len(), |total| buf.len().min(total - consumed));
        let n = source.read(&mut buf[..want])?;
        if n > 0 {
            consumed += n;
            for frame in core.push_samples(&buf[..n]) {
                print_stream_frame(&frame, raw, hex, verbosity);
            }
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
        if n == 0 && total_samples.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let silence = vec![0.0f32; TRAILING_FLUSH_SAMPLES];
    for frame in core.push_samples(&silence) {
        print_stream_frame(&frame, raw, hex, verbosity);
    }

    Ok(())
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
/// Decode failures are reported on stderr (never stdout, so they never
/// corrupt a `--raw` capture), suppressed entirely in `Verbosity::Quiet`.
fn print_stream_frame(
    frame: &coppa_engine::StreamFrame,
    raw: bool,
    hex: bool,
    verbosity: Verbosity,
) {
    match &frame.payload {
        Ok(bytes) => {
            if raw {
                println!("{}", hex_string(bytes));
            } else {
                println!("{}", format_decoded(bytes, frame.snr_db, hex));
            }
            if verbosity == Verbosity::Verbose {
                eprintln!(
                    "[verbose] level={} cfo_hz={:.1} frame_start={}",
                    frame.speed_level, frame.cfo_hz, frame.frame_start
                );
            }
        }
        Err(e) => {
            if verbosity != Verbosity::Quiet {
                eprintln!("Decode failed: {}", e);
            }
        }
    }
}

fn cmd_loopback(message: &str, profile: Option<&str>, verbosity: Verbosity) -> Result<()> {
    if verbosity != Verbosity::Quiet {
        println!("Coppa Loopback Test");
        println!("===================");
        println!("Input: \"{}\"", message);
    }

    let config = resolve_config(profile)?;
    let core = CoppaCore::with_config(config);

    if verbosity == Verbosity::Verbose {
        eprintln!(
            "[verbose] Config: sample_rate={}, speed_level={}",
            core.config().sample_rate,
            core.config().speed_level,
        );
    }

    let samples = core.encode(message)?;
    if verbosity != Verbosity::Quiet {
        println!("Encoded: {} samples", samples.len());
    }

    let decoded = core.decode(&samples)?;
    if verbosity != Verbosity::Quiet {
        println!("Decoded: \"{}\"", decoded);
    }

    if message == decoded {
        if verbosity != Verbosity::Quiet {
            println!("PASS: Loopback test successful!");
        }
    } else {
        println!("FAIL: Messages don't match!");
        println!("  Expected: \"{}\"", message);
        println!("  Got:      \"{}\"", decoded);
        std::process::exit(1);
    }

    Ok(())
}

fn cmd_devices() -> Result<()> {
    println!("Available audio devices:");
    let devices = coppa_audio::list_devices()?;
    if devices.is_empty() {
        println!("  (none found - CPAL backend may not be available)");
    } else {
        for dev in &devices {
            println!(
                "  {} (in: {}ch, out: {}ch, max: {} Hz)",
                dev.name, dev.input_channels, dev.output_channels, dev.max_sample_rate
            );
        }
    }
    Ok(())
}

fn cmd_config(profile: Option<&str>) -> Result<()> {
    if let Some(name) = profile {
        if let Some(p) = coppa_engine::profiles::get_profile(name) {
            println!("Profile: {}", p.name);
            println!("  Description: {}", p.description);
            println!("  Speed level: {}", p.speed_level);
            println!("  Max payload: {} bytes", p.max_payload);
            println!("  ARQ window: {}", p.arq_window);
            println!("  Compression: {}", p.compression);
            println!("  Sample rate: {} Hz", p.sample_rate);
        } else {
            // F6: Fix config debug format -- use join(", ") instead of {:?}
            println!("Unknown profile: {}", name);
            let names = coppa_engine::profiles::list_profiles();
            println!("Available: {}", names.join(", "));
        }
    } else {
        println!("Available profiles:");
        for name in coppa_engine::profiles::list_profiles() {
            let p = coppa_engine::profiles::get_profile(name).unwrap();
            println!("  {} - {}", p.name, p.description);
        }
    }
    Ok(())
}

#[cfg(feature = "kiss-tnc")]
fn cmd_tnc(port: u16, device: Option<String>, rig: Option<String>, vox: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let config = coppa_daemon::tnc::TncConfig {
            kiss_port: port,
            audio_device: device,
            rig_address: rig,
            vox_mode: vox,
            sample_rate: 48000,
            ..Default::default()
        };
        coppa_daemon::tnc::run_tnc(config).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        cmd_loopback("Hello", None, Verbosity::Normal).expect("loopback should succeed");
    }

    #[test]
    fn test_loopback_short_message() {
        cmd_loopback("Hi", None, Verbosity::Normal).expect("short loopback should succeed");
    }

    #[test]
    fn test_loopback_with_profile() {
        cmd_loopback("Test", Some("HF_STANDARD"), Verbosity::Normal)
            .expect("loopback with profile should succeed");
    }

    #[test]
    fn test_loopback_quiet() {
        cmd_loopback("Quiet", None, Verbosity::Quiet).expect("quiet loopback should succeed");
    }

    #[test]
    fn test_loopback_verbose() {
        cmd_loopback("Verbose", None, Verbosity::Verbose).expect("verbose loopback should succeed");
    }

    #[test]
    fn test_tx_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tx.wav");
        let path_str = path.to_str().unwrap();
        cmd_tx(
            "test message",
            Some(path_str),
            None,
            None,
            None,
            "none",
            "127.0.0.1:4532",
            50,
            200,
            None,
            None,
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
            Some(path_str),
            None,
            None,
            "none",
            "127.0.0.1:4532",
            50,
            200,
            None,
            None,
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
            Some(path_str),
            None,
            None,
            "none",
            "127.0.0.1:4532",
            50,
            200,
            None,
            None,
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
    fn test_tx_with_callsign() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_tx_cs.wav");
        let path_str = path.to_str().unwrap();
        cmd_tx(
            "CQ CQ",
            Some(path_str),
            None,
            Some("VK2ABC"),
            None,
            "none",
            "127.0.0.1:4532",
            50,
            200,
            None,
            None,
            Verbosity::Verbose,
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

    #[test]
    fn test_config_list() {
        cmd_config(None).expect("config list should succeed");
    }

    #[test]
    fn test_config_named_profile() {
        let profiles = coppa_engine::profiles::list_profiles();
        if let Some(&name) = profiles.first() {
            cmd_config(Some(name)).expect("config with valid profile should succeed");
        }
    }

    #[test]
    fn test_config_unknown_profile() {
        cmd_config(Some("NONEXISTENT_PROFILE"))
            .expect("config with unknown profile should not error");
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
            Some(Commands::Tx {
                rigctld,
                ptt_lead_ms,
                ptt_tail_ms,
                ..
            }) => {
                assert_eq!(rigctld, "192.168.1.50:4532");
                assert_eq!(ptt_lead_ms, 75);
                assert_eq!(ptt_tail_ms, 500);
            }
            _ => panic!("expected Tx command"),
        }
    }

    #[test]
    fn test_cli_tx_ptt_flags_default() {
        let cli = Cli::parse_from(["coppa", "tx", "hello"]);
        match cli.command {
            Some(Commands::Tx {
                rigctld,
                ptt_lead_ms,
                ptt_tail_ms,
                ..
            }) => {
                assert_eq!(rigctld, "127.0.0.1:4532");
                assert_eq!(ptt_lead_ms, 50);
                assert_eq!(ptt_tail_ms, 200);
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
            Some(Commands::Tune {
                rigctld,
                ptt_lead_ms,
                ptt_tail_ms,
                ..
            }) => {
                assert_eq!(rigctld, "10.0.0.5:4532");
                assert_eq!(ptt_lead_ms, 10);
                assert_eq!(ptt_tail_ms, 20);
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
            pub events: Vec<(Duration, String)>,
            fractional: f64,
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
                    events: Vec::new(),
                    fractional: 0.0,
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
        let ten_m = cli_frequency_evidence(None, Some("10m")).unwrap();
        let two_m = cli_frequency_evidence(None, Some("2m")).unwrap();

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
            None,
            "none",
            "127.0.0.1:1",
            0,
            0,
            None,
            Some("21m"),
            Verbosity::Quiet,
        )
        .unwrap_err()
        .to_string()
        .contains("expected an amateur band label"));
    }

    #[test]
    fn cli_band_flag_typo_is_a_hard_error() {
        let err = cli_frequency_evidence(None, Some("21m")).unwrap_err();
        assert!(
            err.to_string().contains("expected an amateur band label"),
            "{err}"
        );
        assert!(err.to_string().contains("--band"), "{err}");
    }

    #[test]
    fn cli_frequency_band_contradiction_is_a_hard_error() {
        let err = cli_frequency_evidence(Some(14_074_000), Some("2m")).unwrap_err();
        assert!(err.to_string().contains("lies outside band"), "{err}");
        assert!(cli_frequency_evidence(Some(144_500_000), Some("2m")).is_ok());
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
            None,
            "rigctld-typo",
            "127.0.0.1:1",
            0,
            0,
            None,
            None,
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
            Some(Commands::Tx {
                frequency_hz, band, ..
            }) => {
                assert_eq!(frequency_hz, Some(144_500_000));
                assert_eq!(band, None);
            }
            _ => panic!("expected Tx command"),
        }
        let cli = Cli::parse_from(["coppa", "tune", "--band", "2m"]);
        match cli.command {
            Some(Commands::Tune { band, .. }) => assert_eq!(band.as_deref(), Some("2m")),
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
            None,
            "none",
            "127.0.0.1:1",
            0,
            0,
            None,
            None,
            Verbosity::Quiet,
        )
        .unwrap_err();
        assert!(err.to_string().contains("refusing to transmit"), "{err}");

        let path = std::env::temp_dir().join(format!("cop40_{}.wav", std::process::id()));
        cmd_tx(
            "hi",
            Some(path.to_str().unwrap()),
            Some("VHF_FAST"),
            None,
            None,
            "none",
            "127.0.0.1:1",
            0,
            0,
            None,
            None,
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
                Some(path_str),
                None,
                None,
                None,
                "none",
                "127.0.0.1:4532",
                50,
                200,
                None,
                None,
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
        let Some(Commands::Rx(args)) = cli.command else {
            panic!("expected Rx")
        };
        assert_eq!(args.path(), Some("x.wav"));
        assert_eq!(args.duration, 5);
        assert_eq!(args.callsign.as_deref(), Some("VK2ABC"));
        assert!(args.raw && args.hex);
        assert_eq!(args.profile.as_deref(), Some("HF_ROBUST"));
        assert_eq!(args.device.as_deref(), Some("Mic"));

        let cli = Cli::parse_from(["coppa", "rx", "-i", "y.wav"]);
        let Some(Commands::Rx(args)) = cli.command else {
            panic!("expected Rx")
        };
        assert_eq!(args.path(), Some("y.wav"));
        assert_eq!(args.duration, 0);
    }

    #[test]
    fn rx_rejects_a_path_and_input_together() {
        let err = Cli::try_parse_from(["coppa", "rx", "a.wav", "-i", "b.wav"])
            .err()
            .expect("FILE and -i together must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
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

    #[test]
    fn stream_decode_stops_a_stalled_live_source_at_its_deadline() {
        let start = std::time::Instant::now();
        let deadline = Some(start + std::time::Duration::from_millis(100));
        stream_decode(
            &mut CoppaCore::new(),
            &mut Stalled,
            None,
            deadline,
            false,
            false,
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
    fn stream_decode_stops_a_streaming_live_source_at_its_deadline() {
        let start = std::time::Instant::now();
        let deadline = Some(start + std::time::Duration::from_millis(100));
        stream_decode(
            &mut CoppaCore::new(),
            &mut Endless,
            None,
            deadline,
            false,
            false,
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
        stream_decode(
            &mut CoppaCore::new(),
            &mut source,
            Some(10_000),
            None,
            true,
            false,
            Verbosity::Quiet,
        )
        .unwrap();
        assert_eq!(source.handed_out, 10_000);
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
        let Some(Commands::Rx(rx)) = parse("rx") else {
            panic!("expected Rx")
        };
        let Some(Commands::Listen(listen)) = parse("listen") else {
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
}
