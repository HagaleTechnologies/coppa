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
}

/// Maximum number of concurrent WebSocket client connections.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Maximum size (bytes) of a complete WebSocket message we will buffer.
const MAX_WS_MESSAGE_SIZE: usize = 1 << 20; // 1 MiB

/// Maximum size (bytes) of a single WebSocket frame.
const MAX_WS_FRAME_SIZE: usize = 256 * 1024;

/// WebSocket server configuration.
pub struct WebSocketServer {
    port: u16,
    bind_addr: String,
    event_tx: mpsc::Sender<HostEvent>,
    event_rx: Option<mpsc::Receiver<HostEvent>>,
    /// Broadcast channel for pushing messages from the engine to all connected WS clients.
    broadcast_tx: tokio::sync::broadcast::Sender<String>,
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
        let ws_config = WebSocketConfig {
            max_message_size: Some(MAX_WS_MESSAGE_SIZE),
            max_frame_size: Some(MAX_WS_FRAME_SIZE),
            ..Default::default()
        };

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

                // Main select loop: handle both client messages and engine broadcasts
                loop {
                    tokio::select! {
                        // Engine broadcast → client
                        result = broadcast_rx.recv() => {
                            match result {
                                Ok(json) => {
                                    if sink.send(tokio_tungstenite::tungstenite::Message::Text(json)).await.is_err() {
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
                                                let resp = WsServerMessage::Status {
                                                    connected: false,
                                                    remote_call: None,
                                                    snr: None,
                                                };
                                                if let Ok(json) = serde_json::to_string(&resp) {
                                                    let _ = sink
                                                        .send(tokio_tungstenite::tungstenite::Message::Text(json))
                                                        .await;
                                                }
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
                                                .send(tokio_tungstenite::tungstenite::Message::Text(json))
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("connected"));
        assert!(json.contains("VK3DEF"));
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("remote_call"));
        assert!(!json.contains("snr"));
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
                                            };
                                            if let Ok(json) = serde_json::to_string(&resp) {
                                                let _ = sink.send(Message::Text(json)).await;
                                            }
                                            continue;
                                        }
                                        WsClientMessage::MyCall { .. } => HostEvent::VaraCommand {
                                            client_id,
                                            command: text.to_string(),
                                        },
                                    };
                                    let _ = tx.send(host_event).await;
                                }
                                Err(e) => {
                                    let resp = WsServerMessage::Error {
                                        message: format!("Invalid message: {}", e),
                                    };
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = sink.send(Message::Text(json)).await;
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
        ws.send(Message::Text(
            r#"{"type":"send","data":"Hello"}"#.to_string(),
        ))
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
        ws.send(Message::Text(r#"{"type":"status"}"#.to_string()))
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

    #[tokio::test]
    async fn test_ws_integration_invalid_json() {
        let (port, mut event_rx) = start_server().await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let _ = event_rx.recv().await.unwrap();

        // Send garbage JSON
        ws.send(Message::Text("not valid json".to_string()))
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
}
