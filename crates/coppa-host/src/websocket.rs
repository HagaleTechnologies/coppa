//! WebSocket JSON API for web-based clients.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::HostEvent;

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
        /// Current speed level (1-10) of the last decoded frame (decision 8).
        #[serde(skip_serializing_if = "Option::is_none")]
        level: Option<u8>,
        /// Carrier frequency offset (Hz) estimated on the last decoded frame
        /// (decision 8).
        #[serde(skip_serializing_if = "Option::is_none")]
        cfo: Option<f32>,
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
}

/// Maximum number of concurrent WebSocket client connections.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

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
}

/// Whether a pre-serialized broadcast JSON string is a `spectrum` message
/// (`WsServerMessage::Spectrum`), used by each connection's forwarding loop
/// in [`WebSocketServer::run`] to gate delivery on that connection's own
/// opt-in state.
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
/// events, plus at most a 4 Hz spectrum stream, across up to
/// `MAX_CONCURRENT_CONNECTIONS` clients) far too low for the parse cost to
/// matter.
fn is_spectrum_broadcast(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("type")?.as_str().map(|s| s == "spectrum"))
        .unwrap_or(false)
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
        }
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
    /// Each connection gets a unique client ID. Incoming JSON messages are
    /// parsed as `WsClientMessage` and translated to `HostEvent`s.
    /// Engine-originated messages are broadcast to all connected clients
    /// via the `broadcast_sender()` channel.
    #[cfg(feature = "websocket")]
    pub async fn run(&self) -> anyhow::Result<()> {
        use futures_util::{SinkExt, StreamExt};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use tokio::net::TcpListener;
        use tokio::sync::Semaphore;
        use tokio_tungstenite::accept_async_with_config;
        use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = TcpListener::bind(&addr).await?;
        println!("WebSocket server listening on ws://{}", addr);

        // Bound per-connection memory: cap WebSocket message/frame sizes.
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WS_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_WS_FRAME_SIZE));

        let next_id = AtomicU32::new(1);
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

        loop {
            let (stream, peer) = listener.accept().await?;
            // Cap concurrent connections; drop the client if at capacity.
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    eprintln!(
                        "WebSocket connection limit reached, dropping client {}",
                        peer
                    );
                    drop(stream);
                    continue;
                }
            };
            let client_id = next_id.fetch_add(1, Ordering::Relaxed);
            let event_tx = self.event_tx.clone();
            let mut broadcast_rx = self.broadcast_tx.subscribe();
            let status = self.status.clone();

            println!("WebSocket client {} connected from {}", client_id, peer);

            tokio::spawn(async move {
                let _permit = permit;
                let ws = match accept_async_with_config(stream, Some(ws_config)).await {
                    Ok(ws) => ws,
                    Err(e) => {
                        eprintln!("WebSocket handshake failed for {}: {}", peer, e);
                        return;
                    }
                };

                let _ = event_tx.send(HostEvent::Connected { client_id }).await;

                let (mut sink, mut ws_stream) = ws.split();

                // Per-connection opt-in state for the `spectrum` broadcast stream
                // (Phase 4 Task 4) -- see `WsServerMessage::Spectrum`'s doc for why
                // this defaults to off and must be explicitly requested.
                let mut spectrum_enabled = false;

                // Main select loop: handle both client messages and engine broadcasts
                loop {
                    tokio::select! {
                        // Engine broadcast → client
                        result = broadcast_rx.recv() => {
                            match result {
                                Ok(json) => {
                                    if is_spectrum_broadcast(&json) && !spectrum_enabled {
                                        // This client hasn't opted in; every other
                                        // broadcast message type is delivered
                                        // unconditionally, matching this channel's
                                        // pre-existing behavior.
                                        continue;
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
                println!("WebSocket client {} disconnected", client_id);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_is_spectrum_broadcast_identifies_spectrum_messages_only() {
        let spectrum_json = serde_json::to_string(&WsServerMessage::Spectrum {
            bins: vec![0.0; 128],
            timestamp_ms: 0,
        })
        .unwrap();
        assert!(is_spectrum_broadcast(&spectrum_json));

        let data_json = serde_json::to_string(&WsServerMessage::Data {
            data: "hello".to_string(),
        })
        .unwrap();
        assert!(!is_spectrum_broadcast(&data_json));

        let status_json = serde_json::to_string(&WsServerMessage::Status {
            connected: false,
            remote_call: None,
            snr: None,
            level: None,
            cfo: None,
        })
        .unwrap();
        assert!(!is_spectrum_broadcast(&status_json));

        assert!(!is_spectrum_broadcast("not valid json"));
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("remote_call"));
        assert!(!json.contains("snr"));
        assert!(!json.contains("level"));
        assert!(!json.contains("cfo"));
    }
}

/// Integration tests that require the `websocket` feature (tokio-tungstenite).
#[cfg(test)]
#[cfg(feature = "websocket")]
mod integration_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

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
                                        WsClientMessage::Spectrum { .. } => {
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

        let server = WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string());
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
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

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
            } => {
                assert!(connected, "status should reflect the live connected flag");
                assert_eq!(remote_call.as_deref(), Some("VK3DEF"));
                assert_eq!(snr, Some(18), "status should carry the real SNR");
                assert_eq!(level, Some(5), "status should carry the real speed level");
                assert_eq!(cfo, Some(-3.5), "status should carry the real CFO estimate");
            }
            other => panic!("Expected Status, got {:?}", other),
        }

        ws.close(None).await.ok();
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

        let server = WebSocketServer::with_bind_addr(port, "127.0.0.1".to_string());
        let broadcast_tx = server.broadcast_sender();

        tokio::spawn(async move {
            let _ = server.run().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);

        // Client A subscribes to spectrum.
        let (mut ws_a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        ws_a.send(Message::Text(
            r#"{"type":"spectrum","enabled":true}"#.into(),
        ))
        .await
        .unwrap();

        // Client B never opts in.
        let (mut ws_b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

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
}
