//! Coppa Daemon (coppad) - long-running service for the Coppa digital communications system.
//!
//! Manages the engine lifecycle, host interfaces, and radio control in a
//! persistent background process.

// Shared with the library target; the binary uses only part of its API
// (and none of the output worker without `cpal-backend`).
#[allow(dead_code)]
mod audio_output;
mod cli;
mod config;
mod event_loop;
mod ptt_guardian;
#[cfg(feature = "websocket")]
mod spectrum;

#[cfg(feature = "kiss-tnc")]
mod tnc;
mod tx_chunker;

use std::io::Write;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, ConfigSource, LogLevel, Mode};
use config::DaemonConfig;
use event_loop::{DaemonEvent, EventLoop};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse first: help, version and usage errors exit here, before any
    // logging, config loading or service startup.
    let cli = Cli::parse();

    match cli.mode() {
        Mode::PrintDefaultConfig => print_default_config(),
        Mode::Check(source) => check_config(&source),
        Mode::Tnc => run_tnc(cli.log_level).await,
        Mode::PttGuardian {
            ptt_method,
            rigctld_address,
        } => run_ptt_guardian(cli.log_level, ptt_method, rigctld_address).await,
        Mode::Run(source) => {
            // Load before logging so a config error is the only output.
            let config = load_config(&source)?;
            init_tracing(cli.log_level);
            log_banner();
            run_daemon(config).await
        }
    }
}

/// Load the configuration a command selected. An explicit path must exist;
/// the implicit `./coppad.toml` falls back to defaults only when absent.
fn load_config(source: &ConfigSource) -> Result<DaemonConfig> {
    match source {
        ConfigSource::Explicit(path) => DaemonConfig::load(path)
            .with_context(|| format!("Failed to load config {}", path.display())),
        ConfigSource::Implicit => DaemonConfig::load_or_default(cli::DEFAULT_CONFIG_PATH),
    }
}

/// `--check`: read and deserialize the configuration only. Deliberately
/// returns before PTT parsing or `EventLoop::new`, which can touch hardware.
fn check_config(source: &ConfigSource) -> Result<()> {
    load_config(source)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "Configuration syntax and field types OK.")?;
    writeln!(stdout, "Hardware and runtime settings were not checked.")?;
    Ok(())
}

/// `--print-default-config`: the built-in configuration as TOML, read from
/// `DaemonConfig::default()` so it tracks future defaults.
fn print_default_config() -> Result<()> {
    let toml = toml::to_string_pretty(&DaemonConfig::default())
        .context("Failed to serialize the default config")?;
    writeln!(std::io::stdout().lock(), "{}", toml.trim_end())?;
    Ok(())
}

/// Structured logging for the run modes: `--log-level` overrides `RUST_LOG`,
/// which falls back to `info` when absent or invalid.
fn init_tracing(level: Option<LogLevel>) {
    let rust_log = std::env::var(tracing_subscriber::EnvFilter::DEFAULT_ENV).ok();
    tracing_subscriber::fmt()
        .with_env_filter(cli::log_filter(level, rust_log.as_deref()))
        .init();
}

fn log_banner() {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "coppad - Coppa Daemon starting"
    );
}

/// `--tnc`: the KISS TNC with its own defaults (no daemon config file).
#[cfg(feature = "kiss-tnc")]
async fn run_tnc(level: Option<LogLevel>) -> Result<()> {
    init_tracing(level);
    log_banner();
    tnc::run_tnc(tnc::TncConfig::default()).await
}

/// `--tnc` in a build without the TNC: fail before logging anything.
#[cfg(not(feature = "kiss-tnc"))]
async fn run_tnc(_level: Option<LogLevel>) -> Result<()> {
    anyhow::bail!("TNC mode requires the kiss-tnc feature")
}

/// `--ptt-guardian` (internal, COP-31): read the daemon's PTT reports on
/// stdin until it exits, then release PTT if it exited keyed. Exits 1 if the
/// release fails after its bounded retries.
async fn run_ptt_guardian(
    level: Option<LogLevel>,
    ptt_method: String,
    rigctld_address: String,
) -> Result<()> {
    init_tracing(level);

    // Replace the default actions of SIGINT, SIGTERM and SIGHUP for the
    // guardian's whole life: it must outlive the daemon, and only stdin EOF
    // ends it. The guardian runs in its own process group, but a signal sent
    // to it directly (or a terminal hang-up) must not stop it either. The
    // streams are never polled; holding them keeps the handlers installed.
    #[cfg(unix)]
    let (_sigint, _sigterm, _sighup) = {
        use tokio::signal::unix::{signal, SignalKind};
        (
            signal(SignalKind::interrupt()).context("failed to register SIGINT handler")?,
            signal(SignalKind::terminate()).context("failed to register SIGTERM handler")?,
            signal(SignalKind::hangup()).context("failed to register SIGHUP handler")?,
        )
    };

    let ptt = config::RadioConfig {
        ptt_method: ptt_method.clone(),
        rigctld_address: rigctld_address.clone(),
        ..Default::default()
    }
    .ptt_config()?;

    let outcome = tokio::task::spawn_blocking(move || {
        let mut first_attempt = true;
        ptt_guardian::watch(std::io::stdin().lock(), ptt_guardian::RELEASE_RETRY, || {
            if std::mem::take(&mut first_attempt) {
                tracing::warn!(
                    target: "coppad::ptt_guardian",
                    ptt_method = %ptt_method,
                    "coppad exited while PTT was keyed; releasing it"
                );
            }
            ptt_guardian::release_once(&ptt, &rigctld_address)
        })
    })
    .await
    .context("PTT guardian task failed")?;

    match outcome {
        ptt_guardian::Outcome::ExitedUnkeyed => {
            tracing::debug!(
                target: "coppad::ptt_guardian",
                "coppad exited with PTT released; nothing to do"
            );
            Ok(())
        }
        ptt_guardian::Outcome::Released => {
            tracing::info!(target: "coppad::ptt_guardian", "PTT released");
            Ok(())
        }
        ptt_guardian::Outcome::ReleaseFailed(e) => {
            tracing::error!(
                target: "coppad::ptt_guardian",
                error = %e,
                "Could not release PTT after coppad exited; the transmitter may still be keyed"
            );
            Err(e)
        }
    }
}

async fn run_daemon(config: DaemonConfig) -> Result<()> {
    if config.engine.callsign.is_empty() {
        tracing::warn!("No callsign configured. Set [engine] callsign in config.");
    }

    tracing::info!(
        profile = %config.engine.profile,
        sample_rate = config.audio.sample_rate,
        ptt_method = %config.radio.ptt_method,
        "Daemon configuration loaded"
    );

    // Create event loop. Fails loudly (rather than silently falling back to
    // NullPtt) if [radio] ptt_method doesn't parse or names an
    // unrecognized/unbuilt PTT backend -- see EventLoop::create_ptt.
    let mut event_loop = EventLoop::new(config.clone())
        .map_err(|e| anyhow::anyhow!("failed to start daemon event loop: {e}"))?;

    // COP-31: for rigctld or GPIO PTT, start the out-of-process guardian
    // that releases PTT if this process dies keyed (SIGKILL, OOM, crash).
    // Failing to start it is logged, not fatal.
    if event_loop.ptt_guardian_eligible() {
        match ptt_guardian::PttGuardian::spawn(&config.radio) {
            Ok(guardian) => {
                tracing::info!(
                    pid = guardian.pid().unwrap_or_default(),
                    "PTT guardian started"
                );
                event_loop.set_ptt_guardian(guardian);
            }
            Err(e) => tracing::error!(
                error = %e,
                "Could not start the PTT guardian; if coppad is killed while transmitting, \
                 PTT may stay keyed"
            ),
        }
    }

    let event_tx = event_loop.event_sender();

    // Create response channel and wire it to the event loop
    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel(64);
    event_loop.set_response_tx(response_tx);

    // Wire audio ring buffers
    let (audio_out_producer, audio_out_consumer) =
        coppa_audio::audio_ring(config.audio.buffer_size);
    let (audio_in_producer, audio_in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    event_loop.set_audio_in(audio_in_consumer);

    // E5: Get shutdown flag for audio threads
    let shutdown_flag = event_loop.shutdown_flag();

    // Connect CPAL audio streams to ring buffers
    #[cfg(feature = "cpal-backend")]
    {
        use coppa_audio::{AudioSink, AudioSource};

        let sample_rate = config.audio.sample_rate;
        let buf_size = config.audio.buffer_size;

        // Spawn audio input: CPAL mic -> ring buffer -> event loop polls it
        let mut audio_in_prod = audio_in_producer;
        let input_shutdown = shutdown_flag.clone();
        let input_device_name = config.audio.input_device.clone();
        let source_result = if input_device_name.is_empty() {
            coppa_audio::CpalSource::new(sample_rate, buf_size)
        } else {
            match coppa_audio::find_input_device_by_name(&input_device_name) {
                Some(device) => {
                    tracing::info!(device = %input_device_name, "Audio input: using named device");
                    coppa_audio::cpal_backend::CpalSource::from_device(
                        device,
                        sample_rate,
                        buf_size,
                    )
                }
                None => {
                    tracing::warn!(
                        device = %input_device_name,
                        "Audio input device not found, falling back to default"
                    );
                    coppa_audio::CpalSource::new(sample_rate, buf_size)
                }
            }
        };
        match source_result {
            Ok(mut source) => {
                if let Err(e) = source.start() {
                    eprintln!("Failed to start audio input: {}", e);
                } else {
                    tracing::info!(sample_rate, "Audio input started");
                    tokio::task::spawn_blocking(move || {
                        let mut buf = vec![0.0f32; 1024];
                        loop {
                            // E5: Check shutdown flag in addition to is_abandoned
                            if audio_in_prod.is_abandoned()
                                || input_shutdown.load(std::sync::atomic::Ordering::Acquire)
                            {
                                break;
                            }
                            match source.read(&mut buf) {
                                Ok(n) if n > 0 => {
                                    audio_in_prod.write(&buf[..n]);
                                }
                                _ => {
                                    std::thread::sleep(std::time::Duration::from_millis(10));
                                }
                            }
                        }
                    });
                }
            }
            Err(e) => eprintln!("No audio input device: {}", e),
        }

        // Spawn audio output: event loop writes to ring buffer -> CPAL speaker.
        // The sink is created, started, stopped and dropped on this one
        // worker thread (CPAL streams are thread-affine), and the worker
        // retains each chunk until the sink has accepted all of it, feeding
        // playback progress back so PTT is released from samples played.
        let output_shutdown = shutdown_flag.clone();
        let output_device_name = config.audio.output_device.clone();
        let output_feedback = std::sync::Arc::new(audio_output::OutputFeedback::default());
        // Only wire the audio-out ring when this worker drains it: TX audio
        // is retained until the ring accepts it, so an undrained ring would
        // accumulate every transmission. If the device fails to open, the
        // worker drops its end, the ring reads as abandoned and nothing more
        // is queued.
        event_loop.set_audio_out(audio_out_producer);
        event_loop.set_output_feedback(output_feedback.clone());
        tokio::task::spawn_blocking(move || {
            let sink_result = if output_device_name.is_empty() {
                coppa_audio::CpalSink::new(sample_rate, buf_size)
            } else {
                match coppa_audio::find_output_device_by_name(&output_device_name) {
                    Some(device) => {
                        tracing::info!(device = %output_device_name, "Audio output: using named device");
                        coppa_audio::cpal_backend::CpalSink::from_device(
                            device,
                            sample_rate,
                            buf_size,
                        )
                    }
                    None => {
                        tracing::warn!(
                            device = %output_device_name,
                            "Audio output device not found, falling back to default"
                        );
                        coppa_audio::CpalSink::new(sample_rate, buf_size)
                    }
                }
            };
            let mut sink = match sink_result {
                Ok(sink) => sink,
                Err(e) => {
                    eprintln!("No audio output device: {}", e);
                    output_feedback.fail();
                    return;
                }
            };
            if let Err(e) = sink.start() {
                eprintln!("Failed to start audio output: {}", e);
                output_feedback.fail();
                return;
            }
            tracing::info!(sample_rate, "Audio output started");
            audio_output::OutputWorker::new(1024, sample_rate, output_feedback).run(
                audio_out_consumer,
                sink,
                |sink| sink.playback_progress(),
                output_shutdown,
            );
        });
    }

    // Suppress unused variable warnings when cpal-backend is not enabled
    #[cfg(not(feature = "cpal-backend"))]
    {
        // No hardware output: leave the audio-out ring unwired so TX audio is
        // not queued for a consumer that never drains it.
        drop(audio_out_producer);
        drop(audio_out_consumer);
        let _ = audio_in_producer;
        let _ = shutdown_flag;
    }

    // Start VARA TCP server
    if config.host.vara_enabled {
        let mut vara_server = coppa_host::vara::VaraServer::with_bind_addr(
            config.host.vara_command_port,
            config.host.vara_data_port,
            config.host.bind_address.clone(),
        );
        tracing::info!(
            command_port = config.host.vara_command_port,
            data_port = config.host.vara_data_port,
            "VARA TCP server starting"
        );

        // Pipe VARA events into the daemon event loop
        if let Some(mut vara_rx) = vara_server.take_event_rx() {
            let vara_event_tx = event_tx.clone();
            tokio::spawn(async move {
                while let Some(host_event) = vara_rx.recv().await {
                    if vara_event_tx
                        .send(DaemonEvent::Host(host_event))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }

        // Wire VARA command-port response senders so the event loop can broadcast
        // SNR/PTT/BUFFER/BUSY telemetry (decision 8, Phase 3 Task 7).
        event_loop.set_vara_responses(vara_server.response_senders());

        // Bridge response_rx to VARA data_senders (decoded data goes via the data port)
        let vara_data_senders = vara_server.data_senders();
        tokio::spawn(async move {
            while let Some(response) = response_rx.recv().await {
                // StatusUpdate is deliberately not forwarded: EventLoop::notify_session
                // pushes session state directly to the command port and WebSocket.
                if let coppa_host::HostResponse::DataOut { client_id, data } = &response {
                    let senders = vara_data_senders.lock().await;
                    if *client_id == 0 {
                        // Broadcast to all data-port clients
                        for tx in senders.values() {
                            let _ = tx.try_send(data.clone());
                        }
                    } else if let Some(tx) = senders.get(client_id) {
                        let _ = tx.try_send(data.clone());
                    }
                }
            }
        });

        // Actually start the TCP listeners
        tokio::spawn(async move {
            if let Err(e) = vara_server.run().await {
                eprintln!("VARA server error: {}", e);
            }
        });
    }

    // Start WebSocket server if enabled
    #[cfg(feature = "websocket")]
    if config.host.websocket_enabled {
        let ws_port = config.host.websocket_port;
        tracing::info!(port = ws_port, "WebSocket server starting");
        let mut ws_server = coppa_host::websocket::WebSocketServer::with_bind_addr(
            ws_port,
            config.host.bind_address.clone(),
        );

        // Pipe WebSocket events into the daemon event loop
        if let Some(mut ws_rx) = ws_server.take_event_rx() {
            let ws_event_tx = event_tx.clone();
            tokio::spawn(async move {
                while let Some(host_event) = ws_rx.recv().await {
                    if ws_event_tx
                        .send(DaemonEvent::Host(host_event))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }

        // Wire broadcast sender so the event loop can forward decoded data to WS clients
        event_loop.set_ws_broadcast(ws_server.broadcast_sender());
        // Wire the live status snapshot so `status` replies carry real values
        // (decision 8, Phase 3 Task 7).
        event_loop.set_ws_status(ws_server.status());

        tokio::spawn(async move {
            if let Err(e) = ws_server.run().await {
                eprintln!("WebSocket server error: {}", e);
            }
        });
    }

    // Handle shutdown signals
    let shutdown_tx = event_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Received SIGINT, shutting down");
        let _ = shutdown_tx.send(DaemonEvent::Shutdown).await;
    });

    #[cfg(unix)]
    {
        let sigterm_tx = event_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            sigterm.recv().await;
            tracing::info!("Received SIGTERM, shutting down");
            let _ = sigterm_tx.send(DaemonEvent::Shutdown).await;
        });
    }

    tracing::info!("Daemon ready");

    // Run the event loop
    event_loop.run().await?;

    tracing::info!("Daemon stopped");
    Ok(())
}
