//! Coppa CLI - command-line interface for the Coppa digital communications system.

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
    },
    /// Receive and decode audio.
    Rx {
        /// Input file containing audio samples (WAV format).
        #[arg(short, long)]
        input: Option<String>,
        /// Operational profile name.
        #[arg(long)]
        profile: Option<String>,
        /// Print only the decoded text with no labels.
        #[arg(long)]
        raw: bool,
    },
    /// Run a loopback test (encode -> decode).
    Loopback {
        /// Message to test.
        message: String,
        /// Operational profile name.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Listen for incoming transmissions.
    Listen {
        /// Duration in seconds (0 = indefinite).
        #[arg(short, long, default_value = "0")]
        duration: u64,
        /// Operational profile name.
        #[arg(long)]
        profile: Option<String>,
        /// Callsign for station identification.
        #[arg(long)]
        callsign: Option<String>,
        /// Print only the decoded text with no labels.
        #[arg(long)]
        raw: bool,
        /// Audio input device name (substring match).
        #[arg(long)]
        device: Option<String>,
    },
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
    },
    /// List available audio devices.
    Devices,
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

fn main() -> Result<()> {
    let cli = Cli::parse();

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
        }) => cmd_tx(
            &message,
            output.as_deref(),
            profile.as_deref(),
            callsign.as_deref(),
            device.as_deref(),
            &ptt,
            verbosity,
        )?,
        Some(Commands::Rx {
            input,
            profile,
            raw,
        }) => cmd_rx(input.as_deref(), profile.as_deref(), raw, verbosity)?,
        Some(Commands::Loopback { message, profile }) => {
            cmd_loopback(&message, profile.as_deref(), verbosity)?
        }
        Some(Commands::Listen {
            duration,
            profile,
            callsign,
            raw,
            device,
        }) => cmd_listen(
            duration,
            profile.as_deref(),
            callsign.as_deref(),
            raw,
            device.as_deref(),
            verbosity,
        )?,
        Some(Commands::Tune {
            seconds,
            single,
            output,
            profile,
            device,
            ptt,
        }) => cmd_tune(
            seconds,
            single,
            output.as_deref(),
            profile.as_deref(),
            device.as_deref(),
            &ptt,
            verbosity,
        )?,
        Some(Commands::Devices) => cmd_devices()?,
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

fn cmd_tx(
    message: &str,
    output: Option<&str>,
    profile: Option<&str>,
    callsign: Option<&str>,
    device: Option<&str>,
    ptt: &str,
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
        Some(path) => {
            #[cfg(feature = "file-backend")]
            {
                use coppa_audio::{file_backend::WavSink, AudioSink};
                let mut sink = WavSink::new(path, core.config().sample_rate);
                sink.start()?;
                sink.write(&samples)?;
                sink.stop()?;
                if verbosity != Verbosity::Quiet {
                    println!("Written to {}", path);
                }
            }
            #[cfg(not(feature = "file-backend"))]
            {
                let _ = path;
                if verbosity != Verbosity::Quiet {
                    println!("File output not available (compile with file-backend feature)");
                }
            }
        }
        None => {
            #[cfg(feature = "cpal-backend")]
            {
                use coppa_audio::AudioSink;
                use coppa_radio::{PttControl, PttState};

                let engine_config = core.config();

                // Create PTT controller
                let mut ptt_ctrl: Box<dyn PttControl> = match ptt {
                    "rigctld" => match coppa_radio::RigctldClient::connect("127.0.0.1:4532") {
                        Ok(client) => Box::new(client),
                        Err(e) => {
                            eprintln!("WARNING: rigctld connect failed ({}), using no PTT", e);
                            Box::new(coppa_radio::NullPtt::new())
                        }
                    },
                    "vox" => Box::new(coppa_radio::VoxPtt::new()),
                    _ => Box::new(coppa_radio::NullPtt::new()),
                };

                // Create audio sink
                let mut sink = match device {
                    Some(name) => match coppa_audio::find_output_device_by_name(name) {
                        Some(dev) => {
                            if verbosity != Verbosity::Quiet {
                                eprintln!("Using output device matching '{}'", name);
                            }
                            coppa_audio::cpal_backend::CpalSink::from_device(
                                dev,
                                engine_config.sample_rate,
                                8192,
                            )?
                        }
                        None => {
                            eprintln!(
                                "WARNING: No output device matching '{}', using default",
                                name
                            );
                            coppa_audio::cpal_backend::CpalSink::new(
                                engine_config.sample_rate,
                                8192,
                            )?
                        }
                    },
                    None => {
                        coppa_audio::cpal_backend::CpalSink::new(engine_config.sample_rate, 8192)?
                    }
                };
                sink.start()?;

                // Key PTT
                let _ = ptt_ctrl.set_ptt(PttState::Tx);
                std::thread::sleep(std::time::Duration::from_millis(50));

                // Write audio
                sink.write(&samples)?;

                // Wait for audio to play out
                let duration_ms =
                    (samples.len() as f64 / engine_config.sample_rate as f64 * 1000.0) as u64;
                std::thread::sleep(std::time::Duration::from_millis(duration_ms + 200));

                // Unkey PTT
                let _ = ptt_ctrl.set_ptt(PttState::Rx);
                sink.stop()?;

                if verbosity != Verbosity::Quiet {
                    eprintln!(
                        "Transmitted {} samples ({:.2}s)",
                        samples.len(),
                        duration_ms as f64 / 1000.0
                    );
                }
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                let _ = device;
                let _ = ptt;
                if verbosity != Verbosity::Quiet {
                    println!("Live audio output not available (compile with cpal-backend feature)");
                }
            }
        }
    }

    Ok(())
}

/// TX level calibration ("TUNE"): generate the standard SSB two-tone signal
/// (or a single tone via `single`), key PTT, stream it out, then unkey —
/// mirroring `cmd_tx`'s live-playback path. See `docs/OPERATING.md` for the
/// calibration procedure this is meant to support.
fn cmd_tune(
    seconds: f32,
    single: Option<f32>,
    output: Option<&str>,
    profile: Option<&str>,
    device: Option<&str>,
    ptt: &str,
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
        Some(path) => {
            #[cfg(feature = "file-backend")]
            {
                use coppa_audio::{file_backend::WavSink, AudioSink};
                let mut sink = WavSink::new(path, core.config().sample_rate);
                sink.start()?;
                sink.write(&samples)?;
                sink.stop()?;
                if verbosity != Verbosity::Quiet {
                    println!("Written to {}", path);
                }
            }
            #[cfg(not(feature = "file-backend"))]
            {
                let _ = path;
                if verbosity != Verbosity::Quiet {
                    println!("File output not available (compile with file-backend feature)");
                }
            }
        }
        None => {
            #[cfg(feature = "cpal-backend")]
            {
                use coppa_audio::AudioSink;
                use coppa_radio::{PttControl, PttState};

                let engine_config = core.config();

                // Create PTT controller
                let mut ptt_ctrl: Box<dyn PttControl> = match ptt {
                    "rigctld" => match coppa_radio::RigctldClient::connect("127.0.0.1:4532") {
                        Ok(client) => Box::new(client),
                        Err(e) => {
                            eprintln!("WARNING: rigctld connect failed ({}), using no PTT", e);
                            Box::new(coppa_radio::NullPtt::new())
                        }
                    },
                    "vox" => Box::new(coppa_radio::VoxPtt::new()),
                    _ => Box::new(coppa_radio::NullPtt::new()),
                };

                // Create audio sink
                let mut sink = match device {
                    Some(name) => match coppa_audio::find_output_device_by_name(name) {
                        Some(dev) => {
                            if verbosity != Verbosity::Quiet {
                                eprintln!("Using output device matching '{}'", name);
                            }
                            coppa_audio::cpal_backend::CpalSink::from_device(
                                dev,
                                engine_config.sample_rate,
                                8192,
                            )?
                        }
                        None => {
                            eprintln!(
                                "WARNING: No output device matching '{}', using default",
                                name
                            );
                            coppa_audio::cpal_backend::CpalSink::new(
                                engine_config.sample_rate,
                                8192,
                            )?
                        }
                    },
                    None => {
                        coppa_audio::cpal_backend::CpalSink::new(engine_config.sample_rate, 8192)?
                    }
                };
                sink.start()?;

                // Key PTT
                let _ = ptt_ctrl.set_ptt(PttState::Tx);
                std::thread::sleep(std::time::Duration::from_millis(50));

                // Stream the tone
                sink.write(&samples)?;

                // Wait for the tone to play out
                let duration_ms =
                    (samples.len() as f64 / engine_config.sample_rate as f64 * 1000.0) as u64;
                std::thread::sleep(std::time::Duration::from_millis(duration_ms + 200));

                // Unkey PTT
                let _ = ptt_ctrl.set_ptt(PttState::Rx);
                sink.stop()?;

                if verbosity != Verbosity::Quiet {
                    eprintln!(
                        "Transmitted {} samples ({:.2}s)",
                        samples.len(),
                        duration_ms as f64 / 1000.0
                    );
                }
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                let _ = device;
                let _ = ptt;
                if verbosity != Verbosity::Quiet {
                    println!("Live audio output not available (compile with cpal-backend feature)");
                }
            }
        }
    }

    Ok(())
}

fn cmd_rx(
    input: Option<&str>,
    profile: Option<&str>,
    raw: bool,
    verbosity: Verbosity,
) -> Result<()> {
    if let Some(path) = input {
        #[cfg(feature = "file-backend")]
        {
            use coppa_audio::{file_backend::WavSource, AudioSource};
            let mut source = WavSource::open(path)?;
            let total = source.total_samples();
            if verbosity != Verbosity::Quiet && !raw {
                println!("Reading {} samples from {}", total, path);
            }

            let mut samples = vec![0.0f32; total];
            source.start()?;
            source.read(&mut samples)?;

            let config = resolve_config(profile)?;
            let core = CoppaCore::with_config(config);

            if verbosity == Verbosity::Verbose {
                eprintln!(
                    "[verbose] Input samples: {}, sample_rate: {}",
                    total,
                    core.config().sample_rate
                );
            }

            match core.decode(&samples) {
                Ok(message) => {
                    if raw {
                        print!("{}", message);
                    } else {
                        println!("Decoded: \"{}\"", message);
                    }
                }
                Err(e) => println!("Decode failed: {}", e),
            }
        }
        #[cfg(not(feature = "file-backend"))]
        {
            let _ = path;
            let _ = profile;
            let _ = raw;
            if verbosity != Verbosity::Quiet {
                println!("File input not available (compile with file-backend feature)");
            }
        }
    } else if verbosity != Verbosity::Quiet {
        println!("Live audio input not yet implemented. Use --input to specify a WAV file.");
    }

    Ok(())
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

fn cmd_listen(
    duration: u64,
    profile: Option<&str>,
    callsign: Option<&str>,
    raw: bool,
    device: Option<&str>,
    verbosity: Verbosity,
) -> Result<()> {
    if verbosity != Verbosity::Quiet && !raw {
        if duration == 0 {
            println!("Listening indefinitely (Ctrl+C to stop)...");
        } else {
            println!("Listening for {} seconds...", duration);
        }
    }

    if verbosity == Verbosity::Verbose {
        if let Some(cs) = callsign {
            eprintln!("[verbose] Callsign: {}", cs);
        }
    }

    #[cfg(feature = "cpal-backend")]
    {
        use coppa_audio::AudioSource;

        let config = resolve_config(profile)?;
        let engine_rate = config.sample_rate;
        let mut source = match device {
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

        let core = CoppaCore::with_config(config);
        let mut read_buf = vec![0.0f32; 4000];
        let mut window: Vec<f32> = Vec::with_capacity(65536);
        let max_window = 64000;
        let start = std::time::Instant::now();
        let mut last_liveness = std::time::Instant::now();

        loop {
            if duration > 0 && start.elapsed().as_secs() >= duration {
                break;
            }

            // F5: Liveness indicator -- print "." to stderr every 5 seconds
            if verbosity != Verbosity::Quiet && last_liveness.elapsed().as_secs() >= 5 {
                eprint!(".");
                last_liveness = std::time::Instant::now();
            }

            let n = source.read(&mut read_buf)?;
            if n > 0 {
                window.extend_from_slice(&read_buf[..n]);

                if verbosity == Verbosity::Verbose {
                    eprintln!("[verbose] Buffer: {} samples", window.len());
                }

                match core.decode(&window) {
                    Ok(message) => {
                        if raw {
                            print!("{}", message);
                        } else {
                            println!("Decoded: \"{}\"", message);
                        }
                        window.clear();
                    }
                    Err(_) => {
                        if window.len() > max_window {
                            let drain = window.len() / 2;
                            window.drain(..drain);
                        }
                    }
                }
            }
        }

        source.stop()?;
        if verbosity != Verbosity::Quiet && !raw {
            println!("\nStopped listening.");
        }
    }

    #[cfg(not(feature = "cpal-backend"))]
    {
        let _ = duration;
        let _ = profile;
        let _ = callsign;
        let _ = raw;
        let _ = device;
        if verbosity != Verbosity::Quiet {
            println!("(Live audio capture not available -- compile with cpal-backend feature)");
        }
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
        };
        coppa_daemon::tnc::run_tnc(config).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let result = cmd_rx(
                Some("/tmp/coppa_nonexistent_12345.wav"),
                None,
                false,
                Verbosity::Normal,
            );
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
}
