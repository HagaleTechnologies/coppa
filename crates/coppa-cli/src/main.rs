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
        /// rigctld address for CAT PTT (only used when --ptt rigctld).
        #[arg(long, default_value = "127.0.0.1:4532")]
        rigctld: String,
        /// Milliseconds to key PTT before audio starts playing.
        #[arg(long, default_value = "50")]
        ptt_lead_ms: u64,
        /// Milliseconds to hold PTT keyed after audio finishes playing.
        #[arg(long, default_value = "200")]
        ptt_tail_ms: u64,
    },
    /// Receive and decode audio (streaming: WAV file if --input is given,
    /// otherwise live capture from an audio input device until Ctrl+C).
    Rx {
        /// Input file containing audio samples (WAV format). If omitted,
        /// captures live audio from an input device instead.
        #[arg(short, long)]
        input: Option<String>,
        /// Operational profile name.
        #[arg(long)]
        profile: Option<String>,
        /// Print only the decoded payload (lowercase hex) with no labels.
        #[arg(long)]
        raw: bool,
        /// Audio input device name (substring match, live capture only).
        #[arg(long)]
        device: Option<String>,
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
        /// rigctld address for CAT PTT (only used when --ptt rigctld).
        #[arg(long, default_value = "127.0.0.1:4532")]
        rigctld: String,
        /// Milliseconds to key PTT before audio starts playing.
        #[arg(long, default_value = "50")]
        ptt_lead_ms: u64,
        /// Milliseconds to hold PTT keyed after audio finishes playing.
        #[arg(long, default_value = "200")]
        ptt_tail_ms: u64,
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
            rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
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
            verbosity,
        )?,
        Some(Commands::Rx {
            input,
            profile,
            raw,
            device,
        }) => cmd_rx(
            input.as_deref(),
            profile.as_deref(),
            raw,
            device.as_deref(),
            verbosity,
        )?,
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
            rigctld,
            ptt_lead_ms,
            ptt_tail_ms,
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

/// Build the PTT controller for `--ptt <method>`, using `rigctld_addr` when
/// `method` is `"rigctld"`. Split out from `transmit_live` so the
/// `--rigctld`/CLI-flag plumbing can be unit-tested without a real audio
/// device.
#[cfg(feature = "cpal-backend")]
fn build_ptt(ptt: &str, rigctld_addr: &str) -> Box<dyn coppa_radio::PttControl> {
    match ptt {
        "rigctld" => match coppa_radio::RigctldClient::connect(rigctld_addr) {
            Ok(client) => Box::new(client),
            Err(e) => {
                eprintln!("WARNING: rigctld connect failed ({}), using no PTT", e);
                Box::new(coppa_radio::NullPtt::new())
            }
        },
        "vox" => Box::new(coppa_radio::VoxPtt::new()),
        _ => Box::new(coppa_radio::NullPtt::new()),
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
    verbosity: Verbosity,
) -> Result<()> {
    use coppa_audio::AudioSink;

    // Create PTT controller
    let mut ptt_ctrl = build_ptt(ptt, rigctld_addr);

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
        ptt_ctrl.as_mut(),
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
                    verbosity,
                )?;
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                let _ = device;
                let _ = ptt;
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
                    verbosity,
                )?;
            }
            #[cfg(not(feature = "cpal-backend"))]
            {
                let _ = device;
                let _ = ptt;
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

/// `coppa rx`: stream audio (a WAV file via `--input`, or a live capture
/// device otherwise) through `CoppaCore::push_samples` (the same
/// `StreamingReceiver`-based path the daemon uses), printing each decoded
/// frame's payload + SNR as it completes.
///
/// Unlike the old one-shot `core.decode(&samples)` batch call this replaces,
/// `push_samples` never forces a UTF-8 conversion (Phase 4 Task 3.5) -- frame
/// payloads are raw bytes, printed here as lowercase hex (see
/// `print_stream_frame`'s doc for why hex, not text).
fn cmd_rx(
    input: Option<&str>,
    profile: Option<&str>,
    raw: bool,
    device: Option<&str>,
    verbosity: Verbosity,
) -> Result<()> {
    let config = resolve_config(profile)?;
    let mut core = CoppaCore::with_config(config);

    if verbosity == Verbosity::Verbose {
        eprintln!("[verbose] sample_rate: {}", core.config().sample_rate);
    }

    if let Some(path) = input {
        #[cfg(feature = "file-backend")]
        {
            use coppa_audio::{file_backend::WavSource, AudioSource};
            let mut source = WavSource::open(path)?;
            let total = source.total_samples();
            if verbosity != Verbosity::Quiet && !raw {
                println!("Reading {} samples from {}", total, path);
            }
            source.start()?;
            stream_decode(&mut core, &mut source, Some(total), raw, verbosity)?;
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
            if verbosity != Verbosity::Quiet && !raw {
                println!("Listening for live audio (Ctrl+C to stop)...");
            }
            stream_decode(&mut core, &mut source, None, raw, verbosity)?;
            source.stop()?;
        }
        #[cfg(not(feature = "cpal-backend"))]
        {
            let _ = device;
            let _ = &mut core;
            if verbosity != Verbosity::Quiet {
                println!("Live audio input not available (compile with cpal-backend feature)");
            }
        }
    }

    Ok(())
}

/// Trailing silence (samples) fed to `core` after a finite (`--input <wav>`)
/// source is exhausted, so a frame whose nominal end coincides with (or is
/// close to) the file's own end can still complete.
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
/// `total_samples`, when known (the `--input <wav>` case), bounds the loop to
/// exactly that many samples so it terminates at end-of-file without relying
/// on a `read() == 0` EOF signal (live sources also return 0 whenever no new
/// audio has arrived yet, which is not EOF -- see the `None` branch below).
/// Once exhausted, `TRAILING_FLUSH_SAMPLES` of silence are pushed so a frame
/// right at the file's end can still complete (see that constant's doc).
///
/// When `total_samples` is `None` (live capture), this runs until the
/// process is interrupted (Ctrl+C), briefly sleeping on an empty read to
/// avoid a busy-spin against the non-blocking device source.
fn stream_decode(
    core: &mut CoppaCore,
    source: &mut dyn coppa_audio::AudioSource,
    total_samples: Option<usize>,
    raw: bool,
    verbosity: Verbosity,
) -> Result<()> {
    let mut buf = vec![0.0f32; 4096];
    let mut consumed = 0usize;
    loop {
        let n = source.read(&mut buf)?;
        if n > 0 {
            consumed += n;
            for frame in core.push_samples(&buf[..n]) {
                print_stream_frame(&frame, raw, verbosity);
            }
        }
        match total_samples {
            Some(total) => {
                if consumed >= total {
                    break;
                }
            }
            None if n == 0 => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            None => {}
        }
    }

    if total_samples.is_some() {
        let silence = vec![0.0f32; TRAILING_FLUSH_SAMPLES];
        for frame in core.push_samples(&silence) {
            print_stream_frame(&frame, raw, verbosity);
        }
    }

    Ok(())
}

/// Print one decoded `StreamFrame` (`CoppaCore::push_samples`'s per-frame
/// result). Payloads are raw bytes -- never UTF-8-forced, see
/// `coppa_engine::StreamFrame::payload`'s doc -- so printed as lowercase hex,
/// directly comparable to `testdata/golden/manifest.toml`'s `payload_hex`
/// field. `raw` mirrors the flag's existing meaning ("print only the decoded
/// content with no labels"): just the hex string, one per line, on stdout.
/// Decode failures are reported on stderr (never stdout, so they never
/// corrupt a `--raw` capture), suppressed entirely in `Verbosity::Quiet`.
fn print_stream_frame(frame: &coppa_engine::StreamFrame, raw: bool, verbosity: Verbosity) {
    match &frame.payload {
        Ok(bytes) => {
            let hex: String = bytes.iter().map(|b: &u8| format!("{:02x}", b)).collect();
            if raw {
                println!("{}", hex);
            } else {
                println!("Decoded: {}  (SNR: {:.1} dB)", hex, frame.snr_db);
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
            ..Default::default()
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
            "127.0.0.1:4532",
            50,
            200,
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
                None,
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
        let _ = build_ptt("none", "127.0.0.1:1");
        let _ = build_ptt("vox", "127.0.0.1:1");
    }

    /// Task 4 (Phase 4): `cmd_rx`'s `--input` path now streams through
    /// `CoppaCore::push_samples` in chunks (rather than a single batch
    /// `core.decode()` call) -- a round-trip smoke test that this still
    /// decodes cleanly end-to-end (content is checked separately by the
    /// dedicated `tests/rx_golden.rs` integration test, which asserts on
    /// actual printed hex; this just proves `cmd_rx` doesn't error).
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
                Verbosity::Quiet,
            )
            .expect("tx to file should succeed");

            let result = cmd_rx(Some(path_str), None, true, None, Verbosity::Quiet);
            std::fs::remove_file(&path).ok();
            result.expect("streaming rx of a freshly-encoded WAV should succeed");
        }
    }
}
