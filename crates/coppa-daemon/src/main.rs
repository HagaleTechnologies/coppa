//! Coppa Daemon (coppad) - long-running service for the Coppa digital communications system.
//!
//! Manages the engine lifecycle, host interfaces, and radio control in a
//! persistent background process.

// Diagnostics go through `tracing`; `--check` and `--print-default-config`
// write their output with `writeln!`.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]

// Shared with the library target; the binary uses only part of its API
// (and none of the output worker without `cpal-backend`).
#[cfg(feature = "ardop")]
mod ardop_wiring;
#[allow(dead_code)]
mod audio_output;
mod build_info;
mod cli;
mod config;
mod event_loop;
mod level_meter;
mod logging;
mod ptt_guardian;
#[cfg(feature = "websocket")]
mod spectrum;
mod startup_report;

#[cfg(feature = "kiss-tnc")]
mod tnc;
mod tx_chunker;
mod vara_wiring;

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, ConfigSource, LogLevel, Mode};
use config::{config_candidates, ConfigEnv, DaemonConfig, Discovered};
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
            let (config, discovered) = load_config(&source)?;
            init_tracing(cli.log_level);
            log_banner();
            log_config_source(&source, discovered.as_ref());
            // COP-32: a fatal error after logging starts is logged at ERROR,
            // then returned so `main` prints `Error: ...` and exits 1.
            run_daemon(config)
                .await
                .inspect_err(|e| tracing::error!("{e:#}"))
        }
    }
}

/// Load the configuration a command selected. An explicit path must exist;
/// without one, the first file found by the COP-49 search is used, falling
/// back to defaults only when none exists. The search record is returned for
/// an implicit source only.
fn load_config(source: &ConfigSource) -> Result<(DaemonConfig, Option<Discovered>)> {
    match source {
        ConfigSource::Explicit(path) => {
            let config = DaemonConfig::load(path)
                .with_context(|| format!("Failed to load config {}", path.display()))?;
            Ok((config, None))
        }
        ConfigSource::Implicit => {
            let discovered =
                DaemonConfig::discover(&config_candidates(&ConfigEnv::from_process()))?;
            Ok((discovered.config.clone(), Some(discovered)))
        }
    }
}

/// COP-49: log which file the daemon runs on, or warn that the search found
/// none and the daemon runs on built-in defaults.
fn log_config_source(source: &ConfigSource, discovered: Option<&Discovered>) {
    let used = match source {
        ConfigSource::Explicit(path) => Some(path.as_path()),
        ConfigSource::Implicit => discovered.and_then(|d| d.source.as_deref()),
    };
    if let Some(path) = used {
        tracing::info!(path = %path.display(), "Using configuration file");
    } else if let Some(discovered) = discovered {
        tracing::warn!(
            "No configuration file found; using built-in defaults (searched: {}). Copy \
             coppad.toml.example to one of these paths to configure this station.",
            discovered.search_summary()
        );
    }
}

/// `--check`: read and deserialize the configuration only. Deliberately
/// returns before PTT parsing or `EventLoop::new`, which can touch hardware.
/// For an implicit source it first says which file it checked (COP-49).
fn check_config(source: &ConfigSource) -> Result<()> {
    let (_, discovered) = load_config(source)?;
    let mut stdout = std::io::stdout().lock();
    if let Some(discovered) = discovered {
        match &discovered.source {
            Some(path) => writeln!(stdout, "Using configuration file {}", path.display())?,
            None => writeln!(
                stdout,
                "No configuration file found; checking the built-in defaults (searched: {}).",
                discovered.search_summary()
            )?,
        }
    }
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
    use tracing_subscriber::prelude::*;
    let format = tracing_subscriber::fmt::layer().with_ansi(logging::ansi_enabled(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").as_deref(),
    ));
    let subscriber = tracing_subscriber::registry()
        .with(cli::log_filter(level, rust_log.as_deref()))
        .with(format);
    #[cfg(feature = "websocket")]
    let subscriber = subscriber.with(ws_log::WsLogLayer::global().clone());
    subscriber.init();
}

fn log_banner() {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        git_sha = build_info::GIT_SHA,
        features = %build_info::features(),
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
    if let Some(warning) = config.radio.turnaround_warning() {
        tracing::warn!("{warning}");
    }

    tracing::info!(
        profile = %config.engine.profile,
        preferred_hardware_rate = config.audio.sample_rate,
        ptt_method = %config.radio.ptt_method,
        turnaround_ms = config.radio.turnaround_ms,
        "Daemon configuration loaded"
    );

    // COP-32/COP-36: config checks first, before any hardware or socket is
    // touched. The profile check comes first so a typo'd name is reported as
    // an unknown profile, not through check_sample_rate's fallback rate.
    // COP-53: a WebSocket-enabled config needs a valid token and origin list,
    // checked in every build (the keys deserialize in every build).
    config.check_profile().map_err(anyhow::Error::msg)?;
    config.check_buffer_size().map_err(anyhow::Error::msg)?;
    config.check_sample_rate().map_err(anyhow::Error::msg)?;
    config.check_audio_selectors().map_err(anyhow::Error::msg)?;
    config.check_session().map_err(anyhow::Error::msg)?;
    config.check_websocket_auth().map_err(anyhow::Error::msg)?;
    config
        .check_websocket_built(cfg!(feature = "websocket"))
        .map_err(anyhow::Error::msg)?;
    config
        .check_ardop_built(cfg!(feature = "ardop"))
        .map_err(anyhow::Error::msg)?;
    #[cfg(not(feature = "cpal-backend"))]
    check_no_selected_devices(&config.audio)?;

    // Create event loop. Fails loudly (rather than silently falling back to
    // NullPtt) if [radio] ptt_method doesn't parse or names an
    // unrecognized/unbuilt PTT backend -- see EventLoop::create_ptt. An
    // unreachable rigctld still falls back to NullPtt, but is recorded in
    // startup_problems so the daemon reports itself degraded (COP-32).
    let mut event_loop = EventLoop::new(config.clone())
        .map_err(|e| anyhow::anyhow!("failed to start daemon event loop: {e}"))?;

    // COP-31: for rigctld or GPIO PTT, start the out-of-process guardian
    // that releases PTT if this process dies keyed (SIGKILL, OOM, crash).
    // Failing to start it is logged, not fatal.
    // COP-50: the guardian's state is kept for the startup summary.
    let guardian = if event_loop.ptt_guardian_eligible() {
        match ptt_guardian::PttGuardian::spawn(&config.radio) {
            Ok(guardian) => {
                let pid = guardian.pid().unwrap_or_default();
                tracing::info!(pid, "PTT guardian started");
                event_loop.set_ptt_guardian(guardian);
                startup_report::Guardian::Running { pid }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Could not start the PTT guardian; if coppad is killed while transmitting, \
                     PTT may stay keyed"
                );
                startup_report::Guardian::FailedToStart(format!("{e:#}"))
            }
        }
    } else {
        event_loop
            .ptt_startup()
            .guardian_not_started(config.radio.ptt_guardian)
    };

    let event_tx = event_loop.event_sender();

    // Wire audio ring buffers
    let (audio_out_producer, audio_out_consumer) =
        coppa_audio::audio_ring(config.audio.buffer_size);
    let (audio_in_producer, audio_in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    event_loop.set_audio_in(audio_in_consumer);

    // E5: Get shutdown flag for audio threads
    let shutdown_flag = event_loop.shutdown_flag();
    // COP-32: stop any audio worker already started if startup fails later.
    let _stop_audio = StopAudioOnExit(shutdown_flag.clone());

    // COP-32: bind host listeners on this path, before audio, so a port that
    // is already taken is a startup error instead of a spawned task's log.
    let vara_listeners = if config.host.vara_enabled {
        let cmd = bind_host_port(
            "VARA command port",
            &config.host.bind_address,
            config.host.vara_command_port,
            "vara_command_port",
        )
        .await?;
        let data = bind_host_port(
            "VARA data port",
            &config.host.bind_address,
            config.host.vara_data_port,
            "vara_data_port",
        )
        .await?;
        Some((cmd, data))
    } else {
        None
    };
    // COP-97: the ARDOP TNC host interface's two listeners, bound the same
    // way (data = command + 1, how every ARDOP client derives it).
    let ardop_listeners = if config.host.ardop_enabled {
        let cmd = bind_host_port(
            "ARDOP command port",
            &config.host.bind_address,
            config.host.ardop_command_port,
            "ardop_command_port",
        )
        .await?;
        let data = bind_host_port(
            "ARDOP data port",
            &config.host.bind_address,
            config.host.ardop_data_port,
            "ardop_data_port",
        )
        .await?;
        Some((cmd, data))
    } else {
        None
    };
    #[cfg(feature = "websocket")]
    let ws_listener = if config.host.websocket_enabled {
        Some(
            bind_host_port(
                "WebSocket port",
                &config.host.bind_address,
                config.host.websocket_port,
                "websocket_port",
            )
            .await?,
        )
    } else {
        None
    };

    // COP-50: every host listener's state, for the startup summary.
    let vara_state = match &vara_listeners {
        Some((cmd, data)) => (listener_state(cmd)?, listener_state(data)?),
        None => (
            startup_report::Listener::Disabled,
            startup_report::Listener::Disabled,
        ),
    };
    #[cfg(feature = "ardop")]
    let ardop_state = match &ardop_listeners {
        Some((cmd, data)) => (listener_state(cmd)?, listener_state(data)?),
        None => (
            startup_report::Listener::Disabled,
            startup_report::Listener::Disabled,
        ),
    };
    // A build without the feature never gets here with `ardop_enabled`
    // (check_ardop_built refused it), so NotBuilt is unreachable; the arm
    // keeps the summary shape honest anyway.
    #[cfg(not(feature = "ardop"))]
    let ardop_state = (
        startup_report::Listener::NotBuilt,
        startup_report::Listener::NotBuilt,
    );
    #[cfg(feature = "websocket")]
    let ws_state = match &ws_listener {
        Some(listener) => listener_state(listener)?,
        None => startup_report::Listener::Disabled,
    };
    #[cfg(not(feature = "websocket"))]
    let ws_state = startup_report::Listener::NotBuilt;

    // Connect CPAL audio streams to ring buffers
    #[cfg(feature = "cpal-backend")]
    let audio_devices = {
        use coppa_audio::AudioSource;

        let engine_rate = coppa_engine::profiles::get_profile(&config.engine.profile)
            .expect("profile checked before audio startup")
            .sample_rate;
        let preferred_rate = config.audio.sample_rate;
        let buf_size = config.audio.buffer_size;

        // COP-32: a named device that is not there is a startup error, not a
        // silent fallback to the system default. Both names are checked
        // before either stream opens.
        resolve_input(&config.audio.input_device, &config.audio.input_device_id)?;
        resolve_output(&config.audio.output_device, &config.audio.output_device_id)?;

        // Spawn audio input: CPAL mic -> ring buffer -> event loop polls it.
        // The source is opened, read and dropped on this one worker thread
        // (CPAL streams are thread-affine); the worker reports the open
        // result before the daemon may call itself ready.
        let mut audio_in_prod = audio_in_producer;
        let input_shutdown = shutdown_flag.clone();
        let input_device_name = config.audio.input_device.clone();
        let input_device_id = config.audio.input_device_id.clone();
        let input_channel = config.audio.input_channel;
        let (input_opened_tx, input_opened_rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let mut source = match open_input(
                &input_device_name,
                &input_device_id,
                engine_rate,
                preferred_rate,
                buf_size,
                input_channel,
            ) {
                Ok(source) => {
                    let _ = input_opened_tx.send(Ok((
                        source.device_name().to_string(),
                        source.hardware_rate(),
                    )));
                    source
                }
                Err(e) => {
                    let _ = input_opened_tx.send(Err(e));
                    return;
                }
            };
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
        let (input_name, input_hw_rate) = input_opened_rx
            .await
            .context("audio input worker stopped before opening its device")??;
        tracing::info!(device = ?input_name, hardware_rate = input_hw_rate, engine_rate, "Audio input started");

        // Spawn audio output: event loop writes to ring buffer -> CPAL speaker.
        // The sink is created, started, stopped and dropped on this one
        // worker thread (CPAL streams are thread-affine), and the worker
        // retains each chunk until the sink has accepted all of it, feeding
        // playback progress back so PTT is released from samples played.
        let output_shutdown = shutdown_flag.clone();
        let output_device_name = config.audio.output_device.clone();
        let output_device_id = config.audio.output_device_id.clone();
        let output_channel = config.audio.output_channel;
        let output_feedback = std::sync::Arc::new(audio_output::OutputFeedback::default());
        // Only wire the audio-out ring when this worker drains it: TX audio
        // is retained until the ring accepts it, so an undrained ring would
        // accumulate every transmission.
        event_loop.set_audio_out(audio_out_producer);
        event_loop.set_output_feedback(output_feedback.clone());
        let (output_opened_tx, output_opened_rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let sink = match open_output(
                &output_device_name,
                &output_device_id,
                engine_rate,
                preferred_rate,
                buf_size,
                output_channel,
            ) {
                Ok(sink) => {
                    let _ = output_opened_tx
                        .send(Ok((sink.device_name().to_string(), sink.hardware_rate())));
                    sink
                }
                Err(e) => {
                    output_feedback.fail();
                    let _ = output_opened_tx.send(Err(e));
                    return;
                }
            };
            audio_output::OutputWorker::new(1024, engine_rate, output_feedback).run(
                audio_out_consumer,
                sink,
                |sink| sink.playback_progress(),
                output_shutdown,
            );
        });
        let (output_name, output_hw_rate) = output_opened_rx
            .await
            .context("audio output worker stopped before opening its device")??;
        tracing::info!(device = ?output_name, hardware_rate = output_hw_rate, engine_rate, "Audio output started");
        (
            input_name,
            output_name,
            input_hw_rate,
            output_hw_rate,
            engine_rate,
        )
    };

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

    // Start the host servers (COP-46: the wiring lives in `vara_wiring` and
    // `ardop_wiring` so the integration tests drive the same code).
    //
    // COP-97 (P4): one response channel and one `forward_data_out` drainer
    // feed every enabled data-carrying surface, so `set_response_tx` is
    // registered exactly once per daemon however many surfaces are on.
    let mut data_out_senders: Vec<event_loop::VaraDataSenders> = Vec::new();
    let host_surfaces_on = vara_listeners.is_some() || ardop_listeners.is_some();
    let (response_tx, response_rx) = tokio::sync::mpsc::unbounded_channel();
    if host_surfaces_on {
        event_loop.set_response_tx(response_tx);
    }
    if let Some((cmd_listener, data_listener)) = vara_listeners {
        data_out_senders.push(vara_wiring::serve_vara(
            &mut event_loop,
            cmd_listener,
            data_listener,
            &config.host,
        ));
    }
    #[cfg(feature = "ardop")]
    if let Some((cmd_listener, data_listener)) = ardop_listeners {
        data_out_senders.push(ardop_wiring::serve_ardop(
            &mut event_loop,
            cmd_listener,
            data_listener,
            &config.host,
        ));
    }
    if host_surfaces_on {
        // The queue is unbounded, so it needs exactly one drainer.
        tokio::spawn(event_loop::forward_data_out(response_rx, data_out_senders));
    } else {
        drop(response_rx);
    }

    // Start WebSocket server if enabled
    #[cfg(feature = "websocket")]
    if let Some(ws_listener) = ws_listener {
        tracing::info!(
            addr = %vara_wiring::listen_addr(&ws_listener),
            allowed_origins = config.host.websocket_allowed_origins.len(),
            "WebSocket server listening"
        );
        // COP-53: every handshake must present the token and, from a browser,
        // an allowlisted Origin. check_websocket_auth already refused a missing
        // token; an empty one here would make the server refuse everything.
        let auth = coppa_host::websocket::WsAuth::new(
            config
                .host
                .websocket_token
                .as_ref()
                .map_or("", config::SecretToken::expose),
            &config.host.websocket_allowed_origins,
        );
        let mut ws_server = coppa_host::websocket::WebSocketServer::with_bind_addr(
            config.host.websocket_port,
            config.host.bind_address.clone(),
        )
        .with_auth(auth);

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

        // Wire separate telemetry and reliable data/state paths.
        event_loop.set_ws_broadcast(ws_server.broadcast_sender());
        event_loop.set_ws_push(ws_server.push_handle());
        ws_log::WsLogLayer::global().set_sender(ws_server.broadcast_sender());
        // Wire the live status snapshot so `status` replies carry real values
        // (decision 8, Phase 3 Task 7).
        event_loop.set_ws_status(ws_server.status());

        tokio::spawn(async move {
            if let Err(e) = ws_server.run_with_listener(ws_listener).await {
                tracing::error!("WebSocket server stopped: {e:#}");
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

    // COP-50: state what startup actually did, then ready or degraded.
    tracing::info!(
        bind_address = %config.host.bind_address,
        vara_command = %vara_state.0,
        vara_data = %vara_state.1,
        ardop_command = %ardop_state.0,
        ardop_data = %ardop_state.1,
        websocket = %ws_state,
        "Host listeners"
    );
    if startup_report::exposed([
        &vara_state.0,
        &vara_state.1,
        &ardop_state.0,
        &ardop_state.1,
        &ws_state,
    ]) {
        tracing::warn!(
            bind_address = %config.host.bind_address,
            "Host listeners accept connections from other hosts; anyone who can reach them \
             can key the transmitter. Use bind_address = \"127.0.0.1\" unless this network \
             is trusted and firewalled"
        );
    }
    #[cfg(feature = "cpal-backend")]
    tracing::info!(
        input = ?audio_devices.0,
        output = ?audio_devices.1,
        input_hardware_rate = audio_devices.2,
        output_hardware_rate = audio_devices.3,
        engine_rate = audio_devices.4,
        "Audio devices"
    );
    #[cfg(not(feature = "cpal-backend"))]
    tracing::warn!(
        "Audio devices: none; this coppad was built without the cpal-backend feature, so it \
         can neither receive nor transmit"
    );
    let ptt = event_loop.ptt_startup();
    if ptt.problem().is_some() {
        tracing::warn!(
            method = ?ptt.method(),
            active = %ptt.active(),
            state = ?ptt.state(),
            guardian = ?guardian.to_string(),
            "PTT"
        );
    } else {
        tracing::info!(
            method = ?ptt.method(),
            active = %ptt.active(),
            state = ?ptt.state(),
            guardian = ?guardian.to_string(),
            "PTT"
        );
    }

    // COP-32: "Daemon ready" only when nothing degraded the startup.
    let problems = event_loop.startup_problems();
    if problems.is_empty() {
        tracing::info!("Daemon ready");
    } else {
        tracing::error!(problems = %problems.join("; "), "Daemon running degraded");
    }

    // Run the event loop
    event_loop.run().await?;

    tracing::info!("Daemon stopped");
    Ok(())
}

/// COP-32: a build without `cpal-backend` opens no audio device, so a named
/// one can never be found.
#[cfg(not(feature = "cpal-backend"))]
fn check_no_selected_devices(audio: &config::AudioConfig) -> Result<()> {
    for (key, name) in [
        ("input_device", &audio.input_device),
        ("output_device", &audio.output_device),
        ("input_device_id", &audio.input_device_id),
        ("output_device_id", &audio.output_device_id),
    ] {
        if !name.is_empty() {
            anyhow::bail!(
                "[audio] {key} = {name:?} is set, but this coppad was built without the \
                 cpal-backend feature and opens no audio devices; rebuild with \
                 --features cpal-backend, or clear [audio] {key}"
            );
        }
    }
    Ok(())
}

/// COP-32: bind one host listener on `run_daemon`'s own path, so a taken
/// port fails startup.
async fn bind_host_port(
    what: &str,
    bind_address: &str,
    port: u16,
    key: &str,
) -> Result<tokio::net::TcpListener> {
    let addr = format!("{bind_address}:{port}");
    tokio::net::TcpListener::bind(&addr).await.with_context(|| {
        format!("failed to bind the {what} to {addr} ([host] bind_address, {key})")
    })
}

/// COP-50: a bound listener's state, with the address the OS assigned.
fn listener_state(listener: &tokio::net::TcpListener) -> Result<startup_report::Listener> {
    let addr = listener
        .local_addr()
        .context("failed to read a bound host listener's address")?;
    Ok(startup_report::Listener::Listening(addr))
}

/// COP-32: sets the audio workers' shutdown flag when `run_daemon` returns,
/// including on a startup error after a worker has started.
struct StopAudioOnExit(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for StopAudioOnExit {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(feature = "cpal-backend")]
#[derive(Clone, Copy)]
enum AudioDirection {
    Input,
    Output,
}

#[cfg(feature = "cpal-backend")]
impl AudioDirection {
    fn label(self) -> &'static str {
        match self {
            AudioDirection::Input => "input",
            AudioDirection::Output => "output",
        }
    }
}

/// Resolve before startup and again on the worker thread to catch unplugging.
#[cfg(feature = "cpal-backend")]
fn selection_error(direction: AudioDirection, id: &str, error: anyhow::Error) -> anyhow::Error {
    let key = if id.is_empty() { "device" } else { "device_id" };
    match error.downcast_ref::<coppa_audio::DeviceSelectionError>() {
        Some(coppa_audio::DeviceSelectionError::MissingName { name, .. }) => anyhow::anyhow!(
            "audio {} device {name:?} not found ([audio] {}_{}); run `coppa devices` to list available names and IDs",
            direction.label(), direction.label(), key
        ),
        _ => anyhow::anyhow!(
            "audio {} device selection failed ([audio] {}_{}): {error:#}",
            direction.label(), direction.label(), key
        ),
    }
}

#[cfg(feature = "cpal-backend")]
fn resolve_input(name: &str, id: &str) -> Result<()> {
    let selected_name = (!name.is_empty()).then_some(name);
    let selected_id = (!id.is_empty()).then_some(id);
    coppa_audio::resolve_input_device(selected_name, selected_id)
        .map(|_| ())
        .map_err(|e| selection_error(AudioDirection::Input, id, e))
}

#[cfg(feature = "cpal-backend")]
fn resolve_output(name: &str, id: &str) -> Result<()> {
    let selected_name = (!name.is_empty()).then_some(name);
    let selected_id = (!id.is_empty()).then_some(id);
    coppa_audio::resolve_output_device(selected_name, selected_id)
        .map(|_| ())
        .map_err(|e| selection_error(AudioDirection::Output, id, e))
}

#[cfg(feature = "cpal-backend")]
fn device_label(direction: AudioDirection, name: &str, id: &str) -> String {
    if !id.is_empty() {
        format!("{} device ID {id:?}", direction.label())
    } else if name.is_empty() {
        format!("the default {} device", direction.label())
    } else {
        format!("{} device {name:?}", direction.label())
    }
}

/// Open and start the capture stream on the calling (worker) thread.
#[cfg(feature = "cpal-backend")]
fn open_input(
    name: &str,
    id: &str,
    engine_rate: u32,
    preferred_rate: u32,
    buf_size: usize,
    channel: u16,
) -> Result<coppa_audio::LiveSource> {
    use coppa_audio::AudioSource;
    let opened = coppa_audio::resolve_input_device(
        (!name.is_empty()).then_some(name),
        (!id.is_empty()).then_some(id),
    )
    .map_err(|e| selection_error(AudioDirection::Input, id, e))
    .and_then(|device| {
        coppa_audio::LiveSource::new(device, engine_rate, preferred_rate, buf_size, channel)
    });
    opened
        .and_then(|mut source| source.start().map(|()| source))
        .with_context(|| {
            format!(
                "failed to open {} on input channel {channel} (preferred hardware rate {preferred_rate} Hz, engine rate {engine_rate} Hz)",
                device_label(AudioDirection::Input, name, id)
            )
        })
}

/// Open and start the playback stream on the calling (worker) thread.
#[cfg(feature = "cpal-backend")]
fn open_output(
    name: &str,
    id: &str,
    engine_rate: u32,
    preferred_rate: u32,
    buf_size: usize,
    channel: u16,
) -> Result<coppa_audio::LiveSink> {
    use coppa_audio::AudioSink;
    let opened = coppa_audio::resolve_output_device(
        (!name.is_empty()).then_some(name),
        (!id.is_empty()).then_some(id),
    )
    .map_err(|e| selection_error(AudioDirection::Output, id, e))
    .and_then(|device| {
        coppa_audio::LiveSink::new(device, engine_rate, preferred_rate, buf_size, channel)
    });
    opened
        .and_then(|mut sink| sink.start().map(|()| sink))
        .with_context(|| {
            format!(
                "failed to open {} on output channel {channel} (preferred hardware rate {preferred_rate} Hz, engine rate {engine_rate} Hz)",
                device_label(AudioDirection::Output, name, id)
            )
        })
}

#[cfg(feature = "websocket")]
mod ws_log;
