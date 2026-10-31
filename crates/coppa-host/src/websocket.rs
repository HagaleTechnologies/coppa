//! WebSocket JSON API for web-based clients.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::HostEvent;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::http::{header, HeaderValue, StatusCode};

/// JSON message from a WebSocket client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsClientMessage {
    /// Set station callsign.
    #[serde(rename = "mycall")]
    MyCall { callsign: String },
    /// Connect to remote station.
    #[serde(rename = "connect")]
    Connect { source: String, destination: String },
    /// Disconnect.
    #[serde(rename = "disconnect")]
    Disconnect,
    /// Send data.
    #[serde(rename = "send")]
    Send { data: String },
    /// Get status.
    #[serde(rename = "status")]
    Status,
    /// Opt in/out of periodic `spectrum` broadcast messages (Phase 4 Task 4).
    /// See [`WsServerMessage::Spectrum`]'s doc for why this is opt-in rather
    /// than broadcast to every client unconditionally.
    #[serde(rename = "spectrum")]
    Spectrum { enabled: bool },
    /// Opt in/out of periodic `audio_level` broadcast messages (COP-65). See
    /// [`WsServerMessage::AudioLevel`].
    #[serde(rename = "audio_level")]
    AudioLevel { enabled: bool },
}

/// JSON message sent to a WebSocket client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsServerMessage {
    /// Connection status update.
    #[serde(rename = "status")]
    Status {
        connected: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        remote_call: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snr: Option<i32>,
        /// Current speed level (1-9) of the last decoded frame (decision 8).
        #[serde(skip_serializing_if = "Option::is_none")]
        level: Option<u8>,
        /// Carrier frequency offset (Hz) estimated on the last decoded frame
        /// (decision 8).
        #[serde(skip_serializing_if = "Option::is_none")]
        cfo: Option<f32>,
        /// `coppa_ml::CpGate`'s live short-CP recommendation: `Some(true)` =
        /// `CpRecommendation::ShortCp`, `Some(false)` = `CpRecommendation::
        /// LongCp`, `None` = `[engine] cp_gate_enabled` is off or no frame has
        /// decoded yet. Measurement/telemetry only -- does not mean the daemon
        /// has switched (or will switch) CP profile. See `docs/superpowers/
        /// specs/2026-07-25-cpgate-daemon-wiring-design.md`.
        #[serde(skip_serializing_if = "Option::is_none")]
        short_cp_ok: Option<bool>,
        /// COP-2: count of distinct CP-mode desync episodes the daemon has observed
        /// since start (`EventLoop::check_cp_desync`). A non-zero value means this
        /// station has, at least once, been transmitting a waveform its peer could
        /// not decode -- see that method's doc for the two conditions. `0` is the
        /// healthy value and the only one an operator should ever see.
        ///
        /// Always serialized (not `skip_serializing_if`), unlike `short_cp_ok`: an
        /// absent field is indistinguishable from "no episodes" to a client, and the
        /// whole point of surfacing this is that the daemon used to report a healthy
        /// station through every operator-facing channel while the link was dead.
        ///
        /// `#[serde(default)]` on the READ side only: a client built against this
        /// version still always SERIALIZES the field (no `skip_serializing_if`
        /// above), but deserializing an older daemon's reply (or a persisted
        /// pre-COP-2 status message) that lacks it must not fail outright --
        /// default to `0`, the healthy value, rather than erroring the whole
        /// message over one new telemetry field.
        #[serde(default)]
        cp_desync_episodes: u32,
    },
    /// Data received from remote station.
    #[serde(rename = "data")]
    Data { data: String },
    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },
    /// Connection established.
    #[serde(rename = "connected")]
    Connected { remote_call: String },
    /// Disconnected.
    #[serde(rename = "disconnected")]
    Disconnected,
    /// Periodic spectrum/waterfall update (Phase 4 Task 4): a 128-bin
    /// log-magnitude (dB) power spectrum of Coppa's ~300-2800 Hz SSB
    /// passband, produced by the daemon from the RX audio stream at a ~4 Hz
    /// update rate (`crates/coppa-daemon/src/spectrum.rs`).
    ///
    /// Opt-in per client (`WsClientMessage::Spectrum { enabled: true }`),
    /// not broadcast to every connection by default: this is the only
    /// message type on the shared broadcast channel with an inherent
    /// periodic rate (4 Hz) rather than being purely event-driven (a decoded
    /// frame, a connect/disconnect) or client-request/response (`status`) --
    /// unconditionally forwarding it to every connected client, most of
    /// which have no use for a waterfall display, would flood them with
    /// data they didn't ask for. `WebSocketServer::run`'s per-connection
    /// forwarding loop checks each broadcast message's own `type` field and
    /// only delivers a `spectrum` message to connections that opted in.
    #[serde(rename = "spectrum")]
    Spectrum {
        /// 128 log-magnitude (dB) bins spanning the ~300-2800 Hz band,
        /// lowest frequency first.
        bins: Vec<f32>,
        /// Wall-clock time this spectrum was computed (Unix epoch, ms).
        timestamp_ms: u64,
    },
    /// RX/TX audio level (COP-65): `rx` once per 100 ms of received audio (not
    /// while transmitting), `tx` once per transmission. Opt-in per client
    /// (`WsClientMessage::AudioLevel { enabled: true }`), like `spectrum`,
    /// because it is periodic. Named `audio_level`, not `level`, because
    /// `status`'s `level` is the speed level.
    ///
    /// Full scale is sample value 1.0 for every dBFS field. The daemon computes
    /// these in `crates/coppa-daemon/src/level_meter.rs`.
    #[serde(rename = "audio_level")]
    AudioLevel {
        /// `rx`: one 100 ms window of raw received audio. `tx`: one whole
        /// transmission as queued for the sound card, including any prepended
        /// station-ID frame.
        direction: AudioLevelDirection,
        /// `20·log10(max |s|)` over the span, floored at -120.0, rounded to
        /// 0.1 dB.
        peak_dbfs: f32,
        /// `20·log10(sqrt(mean(s²)))` over the span, floored at -120.0, rounded
        /// to 0.1 dB. A full-scale sine reads -3.0 (no AES17 +3 dB correction).
        rms_dbfs: f32,
        /// Fraction (0.0-1.0) of samples in the span with `|s| >= 0.99`.
        /// Non-finite samples count as clipped.
        clip_fraction: f32,
        /// `rx`: the daemon's hysteresis state after this window. `tx`:
        /// `clipping` if any sample of the transmission is clipped, else `ok`
        /// (never `underdrive`).
        state: AudioLevelState,
        /// Audio span this reading covers: 100 for `rx`, the transmission's
        /// length for `tx`.
        duration_ms: u32,
        /// Wall-clock time this message was produced (Unix epoch, ms).
        timestamp_ms: u64,
    },
}

/// Which audio stream an [`WsServerMessage::AudioLevel`] reading measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioLevelDirection {
    /// Received audio (sound-card input).
    Rx,
    /// Transmitted audio (as Coppa queues it for the sound-card output).
    Tx,
}

/// Level state carried by [`WsServerMessage::AudioLevel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioLevelState {
    /// Level is usable.
    Ok,
    /// Samples are reaching full scale.
    Clipping,
    /// RX input is too quiet (RX only).
    Underdrive,
}

/// Maximum number of concurrent WebSocket client connections.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// First WebSocket client ID. VARA command-port IDs count from 1 and data-port
/// IDs from `0x8000_0001`; starting here keeps the three ranges apart, so the
/// daemon's reply to one command-port client can never be addressed by a
/// WebSocket client's ID.
pub const FIRST_CLIENT_ID: u32 = 0x4000_0001;

/// Maximum size (bytes) of a complete WebSocket message we will buffer.
const MAX_WS_MESSAGE_SIZE: usize = 1 << 20; // 1 MiB

/// Maximum size (bytes) of a single WebSocket frame.
const MAX_WS_FRAME_SIZE: usize = 256 * 1024;

/// Live status snapshot the daemon updates as frames decode (and as
/// connect/disconnect events happen), read by [`WebSocketServer::run`] whenever a
/// client sends a `status` request. Phase 3 Task 7 / decision 8: "WebSocket
/// `status` reply carries real values (connected, snr, level, cfo)" — before this,
/// every `status` reply was a hardcoded `connected: false` placeholder.
#[derive(Debug, Clone, Default)]
pub struct WsStatus {
    pub connected: bool,
    pub remote_call: Option<String>,
    pub snr: Option<i32>,
    pub level: Option<u8>,
    pub cfo: Option<f32>,
    /// See `WsServerMessage::Status::short_cp_ok`'s doc.
    pub short_cp_ok: Option<bool>,
    /// See `WsServerMessage::Status::cp_desync_episodes`'s doc.
    pub cp_desync_episodes: u32,
}

/// Periodic broadcast streams a connection must opt into before
/// [`WebSocketServer::run`]'s forwarding loop delivers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptInStream {
    /// `WsServerMessage::Spectrum`.
    Spectrum,
    /// `WsServerMessage::AudioLevel`.
    AudioLevel,
}

/// Which opt-in stream, if any, a pre-serialized broadcast JSON string
/// belongs to, used by each connection's forwarding loop in
/// [`WebSocketServer::run`] to gate delivery on that connection's own opt-in
/// state. `None` means the message is delivered unconditionally.
///
/// The shared broadcast channel (`WebSocketServer::broadcast_tx`) carries
/// pre-serialized JSON `String`s, not a typed enum -- by the time a message
/// reaches here it's already text. A full `serde_json::Value` parse (rather
/// than a cheaper raw prefix/substring check on the JSON text) is the
/// deliberate choice here: correctness over cleverness. `#[serde(tag =
/// "type")]`'s internally-tagged encoding does, in practice, always place
/// `"type"` first in the emitted object, so a `starts_with(r#"{"type":
/// "spectrum""#)` check would likely work too and avoid a parse -- but that's
/// an implementation detail of serde's internal tagging, not a stability
/// guarantee this code should quietly depend on. This runs once per
/// broadcast message per connected client, at a message rate (status/data
/// events, plus at most a 4 Hz spectrum stream and a 10 Hz audio-level
/// stream, across up to `MAX_CONCURRENT_CONNECTIONS` clients) far too low for
/// the parse cost to matter.
fn opt_in_stream(json: &str) -> Option<OptInStream> {
    let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
    match value.get("type")?.as_str()? {
        "spectrum" => Some(OptInStream::Spectrum),
        "audio_level" => Some(OptInStream::AudioLevel),
        _ => None,
    }
}

/// COP-53: who may complete a WebSocket handshake. Every handshake must
/// present `token` (as `Authorization: Bearer <token>` or a `token` query
/// parameter; every value presented must be the token), and a handshake carrying an `Origin` header -- every browser
/// sends one -- must name one of `allowed_origins` exactly (ASCII case
/// ignored). Checked during the HTTP upgrade, so a refused client never
/// becomes a WebSocket. An empty token matches nothing.
#[derive(Clone)]
pub struct WsAuth {
    token: String,
    allowed_origins: Vec<String>,
}

impl WsAuth {
    /// Require `token` on every handshake and allow browser pages served from
    /// `allowed_origins` (exact `scheme://host[:port]` strings).
    pub fn new<I, S>(token: impl Into<String>, allowed_origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            token: token.into(),
            allowed_origins: allowed_origins
                .into_iter()
                .map(|o| o.as_ref().to_ascii_lowercase())
                .collect(),
        }
    }
}

impl std::fmt::Debug for WsAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsAuth")
            .field("token", &"<redacted>")
            .field("allowed_origins", &self.allowed_origins)
            .finish()
    }
}

/// Why a WebSocket handshake was refused (COP-53).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsRejection {
    /// 403: an `Origin` header that is not exactly one allowed origin.
    OriginNotAllowed,
    /// 401: no token, or not the configured one.
    TokenMissingOrInvalid,
}

impl WsRejection {
    /// The HTTP response written in place of `101 Switching Protocols`.
    fn error_response(self) -> ErrorResponse {
        let mut response = ErrorResponse::new(None);
        *response.status_mut() = self.status();
        if self == WsRejection::TokenMissingOrInvalid {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }

    fn status(self) -> StatusCode {
        match self {
            WsRejection::OriginNotAllowed => StatusCode::FORBIDDEN,
            WsRejection::TokenMissingOrInvalid => StatusCode::UNAUTHORIZED,
        }
    }
}

/// COP-53: decide whether a WebSocket upgrade request may proceed. The
/// origin is checked first, so a page from a disallowed origin is refused
/// even if the token leaked to it. `None` (a server built without
/// [`WebSocketServer::with_auth`]) refuses everything.
fn authorize_handshake(auth: Option<&WsAuth>, request: &Request) -> Result<(), WsRejection> {
    let Some(auth) = auth else {
        return Err(WsRejection::TokenMissingOrInvalid);
    };
    let mut origins = request.headers().get_all(header::ORIGIN).iter();
    if let Some(first) = origins.next() {
        let allowed = origins.next().is_none()
            && first.to_str().is_ok_and(|o| {
                !o.eq_ignore_ascii_case("null")
                    && auth
                        .allowed_origins
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(o))
            });
        if !allowed {
            return Err(WsRejection::OriginNotAllowed);
        }
    }
    // Every presented value must match, so one request is one guess: a
    // request stuffed with many `token=` values cannot test them all at once.
    let mut presented = false;
    for value in presented_tokens(request) {
        if !token_eq(value, &auth.token) {
            return Err(WsRejection::TokenMissingOrInvalid);
        }
        presented = true;
    }
    if presented {
        Ok(())
    } else {
        Err(WsRejection::TokenMissingOrInvalid)
    }
}

/// Bearer values from `Authorization` headers (scheme matched ignoring ASCII
/// case), then every `token=` query value. Values are compared as sent: the
/// daemon restricts tokens to RFC 3986 unreserved characters, which need no
/// percent-decoding.
fn presented_tokens(request: &Request) -> impl Iterator<Item = &str> {
    let bearer = request
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .filter_map(|value| {
            let value = value.to_str().ok()?;
            let (scheme, rest) = value.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then(|| rest.trim_matches(' '))
        });
    let query = request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.strip_prefix("token="));
    bearer.chain(query)
}

/// Constant-time over equal lengths; an empty `expected` never matches. A
/// length mismatch returns early, which leaks only the length.
fn token_eq(presented: &str, expected: &str) -> bool {
    if expected.is_empty() || presented.len() != expected.len() {
        return false;
    }
    let diff = presented
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    std::hint::black_box(diff) == 0
}

/// COP-53: the tungstenite handshake callback. Runs [`authorize_handshake`]
/// and, on refusal, records why (with the `Origin` for the log line) and
/// answers with the rejection's status instead of `101`.
struct HandshakeGate<'a> {
    auth: Option<&'a WsAuth>,
    rejection: &'a mut Option<(WsRejection, Option<String>)>,
}

impl Callback for HandshakeGate<'_> {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        match authorize_handshake(self.auth, request) {
            Ok(()) => Ok(response),
            Err(r) => {
                *self.rejection = Some((r, origin_for_log(request)));
                Err(r.error_response())
            }
        }
    }
}

/// The `Origin` header value(s) for a rejection log line. Never reads the
/// `Authorization` header or the request URI, which can carry the token.
fn origin_for_log(request: &Request) -> Option<String> {
    let values: Vec<String> = request
        .headers()
        .get_all(header::ORIGIN)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values.join(", "))
    }
}

/// WebSocket server configuration.
pub struct WebSocketServer {
    port: u16,
    bind_addr: String,
    event_tx: mpsc::Sender<HostEvent>,
    event_rx: Option<mpsc::Receiver<HostEvent>>,
    /// Broadcast channel for pushing messages from the engine to all connected WS clients.
    broadcast_tx: tokio::sync::broadcast::Sender<String>,
    /// Shared live status snapshot; see [`WsStatus`]. The daemon holds the sender
    /// side (via [`Self::status`]) and updates it as frames decode.
    status: std::sync::Arc<tokio::sync::Mutex<WsStatus>>,
    /// COP-53 handshake gate; see [`WsAuth`]. `None` refuses every handshake.
    auth: Option<std::sync::Arc<WsAuth>>,
}

impl WebSocketServer {
    /// Create a new WebSocket server on the given port, bound to loopback (127.0.0.1).
    pub fn new(port: u16) -> Self {
        Self::with_bind_addr(port, "127.0.0.1".to_string())
    }

    /// Create a new WebSocket server binding `port` on the given address.
    pub fn with_bind_addr(port: u16, bind_addr: String) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            port,
            bind_addr,
            event_tx,
            event_rx: Some(event_rx),
            broadcast_tx,
            status: std::sync::Arc::new(tokio::sync::Mutex::new(WsStatus::default())),
            auth: None,
        }
    }

    /// COP-53: require `auth` on every handshake. Without this, `run` and
    /// `run_with_listener` refuse every handshake with 401.
    pub fn with_auth(mut self, auth: WsAuth) -> Self {
        self.auth = Some(std::sync::Arc::new(auth));
        self
    }

    /// Default WebSocket port (8400).
    pub fn default_port() -> Self {
        Self::new(8400)
    }

    /// Get the configured port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Take the event receiver.
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<HostEvent>> {
        self.event_rx.take()
    }

    /// Get a clone of the event sender.
    pub fn event_sender(&self) -> mpsc::Sender<HostEvent> {
        self.event_tx.clone()
    }

    /// Get a broadcast sender for pushing engine events to all connected clients.
    ///
    /// Send JSON-serialized `WsServerMessage`s through this to broadcast to all
    /// connected WebSocket clients (e.g., decoded data, status updates).
    pub fn broadcast_sender(&self) -> tokio::sync::broadcast::Sender<String> {
        self.broadcast_tx.clone()
    }

    /// Get a clone of the shared live status snapshot. The daemon updates this
    /// (e.g. on each decoded frame) so that `run()`'s `status` reply reflects real
    /// values instead of a hardcoded placeholder — see [`WsStatus`].
    pub fn status(&self) -> std::sync::Arc<tokio::sync::Mutex<WsStatus>> {
        self.status.clone()
    }

    /// Run the WebSocket server, accepting connections and piping events.
    ///
    /// Every handshake is gated by the [`WsAuth`] set with [`Self::with_auth`]
    /// (COP-53); without one, every handshake is refused with 401. Each
    /// accepted connection gets a unique client ID. Incoming JSON messages are
    /// parsed as `WsClientMessage` and translated to `HostEvent`s.
    /// Engine-originated messages are broadcast to all connected clients
    /// via the `broadcast_sender()` channel.
    #[cfg(feature = "websocket")]
    pub async fn run(&self) -> Result<(), crate::HostError> {
        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(%addr, "WebSocket server listening");
        self.run_with_listener(listener).await
    }

    /// Run the accept loop on a listener the caller already bound.
    ///
    /// `run` binds `bind_addr:port` and delegates here. A caller that must
    /// see a bind failure before it reports itself ready (coppad) binds
    /// first and passes the listener in. The configured `port` is ignored on
    /// this path. Handshakes are gated as described on [`Self::run`].
    #[cfg(feature = "websocket")]
    pub async fn run_with_listener(
        &self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), crate::HostError> {
        use futures_util::{SinkExt, StreamExt};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use tokio::sync::Semaphore;
        use tokio_tungstenite::accept_hdr_async_with_config;
        use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

        // Bound per-connection memory: cap WebSocket message/frame sizes.
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WS_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_WS_FRAME_SIZE));

        let next_id = Arc::new(AtomicU32::new(FIRST_CLIENT_ID));
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

        loop {
            let (stream, peer) = listener.accept().await?;
            // Cap concurrent connections; drop the client if at capacity.
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::warn!(%peer, "WebSocket connection limit reached, dropping client");
                    drop(stream);
                    continue;
                }
            };
            let next_id = next_id.clone();
            let auth = self.auth.clone();
            let broadcast_tx = self.broadcast_tx.clone();
            let event_tx = self.event_tx.clone();
            let status = self.status.clone();

            tokio::spawn(async move {
                let _permit = permit;
                // COP-53: refuse the upgrade itself, so a rejected client never
                // gets a client ID, a broadcast subscription or a parsed message.
                let mut rejection: Option<(WsRejection, Option<String>)> = None;
                let gate = HandshakeGate {
                    auth: auth.as_deref(),
                    rejection: &mut rejection,
                };
                let ws = match accept_hdr_async_with_config(stream, gate, Some(ws_config)).await {
                    Ok(ws) => ws,
                    Err(e) => {
                        // Never log the token, `Authorization` or the URI.
                        match rejection {
                            Some((r @ WsRejection::OriginNotAllowed, origin)) => tracing::warn!(
                                %peer,
                                status = r.status().as_u16(),
                                origin = ?origin.unwrap_or_default(),
                                "WebSocket handshake rejected: origin is not allowed"
                            ),
                            Some((r @ WsRejection::TokenMissingOrInvalid, _)) => tracing::warn!(
                                %peer,
                                status = r.status().as_u16(),
                                "WebSocket handshake rejected: token missing or invalid"
                            ),
                            None => tracing::warn!(%peer, error = %e, "WebSocket handshake failed"),
                        }
                        return;
                    }
                };

                let client_id = next_id.fetch_add(1, Ordering::Relaxed);
                // Subscribe before announcing the client, so no broadcast sent
                // after `Connected` can be missed.
                let mut broadcast_rx = broadcast_tx.subscribe();
                tracing::info!(client_id, %peer, "WebSocket client connected");

                let _ = event_tx.send(HostEvent::Connected { client_id }).await;

                let (mut sink, mut ws_stream) = ws.split();

                // Per-connection opt-in state for the `spectrum` broadcast stream
                // (Phase 4 Task 4) -- see `WsServerMessage::Spectrum`'s doc for why
                // this defaults to off and must be explicitly requested.
                let mut spectrum_enabled = false;
                // Same, for the `audio_level` stream (COP-65). Independent of
                // `spectrum_enabled`: a client can want one without the other.
                let mut audio_level_enabled = false;

                // Main select loop: handle both client messages and engine broadcasts
                loop {
                    tokio::select! {
                        // Engine broadcast → client
                        result = broadcast_rx.recv() => {
                            match result {
                                Ok(json) => {
                                    // Drop an opt-in stream this client hasn't
                                    // asked for; every other broadcast message
                                    // type is delivered unconditionally, matching
                                    // this channel's pre-existing behavior.
                                    match opt_in_stream(&json) {
                                        Some(OptInStream::Spectrum) if !spectrum_enabled => continue,
                                        Some(OptInStream::AudioLevel) if !audio_level_enabled => continue,
                                        _ => {}
                                    }
                                    if sink.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            }
                        }
                        // Client → engine
                        msg = ws_stream.next() => {
                            let msg = match msg {
                                Some(Ok(m)) => m,
                                _ => break,
                            };

                            if msg.is_close() {
                                break;
                            }

                            if msg.is_text() {
                                let text = msg.to_text().unwrap_or("");
                                match serde_json::from_str::<WsClientMessage>(text) {
                                    Ok(client_msg) => {
                                        let host_event = match client_msg {
                                            WsClientMessage::MyCall { .. } => {
                                                HostEvent::VaraCommand {
                                                    client_id,
                                                    command: text.to_string(),
                                                }
                                            }
                                            WsClientMessage::Connect { source, destination } => {
                                                HostEvent::ConnectRequest {
                                                    client_id,
                                                    source,
                                                    destination,
                                                }
                                            }
                                            WsClientMessage::Disconnect => {
                                                HostEvent::DisconnectRequest { client_id }
                                            }
                                            WsClientMessage::Send { data } => {
                                                HostEvent::DataReceived {
                                                    client_id,
                                                    data: data.into_bytes(),
                                                }
                                            }
                                            WsClientMessage::Status => {
                                                let snapshot = status.lock().await.clone();
                                                let resp = WsServerMessage::Status {
                                                    connected: snapshot.connected,
                                                    remote_call: snapshot.remote_call,
                                                    snr: snapshot.snr,
                                                    level: snapshot.level,
                                                    cfo: snapshot.cfo,
                                                    short_cp_ok: snapshot.short_cp_ok,
                                                    cp_desync_episodes: snapshot.cp_desync_episodes,
                                                };
                                                if let Ok(json) = serde_json::to_string(&resp) {
                                                    let _ = sink
                                                        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            WsClientMessage::Spectrum { enabled } => {
                                                spectrum_enabled = enabled;
                                                continue;
                                            }
                                            WsClientMessage::AudioLevel { enabled } => {
                                                audio_level_enabled = enabled;
                                                continue;
                                            }
                                        };
                                        let _ = event_tx.send(host_event).await;
                                    }
                                    Err(e) => {
                                        let resp = WsServerMessage::Error {
                                            message: format!("Invalid message: {}", e),
                                        };
                                        if let Ok(json) = serde_json::to_string(&resp) {
                                            let _ = sink
                                                .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let _ = event_tx.send(HostEvent::Disconnected { client_id }).await;
                tracing::info!(client_id, "WebSocket client disconnected");
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123";

    fn auth() -> WsAuth {
        WsAuth::new(TOKEN, ["http://localhost:3000"])
    }

    fn request(uri: &str, headers: &[(&str, &str)]) -> Request {
        let mut builder = Request::builder().uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(()).unwrap()
    }

    fn bearer() -> String {
        format!("Bearer {TOKEN}")
    }

    /// The ticket's scenario, at the gate: an untrusted Origin with no token.
    #[test]
    fn handshake_from_untrusted_origin_without_token_is_forbidden() {
        let req = request("/", &[("Origin", "https://evil.example")]);
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::OriginNotAllowed)
        );
    }

    #[test]
    fn handshake_from_untrusted_origin_with_valid_token_is_forbidden() {
        let bearer = bearer();
        let req = request(
            "/",
            &[
                ("Origin", "https://evil.example"),
                ("Authorization", bearer.as_str()),
            ],
        );
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::OriginNotAllowed)
        );
    }

    #[test]
    fn handshake_from_allowed_origin_without_token_is_unauthorized() {
        let req = request("/", &[("Origin", "http://localhost:3000")]);
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::TokenMissingOrInvalid)
        );
    }

    #[test]
    fn handshake_with_wrong_token_is_unauthorized() {
        let req = request(
            "/?token=wrong-token-0000000",
            &[("Origin", "http://localhost:3000")],
        );
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::TokenMissingOrInvalid)
        );
    }

    #[test]
    fn handshake_from_allowed_origin_with_query_token_is_accepted() {
        let uri = format!("/?token={TOKEN}");
        let req = request(&uri, &[("Origin", "http://localhost:3000")]);
        assert_eq!(authorize_handshake(Some(&auth()), &req), Ok(()));
    }

    #[test]
    fn handshake_without_origin_with_bearer_token_is_accepted() {
        let bearer = bearer();
        let req = request("/", &[("Authorization", bearer.as_str())]);
        assert_eq!(authorize_handshake(Some(&auth()), &req), Ok(()));
    }

    #[test]
    fn handshake_without_origin_or_token_is_unauthorized() {
        let req = request("/", &[]);
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::TokenMissingOrInvalid)
        );
    }

    #[test]
    fn handshake_from_null_origin_is_forbidden_even_if_listed() {
        let auth = WsAuth::new(TOKEN, ["null"]);
        let bearer = bearer();
        let req = request(
            "/",
            &[("Origin", "null"), ("Authorization", bearer.as_str())],
        );
        assert_eq!(
            authorize_handshake(Some(&auth), &req),
            Err(WsRejection::OriginNotAllowed)
        );
    }

    #[test]
    fn handshake_origin_match_ignores_ascii_case() {
        let bearer = bearer();
        let req = request(
            "/",
            &[
                ("Origin", "http://LOCALHOST:3000"),
                ("Authorization", bearer.as_str()),
            ],
        );
        assert_eq!(authorize_handshake(Some(&auth()), &req), Ok(()));

        let upper = WsAuth::new(TOKEN, ["HTTP://localhost:3000"]);
        let req = request(
            "/",
            &[
                ("Origin", "http://localhost:3000"),
                ("Authorization", bearer.as_str()),
            ],
        );
        assert_eq!(authorize_handshake(Some(&upper), &req), Ok(()));
    }

    #[test]
    fn handshake_with_two_origin_headers_is_forbidden() {
        let bearer = bearer();
        let req = request(
            "/",
            &[
                ("Origin", "http://localhost:3000"),
                ("Origin", "https://evil.example"),
                ("Authorization", bearer.as_str()),
            ],
        );
        assert_eq!(
            authorize_handshake(Some(&auth()), &req),
            Err(WsRejection::OriginNotAllowed)
        );
    }

    #[test]
    fn handshake_bearer_scheme_is_case_insensitive() {
        let lower = format!("bearer {TOKEN}");
        let req = request("/", &[("Authorization", lower.as_str())]);
        assert_eq!(authorize_handshake(Some(&auth()), &req), Ok(()));
    }

    #[test]
    fn handshake_token_only_counts_in_the_token_query_parameter() {
        let wrong_key = format!("/?xtoken={TOKEN}");
        assert_eq!(
            authorize_handshake(Some(&auth()), &request(&wrong_key, &[])),
            Err(WsRejection::TokenMissingOrInvalid)
        );
        let second = format!("/?a=1&token={TOKEN}");
        assert_eq!(
            authorize_handshake(Some(&auth()), &request(&second, &[])),
            Ok(())
        );
    }

    #[test]
    fn handshake_with_several_tokens_needs_every_one_to_match() {
        let many = format!("/?token=guess-0000000000001&token={TOKEN}");
        assert_eq!(
            authorize_handshake(Some(&auth()), &request(&many, &[])),
            Err(WsRejection::TokenMissingOrInvalid)
        );
        let bearer = bearer();
        let both = format!("/?token={TOKEN}");
        assert_eq!(
            authorize_handshake(
                Some(&auth()),
                &request(&both, &[("Authorization", bearer.as_str())])
            ),
            Ok(())
        );
    }

    #[test]
    fn server_without_auth_refuses_every_handshake() {
        let bearer = bearer();
        let req = request("/", &[("Authorization", bearer.as_str())]);
        assert_eq!(
            authorize_handshake(None, &req),
            Err(WsRejection::TokenMissingOrInvalid)
        );
    }

    #[test]
    fn empty_configured_token_never_matches() {
        let empty = WsAuth::new("", Vec::<String>::new());
        assert_eq!(
            authorize_handshake(Some(&empty), &request("/?token=", &[])),
            Err(WsRejection::TokenMissingOrInvalid)
        );
        assert_eq!(
            authorize_handshake(Some(&empty), &request("/", &[("Authorization", "Bearer ")])),
            Err(WsRejection::TokenMissingOrInvalid)
        );
    }

    #[test]
    fn token_eq_compares_whole_values() {
        assert!(token_eq(TOKEN, TOKEN));
        assert!(!token_eq("0123456789abcdef0124", TOKEN));
        assert!(!token_eq("0123456789", TOKEN));
        assert!(!token_eq("0123456789abcdef01234", TOKEN));
        assert!(!token_eq("", ""));
    }

    #[test]
    fn ws_auth_debug_redacts_the_token() {
        let debug = format!("{:?}", auth());
        assert!(debug.contains("http://localhost:3000"), "{debug}");
        assert!(!debug.contains(TOKEN), "{debug}");
    }

    #[test]
    fn rejection_responses_are_403_or_401_with_www_authenticate() {
        let forbidden = WsRejection::OriginNotAllowed.error_response();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert!(forbidden.headers().get(header::WWW_AUTHENTICATE).is_none());
        assert!(forbidden.body().is_none());

        let unauthorized = WsRejection::TokenMissingOrInvalid.error_response();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .unwrap(),
            "Bearer"
        );
        assert!(unauthorized.body().is_none());
    }

    #[test]
    fn rejection_log_origin_never_reads_the_uri_or_authorization() {
        let bearer = bearer();
        let uri = format!("/?token={TOKEN}");
        let req = request(
            &uri,
            &[
                ("Origin", "https://evil.example"),
                ("Authorization", bearer.as_str()),
            ],
        );
        assert_eq!(
            origin_for_log(&req).as_deref(),
            Some("https://evil.example")
        );
        assert_eq!(origin_for_log(&request(&uri, &[])), None);
    }

    #[test]
    fn test_ws_client_message_parse() {
        let json = r#"{"type":"mycall","callsign":"VK2ABC"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::MyCall { callsign } => assert_eq!(callsign, "VK2ABC"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_ws_server_message_format() {
        let msg = WsServerMessage::Status {
            connected: true,
            remote_call: Some("VK3DEF".to_string()),
            snr: Some(15),
            level: Some(4),
            cfo: Some(12.5),
            short_cp_ok: Some(true),
            cp_desync_episodes: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("connected"));
        assert!(json.contains("VK3DEF"));
        assert!(json.contains("\"level\":4"));
        assert!(json.contains("\"cfo\":12.5"));
    }

    #[test]
    fn test_ws_spectrum_client_message_parse() {
        let json = r#"{"type":"spectrum","enabled":true}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Spectrum { enabled } => assert!(enabled),
            _ => panic!("Wrong variant"),
        }

        let json_off = r#"{"type":"spectrum","enabled":false}"#;
        let msg: WsClientMessage = serde_json::from_str(json_off).unwrap();
        match msg {
            WsClientMessage::Spectrum { enabled } => assert!(!enabled),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_ws_spectrum_server_message_format() {
        let msg = WsServerMessage::Spectrum {
            bins: vec![-80.0; 128],
            timestamp_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"spectrum""#));
        assert!(json.contains("\"timestamp_ms\":1700000000000"));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["bins"].as_array().unwrap().len(), 128);
    }

    #[test]
    fn test_opt_in_stream_classifies_spectrum_and_audio_level_only() {
        let spectrum_json = serde_json::to_string(&WsServerMessage::Spectrum {
            bins: vec![0.0; 128],
            timestamp_ms: 0,
        })
        .unwrap();
        assert_eq!(opt_in_stream(&spectrum_json), Some(OptInStream::Spectrum));

        let audio_level_json = serde_json::to_string(&WsServerMessage::AudioLevel {
            direction: AudioLevelDirection::Rx,
            peak_dbfs: -12.1,
            rms_dbfs: -31.4,
            clip_fraction: 0.0,
            state: AudioLevelState::Ok,
            duration_ms: 100,
            timestamp_ms: 0,
        })
        .unwrap();
        assert_eq!(
            opt_in_stream(&audio_level_json),
            Some(OptInStream::AudioLevel)
        );

        let data_json = serde_json::to_string(&WsServerMessage::Data {
            data: "hello".to_string(),
        })
        .unwrap();
        assert_eq!(opt_in_stream(&data_json), None);

        let status_json = serde_json::to_string(&WsServerMessage::Status {
            connected: false,
            remote_call: None,
            snr: None,
            level: None,
            cfo: None,
            short_cp_ok: None,
            cp_desync_episodes: 0,
        })
        .unwrap();
        assert_eq!(opt_in_stream(&status_json), None);

        assert_eq!(opt_in_stream("not valid json"), None);
    }

    #[test]
    fn test_ws_audio_level_client_message_parse() {
        let msg: WsClientMessage =
            serde_json::from_str(r#"{"type":"audio_level","enabled":true}"#).unwrap();
        match msg {
            WsClientMessage::AudioLevel { enabled } => assert!(enabled),
            other => panic!("Wrong variant: {:?}", other),
        }

        let msg: WsClientMessage =
            serde_json::from_str(r#"{"type":"audio_level","enabled":false}"#).unwrap();
        match msg {
            WsClientMessage::AudioLevel { enabled } => assert!(!enabled),
            other => panic!("Wrong variant: {:?}", other),
        }
    }

    #[test]
    fn test_ws_audio_level_server_message_format() {
        let msg = WsServerMessage::AudioLevel {
            direction: AudioLevelDirection::Rx,
            peak_dbfs: -12.1,
            rms_dbfs: -31.4,
            clip_fraction: 0.0,
            state: AudioLevelState::Clipping,
            duration_ms: 100,
            timestamp_ms: 7,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = parsed.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "clip_fraction",
                "direction",
                "duration_ms",
                "peak_dbfs",
                "rms_dbfs",
                "state",
                "timestamp_ms",
                "type",
            ]
        );
        assert_eq!(parsed["type"], "audio_level");
        assert_eq!(parsed["direction"], "rx");
        assert_eq!(parsed["state"], "clipping");
        assert_eq!(parsed["duration_ms"], 100);
        assert_eq!(parsed["timestamp_ms"], 7);

        match serde_json::from_str::<WsServerMessage>(&json).unwrap() {
            WsServerMessage::AudioLevel {
                direction,
                peak_dbfs,
                rms_dbfs,
                clip_fraction,
                state,
                duration_ms,
                timestamp_ms,
            } => {
                assert_eq!(direction, AudioLevelDirection::Rx);
                assert_eq!(peak_dbfs, -12.1);
                assert_eq!(rms_dbfs, -31.4);
                assert_eq!(clip_fraction, 0.0);
                assert_eq!(state, AudioLevelState::Clipping);
                assert_eq!(duration_ms, 100);
                assert_eq!(timestamp_ms, 7);
            }
            other => panic!("expected AudioLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_audio_level_enums_serialize_lowercase() {
        assert_eq!(
            serde_json::to_string(&AudioLevelDirection::Rx).unwrap(),
            r#""rx""#
        );
        assert_eq!(
            serde_json::to_string(&AudioLevelDirection::Tx).unwrap(),
            r#""tx""#
        );
        assert_eq!(
            serde_json::to_string(&AudioLevelState::Ok).unwrap(),
            r#""ok""#
        );
        assert_eq!(
            serde_json::to_string(&AudioLevelState::Clipping).unwrap(),
            r#""clipping""#
        );
        assert_eq!(
            serde_json::to_string(&AudioLevelState::Underdrive).unwrap(),
            r#""underdrive""#
        );
    }

    #[test]
    fn test_ws_connect_message() {
        let json = r#"{"type":"connect","source":"VK2ABC","destination":"VK3DEF"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Connect {
                source,
                destination,
            } => {
                assert_eq!(source, "VK2ABC");
                assert_eq!(destination, "VK3DEF");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_ws_server_default() {
        let server = WebSocketServer::default_port();
        assert_eq!(server.port(), 8400);
    }

    /// COP-28: the WebSocket ID range must sit well above any realistic
    /// command-port ID (which counts from 1) and below the data-port range
    /// (`0x8000_0001`), so an ID-keyed VARA reply can never land on a
    /// WebSocket client's number or vice versa.
    #[test]
    fn first_client_id_is_outside_the_vara_id_ranges() {
        const {
            assert!(FIRST_CLIENT_ID > 0xFFFF); // above any u16-sized id
            assert!(FIRST_CLIENT_ID < 0x8000_0001);
        }
    }

    #[test]
    fn test_ws_error_message() {
        let msg = WsServerMessage::Error {
            message: "Not connected".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Not connected"));
    }

    #[test]
    fn test_ws_send_message_parse() {
        let json = r#"{"type":"send","data":"Hello World"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Send { data } => assert_eq!(data, "Hello World"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_ws_disconnect_message_parse() {
        let json = r#"{"type":"disconnect"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMessage::Disconnect));
    }

    #[test]
    fn test_ws_status_message_parse() {
        let json = r#"{"type":"status"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMessage::Status));
    }

    #[test]
    fn test_ws_server_event_rx() {
        let mut server = WebSocketServer::new(9400);
        assert!(server.take_event_rx().is_some());
        assert!(server.take_event_rx().is_none());
    }

    #[test]
    fn test_ws_connected_message_format() {
        let msg = WsServerMessage::Connected {
            remote_call: "VK3DEF".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("VK3DEF"));
        assert!(json.contains("connected"));
    }

    #[test]
    fn test_ws_disconnected_message_format() {
        let msg = WsServerMessage::Disconnected;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("disconnected"));
    }

    #[test]
    fn test_ws_data_message_format() {
        let msg = WsServerMessage::Data {
            data: "test payload".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("test payload"));
    }

    #[test]
    fn test_ws_status_omits_null_fields() {
        let msg = WsServerMessage::Status {
            connected: false,
            remote_call: None,
            snr: None,
            level: None,
            cfo: None,
            short_cp_ok: None,
            cp_desync_episodes: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("remote_call"));
        assert!(!json.contains("snr"));
        assert!(!json.contains("level"));
        assert!(!json.contains("cfo"));
        assert!(!json.contains("short_cp_ok"));
        // COP-2: the deliberate exception. `cp_desync_episodes` is NOT
        // `skip_serializing_if`, because to a client an absent field and "zero
        // episodes" are indistinguishable -- and the reason this field exists is that
        // a desynced daemon used to look healthy through every operator-facing
        // channel. A healthy `0` must be positively reported, not inferred.
        assert!(
            json.contains(r#""cp_desync_episodes":0"#),
            "cp_desync_episodes must be serialized even at its healthy value: {json}"
        );
    }

    /// COP-2 review finding: a client built against this version must still be able
    /// to deserialize a `status` message from an OLDER daemon (or a persisted
    /// pre-COP-2 message) that predates `cp_desync_episodes` and so never included
    /// it. Without `#[serde(default)]` this fails outright with a missing-field
    /// error instead of defaulting to the healthy `0`.
    #[test]
    fn test_ws_status_deserializes_without_cp_desync_episodes_field() {
        let json = r#"{"type":"status","connected":false}"#;
        let msg: WsServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsServerMessage::Status {
                cp_desync_episodes, ..
            } => assert_eq!(cp_desync_episodes, 0),
            other => panic!("expected Status, got {other:?}"),
        }
    }
}

/// Integration tests that require the `websocket` feature (tokio-tungstenite).
#[cfg(test)]
#[cfg(feature = "websocket")]
mod integration_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    const TOKEN: &str = "0123456789abcdef0123";

    fn test_auth() -> WsAuth {
        WsAuth::new(TOKEN, ["http://localhost:3000"])
    }

    /// A client handshake request for `url` with an optional `Origin` and an
    /// optional `Authorization: Bearer` token (COP-53).
    fn client_request(
        url: &str,
        origin: Option<&str>,
        bearer: Option<&str>,
    ) -> tokio_tungstenite::tungstenite::handshake::client::Request {
        let mut request = url.into_client_request().unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(token) = bearer {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        request
    }

    /// Start a real server on an ephemeral listener; return its address, its
    /// event receiver and its broadcast sender.
    async fn start_real_server(
        auth: Option<WsAuth>,
    ) -> (
        std::net::SocketAddr,
        mpsc::Receiver<HostEvent>,
        tokio::sync::broadcast::Sender<String>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut server = WebSocketServer::with_bind_addr(0, "127.0.0.1".to_string());
        if let Some(auth) = auth {
            server = server.with_auth(auth);
        }
        let event_rx = server.take_event_rx().expect("fresh server");
        let broadcast = server.broadcast_sender();
        tokio::spawn(async move {
            let _ = server.run_with_listener(listener).await;
        });
        (addr, event_rx, broadcast)
    }

    /// Expect the handshake to fail with HTTP `status`; return the response.
    async fn expect_refused(
        request: tokio_tungstenite::tungstenite::handshake::client::Request,
        status: u16,
    ) -> tokio_tungstenite::tungstenite::handshake::client::Response {
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status().as_u16(), status, "{resp:?}");
                *resp
            }
            Err(other) => panic!("expected HTTP {status}, got error {other:?}"),
            Ok(_) => panic!("expected HTTP {status}, but the handshake succeeded"),
        }
    }

    async fn expect_no_event(event_rx: &mut mpsc::Receiver<HostEvent>) {
        if let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_millis(300), event_rx.recv()).await
        {
            panic!("a refused handshake must emit no event, got {event:?}");
        }
    }

    async fn expect_connected(event_rx: &mut mpsc::Receiver<HostEvent>) -> u32 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timed out waiting for the Connected event")
            .expect("event channel closed");
        match event {
            HostEvent::Connected { client_id } => client_id,
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    /// COP-53, the ticket's scenario: a browser page from an untrusted Origin
    /// with no token is refused at the HTTP upgrade, before any command is
    /// processed -- no `Connected` event, no broadcast subscription.
    #[tokio::test]
    async fn real_server_rejects_cross_origin_handshake_before_any_event() {
        let (addr, mut event_rx, broadcast) = start_real_server(Some(test_auth())).await;
        let url = format!("ws://{addr}/");
        expect_refused(
            client_request(&url, Some("https://evil.example"), None),
            403,
        )
        .await;
        expect_no_event(&mut event_rx).await;
        assert_eq!(broadcast.receiver_count(), 0);
    }

    #[tokio::test]
    async fn real_server_rejects_missing_token_with_401_and_no_event() {
        let (addr, mut event_rx, broadcast) = start_real_server(Some(test_auth())).await;
        let url = format!("ws://{addr}/");
        let resp = expect_refused(
            client_request(&url, Some("http://localhost:3000"), None),
            401,
        )
        .await;
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
        expect_no_event(&mut event_rx).await;
        assert_eq!(broadcast.receiver_count(), 0);
    }

    #[tokio::test]
    async fn real_server_accepts_allowed_origin_with_query_token() {
        let (addr, mut event_rx, _broadcast) = start_real_server(Some(test_auth())).await;
        let url = format!("ws://{addr}/?token={TOKEN}");
        let (mut ws, _) = tokio_tungstenite::connect_async(client_request(
            &url,
            Some("http://localhost:3000"),
            None,
        ))
        .await
        .unwrap();
        expect_connected(&mut event_rx).await;

        ws.send(Message::Text(r#"{"type":"status"}"#.into()))
            .await
            .unwrap();
        let resp = ws.next().await.unwrap().unwrap();
        let msg: WsServerMessage = serde_json::from_str(resp.to_text().unwrap()).unwrap();
        assert!(matches!(msg, WsServerMessage::Status { .. }), "{msg:?}");

        ws.send(Message::Text(r#"{"type":"send","data":"Hello"}"#.into()))
            .await
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            HostEvent::DataReceived { data, .. } => assert_eq!(data, b"Hello"),
            other => panic!("expected DataReceived, got {other:?}"),
        }
        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn real_server_accepts_bearer_token_without_origin() {
        let (addr, mut event_rx, _broadcast) = start_real_server(Some(test_auth())).await;
        let url = format!("ws://{addr}/");
        let (ws, _) = tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
            .await
            .unwrap();
        expect_connected(&mut event_rx).await;
        drop(ws);
    }

    #[tokio::test]
    async fn real_server_without_auth_refuses_every_handshake() {
        let (addr, mut event_rx, broadcast) = start_real_server(None).await;
        let url = format!("ws://{addr}/");
        expect_refused(client_request(&url, None, Some(TOKEN)), 401).await;
        expect_no_event(&mut event_rx).await;
        assert_eq!(broadcast.receiver_count(), 0);
    }

    #[tokio::test]
    async fn rejected_handshakes_do_not_consume_client_ids() {
        let (addr, mut event_rx, _broadcast) = start_real_server(Some(test_auth())).await;
        let url = format!("ws://{addr}/");
        expect_refused(
            client_request(&url, Some("https://evil.example"), Some(TOKEN)),
            403,
        )
        .await;
        expect_refused(client_request(&url, None, None), 401).await;
        let (ws, _) = tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(expect_connected(&mut event_rx).await, FIRST_CLIENT_ID);
        drop(ws);
    }

    /// Helper: start a WebSocket server on a random port and return (port, event_rx).
    async fn start_server() -> (u16, mpsc::Receiver<HostEvent>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (event_tx, event_rx) = mpsc::channel(256);

        tokio::spawn(async move {
            use std::sync::atomic::{AtomicU32, Ordering};
            let next_id = AtomicU32::new(1);

            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let client_id = next_id.fetch_add(1, Ordering::Relaxed);
                let tx = event_tx.clone();

                tokio::spawn(async move {
                    let ws = match tokio_tungstenite::accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(_) => return,
                    };

                    let _ = tx.send(HostEvent::Connected { client_id }).await;

                    let (mut sink, mut stream) = ws.split();

                    while let Some(Ok(msg)) = stream.next().await {
                        if msg.is_close() {
                            break;
                        }
                        if msg.is_text() {
                            let text = msg.to_text().unwrap_or("");
                            match serde_json::from_str::<WsClientMessage>(text) {
                                Ok(client_msg) => {
                                    let host_event = match client_msg {
                                        WsClientMessage::Send { data } => HostEvent::DataReceived {
                                            client_id,
                                            data: data.into_bytes(),
                                        },
                                        WsClientMessage::Connect {
                                            source,
                                            destination,
                                        } => HostEvent::ConnectRequest {
                                            client_id,
                                            source,
                                            destination,
                                        },
                                        WsClientMessage::Disconnect => {
                                            HostEvent::DisconnectRequest { client_id }
                                        }
                                        WsClientMessage::Status => {
                                            let resp = WsServerMessage::Status {
                                                connected: false,
                                                remote_call: None,
                                                snr: None,
                                                level: None,
                                                cfo: None,
                                                short_cp_ok: None,
                                                cp_desync_episodes: 0,
                                            };
                                            if let Ok(json) = serde_json::to_string(&resp) {
                                                let _ = sink.send(Message::Text(json.into())).await;
                                            }
                                            continue;
                                        }
                                        WsClientMessage::MyCall { .. } => HostEvent::VaraCommand {
                                            client_id,
                                            command: text.to_string(),
                                        },
                                        WsClientMessage::Spectrum { .. }
                                        | WsClientMessage::AudioLevel { .. } => {
                                            // This test-only helper doesn't exercise
                                            // broadcast forwarding at all (see its own
                                            // doc), so there's no per-connection
                                            // opt-in state to update here.
                                            continue;
                                        }
                                    };
                                    let _ = tx.send(host_event).await;
                                }
                                Err(e) => {
                                    let resp = WsServerMessage::Error {
                                        message: format!("Invalid message: {}", e),
                                    };
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = sink.send(Message::Text(json.into())).await;
                                    }
                                }
                            }
                        }
                    }

                    let _ = tx.send(HostEvent::Disconnected { client_id }).await;
                });
            }
        });

        (port, event_rx)
    }

    #[tokio::test]
    async fn test_ws_integration_send_data() {
        let (port, mut event_rx) = start_server().await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        // Should get Connected event
        let event = event_rx.recv().await.unwrap();
        assert!(matches!(event, HostEvent::Connected { .. }));

        // Send data
        ws.send(Message::Text(r#"{"type":"send","data":"Hello"}"#.into()))
            .await
            .unwrap();

        let event = event_rx.recv().await.unwrap();
        match event {
            HostEvent::DataReceived { data, .. } => {
                assert_eq!(data, b"Hello");
            }
            other => panic!("Expected DataReceived, got {:?}", other),
        }

        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn test_ws_integration_status_response() {
        let (port, mut event_rx) = start_server().await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        // Consume Connected event
        let _ = event_rx.recv().await.unwrap();

        // Request status
        ws.send(Message::Text(r#"{"type":"status"}"#.into()))
            .await
            .unwrap();

        // Should get a JSON response back (not a HostEvent)
        let resp = ws.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let msg: WsServerMessage = serde_json::from_str(text).unwrap();
        assert!(matches!(
            msg,
            WsServerMessage::Status {
                connected: false,
                ..
            }
        ));

        ws.close(None).await.ok();
    }

    /// Task 7 required scenario: "WebSocket `status` carries real `snr`/`level`."
    /// Unlike `test_ws_integration_status_response` above (which exercises the
    /// hand-rolled `start_server()` test helper, a separate stand-in that predates
    /// this task and still hardcodes `connected: false`), this test runs the real
    /// `WebSocketServer::run()` and verifies its `status` reply reflects whatever
    /// the daemon last wrote into `WebSocketServer::status()` -- e.g. after a frame
    /// decodes with a known SNR/speed level/CFO.
    #[tokio::test]
    async fn test_ws_integration_real_server_status_carries_live_values() {
        // Bind on an ephemeral port so we can discover it before calling run().
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // free it up for the real server to bind

        let server =
            WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string()).with_auth(test_auth());
        let status = server.status();

        // Simulate what the daemon does after a frame decodes: write real values
        // into the shared status snapshot.
        {
            let mut snap = status.lock().await;
            snap.connected = true;
            snap.remote_call = Some("VK3DEF".to_string());
            snap.snr = Some(18);
            snap.level = Some(5);
            snap.cfo = Some(-3.5);
        }

        tokio::spawn(async move {
            let _ = server.run().await;
        });
        // Give the server a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
            .await
            .unwrap();

        ws.send(Message::Text(r#"{"type":"status"}"#.into()))
            .await
            .unwrap();

        let resp = ws.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let msg: WsServerMessage = serde_json::from_str(text).unwrap();
        match msg {
            WsServerMessage::Status {
                connected,
                remote_call,
                snr,
                level,
                cfo,
                short_cp_ok,
                cp_desync_episodes,
            } => {
                assert!(connected, "status should reflect the live connected flag");
                assert_eq!(remote_call.as_deref(), Some("VK3DEF"));
                assert_eq!(snr, Some(18), "status should carry the real SNR");
                assert_eq!(level, Some(5), "status should carry the real speed level");
                assert_eq!(cfo, Some(-3.5), "status should carry the real CFO estimate");
                assert_eq!(
                    short_cp_ok, None,
                    "short_cp_ok should stay None when cp_gate isn't fed"
                );
                assert_eq!(
                    cp_desync_episodes, 0,
                    "a station that never desynced must report 0, and must report it \
                     over the wire rather than by omission"
                );
            }
            other => panic!("Expected Status, got {:?}", other),
        }

        ws.close(None).await.ok();
    }

    /// COP-32: `run_with_listener` serves the listener its caller bound, so
    /// coppad can bind (and fail) before it reports itself ready.
    #[tokio::test]
    async fn run_with_listener_serves_the_callers_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // The configured port is ignored on this path.
        let mut server =
            WebSocketServer::with_bind_addr(0, "127.0.0.1".to_string()).with_auth(test_auth());
        let mut event_rx = server.take_event_rx().expect("fresh server");
        tokio::spawn(async move {
            let _ = server.run_with_listener(listener).await;
        });

        let url = format!("ws://{addr}");
        let (ws, _) = tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
            .await
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timed out waiting for the Connected event")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Connected { .. }), "{event:?}");
        drop(ws);
    }

    /// COP-28: the real `run()` numbers WebSocket clients from
    /// `FIRST_CLIENT_ID` (`0x4000_0001`), outside both the VARA command-port
    /// range (from 1) and the data-port range (from `0x8000_0001`), so an
    /// ID-keyed VARA reply can never be addressed by a WebSocket client's ID.
    #[tokio::test]
    async fn real_server_numbers_clients_from_first_client_id() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut server =
            WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string()).with_auth(test_auth());
        let mut event_rx = server.take_event_rx().expect("fresh server");

        tokio::spawn(async move {
            let _ = server.run().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (ws, _) = tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
            .await
            .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timed out waiting for the first client's Connected event")
            .expect("event channel closed");
        assert!(matches!(
            event,
            HostEvent::Connected {
                client_id: FIRST_CLIENT_ID
            }
        ));

        drop(ws);
    }

    #[tokio::test]
    async fn test_ws_integration_invalid_json() {
        let (port, mut event_rx) = start_server().await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let _ = event_rx.recv().await.unwrap();

        // Send garbage JSON
        ws.send(Message::Text("not valid json".into()))
            .await
            .unwrap();

        // Should get an error response
        let resp = ws.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let msg: WsServerMessage = serde_json::from_str(text).unwrap();
        assert!(matches!(msg, WsServerMessage::Error { .. }));

        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn test_ws_integration_disconnect_event() {
        let (port, mut event_rx) = start_server().await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        // Connected
        let event = event_rx.recv().await.unwrap();
        assert!(matches!(event, HostEvent::Connected { .. }));

        // Drop the connection
        drop(ws);

        // Should get Disconnected
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, HostEvent::Disconnected { .. }));
    }

    /// Phase 4 Task 4 required scenario: "WS client subscribing gets spectrum
    /// frames" -- AND the flip side, which is the actual point of making this
    /// opt-in: a client that never sends `{"type":"spectrum","enabled":true}`
    /// must NOT receive `spectrum` broadcasts, even though every other
    /// broadcast message type (e.g. `data`) reaches every connection
    /// unconditionally. Runs the real `WebSocketServer::run()` (not the
    /// hand-rolled `start_server()` helper, which doesn't touch the broadcast
    /// channel at all).
    #[tokio::test]
    async fn test_ws_spectrum_is_opt_in_per_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let server =
            WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string()).with_auth(test_auth());
        let broadcast_tx = server.broadcast_sender();

        tokio::spawn(async move {
            let _ = server.run().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);

        // Client A subscribes to spectrum.
        let (mut ws_a, _) =
            tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
                .await
                .unwrap();
        ws_a.send(Message::Text(
            r#"{"type":"spectrum","enabled":true}"#.into(),
        ))
        .await
        .unwrap();

        // Client B never opts in.
        let (mut ws_b, _) =
            tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
                .await
                .unwrap();

        // Give both connections' select loops a moment to register the
        // subscribe message (Client A) before broadcasting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let spectrum_msg = WsServerMessage::Spectrum {
            bins: vec![-90.0; 128],
            timestamp_ms: 12345,
        };
        let spectrum_json = serde_json::to_string(&spectrum_msg).unwrap();
        broadcast_tx.send(spectrum_json.clone()).unwrap();

        // Client A (subscribed) must receive it.
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws_a.next())
            .await
            .expect("subscribed client should receive the spectrum broadcast")
            .unwrap()
            .unwrap();
        let received: WsServerMessage = serde_json::from_str(resp.to_text().unwrap()).unwrap();
        match received {
            WsServerMessage::Spectrum { bins, timestamp_ms } => {
                assert_eq!(bins.len(), 128);
                assert_eq!(timestamp_ms, 12345);
            }
            other => panic!("expected Spectrum, got {:?}", other),
        }

        // Client B (not subscribed) must NOT receive the spectrum message.
        // Prove it not merely by a timeout (which could also mean "arriving
        // late") but by following up with a distinguishable broadcast Client
        // B unconditionally SHOULD receive (a `data` message, gated on
        // nothing) and confirming THAT is the first thing Client B sees.
        let canary = WsServerMessage::Data {
            data: "canary".to_string(),
        };
        broadcast_tx
            .send(serde_json::to_string(&canary).unwrap())
            .unwrap();

        let resp_b = tokio::time::timeout(std::time::Duration::from_secs(2), ws_b.next())
            .await
            .expect("client B should still receive the unconditional canary broadcast")
            .unwrap()
            .unwrap();
        let received_b: WsServerMessage = serde_json::from_str(resp_b.to_text().unwrap()).unwrap();
        match received_b {
            WsServerMessage::Data { data } => {
                assert_eq!(
                    data, "canary",
                    "non-subscribed client's first received message must be the canary, \
                     not the filtered-out spectrum broadcast"
                );
            }
            other => panic!("expected the canary Data message, got {:?}", other),
        }

        ws_a.close(None).await.ok();
        ws_b.close(None).await.ok();
    }

    /// Receive one text message within 2 s and parse it as a `WsServerMessage`.
    async fn next_server_message<S>(ws: &mut S, who: &str) -> WsServerMessage
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap_or_else(|_| panic!("{who} should receive a message"))
            .unwrap()
            .unwrap();
        serde_json::from_str(resp.to_text().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn test_ws_audio_level_and_spectrum_opt_ins_are_independent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let server =
            WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string()).with_auth(test_auth());
        let broadcast_tx = server.broadcast_sender();

        tokio::spawn(async move {
            let _ = server.run().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);

        // Client A opts into audio_level only; client B into spectrum only.
        let (mut ws_a, _) =
            tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
                .await
                .unwrap();
        ws_a.send(Message::Text(
            r#"{"type":"audio_level","enabled":true}"#.into(),
        ))
        .await
        .unwrap();
        let (mut ws_b, _) =
            tokio_tungstenite::connect_async(client_request(&url, None, Some(TOKEN)))
                .await
                .unwrap();
        ws_b.send(Message::Text(
            r#"{"type":"spectrum","enabled":true}"#.into(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let audio_level_json = serde_json::to_string(&WsServerMessage::AudioLevel {
            direction: AudioLevelDirection::Rx,
            peak_dbfs: -12.1,
            rms_dbfs: -31.4,
            clip_fraction: 0.0,
            state: AudioLevelState::Ok,
            duration_ms: 100,
            timestamp_ms: 1,
        })
        .unwrap();
        let spectrum_json = serde_json::to_string(&WsServerMessage::Spectrum {
            bins: vec![-90.0; 128],
            timestamp_ms: 2,
        })
        .unwrap();
        let canary = |data: &str| {
            serde_json::to_string(&WsServerMessage::Data {
                data: data.to_string(),
            })
            .unwrap()
        };

        broadcast_tx.send(audio_level_json.clone()).unwrap();
        broadcast_tx.send(spectrum_json).unwrap();
        broadcast_tx.send(canary("canary")).unwrap();

        // A: audio_level, then the canary -- never the spectrum.
        match next_server_message(&mut ws_a, "client A").await {
            WsServerMessage::AudioLevel { timestamp_ms, .. } => assert_eq!(timestamp_ms, 1),
            other => panic!("client A expected AudioLevel, got {:?}", other),
        }
        match next_server_message(&mut ws_a, "client A").await {
            WsServerMessage::Data { data } => assert_eq!(data, "canary"),
            other => panic!("client A expected the canary, got {:?}", other),
        }

        // B: spectrum, then the canary -- never the audio_level.
        match next_server_message(&mut ws_b, "client B").await {
            WsServerMessage::Spectrum { timestamp_ms, .. } => assert_eq!(timestamp_ms, 2),
            other => panic!("client B expected Spectrum, got {:?}", other),
        }
        match next_server_message(&mut ws_b, "client B").await {
            WsServerMessage::Data { data } => assert_eq!(data, "canary"),
            other => panic!("client B expected the canary, got {:?}", other),
        }

        // A opts back out: the next audio_level is dropped for it.
        ws_a.send(Message::Text(
            r#"{"type":"audio_level","enabled":false}"#.into(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        broadcast_tx.send(audio_level_json).unwrap();
        broadcast_tx.send(canary("canary2")).unwrap();
        match next_server_message(&mut ws_a, "client A").await {
            WsServerMessage::Data { data } => assert_eq!(data, "canary2"),
            other => panic!("opted-out client A expected canary2, got {:?}", other),
        }

        ws_a.close(None).await.ok();
        ws_b.close(None).await.ok();
    }
}
