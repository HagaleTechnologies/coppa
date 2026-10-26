//! Main event loop for the Coppa daemon.
//!
//! Uses `tokio::select!` to multiplex audio, host, and radio events.

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_engine::{CoppaCore, OfdmBand};
use coppa_host::vara::{VaraCommand, VaraResponse};
use coppa_host::HostEvent;
use coppa_ml::CpRecommendation;
use coppa_ml::{BusyGate, CpGate, RateLoop};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::cp_negotiator::{ContentAction, CpMode, CpNegotiator};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu, StationIdPayload};
use coppa_protocol::modem::max_payload_for_level;
use coppa_protocol::session::{LinkCapabilities, SessionManager, SessionState};
use coppa_protocol::transport::{TransportPdu, TransportType, TRANSPORT_HEADER_SIZE};
use coppa_radio::band::{self, FrequencyConfig, FrequencyEvidence, GateRefusal};
use coppa_radio::{NullPtt, PttBackend, PttControl, PttState};
use rand::RngExt;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};

use crate::audio_output::{
    wait_for_discard, wait_for_playout, IntervalProgress, OutputFeedback, PlayoutWatch, StallClock,
    TxOutput, PROGRESS_TICK,
};
use crate::config::DaemonConfig;
use crate::level_meter::{self, LevelReading, LevelState, RxLevelMeter};
use crate::tx_chunker::chunk_to_fit;

/// Map of connected VARA command-port clients' response senders
/// (`VaraServer::response_senders()`), used to broadcast `VaraResponse` telemetry.
type VaraResponseSenders = Arc<Mutex<HashMap<u32, mpsc::Sender<VaraResponse>>>>;

/// How often every command-port client hears `IAMALIVE`. VARA sends it every
/// 60 s; Pat closes the modem after 2 minutes without a line, and vARIM after
/// 300 s.
const VARA_KEEPALIVE_PERIOD: Duration = Duration::from_secs(60);

/// COP-37: what every transmit path logs, at `ERROR`, when it refuses
/// because no valid local callsign is in force (`local_callsign` is `None`).
const NO_CALLSIGN_TX_REFUSAL: &str = "Transmission refused: no valid local callsign; \
     set [engine] callsign or MYCALL before transmitting";

/// How often `wait_for_rx_turnaround` drains the input ring: `run()`'s
/// `audio_poll` cadence, so the ring never fills during the hold.
const TURNAROUND_POLL: Duration = Duration::from_millis(20);

/// Smallest one-codeword payload budget on the whole speed ladder (COP-41,
/// D4), derived rather than hardcoded: `min(max_payload_for_level(l))` for
/// `l` in 1..=9. ARQ chunks are sized against THIS, not the current level,
/// because `check_arq_retransmits`/`try_drain_tx_queue` re-encode a segment
/// at whatever level is current *later* -- `RateLoop`'s drop-fast hysteresis
/// makes mid-stream rate drops ordinary, and a segment that no longer fits
/// is permanently stuck (COP-30: spent segments are never evicted). Today
/// this is level 1's 56 bytes (`k_used` 486/8 - 4); the derivation keeps
/// that true by construction if the ladder ever changes.
fn min_level_budget() -> usize {
    (1..=9)
        .filter_map(max_payload_for_level)
        .min()
        .expect("every wire level 1..=9 has a payload budget (speed_levels.rs)")
}

/// One chunked piece of an established-session host write (COP-41): the
/// fully-framed `MacPdu` bytes ready for `encode_bytes`, plus the host
/// payload bytes it carries -- `BUFFER` counts the unit the host wrote, not
/// framing (COP-30).
struct SessionTxChunk {
    /// The `SessionManager` slot the chunk was framed for. Slots are reused,
    /// so a session's end must discard its chunks (`discard_session_tx`)
    /// rather than leave them for whichever session takes the slot next.
    session_id: u8,
    pdu_bytes: Vec<u8>,
    host_len: usize,
}

/// The reply a VARA command-port client gets for one command, decided before
/// the command takes effect. `WRONG` is only for lines coppa does not
/// recognise or cannot apply: Pat's dial waits only for
/// `CONNECTED`/`DISCONNECTED`, so `WRONG` to a well-formed `CONNECT` would
/// hang it, and Pat's `Version()` waits only for `VERSION`/`WRONG`.
///
/// The reply rule, one row per command the parser can produce:
///
/// - `MYCALL <call>` valid → `OK` (the daemon applies it as the runtime
///   callsign); invalid or missing → `WRONG`.
/// - `LISTEN ON`/`LISTEN OFF` → `OK`; any other argument → `WRONG`.
/// - `TUNE [seconds]` (coppa extension) → `OK`, even when the tone itself is
///   then refused for want of a callsign (`transmit_samples` logs why).
/// - `CONNECT <src> <dst>` → `OK` (then the eventual `CONNECTED`/`DISCONNECTED`
///   outcome); fewer than two callsigns → `WRONG`.
/// - `DISCONNECT` → `OK` (then `DISCONNECTED` if a session existed).
/// - `VERSION` → a `VERSION Coppa <pkg version>` line, never `OK`.
/// - `COMPRESSION …`, `BW500`, `BW2300`, `BW2750`, `ABORT` → `OK`: accepted,
///   not implemented (matches VARA's own replies to commands Pat sends every
///   session).
/// - anything else (`PUBLIC`, `CWID`, `CHAT`, `WINLINK SESSION`,
///   `P2P SESSION`, `CLEANTXBUFFER`, `FOOBAR`, WebSocket JSON, …) → `WRONG`.
fn vara_reply(command: &VaraCommand) -> VaraResponse {
    match command {
        VaraCommand::Version => VaraResponse::coppa_version(),
        VaraCommand::MyCall(call) if Callsign::new(call).is_ok() => VaraResponse::Ok,
        VaraCommand::MyCall(_) | VaraCommand::Unknown(_) => VaraResponse::Wrong,
        // COMPRESSION, BW* and ABORT are accepted but not implemented.
        VaraCommand::Listen(_)
        | VaraCommand::Tune(_)
        | VaraCommand::Connect { .. }
        | VaraCommand::Disconnect
        | VaraCommand::Compression(_)
        | VaraCommand::Bandwidth(_)
        | VaraCommand::Abort => VaraResponse::Ok,
    }
}

/// Send `response` to every client in `senders`, dropping it for any whose
/// queue is full.
async fn broadcast_vara(senders: &VaraResponseSenders, response: VaraResponse) {
    for tx in senders.lock().await.values() {
        let _ = tx.try_send(response.clone());
    }
}

/// Send `IAMALIVE` to every command-port client each `period`, the first one
/// `period` from now. It is a task of its own because the event loop can wait
/// for a long time (the busy-channel holdoff in `wait_for_clear_channel` has
/// no upper bound), and a client must keep hearing from the daemon meanwhile.
fn spawn_vara_keepalive(
    senders: VaraResponseSenders,
    period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            broadcast_vara(&senders, VaraResponse::IAmAlive).await;
        }
    })
}

/// A session-state change broadcast to attached hosts by `notify_session`.
enum SessionNotice {
    /// Handshake complete. VARA names the calling station first.
    Connected {
        local: Callsign,
        remote: Callsign,
        we_called: bool,
    },
    /// A session ended or a connect attempt failed.
    Disconnected,
}

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
    /// Automatic end of the keyed transmission tagged `generation`:
    /// `completed` is true once its last sample is predicted to have played
    /// and the PTT tail has elapsed, false when it was cancelled (output
    /// failure, no playback progress, or `max_tx_duration_s`). Feedback for
    /// any generation but the newest is stale and ignored.
    TxComplete { generation: u64, completed: bool },
    /// Shutdown signal received.
    Shutdown,
}

/// Lock the shared TX output, recovering from poisoning (its state is
/// always consistent between calls; nothing panics while holding it).
fn lock_tx_output(tx_output: &std::sync::Mutex<TxOutput>) -> std::sync::MutexGuard<'_, TxOutput> {
    tx_output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Log an RX level state change (COP-65): a WARN naming the fix on entering
/// `Clipping` or `Underdrive`, an INFO on recovery. dB fields go through
/// `level_meter::round_db`, as the WebSocket message does, so logs and wire
/// always show the same number.
fn log_rx_level_transition(state: LevelState, reading: &LevelReading) {
    let msg = level_meter::rx_transition_message(state, reading);
    let peak_dbfs = level_meter::round_db(reading.peak_dbfs);
    let rms_dbfs = level_meter::round_db(reading.rms_dbfs);
    match state {
        LevelState::Clipping => tracing::warn!(
            peak_dbfs,
            rms_dbfs,
            clipped_percent = level_meter::round_db(reading.clip_fraction * 100.0),
            "{msg}"
        ),
        LevelState::Underdrive if reading.rms_dbfs <= level_meter::SILENCE_FLOOR_DBFS => {
            tracing::warn!(rms_dbfs, "{msg}")
        }
        LevelState::Underdrive => tracing::warn!(
            rms_dbfs,
            threshold_dbfs = level_meter::UNDERDRIVE_RMS_DBFS,
            "{msg}"
        ),
        LevelState::Ok => tracing::info!(peak_dbfs, rms_dbfs, "{msg}"),
    }
}

/// The main daemon event loop.
pub struct EventLoop {
    config: DaemonConfig,
    engine: CoppaCore,
    event_rx: mpsc::Receiver<DaemonEvent>,
    event_tx: mpsc::Sender<DaemonEvent>,
    running: bool,
    /// The audio-out ring and the TX samples retained ahead of it until it
    /// has room, shared with the feeder task `run` spawns so a frame larger
    /// than the ring is delivered whole instead of truncated.
    tx_output: Arc<std::sync::Mutex<TxOutput>>,
    /// Optional audio input ring buffer consumer for reading RX samples.
    audio_in: Option<AudioRingConsumer>,
    /// PTT controller (defaults to NullPtt if unconfigured). Retains a
    /// connected rigctld client so the wideband gate can read the rig's dial
    /// frequency (COP-40).
    ptt: PttBackend,
    /// COP-32: degraded-startup conditions `coppad` reports in place of
    /// "Daemon ready". See [`EventLoop::startup_problems`].
    startup_problems: Vec<String>,
    /// COP-40: the operator-stated frequency source (`[radio] frequency_hz`
    /// / `band`), validated at startup -- the fallback evidence the
    /// wideband gate in `transmit_samples` uses when no live rigctld answer
    /// is available.
    radio_frequency: FrequencyConfig,
    /// COP-40: set when `try_drain_tx_queue` left the queue intact because
    /// the wideband gate refused; `run()`'s 500 ms poll retries the drain
    /// while it is set, so held payloads go out once the rig is back on a
    /// legal frequency instead of waiting for the next host enqueue.
    tx_held_by_frequency_gate: bool,
    /// COP-41: `tx_held_by_frequency_gate`'s counterpart for
    /// `session_tx_queue`, set by `drain_session_tx_queue` and retried by
    /// the same poll.
    session_tx_held_by_frequency_gate: bool,
    /// Optional sender for host responses (decoded data, status updates).
    response_tx: Option<mpsc::Sender<coppa_host::HostResponse>>,
    /// Playback progress from the audio output worker. `None` when no
    /// hardware output is wired (tests, headless builds), in which case PTT
    /// release falls back to the nominal audio duration.
    output_feedback: Option<Arc<OutputFeedback>>,
    /// Generation of the newest keyed transmission; only its completion may
    /// release PTT automatically.
    tx_generation: u64,
    /// When automatic TX keyed the radio for the current continuous keyed
    /// interval. `max_tx_duration_s` is measured from here rather than from
    /// each job's start, so transmissions queued behind an active one cannot
    /// push the forced unkey back. Cleared whenever PTT is released.
    tx_keyed_since: Option<tokio::time::Instant>,
    /// Playback-progress record for the current continuous keyed interval,
    /// shared by every transmission queued in it so the no-progress bound is
    /// not restarted by each newer job. Cleared whenever PTT is released.
    tx_stall_clock: Option<Arc<StallClock>>,
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
    /// COP-2 desync canary latch: `true` while the CP-mode invariant is currently
    /// violated (see `drive_cp_negotiation`'s canary block for both conditions).
    ///
    /// Exists so the canary logs **once per episode** rather than once per 500 ms
    /// poll. Without it a single divergence emits the identical warning twice a
    /// second — ~7,200 lines/hour — for as long as the link stays broken, which is
    /// indefinitely: every COP-1 give-up trigger reads state a *completed* handshake
    /// has already cleared, so nothing can rescue the condition, and burying the
    /// operator in duplicates of the one line that matters is the opposite of a
    /// tripwire. Cleared when the invariant holds again, so a second, later episode
    /// warns again.
    cp_desync_warned: bool,
    /// Number of distinct CP-mode desync episodes observed since daemon start
    /// (rising edges of `cp_desync_warned`). Surfaced in the WebSocket `status`
    /// snapshot so the condition is visible to a monitoring client and not only to
    /// whoever is reading logs -- the canary otherwise reported a healthy station
    /// through every operator-facing channel while the link was undecodable.
    cp_desync_episodes: u32,
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
    /// alongside VARA telemetry whenever a frame decodes and at every session
    /// transition (`notify_session`).
    #[cfg(feature = "websocket")]
    ws_status: Option<Arc<Mutex<coppa_host::websocket::WsStatus>>>,
    /// Optional map of connected VARA command-port clients' response senders, for
    /// broadcasting `VaraResponse` telemetry (SNR/PTT/BUFFER/BUSY — decision 8).
    /// Wired by `main.rs` from `VaraServer::response_senders()`.
    vara_responses: Option<VaraResponseSenders>,
    /// Outbound raw payload bytes queued for encode+transmit (the sessionless
    /// raw/ARQ `HostEvent::DataReceived` TX path, enabled by
    /// `[host] sessionless_tx_enabled`), tagged with the ARQ seq
    /// number assigned by `ArqTx::send` when this entry came from the
    /// ARQ-enabled path (`None` otherwise -- non-ARQ sends are never probe
    /// candidates). Its `None`-tagged entries count toward `BUFFER`
    /// telemetry; see `buffered_tx_bytes`.
    tx_queue: VecDeque<(Option<u8>, Vec<u8>)>,
    /// Established-session data chunks waiting for their keyed interval
    /// (COP-41): every `HostEvent::DataReceived` on a live session is split
    /// into pieces that each encode at the engine's current speed level, and
    /// this queue paces them ONE FRAME PER KEYED INTERVAL -- drained by a
    /// single pop in `drain_session_tx_queue` from PTT release (the same
    /// convention as `try_drain_tx_queue`), never as one long keyed blast,
    /// which the 30 s `max_tx_duration_s` watchdog would cancel mid-message.
    /// `host_len` terms count toward `BUFFER` telemetry; see
    /// `buffered_tx_bytes`.
    session_tx_queue: VecDeque<SessionTxChunk>,
    /// Host bytes waiting for ARQ window space (COP-41, D5): an event too
    /// large for the 8-segment window is held here -- never dropped -- and
    /// `pump_arq_backlog` moves it into window segments as ACKs free space.
    /// Counted by `BUFFER` in the unit the host wrote; a byte that becomes
    /// an ARQ segment keeps the sum unchanged (it moves between counted
    /// terms), so the backlog->segment transition fires no spurious `BUFFER`
    /// line.
    arq_tx_backlog: VecDeque<u8>,
    /// Host payload bytes handed to `transmit_samples` on a path with no ACK
    /// to wait for (the non-ARQ raw path and established-session data).
    /// Cleared at PTT release (`set_ptt(false)`): by then every frame handed
    /// over earlier has played or been discarded. See `buffered_tx_bytes`.
    tx_on_air_bytes: usize,
    /// The last value sent as `BUFFER`, so `report_buffer` emits only on
    /// change.
    last_reported_buffer: usize,
    /// Spectral-occupancy busy gate (decision 8: `BUSY ON`/`OFF` telemetry), fed
    /// raw incoming audio in `handle_audio_in`.
    busy_gate: BusyGate,
    /// RX audio level meter (COP-65), fed raw incoming audio by
    /// `observe_rx_level` from both raw RX taps; see that method's doc.
    rx_level_meter: RxLevelMeter,
    /// Session manager for connected-mode operation.
    session_mgr: SessionManager,
    /// Local station callsign: `[engine] callsign` at startup, replaced by a
    /// valid command-port `MYCALL`. New sessions, station ID and beacons read
    /// it. A session keeps the callsign it was set up with. COP-37: `None`
    /// (empty or invalid config, no valid `MYCALL` yet) refuses every
    /// transmission -- see `transmit_samples`. Nothing ever sets it back to
    /// `None`, so a payload accepted for TX keeps its callsign.
    local_callsign: Option<Callsign>,
    /// How often `run()` sends `IAMALIVE` to every command-port client;
    /// `VARA_KEEPALIVE_PERIOD` outside tests.
    vara_keepalive_period: Duration,
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
    /// COP-60: earliest instant this station may key PTT after the last frame
    /// it decoded (`[radio] turnaround_ms` after that decode). The sender is
    /// still keyed for its PTT tail after its last sample, so a reply keyed
    /// sooner goes out while the peer is deaf. `None` until the first decode;
    /// enforced by `wait_for_rx_turnaround`.
    rx_turnaround_until: Option<tokio::time::Instant>,
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
    /// `observe_busy_gate_from_audio_input` while `wait_for_clear_channel` or
    /// `wait_for_rx_turnaround` was blocked, already fed to `busy_gate.observe` there but not yet
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
    /// unrecognized/unbuilt PTT backend -- see `create_ptt` -- if
    /// `[radio] frequency_hz`/`band` don't validate, or if a wideband
    /// profile has no frequency source at or above 29.7 MHz -- see
    /// `check_frequency_gate_at_startup`.
    pub fn new(config: DaemonConfig) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(256);
        let radio_frequency = config
            .radio
            .frequency_config()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut ptt = Self::create_ptt(&config)?;
        // COP-32: conditions the daemon runs through but must not call ready.
        let mut startup_problems = Vec::new();
        if config.radio.ptt_config() == Ok(crate::config::PttConfig::Rigctld) && !ptt.is_rigctld() {
            startup_problems.push(format!(
                "no PTT: rigctld at {} is unreachable",
                config.radio.rigctld_address
            ));
        }

        // Create engine from profile; all profiles use 48kHz internally
        let engine =
            if let Some(profile) = coppa_engine::profiles::get_profile(&config.engine.profile) {
                CoppaCore::from_profile(profile)
            } else {
                CoppaCore::with_config(coppa_engine::EngineConfig::default())
            };
        Self::check_frequency_gate_at_startup(&config, &engine, &radio_frequency, &mut ptt)?;

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
                    // `local_callsign` at `None`, exactly like an empty one:
                    // the daemon still starts and receives, but every
                    // transmission is refused (COP-37, `transmit_samples`),
                    // and with it station ID, beacons and connects. Warn
                    // once at startup so the operator learns why before the
                    // first refusal.
                    tracing::warn!(
                        callsign = %config.engine.callsign,
                        error = %e,
                        "Invalid [engine] callsign; transmission disabled until a \
                         valid MYCALL is set"
                    );
                    None
                }
            }
        };

        let busy_gate = BusyGate::new(config.audio.sample_rate as f32);
        let rx_level_meter = RxLevelMeter::new(config.audio.sample_rate);
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
        let rate_loop = Self::build_rate_loop(&config, &engine, engine.speed_level());

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
            tx_output: Arc::new(std::sync::Mutex::new(TxOutput::default())),
            audio_in: None,
            ptt,
            startup_problems,
            radio_frequency,
            tx_held_by_frequency_gate: false,
            session_tx_held_by_frequency_gate: false,
            response_tx: None,
            output_feedback: None,
            tx_generation: 0,
            tx_keyed_since: None,
            tx_stall_clock: None,
            audio_in_overflow_count: 0,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            arq_tx,
            arq_rx,
            arq_session_id: 0,
            rate_loop,
            cp_gate,
            cp_negotiator,
            cp_propose_seq: None,
            cp_desync_warned: false,
            cp_desync_episodes: 0,
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
            session_tx_queue: VecDeque::new(),
            arq_tx_backlog: VecDeque::new(),
            tx_on_air_bytes: 0,
            last_reported_buffer: 0,
            busy_gate,
            rx_level_meter,
            session_mgr: SessionManager::new(),
            local_callsign,
            vara_keepalive_period: VARA_KEEPALIVE_PERIOD,
            listening: false,
            is_transmitting: false,
            tx_frame_count: 0,
            max_frames_per_turn: 4,
            rx_turnaround_until: None,
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
    /// behavior -- log an ERROR, fall back to `NullPtt`, and record a
    /// startup problem so coppad reports itself degraded rather than ready
    /// (COP-32) -- is kept, since it's a live/transient condition rather
    /// than an unrecognized config.
    /// A wideband profile that relies on that rigctld as its only frequency
    /// source still refuses to start -- `check_frequency_gate_at_startup`
    /// sees the fallback as "no source answering".
    ///
    /// A connected rigctld client is returned as `PttBackend::Rigctld`, not
    /// boxed as `dyn PttControl`, so its dial frequency stays reachable.
    fn create_ptt(config: &DaemonConfig) -> Result<PttBackend> {
        let parsed = config
            .radio
            .ptt_config()
            .map_err(|e| anyhow::anyhow!("invalid [radio] ptt_method: {e}"))?;

        match parsed {
            crate::config::PttConfig::None => Ok(PttBackend::Other(Box::new(NullPtt::new()))),
            crate::config::PttConfig::Vox => {
                Ok(PttBackend::Other(Box::new(coppa_radio::VoxPtt::new())))
            }
            crate::config::PttConfig::Rigctld => {
                match coppa_radio::rigctld::RigctldClient::connect(&config.radio.rigctld_address) {
                    Ok(client) => Ok(PttBackend::Rigctld(client)),
                    Err(e) => {
                        tracing::error!(
                            address = %config.radio.rigctld_address,
                            error = %e,
                            "Failed to connect to rigctld; PTT is disabled and \
                             transmissions will not key the radio"
                        );
                        Ok(PttBackend::Other(Box::new(NullPtt::new())))
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
                    Ok(PttBackend::Other(Box::new(ptt)))
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
                    Ok(PttBackend::Other(Box::new(ptt)))
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

    /// COP-40: refuse to start a wideband profile that can never legally
    /// transmit.
    ///
    /// 47 CFR 97.307(f)(3) caps emissions below 29.7 MHz at 2.8 kHz occupied
    /// bandwidth. A narrowband engine (every HF profile, 2400 Hz) is legal on
    /// any band and skips this entirely. A wideband engine (`vhf_wide`,
    /// 5600 Hz) must have frequency evidence at or above 29.7 MHz: a live
    /// rigctld answer, else `[radio] frequency_hz`, else `[radio] band`.
    /// No evidence, or evidence below 29.7 MHz, is a hard startup error.
    ///
    /// Only the constructed engine is checked: a `LevelDerived` engine that
    /// starts narrowband and later climbs toward `vhf_wide` is rebound to HF by
    /// `apply_speed_level` (COP-21) unless its frequency evidence permits the
    /// wideband profile, and is gated per transmission by
    /// `wideband_frequency_check` otherwise.
    fn check_frequency_gate_at_startup(
        config: &DaemonConfig,
        engine: &CoppaCore,
        radio_frequency: &FrequencyConfig,
        ptt: &mut PttBackend,
    ) -> Result<()> {
        let occupied_hz = engine.occupied_bandwidth_hz();
        if band::gate(occupied_hz, None).is_ok() {
            return Ok(());
        }
        let profile = &config.engine.profile;
        let live_hz = ptt.poll_frequency();
        if let Some(live) = live_hz {
            if radio_frequency.contradicts(live) {
                tracing::warn!(
                    live_hz = live,
                    stated = ?radio_frequency.static_evidence(),
                    "rigctld's dial frequency contradicts [radio] frequency_hz/band; \
                     using the live value"
                );
            }
        }
        let evidence = band::resolve_evidence(live_hz, radio_frequency);
        match band::gate(occupied_hz, evidence.as_ref()) {
            Ok(()) => {
                let evidence = evidence.expect("a wideband gate pass implies evidence");
                tracing::info!(
                    profile = %profile,
                    occupied_hz,
                    evidence = %evidence.describe(),
                    "wideband profile cleared the 29.7 MHz regulatory gate"
                );
                Ok(())
            }
            Err(GateRefusal::BelowThreshold { .. }) => match evidence {
                Some(FrequencyEvidence::Live(hz)) => Err(anyhow::anyhow!(
                    "rigctld reports an operating frequency of {hz} Hz, below 29.7 MHz, but \
                     profile '{profile}' emits {occupied_hz} Hz occupied bandwidth — a \
                     wideband emission below 29.7 MHz violates 47 CFR 97.307(f)(3); use an \
                     HF profile (levels 1-8) or tune the rig to a VHF+ band"
                )),
                _ => Err(anyhow::anyhow!(
                    "[radio] states an operating frequency below 29.7 MHz, but profile \
                     '{profile}' emits {occupied_hz} Hz occupied bandwidth — a wideband \
                     emission below 29.7 MHz violates 47 CFR 97.307(f)(3); use an HF \
                     profile (levels 1-8) or a VHF+ frequency/band"
                )),
            },
            Err(GateRefusal::NoSource) => {
                if config.radio.ptt_config() == Ok(crate::config::PttConfig::Rigctld) {
                    // Connected but not answering `f`, or never connected
                    // (`create_ptt`'s soft fallback to no PTT).
                    let state = if ptt.is_rigctld() {
                        "is not answering the frequency query"
                    } else {
                        "is unreachable"
                    };
                    Err(anyhow::anyhow!(
                        "profile '{profile}' emits {occupied_hz} Hz occupied bandwidth, which \
                         47 CFR 97.307(f)(3) caps at 2.8 kHz below 29.7 MHz, and its only \
                         frequency source (rigctld at {}) {state}: start rigctld, or set \
                         [radio] frequency_hz or [radio] band as a fallback frequency source",
                        config.radio.rigctld_address
                    ))
                } else {
                    Err(anyhow::anyhow!(
                        "profile '{profile}' emits {occupied_hz} Hz occupied bandwidth, which \
                         47 CFR 97.307(f)(3) caps at 2.8 kHz below 29.7 MHz: set [radio] \
                         frequency_hz or [radio] band, or set ptt_method = \"rigctld\" so the \
                         gate can read the dial frequency"
                    ))
                }
            }
        }
    }

    /// COP-40: the per-transmission half of the wideband regulatory gate.
    ///
    /// Narrowband engines return `Ok` with no I/O. A wideband engine
    /// re-resolves its frequency evidence on every call -- one rigctld `f`
    /// round trip, falling back to the stated `[radio] frequency_hz`/`band`
    /// -- so a rig retuned below 29.7 MHz, or a rigctld that stopped
    /// answering with no stated fallback, is refused on the very next frame.
    /// Logs the refusal; the caller only has to not transmit.
    fn wideband_frequency_check(&mut self) -> Result<(), GateRefusal> {
        let occupied_hz = self.engine.occupied_bandwidth_hz();
        if band::gate(occupied_hz, None).is_ok() {
            return Ok(());
        }
        let evidence = self.frequency_evidence();
        band::gate(occupied_hz, evidence.as_ref()).inspect_err(|refusal| match refusal {
            GateRefusal::BelowThreshold { evidence_hz, .. } => tracing::error!(
                "refusing wideband transmission: {occupied_hz} Hz occupied bandwidth but {} \
                 (< {}) — 47 CFR 97.307(f)(3) caps HF emissions at 2.8 kHz",
                evidence.map_or_else(|| format!("{evidence_hz} Hz"), |e| e.describe()),
                band::US_WIDEBAND_MIN_HZ
            ),
            GateRefusal::NoSource => tracing::error!(
                "refusing wideband transmission: {occupied_hz} Hz occupied bandwidth and no \
                 frequency source is answering{} — 47 CFR 97.307(f)(3); set [radio] \
                 frequency_hz or [radio] band as a fallback source",
                if self.ptt.is_rigctld() {
                    " (rigctld not answering)"
                } else if self.config.radio.ptt_config() == Ok(crate::config::PttConfig::Rigctld) {
                    " (rigctld unreachable)"
                } else {
                    ""
                }
            ),
        })
    }

    /// This station's frequency evidence right now: one rigctld `f` round
    /// trip, falling back to the stated `[radio] frequency_hz`/`band`. Warns
    /// when the live dial contradicts the stated source. Shared by the COP-40
    /// per-transmission gate and the COP-21 HF rebind.
    fn frequency_evidence(&mut self) -> Option<FrequencyEvidence> {
        let live_hz = self.ptt.poll_frequency();
        if let Some(live) = live_hz {
            if self.radio_frequency.contradicts(live) {
                tracing::warn!(
                    live_hz = live,
                    stated = ?self.radio_frequency.static_evidence(),
                    "rigctld's dial frequency contradicts [radio] frequency_hz/band; \
                     using the live value"
                );
            }
        }
        band::resolve_evidence(live_hz, &self.radio_frequency)
    }

    /// The daemon's `RateLoop` over `engine`'s ladder (`available_speed_levels`),
    /// starting at `initial_level` (clamped into that ladder). Shared by `new`
    /// and the COP-21 HF rebind so both get the same `raise_dwell` and probing
    /// setup.
    fn build_rate_loop(config: &DaemonConfig, engine: &CoppaCore, initial_level: u8) -> RateLoop {
        let rate_loop = RateLoop::new(engine.available_speed_levels(), 5, initial_level);
        if config.engine.rate_loop_probe_interval > 0 {
            rate_loop.with_probing(
                config.engine.rate_loop_probe_interval,
                config.engine.rate_loop_probe_offset,
            )
        } else {
            rate_loop
        }
    }

    /// COP-21: keep a level-derived engine off `vhf_wide` unless the station
    /// can show it is at or above 29.7 MHz.
    ///
    /// `coppad` only builds an `OfdmBand::LevelDerived` engine when `[engine]
    /// profile` is not a built-in name (`EventLoop::new` falls back to
    /// `EngineConfig::default()`). On such an engine the speed level picks the
    /// band, so a `RateLoop` climb to level 5+ would select `vhf_wide`
    /// (5600 Hz), which 47 CFR 97.307(f)(3) forbids below 29.7 MHz. Rather than
    /// let COP-40's gate refuse every such frame, this rebinds the engine to
    /// `OfdmBand::Hf` (`CoppaCore::set_ofdm_band`) so the level goes on air as
    /// `hf_standard` (2400 Hz): the station keeps transmitting on an HF-legal
    /// profile instead of going silent. A level above HF's ceiling is lowered
    /// to `HF_MAX_SPEED_LEVEL` first, and `RateLoop` is rebuilt over the HF
    /// ladder from its own current level.
    ///
    /// Only `LevelDerived` engines are touched: an explicitly configured
    /// `VHF_FAST` engine keeps COP-40's startup error and per-transmission
    /// refusal and is never rewritten. The binding is one-way for the session:
    /// evidence that later returns above 29.7 MHz does not undo it. Wideband
    /// is decided by `band::gate` on the width `level` would actually produce
    /// (`occupied_bandwidth_hz_at`), never a hard-coded level, and no rig I/O
    /// happens unless `level` would be wideband on a level-derived engine. No
    /// evidence at all binds to HF (fail closed, matching COP-40).
    fn keep_level_derived_engine_hf_legal(&mut self, level: u8) {
        if self.engine.config().ofdm_band != OfdmBand::LevelDerived {
            return;
        }
        let occupied_hz = self.engine.occupied_bandwidth_hz_at(level);
        if band::gate(occupied_hz, None).is_ok() {
            return;
        }
        let evidence = self.frequency_evidence();
        if band::gate(occupied_hz, evidence.as_ref()).is_ok() {
            return;
        }
        if self.engine.speed_level() > coppa_engine::HF_MAX_SPEED_LEVEL {
            if let Err(e) = self
                .engine
                .set_speed_level(coppa_engine::HF_MAX_SPEED_LEVEL)
            {
                tracing::warn!(error = %e, "Failed to lower the speed level for the HF profile");
                return;
            }
        }
        if let Err(e) = self.engine.set_ofdm_band(OfdmBand::Hf) {
            tracing::warn!(error = %e, "Failed to rebind the engine to the HF profile");
            return;
        }
        self.rate_loop =
            Self::build_rate_loop(&self.config, &self.engine, self.rate_loop.current_level());
        tracing::warn!(
            "speed level {level} selects vhf_wide ({occupied_hz} Hz occupied bandwidth) on this \
             engine because [engine] profile \"{}\" is not a built-in profile, but {}; using \
             the HF profile ({} Hz, levels 1-{}) for the rest of this session -- 47 CFR \
             97.307(f)(3) caps emissions below 29.7 MHz at 2.8 kHz",
            self.config.engine.profile,
            evidence.map_or_else(
                || "no frequency source is answering".to_string(),
                |e| e.describe()
            ),
            self.engine.occupied_bandwidth_hz(),
            coppa_engine::HF_MAX_SPEED_LEVEL,
        );
    }

    /// COP-21: the one place the daemon changes the engine's speed level.
    ///
    /// Runs `keep_level_derived_engine_hf_legal` first, so a level-derived
    /// engine is rebound to HF *before* a wideband level can take effect (and
    /// before anything is encoded at it), then clamps `level` to the engine's
    /// ladder -- a no-op unless that rebind just happened while `level` was 9
    /// -- and applies it. Errors are `CoppaCore::set_speed_level`'s.
    fn apply_speed_level(&mut self, level: u8) -> Result<()> {
        self.keep_level_derived_engine_hf_legal(level);
        let level = self
            .engine
            .available_speed_levels()
            .last()
            .map_or(level, |&max| level.min(max));
        self.engine.set_speed_level(level)
    }

    /// COP-32: conditions found while starting that leave the daemon running
    /// but unable to do part of its job (today: rigctld unreachable, so no
    /// PTT). Empty when startup was clean.
    pub fn startup_problems(&self) -> &[String] {
        &self.startup_problems
    }

    /// Get a sender for injecting events into the loop.
    pub fn event_sender(&self) -> mpsc::Sender<DaemonEvent> {
        self.event_tx.clone()
    }

    /// Set the audio output ring buffer for TX sample playback.
    /// Wire playback progress from the audio output worker, so automatic PTT
    /// release follows samples actually played.
    #[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))] // used by coppad's CPAL output wiring
    pub fn set_output_feedback(&mut self, feedback: Arc<OutputFeedback>) {
        self.output_feedback = Some(feedback);
    }

    #[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))] // coppad wires it only with CPAL output
    pub fn set_audio_out(&mut self, producer: AudioRingProducer) {
        self.tx_output().set_producer(producer);
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

    /// Publish `cp_desync_episodes` to the WebSocket status snapshot, independent
    /// of the decode event loop.
    ///
    /// Review finding (COP-2): the snapshot used to update `cp_desync_episodes`
    /// only at the same point as `connected`/`snr`/`level`/`cfo` -- i.e. only on a
    /// successful decode. That is fine for those fields (they describe the current
    /// decode), but wrong for this one: the VHF-profile desync this canary exists
    /// to catch is precisely the case where the peer can no longer decode this
    /// station's frames, which on a duplex-ish link commonly means this station
    /// stops decoding the peer too. The counter incremented internally in
    /// `check_cp_desync` regardless, but the WebSocket-facing snapshot stayed at
    /// its last decode-time value -- `0` if the desync's first episode ever -- for
    /// as long as the outage lasted, i.e. the snapshot could read "healthy"
    /// throughout the exact outage it exists to surface.
    ///
    /// `check_cp_desync` itself stays synchronous (see its doc: it runs off the
    /// 500 ms retransmit poll and cannot take this `async` lock without making
    /// that whole poll path async for one `u32` field). Called from
    /// `check_arq_retransmits` instead, right after `drive_cp_negotiation`, which
    /// already runs on that same poll independent of decode activity.
    #[cfg(feature = "websocket")]
    async fn publish_desync_episodes(&self) {
        if let Some(ref status) = self.ws_status {
            let mut snap = status.lock().await;
            snap.cp_desync_episodes = self.cp_desync_episodes;
        }
    }

    #[cfg(not(feature = "websocket"))]
    async fn publish_desync_episodes(&self) {}

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
            broadcast_vara(senders, response).await;
        }
    }

    /// Send `response` to the command-port client that sent the command it
    /// answers. Telemetry and session state go to every client (`emit_vara`,
    /// `notify_session`); a reply does not. WebSocket IDs start at
    /// `coppa_host::websocket::FIRST_CLIENT_ID`, so a reply to a
    /// WebSocket-originated event finds no sender here and is dropped.
    ///
    /// Uses `try_send`, never `send().await`: a client that stops reading must
    /// not stall the modem's event loop (same choice as `notify_session`).
    async fn reply_vara(&self, client_id: u32, response: VaraResponse) {
        let Some(ref senders) = self.vara_responses else {
            return;
        };
        let senders = senders.lock().await;
        let Some(tx) = senders.get(&client_id) else {
            return;
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(response.clone()) {
            tracing::warn!(
                client_id,
                line = response.format().trim_end(),
                "VARA command-port queue full; reply dropped"
            );
        }
    }

    #[cfg(feature = "websocket")]
    fn established_remote(&self) -> Option<Callsign> {
        self.session_mgr.active_sessions().iter().find_map(|&id| {
            self.session_mgr
                .get(id)
                .filter(|session| session.is_established())
                .map(|session| session.remote.clone())
        })
    }

    /// Call after `session_mgr` reflects the change. Broadcasts to all hosts:
    /// session state is link-wide; every attached host wants it.
    async fn notify_session(&self, client_id: u32, notice: SessionNotice) {
        let (status, vara) = match &notice {
            SessionNotice::Connected {
                local,
                remote,
                we_called,
            } => {
                let (source, destination) = if *we_called {
                    (local, remote)
                } else {
                    (remote, local)
                };
                (
                    format!("CONNECTED {}", remote),
                    VaraResponse::Connected(
                        source.to_string(),
                        destination.to_string(),
                        self.engine.occupied_bandwidth_hz(),
                    ),
                )
            }
            SessionNotice::Disconnected => ("DISCONNECTED".to_string(), VaraResponse::Disconnected),
        };
        if let Some(ref tx) = self.response_tx {
            let _ = tx.try_send(coppa_host::HostResponse::StatusUpdate { client_id, status });
        }
        // A dropped state line leaves the host waiting out its own connect or
        // disconnect timer, so log it. Closed channels belong to departed clients.
        if let Some(ref senders) = self.vara_responses {
            for (id, tx) in senders.lock().await.iter() {
                if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(vara.clone()) {
                    tracing::warn!(
                        client_id = id,
                        line = vara.format().trim_end(),
                        "VARA command-port queue full; session-state line dropped"
                    );
                }
            }
        }
        #[cfg(feature = "websocket")]
        {
            let remote = self.established_remote();
            if let Some(ref status) = self.ws_status {
                let mut snap = status.lock().await;
                snap.connected = remote.is_some();
                snap.remote_call = remote.map(|cs| cs.to_string());
            }
            if let Some(ref ws_tx) = self.ws_broadcast {
                let msg = match notice {
                    SessionNotice::Connected { remote, .. } => {
                        coppa_host::websocket::WsServerMessage::Connected {
                            remote_call: remote.to_string(),
                        }
                    }
                    SessionNotice::Disconnected => {
                        coppa_host::websocket::WsServerMessage::Disconnected
                    }
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = ws_tx.send(json); // An error means no client is subscribed.
                }
            }
        }
    }

    async fn reap_timed_out_sessions(&mut self) {
        let removed = self.session_mgr.cleanup_timed_out();
        for id in removed {
            tracing::warn!(session_id = id, "Session timed out");
            self.discard_session_tx(id).await;
            self.notify_session(0, SessionNotice::Disconnected).await;
        }
    }

    /// COP-40: retry a TX-queue (or, COP-41, session-queue) drain the
    /// wideband gate held back. A no-op unless the last drain attempt was
    /// refused, so it never changes when an ordinary queue drains.
    async fn retry_held_tx_drain(&mut self) {
        if self.tx_held_by_frequency_gate {
            self.try_drain_tx_queue().await;
        }
        if self.session_tx_held_by_frequency_gate {
            self.drain_session_tx_queue().await;
        }
    }

    /// Push one raw payload onto the outbound TX queue, report the resulting
    /// `BUFFER` telemetry, and attempt to start transmitting immediately if the
    /// link is currently idle. See `tx_queue`'s field doc and `try_drain_tx_queue`.
    async fn enqueue_tx(&mut self, seq: Option<u8>, data: Vec<u8>) {
        self.tx_queue.push_back((seq, data));
        self.report_buffer().await;
        self.try_drain_tx_queue().await;
    }

    /// Host payload bytes this modem still holds (COP-30): what `BUFFER`
    /// reports. The sum of
    /// - every un-acked ARQ segment's payload (`ArqTx::unacked_bytes`),
    ///   which leaves the sum only when the peer ACKs it or a `Reset`
    ///   rebuilds `arq_tx` -- a segment with its retransmit budget spent
    ///   stays counted, since nothing has acked it;
    /// - `None`-tagged (non-ARQ) `tx_queue` entries not yet popped. A
    ///   `Some`-tagged entry is the `TransportPdu`-wrapped copy of a payload
    ///   `ArqTx` already counts, so it adds nothing;
    /// - queued established-session chunks (`session_tx_queue`, COP-41):
    ///   each chunk's `host_len`, the bytes the host wrote that the chunk
    ///   carries;
    /// - `arq_tx_backlog` bytes (COP-41, D5): host bytes waiting for ARQ
    ///   window space. A backlog byte that becomes an ARQ segment moves
    ///   between counted terms, leaving the sum unchanged;
    /// - `tx_on_air_bytes`: payloads with no ACK to wait for, counted until
    ///   PTT releases after they play.
    ///
    /// MAC, transport and FEC framing are not counted: this is the unit the
    /// host wrote.
    fn buffered_tx_bytes(&self) -> usize {
        let queued: usize = self
            .tx_queue
            .iter()
            .filter(|(seq, _)| seq.is_none())
            .map(|(_, data)| data.len())
            .sum();
        let session_queued: usize = self.session_tx_queue.iter().map(|c| c.host_len).sum();
        let unacked = self.arq_tx.as_ref().map_or(0, ArqTx::unacked_bytes);
        queued + session_queued + self.arq_tx_backlog.len() + self.tx_on_air_bytes + unacked
    }

    /// Emit `BUFFER` with `buffered_tx_bytes` if it differs from the last
    /// value sent. The only place `VaraResponse::Buffer` is built; called
    /// wherever the sum can change.
    async fn report_buffer(&mut self) {
        let bytes = self.buffered_tx_bytes();
        if bytes != self.last_reported_buffer {
            self.last_reported_buffer = bytes;
            self.emit_vara(VaraResponse::Buffer(bytes)).await;
        }
    }

    /// If the link isn't currently mid-transmission, pop the next queued payload
    /// (if any) and encode+transmit it. Called after every enqueue and after
    /// every PTT release, so the queue drains one frame at a time as each
    /// transmission completes. The pop itself never changes `BUFFER`: a
    /// non-ARQ payload moves to `tx_on_air_bytes`, and an ARQ one stays in
    /// `ArqTx` until acked.
    async fn try_drain_tx_queue(&mut self) {
        if self.is_transmitting {
            return;
        }
        // COP-21: a level-derived engine left on vhf_wide by a retune moves to HF
        // rather than holding the queue.
        if !self.tx_queue.is_empty() {
            self.keep_level_derived_engine_hf_legal(self.engine.speed_level());
        }
        // COP-40: hold queued payloads (rather than pop one and have
        // `transmit_samples` refuse it) while the wideband gate refuses;
        // `retry_held_tx_drain` tries again on the next poll. COP-21 above has
        // already rebound a level-derived engine to HF, so only an explicitly
        // wideband (`VHF_FAST`) engine is still held here.
        self.tx_held_by_frequency_gate =
            !self.tx_queue.is_empty() && self.wideband_frequency_check().is_err();
        if self.tx_held_by_frequency_gate {
            return;
        }
        if let Some((seq_opt, data)) = self.tx_queue.pop_front() {
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
                if let Err(e) = self.apply_speed_level(level) {
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
                    if seq_opt.is_none() {
                        self.tx_on_air_bytes += data.len();
                    }
                    Box::pin(self.transmit_samples(&samples)).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Encode failed for queued TX frame");
                    // A dropped non-ARQ payload leaves the count here.
                    self.report_buffer().await;
                }
            }

            if is_probe {
                if let Err(e) = self.apply_speed_level(self.rate_loop.current_level()) {
                    tracing::warn!(error = %e, "Failed to revert speed level after probe");
                }
            }
        }
    }

    /// COP-41: pace established-session chunks one frame per keyed interval.
    /// Mirrors `try_drain_tx_queue`'s guard-and-single-pop convention: while
    /// a transmission is keyed nothing new starts, and each PTT release pops
    /// at most one chunk. Session chunks deliberately take none of
    /// `rate_loop`'s or the probe machinery's levels (D3): a session link
    /// has no ACK feedback, so its level is static and the engine's current
    /// level -- the one the chunks were sized against -- is the right one.
    async fn drain_session_tx_queue(&mut self) {
        if self.is_transmitting {
            return;
        }
        // COP-40, as in `try_drain_tx_queue`: hold the chunks (rather than
        // pop one and have `transmit_samples` refuse it -- session data has
        // no MAC-level retry) while the wideband gate refuses;
        // `retry_held_tx_drain` tries again on the next poll.
        self.session_tx_held_by_frequency_gate =
            !self.session_tx_queue.is_empty() && self.wideband_frequency_check().is_err();
        if self.session_tx_held_by_frequency_gate {
            return;
        }
        if let Some(chunk) = self.session_tx_queue.pop_front() {
            match self.engine.encode_bytes(&chunk.pdu_bytes) {
                Ok(samples) => {
                    // No MAC-level ACK exists for session data, so
                    // its bytes stay in `BUFFER` until PTT releases.
                    self.tx_on_air_bytes += chunk.host_len;
                    self.report_buffer().await;
                    // Boxed: `transmit_samples` -> `handle_ptt_change` -> (on
                    // PTT release) `drain_session_tx_queue` forms the same
                    // 3-way async call cycle `try_drain_tx_queue` documents;
                    // one edge needs indirection to give the compiler a
                    // finite-sized future.
                    let generation = self.tx_generation;
                    Box::pin(self.transmit_samples(&samples)).await;
                    if self.tx_generation == generation {
                        // The gate passed above but refused after the
                        // courtesy wait (the one `transmit_samples` path
                        // that keys nothing and leaves `tx_generation`
                        // alone). Put the chunk back at the head, so the
                        // retry poll resumes the stream without a hole.
                        self.session_tx_queue.push_front(chunk);
                        self.session_tx_held_by_frequency_gate = true;
                        self.report_buffer().await;
                        return;
                    }
                    self.tx_frame_count += 1;
                    tracing::debug!(
                        frame = self.tx_frame_count,
                        max = self.max_frames_per_turn,
                        "Session data frame transmitted"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to encode session data");
                }
            }
        }
    }

    /// Drop the unsent `session_tx_queue` chunks of a session that has ended
    /// (local or remote DISCONNECT, or timeout), and report the `BUFFER`
    /// change. Without this the queue keeps transmitting a dead session's
    /// data on every PTT release, and a reconnect to the same peer could
    /// deliver those stale bytes into the new session. A chunk already on
    /// the air stays in `tx_on_air_bytes` until its PTT release, as usual.
    async fn discard_session_tx(&mut self, session_id: u8) {
        let before = self.session_tx_queue.len();
        self.session_tx_queue
            .retain(|chunk| chunk.session_id != session_id);
        let dropped = before - self.session_tx_queue.len();
        if dropped > 0 {
            tracing::info!(
                session_id,
                chunks = dropped,
                "Session ended; discarding its unsent data"
            );
            self.report_buffer().await;
        }
    }

    /// COP-41 (D4/D5): move `arq_tx_backlog` bytes into ARQ window segments
    /// as window space allows. Chunks are sized against the ladder-minimum
    /// budget (`min_level_budget`) so every segment re-encodes at ANY speed
    /// level a later retransmit or rate drop uses. Called after every
    /// backlog append and wherever an ACK (or a Reset's fresh window) frees
    /// window space; a full window is a normal stop, not a drop -- the bytes
    /// stay held and `BUFFER` keeps counting them.
    ///
    /// Each chunk's `fits` probe is built with the seq `ArqTx::send` will
    /// actually assign (computed from `send_base + in_flight`), so the
    /// measured PDU is byte-identical to the one that gets queued and the
    /// exact-fit guarantee survives compression's content dependence.
    async fn pump_arq_backlog(&mut self) {
        while !self.arq_tx_backlog.is_empty() && self.arq_tx.as_ref().is_some_and(ArqTx::can_send) {
            let chunk: Vec<u8> = {
                let arq_tx = self.arq_tx.as_ref().expect("can_send checked above");
                let next_seq = arq_tx.send_base().wrapping_add(arq_tx.in_flight());
                let fits = |c: &[u8]| {
                    let pdu =
                        TransportPdu::new_reliable(self.arq_session_id, next_seq, 0, c.to_vec());
                    self.engine.encoded_payload_len(&pdu.to_bytes()) <= min_level_budget()
                };
                let budget = min_level_budget();
                let backlog = self.arq_tx_backlog.make_contiguous();
                chunk_to_fit(backlog, TRANSPORT_HEADER_SIZE, budget, fits)
                    .into_iter()
                    .next()
                    .expect("a non-empty backlog always yields at least one chunk")
            };
            match self
                .arq_tx
                .as_mut()
                .expect("can_send checked above")
                .send(chunk.clone(), Instant::now())
            {
                Ok(seq) => {
                    let pdu =
                        TransportPdu::new_reliable(self.arq_session_id, seq, 0, chunk.clone());
                    let tx_bytes = pdu.to_bytes();
                    self.arq_tx_backlog.drain(..chunk.len());
                    // `seq` is `Some` for the fresh ARQ-tracked segment --
                    // used by active overshoot probing
                    // (`RateLoop::level_for_next_transmission`) to attribute
                    // a probe's later outcome back to its ACK/timeout.
                    self.enqueue_tx(Some(seq), tx_bytes).await;
                }
                Err(e) => {
                    // Unreachable while single-threaded (the `can_send` check
                    // and the `send` cannot interleave), but stop and hold
                    // rather than drop if it ever fires.
                    tracing::debug!(error = %e, "ARQ window full; backlog holds the rest");
                    break;
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
        // TX feeder: moves retained TX samples into the audio-out ring as it
        // drains, every PROGRESS_TICK, independently of whatever this loop is
        // awaiting (busy-channel holdoff, decode), so audio already playing
        // is never starved mid-frame.
        let feeder = {
            let tx_output = self.tx_output.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(PROGRESS_TICK);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    lock_tx_output(&tx_output).flush();
                }
            })
        };
        // VARA command-port keepalive: `IAMALIVE` to every client every
        // `vara_keepalive_period`, from a task of its own so a stalled event
        // loop (see `wait_for_clear_channel`) cannot block it.
        // `set_vara_responses` is always called before `run()` (main.rs,
        // tests), so a wired daemon always gets the keepalive.
        let keepalive = self
            .vara_responses
            .clone()
            .map(|senders| spawn_vara_keepalive(senders, self.vara_keepalive_period));

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
                        Some(DaemonEvent::TxComplete { generation, completed }) => {
                            self.handle_tx_complete(generation, completed).await;
                        }
                    }
                }
                _ = audio_poll.tick() => {
                    self.poll_audio_input().await;
                }
                _ = retransmit_poll.tick() => {
                    // E6: Check for ARQ retransmits
                    self.check_arq_retransmits().await;
                    self.retry_held_tx_drain().await;
                }
                _ = beacon_poll.tick() => {
                    self.maybe_send_beacon().await;
                }
                _ = session_cleanup.tick() => {
                    self.reap_timed_out_sessions().await;
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

        feeder.abort();
        if let Some(keepalive) = keepalive {
            keepalive.abort();
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
        // A reply to a frame in that audio may have held TX (COP-60) and
        // parked newer audio again. The ring is newer still, so leave it for
        // the next poll, which decodes the parked audio first.
        if !self.pending_busy_wait_audio.is_empty() {
            return;
        }
        if let Some(buf) = self.drain_audio_input_ring() {
            self.handle_audio_in(&buf).await;
        }
    }

    /// Read available audio input samples and feed them to
    /// `busy_gate.observe` only -- no frame decode, no `MacPdu` dispatch.
    ///
    /// Used exclusively by `wait_for_clear_channel`'s poll loop (Finding 1,
    /// Phase 4 Task 3 review) and `wait_for_rx_turnaround`'s (COP-60, which
    /// needs only the drain) so real RX audio keeps updating busy-gate
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
            self.observe_raw_rx(&buf).await;
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

                // If there's an established session, wrap data in a MAC PDU.
                // The frame's source is the SESSION's own local callsign (not
                // `self.local_callsign`, which a mid-session `MYCALL` can
                // change): the peer looks the session up by `pdu.src`, so a
                // runtime callsign change must not make it silently drop
                // every later data frame.
                let session_info = self.session_mgr.active_sessions().iter().find_map(|&id| {
                    self.session_mgr
                        .get(id)
                        .filter(|s| s.is_established())
                        .map(|s| (id, s.remote.clone(), s.local.clone(), s.ssid))
                });

                if let Some((session_id, remote, local, ssid)) = session_info {
                    // COP-41: one host event is NOT one over-the-air frame.
                    // Split the event into chunks that each encode at the
                    // engine's current speed level (D2: measured via
                    // `encoded_payload_len`, never predicted -- compression
                    // is content-dependent by ~2x), then pace them one frame
                    // per keyed interval through `session_tx_queue` (D3).
                    let budget = max_payload_for_level(self.engine.speed_level())
                        .expect("the engine's speed level is always a valid wire level");
                    let chunks = chunk_to_fit(&data, MacPdu::HEADER_SIZE, budget, |c| {
                        let pdu = MacPdu::new_data(remote.clone(), local.clone(), ssid, c.to_vec());
                        self.engine.encoded_payload_len(&pdu.to_bytes()) <= budget
                    });
                    for chunk in chunks {
                        let host_len = chunk.len();
                        let pdu = MacPdu::new_data(remote.clone(), local.clone(), ssid, chunk);
                        self.session_tx_queue.push_back(SessionTxChunk {
                            session_id,
                            pdu_bytes: pdu.to_bytes(),
                            host_len,
                        });
                    }
                    self.report_buffer().await;
                    self.drain_session_tx_queue().await;
                    return;
                }

                // No established session (none at all, or one still
                // connecting): drop rather than key the transmitter for
                // nobody, unless the operator opted into sessionless TX.
                if !self.config.host.sessionless_tx_enabled {
                    tracing::warn!(
                        client_id,
                        bytes = data.len(),
                        "No established session; host data dropped, not transmitted"
                    );
                    return;
                }

                // COP-37: refuse here, before ARQ registration or queueing,
                // so a refused payload leaves no outstanding ARQ segment or
                // `BUFFER` count behind. (An established session above
                // implies a callsign: both connect paths require one.)
                if self.local_callsign.is_none() {
                    tracing::error!(client_id, bytes = data.len(), "{NO_CALLSIGN_TX_REFUSAL}");
                    return;
                }

                // Sessionless TX: raw/ARQ encode path.
                // COP-41: this path chunks the event too. With ARQ enabled,
                // the whole event's bytes go to `arq_tx_backlog` and
                // `pump_arq_backlog` segments them against the
                // ladder-minimum budget as window space allows (D4/D5) --
                // window-full is a normal pump stop (bytes are held, never
                // dropped), not the drop-with-warning it used to be.
                if self.config.engine.arq_enabled && self.arq_tx.is_some() {
                    self.arq_tx_backlog.extend(data.iter().copied());
                    self.report_buffer().await;
                    self.pump_arq_backlog().await;
                } else {
                    let budget = max_payload_for_level(self.engine.speed_level())
                        .expect("the engine's speed level is always a valid wire level");
                    for chunk in chunk_to_fit(&data, 0, budget, |c| {
                        self.engine.encoded_payload_len(c) <= budget
                    }) {
                        // Queue for encode+transmit (`BUFFER` telemetry counts
                        // the host bytes still held; see `buffered_tx_bytes`).
                        // Chunks partition the event, so the byte totals
                        // COP-30's tests pin still hold.
                        self.enqueue_tx(None, chunk).await;
                    }
                }
            }
            HostEvent::VaraCommand { client_id, command } => {
                tracing::debug!(client_id, command = %command, "VARA command received");
                let command = VaraCommand::parse(&command);
                // Reply first: it answers the command itself, so it reaches
                // the client before anything the command causes (TUNE's PTT
                // lines).
                self.reply_vara(client_id, vara_reply(&command)).await;
                match command {
                    VaraCommand::Listen(on) => {
                        self.listening = on;
                        if on {
                            tracing::info!("Listening for incoming connections");
                        } else {
                            tracing::info!("Stopped listening for incoming connections");
                        }
                    }
                    VaraCommand::Tune(seconds) => {
                        // Task 1 (Phase 4): TX level calibration. `TUNE` (or
                        // `TUNE <seconds>`) generates the standard SSB
                        // two-tone calibration signal and sends it through
                        // the same PTT-key/stream/PTT-unkey path real frames
                        // use (`transmit_samples`), so an operator can set
                        // their radio's audio drive level via ALC exactly as
                        // they would for real traffic.
                        let seconds = seconds.unwrap_or(10.0);
                        tracing::info!(seconds, "TUNE: transmitting TX-level calibration tone");
                        let samples = self.engine.tune_tone(seconds, None);
                        self.transmit_samples(&samples).await;
                    }
                    VaraCommand::MyCall(call) => match Callsign::new(&call) {
                        Ok(callsign) => {
                            tracing::info!(callsign = %callsign, "Local callsign set by MYCALL");
                            self.local_callsign = Some(callsign);
                        }
                        // Already answered WRONG by `vara_reply`; keep the
                        // callsign in force.
                        Err(e) => {
                            tracing::warn!(callsign = %call, error = %e, "MYCALL rejected")
                        }
                    },
                    // VERSION is answered by the reply itself. COMPRESSION,
                    // BW* and ABORT are accepted but not implemented.
                    // CONNECT/DISCONNECT arrive as
                    // ConnectRequest/DisconnectRequest.
                    _ => {}
                }
            }
            HostEvent::ConnectRequest {
                client_id,
                source: _,
                destination,
            } => {
                tracing::info!(client_id, destination = %destination, "Connect request");
                // Reply first (never WRONG: Pat's dial waits only on
                // CONNECTED/DISCONNECTED), before every early-return
                // DISCONNECTED and before the CONNECT_REQ goes on air.
                self.reply_vara(client_id, VaraResponse::Ok).await;

                let local = match self.local_callsign {
                    Some(ref cs) => cs.clone(),
                    None => {
                        tracing::warn!("Connect request but no local callsign configured");
                        self.notify_session(client_id, SessionNotice::Disconnected)
                            .await;
                        return;
                    }
                };

                let remote = match Callsign::new(&destination) {
                    Ok(cs) => cs,
                    Err(e) => {
                        tracing::warn!(error = %e, "Invalid destination callsign");
                        self.notify_session(client_id, SessionNotice::Disconnected)
                            .await;
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
                                    self.notify_session(client_id, SessionNotice::Disconnected)
                                        .await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to create session");
                        self.notify_session(client_id, SessionNotice::Disconnected)
                            .await;
                    }
                }
            }
            HostEvent::DisconnectRequest { client_id } => {
                tracing::info!(client_id, "Disconnect request");
                self.reply_vara(client_id, VaraResponse::Ok).await;

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
                    // The session's unsent chunks end with it; none may
                    // play after the host is told DISCONNECTED.
                    self.discard_session_tx(id).await;
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
                    self.notify_session(client_id, SessionNotice::Disconnected)
                        .await;
                } else {
                    tracing::debug!("Disconnect request but no active session");
                }
            }
        }
    }

    async fn handle_audio_in(&mut self, samples: &[f32]) {
        self.observe_raw_rx(samples).await;
        #[cfg(feature = "websocket")]
        self.maybe_broadcast_spectrum(samples);
        self.decode_and_dispatch_audio(samples).await;
    }

    /// The observation prelude shared by both raw RX taps (`handle_audio_in`
    /// and `observe_busy_gate_from_audio_input`). Each raw sample passes
    /// through exactly one of them, so everything here sees every sample
    /// exactly once; decode stays split out (Finding 1) and is not part of
    /// this.
    async fn observe_raw_rx(&mut self, samples: &[f32]) {
        // Spectral-occupancy busy gate (decision 8): fed every incoming audio
        // block, regardless of whether it ends up containing a decodable frame.
        // Only emits telemetry on an actual BUSY ON/OFF transition.
        if let Some(new_state) = self.busy_gate.observe(samples) {
            self.emit_vara(VaraResponse::Busy(new_state)).await;
        }
        self.observe_rx_level(samples);
    }

    /// COP-65: meter raw RX audio (both raw taps call this through
    /// `observe_raw_rx`, so every sample is metered exactly once). Logs a
    /// WARN/INFO line only when the RX level state changes, never per window,
    /// and broadcasts one `audio_level` message per 100 ms window when a
    /// WebSocket host is attached.
    ///
    /// Paused while transmitting: the input then carries the radio's muted
    /// output or its own TX monitor, not the channel. The in-progress window
    /// is discarded rather than merged with post-TX audio.
    fn observe_rx_level(&mut self, samples: &[f32]) {
        if self.is_transmitting {
            self.rx_level_meter.discard_partial_window();
            return;
        }
        for update in self.rx_level_meter.observe(samples) {
            if update.transition.is_some() {
                log_rx_level_transition(update.state, &update.reading);
            }
            #[cfg(feature = "websocket")]
            self.broadcast_audio_level(
                coppa_host::websocket::AudioLevelDirection::Rx,
                &update.reading,
                update.state,
            );
        }
    }

    /// COP-65: measure one whole transmission as it will play (station-ID
    /// prepend included), log one `TX audio level` INFO line, warn if any
    /// sample is digitally clipped, and broadcast a `tx` `audio_level`
    /// message when a WebSocket host is attached.
    fn report_tx_level(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let reading = level_meter::measure(samples);
        let state = level_meter::tx_state(&reading);
        tracing::info!(
            peak_dbfs = level_meter::round_db(reading.peak_dbfs),
            rms_dbfs = level_meter::round_db(reading.rms_dbfs),
            duration_ms = level_meter::duration_ms(&reading, self.config.audio.sample_rate),
            "TX audio level"
        );
        if state == LevelState::Clipping {
            tracing::warn!(
                peak_dbfs = level_meter::round_db(reading.peak_dbfs),
                clipped_percent = level_meter::round_db(reading.clip_fraction * 100.0),
                "{}",
                level_meter::TX_CLIPPING_MESSAGE
            );
        }
        #[cfg(feature = "websocket")]
        self.broadcast_audio_level(
            coppa_host::websocket::AudioLevelDirection::Tx,
            &reading,
            state,
        );
    }

    /// Broadcast one `audio_level` message over `ws_broadcast` (COP-65). A
    /// no-op when no host is attached, like `maybe_broadcast_spectrum`;
    /// per-client opt-in filtering happens on the `coppa-host::websocket`
    /// side.
    #[cfg(feature = "websocket")]
    fn broadcast_audio_level(
        &self,
        direction: coppa_host::websocket::AudioLevelDirection,
        reading: &LevelReading,
        state: LevelState,
    ) {
        let Some(ref ws_tx) = self.ws_broadcast else {
            return;
        };
        let msg = level_meter::ws_message(direction, reading, state, self.config.audio.sample_rate);
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = ws_tx.send(json);
        }
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
                    // COP-60: whoever sent this frame is still keyed for its PTT tail.
                    // Anchored at decode, which is never earlier than the frame's last
                    // sample arriving.
                    self.rx_turnaround_until = Some(
                        tokio::time::Instant::now()
                            + Duration::from_millis(self.config.radio.turnaround_ms),
                    );

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
                    // updates. `notify_session` also refreshes connected/remote_call
                    // at every transition, without waiting for another decode.
                    #[cfg(feature = "websocket")]
                    if let Some(ref status) = self.ws_status {
                        let remote = self.established_remote();
                        let mut snap = status.lock().await;
                        snap.connected = remote.is_some();
                        snap.remote_call = remote.map(|cs| cs.to_string());
                        snap.snr = Some(snr_db.round() as i32);
                        snap.level = Some(speed_level);
                        snap.cfo = Some(cfo_hz);
                        snap.short_cp_ok = if self.config.engine.cp_gate_enabled {
                            Some(self.cp_gate.current() == CpRecommendation::ShortCp)
                        } else {
                            None
                        };
                        // COP-2: surface the desync canary where an operator can see
                        // it. `check_cp_desync` runs off the 500 ms retransmit poll and
                        // is not `async`, so it cannot take this lock itself; it
                        // increments a plain counter on `EventLoop` and this
                        // already-async snapshot point publishes it. That inherits the
                        // snapshot's existing "only refreshed on a decode event"
                        // limitation, which is acceptable here for the same reason it is
                        // for `connected`: the counter is monotonic, so a stale read
                        // under-reports a live problem but can never invent one.
                        snap.cp_desync_episodes = self.cp_desync_episodes;
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
                                            self.report_buffer().await;
                                            // COP-41: the ACK freed window
                                            // space -- move backlog bytes
                                            // into new segments.
                                            self.pump_arq_backlog().await;
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
                                        self.report_buffer().await;
                                        // COP-41: the ACK freed window space
                                        // -- move backlog bytes into new
                                        // segments.
                                        self.pump_arq_backlog().await;
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
                                                if let Err(e) = self.apply_speed_level(
                                                    self.rate_loop.current_level(),
                                                ) {
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
                                        // The rebuilt `arq_tx` holds nothing,
                                        // so its old segments leave `BUFFER`.
                                        self.report_buffer().await;
                                        // COP-41: the fresh (empty) window
                                        // drains the backlog into new
                                        // segments.
                                        self.pump_arq_backlog().await;
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

    /// Low-level, lossless queueing of raw samples for the audio-out ring.
    /// Does NOT assert PTT, does NOT apply busy-channel-courtesy deferral,
    /// and does NOT trigger the station-ID timer -- callers that are actually
    /// keying a transmitter must go through `transmit_samples` (the real PTT
    /// chokepoint) instead, which itself calls this as its last step. The
    /// only other direct caller today is the `DaemonEvent::AudioOut` arm in
    /// `run()`, a raw pass-through hook (unused by any in-tree production TX
    /// path as of this writing; exercised only by this file's own tests) --
    /// a future real caller of that event should route through
    /// `transmit_samples` instead if it represents an actual transmission.
    ///
    /// Samples the ring has no room for are retained in `tx_output`
    /// (in order, behind anything already pending) and moved on later TX
    /// ticks; nothing is dropped. Raw samples share that FIFO and its
    /// cumulative count, so they cannot corrupt a keyed job's playback target.
    ///
    /// Returns false when the samples could not be queued because no live
    /// output ring is wired.
    fn handle_audio_out(&mut self, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return true;
        }
        if self.tx_output().submit(samples).is_none() {
            tracing::debug!(samples = samples.len(), "Audio out: no output device wired");
            return false;
        }
        true
    }

    /// The shared TX output state (see `tx_output`).
    fn tx_output(&self) -> std::sync::MutexGuard<'_, TxOutput> {
        lock_tx_output(&self.tx_output)
    }

    /// Move retained TX samples into the audio-out ring as far as it has
    /// room (the feeder task's step, also used by tests that drive delivery
    /// without `run`).
    #[cfg(test)]
    fn flush_pending_audio_out(&self) {
        self.tx_output().flush();
    }

    /// Handle the automatic end of a keyed transmission (see
    /// `DaemonEvent::TxComplete`). Stale generations are ignored so an
    /// earlier job's completion can never unkey a later one. A cancelled job
    /// discards its still-pending samples -- including the tail already
    /// buffered in the final sink -- so they cannot play after PTT is
    /// released, and the next queued transmission keys only once that
    /// discard has taken effect, so the stale tail cannot open its frame.
    async fn handle_tx_complete(&mut self, generation: u64, completed: bool) {
        if generation != self.tx_generation {
            tracing::debug!(
                generation,
                current = self.tx_generation,
                "Ignoring stale TX completion"
            );
            return;
        }
        if !completed {
            let (discarded, submitted) = {
                let mut tx_output = self.tx_output();
                (tx_output.cancel(), tx_output.submitted())
            };
            tracing::error!(
                generation,
                discarded,
                "Transmission cancelled before it finished playing; releasing PTT"
            );
            if let Some(feedback) = self.output_feedback.clone() {
                // Also drop what the output worker and the final sink have
                // not yet played, so none of it plays after PTT is released.
                feedback.discard_through(submitted);
                self.set_ptt(false).await;
                // Hold the next transmission until the cancelled audio is
                // gone; otherwise draining the TX queue rekeys at once and
                // the sink's buffered tail plays at the start of that frame.
                if !wait_for_discard(&feedback, submitted).await {
                    tracing::warn!(
                        generation,
                        "Cancelled TX audio not yet dropped by the output; \
                         new audio stays held behind the drop"
                    );
                }
                self.try_drain_tx_queue().await;
                self.drain_session_tx_queue().await;
                return;
            }
        }
        self.handle_ptt_change(false).await;
    }

    /// Transmit encoded audio samples: assert PTT, queue every sample for the
    /// audio-out ring, and release PTT once the last one has played.
    ///
    /// Returning from this function does NOT mean the audio has played: the
    /// samples are retained and paced into the bounded rings by `run`'s TX
    /// tick, and a spawned task waits for the output worker to report that
    /// the last sample's predicted presentation time has passed (then the PTT
    /// tail), or cancels the transmission on output failure, five seconds
    /// without playback progress, or `max_tx_duration_s`. Both bounds, and
    /// the PTT tail, are measured across the continuous keyed interval rather
    /// than per job, so transmissions queued behind an active one can neither
    /// restart them nor stretch the keyed time past `max_tx_duration_s`.
    ///
    /// This is the single chokepoint every TX path in this event loop funnels
    /// through (host-driven encode, ARQ-adjacent session control frames
    /// including session keepalives, the raw/ARQ TX-queue drain, ARQ
    /// retransmits, `TUNE`), so busy-channel courtesy (Phase 4 Task 3,
    /// decision: gate here rather than duplicate at each call site), the
    /// RX->TX turnaround guard (COP-60, `wait_for_rx_turnaround`) and the
    /// station-ID timer's prepend all live here and apply uniformly to
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
    ///
    /// COP-40: the wideband regulatory gate runs first, before the busy
    /// wait, and again after the wait before the station-ID prepend, PTT,
    /// or any audio. A refused transmission keys nothing and queues
    /// nothing. Callers observe a lost transmission, which ARQ already
<<<<<<< HEAD
    /// retransmits or times out on like any other loss. COP-21: a refusal
    /// also rebinds a level-derived engine to HF, so the next frame it
    /// encodes is HF-legal; the refused samples themselves are never
    /// re-encoded.
=======
    /// retransmits or times out on like any other loss.
    ///
    /// COP-37: before either, no valid local callsign (`local_callsign` is
    /// `None`: `[engine] callsign` empty or invalid and no valid `MYCALL`
    /// since) refuses the transmission outright, logging
    /// `NO_CALLSIGN_TX_REFUSAL` at `ERROR` for each attempt. This covers
    /// every caller, including `TUNE`, which no upstream feature gate sees.
>>>>>>> af7712e23e02222bf6db1e6e23da05aa8795fe2c
    async fn transmit_samples(&mut self, samples: &[f32]) {
        if self.local_callsign.is_none() {
            tracing::error!("{NO_CALLSIGN_TX_REFUSAL}");
            if !self.is_transmitting {
                // As for the wideband refusal below, the refused payload
                // never reaches the air, so no PTT release will drop it from
                // `BUFFER`. Nothing is held for a retry: a payload is only
                // queued with a callsign in force (see the sessionless
                // `DataReceived` path), and nothing clears it once set.
                self.tx_on_air_bytes = 0;
                self.report_buffer().await;
            }
            return;
        }
        let allowed = if self.wideband_frequency_check().is_ok() {
            self.wait_for_clear_channel().await;
            self.wait_for_rx_turnaround().await;
            // The rig can retune during the unbounded courtesy wait and
            // its backoff. Resolve fresh evidence before committing to TX.
            self.wideband_frequency_check().is_ok()
        } else {
            false
        };
        if !allowed {
            // COP-21: the refused samples were encoded on vhf_wide; make the next
            // frame a level-derived engine encodes HF-legal.
            self.keep_level_derived_engine_hf_legal(self.engine.speed_level());
            if !self.is_transmitting {
                // The refused payload never reaches the air, so no PTT
                // release will ever drop it from `BUFFER`.
                self.tx_on_air_bytes = 0;
                self.report_buffer().await;
                // The drain that popped this payload passed its own gate
                // check and therefore cleared `tx_held_by_frequency_gate`,
                // but the rig can retune below 29.7 MHz during the
                // unbounded courtesy wait above, and this refusal then
                // creates no TxComplete/PTT-release event that would ever
                // re-trigger a drain. Re-arm the held flag while payloads
                // remain queued so the 500 ms `retry_held_tx_drain` poll
                // resumes the queue once the gate reopens.
                self.tx_held_by_frequency_gate = !self.tx_queue.is_empty();
                self.session_tx_held_by_frequency_gate = !self.session_tx_queue.is_empty();
            }
            return;
        }
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

        // COP-65: the complete waveform about to play, after the gate (a
        // refused transmission returned above and reports nothing).
        self.report_tx_level(samples);

        // Assert PTT before transmitting
        self.handle_ptt_change(true).await;
        let keyed_since = *self
            .tx_keyed_since
            .get_or_insert_with(tokio::time::Instant::now);
        let played_now = self.output_feedback.as_ref().map_or(0, |f| f.played());
        let stall_clock = self
            .tx_stall_clock
            .get_or_insert_with(|| Arc::new(StallClock::new(played_now, keyed_since)))
            .clone();
        // Enforce PTT pre-delay before writing audio
        let pre_delay_ms = self.config.radio.ptt_pre_delay_ms;
        if pre_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(pre_delay_ms)).await;
        }
        let prior_target = self.tx_output().submitted();
        let queued = self.handle_audio_out(samples);
        // Release PTT once this transmission's last sample has played (not
        // after its nominal duration): every sample stays queued until the
        // rings accept it, so the cumulative submitted count is this job's
        // playback target. Completion arrives as a generation-tagged
        // `TxComplete`, so only the newest keyed transmission can unkey.
        self.tx_generation += 1;
        let generation = self.tx_generation;
        let target = self.tx_output().submitted();
        let tail = std::time::Duration::from_millis(self.config.radio.ptt_tail_delay_ms);
        // What is left of `max_tx_duration_s` for the continuous keyed
        // interval this job joins. A job queued behind an active one gets
        // only the remainder, so its watchdog fires no later than the active
        // job's would have.
        let deadline =
            keyed_since + std::time::Duration::from_secs(self.config.radio.max_tx_duration_s);
        let max_tx = deadline.saturating_duration_since(tokio::time::Instant::now());
        // Whenever an output worker is wired, completion comes only from its
        // feedback -- even if its end of the ring is gone, so a failed or
        // dead output cancels the transmission instead of reporting success.
        let feedback = self.output_feedback.clone();
        // Headless fallback (no output worker reporting progress): the
        // nominal audio duration plus the ring drain time, as before.
        let sample_rate = self.config.audio.sample_rate.max(1) as u64;
        let nominal = std::time::Duration::from_millis(
            (samples.len() as u64 * 1000) / sample_rate
                + (self.config.audio.buffer_size as u64 * 1000) / sample_rate,
        );
        let ptt_event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let completed = match feedback {
                // An output worker is wired but its ring is gone: nothing
                // can play, so cancel at once rather than report success.
                Some(_) if !queued => {
                    tracing::error!(generation, "TX audio could not be queued; output is down");
                    false
                }
                Some(feedback) => {
                    let watch = PlayoutWatch::new(target, Instant::now(), Some(max_tx));
                    let interval = IntervalProgress {
                        clock: &stall_clock,
                        prior_target,
                    };
                    match wait_for_playout(&feedback, watch, Some(interval)).await {
                        Ok(()) => true,
                        Err(why) => {
                            tracing::error!(generation, reason = %why, "TX playback did not complete");
                            false
                        }
                    }
                }
                None => {
                    let capped = nominal.min(max_tx);
                    if nominal > max_tx {
                        tracing::warn!(
                            tx_duration_ms = nominal.as_millis() as u64,
                            remaining_max_tx_ms = max_tx.as_millis() as u64,
                            "TX duration exceeds what is left of max TX; capping PTT release delay"
                        );
                    }
                    tokio::time::sleep(capped).await;
                    true
                }
            };
            if completed {
                // The tail counts against the keyed interval's
                // `max_tx_duration_s` bound too: a transmission that finished
                // playing near the deadline must not stay keyed past it.
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if tail > remaining {
                    tracing::warn!(
                        tail_ms = tail.as_millis() as u64,
                        remaining_max_tx_ms = remaining.as_millis() as u64,
                        "PTT tail exceeds what is left of max TX; capping it"
                    );
                }
                tokio::time::sleep(tail.min(remaining)).await;
            }
            if let Err(e) = ptt_event_tx
                .send(DaemonEvent::TxComplete {
                    generation,
                    completed,
                })
                .await
            {
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
    /// practice, not a regulatory identification requirement. Confirmed with
    /// Tony (project owner) as a deliberate design decision, not an oversight
    /// against the brief's literal "all three features off when callsign
    /// unset" text. Since COP-37 `transmit_samples` refuses a transmission
    /// with no callsign before it ever reaches this wait.
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

    /// COP-60 RX->TX turnaround: hold this key-up until `rx_turnaround_until`.
    /// A no-op when no frame was decoded within `turnaround_ms`, or when PTT
    /// is already asserted (no key-up edge to protect). Drains the input ring
    /// while it waits, via the same no-dispatch helper `wait_for_clear_channel`
    /// uses: the 8,192-sample production ring holds 170 ms, so a plain sleep
    /// drops RX audio.
    async fn wait_for_rx_turnaround(&mut self) {
        if self.is_transmitting {
            return;
        }
        let Some(until) = self.rx_turnaround_until else {
            return;
        };
        let remaining = until.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        tracing::debug!(
            wait_ms = remaining.as_millis() as u64,
            "Holding TX for RX->TX turnaround"
        );
        loop {
            self.observe_busy_gate_from_audio_input().await;
            let remaining = until.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            tokio::time::sleep(remaining.min(TURNAROUND_POLL)).await;
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
                if let Err(e) = self.apply_speed_level(self.rate_loop.current_level()) {
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
    /// one-frame-at-a-time pattern: `transmit_samples` tracks its own
    /// playback completion asynchronously per call (a generation-tagged
    /// `TxComplete`, so only the newest call can release PTT), and every
    /// call's samples join one FIFO behind the previous call's, so sending
    /// retransmit N+1 only after
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
        let mut timed_out = false;
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
                timed_out = true;
            }

            for seq in retransmit_seqs {
                if let Some(data) = arq_tx.get_segment_data(seq) {
                    let pdu =
                        TransportPdu::new_reliable(self.arq_session_id, seq, 0, data.to_vec());
                    retransmit_pdus.push((seq, pdu.to_bytes()));
                }
            }
        }
        // Applied after the `arq_tx` borrow ends: `apply_speed_level` needs all of
        // `self` (COP-21), and must still run before the encode below.
        if timed_out {
            if let Err(e) = self.apply_speed_level(self.rate_loop.current_level()) {
                tracing::warn!(error = %e, "Failed to apply RateLoop's recommended speed level after timeout");
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
        // COP-2 review finding: publish the desync counter here too, not only on
        // the next successful decode -- see `publish_desync_episodes`'s doc.
        self.publish_desync_episodes().await;
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
    /// Confirm seq was dropped before the G2 block could read and
    /// abandon it -- leaking the very window slot G2 exists to reclaim.
    ///
    /// So: read every tracked leg first, decide, then abandon them all,
    /// `abort()` once, and `set_cp_profile` once. Abandoning the whole set
    /// together is also what makes the window really come back to
    /// `in_flight() == 0` -- `ArqTx::abandon` only advances `send_base` over a
    /// fully-resolved prefix, so abandoning a subset frees no room (see its
    /// doc). That block lives in [`EventLoop::resolve_cp_giveups`], split out
    /// only so COP-2's desync canary can carry its own doc without burying
    /// this one; the order and the semantics are unchanged.
    fn drive_cp_negotiation(&mut self, now: Instant) {
        if !self.config.engine.cp_negotiation_enabled {
            return;
        }

        self.check_cp_desync();
        self.resolve_cp_giveups(now);
    }

    /// COP-2 CP-mode desync canary: a regression tripwire, latched, covering both
    /// desyncs that can leave this station transmitting a waveform its peer cannot
    /// decode.
    ///
    /// # The two conditions, and why one is not enough
    ///
    /// 1. **Bookkeeping desync** -- `engine.cp_mode() != cp_negotiator.current()`.
    ///    After `set_speed_level` stopped rewriting `cp_mode` no production path can
    ///    diverge these (every `set_cp_profile` caller passes the mode the
    ///    negotiator just moved to), so this is future-regression cost only.
    /// 2. **Profile desync at VHF** -- `engine.on_vhf_profile() &&
    ///    cp_negotiator.current() != LongCp`. Condition 1 is structurally blind to
    ///    it. COP-2 redefined `cp_mode()` to report the retained, *dormant*
    ///    negotiated mode while the engine is on the VHF profile (see
    ///    `CoppaCore::cp_mode`'s doc) while `select_ofdm_profile` builds `vhf_wide`
    ///    regardless -- so a station that climbed to VHF with `ShortCp` negotiated
    ///    is on air as `vhf_wide` while its peer listens on
    ///    `hf_standard_short_cp`, mutually undecodable, and condition 1 reads
    ///    perfectly consistent. COP-23 made this unreachable for HF-configured
    ///    engines (every named HF profile, the daemon's default included): their
    ///    band is pinned, so a `RateLoop` climb past level 5 stays on an HF profile
    ///    and the negotiated mode stays on air. It is still reachable for engines
    ///    that genuinely resolve to VHF -- the `OfdmBand::LevelDerived` fallback
    ///    `EventLoop::new` builds for an unrecognized `[engine] profile`, once it
    ///    climbs past level 5, and `VHF_FAST`-configured stations -- which is why
    ///    the check reads the engine's resolved band (`on_vhf_profile()`) rather than
    ///    the raw speed level: the level alone would false-fire on every healthy
    ///    HF-configured link above level 5.
    ///
    /// # Condition 2's known false-positive, and why it stays as-is
    ///
    /// Review finding (COP-2): if the PEER is *also* on the VHF profile with
    /// `ShortCp` negotiated, its engine builds `vhf_wide` too --
    /// both waveforms match, nothing is actually desynced, yet this station still
    /// flags itself because the check is entirely local: `on_vhf_profile()` here
    /// reflects this station's own band and RateLoop-driven speed level, and this
    /// daemon has no channel carrying the peer's speed level or on-air profile. Distinguishing "peer independently reached VHF too" from "peer is
    /// still on HF and can no longer decode me" needs peer-profile telemetry that
    /// does not exist on the wire today -- adding it is a protocol change, not a
    /// canary fix, and out of COP-2's scope.
    ///
    /// Left in place anyway, deliberately: condition 2 is the only branch that
    /// fires in practice (condition 1 is future-regression cost only, see above),
    /// so removing or gating it away would leave this canary permanently silent on
    /// its one real job. `check_cp_desync`'s log line reflects the asymmetry --
    /// worded as a confirmed violation only when condition 1 also holds, and as a
    /// local-evidence-only "possible" desync when condition 2 fired alone -- but
    /// `cp_desync_episodes` still counts either, since a local-only signal that
    /// sometimes over-reports remains more useful to an operator than no signal at
    /// all for the case that dominates in the field.
    ///
    /// # It does not self-heal, and it does not panic
    ///
    /// Repairing here would mask the regression and, on a 500 ms poll, could rebuild
    /// the transceiver twice a second against whatever kept rewriting it. Not a
    /// `debug_assert!` either: a desync is a recoverable link condition, and
    /// panicking inside the daemon's poll loop is a worse failure than warning.
    ///
    /// # Cost
    ///
    /// Three field reads per tick -- but only once `arq_enabled` is on, since
    /// `check_arq_retransmits` returns before `drive_cp_negotiation` is ever reached
    /// without it. On the shipped default this runs zero times (see
    /// `DaemonConfig`'s `cp_negotiation_enabled` doc).
    fn check_cp_desync(&mut self) {
        let engine_cp_mode = self.engine.cp_mode();
        let negotiator_cp_mode = self.cp_negotiator.current();
        let speed_level = self.engine.speed_level();
        // On an HF profile `cp_mode()` is the profile on air; on the VHF one,
        // `select_ofdm_profile` ignores the mode and builds `vhf_wide`. Which
        // profile is selected depends on the engine's band, not the level alone
        // (COP-23: an HF-configured engine stays on HF above level 5).
        let cp_mode_in_effect = !self.engine.on_vhf_profile();

        let bookkeeping_desync = engine_cp_mode != negotiator_cp_mode;
        let vhf_profile_desync = !cp_mode_in_effect && negotiator_cp_mode != CpMode::LongCp;
        let desynced = bookkeeping_desync || vhf_profile_desync;

        if desynced && !self.cp_desync_warned {
            self.cp_desync_warned = true;
            self.cp_desync_episodes = self.cp_desync_episodes.saturating_add(1);
            // Condition 1 (bookkeeping_desync) is confirmed by construction --
            // both sides of the comparison are this station's own state. Condition
            // 2 alone (vhf_profile_desync) is local evidence only: it cannot rule
            // out the peer having independently reached VHF too, in which case
            // both waveforms already match and nothing is actually desynced (see
            // this fn's doc, "Condition 2's known false-positive").
            let message = if bookkeeping_desync {
                "CP mode desync (COP-2 invariant violated); logged ONCE per episode, \
                 not repaired -- see check_cp_desync"
            } else {
                "Possible CP mode desync (VHF profile risk, local evidence only -- \
                 cannot confirm without peer-profile telemetry); logged ONCE per \
                 episode, not repaired -- see check_cp_desync"
            };
            tracing::warn!(
                // Named `engine_cp_mode`, but on the VHF profile it is the DORMANT
                // negotiated mode, not the live waveform -- hence `cp_mode_in_effect`
                // beside it, so a log reader cannot mistake the two.
                ?engine_cp_mode,
                ?negotiator_cp_mode,
                speed_level,
                cp_mode_in_effect,
                bookkeeping_desync,
                vhf_profile_desync,
                episode = self.cp_desync_episodes,
                "{message}"
            );
        } else if !desynced && self.cp_desync_warned {
            self.cp_desync_warned = false;
            tracing::info!(
                ?negotiator_cp_mode,
                speed_level,
                episodes = self.cp_desync_episodes,
                "CP mode desync cleared"
            );
        }
    }

    /// COP-1's G1-G4 give-up triggers, resolved as one pass. Split out of
    /// `drive_cp_negotiation` only so the desync canary above can carry its own doc;
    /// the ordering (canary first, then give-ups) and the semantics are unchanged.
    fn resolve_cp_giveups(&mut self, now: Instant) {
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
            MacFrameType::ConnectCfm => self.handle_connect_cfm_rx(pdu).await,
            MacFrameType::Disconnect => self.handle_incoming_disconnect(pdu).await,
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
                    let notice = SessionNotice::Connected {
                        local: session.local.clone(),
                        remote,
                        we_called: true,
                    };
                    // The session is established now; the busy-channel courtesy
                    // wait in transmit_samples can delay the CFM by seconds.
                    self.notify_session(0, notice).await;
                    let cfm_bytes = cfm_pdu.to_bytes();
                    match self.engine.encode_bytes(&cfm_bytes) {
                        Ok(samples) => self.transmit_samples(&samples).await,
                        Err(e) => tracing::warn!(error = %e, "Failed to encode CONNECT_CFM"),
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

    async fn handle_connect_cfm_rx(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        if let Some(session) = self.session_mgr.find_by_remote_mut(&remote) {
            match session.handle_connect_cfm(&pdu.payload) {
                Ok(()) => {
                    session.confirm_established();
                    tracing::info!(remote = %remote, "Session fully established (responder)");
                    let notice = SessionNotice::Connected {
                        local: session.local.clone(),
                        remote,
                        we_called: false,
                    };
                    self.notify_session(0, notice).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to handle CONNECT_CFM");
                }
            }
        } else {
            tracing::debug!(remote = %remote, "CONNECT_CFM from unknown remote");
        }
    }

    async fn handle_incoming_disconnect(&mut self, pdu: MacPdu) {
        let remote = pdu.src.clone();
        // Find session ID first to avoid borrow issues
        let session_id = self.session_mgr.find_by_remote(&remote).map(|s| s.id);
        if let Some(id) = session_id {
            if let Some(session) = self.session_mgr.get_mut(id) {
                session.handle_disconnect();
            }
            self.session_mgr.remove(id);
            tracing::info!(remote = %remote, "Session disconnected by remote");
            self.discard_session_tx(id).await;
            self.notify_session(0, SessionNotice::Disconnected).await;
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
        self.set_ptt(tx).await;
        if !tx {
            // PTT just released: continue draining the TX queues if more is
            // queued (one frame per keyed interval -- COP-41 paces session
            // chunks the same way `try_drain_tx_queue` paces raw/ARQ ones).
            self.try_drain_tx_queue().await;
            self.drain_session_tx_queue().await;
        }
    }

    /// Change physical PTT and the keyed-interval bookkeeping, without
    /// draining the TX queue on release (see `handle_ptt_change`).
    async fn set_ptt(&mut self, tx: bool) {
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
            self.tx_keyed_since = None;
            self.tx_stall_clock = None;
            // Every frame handed to `transmit_samples` before this release has
            // played or been discarded (only the newest generation unkeys).
            self.tx_on_air_bytes = 0;
            self.report_buffer().await;
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

    /// COP-37: a valid callsign for stations that must transmit. A daemon
    /// with no valid callsign refuses every transmission, so receive-only
    /// and refusal tests keep `DaemonConfig::default()`'s empty one.
    const TEST_CALLSIGN: &str = "N0CALL";

    /// `DaemonConfig::default()` with `TEST_CALLSIGN` configured.
    fn tx_config() -> DaemonConfig {
        let mut config = DaemonConfig::default();
        config.engine.callsign = TEST_CALLSIGN.to_string();
        config
    }

    #[test]
    fn rate_loop_caps_peer_recommendations_at_the_engine_band_limit() {
        for (profile, maximum) in [("HF_STANDARD", 8), ("VHF_FAST", 9)] {
            let mut config = DaemonConfig::default();
            config.engine.profile = profile.to_string();
            // COP-40: a wideband (VHF) profile needs a frequency source.
            config.radio.band = Some("2m".into());
            let mut event_loop = EventLoop::new(config).unwrap();
            for _ in 0..60 {
                event_loop.rate_loop.on_ack(9, true);
            }
            assert_eq!(event_loop.rate_loop.current_level(), maximum, "{profile}");
        }
    }

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
        let mut config = tx_config();
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
        let mut config = tx_config();
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

    // ── COP-2: the desync canary's TRUE branch ────────────────────────────
    //
    // The canary (`EventLoop::check_cp_desync`) is the only production code
    // this branch adds to the daemon, and until these tests nothing took its
    // true branch: every other test in this file that moves `cp_negotiator`
    // also calls `engine.set_cp_profile` with the same mode, so the invariant
    // holds at every `drive_cp_negotiation` call, and the one test that leaves
    // negotiator state armed without syncing the engine returns at the
    // `cp_negotiation_enabled` guard before reaching the canary. A regression
    // detector installed for a FUTURE writer was itself unpinned -- an inverted
    // comparison, a swapped field pair, or the whole block vanishing in a
    // refactor would all have left the suite green.
    //
    // These are STATE-based, not log-based, deliberately: there is no
    // log-capture harness anywhere in the workspace (no `tracing-test`
    // dependency in any Cargo.toml; the only `tracing_subscriber` use is
    // `main.rs:21`), and the latch/counter the canary now carries make its
    // behaviour observable without adding one.

    /// Put a station in the post-success desync state: negotiator settled on
    /// `mode`, engine never told. `on_peer_switched` is what makes it
    /// *post-success* -- it disarms probation and clears `revert_to`, so no
    /// COP-1 give-up trigger is armed and `drive_cp_negotiation` has nothing to
    /// do except run the canary. That is precisely the hole COP-2 exists to
    /// detect: every G1-G4 trigger reads state a completed handshake has
    /// already cleared.
    fn desync_negotiator_only(station: &mut EventLoop, mode: CpMode, now: Instant) {
        station.cp_negotiator.apply_as_confirmer(mode, now);
        assert!(
            station.cp_negotiator.on_peer_switched(mode),
            "test setup: the third leg must be accepted, leaving no armed trigger"
        );
        assert!(
            !station.cp_negotiator.negotiation_in_flight(),
            "test setup: this must be the POST-SUCCESS state, not an in-flight one"
        );
    }

    #[tokio::test]
    async fn the_cp_desync_canary_fires_once_per_episode_and_never_repairs() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(producer);
        let t0 = Instant::now();

        desync_negotiator_only(&mut a, CpMode::ShortCp, t0);
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "test setup: the engine was deliberately NOT told"
        );
        assert_eq!(a.cp_desync_episodes, 0, "test setup: nothing observed yet");

        a.drive_cp_negotiation(t0);

        assert_eq!(
            a.cp_desync_episodes, 1,
            "the canary must see the divergence"
        );
        assert!(a.cp_desync_warned, "and latch it");

        // The documented no-self-heal contract. Repairing here would mask the
        // regression and, on a 500 ms poll, rebuild the transceiver twice a second.
        assert_eq!(
            a.engine.cp_mode(),
            CpMode::LongCp,
            "the canary must NOT repair the engine"
        );
        assert_eq!(
            a.cp_negotiator.current(),
            CpMode::ShortCp,
            "nor rewind the negotiator"
        );
        let mut buf = vec![0.0f32; 4096];
        assert_eq!(
            consumer.read(&mut buf),
            0,
            "a give-up is silent by design and the canary transmits nothing at all"
        );

        // The latch: 500 ms polls do not each emit a fresh episode. Without it a
        // single divergence logs ~7,200 identical lines an hour, indefinitely,
        // because nothing can clear the condition on its own.
        for _ in 0..10 {
            a.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS * 10));
        }
        assert_eq!(
            a.cp_desync_episodes, 1,
            "one episode, however many polls observe it"
        );

        // Cleared when the invariant holds again...
        a.engine.set_cp_profile(CpMode::ShortCp);
        a.drive_cp_negotiation(t0);
        assert!(!a.cp_desync_warned, "the latch must clear");
        assert_eq!(a.cp_desync_episodes, 1, "clearing is not a new episode");

        // ...and a LATER divergence is a genuinely new episode, not swallowed by a
        // stuck latch.
        a.engine.set_cp_profile(CpMode::LongCp);
        a.drive_cp_negotiation(t0);
        assert_eq!(a.cp_desync_episodes, 2);
        assert!(a.cp_desync_warned);
    }

    /// The band-aware clause, and the reason it is not redundant with the
    /// bookkeeping comparison: here engine and negotiator AGREE (both `ShortCp`),
    /// so `engine.cp_mode() != cp_negotiator.current()` is false — yet the station
    /// is on air as `vhf_wide` while its peer listens on `hf_standard_short_cp`.
    ///
    /// COP-23 closed this for every engine built from a named profile (an
    /// HF-configured engine no longer leaves HF at level 5 -- see
    /// `the_cp_desync_canary_does_not_fire_when_an_hf_configured_engine_climbs_past_level_5`),
    /// so the premise needs an engine with no band preference: `CoppaCore::new()`
    /// is `OfdmBand::LevelDerived`, the daemon's own fallback for an unrecognized
    /// `[engine] profile`, and still crosses to `vhf_wide` at level 5. A canary that
    /// checked only the bookkeeping pair would be blind to it.
    #[tokio::test]
    async fn the_cp_desync_canary_sees_a_vhf_profile_desync_that_bookkeeping_cannot() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        // A LevelDerived engine (no band preference), so level 5 is genuinely
        // `vhf_wide` on air.
        a.engine = coppa_engine::CoppaCore::new();
        let t0 = Instant::now();

        desync_negotiator_only(&mut a, CpMode::ShortCp, t0);
        a.engine.set_cp_profile(CpMode::ShortCp);
        a.engine
            .set_speed_level(5)
            .expect("5 is a valid wire speed level");

        assert_eq!(
            a.engine.cp_mode(),
            a.cp_negotiator.current(),
            "test premise: bookkeeping AGREES here, so only the level-aware clause \
             can detect this"
        );
        assert_eq!(a.engine.speed_level(), 5, "test premise: at level 5");
        assert!(
            a.engine.on_vhf_profile(),
            "test premise: genuinely on vhf_wide"
        );

        a.drive_cp_negotiation(t0);

        assert_eq!(
            a.cp_desync_episodes, 1,
            "on air as vhf_wide while the peer believes short-CP HF must be seen"
        );

        // Dropping back into the HF range makes the negotiated mode live again, and
        // the condition clears -- so this clause cannot fire on a station that is
        // simply using short CP correctly.
        a.engine
            .set_speed_level(4)
            .expect("4 is a valid wire speed level");
        a.drive_cp_negotiation(t0);
        assert!(
            !a.cp_desync_warned,
            "back below level 5 the negotiated mode is in effect again"
        );
        assert_eq!(a.cp_desync_episodes, 1);
    }

    /// COP-23's inverse pin: the exact setup the VHF-profile test above used to
    /// run on (an HF-configured `cp_enabled_config()` engine, `ShortCp`
    /// negotiated, level 5) is now a HEALTHY link -- the engine stays on
    /// `hf_standard_short_cp`, the negotiated mode is on air, and nothing is
    /// desynced. A canary still keyed on `speed_level < 5` would false-fire here.
    #[tokio::test]
    async fn the_cp_desync_canary_does_not_fire_when_an_hf_configured_engine_climbs_past_level_5() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let t0 = Instant::now();

        desync_negotiator_only(&mut a, CpMode::ShortCp, t0);
        a.engine.set_cp_profile(CpMode::ShortCp);
        a.engine
            .set_speed_level(5)
            .expect("5 is a valid wire speed level");

        assert_eq!(a.engine.speed_level(), 5, "test premise: at level 5");
        assert!(
            !a.engine.on_vhf_profile(),
            "test premise: an HF-configured engine stays on an HF profile"
        );

        a.drive_cp_negotiation(t0);

        assert_eq!(
            a.cp_desync_episodes, 0,
            "an HF-configured engine at level 5 is still on hf_standard_short_cp, \
             matching its peer -- the canary must not flag it"
        );
        assert!(!a.cp_desync_warned);
    }

    /// Review finding (COP-2): the WebSocket snapshot's `cp_desync_episodes` used
    /// to update only inside `decode_and_dispatch_audio`'s decode-event handling,
    /// so a station whose peer can no longer decode it at all -- precisely the
    /// scenario this canary exists to catch -- could keep publishing a stale `0`
    /// indefinitely. `check_arq_retransmits` (the 500 ms poll `drive_cp_negotiation`
    /// itself runs on) must publish it too, with no decode involved anywhere in
    /// this test.
    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn check_arq_retransmits_publishes_desync_episodes_without_a_decode() {
        let mut a = EventLoop::new(cp_enabled_config()).unwrap();
        let status = Arc::new(Mutex::new(coppa_host::websocket::WsStatus::default()));
        a.set_ws_status(status.clone());
        let t0 = Instant::now();

        desync_negotiator_only(&mut a, CpMode::ShortCp, t0);
        assert_eq!(
            status.lock().await.cp_desync_episodes,
            0,
            "test setup: nothing observed yet"
        );

        // No decode anywhere in this test -- only the poll path.
        a.check_arq_retransmits().await;

        assert_eq!(
            a.cp_desync_episodes, 1,
            "test setup: the internal counter did increment"
        );
        assert_eq!(
            status.lock().await.cp_desync_episodes,
            1,
            "the WebSocket snapshot must see it too, without waiting on a decode"
        );
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
    /// `!(engine.on_vhf_profile() && cp_negotiator.current() != CpMode::LongCp)` --
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

    #[tokio::test]
    async fn real_handshake_pushes_connected_to_both_stations() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".into();
        config.radio.ptt_pre_delay_ms = 0;
        config.radio.ptt_tail_delay_ms = 0;
        let mut a = EventLoop::new(config.clone()).unwrap();
        config.engine.callsign = "W1AW".into();
        let mut b = EventLoop::new(config).unwrap();
        a.listening = true;
        b.listening = true;
        let (a_producer, mut a_consumer) = coppa_audio::audio_ring(1_000_000);
        let (b_producer, mut b_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        b.set_audio_out(b_producer);
        let (a_tx, mut a_rx) = mpsc::channel(64);
        let (b_tx, mut b_rx) = mpsc::channel(64);
        a.set_vara_responses(Arc::new(Mutex::new(HashMap::from([(1, a_tx)]))));
        b.set_vara_responses(Arc::new(Mutex::new(HashMap::from([(1, b_tx)]))));
        fn session_lines(rx: &mut mpsc::Receiver<VaraResponse>) -> Vec<VaraResponse> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|response| {
                    matches!(
                        response,
                        VaraResponse::Connected(..)
                            | VaraResponse::Disconnected
                            | VaraResponse::Pending
                    )
                })
                .collect()
        }
        a.handle_host_event(HostEvent::ConnectRequest {
            client_id: 1,
            source: "VK3ABC".into(),
            destination: "W1AW".into(),
        })
        .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_REQ"))
            .await;
        assert!(
            session_lines(&mut b_rx).is_empty(),
            "REQ is not established"
        );
        a.decode_and_dispatch_audio(&take_leg(&mut b_consumer, "CONNECT_ACK"))
            .await;
        assert_eq!(
            session_lines(&mut a_rx),
            [VaraResponse::Connected(
                "VK3ABC".into(),
                "W1AW".into(),
                2400
            )]
        );
        assert!(
            session_lines(&mut b_rx).is_empty(),
            "responder waits for CFM"
        );
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_CFM"))
            .await;
        assert_eq!(
            session_lines(&mut b_rx),
            [VaraResponse::Connected(
                "VK3ABC".into(),
                "W1AW".into(),
                2400
            )]
        );
        assert!(session_lines(&mut a_rx).is_empty());
    }

    /// COP-28: a `MYCALL` on each station replaces the configured callsign
    /// before the session starts, and both stations' `CONNECTED` lines name
    /// the MYCALL callsigns (caller first). This covers both of Pat's
    /// `handleConnected` arms — the initiator's `src == myCall` and the
    /// responder's `dst == myCall` — which are what keep Pat from panicking
    /// with `unhandled CONNECTED cmd`.
    #[tokio::test]
    async fn mycall_names_both_stations_in_connected() {
        fn session_lines(rx: &mut mpsc::Receiver<VaraResponse>) -> Vec<VaraResponse> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|response| {
                    matches!(
                        response,
                        VaraResponse::Connected(..)
                            | VaraResponse::Disconnected
                            | VaraResponse::Pending
                    )
                })
                .collect()
        }
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".into();
        config.radio.ptt_pre_delay_ms = 0;
        config.radio.ptt_tail_delay_ms = 0;
        let mut a = EventLoop::new(config.clone()).unwrap();
        config.engine.callsign = "W1AW".into();
        let mut b = EventLoop::new(config).unwrap();
        a.listening = true;
        b.listening = true;
        let (a_producer, mut a_consumer) = coppa_audio::audio_ring(1_000_000);
        let (b_producer, mut b_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        b.set_audio_out(b_producer);
        let (a_tx, mut a_rx) = mpsc::channel(64);
        let (b_tx, mut b_rx) = mpsc::channel(64);
        a.set_vara_responses(Arc::new(Mutex::new(HashMap::from([(1, a_tx)]))));
        b.set_vara_responses(Arc::new(Mutex::new(HashMap::from([(1, b_tx)]))));

        a.handle_host_event(HostEvent::VaraCommand {
            client_id: 1,
            command: "MYCALL W5AU".into(),
        })
        .await;
        b.handle_host_event(HostEvent::VaraCommand {
            client_id: 1,
            command: "MYCALL K7XYZ".into(),
        })
        .await;
        assert_eq!(a.local_callsign, Some(Callsign::new("W5AU").unwrap()));
        assert_eq!(b.local_callsign, Some(Callsign::new("K7XYZ").unwrap()));

        a.handle_host_event(HostEvent::ConnectRequest {
            client_id: 1,
            source: "W5AU".into(),
            destination: "K7XYZ".into(),
        })
        .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_REQ"))
            .await;
        a.decode_and_dispatch_audio(&take_leg(&mut b_consumer, "CONNECT_ACK"))
            .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_CFM"))
            .await;

        let expected = [VaraResponse::Connected("W5AU".into(), "K7XYZ".into(), 2400)];
        assert_eq!(session_lines(&mut a_rx), expected);
        assert_eq!(session_lines(&mut b_rx), expected);
    }

    /// COP-28: a `MYCALL` that arrives mid-session must not break the
    /// session. In-session data frames carry the session's OWN local
    /// callsign, not the current `local_callsign`, so the peer's lookup by
    /// `pdu.src` still finds the session and delivers the data.
    #[tokio::test]
    async fn mycall_mid_session_keeps_the_session_callsign() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".into();
        config.radio.ptt_pre_delay_ms = 0;
        config.radio.ptt_tail_delay_ms = 0;
        let mut a = EventLoop::new(config.clone()).unwrap();
        config.engine.callsign = "W1AW".into();
        let mut b = EventLoop::new(config).unwrap();
        a.listening = true;
        b.listening = true;
        let (a_producer, mut a_consumer) = coppa_audio::audio_ring(1_000_000);
        let (b_producer, mut b_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        b.set_audio_out(b_producer);

        // Establish the session as the configured callsigns.
        a.handle_host_event(HostEvent::ConnectRequest {
            client_id: 1,
            source: "VK3ABC".into(),
            destination: "W1AW".into(),
        })
        .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_REQ"))
            .await;
        a.decode_and_dispatch_audio(&take_leg(&mut b_consumer, "CONNECT_ACK"))
            .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_CFM"))
            .await;
        assert!(a.session_mgr.active_sessions().iter().any(|&id| {
            a.session_mgr
                .get(id)
                .map(|s| s.is_established())
                .unwrap_or(false)
        }));

        // Only now wire b's response channel, so the handshake's
        // StatusUpdate lines are dropped and the next delivery is the data
        // frame's DataOut.
        let (data_tx, mut data_rx) = mpsc::channel(64);
        b.set_response_tx(data_tx);

        // Mid-session MYCALL on the sender, then a data frame.
        a.handle_host_event(HostEvent::VaraCommand {
            client_id: 1,
            command: "MYCALL W5AU".into(),
        })
        .await;
        assert_eq!(a.local_callsign, Some(Callsign::new("W5AU").unwrap()));
        a.handle_host_event(HostEvent::DataReceived {
            client_id: 1,
            data: b"hello".to_vec(),
        })
        .await;
        // COP-41: session data is paced one frame per keyed interval, and
        // `a` is still keyed from its CONNECT_CFM playback (whose scheduled
        // release never fires without a `run()` loop) -- release it so the
        // queued chunk drains, exactly as the real release task would.
        a.handle_ptt_change(false).await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "data frame"))
            .await;

        let got = tokio::time::timeout(Duration::from_secs(2), data_rx.recv())
            .await
            .expect("b must deliver the data frame's payload")
            .expect("response channel closed");
        match got {
            coppa_host::HostResponse::DataOut { client_id: _, data } => {
                assert_eq!(data, b"hello");
            }
            other => panic!("expected DataOut, got {:?}", other),
        }
    }

    /// COP-41: a large session write is chunked into frames that each
    /// encode at the current level, paced one per keyed interval, and the
    /// far end reassembles the byte stream exactly -- each chunk is an
    /// ordinary independent MAC data frame, `handle_session_data` forwards
    /// payloads in arrival order, and B2F framing reassembles at the host.
    /// Drives both a 1000-byte write (fast variant) and the ticket's
    /// 8000-byte number through the real encode/audio/decode path.
    #[tokio::test]
    async fn large_session_write_is_chunked_and_delivered_in_order() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".into();
        config.radio.ptt_pre_delay_ms = 0;
        config.radio.ptt_tail_delay_ms = 0;
        let mut a = EventLoop::new(config.clone()).unwrap();
        config.engine.callsign = "W1AW".into();
        let mut b = EventLoop::new(config).unwrap();
        a.listening = true;
        b.listening = true;
        let (a_producer, mut a_consumer) = coppa_audio::audio_ring(4_000_000);
        let (b_producer, mut b_consumer) = coppa_audio::audio_ring(1_000_000);
        a.set_audio_out(a_producer);
        b.set_audio_out(b_producer);

        // Establish the session with the real three-leg handshake.
        a.handle_host_event(HostEvent::ConnectRequest {
            client_id: 1,
            source: "VK3ABC".into(),
            destination: "W1AW".into(),
        })
        .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_REQ"))
            .await;
        a.decode_and_dispatch_audio(&take_leg(&mut b_consumer, "CONNECT_ACK"))
            .await;
        b.decode_and_dispatch_audio(&take_leg(&mut a_consumer, "CONNECT_CFM"))
            .await;

        // Patterned, reproducible payload (compressible enough to exercise
        // the measurement path, not degenerate like a constant fill).
        let pattern =
            |size: usize| -> Vec<u8> { (0..size).map(|i| ((i * 31 + 7) % 251) as u8).collect() };
        for size in [1000usize, 8000] {
            let payload = pattern(size);
            // Fresh response channel per transfer so the collected bytes are
            // exactly this transfer's DataOut stream.
            let (data_tx, mut data_rx) = mpsc::channel(4096);
            b.set_response_tx(data_tx);
            let frames_before = a.tx_frame_count;

            a.handle_host_event(HostEvent::DataReceived {
                client_id: 1,
                data: payload.clone(),
            })
            .await;

            // Drive one chunk per keyed interval: deliver whatever `a` has
            // transmitted, then release PTT to start the next chunk. The
            // first chunk transmits immediately from the `DataReceived`
            // drain when the link is idle (and from the first release when
            // the previous transfer's last frame is still keyed).
            let mut collected: Vec<u8> = Vec::with_capacity(size);
            loop {
                let mut buf = vec![0.0f32; 4_000_000];
                let read = a_consumer.read(&mut buf);
                if read > 0 {
                    unstick_decoder(&mut b);
                    b.decode_and_dispatch_audio(&with_lead_and_trail(&buf[..read]))
                        .await;
                    while let Ok(resp) = data_rx.try_recv() {
                        if let coppa_host::HostResponse::DataOut { data, .. } = resp {
                            collected.extend_from_slice(&data);
                        }
                    }
                }
                if a.session_tx_queue.is_empty() && read == 0 {
                    break;
                }
                if !a.session_tx_queue.is_empty() {
                    a.handle_ptt_change(false).await;
                }
            }

            assert_eq!(
                collected, payload,
                "{size}-byte session write must arrive as the same byte stream"
            );
            // Chunks are at most 102 host bytes at level 2 (budget 117 minus
            // the 14-byte MAC header minus one), so this many bytes cannot
            // fit in fewer frames.
            let min_frames = size.div_ceil(103);
            let frames = a.tx_frame_count - frames_before;
            assert!(
                frames >= min_frames,
                "{size} bytes need >= {min_frames} frames (<= 102 host bytes \
                 each), got {frames}"
            );
            assert!(a.session_tx_queue.is_empty());
        }
    }

    /// COP-41 (D4): `min_level_budget` is derived from the ladder, not a
    /// bare constant -- it must stay at or below every level's budget and
    /// track the ladder minimum through any future renumbering.
    #[test]
    fn min_level_budget_is_the_ladder_minimum() {
        for level in 1..=9u8 {
            assert!(
                min_level_budget() <= max_payload_for_level(level).unwrap(),
                "min_level_budget must not exceed level {level}'s budget"
            );
        }
        // The current ladder's minimum is level 1's 56 bytes (k_used 486:
        // 486/8 - 4 -- see speed_levels.rs's own pinned table).
        assert_eq!(min_level_budget(), 56);
        assert_eq!(min_level_budget(), max_payload_for_level(1).unwrap());
    }

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
    //
    // COP-23 note: these three tests keep their "VHF excursion" names, but
    // `cp_pair()` builds HF_STANDARD-configured engines, whose band is pinned to
    // HF -- so "VHF" below means "speed level >= 5", not the `vhf_wide` profile.
    // Neither station leaves `hf_standard_short_cp` any more, and the negotiated
    // mode stays on air throughout rather than going dormant; every assertion
    // (level, `cp_mode`, decodability) holds a fortiori.

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
        a.rate_loop = RateLoop::new(a.engine.available_speed_levels(), 5, 4);

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
            let ack = TransportPdu::new_ack_with_rate(a.arq_session_id, 0, 0, 9);
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
            "the negotiated CP mode must be RETAINED across the level-5 \
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
        b.rate_loop = RateLoop::new(b.engine.available_speed_levels(), 5, 4).with_probing(1, 1);

        // A REAL ARQ window holding the seq the queued frame names, rather than a
        // bare `Some(0)` with no segment behind it. That is what makes the
        // probe-success half below reachable: `resolve_probe_if_acked` fires only
        // from inside `if let Some(ref mut arq_tx)`, off a seq that
        // `ArqTx::process_ack` really reports as newly acked.
        let mut probe_arq_tx = ArqTx::new(ArqConfig::default());
        let probe_seq = probe_arq_tx
            .send(b"probe me".to_vec(), Instant::now())
            .expect("a fresh ARQ window should have room");
        b.arq_tx = Some(probe_arq_tx);
        b.tx_queue
            .push_back((Some(probe_seq), b"probe me".to_vec()));

        // MANDATORY, not hygiene. `negotiate` made `b` transmit, so
        // `is_transmitting` is still true: PTT release arrives only as a
        // `DaemonEvent::TxComplete` from the spawned playback-completion task
        // through the event channel, which a unit test never drains. Without this line
        // `try_drain_tx_queue` early-returns, no probe is taken,
        // `set_speed_level` is never called, and this test would pass
        // IDENTICALLY before and after the fix -- proving nothing. Poking this
        // field directly has existing precedent in this file (see
        // `enqueue_tx_carries_optional_arq_seq_through_the_queue`).
        b.is_transmitting = false;
        b.try_drain_tx_queue().await;

        assert_eq!(
            b.probe_state,
            Some((probe_seq, 5)),
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

        // ── The probe SUCCEEDS: the longest-lived form of the state ──────────
        //
        // `resolve_probe_if_acked` (`:1722`) is the fifth and last daemon call site
        // that drives `set_speed_level` across the HF/VHF boundary, and it was the
        // one no test exercised across that boundary -- on this branch or on main.
        // It matters more than the other four, not less: `on_probe_result` on a
        // SUCCESSFUL probe PROMOTES rather than reverts, so this is the only path
        // that crosses into VHF and leaves `RateLoop`'s steady-state level up there.
        // That is the longest-lived form of the retained-`ShortCp`-at-VHF state, and
        // the state that used to be the point of no return: pre-fix the mode was
        // already gone, and every later frame -- for the whole VHF dwell and after
        // it -- went out on the wrong profile.
        let ack = TransportPdu::new_ack_with_rate(
            b.arq_session_id,
            probe_seq.wrapping_add(1), // cumulative: "next expected"
            0,
            9,
        );
        unstick_decoder(&mut b);
        b.decode_and_dispatch_audio(&peer_frame(
            &ack,
            coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
        ))
        .await;

        assert_eq!(
            b.probe_state, None,
            "test setup: the ACK must really have resolved the probe"
        );
        assert_eq!(
            b.rate_loop.current_level(),
            5,
            "a SUCCESSFUL probe promotes RateLoop's steady state to the probed \
             level, so `b` now DWELLS at VHF rather than passing through it"
        );
        assert_eq!(b.engine.speed_level(), 5, "test setup: and been applied");
        assert_eq!(
            b.engine.cp_mode(),
            CpMode::ShortCp,
            "the negotiated mode must survive a promotion into VHF, not just a \
             one-frame excursion"
        );
        assert_engine_matches_negotiator(&b, "b");

        // Now come back down through the REAL timeout path and prove the link still
        // works -- the assertion that fails loudest pre-fix.
        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        arq_tx
            .send(
                b"expired segment".to_vec(),
                Instant::now() - Duration::from_millis(100),
            )
            .expect("a fresh ARQ window should have room");
        b.arq_tx = Some(arq_tx);

        b.check_arq_retransmits().await;

        assert_eq!(
            b.rate_loop.current_level(),
            4,
            "test setup: the retransmit timeout must really have stepped 5 -> 4"
        );
        assert_eq!(b.engine.speed_level(), 4, "test setup: and been applied");

        assert_engine_matches_negotiator(&b, "b");
        assert_converged(&a, &b, CpMode::ShortCp);

        let samples = b.engine.encode_bytes(b"still alive").unwrap();
        assert_eq!(
            a.engine
                .decode_bytes(&samples)
                .expect("a must decode b's frame after b dwelt at VHF and returned"),
            b"still alive".to_vec()
        );
    }

    /// The third trajectory: a `RateLoop` excursion across the HF/VHF boundary
    /// **while a negotiation is still armed**.
    ///
    /// Both Phase 5 tests above run `negotiate(...)` to completion first, and their
    /// section comment scopes them deliberately to the window *after* a handshake
    /// fully succeeded. That leaves the in-flight window uncovered, and it is not
    /// the same window: it terminates somewhere neither of them reaches — COP-1's
    /// give-up block, where `abort()` + `set_cp_profile(reverted)` re-applies the
    /// **pre-negotiation** mode. Nothing pinned that the mode `set_speed_level`
    /// retained and the mode the give-up reverts to still agree once a VHF excursion
    /// has happened in between.
    ///
    /// Pre-fix this fails at the mid-excursion assertion, not at the give-up: the
    /// climb to level 5 rewrote `cp_mode` to `LongCp` while the negotiator sat on
    /// `ShortCp`, so engine and negotiator diverged *during* the handshake and every
    /// later decision — including the give-up's own `set_cp_profile` — was taken
    /// against a state that had already drifted.
    ///
    /// Direction note, stated rather than implied: this drives `LongCp -> ShortCp`,
    /// which is the direction `apply_as_confirmer` + probation naturally produces
    /// and the one where the retained mode is a *non-default*. The
    /// `ShortCp -> LongCp` direction — where "pre-negotiation mode" and a hardcoded
    /// `LongCp` genuinely differ, the distinction `cp_negotiator`'s module doc says
    /// got it wrong once — is already covered by COP-1's
    /// `lost_confirm_in_the_short_to_long_direction_*` tests; this one is about the
    /// `set_speed_level` interaction, which those do not exercise.
    #[tokio::test]
    async fn a_rate_loop_vhf_excursion_mid_handshake_still_gives_up_consistently() {
        let (a, mut b, _a_consumer, _b_consumer) = cp_pair();
        let t0 = Instant::now();

        // A real in-flight state, not a simulated one: `b` has switched to ShortCp
        // as the confirmer (probation armed, `revert_to` = LongCp) and has a real
        // CP-control segment outstanding for its third leg. `a` was never told
        // anything and stays on LongCp — which is what makes the convergence
        // assertion at the end meaningful.
        b.cp_negotiator.apply_as_confirmer(CpMode::ShortCp, t0);
        b.engine.set_cp_profile(CpMode::ShortCp);
        let switched_seq = b
            .cp_control_arq_tx
            .send(CpNegotiator::switched_payload(CpMode::ShortCp), t0)
            .expect("the CP-control window must have room");
        b.cp_negotiator.track_pending_switched(switched_seq);
        assert!(
            b.cp_negotiator.negotiation_in_flight(),
            "test setup: this must be an IN-FLIGHT negotiation, unlike the two \
             post-success tests above"
        );

        // Climb across the boundary through the real ACK path, exactly as
        // `a_rate_loop_vhf_excursion_does_not_desync_a_negotiated_short_cp` does —
        // ACKs encoded under `hf_standard_short_cp`, `b`'s real current profile.
        b.rate_loop = RateLoop::new(b.engine.available_speed_levels(), 5, 4);
        for _ in 0..5 {
            let ack = TransportPdu::new_ack_with_rate(b.arq_session_id, 0, 0, 9);
            unstick_decoder(&mut b);
            b.decode_and_dispatch_audio(&peer_frame(
                &ack,
                coppa_codec::ofdm::CoppaProfile::hf_standard_short_cp(),
            ))
            .await;
        }
        assert_eq!(
            b.engine.speed_level(),
            5,
            "test setup: really crossed to VHF"
        );

        // THE assertion this test exists for: mid-handshake, at VHF, the engine's
        // retained mode and the negotiator's must still agree. Pre-fix they do not.
        assert_engine_matches_negotiator(&b, "b (mid-handshake, dwelling at VHF)");

        // Back down through the real timeout path.
        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        arq_tx
            .send(
                b"expired segment".to_vec(),
                Instant::now() - Duration::from_millis(100),
            )
            .expect("a fresh ARQ window should have room");
        b.arq_tx = Some(arq_tx);
        b.check_arq_retransmits().await;
        assert_eq!(b.engine.speed_level(), 4, "test setup: stepped back 5 -> 4");
        assert_engine_matches_negotiator(&b, "b (mid-handshake, back in HF)");

        // G3: probation expires with no proof `a` ever switched. The give-up must
        // walk BOTH the negotiator and the engine back to the pre-negotiation mode,
        // which is the mode `a` -- never told anything -- is still listening on.
        b.drive_cp_negotiation(t0 + Duration::from_secs(SWITCH_PROBATION_SECS + 1));

        assert_engine_matches_negotiator(&b, "b (after the give-up)");
        assert_converged(&a, &b, CpMode::LongCp);
        assert_eq!(
            b.cp_negotiator.pending_switched_seq(),
            None,
            "the give-up must release the tracked leg, not just the mode"
        );

        // Alive, not merely consistent.
        let samples = b.engine.encode_bytes(b"link alive").unwrap();
        assert_eq!(
            a.engine
                .decode_bytes(&samples)
                .expect("a must decode b's frame after a mid-handshake VHF excursion"),
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
            RateLoop::new(event_loop.engine.available_speed_levels(), 5, 1).with_probing(1, 1);

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
            RateLoop::new(event_loop.engine.available_speed_levels(), 5, 4).with_probing(1, 1);

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
            RateLoop::new(event_loop.engine.available_speed_levels(), 5, 1).with_probing(1, 1);

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
        // (The same channel also carries COP-65 `audio_level` messages: these
        // two pushes complete one 100 ms level window, so only `spectrum`
        // messages count here.)
        event_loop.handle_audio_in(&samples).await;
        let mut rebroadcast = false;
        while let Ok(json) = ws_rx.try_recv() {
            if let Ok(coppa_host::websocket::WsServerMessage::Spectrum { .. }) =
                serde_json::from_str(&json)
            {
                rebroadcast = true;
            }
        }
        assert!(
            !rebroadcast,
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

    // ── COP-65: RX/TX audio level metering ────────────────────────────────

    /// One `audio_level` message, as the broadcast channel carried it.
    #[cfg(feature = "websocket")]
    #[derive(Debug, Clone, Copy)]
    struct AudioLevelMsg {
        direction: coppa_host::websocket::AudioLevelDirection,
        peak_dbfs: f32,
        rms_dbfs: f32,
        clip_fraction: f32,
        state: coppa_host::websocket::AudioLevelState,
        duration_ms: u32,
    }

    /// Drain every pending broadcast and keep only the `audio_level` messages,
    /// in order, optionally only one direction.
    #[cfg(feature = "websocket")]
    fn drain_audio_levels(
        ws_rx: &mut tokio::sync::broadcast::Receiver<String>,
        direction: Option<coppa_host::websocket::AudioLevelDirection>,
    ) -> Vec<AudioLevelMsg> {
        let mut out = Vec::new();
        while let Ok(json) = ws_rx.try_recv() {
            if let Ok(coppa_host::websocket::WsServerMessage::AudioLevel {
                direction: d,
                peak_dbfs,
                rms_dbfs,
                clip_fraction,
                state,
                duration_ms,
                ..
            }) = serde_json::from_str(&json)
            {
                if direction.is_none_or(|want| want == d) {
                    out.push(AudioLevelMsg {
                        direction: d,
                        peak_dbfs,
                        rms_dbfs,
                        clip_fraction,
                        state,
                        duration_ms,
                    });
                }
            }
        }
        out
    }

    /// Alternating full-scale samples: every sample is clipped.
    fn full_scale_square(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn rx_audio_level_is_broadcast_ten_times_per_second_of_audio() {
        use coppa_host::websocket::{AudioLevelDirection, AudioLevelState};

        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        event_loop.handle_audio_in(&vec![0.01f32; 48_000]).await;

        let all = drain_audio_levels(&mut ws_rx, None);
        assert_eq!(all.len(), 10, "one rx message per 100 ms window");
        for m in &all {
            assert_eq!(m.direction, AudioLevelDirection::Rx);
            assert_eq!(m.peak_dbfs, -40.0);
            assert_eq!(m.rms_dbfs, -40.0);
            assert_eq!(m.clip_fraction, 0.0);
            assert_eq!(m.state, AudioLevelState::Ok);
            assert_eq!(m.duration_ms, 100);
        }
    }

    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn clipped_rx_audio_is_reported_as_clipping() {
        use coppa_host::websocket::{AudioLevelDirection, AudioLevelState};

        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        event_loop.handle_audio_in(&full_scale_square(14_400)).await;

        let rx = drain_audio_levels(&mut ws_rx, Some(AudioLevelDirection::Rx));
        assert_eq!(rx.len(), 3);
        assert_eq!(rx[0].clip_fraction, 1.0);
        assert_eq!(
            rx[0].state,
            AudioLevelState::Ok,
            "one clipped window is not yet clipping"
        );
        assert_eq!(rx[1].state, AudioLevelState::Clipping);
        assert_eq!(rx[2].state, AudioLevelState::Clipping);
    }

    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn rx_audio_is_not_metered_while_transmitting() {
        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        // Half a window of clipped audio before keying.
        event_loop.handle_audio_in(&vec![1.0f32; 2400]).await;
        event_loop.is_transmitting = true;
        event_loop.handle_audio_in(&vec![1.0f32; 48_000]).await;
        assert!(
            drain_audio_levels(&mut ws_rx, None).is_empty(),
            "no audio_level while transmitting"
        );
        event_loop.is_transmitting = false;

        event_loop.handle_audio_in(&vec![0.01f32; 4800]).await;
        let rx = drain_audio_levels(&mut ws_rx, None);
        assert_eq!(rx.len(), 1);
        assert_eq!(
            rx[0].clip_fraction, 0.0,
            "the pre-TX partial window was discarded, not merged"
        );
    }

    #[cfg(feature = "websocket")]
    #[tokio::test(start_paused = true)]
    async fn busy_wait_audio_is_metered_exactly_once() {
        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);
        let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_in(audio_in_rx);

        audio_in_tx.write(&vec![0.01f32; 9600]);
        event_loop.observe_busy_gate_from_audio_input().await;
        assert_eq!(drain_audio_levels(&mut ws_rx, None).len(), 2);
        assert!(!event_loop.pending_busy_wait_audio.is_empty());

        // The deferred decode of that same audio must not meter it again.
        event_loop.poll_audio_input().await;
        assert!(event_loop.pending_busy_wait_audio.is_empty());
        assert!(drain_audio_levels(&mut ws_rx, None).is_empty());
    }

    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn each_transmission_reports_one_tx_audio_level() {
        use coppa_host::websocket::{AudioLevelDirection, AudioLevelState};

        let mut config = tx_config();
        config.radio.ptt_pre_delay_ms = 0;
        let mut event_loop = EventLoop::new(config).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        let samples = event_loop.engine.encode_bytes(b"COP-65").unwrap();
        event_loop.transmit_samples(&samples).await;

        let all = drain_audio_levels(&mut ws_rx, None);
        assert_eq!(all.len(), 1, "exactly one message per transmission");
        let tx = all[0];
        assert_eq!(tx.direction, AudioLevelDirection::Tx);
        assert_eq!(
            tx.peak_dbfs, -6.0,
            "every frame is normalized to -6.02 dBFS"
        );
        assert_eq!(tx.state, AudioLevelState::Ok);
        assert_eq!(tx.clip_fraction, 0.0);
        assert!(
            (-22.0..=-16.0).contains(&tx.rms_dbfs),
            "rms {}",
            tx.rms_dbfs
        );
        assert_eq!(
            tx.duration_ms as usize,
            samples.len() * 1000 / 48_000,
            "no station ID was prepended, so the span is the frame itself"
        );
    }

    #[cfg(feature = "websocket")]
    #[tokio::test]
    async fn tune_tone_tx_level_reads_minus_6_peak_minus_12_rms() {
        use coppa_host::websocket::AudioLevelDirection;

        let mut config = tx_config();
        config.radio.ptt_pre_delay_ms = 0;
        let mut event_loop = EventLoop::new(config).unwrap();
        let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
        event_loop.set_ws_broadcast(ws_tx);

        let tone = event_loop.engine.tune_tone(1.0, None);
        event_loop.transmit_samples(&tone).await;

        let tx = drain_audio_levels(&mut ws_rx, Some(AudioLevelDirection::Tx));
        assert_eq!(tx.len(), 1);
        assert_eq!(tx[0].peak_dbfs, -6.0);
        assert_eq!(tx[0].rms_dbfs, -12.0);
    }

    #[tokio::test]
    async fn rx_level_state_tracks_clipping_without_a_websocket_host() {
        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        event_loop.handle_audio_in(&full_scale_square(9600)).await;
        assert_eq!(
            event_loop.rx_level_meter.state(),
            crate::level_meter::LevelState::Clipping
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

    /// Shared fixture for COP-22's TX delivery tests: an event loop wired to
    /// a production-size (8,192) audio-out ring, the production output
    /// worker, and a simulated 8,192-sample device ring.
    struct TxRig {
        event_loop: EventLoop,
        outer: AudioRingConsumer,
        worker: crate::audio_output::OutputWorker,
        sink: crate::audio_output::test_support::SimSink,
        feedback: Arc<OutputFeedback>,
    }

    impl TxRig {
        fn new(mut config: DaemonConfig) -> Self {
            config.radio.ptt_pre_delay_ms = 0;
            config.radio.ptt_tail_delay_ms = 0;
            // COP-37: a TX rig transmits, so it needs a callsign.
            config.engine.callsign = TEST_CALLSIGN.to_string();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (producer, outer) = coppa_audio::audio_ring(8192);
            event_loop.set_audio_out(producer);
            let feedback = Arc::new(OutputFeedback::default());
            event_loop.set_output_feedback(feedback.clone());
            Self {
                event_loop,
                outer,
                worker: crate::audio_output::OutputWorker::new(1024, 48_000, feedback.clone()),
                sink: crate::audio_output::test_support::SimSink::new(8192),
                feedback,
            }
        }

        /// One delivery round: the `run` TX tick, worker forwarding, and a
        /// simulated device callback of `callback` samples.
        fn pump(&mut self, callback: usize) {
            self.event_loop.flush_pending_audio_out();
            for _ in 0..4 {
                self.worker
                    .step(
                        &mut self.outer,
                        &mut self.sink,
                        crate::audio_output::test_support::sim_progress,
                    )
                    .unwrap();
            }
            self.sink.callback(callback);
        }

        fn deliver(&mut self, total: usize, callbacks: &[usize]) {
            let mut i = 0;
            while self.sink.captured.len() < total {
                self.pump(callbacks[i % callbacks.len()]);
                i += 1;
                assert!(i < 1_000_000, "TX delivery did not converge");
            }
            // The worker publishes progress on its next step, as the
            // production worker loop does continuously.
            self.pump(0);
        }

        async fn next_completion(&mut self) -> (u64, bool) {
            let event =
                tokio::time::timeout(Duration::from_secs(10), self.event_loop.event_rx.recv())
                    .await
                    .expect("TX completion should arrive")
                    .expect("event channel open");
            match event {
                DaemonEvent::TxComplete {
                    generation,
                    completed,
                } => (generation, completed),
                other => panic!("expected TxComplete, got {other:?}"),
            }
        }
    }

    /// COP-22 AC1: a real 65,520-sample encoded frame, transmitted through
    /// an 8,192-sample audio-out ring and an 8,192-sample device ring with
    /// delayed, uneven consumption, reaches the final sink whole and in
    /// order; PTT is released only by that transmission's completion.
    #[tokio::test]
    async fn full_frame_reaches_sink_through_both_rings() {
        let mut rig = TxRig::new(DaemonConfig::default());
        let frame = rig.event_loop.engine.encode_bytes(b"COP-22").unwrap();
        assert_eq!(frame.len(), 65_520);
        rig.event_loop.transmit_samples(&frame).await;
        assert!(rig.event_loop.is_transmitting);
        assert!(
            rig.event_loop.tx_output().pending_len() > 0,
            "the unwritten suffix is retained, not dropped"
        );
        rig.deliver(frame.len(), &[480, 0, 4096, 64, 0, 0, 1000]);
        assert_eq!(rig.sink.captured, frame);
        assert_eq!(rig.event_loop.tx_output().overflow_count(), 0);

        let (generation, completed) = rig.next_completion().await;
        assert_eq!((generation, completed), (1, true));
        rig.event_loop
            .handle_tx_complete(generation, completed)
            .await;
        assert!(!rig.event_loop.is_transmitting);
    }

    /// PTT stays keyed while presentation is pending after both rings have
    /// drained, and is released only once playback is predicted complete.
    #[tokio::test]
    async fn ptt_held_until_predicted_presentation_ends() {
        let mut rig = TxRig::new(DaemonConfig::default());
        let samples: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        rig.sink.remaining = Duration::from_secs(3600);
        rig.event_loop.transmit_samples(&samples).await;
        rig.deliver(samples.len(), &[2048]);
        assert_eq!(rig.outer.available(), 0, "both rings drained");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rig.event_loop.event_rx.recv())
                .await
                .is_err(),
            "no completion while the last sample is still ahead of the DAC"
        );
        rig.sink.remaining = Duration::ZERO;
        rig.pump(0);
        assert_eq!(rig.next_completion().await, (1, true));
    }

    /// Consecutive transmissions (e.g. a control frame queued behind an
    /// active one) play in FIFO order, and the earlier job's completion is
    /// stale: it cannot unkey the newer transmission.
    #[tokio::test]
    async fn stale_completion_cannot_unkey_newer_transmission() {
        let mut rig = TxRig::new(DaemonConfig::default());
        let a: Vec<f32> = (0..30_000).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..12_345).map(|i| -(i as f32) - 1.0).collect();
        rig.event_loop.transmit_samples(&a).await;
        rig.event_loop.transmit_samples(&b).await;
        rig.deliver(a.len() + b.len(), &[1000, 333]);
        let mut expected = a.clone();
        expected.extend_from_slice(&b);
        assert_eq!(rig.sink.captured, expected);

        let mut completions = vec![rig.next_completion().await, rig.next_completion().await];
        completions.sort();
        assert_eq!(completions, vec![(1, true), (2, true)]);
        rig.event_loop.handle_tx_complete(1, true).await;
        assert!(
            rig.event_loop.is_transmitting,
            "stale generation must not release PTT"
        );
        rig.event_loop.handle_tx_complete(2, true).await;
        assert!(!rig.event_loop.is_transmitting);
    }

    /// An output failure cancels the transmission: PTT is released, the
    /// completion is not a success, and the retained suffix is discarded so
    /// it cannot play after the radio is unkeyed.
    #[tokio::test]
    async fn output_failure_cancels_and_discards_pending() {
        let mut rig = TxRig::new(DaemonConfig::default());
        let samples = vec![0.25f32; 50_000];
        rig.event_loop.transmit_samples(&samples).await;
        rig.feedback.fail();
        let (generation, completed) = rig.next_completion().await;
        assert_eq!((generation, completed), (1, false));
        rig.event_loop
            .handle_tx_complete(generation, completed)
            .await;
        assert!(!rig.event_loop.is_transmitting);
        assert_eq!(rig.event_loop.tx_output().pending_len(), 0);
        rig.event_loop.flush_pending_audio_out();
        rig.pump(8192);
        rig.pump(8192);
        assert!(
            rig.sink.captured.is_empty(),
            "nothing had reached the device yet, so nothing may play after the cancel"
        );
    }

    /// A tune longer than `max_tx_duration_s` (and larger than both rings)
    /// is aborted by the watchdog, not reported as a completed TX.
    #[tokio::test(start_paused = true)]
    async fn watchdog_aborts_overlong_tune() {
        let mut config = DaemonConfig::default();
        config.radio.max_tx_duration_s = 1;
        let mut rig = TxRig::new(config);
        let tune = vec![0.5f32; 48_000 * 3];
        rig.event_loop.transmit_samples(&tune).await;
        // Keep the device consuming slowly so only the watchdog can end it.
        rig.pump(480);
        let (generation, completed) = rig.next_completion().await;
        assert_eq!((generation, completed), (1, false));
        rig.event_loop
            .handle_tx_complete(generation, completed)
            .await;
        assert!(!rig.event_loop.is_transmitting);
        assert_eq!(rig.event_loop.tx_output().pending_len(), 0);
    }

    /// A watchdog-cancelled transmission whose tail is already buffered in
    /// the final sink's ring must not play that tail at the start of the
    /// next transmission: the sink is flushed before new audio reaches it.
    #[tokio::test(start_paused = true)]
    async fn cancelled_tail_in_sink_does_not_open_next_frame() {
        let mut config = DaemonConfig::default();
        config.radio.max_tx_duration_s = 1;
        let mut rig = TxRig::new(config);
        let stale = vec![0.5f32; 48_000 * 3];
        rig.event_loop.transmit_samples(&stale).await;
        // Fill the device ring with the doomed transmission, but let the
        // device consume none of it before the watchdog fires.
        for _ in 0..16 {
            rig.pump(0);
        }
        assert!(rig.sink.captured.is_empty());
        let (generation, completed) = rig.next_completion().await;
        assert_eq!((generation, completed), (1, false));
        rig.event_loop
            .handle_tx_complete(generation, completed)
            .await;
        assert!(!rig.event_loop.is_transmitting);

        let next: Vec<f32> = (0..20_000).map(|i| -(i as f32) - 1.0).collect();
        rig.event_loop.transmit_samples(&next).await;
        rig.deliver(next.len(), &[480, 1000]);
        for _ in 0..4 {
            rig.pump(8192);
        }
        assert_eq!(
            rig.sink.captured, next,
            "the cancelled transmission's buffered tail played ahead of the next frame"
        );
        assert_eq!(rig.next_completion().await, (2, true));
    }

    /// A transmission queued behind an active one cannot restart the
    /// watchdog: `max_tx_duration_s` bounds the continuous keyed interval
    /// from the first key-up, so the newest job is cancelled (and PTT
    /// released) when that interval runs out, not `max_tx` after it queued.
    #[tokio::test(start_paused = true)]
    async fn queued_transmission_cannot_extend_the_watchdog() {
        let mut config = DaemonConfig::default();
        config.radio.max_tx_duration_s = 2;
        let mut rig = TxRig::new(config);
        let keyed = tokio::time::Instant::now();
        rig.event_loop
            .transmit_samples(&[0.5f32; 48_000 * 10])
            .await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        rig.event_loop.transmit_samples(&[0.5f32; 48_000]).await;
        loop {
            let (generation, completed) = rig.next_completion().await;
            if generation == 2 {
                assert!(!completed, "the watchdog aborts, it does not complete");
                break;
            }
        }
        assert!(
            keyed.elapsed() < Duration::from_millis(2500),
            "forced unkey after {:?}, past the 2 s keyed-interval bound",
            keyed.elapsed()
        );
        rig.event_loop.handle_tx_complete(2, false).await;
        assert!(!rig.event_loop.is_transmitting);
        assert_eq!(rig.event_loop.tx_keyed_since, None);
    }

    /// No downstream progress for five seconds fails the transmission.
    #[tokio::test(start_paused = true)]
    async fn stalled_output_fails_transmission() {
        let mut rig = TxRig::new(DaemonConfig::default());
        rig.event_loop.transmit_samples(&[0.1f32; 20_000]).await;
        let started = tokio::time::Instant::now();
        assert_eq!(rig.next_completion().await, (1, false));
        assert!(started.elapsed() >= crate::audio_output::STALL_TIMEOUT);
    }

    /// A transmission queued behind a stalled one inherits its no-progress
    /// window: the newest job is failed five seconds after playback last
    /// advanced, not five seconds after it was queued, so frequent ARQ/control
    /// transmissions cannot hold a stalled output keyed until the max-TX
    /// deadline.
    #[tokio::test(start_paused = true)]
    async fn stall_window_spans_queued_transmissions() {
        let mut rig = TxRig::new(DaemonConfig::default());
        let keyed = tokio::time::Instant::now();
        rig.event_loop.transmit_samples(&[0.1f32; 20_000]).await;
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            rig.event_loop.transmit_samples(&[0.1f32; 4_800]).await;
        }
        loop {
            let (generation, completed) = rig.next_completion().await;
            if generation == 4 {
                assert!(!completed, "a stalled output fails, it does not complete");
                break;
            }
        }
        let stall = crate::audio_output::STALL_TIMEOUT;
        assert!(
            keyed.elapsed() >= stall && keyed.elapsed() < stall + Duration::from_millis(100),
            "stall failure after {:?}, not ~{stall:?} after playback last advanced",
            keyed.elapsed()
        );
    }

    /// A job queued while nothing is outstanding (here: during the previous
    /// job's PTT tail) starts a fresh no-progress window rather than being
    /// failed for the idle time before it.
    #[tokio::test(start_paused = true)]
    async fn idle_gap_does_not_count_as_a_stall() {
        let mut rig = TxRig::new(DaemonConfig::default());
        rig.event_loop.config.radio.ptt_tail_delay_ms = 8_000;
        let first = vec![0.2f32; 4_800];
        rig.event_loop.transmit_samples(&first).await;
        rig.deliver(first.len(), &[4096]);
        // Job 1 has played; it is now in its 8 s tail with nothing pending.
        tokio::time::sleep(Duration::from_millis(6_000)).await;
        let second = vec![0.3f32; 4_800];
        rig.event_loop.transmit_samples(&second).await;
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        rig.deliver(first.len() + second.len(), &[4096]);
        loop {
            let (generation, completed) = rig.next_completion().await;
            if generation == 2 {
                assert!(completed, "idle time before a job is not a stall");
                break;
            }
        }
    }

    /// The PTT tail is capped at the keyed interval's `max_tx_duration_s`
    /// deadline: a transmission that finishes playing near the limit unkeys
    /// at the limit, not up to `ptt_tail_delay_ms` past it.
    #[tokio::test(start_paused = true)]
    async fn ptt_tail_is_capped_at_the_max_tx_deadline() {
        let mut config = DaemonConfig::default();
        config.radio.max_tx_duration_s = 2;
        let mut rig = TxRig::new(config);
        rig.event_loop.config.radio.ptt_tail_delay_ms = 5_000;
        let keyed = tokio::time::Instant::now();
        let samples = vec![0.4f32; 9_600];
        rig.event_loop.transmit_samples(&samples).await;
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        rig.deliver(samples.len(), &[4096]);
        assert_eq!(rig.next_completion().await, (1, true));
        let deadline = Duration::from_secs(2);
        assert!(
            keyed.elapsed() >= deadline && keyed.elapsed() < deadline + Duration::from_millis(50),
            "PTT released after {:?}; the tail must end at the {deadline:?} max-TX deadline",
            keyed.elapsed()
        );
    }

    /// A due station-ID frame is prepended to the payload: both play whole,
    /// ID first, under one keyed transmission larger than both rings.
    #[tokio::test]
    async fn station_id_prefix_and_payload_both_play_whole() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "N0CALL".to_string();
        config.station_id.id_interval_secs = 600;
        let mut rig = TxRig::new(config);
        rig.event_loop.last_id_time = Instant::now() - Duration::from_secs(601);
        let id_frame = rig
            .event_loop
            .encode_id_beacon_frame()
            .expect("a configured callsign builds an ID frame");
        let payload = rig.event_loop.engine.encode_bytes(b"payload").unwrap();
        rig.event_loop.transmit_samples(&payload).await;
        let mut expected = id_frame;
        expected.extend_from_slice(&payload);
        assert!(expected.len() > 2 * 8192);
        rig.deliver(expected.len(), &[1024, 0, 3000]);
        assert_eq!(rig.sink.captured, expected);
        assert_eq!(rig.next_completion().await, (1, true));
    }

    /// PTT lead elapses before any sample is queued, and the PTT tail is
    /// measured from the last sample's playback, not from the enqueue.
    #[tokio::test(start_paused = true)]
    async fn lead_precedes_audio_and_tail_follows_final_playback() {
        let mut rig = TxRig::new(DaemonConfig::default());
        rig.event_loop.config.radio.ptt_pre_delay_ms = 50;
        rig.event_loop.config.radio.ptt_tail_delay_ms = 200;
        let samples = vec![0.3f32; 30_000];
        let started = tokio::time::Instant::now();
        rig.event_loop.transmit_samples(&samples).await;
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "lead before audio"
        );
        // Hold playback for a while: no completion may arrive meanwhile.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(rig.event_loop.event_rx.try_recv().is_err());
        rig.deliver(samples.len(), &[4096]);
        let played_at = tokio::time::Instant::now();
        assert_eq!(rig.next_completion().await, (1, true));
        assert!(
            played_at.elapsed() >= Duration::from_millis(200),
            "tail after playback"
        );
    }

    /// If the output worker's end of the ring disappears, the transmission
    /// still ends (as a failure) within the stall bound instead of holding
    /// PTT forever.
    #[tokio::test(start_paused = true)]
    async fn dropped_output_consumer_ends_transmission_bounded() {
        let TxRig {
            mut event_loop,
            outer,
            ..
        } = TxRig::new(DaemonConfig::default());
        drop(outer);
        event_loop.transmit_samples(&[0.2f32; 20_000]).await;
        let event = tokio::time::timeout(Duration::from_secs(10), event_loop.event_rx.recv())
            .await
            .expect("completion within the stall bound")
            .unwrap();
        assert!(matches!(
            event,
            DaemonEvent::TxComplete {
                generation: 1,
                completed: false
            }
        ));
    }

    /// `run`'s feeder task keeps moving retained TX samples into a small
    /// audio-out ring as a consumer drains it, until the whole transmission
    /// has been delivered in order.
    #[tokio::test]
    async fn run_feeder_delivers_backpressured_audio_whole() {
        let mut event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        let tx = event_loop.event_sender();
        let (producer, mut consumer) = coppa_audio::audio_ring(1024);
        event_loop.set_audio_out(producer);
        let samples: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        let expected = samples.clone();
        let driver = async move {
            tx.send(DaemonEvent::AudioOut(samples)).await.unwrap();
            let mut captured = Vec::new();
            let mut buf = vec![0.0f32; 700];
            while captured.len() < expected.len() {
                let n = consumer.read(&mut buf);
                captured.extend_from_slice(&buf[..n]);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            tx.send(DaemonEvent::Shutdown).await.unwrap();
            assert_eq!(captured, expected);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(event_loop.run(), driver)
        })
        .await
        .expect("feeder must deliver the whole transmission");
        result.unwrap();
    }

    /// Shutdown is still handled while TX output is backpressured.
    #[tokio::test]
    async fn shutdown_is_serviced_while_tx_backpressured() {
        let config = DaemonConfig::default();
        let mut event_loop = EventLoop::new(config).unwrap();
        let tx = event_loop.event_sender();
        let (producer, _consumer) = coppa_audio::audio_ring(1024);
        event_loop.set_audio_out(producer);
        tx.send(DaemonEvent::AudioOut(vec![0.5; 10_000]))
            .await
            .unwrap();
        tx.send(DaemonEvent::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), event_loop.run())
            .await
            .expect("run() must exit on shutdown while output is full")
            .unwrap();
        assert_eq!(event_loop.tx_output().pending_len(), 10_000 - 1024);
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
    async fn arq_host_data_without_a_session_creates_no_segment() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        let mut event_loop = EventLoop::new(config).unwrap();
        let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
        event_loop.set_audio_out(producer);

        event_loop
            .handle_host_event(HostEvent::DataReceived {
                client_id: 42,
                data: b"hello".to_vec(),
            })
            .await;

        assert_eq!(event_loop.arq_tx.as_ref().unwrap().in_flight(), 0);
        assert!(event_loop.tx_queue.is_empty());
        assert_eq!(consumer.read(&mut [0.0; 1024]), 0);
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

    // ── COP-28: OK/WRONG/VERSION replies, runtime MYCALL, IAMALIVE ──────────

    /// The reply rule for every command kind the parser can produce, as one
    /// table driven through the pure `vara_reply` function. See `vara_reply`'s
    /// rustdoc for the reasoning behind each row.
    #[test]
    fn vara_reply_answers_every_command_kind() {
        let ok = vec![
            "MYCALL W5AU",
            "mycall w5au-10",
            "LISTEN ON",
            "LISTEN OFF",
            "TUNE",
            "TUNE 1",
            "CONNECT W5AU K7XYZ",
            "DISCONNECT",
            "COMPRESSION TEXT",
            "BW2300",
            "BW500",
            "ABORT",
        ];
        let wrong = vec![
            "MYCALL VE3ABCD-15", // 10 chars
            "MYCALL W5AU!",
            "MYCALL",
            "LISTEN",
            "LISTEN MAYBE",
            "CONNECT W5AU",
            "PUBLIC ON",
            "CWID ON",
            "WINLINK SESSION",
            "P2P SESSION",
            "CLEANTXBUFFER",
            "FOOBAR",
            r#"{"type":"mycall","callsign":"W5AU"}"#,
        ];
        for line in ok {
            assert_eq!(
                vara_reply(&VaraCommand::parse(line)),
                VaraResponse::Ok,
                "{line:?} should be answered OK"
            );
        }
        for line in wrong {
            assert_eq!(
                vara_reply(&VaraCommand::parse(line)),
                VaraResponse::Wrong,
                "{line:?} should be answered WRONG"
            );
        }
        assert_eq!(
            vara_reply(&VaraCommand::parse("VERSION")),
            VaraResponse::coppa_version(),
            "VERSION is answered with the version line, never OK"
        );
    }

    /// COP-28: a valid `MYCALL` replaces the configured callsign (upper-cased
    /// by `Callsign::new`); invalid ones leave it untouched.
    #[tokio::test]
    async fn mycall_replaces_the_configured_callsign() {
        let mut config = DaemonConfig::default();
        config.engine.callsign = "VK3ABC".into();
        let mut station = EventLoop::new(config).unwrap();

        station
            .handle_host_event(HostEvent::VaraCommand {
                client_id: 1,
                command: "MYCALL w5au".into(),
            })
            .await;
        assert_eq!(station.local_callsign, Some(Callsign::new("W5AU").unwrap()));

        for rejected in ["MYCALL W5AU!", "MYCALL VE3ABCD-15"] {
            station
                .handle_host_event(HostEvent::VaraCommand {
                    client_id: 1,
                    command: rejected.into(),
                })
                .await;
        }
        assert_eq!(
            station.local_callsign,
            Some(Callsign::new("W5AU").unwrap()),
            "an invalid MYCALL must keep the callsign in force"
        );
    }

    /// COP-28: a station with no configured callsign gets one from `MYCALL`,
    /// which arms the station-ID/beacon path that `local_callsign: None`
    /// silently disables.
    #[tokio::test]
    async fn mycall_gives_an_unconfigured_station_a_callsign() {
        let mut station = EventLoop::new(DaemonConfig::default()).unwrap();
        assert_eq!(station.local_callsign, None);
        assert!(station.build_beacon_mac_pdu().is_none());

        station
            .handle_host_event(HostEvent::VaraCommand {
                client_id: 1,
                command: "MYCALL W5AU".into(),
            })
            .await;
        assert_eq!(station.local_callsign, Some(Callsign::new("W5AU").unwrap()));

        let pdu = station
            .build_beacon_mac_pdu()
            .expect("MYCALL should arm the station-ID/beacon path");
        assert_eq!(pdu.src.as_str(), "W5AU");
    }

    // ── COP-37: no valid callsign, no transmission ───────────────────────

    /// This thread's `tracing` output, captured so a test can assert what a
    /// refusal logged. `install` scopes the capture to the returned guard;
    /// `#[tokio::test]`'s current-thread runtime runs the event loop, and
    /// every task it spawns, on the test's own thread.
    #[derive(Clone, Default)]
    struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogCapture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log capture lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl LogCapture {
        fn install(&self) -> tracing::subscriber::DefaultGuard {
            let writer = self.clone();
            tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_writer(move || writer.clone())
                    .with_ansi(false)
                    .finish(),
            )
        }

        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("log capture lock")).into_owned()
        }

        /// How many captured `ERROR` lines carry the callsign refusal.
        fn refusals(&self) -> usize {
            self.contents()
                .lines()
                .filter(|line| line.contains("ERROR") && line.contains(NO_CALLSIGN_TX_REFUSAL))
                .count()
        }
    }

    /// A station wired to an audio-out ring and a VARA command-port client
    /// (`client_id` 1) whose replies and telemetry the test reads back.
    fn wired_station(
        config: DaemonConfig,
    ) -> (
        EventLoop,
        coppa_audio::AudioRingConsumer,
        mpsc::Receiver<VaraResponse>,
    ) {
        let mut station = EventLoop::new(config).unwrap();
        let (audio_tx, audio_rx) = coppa_audio::audio_ring(1_000_000);
        station.set_audio_out(audio_tx);
        let (vara_tx, vara_rx) = mpsc::channel(64);
        station.set_vara_responses(Arc::new(Mutex::new(HashMap::from([(1, vara_tx)]))));
        (station, audio_rx, vara_rx)
    }

    async fn send_vara_command(station: &mut EventLoop, command: &str) {
        station
            .handle_host_event(HostEvent::VaraCommand {
                client_id: 1,
                command: command.into(),
            })
            .await;
    }

    fn drain_vara(rx: &mut mpsc::Receiver<VaraResponse>) -> Vec<VaraResponse> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// The audio the station has written for transmission, once the `run`
    /// TX tick would have moved it into the audio-out ring.
    fn transmitted_samples(
        station: &EventLoop,
        audio_rx: &mut coppa_audio::AudioRingConsumer,
    ) -> usize {
        station.flush_pending_audio_out();
        audio_rx.available()
    }

    /// The ticket's scenario: with no callsign or an invalid one configured,
    /// `TUNE` keys nothing and writes no audio, and the daemon logs why. The
    /// daemon still starts (receive-only) and still answers `OK`, which only
    /// says the command parsed.
    #[tokio::test]
    async fn tune_is_refused_and_logged_without_a_valid_callsign() {
        for callsign in ["", "W5AU!"] {
            let logs = LogCapture::default();
            let _guard = logs.install();
            let mut config = DaemonConfig::default();
            config.engine.callsign = callsign.to_string();
            let (mut station, mut audio_rx, mut vara_rx) = wired_station(config);
            assert_eq!(station.local_callsign, None, "{callsign:?}");
            if !callsign.is_empty() {
                assert!(
                    logs.contents()
                        .contains("transmission disabled until a valid MYCALL is set"),
                    "an invalid callsign must warn at startup that TX is disabled, got: {}",
                    logs.contents()
                );
            }

            send_vara_command(&mut station, "TUNE 1").await;

            assert_eq!(
                drain_vara(&mut vara_rx),
                vec![VaraResponse::Ok],
                "{callsign:?}: TUNE must be answered OK with no PTT bracket"
            );
            assert!(!station.is_transmitting, "{callsign:?}");
            assert_eq!(
                transmitted_samples(&station, &mut audio_rx),
                0,
                "{callsign:?}: no tone may reach the audio sink"
            );
            assert_eq!(
                logs.refusals(),
                1,
                "{callsign:?}: the refusal must be logged once, got: {}",
                logs.contents()
            );
        }
    }

    /// Sessionless host data with no callsign is refused before ARQ
    /// registration or queueing, so it leaves no outstanding ARQ segment, no
    /// queued payload and no `BUFFER` count behind -- with ARQ off and on.
    #[tokio::test]
    async fn sessionless_host_data_is_refused_without_a_callsign_and_leaves_nothing_buffered() {
        for arq_enabled in [false, true] {
            let logs = LogCapture::default();
            let _guard = logs.install();
            let mut config = DaemonConfig::default();
            config.host.sessionless_tx_enabled = true;
            config.engine.arq_enabled = arq_enabled;
            let (mut station, mut audio_rx, mut vara_rx) = wired_station(config);

            station
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x42; 40],
                })
                .await;

            assert_eq!(
                drain_vara(&mut vara_rx),
                Vec::<VaraResponse>::new(),
                "arq={arq_enabled}: no BUFFER or PTT telemetry for a refused payload"
            );
            assert!(station.tx_queue.is_empty(), "arq={arq_enabled}");
            assert_eq!(station.buffered_tx_bytes(), 0, "arq={arq_enabled}");
            if let Some(arq_tx) = station.arq_tx.as_ref() {
                assert_eq!(arq_tx.in_flight(), 0, "no ARQ segment may be registered");
                assert_eq!(arq_tx.unacked_bytes(), 0);
            }
            assert!(!station.is_transmitting, "arq={arq_enabled}");
            assert_eq!(transmitted_samples(&station, &mut audio_rx), 0);
            assert_eq!(
                logs.refusals(),
                1,
                "arq={arq_enabled}: the refusal must be logged once, got: {}",
                logs.contents()
            );
        }
    }

    /// The same `TUNE` transmits once a valid callsign is in force, whether
    /// configured or given later by `MYCALL`.
    #[tokio::test]
    async fn tune_transmits_once_a_valid_callsign_is_in_force() {
        let logs = LogCapture::default();
        let _guard = logs.install();

        let (mut configured, mut audio_rx, mut vara_rx) = wired_station(tx_config());
        send_vara_command(&mut configured, "TUNE 1").await;
        assert_eq!(
            drain_vara(&mut vara_rx),
            vec![VaraResponse::Ok, VaraResponse::Ptt(true)],
            "a configured callsign must let TUNE key PTT"
        );
        assert!(transmitted_samples(&configured, &mut audio_rx) > 0);

        let (mut unconfigured, mut audio_rx, mut vara_rx) = wired_station(DaemonConfig::default());
        send_vara_command(&mut unconfigured, "TUNE 1").await;
        assert_eq!(drain_vara(&mut vara_rx), vec![VaraResponse::Ok]);
        assert_eq!(transmitted_samples(&unconfigured, &mut audio_rx), 0);
        assert_eq!(logs.refusals(), 1, "got: {}", logs.contents());

        send_vara_command(&mut unconfigured, "MYCALL W5AU").await;
        send_vara_command(&mut unconfigured, "TUNE 1").await;
        assert_eq!(
            drain_vara(&mut vara_rx),
            vec![VaraResponse::Ok, VaraResponse::Ok, VaraResponse::Ptt(true)],
            "a valid MYCALL must let the next TUNE key PTT"
        );
        assert!(transmitted_samples(&unconfigured, &mut audio_rx) > 0);
        assert_eq!(
            logs.refusals(),
            1,
            "only the TUNE before MYCALL may be refused, got: {}",
            logs.contents()
        );
    }

    /// COP-28: the keepalive sends `IAMALIVE` to every wired client exactly
    /// once per 60 s period, with the first one a full period after start
    /// (nothing at t=0).
    #[tokio::test(start_paused = true)]
    async fn keepalive_sends_iamalive_every_sixty_seconds() {
        let (tx1, mut rx1) = mpsc::channel(4);
        let (tx2, mut rx2) = mpsc::channel(4);
        let senders: VaraResponseSenders =
            Arc::new(Mutex::new(HashMap::from([(1, tx1), (2, tx2)])));
        let task = spawn_vara_keepalive(senders, VARA_KEEPALIVE_PERIOD);

        tokio::time::advance(Duration::from_secs(59)).await;
        for rx in [&mut rx1, &mut rx2] {
            let got = tokio::time::timeout(Duration::from_millis(1), rx.recv()).await;
            assert!(got.is_err(), "nothing may be sent before the first tick");
        }

        for _ in 0..2 {
            tokio::time::advance(Duration::from_secs(60)).await;
            for rx in [&mut rx1, &mut rx2] {
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(1), rx.recv())
                        .await
                        .expect("both clients must hear each IAMALIVE")
                        .expect("channel closed"),
                    VaraResponse::IAmAlive
                );
            }
        }

        task.abort();
    }

    #[test]
    fn event_loop_default_keepalive_period_is_sixty_seconds() {
        let event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
        assert_eq!(event_loop.vara_keepalive_period, Duration::from_secs(60));
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
        let mut config = tx_config();
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
    async fn cop21_a_peer_ack_that_raises_a_level_derived_engine_to_5_lands_on_hf() {
        let mut config = DaemonConfig::default();
        config.engine.arq_enabled = true;
        config.engine.profile = "HF-STANDARD-TYPO".into();
        config.radio.frequency_hz = Some(14_074_000);
        let mut event_loop = EventLoop::new(config).unwrap();
        event_loop.arq_tx = Some(ArqTx::new(ArqConfig::default()));
        event_loop.engine.set_speed_level(4).unwrap();
        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 4);
        for _ in 0..5 {
            let ack = TransportPdu::new_ack_with_rate(0, 0, 0, 9);
            event_loop
                .decode_and_dispatch_audio(&peer_frame(
                    &ack,
                    coppa_codec::ofdm::CoppaProfile::hf_standard(),
                ))
                .await;
        }
        assert_eq!(
            event_loop.rate_loop.current_level(),
            5,
            "premise: RateLoop crossed 4 -> 5"
        );
        assert_eq!(event_loop.engine.speed_level(), 5);
        assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
        assert_eq!(event_loop.engine.config().ofdm_band, OfdmBand::Hf);
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
            let ack_pdu = TransportPdu::new_ack_with_rate(0, 0, 0, 9);
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
        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 1);
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
        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 1);
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

        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 5);
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

        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 5);
        assert_eq!(event_loop.rate_loop.current_level(), 5);

        let arq_config = ArqConfig::new(8, 5, Duration::from_millis(20))
            .expect("window_size=8 is within 1..=MAX_WINDOW_SIZE");
        let mut arq_tx = ArqTx::new(arq_config);
        let send_time = Instant::now() - Duration::from_millis(100);
        let seq = arq_tx
            .send(b"probed segment".to_vec(), send_time)
            .expect("a fresh ARQ window should have room");
        event_loop.arq_tx = Some(arq_tx);
        event_loop.probe_state = Some((seq, 8)); // pretend we probed up to the HF ceiling

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

        event_loop.rate_loop = RateLoop::new(event_loop.engine.available_speed_levels(), 5, 5);
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
        event_loop.probe_state = Some((probe_seq, 8));

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
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
        use tokio::net::TcpStream;

        /// Config tuned so a real `transmit_samples` call's scheduled PTT-release
        /// delay resolves in ~0ms: a very high nominal sample rate makes
        /// `audio_duration_ms`/`drain_ms` truncate to 0 (integer ms math), and the
        /// pre/tail delays are zeroed. This only affects the *scheduling* math in
        /// `transmit_samples` — the engine's own internal encode rate is fixed at
        /// 48 kHz regardless (see `CLAUDE.md`), so encoding real payloads is
        /// unaffected. It carries `TEST_CALLSIGN` (COP-37) so the station may
        /// transmit at all.
        fn fast_ptt_config() -> DaemonConfig {
            let mut config = tx_config();
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

            let greeting = read_line(&mut reader).await;
            assert!(
                greeting.starts_with("VERSION"),
                "expected a VERSION greeting first, got: {}",
                greeting
            );

            (reader, write_half)
        }

        /// `connect_mock_vara_client`'s socket/server wiring plus the event
        /// bridge `main.rs` installs: the server's `HostEvent`s are forwarded
        /// into the loop's own channel, so commands written on the socket
        /// reach `handle_host_event` for real and get their reply. Tests that
        /// drive commands through the socket (rather than injecting
        /// `DaemonEvent::Host` by hand) need this helper AND a running
        /// `run()` (in a `LocalSet`), or nothing consumes the forwarded
        /// events.
        async fn connect_bridged_vara_client(
            event_loop: &mut EventLoop,
            cmd_port: u16,
            data_port: u16,
        ) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
            let cmd_listener = tokio::net::TcpListener::bind(("127.0.0.1", cmd_port))
                .await
                .expect("bind VARA command port");
            let data_listener = tokio::net::TcpListener::bind(("127.0.0.1", data_port))
                .await
                .expect("bind VARA data port");
            connect_bridged_vara_client_on(event_loop, cmd_listener, data_listener).await
        }

        /// `connect_bridged_vara_client` on OS-assigned ports: binds both
        /// listeners to `127.0.0.1:0` and returns the data port's actual
        /// address alongside the command-port client, so concurrent test runs
        /// (separate worktrees, parallel CI slots) can never cross-attach.
        async fn connect_bridged_vara_client_ephemeral(
            event_loop: &mut EventLoop,
        ) -> (
            BufReader<OwnedReadHalf>,
            OwnedWriteHalf,
            std::net::SocketAddr,
        ) {
            let cmd_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind VARA command port");
            let data_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind VARA data port");
            let data_addr = data_listener.local_addr().expect("data listener address");
            let (reader, writer) =
                connect_bridged_vara_client_on(event_loop, cmd_listener, data_listener).await;
            (reader, writer, data_addr)
        }

        async fn connect_bridged_vara_client_on(
            event_loop: &mut EventLoop,
            cmd_listener: tokio::net::TcpListener,
            data_listener: tokio::net::TcpListener,
        ) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
            let cmd_addr = cmd_listener.local_addr().expect("command listener address");
            let mut server = VaraServer::new(cmd_addr.port(), 0);
            event_loop.set_vara_responses(server.response_senders());
            let mut events = server.take_event_rx().expect("fresh server");
            let to_loop = event_loop.event_sender();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if to_loop.send(DaemonEvent::Host(event)).await.is_err() {
                        break;
                    }
                }
            });
            tokio::spawn(async move {
                server
                    .run_with_listeners(cmd_listener, data_listener)
                    .await
                    .expect("VARA server accept loops");
            });

            // Listeners are already bound, so the connect cannot race the bind.
            let stream = TcpStream::connect(cmd_addr)
                .await
                .expect("mock client should connect to the VARA command port");
            let (read_half, write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let greeting = read_line(&mut reader).await;
            assert!(
                greeting.starts_with("VERSION"),
                "expected a VERSION greeting first, got: {}",
                greeting
            );

            (reader, write_half)
        }

        /// Read one `\r`-terminated VARA response line, timing out (rather than
        /// hanging forever) if the client never receives one.
        async fn read_line(reader: &mut BufReader<OwnedReadHalf>) -> String {
            let mut line = Vec::new();
            let n =
                tokio::time::timeout(Duration::from_secs(2), reader.read_until(b'\r', &mut line))
                    .await
                    .expect("timed out waiting for a VARA response line")
                    .expect("reading a VARA response line should not error");
            assert!(n > 0, "connection closed before a response line arrived");
            String::from_utf8(line).expect("VARA response lines are UTF-8")
        }

        /// Drain every line currently available within a short per-line window,
        /// stopping (not erroring) once nothing more arrives — used where the
        /// exact response count is a secondary detail and the assertions care
        /// about relative order/content of a subset (e.g. only the `BUSY` lines).
        async fn read_available_lines(reader: &mut BufReader<OwnedReadHalf>) -> Vec<String> {
            let mut lines = Vec::new();
            loop {
                let mut line = Vec::new();
                match tokio::time::timeout(
                    Duration::from_millis(300),
                    reader.read_until(b'\r', &mut line),
                )
                .await
                {
                    Ok(Ok(n)) if n > 0 => {
                        lines.push(String::from_utf8(line).expect("VARA response lines are UTF-8"))
                    }
                    _ => break,
                }
            }
            lines
        }

        fn session_config() -> DaemonConfig {
            let mut config = fast_ptt_config();
            config.engine.callsign = "VK3ABC".into();
            config
        }

        fn peer() -> coppa_protocol::session::Session {
            coppa_protocol::session::Session::new(
                0,
                Callsign::new("W1AW").unwrap(),
                Callsign::new("VK3ABC").unwrap(),
                0,
                LinkCapabilities::default(),
            )
        }

        fn seed_established(event_loop: &mut EventLoop) -> u8 {
            let id = event_loop
                .session_mgr
                .create(
                    Callsign::new("VK3ABC").unwrap(),
                    Callsign::new("W1AW").unwrap(),
                    0,
                    LinkCapabilities::default(),
                )
                .unwrap();
            let session = event_loop.session_mgr.get_mut(id).unwrap();
            let req = session.initiate().unwrap();
            let ack = peer().handle_connect_req(&req.payload).unwrap();
            session.handle_connect_ack(&ack.payload).unwrap();
            id
        }

        fn is_session_line(line: &str) -> bool {
            ["CONNECTED", "DISCONNECTED", "CONNECTING", "PENDING"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        }

        async fn next_session_line(reader: &mut BufReader<OwnedReadHalf>) -> String {
            loop {
                let line = read_line(reader).await;
                if is_session_line(&line) {
                    return line;
                }
            }
        }

        #[tokio::test]
        async fn initiator_hears_connected_then_remote_disconnect() {
            let mut station = EventLoop::new(session_config()).unwrap();
            let (mut reader, _writer) = connect_mock_vara_client(&mut station, 19440, 19441).await;
            station
                .handle_host_event(HostEvent::ConnectRequest {
                    client_id: 1,
                    source: "VK3ABC".into(),
                    destination: "W1AW".into(),
                })
                .await;
            let mut remote = peer();
            let ack = remote
                .handle_connect_req(&LinkCapabilities::default().to_bytes())
                .unwrap();
            station.handle_mac_pdu(ack).await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "CONNECTED VK3ABC W1AW 2400"
            );
            station.handle_mac_pdu(remote.disconnect().unwrap()).await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "DISCONNECTED"
            );
        }

        #[tokio::test]
        async fn responder_hears_connected_only_after_cfm() {
            let mut station = EventLoop::new(session_config()).unwrap();
            station.listening = true;
            let (mut reader, _writer) = connect_mock_vara_client(&mut station, 19442, 19443).await;
            let mut remote = peer();
            station.handle_mac_pdu(remote.initiate().unwrap()).await;
            assert!(read_available_lines(&mut reader)
                .await
                .iter()
                .all(|line| !is_session_line(line)));
            let cfm = remote
                .handle_connect_ack(&LinkCapabilities::default().to_bytes())
                .unwrap();
            station.handle_mac_pdu(cfm).await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "CONNECTED W1AW VK3ABC 2400"
            );
        }

        /// COP-28: the `CONNECTED` bandwidth token follows the live profile
        /// rather than being a constant — a VHF station reports its wider
        /// 112-carrier x 50 Hz = 5600 Hz occupancy.
        #[tokio::test]
        async fn vhf_station_reports_its_wider_bandwidth() {
            let mut config = session_config();
            config.engine.profile = "VHF_FAST".into();
            // COP-40: a wideband station declares its band honestly.
            config.radio.band = Some("2m".into());
            let mut station = EventLoop::new(config).unwrap();
            let (mut reader, _writer) = connect_mock_vara_client(&mut station, 19446, 19447).await;
            station
                .handle_host_event(HostEvent::ConnectRequest {
                    client_id: 1,
                    source: "VK3ABC".into(),
                    destination: "W1AW".into(),
                })
                .await;
            let mut remote = peer();
            let ack = remote
                .handle_connect_req(&LinkCapabilities::default().to_bytes())
                .unwrap();
            station.handle_mac_pdu(ack).await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "CONNECTED VK3ABC W1AW 5600"
            );
        }

        #[tokio::test]
        async fn local_disconnect_timeout_and_failed_connect_push_disconnected() {
            let mut station = EventLoop::new(session_config()).unwrap();
            let (mut reader, _writer) = connect_mock_vara_client(&mut station, 19444, 19445).await;
            seed_established(&mut station);
            station
                .handle_host_event(HostEvent::DisconnectRequest { client_id: 1 })
                .await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "DISCONNECTED"
            );
            let id = seed_established(&mut station);
            station.session_mgr.get_mut(id).unwrap().session_timeout = Duration::ZERO;
            station.reap_timed_out_sessions().await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "DISCONNECTED"
            );
            station
                .handle_host_event(HostEvent::ConnectRequest {
                    client_id: 1,
                    source: "VK3ABC".into(),
                    destination: "NOT/A/CALL".into(),
                })
                .await;
            assert_eq!(
                next_session_line(&mut reader).await.trim_end(),
                "DISCONNECTED"
            );
        }

        /// COP-28: Pat's startup burst — commands the parser does not know
        /// (`PUBLIC`, `CWID`), commands it accepts-but-ignores
        /// (`COMPRESSION`, `LISTEN`), `MYCALL`, `VERSION` and garbage — each
        /// gets exactly one reply, in order, and no reply is doubled.
        #[tokio::test]
        async fn pat_startup_commands_each_get_their_reply() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, mut write_half) =
                connect_bridged_vara_client(&mut event_loop, 19448, 19449).await;

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    write_half
                        .write_all(
                            b"PUBLIC ON\rCWID ON\rCOMPRESSION TEXT\rMYCALL W5AU\rLISTEN OFF\r\
                              VERSION\rFOOBAR\r",
                        )
                        .await
                        .unwrap();
                    let mut replies = Vec::new();
                    for _ in 0..7 {
                        replies.push(read_line(&mut reader).await.trim_end().to_string());
                    }
                    let version = format!("VERSION Coppa {}", env!("CARGO_PKG_VERSION"));
                    assert_eq!(
                        replies,
                        vec![
                            "WRONG",
                            "WRONG",
                            "OK",
                            "OK",
                            "OK",
                            version.trim_end(),
                            "WRONG",
                        ],
                        "each command gets exactly the reply the table promises"
                    );
                    assert!(
                        read_available_lines(&mut reader).await.is_empty(),
                        "no reply may be doubled"
                    );

                    // COP-45: the greeting and earlier query replies have been consumed,
                    // so each subsequent VERSION query must produce a fresh reply.
                    for _ in 0..2 {
                        write_half.write_all(b"VERSION\r").await.unwrap();
                        assert_eq!(read_line(&mut reader).await.trim_end(), version);
                        assert!(
                            read_available_lines(&mut reader).await.is_empty(),
                            "each VERSION query must produce exactly one reply"
                        );
                    }
                })
                .await;
        }

        /// COP-28: a well-formed `CONNECT` is answered `OK` before its
        /// outcome (`DISCONNECTED` on failure, PTT lines once the CONNECT_REQ
        /// goes on air) — never `WRONG`, which would hang Pat's dial. Also
        /// covers `DISCONNECT`'s `OK` on this port.
        #[tokio::test]
        async fn connect_gets_ok_before_its_outcome() {
            let config = session_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, mut write_half) =
                connect_bridged_vara_client(&mut event_loop, 19450, 19451).await;

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    // DISCONNECT with no session: exactly one line, its OK.
                    write_half.write_all(b"DISCONNECT\r").await.unwrap();
                    assert_eq!(read_line(&mut reader).await.trim_end(), "OK");
                    assert!(
                        read_available_lines(&mut reader).await.is_empty(),
                        "a session-less DISCONNECT produces nothing but its OK"
                    );

                    // A failed CONNECT: OK first, then the DISCONNECTED outcome.
                    write_half
                        .write_all(b"CONNECT VK3ABC NOT/A/CALL\r")
                        .await
                        .unwrap();
                    assert_eq!(read_line(&mut reader).await.trim_end(), "OK");
                    assert_eq!(read_line(&mut reader).await.trim_end(), "DISCONNECTED");

                    // A well-formed CONNECT: OK first, the CONNECT_REQ on air
                    // (PTT ON among the later lines), and no WRONG anywhere.
                    write_half
                        .write_all(b"CONNECT VK3ABC W1AW\r")
                        .await
                        .unwrap();
                    assert_eq!(read_line(&mut reader).await.trim_end(), "OK");
                    let mut saw_ptt_on = false;
                    for line in read_available_lines(&mut reader).await {
                        let line = line.trim_end();
                        if line == "PTT ON" {
                            saw_ptt_on = true;
                        }
                        assert_ne!(
                            line, "WRONG",
                            "a well-formed CONNECT is never answered WRONG"
                        );
                    }
                    assert!(
                        saw_ptt_on,
                        "the CONNECT_REQ must go on air after the OK (PTT ON)"
                    );
                })
                .await;
        }

        /// COP-28: `DISCONNECT` gets its `OK` on the socket, whether or not a
        /// session existed (the session-existence cases are covered by
        /// `local_disconnect_timeout_and_failed_connect_push_disconnected`).
        #[tokio::test]
        async fn disconnect_gets_ok() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, mut write_half) =
                connect_bridged_vara_client(&mut event_loop, 19458, 19459).await;

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    write_half.write_all(b"DISCONNECT\r").await.unwrap();
                    assert_eq!(read_line(&mut reader).await.trim_end(), "OK");
                    assert!(
                        read_available_lines(&mut reader).await.is_empty(),
                        "a session-less DISCONNECT produces nothing but its OK"
                    );
                })
                .await;
        }

        /// COP-28: a reply goes only to the client that asked, never to every
        /// attached command-port client.
        #[tokio::test]
        async fn replies_go_only_to_the_client_that_asked() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader_a, mut write_a) =
                connect_bridged_vara_client(&mut event_loop, 19452, 19453).await;

            // A second raw client on the same server: greeting, then silence.
            let stream_b = TcpStream::connect(("127.0.0.1", 19452)).await.unwrap();
            let (read_b, _write_b) = stream_b.into_split();
            let mut reader_b = BufReader::new(read_b);
            assert!(
                read_line(&mut reader_b).await.starts_with("VERSION"),
                "client B should also get the greeting"
            );

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    write_a.write_all(b"FOOBAR\r").await.unwrap();
                    assert_eq!(read_line(&mut reader_a).await.trim_end(), "WRONG");
                    let b_lines = read_available_lines(&mut reader_b).await;
                    assert!(
                        b_lines.iter().all(|l| l.trim_end() != "WRONG"),
                        "client B must not see client A's reply: {:?}",
                        b_lines
                    );
                })
                .await;
        }

        /// COP-28: a WebSocket-originated event (whose client ID is from
        /// `coppa_host::websocket::FIRST_CLIENT_ID`'s range) must never draw
        /// an OK/WRONG reply on a VARA command-port socket, and the WS
        /// `mycall` message stays a no-op.
        #[cfg(feature = "websocket")]
        #[tokio::test]
        async fn websocket_ids_never_draw_a_vara_reply() {
            use coppa_host::websocket::FIRST_CLIENT_ID;
            let config = session_config();
            let mut station = EventLoop::new(config).unwrap();
            let (mut reader, _writer) = connect_mock_vara_client(&mut station, 19454, 19455).await;

            station
                .handle_host_event(HostEvent::VaraCommand {
                    client_id: FIRST_CLIENT_ID,
                    command: r#"{"type":"mycall","callsign":"W5AU"}"#.to_string(),
                })
                .await;
            station
                .handle_host_event(HostEvent::ConnectRequest {
                    client_id: FIRST_CLIENT_ID,
                    source: "VK3ABC".to_string(),
                    destination: "NOT/A/CALL".to_string(),
                })
                .await;

            assert_eq!(
                station.local_callsign,
                Some(Callsign::new("VK3ABC").unwrap()),
                "the WS mycall message must stay a no-op"
            );
            let lines = read_available_lines(&mut reader).await;
            assert!(
                lines
                    .iter()
                    .all(|l| l.trim_end() != "OK" && l.trim_end() != "WRONG"),
                "a WebSocket-originated event must not draw a command-port reply: {:?}",
                lines
            );
        }

        /// COP-28: `IAMALIVE` reaches the command port while `run()` is
        /// running, the first one a full period after start (nothing at t=0).
        /// Uses a shortened `vara_keepalive_period` so the test runs in
        /// milliseconds; the real 60 s constant is pinned by
        /// `keepalive_sends_iamalive_every_sixty_seconds`.
        #[tokio::test]
        async fn iamalive_reaches_the_command_port_while_running() {
            let config = fast_ptt_config();
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.vara_keepalive_period = Duration::from_millis(250);
            let (mut reader, _write_half) =
                connect_bridged_vara_client(&mut event_loop, 19456, 19457).await;
            let started = Instant::now();

            let local = tokio::task::LocalSet::new();
            local.spawn_local(async move {
                let _ = event_loop.run().await;
            });

            local
                .run_until(async move {
                    assert_eq!(read_line(&mut reader).await.trim_end(), "IAMALIVE");
                    assert!(
                        started.elapsed() >= Duration::from_millis(200),
                        "the first IAMALIVE must arrive a full period after start, not at t=0"
                    );
                    assert_eq!(read_line(&mut reader).await.trim_end(), "IAMALIVE");
                })
                .await;
        }

        #[cfg(feature = "websocket")]
        #[tokio::test]
        async fn websocket_hears_connected_and_disconnected_and_status_follows() {
            use coppa_host::websocket::{WsServerMessage, WsStatus};
            // The same channel carries COP-65 `audio_level` messages (one per
            // transmission the connect/disconnect handshake makes); only the
            // session pushes count here.
            fn next_session_push(
                rx: &mut tokio::sync::broadcast::Receiver<String>,
            ) -> Option<WsServerMessage> {
                while let Ok(json) = rx.try_recv() {
                    let msg: WsServerMessage = serde_json::from_str(&json).unwrap();
                    if !matches!(msg, WsServerMessage::AudioLevel { .. }) {
                        return Some(msg);
                    }
                }
                None
            }
            let mut station = EventLoop::new(session_config()).unwrap();
            let (tx, mut rx) = tokio::sync::broadcast::channel(16);
            let status = Arc::new(Mutex::new(WsStatus::default()));
            station.set_ws_broadcast(tx);
            station.set_ws_status(status.clone());
            station
                .handle_host_event(HostEvent::ConnectRequest {
                    client_id: 1,
                    source: "VK3ABC".into(),
                    destination: "W1AW".into(),
                })
                .await;
            let mut remote = peer();
            let ack = remote
                .handle_connect_req(&LinkCapabilities::default().to_bytes())
                .unwrap();
            station.handle_mac_pdu(ack).await;
            let msg = next_session_push(&mut rx).expect("connected push");
            assert!(
                matches!(msg, WsServerMessage::Connected { remote_call } if remote_call == "W1AW")
            );
            {
                let snap = status.lock().await;
                assert!(snap.connected);
                assert_eq!(snap.remote_call.as_deref(), Some("W1AW"));
            }
            station.handle_mac_pdu(remote.disconnect().unwrap()).await;
            let msg = next_session_push(&mut rx).expect("disconnected push");
            assert!(matches!(msg, WsServerMessage::Disconnected));
            let snap = status.lock().await;
            assert!(!snap.connected);
            assert_eq!(snap.remote_call, None);
            assert!(
                next_session_push(&mut rx).is_none(),
                "exactly one push per transition"
            );
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

        /// Collect lines until a full window passes without another line.
        /// Command-port responses end in a bare CR (COP-27), so split on `\r`.
        async fn lines_within(
            reader: &mut BufReader<OwnedReadHalf>,
            window: Duration,
        ) -> Vec<String> {
            let mut lines = Vec::new();
            loop {
                let mut line = Vec::new();
                match tokio::time::timeout(window, reader.read_until(b'\r', &mut line)).await {
                    Ok(Ok(0)) | Err(_) => break,
                    Ok(Ok(_)) => lines.push(
                        String::from_utf8(line)
                            .expect("VARA response lines are UTF-8")
                            .trim()
                            .to_string(),
                    ),
                    Ok(Err(error)) => panic!("reading VARA response failed: {error}"),
                }
            }
            lines
        }

        #[tokio::test]
        async fn host_data_without_a_session_is_not_transmitted() {
            let mut event_loop = EventLoop::new(fast_ptt_config()).unwrap();
            let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(producer);
            let (mut reader, _write_half, data_addr) =
                connect_bridged_vara_client_ephemeral(&mut event_loop).await;
            let mut data = TcpStream::connect(data_addr).await.unwrap();
            let tx = event_loop.event_sender();
            let local = tokio::task::LocalSet::new();
            let running = local.spawn_local(async move { event_loop.run().await });

            local
                .run_until(async move {
                    data.write_all(b"hello").await.unwrap();
                    let lines = lines_within(&mut reader, Duration::from_millis(1500)).await;
                    assert!(
                        !lines
                            .iter()
                            .any(|s| s.starts_with("PTT") || s.starts_with("BUFFER")),
                        "host data without a session must not transmit, got {lines:?}"
                    );
                    tx.send(DaemonEvent::Shutdown).await.unwrap();
                    running.await.unwrap().unwrap();
                })
                .await;
            assert_eq!(consumer.read(&mut [0.0; 1024]), 0);
        }

        #[tokio::test]
        async fn host_data_while_connecting_is_not_transmitted() {
            let mut config = fast_ptt_config();
            config.engine.callsign = "N0CALL".into();
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, mut writer, data_addr) =
                connect_bridged_vara_client_ephemeral(&mut event_loop).await;
            let mut data = TcpStream::connect(data_addr).await.unwrap();
            let tx = event_loop.event_sender();
            let local = tokio::task::LocalSet::new();
            let running = local.spawn_local(async move { event_loop.run().await });

            local
                .run_until(async move {
                    writer.write_all(b"CONNECT N0CALL W1AW\r").await.unwrap();
                    let lines = lines_within(&mut reader, Duration::from_millis(1500)).await;
                    let ptt: Vec<&str> = lines
                        .iter()
                        .map(String::as_str)
                        .filter(|s| s.starts_with("PTT"))
                        .collect();
                    assert_eq!(ptt, ["PTT ON", "PTT OFF"], "CONNECT_REQ must transmit");

                    data.write_all(b"hello").await.unwrap();
                    let lines = lines_within(&mut reader, Duration::from_millis(1500)).await;
                    assert!(
                        !lines
                            .iter()
                            .any(|s| s.starts_with("PTT") || s.starts_with("BUFFER")),
                        "host data while connecting must not transmit, got {lines:?}"
                    );
                    tx.send(DaemonEvent::Shutdown).await.unwrap();
                    running.await.unwrap().unwrap();
                })
                .await;
        }

        /// Required scenario: "transmit -> PTT ON/OFF bracket."
        ///
        /// `EventLoop` isn't `Send` (its engine holds trait objects/raw-pointer
        /// ring-buffer internals that aren't), so `run()` can't go through a plain
        /// `tokio::spawn`. A `LocalSet` + `spawn_local` runs it concurrently with
        /// the rest of this test on the same thread instead, with no `Send` bound.
        #[tokio::test]
        async fn test_ptt_telemetry_brackets_transmission() {
            let mut config = fast_ptt_config();
            // Sessionless TX on: the simplest way to make host data key PTT.
            config.host.sessionless_tx_enabled = true;
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
            // `run()`'s event-channel dispatch of the `DaemonEvent::TxComplete`
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

                    // COP-28: the OK reply comes first, before the PTT
                    // bracket the command causes.
                    let mut lines = Vec::new();
                    lines.push(read_line(&mut reader).await);
                    assert_eq!(
                        lines[0].trim_end(),
                        "OK",
                        "TUNE must be answered OK before its PTT bracket"
                    );
                    for _ in 0..7 {
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

        /// The `BUFFER n` values among `lines`, terminator stripped.
        fn buffer_values(lines: &[String]) -> Vec<&str> {
            lines
                .iter()
                .map(|s| s.trim_end())
                .filter(|s| s.starts_with("BUFFER"))
                .collect()
        }

        /// COP-30: on the non-ARQ raw path, `BUFFER` counts host payload
        /// bytes (not frames), and a frame's bytes leave the count only when
        /// PTT releases after it has played -- not when it is popped for TX.
        /// COP-41: the host writes now go through the chunker, so the
        /// 5000-byte write becomes ~50 queue entries and intermediate
        /// `BUFFER` lines appear both as it enqueues and as it drains one
        /// frame per release -- the byte totals are asserted instead of the
        /// old exact six-line sequence.
        ///
        /// Forces `is_transmitting = true` before writing so every byte
        /// stacks up in the queue instead of draining immediately
        /// (simulating a client bursting data faster than the half-duplex
        /// link can send it), then drains deterministically (bypassing the
        /// real scheduled-PTT-release timer, which
        /// `test_ptt_telemetry_brackets_transmission` already covers) by
        /// calling the same internal hooks each real transmission's
        /// completion would trigger.
        #[tokio::test]
        async fn test_buffer_telemetry_counts_bytes_and_drains_on_ptt_release() {
            let mut config = fast_ptt_config();
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19490, 19491).await;

            event_loop.is_transmitting = true;
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x11; 5],
                })
                .await;
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x22; 5000],
                })
                .await;
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x33; 6],
                })
                .await;
            assert!(!event_loop.tx_queue.is_empty());

            event_loop.is_transmitting = false;
            while !event_loop.tx_queue.is_empty() {
                event_loop.handle_ptt_change(false).await;
                // `broadcast_vara` drops lines once the client's 64-line
                // response channel is full, and nothing in the drain yields
                // to the forwarder task: let it flush each frame's lines.
                tokio::task::yield_now().await;
            }
            // Release the final frame's bytes too.
            event_loop.handle_ptt_change(false).await;
            assert_eq!(event_loop.tx_queue.len(), 0);

            let lines = read_available_lines(&mut reader).await;
            let values = buffer_values(&lines);
            let nums: Vec<usize> = values
                .iter()
                .filter_map(|l| l.strip_prefix("BUFFER ").and_then(|n| n.parse().ok()))
                .collect();
            assert_eq!(nums.first(), Some(&5), "got: {:?}", lines);
            // Everything enqueues while keyed, so the running count peaks at
            // exactly 5 + 5000 + 6 -- the byte totals COP-30 pinned.
            let peak = nums.iter().position(|&n| n == 5011);
            assert_eq!(
                nums.last(),
                Some(&0),
                "the last BUFFER line must be zero, got: {:?}",
                lines
            );
            let peak = peak.expect("BUFFER must peak at the full 5 + 5000 + 6 bytes");
            assert!(
                nums[..=peak].windows(2).all(|w| w[0] < w[1]),
                "values must rise to the peak as chunks enqueue, got: {:?}",
                lines
            );
            assert!(
                nums[peak..].windows(2).all(|w| w[0] > w[1]),
                "values must decline as each frame's PTT releases, got: {:?}",
                lines
            );
        }

        /// COP-30: with ARQ on, a segment's bytes stay in `BUFFER` after its
        /// frame has played, until the peer's ACK for it is decoded.
        #[tokio::test]
        async fn test_buffer_held_until_arq_ack() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            // COP-29: these exercise the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19492, 19493).await;

            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x44; 5],
                })
                .await;
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x55; 40],
                })
                .await;
            // Both frames finish playing; neither is acked yet.
            event_loop.handle_ptt_change(false).await;
            event_loop.handle_ptt_change(false).await;
            assert_eq!(event_loop.arq_tx.as_ref().unwrap().in_flight(), 2);
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines).last(),
                Some(&"BUFFER 45"),
                "played but un-acked bytes must stay counted, got: {:?}",
                lines
            );

            // The peer cumulatively ACKs seq 0 (the 5 bytes).
            let session_id = event_loop.arq_session_id;
            event_loop
                .decode_and_dispatch_audio(&super::peer_frame(
                    &TransportPdu::new_ack_with_rate(session_id, 1, 0, 2),
                    coppa_codec::ofdm::CoppaProfile::hf_standard(),
                ))
                .await;
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines),
                vec!["BUFFER 40"],
                "the ACK for seq 0 must remove its 5 bytes, got: {:?}",
                lines
            );

            // Then seq 1 (the 40 bytes).
            super::unstick_decoder(&mut event_loop);
            event_loop
                .decode_and_dispatch_audio(&super::peer_frame(
                    &TransportPdu::new_ack_with_rate(session_id, 2, 0, 2),
                    coppa_codec::ofdm::CoppaProfile::hf_standard(),
                ))
                .await;
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines),
                vec!["BUFFER 0"],
                "the ACK for seq 1 must empty the buffer, got: {:?}",
                lines
            );
            assert_eq!(event_loop.arq_tx.as_ref().unwrap().in_flight(), 0);
        }

        /// COP-30: established-session data reports `BUFFER` too -- before
        /// its PTT keys -- and drains at PTT release. COP-41 paces session
        /// chunks one frame per keyed interval, so the second write's bytes
        /// are held in the session queue while the first frame plays: the
        /// exact old line sequence no longer holds, and the assertions check
        /// the BUFFER values' first/contains plus the PTT alternation
        /// instead.
        #[tokio::test]
        async fn test_session_path_emits_buffer_and_drains_on_ptt_release() {
            let mut event_loop = EventLoop::new(session_config()).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19494, 19495).await;
            seed_established(&mut event_loop);

            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x66; 12],
                })
                .await;
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![0x77; 4],
                })
                .await;
            event_loop.handle_ptt_change(false).await;

            let lines = read_available_lines(&mut reader).await;
            let values = buffer_values(&lines);
            assert_eq!(
                values.first(),
                Some(&"BUFFER 12"),
                "the first write's bytes must be counted before they key, got: {:?}",
                lines
            );
            assert!(
                values.contains(&"BUFFER 16"),
                "the second write's bytes must be counted while the first frame \
                 is keyed, got: {:?}",
                lines
            );
            assert!(
                values.contains(&"BUFFER 4"),
                "the held second frame must replace the released first frame's \
                 bytes, got: {:?}",
                lines
            );
            // Pacing (COP-41 D3): every PTT ON after the first must follow a
            // release -- one frame per keyed interval, never two frames in
            // one keyed blast.
            let ptt: Vec<&str> = lines
                .iter()
                .map(|s| s.trim_end())
                .filter(|s| s.starts_with("PTT"))
                .collect();
            assert_eq!(
                ptt,
                ["PTT ON", "PTT OFF", "PTT ON"],
                "two writes must key two separate intervals, got: {:?}",
                lines
            );
            let first_buffer = lines.iter().position(|l| l.trim_end() == "BUFFER 12");
            let first_ptt_on = lines.iter().position(|l| l.trim_end() == "PTT ON");
            assert!(
                first_buffer.is_some() && first_ptt_on.is_some() && first_buffer < first_ptt_on,
                "BUFFER 12 must reach the host before the first PTT ON, got: {:?}",
                lines
            );
        }

        /// COP-30 asserted the old drop behavior: a payload the full ARQ
        /// window rejected never reached `BUFFER`. COP-41 (D5) replaced the
        /// window-full drop with a backlog hold, so the ninth write's bytes
        /// ARE counted now -- held, not dropped, waiting for ACKs to free
        /// window space.
        #[tokio::test]
        async fn test_arq_window_full_holds_in_backlog() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            // COP-29: these exercise the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19496, 19497).await;

            event_loop.is_transmitting = true;
            for i in 0..9u8 {
                event_loop
                    .handle_host_event(HostEvent::DataReceived {
                        client_id: 1,
                        data: vec![i; 10],
                    })
                    .await;
            }
            assert_eq!(event_loop.arq_tx.as_ref().unwrap().in_flight(), 8);
            assert_eq!(event_loop.arq_tx_backlog.len(), 10);

            let lines = read_available_lines(&mut reader).await;
            let values = buffer_values(&lines);
            assert_eq!(
                values.len(),
                9,
                "one BUFFER line per write (every byte is held), got: {:?}",
                lines
            );
            assert_eq!(values.last(), Some(&"BUFFER 90"));
        }

        /// COP-41: session chunks are sized by MEASURING the framed PDU
        /// (`encoded_payload_len`), never predicted -- 2000 incompressible
        /// bytes split into chunks that each fit the level-2 budget (117),
        /// pinning the halving path for binary content. Without chunking
        /// this write produced zero samples (the research probe).
        #[tokio::test]
        async fn session_chunks_respect_the_encoded_budget() {
            let mut event_loop = EventLoop::new(session_config()).unwrap();
            seed_established(&mut event_loop);
            // Hold the queue so every chunk stays inspectable.
            event_loop.is_transmitting = true;

            let mut seed: u32 = 0xC0_41_C0_41;
            let data: Vec<u8> = (0..2000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (seed >> 16) as u8
                })
                .collect();
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: data.clone(),
                })
                .await;

            assert!(
                !event_loop.session_tx_queue.is_empty(),
                "2000 incompressible bytes must chunk, not vanish"
            );
            let budget = max_payload_for_level(event_loop.engine.speed_level()).unwrap();
            let mut host_bytes = 0usize;
            let mut host_chunks: Vec<Vec<u8>> = Vec::new();
            for chunk in &event_loop.session_tx_queue {
                assert!(
                    event_loop.engine.encoded_payload_len(&chunk.pdu_bytes) <= budget,
                    "every chunk's framed PDU must fit the level budget"
                );
                host_bytes += chunk.host_len;
                host_chunks.push(chunk.pdu_bytes[MacPdu::HEADER_SIZE..].to_vec());
            }
            assert_eq!(host_bytes, 2000, "chunks must partition the write");
            assert_eq!(
                host_chunks.concat(),
                data,
                "chunk payloads must reassemble the input in order"
            );

            // And the first chunk really encodes once released.
            event_loop.is_transmitting = false;
            let first_len = event_loop.session_tx_queue.front().unwrap().host_len;
            event_loop.drain_session_tx_queue().await;
            assert_eq!(event_loop.tx_on_air_bytes, first_len);
            assert_eq!(event_loop.session_tx_queue.len() + 1, host_chunks.len());
        }

        /// COP-41: no pacing regression for the common case -- a small
        /// write on an idle link still transmits immediately, before any
        /// PTT release.
        #[tokio::test]
        async fn small_session_write_transmits_immediately_when_idle() {
            let mut event_loop = EventLoop::new(session_config()).unwrap();
            let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(producer);
            seed_established(&mut event_loop);

            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: b"hello".to_vec(),
                })
                .await;

            let mut buf = vec![0.0f32; 1_000_000];
            assert!(
                consumer.read(&mut buf) > 0,
                "a 5-byte write on an idle link must transmit without waiting \
                 for a PTT release"
            );
            assert!(event_loop.session_tx_queue.is_empty());
        }

        /// COP-41 (D3): a write that arrives while the link is keyed waits
        /// for the current frame's PTT release before its own keyed
        /// interval -- it never silently joins the keyed interval as the
        /// pre-COP-41 session path did.
        #[tokio::test]
        async fn session_write_while_transmitting_waits_for_release() {
            let mut event_loop = EventLoop::new(session_config()).unwrap();
            let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(producer);
            seed_established(&mut event_loop);
            event_loop.is_transmitting = true;

            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: b"hello".to_vec(),
                })
                .await;
            let mut buf = vec![0.0f32; 1_000_000];
            assert_eq!(
                consumer.read(&mut buf),
                0,
                "nothing may join an already-keyed interval"
            );
            assert_eq!(event_loop.session_tx_queue.len(), 1);

            event_loop.handle_ptt_change(false).await;
            assert!(
                consumer.read(&mut buf) > 0,
                "the release must start the queued chunk's own keyed interval"
            );
            assert!(event_loop.session_tx_queue.is_empty());
        }

        /// A session's unsent chunks die with the session (COP-41 review):
        /// after a local DISCONNECT, a remote DISCONNECT, or a timeout, the
        /// queue holds none of its chunks, `BUFFER` stops counting them,
        /// and the next PTT release keys nothing -- before this, the queue
        /// kept transmitting the dead session's data on every release.
        #[tokio::test]
        async fn ended_session_discards_its_unsent_chunks() {
            #[derive(Debug, Clone, Copy)]
            enum End {
                LocalDisconnect,
                RemoteDisconnect,
                Timeout,
            }
            for end in [End::LocalDisconnect, End::RemoteDisconnect, End::Timeout] {
                let mut event_loop = EventLoop::new(session_config()).unwrap();
                let (producer, mut consumer) = coppa_audio::audio_ring(4_000_000);
                event_loop.set_audio_out(producer);
                let id = seed_established(&mut event_loop);
                // Hold the queue so the whole write stays unsent.
                event_loop.is_transmitting = true;
                event_loop
                    .handle_host_event(HostEvent::DataReceived {
                        client_id: 1,
                        data: (0..1000).map(|i| (i % 251) as u8).collect(),
                    })
                    .await;
                assert!(event_loop.session_tx_queue.len() > 1, "{end:?}");
                assert_eq!(event_loop.buffered_tx_bytes(), 1000, "{end:?}");

                match end {
                    End::LocalDisconnect => {
                        event_loop
                            .handle_host_event(HostEvent::DisconnectRequest { client_id: 1 })
                            .await;
                    }
                    End::RemoteDisconnect => {
                        let disc = MacPdu::new(
                            MacFrameType::Disconnect,
                            Callsign::new("VK3ABC").unwrap(),
                            Callsign::new("W1AW").unwrap(),
                            0,
                            Vec::new(),
                        );
                        event_loop.handle_incoming_disconnect(disc).await;
                    }
                    End::Timeout => {
                        // A zero timeout, not a back-dated `Instant`: on
                        // Windows `Instant` cannot go back past boot.
                        event_loop.session_mgr.get_mut(id).unwrap().session_timeout =
                            Duration::ZERO;
                        event_loop.reap_timed_out_sessions().await;
                    }
                }
                assert!(event_loop.session_mgr.get(id).is_none(), "{end:?}");
                assert!(event_loop.session_tx_queue.is_empty(), "{end:?}");
                assert_eq!(event_loop.buffered_tx_bytes(), 0, "{end:?}");

                // Discard whatever the end itself keyed (the local
                // DISCONNECT frame), then release: no stale chunk may play.
                let mut buf = vec![0.0f32; 4_000_000];
                while consumer.read(&mut buf) > 0 {}
                let frames_before = event_loop.tx_frame_count;
                event_loop.handle_ptt_change(false).await;
                assert_eq!(
                    consumer.read(&mut buf),
                    0,
                    "{end:?}: the release must not transmit the ended session's data"
                );
                assert_eq!(event_loop.tx_frame_count, frames_before, "{end:?}");
            }
        }

        /// Unsent session chunks resume after canceled playback, once the
        /// output worker has discarded the canceled frame's buffered audio.
        #[tokio::test(start_paused = true)]
        async fn cancelled_output_resumes_session_chunks_after_discard() {
            let mut rig = TxRig::new(session_config());
            seed_established(&mut rig.event_loop);
            rig.event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: (0..300).map(|i| (i % 251) as u8).collect(),
                })
                .await;
            let remaining: Vec<_> = rig
                .event_loop
                .session_tx_queue
                .iter()
                .map(|chunk| {
                    rig.event_loop
                        .engine
                        .encode_bytes(&chunk.pdu_bytes)
                        .unwrap()
                })
                .collect();
            assert!(remaining.len() > 1);
            let held_bytes = rig.event_loop.buffered_tx_bytes() - rig.event_loop.tx_on_air_bytes;

            // Stall with canceled audio in both rings and the retained suffix.
            for _ in 0..16 {
                rig.pump(0);
            }
            let (generation, completed) = rig.next_completion().await;
            assert_eq!((generation, completed), (1, false));
            let submitted = {
                let output = rig.event_loop.tx_output();
                output.submitted() - output.pending_len() as u64
            };
            let tx_output = rig.event_loop.tx_output.clone();
            tokio::join!(
                rig.event_loop.handle_tx_complete(generation, completed),
                async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    assert_eq!(
                        lock_tx_output(&tx_output).submitted(),
                        submitted,
                        "the next frame must wait for the discard barrier"
                    );
                    for _ in 0..100 {
                        if rig.feedback.played() >= submitted {
                            break;
                        }
                        rig.worker
                            .step(
                                &mut rig.outer,
                                &mut rig.sink,
                                crate::audio_output::test_support::sim_progress,
                            )
                            .unwrap();
                        rig.sink.callback(0);
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    assert!(rig.feedback.played() >= submitted, "discard must settle");
                }
            );
            assert!(
                rig.event_loop.is_transmitting,
                "the unsent tail must resume"
            );
            assert_eq!(rig.event_loop.buffered_tx_bytes(), held_bytes);
            assert_eq!(rig.event_loop.session_tx_queue.len(), remaining.len() - 1);

            let mut expected = Vec::new();
            for frame in remaining {
                expected.extend(frame);
                rig.deliver(expected.len(), &[480, 1000]);
                assert_eq!(
                    rig.sink.captured, expected,
                    "only unsent chunks should play"
                );
                let (generation, completed) = rig.next_completion().await;
                assert!(completed);
                rig.event_loop
                    .handle_tx_complete(generation, completed)
                    .await;
            }
            assert!(!rig.event_loop.is_transmitting);
            assert!(rig.event_loop.session_tx_queue.is_empty());
            assert_eq!(rig.event_loop.buffered_tx_bytes(), 0);
        }

        /// COP-41 (D5): the ARQ window's overflow is HELD in a backlog and
        /// pumped into segments as ACKs free window space -- the no-silent-
        /// loss test. Before COP-41 one oversized segment was tracked,
        /// never encoded, and stayed in `BUFFER` forever.
        #[tokio::test]
        async fn large_arq_write_fills_window_then_pumps_on_ack() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19500, 19501).await;

            let mut seed: u32 = 0xB2_F2_00_2D;
            let data: Vec<u8> = (0..2000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (seed >> 16) as u8
                })
                .collect();
            event_loop
                .handle_host_event(HostEvent::DataReceived { client_id: 1, data })
                .await;

            assert_eq!(
                event_loop.arq_tx.as_ref().unwrap().in_flight(),
                8,
                "the 8-segment default window must be full"
            );
            assert!(
                !event_loop.arq_tx_backlog.is_empty(),
                "the window's overflow must be held in the backlog"
            );
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines).last(),
                Some(&"BUFFER 2000"),
                "every byte of the write must be counted, got: {:?}",
                lines
            );

            // Cumulatively ACK each full window (real decoded ACK frames,
            // the same `peer_frame` path COP-30's test uses) until every
            // byte has been segmented and acked. Counting each segment's
            // payload exactly once proves no byte was dropped.
            let mut segmented_total = 0usize;
            loop {
                let in_flight = event_loop.arq_tx.as_ref().unwrap().in_flight();
                if in_flight == 0 {
                    assert!(
                        event_loop.arq_tx_backlog.is_empty(),
                        "an empty window with a non-empty backlog would strand \
                         bytes: the ACK-side pump must have segmented them"
                    );
                    break;
                }
                let base = event_loop.arq_tx.as_ref().unwrap().send_base();
                for i in 0..in_flight {
                    let seq = base.wrapping_add(i);
                    if let Some(seg) = event_loop.arq_tx.as_ref().unwrap().get_segment_data(seq) {
                        segmented_total += seg.len();
                    }
                }
                let ack_num = base.wrapping_add(in_flight);
                let session_id = event_loop.arq_session_id;
                super::unstick_decoder(&mut event_loop);
                event_loop
                    .decode_and_dispatch_audio(&super::peer_frame(
                        &TransportPdu::new_ack_with_rate(session_id, ack_num, 0, 2),
                        coppa_codec::ofdm::CoppaProfile::hf_standard(),
                    ))
                    .await;
            }
            assert_eq!(
                segmented_total, 2000,
                "every byte of the write must pass through an ARQ segment"
            );
            assert!(event_loop.arq_tx_backlog.is_empty());
            assert_eq!(event_loop.arq_tx.as_ref().unwrap().in_flight(), 0);
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines).last(),
                Some(&"BUFFER 0"),
                "BUFFER must reach 0 once everything is acked, got: {:?}",
                lines
            );
        }

        /// COP-41 (D4): ARQ segments are sized against the ladder-MINIMUM
        /// budget (56), so every segment re-encodes at any speed level a
        /// later retransmit or `RateLoop` drop uses.
        #[tokio::test]
        async fn arq_chunks_encode_at_every_level() {
            let mut config = fast_ptt_config();
            config.engine.arq_enabled = true;
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            // Hold the queue so the window's segments stay inspectable.
            event_loop.is_transmitting = true;

            let mut seed: u32 = 0x4A_C1_00_2D;
            let data: Vec<u8> = (0..2000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (seed >> 16) as u8
                })
                .collect();
            event_loop
                .handle_host_event(HostEvent::DataReceived { client_id: 1, data })
                .await;

            let entries: Vec<Vec<u8>> = event_loop
                .tx_queue
                .iter()
                .filter(|(seq, _)| seq.is_some())
                .map(|(_, bytes)| bytes.clone())
                .collect();
            assert!(
                entries.len() == 8,
                "the full window's worth of segments must be queued, got {}",
                entries.len()
            );
            for bytes in &entries {
                assert!(
                    event_loop.engine.encoded_payload_len(bytes) <= 56,
                    "every segment must fit the ladder-minimum budget"
                );
            }
            let levels = event_loop.engine.available_speed_levels();
            assert!(levels.len() >= 8, "the ladder has 8+ levels");
            for level in levels {
                event_loop
                    .engine
                    .set_speed_level(level)
                    .expect("ladder level must apply");
                for bytes in &entries {
                    assert!(
                        event_loop.engine.encode_bytes(bytes).is_ok(),
                        "level {level} must re-encode every segment (a segment \
                         that no longer fits after a rate drop is stuck forever)"
                    );
                }
            }
        }

        /// COP-41: the raw non-ARQ path chunks too -- one 8000-byte event
        /// becomes many modem-sized frames whose bytes reassemble exactly,
        /// draining one frame per PTT release down to `BUFFER 0`.
        #[tokio::test]
        async fn large_raw_write_chunks_into_modem_sized_frames() {
            let mut config = fast_ptt_config();
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19502, 19503).await;
            event_loop.is_transmitting = true;

            let data: Vec<u8> = (0..8000).map(|i| ((i * 31 + 7) % 251) as u8).collect();
            event_loop
                .handle_host_event(HostEvent::DataReceived { client_id: 1, data })
                .await;

            assert!(
                event_loop.tx_queue.len() > 1,
                "an 8000-byte write must become many frames, got {}",
                event_loop.tx_queue.len()
            );
            let budget = max_payload_for_level(event_loop.engine.speed_level()).unwrap();
            let mut joined = Vec::with_capacity(8000);
            for (seq, bytes) in &event_loop.tx_queue {
                assert!(seq.is_none(), "non-ARQ entries carry no seq");
                assert!(
                    event_loop.engine.encoded_payload_len(bytes) <= budget,
                    "every entry must fit the level budget"
                );
                joined.extend_from_slice(bytes);
            }
            assert_eq!(
                joined,
                (0..8000)
                    .map(|i| ((i * 31 + 7) % 251) as u8)
                    .collect::<Vec<u8>>()
            );

            event_loop.is_transmitting = false;
            while !event_loop.tx_queue.is_empty() {
                event_loop.handle_ptt_change(false).await;
                // `broadcast_vara` drops lines once the client's 64-line
                // response channel is full, and nothing in the drain yields
                // to the forwarder task: let it flush each frame's lines.
                tokio::task::yield_now().await;
            }
            event_loop.handle_ptt_change(false).await;
            let lines = read_available_lines(&mut reader).await;
            assert_eq!(
                buffer_values(&lines).last(),
                Some(&"BUFFER 0"),
                "BUFFER must drain to 0, got: {:?}",
                lines
            );
        }

        /// COP-41: an incompressible write that cannot encode as one frame
        /// is split, not dropped -- without chunking, 300 random bytes
        /// failed to encode and produced zero audio (research probe: 102
        /// random bytes -> 182 encoded > the 117-byte level-2 budget).
        #[tokio::test]
        async fn incompressible_oversize_write_is_split_not_dropped() {
            let mut config = fast_ptt_config();
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (producer, mut consumer) = coppa_audio::audio_ring(4_000_000);
            event_loop.set_audio_out(producer);

            let mut seed: u32 = 0x1C_0F_41;
            let data: Vec<u8> = (0..300)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (seed >> 16) as u8
                })
                .collect();
            event_loop
                .handle_host_event(HostEvent::DataReceived { client_id: 1, data })
                .await;

            let mut buf = vec![0.0f32; 4_000_000];
            assert!(
                consumer.read(&mut buf) > 0,
                "the first chunk must transmit immediately on the idle link"
            );
            assert!(
                event_loop.tx_queue.len() >= 2,
                "300 incompressible bytes must be split into several frames"
            );
            while !event_loop.tx_queue.is_empty() {
                event_loop.handle_ptt_change(false).await;
            }
            event_loop.handle_ptt_change(false).await;
            assert_eq!(
                event_loop.buffered_tx_bytes(),
                0,
                "every chunk must have played and left the count"
            );
        }

        /// COP-41: an empty host write produces no queue entry, no frame,
        /// and no `BUFFER` line (it used to encode a header-only PDU and
        /// transmit it).
        #[tokio::test]
        async fn empty_write_produces_no_frame_or_buffer_line() {
            let mut config = session_config();
            // COP-29: this exercises the sessionless raw/ARQ path, now opt-in.
            config.host.sessionless_tx_enabled = true;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut reader, _write_half) =
                connect_mock_vara_client(&mut event_loop, 19504, 19505).await;
            let (producer, mut consumer) = coppa_audio::audio_ring(1_000_000);
            event_loop.set_audio_out(producer);

            // Sessionless (raw path)...
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![],
                })
                .await;
            // ...and on an established session.
            seed_established(&mut event_loop);
            event_loop
                .handle_host_event(HostEvent::DataReceived {
                    client_id: 1,
                    data: vec![],
                })
                .await;

            assert!(event_loop.tx_queue.is_empty());
            assert!(event_loop.session_tx_queue.is_empty());
            assert!(event_loop.arq_tx_backlog.is_empty());
            let mut buf = vec![0.0f32; 1_000_000];
            assert_eq!(
                consumer.read(&mut buf),
                0,
                "no frame may be built from an empty write"
            );
            let lines = read_available_lines(&mut reader).await;
            assert!(
                buffer_values(&lines).is_empty(),
                "an empty write must not move BUFFER, got: {:?}",
                lines
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
            let mut config = tx_config();
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
            let mut config = tx_config();
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
            let config = tx_config();
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

            assert!(
                !event_loop.id_due(),
                "no ID should be due without a configured callsign"
            );
            event_loop.transmit_samples(&payload_samples).await;
            // COP-37: the transmission itself is refused, ID and all.
            assert_eq!(
                audio_out_rx.available(),
                0,
                "nothing, ID or payload, should be sent without a configured callsign"
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

    // ── COP-40: operating-frequency source and the wideband HF/VHF gate ──

    mod cop40_frequency_gate {
        use super::*;
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::Mutex;

        /// A fake rigctld serving one client: answers `f` with whatever the
        /// shared answer holds, `T <n>` with `RPRT 0`, `t` with the keyed
        /// state. Setting the shared `alive` flag to false makes it hang up
        /// on the next command, like a rigctld that died mid-session.
        struct FakeRig {
            addr: String,
            answer: Arc<Mutex<String>>,
            alive: Arc<AtomicBool>,
            /// How many `f` (frequency) queries the rig has answered (COP-21).
            f_queries: Arc<std::sync::atomic::AtomicUsize>,
        }

        fn fake_rigctld(answer: &str) -> FakeRig {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let answer = Arc::new(Mutex::new(answer.to_string()));
            let alive = Arc::new(AtomicBool::new(true));
            let f_queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let (shared, live, queries) = (
                Arc::clone(&answer),
                Arc::clone(&alive),
                Arc::clone(&f_queries),
            );
            std::thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut writer = stream.try_clone().unwrap();
                let mut ptt = "0".to_string();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if !live.load(Ordering::SeqCst) {
                        break;
                    }
                    let reply = match line.trim() {
                        "f" => {
                            // Counted before the reply is written, so a read
                            // after the client has its answer is race-free.
                            queries.fetch_add(1, Ordering::SeqCst);
                            shared.lock().unwrap().clone()
                        }
                        "t" => ptt.clone(),
                        cmd if cmd.starts_with("T ") => {
                            ptt = cmd[2..].to_string();
                            "RPRT 0".to_string()
                        }
                        _ => "RPRT -1".to_string(),
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
            FakeRig {
                addr,
                answer,
                alive,
                f_queries,
            }
        }

        fn wideband_config() -> DaemonConfig {
            let mut config = tx_config();
            config.engine.profile = "VHF_FAST".into();
            config.audio.sample_rate = 100_000_000;
            config.audio.buffer_size = 0;
            config.radio.ptt_pre_delay_ms = 0;
            config.radio.ptt_tail_delay_ms = 0;
            config
        }

        fn rigctld_config(rig: &FakeRig) -> DaemonConfig {
            let mut config = wideband_config();
            config.radio.ptt_method = "rigctld".into();
            config.radio.rigctld_address = rig.addr.clone();
            config
        }

        fn startup_error(config: DaemonConfig) -> String {
            EventLoop::new(config)
                .err()
                .expect("startup must be refused")
                .to_string()
        }

        // ── Phase 3: startup gate ─────────────────────────────────────────

        #[test]
        fn wideband_profile_without_any_frequency_source_is_a_startup_error() {
            let err = startup_error(wideband_config());
            assert!(err.contains("97.307"), "{err}");
            assert!(err.contains("frequency_hz"), "{err}");
            assert!(err.contains("VHF_FAST"), "{err}");
            assert!(err.contains("5600 Hz"), "{err}");
        }

        #[test]
        fn wideband_profile_with_a_vhf_band_label_starts() {
            let mut config = wideband_config();
            config.radio.band = Some("2m".into());
            assert!(EventLoop::new(config).is_ok());
        }

        #[test]
        fn wideband_profile_with_a_stated_vhf_frequency_starts() {
            let mut config = wideband_config();
            config.radio.frequency_hz = Some(144_500_000);
            assert!(EventLoop::new(config).is_ok());
        }

        #[test]
        fn wideband_profile_with_a_stated_hf_frequency_is_a_startup_error() {
            let mut config = wideband_config();
            config.radio.frequency_hz = Some(14_074_000);
            let err = startup_error(config);
            assert!(err.contains("below 29.7 MHz"), "{err}");
            assert!(err.contains("97.307"), "{err}");
        }

        #[test]
        fn wideband_profile_with_band_10m_is_a_startup_error() {
            let mut config = wideband_config();
            config.radio.band = Some("10m".into());
            let err = startup_error(config);
            assert!(err.contains("97.307"), "{err}");
        }

        #[test]
        fn a_typod_band_is_a_startup_error_even_for_narrowband() {
            let mut config = DaemonConfig::default();
            config.radio.band = Some("21m".into());
            let err = startup_error(config);
            assert!(err.contains("invalid [radio] band \"21m\""), "{err}");
        }

        #[test]
        fn wideband_profile_with_dead_rigctld_and_no_static_source_is_a_startup_error() {
            let mut config = wideband_config();
            config.radio.ptt_method = "rigctld".into();
            config.radio.rigctld_address = "127.0.0.1:1".into();
            let err = startup_error(config);
            assert!(err.contains("frequency source"), "{err}");
            assert!(err.contains("127.0.0.1:1"), "{err}");
            assert!(err.contains("is unreachable"), "{err}");
        }

        #[test]
        fn wideband_profile_with_rigctld_not_answering_f_is_a_startup_error() {
            let rig = fake_rigctld("CHKVFO?");
            let err = startup_error(rigctld_config(&rig));
            assert!(err.contains("frequency source"), "{err}");
            assert!(
                err.contains("is not answering the frequency query"),
                "{err}"
            );
        }

        #[test]
        fn wideband_profile_with_answering_rigctld_starts() {
            let rig = fake_rigctld("144500000");
            let event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            assert!(event_loop.ptt.is_rigctld());
        }

        #[test]
        fn wideband_profile_with_rigctld_on_hf_is_a_startup_error() {
            let rig = fake_rigctld("14074000");
            let err = startup_error(rigctld_config(&rig));
            assert!(err.contains("14074000"), "{err}");
            assert!(err.contains("97.307"), "{err}");
        }

        #[test]
        fn narrowband_default_config_still_starts() {
            let event_loop = EventLoop::new(DaemonConfig::default()).unwrap();
            assert!(event_loop.engine.occupied_bandwidth_hz() <= 2_800);
        }

        #[test]
        fn narrowband_keeps_the_soft_rigctld_fallback() {
            let mut config = DaemonConfig::default();
            config.radio.ptt_method = "rigctld".into();
            config.radio.rigctld_address = "127.0.0.1:1".into();
            let event_loop = EventLoop::new(config).unwrap();
            assert!(!event_loop.ptt.is_rigctld());
            // COP-32: the fallback is reported, so coppad does not say ready.
            assert_eq!(
                event_loop.startup_problems(),
                ["no PTT: rigctld at 127.0.0.1:1 is unreachable"]
            );
        }

        #[test]
        fn a_clean_startup_reports_no_problems() {
            assert!(EventLoop::new(DaemonConfig::default())
                .unwrap()
                .startup_problems()
                .is_empty());
            let rig = fake_rigctld("144500000");
            assert!(EventLoop::new(rigctld_config(&rig))
                .unwrap()
                .startup_problems()
                .is_empty());
        }

        // ── Phase 4: per-transmission gate ────────────────────────────────

        const SAMPLES: [f32; 64] = [0.0; 64];

        #[tokio::test]
        async fn wideband_tx_with_answering_vhf_rigctld_transmits() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 1, "audio was queued");
            assert!(event_loop.is_transmitting);
            assert_eq!(
                PttControl::get_ptt(&mut event_loop.ptt).unwrap(),
                PttState::Tx
            );
        }

        #[tokio::test]
        async fn wideband_tx_refused_after_retune_below_29m7() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 1);
            event_loop.handle_ptt_change(false).await;
            let submitted = event_loop.tx_output().submitted();

            *rig.answer.lock().unwrap() = "14074000".into();
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 1, "no new transmission");
            assert_eq!(
                event_loop.tx_output().submitted(),
                submitted,
                "no new audio"
            );
            assert!(!event_loop.is_transmitting, "PTT stays unkeyed");
        }

        #[tokio::test]
        async fn wideband_tx_refused_when_rigctld_dies_mid_session() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            rig.alive.store(false, Ordering::SeqCst);
            assert_eq!(
                event_loop.wideband_frequency_check(),
                Err(GateRefusal::NoSource)
            );
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 0);
            assert!(!event_loop.is_transmitting);
        }

        #[tokio::test(start_paused = true)]
        async fn wideband_tx_refused_after_retune_during_busy_wait() {
            let rig = fake_rigctld("144500000");
            let mut config = rigctld_config(&rig);
            config.audio.sample_rate = 48_000;
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(32_768);
            let (audio_out_tx, audio_out_rx) = coppa_audio::audio_ring(32_768);
            event_loop.set_audio_in(audio_in_rx);
            event_loop.set_audio_out(audio_out_tx);
            let mut counter = 1u32;
            let mut noise = |amplitude: f32| -> Vec<f32> {
                (0..1024)
                    .map(|_| {
                        counter = counter.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        amplitude * ((counter >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
                    })
                    .collect()
            };
            for _ in 0..10 {
                event_loop.busy_gate.observe(&noise(0.01));
            }
            for _ in 0..5 {
                event_loop.busy_gate.observe(&noise(0.5));
            }
            assert!(event_loop.busy_gate.current());
            event_loop.tx_on_air_bytes = 5;
            {
                let tx = event_loop.transmit_samples(&SAMPLES);
                tokio::pin!(tx);
                tokio::select! {
                    biased;
                    _ = &mut tx => panic!("transmission must wait for the busy channel"),
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
                // The initial frequency check passed. Retune while held,
                // then feed quiet audio to release the courtesy wait.
                *rig.answer.lock().unwrap() = "14074000".into();
                audio_in_tx.write(&noise(0.01));
                tokio::time::timeout(Duration::from_secs(5), tx)
                    .await
                    .unwrap();
            }
            assert!(!event_loop.busy_gate.current());
            assert_eq!(
                event_loop.tx_generation, 0,
                "no transmission after the retune"
            );
            assert_eq!(audio_out_rx.available(), 0, "no audio queued");
            assert_eq!(event_loop.tx_on_air_bytes, 0, "refused bytes leave BUFFER");
            assert!(!event_loop.is_transmitting);
            assert_eq!(
                PttControl::get_ptt(&mut event_loop.ptt).unwrap(),
                PttState::Rx
            );
        }

        #[tokio::test]
        async fn a_stated_vhf_source_backs_a_rigctld_that_dies() {
            let rig = fake_rigctld("144500000");
            let mut config = rigctld_config(&rig);
            config.radio.band = Some("2m".into());
            let mut event_loop = EventLoop::new(config).unwrap();
            rig.alive.store(false, Ordering::SeqCst);
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 1, "stated band is the fallback");
        }

        #[tokio::test]
        async fn a_live_hf_answer_overrides_a_stated_vhf_band() {
            let rig = fake_rigctld("144500000");
            let mut config = rigctld_config(&rig);
            config.radio.band = Some("2m".into());
            let mut event_loop = EventLoop::new(config).unwrap();
            *rig.answer.lock().unwrap() = "14074000".into();
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 0, "the live answer wins");
        }

        #[tokio::test]
        async fn narrowband_tx_with_no_source_still_transmits() {
            let mut config = wideband_config();
            config.engine.profile = "HF_STANDARD".into();
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 1);
            assert!(event_loop.is_transmitting);
        }

        #[tokio::test]
        async fn refused_wideband_tx_does_not_key_ptt() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            *rig.answer.lock().unwrap() = "7074000".into();
            event_loop.tx_on_air_bytes = 5;
            event_loop.transmit_samples(&SAMPLES).await;
            assert!(!event_loop.is_transmitting);
            assert_eq!(event_loop.tx_on_air_bytes, 0, "refused bytes leave BUFFER");
            *rig.answer.lock().unwrap() = "144500000".into();
            assert_eq!(
                PttControl::get_ptt(&mut event_loop.ptt).unwrap(),
                PttState::Rx,
                "the rig was never keyed"
            );
        }

        /// COP-65: a transmission the gate refuses never reaches the air, so
        /// it reports no TX level; the same call allowed reports exactly one.
        #[cfg(feature = "websocket")]
        #[tokio::test]
        async fn refused_wideband_transmission_reports_no_tx_audio_level() {
            use coppa_host::websocket::AudioLevelDirection;

            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<String>(64);
            event_loop.set_ws_broadcast(ws_tx);

            *rig.answer.lock().unwrap() = "7074000".into();
            event_loop.transmit_samples(&SAMPLES).await;
            assert!(!event_loop.is_transmitting);
            assert!(
                drain_audio_levels(&mut ws_rx, Some(AudioLevelDirection::Tx)).is_empty(),
                "a refused transmission reports no TX level"
            );
            *rig.answer.lock().unwrap() = "144500000".into();
            assert_eq!(
                PttControl::get_ptt(&mut event_loop.ptt).unwrap(),
                PttState::Rx,
                "the rig was never keyed"
            );

            // Control: back on 2m, the same call transmits and reports once.
            event_loop.transmit_samples(&SAMPLES).await;
            assert!(event_loop.is_transmitting);
            assert_eq!(
                drain_audio_levels(&mut ws_rx, Some(AudioLevelDirection::Tx)).len(),
                1
            );
        }

        /// A queued payload is held, not dropped, while the gate refuses,
        /// and goes out on the next poll once the rig is back on 2m.
        #[tokio::test]
        async fn a_held_tx_queue_drains_once_the_rig_is_back_on_vhf() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            *rig.answer.lock().unwrap() = "14074000".into();
            event_loop.enqueue_tx(None, b"hello".to_vec()).await;
            assert_eq!(event_loop.tx_queue.len(), 1, "held, not dropped");
            assert!(event_loop.tx_held_by_frequency_gate);
            assert_eq!(event_loop.tx_generation, 0);

            event_loop.retry_held_tx_drain().await;
            assert_eq!(event_loop.tx_queue.len(), 1, "still on HF: still held");

            *rig.answer.lock().unwrap() = "144500000".into();
            event_loop.retry_held_tx_drain().await;
            assert!(event_loop.tx_queue.is_empty());
            assert!(!event_loop.tx_held_by_frequency_gate);
            assert_eq!(event_loop.tx_generation, 1, "the held payload went out");
        }

        /// The post-wait refusal must re-arm the held-queue retry: the drain
        /// that popped the refused payload had cleared the flag after its own
        /// check passed, and a refused transmission creates no TxComplete or
        /// PTT-release event that would ever re-trigger a drain. Without the
        /// re-arm, every payload still queued behind the refused one stalls
        /// until an unrelated host enqueue.
        #[tokio::test(start_paused = true)]
        async fn a_post_wait_refusal_leaves_the_remaining_queue_retryable() {
            let rig = fake_rigctld("144500000");
            let mut config = rigctld_config(&rig);
            config.audio.sample_rate = 48_000;
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(32_768);
            let (audio_out_tx, _audio_out_rx) = coppa_audio::audio_ring(32_768);
            event_loop.set_audio_in(audio_in_rx);
            event_loop.set_audio_out(audio_out_tx);
            let mut counter = 1u32;
            let mut noise = |amplitude: f32| -> Vec<f32> {
                (0..1024)
                    .map(|_| {
                        counter = counter.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        amplitude * ((counter >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
                    })
                    .collect()
            };
            for _ in 0..10 {
                event_loop.busy_gate.observe(&noise(0.01));
            }
            for _ in 0..5 {
                event_loop.busy_gate.observe(&noise(0.5));
            }
            assert!(event_loop.busy_gate.current());
            // Two queued non-ARQ payloads: the drain passes its gate check
            // on 2m, pops the first, and blocks in the courtesy wait.
            event_loop.tx_queue.push_back((None, b"first".to_vec()));
            event_loop.tx_queue.push_back((None, b"second".to_vec()));
            {
                let drain = event_loop.try_drain_tx_queue();
                tokio::pin!(drain);
                tokio::select! {
                    biased;
                    _ = &mut drain => panic!("drain must wait for the busy channel"),
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
                // The rig retunes to HF during the wait, then the channel
                // goes quiet and the courtesy wait releases into a refusal.
                *rig.answer.lock().unwrap() = "14074000".into();
                audio_in_tx.write(&noise(0.01));
                tokio::time::timeout(Duration::from_secs(5), drain)
                    .await
                    .unwrap();
            }
            assert_eq!(
                event_loop.tx_generation, 0,
                "the popped payload was refused"
            );
            assert_eq!(
                event_loop.tx_queue.len(),
                1,
                "the second payload stays queued"
            );
            assert_eq!(event_loop.tx_on_air_bytes, 0, "refused bytes leave BUFFER");
            assert!(
                event_loop.tx_held_by_frequency_gate,
                "the remaining queue is re-armed for the retry poll"
            );

            // Back on 2m, the ordinary retry poll (no new enqueue, no other
            // drain trigger) resumes the remaining payload.
            *rig.answer.lock().unwrap() = "144500000".into();
            event_loop.retry_held_tx_drain().await;
            assert!(event_loop.tx_queue.is_empty());
            assert!(!event_loop.tx_held_by_frequency_gate);
            assert_eq!(
                event_loop.tx_generation, 1,
                "the remaining payload went out"
            );
        }

        /// One established-session chunk carrying `payload`, as the
        /// `DataReceived` session arm queues it.
        fn session_chunk(payload: &[u8]) -> SessionTxChunk {
            let pdu = MacPdu::new_data(
                Callsign::new("W1AW").unwrap(),
                Callsign::new("VK3ABC").unwrap(),
                0,
                payload.to_vec(),
            );
            SessionTxChunk {
                session_id: 0,
                pdu_bytes: pdu.to_bytes(),
                host_len: payload.len(),
            }
        }

        /// COP-41 review: established-session chunks are held, not popped
        /// into a refusal and lost, while the gate refuses between chunks
        /// (session data has no MAC-level retry), and the ordinary retry
        /// poll resumes them once the rig is back on 2m -- no PTT release
        /// or host write is needed to restart the stream.
        #[tokio::test]
        async fn held_session_chunks_drain_once_the_rig_is_back_on_vhf() {
            let rig = fake_rigctld("144500000");
            let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
            for payload in [&b"first"[..], b"second"] {
                event_loop
                    .session_tx_queue
                    .push_back(session_chunk(payload));
            }
            *rig.answer.lock().unwrap() = "14074000".into();
            event_loop.drain_session_tx_queue().await;
            assert_eq!(event_loop.session_tx_queue.len(), 2, "held, not dropped");
            assert!(event_loop.session_tx_held_by_frequency_gate);
            assert_eq!(event_loop.tx_generation, 0);
            assert_eq!(
                event_loop.buffered_tx_bytes(),
                11,
                "BUFFER still counts them"
            );

            event_loop.retry_held_tx_drain().await;
            assert_eq!(
                event_loop.session_tx_queue.len(),
                2,
                "still on HF: still held"
            );

            *rig.answer.lock().unwrap() = "144500000".into();
            event_loop.retry_held_tx_drain().await;
            assert_eq!(event_loop.session_tx_queue.len(), 1);
            assert!(!event_loop.session_tx_held_by_frequency_gate);
            assert_eq!(event_loop.tx_generation, 1, "the first chunk went out");
        }

        /// COP-41 review: a session chunk that passes the pre-pop check but
        /// is refused after the courtesy wait goes back at the head of the
        /// queue rather than leaving a hole in the host's byte stream
        /// (session data has no MAC-level retry), and the retry poll sends
        /// it once the rig is back on 2m.
        #[tokio::test(start_paused = true)]
        async fn a_post_wait_refusal_requeues_the_session_chunk() {
            let rig = fake_rigctld("144500000");
            let mut config = rigctld_config(&rig);
            config.audio.sample_rate = 48_000;
            config.station_id.busy_hold_ms = 10;
            let mut event_loop = EventLoop::new(config).unwrap();
            let (mut audio_in_tx, audio_in_rx) = coppa_audio::audio_ring(32_768);
            let (audio_out_tx, _audio_out_rx) = coppa_audio::audio_ring(32_768);
            event_loop.set_audio_in(audio_in_rx);
            event_loop.set_audio_out(audio_out_tx);
            let mut counter = 1u32;
            let mut noise = |amplitude: f32| -> Vec<f32> {
                (0..1024)
                    .map(|_| {
                        counter = counter.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        amplitude * ((counter >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
                    })
                    .collect()
            };
            for _ in 0..10 {
                event_loop.busy_gate.observe(&noise(0.01));
            }
            for _ in 0..5 {
                event_loop.busy_gate.observe(&noise(0.5));
            }
            assert!(event_loop.busy_gate.current());
            event_loop
                .session_tx_queue
                .push_back(session_chunk(b"first"));
            event_loop
                .session_tx_queue
                .push_back(session_chunk(b"second"));
            {
                let drain = event_loop.drain_session_tx_queue();
                tokio::pin!(drain);
                tokio::select! {
                    biased;
                    _ = &mut drain => panic!("drain must wait for the busy channel"),
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
                *rig.answer.lock().unwrap() = "14074000".into();
                audio_in_tx.write(&noise(0.01));
                tokio::time::timeout(Duration::from_secs(5), drain)
                    .await
                    .unwrap();
            }
            assert_eq!(event_loop.tx_generation, 0, "the popped chunk was refused");
            assert_eq!(
                event_loop.session_tx_queue.len(),
                2,
                "the refused chunk is back in the queue"
            );
            assert_eq!(
                event_loop.session_tx_queue.front().unwrap().host_len,
                5,
                "at the head, ahead of the chunk behind it"
            );
            assert_eq!(event_loop.buffered_tx_bytes(), 11, "BUFFER still counts it");
            assert!(event_loop.session_tx_held_by_frequency_gate);

            *rig.answer.lock().unwrap() = "144500000".into();
            event_loop.retry_held_tx_drain().await;
            assert_eq!(event_loop.tx_generation, 1, "the refused chunk went out");
            assert_eq!(event_loop.session_tx_queue.len(), 1);
            assert_eq!(event_loop.session_tx_queue.front().unwrap().host_len, 6);
        }

        /// A `LevelDerived` engine starts narrowband (no startup check
        /// applies) but is gated the moment it climbs into `vhf_wide`.
        #[tokio::test]
        async fn a_level_derived_engine_is_gated_when_it_climbs_to_vhf() {
            let mut config = wideband_config();
            config.engine.profile = "HF_STANDARD".into();
            let mut event_loop = EventLoop::new(config).unwrap();
            event_loop.engine = coppa_engine::CoppaCore::new();
            event_loop.engine.set_speed_level(5).unwrap();
            assert!(event_loop.engine.occupied_bandwidth_hz() > 2_800);
            event_loop.transmit_samples(&SAMPLES).await;
            assert_eq!(event_loop.tx_generation, 0);
        }

        // ── COP-21: a level-derived engine uses HF, not vhf_wide, below 29.7 MHz ──
        mod cop21_hf_legal_profile {
            use super::*;

            /// `EventLoop::new` falls back to `EngineConfig::default()`
            /// (`OfdmBand::LevelDerived`) for any name `get_profile` does not know.
            const UNKNOWN_PROFILE: &str = "HF-STANDARD-TYPO";

            fn level_derived(mut config: DaemonConfig) -> DaemonConfig {
                config.engine.profile = UNKNOWN_PROFILE.into();
                config
            }

            /// Raise `RateLoop` the way peer ACKs do (raise_dwell = 5).
            fn climb_to(event_loop: &mut EventLoop, level: u8) {
                while event_loop.rate_loop.current_level() < level {
                    event_loop.rate_loop.on_ack(9, true);
                }
            }

            /// Given the daemon knows (or cannot show) it is below 29.7 MHz,
            /// when RateLoop selects level 5, then the frame goes out on HF.
            async fn level_5_goes_out_on_hf(config: DaemonConfig) {
                let mut event_loop = EventLoop::new(level_derived(config)).unwrap();
                assert_eq!(
                    event_loop.engine.config().ofdm_band,
                    OfdmBand::LevelDerived,
                    "premise"
                );
                climb_to(&mut event_loop, 5);
                event_loop.enqueue_tx(None, b"hello".to_vec()).await;
                assert_eq!(event_loop.engine.speed_level(), 5);
                assert!(!event_loop.engine.on_vhf_profile(), "not vhf_wide");
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
                assert_eq!(event_loop.engine.config().ofdm_band, OfdmBand::Hf);
                assert_eq!(event_loop.tx_generation, 1, "the frame went on air");
                assert!(event_loop.tx_queue.is_empty());
                assert!(!event_loop.tx_held_by_frequency_gate);
            }

            #[tokio::test]
            async fn level_5_on_a_stated_hf_frequency_uses_the_hf_profile() {
                let mut config = wideband_config();
                config.radio.frequency_hz = Some(14_074_000);
                level_5_goes_out_on_hf(config).await;
            }

            #[tokio::test]
            async fn level_5_with_rigctld_on_hf_uses_the_hf_profile() {
                let rig = fake_rigctld("7074000");
                level_5_goes_out_on_hf(rigctld_config(&rig)).await;
            }

            #[tokio::test]
            async fn level_5_with_a_stated_10m_band_uses_the_hf_profile() {
                let mut config = wideband_config();
                config.radio.band = Some("10m".into()); // lower edge 28.0 MHz
                level_5_goes_out_on_hf(config).await;
            }

            #[tokio::test]
            async fn level_5_with_no_frequency_source_uses_the_hf_profile() {
                level_5_goes_out_on_hf(wideband_config()).await;
            }

            #[tokio::test]
            async fn level_5_on_2m_still_uses_vhf_wide() {
                let mut config = wideband_config();
                config.radio.band = Some("2m".into());
                let mut event_loop = EventLoop::new(level_derived(config)).unwrap();
                climb_to(&mut event_loop, 5);
                event_loop.enqueue_tx(None, b"hello".to_vec()).await;
                assert!(event_loop.engine.on_vhf_profile());
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 5600);
                assert_eq!(event_loop.engine.config().ofdm_band, OfdmBand::LevelDerived);
                assert_eq!(event_loop.tx_generation, 1);
            }

            #[tokio::test]
            async fn apply_speed_level_rebinds_before_the_level_takes_effect() {
                let mut config = wideband_config();
                config.radio.frequency_hz = Some(14_074_000);
                let mut event_loop = EventLoop::new(level_derived(config)).unwrap();
                event_loop.apply_speed_level(5).unwrap();
                assert_eq!(event_loop.engine.speed_level(), 5);
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
                // RateLoop's ladder now tops out at HF's level 8.
                for _ in 0..60 {
                    event_loop.rate_loop.on_ack(9, true);
                }
                assert_eq!(event_loop.rate_loop.current_level(), 8);
            }

            #[tokio::test]
            async fn hf_engines_and_level_derived_engines_below_5_never_query_the_rig() {
                for (profile, climb_to_level) in [("HF_STANDARD", 8), (UNKNOWN_PROFILE, 4)] {
                    let rig = fake_rigctld("7074000");
                    let mut config = rigctld_config(&rig);
                    config.engine.profile = profile.into();
                    let mut event_loop = EventLoop::new(config).unwrap();
                    climb_to(&mut event_loop, climb_to_level);
                    for i in 0..3u8 {
                        event_loop.enqueue_tx(None, vec![i]).await;
                        event_loop.handle_ptt_change(false).await;
                    }
                    event_loop.apply_speed_level(climb_to_level).unwrap();
                    assert_eq!(rig.f_queries.load(Ordering::SeqCst), 0, "{profile}");
                }
            }

            /// On 2m at `level`, one frame sent on vhf_wide, then the rig is
            /// retuned to 20m.
            async fn on_vhf_wide_then_retuned(level: u8) -> (FakeRig, EventLoop) {
                let rig = fake_rigctld("144500000");
                let mut event_loop = EventLoop::new(level_derived(rigctld_config(&rig))).unwrap();
                climb_to(&mut event_loop, level);
                event_loop.enqueue_tx(None, b"one".to_vec()).await;
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 5600, "premise");
                assert_eq!(event_loop.tx_generation, 1, "premise: legal on 2m");
                event_loop.handle_ptt_change(false).await;
                *rig.answer.lock().unwrap() = "14074000".into();
                (rig, event_loop)
            }

            #[tokio::test]
            async fn a_retune_below_29m7_sends_the_next_payload_on_hf() {
                let (_rig, mut event_loop) = on_vhf_wide_then_retuned(6).await;
                event_loop.enqueue_tx(None, b"two".to_vec()).await;
                assert_eq!(event_loop.tx_generation, 2, "sent, not held");
                assert!(!event_loop.tx_held_by_frequency_gate);
                assert_eq!(event_loop.engine.speed_level(), 6);
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
            }

            #[tokio::test]
            async fn a_retune_from_level_9_lands_on_hf_level_8() {
                let (_rig, mut event_loop) = on_vhf_wide_then_retuned(9).await;
                event_loop.enqueue_tx(None, b"two".to_vec()).await;
                assert_eq!(event_loop.tx_generation, 2);
                assert_eq!(event_loop.engine.speed_level(), 8);
                assert_eq!(event_loop.rate_loop.current_level(), 8);
                assert_eq!(
                    event_loop.engine.available_speed_levels(),
                    (1..=8).collect::<Vec<_>>()
                );
            }

            /// Samples already encoded on vhf_wide are still refused (D8); the
            /// engine is HF before anything else is encoded.
            #[tokio::test]
            async fn a_refused_pre_encoded_wideband_frame_is_still_refused_but_the_next_goes_out() {
                let (_rig, mut event_loop) = on_vhf_wide_then_retuned(6).await;
                event_loop.transmit_samples(&SAMPLES).await;
                assert_eq!(event_loop.tx_generation, 1, "the vhf_wide frame is refused");
                assert_eq!(event_loop.engine.config().ofdm_band, OfdmBand::Hf);
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
                event_loop.transmit_samples(&SAMPLES).await;
                assert_eq!(event_loop.tx_generation, 2, "the next frame goes out");
            }

            #[tokio::test]
            async fn the_hf_binding_holds_after_the_rig_returns_to_vhf() {
                let (rig, mut event_loop) = on_vhf_wide_then_retuned(6).await;
                event_loop.enqueue_tx(None, b"two".to_vec()).await;
                event_loop.handle_ptt_change(false).await;
                *rig.answer.lock().unwrap() = "144500000".into();
                for _ in 0..60 {
                    event_loop.rate_loop.on_ack(9, true);
                }
                event_loop.enqueue_tx(None, b"three".to_vec()).await;
                assert_eq!(
                    event_loop.engine.config().ofdm_band,
                    OfdmBand::Hf,
                    "one-way (D5)"
                );
                assert_eq!(event_loop.engine.speed_level(), 8);
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 2400);
            }

            /// D2: an explicit VHF_FAST engine is refused, never rebound.
            #[tokio::test]
            async fn a_vhf_fast_engine_is_refused_not_rebound() {
                let rig = fake_rigctld("144500000");
                let mut event_loop = EventLoop::new(rigctld_config(&rig)).unwrap();
                *rig.answer.lock().unwrap() = "14074000".into();
                event_loop.transmit_samples(&SAMPLES).await;
                event_loop.enqueue_tx(None, b"held".to_vec()).await;
                assert_eq!(event_loop.tx_generation, 0);
                assert!(event_loop.tx_held_by_frequency_gate);
                assert_eq!(event_loop.engine.config().ofdm_band, OfdmBand::Vhf);
                assert_eq!(event_loop.engine.occupied_bandwidth_hz(), 5600);
            }
        }
    }

    // ── COP-60: replies wait out the peer's PTT tail before keying ───────

    mod cop60_turnaround {
        use super::*;
        use coppa_engine::profiles::get_profile;

        type Edge = (tokio::time::Instant, PttState);
        type Edges = Arc<std::sync::Mutex<Vec<Edge>>>;

        /// PTT backend recording every edge with its (virtual) time.
        #[derive(Clone, Default)]
        struct PttLog(Edges);

        impl PttControl for PttLog {
            fn set_ptt(&mut self, state: PttState) -> anyhow::Result<()> {
                self.0
                    .lock()
                    .unwrap()
                    .push((tokio::time::Instant::now(), state));
                Ok(())
            }

            fn get_ptt(&mut self) -> anyhow::Result<PttState> {
                Ok(self.0.lock().unwrap().last().map_or(PttState::Rx, |e| e.1))
            }
        }

        impl PttLog {
            fn edges(&self) -> Vec<Edge> {
                self.0.lock().unwrap().clone()
            }

            /// Time of the first edge into `state`, if any.
            fn first(&self, state: PttState) -> Option<tokio::time::Instant> {
                self.edges()
                    .into_iter()
                    .find(|edge| edge.1 == state)
                    .map(|edge| edge.0)
            }

            fn keyed(&self) -> bool {
                self.0
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|edge| edge.1 == PttState::Tx)
            }
        }

        fn install_ptt_log(station: &mut EventLoop) -> PttLog {
            let log = PttLog::default();
            station.ptt = PttBackend::Other(Box::new(log.clone()));
            log
        }

        /// A real ARQ data frame from a peer `CoppaCore`, padded for the
        /// streaming receiver.
        fn peer_data_frame() -> Vec<f32> {
            let peer = CoppaCore::from_profile(get_profile("HF_STANDARD").unwrap());
            let pdu = TransportPdu::new_reliable(5, 0, 0, b"hello coppa".to_vec());
            with_lead_and_trail(&peer.encode_bytes(&pdu.to_bytes()).unwrap())
        }

        /// ARQ on, with `TEST_CALLSIGN` (COP-37) so the station may
        /// transmit at all.
        fn arq_config() -> DaemonConfig {
            let mut config = tx_config();
            config.engine.arq_enabled = true;
            config
        }

        const TURNAROUND: Duration = Duration::from_millis(500);

        #[tokio::test(start_paused = true)]
        async fn ack_keys_no_sooner_than_turnaround_after_the_decoded_frame() {
            let mut station = EventLoop::new(arq_config()).unwrap();
            assert_eq!(station.config.radio.turnaround_ms, 500);
            let ptt = install_ptt_log(&mut station);
            let frame = peer_data_frame();

            let t0 = tokio::time::Instant::now();
            station.decode_and_dispatch_audio(&frame).await;

            let keyed = ptt
                .first(PttState::Tx)
                .expect("the decoded data frame must be ACKed");
            assert!(
                keyed - t0 >= TURNAROUND,
                "ACK keyed {:?} after decode, before the {TURNAROUND:?} turnaround",
                keyed - t0
            );
        }

        #[tokio::test(start_paused = true)]
        async fn connect_ack_also_waits_out_the_turnaround() {
            let mut config = DaemonConfig::default();
            config.engine.callsign = "VK3ABC".into();
            let mut a = EventLoop::new(config.clone()).unwrap();
            config.engine.callsign = "W1AW".into();
            let mut b = EventLoop::new(config).unwrap();
            b.listening = true;
            let (a_producer, mut a_consumer) = coppa_audio::audio_ring(1_000_000);
            let (b_producer, _b_consumer) = coppa_audio::audio_ring(1_000_000);
            a.set_audio_out(a_producer);
            b.set_audio_out(b_producer);
            let b_ptt = install_ptt_log(&mut b);

            a.handle_host_event(HostEvent::ConnectRequest {
                client_id: 1,
                source: "VK3ABC".into(),
                destination: "W1AW".into(),
            })
            .await;
            let connect_req = take_leg(&mut a_consumer, "CONNECT_REQ");

            let t0 = tokio::time::Instant::now();
            b.decode_and_dispatch_audio(&connect_req).await;

            let keyed = b_ptt
                .first(PttState::Tx)
                .expect("b must answer the CONNECT_REQ with a CONNECT_ACK");
            assert!(
                keyed - t0 >= TURNAROUND,
                "CONNECT_ACK keyed {:?} after decode, before the {TURNAROUND:?} turnaround",
                keyed - t0
            );
        }

        #[tokio::test(start_paused = true)]
        async fn transmit_with_no_recent_decode_keys_at_once() {
            // (a) No frame ever decoded: no hold.
            let mut idle = EventLoop::new(tx_config()).unwrap();
            let idle_ptt = install_ptt_log(&mut idle);
            let t0 = tokio::time::Instant::now();
            idle.transmit_samples(&[0.1; 4800]).await;
            assert_eq!(idle_ptt.first(PttState::Tx), Some(t0));

            // (b) The last decode is older than the turnaround: no hold. ARQ is
            // off, so the decode itself transmits nothing.
            let mut station = EventLoop::new(tx_config()).unwrap();
            let ptt = install_ptt_log(&mut station);
            station.decode_and_dispatch_audio(&peer_data_frame()).await;
            assert!(
                station.rx_turnaround_until.is_some(),
                "the peer frame must decode, or this case proves nothing"
            );
            assert_eq!(ptt.first(PttState::Tx), None, "ARQ off: no reply");
            tokio::time::advance(Duration::from_millis(600)).await;
            let t1 = tokio::time::Instant::now();
            station.transmit_samples(&[0.1; 4800]).await;
            assert_eq!(ptt.first(PttState::Tx), Some(t1));
        }

        #[tokio::test(start_paused = true)]
        async fn zero_turnaround_keys_at_once() {
            let mut config = arq_config();
            config.radio.turnaround_ms = 0;
            let mut station = EventLoop::new(config).unwrap();
            let ptt = install_ptt_log(&mut station);
            let frame = peer_data_frame();

            let t0 = tokio::time::Instant::now();
            station.decode_and_dispatch_audio(&frame).await;

            assert_eq!(
                ptt.first(PttState::Tx),
                Some(t0),
                "turnaround_ms = 0 disables the guard"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn already_keyed_transmission_is_not_held() {
            let mut station = EventLoop::new(arq_config()).unwrap();
            let ptt = install_ptt_log(&mut station);
            // Mid-transmission: a job queued behind the active one makes no
            // key-up edge, so there is nothing to protect.
            station.is_transmitting = true;
            let frame = peer_data_frame();

            let t0 = tokio::time::Instant::now();
            station.decode_and_dispatch_audio(&frame).await;
            let elapsed = t0.elapsed();

            assert_eq!(station.tx_generation, 1, "the ACK must still be sent");
            assert!(ptt.first(PttState::Tx).is_some());
            let pre_delay = Duration::from_millis(station.config.radio.ptt_pre_delay_ms);
            assert!(
                elapsed <= pre_delay,
                "an already-keyed station waited {elapsed:?}, more than its {pre_delay:?} pre-delay"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn turnaround_wait_keeps_draining_the_input_ring() {
            const CHUNK: usize = 960; // 20 ms at 48 kHz
            const CHUNKS: usize = 27; // 540 ms, more than 3x the ring
            let mut config = arq_config();
            config.radio.ptt_pre_delay_ms = 0;
            let ring = config.audio.buffer_size;
            assert_eq!(ring, 8192, "the production input ring size");
            let mut station = EventLoop::new(config).unwrap();
            install_ptt_log(&mut station);
            let (mut producer, consumer) = coppa_audio::audio_ring(ring);
            station.set_audio_in(consumer);
            let frame = peer_data_frame();

            let writer = async {
                for _ in 0..CHUNKS {
                    producer.write(&[0.0; CHUNK]);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            tokio::join!(station.decode_and_dispatch_audio(&frame), writer);

            assert_eq!(station.tx_generation, 1, "the ACK must have been sent");
            let audio_in = station.audio_in.as_ref().unwrap();
            assert_eq!(
                audio_in.overflow_count(),
                0,
                "the input ring overflowed while the ACK was held"
            );
            assert_eq!(
                station.pending_busy_wait_audio.len() + audio_in.available(),
                CHUNKS * CHUNK,
                "every written sample is held for decode or still in the ring"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn audio_parked_during_a_hold_is_decoded_before_newer_ring_audio() {
            const CHUNK: usize = 960;
            const CHUNKS: usize = 30; // 600 ms
            let mut station = EventLoop::new(arq_config()).unwrap();
            install_ptt_log(&mut station);
            let (mut producer, consumer) = coppa_audio::audio_ring(8192);
            station.set_audio_in(consumer);
            // Audio parked by an earlier hold, holding a frame whose ACK
            // holds TX again (and parks more audio) while it is decoded.
            station.pending_busy_wait_audio = peer_data_frame();

            let writer = async {
                for _ in 0..CHUNKS {
                    producer.write(&[0.0; CHUNK]);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            tokio::join!(station.poll_audio_input(), writer);

            assert_eq!(station.tx_generation, 1, "the parked frame must be ACKed");
            assert!(!station.pending_busy_wait_audio.is_empty());
            // Ring audio written during the ACK's pre-delay is newer than the
            // audio the hold parked, so this poll must not have consumed it.
            assert_eq!(
                station.pending_busy_wait_audio.len()
                    + station.audio_in.as_ref().unwrap().available(),
                CHUNKS * CHUNK,
                "newer ring audio was decoded ahead of older parked audio"
            );
        }

        // ── Simulated H-090 stage (d): a half-duplex cable loop ──────────

        /// One cable tick: 960 samples at 48 kHz, `run()`'s `audio_poll`
        /// cadence.
        const CABLE_TICK: Duration = Duration::from_millis(20);
        /// The ACK must key at least this long after the peer's PTT-off.
        const MIN_GUARD_MARGIN: Duration = Duration::from_millis(50);
        /// Virtual time costs no decode CPU, so one cable tick plus one poll
        /// covers the simulator's processing latency.
        const SIM_CEILING_SLACK: Duration = Duration::from_millis(100);
        /// Virtual time each cable run lasts.
        const CABLE_RUN: Duration = Duration::from_secs(5);

        struct CableRun {
            a_edges: Vec<Edge>,
            b_edges: Vec<Edge>,
            /// Tick at which the last chunk of `a`'s TX audio was delivered
            /// to `b`.
            a_last_sample_at_b: Option<tokio::time::Instant>,
            a_unacked_bytes: usize,
            a_in_overflow: u64,
            b_in_overflow: u64,
        }

        /// Station `a` sends one ARQ data frame to `b` over the real host path
        /// (`HostEvent::DataReceived` with `[host] sessionless_tx_enabled`),
        /// and both real `run()` loops plus the cable are joined in one task.
        ///
        /// The cable models half-duplex RF: each tick it moves up to 960
        /// samples each way, zero-padded (a sound card delivers silence, not
        /// nothing), and zeroes the samples bound for a station whose PTT is
        /// keyed, since a keyed rig hears nothing. Input rings are
        /// `config.audio.buffer_size` (8,192), as `main.rs` sizes them, so a
        /// hold that stops draining them overflows here too. After
        /// `CABLE_RUN` of virtual time both loops get `Shutdown`.
        ///
        /// With no output worker wired, `transmit_samples`'s headless release
        /// path holds the sender keyed for its last sample plus a
        /// `buffer_size` drain (~170 ms) plus `ptt_tail_delay_ms`, so the
        /// simulated peer releases PTT later than real hardware does (presented
        /// last sample plus tail). The simulator's minimum guard is therefore
        /// the stricter one. `docs/testing/cable-loop-turnaround.md` holds the
        /// hardware counterpart COP-20 runs.
        async fn run_cable_loop(config: DaemonConfig) -> CableRun {
            let mut a = EventLoop::new(config.clone()).unwrap();
            let mut b = EventLoop::new(config.clone()).unwrap();
            // ARQ timers run on real `std::time::Instant`s with a 5 s initial
            // RTO; a slow runner must not add a retransmit key-up mid-test.
            a.arq_tx = Some(ArqTx::new(ArqConfig {
                initial_rto: Duration::from_secs(60),
                ..ArqConfig::default()
            }));
            let a_ptt = install_ptt_log(&mut a);
            let b_ptt = install_ptt_log(&mut b);
            let (a_out_producer, mut a_out) = coppa_audio::audio_ring(1_000_000);
            let (b_out_producer, mut b_out) = coppa_audio::audio_ring(1_000_000);
            a.set_audio_out(a_out_producer);
            b.set_audio_out(b_out_producer);
            let ring = config.audio.buffer_size;
            let (mut a_in, a_in_consumer) = coppa_audio::audio_ring(ring);
            let (mut b_in, b_in_consumer) = coppa_audio::audio_ring(ring);
            a.set_audio_in(a_in_consumer);
            b.set_audio_in(b_in_consumer);
            let (a_events, b_events) = (a.event_sender(), b.event_sender());
            a_events
                .send(DaemonEvent::Host(HostEvent::DataReceived {
                    client_id: 1,
                    data: b"hello coppa".to_vec(),
                }))
                .await
                .unwrap();

            let mut a_last_sample_at_b = None;
            let cable = async {
                let start = tokio::time::Instant::now();
                let mut tick = tokio::time::interval(CABLE_TICK);
                let mut chunk = vec![0.0f32; 960];
                while start.elapsed() < CABLE_RUN {
                    tick.tick().await;
                    chunk.fill(0.0);
                    if a_out.read(&mut chunk) > 0 {
                        a_last_sample_at_b = Some(tokio::time::Instant::now());
                    }
                    if b_ptt.keyed() {
                        chunk.fill(0.0);
                    }
                    b_in.write(&chunk);

                    chunk.fill(0.0);
                    b_out.read(&mut chunk);
                    if a_ptt.keyed() {
                        chunk.fill(0.0);
                    }
                    a_in.write(&chunk);
                }
                a_events.send(DaemonEvent::Shutdown).await.unwrap();
                b_events.send(DaemonEvent::Shutdown).await.unwrap();
            };
            let (a_result, b_result, ()) = tokio::join!(a.run(), b.run(), cable);
            a_result.unwrap();
            b_result.unwrap();

            CableRun {
                a_edges: a_ptt.edges(),
                b_edges: b_ptt.edges(),
                a_last_sample_at_b,
                a_unacked_bytes: a.arq_tx.as_ref().unwrap().unacked_bytes(),
                a_in_overflow: a.audio_in.as_ref().unwrap().overflow_count(),
                b_in_overflow: b.audio_in.as_ref().unwrap().overflow_count(),
            }
        }

        struct TurnaroundCheck {
            /// `b`'s first PTT-on minus `a`'s first PTT-off, required
            /// `>= MIN_GUARD_MARGIN`.
            min_guard: Result<Duration, String>,
            /// `b`'s first PTT-on minus `a_last_sample_at_b`, required
            /// `<= turnaround + SIM_CEILING_SLACK`.
            ceiling: Result<Duration, String>,
        }

        fn check_turnaround(run: &CableRun, turnaround: Duration) -> TurnaroundCheck {
            let first = |edges: &[Edge], state: PttState, after: Option<tokio::time::Instant>| {
                edges
                    .iter()
                    .find(|edge| edge.1 == state && after.is_none_or(|t| edge.0 >= t))
                    .map(|edge| edge.0)
            };
            let a_on = first(&run.a_edges, PttState::Tx, None);
            let a_off = a_on.and_then(|on| first(&run.a_edges, PttState::Rx, Some(on)));
            let b_on = first(&run.b_edges, PttState::Tx, None);

            let min_guard = match (a_off, b_on) {
                (None, _) => Err("a never released PTT".to_string()),
                (_, None) => Err("b never keyed an ACK".to_string()),
                (Some(a_off), Some(b_on)) if b_on < a_off => {
                    Err(format!("b keyed {:?} before a's PTT-off", a_off - b_on))
                }
                (Some(a_off), Some(b_on)) if b_on - a_off < MIN_GUARD_MARGIN => Err(format!(
                    "b keyed only {:?} after a's PTT-off, need >= {MIN_GUARD_MARGIN:?}",
                    b_on - a_off
                )),
                (Some(a_off), Some(b_on)) => Ok(b_on - a_off),
            };
            let limit = turnaround + SIM_CEILING_SLACK;
            let ceiling = match (run.a_last_sample_at_b, b_on) {
                (None, _) => Err("a's audio never reached b".to_string()),
                (_, None) => Err("b never keyed an ACK".to_string()),
                (Some(last), Some(b_on)) if b_on < last => Err(format!(
                    "b keyed {:?} before a's last sample arrived",
                    last - b_on
                )),
                (Some(last), Some(b_on)) if b_on - last > limit => Err(format!(
                    "b keyed {:?} after a's last sample, over the {limit:?} ceiling",
                    b_on - last
                )),
                (Some(last), Some(b_on)) => Ok(b_on - last),
            };
            TurnaroundCheck { min_guard, ceiling }
        }

        #[tokio::test(start_paused = true)]
        async fn cable_loop_ack_keys_after_the_peers_real_ptt_off_edge() {
            let mut config = arq_config();
            config.host.sessionless_tx_enabled = true;
            let turnaround = Duration::from_millis(config.radio.turnaround_ms);
            let run = run_cable_loop(config).await;
            let check = check_turnaround(&run, turnaround);

            let margin = check.min_guard.expect("minimum guard");
            let latency = check.ceiling.expect("ceiling");
            assert!(margin >= MIN_GUARD_MARGIN, "{margin:?}");
            assert!(latency <= turnaround + SIM_CEILING_SLACK, "{latency:?}");
            assert_eq!(
                run.a_unacked_bytes, 0,
                "the ACK must reach a through the half-duplex cable"
            );
            assert_eq!(run.a_in_overflow, 0, "a's input ring overflowed");
            assert_eq!(run.b_in_overflow, 0, "b's input ring overflowed");
        }

        #[tokio::test(start_paused = true)]
        async fn cable_loop_zero_turnaround_passes_the_ceiling_but_fails_the_minimum_guard() {
            let mut config = arq_config();
            config.host.sessionless_tx_enabled = true;
            config.radio.turnaround_ms = 0;
            let run = run_cable_loop(config).await;
            let check = check_turnaround(&run, Duration::ZERO);

            assert!(
                check.ceiling.is_ok(),
                "a zero-turnaround station answers fast: {:?}",
                check.ceiling
            );
            let violation = check
                .min_guard
                .expect_err("a zero-turnaround station keys while the peer is still keyed");
            assert!(violation.contains("before a's PTT-off"), "{violation}");
            assert!(
                run.a_unacked_bytes > 0,
                "an ACK sent into a keyed peer is lost"
            );
        }
    }
}
