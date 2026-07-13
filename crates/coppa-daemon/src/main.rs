//! Coppa Daemon (coppad) - long-running service for the Coppa digital communications system.
//!
//! Manages the engine lifecycle, host interfaces, and radio control in a
//! persistent background process.

mod config;
mod event_loop;
mod spectrum;

#[cfg(feature = "kiss-tnc")]
mod tnc;

use anyhow::Result;
use config::DaemonConfig;
use event_loop::{DaemonEvent, EventLoop};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging; RUST_LOG controls verbosity (default: info)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "coppad - Coppa Daemon starting"
    );

    // Check for --tnc flag early and branch to TNC daemon mode if present
    if std::env::args().any(|a| a == "--tnc") {
        #[cfg(feature = "kiss-tnc")]
        {
            let tnc_config = tnc::TncConfig::default();
            return tnc::run_tnc(tnc_config).await;
        }
        #[cfg(not(feature = "kiss-tnc"))]
        {
            anyhow::bail!("TNC mode requires the kiss-tnc feature");
        }
    }

    // Load configuration
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "coppad.toml".to_string());

    let config = DaemonConfig::load_or_default(&config_path)?;

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
    let event_tx = event_loop.event_sender();

    // Create response channel and wire it to the event loop
    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel(64);
    event_loop.set_response_tx(response_tx);

    // Wire audio ring buffers
    let (audio_out_producer, audio_out_consumer) =
        coppa_audio::audio_ring(config.audio.buffer_size);
    let (audio_in_producer, audio_in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    event_loop.set_audio_out(audio_out_producer);
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

        // Spawn audio output: event loop writes to ring buffer -> CPAL speaker
        let mut audio_out_cons = audio_out_consumer;
        let output_shutdown = shutdown_flag.clone();
        let output_device_name = config.audio.output_device.clone();
        let sink_result = if output_device_name.is_empty() {
            coppa_audio::CpalSink::new(sample_rate, buf_size)
        } else {
            match coppa_audio::find_output_device_by_name(&output_device_name) {
                Some(device) => {
                    tracing::info!(device = %output_device_name, "Audio output: using named device");
                    coppa_audio::cpal_backend::CpalSink::from_device(device, sample_rate, buf_size)
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
        match sink_result {
            Ok(mut sink) => {
                if let Err(e) = sink.start() {
                    eprintln!("Failed to start audio output: {}", e);
                } else {
                    tracing::info!(sample_rate, "Audio output started");
                    tokio::task::spawn_blocking(move || {
                        let mut buf = vec![0.0f32; 1024];
                        loop {
                            // E5: Check shutdown flag in addition to is_abandoned
                            if audio_out_cons.is_abandoned()
                                || output_shutdown.load(std::sync::atomic::Ordering::Acquire)
                            {
                                break;
                            }
                            let n = audio_out_cons.read(&mut buf);
                            if n > 0 {
                                let _ = sink.write(&buf[..n]);
                            } else {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    });
                }
            }
            Err(e) => eprintln!("No audio output device: {}", e),
        }
    }

    // Suppress unused variable warnings when cpal-backend is not enabled
    #[cfg(not(feature = "cpal-backend"))]
    {
        let _ = audio_in_producer;
        let _ = audio_out_consumer;
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
