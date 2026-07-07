//! Main event loop for the Coppa daemon.
//!
//! Uses `tokio::select!` to multiplex audio, host, and radio events.

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_engine::CoppaCore;
use coppa_host::HostEvent;
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::session::{LinkCapabilities, SessionManager, SessionState};
use coppa_protocol::transport::{TransportPdu, TransportType};
use coppa_radio::{NullPtt, PttControl, PttState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::config::DaemonConfig;

/// Event types flowing through the daemon event loop.
#[derive(Debug)]
#[allow(dead_code)] // AudioIn/AudioOut wired in future AFSK TX/RX path
pub enum DaemonEvent {
    /// Event from a host interface.
    Host(HostEvent),
    /// Audio samples received from input.
    AudioIn(Vec<f32>),
    /// Request to transmit audio samples.
    AudioOut(Vec<f32>),
    /// PTT state change.
    PttChange(bool),
    /// Shutdown signal received.
    Shutdown,
}

/// The main daemon event loop.
pub struct EventLoop {
    config: DaemonConfig,
    engine: CoppaCore,
    event_rx: mpsc::Receiver<DaemonEvent>,
    event_tx: mpsc::Sender<DaemonEvent>,
    running: bool,
    /// Optional audio output ring buffer producer for writing TX samples.
    audio_out: Option<AudioRingProducer>,
    /// Optional audio input ring buffer consumer for reading RX samples.
    audio_in: Option<AudioRingConsumer>,
    /// PTT controller (defaults to NullPtt if unconfigured).
    ptt: Box<dyn PttControl>,
    /// Optional sender for host responses (decoded data, status updates).
    response_tx: Option<mpsc::Sender<coppa_host::HostResponse>>,
    /// Counter for audio output ring buffer overflow (dropped samples).
    audio_out_overflow_count: u64,
    /// Last-seen value of the audio input ring's overflow counter (see
    /// `AudioRingConsumer::overflow_count`); `poll_audio_input` compares against
    /// this each poll and logs a warning when it grows (silent RX sample loss was
    /// a Phase-0-era finding).
    audio_in_overflow_count: u64,
    /// Shutdown flag shared with audio threads for clean shutdown (E5).
    shutdown_flag: Arc<AtomicBool>,
    /// ARQ transmitter state (active when arq_enabled is true).
    arq_tx: Option<ArqTx>,
    /// ARQ receiver state (active when arq_enabled is true).
    arq_rx: Option<ArqRx>,
    /// Current ARQ session ID.
    arq_session_id: u8,
    /// Next TX sequence number for transport PDUs.
    #[allow(dead_code)] // used when ARQ TX path sends segmented frames
    arq_next_seq: u8,
    /// Optional WebSocket broadcast sender for forwarding decoded data.
    #[cfg(feature = "websocket")]
    ws_broadcast: Option<tokio::sync::broadcast::Sender<String>>,
    /// Session manager for connected-mode operation.
    session_mgr: SessionManager,
    /// Local station callsign (parsed from config).
    local_callsign: Option<Callsign>,
    /// Whether we are listening for incoming connections.
    listening: bool,
    /// Whether we are currently in a TX turn.
    #[allow(dead_code)] // enforcement deferred to real-world testing
    is_transmitting: bool,
    /// Number of data frames sent in the current TX turn.
    tx_frame_count: usize,
    /// Maximum data frames per TX turn before yielding.
    max_frames_per_turn: usize,
    /// Turnaround delay in ms between RX/TX switching.
    #[allow(dead_code)] // enforcement deferred to real-world testing
    turnaround_ms: u64,
}

impl EventLoop {
    /// Create a new event loop with the given configuration.
    pub fn new(config: DaemonConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let ptt = Self::create_ptt(&config);

        // Create engine from profile; all profiles use 48kHz internally
        let engine =
            if let Some(profile) = coppa_engine::profiles::get_profile(&config.engine.profile) {
                CoppaCore::from_profile(profile)
            } else {
                CoppaCore::with_config(coppa_engine::EngineConfig::default())
            };

        // E6: Initialize ARQ state if enabled
        let (arq_tx, arq_rx) = if config.engine.arq_enabled {
            let arq_config = ArqConfig::default();
            (Some(ArqTx::new(arq_config)), Some(ArqRx::new(8)))
        } else {
            (None, None)
        };

        let local_callsign = if config.engine.callsign.is_empty() {
            None
        } else {
            Callsign::new(&config.engine.callsign).ok()
        };

        Self {
            config,
            engine,
            event_rx,
            event_tx,
            running: false,
            audio_out: None,
            audio_in: None,
            ptt,
            response_tx: None,
            audio_out_overflow_count: 0,
            audio_in_overflow_count: 0,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            arq_tx,
            arq_rx,
            arq_session_id: 0,
            arq_next_seq: 0,
            #[cfg(feature = "websocket")]
            ws_broadcast: None,
            session_mgr: SessionManager::new(),
            local_callsign,
            listening: false,
            is_transmitting: false,
            tx_frame_count: 0,
            max_frames_per_turn: 4,
            turnaround_ms: 500,
        }
    }

    /// Create the appropriate PTT controller based on config.
    fn create_ptt(config: &DaemonConfig) -> Box<dyn PttControl> {
        match config.radio.ptt_method.as_str() {
            "vox" => Box::new(coppa_radio::VoxPtt::new()),
            "rigctld" => {
                match coppa_radio::rigctld::RigctldClient::connect(&config.radio.rigctld_address) {
                    Ok(client) => Box::new(client),
                    Err(e) => {
                        tracing::warn!(
                            address = %config.radio.rigctld_address,
                            error = %e,
                            "Failed to connect to rigctld; falling back to no PTT"
                        );
                        Box::new(NullPtt::new())
                    }
                }
            }
            // E4: Serial and GPIO PTT are not yet implemented
            "serial" => {
                tracing::warn!(
                    "PTT method 'serial' is not yet implemented; falling back to no PTT"
                );
                Box::new(NullPtt::new())
            }
            "gpio" => {
                tracing::warn!("PTT method 'gpio' is not yet implemented; falling back to no PTT");
                Box::new(NullPtt::new())
            }
            _ => Box::new(NullPtt::new()),
        }
    }

    /// Get a sender for injecting events into the loop.
    pub fn event_sender(&self) -> mpsc::Sender<DaemonEvent> {
        self.event_tx.clone()
    }

    /// Set the audio output ring buffer for TX sample playback.
    pub fn set_audio_out(&mut self, producer: AudioRingProducer) {
        self.audio_out = Some(producer);
    }

    /// Set the audio input ring buffer for RX sample capture.
    pub fn set_audio_in(&mut self, consumer: AudioRingConsumer) {
        self.audio_in = Some(consumer);
    }

    /// Set the response sender for sending decoded data back to host clients.
    pub fn set_response_tx(&mut self, tx: mpsc::Sender<coppa_host::HostResponse>) {
        self.response_tx = Some(tx);
    }

    /// Set the WebSocket broadcast sender for forwarding decoded data to WS clients.
    #[cfg(feature = "websocket")]
    pub fn set_ws_broadcast(&mut self, tx: tokio::sync::broadcast::Sender<String>) {
        self.ws_broadcast = Some(tx);
    }

    /// Get a clone of the shutdown flag for use in audio threads (E5).
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown_flag.clone()
    }

    /// Run the event loop until shutdown.
    ///
    /// Polls the event channel and optionally reads audio input samples
    /// from the ring buffer on a periodic interval.
    pub async fn run(&mut self) -> Result<()> {
        self.running = true;
        tracing::info!(profile = %self.config.engine.profile, "Event loop started");

        let mut audio_poll = tokio::time::interval(tokio::time::Duration::from_millis(20));
        // E6: ARQ retransmit check interval (500ms)
        let mut retransmit_poll = tokio::time::interval(tokio::time::Duration::from_millis(500));
        // Session cleanup and keepalive interval (5s)
        let mut session_cleanup = tokio::time::interval(tokio::time::Duration::from_secs(5));

        while self.running {
            tokio::select! {
                event = self.event_rx.recv() => {
                    match event {
                        Some(DaemonEvent::Shutdown) | None => {
                            tracing::info!("Shutdown signal received");
                            // E5: Signal audio threads to stop
                            self.shutdown_flag.store(true, Ordering::Release);
                            self.running = false;
                        }
                        Some(DaemonEvent::Host(host_event)) => {
                            self.handle_host_event(host_event).await;
                        }
                        Some(DaemonEvent::AudioIn(samples)) => {
                            self.handle_audio_in(&samples).await;
                        }
                        Some(DaemonEvent::AudioOut(samples)) => {
                            self.handle_audio_out(&samples);
                        }
                        Some(DaemonEvent::PttChange(tx)) => {
                            self.handle_ptt_change(tx);
                        }
                    }
                }
                _ = audio_poll.tick() => {
                    self.poll_audio_input().await;
                }
                _ = retransmit_poll.tick() => {
                    // E6: Check for ARQ retransmits
                    self.check_arq_retransmits();
                }
                _ = session_cleanup.tick() => {
                    let removed = self.session_mgr.cleanup_timed_out();
                    for id in removed {
                        tracing::warn!(session_id = id, "Session timed out");
                        if let Some(ref tx) = self.response_tx {
                            let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                                client_id: 0,
                                status: "DISCONNECTED".to_string(),
                            });
                        }
                    }
                    // Send keepalives for active sessions
                    let active = self.session_mgr.active_sessions();
                    for id in active {
                        let needs = self.session_mgr.get(id).map(|s| s.needs_keepalive()).unwrap_or(false);
                        if needs {
                            if let Some(session) = self.session_mgr.get_mut(id) {
                                if let Ok(ka_pdu) = session.keepalive() {
                                    let ka_bytes = ka_pdu.to_bytes();
                                    if let Ok(samples) = self.engine.encode_bytes(&ka_bytes) {
                                        self.handle_audio_out(&samples);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Poll the audio input ring buffer for new samples.
    async fn poll_audio_input(&mut self) {
        let mut chunk = None;
        if let Some(ref mut consumer) = self.audio_in {
            let available = consumer.available();
            if available > 0 {
                let mut buf = vec![0.0f32; available];
                let read = consumer.read(&mut buf);
                if read > 0 {
                    buf.truncate(read);
                    chunk = Some(buf);
                }
            }

            // Check the input ring's overflow counter each poll and log a warning
            // when it grows — silent RX sample loss was a Phase-0-era finding.
            let overflow = consumer.overflow_count();
            if overflow > self.audio_in_overflow_count {
                tracing::warn!(
                    dropped = overflow - self.audio_in_overflow_count,
                    cumulative_dropped = overflow,
                    "Audio input buffer overflow"
                );
                self.audio_in_overflow_count = overflow;
            }
        }
        if let Some(buf) = chunk {
            self.handle_audio_in(&buf).await;
        }
    }

    async fn handle_host_event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Connected { client_id } => {
                tracing::info!(client_id, "Client connected");
            }
            HostEvent::Disconnected { client_id } => {
                tracing::info!(client_id, "Client disconnected");
            }
            HostEvent::DataReceived { client_id, data } => {
                tracing::debug!(client_id, bytes = data.len(), "Data received from client");

                // If there's an established session, wrap data in a MAC PDU
                let session_info = self.session_mgr.active_sessions().iter().find_map(|&id| {
                    self.session_mgr
                        .get(id)
                        .filter(|s| s.is_established())
                        .map(|s| (s.remote.clone(), s.ssid))
                });

                if let Some((remote, ssid)) = session_info {
                    if let Some(ref local) = self.local_callsign {
                        let mac_pdu = MacPdu::new_data(remote, local.clone(), ssid, data.clone());
                        let pdu_bytes = mac_pdu.to_bytes();
                        match self.engine.encode_bytes(&pdu_bytes) {
                            Ok(samples) => {
                                self.transmit_samples(&samples).await;
                                self.tx_frame_count += 1;
                                tracing::debug!(
                                    frame = self.tx_frame_count,
                                    max = self.max_frames_per_turn,
                                    "Session data frame transmitted"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(client_id, error = %e, "Failed to encode session data")
                            }
                        }
                        return;
                    }
                }

                // Fall through: no session — use raw/ARQ encode path
                // E6: If ARQ is enabled, wrap data in a TransportPdu before encoding
                let tx_bytes = if self.config.engine.arq_enabled {
                    if let Some(ref mut arq_tx) = self.arq_tx {
                        let now = Instant::now();
                        match arq_tx.send(data.clone(), now) {
                            Ok(seq) => {
                                let pdu = TransportPdu::new_reliable(
                                    self.arq_session_id,
                                    seq,
                                    0, // ack_num filled by ARQ layer
                                    data.clone(),
                                );
                                pdu.to_bytes()
                            }
                            Err(e) => {
                                tracing::warn!(client_id, error = %e, "ARQ window full; dropping TX");
                                return;
                            }
                        }
                    } else {
                        data.clone()
                    }
                } else {
                    data.clone()
                };

                // Encode binary data via the engine's byte-level API
                match self.engine.encode_bytes(&tx_bytes) {
                    Ok(samples) => {
                        self.transmit_samples(&samples).await;
                    }
                    Err(e) => {
                        tracing::warn!(client_id, error = %e, "Encode failed");
                    }
                }
            }
            HostEvent::VaraCommand { client_id, command } => {
                tracing::debug!(client_id, command = %command, "VARA command received");
                let cmd = command.trim().to_uppercase();
                if cmd == "LISTEN ON" {
                    self.listening = true;
                    tracing::info!("Listening for incoming connections");
                } else if cmd == "LISTEN OFF" {
                    self.listening = false;
                    tracing::info!("Stopped listening for incoming connections");
                }
            }
            HostEvent::ConnectRequest {
                client_id,
                source: _,
                destination,
            } => {
                tracing::info!(client_id, destination = %destination, "Connect request");

                let local = match self.local_callsign {
                    Some(ref cs) => cs.clone(),
                    None => {
                        tracing::warn!("Connect request but no local callsign configured");
                        if let Some(ref tx) = self.response_tx {
                            let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                                client_id,
                                status: "DISCONNECTED".to_string(),
                            });
                        }
                        return;
                    }
                };

                let remote = match Callsign::new(&destination) {
                    Ok(cs) => cs,
                    Err(e) => {
                        tracing::warn!(error = %e, "Invalid destination callsign");
                        if let Some(ref tx) = self.response_tx {
                            let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                                client_id,
                                status: "DISCONNECTED".to_string(),
                            });
                        }
                        return;
                    }
                };

                let caps = LinkCapabilities::default();
                match self.session_mgr.create(local, remote.clone(), 0, caps) {
                    Ok(id) => {
                        if let Some(session) = self.session_mgr.get_mut(id) {
                            match session.initiate() {
                                Ok(req_pdu) => {
                                    let pdu_bytes = req_pdu.to_bytes();
                                    match self.engine.encode_bytes(&pdu_bytes) {
                                        Ok(samples) => self.transmit_samples(&samples).await,
                                        Err(e) => {
                                            tracing::warn!(error = %e, "Failed to encode CONNECT_REQ")
                                        }
                                    }
                                    if let Some(ref tx) = self.response_tx {
                                        let _ =
                                            tx.try_send(coppa_host::HostResponse::StatusUpdate {
                                                client_id,
                                                status: format!("CONNECTING {}", remote),
                                            });
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to initiate session");
                                    self.session_mgr.remove(id);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to create session");
                        if let Some(ref tx) = self.response_tx {
                            let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                                client_id,
                                status: "DISCONNECTED".to_string(),
                            });
                        }
                    }
                }
            }
            HostEvent::DisconnectRequest { client_id } => {
                tracing::info!(client_id, "Disconnect request");

                // Find first active established session
                let session_id = self
                    .session_mgr
                    .active_sessions()
                    .iter()
                    .find(|&&id| {
                        self.session_mgr
                            .get(id)
                            .map(|s| s.state != SessionState::Idle)
                            .unwrap_or(false)
                    })
                    .copied();

                if let Some(id) = session_id {
                    if let Some(session) = self.session_mgr.get_mut(id) {
                        match session.disconnect() {
                            Ok(disc_pdu) => {
                                let pdu_bytes = disc_pdu.to_bytes();
                                match self.engine.encode_bytes(&pdu_bytes) {
                                    Ok(samples) => self.transmit_samples(&samples).await,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Failed to encode DISCONNECT")
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to disconnect session");
                            }
                        }
                    }
                    self.session_mgr.remove(id);
                    if let Some(ref tx) = self.response_tx {
                        let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                            client_id,
                            status: "DISCONNECTED".to_string(),
                        });
                    }
                } else {
                    tracing::debug!("Disconnect request but no active session");
                }
            }
        }
    }

    async fn handle_audio_in(&mut self, samples: &[f32]) {
        // `CoppaCore::push_samples` owns all the buffering/sync/frame-boundary
        // bookkeeping the old DECODE_WINDOW/SLIDE_STEP/MAX_STREAM_BUFFER block used
        // to do by hand here; this just dispatches whatever frames complete as a
        // result of this chunk.
        //
        // Whichever call completes a candidate runs the full demod/FEC pass
        // synchronously (see `StreamingReceiver::push_samples`'s doc) — since we're
        // called with no `spawn_blocking`, that stalls this async event loop for
        // the frame's decode time (~tens of ms). Accepted for now: input audio is
        // buffered in `audio_in`'s ring during the stall, and its overflow counter
        // (`poll_audio_input`) would surface it if that ring ever actually
        // overflowed. Moving the decode to a worker thread would be the fix if this
        // ever becomes a real problem.
        for frame in self.engine.push_samples(samples) {
            match frame.message {
                Ok(message) => {
                    let decoded_bytes = message.as_bytes();

                    // Try to parse as MAC PDU for session handling
                    if let Ok(mac_pdu) = MacPdu::from_bytes(decoded_bytes) {
                        self.handle_mac_pdu(mac_pdu).await;
                        continue;
                    }

                    // E6: If ARQ enabled, parse decoded bytes as TransportPdu
                    let output_data = if self.config.engine.arq_enabled {
                        match TransportPdu::from_bytes(decoded_bytes) {
                            Ok(pdu) => {
                                match pdu.transport_type {
                                    TransportType::Reliable | TransportType::Unreliable => {
                                        // Feed to ARQ receiver
                                        if let Some(ref mut arq_rx) = self.arq_rx {
                                            let delivered =
                                                arq_rx.receive(pdu.seq_num, pdu.payload.clone());
                                            // Process ACK info back to our TX side
                                            if let Some(ref mut arq_tx) = self.arq_tx {
                                                arq_tx.process_ack(
                                                    pdu.ack_num,
                                                    pdu.ack_bitmap,
                                                    Instant::now(),
                                                );
                                            }
                                            // Collect all delivered payloads
                                            let mut all_data = Vec::new();
                                            for (_seq, data) in delivered {
                                                all_data.extend(data);
                                            }
                                            all_data
                                        } else {
                                            pdu.payload
                                        }
                                    }
                                    TransportType::Ack | TransportType::Nak => {
                                        // Pure ACK/NAK: process and don't forward to host
                                        if let Some(ref mut arq_tx) = self.arq_tx {
                                            arq_tx.process_ack(
                                                pdu.ack_num,
                                                pdu.ack_bitmap,
                                                Instant::now(),
                                            );
                                        }
                                        Vec::new()
                                    }
                                    TransportType::Reset => {
                                        // Reset ARQ state
                                        self.arq_tx = Some(ArqTx::new(ArqConfig::default()));
                                        self.arq_rx = Some(ArqRx::new(8));
                                        Vec::new()
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, "Failed to parse TransportPdu; forwarding raw bytes");
                                decoded_bytes.to_vec()
                            }
                        }
                    } else {
                        decoded_bytes.to_vec()
                    };

                    if !output_data.is_empty() {
                        tracing::info!(bytes = output_data.len(), "Frame decoded successfully");
                        // Send decoded data back to host clients
                        if let Some(ref tx) = self.response_tx {
                            let response = coppa_host::HostResponse::DataOut {
                                client_id: 0, // broadcast to all clients
                                data: output_data.clone(),
                            };
                            if let Err(e) = tx.try_send(response) {
                                tracing::warn!(error = %e, "Failed to send decoded response to host");
                            }
                        }
                        // Forward to WebSocket broadcast channel
                        #[cfg(feature = "websocket")]
                        if let Some(ref ws_tx) = self.ws_broadcast {
                            let text = String::from_utf8_lossy(&output_data).into_owned();
                            let _ = ws_tx.send(text);
                        }
                    }
                    // Feed the real per-carrier-noise SNR estimate (`StreamFrame::
                    // snr_db`) to the rate controller — replaces the crude
                    // whole-buffer RMS proxy (`20*log10(rms) + 40`) used before
                    // Task 7, which was a known hack (see the Task 7 report).
                    let new_mcs = self.engine.rate_controller_mut().update(frame.snr_db, true);
                    tracing::debug!(
                        snr_db = %format!("{:.1}", frame.snr_db),
                        mcs = new_mcs,
                        "Rate controller updated (decode success)"
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        frame_start = frame.frame_start,
                        "Streaming frame failed to decode"
                    );
                }
            }
        }
    }

    fn handle_audio_out(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        match self.audio_out {
            Some(ref mut producer) => {
                let written = producer.write(samples);
                if written < samples.len() {
                    // E1: Track cumulative overflow with a counter
                    let dropped = samples.len() - written;
                    self.audio_out_overflow_count += dropped as u64;
                    tracing::warn!(
                        dropped,
                        total = samples.len(),
                        cumulative_dropped = self.audio_out_overflow_count,
                        "Audio output buffer overflow"
                    );
                }
            }
            None => {
                tracing::debug!(samples = samples.len(), "Audio out: no output device wired");
            }
        }
    }

    /// Transmit encoded audio samples: assert PTT, write to ring buffer, schedule PTT release.
    async fn transmit_samples(&mut self, samples: &[f32]) {
        // Assert PTT before transmitting
        self.handle_ptt_change(true);
        // Enforce PTT pre-delay before writing audio
        let pre_delay_ms = self.config.radio.ptt_pre_delay_ms;
        if pre_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(pre_delay_ms)).await;
        }
        self.handle_audio_out(samples);
        // Schedule delayed PTT release based on audio duration + ring buffer drain time
        let sample_count = samples.len();
        let sample_rate = self.config.audio.sample_rate;
        let audio_duration_ms = (sample_count as u64 * 1000) / sample_rate as u64;
        let drain_ms =
            (self.config.audio.buffer_size as u64 * 1000) / self.config.audio.sample_rate as u64;
        let tail_delay_ms = self.config.radio.ptt_tail_delay_ms;
        let total_delay_ms = audio_duration_ms + drain_ms + tail_delay_ms;
        let max_tx_ms = self.config.radio.max_tx_duration_s * 1000;
        let capped_delay_ms = total_delay_ms.min(max_tx_ms);
        if total_delay_ms > max_tx_ms {
            tracing::warn!(
                tx_duration_ms = total_delay_ms,
                max_tx_ms,
                "TX duration exceeds max; capping PTT release delay"
            );
        }
        let total_delay = std::time::Duration::from_millis(capped_delay_ms);
        let ptt_event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(total_delay).await;
            if let Err(e) = ptt_event_tx.send(DaemonEvent::PttChange(false)).await {
                tracing::warn!(error = %e, "Failed to deassert PTT after TX");
            }
        });
    }

    /// E6: Check for ARQ retransmits and send them.
    fn check_arq_retransmits(&mut self) {
        if !self.config.engine.arq_enabled {
            return;
        }
        let now = Instant::now();
        // Collect retransmit data first to avoid borrow conflict
        let mut retransmit_pdus: Vec<Vec<u8>> = Vec::new();
        if let Some(ref mut arq_tx) = self.arq_tx {
            let retransmit_seqs = arq_tx.get_retransmits(now);
            for seq in retransmit_seqs {
                if let Some(data) = arq_tx.get_segment_data(seq) {
                    let pdu =
                        TransportPdu::new_reliable(self.arq_session_id, seq, 0, data.to_vec());
                    retransmit_pdus.push(pdu.to_bytes());
                }
            }
        }
        // Now encode and transmit (no more borrow on arq_tx)
        for pdu_bytes in retransmit_pdus {
            match self.engine.encode_bytes(&pdu_bytes) {
                Ok(samples) => {
                    self.handle_audio_out(&samples);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ARQ retransmit encode failed");
                }
            }
        }
    }

    // ── Session handling methods ──────────────────────────────────────

    async fn handle_mac_pdu(&mut self, pdu: MacPdu) {
        match pdu.frame_type {
            MacFrameType::ConnectReq => self.handle_incoming_connect(pdu).await,
            MacFrameType::ConnectAck => self.handle_connect_ack_rx(pdu).await,
            MacFrameType::ConnectCfm => self.handle_connect_cfm_rx(pdu),
            MacFrameType::Disconnect => self.handle_incoming_disconnect(pdu),
            MacFrameType::Data => self.handle_session_data(pdu),
            MacFrameType::Keepalive => self.handle_keepalive_rx(pdu),
            _ => {
                tracing::debug!(frame_type = ?pdu.frame_type, "Unhandled MAC frame type");
            }
        }
    }

    async fn handle_incoming_connect(&mut self, pdu: MacPdu) {
        if !self.listening {
            tracing::debug!("CONNECT_REQ received but not listening; ignoring");
            return;
        }

        let local = match self.local_callsign {
            Some(ref cs) => cs.clone(),
            None => {
                tracing::debug!("CONNECT_REQ received but no local callsign configured");
                return;
            }
        };

        // Check that this is addressed to us
        if !pdu.dest.as_str().is_empty() && pdu.dest != local {
            tracing::debug!(
                dest = %pdu.dest, local = %local,
                "CONNECT_REQ not addressed to us"
            );
            return;
        }

        let remote = pdu.src.clone();
        let caps = LinkCapabilities::default();
        match self
            .session_mgr
            .create(local, remote.clone(), pdu.ssid, caps)
        {
            Ok(id) => {
                if let Some(session) = self.session_mgr.get_mut(id) {
                    match session.handle_connect_req(&pdu.payload) {
                        Ok(ack_pdu) => {
                            let ack_bytes = ack_pdu.to_bytes();
                            match self.engine.encode_bytes(&ack_bytes) {
                                Ok(samples) => self.transmit_samples(&samples).await,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to encode CONNECT_ACK")
                                }
                            }
                            // Don't send CONNECTED yet — wait for CONNECT_CFM to complete handshake
                            tracing::info!(remote = %remote, "CONNECT_ACK sent, awaiting CFM");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to handle CONNECT_REQ");
                            self.session_mgr.remove(id);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create session for incoming connect");
            }
        }
    }

    async fn handle_connect_ack_rx(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        if let Some(session) = self.session_mgr.find_by_remote_mut(&remote) {
            match session.handle_connect_ack(&pdu.payload) {
                Ok(cfm_pdu) => {
                    let cfm_bytes = cfm_pdu.to_bytes();
                    match self.engine.encode_bytes(&cfm_bytes) {
                        Ok(samples) => self.transmit_samples(&samples).await,
                        Err(e) => tracing::warn!(error = %e, "Failed to encode CONNECT_CFM"),
                    }
                    if let Some(ref tx) = self.response_tx {
                        let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                            client_id: 0,
                            status: format!("CONNECTED {}", remote),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to handle CONNECT_ACK");
                }
            }
        } else {
            tracing::debug!(remote = %remote, "CONNECT_ACK from unknown remote");
        }
    }

    fn handle_connect_cfm_rx(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        if let Some(session) = self.session_mgr.find_by_remote_mut(&remote) {
            match session.handle_connect_cfm(&pdu.payload) {
                Ok(()) => {
                    session.confirm_established();
                    tracing::info!(remote = %remote, "Session fully established (responder)");
                    if let Some(ref tx) = self.response_tx {
                        let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                            client_id: 0,
                            status: format!("CONNECTED {}", remote),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to handle CONNECT_CFM");
                }
            }
        } else {
            tracing::debug!(remote = %remote, "CONNECT_CFM from unknown remote");
        }
    }

    fn handle_incoming_disconnect(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        // Find session ID first to avoid borrow issues
        let session_id = self.session_mgr.find_by_remote(&remote).map(|s| s.id);
        if let Some(id) = session_id {
            if let Some(session) = self.session_mgr.get_mut(id) {
                session.handle_disconnect();
            }
            self.session_mgr.remove(id);
            tracing::info!(remote = %remote, "Session disconnected by remote");
            if let Some(ref tx) = self.response_tx {
                let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate {
                    client_id: 0,
                    status: "DISCONNECTED".to_string(),
                });
            }
        } else {
            tracing::debug!(remote = %remote, "DISCONNECT from unknown remote");
        }
    }

    fn handle_session_data(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        if let Some(session) = self.session_mgr.find_by_remote_mut(&remote) {
            if session.is_established() {
                self.tx_frame_count = 0; // Our turn to transmit starts fresh
                session.touch();
                if let Some(ref tx) = self.response_tx {
                    let _ = tx.try_send(coppa_host::HostResponse::DataOut {
                        client_id: 0,
                        data: pdu.payload,
                    });
                }
            } else {
                tracing::debug!(
                    remote = %remote, state = ?session.state,
                    "Data received but session not established"
                );
            }
        } else {
            tracing::debug!(remote = %remote, "Data from unknown remote");
        }
    }

    fn handle_keepalive_rx(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        if let Some(session) = self.session_mgr.find_by_remote_mut(&remote) {
            session.touch();
        }
    }

    fn handle_ptt_change(&mut self, tx: bool) {
        let state = if tx { PttState::Tx } else { PttState::Rx };
        if let Err(e) = self.ptt.set_ptt(state) {
            tracing::warn!(error = %e, "PTT control error");
        }
        tracing::info!(state = if tx { "TX" } else { "RX" }, "PTT state change");
    }

    /// Check if the loop is running.
    #[allow(dead_code)] // lifecycle API for daemon supervisors
    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_loop_shutdown() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Send shutdown immediately
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(!event_loop.is_running());
    }

    #[tokio::test]
    async fn test_event_loop_host_event() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::Connected { client_id: 1 }))
            .await
            .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_loop_ptt_change() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::PttChange(true)).await.unwrap();
        tx.send(DaemonEvent::PttChange(false)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_audio_in_decode() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Send silence (won't decode, but should not crash)
        let silence = vec![0.0f32; 1000];
        tx.send(DaemonEvent::AudioIn(silence)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_audio_out_event() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        let samples = vec![0.5f32; 100];
        tx.send(DaemonEvent::AudioOut(samples)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_audio_out_with_ring_buffer() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Wire up a ring buffer
        let (producer, mut consumer) = coppa_audio::audio_ring(8192);
        event_loop.set_audio_out(producer);

        let samples = vec![1.0f32; 100];
        tx.send(DaemonEvent::AudioOut(samples)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();

        // Verify samples were written to the ring buffer
        let mut buf = vec![0.0f32; 200];
        let read = consumer.read(&mut buf);
        assert_eq!(read, 100);
        assert_eq!(buf[0], 1.0);
    }

    #[tokio::test]
    async fn test_audio_in_with_ring_buffer() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Wire up a ring buffer for input
        let (mut producer, consumer) = coppa_audio::audio_ring(8192);
        event_loop.set_audio_in(consumer);

        // Push silence into the ring buffer (will be polled by event loop)
        producer.write(&[0.0f32; 100]);

        // Send shutdown after one tick
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_host_event_data_received() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::DataReceived {
            client_id: 42,
            data: b"Hello".to_vec(),
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_events_sequence() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::Connected { client_id: 1 }))
            .await
            .unwrap();
        tx.send(DaemonEvent::PttChange(true)).await.unwrap();
        tx.send(DaemonEvent::AudioOut(vec![1.0; 50])).await.unwrap();
        tx.send(DaemonEvent::AudioIn(vec![0.0; 50])).await.unwrap();
        tx.send(DaemonEvent::PttChange(false)).await.unwrap();
        tx.send(DaemonEvent::Host(HostEvent::Disconnected { client_id: 1 }))
            .await
            .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_shutdown_leaves_not_running() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Send shutdown and verify the loop exits with running=false
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(!event_loop.is_running());
    }

    #[tokio::test]
    async fn test_ptt_uses_null_by_default() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // PTT change should not error with NullPtt
        tx.send(DaemonEvent::PttChange(true)).await.unwrap();
        tx.send(DaemonEvent::PttChange(false)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_connect_requires_callsign() {
        let config = DaemonConfig::default(); // callsign is empty
        let mut event_loop = EventLoop::new(config);
        let (resp_tx, mut resp_rx) = mpsc::channel(16);
        event_loop.set_response_tx(resp_tx);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::ConnectRequest {
            client_id: 1,
            source: String::new(),
            destination: "W1AW".to_string(),
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();

        // Should have sent DISCONNECTED status since no callsign
        let resp = resp_rx.try_recv().unwrap();
        match resp {
            coppa_host::HostResponse::StatusUpdate { status, .. } => {
                assert_eq!(status, "DISCONNECTED");
            }
            _ => panic!("Expected StatusUpdate"),
        }
    }

    #[tokio::test]
    async fn test_connect_with_callsign_creates_session() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".to_string();
        let mut event_loop = EventLoop::new(config);
        let (resp_tx, mut resp_rx) = mpsc::channel(16);
        event_loop.set_response_tx(resp_tx);
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::ConnectRequest {
            client_id: 1,
            source: "VK3ABC".to_string(),
            destination: "W1AW".to_string(),
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();

        // Should have sent CONNECTING status
        let resp = resp_rx.try_recv().unwrap();
        match resp {
            coppa_host::HostResponse::StatusUpdate { status, .. } => {
                assert!(
                    status.starts_with("CONNECTING"),
                    "Expected CONNECTING, got: {}",
                    status
                );
            }
            _ => panic!("Expected StatusUpdate"),
        }

        // Session should exist in Connecting state
        let active = event_loop.session_mgr.active_sessions();
        assert_eq!(active.len(), 1);
        let session = event_loop.session_mgr.get(active[0]).unwrap();
        assert_eq!(session.state, SessionState::Connecting);
    }

    #[tokio::test]
    async fn test_disconnect_without_session() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".to_string();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        // Should not panic when disconnecting with no active session
        tx.send(DaemonEvent::Host(HostEvent::DisconnectRequest {
            client_id: 1,
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_listen_on_off() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        let tx = event_loop.event_sender();

        assert!(!event_loop.listening);

        tx.send(DaemonEvent::Host(HostEvent::VaraCommand {
            client_id: 1,
            command: "LISTEN ON".to_string(),
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(event_loop.listening);
    }

    #[tokio::test]
    async fn test_listen_off() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config);
        event_loop.listening = true;
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::Host(HostEvent::VaraCommand {
            client_id: 1,
            command: "LISTEN OFF".to_string(),
        }))
        .await
        .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(!event_loop.listening);
    }
}
