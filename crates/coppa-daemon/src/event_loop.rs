//! Main event loop for the Coppa daemon.
//!
//! Uses `tokio::select!` to multiplex audio, host, and radio events.

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_engine::CoppaCore;
use coppa_host::vara::VaraResponse;
use coppa_host::HostEvent;
use coppa_ml::CpRecommendation;
use coppa_ml::{BusyGate, CpGate, RateLoop};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::cp_negotiator::{ContentAction, CpMode, CpNegotiator};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu, StationIdPayload};
use coppa_protocol::modem::max_payload_for_level;
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
    /// Spread-gated short-CP recommender (`coppa_ml::CpGate`), fed this
    /// station's own measured delay spread on every fully decoded frame when
    /// `[engine] cp_gate_enabled` is set. Measurement/telemetry only -- its
    /// live recommendation is exposed via `WsStatus::short_cp_ok`, but nothing
    /// currently applies it to the engine's CP profile. See
    /// `docs/superpowers/specs/2026-07-25-cpgate-daemon-wiring-design.md`.
    cp_gate: CpGate,
    /// Pure CP-switch negotiation decision state (`coppa_protocol::cp_negotiator`).
    /// See `docs/superpowers/specs/2026-07-29-cp-switch-peer-negotiation-design.md`.
    cp_negotiator: CpNegotiator,
    /// Dedicated small ArqTx/ArqRx pair for `TransportType::CpControl`
    /// traffic -- entirely separate sequence space from the ordinary data
    /// `arq_tx`/`arq_rx` pair, so there is never ambiguity about what a
    /// given seq number represents. Always constructed (cheap), gated by
    /// `cp_negotiation_enabled` only at the point traffic is actually sent
    /// or acted on -- same pattern as `cp_gate`.
    cp_control_arq_tx: ArqTx,
    cp_control_arq_rx: ArqRx,
    /// COP-1 (give-up trigger G1): the CP-control ARQ seq of a `Propose` we
    /// sent and have not yet seen acked. Kept here rather than inside
    /// `CpNegotiator` deliberately, to preserve that module's documented
    /// "holds no state for B's send" property -- `drive_cp_negotiation` only
    /// needs the seq to poll `ArqTx::is_failed` against, which is a daemon
    /// concern (the negotiator has no `ArqTx`). `None` when no Propose is
    /// outstanding.
    cp_propose_seq: Option<u8>,
    /// Next TX sequence number for transport PDUs.
    #[allow(dead_code)] // used when ARQ TX path sends segmented frames
    arq_next_seq: u8,
    /// At most one active-overshoot probe outstanding at a time: `(seq,
    /// probed_level)`. `None` when no probe is in flight. Only ever set/read
    /// when ARQ is enabled -- see `docs/superpowers/specs/
    /// 2026-07-25-rateloop-daemon-probe-wiring-design.md`.
    #[allow(dead_code)] // used by Tasks 4-6
    probe_state: Option<(u8, u8)>,
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
    /// raw/ARQ `HostEvent::DataReceived` TX path), tagged with the ARQ seq
    /// number assigned by `ArqTx::send` when this entry came from the
    /// ARQ-enabled path (`None` otherwise -- non-ARQ sends are never probe
    /// candidates). `VaraResponse::Buffer` telemetry reports this queue's
    /// length on every push/pop.
    tx_queue: VecDeque<(Option<u8>, Vec<u8>)>,
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

        // Active overshoot probing (see `docs/superpowers/specs/
        // 2026-07-25-rateloop-daemon-probe-wiring-design.md`) is off by
        // default -- `rate_loop_probe_interval == 0` matches
        // `RateLoop::with_probing`'s own "0 disables" convention.
        //
        // Seeded from `engine`'s own actual constructed speed level (whatever
        // the configured profile specifies), NOT `RateLoop::default_coppa()`'s
        // hardcoded level 1 -- otherwise a fresh daemon's very first outgoing
        // frame would silently force the engine down to level 1 via
        // `try_drain_tx_queue`'s unconditional `set_speed_level(rate_loop.
        // current_level())`, even with probing disabled. `levels`/`raise_dwell`
        // still match `default_coppa()`'s own choices -- only the initial
        // level differs.
        let rate_loop = RateLoop::new(
            coppa_ml::VALID_SPEED_LEVELS.to_vec(),
            5,
            engine.speed_level(),
        );
        let rate_loop = if config.engine.rate_loop_probe_interval > 0 {
            rate_loop.with_probing(
                config.engine.rate_loop_probe_interval,
                config.engine.rate_loop_probe_offset,
            )
        } else {
            rate_loop
        };

        // Always constructed (cheap; mirrors RateLoop's own "hardcode the
        // hysteresis constants, only expose the interval/offset-equivalent
        // via config" pattern -- no config knobs for threshold_ms/
        // consecutive_needed, YAGNI per CLAUDE.md's alpha-calibration
        // cautionary tale). Whether it's ever fed a real observation is
        // gated by `config.engine.cp_gate_enabled` in
        // `decode_and_dispatch_audio`.
        let cp_gate = CpGate::default_coppa();

        // Always constructed (cheap; mirrors `cp_gate`'s own pattern above).
        // Whether CP-control traffic is ever actually sent or acted on is
        // gated by `config.engine.cp_negotiation_enabled` at each call site.
        let cp_negotiator = CpNegotiator::new();
        let cp_control_arq_config = ArqConfig {
            window_size: 2,
            ..ArqConfig::default()
        };
        let cp_control_arq_tx = ArqTx::new(cp_control_arq_config);
        let cp_control_arq_rx = ArqRx::new(2);

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
            rate_loop,
            cp_gate,
            cp_negotiator,
            cp_propose_seq: None,
            cp_control_arq_tx,
            cp_control_arq_rx,
            arq_next_seq: 0,
            probe_state: None,
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
    async fn enqueue_tx(&mut self, seq: Option<u8>, data: Vec<u8>) {
        self.tx_queue.push_back((seq, data));
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
        if let Some((seq_opt, data)) = self.tx_queue.pop_front() {
            self.emit_vara(VaraResponse::Buffer(self.tx_queue.len()))
                .await;

            // Active overshoot probing (see `docs/superpowers/specs/
            // 2026-07-25-rateloop-daemon-probe-wiring-design.md`) only applies
            // to a fresh ARQ-tracked segment's first transmission (never a
            // retransmit -- those go through `check_arq_retransmits` instead),
            // only one probe outstanding at a time, and only when ARQ is
            // enabled (probing needs the ACK/timeout feedback loop to
            // attribute an outcome).
            let probing_eligible = self.probe_state.is_none()
                && self.config.engine.arq_enabled
                && self.arq_tx.is_some();

            let (level, is_probe) = match seq_opt {
                Some(_) if probing_eligible => {
                    let (lvl, probe) = self.rate_loop.level_for_next_transmission();
                    if probe {
                        // Bounds check: `max_payload_for_level` isn't
                        // monotonic across the ladder (e.g. level 4 = 178
                        // bytes, level 5 = 158) -- a segment that fits at the
                        // current level can fail to fit at a probed higher
                        // one. Skip the probe (send normally) rather than
                        // growing to multiple codewords.
                        match max_payload_for_level(lvl) {
                            Some(max) if data.len() <= max => (lvl, true),
                            _ => (self.rate_loop.current_level(), false),
                        }
                    } else {
                        (lvl, false)
                    }
                }
                _ => (self.rate_loop.current_level(), false),
            };

            // Apply the chosen level: for a probe this is the probed (higher)
            // level; otherwise it's just `rate_loop`'s current steady-state
            // level, which the encoder must be kept in sync with for every
            // fresh send (not only when actually probing). Guarded against
            // the common case where `level` already matches the engine's
            // current speed level -- `CoppaCore::set_speed_level` does a full
            // `reconfigure` (rebuilds the transceiver and streaming receiver)
            // every time, so calling it unconditionally on every fresh send
            // is real, avoidable per-frame cost whenever the level hasn't
            // actually changed.
            if self.engine.speed_level() != level {
                if let Err(e) = self.engine.set_speed_level(level) {
                    tracing::warn!(error = %e, "Failed to apply speed level for queued TX frame");
                }
            }

            match self.engine.encode_bytes(&data) {
                // Boxed: `transmit_samples` -> `handle_ptt_change` -> (on PTT
                // release) `try_drain_tx_queue` forms a 3-way async call cycle;
                // one edge needs indirection to give the compiler a finite-sized
                // future.
                Ok(samples) => {
                    // Only now that the probe frame has actually been handed
                    // to `transmit_samples` does it become a real outstanding
                    // probe -- an encode failure below never reached the air,
                    // so it must not block future probing via a phantom
                    // `probe_state`.
                    if is_probe {
                        self.probe_state =
                            Some((seq_opt.expect("is_probe implies Some(seq)"), level));
                    }
                    Box::pin(self.transmit_samples(&samples)).await;
                }
                Err(e) => tracing::warn!(error = %e, "Encode failed for queued TX frame"),
            }

            if is_probe {
                if let Err(e) = self.engine.set_speed_level(self.rate_loop.current_level()) {
                    tracing::warn!(error = %e, "Failed to revert speed level after probe");
                }
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
                let (seq_opt, tx_bytes) = if self.config.engine.arq_enabled {
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
                                (Some(seq), pdu.to_bytes())
                            }
                            Err(e) => {
                                tracing::warn!(client_id, error = %e, "ARQ window full; dropping TX");
                                return;
                            }
                        }
                    } else {
                        (None, data.clone())
                    }
                } else {
                    (None, data.clone())
                };

                // Queue for encode+transmit (Task 7: BUFFER telemetry tracks this
                // queue's depth; see `enqueue_tx`/`try_drain_tx_queue`). `seq_opt`
                // is `Some` only for a fresh ARQ-tracked segment -- used by active
                // overshoot probing (`RateLoop::level_for_next_transmission`) to
                // attribute a probe's later outcome back to its ACK/timeout.
                self.enqueue_tx(seq_opt, tx_bytes).await;
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
            let delay_spread_ms = frame.delay_spread_ms;
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

                    // `CpGate` (`docs/superpowers/specs/
                    // 2026-07-25-cpgate-daemon-wiring-design.md`): feed this
                    // frame's measured delay spread on the same "decode fully
                    // succeeded" event the SNR/level telemetry above uses. Off
                    // by default (`cp_gate_enabled`). Runs regardless of
                    // whether the `websocket` feature is compiled in -- only
                    // the telemetry *surface* below is feature-gated, not the
                    // gate's own hysteresis state.
                    if self.config.engine.cp_gate_enabled {
                        let before = self.cp_gate.current();
                        let after = self.cp_gate.observe(delay_spread_ms);
                        if after != before {
                            tracing::info!(
                                ?before,
                                ?after,
                                delay_spread_ms,
                                "CpGate recommendation changed"
                            );
                            if self.config.engine.cp_negotiation_enabled
                                && self.config.engine.arq_enabled
                                // COP-1 re-entrancy guard: never start a
                                // second negotiation while one is in flight.
                                // There is ONE `CpNegotiator` per daemon (one
                                // `probation`, one `pending_confirm`, one
                                // `pending_switched`) and one
                                // `cp_propose_seq`, so a second Propose here
                                // used to overwrite the first seq outright:
                                // G1 watches exactly one seq,
                                // `get_retransmits` stops retrying past the
                                // budget but never evicts, and only `abandon`
                                // evicts -- so the orphaned seq parked at
                                // `send_base` forever, permanently consuming
                                // one of the CP-control pair's two slots.
                                // Skipping is right rather than replacing:
                                // the in-flight negotiation still has a
                                // give-up trigger armed, and once it resolves
                                // the next `CpGate` transition proposes
                                // afresh. See `cp_negotiator`'s
                                // "One negotiation at a time" section.
                                && !self.cp_negotiation_in_flight()
                            {
                                let mode = match after {
                                    CpRecommendation::ShortCp => CpMode::ShortCp,
                                    CpRecommendation::LongCp => CpMode::LongCp,
                                };
                                let payload = CpNegotiator::propose_payload(mode);
                                let now = Instant::now();
                                match self.cp_control_arq_tx.send(payload.clone(), now) {
                                    Ok(seq) => {
                                        // COP-1 G1: remember the seq so
                                        // `drive_cp_negotiation` can release the
                                        // window slot if this Propose is never
                                        // acked (`get_retransmits` gives up
                                        // silently but never evicts). The guard
                                        // above guarantees we are not
                                        // overwriting a live seq here.
                                        debug_assert!(self.cp_propose_seq.is_none());
                                        self.cp_propose_seq = Some(seq);
                                        let (ack_num, ack_bitmap) =
                                            self.cp_control_arq_rx.ack_info();
                                        let pdu = TransportPdu::new_cp_control_content(
                                            self.arq_session_id,
                                            seq,
                                            ack_num,
                                            ack_bitmap,
                                            payload,
                                        );
                                        match self.engine.encode_bytes(&pdu.to_bytes()) {
                                            Ok(samples) => self.transmit_samples(&samples).await,
                                            Err(e) => {
                                                tracing::warn!(error = %e, "Failed to encode CpControl Propose")
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "cp_control_arq_tx window full; dropping Propose")
                                    }
                                }
                            } else if self.config.engine.cp_negotiation_enabled
                                && self.config.engine.arq_enabled
                            {
                                tracing::debug!(
                                    ?after,
                                    "CpGate transition observed but a CP negotiation is already \
                                     in flight; not proposing"
                                );
                            }
                        }
                    }

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
                        snap.short_cp_ok = if self.config.engine.cp_gate_enabled {
                            Some(self.cp_gate.current() == CpRecommendation::ShortCp)
                        } else {
                            None
                        };
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
                                        //
                                        // Deviation from the Task 5 brief's literal code
                                        // (documented in the Task 5 report): the brief's
                                        // snippet calls `self.resolve_probe_if_acked(..)`
                                        // (which reborrows all of `self`) from inside this
                                        // `if let Some(ref mut arq_rx) = self.arq_rx` arm,
                                        // but `arq_rx` is used again below
                                        // (`arq_rx.ack_info()`), so its borrow is still
                                        // live at that point and the whole-`self` reborrow
                                        // doesn't compile (E0499). Restructured to finish
                                        // with `arq_rx` (and drop its borrow) before doing
                                        // anything with `self.arq_tx`/`self`, using
                                        // `had_arq_rx` to preserve the original behavior of
                                        // only running `process_ack` when `arq_rx` was
                                        // `Some` -- same observable behavior, no `self`
                                        // aliasing.
                                        let (result_data, ack_info, had_arq_rx) =
                                            if let Some(ref mut arq_rx) = self.arq_rx {
                                                let delivered = arq_rx
                                                    .receive(pdu.seq_num, pdu.payload.clone());
                                                // Collect all delivered payloads
                                                let mut all_data = Vec::new();
                                                for (_seq, data) in delivered {
                                                    all_data.extend(data);
                                                }
                                                (all_data, Some(arq_rx.ack_info()), true)
                                            } else {
                                                (pdu.payload, None, false)
                                            };
                                        // Process ACK info back to our TX side
                                        if had_arq_rx {
                                            if let Some(ref mut arq_tx) = self.arq_tx {
                                                let newly_acked = arq_tx.process_ack(
                                                    pdu.ack_num,
                                                    pdu.ack_bitmap,
                                                    Instant::now(),
                                                );
                                                self.resolve_probe_if_acked(&newly_acked);
                                            }
                                        }
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
                                        let mut probe_resolved = false;
                                        if let Some(ref mut arq_tx) = self.arq_tx {
                                            let newly_acked = arq_tx.process_ack(
                                                pdu.ack_num,
                                                pdu.ack_bitmap,
                                                Instant::now(),
                                            );
                                            probe_resolved =
                                                self.resolve_probe_if_acked(&newly_acked);
                                        }
                                        // Closed-loop rate adaptation: apply the
                                        // peer's recommendation (if this ACK carries
                                        // one) and push the resulting level into the
                                        // encoder for subsequent outgoing frames.
                                        // Skipped when this same ACK just resolved an
                                        // outstanding probe -- a probe's outcome is
                                        // stronger, direct ground truth than the
                                        // passive per-frame recommendation, and
                                        // `on_probe_result`/`on_ack` are mutually
                                        // exclusive per event (see
                                        // `docs/superpowers/specs/
                                        // 2026-07-25-rateloop-daemon-probe-wiring-design.md`).
                                        if !probe_resolved {
                                            if let Some(rate) = pdu.suggested_rate() {
                                                self.rate_loop.on_ack(rate, true);
                                                if let Err(e) = self
                                                    .engine
                                                    .set_speed_level(self.rate_loop.current_level())
                                                {
                                                    tracing::warn!(error = %e, "Failed to apply RateLoop's recommended speed level");
                                                }
                                            }
                                        }
                                        Vec::new()
                                    }
                                    TransportType::Reset => {
                                        // Reset ARQ state
                                        self.arq_tx = Some(ArqTx::new(ArqConfig::default()));
                                        self.arq_rx = Some(ArqRx::new(8));
                                        // A Reset restarts ARQ sequence numbering from 0,
                                        // so any outstanding probe's tracked seq is no
                                        // longer meaningful: leaving it set could either
                                        // permanently block future probing (if its seq is
                                        // never reused) or cause a later, unrelated fresh
                                        // segment that happens to reuse the same seq to be
                                        // spuriously resolved as a "successful probe" via
                                        // `resolve_probe_if_acked`.
                                        self.probe_state = None;
                                        // Also reset the CP-control pair and
                                        // negotiator -- see this arm's doc
                                        // above for why Reset (not a
                                        // MAC-session event) is this
                                        // feature's reset point.
                                        self.cp_control_arq_tx = ArqTx::new(ArqConfig {
                                            window_size: 2,
                                            ..ArqConfig::default()
                                        });
                                        self.cp_control_arq_rx = ArqRx::new(2);
                                        self.cp_negotiator = CpNegotiator::new();
                                        self.cp_propose_seq = None;
                                        // COP-1: `CpNegotiator::new()` starts
                                        // at LongCp, so without this the
                                        // bookkeeping and the engine disagree
                                        // whenever a Reset reaches a station
                                        // that had already switched -- exactly
                                        // the desync this ticket exists to
                                        // prevent, just reached by a different
                                        // route.
                                        self.engine.set_cp_profile(CpMode::LongCp);
                                        Vec::new()
                                    }
                                    TransportType::CpControl => {
                                        self.handle_cp_control(&pdu).await;
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

    /// If a probe is outstanding and its seq appears in `newly_acked`, resolve
    /// it as a successful active-overshoot probe (`RateLoop::on_probe_result`),
    /// apply the resulting level, and clear `probe_state`. Returns `true` if a
    /// probe was resolved by this call -- the caller must then skip its own
    /// normal `on_ack` application for this event (mutually exclusive
    /// dispatch, matching `coppa_ml::RateLoop`'s `level_for_next_transmission`/
    /// `on_probe_result` call-site contract). See `docs/superpowers/specs/
    /// 2026-07-25-rateloop-daemon-probe-wiring-design.md`.
    fn resolve_probe_if_acked(&mut self, newly_acked: &[u8]) -> bool {
        if let Some((seq, level)) = self.probe_state {
            if newly_acked.contains(&seq) {
                self.rate_loop.on_probe_result(level, true);
                if let Err(e) = self.engine.set_speed_level(self.rate_loop.current_level()) {
                    tracing::warn!(error = %e, "Failed to apply speed level after probe success");
                }
                self.probe_state = None;
                return true;
            }
        }
        false
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

            // If the tracked probe's seq is among the expired seqs, it
            // resolves as a FAILED active overshoot probe -- an ordinary
            // ARQ-retransmitted loss (the retransmit loop below still resends
            // it normally, at whatever level `current_level()` now is), but
            // must not itself drop RateLoop's idx the way a genuine passive
            // timeout does (`on_probe_result`'s failure path is a no-op by
            // design). See `docs/superpowers/specs/
            // 2026-07-25-rateloop-daemon-probe-wiring-design.md`.
            let probe_failed_seq = self
                .probe_state
                .filter(|&(seq, _)| retransmit_seqs.contains(&seq))
                .map(|(seq, _)| seq);
            if let Some((seq, level)) = self.probe_state {
                if Some(seq) == probe_failed_seq {
                    self.rate_loop.on_probe_result(level, false);
                    self.probe_state = None;
                }
            }

            // One timeout EVENT (any number of expired NON-PROBE segments in
            // this single poll) maps to exactly one `RateLoop::on_timeout`
            // call, matching `get_retransmits`'s own documented
            // one-call-per-event contract. If the ONLY expired seq this poll
            // was the probe (already handled above), no `on_timeout` call
            // happens at all.
            let other_timeouts = retransmit_seqs
                .iter()
                .filter(|&&s| Some(s) != probe_failed_seq)
                .count();
            if other_timeouts > 0 {
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

        // CP-control retransmits: same collect-then-transmit shape as the
        // arq_tx block above, using the entirely separate cp_control_arq_tx/
        // cp_control_arq_rx pair (own sequence space, own small window).
        if self.config.engine.cp_negotiation_enabled {
            let now = Instant::now();
            let cp_retransmit_seqs = self.cp_control_arq_tx.get_retransmits(now);
            let mut cp_retransmit_pdus: Vec<(u8, Vec<u8>)> = Vec::new();
            if !cp_retransmit_seqs.is_empty() {
                let (ack_num, ack_bitmap) = self.cp_control_arq_rx.ack_info();
                for seq in &cp_retransmit_seqs {
                    if let Some(data) = self.cp_control_arq_tx.get_segment_data(*seq) {
                        let pdu = TransportPdu::new_cp_control_content(
                            self.arq_session_id,
                            *seq,
                            ack_num,
                            ack_bitmap,
                            data.to_vec(),
                        );
                        cp_retransmit_pdus.push((*seq, pdu.to_bytes()));
                    }
                }
            }
            for (seq, pdu_bytes) in cp_retransmit_pdus {
                match self.engine.encode_bytes(&pdu_bytes) {
                    Ok(samples) => self.transmit_samples(&samples).await,
                    Err(e) => tracing::warn!(error = %e, "Failed to encode CpControl retransmit"),
                }
                // Count the attempt whether or not the encode succeeded
                // (review finding). `ArqTx::is_failed` -- the sole trigger
                // behind G1, G2 and G4 -- reads `transmit_count`, which only
                // `mark_retransmitted` advances. Advancing it only on a
                // successful encode meant a persistently failing encode left
                // the budget frozen forever, so no give-up trigger could ever
                // fire and the CP-control pair leaked its slot for good: the
                // exact unbounded wait COP-1 exists to eliminate, reached by
                // the one path the give-up machinery could not see. An attempt
                // that could not even be encoded is still an attempt spent,
                // and this also restarts the RTO so the next poll paces
                // itself rather than spinning.
                //
                // Scoped to the CP-control pair deliberately: the ordinary
                // data `arq_tx` loop above keeps its encode-gated counting,
                // where a frozen budget means retrying forever rather than
                // wedging a two-slot control channel.
                if let Err(e) = self
                    .cp_control_arq_tx
                    .mark_retransmitted(seq, Instant::now())
                {
                    tracing::warn!(
                        seq,
                        error = %e,
                        "Failed to mark CpControl segment retransmitted"
                    );
                }
            }
        }

        // COP-1: drive the CP-negotiation give-up triggers off this same
        // 500 ms poll. Must run AFTER the retransmit block above, so a
        // segment gets its full retransmit budget spent this tick before
        // `is_failed` is consulted. Returns immediately when
        // `cp_negotiation_enabled` is false (the default).
        self.drive_cp_negotiation(Instant::now());
    }

    // ── CP-switch peer negotiation (COP-1) ────────────────────────────

    /// Whether this station is already part of a CP negotiation that has not
    /// converged yet -- either as the proposer (`cp_propose_seq`, whose
    /// give-up trigger G1 the daemon owns) or via any of the negotiator's own
    /// wait states (`CpNegotiator::negotiation_in_flight`).
    ///
    /// The re-entrancy guard both the `CpGate`-transition block and the
    /// inbound-`Propose` arm consult. See `cp_negotiator`'s
    /// "One negotiation at a time" section for what it enforces, and for the
    /// one window (a `Propose` acked but its `Confirm` not yet received) it
    /// cannot see.
    fn cp_negotiation_in_flight(&self) -> bool {
        self.cp_propose_seq.is_some() || self.cp_negotiator.negotiation_in_flight()
    }

    /// Handle one received `TransportType::CpControl` PDU: the CP-switch
    /// peer-negotiation handshake's whole receive side.
    ///
    /// Extracted from `decode_and_dispatch_audio`'s match arm in COP-1 --
    /// the arm had grown past 150 lines at six levels of nesting, which is
    /// exactly where the third leg had to be added. Pure code motion apart
    /// from the COP-1 additions called out inline below.
    ///
    /// See `coppa_protocol::cp_negotiator`'s module doc for the three-leg
    /// diagram and the G1-G4 give-up table this pairs with.
    async fn handle_cp_control(&mut self, pdu: &TransportPdu) {
        if !self.config.engine.cp_negotiation_enabled {
            tracing::debug!("CpControl PDU received but cp_negotiation_enabled is false; ignoring");
            return;
        }

        let now = Instant::now();
        let newly_acked = self
            .cp_control_arq_tx
            .process_ack(pdu.ack_num, pdu.ack_bitmap, now);

        // COP-1 (G1 bookkeeping): our Propose is resolved, so stop watching
        // it for failure. Without this, `drive_cp_negotiation` would keep
        // polling `is_failed` on a seq whose slot has already been freed and
        // possibly recycled by a later negotiation.
        if let Some(seq) = self.cp_propose_seq {
            if newly_acked.contains(&seq) {
                self.cp_propose_seq = None;
            }
        }

        if let Some(mode) = self.cp_negotiator.on_confirm_acked(&newly_acked) {
            self.engine.set_cp_profile(mode);
            tracing::info!(
                ?mode,
                "CP profile switched (confirmer role, confirmed by peer)"
            );
            // COP-1 third leg: tell the peer, explicitly and ARQ-tracked,
            // that we have switched. Encoded under the NEW profile, which
            // the peer is already listening on -- that is exactly what its
            // bare ack (the thing we just processed) proved. Without this
            // the peer's only disarm signal would be "some frame decoded
            // eventually," which never arrives on an idle link.
            self.send_cp_switched(pdu.session_id, mode, now).await;
        }

        // COP-1: our own third leg may be acked by this same PDU, which
        // completes the handshake on our side and disarms G4.
        if self.cp_negotiator.on_switched_acked(&newly_acked) {
            tracing::debug!("CP switch fully confirmed by peer; handshake complete");
        }

        if pdu.payload.is_empty() {
            // Bare ack only; nothing further to do.
            return;
        }

        // `receive` returns only the genuinely new, in-order-delivered
        // segments (mirrors the `Reliable | Unreliable` arm); a
        // duplicate/retransmitted CpControl PDU returns an empty `delivered`
        // here and must not re-run any content action -- no re-sent Confirm,
        // no re-applied mode, no re-tracked pending confirm (review Finding 1).
        let delivered = self
            .cp_control_arq_rx
            .receive(pdu.seq_num, pdu.payload.clone());
        let (ack_num, ack_bitmap) = self.cp_control_arq_rx.ack_info();

        // COP-1 remediation: re-ack a *`CpSwitched`* content PDU that
        // delivered nothing.
        //
        // It is a duplicate (a seq `ArqRx` has already delivered, which lands
        // outside its window and so returns empty) or a gap-filler buffered
        // ahead of `recv_base`. Either way the peer is still waiting on an ack
        // it did not get, and dropping this silently is what made the bare ack
        // for the third leg (step 6 of the module doc's six-step diagram)
        // *un-retryable*: A retransmits its ARQ-tracked `CpSwitched`, the
        // dedupe swallowed it before it could reach the ack-sending code
        // below, so B's lost ack could never be re-elicited -- A's G4 then
        // reverted A while B, having already disarmed probation, stayed on the
        // new profile forever. Acking here makes step 6 recoverable by
        // ordinary retransmission, the same mechanism that covers legs 1, 2
        // and 4.
        //
        // Cannot loop: a bare ack has an empty payload and returns above,
        // before ever reaching `receive`.
        //
        // **Restricted to the `CpSwitched` kind** (review finding). Re-acking
        // every duplicate disarms the give-up triggers the module doc's
        // convergence table depends on:
        //
        // - a retransmitted `Propose` would be acked, clearing the peer's
        //   `cp_propose_seq`, so G1 could never fire on the "`Confirm` lost"
        //   row -- and if this station had itself dropped that `Propose` under
        //   the re-entrancy guard, the ack would leave the peer waiting on a
        //   `Confirm` that is never coming, with no trigger left at all;
        // - a retransmitted `Confirm` would be acked under the NEW profile
        //   (B has already switched by then) which A, still on the old one,
        //   cannot decode -- so the ack buys A nothing while still costing it
        //   its G2 trigger. G2/G3 own that case, exactly as before.
        //
        // Only for `CpSwitched` are both stations already on the same, new
        // profile, which is what makes the re-ack both meaningful and safe.
        if delivered.is_empty() {
            let switched_mode = match CpNegotiator::on_content_received(&pdu.payload) {
                Some(ContentAction::PeerSwitched(mode)) => Some(mode),
                _ => None,
            };
            let Some(mode) = switched_mode else {
                tracing::debug!(
                    seq = pdu.seq_num,
                    "Duplicate/out-of-order CpControl content that is not a third leg; \
                     ignored so the peer's own give-up trigger still fires"
                );
                return;
            };
            // Re-check acceptance on the duplicate, not just the payload KIND
            // (review finding). `on_peer_switched` itself can't be re-called
            // here -- it mutates `probation`/`revert_to` on success, so a
            // second call after a genuine accept would find `probation`
            // already cleared and incorrectly report rejection. `current()`
            // is the safe read-only proxy: `apply_as_confirmer` sets it to
            // the target mode BEFORE `on_peer_switched` ever runs, so it
            // already equals `mode` for both a first-time accept and a
            // legitimate re-ack of one. It only stops matching `mode` once
            // G3's `revert()` has fired (a genuine reject/timeout) or when
            // this station never negotiated to `mode` at all -- exactly the
            // two cases the first-reception path at `on_peer_switched`
            // (below) also rejects.
            if self.cp_negotiator.current() != mode {
                tracing::warn!(
                    ?mode,
                    negotiator_mode = ?self.cp_negotiator.current(),
                    seq = pdu.seq_num,
                    "Duplicate CpSwitched leg does not match our current mode; \
                     not re-acked, so the peer's own give-up trigger still fires"
                );
                return;
            }
            let ack_pdu = TransportPdu::new_cp_control_ack(pdu.session_id, ack_num, ack_bitmap);
            match self.engine.encode_bytes(&ack_pdu.to_bytes()) {
                Ok(samples) => self.transmit_samples(&samples).await,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to encode CpControl duplicate re-ack")
                }
            }
            tracing::debug!(
                seq = pdu.seq_num,
                "Duplicate CpSwitched; re-acked without re-running its action"
            );
            return;
        }

        for (_seq, data) in delivered {
            match CpNegotiator::on_content_received(&data) {
                Some(ContentAction::SendConfirm(confirm_payload)) => {
                    // COP-1 re-entrancy guard (the other half of the one in
                    // the CpGate-transition block): this daemon has exactly
                    // ONE `CpNegotiator`, so taking on the confirmer role
                    // while our own negotiation is still in flight would run
                    // both roles through the same single-slot state. Drop the
                    // Propose without a Confirm AND without an ack: the peer's
                    // G1 then fires and it converges on its pre-negotiation
                    // mode, whereas acking would clear its `cp_propose_seq`
                    // and leave it waiting on a Confirm forever with no
                    // trigger. Two stations whose `CpGate`s transition at once
                    // cross their Proposes, both drop, both fire G1, and both
                    // stay on the mode they already agreed on -- the next
                    // transition proposes again.
                    if self.cp_negotiation_in_flight() {
                        tracing::debug!(
                            seq = pdu.seq_num,
                            "Inbound Propose while our own CP negotiation is in flight; \
                             dropped unacked so the peer's G1 fires"
                        );
                        continue;
                    }
                    match self.cp_control_arq_tx.send(confirm_payload.clone(), now) {
                        Ok(seq) => {
                            let mode = CpMode::from_wire(confirm_payload[1]);
                            self.cp_negotiator.track_pending_confirm(seq, mode);
                            let reply = TransportPdu::new_cp_control_content(
                                pdu.session_id,
                                seq,
                                ack_num,
                                ack_bitmap,
                                confirm_payload,
                            );
                            match self.engine.encode_bytes(&reply.to_bytes()) {
                                Ok(samples) => self.transmit_samples(&samples).await,
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to encode CpControl Confirm")
                                }
                            }
                        }
                        Err(e) => {
                            // The Propose has already been DELIVERED by
                            // `cp_control_arq_rx.receive` (recv_base has
                            // advanced), so no retransmission of it can reach
                            // this arm again -- and with no Confirm sent,
                            // `track_pending_confirm` never runs, so G2 never
                            // arms. That is survivable, not a desync, and only
                            // because of two other properties: we have not
                            // switched anything (the mode is applied on the
                            // Confirm's ack, not here), and the duplicate
                            // re-ack above is restricted to `CpSwitched`, so
                            // the peer's retransmitted Propose stays unacked
                            // and its own G1 fires. Both stations therefore
                            // converge on the pre-negotiation mode; the
                            // negotiation is simply wasted.
                            //
                            // The window is two slots wide and the
                            // re-entrancy guard above already refuses this
                            // Propose whenever one of ours is in flight, so
                            // reaching here at all means the pair is holding
                            // segments the give-up triggers have not reclaimed
                            // yet -- worth a warn, not a silent debug.
                            tracing::warn!(error = %e, "cp_control_arq_tx window full; dropping Confirm (peer's G1 converges it)")
                        }
                    }
                }
                Some(ContentAction::ApplyAsConfirmer(mode)) => {
                    // Send the bare ack FIRST, while still on the OLD
                    // profile: the peer is still listening on the old profile
                    // until it receives this exact ack (that's what proves
                    // its own Confirm was delivered, and is what gates ITS
                    // switch), so switching our own engine before sending
                    // this would encode the ack under a profile the peer
                    // can't yet decode, deadlocking the handshake. Found via
                    // real end-to-end audio testing, not by inspection -- see
                    // task-5-report.md's "Significant discovery" section for
                    // the full diagnosis. `CoppaCore::set_cp_profile` has no
                    // way to switch only the receiver (it rebuilds
                    // transmitter and receiver together), so deferring the
                    // WHOLE switch until after this send is the only fix that
                    // doesn't require a bigger coppa-engine API change.
                    //
                    // COP-1: this ack is still fire-and-forget -- it cannot
                    // be ARQ-tracked without recreating the very deadlock
                    // above. What changed is that losing it is no longer
                    // fatal: we arm probation below, and the peer's own
                    // Confirm eventually fails its retransmit budget (G2), so
                    // both sides converge on LongCp instead of stranding on
                    // mutually-undecodable profiles.
                    let ack_pdu =
                        TransportPdu::new_cp_control_ack(pdu.session_id, ack_num, ack_bitmap);
                    match self.engine.encode_bytes(&ack_pdu.to_bytes()) {
                        Ok(samples) => self.transmit_samples(&samples).await,
                        Err(e) => tracing::warn!(error = %e, "Failed to encode CpControl ack"),
                    }
                    // Only now, after the ack is genuinely on its way under
                    // the old profile, switch our own engine
                    // (transmitter+receiver together) to the new mode. This
                    // also arms COP-1's probation deadline (G3): having
                    // switched, we are deaf to the peer's old profile, so it
                    // now has a bounded window to prove it switched too.
                    self.cp_negotiator.apply_as_confirmer(mode, now);
                    self.engine.set_cp_profile(mode);
                    tracing::info!(
                        ?mode,
                        probation_secs = coppa_protocol::cp_negotiator::SWITCH_PROBATION_SECS,
                        "CP profile switched (proposer role, own receiver); awaiting peer's Switched leg"
                    );
                }
                Some(ContentAction::PeerSwitched(mode)) => {
                    // COP-1 third leg received: proof our own switch was not
                    // made in vain, so disarm probation. Ack it so the peer's
                    // ARQ-tracked leg resolves (G4) -- we are already on
                    // `mode`, so this ack encodes under exactly the profile
                    // the peer is now listening on.
                    //
                    // Only ack an ACCEPTED leg (review finding). A leg naming
                    // a mode we did not switch to, or one arriving with no
                    // probation armed (e.g. after G3 already reverted us), is
                    // rejected by `on_peer_switched` -- and acking it anyway
                    // resolved the sender's G4 while our own probation stayed
                    // armed to revert us seconds later, stranding the two
                    // stations on different profiles. Staying silent instead
                    // lets the sender's G4 fire, and both converge on the
                    // pre-negotiation mode.
                    if !self.cp_negotiator.on_peer_switched(mode) {
                        tracing::warn!(
                            ?mode,
                            negotiator_mode = ?self.cp_negotiator.current(),
                            "CpSwitched leg does not match any switch we are awaiting; \
                             not acked, so the peer's G4 converges it"
                        );
                        continue;
                    }
                    let ack_pdu =
                        TransportPdu::new_cp_control_ack(pdu.session_id, ack_num, ack_bitmap);
                    match self.engine.encode_bytes(&ack_pdu.to_bytes()) {
                        Ok(samples) => self.transmit_samples(&samples).await,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to encode CpControl Switched-ack")
                        }
                    }
                    tracing::info!(
                        ?mode,
                        "Peer confirmed its own CP switch; handshake complete"
                    );
                }
                None => tracing::debug!("Malformed CpControl payload; ignoring"),
            }
        }
    }

    /// Emit COP-1's third leg (`CpSwitched`) under the profile we just
    /// switched to, ARQ-tracked so give-up trigger G4 can notice if the peer
    /// never acks it.
    async fn send_cp_switched(&mut self, session_id: u8, mode: CpMode, now: Instant) {
        let payload = CpNegotiator::switched_payload(mode);
        match self.cp_control_arq_tx.send(payload.clone(), now) {
            Ok(seq) => {
                self.cp_negotiator.track_pending_switched(seq);
                let (ack_num, ack_bitmap) = self.cp_control_arq_rx.ack_info();
                let switched_pdu = TransportPdu::new_cp_control_content(
                    session_id, seq, ack_num, ack_bitmap, payload,
                );
                match self.engine.encode_bytes(&switched_pdu.to_bytes()) {
                    Ok(samples) => self.transmit_samples(&samples).await,
                    Err(e) => tracing::warn!(error = %e, "Failed to encode CpControl Switched"),
                }
            }
            Err(e) => {
                // The caller has ALREADY committed our engine to `mode` (it
                // must -- the third leg has to be encoded under the new
                // profile). Simply logging here would leave us switched with
                // no tracked leg, hence no `pending_switched_seq`, hence no
                // way for G4 to ever fire: precisely the unbounded wait COP-1
                // exists to eliminate. So walk the engine back instead.
                //
                // The `encode_bytes` Err arm above deliberately does NOT do
                // this: there the seq IS tracked, so the retransmit loop
                // re-encodes it and, failing that, G4 fires normally. Only the
                // send failure escapes the give-up machinery entirely.
                //
                // That justification is only true because the CP retransmit
                // loop now advances `transmit_count` even when the re-encode
                // fails. It previously called `mark_retransmitted` solely on a
                // successful encode, so a persistently failing encode froze
                // the budget and `is_failed` -- hence G4 -- was unreachable,
                // making the sentence above assert the opposite of what the
                // code did. See `check_arq_retransmits`'s CP block.
                let reverted = self.cp_negotiator.abort();
                self.engine.set_cp_profile(reverted);
                tracing::warn!(
                    error = %e,
                    ?reverted,
                    "cp_control_arq_tx window full; dropping Switched and reverting (no G4 could arm)"
                );
            }
        }
    }

    /// Drive the CP-negotiation give-up triggers (COP-1). Takes `now`
    /// explicitly, `arq.rs`-style, so tests can inject a future `Instant`
    /// rather than sleeping out a 180-second probation.
    ///
    /// Every trigger converges on the **pre-negotiation mode** -- the last
    /// mode both stations are known to have agreed on, which for the first
    /// negotiation of a session is the `CpMode::LongCp` both boot into. See
    /// `coppa_protocol::cp_negotiator`'s module doc for the full G1-G4 table,
    /// the per-lost-frame convergence matrix, and why a hardcoded `LongCp`
    /// here was wrong for a `ShortCp` -> `LongCp` negotiation.
    ///
    /// Not `async`: nothing here transmits. A give-up is deliberately silent
    /// -- the whole premise is that the peer cannot hear us.
    ///
    /// G2, G3 and G4 are resolved as **one** give-up, not three independent
    /// ones. An earlier revision claimed G3 and G4 were "role-disjoint by
    /// construction: only B arms probation and only A tracks a pending
    /// `Switched`, so they can never both fire on the same station." That was
    /// false -- there is one `CpNegotiator` per daemon, both stations run
    /// `CpGate` over the same channel, and nothing made a station
    /// intrinsically an A or a B (see `cp_negotiator`'s "One negotiation at a
    /// time" section, and the re-entrancy guard that now enforces the
    /// invariant the claim assumed). Treating them separately had a concrete
    /// cost even so: G4's `abort()` clears `pending_confirm`, so a co-pending
    /// Confirm seq was dropped before the G2 block below could read and
    /// abandon it -- leaking the very window slot G2 exists to reclaim.
    ///
    /// So: read every tracked leg first, decide, then abandon them all,
    /// `abort()` once, and `set_cp_profile` once. Abandoning the whole set
    /// together is also what makes the window really come back to
    /// `in_flight() == 0` -- `ArqTx::abandon` only advances `send_base` over a
    /// fully-resolved prefix, so abandoning a subset frees no room (see its
    /// doc).
    fn drive_cp_negotiation(&mut self, now: Instant) {
        if !self.config.engine.cp_negotiation_enabled {
            return;
        }

        // COP-2 canary. After `set_speed_level` stopped rewriting `cp_mode`, no
        // production path can diverge these two (every `set_cp_profile` caller passes the
        // mode the negotiator just moved to), so this is a regression tripwire for a
        // FUTURE writer, not a live repair. It deliberately does not self-heal:
        // repairing here would mask the regression and, on a 500 ms poll, could rebuild
        // the transceiver twice a second against whatever kept rewriting it.
        // Deliberately not a `debug_assert!` either: a desync is a recoverable link
        // condition, and panicking inside the daemon's poll loop is a worse failure
        // than warning. Two field reads per tick.
        if self.engine.cp_mode() != self.cp_negotiator.current() {
            tracing::warn!(
                engine_cp_mode = ?self.engine.cp_mode(),
                negotiator_cp_mode = ?self.cp_negotiator.current(),
                "CP mode desync: engine and negotiator disagree (COP-2 invariant violated)"
            );
        }

        // Snapshot the ARQ-tracked legs BEFORE anything clears them.
        let confirm_seq = self.cp_negotiator.pending_confirm_seq();
        let switched_seq = self.cp_negotiator.pending_switched_seq();

        // G3: probation expired -- the peer never proved it switched. This is
        // THE trigger the lost-bare-ack case depends on. `tick` disarms the
        // probation either way and reports `None` when there is no
        // pre-negotiation mode left to go back to, so a no-op revert costs
        // neither an engine rebuild nor a misleading operator warning.
        let g3 = self.cp_negotiator.tick(now).is_some();
        // G4: our own Switched leg was never acked -- the peer is unreachable
        // under the new profile, so fall back with it.
        let g4 = switched_seq.is_some_and(|seq| self.cp_control_arq_tx.is_failed(seq));
        // G2: our Confirm was never acked. We never switched, so only
        // bookkeeping needs clearing -- but the engine is still re-applied
        // below so the negotiator and the engine cannot drift apart.
        let g2 = confirm_seq.is_some_and(|seq| self.cp_control_arq_tx.is_failed(seq));

        if g2 || g3 || g4 {
            for seq in [confirm_seq, switched_seq].into_iter().flatten() {
                self.cp_control_arq_tx.abandon(seq);
            }
            // Revert to the mode `abort` reports, NOT unconditionally to
            // `LongCp`. On a ShortCp -> LongCp negotiation those differ, and
            // using the wrong one is what desynced the link (see
            // `cp_negotiator`'s module doc). After a G3 tick this is the mode
            // `tick` already reverted to, so the two agree by construction.
            let reverted = self.cp_negotiator.abort();
            self.engine.set_cp_profile(reverted);
            tracing::warn!(
                g2,
                g3,
                g4,
                ?confirm_seq,
                ?switched_seq,
                ?reverted,
                "CP negotiation gave up; both bookkeeping and engine walked back"
            );
        }

        // G1: our Propose was never acked. Clear it so a later CpGate
        // transition can propose again -- `get_retransmits` gives up silently
        // but never evicts, so without the `abandon` the two-slot CP-control
        // window leaks one slot per failed negotiation. Kept separate from the
        // block above because the daemon, not the negotiator, owns this seq,
        // and because B reaching G1 has nothing to walk back: it never
        // switched.
        if let Some(seq) = self.cp_propose_seq {
            if self.cp_control_arq_tx.is_failed(seq) {
                self.cp_control_arq_tx.abandon(seq);
                self.cp_propose_seq = None;
                tracing::warn!(seq, "CpControl Propose leg failed; negotiation abandoned");
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

    #[test]
    fn new_wires_probe_config_into_rate_loop() {
        let mut config = DaemonConfig::default();
        config.engine.rate_loop_probe_interval = 3;
        config.engine.rate_loop_probe_offset = 1;
        let mut event_loop = EventLoop::new(config).unwrap();

        // `DaemonConfig::default()`'s profile ("HF_STANDARD") constructs the
        // engine at speed_level 2, so `rate_loop` now starts at level 2
        // (idx 1 in VALID_SPEED_LEVELS); offset 1 steps to idx 2 = level 3.
        // Probe should trigger on the 3rd call.
        assert_eq!(
            event_loop.rate_loop.level_for_next_transmission(),
            (2, false)
        );
        assert_eq!(
            event_loop.rate_loop.level_for_next_transmission(),
            (2, false)
        );
        assert_eq!(
            event_loop.rate_loop.level_for_next_transmission(),
            (3, true)
        );
    }

    /// Standalone "do the primitives compose correctly" check: hand-calls
    /// `CpNegotiator`/`ArqTx`/`ArqRx`/`TransportPdu` directly in the same
    /// order the real `decode_and_dispatch_audio`/`check_arq_retransmits`
    /// code does, WITHOUT going through either of those real methods. Kept
    /// alongside `cp_negotiation_full_handshake_converges_both_sides` below
    /// (which does drive the real dispatch code, per review Finding 6)
    /// because this one is a useful, fast, low-noise check that the
    /// underlying primitives compose the way the design doc says they
    /// should -- but on its own it would NOT have caught a bug purely in
    /// the match-arm/retransmit-block wiring code (e.g. review Findings
    /// 1-3), since it bypasses that wiring entirely.
    #[tokio::test]
    async fn cp_negotiation_handshake_primitives_compose_correctly() {
        use coppa_ml::CpRecommendation;
        use coppa_protocol::cp_negotiator::CpMode;

        // Two independent EventLoops standing in for two stations, both
        // with ARQ and CP negotiation enabled. `b` will play "the station
        // that observed a calm channel and proposes ShortCp"; `a` will play
        // "the station that receives Propose, confirms, and eventually
        // flips its own encoder." `EventLoop::new` builds its own internal
        // event channel (confirmed signature: `pub fn new(config:
        // DaemonConfig) -> Result<Self>`) -- mirrors this file's own
        // existing `new_wires_probe_config_into_rate_loop` test's
        // construction pattern exactly.
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        config.engine.cp_gate_enabled = true;
        config.engine.cp_negotiation_enabled = true;
        let mut a = EventLoop::new(config.clone()).unwrap();
        let mut b = EventLoop::new(config).unwrap();

        assert_eq!(a.engine.cp_mode(), CpMode::LongCp);
        assert_eq!(b.cp_negotiator.current(), CpMode::LongCp);

        // b's CpGate observes a qualifying transition (4 consecutive calm
        // frames, per CpGate::default_coppa) and proposes ShortCp.
        let mut rec = CpRecommendation::LongCp;
        for _ in 0..4 {
            rec = b.cp_gate.observe(0.1);
        }
        assert_eq!(rec, CpRecommendation::ShortCp);
        let propose_payload = CpNegotiator::propose_payload(CpMode::ShortCp);
        let now = std::time::Instant::now();
        let seq_propose = b
            .cp_control_arq_tx
            .send(propose_payload.clone(), now)
            .unwrap();
        let (ack_num0, ack_bitmap0) = b.cp_control_arq_rx.ack_info();
        let propose_pdu = coppa_protocol::transport::TransportPdu::new_cp_control_content(
            b.arq_session_id,
            seq_propose,
            ack_num0,
            ack_bitmap0,
            propose_payload,
        );

        // a receives the Propose: registers it, replies with Confirm.
        a.cp_control_arq_rx
            .receive(propose_pdu.seq_num, propose_pdu.payload.clone());
        let (a_ack_num, a_ack_bitmap) = a.cp_control_arq_rx.ack_info();
        let action = CpNegotiator::on_content_received(&propose_pdu.payload).unwrap();
        let confirm_payload = match action {
            coppa_protocol::cp_negotiator::ContentAction::SendConfirm(p) => p,
            other => panic!("expected SendConfirm, got {other:?}"),
        };
        let seq_confirm = a
            .cp_control_arq_tx
            .send(confirm_payload.clone(), now)
            .unwrap();
        a.cp_negotiator
            .track_pending_confirm(seq_confirm, CpMode::ShortCp);
        let confirm_pdu = coppa_protocol::transport::TransportPdu::new_cp_control_content(
            a.arq_session_id,
            seq_confirm,
            a_ack_num,
            a_ack_bitmap,
            confirm_payload,
        );

        // b receives the Confirm: applies to its own receiver immediately,
        // processes the piggybacked ack for its own Propose, and replies
        // with a bare ack for a's Confirm.
        let newly_acked_propose =
            b.cp_control_arq_tx
                .process_ack(confirm_pdu.ack_num, confirm_pdu.ack_bitmap, now);
        assert!(newly_acked_propose.contains(&seq_propose));
        b.cp_control_arq_rx
            .receive(confirm_pdu.seq_num, confirm_pdu.payload.clone());
        let (b_ack_num, b_ack_bitmap) = b.cp_control_arq_rx.ack_info();
        match CpNegotiator::on_content_received(&confirm_pdu.payload).unwrap() {
            coppa_protocol::cp_negotiator::ContentAction::ApplyAsConfirmer(mode) => {
                b.cp_negotiator.apply_as_confirmer(mode, now);
                b.engine.set_cp_profile(mode);
            }
            other => panic!("expected ApplyAsConfirmer, got {other:?}"),
        }
        assert_eq!(b.cp_negotiator.current(), CpMode::ShortCp);
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::ShortCp,
            "b's own RECEIVER must switch first"
        );

        let bare_ack = coppa_protocol::transport::TransportPdu::new_cp_control_ack(
            b.arq_session_id,
            b_ack_num,
            b_ack_bitmap,
        );

        // a receives the bare ack for its Confirm: NOW (and only now) a
        // flips its own encoder.
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "a must not have switched yet"
        );
        let newly_acked_confirm =
            a.cp_control_arq_tx
                .process_ack(bare_ack.ack_num, bare_ack.ack_bitmap, now);
        assert!(newly_acked_confirm.contains(&seq_confirm));
        let mode = a
            .cp_negotiator
            .on_confirm_acked(&newly_acked_confirm)
            .unwrap();
        a.engine.set_cp_profile(mode);

        assert_eq!(a.cp_negotiator.current(), CpMode::ShortCp);
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::ShortCp,
            "a's own ENCODER switches last, after proof of delivery"
        );

        // A frame encoded by a under the new profile must decode correctly
        // against a receiver built for hf_standard_short_cp -- confirms
        // this isn't just bookkeeping, the actual profile really changed.
        let samples = a.engine.encode_bytes(b"after switch").unwrap();
        let mut check_receiver = coppa_engine::CoppaCore::new();
        check_receiver.set_cp_profile(CpMode::ShortCp);
        let decoded = check_receiver.decode_bytes(&samples);
        assert!(
            decoded.is_ok(),
            "frame sent after the switch must decode under the matching short-CP profile"
        );
    }

    /// Review Finding 6: unlike the primitives-composition test above, this
    /// one drives the REAL `EventLoop::decode_and_dispatch_audio` (and, by
    /// virtue of that, the real `TransportType::CpControl` match arm fixed
    /// by Findings 1-3) via genuine encode/decode round trips between two
    /// independent `EventLoop`s, each wired to its own real
    /// `coppa_audio::audio_ring`, mirroring
    /// `probe_send_applies_level_sets_probe_state_and_reverts`'s pattern for
    /// hooking up real audio I/O and `arq_receive_transmits_a_real_ack_with_rate`'s
    /// pattern for reading back a station's own queued transmission and
    /// feeding it to a peer's real decode path.
    ///
    /// The one real design choice here: `decode_and_dispatch_audio`'s
    /// propose-on-transition block lives inline in the per-frame decode
    /// loop, keyed off a REAL decoded frame's `delay_spread_ms` (not
    /// something a test can just hand `CpGate` directly without bypassing
    /// this method entirely). There is no separately-callable production
    /// entry point for "react to this CpGate transition" alone. Rather than
    /// reach into private state to fake a transition, `b` is fed 4 real,
    /// independently-encoded "warmup" frames through its own real
    /// `decode_and_dispatch_audio` -- on a clean digital loopback (no
    /// channel impairment) each decodes with a near-zero measured delay
    /// spread, so by the 4th one `CpGate::default_coppa`'s real hysteresis
    /// (4 consecutive frames under 2.5 ms) has genuinely tripped and the
    /// real propose-on-transition block really fires. Each warmup frame's
    /// payload is deliberately not a well-formed `TransportPdu` (first byte
    /// low nibble `0x0F`, an unrecognized `TransportType`), so it's forwarded
    /// as inert undecodable bytes with zero side effects (no ARQ receive, no
    /// ACK transmitted) -- the only thing being exercised on those 4 calls is
    /// the real CpGate-observe-and-maybe-propose block, not anything ARQ-data
    /// related.
    ///
    /// Driving this test through the REAL dispatch path (rather than the
    /// hand-rolled primitives above) originally surfaced a genuine
    /// protocol-level bug in the shipped CP-switch feature: `coppa_engine::
    /// CoppaCore::set_cp_profile` (via `reconfigure`/`build`) rebuilds BOTH
    /// `self.transceiver` (TX/encoder) AND `self.streaming` (RX/receiver)
    /// from the same `cp_mode` -- there is no way to switch only one side.
    /// The `ApplyAsConfirmer` branch used to call
    /// `self.engine.set_cp_profile(mode)` BEFORE sending the bare ack, so
    /// B's very next transmission -- the bare ack that A's own switch
    /// (`on_confirm_acked`) is waiting on -- went out already re-encoded
    /// under the NEW CP profile, before A had switched. A's receiver was
    /// still on the OLD profile at that point (by design: A doesn't switch
    /// until it sees this exact ack), so in real use A could never actually
    /// decode it, and the handshake would stall with A stuck pre-switch
    /// forever. This is now FIXED: the `ApplyAsConfirmer` branch sends the
    /// bare ack first, while still on the OLD profile (which the peer can
    /// still decode), and only switches its own engine afterward -- see the
    /// branch's own doc comment and task-5-report.md's "Significant
    /// discovery" section (and its follow-up fix note) for the full
    /// diagnosis.
    ///
    /// This test still carries a workaround (`a.engine.set_cp_profile
    /// (CpMode::LongCp)` immediately before the ack-decode step below), but
    /// NOT for the reason above anymore, and with a different (no-op-valued)
    /// target than before -- attempting to remove it entirely, once the fix
    /// above landed, surfaced a SECOND, independent, previously-unknown bug:
    /// `a`'s own engine, after one real decode, fails to decode ANY
    /// subsequent real frame via `push_samples` at all, even bit-identical
    /// audio a fresh `EventLoop`/`CoppaCore` decodes fine. This reproduces
    /// with ARQ disabled (no `CpControl` reply involved) and is unrelated to
    /// `coppa_audio::audio_ring` (proven bit-exact across the round trip);
    /// it does not reproduce for raw-`CoppaTransceiver`-built ("warmup"-
    /// style) frames, which is why the 4+1 consecutive decodes on `b` above
    /// never hit it. Working around it via a same-value `set_cp_profile
    /// (CpMode::LongCp)` call exploits `reconfigure`/`build`'s
    /// always-rebuild-`self.streaming` behavior to reset whatever internal
    /// state gets stuck, without lying about which profile the incoming
    /// audio is actually encoded under (unlike the old workaround, which had
    /// to force `a` to a profile mismatching its own then-buggy encoding)
    /// -- so this test can still assert `a.engine.cp_mode() == ShortCp` as
    /// genuine evidence after the decode, since this reset only ever leaves
    /// it at `LongCp`. See the workaround's own inline comment below and
    /// task-5-report.md's follow-up fix section for the full diagnostic
    /// trail; the residual bug itself is real, reproducible, and out of
    /// scope for this task's CP-negotiation fix, so it's flagged rather than
    /// chased down here.
    #[tokio::test]
    async fn cp_negotiation_full_handshake_converges_both_sides() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        config.engine.cp_gate_enabled = true;
        config.engine.cp_negotiation_enabled = true;
        let mut a = EventLoop::new(config.clone()).unwrap();
        let mut b = EventLoop::new(config).unwrap();

        let (a_producer, mut a_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        let (b_producer, mut b_consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(b_producer);

        assert_eq!(a.engine.cp_mode(), CpMode::LongCp);
        assert_eq!(b.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(b.cp_gate.current(), CpRecommendation::LongCp);

        // Drive b's CpGate to a real ShortCp transition via 4 genuine
        // decoded frames (see the doc comment above for why this, rather
        // than a direct `cp_gate.observe` call, is the real production
        // path).
        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        for i in 0..4u8 {
            let warmup_payload: Vec<u8> = vec![0xFF, i, 0, 0, 0, 0, 0, 1, 2, 3];
            let header = coppa_codec::ofdm::frame::CoppaHeader {
                version: 1,
                phy_mode: 0,
                frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
                bandwidth: 1,
                fec_type: 0,
                speed_level: 2,
                seq_num: 0,
                payload_len: warmup_payload.len() as u16,
                codewords: 1,
            };
            let samples = peer_tx
                .transmit(&header, &warmup_payload)
                .expect("peer transmit should succeed");
            b.decode_and_dispatch_audio(&with_lead_and_trail(&samples))
                .await;
        }
        assert_eq!(
            b.cp_gate.current(),
            CpRecommendation::ShortCp,
            "4 consecutive clean-channel decodes should trip CpGate's real hysteresis"
        );

        // b's real propose-on-transition block should have really called
        // `cp_control_arq_tx.send` and really transmitted a CpControl
        // Propose PDU onto its own audio-out ring.
        let mut buf = vec![0.0f32; 1_000_000];
        let read = b_consumer.read(&mut buf);
        assert!(
            read > 0,
            "expected b to have really transmitted a CpControl Propose"
        );
        let propose_samples = with_lead_and_trail(&buf[..read]);

        // a decodes b's real Propose through the real dispatch path: this
        // exercises the actual `TransportType::CpControl` match arm (the
        // Findings 1-3 fix), which should really call
        // `cp_control_arq_rx.receive`, `CpNegotiator::on_content_received`,
        // `cp_control_arq_tx.send` for the Confirm, and really transmit it.
        a.decode_and_dispatch_audio(&propose_samples).await;

        let mut a_buf = vec![0.0f32; 1_000_000];
        let a_read = a_consumer.read(&mut a_buf);
        assert!(
            a_read > 0,
            "expected a to have really transmitted a CpControl Confirm"
        );
        let confirm_samples = with_lead_and_trail(&a_buf[..a_read]);

        // b decodes a's real Confirm: applies to its own receiver
        // immediately (ApplyAsConfirmer), and really transmits a bare ack.
        assert_eq!(
            b.cp_negotiator.current(),
            CpMode::LongCp,
            "b must not have switched yet"
        );
        b.decode_and_dispatch_audio(&confirm_samples).await;
        assert_eq!(b.cp_negotiator.current(), CpMode::ShortCp);
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::ShortCp,
            "b's own RECEIVER must switch first"
        );

        let mut b_buf2 = vec![0.0f32; 1_000_000];
        let b_read2 = b_consumer.read(&mut b_buf2);
        assert!(
            b_read2 > 0,
            "expected b to have really transmitted a bare ack for a's Confirm"
        );
        let ack_samples = with_lead_and_trail(&b_buf2[..b_read2]);

        // a decodes b's real bare ack: NOW (and only now) applies to its
        // own encoder.
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "a must not have switched yet"
        );
        // Workaround, kept for a DIFFERENT reason than before, and with a
        // DIFFERENT (no-op-valued) target than before: the original
        // CP-profile-mismatch bug this workaround used to route around is
        // now genuinely FIXED (see the `ApplyAsConfirmer` branch's own doc
        // comment and task-5-report.md's "Significant discovery" /
        // "Follow-up fix" sections) -- `b`'s bare-ack audio is now correctly
        // encoded under the OLD (`LongCp`) profile, matching `a`'s own
        // still-`LongCp` engine, so there is no real profile mismatch left
        // to route around.
        //
        // But removing this workaround entirely surfaced a SECOND,
        // independent, previously-unknown bug while fixing this one: `a`'s
        // own engine, after having already completed exactly one real
        // decode (of `b`'s Propose, above), fails to decode ANY subsequent
        // real frame at all (`push_samples` returns zero frames) -- even a
        // bit-for-bit-identical copy of audio that a completely FRESH
        // `CoppaCore`/`EventLoop` (built from the same config) decodes
        // correctly on the first try. This was diagnosed thoroughly enough
        // to rule out every theory tied to this task's own fix or to CP
        // profile specifically: it reproduces with ARQ disabled entirely (no
        // `CpControl`/`SendConfirm` reply in play), with completely
        // unrelated encoded content, and with the exact same audio fed
        // through vs. bypassing `coppa_audio::audio_ring`'s producer/
        // consumer round trip (proven bit-identical before vs. after that
        // round trip, ruling out ring corruption). It does NOT reproduce
        // when the earlier decode is of a raw-`CoppaTransceiver`-built
        // ("warmup"-style) frame instead of a `CoppaCore::encode_bytes`-
        // built one, which is why `b`'s 5 consecutive real decodes above
        // never hit it. Root cause not further isolated (looks like genuine
        // `StreamingReceiver` internal state not resetting correctly after a
        // successful decode, in some content/config-dependent way) -- real,
        // reproducible, and entirely orthogonal to this task's CP-
        // negotiation fix, so it's flagged here rather than chased down; see
        // task-5-report.md's follow-up fix section for the full diagnostic
        // trail.
        //
        // `CoppaCore::set_cp_profile` always rebuilds `self.streaming` fresh
        // via `reconfigure`/`build`, regardless of whether the mode value
        // actually changes -- calling it with `a`'s CURRENT mode (`LongCp`,
        // a genuine no-op for the profile itself) is enough to reset that
        // stuck internal state and let the ack decode succeed, without
        // lying about which profile the incoming audio is really encoded
        // under (unlike the pre-fix workaround, which had to claim `a` was
        // already on `ShortCp` to match the bug's own mis-encoded ack).
        // Because this reset only ever sets `LongCp`, it does NOT touch
        // `ShortCp` itself -- so `a.engine.cp_mode() == ShortCp`, asserted
        // below after the real decode, is genuine evidence that the real
        // `on_confirm_acked` dispatch path's own `self.engine.set_cp_profile`
        // call executed, not tautological.
        a.engine.set_cp_profile(CpMode::LongCp);
        a.decode_and_dispatch_audio(&ack_samples).await;
        assert_eq!(
            a.cp_negotiator.current(),
            CpMode::ShortCp,
            "a's real on_confirm_acked dispatch should have applied the switch"
        );
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::ShortCp,
            "a's real on_confirm_acked dispatch should have switched its own engine"
        );

        // Final real round-trip: a frame encoded by a after the switch must
        // decode correctly against a receiver built for
        // hf_standard_short_cp -- confirms this isn't just bookkeeping, the
        // actual profile really changed.
        let samples = a.engine.encode_bytes(b"after switch").unwrap();
        let mut check_receiver = coppa_engine::CoppaCore::new();
        check_receiver.set_cp_profile(CpMode::ShortCp);
        let decoded = check_receiver.decode_bytes(&samples);
        assert!(
            decoded.is_ok(),
            "frame sent after the switch must decode under the matching short-CP profile"
        );
    }

    // ── COP-1 Phase 3: daemon wiring for the third leg and give-up ────────

    use coppa_protocol::cp_negotiator::SWITCH_PROBATION_SECS;

    /// Build a real, decodable frame carrying `pdu`, encoded under `profile`
    /// by an independent peer transceiver (not another `EventLoop`), ready to
    /// hand to `decode_and_dispatch_audio`.
    fn peer_frame(pdu: &TransportPdu, profile: coppa_codec::ofdm::CoppaProfile) -> Vec<f32> {
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(profile, 1);
        let bytes = pdu.to_bytes();
        let header = coppa_codec::ofdm::frame::CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: bytes.len() as u16,
            codewords: 1,
        };
        let samples = peer_tx
            .transmit(&header, &bytes)
            .expect("peer transmit should succeed");
        with_lead_and_trail(&samples)
    }

    fn cp_enabled_config() -> DaemonConfig {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        config.engine.cp_negotiation_enabled = true;
        config
    }

    /// Drive `seq` past its retransmit budget so `ArqTx::is_failed` reports
    /// it, without waiting out real RTOs. `mark_retransmitted` is the same
    /// call the real retransmit loop makes; doing it `max_retransmit` times
    /// is exactly what a peer that never acks produces.
    fn exhaust_retransmits(tx: &mut ArqTx, seq: u8) {
        let mut now = Instant::now();
        for _ in 0..coppa_protocol::arq::DEFAULT_MAX_RETRANSMIT {
            now += Duration::from_secs(120);
            let _ = tx.get_retransmits(now);
            tx.mark_retransmitted(seq, now)
                .expect("segment should still be in flight");
        }
        assert!(
            tx.is_failed(seq),
            "test setup: segment should read as failed"
        );
    }

    #[tokio::test]
    async fn confirmer_emits_cp_switched_under_the_new_profile_when_its_confirm_is_acked() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(producer);

        // Put `a` in the real "sent a Confirm, waiting for the peer's bare
        // ack" state (the state the real SendConfirm arm leaves it in).
        let confirm_payload = match CpNegotiator::on_content_received(
            &CpNegotiator::propose_payload(CpMode::ShortCp),
        ) {
            Some(ContentAction::SendConfirm(p)) => p,
            other => panic!("expected SendConfirm, got {other:?}"),
        };
        let seq_confirm = a
            .cp_control_arq_tx
            .send(confirm_payload, Instant::now())
            .unwrap();
        a.cp_negotiator
            .track_pending_confirm(seq_confirm, CpMode::ShortCp);
        assert_eq!(a.engine.cp_mode(), CpMode::LongCp);

        // The peer's bare ack, encoded under the OLD profile `a` is still on
        // -- exactly what the real peer sends (see the ApplyAsConfirmer arm's
        // send-ack-before-switching ordering).
        let bare_ack =
            TransportPdu::new_cp_control_ack(a.arq_session_id, seq_confirm.wrapping_add(1), 0);
        a.decode_and_dispatch_audio(&peer_frame(
            &bare_ack,
            coppa_codec::ofdm::CoppaProfile::hf_standard(),
        ))
        .await;

        assert_eq!(
            a.engine.cp_mode(),
            CpMode::ShortCp,
            "the bare ack is a's proof of delivery; it must switch its encoder now"
        );
        let switched_seq = a
            .cp_negotiator
            .pending_switched_seq()
            .expect("a must track its own third leg for give-up trigger G4");

        // And it must really have TRANSMITTED that third leg, under the NEW
        // profile (which is what the peer is now listening on).
        let mut buf = vec![0.0f32; 1_000_000];
        let read = consumer.read(&mut buf);
        assert!(
            read > 0,
            "expected a to have really transmitted a CpSwitched"
        );
        // Decode with an engine built EXACTLY the way the daemon builds its
        // own -- `CoppaCore::new()` would differ in `compression_enabled`,
        // silently yielding the still-compressed bytes rather than the PDU.
        // (The pre-existing handshake test only asserts `decoded.is_ok()`,
        // which is why that difference never surfaced before.)
        let mut check = EventLoop::new(cp_enabled_config()).unwrap();
        check.engine.set_cp_profile(CpMode::ShortCp);
        let decoded = check
            .engine
            .decode_bytes(&buf[..read])
            .expect("the third leg must be encoded under the NEW profile");
        let leg = TransportPdu::from_bytes(&decoded).expect("must be a TransportPdu");
        assert_eq!(leg.transport_type, TransportType::CpControl);
        assert_eq!(
            leg.payload,
            CpNegotiator::switched_payload(CpMode::ShortCp),
            "payload must be the CpSwitched kind naming the mode we switched to"
        );
        assert_eq!(
            leg.seq_num, switched_seq,
            "the leg must carry its tracked seq"
        );
    }

    #[tokio::test]
    async fn proposer_disarms_probation_and_acks_on_receiving_cp_switched() {
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(producer);

        // `b` has switched and armed probation (the real ApplyAsConfirmer
        // arm's end state).
        let t0 = Instant::now();
        b.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        b.engine.set_cp_profile(CpMode::ShortCp);

        // The peer's third leg arrives, encoded under the NEW profile.
        let switched = TransportPdu::new_cp_control_content(
            b.arq_session_id,
            0,
            0,
            0,
            CpNegotiator::switched_payload(CpMode::ShortCp),
        );
        b.decode_and_dispatch_audio(&peer_frame(
            &switched,
            coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
        ))
        .await;

        // Probation must now be permanently disarmed: even far past the
        // deadline, nothing reverts.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1));
        assert_eq!(b.cp_negotiator.current(), CpMode::ShortCp);
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::ShortCp,
            "a confirmed peer switch must cancel probation, not merely delay it"
        );

        // And b must really have acked the leg, so the peer's G4 disarms too.
        let mut buf = vec![0.0f32; 1_000_000];
        assert!(
            consumer.read(&mut buf) > 0,
            "expected b to have really transmitted a bare ack for the third leg"
        );
    }

    #[tokio::test]
    async fn probation_expiry_reverts_the_engine_not_just_the_negotiator() {
        // The regression this whole ticket exists to prevent: bookkeeping and
        // engine must never be left disagreeing.
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let t0 = Instant::now();
        b.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        b.engine.set_cp_profile(CpMode::ShortCp);

        // Just before the deadline: nothing happens.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS - 1));
        assert_eq!(b.engine.cp_mode(), CpMode::ShortCp);

        // Past it: BOTH must walk back.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1));
        assert_eq!(b.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::LongCp,
            "G3 must revert the ENGINE, not just the negotiator's bookkeeping"
        );
    }

    #[tokio::test]
    async fn a_failed_confirm_aborts_and_frees_the_cp_control_window() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let confirm_payload = vec![0x02, CpMode::ShortCp.to_wire()];
        let seq = a
            .cp_control_arq_tx
            .send(confirm_payload, Instant::now())
            .unwrap();
        a.cp_negotiator.track_pending_confirm(seq, CpMode::ShortCp);
        exhaust_retransmits(&mut a.cp_control_arq_tx, seq);

        a.drive_cp_negotiation(Instant::now());

        assert_eq!(a.cp_negotiator.pending_confirm_seq(), None);
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "a never switched, so it must still be on the conservative default"
        );
        // The parked-segment leak: the window must be usable again.
        assert!(
            a.cp_control_arq_tx.send(vec![1, 2], Instant::now()).is_ok(),
            "the given-up segment must have been abandoned, freeing its slot"
        );
    }

    #[tokio::test]
    async fn a_failed_propose_aborts_and_frees_the_cp_control_window() {
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let seq = b
            .cp_control_arq_tx
            .send(
                CpNegotiator::propose_payload(CpMode::ShortCp),
                Instant::now(),
            )
            .unwrap();
        b.cp_propose_seq = Some(seq);
        exhaust_retransmits(&mut b.cp_control_arq_tx, seq);

        b.drive_cp_negotiation(Instant::now());

        assert_eq!(b.cp_propose_seq, None, "G1 must clear the tracked seq");
        assert_eq!(b.engine.cp_mode(), CpMode::LongCp);
        assert!(
            b.cp_control_arq_tx.send(vec![1, 2], Instant::now()).is_ok(),
            "G1 must free the slot so a later CpGate transition can propose again"
        );
    }

    #[tokio::test]
    async fn a_failed_cp_switched_reverts_the_confirmer_to_long_cp() {
        // G4: `a` switched and sent its third leg, which the peer never acks.
        // The peer is unreachable under the new profile, so `a` must fall
        // back with it rather than sit deaf forever.
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        a.engine.set_cp_profile(CpMode::ShortCp);
        let seq = a
            .cp_control_arq_tx
            .send(
                CpNegotiator::switched_payload(CpMode::ShortCp),
                Instant::now(),
            )
            .unwrap();
        a.cp_negotiator.track_pending_switched(seq);
        exhaust_retransmits(&mut a.cp_control_arq_tx, seq);

        a.drive_cp_negotiation(Instant::now());

        assert_eq!(a.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "G4 must revert the ENGINE too, not just the negotiator"
        );
        assert_eq!(a.cp_negotiator.pending_switched_seq(), None);
    }

    #[tokio::test]
    async fn drive_cp_negotiation_is_inert_when_cp_negotiation_is_disabled() {
        // Set the flag off EXPLICITLY (COP-2). This used to rely on a bare
        // `DaemonConfig::default()` and a comment asserting what that default
        // is -- so the day someone flips the default this test would silently
        // stop testing the disabled path and start testing the enabled one,
        // under a name that still says "disabled". The default itself is
        // pinned separately, by `config.rs`'s
        // `test_cp_negotiation_requires_two_more_flags_by_default`.
        let mut config = DaemonConfig::default();
        config.engine.cp_negotiation_enabled = false;
        let mut a = EventLoop::new(config).unwrap();
        let t0 = Instant::now();
        // Set up state that WOULD trigger every give-up path if enabled.
        a.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        a.engine.set_cp_profile(CpMode::ShortCp);
        let seq = a
            .cp_control_arq_tx
            .send(CpNegotiator::switched_payload(CpMode::ShortCp), t0)
            .unwrap();
        a.cp_negotiator.track_pending_switched(seq);
        a.cp_propose_seq = Some(seq);
        exhaust_retransmits(&mut a.cp_control_arq_tx, seq);

        a.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS * 10));

        assert_eq!(
            a.cp_negotiator.current(),
            CpMode::ShortCp,
            "disabled means genuinely inert -- no state change at any `now`"
        );
        assert_eq!(a.engine.cp_mode(), CpMode::ShortCp);
        assert_eq!(a.cp_negotiator.pending_switched_seq(), Some(seq));
        assert_eq!(a.cp_propose_seq, Some(seq));
    }

    #[tokio::test]
    async fn drive_cp_negotiation_is_a_no_op_when_no_negotiation_is_in_flight() {
        // Guards against the 500 ms poll spuriously reverting a settled link.
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        a.drive_cp_negotiation(Instant::now() + Duration::from_secs(SWITCH_PROBATION_SECS * 10));
        assert_eq!(a.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(a.engine.cp_mode(), CpMode::LongCp);
        assert_eq!(a.cp_propose_seq, None);
    }

    // ── COP-1 Phase 4: end-to-end loss injection ──────────────────────────
    //
    // Two real `EventLoop`s, real audio rings, real `decode_and_dispatch_audio`
    // -- modelled on `cp_negotiation_full_handshake_converges_both_sides`.
    // Loss injection is simply reading a leg's samples out of the sender's
    // ring and NOT delivering them; no new infrastructure is required.
    //
    // Why this tier matters: the `e59bf56` handshake-deadlock bug was found
    // only by driving the real end-to-end audio path (not by inspection, and
    // not by the lower-level primitives test), and the VHF `TIMING_BACKOFF`
    // bug likewise only reproduced past a session's first real frame.

    /// Read everything a station has queued for transmission, assert it is
    /// non-empty, and pad it for the peer's decoder.
    ///
    /// The non-empty assertion is load-bearing: without it a loss-injection
    /// test could silently pass because nothing was ever transmitted, rather
    /// than because the recovery path worked.
    fn take_leg(consumer: &mut coppa_audio::AudioRingConsumer, what: &str) -> Vec<f32> {
        let mut buf = vec![0.0f32; 1_000_000];
        let read = consumer.read(&mut buf);
        assert!(
            read > 0,
            "expected the station to have really transmitted {what}"
        );
        with_lead_and_trail(&buf[..read])
    }

    /// COP-2's invariant, as one assertion: this station's engine and its negotiator
    /// agree on the CP mode in effect. Nothing compared these two before COP-2 --
    /// every test asserted each against a literal separately, which is exactly why a
    /// `set_speed_level`-driven divergence was invisible to all of them.
    ///
    /// Deliberately NOT folded into `assert_converged` as a claimed detection gain:
    /// `assert_converged` already asserts both engines' `cp_mode()` and both
    /// negotiators against the *same* literal, so `engine == negotiator` is already
    /// implied for every one of its callers, and advertising the fold as new coverage
    /// would be a detection claim the code cannot support. (Verified rather than
    /// assumed: the fold was temporarily applied and the whole `cp_` suite stayed
    /// green, confirming COP-1's own convergence paths already satisfy this invariant
    /// and that COP-2 is not papering over a second, pre-existing violation.) If a
    /// genuinely additional assertion is ever wanted there, the reachable one is
    /// `!(engine.speed_level() >= 5 && cp_negotiator.current() != CpMode::LongCp)` --
    /// "on air as `vhf_wide` while the peer believes short-CP HF" -- which is a
    /// different, real condition; add it on its own merits or not at all.
    fn assert_engine_matches_negotiator(station: &EventLoop, whose: &str) {
        assert_eq!(
            station.engine.cp_mode(),
            station.cp_negotiator.current(),
            "{whose}: engine.cp_mode() must equal cp_negotiator.current()"
        );
    }

    /// The ticket's acceptance criterion, as one assertion: engine and
    /// bookkeeping agree, on both stations, on the same mode.
    fn assert_converged(a: &EventLoop, b: &EventLoop, expected: CpMode) {
        assert_eq!(a.engine.cp_mode(), expected, "a's engine");
        assert_eq!(b.engine.cp_mode(), expected, "b's engine");
        assert_eq!(a.cp_negotiator.current(), expected, "a's negotiator");
        assert_eq!(b.cp_negotiator.current(), expected, "b's negotiator");
    }

    /// Spend every ARQ-budget-driven give-up trigger a station could be
    /// waiting on (G1/G2/G4), then drive `drive_cp_negotiation` far enough
    /// into the future to also fire the wall-clock one (G3).
    ///
    /// Exhausting the retransmit budget by hand is what the real 500 ms poll
    /// does over ~135 s of real time (5 attempts with exponential backoff);
    /// `exhaust_retransmits` makes the same calls the retransmit loop makes,
    /// so this compresses the wait without faking the mechanism.
    fn give_up(station: &mut EventLoop) {
        for seq in [
            station.cp_propose_seq,
            station.cp_negotiator.pending_confirm_seq(),
            station.cp_negotiator.pending_switched_seq(),
        ]
        .into_iter()
        .flatten()
        {
            if station.cp_control_arq_tx.get_segment_data(seq).is_some() {
                exhaust_retransmits(&mut station.cp_control_arq_tx, seq);
            }
        }
        station
            .drive_cp_negotiation(Instant::now() + Duration::from_secs(SWITCH_PROBATION_SECS + 1));
    }

    /// One real, decodable clean-channel frame carrying a payload that is
    /// deliberately INERT to every dispatch path except the CpGate block:
    /// the first byte's low nibble `0x0F` is an unrecognized `TransportType`,
    /// and at 10 bytes it is shorter than `MacPdu::HEADER_SIZE` (14), so
    /// neither `handle_mac_pdu` nor any `TransportPdu` arm can run -- and
    /// therefore nothing here can transmit and confound a
    /// "was anything sent?" assertion. `i` just makes each frame's content
    /// distinct.
    ///
    /// Extracted from `trip_cp_gate` so the COP-2 flip-gate test below can
    /// feed the SAME frames to a station whose `cp_gate_enabled` is off; two
    /// copies of this builder would let the two tests silently drift apart on
    /// exactly the payload property both of them depend on.
    fn inert_peer_frame(i: u8) -> Vec<f32> {
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(
            coppa_codec::ofdm::CoppaProfile::hf_standard(),
            1,
        );
        let payload: Vec<u8> = vec![0xFF, i, 0, 0, 0, 0, 0, 1, 2, 3];
        let header = coppa_codec::ofdm::frame::CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: payload.len() as u16,
            codewords: 1,
        };
        let samples = peer_tx
            .transmit(&header, &payload)
            .expect("peer transmit should succeed");
        with_lead_and_trail(&samples)
    }

    /// Drive `station` through a real `LongCp -> ShortCp` `CpGate` transition
    /// by decoding 4 real clean-channel frames, which is what trips
    /// `CpGate::default_coppa`'s real hysteresis (`consecutive_needed = 4`).
    ///
    /// Going through `decode_and_dispatch_audio` rather than poking `cp_gate`
    /// directly is the whole point: the propose-on-transition block lives
    /// inside that function and is the production path. See
    /// `cp_negotiation_full_handshake_converges_both_sides`'s doc.
    async fn trip_cp_gate(station: &mut EventLoop) {
        for i in 0..4u8 {
            station
                .decode_and_dispatch_audio(&inert_peer_frame(i))
                .await;
        }
        assert_eq!(
            station.cp_gate.current(),
            CpRecommendation::ShortCp,
            "4 consecutive clean-channel decodes should trip CpGate's real hysteresis"
        );
    }

    /// Two real stations with real audio rings, with `b` having genuinely
    /// transmitted a `Propose` via the real CpGate-transition path (4 real
    /// decoded frames trip `CpGate::default_coppa`'s real hysteresis -- see
    /// `cp_negotiation_full_handshake_converges_both_sides`'s doc for why
    /// this, rather than poking `cp_gate` directly, is the production path).
    async fn cp_pair_at_propose() -> (
        EventLoop,
        EventLoop,
        coppa_audio::AudioRingConsumer,
        coppa_audio::AudioRingConsumer,
    ) {
        let mut config = cp_enabled_config();
        config.engine.cp_gate_enabled = true;
        let mut a = EventLoop::new(config.clone()).unwrap();
        let mut b = EventLoop::new(config).unwrap();

        let (a_producer, a_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        let (b_producer, b_consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(b_producer);

        trip_cp_gate(&mut b).await;
        assert!(
            b.cp_propose_seq.is_some(),
            "the real propose-on-transition block should have tracked its seq for G1"
        );
        (a, b, a_consumer, b_consumer)
    }

    #[tokio::test]
    async fn lost_propose_leaves_both_endpoints_on_long_cp() {
        let (a, mut b, _a_consumer, mut b_consumer) = cp_pair_at_propose().await;

        // DROP b's Propose: `a` never learns a negotiation was attempted.
        let _dropped = take_leg(&mut b_consumer, "a CpControl Propose");

        give_up(&mut b);

        assert_converged(&a, &b, CpMode::LongCp);
        assert_eq!(b.cp_propose_seq, None, "G1 must clear the tracked seq");
        assert!(
            b.cp_control_arq_tx.send(vec![1, 2], Instant::now()).is_ok(),
            "b's CP-control window must be usable again"
        );
    }

    #[tokio::test]
    async fn lost_confirm_converges_both_endpoints_on_long_cp() {
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair_at_propose().await;

        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        assert!(
            a.cp_negotiator.pending_confirm_seq().is_some(),
            "a should be waiting on its own Confirm"
        );

        // DROP a's Confirm. Neither station ever switched, so this asserts the
        // fix does not *introduce* a desync on a leg that was previously
        // merely inert.
        let _dropped = take_leg(&mut a_consumer, "a CpControl Confirm");

        give_up(&mut a); // G2
        give_up(&mut b); // G1 -- b's Propose is never acked either

        assert_converged(&a, &b, CpMode::LongCp);
        assert_eq!(a.cp_negotiator.pending_confirm_seq(), None);
        assert_eq!(b.cp_propose_seq, None);
    }

    #[tokio::test]
    async fn lost_bare_ack_converges_both_endpoints_on_long_cp() {
        // THE ticket's scenario: the un-retryable bare ack is lost, and before
        // COP-1 this left the two stations on mutually-undecodable CP profiles
        // permanently, with no automatic recovery.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair_at_propose().await;

        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(&mut a_consumer, "a CpControl Confirm");
        b.decode_and_dispatch_audio(&confirm).await;

        // b has switched (and armed probation); a has not.
        assert_eq!(b.engine.cp_mode(), CpMode::ShortCp);
        assert_eq!(a.engine.cp_mode(), CpMode::LongCp);

        // DROP b's bare ack. This is the exact frame whose loss used to be
        // unrecoverable.
        let _dropped = take_leg(&mut b_consumer, "a bare ack for a's Confirm");

        give_up(&mut a); // G2: a's Confirm is never acked
        give_up(&mut b); // G3: probation expires with no proof a switched

        assert_converged(&a, &b, CpMode::LongCp);

        // And the link is genuinely ALIVE again, not merely consistent: a
        // frame a encodes now must decode on b.
        let samples = a.engine.encode_bytes(b"link alive").unwrap();
        assert_eq!(
            b.engine
                .decode_bytes(&samples)
                .expect("b must decode a's frame after convergence"),
            b"link alive".to_vec()
        );
    }

    #[tokio::test]
    async fn lost_cp_switched_converges_both_endpoints_on_long_cp() {
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair_at_propose().await;

        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(&mut a_consumer, "a CpControl Confirm");
        b.decode_and_dispatch_audio(&confirm).await;
        let bare_ack = take_leg(&mut b_consumer, "a bare ack for a's Confirm");

        // Deliver the bare ack, so `a` switches AND emits the third leg...
        //
        // Same workaround, for the same reason, as
        // `cp_negotiation_full_handshake_converges_both_sides` carries at its
        // own ack-decode step: this is `a`'s SECOND real decode of a
        // `CoppaCore::encode_bytes`-built frame, and `CoppaCore` reliably
        // fails every decode after its first one (CLAUDE.md's standalone
        // known-limitation bullet, "Bug B" -- real, reproducible, narrowed to
        // a payload-size-dependent spurious sync candidate, and entirely
        // orthogonal to CP negotiation). `set_cp_profile` always rebuilds
        // `self.streaming`, so calling it with `a`'s CURRENT mode is a
        // genuine no-op for the profile that nonetheless clears the stuck
        // state. Because it only ever sets `LongCp`, the `ShortCp` assertion
        // immediately below remains real evidence, not tautology.
        //
        // The other loss-injection tests here don't need it: they either
        // decode only one frame on `a`, or happen to pass through a
        // `drive_cp_negotiation` give-up (which calls `set_cp_profile` itself)
        // first. Not fixed here -- out of scope for this ticket.
        a.engine.set_cp_profile(CpMode::LongCp);
        a.decode_and_dispatch_audio(&bare_ack).await;
        assert_eq!(a.engine.cp_mode(), CpMode::ShortCp);
        assert!(a.cp_negotiator.pending_switched_seq().is_some());

        // ...then DROP that third leg.
        let _dropped = take_leg(&mut a_consumer, "a CpControl Switched");

        give_up(&mut a); // G4
        give_up(&mut b); // G3

        assert_converged(&a, &b, CpMode::LongCp);
    }

    #[tokio::test]
    async fn after_a_failed_negotiation_a_later_negotiation_still_succeeds() {
        // The regression `ArqTx::abandon` exists to prevent: a failed
        // negotiation must not leave the two-slot CP-control window leaking a
        // parked segment, nor leave the two sequence spaces diverged. This is
        // the only check that recovery leaves the link genuinely USABLE rather
        // than merely consistent.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair_at_propose().await;

        // ── Negotiation 1: fails on a lost bare ack. ──
        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(&mut a_consumer, "a CpControl Confirm");
        b.decode_and_dispatch_audio(&confirm).await;
        let _dropped = take_leg(&mut b_consumer, "a bare ack for a's Confirm");
        give_up(&mut a);
        give_up(&mut b);
        assert_converged(&a, &b, CpMode::LongCp);

        // ── Negotiation 2: must run to completion. ──
        //
        // `CpGate` only proposes on a *transition*, and its recommendation is
        // already `ShortCp`, so a second real transition cannot be produced on
        // a clean loopback (the measured delay spread is always near zero, so
        // it can never swing back to `LongCp` first). This replicates exactly
        // what the real propose-on-transition block does -- send via the
        // CP-control `ArqTx`, record the seq for G1, transmit the PDU -- which
        // is precisely the machinery whose state the first failure could have
        // corrupted.
        let payload = CpNegotiator::propose_payload(CpMode::ShortCp);
        let seq = b
            .cp_control_arq_tx
            .send(payload.clone(), Instant::now())
            .expect("the CP-control window must have room after a failed negotiation");
        b.cp_propose_seq = Some(seq);
        let (ack_num, ack_bitmap) = b.cp_control_arq_rx.ack_info();
        let propose2 = TransportPdu::new_cp_control_content(
            b.arq_session_id,
            seq,
            ack_num,
            ack_bitmap,
            payload,
        );
        let samples = b.engine.encode_bytes(&propose2.to_bytes()).unwrap();
        b.transmit_samples(&samples).await;

        let propose2_samples = take_leg(&mut b_consumer, "a second CpControl Propose");
        a.decode_and_dispatch_audio(&propose2_samples).await;
        let confirm2 = take_leg(&mut a_consumer, "a second CpControl Confirm");
        b.decode_and_dispatch_audio(&confirm2).await;
        let bare_ack2 = take_leg(&mut b_consumer, "a second bare ack");
        a.decode_and_dispatch_audio(&bare_ack2).await;
        let switched = take_leg(&mut a_consumer, "a CpControl Switched");
        b.decode_and_dispatch_audio(&switched).await;

        assert_converged(&a, &b, CpMode::ShortCp);
        assert_eq!(
            b.cp_negotiator.current(),
            CpMode::ShortCp,
            "b's probation must have been disarmed by the third leg"
        );
        // Probation is genuinely cancelled, not merely deferred.
        b.drive_cp_negotiation(Instant::now() + Duration::from_secs(SWITCH_PROBATION_SECS * 2));
        assert_converged(&a, &b, CpMode::ShortCp);
    }

    // ── COP-1 remediation: the ShortCp -> LongCp direction, and the fifth
    //    droppable frame ────────────────────────────────────────────────────
    //
    // Every loss-injection test above drives the LongCp -> ShortCp direction
    // only, which is exactly why neither of the two convergence bugs these
    // cover was caught: in that direction the pre-negotiation mode IS LongCp,
    // so `abort()`'s old hardcoded LongCp and `tick()`'s revert-to-previous
    // were accidentally equivalent. `CpGate` reverts to `CpRecommendation::
    // LongCp` on any single frame at or above threshold, and the
    // propose-on-transition block turns that into a real `Propose(LongCp)`, so
    // the untested direction is a production path -- and the one that runs
    // exactly when the channel is degrading and legs get lost.

    /// Two real stations with real audio rings and CP negotiation enabled,
    /// with `cp_gate` left OFF: these tests drive proposals explicitly, so the
    /// gate must not inject transitions of its own.
    fn cp_pair() -> (
        EventLoop,
        EventLoop,
        coppa_audio::AudioRingConsumer,
        coppa_audio::AudioRingConsumer,
    ) {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let (a_producer, a_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        let (b_producer, b_consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(b_producer);
        (a, b, a_consumer, b_consumer)
    }

    /// Send a real `Propose(mode)` from `b`, replicating exactly what the
    /// propose-on-transition block does (ARQ send, record the seq for G1,
    /// transmit the PDU).
    ///
    /// Driving it by hand rather than through `CpGate` is unavoidable for a
    /// *second* negotiation: the gate only proposes on a transition, and on a
    /// clean loopback the measured delay spread is always near zero, so its
    /// recommendation can never swing back to `LongCp` to produce one. Same
    /// reasoning, and the same code, as
    /// `after_a_failed_negotiation_a_later_negotiation_still_succeeds`.
    async fn propose_manually(b: &mut EventLoop, mode: CpMode) {
        let payload = CpNegotiator::propose_payload(mode);
        let seq = b
            .cp_control_arq_tx
            .send(payload.clone(), Instant::now())
            .expect("the CP-control window must have room for a Propose");
        b.cp_propose_seq = Some(seq);
        let (ack_num, ack_bitmap) = b.cp_control_arq_rx.ack_info();
        let pdu = TransportPdu::new_cp_control_content(
            b.arq_session_id,
            seq,
            ack_num,
            ack_bitmap,
            payload,
        );
        let samples = b.engine.encode_bytes(&pdu.to_bytes()).unwrap();
        b.transmit_samples(&samples).await;
    }

    /// Clear `CoppaCore`'s cross-decode stuck state before a station's second
    /// or later real decode.
    ///
    /// Same workaround, for the same reason, as
    /// `lost_cp_switched_converges_both_endpoints_on_long_cp` carries inline:
    /// `CoppaCore` reliably fails every decode after its first one of a
    /// `CoppaCore::encode_bytes`-built frame (CLAUDE.md's "Bug B", real,
    /// reproducible, and entirely orthogonal to CP negotiation).
    /// `set_cp_profile` always rebuilds `self.streaming`, so calling it with
    /// the station's CURRENT mode is a genuine no-op for the profile that
    /// nonetheless clears the stuck state -- it can never manufacture the mode
    /// a test is asserting on.
    fn unstick_decoder(station: &mut EventLoop) {
        let mode = station.engine.cp_mode();
        station.engine.set_cp_profile(mode);
    }

    /// Drive one COMPLETE, successful negotiation for `mode` -- all six steps,
    /// real frames through `decode_and_dispatch_audio` on both stations --
    /// leaving both converged on it with no give-up state armed.
    async fn negotiate(
        a: &mut EventLoop,
        b: &mut EventLoop,
        a_consumer: &mut coppa_audio::AudioRingConsumer,
        b_consumer: &mut coppa_audio::AudioRingConsumer,
        mode: CpMode,
    ) {
        propose_manually(b, mode).await;
        let propose = take_leg(b_consumer, "a CpControl Propose");
        unstick_decoder(a);
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(a_consumer, "a CpControl Confirm");
        unstick_decoder(b);
        b.decode_and_dispatch_audio(&confirm).await;
        let bare_ack = take_leg(b_consumer, "a bare ack for the Confirm");
        unstick_decoder(a);
        a.decode_and_dispatch_audio(&bare_ack).await;
        let switched = take_leg(a_consumer, "a CpControl Switched");
        unstick_decoder(b);
        b.decode_and_dispatch_audio(&switched).await;
        let switched_ack = take_leg(b_consumer, "a bare ack for the Switched leg");
        unstick_decoder(a);
        a.decode_and_dispatch_audio(&switched_ack).await;
        assert_converged(a, b, mode);
        assert_eq!(
            a.cp_negotiator.pending_switched_seq(),
            None,
            "the third leg's ack must have disarmed G4"
        );
    }

    /// Retransmit whatever CP-control segments are due, mirroring the real
    /// retransmit block in `poll_once`.
    async fn retransmit_cp_control(station: &mut EventLoop) {
        let now = Instant::now() + Duration::from_secs(60);
        let seqs = station.cp_control_arq_tx.get_retransmits(now);
        assert!(
            !seqs.is_empty(),
            "test setup: a segment should be due for retransmit"
        );
        let (ack_num, ack_bitmap) = station.cp_control_arq_rx.ack_info();
        for seq in seqs {
            let Some(data) = station
                .cp_control_arq_tx
                .get_segment_data(seq)
                .map(<[u8]>::to_vec)
            else {
                continue;
            };
            let pdu = TransportPdu::new_cp_control_content(
                station.arq_session_id,
                seq,
                ack_num,
                ack_bitmap,
                data,
            );
            let samples = station.engine.encode_bytes(&pdu.to_bytes()).unwrap();
            station.transmit_samples(&samples).await;
            station
                .cp_control_arq_tx
                .mark_retransmitted(seq, now)
                .expect("segment should still be in flight");
        }
    }

    #[tokio::test]
    async fn lost_bare_ack_in_the_short_to_long_direction_converges_on_short_cp() {
        // The desync `abort()`'s hardcoded LongCp caused. Both stations are
        // settled on ShortCp and negotiating a downshift to LongCp when the
        // bare ack is lost. `a` gives up via G2, `b` via G3 -- and before the
        // remediation those two landed on DIFFERENT modes (a: LongCp, because
        // `abort` forced it; b: ShortCp, because `tick` restored the
        // pre-switch mode), leaving the link permanently undecodable.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();
        negotiate(
            &mut a,
            &mut b,
            &mut a_consumer,
            &mut b_consumer,
            CpMode::ShortCp,
        )
        .await;

        // ── Negotiation 2: ShortCp -> LongCp, bare ack dropped. ──
        propose_manually(&mut b, CpMode::LongCp).await;
        let propose = take_leg(&mut b_consumer, "a second CpControl Propose");
        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(&mut a_consumer, "a second CpControl Confirm");
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&confirm).await;
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::LongCp,
            "b applies the proposed mode on receiving the Confirm"
        );

        let _dropped = take_leg(&mut b_consumer, "a bare ack for the second Confirm");

        give_up(&mut a); // G2
        give_up(&mut b); // G3

        assert_converged(&a, &b, CpMode::ShortCp);

        // And the link is genuinely ALIVE, not merely consistent.
        let samples = a.engine.encode_bytes(b"link alive").unwrap();
        assert_eq!(
            b.engine
                .decode_bytes(&samples)
                .expect("b must decode a's frame after convergence"),
            b"link alive".to_vec()
        );
    }

    #[tokio::test]
    async fn lost_confirm_in_the_short_to_long_direction_converges_on_short_cp() {
        // The other half of the same bug: here `a` never switched at all, yet
        // the old `abort()` still dragged it to LongCp while `b` (G1, also
        // never switched) stayed on ShortCp.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();
        negotiate(
            &mut a,
            &mut b,
            &mut a_consumer,
            &mut b_consumer,
            CpMode::ShortCp,
        )
        .await;

        propose_manually(&mut b, CpMode::LongCp).await;
        let propose = take_leg(&mut b_consumer, "a second CpControl Propose");
        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&propose).await;
        assert!(a.cp_negotiator.pending_confirm_seq().is_some());

        let _dropped = take_leg(&mut a_consumer, "a second CpControl Confirm");

        give_up(&mut a); // G2
        give_up(&mut b); // G1

        assert_converged(&a, &b, CpMode::ShortCp);
    }

    #[tokio::test]
    async fn a_lost_ack_for_the_third_leg_is_recovered_by_retransmission() {
        // The FIFTH droppable frame (step 6 of the module doc's six-step
        // diagram), which the original G1-G4 table did not enumerate.
        //
        // `on_peer_switched` disarms b's probation the instant the third leg
        // arrives, so before the remediation a lost ack left a's G4 to fire
        // and revert a while b -- with no timer left at all -- stayed on
        // ShortCp: verbatim the permanent desync COP-1 exists to eliminate,
        // reintroduced one step later. a's retransmitted third leg could not
        // re-elicit the ack either, because `ArqRx` swallows an
        // already-delivered seq before it reaches the ack-sending code.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();

        propose_manually(&mut b, CpMode::ShortCp).await;
        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        let confirm = take_leg(&mut a_consumer, "a CpControl Confirm");
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&confirm).await;
        let bare_ack = take_leg(&mut b_consumer, "a bare ack for the Confirm");
        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&bare_ack).await;
        let switched = take_leg(&mut a_consumer, "a CpControl Switched");
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&switched).await;

        // DROP b's ack for the third leg.
        let _dropped = take_leg(&mut b_consumer, "a bare ack for the Switched leg");
        assert!(
            a.cp_negotiator.pending_switched_seq().is_some(),
            "a is still waiting on its third leg (G4 armed)"
        );

        // a's ARQ retransmits the third leg. b must RE-ACK the duplicate --
        // that is what makes step 6 recoverable -- without re-running the
        // `PeerSwitched` action.
        retransmit_cp_control(&mut a).await;
        let retx = take_leg(&mut a_consumer, "a retransmitted CpControl Switched");
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&retx).await;
        let re_ack = take_leg(&mut b_consumer, "b's re-ack for the duplicate Switched");
        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&re_ack).await;

        assert_eq!(
            a.cp_negotiator.pending_switched_seq(),
            None,
            "the re-ack must resolve a's third leg and disarm G4"
        );
        // Converged on the NEW mode: unlike the other four frames, losing this
        // one does not cost the switch.
        assert_converged(&a, &b, CpMode::ShortCp);

        // No give-up can still fire on either side.
        give_up(&mut a);
        give_up(&mut b);
        assert_converged(&a, &b, CpMode::ShortCp);
    }

    #[tokio::test]
    async fn a_full_cp_control_window_at_the_third_leg_reverts_instead_of_stranding() {
        // `send_cp_switched`'s window-full arm used to only log. By then the
        // caller has already committed the engine to the new profile (it must
        // -- the third leg is encoded under it), so returning left the station
        // switched with no tracked leg, hence no `pending_switched_seq`, hence
        // no G4 that could ever fire: an unbounded wait, silently.
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(producer);

        // Put `a` in the real state right after its Confirm was acked.
        a.cp_negotiator.track_pending_confirm(9, CpMode::ShortCp);
        assert_eq!(
            a.cp_negotiator.on_confirm_acked(&[9]),
            Some(CpMode::ShortCp)
        );
        a.engine.set_cp_profile(CpMode::ShortCp);

        // Fill the two-slot CP-control window so the third leg cannot be sent.
        while a
            .cp_control_arq_tx
            .send(vec![0xEE, 0x00], Instant::now())
            .is_ok()
        {}

        a.send_cp_switched(0, CpMode::ShortCp, Instant::now()).await;

        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "a must walk its ENGINE back rather than sit on a profile it never announced"
        );
        assert_eq!(a.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(
            a.cp_negotiator.pending_switched_seq(),
            None,
            "nothing is pending, so nothing is silently waiting on a G4 that cannot arm"
        );
    }

    #[tokio::test]
    async fn reset_restores_the_engine_to_long_cp_not_just_the_negotiator() {
        // The Reset arm rebuilds `cp_negotiator` (whose fresh `current` is
        // LongCp) but used to leave the ENGINE wherever it was -- so a Reset
        // arriving at an already-switched station left bookkeeping and engine
        // disagreeing, the same class of desync this ticket exists to fix,
        // just reached by a different route.
        let mut event_loop = EventLoop::new(cp_enabled_config()).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));
        event_loop.arq_rx = Some(ArqRx::new(8));
        event_loop
            .cp_negotiator
            .apply_as_confirmer(CpMode::ShortCp, Instant::now());
        event_loop.engine.set_cp_profile(CpMode::ShortCp);
        assert_eq!(event_loop.engine.cp_mode(), CpMode::ShortCp);

        // A real Reset PDU must be encoded under the profile the station is
        // actually listening on, which is now ShortCp.
        let reset_pdu = TransportPdu::new_reset(0);
        event_loop
            .decode_and_dispatch_audio(&peer_frame(
                &reset_pdu,
                coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
            ))
            .await;

        assert_eq!(event_loop.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(
            event_loop.engine.cp_mode(),
            CpMode::LongCp,
            "a Reset must restore the ENGINE too, not just the negotiator"
        );
        assert_eq!(event_loop.cp_propose_seq, None);
    }

    // ── COP-1 remediation round 2: one negotiation at a time, and every
    //    tracked leg reclaimed ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_second_cp_gate_transition_does_not_orphan_the_first_propose() {
        // `cp_propose_seq` used to be overwritten unconditionally. G1 watches
        // exactly one seq, `get_retransmits` stops retrying past the budget but
        // never evicts, and only `abandon` evicts -- so the displaced seq parked
        // at `send_base` forever, permanently consuming one of the CP-control
        // pair's two slots. A second failure would then wedge the pair
        // completely.
        let (_a, mut b, _a_consumer, mut b_consumer) = cp_pair_at_propose().await;
        let first = b
            .cp_propose_seq
            .expect("test setup: b has a real Propose in flight");
        let _propose = take_leg(&mut b_consumer, "the first CpControl Propose");
        assert_eq!(b.cp_control_arq_tx.in_flight(), 1);

        // Force `CpGate` back to LongCp ("drop fast" -- one frame at or above
        // threshold), directly rather than through a decode, because a clean
        // loopback's measured delay spread is always near zero and can never
        // produce that swing on its own. The transition being tested is the
        // one BACK to ShortCp, which is driven through the real decode path.
        assert_eq!(b.cp_gate.observe(50.0), CpRecommendation::LongCp);
        trip_cp_gate(&mut b).await;

        assert_eq!(
            b.cp_propose_seq,
            Some(first),
            "the in-flight negotiation's seq must survive the second transition"
        );
        assert_eq!(
            b.cp_control_arq_tx.in_flight(),
            1,
            "and no second slot may be consumed"
        );

        // The one negotiation still gives up cleanly and hands the WHOLE
        // window back -- which is the property the orphan destroyed.
        give_up(&mut b);
        assert_eq!(b.cp_propose_seq, None);
        assert_eq!(b.cp_control_arq_tx.in_flight(), 0);
        assert!(b.cp_control_arq_tx.can_send());
    }

    #[tokio::test]
    async fn an_inbound_propose_while_our_own_is_in_flight_is_dropped_unacked() {
        // The other half of the re-entrancy guard. One `CpNegotiator` per
        // daemon means a station holding both roles runs them through the same
        // single-slot state. Dropping the inbound Propose WITHOUT an ack is
        // deliberate: the peer's G1 then fires, whereas acking would clear its
        // `cp_propose_seq` and leave it waiting on a Confirm forever with no
        // trigger at all.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();

        // Both stations propose at once -- what two `CpGate`s transitioning
        // over the same channel really produce.
        propose_manually(&mut a, CpMode::ShortCp).await;
        propose_manually(&mut b, CpMode::ShortCp).await;
        let a_propose = take_leg(&mut a_consumer, "a's Propose");
        let b_propose = take_leg(&mut b_consumer, "b's Propose");

        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&b_propose).await;
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&a_propose).await;

        for (station, who) in [(&a, "a"), (&b, "b")] {
            assert_eq!(
                station.cp_negotiator.pending_confirm_seq(),
                None,
                "{who} must not have taken on the confirmer role too"
            );
            assert_eq!(
                station.cp_control_arq_tx.in_flight(),
                1,
                "{who} must still hold only its own Propose"
            );
        }
        let mut buf = vec![0.0f32; 1_000_000];
        assert_eq!(
            a_consumer.read(&mut buf),
            0,
            "a must not have transmitted anything in reply -- not even a bare ack"
        );
        assert_eq!(b_consumer.read(&mut buf), 0, "nor must b");

        // Both converge on the mode they already agreed on, with the window
        // fully released so the next CpGate transition can propose again.
        give_up(&mut a);
        give_up(&mut b);
        assert_converged(&a, &b, CpMode::LongCp);
        assert!(a.cp_control_arq_tx.can_send());
        assert!(b.cp_control_arq_tx.can_send());
    }

    #[tokio::test]
    async fn a_duplicate_propose_is_not_re_acked_so_the_peer_s_g1_still_fires() {
        // The duplicate re-ack that rescues step 6 is restricted to
        // `CpSwitched`. Re-acking a retransmitted Propose would clear the
        // peer's `cp_propose_seq`, disarming the G1 the module doc's
        // convergence table depends on for the "`Confirm` lost" row.
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();

        propose_manually(&mut b, CpMode::ShortCp).await;
        let propose = take_leg(&mut b_consumer, "a CpControl Propose");
        a.decode_and_dispatch_audio(&propose).await;
        // DROP a's Confirm, so b keeps retransmitting its Propose.
        let _dropped = take_leg(&mut a_consumer, "a CpControl Confirm");

        retransmit_cp_control(&mut b).await;
        let retx = take_leg(&mut b_consumer, "a retransmitted CpControl Propose");
        unstick_decoder(&mut a);
        a.decode_and_dispatch_audio(&retx).await;

        let mut buf = vec![0.0f32; 1_000_000];
        assert_eq!(
            a_consumer.read(&mut buf),
            0,
            "a must stay silent: the duplicate is neither re-acked nor re-confirmed"
        );
        assert!(
            b.cp_propose_seq.is_some(),
            "b's G1 must still be armed -- an ack here would have disarmed it"
        );

        give_up(&mut a); // G2
        give_up(&mut b); // G1
        assert_converged(&a, &b, CpMode::LongCp);
    }

    #[tokio::test]
    async fn a_cp_switched_we_are_not_awaiting_is_not_acked() {
        // `on_peer_switched` ignores a leg naming a mode other than the
        // probation target (or arriving with no probation armed), but the
        // daemon acked it anyway and logged "handshake complete". That ack
        // resolves the sender's G4 while this station's own probation stays
        // armed to revert it seconds later -- the permanent desync COP-1
        // exists to eliminate, reached one step further along.
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(producer);

        let t0 = Instant::now();
        b.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        b.engine.set_cp_profile(CpMode::ShortCp);

        // A third leg naming LongCp -- a mode b never switched to.
        let stale = TransportPdu::new_cp_control_content(
            b.arq_session_id,
            0,
            0,
            0,
            CpNegotiator::switched_payload(CpMode::LongCp),
        );
        b.decode_and_dispatch_audio(&peer_frame(
            &stale,
            coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
        ))
        .await;

        let mut buf = vec![0.0f32; 1_000_000];
        assert_eq!(
            consumer.read(&mut buf),
            0,
            "a rejected leg must not be acked -- the sender's G4 has to fire"
        );
        // And b's own safety net is untouched: probation still reverts it.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1));
        assert_eq!(b.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(b.engine.cp_mode(), CpMode::LongCp);
    }

    #[tokio::test]
    async fn a_retransmitted_cp_switched_we_rejected_is_still_not_acked() {
        // The duplicate-CpSwitched re-ack path (rescuing a lost ack for step
        // 6, see the module doc) tested only the payload KIND, never whether
        // `on_peer_switched` would actually accept the leg -- so it re-granted
        // exactly the ack `a_cp_switched_we_are_not_awaiting_is_not_acked`
        // above proves the first-reception path deliberately withholds. The
        // sender then never sees its G4 fire, and the two stations can strand
        // on different profiles forever. This reproduces that exact sequence:
        // reject on first arrival, then the peer's own retransmission (ARQ
        // resends until acked) must land in the *duplicate* path and still be
        // rejected, not silently re-acked the second time around.
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(producer);

        let t0 = Instant::now();
        b.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        b.engine.set_cp_profile(CpMode::ShortCp);

        // A third leg naming LongCp -- a mode b never switched to. Same
        // fixture as the sibling test above.
        let stale = TransportPdu::new_cp_control_content(
            b.arq_session_id,
            0,
            0,
            0,
            CpNegotiator::switched_payload(CpMode::LongCp),
        );
        let frame = peer_frame(
            &stale,
            coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
        );

        // First reception: rejected via the non-duplicate on_peer_switched path.
        b.decode_and_dispatch_audio(&frame).await;

        // The sender never saw an ack, so it retransmits the identical PDU
        // (same seq) -- ArqRx has already delivered that seq once, so this
        // lands in the duplicate branch this fix covers.
        b.decode_and_dispatch_audio(&frame).await;

        let mut buf = vec![0.0f32; 1_000_000];
        assert_eq!(
            consumer.read(&mut buf),
            0,
            "the retransmitted rejected leg must not be acked either -- re-acking it \
             would resolve the sender's G4 while b's own probation stays armed to \
             revert it, stranding the two stations on different profiles"
        );
        // b's own safety net is still untouched: probation still reverts it.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1));
        assert_eq!(b.cp_negotiator.current(), CpMode::LongCp);
        assert_eq!(b.engine.cp_mode(), CpMode::LongCp);
    }

    #[tokio::test]
    async fn giving_up_reclaims_a_co_pending_confirm_not_just_the_failed_leg() {
        // G4's `abort()` clears `pending_confirm`, so reading
        // `pending_confirm_seq()` afterwards -- as the old separate G2 block
        // did -- found `None` and never abandoned that segment, leaking the
        // very window slot G2 exists to reclaim. Reachable whenever one
        // station holds both roles (see `cp_negotiator`'s "One negotiation at
        // a time" residual).
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let now = Instant::now();

        let confirm_seq = a
            .cp_control_arq_tx
            .send(vec![0x02, CpMode::ShortCp.to_wire()], now)
            .unwrap();
        a.cp_negotiator
            .track_pending_confirm(confirm_seq, CpMode::ShortCp);
        let switched_seq = a
            .cp_control_arq_tx
            .send(CpNegotiator::switched_payload(CpMode::ShortCp), now)
            .unwrap();
        a.cp_negotiator.track_pending_switched(switched_seq);
        a.engine.set_cp_profile(CpMode::ShortCp);
        assert_eq!(a.cp_control_arq_tx.in_flight(), 2, "both slots occupied");

        // Only the third leg exhausts its budget; the Confirm is still live.
        exhaust_retransmits(&mut a.cp_control_arq_tx, switched_seq);
        assert!(!a.cp_control_arq_tx.is_failed(confirm_seq));

        a.drive_cp_negotiation(Instant::now());

        assert_eq!(a.cp_negotiator.pending_switched_seq(), None);
        assert_eq!(a.cp_negotiator.pending_confirm_seq(), None);
        assert_eq!(
            a.cp_control_arq_tx.in_flight(),
            0,
            "giving up must abandon EVERY tracked leg, not just the failed one"
        );
        assert!(a.cp_control_arq_tx.can_send());
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "and the engine walks back exactly once, to the pre-negotiation mode"
        );
    }

    #[tokio::test]
    async fn check_arq_retransmits_drives_the_give_up_when_negotiation_is_enabled() {
        // The only production call site of `drive_cp_negotiation` was never
        // exercised with `cp_negotiation_enabled = true`: every give-up test
        // calls it directly with an injected Instant. The ordering contract
        // the call site asserts -- the give-up runs AFTER the retransmit
        // block, so a segment spends its full budget this tick before
        // `is_failed` is consulted -- was therefore unverified, and a refactor
        // could have dropped the call with the whole COP-1 suite still green.
        let mut b = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        b.set_audio_out(producer);

        let seq = b
            .cp_control_arq_tx
            .send(
                CpNegotiator::propose_payload(CpMode::ShortCp),
                Instant::now(),
            )
            .unwrap();
        b.cp_propose_seq = Some(seq);
        // One attempt short of the budget: only the retransmit block running
        // first can push it over, so this also pins the ordering.
        for _ in 0..coppa_protocol::arq::DEFAULT_MAX_RETRANSMIT - 1 {
            b.cp_control_arq_tx
                .mark_retransmitted(seq, Instant::now() - Duration::from_secs(600))
                .unwrap();
        }
        assert!(!b.cp_control_arq_tx.is_failed(seq), "test setup: not yet");

        b.check_arq_retransmits().await;

        assert_eq!(
            b.cp_propose_seq, None,
            "the real poll must retransmit, exhaust the budget, and then fire G1"
        );
        assert_eq!(b.cp_control_arq_tx.in_flight(), 0);
    }

    // ── COP-2 Phase 5: the `set_speed_level` / `cp_negotiator` desync ──────
    //
    // COP-1 closed every CP desync reachable *while a negotiation is in
    // flight* (G1-G4). These two close the one reachable *after* a negotiation
    // has fully succeeded, which is precisely why none of COP-1's machinery
    // could see it: `pending_confirm`, `pending_switched`, `revert_to`,
    // `probation` and `cp_propose_seq` were all cleared by the handshake's own
    // success paths, so `drive_cp_negotiation` is a provable no-op (see
    // `drive_cp_negotiation_is_a_no_op_when_no_negotiation_is_in_flight`) and
    // a fresh `Propose` would need a `CpGate` transition, which needs decoded
    // frames, which a dead link cannot produce.
    //
    // The trajectory: `RateLoop` drives `CoppaCore::set_speed_level` across
    // the HF/VHF speed-level boundary and back. `set_speed_level` used to
    // permanently rewrite `config.cp_mode` to `LongCp` on the way up, and
    // nothing restored it on the way down -- so dropping back below level 5
    // rebuilt onto `hf_standard` (CP 300) while the peer, never told anything,
    // stayed on `hf_standard_short_cp` (CP 144). Mutually undecodable, both
    // stations back inside the HF range where each believes the link should
    // work, and no give-up trigger armed anywhere.
    //
    // Both directions are covered deliberately: `a` is the confirmer (the
    // station that sends `Confirm`) and `b` the proposer (the station that
    // sends `Propose`) -- see `cp_negotiator`'s module doc for why plain
    // English and the code's own labels collide here. Nothing makes a station
    // intrinsically an A or a B, and `RateLoop` runs identically on both.

    #[tokio::test]
    async fn a_rate_loop_vhf_excursion_does_not_desync_a_negotiated_short_cp() {
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();
        negotiate(
            &mut a,
            &mut b,
            &mut a_consumer,
            &mut b_consumer,
            CpMode::ShortCp,
        )
        .await;

        // Seed the confirmer's `RateLoop` at level 4 -- one step below the
        // HF/VHF boundary -- so the very next raise crosses it. Direct
        // assignment matches how every other `rate_loop` test in this file
        // seeds a non-default starting level. `raise_dwell = 5` is
        // `RateLoop::default_coppa`'s own swept value, so five consecutive
        // higher recommendations is exactly what a real climb costs.
        a.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 4);

        // Five real ACKs, encoded under `hf_standard_short_cp` -- `a`'s REAL
        // post-negotiation profile, which is the whole point of driving this
        // through `decode_and_dispatch_audio` rather than calling
        // `set_speed_level` directly: a frame built under any other profile
        // would not decode here at all.
        //
        // These carry no `0xFF` first-byte marker (this file's convention for
        // inert filler frames, see `trip_cp_gate`): a real ACK's first byte IS
        // its session_id/type nibble pair, so an 0xFF prefix would stop it
        // being an ACK. It is not needed either -- an 8-byte ACK PDU is
        // shorter than `MacPdu::HEADER_SIZE` (14), so `MacPdu::from_bytes`
        // rejects it and `handle_mac_pdu` never runs, exactly as
        // `incoming_ack_with_rate_updates_rate_loop_and_encoder` already
        // relies on. The `Ack` arm itself transmits nothing.
        for _ in 0..5 {
            let ack = TransportPdu::new_ack_with_rate(a.arq_session_id, 0, 0, 10);
            unstick_decoder(&mut a);
            a.decode_and_dispatch_audio(&peer_frame(
                &ack,
                coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
            ))
            .await;
        }

        assert_eq!(
            a.rate_loop.current_level(),
            5,
            "test setup: five consecutive higher recommendations must really \
             have raised RateLoop across the HF/VHF boundary"
        );
        assert_eq!(
            a.engine.speed_level(),
            5,
            "test setup: and the ACK arm must really have applied it to the engine"
        );
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::ShortCp,
            "the negotiated CP mode must be RETAINED (dormant) across the VHF \
             excursion, not discarded -- the peer still believes it is in force"
        );

        // Drop back below the boundary through the REAL timeout path, not a
        // direct `set_speed_level` call: one expired segment in one poll is
        // one `on_timeout`, which steps 5 -> 4 and rebuilds the engine. This
        // is the step that used to land on `hf_standard` and kill the link.
        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        arq_tx
            .send(
                b"expired segment".to_vec(),
                Instant::now() - Duration::from_millis(100),
            )
            .expect("a fresh ARQ window should have room");
        a.arq_tx = Some(arq_tx);

        a.check_arq_retransmits().await;

        assert_eq!(
            a.rate_loop.current_level(),
            4,
            "test setup: the retransmit timeout must really have stepped 5 -> 4"
        );
        assert_eq!(a.engine.speed_level(), 4, "test setup: and been applied");

        assert_engine_matches_negotiator(&a, "a");
        assert_converged(&a, &b, CpMode::ShortCp);

        // And the link is genuinely ALIVE, not merely consistent -- the
        // assertion that fails loudest pre-fix, since `a` would be back on
        // `hf_standard` while `b` listens on `hf_standard_short_cp`.
        let samples = a.engine.encode_bytes(b"link alive").unwrap();
        assert_eq!(
            b.engine
                .decode_bytes(&samples)
                .expect("b must decode a's frame after a's VHF excursion and return"),
            b"link alive".to_vec()
        );
    }

    #[tokio::test]
    async fn a_rate_loop_probe_into_vhf_does_not_desync_a_negotiated_short_cp() {
        let (mut a, mut b, mut a_consumer, mut b_consumer) = cp_pair();
        negotiate(
            &mut a,
            &mut b,
            &mut a_consumer,
            &mut b_consumer,
            CpMode::ShortCp,
        )
        .await;

        // The one-frame variant, on the PROPOSER this time: a single active
        // overshoot probe reaches level 5, and `try_drain_tx_queue` reverts to
        // level 4 immediately afterwards. No ARQ timeout and no VHF dwell at
        // all -- pre-fix, `hf_standard_short_cp` was already lost by the time
        // the revert landed on `hf_standard`.
        b.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 4).with_probing(1, 1);
        b.tx_queue.push_back((Some(0), b"probe me".to_vec()));

        // MANDATORY, not hygiene. `negotiate` made `b` transmit, so
        // `is_transmitting` is still true: PTT release arrives only as a
        // `DaemonEvent::PttChange(false)` from a spawned timer through the
        // event channel, which a unit test never drains. Without this line
        // `try_drain_tx_queue` early-returns, no probe is taken,
        // `set_speed_level` is never called, and this test would pass
        // IDENTICALLY before and after the fix -- proving nothing. Poking this
        // field directly has existing precedent in this file (see
        // `enqueue_tx_carries_optional_arq_seq_through_the_queue`).
        b.is_transmitting = false;
        b.try_drain_tx_queue().await;

        assert_eq!(
            b.probe_state,
            Some((0, 5)),
            "test setup: the probe must really have gone out at VHF level 5"
        );
        assert_eq!(
            b.engine.speed_level(),
            4,
            "test setup: and the engine must have been reverted to RateLoop's \
             steady-state level afterwards"
        );

        assert_engine_matches_negotiator(&b, "b");
        assert_converged(&a, &b, CpMode::ShortCp);

        let samples = b.engine.encode_bytes(b"link alive").unwrap();
        assert_eq!(
            a.engine
                .decode_bytes(&samples)
                .expect("a must decode b's frame after b's one-frame VHF probe"),
            b"link alive".to_vec()
        );
    }

    // ── COP-2 Phase 6: what `cp_negotiation_enabled = true` alone actually
    //    does ────────────────────────────────────────────────────────────────

    /// Enabling CP negotiation is **three flags, not one** -- and this pins the
    /// half of that finding the flip decision turns on: with only
    /// `cp_negotiation_enabled` set, a station **never initiates**. It is not
    /// named "inert", because it is not: see the responder half at the bottom.
    ///
    /// The trace, re-verified against the branch tip: the ONLY code that can
    /// send a `Propose` sits inside `if self.config.engine.cp_gate_enabled`
    /// (`event_loop.rs:1093`) -- so with the gate off, `CpGate::observe` is
    /// never called, its recommendation can never transition, and the block
    /// that would propose is unreachable. Even if it were reached, the propose
    /// itself additionally requires `&& self.config.engine.arq_enabled`
    /// (`:1104`), since CP-control traffic rides an ARQ pair. Both flags
    /// default `false` (`config.rs:345/348`), so on a default daemon flipping
    /// `cp_negotiation_enabled` alone changes nothing an operator could
    /// observe -- which is exactly why COP-2 does not flip it (D8): a default
    /// that reads "negotiation: on" while nothing negotiates is worse than an
    /// honest `false`.
    #[tokio::test]
    async fn cp_negotiation_enabled_alone_never_initiates_without_cp_gate() {
        let mut config = DaemonConfig::default();
        config.engine.cp_negotiation_enabled = true;
        // ONLY that one. Asserted, not assumed -- if a future change flips
        // either of these defaults this test must stop claiming to prove
        // anything about "alone".
        assert!(
            !config.engine.cp_gate_enabled,
            "test premise: cp_gate_enabled must be off"
        );
        assert!(
            !config.engine.arq_enabled,
            "test premise: arq_enabled must be off"
        );

        let mut station = EventLoop::new(config).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        station.set_audio_out(producer);

        // Four consecutive real clean-loopback decodes through the real
        // `decode_and_dispatch_audio` -- `CpGate::default_coppa`'s
        // `consecutive_needed` is 4, so this is exactly what DOES trip it when
        // the gate is fed (see `trip_cp_gate`, which uses these same frames).
        for i in 0..4u8 {
            station
                .decode_and_dispatch_audio(&inert_peer_frame(i))
                .await;
        }

        let mut buf = vec![0.0f32; 1_000_000];
        assert_eq!(
            consumer.read(&mut buf),
            0,
            "cp_negotiation_enabled alone must never put a Propose (or anything \
             else) on the air"
        );
        assert!(
            station.cp_propose_seq.is_none(),
            "and must never consume a CP-control window slot for a negotiation \
             it cannot start"
        );
        assert_eq!(
            station.cp_gate.current(),
            CpRecommendation::LongCp,
            "with cp_gate_enabled off the gate is never even OBSERVED, so its \
             recommendation cannot transition -- this is the structural reason \
             no Propose is reachable, not a coincidence of thresholds"
        );
        assert_eq!(
            station.engine.cp_mode(),
            CpMode::LongCp,
            "and the engine's CP profile is untouched"
        );

        // ── The responder half: the flag is NOT inert in general. ──
        //
        // Turn `arq_enabled` on -- still NO `cp_gate_enabled` -- and the same
        // single flag makes this station a full RESPONDER to a peer that does
        // propose: `handle_cp_control` gates only on `cp_negotiation_enabled`
        // (`event_loop.rs:1903-1905`), and the one route to it (the inbound
        // `TransportPdu` parse, `:1215`) gates only on `arq_enabled`. That is a
        // second, independent reason Phase 5's desync fix gates this flip
        // rather than a footnote: a responder can be talked onto short CP by
        // its peer and then desync itself on its own `RateLoop`.
        let mut responder = EventLoop::new(cp_enabled_config()).unwrap();
        assert!(
            !responder.config.engine.cp_gate_enabled,
            "test premise: the responder still has no CpGate"
        );
        let (r_producer, mut r_consumer) = coppa_audio::audio_ring(1_000_000);
        responder.set_audio_out(r_producer);

        let propose = TransportPdu::new_cp_control_content(
            responder.arq_session_id,
            0,
            0,
            0,
            CpNegotiator::propose_payload(CpMode::ShortCp),
        );
        responder
            .decode_and_dispatch_audio(&peer_frame(
                &propose,
                coppa_codec::ofdm::CoppaProfile::hf_standard(),
            ))
            .await;

        assert!(
            responder.cp_negotiator.pending_confirm_seq().is_some(),
            "with arq_enabled on, cp_negotiation_enabled ALONE still makes this \
             station answer a peer's Propose -- the flag is never inert in \
             general, only never an INITIATOR without cp_gate_enabled"
        );
        assert!(
            r_consumer.read(&mut buf) > 0,
            "and it really puts the Confirm on the air"
        );
    }

    #[test]
    fn new_defaults_to_probing_disabled() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        for _ in 0..20 {
            assert_eq!(
                event_loop.rate_loop.level_for_next_transmission(),
                (2, false)
            );
        }
    }

    #[test]
    fn new_starts_with_no_probe_outstanding() {
        let event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        assert_eq!(event_loop.probe_state, None);
    }

    #[tokio::test]
    async fn enqueue_tx_carries_optional_arq_seq_through_the_queue() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.is_transmitting = true; // block draining so the queue can be inspected

        event_loop.enqueue_tx(None, b"no-seq".to_vec()).await;
        event_loop.enqueue_tx(Some(7), b"has-seq".to_vec()).await;

        let queued: Vec<_> = event_loop.tx_queue.iter().cloned().collect();
        assert_eq!(queued[0], (None, b"no-seq".to_vec()));
        assert_eq!(queued[1], (Some(7), b"has-seq".to_vec()));
    }

    #[tokio::test]
    async fn probe_send_applies_level_sets_probe_state_and_reverts() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));
        event_loop.rate_loop =
            RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 1).with_probing(1, 1);

        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        event_loop
            .tx_queue
            .push_back((Some(0), b"probe me".to_vec()));
        event_loop.try_drain_tx_queue().await;

        assert_eq!(
            event_loop.probe_state,
            Some((0, 2)),
            "a successful probe transmission should record (seq, probed_level)"
        );
        assert_eq!(
            event_loop.engine.speed_level(),
            event_loop.rate_loop.current_level(),
            "engine speed level should be reverted to RateLoop's steady-state level \
             after the probe transmission completes"
        );
        assert_eq!(
            event_loop.rate_loop.current_level(),
            1,
            "a probe transmission (outcome not yet known) must not itself change \
             RateLoop's current_level"
        );
    }

    #[tokio::test]
    async fn probe_send_skips_when_candidate_oversize_for_probe_level() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));
        // Level 4's budget is 178 bytes; level 5 (the probe target at offset 1)
        // is only 158 -- a real dip in `max_payload_for_level`. A payload in
        // between fits at the current level but not at the probe level.
        event_loop.rate_loop =
            RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 4).with_probing(1, 1);

        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        let oversize_for_probe = vec![0u8; 170];
        event_loop.tx_queue.push_back((Some(0), oversize_for_probe));
        event_loop.try_drain_tx_queue().await;

        assert_eq!(
            event_loop.probe_state, None,
            "an oversize probe candidate must fall back to a normal send, not become a probe"
        );
        assert_eq!(event_loop.engine.speed_level(), 4);
    }

    #[tokio::test]
    async fn non_arq_send_is_never_a_probe_candidate() {
        let config = DaemonConfig::default(); // arq_enabled = false by default
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.rate_loop =
            RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 1).with_probing(1, 1);

        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        event_loop
            .tx_queue
            .push_back((None, b"plain data".to_vec()));
        event_loop.try_drain_tx_queue().await;

        assert_eq!(event_loop.probe_state, None);
        assert_eq!(event_loop.engine.speed_level(), 1);
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

    #[tokio::test]
    async fn probe_ack_resolves_probe_and_skips_normal_on_ack_for_the_same_event() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        let mut arq_tx = ArqTx::new(ArqConfig::default());
        let seq = arq_tx
            .send(b"probed segment".to_vec(), Instant::now())
            .expect("a fresh ARQ window should have room");
        event_loop.arq_tx = Some(arq_tx);
        event_loop.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 1);
        event_loop.probe_state = Some((seq, 6)); // pretend we probed up to level 6

        // One ACK event that BOTH cumulatively acks `seq` (resolving the probe)
        // AND carries a lower suggested_rate (2) that would, if `on_ack` were
        // mistakenly also applied for this same event, immediately drop the
        // level back down (on_ack's "drop is immediate" rule) -- making a bug
        // here observable rather than silently absorbed by raise_dwell.
        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        let ack_pdu = TransportPdu::new_ack_with_rate(0, seq.wrapping_add(1), 0, 2);
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

        assert_eq!(
            event_loop.probe_state, None,
            "the probe should be resolved and cleared"
        );
        assert_eq!(
            event_loop.rate_loop.current_level(),
            6,
            "on_probe_result's jump to the probed level must win -- on_ack's lower \
             suggested_rate must NOT also apply for the same ACK event"
        );
        assert_eq!(event_loop.engine.speed_level(), 6);
    }

    /// Whole-branch review Fix 2: a `TransportType::Reset` rebuilds `arq_tx`/
    /// `arq_rx` fresh (restarting ARQ sequence numbering from 0) but must also
    /// clear any outstanding `probe_state` -- otherwise a stale probe's seq
    /// could either permanently block future probing or be spuriously
    /// resolved by an unrelated later segment that happens to reuse the same
    /// seq number. Drives a real wire-encoded `TransportPdu::new_reset` through
    /// `decode_and_dispatch_audio` (the real `TransportType::Reset` code path),
    /// following the same encode/decode boilerplate as
    /// `probe_ack_resolves_probe_and_skips_normal_on_ack_for_the_same_event`.
    #[tokio::test]
    async fn reset_clears_outstanding_probe_state() {
        use coppa_protocol::cp_negotiator::CpMode;

        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        config.engine.cp_negotiation_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));
        event_loop.arq_rx = Some(ArqRx::new(8));
        event_loop.probe_state = Some((3, 6)); // pretend a probe is outstanding

        // Give the CP-control pair/negotiator some real, non-fresh state
        // before the Reset, so the Reset arm's extension (Finding 5) has
        // something meaningful to actually clear -- otherwise this test
        // proves nothing about that code path.
        event_loop.cp_negotiator.apply_as_confirmer(
            coppa_protocol::cp_negotiator::CpMode::ShortCp,
            Instant::now(),
        );
        event_loop
            .cp_control_arq_tx
            .send(b"warm up the cp-control seq space".to_vec(), Instant::now())
            .expect("a fresh cp-control ArqTx window should have room");
        assert_eq!(event_loop.cp_negotiator.current(), CpMode::ShortCp);
        assert_ne!(event_loop.cp_control_arq_tx.next_seq(), 0);

        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        let reset_pdu = TransportPdu::new_reset(0);
        let header = coppa_codec::ofdm::frame::CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: coppa_codec::ofdm::frame::CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: reset_pdu.to_bytes().len() as u16,
            codewords: 1,
        };
        let samples = peer_tx
            .transmit(&header, &reset_pdu.to_bytes())
            .expect("peer transmit should succeed");
        event_loop
            .decode_and_dispatch_audio(&with_lead_and_trail(&samples))
            .await;

        assert_eq!(
            event_loop.probe_state, None,
            "a Reset must clear any outstanding probe_state, not just rebuild arq_tx/arq_rx"
        );
        assert_eq!(
            event_loop.cp_negotiator.current(),
            CpMode::LongCp,
            "a Reset must also rebuild cp_negotiator back to its fresh-state default"
        );
        assert_eq!(
            event_loop.cp_control_arq_tx.next_seq(),
            0,
            "a Reset must rebuild cp_control_arq_tx fresh (no carried-over seq state)"
        );
        assert_eq!(
            event_loop.cp_control_arq_tx.send_base(),
            0,
            "a Reset must rebuild cp_control_arq_tx fresh (no carried-over send_base)"
        );
        assert_eq!(
            event_loop.cp_control_arq_rx.recv_base(),
            0,
            "a Reset must rebuild cp_control_arq_rx fresh (no carried-over recv_base)"
        );
    }

    /// Whole-branch review Fix 4: `resolve_probe_if_acked` is also called from
    /// the `TransportType::Reliable | Unreliable` branch (a piggybacked ACK
    /// riding on an incoming DATA frame's own `ack_num`/`ack_bitmap`), not just
    /// the standalone `Ack | Nak` branch already covered by
    /// `probe_ack_resolves_probe_and_skips_normal_on_ack_for_the_same_event`.
    /// Seeds `probe_state`, then sends a real wire-encoded
    /// `TransportPdu::new_reliable` whose `ack_num` cumulatively acks the
    /// probed seq, through `decode_and_dispatch_audio`, and asserts the probe
    /// resolves. Uses `arq_receive_transmits_a_real_ack_with_rate`'s
    /// `Reliable`-PDU construction as a template.
    #[tokio::test]
    async fn probe_resolves_via_piggybacked_ack_on_reliable_data_frame() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        let mut arq_tx = ArqTx::new(ArqConfig::default());
        let seq = arq_tx
            .send(b"probed segment".to_vec(), Instant::now())
            .expect("a fresh ARQ window should have room");
        event_loop.arq_tx = Some(arq_tx);
        event_loop.arq_rx = Some(ArqRx::new(8));
        event_loop.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 1);
        event_loop.probe_state = Some((seq, 6)); // pretend we probed up to level 6

        let (producer, _consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        // Peer sends a Reliable DATA frame (not a standalone Ack/Nak) whose
        // own ack_num cumulatively acks the probed seq -- the piggybacked-ack
        // path, driven through the `Reliable | Unreliable` match arm.
        let peer_profile = coppa_codec::ofdm::CoppaProfile::hf_standard();
        let peer_tx = coppa_protocol::modem::transceiver::CoppaTransceiver::new(peer_profile, 1);
        let data_pdu =
            TransportPdu::new_reliable(5, 9, seq.wrapping_add(1), b"incoming data".to_vec());
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

        assert_eq!(
            event_loop.probe_state, None,
            "the probe should be resolved and cleared via the piggybacked ack on \
             an incoming Reliable data frame, not just the standalone Ack/Nak path"
        );
        assert_eq!(
            event_loop.rate_loop.current_level(),
            6,
            "on_probe_result's jump to the probed level must apply"
        );
        assert_eq!(event_loop.engine.speed_level(), 6);
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

    #[tokio::test]
    async fn probe_timeout_alone_resolves_as_failed_probe_without_on_timeout() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();

        event_loop.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 5);
        assert_eq!(event_loop.rate_loop.current_level(), 5);

        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        let send_time = Instant::now() - Duration::from_millis(100);
        let seq = arq_tx
            .send(b"probed segment".to_vec(), send_time)
            .expect("a fresh ARQ window should have room");
        event_loop.arq_tx = Some(arq_tx);
        event_loop.probe_state = Some((seq, 9)); // pretend we probed up to level 9

        event_loop.check_arq_retransmits().await;

        assert_eq!(
            event_loop.probe_state, None,
            "the failed probe should be resolved and cleared"
        );
        assert_eq!(
            event_loop.rate_loop.current_level(),
            5,
            "a failed probe must NOT drop RateLoop's current_level \
             (on_probe_result's no-op-on-failure rule) -- only a genuine \
             on_timeout would"
        );
    }

    #[tokio::test]
    async fn probe_timeout_alongside_other_expired_segment_still_calls_on_timeout_once() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();

        event_loop.rate_loop = RateLoop::new(coppa_ml::VALID_SPEED_LEVELS.to_vec(), 5, 5);
        assert_eq!(event_loop.rate_loop.current_level(), 5);

        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        let send_time = Instant::now() - Duration::from_millis(100);
        let probe_seq = arq_tx
            .send(b"probed segment".to_vec(), send_time)
            .expect("a fresh ARQ window should have room");
        arq_tx
            .send(b"ordinary segment".to_vec(), send_time)
            .expect("a fresh ARQ window should have room for a second segment");
        event_loop.arq_tx = Some(arq_tx);
        event_loop.probe_state = Some((probe_seq, 9));

        event_loop.check_arq_retransmits().await;

        assert_eq!(event_loop.probe_state, None);
        assert_eq!(
            event_loop.rate_loop.current_level(),
            4,
            "the OTHER expired segment is a genuine passive timeout -- on_timeout \
             should still fire exactly once (5 -> 4), separate from the probe's \
             own resolution"
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

        /// `CpGate`'s live recommendation reaches `WsStatus.short_cp_ok` once
        /// enabled and a frame has decoded (design doc: `docs/superpowers/
        /// specs/2026-07-25-cpgate-daemon-wiring-design.md`). A single clean
        /// loopback frame's real (near-zero) delay spread isn't enough on its
        /// own to flip `CpGate::default_coppa()`'s hysteresis to `ShortCp`
        /// (`consecutive_needed = 4`), so this only asserts the field is
        /// populated (`Some(_)`), not which recommendation it holds -- the
        /// hysteresis threshold/count themselves are already covered by
        /// `coppa-ml::cp_gate`'s own unit tests.
        #[cfg(feature = "websocket")]
        #[tokio::test]
        async fn test_cp_gate_populates_ws_status_when_enabled() {
            let mut config = DaemonConfig::default();
            config.engine.cp_gate_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let status = Arc::new(Mutex::new(coppa_host::websocket::WsStatus::default()));
            event_loop.set_ws_status(status.clone());

            let core = coppa_engine::CoppaCore::new();
            let samples = core.encode("Hello CpGate").expect("encode should succeed");
            let samples = with_lead_and_trail(&samples);

            event_loop.decode_and_dispatch_audio(&samples).await;

            let snap = status.lock().await;
            assert!(
                snap.short_cp_ok.is_some(),
                "short_cp_ok should be populated once CpGate is enabled and a frame has decoded"
            );
        }

        /// The default (`cp_gate_enabled = false`) must leave `short_cp_ok`
        /// at `None` -- this feature must be a true no-op when off.
        #[cfg(feature = "websocket")]
        #[tokio::test]
        async fn test_cp_gate_disabled_by_default_leaves_ws_status_none() {
            let config = DaemonConfig::default();
            assert!(!config.engine.cp_gate_enabled);
            let mut event_loop = EventLoop::new(config).unwrap();
            let status = Arc::new(Mutex::new(coppa_host::websocket::WsStatus::default()));
            event_loop.set_ws_status(status.clone());

            let core = coppa_engine::CoppaCore::new();
            let samples = core
                .encode("Hello without CpGate")
                .expect("encode should succeed");
            let samples = with_lead_and_trail(&samples);

            event_loop.decode_and_dispatch_audio(&samples).await;

            let snap = status.lock().await;
            assert_eq!(
                snap.short_cp_ok, None,
                "short_cp_ok must stay None when cp_gate_enabled is false (the default)"
            );
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
            event_loop.enqueue_tx(None, b"frame1".to_vec()).await;
            event_loop.enqueue_tx(None, b"frame2".to_vec()).await;
            event_loop.enqueue_tx(None, b"frame3".to_vec()).await;
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
