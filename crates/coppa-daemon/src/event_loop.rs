//! Main event loop for the Coppa daemon.
//!
//! Uses `tokio::select!` to multiplex audio, host, and radio events.

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_engine::CoppaCore;
use coppa_host::vara::VaraResponse;
use coppa_host::HostEvent;
use coppa_ml::{BusyGate, RateLoop};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu, StationIdPayload};
use coppa_protocol::session::{LinkCapabilities, SessionManager, SessionState};
use coppa_protocol::transport::{TransportPdu, TransportType};
use coppa_radio::{NullPtt, PttControl, PttState};
use rand::RngExt;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    /// Sender-side closed-loop rate controller (Phase 3 Task 4's `coppa_ml::
    /// RateLoop`), updated from the peer's ACK-carried recommendation
    /// (`TransportPdu::suggested_rate`) and applied to `self.engine` via
    /// `CoppaCore::set_speed_level`.
    rate_loop: RateLoop,
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
    /// Time of the last station-ID frame actually sent (or `EventLoop`
    /// construction, if none yet). Compared against `[station_id]
    /// id_interval_secs` in `id_due` -- see `transmit_samples`'s doc for why
    /// an ID is only ever prepended to a real outgoing transmission, never
    /// sent standalone on a bare timer (Phase 4 Task 3).
    last_id_time: Instant,
    /// Time of the last standalone beacon frame actually sent (or
    /// `EventLoop` construction, if none yet). Compared against
    /// `[station_id] beacon_interval_secs` in `maybe_send_beacon` (Phase 4
    /// Task 3).
    last_beacon_time: Instant,
    /// Raw audio samples read from the input ring by
    /// `observe_busy_gate_from_audio_input` while `wait_for_clear_channel`
    /// was blocked, already fed to `busy_gate.observe` there but not yet
    /// decoded/dispatched. Flushed (decode-and-dispatch only, no repeat
    /// busy-gate observation) by the next `poll_audio_input` call from
    /// `run`'s main select loop -- see `observe_busy_gate_from_audio_input`'s
    /// doc (Finding 1 fix, Phase 4 Task 3 review). Empty outside of a busy
    /// wait; no data is lost, decode is just deferred.
    pending_busy_wait_audio: Vec<f32>,
    /// Dedicated FFT sensor for the `spectrum` WebSocket broadcast (Phase 4
    /// Task 4) -- separate from `busy_gate`'s own internal `SpectrumSensor`
    /// (smaller FFT, tuned for occupancy margin rather than bin resolution).
    /// Only meaningful (and only fed) when `ws_broadcast` is set; see
    /// `maybe_broadcast_spectrum`.
    #[cfg(feature = "websocket")]
    spectrum_sensor: coppa_ml::SpectrumSensor,
    /// Rolling window of the most recent
    /// `crate::spectrum::SPECTRUM_FFT_SIZE` raw RX samples, fed by every
    /// `handle_audio_in` call -- `maybe_broadcast_spectrum`'s FFT input.
    #[cfg(feature = "websocket")]
    spectrum_buffer: Vec<f32>,
    /// Wall-clock time of the last `spectrum` broadcast (or `EventLoop`
    /// construction, if none yet) -- gates `maybe_broadcast_spectrum` to
    /// `crate::spectrum::SPECTRUM_UPDATE_HZ` rather than computing/
    /// broadcasting one on every audio callback.
    #[cfg(feature = "websocket")]
    last_spectrum_broadcast: Instant,
}

impl EventLoop {
    /// Create a new event loop with the given configuration.
    ///
    /// Fails if `[radio] ptt_method` doesn't parse or names an
    /// unrecognized/unbuilt PTT backend -- see `create_ptt`.
    pub fn new(config: DaemonConfig) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(256);
        let ptt = Self::create_ptt(&config)?;

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
            match Callsign::new(&config.engine.callsign) {
                Ok(cs) => Some(cs),
                Err(e) => {
                    // A non-empty but unparseable callsign string leaves
                    // `local_callsign` at `None`, which silently disables the
                    // station-ID timer and beacon mode (both check
                    // `local_callsign.is_some()`, not the raw config string --
                    // see `id_due`/`maybe_send_beacon`). Warn once at startup
                    // so this doesn't look like the feature is "on" per config
                    // but never actually fires.
                    tracing::warn!(
                        callsign = %config.engine.callsign,
                        error = %e,
                        "Invalid [engine] callsign; station ID/beacon and \
                         connect handling will be unavailable"
                    );
                    None
                }
            }
        };

        let busy_gate = BusyGate::new(config.audio.sample_rate as f32);
        #[cfg(feature = "websocket")]
        let spectrum_sensor = coppa_ml::SpectrumSensor::new(
            crate::spectrum::SPECTRUM_FFT_SIZE,
            config.audio.sample_rate as f32,
        );

        Ok(Self {
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
            rate_loop: RateLoop::default_coppa(),
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
            last_id_time: Instant::now(),
            last_beacon_time: Instant::now(),
            pending_busy_wait_audio: Vec::new(),
            #[cfg(feature = "websocket")]
            spectrum_sensor,
            #[cfg(feature = "websocket")]
            spectrum_buffer: Vec::new(),
            #[cfg(feature = "websocket")]
            last_spectrum_broadcast: Instant::now(),
        })
    }

    /// Create the appropriate PTT controller based on config.
    ///
    /// Fails loudly (hard startup error) on an unrecognized/unimplemented
    /// `[radio] ptt_method` -- `NullPtt` is only reachable via an explicit
    /// `ptt_method = "none"` (or blank), never as a silent fallback for a
    /// typo'd or unbuilt backend. The one deliberate exception is
    /// `rigctld`'s *runtime connection* failure (address configured
    /// correctly, but nothing answering it right now): that already-existing
    /// behavior -- warn and fall back to `NullPtt` -- is unchanged, since
    /// it's a live/transient condition rather than an unrecognized config.
    fn create_ptt(config: &DaemonConfig) -> Result<Box<dyn PttControl>> {
        let parsed = config
            .radio
            .ptt_config()
            .map_err(|e| anyhow::anyhow!("invalid [radio] ptt_method: {e}"))?;

        match parsed {
            crate::config::PttConfig::None => Ok(Box::new(NullPtt::new())),
            crate::config::PttConfig::Vox => Ok(Box::new(coppa_radio::VoxPtt::new())),
            crate::config::PttConfig::Rigctld => {
                match coppa_radio::rigctld::RigctldClient::connect(&config.radio.rigctld_address) {
                    Ok(client) => Ok(Box::new(client)),
                    Err(e) => {
                        tracing::warn!(
                            address = %config.radio.rigctld_address,
                            error = %e,
                            "Failed to connect to rigctld; falling back to no PTT"
                        );
                        Ok(Box::new(NullPtt::new()))
                    }
                }
            }
            crate::config::PttConfig::Serial { port, line } => {
                #[cfg(feature = "serial-ptt")]
                {
                    let serial_line = match line {
                        crate::config::PttSerialLine::Dtr => {
                            coppa_radio::ptt_serial::SerialPttLine::Dtr
                        }
                        crate::config::PttSerialLine::Rts => {
                            coppa_radio::ptt_serial::SerialPttLine::Rts
                        }
                    };
                    let ptt = coppa_radio::ptt_serial::SerialPtt::open(&port, serial_line, false)
                        .map_err(|e| {
                        anyhow::anyhow!("failed to open serial PTT port {port}: {e}")
                    })?;
                    Ok(Box::new(ptt))
                }
                #[cfg(not(feature = "serial-ptt"))]
                {
                    let _ = (port, line);
                    Err(anyhow::anyhow!(
                        "PTT method 'serial' requires coppad to be built with \
                         --features serial-ptt"
                    ))
                }
            }
            crate::config::PttConfig::Gpio { pin } => {
                #[cfg(all(feature = "gpio-ptt", target_os = "linux"))]
                {
                    let pin_num: u32 = pin.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "invalid GPIO pin {pin:?}: expected a plain pin number, e.g. \"gpio:17\""
                        )
                    })?;
                    let ptt = coppa_radio::ptt_gpio::GpioPtt::open(pin_num, false)
                        .map_err(|e| anyhow::anyhow!("failed to open GPIO PTT pin {pin}: {e}"))?;
                    Ok(Box::new(ptt))
                }
                #[cfg(not(all(feature = "gpio-ptt", target_os = "linux")))]
                {
                    let _ = pin;
                    Err(anyhow::anyhow!(
                        "PTT method 'gpio' requires coppad to be built with \
                         --features gpio-ptt, on Linux"
                    ))
                }
            }
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
        // Beacon-mode check interval (Phase 4 Task 3): cheap no-op tick when
        // beacon mode is disabled (the default); see `maybe_send_beacon`.
        let mut beacon_poll = tokio::time::interval(tokio::time::Duration::from_secs(1));

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
                    self.check_arq_retransmits().await;
                }
                _ = beacon_poll.tick() => {
                    self.maybe_send_beacon().await;
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
                                        self.transmit_samples(&samples).await;
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

    /// Drain whatever samples are currently available on the audio input
    /// ring (non-blocking), if any, logging when the ring's own overflow
    /// counter grows (silent RX sample loss was a Phase-0-era finding).
    /// Shared by `poll_audio_input` (full decode+dispatch) and
    /// `observe_busy_gate_from_audio_input` (busy-gate-only, used while
    /// `wait_for_clear_channel` is blocked) so both read from the ring the
    /// same way.
    fn drain_audio_input_ring(&mut self) -> Option<Vec<f32>> {
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
        chunk
    }

    /// Poll the audio input ring buffer for new samples, and flush anything
    /// buffered by a prior busy-wait (see `pending_busy_wait_audio`) ahead of
    /// it. Only ever called from `run`'s main `tokio::select!` loop (the
    /// `audio_poll` tick) -- this is deliberate: it's the one place full MAC
    /// PDU decode/dispatch (which may itself call `transmit_samples`, e.g.
    /// for a CONNECT_ACK/CFM response) is allowed to run from. See
    /// `observe_busy_gate_from_audio_input`'s doc for the narrower method
    /// `wait_for_clear_channel` uses instead, and why.
    async fn poll_audio_input(&mut self) {
        // Decode+dispatch audio observed (for busy-gate purposes only) during
        // a prior busy-wait first, preserving temporal order against
        // whatever's freshly available on the ring below. Not re-fed to
        // `busy_gate.observe` here -- that already happened when it was read.
        if !self.pending_busy_wait_audio.is_empty() {
            let pending = std::mem::take(&mut self.pending_busy_wait_audio);
            self.decode_and_dispatch_audio(&pending).await;
        }
        if let Some(buf) = self.drain_audio_input_ring() {
            self.handle_audio_in(&buf).await;
        }
    }

    /// Read available audio input samples and feed them to
    /// `busy_gate.observe` only -- no frame decode, no `MacPdu` dispatch.
    ///
    /// Used exclusively by `wait_for_clear_channel`'s poll loop (Finding 1,
    /// Phase 4 Task 3 review) so real RX audio keeps updating busy-gate
    /// occupancy while that call is blocked waiting for the channel to
    /// clear, without also running `handle_audio_in`'s full protocol
    /// dispatch on this call stack. That dispatch (frame decode ->
    /// `handle_mac_pdu` -> e.g. `handle_incoming_connect` /
    /// `handle_connect_ack_rx`, both of which call `transmit_samples`
    /// directly) used to run here via a boxed recursive call into
    /// `poll_audio_input`; that was a real reentrancy hazard -- a
    /// CONNECT_REQ/CONNECT_ACK decoded mid-wait could run a second, nested
    /// PTT-key/write-audio/schedule-release cycle interleaved with the
    /// already-in-flight *outer* `transmit_samples` call, before
    /// `is_transmitting` is even set (it's only set once the outer call
    /// reaches `handle_ptt_change` *after* this wait returns, so
    /// `try_drain_tx_queue`'s guard can't catch it either).
    ///
    /// Samples read here are saved into `pending_busy_wait_audio` rather than
    /// dropped: `poll_audio_input` decodes and dispatches them for real, via
    /// the normal path, the next time it runs from `run`'s main select loop.
    /// No incoming traffic is lost -- decode is just deferred until it's safe
    /// to run full dispatch again.
    async fn observe_busy_gate_from_audio_input(&mut self) {
        if let Some(buf) = self.drain_audio_input_ring() {
            if let Some(new_state) = self.busy_gate.observe(&buf) {
                self.emit_vara(VaraResponse::Busy(new_state)).await;
            }
            self.pending_busy_wait_audio.extend_from_slice(&buf);
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
                } else if cmd == "TUNE" || cmd.starts_with("TUNE ") {
                    // Task 1 (Phase 4): TX level calibration. `TUNE` (or `TUNE
                    // <seconds>`) generates the standard SSB two-tone
                    // calibration signal and sends it through the same
                    // PTT-key/stream/PTT-unkey path real frames use
                    // (`transmit_samples`), so an operator can set their
                    // radio's audio drive level via ALC exactly as they would
                    // for real traffic.
                    let seconds = cmd
                        .strip_prefix("TUNE ")
                        .and_then(|s| s.trim().parse::<f32>().ok())
                        .filter(|s| *s > 0.0)
                        .unwrap_or(10.0);
                    tracing::info!(seconds, "TUNE: transmitting TX-level calibration tone");
                    let samples = self.engine.tune_tone(seconds, None);
                    self.transmit_samples(&samples).await;
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
        #[cfg(feature = "websocket")]
        self.maybe_broadcast_spectrum(samples);
        self.decode_and_dispatch_audio(samples).await;
    }

    /// Waterfall spectrum production (Phase 4 Task 4): accumulate `samples`
    /// into a rolling `crate::spectrum::SPECTRUM_FFT_SIZE`-sample window and,
    /// no more often than `crate::spectrum::SPECTRUM_UPDATE_HZ`, compute and
    /// broadcast a `spectrum` WebSocket message over `ws_broadcast` (the same
    /// existing conduit the "data" broadcast already uses -- see
    /// `set_ws_broadcast`'s doc; per-client opt-in filtering happens on the
    /// `coppa-host::websocket` side, not here).
    ///
    /// A no-op whenever `ws_broadcast` isn't set (no host attached) -- this
    /// only ever runs with a real audio-in consumer wired up (`set_audio_in`),
    /// so the FFT cost of a disconnected/headless daemon is never paid; once a
    /// host IS attached, this computes/serializes a spectrum on every
    /// `SPECTRUM_UPDATE_HZ` tick regardless of whether any currently-connected
    /// client has actually opted in (the daemon has no visibility into that
    /// per-connection state) -- cheap enough (one FFT of `SPECTRUM_FFT_SIZE`
    /// samples at 4 Hz) not to bother threading that visibility through.
    #[cfg(feature = "websocket")]
    fn maybe_broadcast_spectrum(&mut self, samples: &[f32]) {
        let Some(ref ws_tx) = self.ws_broadcast else {
            return;
        };

        self.spectrum_buffer.extend_from_slice(samples);
        if self.spectrum_buffer.len() > crate::spectrum::SPECTRUM_FFT_SIZE {
            let excess = self.spectrum_buffer.len() - crate::spectrum::SPECTRUM_FFT_SIZE;
            self.spectrum_buffer.drain(0..excess);
        }
        if self.spectrum_buffer.len() < crate::spectrum::SPECTRUM_FFT_SIZE {
            return; // not enough audio yet for a full-resolution spectrum
        }

        let period = Duration::from_secs_f64(1.0 / crate::spectrum::SPECTRUM_UPDATE_HZ);
        if self.last_spectrum_broadcast.elapsed() < period {
            return;
        }
        self.last_spectrum_broadcast = Instant::now();

        let bins = crate::spectrum::compute_spectrum_bins(
            &self.spectrum_sensor,
            &self.spectrum_buffer,
            self.config.audio.sample_rate as f32,
        );
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let msg = coppa_host::websocket::WsServerMessage::Spectrum { bins, timestamp_ms };
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = ws_tx.send(json);
        }
    }

    /// Decode whatever frames complete as a result of `samples` and dispatch
    /// each one (MAC PDU handling, ARQ, host forwarding, WebSocket
    /// broadcast). Split out from `handle_audio_in` (Finding 1, Phase 4 Task
    /// 3 review) so `poll_audio_input` can run this on audio that was merely
    /// *observed* by the busy gate during a `wait_for_clear_channel` busy
    /// wait (see `observe_busy_gate_from_audio_input`) without re-feeding
    /// that same audio into `busy_gate.observe` a second time.
    ///
    /// `CoppaCore::push_samples` owns all the buffering/sync/frame-boundary
    /// bookkeeping the old DECODE_WINDOW/SLIDE_STEP/MAX_STREAM_BUFFER block used
    /// to do by hand here; this just dispatches whatever frames complete as a
    /// result of this chunk.
    ///
    /// Whichever call completes a candidate runs the full demod/FEC pass
    /// synchronously (see `StreamingReceiver::push_samples`'s doc) — since we're
    /// called with no `spawn_blocking`, that stalls this async event loop for
    /// the frame's decode time (~tens of ms). Accepted for now: input audio is
    /// buffered in `audio_in`'s ring during the stall, and its overflow counter
    /// (`poll_audio_input`) would surface it if that ring ever actually
    /// overflowed. Moving the decode to a worker thread would be the fix if this
    /// ever becomes a real problem.
    async fn decode_and_dispatch_audio(&mut self, samples: &[f32]) {
        for frame in self.engine.push_samples(samples) {
            let snr_db = frame.snr_db;
            let recommended_level = frame.recommended_level;
            #[cfg(feature = "websocket")]
            let cfo_hz = frame.cfo_hz;
            #[cfg(feature = "websocket")]
            let speed_level = frame.speed_level;
            match frame.payload {
                Ok(payload) => {
                    // Telemetry: SNR (decision 8) after every decoded frame,
                    // regardless of what the frame's payload turns out to be —
                    // `DecodedFrame::snr_db` is known as soon as decode succeeds.
                    self.emit_vara(VaraResponse::Snr(snr_db.round() as i32))
                        .await;

                    // WebSocket `status` reply: keep the live snapshot current
                    // (decision 8: "connected, snr, level, cfo").
                    //
                    // Review finding: `connected` must NOT be "was any frame ever
                    // decoded since daemon start" (that flips true once and stays
                    // true forever, even after the session drops or the remote goes
                    // silent -- a monitoring client would misread a dead link as
                    // live). Recomputed from `session_mgr`'s real established-session
                    // state instead, at the same point the rest of the snapshot
                    // updates. Still only refreshed on a decode event (this whole
                    // snapshot has no independent tick), so a session that drops
                    // WITHOUT any further decode won't flip this back to false until
                    // the next decoded frame -- an accepted, smaller residual gap,
                    // not the same "stays true forever, unconditionally" bug.
                    #[cfg(feature = "websocket")]
                    if let Some(ref status) = self.ws_status {
                        let established = self.session_mgr.active_sessions().iter().any(|&id| {
                            self.session_mgr.get(id).is_some_and(|s| s.is_established())
                        });
                        let mut snap = status.lock().await;
                        snap.connected = established;
                        snap.snr = Some(snr_db.round() as i32);
                        snap.level = Some(speed_level);
                        snap.cfo = Some(cfo_hz);
                    }

                    let decoded_bytes = payload.as_slice();

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
                                        let (result_data, ack_info) =
                                            if let Some(ref mut arq_rx) = self.arq_rx {
                                                let delivered = arq_rx
                                                    .receive(pdu.seq_num, pdu.payload.clone());
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
                                                (all_data, Some(arq_rx.ack_info()))
                                            } else {
                                                (pdu.payload, None)
                                            };
                                        // Acknowledge every successfully-processed
                                        // incoming data PDU (one ACK per frame, per
                                        // decision 4 -- batching was considered and
                                        // not chosen). Mirrors the RECEIVED pdu's own
                                        // session_id back rather than sourcing it
                                        // from either of this daemon's own two
                                        // (mutually inconsistent) session-id fields.
                                        if let Some((ack_num, ack_bitmap)) = ack_info {
                                            let ack_pdu = TransportPdu::new_ack_with_rate(
                                                pdu.session_id,
                                                ack_num,
                                                ack_bitmap,
                                                recommended_level,
                                            );
                                            match self.engine.encode_bytes(&ack_pdu.to_bytes()) {
                                                Ok(ack_samples) => {
                                                    self.transmit_samples(&ack_samples).await;
                                                }
                                                Err(e) => {
                                                    tracing::warn!(error = %e, "Failed to encode outgoing ACK");
                                                }
                                            }
                                        }
                                        result_data
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
                                        // Closed-loop rate adaptation: apply the
                                        // peer's recommendation (if this ACK carries
                                        // one) and push the resulting level into the
                                        // encoder for subsequent outgoing frames.
                                        if let Some(rate) = pdu.suggested_rate() {
                                            self.rate_loop.on_ack(rate, true);
                                            if let Err(e) = self
                                                .engine
                                                .set_speed_level(self.rate_loop.current_level())
                                            {
                                                tracing::warn!(error = %e, "Failed to apply RateLoop's recommended speed level");
                                            }
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
                    // Closed-loop rate adaptation (Phase 3 Task 4) is wired: `self.
                    // rate_loop` (`coppa_ml::RateLoop`) is fed from the peer's ACK-carried
                    // recommendation (`TransportPdu::suggested_rate` on incoming
                    // `TransportType::Ack | Nak` PDUs, see that match arm) and from
                    // retransmit-timeout events in `check_arq_retransmits`, then applied
                    // to outgoing frames via `CoppaCore::set_speed_level`. See
                    // `crates/coppa-ml/src/rate_loop.rs` for the controller itself.
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

    /// Low-level write of raw samples to the audio-out ring. Does NOT assert
    /// PTT, does NOT apply busy-channel-courtesy deferral, and does NOT
    /// trigger the station-ID timer -- callers that are actually keying a
    /// transmitter must go through `transmit_samples` (the real PTT
    /// chokepoint) instead, which itself calls this as its last step. The
    /// only other direct caller today is the `DaemonEvent::AudioOut` arm in
    /// `run()`, a raw pass-through hook (unused by any in-tree production TX
    /// path as of this writing; exercised only by this file's own tests) --
    /// a future real caller of that event should route through
    /// `transmit_samples` instead if it represents an actual transmission.
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
    ///
    /// This is the single chokepoint every TX path in this event loop funnels
    /// through (host-driven encode, ARQ-adjacent session control frames
    /// including session keepalives, the raw/ARQ TX-queue drain, ARQ
    /// retransmits, `TUNE`), so busy-channel courtesy (Phase 4 Task 3,
    /// decision: gate here rather than duplicate at each call site) and the
    /// station-ID timer's prepend both live here and apply uniformly to
    /// every caller -- none of them are exempted; deferring a
    /// CONNECT_ACK/CFM by a fraction of a second for channel courtesy is
    /// judged better than bulldozing over real QRM, and none of this event
    /// loop's TX paths have a "must never be deferred" real-time constraint.
    ///
    /// Historical note: until this file's Phase 4 whole-branch-review fix,
    /// `check_arq_retransmits` and the session-keepalive sender in `run()`
    /// both wrote directly to the audio-out ring via `handle_audio_out`,
    /// bypassing PTT assertion, busy-channel deferral, and the station-ID
    /// prepend entirely -- silently inert while PTT was a stub, but a real
    /// on-air-silence bug once PTT became real hardware control (Task 2).
    /// Both are now routed through this function like every other TX path,
    /// making the "none of them are exempted" claim above actually true.
    async fn transmit_samples(&mut self, samples: &[f32]) {
        self.wait_for_clear_channel().await;

        // Station-ID timer (Phase 4 Task 3): prepend an ID/beacon frame to
        // this transmission if due. Deliberately only checked here, at an
        // actual TX opportunity -- an idle station that never transmits never
        // needs to identify, so "no activity -> no ID" falls out of this
        // placement for free rather than needing separate bookkeeping.
        let combined;
        let samples: &[f32] = if self.id_due() {
            match self.encode_id_beacon_frame() {
                Some(id_samples) => {
                    self.last_id_time = Instant::now();
                    let mut buf = id_samples;
                    buf.extend_from_slice(samples);
                    combined = buf;
                    &combined
                }
                None => samples,
            }
        } else {
            samples
        };

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

    /// Busy-channel courtesy gate (Phase 4 Task 3): if `[station_id]
    /// busy_hold_ms` is `0`, or the channel doesn't currently read busy, this
    /// is a no-op (no wait, no holdoff -- an already-clear channel never
    /// pays any extra TX latency for this feature). Otherwise, waits for
    /// `coppa_ml::BusyGate` to read clear, then applies a randomized 0.5-2s
    /// courtesy backoff (so multiple stations that were all waiting on the
    /// same busy channel don't all key up in the same instant once it
    /// clears), re-checking busy state after the backoff in case the channel
    /// went busy again during it -- looping back to wait again if so.
    ///
    /// Deliberately independent of `callsign`/station-ID configuration
    /// (unlike `id_due`/`maybe_send_beacon`, which both require
    /// `local_callsign.is_some()`): channel courtesy is basic good operating
    /// practice, not a regulatory identification requirement, so it applies
    /// even when no callsign is configured. Confirmed with Tony (project
    /// owner) as a deliberate design decision, not an oversight against the
    /// brief's literal "all three features off when callsign unset" text.
    ///
    /// `BusyGate` only updates when fed fresh audio, which this event loop
    /// otherwise only reads from `run`'s own `audio_poll` tick -- a
    /// `tokio::select!` branch that can't run concurrently with this same
    /// call (both execute on the same task). This loop therefore drains the
    /// audio input ring and feeds `BusyGate::observe` itself on every
    /// iteration (`observe_busy_gate_from_audio_input` -- see its doc for why
    /// it's a narrower call than `poll_audio_input`), so real RX audio queued
    /// in the input ring (kept filling by a separate OS-level audio
    /// thread/callback even while this task is blocked here) actually
    /// reaches the gate during the wait, instead of its state going stale for
    /// the whole hold.
    async fn wait_for_clear_channel(&mut self) {
        let hold_ms = self.config.station_id.busy_hold_ms;
        if hold_ms == 0 || !self.busy_gate.current() {
            return;
        }
        loop {
            while self.busy_gate.current() {
                self.observe_busy_gate_from_audio_input().await;
                tokio::time::sleep(Duration::from_millis(hold_ms)).await;
            }

            let holdoff_secs: f32 = rand::rng().random_range(0.5f32..2.0f32);
            tokio::time::sleep(Duration::from_secs_f32(holdoff_secs)).await;
            self.observe_busy_gate_from_audio_input().await; // pick up anything that arrived during the holdoff

            if !self.busy_gate.current() {
                return;
            }
            // Busy again during the holdoff -- loop back and wait again.
        }
    }

    /// Whether a station-ID frame is due to be prepended to the next
    /// transmission (Phase 4 Task 3): requires a configured callsign, a
    /// non-zero `[station_id] id_interval_secs`, and at least that many
    /// seconds since the last ID actually sent (or since `EventLoop`
    /// construction, if none yet).
    fn id_due(&self) -> bool {
        // Check `local_callsign` (the parsed form), not the raw config
        // string -- an invalid-but-non-empty `callsign` string parses to
        // `local_callsign: None` (see `EventLoop::new`), and matches the
        // same check `build_beacon_mac_pdu` uses, so this can't report "due"
        // for a frame that `build_beacon_mac_pdu` then silently refuses to
        // build.
        if self.local_callsign.is_none() {
            return false;
        }
        let interval = self.config.station_id.id_interval_secs;
        if interval == 0 {
            return false;
        }
        self.last_id_time.elapsed() >= Duration::from_secs(interval)
    }

    /// Build the `Beacon`-type `MacPdu` carrying this station's
    /// `StationIdPayload` (callsign + optional grid + level, per Task 3's
    /// brief). `dest`/`src` are both this station's own callsign: a
    /// station-ID/beacon frame isn't directed at any particular remote
    /// (nothing in this codebase has a dedicated "CQ"/broadcast callsign
    /// constant, and self-addressing is a reasonable, simple convention for
    /// "not directed at anyone"). Returns `None` if no local callsign is
    /// configured.
    fn build_beacon_mac_pdu(&self) -> Option<MacPdu> {
        let local = self.local_callsign.clone()?;
        let id_payload = StationIdPayload {
            callsign: local.as_str().to_string(),
            grid: self.config.engine.grid.clone(),
            level: 1,
        };
        // `to_bytes` only fails on a >255-byte callsign/grid; a parsed
        // `Callsign` is already bounded well under that, and `grid` is a
        // small free-text locator in practice, but this is still real,
        // operator-supplied config -- log and skip rather than panic/corrupt
        // the frame on the (currently unreachable in practice) error path.
        let payload_bytes = match id_payload.to_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to encode station-ID payload");
                return None;
            }
        };
        Some(MacPdu::new(
            MacFrameType::Beacon,
            local.clone(),
            local,
            0,
            payload_bytes,
        ))
    }

    /// Encode a station-ID/beacon frame at speed level 1 (the most robust
    /// single-codeword level -- Task 3's brief), for prepending
    /// (`transmit_samples`) or standalone sending (`maybe_send_beacon`).
    /// Returns `None` if no local callsign is configured, or if encoding
    /// unexpectedly fails (logged; the caller falls back to proceeding
    /// without an ID rather than dropping the real payload it was about to
    /// send).
    fn encode_id_beacon_frame(&self) -> Option<Vec<f32>> {
        let pdu = self.build_beacon_mac_pdu()?;
        match self.engine.encode_bytes_at_level(&pdu.to_bytes(), 1) {
            Ok(samples) => Some(samples),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to encode station-ID/beacon frame");
                None
            }
        }
    }

    /// Beacon mode (Phase 4 Task 3): called on `beacon_poll`'s 1s tick.
    /// Sends a standalone beacon frame once `[station_id]
    /// beacon_interval_secs` has elapsed since the last one, if a callsign
    /// is configured and the channel currently reads clear. If busy, this
    /// cycle is skipped (not deferred) -- the next tick will try again, per
    /// the brief's "sends a beacon every interval when enabled and channel
    /// is clear."
    async fn maybe_send_beacon(&mut self) {
        let interval = self.config.station_id.beacon_interval_secs;
        // See `id_due`'s comment: check the parsed `local_callsign`, not the
        // raw config string, so this can't report "due" for a callsign that
        // `build_beacon_mac_pdu` will then silently refuse to build a frame for.
        if interval == 0 || self.local_callsign.is_none() {
            return;
        }
        if self.last_beacon_time.elapsed() < Duration::from_secs(interval) {
            return;
        }
        if self.busy_gate.current() {
            return;
        }
        if let Some(samples) = self.encode_id_beacon_frame() {
            self.last_beacon_time = Instant::now();
            // A beacon already fully identifies the station; avoid also
            // prepending a redundant separate ID frame to it.
            self.last_id_time = Instant::now();
            self.transmit_samples(&samples).await;
        }
    }

    /// E6: Check for ARQ retransmits and send them.
    ///
    /// Each retransmitted PDU is routed through `transmit_samples` (the PTT
    /// chokepoint) one at a time, matching `try_drain_tx_queue`'s existing
    /// one-frame-at-a-time pattern: `transmit_samples` schedules its own PTT
    /// release asynchronously per call, so sending retransmit N+1 only after
    /// awaiting retransmit N's `transmit_samples` call keeps each one's PTT
    /// assert/busy-wait/station-ID-prepend logic correctly scoped to that one
    /// frame, rather than batching multiple PDUs under a single PTT key-up.
    async fn check_arq_retransmits(&mut self) {
        if !self.config.engine.arq_enabled {
            return;
        }
        let now = Instant::now();
        // Collect retransmit data first to avoid borrow conflict. Sequence
        // numbers are threaded alongside the encoded PDU bytes so the second
        // loop (after the `arq_tx` borrow ends) can call
        // `ArqTx::mark_retransmitted` for the segment it actually just sent --
        // `get_retransmits`'s documented contract requires this (see its doc):
        // without it, `last_sent`/`transmit_count` never advance, so the same
        // segment reads as "still expired" on every subsequent 500ms poll
        // (an unbounded retransmit storm) and never reaches
        // `config.max_retransmit` to give up.
        let mut retransmit_pdus: Vec<(u8, Vec<u8>)> = Vec::new();
        if let Some(ref mut arq_tx) = self.arq_tx {
            let retransmit_seqs = arq_tx.get_retransmits(now);
            // One timeout EVENT (any number of expired segments in this single
            // poll) maps to exactly one `RateLoop::on_timeout` call, matching
            // `get_retransmits`'s own documented one-call-per-event contract.
            if !retransmit_seqs.is_empty() {
                self.rate_loop.on_timeout();
                if let Err(e) = self.engine.set_speed_level(self.rate_loop.current_level()) {
                    tracing::warn!(error = %e, "Failed to apply RateLoop's recommended speed level after timeout");
                }
            }
            for seq in retransmit_seqs {
                if let Some(data) = arq_tx.get_segment_data(seq) {
                    let pdu =
                        TransportPdu::new_reliable(self.arq_session_id, seq, 0, data.to_vec());
                    retransmit_pdus.push((seq, pdu.to_bytes()));
                }
            }
        }
        // Now encode and transmit (no more borrow on arq_tx)
        for (seq, pdu_bytes) in retransmit_pdus {
            match self.engine.encode_bytes(&pdu_bytes) {
                Ok(samples) => {
                    self.transmit_samples(&samples).await;
                    // Only mark a segment retransmitted once its bytes were
                    // actually sent -- an encode failure below means nothing
                    // went out over the air, so the segment must still read
                    // as due for retry on the next poll rather than have its
                    // `last_sent`/`transmit_count` bookkeeping advance for a
                    // transmission that never happened. Timestamp freshly
                    // *after* `transmit_samples` returns (rather than reusing
                    // the top-of-function `now`), since `transmit_samples`
                    // can itself await for a while (busy-channel courtesy
                    // backoff up to ~2s -- see its doc) before the audio
                    // actually goes out; using a stale pre-wait timestamp
                    // would understate `last_sent` and make the next RTO
                    // check fire early.
                    if let Some(ref mut arq_tx) = self.arq_tx {
                        if let Err(e) = arq_tx.mark_retransmitted(seq, Instant::now()) {
                            tracing::warn!(
                                seq,
                                error = %e,
                                "Failed to mark ARQ segment retransmitted"
                            );
                        }
                    }
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
            MacFrameType::Beacon => self.handle_beacon_rx(pdu),
            _ => {
                tracing::debug!(frame_type = ?pdu.frame_type, "Unhandled MAC frame type");
            }
        }
    }

    /// Received a station-ID/beacon frame (Phase 4 Task 3) from another
    /// station. No session/state-machine effect (a beacon isn't directed at
    /// this station specifically) -- just logged for operator visibility;
    /// full decode of the inner `StationIdPayload` (grid/level) is left to
    /// whatever's consuming the log/telemetry rather than surfaced further,
    /// matching this task's "don't overbuild" guidance.
    fn handle_beacon_rx(&self, pdu: MacPdu) {
        tracing::info!(
            from = %pdu.src,
            bytes = pdu.payload.len(),
            "Station-ID/beacon frame received"
        );
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

    // ── Task 2 (Phase 4): explicit PTT config, no silent NullPtt ─────────

    #[test]
    fn test_create_ptt_explicit_none_succeeds() {
        let mut config = DaemonConfig::default();
        config.radio.ptt_method = "none".to_string();
        assert!(EventLoop::create_ptt(&config).is_ok());
    }

    #[test]
    fn test_create_ptt_unknown_method_is_hard_error() {
        let mut config = DaemonConfig::default();
        config.radio.ptt_method = "carrier-pigeon".to_string();
        let err = EventLoop::create_ptt(&config)
            .err()
            .expect("unrecognized PTT method must be a hard error, not a silent NullPtt");
        assert!(err.to_string().contains("carrier-pigeon"));
    }

    #[test]
    fn test_create_ptt_unimplemented_serial_without_feature_is_hard_error() {
        // Without the `serial-ptt` feature compiled in, a well-formed
        // "serial:..." config must still fail loudly, not fall back silently.
        let mut config = DaemonConfig::default();
        config.radio.ptt_method = "serial:/dev/ttyUSB0:dtr".to_string();
        let result = EventLoop::create_ptt(&config);
        #[cfg(not(feature = "serial-ptt"))]
        assert!(
            result.is_err(),
            "serial PTT without the serial-ptt feature must be a hard error"
        );
        #[cfg(feature = "serial-ptt")]
        {
            // With the feature enabled, parsing succeeds but opening the
            // (nonexistent, in this test environment) port still fails --
            // still an error, just for a different, real-hardware reason.
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_event_loop_new_propagates_ptt_config_error() {
        let mut config = DaemonConfig::default();
        config.radio.ptt_method = "not-a-real-method".to_string();
        assert!(
            EventLoop::new(config).is_err(),
            "EventLoop::new should fail loudly on an unrecognized ptt_method"
        );
    }

    #[tokio::test]
    async fn test_event_loop_shutdown() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();

        // Send shutdown immediately
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(!event_loop.is_running());
    }

    #[tokio::test]
    async fn test_event_loop_host_event() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();

        tx.send(DaemonEvent::PttChange(true)).await.unwrap();
        tx.send(DaemonEvent::PttChange(false)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_audio_in_decode() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();

        // Send silence (won't decode, but should not crash)
        let silence = vec![0.0f32; 1000];
        tx.send(DaemonEvent::AudioIn(silence)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    /// Phase 4 Task 4 required scenario: "spectrum frames of the right
    /// shape/rate" (mock time). Feeds real audio through `handle_audio_in`
    /// and checks the `spectrum` messages broadcast over `ws_broadcast`:
    /// 128 bins each, and rate-limited to `crate::spectrum::SPECTRUM_UPDATE_HZ`
    /// rather than one per audio callback.
    ///
    /// "Mock time" here follows this file's own existing convention for
    /// `Instant`-gated periodic behavior (see `last_id_time`/`last_beacon_time`'s
    /// tests below, e.g. `event_loop.last_id_time = Instant::now() -
    /// Duration::from_secs(600)`): directly backdate the `Instant` field a
    /// rate gate compares against, rather than `tokio::time::advance` (which
    /// only affects `tokio::time::Instant`, not the `std::time::Instant`
    /// these fields actually use, so it wouldn't do anything here).
    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn test_spectrum_broadcast_is_128_bins_and_rate_limited() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();

        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        // Backdate so the very first push (once enough audio has
        // accumulated) is already past the rate-limit period.
        event_loop.last_spectrum_broadcast = Instant::now() - Duration::from_secs(1);

        // Feed exactly one full FFT window's worth of audio in one call.
        let samples = vec![0.01f32; crate::spectrum::SPECTRUM_FFT_SIZE];
        event_loop.handle_audio_in(&samples).await;

        // Drain the broadcast channel for `spectrum` messages.
        let mut first_spectrum = None;
        while let Ok(json) = ws_rx.try_recv() {
            if let Ok(coppa_host::websocket::WsServerMessage::Spectrum { bins, .. }) =
                serde_json::from_str(&json)
            {
                first_spectrum = Some(bins);
            }
        }
        let bins =
            first_spectrum.expect("expected a spectrum broadcast after backdating the rate gate");
        assert_eq!(
            bins.len(),
            crate::spectrum::SPECTRUM_NUM_BINS,
            "spectrum message should carry exactly SPECTRUM_NUM_BINS bins"
        );

        // Immediately push more audio (no backdating this time): real
        // wall-clock elapsed since the broadcast above is far under the
        // rate-limit period, so this must NOT produce a second broadcast.
        event_loop.handle_audio_in(&samples).await;
        assert!(
            ws_rx.try_recv().is_err(),
            "a second push within the rate-limit period must not re-broadcast"
        );

        // Backdate again to simulate the rate-limit period having elapsed,
        // and confirm the gate re-opens.
        event_loop.last_spectrum_broadcast = Instant::now() - Duration::from_secs(1);
        event_loop.handle_audio_in(&samples).await;
        let mut second_spectrum = None;
        while let Ok(json) = ws_rx.try_recv() {
            if let Ok(coppa_host::websocket::WsServerMessage::Spectrum { bins, .. }) =
                serde_json::from_str(&json)
            {
                second_spectrum = Some(bins);
            }
        }
        assert!(
            second_spectrum.is_some(),
            "the rate gate should re-open once its period has (mock-)elapsed"
        );
    }

    #[tokio::test]
    async fn test_audio_out_event() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();

        let samples = vec![0.5f32; 100];
        tx.send(DaemonEvent::AudioOut(samples)).await.unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_audio_out_with_ring_buffer() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();

        // Send shutdown and verify the loop exits with running=false
        tx.send(DaemonEvent::Shutdown).await.unwrap();

        event_loop.run().await.unwrap();
        assert!(!event_loop.is_running());
    }

    #[tokio::test]
    async fn test_ptt_uses_null_by_default() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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
        let mut event_loop = EventLoop::new(config).unwrap();
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

    // ── Task 5: real wire-level round-trip integration tests ─────────────────
    //
    // Unlike `crates/coppa-bench/examples/closed_loop_arq.rs` (which drives
    // `ArqTx`/`ArqRx` directly, in-process, bypassing encode/decode entirely),
    // these two tests exercise the REAL wire path: an independent "peer"
    // `CoppaTransceiver` encodes real OFDM audio samples, `EventLoop` decodes
    // them through its actual `decode_and_dispatch_audio` (the same path
    // `handle_audio_in` uses), and -- for the RX-side test -- whatever the
    // daemon queues as its own outgoing transmission is decoded back with a
    // second, independent `CoppaTransceiver`, exactly as a real remote station
    // would. This is exactly the class of gap this project has been bitten by
    // before (a live decode path silently broken despite passing simulated/
    // unit-level validation).

    /// Zero-lead/trail-padded encode, mirroring `coppa-engine`'s own streaming
    /// tests and this file's `telemetry`/`station_id` submodules: `EventLoop`'s
    /// `decode_and_dispatch_audio` runs through `CoppaCore::push_samples`'s
    /// STREAMING receiver (`StreamingReceiver`), which wants a clean silence
    /// bootstrap before the preamble, plus a little trailing pad so the RX
    /// bandpass filter's group delay doesn't leave `push_samples` seeing
    /// end-of-input before the (filtered-domain) frame is fully buffered. This
    /// padding is NOT needed for `CoppaTransceiver::receive` (the one-shot,
    /// non-streaming decode the RX-side test below uses to read back the
    /// daemon's own transmitted ACK) -- that path re-derives its own timing via
    /// a fresh `SyncDetector::detect_all` on whatever slice it's given, with
    /// zero caller-supplied margin, exactly like every other direct
    /// `CoppaTransceiver::receive` unit test in this workspace.
    fn with_lead_and_trail(samples: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; 8192];
        out.extend_from_slice(samples);
        out.extend(std::iter::repeat_n(0.0f32, 2048));
        out
    }

    #[tokio::test]
    async fn arq_receive_transmits_a_real_ack_with_rate() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_rx = Some(ArqRx::new(8));
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));

        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        // Build a real Reliable TransportPdu from an independent "peer" encoder
        // (a bare CoppaTransceiver, not another EventLoop) and encode it exactly
        // as a real remote station would.
        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        let session_id = 5u8;
        let data_pdu = TransportPdu::new_reliable(session_id, 3, 0, b"hello coppa".to_vec());
        let header = coppa_codec::ofdm::frame::CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: data_pdu.to_bytes().len() as u16,
            codewords: 1,
        };
        let samples = peer_tx
            .transmit(&header, &data_pdu.to_bytes())
            .expect("peer transmit should succeed");

        event_loop
            .decode_and_dispatch_audio(&with_lead_and_trail(&samples))
            .await;

        // Read back whatever the daemon queued as its own outgoing transmission.
        let mut buf = vec![0.0f32; 1_000_000];
        let read = consumer.read(&mut buf);
        assert!(read > 0, "expected the daemon to have transmitted an ACK");

        // Decode it back exactly as the real peer would. `DaemonConfig::default`'s
        // "HF_STANDARD" profile has `compression: true` (see
        // `coppa_engine::profiles::HF_STANDARD`), so `EventLoop`'s own
        // `encode_bytes` call Huffman+LZ4-compresses the ACK's `TransportPdu`
        // bytes before framing -- a bare `CoppaTransceiver::receive` only
        // recovers the still-compressed bytes (marker byte `0xFE` + LZ4 payload)
        // and fails to parse as a `TransportPdu`. A real peer station would also
        // be a `CoppaCore` (or `EventLoop`) built from the same profile, so
        // decode through `CoppaCore::decode_bytes` (public, one-shot,
        // decompression-aware) here too, not a bare transceiver -- otherwise
        // this step doesn't decode "exactly as a real peer would" as intended,
        // it decodes as an incomplete peer would.
        let peer_core = coppa_engine::CoppaCore::from_profile(
            coppa_engine::profiles::get_profile("HF_STANDARD")
                .expect("HF_STANDARD is a built-in profile"),
        );
        let ack_bytes = peer_core
            .decode_bytes(&buf[..read])
            .expect("the transmitted ACK should decode cleanly");
        let ack_pdu = TransportPdu::from_bytes(&ack_bytes).expect("should parse as TransportPdu");

        assert_eq!(ack_pdu.transport_type, TransportType::Ack);
        assert_eq!(
            ack_pdu.session_id & 0x0F,
            session_id & 0x0F,
            "ACK should mirror the received PDU's own session_id"
        );
        assert!(
            ack_pdu.suggested_rate().is_some(),
            "ACK should carry a rate recommendation"
        );
    }

    #[tokio::test]
    async fn incoming_ack_with_rate_updates_rate_loop_and_encoder() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));

        let before_level = event_loop.rate_loop.current_level();

        // A peer ACK recommending a level clearly different from the default
        // (RateLoop::default_coppa starts at level 1) -- enough consecutive
        // higher recommendations to actually step, matching RateLoop's own
        // "raise slow" semantics (raise_dwell = 5 by default).
        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        for _ in 0..5 {
            let ack_pdu = TransportPdu::new_ack_with_rate(0, 0, 0, 10);
            let header = coppa_codec::ofdm::frame::CoppaHeader {
                version: 1,
                phy_mode: 0,
                frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
                bandwidth: 1,
                fec_type: 0,
                speed_level: 2,
                seq_num: 0,
                payload_len: ack_pdu.to_bytes().len() as u16,
                codewords: 1,
            };
            let samples = peer_tx
                .transmit(&header, &ack_pdu.to_bytes())
                .expect("peer transmit should succeed");
            event_loop
                .decode_and_dispatch_audio(&with_lead_and_trail(&samples))
                .await;
        }

        assert!(
            event_loop.rate_loop.current_level() > before_level,
            "RateLoop should have raised its level after 5 consecutive higher recommendations"
        );
        assert_eq!(
            event_loop.engine.speed_level(),
            event_loop.rate_loop.current_level(),
            "the engine's configured speed level should track RateLoop's current level"
        );
    }

    /// Bonus regression test (Task 4 review finding): `check_arq_retransmits`
    /// must call `RateLoop::on_timeout` exactly ONCE per poll, no matter how
    /// many segments expired together in that single `ArqTx::get_retransmits`
    /// call -- not once per expired segment. This is already correct by code
    /// inspection (see `check_arq_retransmits`'s own comment: "one timeout
    /// EVENT ... maps to exactly one `RateLoop::on_timeout` call"), but had no
    /// test locking the contract in before this.
    ///
    /// Seeds two segments whose RTO has already elapsed by the time
    /// `check_arq_retransmits` polls, so `get_retransmits` returns both in one
    /// `Vec` -- the exact "multiple segments expire together" scenario the
    /// contract is about. Starts `rate_loop` at level 5 (not the default level
    /// 1) specifically so a single `on_timeout` step-down (level 5 -> 4) is
    /// distinguishable from the bug this guards against (two calls would drop
    /// to level 3): level 1 can't tell the difference, since
    /// `idx.saturating_sub(1)` floors at 0 either way.
    #[tokio::test]
    async fn check_arq_retransmits_calls_on_timeout_once_per_poll_not_per_segment() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();

        event_loop.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 5);
        assert_eq!(event_loop.rate_loop.current_level(), 5);

        // Two segments, both already past a short RTO by the time
        // `check_arq_retransmits` polls -- `get_retransmits` returns both
        // sequence numbers from this single call.
        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        let send_time = Instant::now() - Duration::from_millis(100);
        arq_tx
            .send(b"segment one".to_vec(), send_time)
            .expect("a fresh ARQ window should have room");
        arq_tx
            .send(b"segment two".to_vec(), send_time)
            .expect("a fresh ARQ window should have room for a second segment");
        event_loop.arq_tx = Some(arq_tx);

        event_loop.check_arq_retransmits().await;

        assert_eq!(
            event_loop.rate_loop.current_level(),
            4,
            "two segments expiring in the same poll should drop RateLoop by \
             exactly ONE step (5 -> 4), not one step per expired segment \
             (which would read as 3)"
        );
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
            let mut event_loop = EventLoop::new(config).unwrap();
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
            let mut event_loop = EventLoop::new(config).unwrap();
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

        /// Regression test for the Phase 4 whole-branch-review PTT-chokepoint
        /// bug: `check_arq_retransmits` used to write straight to the
        /// audio-out ring via `handle_audio_out`, bypassing PTT assertion,
        /// busy-channel-courtesy deferral, and the station-ID timer entirely
        /// -- silently inert while PTT was a stub, but a real on-air-silence
        /// bug once PTT became real hardware control (Task 2). Seeds one ARQ
        /// segment whose RTO has already elapsed and calls
        /// `check_arq_retransmits` directly, confirming it emits real
        /// "PTT ON" telemetry (via `emit_vara`, the same call
        /// `transmit_samples`/`handle_ptt_change` make for every other TX
        /// path) -- not just that audio appeared in the ring.
        ///
        /// Calls `check_arq_retransmits`/`handle_ptt_change` directly rather
        /// than driving them through a spawned `run()` loop and its real
        /// 500ms `retransmit_poll` timer (contrast
        /// `test_ptt_telemetry_brackets_transmission`, which does use a real
        /// `run()` loop for a *single* host-driven TX): a `run()`-driven
        /// version of this test was observed to be genuinely flaky under
        /// parallel test-suite load (multiple overlapping PTT ON/OFF pairs
        /// racing before the first release event could be dispatched).
        /// Calling both methods directly, one shot, sidesteps that
        /// independently-flaky timing entirely while still proving the one
        /// thing this test exists to prove. (`check_arq_retransmits` now
        /// does call `ArqTx::mark_retransmitted` after each retransmit --
        /// see `test_arq_retransmit_marks_retransmitted_and_caps` below for
        /// that contract's own regression coverage -- but this test still
        /// prefers the direct-call pattern for the flakiness reason above.)
        #[tokio::test]
        async fn test_arq_retransmit_asserts_ptt() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19414, 19415).await;

            // Seed one ARQ segment "sent" long enough ago that its RTO has
            // already elapsed by the time `check_arq_retransmits` runs.
            event_loop
                .arq_tx
                .as_mut()
                .expect("arq_enabled = true should construct an ArqTx")
                .send(
                    b"stuck segment".to_vec(),
                    Instant::now() - Duration::from_secs(120),
                )
                .expect("a fresh ARQ window should have room for one segment");

            event_loop.check_arq_retransmits().await;
            let on_line = read_line(&mut reader).await;
            assert_eq!(
                on_line.trim_end(),
                "PTT ON",
                "ARQ retransmit should assert real PTT telemetry, not just \
                 write to the audio-out ring"
            );

            // Simulate the scheduled PTT release completing -- exactly what
            // `run()`'s event-channel dispatch of the `DaemonEvent::PttChange(false)`
            // `transmit_samples` spawns would do, called directly for the
            // same determinism reason as above (mirrors
            // `test_buffer_telemetry_progression_3_to_0`'s established
            // technique of bypassing the real scheduled-release timer, which
            // `test_ptt_telemetry_brackets_transmission` already covers
            // end-to-end for the host-driven TX path).
            event_loop.handle_ptt_change(false).await;
            let off_line = read_line(&mut reader).await;
            assert_eq!(off_line.trim_end(), "PTT OFF");
        }

        /// Regression test for the `get_retransmits`/`mark_retransmitted`
        /// contract bug: `check_arq_retransmits` used to never call
        /// `ArqTx::mark_retransmitted` after actually retransmitting a
        /// segment, so `last_sent` stayed frozen at the segment's original
        /// send time forever (an unbounded retransmit storm -- the same
        /// expired segment retransmitted on every single poll) and
        /// `transmit_count` never advanced (so `max_retransmit`'s bounded
        /// give-up never triggered either). See `crates/coppa-protocol/src/arq.rs`'s
        /// `ArqTx::get_retransmits` doc for the contract.
        ///
        /// Uses a small, custom `ArqConfig` (tiny `initial_rto`, small
        /// `max_retransmit`) swapped directly into `event_loop.arq_tx` --
        /// the same "reach into the private field directly" technique
        /// `test_arq_retransmit_asserts_ptt` uses for seeding a segment --
        /// so the whole test runs in milliseconds of real wall-clock time
        /// rather than waiting out the daemon's real 5s default RTO five
        /// times over. Calls `check_arq_retransmits` directly (not via a
        /// spawned `run()` loop), for the same flakiness reason documented
        /// on `test_arq_retransmit_asserts_ptt`.
        #[tokio::test]
        async fn test_arq_retransmit_marks_retransmitted_and_caps() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19418, 19419).await;

            // Small max_retransmit(2) and a short initial_rto(20ms) so the
            // whole round trip (seed -> expire -> retransmit -> re-expire ->
            // retransmit -> exceed cap) fits in well under a second of real
            // time instead of the crate default 5s RTO x 5 attempts.
            let arq_config = ArqConfig::new(8, 2, Duration::from_millis(20))
                .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
            let mut arq_tx = ArqTx::new(arq_config);
            let send_time = Instant::now() - Duration::from_millis(100);
            arq_tx
                .send(b"stuck segment".to_vec(), send_time)
                .expect("a fresh ARQ window should have room for one segment");
            event_loop.arq_tx = Some(arq_tx);

            // Round 1: the segment's RTO (20ms) has already elapsed relative
            // to `send_time` (100ms ago), so this retransmits it.
            event_loop.check_arq_retransmits().await;
            assert_eq!(
                event_loop.arq_tx.as_ref().unwrap().transmit_count(0),
                Some(2),
                "check_arq_retransmits should call mark_retransmitted, \
                 advancing transmit_count from 1 (send) to 2"
            );

            // Immediately calling again (no time elapsed since the just-updated
            // `last_sent`) must NOT retransmit the same segment again -- this
            // is the core of the bug: before the fix, `last_sent` was frozen
            // at `send_time`, so this second call would retransmit
            // unconditionally on every poll regardless of the real RTO.
            event_loop.check_arq_retransmits().await;
            assert_eq!(
                event_loop.arq_tx.as_ref().unwrap().transmit_count(0),
                Some(2),
                "a segment retransmitted moments ago (well inside its RTO) \
                 must not be retransmitted again immediately"
            );
            assert!(
                !event_loop.arq_tx.as_ref().unwrap().is_failed(0),
                "transmit_count(2) should not yet exceed max_retransmit(2)"
            );

            // Wait out the 20ms RTO for real, then retransmit again -- this
            // is the second (and, per max_retransmit=2, last allowed) retry.
            tokio::time::sleep(Duration::from_millis(40)).await;
            event_loop.check_arq_retransmits().await;
            assert_eq!(
                event_loop.arq_tx.as_ref().unwrap().transmit_count(0),
                Some(3),
                "a second real RTO expiry should retransmit again, advancing \
                 transmit_count to 3"
            );

            // A third RTO expiry must NOT retransmit again: transmit_count(3)
            // already exceeds max_retransmit(2), so `get_retransmits` excludes
            // it and the segment reads as given-up (`is_failed`) -- proving
            // the bounded-retry mechanism this bug also broke now actually
            // triggers.
            tokio::time::sleep(Duration::from_millis(40)).await;
            event_loop.check_arq_retransmits().await;
            assert_eq!(
                event_loop.arq_tx.as_ref().unwrap().transmit_count(0),
                Some(3),
                "a segment already past max_retransmit must not be \
                 retransmitted again"
            );
            assert!(
                event_loop.arq_tx.as_ref().unwrap().is_failed(0),
                "transmit_count(3) > max_retransmit(2) should read as failed/given-up"
            );

            // Drain whatever PTT telemetry accumulated so the mock client's
            // read buffer doesn't matter for this test's assertions -- unlike
            // `test_arq_retransmit_asserts_ptt`, this test cares about `ArqTx`
            // bookkeeping, not the PTT bracket itself (already covered there).
            let _ = read_available_lines(&mut reader).await;
        }

        /// Same regression as `test_arq_retransmit_asserts_ptt`, for the
        /// session-keepalive sender inline in `run()`'s `session_cleanup.tick()`
        /// arm, which had the identical `handle_audio_out`-bypasses-PTT bug.
        /// Seeds one `Established` session whose `last_activity` is already
        /// well past a (deliberately tiny) `keepalive_interval`, so the very
        /// next `session_cleanup` tick (real 5s wall-clock interval; this
        /// event loop's periodic gates all use `std::time::Instant`, not
        /// mockable via `tokio::time::pause`, per this file's own established
        /// convention -- see `test_spectrum_broadcast_is_128_bins_and_rate_limited`'s
        /// doc) sends a keepalive, and confirms it too now produces a real PTT
        /// bracket instead of silently writing to the audio-out ring.
        #[tokio::test]
        async fn test_session_keepalive_asserts_ptt() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19416, 19417).await;

            let local_cs = Callsign::new("VK3ABC").unwrap();
            let remote_cs = Callsign::new("W1AW").unwrap();
            let id = event_loop
                .session_mgr
                .create(local_cs, remote_cs, 0, LinkCapabilities::default())
                .expect("a fresh SessionManager should have a free slot");
            {
                let session = event_loop
                    .session_mgr
                    .get_mut(id)
                    .expect("just-created session should exist");
                session.state = SessionState::Established;
                session.keepalive_interval = Duration::from_millis(1);
                // 65s ago: past the 1ms keepalive_interval, but well inside
                // the default 120s session_timeout, so `cleanup_timed_out`
                // (called earlier in the same tick) doesn't remove the
                // session before the keepalive check runs.
                session.last_activity = Instant::now() - Duration::from_secs(65);
            }

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    // `session_cleanup` only ticks every real 5s (unlike
                    // `retransmit_poll`'s 500ms), so this polls with
                    // `read_available_lines` (no hard per-call timeout
                    // panic) rather than `read_line` (hardcoded 2s timeout --
                    // too short here) across a generous ~10s total budget.
                    let mut lines = Vec::new();
                    for _ in 0..34 {
                        lines.extend(read_available_lines(&mut reader).await);
                        let ptt_so_far: Vec<&str> = lines
                            .iter()
                            .map(|s| s.trim_end())
                            .filter(|s| s.starts_with("PTT"))
                            .collect();
                        if ptt_so_far.len() >= 2
                            && ptt_so_far[0] == "PTT ON"
                            && ptt_so_far[1] == "PTT OFF"
                        {
                            return; // bracket observed in order — test passes
                        }
                    }
                    panic!(
                        "expected a PTT ON ... PTT OFF bracket from the \
                         session keepalive within ~10s, got: {:?}",
                        lines
                    );
                })
                .await;
        }

        /// Task 1 (Phase 4): the VARA `TUNE` command keys PTT, streams the
        /// two-tone calibration signal to the audio-out sink, then unkeys —
        /// exactly the same PTT bracket real frame transmission produces
        /// (`transmit_samples`), verified here with a mock ring-buffer sink
        /// standing in for the real audio device.
        #[tokio::test]
        async fn test_tune_command_keys_ptt_streams_tone_and_unkeys() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (audio_tx, mut audio_rx) = coppa_audio::audio_ring(10_000_000);
            event_loop.set_audio_out(audio_tx);
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19412, 19413).await;
            let tx = event_loop.event_sender();

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    tx.send(DaemonEvent::Host(HostEvent::VaraCommand {
                        client_id: 1,
                        command: "TUNE 1".to_string(),
                    }))
                    .await
                    .unwrap();

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
                        "expected a PTT ON ... PTT OFF bracket for TUNE within the first 8 lines, got: {:?}",
                        lines
                    );
                })
                .await;

            // The mock sink should have received the streamed tone: 1 second
            // at 48kHz (fast_ptt_config's `sample_rate` scheduling override
            // doesn't touch the engine's own fixed 48kHz encode rate).
            let available = audio_rx.available();
            assert!(
                available > 0,
                "TUNE should have streamed tone samples to the audio-out sink"
            );
            let mut buf = vec![0.0f32; available];
            audio_rx.read(&mut buf);
            let peak = buf.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                peak > 0.0,
                "streamed tone samples should be non-silent, got peak {}",
                peak
            );
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
            let mut event_loop = EventLoop::new(config).unwrap();
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
            let mut event_loop = EventLoop::new(config).unwrap();
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

    // ── Task 3 (Phase 4): busy-channel courtesy, station-ID timer, beacon ────

    mod station_id {
        use super::*;

        /// Same lead/trail padding convention as `coppa_engine::CoppaCore`'s and
        /// `telemetry`'s own copies (see their docs): the streaming sync detector
        /// needs a clean silence bootstrap, and the RX bandpass filter's group
        /// delay needs a little trailing pad.
        fn with_lead_and_trail(samples: &[f32]) -> Vec<f32> {
            let mut out = vec![0.0f32; 8192];
            out.extend_from_slice(samples);
            out.extend(std::iter::repeat_n(0.0f32, 2048));
            out
        }

        /// Deterministic per-call PRNG (mirrors the existing busy-telemetry
        /// test's own generator, and `coppa-ml::busy_gate`'s test-doc note about
        /// why a *shared* `static` counter made this exact kind of test flaky
        /// under parallel execution): threads a local counter instead.
        fn noise_block(amplitude: f32, counter: &mut u32) -> Vec<f32> {
            (0..1024)
                .map(|_| {
                    *counter = counter.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    amplitude * ((*counter >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
                })
                .collect()
        }

        fn config_with_callsign() -> DaemonConfig {
            let mut config = DaemonConfig::default();
            config.engine.callsign = "VK3ABC".to_string();
            config
        }

        // ── (a) busy-defer-with-holdoff ───────────────────────────────

        #[tokio::test(start_paused = true)]
        async fn test_transmit_deferred_while_busy_then_holdoff_applied() {
            let mut config = DaemonConfig::default();
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();

            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);
            let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_in(audio_in_rx);

            let mut counter = 1u32;
            // Settle the busy gate's noise floor, then inject a burst so the
            // channel reads busy at the moment `transmit_samples` is called
            // (this is the "injected occupancy" the brief's scenario (a) asks
            // for).
            for _ in 0..10 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.01, &mut counter));
            }
            for _ in 0..5 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.5, &mut counter));
            }
            assert!(
                event_loop.busy_gate.current(),
                "channel should read busy after the injected burst"
            );

            // Pre-load the input ring with several quiet blocks so that when
            // `wait_for_clear_channel`'s loop drives `poll_audio_input` itself
            // (see that method's doc for why it must), it actually observes
            // fresh, below-threshold audio and the gate clears -- without this,
            // the wait would never resolve, since nothing else in this test
            // concurrently feeds the ring.
            for _ in 0..15 {
                audio_in_tx.write(&noise_block(0.01, &mut counter));
            }

            let payload = vec![0.25f32; 500]; // stand-in "encoded frame" audio
            let start = tokio::time::Instant::now();
            event_loop.transmit_samples(&payload).await;
            let elapsed = start.elapsed();

            assert!(
                !event_loop.busy_gate.current(),
                "channel should read clear after the wait"
            );
            assert!(
                elapsed >= Duration::from_millis(500),
                "expected at least the 0.5s courtesy holdoff lower bound, got {:?}",
                elapsed
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "holdoff should stay within its documented 0.5-2s bound (plus polling), got {:?}",
                elapsed
            );

            let available = audio_out_rx.available();
            assert!(
                available >= payload.len(),
                "the deferred transmission should still have gone out eventually"
            );
        }

        // ── Finding 1 fix (Task 3 review): busy-wait reentrancy hazard ──
        //
        // `wait_for_clear_channel` used to drive the *full* `poll_audio_input`
        // (frame decode + MAC-PDU dispatch) on every iteration of its wait
        // loop. A CONNECT_REQ/CONNECT_ACK decoded mid-wait would run
        // `handle_incoming_connect`/`handle_connect_ack_rx`, both of which
        // call `transmit_samples` directly -- a second, nested PTT-key/
        // write-audio/schedule-release cycle interleaved with the
        // already-in-flight *outer* `transmit_samples` call, before
        // `is_transmitting` is even set. The fix makes this structurally
        // impossible: the wait loop now only ever calls
        // `observe_busy_gate_from_audio_input`, which feeds the busy gate
        // but never reaches `decode_and_dispatch_audio`/`handle_mac_pdu`.
        // This test proves the *behavior* that structural change produces:
        // a decodable CONNECT_REQ arriving mid-wait is captured (not
        // dropped) but not dispatched until control returns to `run`'s main
        // loop.
        #[tokio::test(start_paused = true)]
        async fn test_incoming_connect_req_mid_busy_wait_is_not_dispatched_until_after() {
            use coppa_protocol::session::Session;

            let mut config = config_with_callsign(); // local callsign "VK3ABC"
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.listening = true;

            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);
            let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_in(audio_in_rx);

            // Force the busy gate busy, same pattern as the test above.
            let mut counter = 99u32;
            for _ in 0..10 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.01, &mut counter));
            }
            for _ in 0..5 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.5, &mut counter));
            }
            assert!(event_loop.busy_gate.current());

            // Build a real, decodable CONNECT_REQ from a remote station
            // "W1AW", addressed to our own "VK3ABC" -- exactly what
            // `handle_incoming_connect` needs to create a session and fire
            // back a CONNECT_ACK via `transmit_samples`.
            let local_cs = Callsign::new("VK3ABC").unwrap();
            let remote_cs = Callsign::new("W1AW").unwrap();
            let mut remote_session = Session::new(
                0,
                remote_cs.clone(),
                local_cs.clone(),
                0,
                LinkCapabilities::default(),
            );
            let req_pdu = remote_session.initiate().unwrap();
            let req_samples = event_loop
                .engine
                .encode_bytes(&req_pdu.to_bytes())
                .expect("encode should succeed");
            let req_audio = with_lead_and_trail(&req_samples);

            // Sanity check (independent decoder instance, not the event
            // loop's own): the constructed audio really is a decodable
            // frame at the PHY/FEC layer, so this test's premise -- real
            // traffic arriving mid-wait -- actually holds.
            //
            // Previously (before the Phase 4 Task 3.5 fix), `frame.message`
            // would have been `Err(..)` here even on a full, correct decode --
            // `CoppaCore::push_samples`'s old `StreamFrame::message:
            // Result<String>` forced UTF-8 conversion on the decoded payload,
            // which a real (binary) `MacPdu` essentially never satisfies
            // (packed 6-bit callsigns, binary session negotiation payloads).
            // That was a separate, pre-existing bug in the daemon's streaming
            // decode path -- unrelated to Finding 1's reentrancy bug, not fixed
            // by that fix, but fixed since by Task 3.5's `StreamFrame::payload:
            // Result<Vec<u8>>` (no UTF-8 conversion). This sanity check now
            // asserts the full raw-bytes roundtrip, not just "a frame was
            // found".
            // Use the same profile (HF_STANDARD, compression enabled) the
            // daemon's own `event_loop.engine` was built with -- a plain
            // `CoppaCore::new()` defaults to compression *disabled* and would
            // fail to undo `event_loop.engine`'s Huffman+LZ4 compression,
            // which isn't what this sanity check is about.
            let probe_profile = coppa_engine::profiles::get_profile("HF_STANDARD")
                .expect("HF_STANDARD is a built-in profile");
            let mut probe = coppa_engine::CoppaCore::from_profile(probe_profile);
            let probe_frames = probe.push_samples(&req_audio);
            assert_eq!(
                probe_frames.len(),
                1,
                "sanity check: the constructed CONNECT_REQ audio must be \
                 independently decodable for this test's premise to hold"
            );
            assert_eq!(
                probe_frames[0].payload.as_deref().unwrap(),
                req_pdu.to_bytes().as_slice(),
                "sanity check: the binary CONNECT_REQ MacPdu must roundtrip \
                 byte-for-byte through push_samples now that it no longer \
                 forces UTF-8"
            );

            // Pre-load the ring: quiet noise first (so the busy gate reads
            // clear during the wait, same as the injected-occupancy test
            // above), then the CONNECT_REQ audio after it.
            for _ in 0..15 {
                audio_in_tx.write(&noise_block(0.01, &mut counter));
            }
            audio_in_tx.write(&req_audio);

            let payload = vec![0.25f32; 500]; // stand-in "encoded frame" audio
            event_loop.transmit_samples(&payload).await;

            assert!(
                !event_loop.busy_gate.current(),
                "channel should read clear after the wait"
            );
            assert_eq!(
                audio_out_rx.available(),
                payload.len(),
                "exactly the outer transmission should have gone out -- a \
                 nested CONNECT_ACK transmission mid-wait would show up as \
                 extra bytes here"
            );
            assert!(
                event_loop.session_mgr.active_sessions().is_empty(),
                "the CONNECT_REQ must not have been dispatched (no session \
                 created) while the outer transmit_samples call was still \
                 waiting"
            );
            assert!(
                !event_loop.pending_busy_wait_audio.is_empty(),
                "audio observed by the busy gate during the wait must be \
                 queued for later decode, not dropped"
            );

            // Once control returns to the main loop -- simulated here by
            // calling `poll_audio_input` directly, exactly as `run`'s own
            // `audio_poll` tick would -- the deferred audio is handed off to
            // the decoder for real (traffic is deferred, not lost): this must
            // not panic, the pending buffer must drain, and (since the Phase 4
            // Task 3.5 fix) full MAC-level dispatch must actually succeed --
            // this test predates that fix and used to only assert PHY/FEC
            // decodability here, because the old UTF-8-forcing bug meant the
            // CONNECT_REQ could never reach `MacPdu::from_bytes` at all.
            event_loop.poll_audio_input().await;

            assert!(
                event_loop.pending_busy_wait_audio.is_empty(),
                "pending busy-wait audio should have been flushed and handed \
                 to the decoder on the next main-loop poll"
            );
            assert_eq!(
                event_loop.session_mgr.active_sessions().len(),
                1,
                "the deferred CONNECT_REQ should now be fully dispatched end \
                 to end (a session created via handle_incoming_connect) -- \
                 this only happens if MacPdu::from_bytes succeeded on the \
                 frame's payload, proving the Task 3.5 fix rather than just \
                 PHY/FEC decodability"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn test_transmit_not_deferred_when_channel_already_clear() {
            let mut config = DaemonConfig::default();
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            assert!(!event_loop.busy_gate.current());

            let payload = vec![0.25f32; 500];
            let start = tokio::time::Instant::now();
            event_loop.transmit_samples(&payload).await;
            let elapsed = start.elapsed();

            assert!(
                elapsed < Duration::from_millis(100),
                "an already-clear channel must not pay any busy-gate latency, got {:?}",
                elapsed
            );
            assert_eq!(audio_out_rx.available(), payload.len());
        }

        #[tokio::test(start_paused = true)]
        async fn test_busy_gate_disabled_by_default_does_not_defer_tx() {
            // (d) busy-channel gate OFF by default: busy_hold_ms == 0.
            let config = DaemonConfig::default();
            assert_eq!(config.station_id.busy_hold_ms, 0);
            let mut event_loop = EventLoop::new(config).unwrap();
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            let mut counter = 7u32;
            for _ in 0..10 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.01, &mut counter));
            }
            for _ in 0..5 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.5, &mut counter));
            }
            assert!(event_loop.busy_gate.current(), "gate should read busy");

            let payload = vec![0.25f32; 500];
            let start = tokio::time::Instant::now();
            event_loop.transmit_samples(&payload).await;
            let elapsed = start.elapsed();

            assert!(
                elapsed < Duration::from_millis(100),
                "busy_hold_ms == 0 must transmit immediately even while busy, got {:?}",
                elapsed
            );
            assert_eq!(audio_out_rx.available(), payload.len());
        }

        // ── (b) station-ID timer ───────────────────────────────────────

        #[tokio::test]
        async fn test_id_timer_prepends_id_frame_after_interval_elapsed() {
            let mut event_loop = EventLoop::new(config_with_callsign()).unwrap();
            assert_eq!(event_loop.config.station_id.id_interval_secs, 540);
            // Simulate 10 minutes of elapsed time since the last ID (or, here,
            // since construction) without any real sleep.
            event_loop.last_id_time = Instant::now() - Duration::from_secs(600);

            let (audio_out_tx, mut audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            let core = coppa_engine::CoppaCore::new();
            let payload_samples = core.encode_bytes(b"hello").expect("encode should succeed");
            let payload_len = payload_samples.len();

            event_loop.transmit_samples(&payload_samples).await;

            let available = audio_out_rx.available();
            assert!(
                available > payload_len,
                "an ID frame should have been prepended, growing the transmitted audio \
                 (payload was {} samples, got {})",
                payload_len,
                available
            );
            assert!(
                event_loop.last_id_time.elapsed() < Duration::from_secs(1),
                "last_id_time should be refreshed after sending an ID"
            );

            // Verify it's a real, decodable prepended frame: the combined
            // stream should decode as two frames back-to-back (streaming
            // receivers decoding multiple concatenated frames is already
            // exercised elsewhere in this codebase).
            let mut captured = vec![0.0f32; available];
            audio_out_rx.read(&mut captured);
            let padded = with_lead_and_trail(&captured);
            let mut decoder = coppa_engine::CoppaCore::new();
            let frames = decoder.push_samples(&padded);
            assert_eq!(
                frames.len(),
                2,
                "expected the prepended ID frame plus the original payload frame"
            );
            assert_eq!(
                frames[0].speed_level, 1,
                "the ID frame must be sent at speed level 1 (most robust)"
            );
        }

        #[tokio::test]
        async fn test_id_timer_no_prepend_when_interval_not_elapsed() {
            let mut event_loop = EventLoop::new(config_with_callsign()).unwrap();
            // `last_id_time` defaults to construction time -- far less than
            // `id_interval_secs` (540s) has "elapsed".
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            let core = coppa_engine::CoppaCore::new();
            let payload_samples = core.encode_bytes(b"hello").expect("encode should succeed");
            let payload_len = payload_samples.len();

            event_loop.transmit_samples(&payload_samples).await;

            assert_eq!(
                audio_out_rx.available(),
                payload_len,
                "no ID should be prepended before the interval elapses"
            );
        }

        #[tokio::test]
        async fn test_id_timer_no_activity_means_no_id_ever_sent() {
            // "no activity -> no ID": an ID is only ever prepended to a real
            // TX opportunity (see `transmit_samples`'s doc), so a station that
            // never transmits must never emit one either, no matter how much
            // (simulated) time has passed.
            let mut event_loop = EventLoop::new(config_with_callsign()).unwrap();
            event_loop.last_id_time = Instant::now() - Duration::from_secs(3600);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            // No call to transmit_samples / maybe_send_beacon at all.
            assert_eq!(audio_out_rx.available(), 0);
        }

        // ── (c) beacon mode ─────────────────────────────────────────────

        #[tokio::test]
        async fn test_beacon_sends_when_enabled_interval_elapsed_and_clear() {
            let mut config = config_with_callsign();
            config.station_id.beacon_interval_secs = 5;
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.last_beacon_time = Instant::now() - Duration::from_secs(10);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            assert!(!event_loop.busy_gate.current(), "channel starts clear");

            event_loop.maybe_send_beacon().await;

            assert!(
                audio_out_rx.available() > 0,
                "a beacon frame should have been transmitted"
            );
            assert!(
                event_loop.last_beacon_time.elapsed() < Duration::from_secs(1),
                "last_beacon_time should be refreshed after sending"
            );

            // Immediately calling again (interval not yet elapsed) must not
            // send a second beacon.
            let sent_once = audio_out_rx.available();
            event_loop.maybe_send_beacon().await;
            assert_eq!(
                audio_out_rx.available(),
                sent_once,
                "beacon must not re-fire before beacon_interval_secs elapses again"
            );
        }

        #[tokio::test]
        async fn test_beacon_skipped_not_deferred_when_channel_busy() {
            let mut config = config_with_callsign();
            config.station_id.beacon_interval_secs = 5;
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.last_beacon_time = Instant::now() - Duration::from_secs(10);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            let mut counter = 42u32;
            for _ in 0..10 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.01, &mut counter));
            }
            for _ in 0..5 {
                event_loop
                    .busy_gate
                    .observe(&noise_block(0.5, &mut counter));
            }
            assert!(event_loop.busy_gate.current());

            let last_beacon_before = event_loop.last_beacon_time;
            event_loop.maybe_send_beacon().await;

            assert_eq!(
                audio_out_rx.available(),
                0,
                "a busy channel must skip this beacon cycle, not defer it"
            );
            assert_eq!(
                event_loop.last_beacon_time, last_beacon_before,
                "a skipped cycle must not consume the timer, so the very next tick retries"
            );
        }

        #[tokio::test]
        async fn test_beacon_off_by_default() {
            // (d) beacon mode OFF by default: beacon_interval_secs == 0.
            let mut config = config_with_callsign();
            assert_eq!(config.station_id.beacon_interval_secs, 0);
            config.station_id.beacon_interval_secs = 0; // explicit, for clarity
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.last_beacon_time = Instant::now() - Duration::from_secs(3600);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            event_loop.maybe_send_beacon().await;

            assert_eq!(audio_out_rx.available(), 0);
        }

        // ── (d) all three OFF when callsign unset ────────────────────────

        #[tokio::test]
        async fn test_id_and_beacon_off_when_callsign_unset() {
            let mut config = DaemonConfig::default(); // callsign is empty
            config.station_id.id_interval_secs = 1; // would otherwise fire immediately
            config.station_id.beacon_interval_secs = 1;
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.last_id_time = Instant::now() - Duration::from_secs(3600);
            event_loop.last_beacon_time = Instant::now() - Duration::from_secs(3600);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx);

            let core = coppa_engine::CoppaCore::new();
            let payload_samples = core.encode_bytes(b"hello").expect("encode should succeed");
            let payload_len = payload_samples.len();

            event_loop.transmit_samples(&payload_samples).await;
            assert_eq!(
                audio_out_rx.available(),
                payload_len,
                "no ID should be prepended without a configured callsign"
            );

            let (audio_out_tx2, audio_out_rx2) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(audio_out_tx2);
            event_loop.maybe_send_beacon().await;
            assert_eq!(
                audio_out_rx2.available(),
                0,
                "no beacon should be sent without a configured callsign"
            );
        }

        #[test]
        fn test_default_config_all_three_features_off() {
            let config = DaemonConfig::default();
            assert_eq!(
                config.station_id.busy_hold_ms, 0,
                "busy gate off by default"
            );
            assert_eq!(
                config.station_id.beacon_interval_secs, 0,
                "beacon mode off by default"
            );
            assert_eq!(config.engine.callsign, "", "callsign unset by default");
            // id_interval_secs defaults to the FCC-safe 540s (see StationIdConfig's
            // doc for why this alone doesn't mean the feature is "on" -- callsign
            // being unset by default still keeps it inactive).
            assert_eq!(config.station_id.id_interval_secs, 540);
        }
    }
}
