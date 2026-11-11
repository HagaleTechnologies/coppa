//! A connected, reliable session over audio samples: coppad's session
//! protocol without the daemon (COP-139).
//!
//! A [`Link`] is one station's end of a point-to-point session. It runs the
//! same frames `coppad` puts on air (ADR-010): the three-way CONNECT
//! handshake, stop-and-wait ARQ with retransmission, keepalives and a
//! graceful DISCONNECT. It is sans-IO: the caller feeds captured audio and
//! takes audio to play through [`Link::process`], and supplies the time as a
//! `now: Instant` on every call. The link never reads a clock, so it runs on
//! `wasm32-unknown-unknown` and under virtual time in tests.
//!
//! # Driving a link
//!
//! Call [`Link::process`] once per audio block, as an audio callback would:
//! pass the samples captured since the last call and a buffer to fill with
//! samples to play. The return value is the number of signal samples at the
//! start of that buffer; key the transmitter while it is non-zero. While the
//! link is transmitting it ignores the captured samples, as a half-duplex
//! radio does.
//!
//! ```no_run
//! use coppa_engine::link::{Link, LinkConfig, LinkError, LinkEvent};
//! use std::time::{Duration, Instant};
//!
//! /// Connect A to B over a lockstep virtual cable and send 5 bytes. `base`
//! /// is any instant: the link only ever compares the times it is given.
//! fn exchange(base: Instant) -> Result<Vec<u8>, LinkError> {
//!     let mut a = Link::new(LinkConfig::new("N0AAA"))?;
//!     let mut b = Link::new(LinkConfig::new("N0BBB"))?;
//!     b.set_listening(true);
//!     a.connect("N0BBB", base)?;
//!
//!     // 960 samples (20 ms at 48 kHz) per tick, each way.
//!     let (mut a_out, mut b_out) = (vec![0.0f32; 960], vec![0.0f32; 960]);
//!     for tick in 0..6_000u64 {
//!         let now = base + Duration::from_millis(20 * tick);
//!         let (a_in, b_in) = (b_out.clone(), a_out.clone());
//!         a.process(now, &a_in, &mut a_out);
//!         b.process(now, &b_in, &mut b_out);
//!         while let Some(event) = a.poll_event() {
//!             if matches!(event, LinkEvent::Connected { .. }) {
//!                 a.send(b"hello")?;
//!             }
//!         }
//!         if b.received_len() == 5 {
//!             break;
//!         }
//!     }
//!     let mut buf = [0u8; 16];
//!     let n = b.recv(&mut buf);
//!     Ok(buf[..n].to_vec())
//! }
//! ```
//!
//! # Compatibility
//!
//! Both ends must use the same OFDM band and compression setting: a receiver
//! with compression off cannot parse a compressed frame. The link is a subset
//! of coppad's session; see `docs/LIMITATIONS.md` for what it lacks (fade
//! hold, multi-codeword frames, busy-channel detection).

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use coppa_protocol::arq::{self, ArqConfig, ArqRx, ArqTx};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::session::{self, LinkCapabilities, Session};
use coppa_protocol::transport::{TransportPdu, TransportType, TRANSPORT_HEADER_SIZE};
use coppa_protocol::ProtocolError;

use crate::config::EngineConfig;
use crate::engine::CoppaCore;
use crate::error::EngineError;

#[cfg(test)]
mod tests;

/// coppad's session ARQ window (`SESSION_ARQ_WINDOW`): stop-and-wait.
const SESSION_ARQ_WINDOW: u8 = 1;

/// Default half-duplex turnaround: the wait after the last decoded frame
/// before this station keys up.
const DEFAULT_TURNAROUND: Duration = Duration::from_millis(500);

/// Settings for one [`Link`]. `LinkConfig::new(callsign)` fills coppad's defaults.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Engine (modem) settings. Both ends must agree on the band and on
    /// compression. Default: [`EngineConfig::default()`].
    pub engine: EngineConfig,
    /// This station's callsign, 1-8 of `A-Z 0-9 / -` (validated by
    /// [`Link::new`]).
    pub callsign: String,
    /// Wait after the last decoded frame before keying up. Default 500 ms.
    pub turnaround: Duration,
    /// Retransmissions of one data frame before the link gives up.
    /// Default [`arq::DEFAULT_MAX_RETRANSMIT`] (5).
    pub max_retransmit: u8,
    /// Retransmit timeout before the first round-trip sample, and the
    /// minimum CONNECT_REQ retry interval. Handshake retries also wait for
    /// request/response airtime and two turnarounds. Default
    /// [`arq::DEFAULT_RTO_SECS`] (5 s).
    pub initial_rto: Duration,
    /// CONNECT_REQ retries before the handshake fails.
    /// Default [`session::MAX_CONNECT_RETRIES`] (3).
    pub connect_retries: u8,
    /// Idle time on an established link before a keepalive is sent.
    /// Default [`session::DEFAULT_KEEPALIVE_SECS`] (30 s).
    pub keepalive_interval: Duration,
    /// Idle time on an established link before it times out.
    /// Default [`session::DEFAULT_SESSION_TIMEOUT_SECS`] (120 s).
    pub session_timeout: Duration,
}

impl LinkConfig {
    /// A config for `callsign` with coppad's defaults and
    /// [`EngineConfig::default()`] (level 1, compression off).
    pub fn new(callsign: impl Into<String>) -> Self {
        Self {
            engine: EngineConfig::default(),
            callsign: callsign.into(),
            turnaround: DEFAULT_TURNAROUND,
            max_retransmit: arq::DEFAULT_MAX_RETRANSMIT,
            initial_rto: Duration::from_secs_f64(arq::DEFAULT_RTO_SECS),
            connect_retries: session::MAX_CONNECT_RETRIES,
            keepalive_interval: Duration::from_secs(session::DEFAULT_KEEPALIVE_SECS),
            session_timeout: Duration::from_secs(session::DEFAULT_SESSION_TIMEOUT_SECS),
        }
    }
}

/// Where a [`Link`] is in its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkState {
    /// No session.
    Idle,
    /// A handshake is in progress, as caller or as answerer.
    Connecting,
    /// The session is established; [`Link::send`] is accepted.
    Connected,
    /// [`Link::disconnect`] was called; queued data is still being sent.
    Disconnecting,
}

impl fmt::Display for LinkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LinkState::Idle => "IDLE",
            LinkState::Connecting => "CONNECTING",
            LinkState::Connected => "CONNECTED",
            LinkState::Disconnecting => "DISCONNECTING",
        })
    }
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// This station called [`Link::disconnect`].
    Local,
    /// The peer sent DISCONNECT.
    Remote,
    /// Nothing was heard from the peer for the session or handshake timeout.
    Timeout,
    /// A data frame went unacknowledged after every retransmission.
    RetriesExhausted,
    /// The handshake did not complete (no answer to any CONNECT_REQ, or an
    /// invalid CONNECT_CFM).
    HandshakeFailed,
}

/// Something the caller should know about, from [`Link::poll_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkEvent {
    /// The session with `remote` is established.
    Connected {
        /// The peer's callsign.
        remote: String,
    },
    /// The session with `remote` ended. When this end sends a DISCONNECT
    /// frame, the event comes only after `process` has returned that frame's
    /// last sample, so a caller may stop processing as soon as it sees it.
    Disconnected {
        /// The peer's callsign.
        remote: String,
        /// Why it ended.
        reason: DisconnectReason,
    },
}

/// Errors from [`Link`]'s methods.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LinkError {
    /// A callsign failed validation.
    #[error("invalid callsign \"{callsign}\": {source}")]
    InvalidCallsign {
        /// The rejected text.
        callsign: String,
        /// Why it was rejected.
        source: ProtocolError,
    },
    /// The call needs an idle link.
    #[error("link is not idle (state {state})")]
    NotIdle {
        /// The link's state at the call.
        state: LinkState,
    },
    /// The call needs a connected link.
    #[error("link is not connected (state {state})")]
    NotConnected {
        /// The link's state at the call.
        state: LinkState,
    },
    /// A protocol-layer error.
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    /// An engine error.
    #[error("{0}")]
    Engine(#[from] EngineError),
}

/// The internal session phase; [`LinkState`] is its public view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Initiator: CONNECT_REQ sent (or queued), waiting for CONNECT_ACK.
    Calling,
    /// Responder: CONNECT_ACK sent, waiting for CONNECT_CFM.
    Accepting,
    Connected,
    /// `disconnect` called; draining the backlog before DISCONNECT.
    Draining,
    /// The session is over and its DISCONNECT frame is queued or still
    /// being played; `closing` holds the `Disconnected` event until the
    /// last sample has been returned by `process`.
    Closing,
}

/// What a queued frame is, for the bookkeeping done when it is keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Req,
    Ack,
    Data,
    Control,
}

#[derive(Debug)]
struct QueuedFrame {
    kind: FrameKind,
    bytes: Vec<u8>,
}

/// Reliable-delivery state of an established session.
#[derive(Debug)]
struct Arq {
    tx: ArqTx,
    rx: ArqRx,
}

/// One station's end of a connected, reliable session over audio samples.
/// See the [module docs](self).
#[derive(Debug)]
pub struct Link {
    config: LinkConfig,
    core: CoppaCore,
    local: Callsign,
    listening: bool,
    phase: Phase,
    session: Option<Session>,
    arq: Option<Arq>,
    /// Bytes from `send` not yet cut into a data frame.
    backlog: VecDeque<u8>,
    /// Delivered bytes not yet read by `recv`.
    received: VecDeque<u8>,
    events: VecDeque<LinkEvent>,
    /// The `Disconnected` event held back while in [`Phase::Closing`].
    closing: Option<LinkEvent>,
    /// Frames waiting for the channel, in key order.
    queue: VecDeque<QueuedFrame>,
    /// Samples of the frame being transmitted; `tx_pos` is the next one.
    tx_samples: Vec<f32>,
    tx_pos: usize,
    /// The CONNECT_REQ bytes, kept for retries.
    req_frame: Option<Vec<u8>>,
    /// CONNECT_REQs keyed in this handshake.
    req_attempts: u8,
    /// When the latest CONNECT_REQ was keyed; `None` while one is queued.
    req_keyed_at: Option<Instant>,
    /// Initiator: no data ACK yet, so the CONNECT_CFM may have been lost.
    awaiting_first_ack: bool,
    /// When the in-flight data frame was last keyed.
    data_keyed_at: Option<Instant>,
    /// When the last frame was decoded (any station's).
    last_decode: Option<Instant>,
    frames_tx: u64,
    frames_rx: u64,
}

impl Link {
    /// Build a link for `config`. It starts idle and not listening.
    ///
    /// Returns [`LinkError::InvalidCallsign`] for a callsign longer than 8
    /// characters or outside `A-Z 0-9 / -`, and
    /// [`LinkError::Engine`] for an engine config whose speed level is not
    /// available on its band.
    pub fn new(config: LinkConfig) -> Result<Link, LinkError> {
        let local = parse_callsign(&config.callsign)?;
        let core = CoppaCore::with_config(config.engine.clone());
        // Validate the level against the band the same way `set_speed_level` does.
        let level = config.engine.speed_level;
        core.check_level(
            level,
            CoppaCore::resolves_to_vhf(level, config.engine.ofdm_band),
        )?;
        Ok(Link {
            config,
            core,
            local,
            listening: false,
            phase: Phase::Idle,
            session: None,
            arq: None,
            backlog: VecDeque::new(),
            received: VecDeque::new(),
            events: VecDeque::new(),
            closing: None,
            queue: VecDeque::new(),
            tx_samples: Vec::new(),
            tx_pos: 0,
            req_frame: None,
            req_attempts: 0,
            req_keyed_at: None,
            awaiting_first_ack: false,
            data_keyed_at: None,
            last_decode: None,
            frames_tx: 0,
            frames_rx: 0,
        })
    }

    /// Accept (`true`) or refuse (`false`) incoming CONNECT_REQs while idle.
    pub fn set_listening(&mut self, on: bool) {
        self.listening = on;
    }

    /// Whether the link accepts incoming CONNECT_REQs while idle.
    pub fn is_listening(&self) -> bool {
        self.listening
    }

    /// Start a session with `remote`. The CONNECT_REQ goes out on the next
    /// [`Self::process`] call that may key up. Needs an idle link.
    pub fn connect(&mut self, remote: &str, now: Instant) -> Result<(), LinkError> {
        if self.phase != Phase::Idle {
            return Err(LinkError::NotIdle {
                state: self.state(),
            });
        }
        let remote = parse_callsign(remote)?;
        let mut session = self.new_session(remote, 0, now);
        let req = session.initiate(now)?.to_bytes();
        self.reset_session_state();
        self.session = Some(session);
        self.phase = Phase::Calling;
        self.req_frame = Some(req.clone());
        self.queue.push_back(QueuedFrame {
            kind: FrameKind::Req,
            bytes: req,
        });
        Ok(())
    }

    /// Queue `data` for reliable delivery. Needs a connected link. An empty
    /// `data` is accepted and sends nothing.
    pub fn send(&mut self, data: &[u8]) -> Result<(), LinkError> {
        if self.phase != Phase::Connected {
            return Err(LinkError::NotConnected {
                state: self.state(),
            });
        }
        self.backlog.extend(data.iter().copied());
        Ok(())
    }

    /// End the session. A connected link first sends every queued byte, then
    /// DISCONNECT, then reports [`DisconnectReason::Local`] once
    /// [`Self::process`] has returned the DISCONNECT frame's last sample. A
    /// link still in its handshake stops at once. Calling it while already
    /// disconnecting does nothing. Returns [`LinkError::NotConnected`] on an idle link.
    pub fn disconnect(&mut self, now: Instant) -> Result<(), LinkError> {
        match self.phase {
            Phase::Idle => Err(LinkError::NotConnected {
                state: self.state(),
            }),
            Phase::Calling | Phase::Accepting => {
                self.end_session(DisconnectReason::Local, now, false);
                Ok(())
            }
            Phase::Connected => {
                self.phase = Phase::Draining;
                if let Some(s) = self.session.as_mut() {
                    s.touch(now);
                }
                Ok(())
            }
            Phase::Draining | Phase::Closing => Ok(()),
        }
    }

    /// Run the link for one audio block: decode `rx` (48 kHz mono samples
    /// captured since the last call, any count), run the timers, and write
    /// the samples to play into `tx`.
    ///
    /// The signal is written at the start of `tx` and the rest is zeroed; the
    /// return value is the signal length. Key the transmitter while it is
    /// non-zero. `rx` is ignored while the link has samples left to send.
    /// `now` must not go backwards between calls.
    pub fn process(&mut self, now: Instant, rx: &[f32], tx: &mut [f32]) -> usize {
        if !self.is_transmitting() {
            self.receive(now, rx);
            self.run_timers(now);
            self.key_up(now);
        }
        let pending = &self.tx_samples[self.tx_pos..];
        let n = pending.len().min(tx.len());
        tx[..n].copy_from_slice(&pending[..n]);
        tx[n..].fill(0.0);
        self.tx_pos += n;
        if self.tx_pos >= self.tx_samples.len() {
            self.tx_samples.clear();
            self.tx_pos = 0;
        }
        // Report the end only after the DISCONNECT has fully left `tx`, so a
        // caller that stops on `Disconnected` cannot truncate it.
        if self.phase == Phase::Closing && !self.is_transmitting() && self.queue.is_empty() {
            self.phase = Phase::Idle;
            self.events.extend(self.closing.take());
        }
        n
    }

    /// Copy up to `buf.len()` delivered bytes into `buf`, in order, and
    /// return how many were copied.
    pub fn recv(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.received.len());
        for (slot, byte) in buf.iter_mut().zip(self.received.drain(..n)) {
            *slot = byte;
        }
        n
    }

    /// Delivered bytes waiting for [`Self::recv`].
    pub fn received_len(&self) -> usize {
        self.received.len()
    }

    /// The oldest event not yet returned, if any.
    pub fn poll_event(&mut self) -> Option<LinkEvent> {
        self.events.pop_front()
    }

    /// The link's state.
    pub fn state(&self) -> LinkState {
        match self.phase {
            Phase::Idle => LinkState::Idle,
            Phase::Calling | Phase::Accepting => LinkState::Connecting,
            Phase::Connected => LinkState::Connected,
            Phase::Draining | Phase::Closing => LinkState::Disconnecting,
        }
    }

    /// The peer's callsign while a session exists.
    pub fn remote(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.remote.as_str())
    }

    /// Whether samples of a keyed frame are still waiting to be played.
    pub fn is_transmitting(&self) -> bool {
        self.tx_pos < self.tx_samples.len()
    }

    /// Bytes sent with [`Self::send`] and not yet acknowledged: the backlog
    /// plus the in-flight frame's payload.
    pub fn pending_tx_bytes(&self) -> usize {
        self.backlog.len() + self.arq.as_ref().map_or(0, |a| a.tx.unacked_bytes())
    }

    /// Frames keyed since the link was built.
    pub fn frames_transmitted(&self) -> u64 {
        self.frames_tx
    }

    /// Frames decoded since the link was built, including frames for other
    /// stations.
    pub fn frames_received(&self) -> u64 {
        self.frames_rx
    }

    /// The config the link was built with.
    pub fn config(&self) -> &LinkConfig {
        &self.config
    }

    // ── internals ──────────────────────────────────────────────────

    fn new_session(&self, remote: Callsign, ssid: u8, now: Instant) -> Session {
        let mut s = Session::new(
            0,
            self.local.clone(),
            remote,
            ssid,
            LinkCapabilities::default(),
            now,
        );
        s.keepalive_interval = self.config.keepalive_interval;
        s.session_timeout = self.config.session_timeout;
        s
    }

    fn reset_session_state(&mut self) {
        self.arq = None;
        self.backlog.clear();
        self.queue.clear();
        self.req_frame = None;
        self.req_attempts = 0;
        self.req_keyed_at = None;
        self.awaiting_first_ack = false;
        self.data_keyed_at = None;
    }

    fn start_arq(&mut self) {
        let level = self.core.speed_level();
        let profile = CoppaCore::select_ofdm_profile(
            level,
            self.core.cp_mode(),
            self.core.config().ofdm_band,
        );
        let config = ArqConfig::new(
            SESSION_ARQ_WINDOW,
            self.config.max_retransmit,
            self.config.initial_rto,
        )
        .map(|c| c.with_airtime_params(level, self.config.turnaround, profile));
        // A window of 1 is always valid, so neither step can fail.
        if let Ok(tx) = config.and_then(ArqTx::new) {
            self.arq = Some(Arq {
                tx,
                rx: ArqRx::new(SESSION_ARQ_WINDOW),
            });
        }
    }

    fn remote_string(&self) -> String {
        self.remote().unwrap_or_default().to_owned()
    }

    /// End the session with `reason`. With `send_disconnect`, queue one
    /// best-effort DISCONNECT frame, as coppad's `fail_session_link` does,
    /// and hold the `Disconnected` event in [`Phase::Closing`] until
    /// `process` has returned that frame's last sample.
    fn end_session(&mut self, reason: DisconnectReason, now: Instant, send_disconnect: bool) {
        let remote = self.remote_string();
        let frame = if send_disconnect {
            self.session
                .as_mut()
                .and_then(|s| s.disconnect(now).ok())
                .map(|p| p.to_bytes())
        } else {
            None
        };
        self.reset_session_state();
        self.session = None;
        let event = LinkEvent::Disconnected { remote, reason };
        if let Some(bytes) = frame {
            self.queue.push_back(QueuedFrame {
                kind: FrameKind::Control,
                bytes,
            });
            self.phase = Phase::Closing;
            self.closing = Some(event);
        } else {
            self.phase = Phase::Idle;
            self.events.push_back(event);
        }
    }

    fn established(&mut self) {
        self.phase = Phase::Connected;
        self.start_arq();
        let remote = self.remote_string();
        self.events.push_back(LinkEvent::Connected { remote });
    }

    fn queue_ack(&mut self) {
        let (Some(session), Some(arq)) = (self.session.as_ref(), self.arq.as_ref()) else {
            return;
        };
        let bytes = session_ack_frame(session, &arq.rx);
        // One ACK covers everything received, so a newer one replaces any
        // ACK still waiting for the channel.
        self.queue.retain(|f| f.kind != FrameKind::Ack);
        self.queue.push_back(QueuedFrame {
            kind: FrameKind::Ack,
            bytes,
        });
    }

    fn receive(&mut self, now: Instant, rx: &[f32]) {
        if rx.is_empty() {
            return;
        }
        for frame in self.core.push_samples(rx) {
            let Ok(payload) = frame.payload else {
                continue;
            };
            self.frames_rx += 1;
            self.last_decode = Some(now);
            let Ok(pdu) = MacPdu::from_bytes(&payload) else {
                continue;
            };
            self.handle_pdu(pdu, now);
        }
    }

    fn is_from_peer(&self, pdu: &MacPdu) -> bool {
        self.session.as_ref().is_some_and(|s| {
            s.remote == pdu.src && s.local == pdu.dest && s.ssid & 0x0F == pdu.ssid
        })
    }

    fn handle_pdu(&mut self, pdu: MacPdu, now: Instant) {
        if pdu.dest != self.local
            || matches!(pdu.frame_type, MacFrameType::Beacon | MacFrameType::Relay)
        {
            return;
        }
        let from_peer = self.is_from_peer(&pdu);
        match pdu.frame_type {
            MacFrameType::ConnectReq => self.on_connect_req(&pdu, from_peer, now),
            MacFrameType::ConnectAck if from_peer && self.phase == Phase::Calling => {
                let cfm = self
                    .session
                    .as_mut()
                    .and_then(|s| s.handle_connect_ack(&pdu.payload, now).ok());
                if let Some(cfm) = cfm {
                    self.queue.retain(|f| f.kind != FrameKind::Req);
                    self.req_keyed_at = None;
                    self.queue.push_back(QueuedFrame {
                        kind: FrameKind::Control,
                        bytes: cfm.to_bytes(),
                    });
                    self.established();
                    self.awaiting_first_ack = true;
                }
            }
            MacFrameType::ConnectCfm if from_peer && self.phase == Phase::Accepting => {
                let result = self
                    .session
                    .as_mut()
                    .map(|s| s.handle_connect_cfm(&pdu.payload, now));
                match result {
                    Some(Ok(())) => {
                        if let Some(s) = self.session.as_mut() {
                            s.confirm_established();
                        }
                        self.established();
                    }
                    Some(Err(_)) => {
                        self.end_session(DisconnectReason::HandshakeFailed, now, false);
                        return;
                    }
                    None => {}
                }
            }
            MacFrameType::Data
                if from_peer && matches!(self.phase, Phase::Connected | Phase::Draining) =>
            {
                let segment = TransportPdu::from_bytes(&pdu.payload)
                    .ok()
                    .filter(|t| t.transport_type == TransportType::Reliable);
                if let (Some(segment), Some(arq)) = (segment, self.arq.as_mut()) {
                    for (_seq, data) in arq.rx.receive(segment.seq_num, segment.payload) {
                        self.received.extend(data);
                    }
                    // Built after `receive`, so a duplicate is re-ACKed too.
                    self.queue_ack();
                }
            }
            MacFrameType::Ack
                if from_peer && matches!(self.phase, Phase::Connected | Phase::Draining) =>
            {
                self.on_ack(&pdu, now);
            }
            MacFrameType::Disconnect
                if from_peer && !matches!(self.phase, Phase::Idle | Phase::Closing) =>
            {
                if let Some(s) = self.session.as_mut() {
                    s.handle_disconnect();
                }
                self.end_session(DisconnectReason::Remote, now, false);
                return;
            }
            _ => {}
        }
        if self.is_from_peer(&pdu) {
            if let Some(s) = self.session.as_mut() {
                s.touch(now);
            }
        }
    }

    fn on_connect_req(&mut self, pdu: &MacPdu, from_peer: bool, now: Instant) {
        let repeat = self.phase == Phase::Accepting && from_peer;
        if !(repeat || (self.phase == Phase::Idle && self.listening)) {
            return;
        }
        // A repeat REQ from a caller that missed our ACK restarts the
        // handshake with a fresh ACK (coppad's COP-44 handling).
        let mut session = self.new_session(pdu.src.clone(), pdu.ssid, now);
        let Ok(ack) = session.handle_connect_req(&pdu.payload, now) else {
            return;
        };
        self.reset_session_state();
        self.session = Some(session);
        self.phase = Phase::Accepting;
        self.queue.push_back(QueuedFrame {
            kind: FrameKind::Control,
            bytes: ack.to_bytes(),
        });
    }

    fn on_ack(&mut self, pdu: &MacPdu, now: Instant) {
        let Ok(ack) = TransportPdu::from_bytes(&pdu.payload) else {
            return;
        };
        if ack.transport_type != TransportType::Ack {
            return;
        }
        let Some(arq) = self.arq.as_mut() else {
            return;
        };
        // coppad's in-flight guard: an ACK beyond what is in flight is stale
        // or foreign.
        if ack.ack_num.wrapping_sub(arq.tx.send_base()) > arq.tx.in_flight() {
            return;
        }
        arq.tx.process_ack(ack.ack_num, ack.ack_bitmap, now);
        if arq.tx.in_flight() == 0 {
            self.data_keyed_at = None;
            // Retransmissions queued behind a turnaround hold are now stale;
            // keying them would only waste half-duplex airtime.
            self.queue.retain(|f| f.kind != FrameKind::Data);
        }
        if self.awaiting_first_ack {
            self.awaiting_first_ack = false;
            if let Some(s) = self.session.as_mut() {
                s.confirm_established();
            }
        }
    }

    fn run_timers(&mut self, now: Instant) {
        if self.phase == Phase::Calling {
            if let Some(keyed) = self.req_keyed_at {
                // Both handshake frames use this link's fixed level/profile.
                // As for data ARQ, allow the request, ACK and both direction
                // switches before a retry can interrupt the incoming ACK.
                let level = self.core.speed_level();
                let profile = CoppaCore::select_ofdm_profile(
                    level,
                    self.core.cp_mode(),
                    self.core.config().ofdm_band,
                );
                let retry_interval = coppa_protocol::modem::frame_airtime_s(level, &profile)
                    .map(|airtime| {
                        coppa_protocol::session::connect_req_retry_interval(
                            Duration::from_secs_f64(airtime),
                            self.config.turnaround,
                        )
                    })
                    .unwrap_or_default()
                    .max(self.config.initial_rto);
                if now.saturating_duration_since(keyed) >= retry_interval {
                    if self.req_attempts <= self.config.connect_retries {
                        if let Some(req) = self.req_frame.clone() {
                            self.req_keyed_at = None;
                            self.queue.push_back(QueuedFrame {
                                kind: FrameKind::Req,
                                bytes: req,
                            });
                        }
                    } else {
                        self.end_session(DisconnectReason::HandshakeFailed, now, false);
                        return;
                    }
                }
            }
        }
        if self.session.as_ref().is_some_and(|s| s.is_timed_out(now)) {
            let reason = if matches!(self.phase, Phase::Calling | Phase::Accepting) {
                DisconnectReason::HandshakeFailed
            } else {
                DisconnectReason::Timeout
            };
            self.end_session(reason, now, false);
            return;
        }
        if self.phase == Phase::Connected
            && self.queue.is_empty()
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.needs_keepalive(now))
        {
            if let Some(Ok(pdu)) = self.session.as_mut().map(|s| s.keepalive(now)) {
                self.queue.push_back(QueuedFrame {
                    kind: FrameKind::Control,
                    bytes: pdu.to_bytes(),
                });
            }
        }
        self.run_retransmits(now);
    }

    fn run_retransmits(&mut self, now: Instant) {
        let Some(arq) = self.arq.as_mut() else {
            return;
        };
        // The last allowed transmission went unanswered for a full RTO.
        let base = arq.tx.send_base();
        let failed = arq.tx.in_flight() > 0
            && arq.tx.is_failed(base)
            && self
                .data_keyed_at
                .is_some_and(|t| now.saturating_duration_since(t) >= arq.tx.rto());
        if failed {
            self.end_session(DisconnectReason::RetriesExhausted, now, true);
            return;
        }
        let seqs = arq.tx.get_retransmits(now);
        if !seqs.is_empty() {
            // The window is one frame, so an older queued copy of the same
            // segment that never got keyed is superseded by this one.
            self.queue.retain(|f| f.kind != FrameKind::Data);
        }
        for seq in seqs {
            let Some(arq) = self.arq.as_mut() else {
                return;
            };
            let Some(data) = arq.tx.get_segment_data(seq).map(<[u8]>::to_vec) else {
                continue;
            };
            let _ = arq.tx.mark_retransmitted(seq, now);
            let Some(session) = self.session.as_ref() else {
                return;
            };
            if self.awaiting_first_ack {
                if let Some(cfm) = session.last_cfm.as_ref() {
                    self.queue.push_back(QueuedFrame {
                        kind: FrameKind::Control,
                        bytes: cfm.to_bytes(),
                    });
                }
            }
            let bytes = session_data_frame(session, seq, &data);
            self.queue.push_back(QueuedFrame {
                kind: FrameKind::Data,
                bytes,
            });
        }
    }

    fn key_up(&mut self, now: Instant) {
        if self
            .last_decode
            .is_some_and(|t| now.saturating_duration_since(t) < self.config.turnaround)
        {
            return;
        }
        if let Some(frame) = self.queue.pop_front() {
            self.key(frame, now);
            return;
        }
        let draining = self.phase == Phase::Draining;
        if !(self.phase == Phase::Connected || draining) {
            return;
        }
        let in_flight = self.arq.as_ref().map_or(0, |a| a.tx.in_flight());
        if in_flight > 0 {
            return;
        }
        if !self.backlog.is_empty() {
            self.key_new_chunk(now);
        } else if draining {
            self.end_session(DisconnectReason::Local, now, true);
            if let Some(frame) = self.queue.pop_front() {
                self.key(frame, now);
            }
        }
    }

    fn key_new_chunk(&mut self, now: Instant) {
        let Some(seq) = self.arq.as_ref().map(|a| a.tx.next_seq()) else {
            return;
        };
        let take = self.fit_chunk_len(seq);
        let chunk: Vec<u8> = self.backlog.iter().take(take).copied().collect();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let bytes = session_data_frame(session, seq, &chunk);
        if !self.key(
            QueuedFrame {
                kind: FrameKind::Data,
                bytes,
            },
            now,
        ) {
            return;
        }
        self.backlog.drain(..take);
        if let Some(arq) = self.arq.as_mut() {
            let sent = arq.tx.send(chunk, now);
            debug_assert_eq!(sent.ok(), Some(seq), "ArqTx assigned an unexpected seq");
        }
    }

    /// Encode and start transmitting `frame`. Returns whether it was keyed.
    fn key(&mut self, frame: QueuedFrame, now: Instant) -> bool {
        match self.core.encode_bytes(&frame.bytes) {
            Ok(samples) => {
                self.tx_samples.extend_from_slice(&samples);
                self.frames_tx += 1;
                if let Some(s) = self.session.as_mut() {
                    s.touch(now);
                }
                match frame.kind {
                    FrameKind::Req => {
                        self.req_attempts = self.req_attempts.saturating_add(1);
                        self.req_keyed_at = Some(now);
                    }
                    FrameKind::Data => self.data_keyed_at = Some(now),
                    FrameKind::Ack | FrameKind::Control => {}
                }
                true
            }
            Err(e) => {
                // Every frame is sized by the link itself, so this is a bug.
                debug_assert!(false, "link frame failed to encode: {e}");
                if !matches!(self.phase, Phase::Idle | Phase::Closing) {
                    self.end_session(DisconnectReason::Local, now, false);
                }
                false
            }
        }
    }

    /// The largest prefix of the backlog that fits one codeword at this
    /// speed level, framed with `seq`. A private copy of the single-codeword
    /// algorithm in `coppa_daemon::tx_chunker::next_chunk_len`; unify after
    /// COP-110 (PR #204) lands.
    fn fit_chunk_len(&self, seq: u8) -> usize {
        let budget =
            coppa_protocol::modem::max_payload_for_level(self.core.speed_level()).unwrap_or(0);
        let header = MacPdu::HEADER_SIZE + TRANSPORT_HEADER_SIZE;
        let optimistic = budget.saturating_sub(header + 1).max(1);
        // Only the first `optimistic` bytes can go in one chunk; copying the
        // whole backlog here would make a large send() quadratic.
        let rest: Vec<u8> = self.backlog.iter().take(optimistic).copied().collect();
        let Some(session) = self.session.as_ref() else {
            return 1;
        };
        let fits = |c: &[u8]| {
            self.core
                .encoded_payload_len(&session_data_frame(session, seq, c))
                <= budget
        };
        let mut target = optimistic.min(rest.len());
        let mut rejected = None;
        while target > 1 && !fits(&rest[..target]) {
            rejected = Some(target);
            target /= 2;
        }
        if let Some(mut hi) = rejected {
            let mut lo = target;
            if lo > 1 || fits(&rest[..lo]) {
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if fits(&rest[..mid]) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                target = lo;
            }
        }
        target.max(1)
    }
}

/// Validate a callsign, mapping the error to [`LinkError::InvalidCallsign`].
fn parse_callsign(text: &str) -> Result<Callsign, LinkError> {
    Callsign::new(text).map_err(|source| LinkError::InvalidCallsign {
        callsign: text.to_owned(),
        source,
    })
}

/// One session data frame, as coppad's `session_data_frame`
/// (`crates/coppa-daemon/src/event_loop.rs:240`): the transport ACK fields
/// stay zero so a retransmission is byte-identical, and the transport
/// `session_id` is 0.
fn session_data_frame(session: &Session, seq: u8, data: &[u8]) -> Vec<u8> {
    let segment = TransportPdu::new_reliable(0, seq, 0, data.to_vec());
    MacPdu::new_data(
        session.remote.clone(),
        session.local.clone(),
        session.ssid,
        segment.to_bytes(),
    )
    .to_bytes()
}

/// The session ACK frame for everything `rx` holds, as coppad's
/// `session_ack_frame` (`crates/coppa-daemon/src/event_loop.rs:253`).
fn session_ack_frame(session: &Session, rx: &ArqRx) -> Vec<u8> {
    let (ack_num, bitmap) = rx.ack_info();
    let ack = TransportPdu::new_ack(0, ack_num, bitmap);
    MacPdu::new(
        MacFrameType::Ack,
        session.remote.clone(),
        session.local.clone(),
        session.ssid,
        ack.to_bytes(),
    )
    .to_bytes()
}
