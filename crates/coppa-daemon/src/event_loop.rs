//! Main event loop for the Coppa daemon.
//!
//! Uses `tokio::select!` to multiplex audio, host, and radio events.

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_engine::CoppaCore;
use coppa_host::vara::VaraResponse;
use coppa_host::HostEvent;
use coppa_ml::BusyGate;
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::session::{LinkCapabilities, SessionManager, SessionState};
use coppa_protocol::transport::{TransportPdu, TransportType};
use coppa_radio::{NullPtt, PttControl, PttState};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};

use crate::config::DaemonConfig;

/// Map of connected VARA command-port clients' response senders
/// (`VaraServer::response_senders()`), used to broadcast `VaraResponse` telemetry.
type VaraResponseSenders = Arc<Mutex<HashMap<u32, mpsc::Sender<VaraResponse>>>>;

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
    /// Optional shared live-status snapshot for the WebSocket `status` reply
    /// (decision 8: "WebSocket `status` reply carries real values"). Updated
    /// alongside VARA telemetry whenever a frame decodes.
    #[cfg(feature = "websocket")]
    ws_status: Option<Arc<Mutex<coppa_host::websocket::WsStatus>>>,
    /// Optional map of connected VARA command-port clients' response senders, for
    /// broadcasting `VaraResponse` telemetry (SNR/PTT/BUFFER/BUSY — decision 8).
    /// Wired by `main.rs` from `VaraServer::response_senders()`.
    vara_responses: Option<VaraResponseSenders>,
    /// Outbound raw payload bytes queued for encode+transmit (the primary
    /// raw/ARQ `HostEvent::DataReceived` TX path). `VaraResponse::Buffer` telemetry
    /// reports this queue's length on every push/pop.
    tx_queue: VecDeque<Vec<u8>>,
    /// Spectral-occupancy busy gate (decision 8: `BUSY ON`/`OFF` telemetry), fed
    /// raw incoming audio in `handle_audio_in`.
    busy_gate: BusyGate,
    /// Session manager for connected-mode operation.
    session_mgr: SessionManager,
    /// Local station callsign (parsed from config).
    local_callsign: Option<Callsign>,
    /// Whether we are listening for incoming connections.
    listening: bool,
    /// Whether the link is currently mid-transmission (PTT asserted). Gates
    /// `try_drain_tx_queue` so only one frame transmits at a time; set/cleared in
    /// `handle_ptt_change`.
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

        let busy_gate = BusyGate::new(config.audio.sample_rate as f32);

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
            #[cfg(feature = "websocket")]
            ws_status: None,
            vara_responses: None,
            tx_queue: VecDeque::new(),
            busy_gate,
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

    /// Set the shared live-status snapshot the WebSocket server's `status` reply
    /// reads from (`WebSocketServer::status()`). See decision 8.
    #[cfg(feature = "websocket")]
    pub fn set_ws_status(&mut self, status: Arc<Mutex<coppa_host::websocket::WsStatus>>) {
        self.ws_status = Some(status);
    }

    /// Set the map of connected VARA command-port clients' response senders, for
    /// broadcasting `VaraResponse` telemetry (`VaraServer::response_senders()`).
    /// See decision 8.
    pub fn set_vara_responses(&mut self, senders: VaraResponseSenders) {
        self.vara_responses = Some(senders);
    }

    /// Broadcast one `VaraResponse` to every connected VARA command-port client, if
    /// any are wired up (`set_vara_responses`). A no-op (silently) if telemetry
    /// hasn't been wired, or if a given client's channel happens to be full/closed
    /// — telemetry is best-effort and must never block or fail the caller.
    async fn emit_vara(&self, response: VaraResponse) {
        if let Some(ref senders) = self.vara_responses {
            let senders = senders.lock().await;
            for tx in senders.values() {
                let _ = tx.try_send(response.clone());
            }
        }
    }

    /// Push one raw payload onto the outbound TX queue, emit the resulting
    /// `BUFFER` telemetry, and attempt to start transmitting immediately if the
    /// link is currently idle. See `tx_queue`'s field doc and `try_drain_tx_queue`.
    async fn enqueue_tx(&mut self, data: Vec<u8>) {
        self.tx_queue.push_back(data);
        self.emit_vara(VaraResponse::Buffer(self.tx_queue.len()))
            .await;
        self.try_drain_tx_queue().await;
    }

    /// If the link isn't currently mid-transmission, pop the next queued payload
    /// (if any), emit the resulting `BUFFER` count, and encode+transmit it. Called
    /// after every enqueue and after every PTT release, so the queue drains one
    /// frame at a time as each transmission completes.
    async fn try_drain_tx_queue(&mut self) {
        if self.is_transmitting {
            return;
        }
        if let Some(data) = self.tx_queue.pop_front() {
            self.emit_vara(VaraResponse::Buffer(self.tx_queue.len()))
                .await;
            match self.engine.encode_bytes(&data) {
                // Boxed: `transmit_samples` -> `handle_ptt_change` -> (on PTT
                // release) `try_drain_tx_queue` forms a 3-way async call cycle;
                // one edge needs indirection to give the compiler a finite-sized
                // future.
                Ok(samples) => Box::pin(self.transmit_samples(&samples)).await,
                Err(e) => tracing::warn!(error = %e, "Encode failed for queued TX frame"),
            }
        }
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
                            self.handle_ptt_change(tx).await;
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

                // Queue for encode+transmit (Task 7: BUFFER telemetry tracks this
                // queue's depth; see `enqueue_tx`/`try_drain_tx_queue`).
                self.enqueue_tx(tx_bytes).await;
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
        // Spectral-occupancy busy gate (decision 8): fed every incoming audio
        // block, regardless of whether it ends up containing a decodable frame.
        // Only emits telemetry on an actual BUSY ON/OFF transition.
        if let Some(new_state) = self.busy_gate.observe(samples) {
            self.emit_vara(VaraResponse::Busy(new_state)).await;
        }

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
            let snr_db = frame.snr_db;
            #[cfg(feature = "websocket")]
            let cfo_hz = frame.cfo_hz;
            #[cfg(feature = "websocket")]
            let speed_level = frame.speed_level;
            match frame.message {
                Ok(message) => {
                    // Telemetry: SNR (decision 8) after every decoded frame,
                    // regardless of what the frame's payload turns out to be —
                    // `DecodedFrame::snr_db` is known as soon as decode succeeds.
                    self.emit_vara(VaraResponse::Snr(snr_db.round() as i32))
                        .await;

                    // WebSocket `status` reply: keep the live snapshot current
                    // (decision 8: "connected, snr, level, cfo").
                    #[cfg(feature = "websocket")]
                    if let Some(ref status) = self.ws_status {
                        let mut snap = status.lock().await;
                        snap.connected = true;
                        snap.snr = Some(snr_db.round() as i32);
                        snap.level = Some(speed_level);
                        snap.cfo = Some(cfo_hz);
                    }

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
                    // Closed-loop rate adaptation (Phase 3 Task 4) lives in
                    // `coppa_ml::RateLoop` now, not the old `coppa-engine::RateController`
                    // this call site used to feed (deleted: it only ever logged a debug
                    // line, its `current_mcs()`/`rate_controller_mut()` had no other
                    // caller, so nothing downstream regresses). Wiring `RateLoop` into this
                    // daemon requires two pieces this event loop doesn't have yet: (a) the
                    // daemon never constructs/sends an ACK PDU at all (`arq_tx.process_ack`
                    // only *consumes* incoming ACKs; there's no `TransportPdu::
                    // new_ack_with_rate` call site to attach a recommendation to), and (b)
                    // `CoppaTransceiver::receive`'s new recommended-level return
                    // (`recommend_speed_level` over this frame's noise vars) isn't
                    // threaded up through `StreamingReceiver`/`StreamFrame` yet. Both are
                    // real daemon features, not a one-line swap — left for the daemon/ARQ
                    // wiring work this phase's plan explicitly defers. See
                    // `crates/coppa-ml/src/rate_loop.rs` and the validation bench
                    // `crates/coppa-bench/examples/closed_loop_arq.rs` for the
                    // already-working, ARQ-agnostic controller and its acceptance numbers.
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
        self.handle_ptt_change(true).await;
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

    async fn handle_ptt_change(&mut self, tx: bool) {
        let state = if tx { PttState::Tx } else { PttState::Rx };
        if let Err(e) = self.ptt.set_ptt(state) {
            tracing::warn!(error = %e, "PTT control error");
        }
        tracing::info!(state = if tx { "TX" } else { "RX" }, "PTT state change");
        // Telemetry: VaraResponse::Ptt at the same moment physical PTT changes
        // (decision 8), not a separately-timed emission.
        self.emit_vara(VaraResponse::Ptt(tx)).await;
        if tx {
            self.is_transmitting = true;
        } else {
            self.is_transmitting = false;
            // PTT just released: continue draining the TX queue if more is queued.
            self.try_drain_tx_queue().await;
        }
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

    // ── Task 7: live SNR/PTT/BUFFER/BUSY telemetry on the VARA port ──────────
    //
    // These are the "host-level integration tests with a mock client" the Task 7
    // brief calls for: a real `coppa_host::vara::VaraServer` command port, a raw
    // `TcpStream` standing in for a VARA client, wired to `EventLoop` exactly the
    // way `main.rs` wires it (`set_vara_responses`), reading back the literal wire
    // strings `VaraResponse::format()` produces.

    mod telemetry {
        use super::*;
        use coppa_host::vara::VaraServer;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
        use tokio::net::TcpStream;

        /// Config tuned so a real `transmit_samples` call's scheduled PTT-release
        /// delay resolves in ~0ms: a very high nominal sample rate makes
        /// `audio_duration_ms`/`drain_ms` truncate to 0 (integer ms math), and the
        /// pre/tail delays are zeroed. This only affects the *scheduling* math in
        /// `transmit_samples` — the engine's own internal encode rate is fixed at
        /// 48 kHz regardless (see `CLAUDE.md`), so encoding real payloads is
        /// unaffected.
        fn fast_ptt_config() -> DaemonConfig {
            let mut config = DaemonConfig::default();
            config.audio.sample_rate = 100_000_000;
            config.audio.buffer_size = 0;
            config.radio.ptt_pre_delay_ms = 0;
            config.radio.ptt_tail_delay_ms = 0;
            config
        }

        /// Spin up a real `VaraServer` on the given (distinct per test, to avoid
        /// cross-test port collisions) command/data ports, wire its response
        /// senders into `event_loop` (mirroring `main.rs`'s
        /// `set_vara_responses(vara_server.response_senders())`), start the
        /// server, connect a raw `TcpStream` "mock client" to the command port,
        /// and consume its initial `VERSION ...` greeting. Returns a line reader
        /// over the client's read half, plus its write half — callers MUST hold
        /// onto the write half for the test's duration (even unused): dropping it
        /// shuts down the client's write direction, which the server's command
        /// handler reads as EOF and reacts to by tearing down (and no longer
        /// writing) the *whole* connection, well before any later telemetry
        /// response arrives.
        async fn connect_mock_vara_client(
            event_loop: &mut EventLoop,
            cmd_port: u16,
            data_port: u16,
        ) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
            let server = VaraServer::new(cmd_port, data_port);
            event_loop.set_vara_responses(server.response_senders());

            tokio::spawn(async move {
                let _ = server.run().await;
            });
            // Give the server a moment to bind before connecting.
            tokio::time::sleep(Duration::from_millis(50)).await;

            let stream = TcpStream::connect(("127.0.0.1", cmd_port))
                .await
                .expect("mock client should connect to the VARA command port");
            let (read_half, write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let mut greeting = String::new();
            reader
                .read_line(&mut greeting)
                .await
                .expect("should read the initial VERSION greeting");
            assert!(
                greeting.starts_with("VERSION"),
                "expected a VERSION greeting first, got: {}",
                greeting
            );

            (reader, write_half)
        }

        /// Read one `\r\n`-terminated line, timing out (rather than hanging
        /// forever) if the client never receives one.
        async fn read_line(reader: &mut BufReader<OwnedReadHalf>) -> String {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
                .await
                .expect("timed out waiting for a VARA response line")
                .expect("reading a VARA response line should not error");
            assert!(n > 0, "connection closed before a response line arrived");
            line
        }

        /// Drain every line currently available within a short per-line window,
        /// stopping (not erroring) once nothing more arrives — used where the
        /// exact response count is a secondary detail and the assertions care
        /// about relative order/content of a subset (e.g. only the `BUSY` lines).
        async fn read_available_lines(reader: &mut BufReader<OwnedReadHalf>) -> Vec<String> {
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                match tokio::time::timeout(Duration::from_millis(300), reader.read_line(&mut line))
                    .await
                {
                    Ok(Ok(n)) if n > 0 => lines.push(line),
                    _ => break,
                }
            }
            lines
        }

        /// Zero-lead/trail-padded encode, mirroring `coppa-engine`'s own streaming
        /// tests: `StreamingReceiver`'s `SyncDetector` wants a clean silence
        /// bootstrap before the preamble, and the RX bandpass filter's group delay
        /// needs a little trailing pad so `push_samples` doesn't see end-of-input
        /// before the (filtered-domain) frame is fully buffered.
        fn with_lead_and_trail(samples: &[f32]) -> Vec<f32> {
            let mut out = vec![0.0f32; 8192];
            out.extend_from_slice(samples);
            out.extend(std::iter::repeat_n(0.0f32, 2048));
            out
        }

        /// Required scenario: "decoded frame -> SNR line arrives."
        #[tokio::test]
        async fn test_snr_telemetry_emitted_on_decoded_frame() {
            let config = DaemonConfig::default();
            let mut event_loop = EventLoop::new(config);
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19400, 19401).await;

            let core = coppa_engine::CoppaCore::new();
            let samples = core
                .encode("Hello telemetry")
                .expect("encode should succeed");
            let samples = with_lead_and_trail(&samples);

            event_loop.handle_audio_in(&samples).await;

            let line = read_line(&mut reader).await;
            assert!(
                line.starts_with("SNR "),
                "expected an SNR line after a decoded frame, got: {}",
                line
            );
        }

        /// Required scenario: "transmit -> PTT ON/OFF bracket."
        ///
        /// `EventLoop` isn't `Send` (its engine holds trait objects/raw-pointer
        /// ring-buffer internals that aren't), so `run()` can't go through a plain
        /// `tokio::spawn`. A `LocalSet` + `spawn_local` runs it concurrently with
        /// the rest of this test on the same thread instead, with no `Send` bound.
        #[tokio::test]
        async fn test_ptt_telemetry_brackets_transmission() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config);
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19410, 19411).await;
            let tx = event_loop.event_sender();

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    tx.send(DaemonEvent::Host(HostEvent::DataReceived {
                        client_id: 1,
                        data: b"Hello".to_vec(),
                    }))
                    .await
                    .unwrap();

                    // A real TX cycle also emits BUFFER telemetry (enqueue then
                    // drain) — read enough lines to be sure both PTT lines have
                    // arrived, then check their relative order among whatever
                    // else showed up.
                    let mut lines = Vec::new();
                    for _ in 0..8 {
                        lines.push(read_line(&mut reader).await);
                        let ptt_so_far: Vec<&str> = lines
                            .iter()
                            .map(|s| s.trim_end())
                            .filter(|s| s.starts_with("PTT"))
                            .collect();
                        if ptt_so_far == ["PTT ON", "PTT OFF"] {
                            return; // bracket observed in order — test passes
                        }
                    }
                    panic!(
                        "expected a PTT ON ... PTT OFF bracket within the first 8 lines, got: {:?}",
                        lines
                    );
                })
                .await;
        }

        /// Required scenario: "queue 3 frames -> BUFFER 3…0 progression."
        ///
        /// Forces `is_transmitting = true` before enqueuing so all three frames
        /// stack up in the queue instead of draining immediately (simulating a
        /// client bursting frames faster than the half-duplex link can send them),
        /// then drains deterministically (bypassing the real scheduled-PTT-release
        /// timer, which `test_ptt_telemetry_brackets_transmission` already covers)
        /// by calling the same internal hooks `handle_ptt_change(false)` would
        /// trigger on each real transmission's completion.
        #[tokio::test]
        async fn test_buffer_telemetry_progression_3_to_0() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config);
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19420, 19421).await;

            event_loop.is_transmitting = true;
            event_loop.enqueue_tx(b"frame1".to_vec()).await;
            event_loop.enqueue_tx(b"frame2".to_vec()).await;
            event_loop.enqueue_tx(b"frame3".to_vec()).await;
            assert_eq!(event_loop.tx_queue.len(), 3);

            event_loop.is_transmitting = false;
            event_loop.try_drain_tx_queue().await; // starts draining frame1 -> len 2
            event_loop.handle_ptt_change(false).await; // frame1 "done" -> drains frame2 -> len 1
            event_loop.handle_ptt_change(false).await; // frame2 "done" -> drains frame3 -> len 0
            event_loop.handle_ptt_change(false).await; // frame3 "done" -> queue empty, no more drains
            assert_eq!(event_loop.tx_queue.len(), 0);

            let lines = read_available_lines(&mut reader).await;
            let buffer_values: Vec<&str> = lines
                .iter()
                .map(|s| s.trim_end())
                .filter(|s| s.starts_with("BUFFER"))
                .collect();
            assert_eq!(
                buffer_values,
                vec!["BUFFER 1", "BUFFER 2", "BUFFER 3", "BUFFER 2", "BUFFER 1", "BUFFER 0"],
                "expected the queue to build 1,2,3 then drain 2,1,0"
            );
        }

        /// Required scenario: "injected band-limited noise burst -> BUSY ON then
        /// OFF."
        #[tokio::test]
        async fn test_busy_telemetry_on_then_off_from_noise_burst() {
            let config = DaemonConfig::default();
            let mut event_loop = EventLoop::new(config);
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19430, 19431).await;

            // Deterministic PRNG (no external `rand` dependency needed here).
            let mut seed: u32 = 12345;
            let mut next = move || {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                seed
            };
            let mut noise_block = |amplitude: f32| -> Vec<f32> {
                (0..1024)
                    .map(|_| amplitude * ((next() >> 8) as f32 / (1u32 << 24) as f32 - 0.5))
                    .collect()
            };

            // Settle the busy gate's noise floor on quiet blocks first.
            for _ in 0..10 {
                event_loop.handle_audio_in(&noise_block(0.01)).await;
            }
            // Inject a band-limited noise burst well above the settled floor.
            for _ in 0..5 {
                event_loop.handle_audio_in(&noise_block(0.5)).await;
            }
            // Burst ends; back to quiet.
            for _ in 0..10 {
                event_loop.handle_audio_in(&noise_block(0.01)).await;
            }

            let lines = read_available_lines(&mut reader).await;
            let busy_values: Vec<&str> = lines
                .iter()
                .map(|s| s.trim_end())
                .filter(|s| s.starts_with("BUSY"))
                .collect();
            let on_idx = busy_values.iter().position(|&s| s == "BUSY ON");
            let off_idx = busy_values.iter().position(|&s| s == "BUSY OFF");
            assert!(
                on_idx.is_some() && off_idx.is_some() && on_idx.unwrap() < off_idx.unwrap(),
                "expected a BUSY ON before a BUSY OFF, got: {:?}",
                busy_values
            );
        }
    }
}
